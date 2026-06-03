//! `--enable-dev-tools` 時に起動するローカル HTTP サーバ。
//!
//! 目的: screencapture / cliclick / osascript を経由せずにアプリ自身が
//! スクリーンショット撮影・UI 操作エミュレーションを受け付ける。フォーカス奪取や
//! Accessibility 権限の問題を回避し、`curl` だけで検証フローを回せるようにする。
//!
//! 提供エンドポイント:
//! - `GET /screenshot`     — 現フレームを PNG で返す
//! - `POST /action/<name>` — UI 操作の intent flag を立てる（GUI 自動操作の代替）
//!
//! `<name>` は: `toggle_chat`, `toggle_recommend`, `toggle_subs`, `toggle_playlist`,
//! `toggle_history`, `play_pause`, `login`, `like`, `close_overlay`。
//!
//! スレッドモデル:
//! - HTTP サーバはバックグラウンドスレッドで動く
//! - 各リクエストは `Command` を `Sender` 経由でメインスレッドへ送り、
//!   `EventLoopProxy` で起こす
//! - メインスレッドは redraw 内で `Receiver::try_recv()` し、結果を `oneshot 相当`
//!   の reply Sender に書き戻す

use anyhow::{anyhow, Result};
use std::sync::mpsc::{channel, Sender};
use std::thread;
use std::time::Duration;
use tiny_http::{Header, Method, Response, Server};

/// バックグラウンドスレッドからメインスレッドへ送る要求。
pub enum Command {
    /// スクリーンショット。reply に PNG バイト列を送る。
    Screenshot(Sender<Vec<u8>>),
    /// UI アクションの intent flag を立てる。reply にアクション名が既知なら true。
    /// メインスレッドが「次フレームでフラグを処理する」ところまで保証し、
    /// 実際の UI 変化（オーバーレイ表示など）は次の paint で起きる。
    Action(String, Sender<bool>),
}

/// dev-tools HTTP サーバを起動し、listen ポートを返す。
///
/// `cmd_tx` でメインスレッドへ要求を投げ、`proxy` でイベントループを起こす。
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

    match (method, url.as_str()) {
        (Method::Get, "/screenshot") => handle_screenshot(req, cmd_tx, proxy),
        (Method::Post, path) if path.starts_with("/action/") => {
            let name = path.trim_start_matches("/action/").to_string();
            handle_action(req, cmd_tx, proxy, name);
        }
        _ => {
            let _ = req.respond(Response::from_string("not found").with_status_code(404));
        }
    }
}

fn handle_action(
    req: tiny_http::Request,
    cmd_tx: &Sender<Command>,
    proxy: &winit::event_loop::EventLoopProxy<crate::UserEvent>,
    name: String,
) {
    let (tx, rx) = channel();
    if cmd_tx.send(Command::Action(name.clone(), tx)).is_err() {
        let _ = req.respond(
            Response::from_string("dev-tools shutdown").with_status_code(503),
        );
        return;
    }
    let _ = proxy.send_event(crate::UserEvent::Background);

    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(true) => {
            let _ = req.respond(Response::from_string("ok\n").with_status_code(200));
        }
        Ok(false) => {
            let _ = req.respond(
                Response::from_string(format!("unknown action: {name}\n"))
                    .with_status_code(400),
            );
        }
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
    if cmd_tx.send(Command::Screenshot(tx)).is_err() {
        let _ = req.respond(
            Response::from_string("dev-tools shutdown").with_status_code(503),
        );
        return;
    }
    // メインスレッドを起こす（ControlFlow::Wait 中でもフレームを駆動するため）。
    let _ = proxy.send_event(crate::UserEvent::Background);

    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(png) => {
            let resp = Response::from_data(png)
                .with_header("Content-Type: image/png".parse::<Header>().unwrap());
            let _ = req.respond(resp);
        }
        Err(_) => {
            let _ = req.respond(Response::from_string("timeout").with_status_code(504));
        }
    }
}
