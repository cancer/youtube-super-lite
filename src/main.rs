// リリースビルドではコンソールウィンドウを出さない（GUI アプリとしてリンク）。
// デバッグビルドはログ確認のためコンソールを残す。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod design;
mod devtools;
mod ui;
#[cfg(windows)]
mod dcomp_overlay;
mod settings;
#[cfg(windows)]
mod webview_host;

use anyhow::{anyhow, bail, Result};
use winit::event_loop::{ControlFlow, EventLoop};

/// UI 非依存コア（crates/ysl-core）から、bin 側が従来どおり `Quality`/`Codec` を
/// `crate::` 直下で参照できるように再エクスポートする。
pub use ysl_core::types::{Codec, Quality};

/// イベントループを起こす要求（背景スレッド完了時に送る）。
#[derive(Debug, Clone, Copy)]
enum UserEvent {
    Background,
}

/// CLI 引数のパース結果。
struct CliArgs {
    url: Option<String>,
    verbose: bool,
    backend: String,
    volume: Option<f64>,
    enable_dev_tools: bool,
    /// WebView2 プローブ（issue #16 PR1）を有効化するか。無指定時は WebView2 子窓を作らず
    /// 従来と完全に同一挙動（実験機能はフラグ排他）。
    webview_probe: bool,
}

fn parse_args() -> Result<CliArgs> {
    let mut verbose = false;
    let mut backend = ysl_core::yt::auth::DEFAULT_BACKEND.to_string();
    let mut url: Option<String> = None;
    let mut volume: Option<f64> = None;
    let mut enable_dev_tools = false;
    let mut webview_probe = false;

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
            // WebView2 プローブ（issue #16 PR1）を有効化。無指定時は WebView2 子窓を作らない。
            "--webview-probe" => webview_probe = true,
            // 旧フラグ。互換のため受理するが無視する（オーバーレイは常に子窓+DirectComposition、
            // 描画は常にネイティブ版）。
            "--dcomp" | "--legacy" | "--native" => {}
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
        webview_probe,
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
         \x20\x20    --webview-probe       WebView2 子窓プローブ（issue #16 PR1・実験機能）を有効化\n\
         \x20\x20-h, --help                このヘルプを表示",
        ysl_core::yt::auth::DEFAULT_BACKEND
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
    let mut app = ui::NativeApp::new(
        proxy,
        args.url,
        args.verbose,
        args.backend,
        args.volume,
        args.enable_dev_tools,
        args.webview_probe,
    );
    event_loop.run_app(&mut app)?;
    Ok(())
}
