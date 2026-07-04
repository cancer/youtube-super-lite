//! UI 非依存のアプリケーションコア（Controller）。
//!
//! mpv 制御・認証/API 呼び出し・ストリーム解決（native InnerTube）・各種ポーリングなど、描画系（egui/OpenGL）に
//! 依存しない状態とロジックをここに集約する。将来 OpenGL 合成をやめてネイティブ 2D UI に
//! 移行する際も、この Controller をそのまま別フロントエンドから駆動できるようにするのが狙い。

use std::sync::atomic::AtomicI64;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::event_loop::EventLoopProxy;

use ysl_core::yt::{history, recommend, resolve, subscriptions};
use ysl_core::{account, chat, content, gpu_usage, player};
use crate::{Codec, Quality, UserEvent};

/// UI 非依存のアプリ状態 + ロジック。
pub struct Controller {
    /// 動画プレイヤー（mpv + 描画先テクスチャを内包）。
    pub player: player::Player,
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
    pub load_error: Option<String>,
    /// 現在の再生がライブ配信か（videoDetails.isLive）。時間表示↔ライブボタンの切替に使う。
    pub is_live: bool,
    // --- 認証 / API ---
    pub account: account::Account,
    // --- ライブチャット ---
    /// 1 動画 : 1 セッション。`None` にする（≠フィールドの手動リセット）ことが「停止」の全て
    /// （`ChatSession::drop` が RAII でポーラーを止める）。
    pub chat: Option<chat::ChatSession>,
    // --- コンテンツ一覧（おすすめ/チャンネルビュー/登録チャンネル新着/再生履歴/再生リスト/
    //     アバター）。互いに独立した状態機械の集まりなので束ねる型を作らず個別に持つ。 ---
    pub recommend: content::Feed<recommend::VideoItem>,
    pub channel_view: content::ChannelView,
    pub subs: content::Feed<subscriptions::SubVideo>,
    pub history: content::Feed<history::HistoryItem>,
    pub playlist: content::Playlist,
    pub avatars: content::AvatarCache,
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
        let (resolve_tx, resolve_rx) = std::sync::mpsc::channel();
        // lib は winit を知らないため、proxy を Waker（Arc<dyn Fn() + Send + Sync>）に包んで渡す。
        let waker_proxy = proxy.clone();
        let waker: ysl_core::Waker =
            Arc::new(move || { let _ = waker_proxy.send_event(UserEvent::Background); });
        // 解決器ワーカーを起動時に 1 回だけ起動（long-lived = boa/HTTP/base.js を常駐保持）。
        let resolve_handle = resolve::ResolverHandle::spawn(resolve_tx, waker.clone());
        Self {
            player,
            waker,
            current_url: String::new(),
            quality: Quality::Auto,
            codec: Codec::Auto,
            player_offset_ms: Arc::new(AtomicI64::new(0)),
            load_error: None,
            is_live: false,
            account: account::Account::new(backend),
            chat: None,
            recommend: content::Feed::new("recommend"),
            channel_view: content::ChannelView::new(),
            subs: content::Feed::new("subs"),
            history: content::Feed::new("history"),
            playlist: content::Playlist::new(),
            avatars: content::AvatarCache::new(),
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
        if self.account.token().is_none() && self.account.is_busy() {
            self.pending_resolve = Some(url);
        } else {
            self.start_resolve(url);
        }
    }

    /// 現在の `current_url` の動画を再生履歴に載せる（背景スレッドで投げっぱなし）。
    /// ログインしていない、または URL から video_id を取れなければ何もしない。
    pub fn start_mark_watched_if_logged_in(&self) {
        account::start_mark_watched_if_logged_in(self.account.token(), &self.current_url);
    }

    /// 解決を常駐ワーカーに依頼する。ログイン中なら access_token を渡し、
    /// members 限定/年齢制限も解錠できるようにする（M17）。
    pub fn start_resolve(&mut self, url: String) {
        self.resolve_busy = true;
        self.resolve_handle.request(resolve::ResolveRequest {
            url,
            quality: self.quality,
            codec: self.codec,
            access_token: self.account.token().map(|t| t.to_string()),
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

    /// 背景スレッドからの結果を取り込み、跨ぎイベントを routing する
    /// （D4 で `flows::on_logged_in` に昇格する予定の過渡期の配線）。
    pub fn poll_auth(&mut self) {
        for ev in account::poll(&mut self.account) {
            match ev {
                account::AccountEvent::LoggedIn => {
                    // CLI 引数経由で既に load() を通った動画がここで履歴に載る。
                    self.start_mark_watched_if_logged_in();
                    // ログイン確定＝おすすめ（ホームフィード）を先読みしておく（動画非依存）。
                    self.start_recommend();
                    // ログイン待ちで保留していた動画を、access_token 付きで解決開始する。
                    if let Some(url) = self.pending_resolve.take() {
                        self.start_resolve(url);
                    }
                }
                account::AccountEvent::LoginFailed => {
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
        account::start_login(&mut self.account, &self.waker);
    }

    /// 保存済みリフレッシュトークンで自動ログインを背景で開始。
    pub fn start_silent_login(&mut self, refresh_token: String) {
        account::start_silent_login(&mut self.account, refresh_token, &self.waker);
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
        content::poll_feed(&mut self.recommend, &mut self.avatars, &self.waker);
    }

    /// チャンネルビュー（開いたチャンネルの動画一覧）の更新を取り込む。
    pub fn poll_channel(&mut self) {
        content::poll_channel_view(&mut self.channel_view, &mut self.avatars, &self.waker);
    }

    /// チャンネル名からそのチャンネルの動画一覧を背景取得する（名前→channelId→browse）。
    pub fn open_channel(&mut self, name: String) {
        content::open_channel(&mut self.channel_view, name, &self.waker);
    }

    /// 実 channelId(UC...) からそのチャンネルの動画一覧を背景取得する（名前検索を経由しない、
    /// より確実な経路。ケバブメニューの「チャンネルへ」が実IDを持つ場合に使う）。
    pub fn open_channel_by_id(&mut self, id: String, title: String) {
        content::open_channel_by_id(&mut self.channel_view, id, title, &self.waker);
    }

    /// 動画を「後で見る」に保存する（ケバブメニュー）。結果は待たない（fire-and-forget、
    /// 失敗してもオーバーレイをブロックしない。将来的にトースト等で通知してもよい）。
    pub fn save_watch_later(&self, video_id: String) {
        let Some(token) = self.account.token() else { return };
        account::save_watch_later(token, video_id);
    }

    /// feedbackToken を送信する（興味なし／チャンネルをおすすめに表示しない）。fire-and-forget。
    pub fn send_card_feedback(&self, token: String) {
        let Some(access_token) = self.account.token() else { return };
        account::send_card_feedback(access_token, token);
    }

    /// チャンネルアバターの解決結果を取り込む（名前→URL）。
    pub fn poll_channel_avatars(&mut self) {
        content::poll_avatars(&mut self.avatars);
    }

    /// おすすめ（ホームフィード FEwhat_to_watch）を背景スレッドで取得する。要ログイン。
    /// 動画再生とは無関係で、ログイン確定時や一覧を開いた時に呼ぶ。
    pub fn start_recommend(&mut self) {
        let Some(token) = self.account.token() else { return };
        let token = token.to_string();
        content::start_recommend(&mut self.recommend, &token, &self.waker);
    }

    /// 登録チャンネルタブの更新を取り込む（新着フィード + チャンネルリスト）。
    pub fn poll_subs(&mut self) {
        content::poll_feed(&mut self.subs, &mut self.avatars, &self.waker);
    }

    /// 登録チャンネルタブのデータを背景スレッドで取得する。
    pub fn start_subs(&mut self) {
        let Some(token) = self.account.token() else { return };
        let token = token.to_string();
        content::start_subs(&mut self.subs, &token, &self.waker);
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
        content::poll_feed(&mut self.history, &mut self.avatars, &self.waker);
    }

    /// 再生履歴を背景スレッドで取得する。
    pub fn start_history(&mut self) {
        let Some(token) = self.account.token() else { return };
        let token = token.to_string();
        content::start_history(&mut self.history, &token, &self.waker);
    }

    /// 再生リストの更新を取り込む。
    pub fn poll_playlist(&mut self) {
        content::poll_playlist(&mut self.playlist);
    }

    /// 自分の再生リスト一覧を背景スレッドで取得する。
    pub fn start_playlist_list(&mut self) {
        let Some(token) = self.account.token() else { return };
        let token = token.to_string();
        content::start_playlist_list(&mut self.playlist, &token, &self.waker);
    }

    /// 選択した再生リストの動画一覧を背景スレッドで取得する。
    pub fn start_playlist_items(&mut self, playlist_id: String, title: String) {
        let Some(token) = self.account.token() else { return };
        let token = token.to_string();
        content::start_playlist_items(&mut self.playlist, playlist_id, title, &token, &self.waker);
    }

    /// 再生リスト一覧に戻る（動画一覧を閉じる）。
    pub fn playlist_back_to_lists(&mut self) {
        content::back_to_lists(&mut self.playlist);
    }

    /// ライブチャットのポーリングを停止する。
    pub fn stop_chat(&mut self) {
        self.chat = None;
    }

    /// 現在の動画に高評価を付ける（必要ならトークンを更新してから）を背景で開始。
    pub fn start_like(&mut self, video_id: String) {
        account::start_like(&mut self.account, video_id, &self.waker);
    }

}
