//! ネイティブ YouTube 解決器（yt-dlp 置き換え）。
//!
//! 旧実装は `yt-dlp.exe` を毎回起動していた（onefile 起動 ~3秒）。本実装はアプリ内に常駐する
//! ワーカースレッドで InnerTube を直叩きし、URL を渡すだけで再生用ストリームを返す。
//!
//! 経路（PoC U1/U5/U7 で確定）:
//!   - 匿名 VOD     : ANDROID_VR（署名/nsig 不要・最大 2160p adaptive）
//!   - 匿名 ライブ  : ANDROID（hlsManifestUrl を mpv に直渡し）
//!   - ログイン members/年齢制限: TVHTML5 + OAuth Bearer で解錠 → URL の n を nsig 変換（boa）
//!   - YouTube 以外: 解決せず素通し
//!
//! ワーカーは long-lived（HTTP クライアント・nsig エンジン＝base.js キャッシュを保持）。boa の
//! `Context` は `!Send` のため、解決ごとにスレッドを起こすのではなくワーカー1本に集約する。

mod clients;
mod nsig;

use anyhow::{anyhow, bail, Result};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration as StdDuration;

use crate::types::{Codec, Quality};
use crate::Waker;

/// 解決済みのストリーム情報。
#[derive(Clone, Debug)]
pub struct Resolved {
    /// 映像ストリーム URL もしくは EDL 文字列。
    pub video_url: String,
    /// 音声ストリーム URL（adaptive 分離時）。muxed/HLS なら None。
    pub audio_url: Option<String>,
}

/// 背景（ワーカー）からメインスレッドへの通知。型・送出順は旧実装と不変（呼び出し契約）。
pub enum ResolveUpdate {
    /// 再生用のストリーム URL が解決できた（できるだけ早く送る＝即再生開始）。
    Ready(Resolved),
    /// 予備の再生 URL（ローカル中継＝サイドカー）。`Ready`(native)と並列に用意され、
    /// native の再生が mpv で失敗（403/開けない）したとき即座に切り替えるために控えておく。
    Fallback(Resolved),
    /// タイトル・ライブ判定（再生開始後に後追いで届く。表示更新のみ）。
    Meta { title: Option<String>, is_live: bool },
    /// SABR 詰みのライブ配信を検知し、mpv では再生できないため WebView2 の公式 IFrame プレーヤーへ
    /// 経路を切り替える指示（issue #16 PR3）。判定材料は既存 resolve が持つ:
    /// TVHTML5 + Bearer で `status=OK && isLive` かつ `hlsManifestUrl` が返らないケース。
    UseWebview {
        video_id: String,
        title: Option<String>,
        is_live: bool,
    },
    Error(String),
}

/// 解決リクエスト（メイン → ワーカー）。`reply` はリクエストごとに呼び出し側が生成する
/// （再生セッションごとに rx を作るため。前セッション宛の遅延応答は破棄済み rx と一緒に
/// 構造的に死ぬ）。
pub struct ResolveRequest {
    pub url: String,
    pub quality: Quality,
    pub codec: Codec,
    /// ログイン中なら OAuth access_token（認証経路で TVHTML5 に Bearer 付与）。
    pub access_token: Option<String>,
    pub reply: Sender<ResolveUpdate>,
}

/// 常駐解決器へのハンドル。リクエストを送るだけ。
pub struct ResolverHandle {
    req_tx: Sender<ResolveRequest>,
}

impl ResolverHandle {
    /// 解決器ワーカーをアプリ起動時に 1 回だけ起動する（M15）。
    /// 結果は各リクエストに同梱された `reply` に流し、メインスレッドを起こすため `waker` を呼ぶ。
    pub fn spawn(waker: Waker) -> Self {
        let (req_tx, req_rx) = std::sync::mpsc::channel::<ResolveRequest>();
        std::thread::spawn(move || worker_loop(req_rx, waker));
        Self { req_tx }
    }

    /// 解決を依頼する。
    pub fn request(&self, req: ResolveRequest) {
        let _ = self.req_tx.send(req);
    }
}

/// 常駐ワーカー本体。HTTP クライアントと nsig エンジンを保持し続ける。
fn worker_loop(req_rx: Receiver<ResolveRequest>, waker: Waker) {
    // cookie_store: youtube.com 訪問で得る VISITOR_INFO1_LIVE 等を保持し、以後の player 要求に乗せる。
    let http = match reqwest::blocking::Client::builder()
        .timeout(StdDuration::from_secs(20))
        .cookie_store(true)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[resolve] HTTP クライアント生成失敗: {e}");
            return;
        }
    };
    let mut nsig = nsig::NsigSolver::new();
    // 訪問者セッション（visitorData）。起動後 1 回だけ確立してキャッシュ（常駐の肝・M15）。
    let mut visitor: Option<String> = None;
    // gated フォールバックで起動した rustypipe サイドカー（中継プロキシ）。次の解決時に停止する。
    let mut current_sidecar: Option<Child> = None;

    while let Ok(req) = req_rx.recv() {
        // 前回の中継プロキシ（サイドカー）を停止（同時に 1 本だけ生かす）。
        if let Some(mut c) = current_sidecar.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        if visitor.is_none() {
            visitor = clients::fetch_visitor_data(&http).ok();
        }

        if !is_youtube_url(&req.url) {
            // YouTube 以外は素通し（resolve_one が Ok を返す）。
            match resolve_one(&http, &mut nsig, &req, visitor.as_deref()) {
                Ok((resolved, title, is_live)) => {
                    let _ = req.reply.send(ResolveUpdate::Ready(resolved));
                    let _ = req.reply.send(ResolveUpdate::Meta { title, is_live });
                }
                Err(e) => {
                    let _ = req.reply.send(ResolveUpdate::Error(e.to_string()));
                }
            }
            waker();
            continue;
        }

        // 並列: 先にサイドカー（別プロセス）を起動して解決を始めさせ、その間に native を解決する。
        // native は速いが直 URL は googlevideo の制約で mpv 再生が失敗しうる。サイドカー（ローカル
        // 中継）はそれらを吸収する確実な予備として控え、再生失敗時に即切替する。
        // ユーザー設定（解像度/コーデック）は両経路で同一に従わせる。
        let sc = spawn_sidecar(&req.url, &req).ok();
        match resolve_one(&http, &mut nsig, &req, visitor.as_deref()) {
            Ok((resolved, title, is_live)) => {
                // native を即再生。サイドカー結果は Fallback として控える（再生失敗時の即切替用）。
                let _ = req.reply.send(ResolveUpdate::Ready(resolved));
                let _ = req.reply.send(ResolveUpdate::Meta { title, is_live });
                if let Some((child, stdout)) = sc {
                    if let Ok((child, fb, _t, _l)) = read_sidecar_ready(child, stdout) {
                        current_sidecar = Some(child);
                        let _ = req.reply.send(ResolveUpdate::Fallback(fb));
                    }
                }
            }
            // native 全滅（多くは匿名 bot ゲート＝gated）→ サイドカーを本命にする。
            Err(native_err) => match sc.map(|(c, o)| read_sidecar_ready(c, o)) {
                Some(Ok((child, resolved, title, is_live))) => {
                    current_sidecar = Some(child);
                    let _ = req.reply.send(ResolveUpdate::Ready(resolved));
                    let _ = req.reply.send(ResolveUpdate::Meta { title, is_live });
                }
                Some(Err(side_err)) => {
                    let _ = req.reply.send(ResolveUpdate::Error(format!(
                        "解決失敗（native: {native_err} / sidecar: {side_err}）"
                    )));
                }
                None => {
                    let _ = req.reply.send(ResolveUpdate::Error(format!("解決失敗: {native_err}")));
                }
            },
        }
        // メインスレッドの poll_resolve を回すため起床させる。
        waker();
    }
}

/// 本体 exe と同じディレクトリに置かれた解決器サイドカーのパス。
fn sidecar_path() -> Result<std::path::PathBuf> {
    let exe = std::env::current_exe().map_err(|e| anyhow!("current_exe 取得失敗: {e}"))?;
    let dir = exe.parent().ok_or_else(|| anyhow!("exe ディレクトリ不明"))?;
    let name = if cfg!(windows) { "resolver-sidecar.exe" } else { "resolver-sidecar" };
    Ok(dir.join(name))
}

/// rustypipe サイドカーを起動する（解決はサイドカー側が別プロセスで非同期に始める＝並列化の肝）。
/// stdout を切り離して返し、結果は [`read_sidecar_ready`] でブロッキング受信する。
/// ユーザー設定（解像度/コーデック）を引数で渡し、native 経路と同じ選択基準に従わせる。
fn spawn_sidecar(url: &str, req: &ResolveRequest) -> Result<(Child, std::process::ChildStdout)> {
    let video_id = clients::extract_video_id(url).ok_or_else(|| anyhow!("videoId 抽出失敗"))?;
    let max_res = req.quality.height().unwrap_or(0); // 0 = 自動（無制限）
    let codec = match req.codec {
        Codec::H264 => "h264",
        Codec::Vp9 => "vp9",
        Codec::Av1 => "av1",
        Codec::Auto => "auto",
    };
    let exe = sidecar_path()?;
    let mut child = Command::new(&exe)
        .arg(&video_id)
        .arg(max_res.to_string())
        .arg(codec)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("サイドカー起動失敗({}): {e}", exe.display()))?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("サイドカー stdout 取得失敗"))?;
    Ok((child, stdout))
}

/// サイドカーの READY を待ち、ローカル中継 URL を得る。`Child`（プロキシ）は呼び出し側が
/// 保持し、次の解決時に停止する。
fn read_sidecar_ready(
    mut child: Child,
    stdout: std::process::ChildStdout,
) -> Result<(Child, Resolved, Option<String>, bool)> {
    let mut video = String::new();
    let mut audio = String::new();
    let mut title: Option<String> = None;
    let mut is_live = false;
    let mut err: Option<String> = None;
    let mut ready = false;
    // サイドカーは TITLE/IS_LIVE/PROXY_* を出して READY で配信に入る（以後 stdout には書かない）。
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        if let Some(v) = line.strip_prefix("PROXY_VIDEO=") {
            video = v.to_string();
        } else if let Some(v) = line.strip_prefix("PROXY_AUDIO=") {
            audio = v.to_string();
        } else if let Some(v) = line.strip_prefix("TITLE=") {
            if !v.is_empty() {
                title = Some(v.to_string());
            }
        } else if let Some(v) = line.strip_prefix("IS_LIVE=") {
            is_live = v.trim() == "true";
        } else if let Some(v) = line.strip_prefix("ERROR=") {
            err = Some(v.to_string());
            break;
        } else if line.trim() == "READY" {
            ready = true;
            break;
        }
    }

    if let Some(e) = err {
        let _ = child.kill();
        let _ = child.wait();
        bail!("{e}");
    }
    if !ready || video.is_empty() {
        let _ = child.kill();
        let _ = child.wait();
        bail!("サイドカー応答不正（READY/PROXY_VIDEO なし）");
    }
    let resolved = Resolved {
        video_url: video,
        audio_url: if audio.is_empty() { None } else { Some(audio) },
    };
    Ok((child, resolved, title, is_live))
}

/// 1 件を解決する。`(Resolved, title, is_live)` を返す。
fn resolve_one(
    http: &reqwest::blocking::Client,
    _nsig: &mut nsig::NsigSolver,
    req: &ResolveRequest,
    visitor: Option<&str>,
) -> Result<(Resolved, Option<String>, bool)> {
    // YouTube 以外は素通し（M3）。
    if !is_youtube_url(&req.url) {
        return Ok((
            Resolved {
                video_url: req.url.clone(),
                audio_url: None,
            },
            None,
            false,
        ));
    }

    let video_id = clients::extract_video_id(&req.url)
        .ok_or_else(|| anyhow!("videoId を抽出できません: {}", req.url))?;

    // 1) 匿名 VOD: ANDROID_VR（署名/nsig 不要・最高画質）。
    let vr = clients::fetch_player(http, &clients::ANDROID_VR, &video_id, None, visitor)?;
    if vr.status == "OK" && !vr.is_live {
        if let Some(streaming) = &vr.streaming {
            if let Ok((v, a)) = clients::select_streams(streaming, req.quality, req.codec) {
                return Ok((finalize(http, v, a)?, vr.title, vr.is_live));
            }
        }
    }

    // 2) ANDROID: ライブ(HLS) / 公開 VOD フォールバック。
    let and = clients::fetch_player(http, &clients::ANDROID, &video_id, None, visitor)?;
    if and.status == "OK" {
        if let Some(streaming) = &and.streaming {
            if and.is_live {
                if let Some(hls) = clients::hls_manifest(streaming) {
                    return Ok((
                        Resolved {
                            video_url: hls,
                            audio_url: None,
                        },
                        and.title,
                        true,
                    ));
                }
            }
            if let Ok((v, a)) = clients::select_streams(streaming, req.quality, req.codec) {
                return Ok((finalize(http, v, a)?, and.title, and.is_live));
            }
        }
    }

    // 3) ログイン中のライブ: TVHTML5 + OAuth Bearer で解錠し hlsManifestUrl を得る。
    //    YouTube は 2026 以降ライブを全匿名 client で bot ゲート（LOGIN_REQUIRED "Sign in to
    //    confirm you're not a bot"）にしたため、匿名 ANDROID/ANDROID_VR ではライブが取れない。
    //    HLS はセグメントを mpv/ffmpeg が取得し nsig 変換が要らないので、下記 4) の VOD 認証経路の
    //    「403 になるから使わない」制約はライブには当てはまらない（ライブ限定で採用する）。
    if let Some(token) = req.access_token.as_deref() {
        if let Ok(tv) = clients::fetch_player(http, &clients::TVHTML5, &video_id, Some(token), visitor) {
            if tv.status == "OK" && tv.is_live {
                if let Some(streaming) = &tv.streaming {
                    if let Some(hls) = clients::hls_manifest(streaming) {
                        return Ok((
                            Resolved {
                                video_url: hls,
                                audio_url: None,
                            },
                            tv.title,
                            true,
                        ));
                    }
                }
            }
        }
    }

    // 4) 認証経路(TVHTML5+Bearer)の VOD adaptive は、現行 base.js では署名(s)復号も nsig 変換も
    //    適用できず（署名復号は未実装、nsig 抽出も VM 難読化の新 base.js で破綻）、解決できても
    //    stream が 403 になる。壊れた URL を Ok で返すと再生不可になるだけなので使わない。
    //    gated/members VOD 等は worker_loop が rustypipe サイドカー（解決＋ローカル中継）に
    //    フォールバックして再生する。
    bail!(
        "ネイティブ解決不可: android_vr={} android={}（gated VOD はサイドカーへ / ライブは要ログイン）",
        vr.status,
        and.status
    )
}

/// 直リンクを mpv 用に最終化。DASH manifest なら EDL に展開（終了ライブ等の保険・U3）。
fn finalize(
    http: &reqwest::blocking::Client,
    video_url: String,
    audio_url: Option<String>,
) -> Result<Resolved> {
    let video_url = if is_dash_manifest_url(&video_url) {
        build_dash_edl(http, &video_url)?
    } else {
        video_url
    };
    let audio_url = match audio_url {
        Some(a) if is_dash_manifest_url(&a) => Some(build_dash_edl(http, &a)?),
        other => other,
    };
    Ok(Resolved {
        video_url,
        audio_url,
    })
}

/// 指定 URL が YouTube かどうか判定する。
pub fn is_youtube_url(url: &str) -> bool {
    let url = url.trim();
    url.contains("youtube.com/") || url.contains("youtu.be/") || url.contains("youtube-nocookie.com/")
}

/// パーセントデコード（clients/nsig 共用）。
pub(crate) fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let Ok(b) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16) {
                    out.push(b);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// DASH manifest → mpv EDL（終了ライブアーカイブ等の保険。adaptiveFormats 直リンクでは未使用）
// ---------------------------------------------------------------------------

fn is_dash_manifest_url(url: &str) -> bool {
    url.contains("/api/manifest/dash/") || url.ends_with(".mpd")
}

fn build_dash_edl(http: &reqwest::blocking::Client, manifest_url: &str) -> Result<String> {
    let xml = http
        .get(manifest_url)
        .send()?
        .error_for_status()?
        .text()?;

    let mpd: dash_mpd::MPD = dash_mpd::parse(&xml).map_err(|e| anyhow!("MPD パース失敗: {e}"))?;

    let period = mpd.periods.first().ok_or_else(|| anyhow!("MPD に Period がありません"))?;
    let adaptation = period
        .adaptations
        .first()
        .ok_or_else(|| anyhow!("Period に AdaptationSet がありません"))?;
    let representation = adaptation
        .representations
        .first()
        .ok_or_else(|| anyhow!("AdaptationSet に Representation がありません"))?;

    let template = representation
        .SegmentTemplate
        .as_ref()
        .or(adaptation.SegmentTemplate.as_ref())
        .or(period.SegmentTemplate.as_ref())
        .ok_or_else(|| anyhow!("SegmentTemplate が見つかりません"))?;

    let media_tpl = template
        .media
        .as_deref()
        .ok_or_else(|| anyhow!("SegmentTemplate.media がありません"))?;
    let init_tpl = template
        .initialization
        .as_deref()
        .ok_or_else(|| anyhow!("SegmentTemplate.initialization がありません"))?;

    let rep_id = representation.id.as_deref().unwrap_or("");
    let bandwidth = representation.bandwidth.unwrap_or(0);

    let base = resolve_base_url(manifest_url, &mpd, period, adaptation, representation);

    let init_url = expand_url(&base, &substitute_template(init_tpl, rep_id, bandwidth, None, None));

    let start_number = template.startNumber.unwrap_or(1);
    let mut segment_urls = Vec::new();
    if let Some(timeline) = &template.SegmentTimeline {
        let mut number = start_number;
        let mut time: u64 = 0;
        for s in &timeline.segments {
            if let Some(t) = s.t {
                time = t;
            }
            let repeat = s.r.unwrap_or(0).max(0) as u64;
            for _ in 0..=repeat {
                let seg = substitute_template(media_tpl, rep_id, bandwidth, Some(number), Some(time));
                segment_urls.push(expand_url(&base, &seg));
                number += 1;
                time += s.d;
            }
        }
    } else {
        bail!("SegmentTimeline がありません（duration ベースのテンプレートは未対応）");
    }

    if segment_urls.is_empty() {
        bail!("セグメントが空です");
    }

    let mut edl = String::from("edl://!mp4_dash,init=");
    edl.push_str(&edl_escape(&init_url));
    for seg in &segment_urls {
        edl.push(';');
        edl.push_str(&edl_escape(seg));
    }
    Ok(edl)
}

fn substitute_template(
    template: &str,
    rep_id: &str,
    bandwidth: u64,
    number: Option<u64>,
    time: Option<u64>,
) -> String {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' {
            out.push(ch);
            continue;
        }
        if chars.peek() == Some(&'$') {
            chars.next();
            out.push('$');
            continue;
        }
        let mut placeholder = String::new();
        let mut closed = false;
        for c in chars.by_ref() {
            if c == '$' {
                closed = true;
                break;
            }
            placeholder.push(c);
        }
        if !closed {
            out.push('$');
            out.push_str(&placeholder);
            continue;
        }
        let (name, fmt) = match placeholder.find('%') {
            Some(i) => (&placeholder[..i], Some(&placeholder[i..])),
            None => (placeholder.as_str(), None),
        };
        let value: Option<u64> = match name {
            "RepresentationID" => None,
            "Number" => number,
            "Time" => time,
            "Bandwidth" => Some(bandwidth),
            _ => None,
        };
        if name == "RepresentationID" {
            out.push_str(rep_id);
        } else if let Some(v) = value {
            out.push_str(&format_dash_value(v, fmt));
        } else {
            out.push('$');
            out.push_str(&placeholder);
            out.push('$');
        }
    }
    out
}

fn format_dash_value(value: u64, fmt: Option<&str>) -> String {
    let Some(spec) = fmt else {
        return value.to_string();
    };
    let trimmed = spec.trim_start_matches('%').trim_end_matches('d');
    if let Some(width_str) = trimmed.strip_prefix('0') {
        if let Ok(width) = width_str.parse::<usize>() {
            return format!("{:0>width$}", value, width = width);
        }
    }
    value.to_string()
}

fn resolve_base_url(
    manifest_url: &str,
    mpd: &dash_mpd::MPD,
    period: &dash_mpd::Period,
    adaptation: &dash_mpd::AdaptationSet,
    representation: &dash_mpd::Representation,
) -> String {
    let levels: [&[dash_mpd::BaseURL]; 4] = [
        &mpd.base_url,
        &period.BaseURL,
        &adaptation.BaseURL,
        &representation.BaseURL,
    ];
    let mut current = manifest_url.to_string();
    for level in levels {
        if let Some(b) = level.first() {
            current = expand_url(&current, &b.base);
        }
    }
    current
}

fn expand_url(base: &str, target: &str) -> String {
    if target.starts_with("http://") || target.starts_with("https://") {
        return target.to_string();
    }
    if let Some(rest) = target.strip_prefix('/') {
        if let Some(host_end) = base
            .find("://")
            .and_then(|p| base[p + 3..].find('/').map(|q| p + 3 + q))
        {
            return format!("{}/{}", &base[..host_end], rest);
        }
    }
    let base_path = base.split('?').next().unwrap_or(base);
    let dir = match base_path.rfind('/') {
        Some(i) => &base_path[..=i],
        None => "",
    };
    format!("{dir}{target}")
}

fn edl_escape(s: &str) -> String {
    format!("%{}%{}", s.len(), s)
}
