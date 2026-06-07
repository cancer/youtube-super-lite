mod auth;
mod chat;
mod controller;
mod gpu_usage;
mod history;
mod image_cache;
mod mark_watched;
mod native_app;
mod native_overlay;
mod player;
mod playlist;
mod recommend;
mod resolve;
mod subscriptions;

use anyhow::{anyhow, bail, Result};
use std::path::PathBuf;
use winit::event_loop::{ControlFlow, EventLoop};

/// イベントループを起こす要求（mpv の新フレーム / 背景スレッド完了）。
#[derive(Debug, Clone, Copy)]
enum UserEvent {
    MpvRedraw,
    Background,
}

/// 背景スレッド（OAuth / API 呼び出し）からの結果。
enum AuthMsg {
    LoggedIn {
        tokens: auth::Tokens,
        channel: Option<String>,
    },
    Like {
        ok: bool,
        msg: String,
        tokens: Option<auth::Tokens>,
    },
    Failed(String),
}


/// チャットパネルに保持するメッセージの上限。
const CHAT_MAX_MESSAGES: usize = 200;


/// 既定ブラウザで URL を開く。
fn open_in_browser(url: &str) {
    if url.is_empty() {
        return;
    }
    #[cfg(target_os = "windows")]
    {
        // `cmd /C start "" <url>` は URL 中の `&`（OAuth URL に多数ある）を cmd が
        // コマンド区切りと解釈して URL が途中で切れてしまう。rundll32 の
        // FileProtocolHandler は URL を単一引数として受け取るため安全に既定ブラウザで開ける。
        let _ = std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
}

/// 画質（最大の縦解像度）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Quality {
    Auto,
    P2160,
    P1440,
    P1080,
    P720,
    P480,
    P360,
}

impl Quality {
    const ALL: [Quality; 7] = [
        Quality::Auto,
        Quality::P2160,
        Quality::P1440,
        Quality::P1080,
        Quality::P720,
        Quality::P480,
        Quality::P360,
    ];
    fn height(self) -> Option<u32> {
        match self {
            Quality::Auto => None,
            Quality::P2160 => Some(2160),
            Quality::P1440 => Some(1440),
            Quality::P1080 => Some(1080),
            Quality::P720 => Some(720),
            Quality::P480 => Some(480),
            Quality::P360 => Some(360),
        }
    }
    fn label(self) -> &'static str {
        match self {
            Quality::Auto => "自動",
            Quality::P2160 => "2160p",
            Quality::P1440 => "1440p",
            Quality::P1080 => "1080p",
            Quality::P720 => "720p",
            Quality::P480 => "480p",
            Quality::P360 => "360p",
        }
    }
}

/// 映像コーデック。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Codec {
    Auto,
    H264,
    Vp9,
    Av1,
}

impl Codec {
    const ALL: [Codec; 4] = [Codec::Auto, Codec::H264, Codec::Vp9, Codec::Av1];
    /// yt-dlp フォーマットフィルタの vcodec 条件。
    fn vfilter(self) -> &'static str {
        match self {
            Codec::Auto => "",
            Codec::H264 => "[vcodec^=avc1]",
            Codec::Vp9 => "[vcodec^=vp09]",
            Codec::Av1 => "[vcodec^=av01]",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Codec::Auto => "自動",
            Codec::H264 => "H.264",
            Codec::Vp9 => "VP9",
            Codec::Av1 => "AV1",
        }
    }
}

/// 画質・コーデック指定から yt-dlp の `-f` フォーマット文字列を組み立てる。
/// 厳しい条件 → 緩い条件 → 既定 の順でフォールバックし、必ず再生できるようにする。
fn build_ytdlp_format(quality: Quality, codec: Codec) -> String {
    let hf = quality
        .height()
        .map(|h| format!("[height<={h}]"))
        .unwrap_or_default();
    let cf = codec.vfilter();
    if hf.is_empty() && cf.is_empty() {
        return "bestvideo+bestaudio/best".to_string();
    }
    format!("bestvideo{hf}{cf}+bestaudio/bestvideo{hf}+bestaudio/best{hf}/bestvideo+bestaudio/best")
}

/// yt-dlp が PATH 上にあるか確認し、なければ同梱ディレクトリを PATH 先頭に追加する。
fn ensure_ytdlp_on_path() {
    let ytdlp_name = if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" };
    let path_sep = if cfg!(windows) { ";" } else { ":" };

    // システム PATH 上にあればそのまま使う。
    if let Ok(output) = std::process::Command::new("which")
        .arg("yt-dlp")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout);
            println!("yt-dlp found: {}", path.trim());
            return;
        }
    }

    // 同梱ディレクトリを探す。
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.to_path_buf());
            candidates.push(dir.join("tools"));
        }
    }
    candidates.push(PathBuf::from("tools"));

    for dir in &candidates {
        if dir.join(ytdlp_name).exists() {
            let current = std::env::var("PATH").unwrap_or_default();
            std::env::set_var("PATH", format!("{}{path_sep}{current}", dir.display()));
            println!("yt-dlp found: {}", dir.join(ytdlp_name).display());
            return;
        }
    }
    eprintln!("warning: yt-dlp not found; YouTube URLs may fail");
}
/// CLI 引数のパース結果。
struct CliArgs {
    url: Option<String>,
    verbose: bool,
    backend: String,
    volume: Option<f64>,
}

fn parse_args() -> Result<CliArgs> {
    let mut verbose = false;
    let mut backend = auth::DEFAULT_BACKEND.to_string();
    let mut url: Option<String> = None;
    let mut volume: Option<f64> = None;

    let parse_volume = |s: &str| -> Result<f64> {
        let v: f64 = s
            .parse()
            .map_err(|_| anyhow!("--volume の値が不正です: {s}"))?;
        Ok(v.clamp(0.0, 130.0))
    };

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-v" | "--verbose" => verbose = true,
            "--debug-backend" => {
                backend = args
                    .next()
                    .ok_or_else(|| anyhow!("--debug-backend に URL を指定してください"))?;
            }
            // 旧 egui 版のフラグ。互換のため受理するが無視する（現在は常にネイティブ版）。
            "--enable-dev-tools" | "--native" => {}
            // 初期音量（デバッグ用。例: --volume 0 で無音起動）。`--volume=0` 形式も可。
            "--volume" => {
                let v = args
                    .next()
                    .ok_or_else(|| anyhow!("--volume に値を指定してください"))?;
                volume = Some(parse_volume(&v)?);
            }
            s if s.starts_with("--volume=") => {
                volume = Some(parse_volume(&s["--volume=".len()..])?);
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other if other.starts_with('-') => {
                bail!("不明なオプション: {other}");
            }
            _ => {
                if url.is_some() {
                    bail!("URL が複数指定されています");
                }
                url = Some(a);
            }
        }
    }

    Ok(CliArgs {
        url,
        verbose,
        backend: backend.trim_end_matches('/').to_string(),
        volume,
    })
}

fn print_help() {
    println!(
        "YouTube Super Lite\n\
         \n\
         Usage: youtube-super-lite [OPTIONS] [URL]\n\
         \n\
         Options:\n\
         \x20\x20-v, --verbose             mpv の詳細ログを出力\n\
         \x20\x20    --debug-backend URL   認証バックエンドを上書き（デバッグ用、デフォルト: {}）\n\
         \x20\x20    --volume N            初期音量 0-130（デバッグ用。例: --volume 0 で無音）\n\
         \x20\x20-h, --help                このヘルプを表示",
        auth::DEFAULT_BACKEND
    );
}

fn main() -> Result<()> {
    let args = parse_args()?;

    if let Some(url) = &args.url {
        println!("YouTube Super Lite - playing: {url}");
    } else {
        println!("YouTube Super Lite - URL 欄に貼り付けて Enter で再生");
    }

    ensure_ytdlp_on_path();

    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    let mut app =
        native_app::NativeApp::new(proxy, args.url, args.verbose, args.backend, args.volume);
    event_loop.run_app(&mut app)?;
    Ok(())
}
