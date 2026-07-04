//! ライブチャットのドメイン層。1 動画 : 1 `ChatSession`（寿命は現実の寿命に合わせる。
//! design-principles.md「インスタンスの寿命は現実の寿命に合わせる」）。
//!
//! `crate::yt::chat`（InnerTube ポーラー）とは別物: あちらは背景スレッドの実装、
//! こちらはそれを 1 セッション単位で包む状態機械。

use crate::yt::chat as poller;
use crate::Waker;
use std::sync::atomic::AtomicI64;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

/// チャットパネルに保持するメッセージの上限。
const CHAT_MAX_MESSAGES: usize = 200;

/// ライブチャットの 1 接続（1 動画分）。破棄（`None` 代入）でポーリングも止まる（Drop で RAII）。
pub struct ChatSession {
    messages: Vec<poller::ChatMessage>,
    rx: Receiver<poller::ChatUpdate>,
    stop: poller::ChatStop,
    status: String,
}

impl ChatSession {
    pub fn messages(&self) -> &[poller::ChatMessage] {
        &self.messages
    }

    /// チャットが有効か（native_app が「チャットが有効か」の判定に読む。旧 `!chat_status.is_empty()`）。
    pub fn available(&self) -> bool {
        !self.status.is_empty()
    }
}

impl Drop for ChatSession {
    fn drop(&mut self) {
        self.stop.stop();
    }
}

/// ライブチャットのポーリングを背景スレッドで開始する。チャネルはセッションごとに生成するため、
/// 前の動画のポーラーが遅れて送るメッセージは破棄済み rx と一緒に構造的に死ぬ。
pub fn start(video_id: String, offset: Arc<AtomicI64>, waker: &Waker) -> ChatSession {
    let (tx, rx) = std::sync::mpsc::channel();
    let (stopper, stop_flag) = poller::ChatStop::new();

    let waker = Arc::clone(waker);
    std::thread::spawn(move || {
        poller::run_chat_poll(&video_id, &tx, &stop_flag, &offset);
        waker();
    });

    ChatSession {
        messages: Vec::new(),
        rx,
        stop: stopper,
        status: "チャット接続中…".to_string(),
    }
}

/// 背景スレッドからの更新を取り込む。`NotLive` を受けたら `false` を返す
/// （呼び出し側はこのセッションを `None` にして破棄する。停止処理は呼ばない — `Drop` の仕事）。
pub fn poll(s: &mut ChatSession) -> bool {
    while let Ok(update) = s.rx.try_recv() {
        match update {
            poller::ChatUpdate::Messages(msgs) => {
                s.messages.extend(msgs);
                if s.messages.len() > CHAT_MAX_MESSAGES {
                    let excess = s.messages.len() - CHAT_MAX_MESSAGES;
                    s.messages.drain(..excess);
                }
                s.status = format!("チャット ({} 件)", s.messages.len());
            }
            poller::ChatUpdate::Error(e) => {
                s.status = format!("チャットエラー: {e}");
            }
            poller::ChatUpdate::NotLive => {
                s.status.clear();
                return false;
            }
        }
    }
    true
}
