//! 複数ドメインを触る system はここにしか置けない（ドメイン間 import 禁止の帰結）。
//! 現在 3 本。4 本目を足したくなったら、それは本当に跨ぎか（片方のドメイン内で閉じないか）を
//! 疑う — 本当に跨ぎなら Issue で相談する（design-principles.md 原則3）。

use crate::account::{self, Account};
use crate::chat::{self, ChatSession};
use crate::content::{self, Feed};
use crate::playback::{self, Playback};
use crate::yt::auth;
use crate::yt::recommend::VideoItem;
use crate::yt::resolve;
use crate::Waker;
use std::sync::Arc;

/// ①ログイン確定: 再生履歴への記録・おすすめ先読み・保留していた再生の解決
/// （旧 poll_auth の `AccountEvent::LoggedIn` 処理）。
pub fn on_logged_in(pb: &mut Playback, acc: &Account, recommend: &mut Feed<VideoItem>, waker: &Waker) {
    // CLI 引数経由で既に play() を通った動画がここで履歴に載る。
    account::start_mark_watched_if_logged_in(acc.token(), pb.current_url());
    // ログイン確定＝おすすめ（ホームフィード）を先読みしておく（動画非依存）。
    if let Some(token) = acc.token() {
        content::start_recommend(recommend, token, waker);
    }
    // ログイン待ちで保留していた動画を、access_token 付きで解決開始する。
    if let Some(url) = playback::take_pending(pb) {
        playback::start_resolve(pb, url, acc.token());
    }
}

/// ②再生開始: ログイン処理中（tokens 未確定）なら解決を保留する（bot ゲート回避）。
/// ここで匿名解決すると LOGIN_REQUIRED で members/年齢制限はもちろん、多くの通常動画まで
/// 再生不可になるため、ログイン確定後に access_token 付きで解決する。
pub fn play(pb: &mut Playback, acc: &Account, url: &str) {
    let url = url.trim().to_string();
    if url.is_empty() {
        return;
    }
    // ログイン済みなら再生履歴に載せる。CLI 引数経由の起動直後は auto-login が完了する前に
    // ここに来るため token=None になりがちで、その場合はログイン確定時に on_logged_in が
    // 履歴に載せ直す。
    account::start_mark_watched_if_logged_in(acc.token(), &url);

    if !resolve::is_youtube_url(&url) {
        // YouTube 以外の URL（直リンク等）はそのまま mpv に渡す。
        playback::load_direct(pb, url);
        return;
    }

    if acc.token().is_none() && acc.is_busy() {
        playback::hold(pb, url);
    } else {
        playback::start_resolve(pb, url, acc.token());
    }
}

/// ③再生とチャットの連動: 再生開始 + video_id 抽出 + チャット接続
/// （native_app に4箇所コピペされていたコンボ）。
pub fn play_with_chat(pb: &mut Playback, chat_slot: &mut Option<ChatSession>, acc: &Account, url: &str, waker: &Waker) {
    play(pb, acc, url);
    if let Some(video_id) = auth::extract_video_id(pb.current_url()) {
        let offset = Arc::clone(pb.player_offset_ms());
        *chat_slot = Some(chat::start(video_id, offset, waker));
    }
}
