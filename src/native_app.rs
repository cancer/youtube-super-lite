//! ネイティブ版エントリ（`--native`）。OpenGL を一切作らず、mpv を `wid` 経由で
//! D3D11 にウィンドウへ直接描画させ、UI 非依存の [`Controller`](crate::controller::Controller)
//! をそのまま駆動する。egui 版（[`crate::App`]）と並存し、移行検証用の実フロントエンド。
//!
//! 現状（骨組み）: winit ウィンドウ + 埋め込み mpv + Controller + キーボード操作 + 各種 poll。
//! コントローラ等の 2D UI（Direct2D/DirectWrite/WIC の透過オーバーレイ）は後続フェーズで重ねる
//! （probe: src/bin/d2d_overlay_probe.rs で実証済み）。

use anyhow::Result;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use ysl_core::yt::chat::ChatRun;
use crate::controller::Controller;
use ysl_core::player::Player;
use ysl_core::yt::{auth, resolve};
use ysl_core::gpu_usage;
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
    core: Controller,
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

        let mut core = Controller::new(player, self.proxy.clone(), self.backend.clone());

        // 外部アプリへ GPU を譲るための GPU 使用率監視（egui 版と同じく常時起動）。
        if let Some(m) = gpu_usage::start_monitoring() {
            core.gpu_monitor = Some(m);
            eprintln!("[native][auto-hwdec] GPU 使用率の監視を開始");
        }

        // 保存済みリフレッシュトークンがあれば自動ログイン。
        if let Some(rt) = auth::load_refresh_token() {
            core.start_silent_login(rt);
        }

        // CLI で URL 指定があれば再生開始（URL 欄にも反映）。
        let mut url_input = String::new();
        if let Some(url) = self.initial_url.take() {
            url_input = url.clone();
            core.load(&url);
            if let Some(vid) = auth::extract_video_id(&core.current_url) {
                core.start_chat(vid);
            }
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
            core,
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
    /// 現在の一覧ソースの (ヘッダ, カード配列) を返す。
    ///
    /// カードの title/channel/thumb/id は現行データ源から常に埋まる。avatar/duration/live/meta/
    /// verified は `recommend::VideoItem`（おすすめ）では常に埋まるが、subs/history はまだ
    /// パース未対応で既定値のまま。
    /// チャンネル名から解決済みアバター URL を引く（未解決なら空＝プレースホルダ円）。
    fn avatar_for(&self, channel: &str) -> String {
        self.core.channel_avatars.get(channel).cloned().unwrap_or_default()
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
                self.core
                    .sub_feed
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
                self.core
                    .recommend_items
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
                self.core
                    .history_items
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
                format!("{} の動画", self.core.channel_title),
                self.core
                    .channel_items
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
                if !self.core.playlist_items.is_empty() {
                    // 2 階層目: 選択した再生リストの中身（動画）。
                    let rows = self
                        .core
                        .playlist_items
                        .iter()
                        .map(|v| video_card(&v.title, &v.channel, String::new(), &v.video_id))
                        .collect();
                    (
                        format!("再生リスト: {}", self.core.playlist_items_title),
                        rows,
                    )
                } else {
                    // 1 階層目: 再生リスト一覧（Enter で中身を開く）。件数を meta/channel に。
                    let rows = self
                        .core
                        .playlist_lists
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
                if self.core.sub_feed.is_empty() && !self.core.sub_busy {
                    self.core.start_subs();
                }
            }
            ListSource::History => {
                if self.core.history_items.is_empty() && !self.core.history_busy {
                    self.core.start_history();
                }
            }
            ListSource::Playlist => {
                if self.core.playlist_lists.is_empty()
                    && self.core.playlist_items.is_empty()
                    && !self.core.playlist_busy
                {
                    self.core.start_playlist_list();
                }
            }
            ListSource::Recommend => {
                if self.core.recommend_items.is_empty() && self.core.account.token().is_some() {
                    self.core.start_recommend();
                }
            }
            // チャンネルビューは open_channel で取得済み。ここでは何もしない。
            ListSource::Channel => {}
        }
    }

    /// 背景スレッド（認証/API/解決）の結果を取り込む。proxy 起床時に呼ぶ。
    fn poll_all(&mut self) {
        // リプレイチャット用に再生位置を共有。
        self.core
            .player_offset_ms
            .store((self.core.player.time_pos() * 1000.0) as i64, Ordering::Relaxed);
        self.core.poll_auth();
        self.core.poll_chat();
        self.core.poll_recommend();
        self.core.poll_subs();
        self.core.poll_history();
        self.core.poll_channel();
        self.core.poll_channel_avatars();
        self.core.poll_playlist();
        self.core.poll_gpu_usage();
        self.core.poll_resolve();
        // native 直 URL が mpv で再生失敗していれば、並列に用意した中継(サイドカー)へ切替える。
        self.core.check_playback_fallback();
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
                            self.core.load(&url);
                            if let Some(vid) = auth::extract_video_id(&self.core.current_url) {
                                self.core.start_chat(vid);
                            }
                        }
                    }
                    let _ = reply.send(true);
                }
            }
        }
    }

    /// dev-tools のアクション名を UI 状態に反映する（キーボード/オーバーレイの全操作を網羅）。
    /// 既知なら true。
    fn devtools_action(&mut self, name: &str) -> bool {
        let known = match name {
            // --- 再生 ---
            "play_pause" => {
                let p = &self.core.player;
                p.set_paused(!p.paused());
                true
            }
            "seek_fwd" => {
                self.core.player.seek_relative(5.0);
                true
            }
            "seek_back" => {
                self.core.player.seek_relative(-5.0);
                true
            }
            "live_edge" => {
                self.core.player.seek_to_live();
                true
            }
            // --- 音量 ---
            "vol_up" => {
                let p = &self.core.player;
                p.set_volume((p.volume() + 5.0).min(130.0));
                true
            }
            "vol_down" => {
                let p = &self.core.player;
                p.set_volume((p.volume() - 5.0).max(0.0));
                true
            }
            "mute" => {
                let p = &self.core.player;
                p.set_muted(!p.muted());
                true
            }
            // --- 画質 / コーデック ---
            "quality_next" => {
                let all = Quality::ALL;
                let i = all.iter().position(|q| *q == self.core.quality).unwrap_or(0);
                self.core.quality = all[(i + 1) % all.len()];
                if resolve::is_youtube_url(&self.core.current_url) {
                    let u = self.core.current_url.clone();
                    self.core.start_resolve(u);
                }
                true
            }
            "codec_next" => {
                let all = Codec::ALL;
                let i = all.iter().position(|c| *c == self.core.codec).unwrap_or(0);
                self.core.codec = all[(i + 1) % all.len()];
                if resolve::is_youtube_url(&self.core.current_url) {
                    let u = self.core.current_url.clone();
                    self.core.start_resolve(u);
                }
                true
            }
            // --- チャット ---
            "toggle_chat" => {
                self.chat_open = !self.chat_open;
                self.core
                    .player
                    .set_video_margin_right(if self.chat_open { 0.28 } else { 0.0 });
                true
            }
            "chat_font_inc" => {
                self.chat_font_px = (self.chat_font_px + 2.0).clamp(10.0, 28.0);
                true
            }
            "chat_font_dec" => {
                self.chat_font_px = (self.chat_font_px - 2.0).clamp(10.0, 28.0);
                true
            }
            "chat_scroll_up" | "chat_scroll_down" => {
                let d: i32 = if name == "chat_scroll_up" { 3 } else { -3 };
                let max = self.core.chat.as_ref().map_or(0, |c| c.messages().len()).saturating_sub(1);
                self.chat_scroll = ((self.chat_scroll as i32 + d).max(0) as usize).min(max);
                true
            }
            "chat_wider" | "chat_narrower" => {
                let d = if name == "chat_wider" { 0.04 } else { -0.04 };
                self.chat_width_ratio = (self.chat_width_ratio + d).clamp(0.15, 0.6);
                if self.chat_open {
                    self.core
                        .player
                        .set_video_margin_right(self.chat_width_ratio as f64);
                }
                true
            }
            // --- 認証 / 評価 ---
            "login" => {
                if !self.core.account.is_busy() {
                    self.core.start_login();
                }
                true
            }
            "like" => {
                if let Some(vid) = auth::extract_video_id(&self.core.current_url) {
                    self.core.start_like(vid);
                }
                true
            }
            // --- URL 再生 ---
            "play_url" => {
                let url = self.url_input.trim().to_string();
                if !url.is_empty() {
                    self.core.load(&url);
                    if let Some(vid) = auth::extract_video_id(&self.core.current_url) {
                        self.core.start_chat(vid);
                    }
                }
                true
            }
            // --- 一覧 ---
            "toggle_list" => {
                self.list_open = !self.list_open;
                self.card_menu_open = None;
                if self.list_open {
                    self.list_sel = 0;
                    self.ensure_source_fetched();
                }
                true
            }
            "close_overlay" => {
                self.list_open = false;
                self.card_menu_open = None;
                true
            }
            "open_recommend" | "open_subs" | "open_playlist" | "open_history" => {
                self.list_source = match name {
                    "open_recommend" => ListSource::Recommend,
                    "open_subs" => ListSource::Subs,
                    "open_playlist" => ListSource::Playlist,
                    _ => ListSource::History,
                };
                self.list_open = true;
                self.list_sel = 0;
                self.card_menu_open = None;
                self.ensure_source_fetched();
                true
            }
            "list_up" => {
                self.list_sel = self.list_sel.saturating_sub(1);
                true
            }
            "list_down" => {
                let n = self.list_rows().1.len();
                if n > 0 {
                    self.list_sel = (self.list_sel + 1).min(n - 1);
                }
                true
            }
            "list_select" => {
                self.card_menu_open = None;
                self.play_list_index(self.list_sel);
                true
            }
            "list_back" => {
                if self.list_source == ListSource::Channel {
                    // チャンネルビューから おすすめ へ戻る。
                    self.list_source = ListSource::Recommend;
                    self.list_sel = 0;
                } else if self.list_source == ListSource::Playlist
                    && !self.core.playlist_items.is_empty()
                {
                    self.core.playlist_items.clear();
                    self.core.playlist_items_title.clear();
                    self.list_sel = 0;
                }
                true
            }
            _ => false,
        };
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
        let p = &self.core.player;
        let source = match self.list_source {
            ListSource::Subs => "subs",
            ListSource::Recommend => "recommend",
            ListSource::History => "history",
            ListSource::Playlist => "playlist",
            ListSource::Channel => "channel",
        };
        let logged_in = self.core.account.channel_name().is_some_and(|c| !c.is_empty());
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
            "current_url": self.core.current_url,
            "url_input": self.url_input,
            "paused": p.paused(),
            "time_pos": p.time_pos(),
            "duration": p.duration(),
            "seekable": p.seekable(),
            "is_live": self.core.is_live,
            "volume": p.volume(),
            "muted": p.muted(),
            "media_title": p.media_title(),
            "quality": self.core.quality.label(),
            "codec": self.core.codec.label(),
            "chat_open": self.chat_open,
            "chat_font_px": self.chat_font_px,
            "chat_width_ratio": self.chat_width_ratio,
            "chat_scroll": self.chat_scroll,
            "chat_available": self.core.chat.as_ref().is_some_and(|c| c.available()),
            "chat_messages": self.core.chat.as_ref().map_or(0, |c| c.messages().len()),
            "list_open": self.list_open,
            "list_source": source,
            "list_sel": self.list_sel,
            "list_count": self.list_rows().1.len(),
            "card_menu_open": self.card_menu_open,
            "overlay_is_topmost": overlay_is_topmost,
            "logged_in": logged_in,
            "channel": self.core.account.channel_name(),
            "auth_status": self.core.account.status(),
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

    /// 一覧の行 index を再生（再生リスト 1 階層目なら中身を開く）。
    #[cfg(windows)]
    fn play_list_index(&mut self, idx: usize) {
        if self.list_source == ListSource::Playlist && self.core.playlist_items.is_empty() {
            // 再生リスト一覧で選択 → その中身を開く（2 階層目へ）。
            if let Some(pl) = self.core.playlist_lists.get(idx) {
                let id = pl.playlist_id.clone();
                let title = pl.title.clone();
                self.list_sel = 0;
                self.core.start_playlist_items(id, title);
            }
            return;
        }
        let rows = self.list_rows().1;
        if let Some(card) = rows.get(idx) {
            let url = format!("https://www.youtube.com/watch?v={}", card.id);
            self.list_open = false;
            self.url_input = url.clone();
            self.core.load(&url);
            if let Some(v) = auth::extract_video_id(&self.core.current_url) {
                self.core.start_chat(v);
            }
        }
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
                use crate::dcomp_overlay::{Card, ListTab, OverlayAction, PlaybackView};
                let actions = _state
                    .dcomp_overlay
                    .as_mut()
                    .map(|o| o.take_actions())
                    .unwrap_or_default();
                for a in actions {
                    match a {
                        OverlayAction::TogglePause => {
                            let p = &_state.core.player;
                            p.set_paused(!p.paused());
                        }
                        OverlayAction::Seek(frac) => {
                            let p = &_state.core.player;
                            let dur = p.duration();
                            if p.seekable() && dur > 0.0 {
                                p.set_time_pos(frac * dur);
                            }
                        }
                        OverlayAction::SetVolume(v) => {
                            _state.core.player.set_volume(v.clamp(0.0, 130.0))
                        }
                        OverlayAction::VolumeStep(d) => {
                            let p = &_state.core.player;
                            p.set_volume((p.volume() + d).clamp(0.0, 130.0));
                        }
                        OverlayAction::ToggleMute => {
                            let p = &_state.core.player;
                            p.set_muted(!p.muted());
                        }
                        OverlayAction::LiveEdge => _state.core.player.seek_to_live(),
                        OverlayAction::Like => {
                            if let Some(vid) = auth::extract_video_id(&_state.core.current_url) {
                                _state.core.start_like(vid);
                            }
                        }
                        OverlayAction::CycleQuality => {
                            let all = Quality::ALL;
                            let i = all
                                .iter()
                                .position(|q| *q == _state.core.quality)
                                .unwrap_or(0);
                            _state.core.quality = all[(i + 1) % all.len()];
                            if resolve::is_youtube_url(&_state.core.current_url) {
                                let u = _state.core.current_url.clone();
                                _state.core.start_resolve(u);
                            }
                        }
                        OverlayAction::CycleCodec => {
                            let all = Codec::ALL;
                            let i =
                                all.iter().position(|c| *c == _state.core.codec).unwrap_or(0);
                            _state.core.codec = all[(i + 1) % all.len()];
                            if resolve::is_youtube_url(&_state.core.current_url) {
                                let u = _state.core.current_url.clone();
                                _state.core.start_resolve(u);
                            }
                        }
                        OverlayAction::Login => {
                            if !_state.core.account.is_busy() {
                                _state.core.start_login();
                            }
                        }
                        OverlayAction::OpenList(tab) => {
                            use crate::dcomp_overlay::ListTab;
                            _state.list_source = match tab {
                                ListTab::Recommend => ListSource::Recommend,
                                ListTab::Subs => ListSource::Subs,
                                ListTab::Playlist => ListSource::Playlist,
                                ListTab::History => ListSource::History,
                            };
                            _state.list_open = true;
                            _state.list_sel = 0;
                            _state.card_menu_open = None;
                            _state.ensure_source_fetched();
                        }
                        OverlayAction::PlayIndex(idx) => {
                            _state.card_menu_open = None;
                            _state.play_list_index(idx);
                        }
                        OverlayAction::OpenChannelOf(idx) => {
                            // 現在の一覧の idx 番のチャンネルを開く。実 channelId があれば
                            // それを使い（確実）、無ければ名前検索にフォールバックする。
                            let rows = _state.list_rows().1;
                            if let Some(card) = rows.get(idx) {
                                if let Some(id) = card.menu.channel_id.clone() {
                                    _state.core.open_channel_by_id(id, card.channel.clone());
                                    _state.list_source = ListSource::Channel;
                                    _state.list_sel = 0;
                                } else if !card.channel.is_empty() {
                                    _state.core.open_channel(card.channel.clone());
                                    _state.list_source = ListSource::Channel;
                                    _state.list_sel = 0;
                                }
                            }
                            _state.card_menu_open = None;
                        }
                        OverlayAction::OpenCardMenu(idx) => {
                            _state.card_menu_open = Some(idx);
                        }
                        OverlayAction::CloseCardMenu => {
                            _state.card_menu_open = None;
                        }
                        OverlayAction::SaveWatchLater(idx) => {
                            let rows = _state.list_rows().1;
                            if let Some(card) = rows.get(idx) {
                                _state.core.save_watch_later(card.id.clone());
                            }
                            _state.card_menu_open = None;
                        }
                        OverlayAction::NotInterested(idx) => {
                            let rows = _state.list_rows().1;
                            if let Some(token) = rows.get(idx).and_then(|c| c.menu.not_interested_token.clone()) {
                                _state.core.send_card_feedback(token);
                            }
                            _state.card_menu_open = None;
                        }
                        OverlayAction::NotRecommendChannel(idx) => {
                            let rows = _state.list_rows().1;
                            if let Some(token) = rows.get(idx).and_then(|c| c.menu.not_channel_token.clone()) {
                                _state.core.send_card_feedback(token);
                            }
                            _state.card_menu_open = None;
                        }
                        OverlayAction::CloseList => {
                            _state.list_open = false;
                            _state.card_menu_open = None;
                        }
                        OverlayAction::ListScroll(d) => {
                            let n = _state.list_rows().1.len();
                            if n > 0 {
                                let sel = (_state.list_sel as i32 + d).clamp(0, n as i32 - 1);
                                _state.list_sel = sel as usize;
                            }
                        }
                        OverlayAction::ToggleChat => {
                            _state.chat_open = !_state.chat_open;
                            if _state.chat_open {
                                _state.chat_scroll = 0;
                            }
                            let m = if _state.chat_open { _state.chat_width_ratio } else { 0.0 };
                            _state.core.player.set_video_margin_right(m as f64);
                        }
                        OverlayAction::ChatScroll(d) => {
                            let max = _state.core.chat.as_ref().map_or(0, |c| c.messages().len()).saturating_sub(1);
                            _state.chat_scroll =
                                ((_state.chat_scroll as i32 + d).max(0) as usize).min(max);
                        }
                        OverlayAction::SetChatWidth(r) => {
                            _state.chat_width_ratio = (r as f32).clamp(0.15, 0.6);
                            if _state.chat_open {
                                _state
                                    .core
                                    .player
                                    .set_video_margin_right(_state.chat_width_ratio as f64);
                            }
                        }
                        OverlayAction::ChatFontDec => {
                            _state.chat_font_px = (_state.chat_font_px - 2.0).clamp(10.0, 28.0);
                        }
                        OverlayAction::ChatFontInc => {
                            _state.chat_font_px = (_state.chat_font_px + 2.0).clamp(10.0, 28.0);
                        }
                    }
                    _state.last_activity = Instant::now();
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
                let logged_in = _state.core.account.channel_name().is_some_and(|c| !c.is_empty());
                let auth_label = if logged_in {
                    format!("👤 {}", _state.core.account.channel_name().unwrap_or(""))
                } else {
                    format!("🔑 {}", _state.core.account.status())
                };
                let list_sel = _state.list_sel;
                let (list_header, list_cards): (String, Vec<Card>) = if list_open {
                    _state.list_rows()
                } else {
                    (String::new(), Vec::new())
                };
                // チャット行（dcomp 用に整形。連続テキストは 1 セグメントに統合）。
                let chat_open = _state.chat_open;
                let chat_available = _state.core.chat.as_ref().is_some_and(|c| c.available());
                let chat_scroll = _state.chat_scroll;
                let chat_width_ratio = _state.chat_width_ratio;
                let chat_lines: Vec<crate::dcomp_overlay::ChatLine> = if chat_open {
                    use crate::dcomp_overlay::{ChatLine as DLine, ChatSeg as DSeg};
                    _state
                        .core
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
                let p = &_state.core.player;
                let view = PlaybackView {
                    paused: p.paused(),
                    pos: p.time_pos(),
                    dur: p.duration(),
                    seekable: p.seekable(),
                    volume: p.volume(),
                    muted: p.muted(),
                    is_live: _state.core.is_live,
                    quality: _state.core.quality.label().to_string(),
                    codec: _state.core.codec.label().to_string(),
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
                state.core.stop_chat();
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
                let p = &state.core.player;
                p.set_paused(!p.paused());
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
                    let p = &state.core.player;
                    let step = if dy > 0.0 { 5.0 } else { -5.0 };
                    p.set_volume((p.volume() + step).clamp(0.0, 130.0));
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
                if state.ctrl {
                    if let Key::Character(c) = &event.logical_key {
                        match c.as_str().to_ascii_lowercase().as_str() {
                            "l" => {
                                if !state.core.account.is_busy() {
                                    state.core.start_login();
                                }
                                return;
                            }
                            "g" => {
                                if let Some(vid) =
                                    auth::extract_video_id(&state.core.current_url)
                                {
                                    state.core.start_like(vid);
                                }
                                return;
                            }
                            "t" => {
                                state.chat_open = !state.chat_open;
                                state
                                    .core
                                    .player
                                    .set_video_margin_right(if state.chat_open { 0.28 } else { 0.0 });
                                return;
                            }
                            "q" => {
                                let all = Quality::ALL;
                                let i = all
                                    .iter()
                                    .position(|q| *q == state.core.quality)
                                    .unwrap_or(0);
                                state.core.quality = all[(i + 1) % all.len()];
                                if resolve::is_youtube_url(&state.core.current_url) {
                                    let u = state.core.current_url.clone();
                                    state.core.start_resolve(u);
                                }
                                return;
                            }
                            "c" => {
                                let all = Codec::ALL;
                                let i = all
                                    .iter()
                                    .position(|c2| *c2 == state.core.codec)
                                    .unwrap_or(0);
                                state.core.codec = all[(i + 1) % all.len()];
                                if resolve::is_youtube_url(&state.core.current_url) {
                                    let u = state.core.current_url.clone();
                                    state.core.start_resolve(u);
                                }
                                return;
                            }
                            // Ctrl + "-" / "+"（"=" も可）: コメント文字サイズ増減。
                            "-" => {
                                state.chat_font_px = (state.chat_font_px - 2.0).clamp(10.0, 28.0);
                                return;
                            }
                            "+" | "=" => {
                                state.chat_font_px = (state.chat_font_px + 2.0).clamp(10.0, 28.0);
                                return;
                            }
                            _ => {}
                        }
                    }
                }
                // Ctrl+V: クリップボードのテキストを URL 欄へ貼り付け。
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
                    state.list_open = !state.list_open;
                    state.card_menu_open = None;
                    if state.list_open {
                        state.list_sel = 0;
                        state.ensure_source_fetched();
                    }
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
                        .max(1);
                    #[cfg(not(windows))]
                    let cols = 1usize;
                    match &event.logical_key {
                        Key::Named(NamedKey::ArrowUp) => {
                            state.list_sel = state.list_sel.saturating_sub(cols);
                        }
                        Key::Named(NamedKey::ArrowDown) => {
                            let n = state.list_rows().1.len();
                            if n > 0 {
                                state.list_sel = (state.list_sel + cols).min(n - 1);
                            }
                        }
                        Key::Named(NamedKey::ArrowLeft) => {
                            state.list_sel = state.list_sel.saturating_sub(1);
                        }
                        Key::Named(NamedKey::ArrowRight) => {
                            let n = state.list_rows().1.len();
                            if n > 0 {
                                state.list_sel = (state.list_sel + 1).min(n - 1);
                            }
                        }
                        Key::Named(NamedKey::Enter) => {
                            state.card_menu_open = None;
                            if state.list_source == ListSource::Playlist
                                && state.core.playlist_items.is_empty()
                            {
                                // 再生リスト一覧で Enter → その中身を開く（2 階層目へ）。
                                if let Some(pl) = state.core.playlist_lists.get(state.list_sel) {
                                    let id = pl.playlist_id.clone();
                                    let title = pl.title.clone();
                                    state.list_sel = 0;
                                    state.core.start_playlist_items(id, title);
                                }
                            } else {
                                let rows = state.list_rows().1;
                                if let Some(card) = rows.get(state.list_sel) {
                                    let url = format!("https://www.youtube.com/watch?v={}", card.id);
                                    state.list_open = false;
                                    state.url_input = url.clone();
                                    state.core.load(&url);
                                    if let Some(v) =
                                        auth::extract_video_id(&state.core.current_url)
                                    {
                                        state.core.start_chat(v);
                                    }
                                }
                            }
                        }
                        Key::Named(NamedKey::Backspace) => {
                            state.card_menu_open = None;
                            if state.list_source == ListSource::Channel {
                                // チャンネルビューから おすすめ へ戻る。
                                state.list_source = ListSource::Recommend;
                                state.list_sel = 0;
                            } else if state.list_source == ListSource::Playlist
                                && !state.core.playlist_items.is_empty()
                            {
                                // 再生リストの中身表示中なら一覧へ戻る。
                                state.core.playlist_items.clear();
                                state.core.playlist_items_title.clear();
                                state.list_sel = 0;
                            }
                        }
                        Key::Named(NamedKey::Escape) => {
                            state.list_open = false;
                            state.card_menu_open = None;
                        }
                        Key::Character(c) => {
                            state.card_menu_open = None;
                            let next = match c.as_str() {
                                "1" => Some(ListSource::Subs),
                                "2" => Some(ListSource::Recommend),
                                "3" => Some(ListSource::History),
                                "4" => Some(ListSource::Playlist),
                                _ => None,
                            };
                            if let Some(src) = next {
                                state.list_source = src;
                                state.list_sel = 0;
                                state.ensure_source_fetched();
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
                        let p = &state.core.player;
                        p.set_paused(!p.paused());
                    }
                    Key::Named(NamedKey::ArrowRight) => state.core.player.seek_relative(5.0),
                    Key::Named(NamedKey::ArrowLeft) => state.core.player.seek_relative(-5.0),
                    Key::Named(NamedKey::ArrowUp) => {
                        let p = &state.core.player;
                        p.set_volume((p.volume() + 5.0).min(130.0));
                    }
                    Key::Named(NamedKey::ArrowDown) => {
                        let p = &state.core.player;
                        p.set_volume((p.volume() - 5.0).max(0.0));
                    }
                    // --- URL 入力欄の編集 ---
                    Key::Named(NamedKey::Backspace) => {
                        state.url_input.pop();
                    }
                    Key::Named(NamedKey::Escape) => state.url_input.clear(),
                    Key::Named(NamedKey::Enter) => {
                        let url = state.url_input.trim().to_string();
                        if !url.is_empty() {
                            state.core.load(&url);
                            if let Some(vid) = auth::extract_video_id(&state.core.current_url) {
                                state.core.start_chat(vid);
                            }
                        }
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
