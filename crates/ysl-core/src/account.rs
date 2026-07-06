//! ログインのドメイン層。credentials（誰でログインしているか）はアプリ寿命の 1 事実、
//! 進行中の login/like は per-operation（design-principles.md「寿命は現実の寿命に合わせる」）。

use crate::yt::auth;
use crate::Waker;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// 背景スレッド（OAuth / API 呼び出し）からの結果。
pub enum AuthMsg {
    LoggedIn {
        tokens: auth::Tokens,
        channel: Option<String>,
    },
    /// 失効したアクセストークンの自動更新が完了した（セッション継続。channel は変わらない）。
    Refreshed(auth::Tokens),
    Like {
        ok: bool,
        msg: String,
        tokens: Option<auth::Tokens>,
    },
    Failed(String),
}

/// 進行中の login/like タスク。チャネルはタスクごとに生成するため、古い試行の遅延応答が
/// 新しい試行に混入する余地が構造的にない。
struct AuthTask {
    rx: Receiver<AuthMsg>,
}

/// ログイン状態。`tokens`/`channel` はアプリ全体につき 1 つの事実（credentials）。
pub struct Account {
    tokens: Option<auth::Tokens>,
    channel: Option<String>,
    status: String,
    backend: String,
    task: Option<AuthTask>,
    /// トークン自動更新の再試行抑制（失敗直後に毎フレーム更新を撃たないため）。
    refresh_backoff_until: Option<Instant>,
}

/// 呼び出し側（flows の先行形）が反応すべき出来事。auth 内で閉じる処理（トークン保存・
/// status 更新等）は `poll` の中で完結し、ここには現れない。
pub enum AccountEvent {
    LoggedIn,
    /// 失効したアクセストークンの自動更新が完了した。ログインし直しではないので
    /// `LoggedIn` の跨ぎ処理（履歴再送・おすすめ先読み）は不要。保留中の再生の解決と、
    /// 失効中に失敗した取得のやり直しだけを呼び出し側が routing する。
    TokenRefreshed,
    /// ログイン試行が失敗した（`start_like` の失敗は `AuthMsg::Like{ok:false}` で完結するため
    /// 対象外。ログイン待ちで保留していた再生を、匿名のまま解決させる旧 poll_auth の挙動を
    /// 維持するためのイベント）。
    LoginFailed,
}

impl Account {
    pub fn new(backend: String) -> Self {
        Self {
            tokens: None,
            channel: None,
            status: "未ログイン".to_string(),
            backend,
            task: None,
            refresh_backoff_until: None,
        }
    }

    /// 有効なアクセストークン。失効していたら None（失効トークンで叩いても 401 になる
    /// だけなので、未ログインと同じ扱いにする）。自動更新は `ensure_fresh_token` の仕事。
    pub fn token(&self) -> Option<&str> {
        self.tokens
            .as_ref()
            .filter(|t| !t.is_expired())
            .map(|t| t.access_token.as_str())
    }

    pub fn is_busy(&self) -> bool {
        self.task.is_some()
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn channel_name(&self) -> Option<&str> {
        self.channel.as_deref()
    }
}

/// 背景スレッドからの結果を取り込む。跨ぎの仕事（履歴再送・おすすめ先読み・保留 URL の解決）は
/// ここではやらず、`AccountEvent::LoggedIn` を返すだけ（呼び出し側が routing する）。
pub fn poll(a: &mut Account) -> Vec<AccountEvent> {
    let mut events = Vec::new();
    loop {
        // `task.rx` の借用をこの式の中だけに閉じ込める（NLL）。ループ本体で `a.task = None`
        // 等の `&mut a` を自由に使うため。
        let msg = match a.task.as_ref().map(|t| t.rx.try_recv()) {
            Some(Ok(msg)) => msg,
            _ => break,
        };
        match msg {
            AuthMsg::LoggedIn { tokens, channel } => {
                if let Some(rt) = &tokens.refresh_token {
                    auth::save_refresh_token(rt);
                }
                a.tokens = Some(tokens);
                a.channel = channel;
                a.task = None;
                a.status = match &a.channel {
                    Some(name) => format!("ログイン中: {name}"),
                    None => "ログイン済み".to_string(),
                };
                events.push(AccountEvent::LoggedIn);
            }
            AuthMsg::Refreshed(tokens) => {
                a.tokens = Some(tokens);
                a.task = None;
                a.status = match &a.channel {
                    Some(name) => format!("ログイン中: {name}"),
                    None => "ログイン済み".to_string(),
                };
                events.push(AccountEvent::TokenRefreshed);
            }
            AuthMsg::Like { ok, msg, tokens } => {
                if let Some(t) = tokens {
                    a.tokens = Some(t);
                }
                a.task = None;
                a.status = msg;
                let _ = ok;
            }
            AuthMsg::Failed(e) => {
                a.task = None;
                a.status = format!("エラー: {e}");
                events.push(AccountEvent::LoginFailed);
            }
        }
    }
    events
}

/// ログイン（ブラウザで承認 → バックエンドでトークン取得 → チャンネル名取得）を背景で開始。
pub fn start_login(a: &mut Account, waker: &Waker) {
    a.status = "ブラウザで承認してください…".to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    a.task = Some(AuthTask { rx });
    let backend = a.backend.clone();
    let waker = std::sync::Arc::clone(waker);
    std::thread::spawn(move || {
        let result = auth::login(&backend).map(|tokens| {
            let channel = auth::my_channel_title(&tokens.access_token).ok();
            (tokens, channel)
        });
        let _ = match result {
            Ok((tokens, channel)) => tx.send(AuthMsg::LoggedIn { tokens, channel }),
            Err(e) => tx.send(AuthMsg::Failed(e.to_string())),
        };
        waker();
    });
}

/// system: アクセストークンが失効していたら、リフレッシュトークンで背景更新を開始する
/// （ログインセッションの自動継続）。毎ポーリングで呼んでよい冪等な入口 —
/// 有効なうちは何もせず、更新中（is_busy）と失敗直後（バックオフ 30 秒）は再突入しない。
/// 完了は `AccountEvent::TokenRefreshed`、失敗は `AccountEvent::LoginFailed` として届く。
pub fn ensure_fresh_token(a: &mut Account, waker: &Waker) {
    let Some(t) = &a.tokens else { return };
    if !t.is_expired() || a.is_busy() {
        return;
    }
    if a.refresh_backoff_until.is_some_and(|until| Instant::now() < until) {
        return;
    }
    let Some(rt) = t.refresh_token.clone() else { return };
    a.refresh_backoff_until = Some(Instant::now() + Duration::from_secs(30));
    a.status = "セッション更新中…".to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    a.task = Some(AuthTask { rx });
    let backend = a.backend.clone();
    let waker = std::sync::Arc::clone(waker);
    std::thread::spawn(move || {
        let _ = match auth::refresh(&backend, &rt) {
            Ok(tokens) => tx.send(AuthMsg::Refreshed(tokens)),
            Err(e) => tx.send(AuthMsg::Failed(e.to_string())),
        };
        waker();
    });
}

/// 保存済みリフレッシュトークンで自動ログインを背景で開始。
pub fn start_silent_login(a: &mut Account, refresh_token: String, waker: &Waker) {
    a.status = "ログイン情報を復元中…".to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    a.task = Some(AuthTask { rx });
    let backend = a.backend.clone();
    let waker = std::sync::Arc::clone(waker);
    std::thread::spawn(move || {
        let result = auth::refresh(&backend, &refresh_token).map(|tokens| {
            let channel = auth::my_channel_title(&tokens.access_token).ok();
            (tokens, channel)
        });
        let _ = match result {
            Ok((tokens, channel)) => tx.send(AuthMsg::LoggedIn { tokens, channel }),
            Err(e) => tx.send(AuthMsg::Failed(e.to_string())),
        };
        waker();
    });
}

/// 現在の動画に高評価を付ける（必要ならトークンを更新してから）を背景で開始。
pub fn start_like(a: &mut Account, video_id: String, waker: &Waker) {
    let Some(tokens) = a.tokens.clone() else {
        a.status = "先にログインしてください".to_string();
        return;
    };
    a.status = "高評価を送信中…".to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    a.task = Some(AuthTask { rx });
    let backend = a.backend.clone();
    let waker = std::sync::Arc::clone(waker);
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<auth::Tokens> {
            let mut t = tokens;
            if t.is_expired() {
                if let Some(rt) = t.refresh_token.clone() {
                    t = auth::refresh(&backend, &rt)?;
                }
            }
            auth::rate_video(&t.access_token, &video_id, "like")?;
            Ok(t)
        })();
        let _ = match result {
            Ok(t) => tx.send(AuthMsg::Like {
                ok: true,
                msg: "👍 高評価しました".to_string(),
                tokens: Some(t),
            }),
            Err(e) => tx.send(AuthMsg::Like {
                ok: false,
                msg: format!("高評価に失敗: {e}"),
                tokens: None,
            }),
        };
        waker();
    });
}

/// 動画を「後で見る」に保存する（ケバブメニュー）。結果は待たない（fire-and-forget）。
pub fn save_watch_later(token: &str, video_id: String) {
    let access_token = token.to_string();
    std::thread::spawn(move || {
        match crate::yt::subscriptions::add_to_watch_later(&access_token, &video_id) {
            Ok(()) => eprintln!("[menu] 後で見るに保存 ok ({video_id})"),
            Err(e) => eprintln!("[menu] 後で見る保存に失敗: {e:#}"),
        }
    });
}

/// feedbackToken を送信する（興味なし／チャンネルをおすすめに表示しない）。fire-and-forget。
pub fn send_card_feedback(token: &str, feedback_token: String) {
    let access_token = token.to_string();
    std::thread::spawn(move || {
        match crate::yt::subscriptions::send_feedback(&access_token, &feedback_token) {
            Ok(()) => eprintln!("[menu] フィードバック送信 ok"),
            Err(e) => eprintln!("[menu] フィードバック送信に失敗: {e:#}"),
        }
    });
}

/// 現在の URL の動画を再生履歴に載せる（背景スレッドで投げっぱなし）。
/// ログインしていない、または URL から video_id を取れなければ何もしない。
pub fn start_mark_watched_if_logged_in(token: Option<&str>, url: &str) {
    let Some(token) = token else {
        eprintln!("[mark_watched] skip: not logged in (current_url={url})");
        return;
    };
    let Some(video_id) = auth::extract_video_id(url) else {
        eprintln!("[mark_watched] skip: no video_id in url={url}");
        return;
    };
    eprintln!("[mark_watched] spawn for {video_id}");
    let access_token = token.to_string();
    std::thread::spawn(move || {
        match crate::yt::mark_watched::mark_watched(&access_token, &video_id) {
            Ok(_) => eprintln!("[mark_watched] ok ({video_id})"),
            Err(e) => eprintln!("[mark_watched] fail ({video_id}): {e}"),
        }
    });
}
