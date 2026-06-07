//! `--enable-dev-tools` 時に起動するローカル HTTP サーバ。
//!
//! 目的: 外部の screencapture / cliclick 等に依存せず、アプリ自身がスクリーンショット
//! 撮影・UI 操作エミュレーションを受け付ける。フォーカス奪取の問題を避け、`curl` だけで
//! 検証フローを回せるようにする（egui 版から移植）。
//!
//! 提供エンドポイント:
//! - `GET /screenshot`         — 現在のウィンドウ(クライアント領域)を PNG で返す
//! - `POST /action/<name>`     — UI 操作の intent を起こす（GUI 自動操作の代替）
//! - `POST /click?x=&y=`       — 指定座標（/screenshot と同じクライアント px）に左クリックを注入
//! - `POST /type`（body=text, `?enter=1`）— URL 欄へテキスト入力（任意で Enter 再生）
//!
//! `<name>` は: `toggle_chat`, `play_pause`, `login`, `like`, `close_overlay`,
//! `open_recommend`, `open_subs`, `open_playlist`, `open_history`。
//!
//! スレッドモデル: HTTP サーバは背景スレッドで動き、各リクエストは [`Command`] を
//! `Sender` 経由でメインスレッドへ送って `EventLoopProxy` で起こす。メインスレッドは
//! ループ内で `try_recv` し、結果を reply Sender に書き戻す。

use anyhow::{anyhow, Result};
use std::sync::mpsc::{channel, Sender};
use std::thread;
use std::time::Duration;
use tiny_http::{Header, Method, Response, Server};

/// 背景スレッド（HTTP）からメインスレッドへ送る要求。
pub enum Command {
    /// スクリーンショット。reply に PNG バイト列を送る。
    Screenshot(Sender<Vec<u8>>),
    /// UI アクションの intent。reply にアクション名が既知なら true。
    Action(String, Sender<bool>),
    /// 指定座標（クライアント px）に左クリックを注入する。
    Click { x: i32, y: i32, reply: Sender<bool> },
    /// URL 欄へテキストを入力し、必要なら Enter（再生）を送る。
    Type {
        text: String,
        enter: bool,
        reply: Sender<bool>,
    },
}

/// dev-tools HTTP サーバを起動し、listen ポートを返す。
pub fn start(
    cmd_tx: Sender<Command>,
    proxy: winit::event_loop::EventLoopProxy<crate::UserEvent>,
) -> Result<u16> {
    let server = Server::http("127.0.0.1:0")
        .map_err(|e| anyhow!("dev-tools サーバの起動に失敗: {e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| anyhow!("dev-tools サーバの listen アドレス取得に失敗"))?
        .port();

    thread::spawn(move || {
        for req in server.incoming_requests() {
            handle(req, &cmd_tx, &proxy);
        }
    });

    Ok(port)
}

fn handle(
    req: tiny_http::Request,
    cmd_tx: &Sender<Command>,
    proxy: &winit::event_loop::EventLoopProxy<crate::UserEvent>,
) {
    let url = req.url().to_string();
    let method = req.method().clone();
    let path_only = url.split('?').next().unwrap_or(&url);
    match (method, path_only) {
        (Method::Get, "/screenshot") => handle_screenshot(req, cmd_tx, proxy),
        (Method::Post, "/click") => handle_click(req, cmd_tx, proxy, &url),
        (Method::Post, "/type") => handle_type(req, cmd_tx, proxy, &url),
        (Method::Post, path) if path.starts_with("/action/") => {
            let name = path.trim_start_matches("/action/").to_string();
            handle_action(req, cmd_tx, proxy, name);
        }
        _ => {
            let _ = req.respond(Response::from_string("not found").with_status_code(404));
        }
    }
}

/// メインスレッドへ要求を送り、起こして、reply を待つ共通処理。
fn dispatch<T, F>(
    req: tiny_http::Request,
    cmd_tx: &Sender<Command>,
    proxy: &winit::event_loop::EventLoopProxy<crate::UserEvent>,
    make: F,
    rx: std::sync::mpsc::Receiver<T>,
    on_ok: impl FnOnce(tiny_http::Request, T),
) where
    F: FnOnce() -> Command,
{
    if cmd_tx.send(make()).is_err() {
        let _ = req.respond(Response::from_string("dev-tools shutdown").with_status_code(503));
        return;
    }
    let _ = proxy.send_event(crate::UserEvent::Background);
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(v) => on_ok(req, v),
        Err(_) => {
            let _ = req.respond(Response::from_string("timeout").with_status_code(504));
        }
    }
}

fn handle_screenshot(
    req: tiny_http::Request,
    cmd_tx: &Sender<Command>,
    proxy: &winit::event_loop::EventLoopProxy<crate::UserEvent>,
) {
    let (tx, rx) = channel();
    dispatch(req, cmd_tx, proxy, || Command::Screenshot(tx), rx, |req, png| {
        if png.is_empty() {
            let _ = req.respond(Response::from_string("capture failed").with_status_code(500));
        } else {
            let resp = Response::from_data(png)
                .with_header("Content-Type: image/png".parse::<Header>().unwrap());
            let _ = req.respond(resp);
        }
    });
}

fn handle_action(
    req: tiny_http::Request,
    cmd_tx: &Sender<Command>,
    proxy: &winit::event_loop::EventLoopProxy<crate::UserEvent>,
    name: String,
) {
    let (tx, rx) = channel();
    let nm = name.clone();
    dispatch(req, cmd_tx, proxy, || Command::Action(nm, tx), rx, move |req, known| {
        if known {
            let _ = req.respond(Response::from_string("ok\n"));
        } else {
            let _ = req.respond(
                Response::from_string(format!("unknown action: {name}\n")).with_status_code(400),
            );
        }
    });
}

fn handle_click(
    req: tiny_http::Request,
    cmd_tx: &Sender<Command>,
    proxy: &winit::event_loop::EventLoopProxy<crate::UserEvent>,
    url: &str,
) {
    let mut x: Option<i32> = None;
    let mut y: Option<i32> = None;
    if let Some(q) = url.split('?').nth(1) {
        for pair in q.split('&') {
            let mut it = pair.splitn(2, '=');
            match (it.next(), it.next()) {
                (Some("x"), Some(v)) => x = v.parse().ok(),
                (Some("y"), Some(v)) => y = v.parse().ok(),
                _ => {}
            }
        }
    }
    let (Some(x), Some(y)) = (x, y) else {
        let _ = req.respond(
            Response::from_string("usage: POST /click?x=<px>&y=<px>\n").with_status_code(400),
        );
        return;
    };
    let (tx, rx) = channel();
    dispatch(req, cmd_tx, proxy, || Command::Click { x, y, reply: tx }, rx, |req, _| {
        let _ = req.respond(Response::from_string("ok\n"));
    });
}

fn handle_type(
    mut req: tiny_http::Request,
    cmd_tx: &Sender<Command>,
    proxy: &winit::event_loop::EventLoopProxy<crate::UserEvent>,
    url: &str,
) {
    let enter = url.contains("enter=1");
    let mut text = String::new();
    let _ = req.as_reader().read_to_string(&mut text);
    let (tx, rx) = channel();
    dispatch(
        req,
        cmd_tx,
        proxy,
        || Command::Type { text, enter, reply: tx },
        rx,
        |req, _| {
            let _ = req.respond(Response::from_string("ok\n"));
        },
    );
}
