// リリースビルドではコンソールウィンドウを出さない（GUI アプリとしてリンク）。
// デバッグビルドはログ確認のためコンソールを残す。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod auth;
mod chat;
mod controller;
mod devtools;
mod gpu_usage;
mod history;
mod image_cache;
mod mark_watched;
mod native_app;
mod native_overlay;
#[cfg(windows)]
mod dcomp_overlay;
mod player;
mod playlist;
mod recommend;
mod resolve;
mod settings;
mod subscriptions;

use anyhow::{anyhow, bail, Result};
use winit::event_loop::{ControlFlow, EventLoop};

/// イベントループを起こす要求（背景スレッド完了時に送る）。
#[derive(Debug, Clone, Copy)]
enum UserEvent {
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
        use std::os::windows::process::CommandExt;
        // `cmd /C start "" <url>` は URL 中の `&`（OAuth URL に多数ある）を cmd が
        // コマンド区切りと解釈して URL が途中で切れてしまう。rundll32 の
        // FileProtocolHandler は URL を単一引数として受け取るため安全に既定ブラウザで開ける。
        // CREATE_NO_WINDOW: GUI アプリから起動してもコンソール窓を出さない。
        let _ = std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .creation_flags(0x0800_0000)
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
    fn label(self) -> &'static str {
        match self {
            Codec::Auto => "自動",
            Codec::H264 => "H.264",
            Codec::Vp9 => "VP9",
            Codec::Av1 => "AV1",
        }
    }
}


/// CLI 引数のパース結果。
struct CliArgs {
    url: Option<String>,
    verbose: bool,
    backend: String,
    volume: Option<f64>,
    enable_dev_tools: bool,
    /// 新オーバーレイ（子窓 + DirectComposition）を使う。移行中の暫定トグル。
    dcomp: bool,
}

fn parse_args() -> Result<CliArgs> {
    let mut verbose = false;
    let mut backend = auth::DEFAULT_BACKEND.to_string();
    let mut url: Option<String> = None;
    let mut volume: Option<f64> = None;
    let mut enable_dev_tools = false;
    let mut dcomp = false;

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
            // 検証用ローカル HTTP（スクショ/操作注入）を有効化。
            "--enable-dev-tools" => enable_dev_tools = true,
            // 新オーバーレイ（子窓 + DirectComposition）を使う暫定トグル（移行中）。
            "--dcomp" => dcomp = true,
            // 旧 egui 版のフラグ。互換のため受理するが無視する（現在は常にネイティブ版）。
            "--native" => {}
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
        enable_dev_tools,
        dcomp,
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
         \x20\x20    --enable-dev-tools    検証用ローカル HTTP（/screenshot, /click, /action 等）を有効化\n\
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

    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    let mut app = native_app::NativeApp::new(
        proxy,
        args.url,
        args.verbose,
        args.backend,
        args.volume,
        args.enable_dev_tools,
        args.dcomp,
    );
    event_loop.run_app(&mut app)?;
    Ok(())
}
