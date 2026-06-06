//! UI 非依存のアプリケーションコア（Controller）。
//!
//! mpv 制御・認証/API 呼び出し・yt-dlp 解決・各種ポーリングなど、描画系（egui/OpenGL）に
//! 依存しない状態とロジックをここに集約する。将来 OpenGL 合成をやめてネイティブ 2D UI に
//! 移行する際も、この Controller をそのまま別フロントエンドから駆動できるようにするのが狙い。

use anyhow::Result;
use std::sync::atomic::AtomicI64;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use winit::event_loop::EventLoopProxy;

use crate::{
    auth, chat, gpu_usage, history, mark_watched, player, playlist, recommend, resolve,
    subscriptions,
};
use crate::{build_ytdlp_format, AuthMsg, Codec, Quality, UserEvent, CHAT_MAX_MESSAGES};

/// UI 非依存のアプリ状態 + ロジック。
pub struct Controller {
    /// 動画プレイヤー（mpv + 描画先テクスチャを内包）。
    pub player: player::Player,
    pub proxy: EventLoopProxy<UserEvent>,
    /// 現在再生中の URL（ブラウザで YouTube を開くナビゲーション等に使う）。
    pub current_url: String,
    /// 画質・コーデック指定（yt-dlp のフォーマット選択に使う）。
    pub quality: Quality,
    pub codec: Codec,
    /// リプレイチャット用: メインスレッドが mpv の time-pos (ms) を継続的に store し、
    /// チャットスレッドが get_live_chat_replay リクエストに乗せる。
    pub player_offset_ms: Arc<AtomicI64>,
    pub backend: String,
    pub load_error: Option<String>,
    // --- 認証 / API ---
    pub tokens: Option<auth::Tokens>,
    pub channel: Option<String>,
    pub auth_status: String,
    pub auth_busy: bool,
    pub auth_tx: Sender<AuthMsg>,
    pub auth_rx: Receiver<AuthMsg>,
    // --- ライブチャット ---
    pub chat_messages: Vec<chat::ChatMessage>,
    pub chat_tx: Sender<chat::ChatUpdate>,
    pub chat_rx: Receiver<chat::ChatUpdate>,
    pub chat_stop: Option<chat::ChatStop>,
    pub chat_status: String,
    pub chat_visible: bool,
    // --- おすすめ動画 ---
    pub recommend_items: Vec<recommend::VideoItem>,
    pub recommend_tx: Sender<recommend::RecommendUpdate>,
    pub recommend_rx: Receiver<recommend::RecommendUpdate>,
    pub recommend_visible: bool,
    pub recommend_status: String,
    // --- 登録チャンネルタブ ---
    /// 左のチャンネルリスト。
    pub sub_channels: Vec<subscriptions::SubChannel>,
    /// 右ペイン既定: 全登録チャンネルの新着フィード。
    pub sub_feed: Vec<subscriptions::SubVideo>,
    pub sub_tx: Sender<subscriptions::SubUpdate>,
    pub sub_rx: Receiver<subscriptions::SubUpdate>,
    pub sub_visible: bool,
    pub sub_status: String,
    pub sub_busy: bool,
    // --- 再生履歴 ---
    pub history_items: Vec<history::HistoryItem>,
    pub history_tx: Sender<history::HistoryUpdate>,
    pub history_rx: Receiver<history::HistoryUpdate>,
    pub history_visible: bool,
    pub history_status: String,
    pub history_busy: bool,
    // --- 再生リスト ---
    pub playlist_lists: Vec<playlist::PlaylistSummary>,
    pub playlist_items: Vec<playlist::PlaylistItem>,
    pub playlist_items_title: String,
    pub playlist_tx: Sender<playlist::PlaylistUpdate>,
    pub playlist_rx: Receiver<playlist::PlaylistUpdate>,
    pub playlist_visible: bool,
    pub playlist_status: String,
    pub playlist_busy: bool,
    // --- チャンネル動画（登録チャンネルから開くアップロード一覧。再生リストではないのでカード UI）---
    pub channel_videos: Vec<playlist::PlaylistItem>,
    pub channel_tx: Sender<playlist::PlaylistUpdate>,
    pub channel_rx: Receiver<playlist::PlaylistUpdate>,
    pub channel_visible: bool,
    pub channel_status: String,
    pub channel_busy: bool,
    // --- ストリーム解決（yt-dlp）---
    pub resolve_tx: Sender<resolve::ResolveUpdate>,
    pub resolve_rx: Receiver<resolve::ResolveUpdate>,
    pub resolve_busy: bool,
    /// 常時 Some（Windows のみ。他 OS は None）。GPU 使用率を見て mpv の hwdec を切り替える。
    pub gpu_monitor: Option<gpu_usage::Monitor>,
}

impl Controller {
    /// プレイヤー・wake 用 proxy・API バックエンド URL から Controller を構築する。
    /// 各 API 用のチャンネルは内部で生成する。GL 合成版・wid 埋め込み版どちらの
    /// `Player` でも同じく駆動できる（描画方式に依存しない）。
    pub fn new(player: player::Player, proxy: EventLoopProxy<UserEvent>, backend: String) -> Self {
        let (auth_tx, auth_rx) = std::sync::mpsc::channel();
        let (chat_tx, chat_rx) = std::sync::mpsc::channel();
        let (recommend_tx, recommend_rx) = std::sync::mpsc::channel();
        let (sub_tx, sub_rx) = std::sync::mpsc::channel();
        let (history_tx, history_rx) = std::sync::mpsc::channel();
        let (playlist_tx, playlist_rx) = std::sync::mpsc::channel();
        let (channel_tx, channel_rx) = std::sync::mpsc::channel();
        let (resolve_tx, resolve_rx) = std::sync::mpsc::channel();
        Self {
            player,
            proxy,
            current_url: String::new(),
            quality: Quality::Auto,
            codec: Codec::Auto,
            player_offset_ms: Arc::new(AtomicI64::new(0)),
            backend,
            load_error: None,
            tokens: None,
            channel: None,
            auth_status: "未ログイン".to_string(),
            auth_busy: false,
            auth_tx,
            auth_rx,
            chat_messages: Vec::new(),
            chat_tx,
            chat_rx,
            chat_stop: None,
            chat_status: String::new(),
            chat_visible: false,
            recommend_items: Vec::new(),
            recommend_tx,
            recommend_rx,
            recommend_visible: false,
            recommend_status: String::new(),
            sub_channels: Vec::new(),
            sub_feed: Vec::new(),
            sub_tx,
            sub_rx,
            sub_visible: false,
            sub_status: String::new(),
            sub_busy: false,
            history_items: Vec::new(),
            history_tx,
            history_rx,
            history_visible: false,
            history_status: String::new(),
            history_busy: false,
            playlist_lists: Vec::new(),
            playlist_items: Vec::new(),
            playlist_items_title: String::new(),
            playlist_tx,
            playlist_rx,
            playlist_visible: false,
            playlist_status: String::new(),
            playlist_busy: false,
            channel_videos: Vec::new(),
            channel_tx,
            channel_rx,
            channel_visible: false,
            channel_status: String::new(),
            channel_busy: false,
            resolve_tx,
            resolve_rx,
            resolve_busy: false,
            gpu_monitor: None,
        }
    }

    /// 動画を読み込む。YouTube URL は背景で yt-dlp 解決してから mpv に渡す。
    pub fn load(&mut self, url: &str) {
        let url = url.trim().to_string();
        if url.is_empty() {
            return;
        }
        self.current_url = url.clone();
        self.load_error = None;

        // ログイン済みなら再生履歴に載せる。CLI 引数経由の起動直後は auto-login が
        // 完了する前にここに来るため tokens=None になりがちで、その場合は
        // poll_auth で LoggedIn を受け取った時点で改めて起動する。
        self.start_mark_watched_if_logged_in();

        if resolve::is_youtube_url(&url) {
            self.start_resolve(url);
        } else {
            // YouTube 以外の URL（直リンク等）はそのまま mpv に渡す。
            self.mpv_loadfile(&url, None, None);
        }
    }

    /// 現在の `current_url` の動画を再生履歴に載せる（背景スレッドで投げっぱなし）。
    /// ログインしていない、または URL から video_id を取れなければ何もしない。
    pub fn start_mark_watched_if_logged_in(&self) {
        let Some(tokens) = self.tokens.as_ref() else {
            eprintln!("[mark_watched] skip: not logged in (current_url={})", self.current_url);
            return;
        };
        let Some(video_id) = auth::extract_video_id(&self.current_url) else {
            eprintln!("[mark_watched] skip: no video_id in url={}", self.current_url);
            return;
        };
        eprintln!("[mark_watched] spawn for {video_id}");
        let access_token = tokens.access_token.clone();
        std::thread::spawn(move || {
            match mark_watched::mark_watched(&access_token, &video_id) {
                Ok(_) => eprintln!("[mark_watched] ok ({video_id})"),
                Err(e) => eprintln!("[mark_watched] fail ({video_id}): {e}"),
            }
        });
    }

    /// yt-dlp による解決を背景スレッドで開始する。
    pub fn start_resolve(&mut self, url: String) {
        self.resolve_busy = true;
        let tx = self.resolve_tx.clone();
        let proxy = self.proxy.clone();
        let format = build_ytdlp_format(self.quality, self.codec);
        std::thread::spawn(move || {
            resolve::resolve(&url, &format, &tx);
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// 解決結果を取り込み、mpv に loadfile する。
    pub fn poll_resolve(&mut self) {
        while let Ok(update) = self.resolve_rx.try_recv() {
            self.resolve_busy = false;
            match update {
                resolve::ResolveUpdate::Ready(r) => {
                    self.mpv_loadfile(
                        &r.video_url,
                        r.audio_url.as_deref(),
                        r.title.as_deref(),
                    );
                }
                resolve::ResolveUpdate::Error(e) => {
                    self.load_error = Some(e.clone());
                    eprintln!("resolve failed: {e}");
                }
            }
        }
    }

    /// Player に解決済み URL を渡して再生開始する。
    pub fn mpv_loadfile(&mut self, video_url: &str, audio_url: Option<&str>, title: Option<&str>) {
        match self.player.loadfile(video_url, audio_url, title) {
            Ok(_) => println!("loadfile: {video_url}"),
            Err(e) => {
                eprintln!("loadfile failed: {e}");
                self.load_error = Some(e.to_string());
            }
        }
    }

    /// 背景スレッドからの結果を取り込む。
    pub fn poll_auth(&mut self) {
        while let Ok(msg) = self.auth_rx.try_recv() {
            match msg {
                AuthMsg::LoggedIn { tokens, channel } => {
                    if let Some(rt) = &tokens.refresh_token {
                        auth::save_refresh_token(rt);
                    }
                    self.tokens = Some(tokens);
                    self.channel = channel;
                    self.auth_busy = false;
                    self.auth_status = match &self.channel {
                        Some(name) => format!("ログイン中: {name}"),
                        None => "ログイン済み".to_string(),
                    };
                    // CLI 引数経由で既に load() を通った動画がここで履歴に載る。
                    self.start_mark_watched_if_logged_in();
                }
                AuthMsg::Like { ok, msg, tokens } => {
                    if let Some(t) = tokens {
                        self.tokens = Some(t);
                    }
                    self.auth_busy = false;
                    self.auth_status = msg;
                    let _ = ok;
                }
                AuthMsg::Failed(e) => {
                    self.auth_busy = false;
                    self.auth_status = format!("エラー: {e}");
                }
            }
        }
    }

    /// ログイン（ブラウザで承認 → バックエンドでトークン取得 → チャンネル名取得）を背景で開始。
    pub fn start_login(&mut self) {
        self.auth_busy = true;
        self.auth_status = "ブラウザで承認してください…".to_string();
        let backend = self.backend.clone();
        let tx = self.auth_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let result = auth::login(&backend).map(|tokens| {
                let channel = auth::my_channel_title(&tokens.access_token).ok();
                (tokens, channel)
            });
            let _ = match result {
                Ok((tokens, channel)) => tx.send(AuthMsg::LoggedIn { tokens, channel }),
                Err(e) => tx.send(AuthMsg::Failed(e.to_string())),
            };
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// 保存済みリフレッシュトークンで自動ログインを背景で開始。
    pub fn start_silent_login(&mut self, refresh_token: String) {
        self.auth_busy = true;
        self.auth_status = "ログイン情報を復元中…".to_string();
        let backend = self.backend.clone();
        let tx = self.auth_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let result = auth::refresh(&backend, &refresh_token).map(|tokens| {
                let channel = auth::my_channel_title(&tokens.access_token).ok();
                (tokens, channel)
            });
            let _ = match result {
                Ok((tokens, channel)) => tx.send(AuthMsg::LoggedIn { tokens, channel }),
                Err(e) => tx.send(AuthMsg::Failed(e.to_string())),
            };
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// チャット更新を取り込む。
    pub fn poll_chat(&mut self) {
        while let Ok(update) = self.chat_rx.try_recv() {
            match update {
                chat::ChatUpdate::Messages(msgs) => {
                    self.chat_messages.extend(msgs);
                    // 上限を超えたら古いメッセージを捨てる。
                    if self.chat_messages.len() > CHAT_MAX_MESSAGES {
                        let excess = self.chat_messages.len() - CHAT_MAX_MESSAGES;
                        self.chat_messages.drain(..excess);
                    }
                    self.chat_status = format!("チャット ({} 件)", self.chat_messages.len());
                }
                chat::ChatUpdate::Error(e) => {
                    self.chat_status = format!("チャットエラー: {e}");
                }
                chat::ChatUpdate::NotLive => {
                    self.chat_status.clear();
                    self.stop_chat();
                }
            }
        }
    }

    /// ライブチャットのポーリングを背景スレッドで開始する。
    pub fn start_chat(&mut self, video_id: String) {
        self.stop_chat();
        self.chat_messages.clear();
        self.chat_status = "チャット接続中…".to_string();
        self.chat_visible = true;

        let (stopper, stop_flag) = chat::ChatStop::new();
        self.chat_stop = Some(stopper);

        let tx = self.chat_tx.clone();
        let proxy = self.proxy.clone();
        let offset = Arc::clone(&self.player_offset_ms);
        std::thread::spawn(move || {
            chat::run_chat_poll(&video_id, &tx, &stop_flag, &offset);
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// おすすめ動画の更新を取り込む。
    pub fn poll_recommend(&mut self) {
        while let Ok(update) = self.recommend_rx.try_recv() {
            match update {
                recommend::RecommendUpdate::Items(items) => {
                    self.recommend_status = format!("おすすめ ({} 件)", items.len());
                    self.recommend_items = items;
                }
                recommend::RecommendUpdate::Error(e) => {
                    self.recommend_status = format!("取得エラー: {e}");
                }
            }
        }
    }

    /// おすすめ動画を背景スレッドで取得する。
    pub fn start_recommend(&mut self, video_id: String) {
        self.recommend_items.clear();
        self.recommend_status = "おすすめ取得中…".to_string();
        let tx = self.recommend_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            recommend::fetch_recommendations(&video_id, &tx);
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// 登録チャンネルタブの更新を取り込む（新着フィード + チャンネルリスト）。
    pub fn poll_subs(&mut self) {
        while let Ok(update) = self.sub_rx.try_recv() {
            match update {
                subscriptions::SubUpdate::Feed(items) => {
                    // 新着フィードの取得完了でスピナーを止める（こちらが右ペイン主役）。
                    self.sub_busy = false;
                    self.sub_status = "新着".to_string();
                    self.sub_feed = items;
                }
                subscriptions::SubUpdate::Channels(channels) => {
                    self.sub_channels = channels;
                }
                subscriptions::SubUpdate::Error(e) => {
                    self.sub_busy = false;
                    self.sub_status = format!("取得エラー: {e}");
                }
            }
        }
    }

    /// 登録チャンネルタブのデータを背景スレッドで取得する。
    /// 新着フィード（右ペイン既定）とチャンネルリスト（左）を並行取得する。
    pub fn start_subs(&mut self) {
        let Some(tokens) = &self.tokens else {
            self.sub_status = "先にログインしてください".to_string();
            return;
        };
        if self.sub_busy {
            return;
        }
        self.sub_busy = true;
        self.sub_status = "新着を取得中…".to_string();
        self.sub_visible = true;

        let access_token = tokens.access_token.clone();

        // 1. 新着フィード（InnerTube FEsubscriptions）。
        let tx = self.sub_tx.clone();
        let proxy = self.proxy.clone();
        let token = access_token.clone();
        std::thread::spawn(move || {
            subscriptions::fetch_subscription_feed(&token, &tx);
            let _ = proxy.send_event(UserEvent::Background);
        });

        // 2. 左のチャンネルリスト（Data API subscriptions.list）。
        let tx = self.sub_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            subscriptions::fetch_subscribed_channels(&access_token, &tx);
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// GPU 使用率監視スレッドからの hwdec 切替決定を取り込んで mpv に反映する。
    pub fn poll_gpu_usage(&mut self) {
        let Some(monitor) = self.gpu_monitor.as_ref() else {
            return;
        };
        while let Some(decision) = monitor.try_recv() {
            match decision {
                gpu_usage::HwdecDecision::UseSoftware => {
                    eprintln!("[auto-hwdec] GPU 高負荷検出 → SW デコードへ切替");
                    self.player.set_hwdec("no");
                }
                gpu_usage::HwdecDecision::UseHardware => {
                    eprintln!("[auto-hwdec] GPU 負荷低下 → HW デコードへ復帰");
                    self.player.set_hwdec("auto");
                }
            }
        }
    }

    /// 再生履歴の更新を取り込む。
    pub fn poll_history(&mut self) {
        while let Ok(update) = self.history_rx.try_recv() {
            self.history_busy = false;
            match update {
                history::HistoryUpdate::Items(items) => {
                    self.history_status = format!("再生履歴 ({} 件)", items.len());
                    self.history_items = items;
                }
                history::HistoryUpdate::Error(e) => {
                    self.history_status = format!("取得エラー: {e}");
                }
            }
        }
    }

    /// 再生履歴を背景スレッドで取得する。
    pub fn start_history(&mut self) {
        let Some(tokens) = &self.tokens else {
            self.history_status = "先にログインしてください".to_string();
            return;
        };
        if self.history_busy {
            return;
        }
        self.history_busy = true;
        self.history_status = "再生履歴を取得中…".to_string();
        self.history_visible = true;

        let access_token = tokens.access_token.clone();
        let tx = self.history_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            history::fetch_history(&access_token, &tx);
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// 再生リストの更新を取り込む。
    pub fn poll_playlist(&mut self) {
        while let Ok(update) = self.playlist_rx.try_recv() {
            self.playlist_busy = false;
            match update {
                playlist::PlaylistUpdate::Playlists(lists) => {
                    self.playlist_status = format!("再生リスト ({} 件)", lists.len());
                    self.playlist_lists = lists;
                    // リスト一覧に戻ったので動画一覧をクリア。
                    self.playlist_items.clear();
                    self.playlist_items_title.clear();
                }
                playlist::PlaylistUpdate::Items { title, items } => {
                    self.playlist_status = format!("{title} ({} 件)", items.len());
                    self.playlist_items_title = title;
                    self.playlist_items = items;
                }
                playlist::PlaylistUpdate::Error(e) => {
                    self.playlist_status = format!("取得エラー: {e}");
                }
            }
        }
    }

    /// 自分の再生リスト一覧を背景スレッドで取得する。
    pub fn start_playlist_list(&mut self) {
        let Some(tokens) = &self.tokens else {
            self.playlist_status = "先にログインしてください".to_string();
            return;
        };
        if self.playlist_busy {
            return;
        }
        self.playlist_busy = true;
        self.playlist_lists.clear();
        self.playlist_items.clear();
        self.playlist_items_title.clear();
        self.playlist_status = "再生リスト取得中…".to_string();
        self.playlist_visible = true;

        let access_token = tokens.access_token.clone();
        let tx = self.playlist_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            playlist::fetch_my_playlists(&access_token, &tx);
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// 選択した再生リストの動画一覧を背景スレッドで取得する。
    pub fn start_playlist_items(&mut self, playlist_id: String, title: String) {
        let Some(tokens) = &self.tokens else {
            return;
        };
        if self.playlist_busy {
            return;
        }
        self.playlist_busy = true;
        self.playlist_status = format!("{title} を読み込み中…");

        let access_token = tokens.access_token.clone();
        let tx = self.playlist_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            playlist::fetch_playlist_items(&access_token, &playlist_id, &title, &tx);
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    pub fn poll_channel(&mut self) {
        while let Ok(update) = self.channel_rx.try_recv() {
            self.channel_busy = false;
            match update {
                playlist::PlaylistUpdate::Items { title, items } => {
                    self.channel_status = title;
                    self.channel_videos = items;
                }
                playlist::PlaylistUpdate::Error(e) => {
                    self.channel_status = format!("取得エラー: {e}");
                }
                // チャンネルのアップロード取得では Playlists は来ない。
                playlist::PlaylistUpdate::Playlists(_) => {}
            }
        }
    }

    /// 登録チャンネルのアップロード動画一覧をカード UI 用に背景スレッドで取得する。
    /// 内部的にはアップロード再生リスト(UU…)を `fetch_playlist_items` で取るが、
    /// これは「再生リスト」ではないので すべて再生/シャッフル は出さず、専用オーバーレイで
    /// カードグリッド表示する。
    pub fn start_channel_uploads(&mut self, uploads_id: String, title: String) {
        let Some(tokens) = &self.tokens else {
            self.channel_status = "先にログインしてください".to_string();
            return;
        };
        if self.channel_busy {
            return;
        }
        self.channel_busy = true;
        self.channel_visible = true;
        self.channel_videos.clear();
        self.channel_status = format!("{title} を読み込み中…");

        let access_token = tokens.access_token.clone();
        let tx = self.channel_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            playlist::fetch_playlist_items(&access_token, &uploads_id, &title, &tx);
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// ライブチャットのポーリングを停止する。
    pub fn stop_chat(&mut self) {
        if let Some(stopper) = self.chat_stop.take() {
            stopper.stop();
        }
    }

    /// 現在の動画に高評価を付ける（必要ならトークンを更新してから）を背景で開始。
    pub fn start_like(&mut self, video_id: String) {
        let Some(tokens) = self.tokens.clone() else {
            self.auth_status = "先にログインしてください".to_string();
            return;
        };
        self.auth_busy = true;
        self.auth_status = "高評価を送信中…".to_string();
        let backend = self.backend.clone();
        let tx = self.auth_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<auth::Tokens> {
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
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

}
