//! ネイティブ版エントリ（`--native`）。OpenGL を一切作らず、mpv を `wid` 経由で
//! D3D11 にウィンドウへ直接描画させる。`NativeRunning`（shell）が `ysl_core` の各ドメイン
//! （`account`/`playback`/`content`/`chat`）を個別フィールドとして所有し、`flows` の跨ぎ
//! system 経由で駆動する（旧 `Controller` は Issue #11 の再編で消滅）。
//!
//! 現状: winit ウィンドウ + 埋め込み mpv + 各ドメイン + キーボード操作 + 各種 poll。
//! ui/ への分割（地雷コードの隔離、Issue #11 PR U）は未着手。

use anyhow::Result;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use ysl_core::yt::chat::ChatRun;
use ysl_core::player::Player;
use ysl_core::yt::{auth, recommend, resolve, subscriptions, history};
use ysl_core::{account, chat, content, flows, playback};
use crate::{Codec, Quality, UserEvent};

/// 一覧の表示ソース。1/2/3 キーで切替。
#[derive(Clone, Copy, PartialEq)]
enum ListSource {
    Subs,
    Recommend,
    History,
    Playlist,
    /// アバター/チャンネル名クリックで開いた、特定チャンネルの動画一覧。
    Channel,
}

/// 全入力系統（オーバーレイ/dev-tools/キーボード）が組み立てて `apply_action` に渡す行動。
/// 「同一アクションの実装が1箇所ずつ」を実現する唯一のエントリポイント（Issue #11 PR B）。
enum UiAction {
    TogglePause,
    /// シーク（0.0..=1.0 の割合。seekable 時のみ。シークバードラッグ用）。
    SeekTo(f64),
    /// シーク（相対、秒。± の量。キーボード/dev-tools の早送り/早戻し用）。
    SeekBy(f64),
    SetVolume(f64),
    VolumeBy(f64),
    ToggleMute,
    LiveEdge,
    CycleQuality,
    CycleCodec,
    Login,
    Like,
    /// URL 欄の内容を再生する（devtools の play_url・キーボードの Enter で使う）。
    PlayUrl(String),
    /// 一覧の行クリック → その video_id を再生する（座席番号(index)ではなく実 ID）。
    Play { video_id: String },
    /// カードのアバター/チャンネル名クリック → 実 channelId（無ければ名前検索）でチャンネルを開く。
    OpenChannel { id: Option<String>, name: String },
    OpenList(ListSource),
    CloseList,
    ToggleList,
    /// 一覧の選択位置を相対移動する（±1、またはグリッドの列数）。
    ListMove { delta: i32 },
    /// 現在の選択行を再生/ドリルする（devtools の list_select・キーボードの Enter）。
    ListSelect,
    /// 一覧の階層/ビューを戻る（チャンネルビュー→おすすめ、再生リスト中身→一覧）。
    ListBack,
    ToggleChat,
    ChatScroll(i32),
    /// チャット文字サイズを相対変更する（± の px）。
    ChatFontBy(f32),
    /// チャット欄の幅を絶対設定する（ウィンドウ幅比 0.15..=0.6。左端ドラッグ用）。
    SetChatWidth(f32),
    /// チャット欄の幅を相対変更する（± の比率。dev-tools の chat_wider/narrower 用）。
    ChatWidthBy(f32),
    SaveWatchLater { video_id: String },
    /// feedbackToken の送信（興味なし／チャンネルをおすすめに表示しない、いずれも同じ処理）。
    Feedback { token: String },
    OpenCardMenu(usize),
    CloseCardMenu,
}

impl From<crate::dcomp_overlay::OverlayAction> for UiAction {
    fn from(a: crate::dcomp_overlay::OverlayAction) -> Self {
        use crate::dcomp_overlay::{ListTab, OverlayAction};
        match a {
            OverlayAction::TogglePause => UiAction::TogglePause,
            OverlayAction::Seek(frac) => UiAction::SeekTo(frac),
            OverlayAction::SetVolume(v) => UiAction::SetVolume(v),
            OverlayAction::VolumeStep(d) => UiAction::VolumeBy(d),
            OverlayAction::ToggleMute => UiAction::ToggleMute,
            OverlayAction::LiveEdge => UiAction::LiveEdge,
            OverlayAction::Like => UiAction::Like,
            OverlayAction::CycleQuality => UiAction::CycleQuality,
            OverlayAction::CycleCodec => UiAction::CycleCodec,
            OverlayAction::Login => UiAction::Login,
            OverlayAction::OpenList(tab) => UiAction::OpenList(match tab {
                ListTab::Recommend => ListSource::Recommend,
                ListTab::Subs => ListSource::Subs,
                ListTab::Playlist => ListSource::Playlist,
                ListTab::History => ListSource::History,
            }),
            OverlayAction::Play { video_id } => UiAction::Play { video_id },
            OverlayAction::OpenChannel { id, name } => UiAction::OpenChannel { id, name },
            OverlayAction::OpenCardMenu(idx) => UiAction::OpenCardMenu(idx),
            OverlayAction::CloseCardMenu => UiAction::CloseCardMenu,
            OverlayAction::SaveWatchLater(video_id) => UiAction::SaveWatchLater { video_id },
            OverlayAction::NotInterested(token) | OverlayAction::NotRecommendChannel(token) => {
                UiAction::Feedback { token }
            }
            OverlayAction::CloseList => UiAction::CloseList,
            OverlayAction::ListScroll(d) => UiAction::ListMove { delta: d },
            OverlayAction::ToggleChat => UiAction::ToggleChat,
            OverlayAction::ChatScroll(d) => UiAction::ChatScroll(d),
            OverlayAction::SetChatWidth(r) => UiAction::SetChatWidth(r as f32),
            OverlayAction::ChatFontDec => UiAction::ChatFontBy(-2.0),
            OverlayAction::ChatFontInc => UiAction::ChatFontBy(2.0),
        }
    }
}

/// `--native` 起動時のアプリケーション。
pub struct NativeApp {
    proxy: EventLoopProxy<UserEvent>,
    initial_url: Option<String>,
    verbose: bool,
    backend: String,
    initial_volume: Option<f64>,
    enable_dev_tools: bool,
    state: Option<NativeRunning>,
}

struct NativeRunning {
    /// ウィンドウは所有権保持のため抱える（drop するとウィンドウが閉じ、mpv の wid も無効になる）。
    #[allow(dead_code)]
    window: Window,
    /// 親ウィンドウの Win32 HWND（i64）。オーバーレイの追従描画に使う。
    parent_wid: i64,
    /// 背景スレッドがメインループを起こすためのコールバック（proxy を包む。lib は winit を知らない）。
    waker: ysl_core::Waker,
    playback: playback::Playback,
    account: account::Account,
    /// 1 動画 : 1 セッション。`None` にする（≠フィールドの手動リセット）ことが「停止」の全て。
    chat: Option<chat::ChatSession>,
    recommend: content::Feed<recommend::VideoItem>,
    channel_view: content::ChannelView,
    subs: content::Feed<subscriptions::SubVideo>,
    history: content::Feed<history::HistoryItem>,
    playlist: content::Playlist,
    avatars: content::AvatarCache,
    /// URL 入力欄の内容（英数字キーで編集、Enter で再生）。URL は空白を含まないため
    /// Space は再生/一時停止に温存できる（フォーカス概念は持たない）。
    url_input: String,
    /// Ctrl 押下状態（Ctrl+V 貼り付け判定用）。
    #[allow(dead_code)]
    ctrl: bool,
    /// 一覧表示中か、選択位置、表示ソース。
    list_open: bool,
    list_sel: usize,
    list_source: ListSource,
    /// ケバブで開いているカードメニューの index（無ければ None）。
    card_menu_open: Option<usize>,
    /// チャット（右パネル）表示中か。
    chat_open: bool,
    /// チャット（コメント）の文字サイズ（px）。UI（A-/A+）で増減する。
    chat_font_px: f32,
    /// チャット欄の幅（ウィンドウ幅比 0.15..=0.6）。左端ドラッグで変更する。
    chat_width_ratio: f32,
    /// チャットのスクロール量（最新から遡ったメッセージ数。0=最新に追従）。
    chat_scroll: usize,
    /// アプリ窓がフォーカスを持っているか。失っている間はオーバーレイを隠す
    /// （他アプリの上にオーバーレイが残らないようにする）。
    focused: bool,
    /// 動画に重ねる透過オーバーレイ（子窓 + DirectComposition）。Windows のみ。
    /// init 失敗時のみ None。
    #[cfg(windows)]
    dcomp_overlay: Option<crate::dcomp_overlay::DcompOverlay>,
    /// 自動非表示用: 最後に操作（マウス移動/キー/クリック）があった時刻。
    #[cfg(windows)]
    last_activity: Instant,
    #[cfg(windows)]
    overlay_visible: bool,
    /// dev-tools（--enable-dev-tools）からの要求受信口。None なら無効。
    devtools_rx: Option<std::sync::mpsc::Receiver<crate::devtools::Command>>,
    /// 保留中のスクリーンショット返信先。前面化＋再描画を待ってからキャプチャするため遅延させる。
    pending_shot: Option<std::sync::mpsc::Sender<Vec<u8>>>,
    /// スクショ前に待つフレーム数（前面化と合成の反映待ち）。
    shot_delay: u32,
    /// 最後に永続化した設定スナップショット（現在値と異なれば保存する）。
    saved_settings: crate::settings::Settings,
    /// 最後に設定を保存した時刻（保存をデバウンスするため）。
    last_settings_save: Instant,
}

impl NativeApp {
    pub fn new(
        proxy: EventLoopProxy<UserEvent>,
        initial_url: Option<String>,
        verbose: bool,
        backend: String,
        initial_volume: Option<f64>,
        enable_dev_tools: bool,
    ) -> Self {
        Self {
            proxy,
            initial_url,
            verbose,
            backend,
            initial_volume,
            enable_dev_tools,
            state: None,
        }
    }

    fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<NativeRunning> {
        let window = event_loop.create_window(
            Window::default_attributes()
                .with_title("YouTube Super Lite (native / D3D11)")
                .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0)),
        )?;

        // winit ウィンドウの HWND を取り出して mpv の wid に渡す（D3D11 埋め込み）。
        let wid = hwnd_of(&window)?;
        let player = Player::new_embedded(wid, self.verbose)?;
        if let Some(v) = self.initial_volume {
            player.set_volume(v);
        }

        // lib は winit を知らないため、proxy を Waker（Arc<dyn Fn() + Send + Sync>）に包んで渡す。
        let waker_proxy = self.proxy.clone();
        let waker: ysl_core::Waker =
            std::sync::Arc::new(move || { let _ = waker_proxy.send_event(UserEvent::Background); });

        let mut playback_state = playback::Playback::new(player, &waker);
        // 外部アプリへ GPU を譲るための GPU 使用率監視（Playback::new が内部で起動する）。
        if playback_state.has_gpu_monitor() {
            eprintln!("[native][auto-hwdec] GPU 使用率の監視を開始");
        }

        let mut account_state = account::Account::new(self.backend.clone());
        let mut chat_state: Option<chat::ChatSession> = None;

        // 保存済みリフレッシュトークンがあれば自動ログイン。
        if let Some(rt) = auth::load_refresh_token() {
            account::start_silent_login(&mut account_state, rt, &waker);
        }

        // CLI で URL 指定があれば再生開始（URL 欄にも反映）。
        let mut url_input = String::new();
        if let Some(url) = self.initial_url.take() {
            url_input = url.clone();
            flows::play_with_chat(&mut playback_state, &mut chat_state, &account_state, &url, &waker);
        }

        // dev-tools HTTP サーバ（--enable-dev-tools）。
        let devtools_rx = if self.enable_dev_tools {
            let (tx, rx) = std::sync::mpsc::channel();
            match crate::devtools::start(tx, self.proxy.clone()) {
                Ok(port) => {
                    eprintln!("[dev-tools] http://127.0.0.1:{port} （/screenshot, /click, /type, /action/<name>）");
                    Some(rx)
                }
                Err(e) => {
                    eprintln!("[dev-tools] 起動失敗: {e:#}");
                    None
                }
            }
        } else {
            None
        };

        // 動画に重ねる透過オーバーレイ（子窓 + DirectComposition）。init 失敗時のみ None。
        #[cfg(windows)]
        let dcomp_overlay = match crate::dcomp_overlay::DcompOverlay::new(wid) {
            Ok(o) => {
                eprintln!("[native] dcomp overlay (子窓+DirectComposition) を使用");
                Some(o)
            }
            Err(e) => {
                eprintln!("[native] dcomp overlay init failed: {e:#}");
                None
            }
        };

        // 前回保存した UI 設定（文字サイズ・チャット幅）を引き継ぐ。
        let settings = crate::settings::load();

        Ok(NativeRunning {
            window,
            parent_wid: wid,
            waker,
            playback: playback_state,
            account: account_state,
            chat: chat_state,
            recommend: content::Feed::new("recommend"),
            channel_view: content::ChannelView::new(),
            subs: content::Feed::new("subs"),
            history: content::Feed::new("history"),
            playlist: content::Playlist::new(),
            avatars: content::AvatarCache::new(),
            url_input,
            ctrl: false,
            list_open: false,
            list_sel: 0,
            list_source: ListSource::Subs,
            card_menu_open: None,
            chat_open: false,
            chat_font_px: settings.chat_font_px,
            chat_width_ratio: settings.chat_width_ratio,
            chat_scroll: 0,
            focused: true,
            #[cfg(windows)]
            dcomp_overlay,
            #[cfg(windows)]
            last_activity: Instant::now(),
            #[cfg(windows)]
            overlay_visible: true,
            devtools_rx,
            pending_shot: None,
            shot_delay: 0,
            saved_settings: settings,
            last_settings_save: Instant::now(),
        })
    }
}

impl NativeRunning {
    /// player への直接操作（pause/seek/volume 等）用。
    fn player(&self) -> &Player {
        self.playback.player()
    }

    fn current_url(&self) -> &str {
        self.playback.current_url()
    }

    fn is_live(&self) -> bool {
        self.playback.is_live()
    }

    fn quality(&self) -> Quality {
        self.playback.quality()
    }

    fn codec(&self) -> Codec {
        self.playback.codec()
    }

    /// 画質を変更し、現在の URL が YouTube なら再解決する（挙動不変。判断のクロス集約は PR B）。
    fn set_quality(&mut self, q: Quality) {
        playback::set_quality(&mut self.playback, q);
        if resolve::is_youtube_url(self.playback.current_url()) {
            let u = self.playback.current_url().to_string();
            playback::start_resolve(&mut self.playback, u, self.account.token());
        }
    }

    /// コーデックを変更し、現在の URL が YouTube なら再解決する。
    fn set_codec(&mut self, c: Codec) {
        playback::set_codec(&mut self.playback, c);
        if resolve::is_youtube_url(self.playback.current_url()) {
            let u = self.playback.current_url().to_string();
            playback::start_resolve(&mut self.playback, u, self.account.token());
        }
    }

    /// チャットパネルの表示トグル（3 入力系統の共通実装。旧ドリフト: devtools/キーボード版は
    /// 固定 0.28・scroll 未リセットだったが、ユーザーが調整した幅を尊重するオーバーレイ版の
    /// 挙動に統一する — issue #11 PR B で明示された唯一の挙動変更）。
    fn toggle_chat(&mut self) {
        self.chat_open = !self.chat_open;
        if self.chat_open {
            self.chat_scroll = 0;
        }
        let m = if self.chat_open { self.chat_width_ratio } else { 0.0 };
        self.player().set_video_margin_right(m as f64);
    }

    /// 相対シーク（秒）。dev-tools の seek_fwd/seek_back・キーボードの ←→ で使う。
    fn seek_by(&mut self, secs: f64) {
        self.player().seek_relative(secs);
    }

    /// チャット欄の幅を相対変更する。dev-tools の chat_wider/chat_narrower で使う。
    fn chat_width_by(&mut self, delta: f32) {
        self.chat_width_ratio = (self.chat_width_ratio + delta).clamp(0.15, 0.6);
        if self.chat_open {
            self.player().set_video_margin_right(self.chat_width_ratio as f64);
        }
    }

    /// 現在の一覧ソースの (ヘッダ, カード配列) を返す。
    ///
    /// カードの title/channel/thumb/id は現行データ源から常に埋まる。avatar/duration/live/meta/
    /// verified は `recommend::VideoItem`（おすすめ）では常に埋まるが、subs/history はまだ
    /// パース未対応で既定値のまま。
    /// チャンネル名から解決済みアバター URL を引く（未解決なら空＝プレースホルダ円）。
    fn avatar_for(&self, channel: &str) -> String {
        self.avatars.url_for(channel).unwrap_or_default().to_string()
    }

    fn list_rows(&self) -> (String, Vec<crate::dcomp_overlay::Card>) {
        use crate::dcomp_overlay::Card;
        let nav = "  （1新着 2おすすめ 3履歴 4リスト / ↑↓ 選択 / Enter 決定 / Backspace 戻る / Tab・Esc 閉じる）";
        let video_card = |title: &str, channel: &str, thumb: String, id: &str| Card {
            id: id.to_string(),
            title: title.to_string(),
            channel: channel.to_string(),
            thumb,
            ..Card::default()
        };
        let (base, items): (String, Vec<Card>) = match self.list_source {
            ListSource::Subs => (
                "登録チャンネルの新着".to_string(),
                self
                    .subs
                    .items()
                    .iter()
                    .map(|v| Card {
                        id: v.video_id.clone(),
                        title: v.title.clone(),
                        channel: v.channel.clone(),
                        thumb: v.thumbnail.clone(),
                        avatar: self.avatar_for(&v.channel),
                        duration: v.duration,
                        live: v.live,
                        meta: v.meta.clone(),
                        menu: v.menu.clone(),
                        ..Card::default()
                    })
                    .collect(),
            ),
            ListSource::Recommend => (
                "おすすめ".to_string(),
                self
                    .recommend
                    .items()
                    .iter()
                    .map(|v| Card {
                        id: v.video_id.clone(),
                        title: v.title.clone(),
                        channel: v.channel.clone(),
                        thumb: v.thumbnail.clone(),
                        avatar: self.avatar_for(&v.channel),
                        duration: v.duration,
                        live: v.live,
                        meta: v.meta.clone(),
                        verified: v.verified,
                        menu: v.menu.clone(),
                    })
                    .collect(),
            ),
            ListSource::History => (
                "再生履歴".to_string(),
                self
                    .history
                    .items()
                    .iter()
                    .map(|v| Card {
                        id: v.video_id.clone(),
                        title: v.title.clone(),
                        channel: v.channel.clone(),
                        thumb: v.thumbnail.clone(),
                        avatar: self.avatar_for(&v.channel),
                        duration: v.duration,
                        live: v.live,
                        meta: v.meta.clone(),
                        menu: v.menu.clone(),
                        ..Card::default()
                    })
                    .collect(),
            ),
            ListSource::Channel => (
                format!("{} の動画", self.channel_view.title()),
                self
                    .channel_view
                    .items()
                    .iter()
                    .map(|v| Card {
                        id: v.video_id.clone(),
                        title: v.title.clone(),
                        channel: v.channel.clone(),
                        thumb: v.thumbnail.clone(),
                        avatar: self.avatar_for(&v.channel),
                        duration: v.duration,
                        live: v.live,
                        meta: v.meta.clone(),
                        menu: v.menu.clone(),
                        ..Card::default()
                    })
                    .collect(),
            ),
            ListSource::Playlist => {
                if self.playlist.is_items_view() {
                    // 2 階層目: 選択した再生リストの中身（動画）。
                    let rows = self
                        .playlist
                        .items()
                        .iter()
                        .map(|v| video_card(&v.title, &v.channel, String::new(), &v.video_id))
                        .collect();
                    (
                        format!("再生リスト: {}", self.playlist.items_title()),
                        rows,
                    )
                } else {
                    // 1 階層目: 再生リスト一覧（Enter で中身を開く）。件数を meta/channel に。
                    let rows = self
                        .playlist
                        .lists()
                        .iter()
                        .map(|p| Card {
                            id: p.playlist_id.clone(),
                            title: p.title.clone(),
                            channel: format!("{} 件", p.item_count),
                            meta: Some(format!("{} 件", p.item_count)),
                            ..Card::default()
                        })
                        .collect();
                    ("再生リスト".to_string(), rows)
                }
            }
        };
        (format!("{base}{nav}"), items)
    }

    /// 現在の一覧ソースが未取得なら取得を開始する。
    /// おすすめ（ホームフィード）はログイン時に先読みするが、未取得なら開いた時にも取得する。
    fn ensure_source_fetched(&mut self) {
        match self.list_source {
            ListSource::Subs => {
                if self.subs.items().is_empty() && !self.subs.is_busy() {
                    self.start_subs();
                }
            }
            ListSource::History => {
                if self.history.items().is_empty() && !self.history.is_busy() {
                    self.start_history();
                }
            }
            ListSource::Playlist => {
                if self.playlist.lists().is_empty()
                    && self.playlist.items().is_empty()
                    && !self.playlist.is_busy()
                {
                    self.start_playlist_list();
                }
            }
            ListSource::Recommend => {
                if self.recommend.items().is_empty() && self.account.token().is_some() {
                    self.start_recommend();
                }
            }
            // チャンネルビューは open_channel で取得済み。ここでは何もしない。
            ListSource::Channel => {}
        }
    }

    /// 背景スレッド（認証/API/解決）の結果を取り込む。proxy 起床時に呼ぶ。
    fn poll_all(&mut self) {
        // リプレイチャット用に再生位置を共有。
        self.playback
            .player_offset_ms()
            .store((self.playback.player().time_pos() * 1000.0) as i64, Ordering::Relaxed);
        self.poll_auth();
        self.poll_chat();
        content::poll_feed(&mut self.recommend, &mut self.avatars, &self.waker);
        content::poll_feed(&mut self.subs, &mut self.avatars, &self.waker);
        content::poll_feed(&mut self.history, &mut self.avatars, &self.waker);
        content::poll_channel_view(&mut self.channel_view, &mut self.avatars, &self.waker);
        content::poll_avatars(&mut self.avatars);
        content::poll_playlist(&mut self.playlist);
        playback::poll_gpu(&mut self.playback);
        playback::poll_resolve(&mut self.playback);
        // native 直 URL が mpv で再生失敗していれば、並列に用意した中継(サイドカー)へ切替える。
        playback::check_fallback(&mut self.playback);
    }

    /// 背景スレッドからの結果を取り込み、跨ぎイベントを routing する（flows::on_logged_in）。
    fn poll_auth(&mut self) {
        for ev in account::poll(&mut self.account) {
            match ev {
                account::AccountEvent::LoggedIn => {
                    flows::on_logged_in(&mut self.playback, &self.account, &mut self.recommend, &self.waker);
                }
                account::AccountEvent::LoginFailed => {
                    // ログインに失敗しても、保留中の動画は匿名で解決を試みる（最善努力）。
                    if let Some(url) = playback::take_pending(&mut self.playback) {
                        playback::start_resolve(&mut self.playback, url, None);
                    }
                }
            }
        }
    }

    /// チャット更新を取り込む。NotLive を受けたらセッションを破棄する（Drop がポーラーを止める）。
    fn poll_chat(&mut self) {
        if let Some(session) = self.chat.as_mut() {
            if !chat::poll(session) {
                self.chat = None;
            }
        }
    }

    /// 再生開始 + チャット接続（旧 Controller::load + start_chat のコンボ）。
    fn play(&mut self, url: &str) {
        flows::play_with_chat(&mut self.playback, &mut self.chat, &self.account, url, &self.waker);
    }

    /// 動画を「後で見る」に保存する（ケバブメニュー）。fire-and-forget。
    fn save_watch_later(&self, video_id: String) {
        let Some(token) = self.account.token() else { return };
        account::save_watch_later(token, video_id);
    }

    /// feedbackToken を送信する（興味なし／チャンネルをおすすめに表示しない）。fire-and-forget。
    fn send_card_feedback(&self, token: String) {
        let Some(access_token) = self.account.token() else { return };
        account::send_card_feedback(access_token, token);
    }

    /// おすすめ（ホームフィード）を背景スレッドで取得する。要ログイン。
    fn start_recommend(&mut self) {
        let Some(token) = self.account.token() else { return };
        let token = token.to_string();
        content::start_recommend(&mut self.recommend, &token, &self.waker);
    }

    /// 登録チャンネルタブのデータを背景スレッドで取得する。
    fn start_subs(&mut self) {
        let Some(token) = self.account.token() else { return };
        let token = token.to_string();
        content::start_subs(&mut self.subs, &token, &self.waker);
    }

    /// 再生履歴を背景スレッドで取得する。
    fn start_history(&mut self) {
        let Some(token) = self.account.token() else { return };
        let token = token.to_string();
        content::start_history(&mut self.history, &token, &self.waker);
    }

    /// 自分の再生リスト一覧を背景スレッドで取得する。
    fn start_playlist_list(&mut self) {
        let Some(token) = self.account.token() else { return };
        let token = token.to_string();
        content::start_playlist_list(&mut self.playlist, &token, &self.waker);
    }

    /// 選択した再生リストの動画一覧を背景スレッドで取得する。
    fn start_playlist_items(&mut self, playlist_id: String, title: String) {
        let Some(token) = self.account.token() else { return };
        let token = token.to_string();
        content::start_playlist_items(&mut self.playlist, playlist_id, title, &token, &self.waker);
    }

    /// 再生リスト一覧に戻る（動画一覧を閉じる）。
    fn playlist_back_to_lists(&mut self) {
        content::back_to_lists(&mut self.playlist);
    }

    /// ログイン（ブラウザで承認 → バックエンドでトークン取得 → チャンネル名取得）を背景で開始。
    fn start_login(&mut self) {
        account::start_login(&mut self.account, &self.waker);
    }

    /// 現在の動画に高評価を付ける（必要ならトークンを更新してから）を背景で開始。
    fn start_like(&mut self, video_id: String) {
        account::start_like(&mut self.account, video_id, &self.waker);
    }

    /// ライブチャットのポーリングを停止する。
    fn stop_chat(&mut self) {
        self.chat = None;
    }

    /// チャンネル名からそのチャンネルの動画一覧を背景取得する（名前→channelId→browse）。
    fn open_channel(&mut self, name: String) {
        content::open_channel(&mut self.channel_view, name, &self.waker);
    }

    /// 実 channelId(UC...) からそのチャンネルの動画一覧を背景取得する。
    fn open_channel_by_id(&mut self, id: String, title: String) {
        content::open_channel_by_id(&mut self.channel_view, id, title, &self.waker);
    }

    /// dev-tools（--enable-dev-tools）からの要求を処理する。毎フレーム呼ぶ。
    fn poll_devtools(&mut self) {
        use crate::devtools::Command;
        // 借用を切るため先に集めてから処理する。
        let cmds: Vec<Command> = match &self.devtools_rx {
            Some(rx) => rx.try_iter().collect(),
            None => return,
        };
        for cmd in cmds {
            match cmd {
                Command::Screenshot(reply) => {
                    // ウィンドウを前面化し、オーバーレイ込みの合成が画面に反映されてから
                    // （数フレーム後に）キャプチャする。
                    #[cfg(windows)]
                    unsafe {
                        use windows::Win32::Foundation::HWND;
                        use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
                        let _ = SetForegroundWindow(HWND(self.parent_wid as *mut core::ffi::c_void));
                    }
                    self.pending_shot = Some(reply);
                    self.shot_delay = 3;
                }
                Command::State(reply) => {
                    let _ = reply.send(self.state_json());
                }
                Command::Action(name, reply) => {
                    let known = self.devtools_action(&name);
                    let _ = reply.send(known);
                }
                Command::Click { x, y, reply } => {
                    #[cfg(windows)]
                    if let Some(ov) = self.dcomp_overlay.as_ref() {
                        ov.inject_click(x, y);
                    }
                    let _ = reply.send(true);
                }
                Command::Type { text, enter, reply } => {
                    for ch in text.chars() {
                        if !ch.is_control() {
                            self.url_input.push(ch);
                        }
                    }
                    if enter {
                        let url = self.url_input.trim().to_string();
                        if !url.is_empty() {
                            self.play(&url);
                        }
                    }
                    let _ = reply.send(true);
                }
            }
        }
    }

    /// dev-tools のアクション名を `UiAction` に変換して `apply_action` へ渡す
    /// （キーボード/オーバーレイの全操作を網羅）。既知なら true。
    fn devtools_action(&mut self, name: &str) -> bool {
        let action = match name {
            "play_pause" => UiAction::TogglePause,
            "seek_fwd" => UiAction::SeekBy(5.0),
            "seek_back" => UiAction::SeekBy(-5.0),
            "live_edge" => UiAction::LiveEdge,
            "vol_up" => UiAction::VolumeBy(5.0),
            "vol_down" => UiAction::VolumeBy(-5.0),
            "mute" => UiAction::ToggleMute,
            "quality_next" => UiAction::CycleQuality,
            "codec_next" => UiAction::CycleCodec,
            "toggle_chat" => UiAction::ToggleChat,
            "chat_font_inc" => UiAction::ChatFontBy(2.0),
            "chat_font_dec" => UiAction::ChatFontBy(-2.0),
            "chat_scroll_up" => UiAction::ChatScroll(3),
            "chat_scroll_down" => UiAction::ChatScroll(-3),
            "chat_wider" => UiAction::ChatWidthBy(0.04),
            "chat_narrower" => UiAction::ChatWidthBy(-0.04),
            "login" => UiAction::Login,
            "like" => UiAction::Like,
            "play_url" => UiAction::PlayUrl(self.url_input.trim().to_string()),
            "toggle_list" => UiAction::ToggleList,
            "close_overlay" => UiAction::CloseList,
            "open_recommend" => UiAction::OpenList(ListSource::Recommend),
            "open_subs" => UiAction::OpenList(ListSource::Subs),
            "open_playlist" => UiAction::OpenList(ListSource::Playlist),
            "open_history" => UiAction::OpenList(ListSource::History),
            "list_up" => UiAction::ListMove { delta: -1 },
            "list_down" => UiAction::ListMove { delta: 1 },
            "list_select" => UiAction::ListSelect,
            "list_back" => UiAction::ListBack,
            _ => return false,
        };
        let known = self.apply_action(action);
        #[cfg(windows)]
        if known {
            self.last_activity = Instant::now();
        }
        known
    }

    /// 文字サイズ・チャット幅に変更があれば保存する。`force` 時はデバウンスを無視（終了時用）。
    fn maybe_save_settings(&mut self, force: bool) {
        let cur = crate::settings::Settings {
            chat_font_px: self.chat_font_px,
            chat_width_ratio: self.chat_width_ratio,
        };
        let changed = cur.chat_font_px != self.saved_settings.chat_font_px
            || cur.chat_width_ratio != self.saved_settings.chat_width_ratio;
        if !changed {
            return;
        }
        if !force && self.last_settings_save.elapsed() < Duration::from_millis(800) {
            return; // デバウンス（ドラッグ中の連続変更で書きすぎない）。
        }
        crate::settings::save(cur);
        self.saved_settings = cur;
        self.last_settings_save = Instant::now();
    }

    /// 現在の UI 状態を JSON 文字列で返す（dev-tools の /state 用）。
    fn state_json(&self) -> String {
        let p = self.player();
        let source = match self.list_source {
            ListSource::Subs => "subs",
            ListSource::Recommend => "recommend",
            ListSource::History => "history",
            ListSource::Playlist => "playlist",
            ListSource::Channel => "channel",
        };
        let logged_in = self.account.channel_name().is_some_and(|c| !c.is_empty());
        let overlay_visible = {
            #[cfg(windows)]
            {
                self.overlay_visible
            }
            #[cfg(not(windows))]
            {
                false
            }
        };
        // z-order 検証用（入力は動かさない読み取り専用）: オーバーレイが実マウス入力を
        // 受けられる位置（兄弟の最前面）にあるか。
        let overlay_is_topmost: Option<bool> = {
            #[cfg(windows)]
            {
                self.dcomp_overlay.as_ref().map(|o| o.is_topmost())
            }
            #[cfg(not(windows))]
            {
                None
            }
        };
        serde_json::json!({
            "current_url": self.current_url(),
            "url_input": self.url_input,
            "paused": p.paused(),
            "time_pos": p.time_pos(),
            "duration": p.duration(),
            "seekable": p.seekable(),
            "is_live": self.is_live(),
            "volume": p.volume(),
            "muted": p.muted(),
            "media_title": p.media_title(),
            "quality": self.quality().label(),
            "codec": self.codec().label(),
            "chat_open": self.chat_open,
            "chat_font_px": self.chat_font_px,
            "chat_width_ratio": self.chat_width_ratio,
            "chat_scroll": self.chat_scroll,
            "chat_available": self.chat.as_ref().is_some_and(|c| c.available()),
            "chat_messages": self.chat.as_ref().map_or(0, |c| c.messages().len()),
            "list_open": self.list_open,
            "list_source": source,
            "list_sel": self.list_sel,
            "list_count": self.list_rows().1.len(),
            "card_menu_open": self.card_menu_open,
            "overlay_is_topmost": overlay_is_topmost,
            "logged_in": logged_in,
            "channel": self.account.channel_name(),
            "auth_status": self.account.status(),
            "focused": self.focused,
            "overlay_visible": overlay_visible,
        })
        .to_string()
    }

    /// 現在のウィンドウ（クライアント領域）を PNG にして返す（取得不可なら空）。
    #[cfg(windows)]
    fn capture_png(&self) -> Vec<u8> {
        unsafe { capture_client_png(self.parent_wid) }.unwrap_or_default()
    }
    #[cfg(not(windows))]
    fn capture_png(&self) -> Vec<u8> {
        Vec::new()
    }

    /// 一覧の行 index から ID を引いて再生する（devtools/キーボードが使う index ベースの入口）。
    /// 描画順の座席番号(index)をここで一度だけ ID に変換し、以降は [`Self::play_by_id`] という
    /// ID ベースの経路に合流させる（オーバーレイの直接クリックと処理を共有する）。
    #[cfg(windows)]
    fn play_list_index(&mut self, idx: usize) {
        if self.list_source == ListSource::Playlist && !self.playlist.is_items_view() {
            if let Some(pl) = self.playlist.lists().get(idx) {
                self.play_by_id(pl.playlist_id.clone());
            }
            return;
        }
        let rows = self.list_rows().1;
        if let Some(card) = rows.get(idx) {
            self.play_by_id(card.id.clone());
        }
    }

    /// 一覧の行の実 ID を再生する（再生リスト 1 階層目なら ID は playlist_id で、中身を開く）。
    /// オーバーレイの直接クリック（`OverlayAction::Play`）と `play_list_index` の共通処理。
    #[cfg(windows)]
    fn play_by_id(&mut self, video_id: String) {
        self.card_menu_open = None;
        if self.list_source == ListSource::Playlist && !self.playlist.is_items_view() {
            // 再生リスト一覧で選択 → その中身を開く（2 階層目へ）。ここでの ID は playlist_id。
            if let Some(pl) = self.playlist.lists().iter().find(|p| p.playlist_id == video_id) {
                let title = pl.title.clone();
                self.list_sel = 0;
                self.start_playlist_items(video_id.clone(), title);
            }
            return;
        }
        let url = format!("https://www.youtube.com/watch?v={video_id}");
        self.list_open = false;
        self.url_input = url.clone();
        self.play(&url);
    }

    /// 3 入力系統（オーバーレイ/dev-tools/キーボード）の合流点。全員がここへ `UiAction` を
    /// 組み立てて渡すだけにする（Issue #11 PR B）。
    /// 戻り値 = 「ユーザー操作があったか」（呼び出し側が last_activity 更新に使う）。
    #[cfg(windows)]
    fn apply_action(&mut self, a: UiAction) -> bool {
        match a {
            UiAction::TogglePause => {
                let p = self.player();
                p.set_paused(!p.paused());
            }
            UiAction::SeekTo(frac) => {
                let p = self.player();
                let dur = p.duration();
                if p.seekable() && dur > 0.0 {
                    p.set_time_pos(frac * dur);
                }
            }
            UiAction::SeekBy(secs) => self.seek_by(secs),
            UiAction::SetVolume(v) => self.player().set_volume(v.clamp(0.0, 130.0)),
            UiAction::VolumeBy(d) => {
                let p = self.player();
                p.set_volume((p.volume() + d).clamp(0.0, 130.0));
            }
            UiAction::ToggleMute => {
                let p = self.player();
                p.set_muted(!p.muted());
            }
            UiAction::LiveEdge => self.player().seek_to_live(),
            UiAction::Like => {
                if let Some(vid) = auth::extract_video_id(&self.current_url()) {
                    self.start_like(vid);
                }
            }
            UiAction::CycleQuality => {
                let all = Quality::ALL;
                let i = all.iter().position(|q| *q == self.quality()).unwrap_or(0);
                self.set_quality(all[(i + 1) % all.len()]);
            }
            UiAction::CycleCodec => {
                let all = Codec::ALL;
                let i = all.iter().position(|c| *c == self.codec()).unwrap_or(0);
                self.set_codec(all[(i + 1) % all.len()]);
            }
            UiAction::Login => {
                if !self.account.is_busy() {
                    self.start_login();
                }
            }
            UiAction::PlayUrl(url) => {
                if !url.is_empty() {
                    self.play(&url);
                }
            }
            UiAction::OpenList(src) => {
                self.list_source = src;
                self.list_open = true;
                self.list_sel = 0;
                self.card_menu_open = None;
                self.ensure_source_fetched();
            }
            UiAction::Play { video_id } => self.play_by_id(video_id),
            UiAction::OpenChannel { id, name } => {
                // 実 channelId があればそれを使い（確実）、無ければ名前検索にフォールバックする。
                if let Some(id) = id {
                    self.open_channel_by_id(id, name);
                    self.list_source = ListSource::Channel;
                    self.list_sel = 0;
                } else if !name.is_empty() {
                    self.open_channel(name);
                    self.list_source = ListSource::Channel;
                    self.list_sel = 0;
                }
                self.card_menu_open = None;
            }
            UiAction::OpenCardMenu(idx) => {
                self.card_menu_open = Some(idx);
            }
            UiAction::CloseCardMenu => {
                self.card_menu_open = None;
            }
            UiAction::SaveWatchLater { video_id } => {
                self.save_watch_later(video_id);
                self.card_menu_open = None;
            }
            UiAction::Feedback { token } => {
                self.send_card_feedback(token);
                self.card_menu_open = None;
            }
            UiAction::CloseList => {
                self.list_open = false;
                self.card_menu_open = None;
            }
            UiAction::ToggleList => {
                self.list_open = !self.list_open;
                self.card_menu_open = None;
                if self.list_open {
                    self.list_sel = 0;
                    self.ensure_source_fetched();
                }
            }
            UiAction::ListMove { delta } => {
                let n = self.list_rows().1.len();
                if n > 0 {
                    let sel = (self.list_sel as i32 + delta).clamp(0, n as i32 - 1);
                    self.list_sel = sel as usize;
                }
            }
            UiAction::ListSelect => {
                self.card_menu_open = None;
                self.play_list_index(self.list_sel);
            }
            UiAction::ListBack => {
                if self.list_source == ListSource::Channel {
                    // チャンネルビューから おすすめ へ戻る。
                    self.list_source = ListSource::Recommend;
                    self.list_sel = 0;
                } else if self.list_source == ListSource::Playlist && self.playlist.is_items_view() {
                    self.playlist_back_to_lists();
                    self.list_sel = 0;
                }
            }
            UiAction::ToggleChat => self.toggle_chat(),
            UiAction::ChatScroll(d) => {
                let max = self.chat.as_ref().map_or(0, |c| c.messages().len()).saturating_sub(1);
                self.chat_scroll = ((self.chat_scroll as i32 + d).max(0) as usize).min(max);
            }
            UiAction::ChatFontBy(d) => {
                self.chat_font_px = (self.chat_font_px + d).clamp(10.0, 28.0);
            }
            UiAction::SetChatWidth(r) => {
                self.chat_width_ratio = r.clamp(0.15, 0.6);
                if self.chat_open {
                    self.player().set_video_margin_right(self.chat_width_ratio as f64);
                }
            }
            UiAction::ChatWidthBy(d) => self.chat_width_by(d),
        }
        true
    }
}

impl ApplicationHandler<UserEvent> for NativeApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match self.init(event_loop) {
            Ok(running) => self.state = Some(running),
            Err(e) => {
                eprintln!("native init failed: {e:#}");
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: UserEvent) {
        // 背景スレッド完了 or mpv 更新で起こされる。結果を取り込む。
        if let Some(state) = &mut self.state {
            state.poll_all();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(_state) = &mut self.state {
            // 背景スレッドの結果を毎フレーム取り込む（チャットのポーリングは proxy を起こさず
            // channel に送るだけなので、ここで定期的に drain しないと反映されない）。
            _state.poll_all();
            // dev-tools 要求（スクショ/操作注入）を処理（クリック注入はこの後の drain で適用）。
            _state.poll_devtools();
            // 文字サイズ・チャット幅の変更をデバウンス保存（次回起動で引き継ぐ）。
            _state.maybe_save_settings(false);
            // オーバーレイの操作適用・自動非表示・定期再描画。
            #[cfg(windows)]
            if _state.dcomp_overlay.is_some() {
                // 新ホスト（子窓+DComp）: クリック適用＋活動記録＋自動非表示＋描画。
                use crate::dcomp_overlay::{Card, ListTab, PlaybackView};
                let actions = _state
                    .dcomp_overlay
                    .as_mut()
                    .map(|o| o.take_actions())
                    .unwrap_or_default();
                for a in actions {
                    if _state.apply_action(a.into()) {
                        _state.last_activity = Instant::now();
                    }
                }
                if _state
                    .dcomp_overlay
                    .as_mut()
                    .map(|o| o.take_moved())
                    .unwrap_or(false)
                {
                    _state.last_activity = Instant::now();
                }
                // 3 秒無操作で帯を隠す（旧版と同じ。一覧/チャットは UI 移植時に条件追加）。
                let list_open = _state.list_open;
                let active = list_open
                    || _state.chat_open
                    || _state.last_activity.elapsed() < Duration::from_secs(3);
                let logged_in = _state.account.channel_name().is_some_and(|c| !c.is_empty());
                let auth_label = if logged_in {
                    format!("👤 {}", _state.account.channel_name().unwrap_or(""))
                } else {
                    format!("🔑 {}", _state.account.status())
                };
                let list_sel = _state.list_sel;
                let (list_header, list_cards): (String, Vec<Card>) = if list_open {
                    _state.list_rows()
                } else {
                    (String::new(), Vec::new())
                };
                // チャット行（dcomp 用に整形。連続テキストは 1 セグメントに統合）。
                let chat_open = _state.chat_open;
                let chat_available = _state.chat.as_ref().is_some_and(|c| c.available());
                let chat_scroll = _state.chat_scroll;
                let chat_width_ratio = _state.chat_width_ratio;
                let chat_lines: Vec<crate::dcomp_overlay::ChatLine> = if chat_open {
                    use crate::dcomp_overlay::{ChatLine as DLine, ChatSeg as DSeg};
                    _state
                        .chat
                        .as_ref()
                        .map(|c| c.messages())
                        .unwrap_or(&[])
                        .iter()
                        .map(|m| {
                            let mut segs: Vec<DSeg> = Vec::new();
                            for r in &m.runs {
                                match r {
                                    ChatRun::Text(t) => {
                                        if let Some(DSeg::Text(last)) = segs.last_mut() {
                                            last.push_str(t);
                                        } else {
                                            segs.push(DSeg::Text(t.clone()));
                                        }
                                    }
                                    ChatRun::Image { alt, url } => {
                                        segs.push(DSeg::Emoji { url: url.clone(), alt: alt.clone() })
                                    }
                                }
                            }
                            DLine { kind: m.kind, author: m.author.clone(), segs }
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                let p = _state.player();
                let view = PlaybackView {
                    paused: p.paused(),
                    pos: p.time_pos(),
                    dur: p.duration(),
                    seekable: p.seekable(),
                    volume: p.volume(),
                    muted: p.muted(),
                    is_live: _state.is_live(),
                    quality: _state.quality().label().to_string(),
                    codec: _state.codec().label().to_string(),
                    url_input: _state.url_input.clone(),
                    auth_label,
                    logged_in,
                    title: p.media_title(),
                    list_open,
                    list_cards,
                    list_sel,
                    list_tab: match _state.list_source {
                        ListSource::Recommend => ListTab::Recommend,
                        ListSource::Subs => ListTab::Subs,
                        ListSource::Playlist => ListTab::Playlist,
                        ListSource::History => ListTab::History,
                        // チャンネルビューはサイドバーの固定ナビではない（強調なし）。
                        ListSource::Channel => ListTab::Recommend,
                    },
                    card_menu_open: _state.card_menu_open,
                    list_header,
                    chat_available,
                    chat_open,
                    chat_lines,
                    chat_scroll,
                    chat_width_ratio,
                    chat_font_px: _state.chat_font_px,
                };
                if let Some(o) = _state.dcomp_overlay.as_mut() {
                    o.render(active, &view);
                }
            }

            // 保留中のスクリーンショット（dcomp/旧 どちらの経路でも）: 前面化＋再描画の
            // 反映を数フレーム待ってからキャプチャ。capture_png は画面 BitBlt なので合成方式に依らない。
            #[cfg(windows)]
            if _state.pending_shot.is_some() {
                if _state.shot_delay == 0 {
                    if let Some(reply) = _state.pending_shot.take() {
                        let _ = reply.send(_state.capture_png());
                    }
                } else {
                    _state.shot_delay -= 1;
                }
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(33),
            ));
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = &mut self.state else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                state.maybe_save_settings(true); // 終了前に確実に保存。
                state.stop_chat();
                self.state = None;
                event_loop.exit();
            }
            WindowEvent::ModifiersChanged(m) => {
                state.ctrl = m.state().control_key();
            }
            // 動画領域（オーバーレイの透過部）の左クリック = 再生/一時停止。
            // コントロール/一覧/チャットのクリックはオーバーレイ窓が捕捉するためここには来ない。
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                state.apply_action(UiAction::TogglePause);
                #[cfg(windows)]
                {
                    state.last_activity = Instant::now();
                }
            }
            // マウスホイールで音量 ±5（動画プレーヤー慣習。バー上に限らず有効）。
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(pos) => pos.y as f32 / 40.0,
                };
                if dy != 0.0 {
                    let step = if dy > 0.0 { 5.0 } else { -5.0 };
                    state.apply_action(UiAction::VolumeBy(step));
                    #[cfg(windows)]
                    {
                        state.last_activity = Instant::now();
                    }
                }
            }
            // 動画領域上のカーソル移動を活動として記録し、コントロールを表示する。
            // winit の CursorMoved はこの窓のクライアント領域にカーソルがある時だけ届く
            // （他窓に遮蔽されていれば届かない）ので、グローバル座標で自窓上かを推測する必要はない。
            // コントロール帯はオーバーレイ窓が手前にいて CursorMoved が来ないため、そちらは
            // about_to_wait で overlay.take_moved() を見て拾う。
            WindowEvent::CursorMoved { .. } => {
                #[cfg(windows)]
                {
                    state.last_activity = Instant::now();
                }
            }
            // フォーカスを失ったらオーバーレイを隠す（他アプリの上に残らないように）。
            WindowEvent::Focused(focused) => {
                state.focused = focused;
                // フォーカス喪失でオーバーレイは隠さない（非アクティブ窓でもチャット等を表示し続ける）。
                // 可視は about_to_wait の active 判定に委ねる。
                #[cfg(windows)]
                if focused {
                    state.last_activity = Instant::now();
                }
            }
            // ウィンドウのリサイズ/移動にオーバーレイを即追従させる
            // （モーダルなドラッグループ中は about_to_wait が止まるため、ここで直接再描画）。
            // 位置追従は follow_wndproc が WM_MOVE で行うので、ここはサイズ追従の再描画のみ。
            // ここで last_activity をリセットしたり強制表示してはいけない——hwdec 切替に伴う
            // VO 再初期化はプログラム的に Resized を連発し、それを操作扱いすると自動非表示が
            // 効かなくなる（デグレ）。可視判定は about_to_wait に一任する。
            WindowEvent::Resized(size) => {
                // 子窓なので位置は OS が追従。サイズだけ合わせる（位置追従コードは不要）。
                #[cfg(windows)]
                if let Some(o) = state.dcomp_overlay.as_mut() {
                    o.resize(size.width as i32, size.height as i32);
                }
                let _ = size;
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if !event.state.is_pressed() {
                    return;
                }
                // Ctrl+修飾キー: L=ログイン, G=高評価, Q=画質切替, C=コーデック切替。
                // 挙動不変のため、旧実装がここで last_activity を更新していなかった点（他の
                // キー入力と異なり早期 return していた）もそのまま踏襲する（Ctrl+V のみ例外）。
                if state.ctrl {
                    if let Key::Character(c) = &event.logical_key {
                        let action = match c.as_str().to_ascii_lowercase().as_str() {
                            "l" => Some(UiAction::Login),
                            "g" => Some(UiAction::Like),
                            "t" => Some(UiAction::ToggleChat),
                            "q" => Some(UiAction::CycleQuality),
                            "c" => Some(UiAction::CycleCodec),
                            // Ctrl + "-" / "+"（"=" も可）: コメント文字サイズ増減。
                            "-" => Some(UiAction::ChatFontBy(-2.0)),
                            "+" | "=" => Some(UiAction::ChatFontBy(2.0)),
                            _ => None,
                        };
                        if let Some(a) = action {
                            state.apply_action(a);
                            return;
                        }
                    }
                }
                // Ctrl+V: クリップボードのテキストを URL 欄へ貼り付け（テキスト編集そのものなので
                // UiAction 化しない）。
                #[cfg(windows)]
                if state.ctrl {
                    if let Key::Character(c) = &event.logical_key {
                        if c.eq_ignore_ascii_case("v") {
                            if let Some(t) = crate::dcomp_overlay::clipboard_text() {
                                for ch in t.chars() {
                                    if !ch.is_control() {
                                        state.url_input.push(ch);
                                    }
                                }
                            }
                            state.last_activity = Instant::now();
                            return;
                        }
                    }
                }
                // Tab: 一覧を開閉。
                if let Key::Named(NamedKey::Tab) = event.logical_key {
                    state.apply_action(UiAction::ToggleList);
                    #[cfg(windows)]
                    {
                        state.last_activity = Instant::now();
                    }
                    return;
                }
                // 一覧表示中はキーをナビゲーション／ソース切替に使う。
                if state.list_open {
                    // グリッドの 1 行移動量＝現在の列数（未描画時は 1）。
                    #[cfg(windows)]
                    let cols = state
                        .dcomp_overlay
                        .as_ref()
                        .map(|o| o.grid_cols())
                        .unwrap_or(1)
                        .max(1) as i32;
                    #[cfg(not(windows))]
                    let cols = 1i32;
                    match &event.logical_key {
                        Key::Named(NamedKey::ArrowUp) => {
                            state.apply_action(UiAction::ListMove { delta: -cols });
                        }
                        Key::Named(NamedKey::ArrowDown) => {
                            state.apply_action(UiAction::ListMove { delta: cols });
                        }
                        Key::Named(NamedKey::ArrowLeft) => {
                            state.apply_action(UiAction::ListMove { delta: -1 });
                        }
                        Key::Named(NamedKey::ArrowRight) => {
                            state.apply_action(UiAction::ListMove { delta: 1 });
                        }
                        Key::Named(NamedKey::Enter) => {
                            // devtools の list_select と同じ経路（旧: ここだけ play_list_index を
                            // 呼ばずインライン再実装していたドリフトを解消。issue #11 PR B）。
                            state.apply_action(UiAction::ListSelect);
                        }
                        Key::Named(NamedKey::Backspace) => {
                            state.apply_action(UiAction::ListBack);
                        }
                        Key::Named(NamedKey::Escape) => {
                            state.apply_action(UiAction::CloseList);
                        }
                        Key::Character(c) => {
                            state.card_menu_open = None;
                            let src = match c.as_str() {
                                "1" => Some(ListSource::Subs),
                                "2" => Some(ListSource::Recommend),
                                "3" => Some(ListSource::History),
                                "4" => Some(ListSource::Playlist),
                                _ => None,
                            };
                            if let Some(src) = src {
                                state.apply_action(UiAction::OpenList(src));
                            }
                        }
                        _ => {}
                    }
                    #[cfg(windows)]
                    {
                        state.last_activity = Instant::now();
                    }
                    return;
                }
                match event.logical_key {
                    // Space は URL に現れないため再生/一時停止に温存。
                    Key::Named(NamedKey::Space) => {
                        state.apply_action(UiAction::TogglePause);
                    }
                    Key::Named(NamedKey::ArrowRight) => {
                        state.apply_action(UiAction::SeekBy(5.0));
                    }
                    Key::Named(NamedKey::ArrowLeft) => {
                        state.apply_action(UiAction::SeekBy(-5.0));
                    }
                    Key::Named(NamedKey::ArrowUp) => {
                        state.apply_action(UiAction::VolumeBy(5.0));
                    }
                    Key::Named(NamedKey::ArrowDown) => {
                        state.apply_action(UiAction::VolumeBy(-5.0));
                    }
                    // --- URL 入力欄の編集（テキスト編集そのものなので UiAction 化しない）---
                    Key::Named(NamedKey::Backspace) => {
                        state.url_input.pop();
                    }
                    Key::Named(NamedKey::Escape) => state.url_input.clear(),
                    Key::Named(NamedKey::Enter) => {
                        state.apply_action(UiAction::PlayUrl(state.url_input.trim().to_string()));
                    }
                    // 印字可能文字は URL 欄へ追記（IME 不要。URL は英数字記号のみ）。
                    _ => {
                        if let Some(t) = &event.text {
                            for ch in t.chars() {
                                if !ch.is_control() {
                                    state.url_input.push(ch);
                                }
                            }
                        }
                    }
                }
                // キー操作も活動として扱い、オーバーレイの自動非表示を遅らせる。
                #[cfg(windows)]
                {
                    state.last_activity = Instant::now();
                }
            }
            WindowEvent::RedrawRequested => {
                // mpv が wid に自前で D3D11 描画するため、ここでは描画しない。
                // 背景結果の取り込みのみ行う。
                state.poll_all();
            }
            _ => {}
        }
    }
}

/// ウィンドウのクライアント領域を画面から BitBlt で取り込み、PNG バイト列にする。
/// 動画(mpv D3D11)と透過オーバーレイが OS で合成された「見たままの」画を取得する
/// （ウィンドウが可視で前面にある前提。dev-tools の /screenshot 用）。
#[cfg(windows)]
unsafe fn capture_client_png(wid: i64) -> Option<Vec<u8>> {
    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::Graphics::Gdi::{
        BitBlt, ClientToScreen, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject,
        GetDC, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
        HBITMAP, SRCCOPY,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

    let hwnd = HWND(wid as *mut core::ffi::c_void);
    let mut rc = RECT::default();
    if GetClientRect(hwnd, &mut rc).is_err() {
        return None;
    }
    let w = (rc.right - rc.left).max(1);
    let h = (rc.bottom - rc.top).max(1);
    let mut org = POINT { x: 0, y: 0 };
    let _ = ClientToScreen(hwnd, &mut org);

    let screen = GetDC(None);
    let mem = CreateCompatibleDC(screen);
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
        .ok()
        .filter(|b: &HBITMAP| !b.0.is_null());
    let result = (|| {
        let dib = dib?;
        let old = SelectObject(mem, dib);
        let _ = BitBlt(mem, 0, 0, w, h, screen, org.x, org.y, SRCCOPY);
        let n = (w * h * 4) as usize;
        let src = std::slice::from_raw_parts(bits as *const u8, n);
        // BGRA(top-down) → RGBA。
        let mut rgba = vec![0u8; n];
        for i in (0..n).step_by(4) {
            rgba[i] = src[i + 2];
            rgba[i + 1] = src[i + 1];
            rgba[i + 2] = src[i];
            rgba[i + 3] = 255;
        }
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut wtr = enc.write_header().ok()?;
            wtr.write_image_data(&rgba).ok()?;
        }
        SelectObject(mem, old);
        let _ = DeleteObject(dib);
        Some(out)
    })();
    let _ = DeleteDC(mem);
    ReleaseDC(None, screen);
    result
}

/// winit ウィンドウの Win32 HWND を i64 で取り出す（mpv の `wid` 用）。
fn hwnd_of(window: &Window) -> Result<i64> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle()?.as_raw() {
        RawWindowHandle::Win32(h) => Ok(h.hwnd.get() as i64),
        other => Err(anyhow::anyhow!("Win32 以外のウィンドウハンドル: {other:?}")),
    }
}
