//! UI 非依存のアプリケーションコア（Controller）。
//!
//! mpv 制御・認証/API 呼び出し・ストリーム解決（native InnerTube）・各種ポーリングなど、描画系（egui/OpenGL）に
//! 依存しない状態とロジックをここに集約する。将来 OpenGL 合成をやめてネイティブ 2D UI に
//! 移行する際も、この Controller をそのまま別フロントエンドから駆動できるようにするのが狙い。

use anyhow::Result;
use std::sync::atomic::AtomicI64;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::event_loop::EventLoopProxy;

use ysl_core::yt::{auth, history, mark_watched, playlist, recommend, resolve, subscriptions};
use ysl_core::{chat, gpu_usage, player};
use crate::{AuthMsg, Codec, Quality, UserEvent};

/// UI 非依存のアプリ状態 + ロジック。
pub struct Controller {
    /// 動画プレイヤー（mpv + 描画先テクスチャを内包）。
    pub player: player::Player,
    pub proxy: EventLoopProxy<UserEvent>,
    /// 背景スレッドがメインループを起こすためのコールバック。lib（ysl-core）は winit を
    /// 知らないため、proxy をこれに包んで各ドメインの `start_*` に渡す。
    pub waker: ysl_core::Waker,
    /// 現在再生中の URL（ブラウザで YouTube を開くナビゲーション等に使う）。
    pub current_url: String,
    /// 画質・コーデック指定（解決器のフォーマット選択に使う）。
    pub quality: Quality,
    pub codec: Codec,
    /// リプレイチャット用: メインスレッドが mpv の time-pos (ms) を継続的に store し、
    /// チャットスレッドが get_live_chat_replay リクエストに乗せる。
    pub player_offset_ms: Arc<AtomicI64>,
    pub backend: String,
    pub load_error: Option<String>,
    /// 現在の再生がライブ配信か（videoDetails.isLive）。時間表示↔ライブボタンの切替に使う。
    pub is_live: bool,
    // --- 認証 / API ---
    pub tokens: Option<auth::Tokens>,
    pub channel: Option<String>,
    pub auth_status: String,
    pub auth_busy: bool,
    pub auth_tx: Sender<AuthMsg>,
    pub auth_rx: Receiver<AuthMsg>,
    // --- ライブチャット ---
    /// 1 動画 : 1 セッション。`None` にする（≠フィールドの手動リセット）ことが「停止」の全て
    /// （`ChatSession::drop` が RAII でポーラーを止める）。
    pub chat: Option<chat::ChatSession>,
    // --- おすすめ動画 ---
    pub recommend_items: Vec<recommend::VideoItem>,
    pub recommend_tx: Sender<recommend::RecommendUpdate>,
    pub recommend_rx: Receiver<recommend::RecommendUpdate>,
    pub recommend_status: String,
    // --- チャンネルアバター（名前→URL キャッシュ）。TV tile がアバターを持たないので
    //     無認証 WEB 検索で名前から補完する。一覧カードの丸アイコン用。 ---
    pub channel_avatars: std::collections::HashMap<String, String>,
    pub avatar_tx: Sender<(String, String)>,
    pub avatar_rx: Receiver<(String, String)>,
    /// 解決を依頼済み（成否問わず）の名前。二重リクエスト防止。
    pub avatar_requested: std::collections::HashSet<String>,
    // --- チャンネルビュー（アバター/名前クリックで開くチャンネルの動画一覧）---
    pub channel_items: Vec<recommend::VideoItem>,
    pub channel_title: String,
    pub channel_busy: bool,
    pub channel_tx: Sender<recommend::RecommendUpdate>,
    pub channel_rx: Receiver<recommend::RecommendUpdate>,
    // --- 登録チャンネル新着 ---
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
    // --- ストリーム解決（native InnerTube 常駐ワーカー）---
    pub resolve_handle: resolve::ResolverHandle,
    pub resolve_rx: Receiver<resolve::ResolveUpdate>,
    pub resolve_busy: bool,
    /// 自動ログイン完了待ちで解決を保留している URL（auth レース対策）。
    /// 起動直後は silent-login が走っており tokens=None のため、ここで解決すると
    /// 匿名扱いになり bot ゲート(LOGIN_REQUIRED)で多くの動画が再生不可になる。
    /// ログイン確定(または失敗)時に poll_auth が取り出して解決する。
    pub pending_resolve: Option<String>,
    /// 並列解決の予備（ローカル中継＝サイドカー）の再生 URL。native の再生が mpv で失敗
    /// （403/開けない）したとき即座に切り替えるために控える（[`resolve::ResolveUpdate::Fallback`]）。
    pub pending_fallback: Option<resolve::Resolved>,
    /// 直近の native ロード時刻。一定時間内に再生が始まらず idle なら失敗とみなす。
    pub native_load_at: Option<Instant>,
    /// native ロード後、再生開始 or 失敗を監視中か（フォールバック起動の対象）。
    pub fallback_armed: bool,
    /// 常時 Some（Windows のみ。他 OS は None）。GPU 使用率を見て mpv の hwdec を切り替える。
    pub gpu_monitor: Option<gpu_usage::Monitor>,
}

impl Controller {
    /// プレイヤー・wake 用 proxy・API バックエンド URL から Controller を構築する。
    /// 各 API 用のチャンネルは内部で生成する。GL 合成版・wid 埋め込み版どちらの
    /// `Player` でも同じく駆動できる（描画方式に依存しない）。
    pub fn new(player: player::Player, proxy: EventLoopProxy<UserEvent>, backend: String) -> Self {
        let (auth_tx, auth_rx) = std::sync::mpsc::channel();
        let (recommend_tx, recommend_rx) = std::sync::mpsc::channel();
        let (avatar_tx, avatar_rx) = std::sync::mpsc::channel();
        let (channel_tx, channel_rx) = std::sync::mpsc::channel();
        let (sub_tx, sub_rx) = std::sync::mpsc::channel();
        let (history_tx, history_rx) = std::sync::mpsc::channel();
        let (playlist_tx, playlist_rx) = std::sync::mpsc::channel();
        let (resolve_tx, resolve_rx) = std::sync::mpsc::channel();
        // lib は winit を知らないため、proxy を Waker（Arc<dyn Fn() + Send + Sync>）に包んで渡す。
        let waker_proxy = proxy.clone();
        let waker: ysl_core::Waker =
            Arc::new(move || { let _ = waker_proxy.send_event(UserEvent::Background); });
        // 解決器ワーカーを起動時に 1 回だけ起動（long-lived = boa/HTTP/base.js を常駐保持）。
        let resolve_handle = resolve::ResolverHandle::spawn(resolve_tx, waker.clone());
        Self {
            player,
            proxy,
            waker,
            current_url: String::new(),
            quality: Quality::Auto,
            codec: Codec::Auto,
            player_offset_ms: Arc::new(AtomicI64::new(0)),
            backend,
            load_error: None,
            is_live: false,
            tokens: None,
            channel: None,
            auth_status: "未ログイン".to_string(),
            auth_busy: false,
            auth_tx,
            auth_rx,
            chat: None,
            recommend_items: Vec::new(),
            recommend_tx,
            recommend_rx,
            recommend_status: String::new(),
            channel_avatars: std::collections::HashMap::new(),
            avatar_tx,
            avatar_rx,
            avatar_requested: std::collections::HashSet::new(),
            channel_items: Vec::new(),
            channel_title: String::new(),
            channel_busy: false,
            channel_tx,
            channel_rx,
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
            resolve_handle,
            resolve_rx,
            resolve_busy: false,
            pending_resolve: None,
            pending_fallback: None,
            native_load_at: None,
            fallback_armed: false,
            gpu_monitor: None,
        }
    }

    /// 動画を読み込む。YouTube URL は背景（常駐ワーカー）で解決してから mpv に渡す。
    pub fn load(&mut self, url: &str) {
        let url = url.trim().to_string();
        if url.is_empty() {
            return;
        }
        self.current_url = url.clone();
        self.load_error = None;
        self.is_live = false; // 解決完了（poll_resolve）で確定する。
        self.pending_resolve = None;
        // 新しい動画。フォールバック監視状態をリセット。
        self.pending_fallback = None;
        self.fallback_armed = false;
        self.native_load_at = None;

        // ログイン済みなら再生履歴に載せる。CLI 引数経由の起動直後は auto-login が
        // 完了する前にここに来るため tokens=None になりがちで、その場合は
        // poll_auth で LoggedIn を受け取った時点で履歴に載せ直す。
        self.start_mark_watched_if_logged_in();

        if !resolve::is_youtube_url(&url) {
            // YouTube 以外の URL（直リンク等）はそのまま mpv に渡す。
            self.mpv_loadfile(&url, None, None);
            return;
        }

        // 自動ログイン中（tokens 未確定）は解決を保留する。ここで匿名解決すると
        // bot ゲート(LOGIN_REQUIRED)で members/年齢制限はもちろん、多くの通常動画まで
        // 再生不可になる。ログイン確定後に access_token 付きで解決すれば回避できる。
        if self.tokens.is_none() && self.auth_busy {
            self.pending_resolve = Some(url);
        } else {
            self.start_resolve(url);
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

    /// 解決を常駐ワーカーに依頼する。ログイン中なら access_token を渡し、
    /// members 限定/年齢制限も解錠できるようにする（M17）。
    pub fn start_resolve(&mut self, url: String) {
        self.resolve_busy = true;
        self.resolve_handle.request(resolve::ResolveRequest {
            url,
            quality: self.quality,
            codec: self.codec,
            access_token: self.tokens.as_ref().map(|t| t.access_token.clone()),
        });
    }

    /// 解決結果を取り込み、mpv に loadfile する。
    pub fn poll_resolve(&mut self) {
        while let Ok(update) = self.resolve_rx.try_recv() {
            match update {
                resolve::ResolveUpdate::Ready(r) => {
                    // URL が取れ次第すぐ再生（タイトルは後追いの Meta で反映）。
                    self.mpv_loadfile(&r.video_url, r.audio_url.as_deref(), None);
                    // この再生が mpv で失敗したら予備（Fallback）へ切替えるため監視を始める。
                    self.native_load_at = Some(Instant::now());
                    self.fallback_armed = true;
                    self.pending_fallback = None;
                }
                resolve::ResolveUpdate::Fallback(r) => {
                    // 並列に用意された予備（ローカル中継）。再生失敗時まで控える。
                    self.pending_fallback = Some(r);
                }
                resolve::ResolveUpdate::Meta { title, is_live } => {
                    self.resolve_busy = false;
                    self.is_live = is_live;
                    if let Some(t) = title {
                        self.player.set_force_media_title(&t);
                    }
                }
                resolve::ResolveUpdate::Error(e) => {
                    self.resolve_busy = false;
                    self.load_error = Some(e.clone());
                    eprintln!("resolve failed: {e}");
                }
            }
        }
    }

    /// native 再生が mpv で失敗（403/開けない）していないか監視し、失敗していれば並列に用意した
    /// 予備（ローカル中継＝サイドカー）へ即切替する。メインループから毎ティック呼ぶ。
    pub fn check_playback_fallback(&mut self) {
        if !self.fallback_armed {
            return;
        }
        // 再生が始まっていれば（time-pos が進めば）監視終了＝native 成功。
        if self.player.time_pos() > 0.5 {
            self.fallback_armed = false;
            return;
        }
        // ロード直後はバッファリング/起動の猶予を与える。
        match self.native_load_at {
            Some(at) if at.elapsed() >= Duration::from_secs(3) => {}
            _ => return,
        }
        // ファイル未ロードのまま idle = native はそのストリームを開けなかった（403 等）。
        // 予備が届いていれば中継へ切替える（届くまでは待つ）。
        if self.player.idle_active() {
            if let Some(fb) = self.pending_fallback.take() {
                eprintln!("[fallback] native 再生失敗 → ローカル中継(サイドカー)へ切替");
                self.fallback_armed = false;
                self.mpv_loadfile(&fb.video_url, fb.audio_url.as_deref(), None);
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
                    // ログイン確定＝おすすめ（ホームフィード）を先読みしておく（動画非依存）。
                    self.start_recommend();
                    // ログイン待ちで保留していた動画を、access_token 付きで解決開始する。
                    if let Some(url) = self.pending_resolve.take() {
                        self.start_resolve(url);
                    }
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
                    // ログインに失敗しても、保留中の動画は匿名で解決を試みる（最善努力）。
                    if let Some(url) = self.pending_resolve.take() {
                        self.start_resolve(url);
                    }
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

    /// チャット更新を取り込む。NotLive を受けたらセッションを破棄する（Drop がポーラーを止める）。
    pub fn poll_chat(&mut self) {
        if let Some(session) = self.chat.as_mut() {
            if !chat::poll(session) {
                self.chat = None;
            }
        }
    }

    /// ライブチャットのポーリングを背景スレッドで開始する。
    /// 古いセッションを破棄（= 停止）してから新しいセッションに差し替える。
    pub fn start_chat(&mut self, video_id: String) {
        let offset = Arc::clone(&self.player_offset_ms);
        self.chat = Some(chat::start(video_id, offset, &self.waker));
    }

    /// おすすめ動画の更新を取り込む。
    pub fn poll_recommend(&mut self) {
        while let Ok(update) = self.recommend_rx.try_recv() {
            match update {
                recommend::RecommendUpdate::Items(items) => {
                    self.recommend_status = format!("おすすめ ({} 件)", items.len());
                    let names: Vec<String> = items.iter().map(|v| v.channel.clone()).collect();
                    self.recommend_items = items;
                    self.request_channel_avatars(names);
                }
                recommend::RecommendUpdate::Error(e) => {
                    self.recommend_status = format!("取得エラー: {e}");
                }
            }
        }
    }

    /// チャンネルビュー（開いたチャンネルの動画一覧）の更新を取り込む。
    pub fn poll_channel(&mut self) {
        while let Ok(update) = self.channel_rx.try_recv() {
            self.channel_busy = false;
            match update {
                recommend::RecommendUpdate::Items(items) => {
                    let names: Vec<String> = items.iter().map(|v| v.channel.clone()).collect();
                    self.channel_items = items;
                    self.request_channel_avatars(names);
                }
                recommend::RecommendUpdate::Error(_) => {}
            }
        }
    }

    /// チャンネル名からそのチャンネルの動画一覧を背景取得する（名前→channelId→browse）。
    pub fn open_channel(&mut self, name: String) {
        self.channel_title = name.clone();
        self.channel_items.clear();
        self.channel_busy = true;
        let tx = self.channel_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let result = match subscriptions::fetch_channel_id(&name) {
                Some(id) => recommend::fetch_channel_videos(&id)
                    .map_err(|e| e.to_string()),
                None => Err(format!("チャンネルが見つかりません: {name}")),
            };
            let _ = tx.send(match result {
                Ok(items) => recommend::RecommendUpdate::Items(items),
                Err(e) => recommend::RecommendUpdate::Error(e),
            });
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// 実 channelId(UC...) からそのチャンネルの動画一覧を背景取得する（名前検索を経由しない、
    /// より確実な経路。ケバブメニューの「チャンネルへ」が実IDを持つ場合に使う）。
    pub fn open_channel_by_id(&mut self, id: String, title: String) {
        self.channel_title = title;
        self.channel_items.clear();
        self.channel_busy = true;
        let tx = self.channel_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let result = recommend::fetch_channel_videos(&id).map_err(|e| e.to_string());
            let _ = tx.send(match result {
                Ok(items) => recommend::RecommendUpdate::Items(items),
                Err(e) => recommend::RecommendUpdate::Error(e),
            });
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// 動画を「後で見る」に保存する（ケバブメニュー）。結果は待たない（fire-and-forget、
    /// 失敗してもオーバーレイをブロックしない。将来的にトースト等で通知してもよい）。
    pub fn save_watch_later(&self, video_id: String) {
        let Some(tokens) = &self.tokens else { return };
        let access_token = tokens.access_token.clone();
        std::thread::spawn(move || match subscriptions::add_to_watch_later(&access_token, &video_id) {
            Ok(()) => eprintln!("[menu] 後で見るに保存 ok ({video_id})"),
            Err(e) => eprintln!("[menu] 後で見る保存に失敗: {e:#}"),
        });
    }

    /// feedbackToken を送信する（興味なし／チャンネルをおすすめに表示しない）。fire-and-forget。
    pub fn send_card_feedback(&self, token: String) {
        let Some(tokens) = &self.tokens else { return };
        let access_token = tokens.access_token.clone();
        std::thread::spawn(move || match subscriptions::send_feedback(&access_token, &token) {
            Ok(()) => eprintln!("[menu] フィードバック送信 ok"),
            Err(e) => eprintln!("[menu] フィードバック送信に失敗: {e:#}"),
        });
    }

    /// チャンネルアバターの解決結果を取り込む（名前→URL）。
    pub fn poll_channel_avatars(&mut self) {
        while let Ok((name, url)) = self.avatar_rx.try_recv() {
            self.channel_avatars.insert(name, url);
        }
    }

    /// 未解決のチャンネル名のアバターを無認証 WEB 検索で背景解決する（1 スレッドで順次）。
    /// TV tile がアバターを持たないための補完。結果は avatar_tx 経由でメインへ返す。
    pub fn request_channel_avatars(&mut self, names: Vec<String>) {
        let mut todo = Vec::new();
        for name in names {
            if name.is_empty() || self.avatar_requested.contains(&name) {
                continue;
            }
            self.avatar_requested.insert(name.clone());
            todo.push(name);
        }
        if todo.is_empty() {
            return;
        }
        let tx = self.avatar_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            for name in todo {
                if let Some(url) = subscriptions::fetch_channel_avatar(&name) {
                    let _ = tx.send((name, url));
                    let _ = proxy.send_event(UserEvent::Background);
                }
            }
        });
    }

    /// おすすめ（ホームフィード FEwhat_to_watch）を背景スレッドで取得する。要ログイン。
    /// 動画再生とは無関係で、ログイン確定時や一覧を開いた時に呼ぶ。
    pub fn start_recommend(&mut self) {
        let Some(tokens) = &self.tokens else {
            self.recommend_status = "先にログインしてください".to_string();
            return;
        };
        self.recommend_items.clear();
        self.recommend_status = "おすすめ取得中…".to_string();
        let access_token = tokens.access_token.clone();
        let tx = self.recommend_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            recommend::fetch_home_feed(&access_token, &tx);
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// 登録チャンネルタブの更新を取り込む（新着フィード + チャンネルリスト）。
    pub fn poll_subs(&mut self) {
        while let Ok(update) = self.sub_rx.try_recv() {
            match update {
                subscriptions::SubUpdate::Feed(items) => {
                    self.sub_busy = false;
                    self.sub_status = "新着".to_string();
                    let names: Vec<String> = items.iter().map(|v| v.channel.clone()).collect();
                    self.sub_feed = items;
                    self.request_channel_avatars(names);
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

        // 新着フィード（InnerTube FEsubscriptions）。
        let tx = self.sub_tx.clone();
        let proxy = self.proxy.clone();
        let token = tokens.access_token.clone();
        std::thread::spawn(move || {
            subscriptions::fetch_subscription_feed(&token, &tx);
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
                    let names: Vec<String> = items.iter().map(|v| v.channel.clone()).collect();
                    self.history_items = items;
                    self.request_channel_avatars(names);
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

    /// ライブチャットのポーリングを停止する。
    pub fn stop_chat(&mut self) {
        self.chat = None;
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
