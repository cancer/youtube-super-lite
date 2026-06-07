//! yt-dlp を直接呼び出して再生用ストリーム URL を取得する。
//!
//! mpv 同梱の ytdl_hook（Lua）は yt-dlp の JSON 出力をパースするが、
//! 終了済みライブ配信などで JSON が肥大化（数十 MB）すると失敗する。
//! ここでは `yt-dlp -g` を使ってストリーム URL のみを取得し、
//! 通常コンテンツはそのまま mpv に渡す。
//!
//! DASH manifest URL（終了ライブ等）が返ってきた場合は、`dash-mpd` クレートで
//! MPD をパースし、セグメント URL を展開した mpv EDL を構築する。
//! これにより ffmpeg に DASH demuxer が無くてもライブアーカイブを再生できる。

use anyhow::{anyhow, bail, Result};
use std::process::Command;
use std::sync::mpsc::Sender;
use std::time::Duration as StdDuration;

/// yt-dlp 用の `Command` を作る。Windows では CREATE_NO_WINDOW を付け、GUI アプリ
/// （コンソール無し）から起動してもコンソール窓が出ないようにする。
fn ytdlp() -> Command {
    let mut cmd = Command::new("yt-dlp");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// 解決済みのストリーム情報。
#[derive(Clone, Debug)]
pub struct Resolved {
    /// 映像ストリーム URL もしくは EDL 文字列。
    pub video_url: String,
    /// 音声ストリーム URL もしくは EDL 文字列（DASH 分離・通常の DASH/直リンクで指定）。
    pub audio_url: Option<String>,
    /// 動画タイトル（取得失敗時は None）。
    pub title: Option<String>,
}

/// 背景スレッドからメインスレッドへの通知。
pub enum ResolveUpdate {
    Ready(Resolved),
    Error(String),
}

/// 指定 URL を yt-dlp で解決し、結果を `tx` に送る。背景スレッドで呼び出す。
/// `format` は yt-dlp の `-f` 指定（画質/コーデック）。
pub fn resolve(url: &str, format: &str, tx: &Sender<ResolveUpdate>) {
    match resolve_inner(url, format) {
        Ok(r) => {
            let _ = tx.send(ResolveUpdate::Ready(r));
        }
        Err(e) => {
            let _ = tx.send(ResolveUpdate::Error(e.to_string()));
        }
    }
}

fn resolve_inner(url: &str, format: &str) -> Result<Resolved> {
    let output = ytdlp()
        .args(["--no-warnings", "-g", "-f", format, url])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("yt-dlp -g 失敗: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let urls: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();

    let (video_raw, audio_raw) = match urls.len() {
        2 => (urls[0].clone(), Some(urls[1].clone())),
        1 => (urls[0].clone(), None),
        n => bail!("yt-dlp が想定外の URL 数を返しました: {n}"),
    };

    // DASH manifest なら EDL に展開する。それ以外はそのまま渡す。
    let video_url = if is_dash_manifest_url(&video_raw) {
        build_dash_edl(&video_raw)?
    } else {
        video_raw
    };
    let audio_url = match audio_raw {
        Some(a) if is_dash_manifest_url(&a) => Some(build_dash_edl(&a)?),
        other => other,
    };

    let title = fetch_title(url);

    Ok(Resolved {
        video_url,
        audio_url,
        title,
    })
}

fn fetch_title(url: &str) -> Option<String> {
    let output = ytdlp()
        .args([
            "--no-warnings",
            // Windows(日本語ロケール)の frozen yt-dlp はパイプ出力時に cp932 で出すため、
            // UTF-8 を強制する。これがないとタイトルが文字化け（豆腐）する。
            "--encoding",
            "UTF-8",
            "--skip-download",
            "--print",
            "%(title)s",
            url,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let t = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

/// 指定 URL が YouTube かどうか判定する。
pub fn is_youtube_url(url: &str) -> bool {
    let url = url.trim();
    url.contains("youtube.com/")
        || url.contains("youtu.be/")
        || url.contains("youtube-nocookie.com/")
}

// ---------------------------------------------------------------------------
// DASH manifest → mpv EDL
// ---------------------------------------------------------------------------

/// yt-dlp が返した URL が DASH manifest かどうか判定する。
fn is_dash_manifest_url(url: &str) -> bool {
    url.contains("/api/manifest/dash/") || url.ends_with(".mpd")
}

/// DASH manifest を取得し、セグメント URL を展開した mpv EDL 文字列を構築する。
///
/// EDL 形式（fragmented MP4）: `edl://!mp4_dash,init=<init>;<seg1>;<seg2>;...`
fn build_dash_edl(manifest_url: &str) -> Result<String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(StdDuration::from_secs(15))
        .build()?;
    let xml = client
        .get(manifest_url)
        .send()?
        .error_for_status()?
        .text()?;

    let mpd: dash_mpd::MPD =
        dash_mpd::parse(&xml).map_err(|e| anyhow!("MPD パース失敗: {e}"))?;

    let period = mpd
        .periods
        .first()
        .ok_or_else(|| anyhow!("MPD に Period がありません"))?;
    let adaptation = period
        .adaptations
        .first()
        .ok_or_else(|| anyhow!("Period に AdaptationSet がありません"))?;
    let representation = adaptation
        .representations
        .first()
        .ok_or_else(|| anyhow!("AdaptationSet に Representation がありません"))?;

    // SegmentTemplate は Representation > AdaptationSet > Period の優先順で探す。
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

    // BaseURL を解決。MPD > Period > AdaptationSet > Representation の各レベルから集めて、
    // manifest URL ベースで結合する。
    let base = resolve_base_url(manifest_url, &mpd, period, adaptation, representation);

    let init_url = expand_url(
        &base,
        &substitute_template(init_tpl, rep_id, bandwidth, None, None),
    );

    // SegmentTimeline を展開してセグメント番号列を生成。
    let start_number = template.startNumber.unwrap_or(1);
    let mut segment_urls = Vec::new();
    if let Some(timeline) = &template.SegmentTimeline {
        let mut number = start_number;
        let mut time: u64 = 0;
        for s in &timeline.segments {
            // S 要素の t 属性があればそこから時刻を再設定。
            if let Some(t) = s.t {
                time = t;
            }
            // r が None または 0 のときは 1 セグメント。r=N なら N+1 個（DASH 仕様）。
            let repeat = s.r.unwrap_or(0).max(0) as u64;
            for _ in 0..=repeat {
                let seg = substitute_template(
                    media_tpl,
                    rep_id,
                    bandwidth,
                    Some(number),
                    Some(time),
                );
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

    // EDL を構築。EDL 内のフィールドは長さプレフィックス escape を使うと
    // URL 内の ; を安全に通せる: `%<len>%<value>`。
    let mut edl = String::from("edl://!mp4_dash,init=");
    edl.push_str(&edl_escape(&init_url));
    for seg in &segment_urls {
        edl.push(';');
        edl.push_str(&edl_escape(seg));
    }
    Ok(edl)
}

/// テンプレート文字列内の `$RepresentationID$` / `$Number$` / `$Time$` / `$Bandwidth$` を置換する。
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
        // `$$` は `$` にエスケープ。
        if chars.peek() == Some(&'$') {
            chars.next();
            out.push('$');
            continue;
        }
        // `$<name>[%<format>]$` を切り出す。
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
            // 閉じない $ はそのまま戻す。
            out.push('$');
            out.push_str(&placeholder);
            continue;
        }

        // 名前とフォーマット指定（%05d 等）を分割。
        let (name, fmt) = match placeholder.find('%') {
            Some(i) => (&placeholder[..i], Some(&placeholder[i..])),
            None => (placeholder.as_str(), None),
        };

        let value: Option<u64> = match name {
            "RepresentationID" => None, // 文字列なので別経路。
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
            // 未対応 / 値なしはプレースホルダのまま残す。
            out.push('$');
            out.push_str(&placeholder);
            out.push('$');
        }
    }
    out
}

/// DASH の `%0<width>d` 形式の幅指定子に対応。指定がなければ単なる十進表記。
fn format_dash_value(value: u64, fmt: Option<&str>) -> String {
    let Some(spec) = fmt else {
        return value.to_string();
    };
    // 期待形式: %0Nd（幅 N の 0 埋め十進）。それ以外はフォールバックで十進。
    let trimmed = spec.trim_start_matches('%').trim_end_matches('d');
    if let Some(width_str) = trimmed.strip_prefix('0') {
        if let Ok(width) = width_str.parse::<usize>() {
            return format!("{:0>width$}", value, width = width);
        }
    }
    value.to_string()
}

/// BaseURL を解決する。各レベルの BaseURL を順に結合し、最終的に manifest URL を基準として絶対化する。
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

/// 相対 URL を base に対して展開する。
/// 単純化: base が `?` を含む場合は path 部分のみで再構成する（YouTube DASH manifest 向け）。
fn expand_url(base: &str, target: &str) -> String {
    // 絶対 URL ならそのまま。
    if target.starts_with("http://") || target.starts_with("https://") {
        return target.to_string();
    }
    // ルート相対 (/foo) の場合は base のホストまでで切る。
    if let Some(rest) = target.strip_prefix('/') {
        if let Some(host_end) = base
            .find("://")
            .and_then(|p| base[p + 3..].find('/').map(|q| p + 3 + q))
        {
            return format!("{}/{}", &base[..host_end], rest);
        }
    }
    // ?クエリ等を捨て、base の最後の `/` まででディレクトリを取り出して結合。
    let base_path = base.split('?').next().unwrap_or(base);
    let dir = match base_path.rfind('/') {
        Some(i) => &base_path[..=i],
        None => "",
    };
    format!("{dir}{target}")
}

/// EDL の文字列フィールドを `%<len>%<value>` 形式でエスケープする。
fn edl_escape(s: &str) -> String {
    format!("%{}%{}", s.len(), s)
}
