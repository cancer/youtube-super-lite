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
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration as StdDuration;
use winit::event_loop::EventLoopProxy;

use crate::{Codec, Quality, UserEvent};

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
    /// タイトル・ライブ判定（再生開始後に後追いで届く。表示更新のみ）。
    Meta { title: Option<String>, is_live: bool },
    Error(String),
}

/// 解決リクエスト（メイン → ワーカー）。
pub struct ResolveRequest {
    pub url: String,
    pub quality: Quality,
    pub codec: Codec,
    /// ログイン中なら OAuth access_token（認証経路で TVHTML5 に Bearer 付与）。
    pub access_token: Option<String>,
}

/// 常駐解決器へのハンドル。リクエストを送るだけ。
pub struct ResolverHandle {
    req_tx: Sender<ResolveRequest>,
}

impl ResolverHandle {
    /// 解決器ワーカーをアプリ起動時に 1 回だけ起動する（M15）。
    /// 結果は `update_tx` に流し、メインスレッドを起こすため `proxy` に Background を送る。
    pub fn spawn(update_tx: Sender<ResolveUpdate>, proxy: EventLoopProxy<UserEvent>) -> Self {
        let (req_tx, req_rx) = std::sync::mpsc::channel::<ResolveRequest>();
        std::thread::spawn(move || worker_loop(req_rx, update_tx, proxy));
        Self { req_tx }
    }

    /// 解決を依頼する。
    pub fn request(&self, req: ResolveRequest) {
        let _ = self.req_tx.send(req);
    }
}

/// 常駐ワーカー本体。HTTP クライアントと nsig エンジンを保持し続ける。
fn worker_loop(
    req_rx: Receiver<ResolveRequest>,
    update_tx: Sender<ResolveUpdate>,
    proxy: EventLoopProxy<UserEvent>,
) {
    let http = match reqwest::blocking::Client::builder()
        .timeout(StdDuration::from_secs(20))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = update_tx.send(ResolveUpdate::Error(format!("HTTP クライアント生成失敗: {e}")));
            return;
        }
    };
    let mut nsig = nsig::NsigSolver::new();

    while let Ok(req) = req_rx.recv() {
        match resolve_one(&http, &mut nsig, &req) {
            Ok((resolved, title, is_live)) => {
                let _ = update_tx.send(ResolveUpdate::Ready(resolved));
                let _ = update_tx.send(ResolveUpdate::Meta { title, is_live });
            }
            Err(e) => {
                let _ = update_tx.send(ResolveUpdate::Error(e.to_string()));
            }
        }
        // メインスレッドの poll_resolve を回すため起床させる。
        let _ = proxy.send_event(UserEvent::Background);
    }
}

/// 1 件を解決する。`(Resolved, title, is_live)` を返す。
fn resolve_one(
    http: &reqwest::blocking::Client,
    nsig: &mut nsig::NsigSolver,
    req: &ResolveRequest,
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
    let vr = clients::fetch_player(http, &clients::ANDROID_VR, &video_id, None)?;
    if vr.status == "OK" && !vr.is_live {
        if let Some(streaming) = &vr.streaming {
            if let Ok((v, a)) = clients::select_streams(streaming, req.quality, req.codec) {
                return Ok((finalize(http, v, a)?, vr.title, vr.is_live));
            }
        }
    }

    // 2) ANDROID: ライブ(HLS) / 公開 VOD フォールバック。
    let and = clients::fetch_player(http, &clients::ANDROID, &video_id, None)?;
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

    // 3) 認証経路: TVHTML5 + OAuth Bearer（members/年齢制限）→ URL の n を nsig 変換（M17/M10）。
    if let Some(token) = req.access_token.as_deref() {
        let tv = clients::fetch_player(http, &clients::TVHTML5, &video_id, Some(token))?;
        if tv.status == "OK" {
            if let Some(streaming) = &tv.streaming {
                if tv.is_live {
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
                let (v, a) = clients::select_streams(streaming, req.quality, req.codec)?;
                let v = nsig.transform_url(http, &v)?;
                let a = match a {
                    Some(a) => Some(nsig.transform_url(http, &a)?),
                    None => None,
                };
                return Ok((finalize(http, v, a)?, tv.title, tv.is_live));
            }
        }
        bail!("解決失敗（認証経路）: {}", tv.status);
    }

    bail!(
        "解決失敗: android_vr={} android={}（ログインで members/年齢制限に対応）",
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
