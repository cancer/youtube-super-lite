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

use crate::chat::ChatRun;
use crate::controller::Controller;
use crate::player::Player;
use crate::{auth, gpu_usage, resolve, Codec, Quality, UserEvent};

/// 一覧の表示ソース。1/2/3 キーで切替。
#[derive(Clone, Copy, PartialEq)]
enum ListSource {
    Subs,
    Recommend,
    History,
    Playlist,
}

/// `--native` 起動時のアプリケーション。
pub struct NativeApp {
    proxy: EventLoopProxy<UserEvent>,
    initial_url: Option<String>,
    verbose: bool,
    backend: String,
    initial_volume: Option<f64>,
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
    /// チャット（右パネル）表示中か。
    chat_open: bool,
    /// アプリ窓がフォーカスを持っているか。失っている間はオーバーレイを隠す
    /// （他アプリの上にオーバーレイが残らないようにする）。
    focused: bool,
    /// 動画に重ねる透過 2D オーバーレイ（コントローラ表示）。Windows のみ。
    #[cfg(windows)]
    overlay: Option<crate::native_overlay::Overlay>,
    /// 自動非表示用: 最後に操作（マウス移動/キー/クリック）があった時刻と前回カーソル位置。
    #[cfg(windows)]
    last_activity: Instant,
    #[cfg(windows)]
    last_cursor: (i32, i32),
    #[cfg(windows)]
    overlay_visible: bool,
}

impl NativeApp {
    pub fn new(
        proxy: EventLoopProxy<UserEvent>,
        initial_url: Option<String>,
        verbose: bool,
        backend: String,
        initial_volume: Option<f64>,
    ) -> Self {
        Self {
            proxy,
            initial_url,
            verbose,
            backend,
            initial_volume,
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
                core.start_chat(vid.clone());
                core.start_recommend(vid);
            }
        }

        // 動画に重ねる透過 2D オーバーレイ（Direct2D コントローラ）。
        #[cfg(windows)]
        let overlay = {
            let parent = windows::Win32::Foundation::HWND(wid as *mut core::ffi::c_void);
            match crate::native_overlay::Overlay::new(parent) {
                Ok(o) => Some(o),
                Err(e) => {
                    eprintln!("[native] overlay init failed: {e:#}");
                    None
                }
            }
        };

        Ok(NativeRunning {
            window,
            parent_wid: wid,
            core,
            url_input,
            ctrl: false,
            list_open: false,
            list_sel: 0,
            list_source: ListSource::Subs,
            chat_open: false,
            focused: true,
            #[cfg(windows)]
            overlay,
            #[cfg(windows)]
            last_activity: Instant::now(),
            #[cfg(windows)]
            last_cursor: (0, 0),
            #[cfg(windows)]
            overlay_visible: true,
        })
    }
}

impl NativeRunning {
    /// 現在の一覧ソースの (ヘッダ, 行[(タイトル, サムネURL, video_id|playlist_id)]) を返す。
    fn list_rows(&self) -> (String, Vec<(String, String, String)>) {
        let nav = "  （1新着 2おすすめ 3履歴 4リスト / ↑↓ 選択 / Enter 決定 / Backspace 戻る / Tab・Esc 閉じる）";
        let video_row = |title: &str, channel: &str, thumb: String, id: &str| {
            (format!("{title}   |   {channel}"), thumb, id.to_string())
        };
        let (base, items): (String, Vec<(String, String, String)>) = match self.list_source {
            ListSource::Subs => (
                "登録チャンネルの新着".to_string(),
                self.core
                    .sub_feed
                    .iter()
                    .map(|v| video_row(&v.title, &v.channel, v.thumbnail.clone(), &v.video_id))
                    .collect(),
            ),
            ListSource::Recommend => (
                "おすすめ".to_string(),
                self.core
                    .recommend_items
                    .iter()
                    .map(|v| video_row(&v.title, &v.channel, v.thumbnail.clone(), &v.video_id))
                    .collect(),
            ),
            ListSource::History => (
                "再生履歴".to_string(),
                self.core
                    .history_items
                    .iter()
                    .map(|v| video_row(&v.title, &v.channel, v.thumbnail.clone(), &v.video_id))
                    .collect(),
            ),
            ListSource::Playlist => {
                if !self.core.playlist_items.is_empty() {
                    // 2 階層目: 選択した再生リストの中身（動画）。
                    let rows = self
                        .core
                        .playlist_items
                        .iter()
                        .map(|v| video_row(&v.title, &v.channel, String::new(), &v.video_id))
                        .collect();
                    (
                        format!("再生リスト: {}", self.core.playlist_items_title),
                        rows,
                    )
                } else {
                    // 1 階層目: 再生リスト一覧（Enter で中身を開く）。
                    let rows = self
                        .core
                        .playlist_lists
                        .iter()
                        .map(|p| {
                            (
                                format!("{}（{} 件）", p.title, p.item_count),
                                String::new(),
                                p.playlist_id.clone(),
                            )
                        })
                        .collect();
                    ("再生リスト".to_string(), rows)
                }
            }
        };
        (format!("{base}{nav}"), items)
    }

    /// 現在の一覧ソースが未取得なら取得を開始する（Recommend は再生中の動画に紐づくため何もしない）。
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
            ListSource::Recommend => {}
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
        self.core.poll_playlist();
        self.core.poll_gpu_usage();
        self.core.poll_resolve();
    }

    /// オーバーレイを現在のウィンドウ位置・サイズに合わせて再描画する（窓が可視の時のみ）。
    /// `active` はコントロール（上部バー＋下部コントローラ）を描くか。リサイズ/移動イベント
    /// からも呼び、ウィンドウに即追従させる。
    #[cfg(windows)]
    fn render_overlay(&mut self, active: bool) {
        if !self.overlay_visible {
            return;
        }
        use windows::Win32::Foundation::HWND;
        let parent = HWND(self.parent_wid as *mut core::ffi::c_void);
        let url = self.url_input.clone();
        let list_open = self.list_open;
        let list_sel = self.list_sel;
        let (header, titles, thumbs): (String, Vec<String>, Vec<String>) = if list_open {
            let (h, rows) = self.list_rows();
            (
                h,
                rows.iter().map(|r| r.0.clone()).collect(),
                rows.iter().map(|r| r.1.clone()).collect(),
            )
        } else {
            (String::new(), Vec::new(), Vec::new())
        };
        let logged_in = self.core.channel.as_deref().is_some_and(|c| !c.is_empty());
        let auth_label = if logged_in {
            format!("👤 {}", self.core.channel.as_deref().unwrap_or(""))
        } else {
            format!("🔑 {}", self.core.auth_status)
        };
        let quality_label = self.core.quality.label();
        let codec_label = self.core.codec.label();
        let has_recommend = !self.core.recommend_items.is_empty();
        let is_live = self.core.is_live;
        // egui 版と同じく、チャット接続中 or メッセージがある時のみ 💬 を出す。
        let chat_available = !self.core.chat_status.is_empty();
        let chat_open = self.chat_open;
        let chat_lines: Vec<String> = if chat_open {
            self.core
                .chat_messages
                .iter()
                .map(|m| {
                    let text: String = m
                        .runs
                        .iter()
                        .map(|r| match r {
                            ChatRun::Text(t) => t.as_str(),
                            ChatRun::Image { alt } => alt.as_str(),
                        })
                        .collect();
                    format!("{}: {}", m.author, text)
                })
                .collect()
        } else {
            Vec::new()
        };
        if let Some(ov) = self.overlay.as_mut() {
            ov.render(
                &self.core.player,
                parent,
                &url,
                active,
                list_open,
                &titles,
                list_sel,
                &thumbs,
                &header,
                &auth_label,
                logged_in,
                has_recommend,
                quality_label,
                codec_label,
                is_live,
                chat_available,
                chat_open,
                &chat_lines,
            );
        }
    }

    /// オーバーレイ操作を一つ適用する（クリック由来）。キーボードショートカットと同じ効果。
    #[cfg(windows)]
    fn apply_overlay_action(&mut self, action: crate::native_overlay::OverlayAction) {
        use crate::native_overlay::{ListTab, OverlayAction};
        match action {
            OverlayAction::TogglePause => {
                let p = &self.core.player;
                p.set_paused(!p.paused());
            }
            OverlayAction::Seek(frac) => {
                let p = &self.core.player;
                let dur = p.duration();
                if p.seekable() && dur > 0.0 {
                    p.set_time_pos(frac * dur);
                }
            }
            OverlayAction::SetVolume(v) => self.core.player.set_volume(v.clamp(0.0, 130.0)),
            OverlayAction::LiveEdge => self.core.player.seek_to_live(),
            OverlayAction::ToggleMute => {
                let p = &self.core.player;
                p.set_muted(!p.muted());
            }
            OverlayAction::ToggleChat => {
                self.chat_open = !self.chat_open;
                self.core
                    .player
                    .set_video_margin_right(if self.chat_open { 0.28 } else { 0.0 });
            }
            OverlayAction::Like => {
                if let Some(vid) = auth::extract_video_id(&self.core.current_url) {
                    self.core.start_like(vid);
                }
            }
            OverlayAction::Login => {
                if !self.core.auth_busy {
                    self.core.start_login();
                }
            }
            OverlayAction::CycleQuality => {
                let all = Quality::ALL;
                let i = all
                    .iter()
                    .position(|q| *q == self.core.quality)
                    .unwrap_or(0);
                self.core.quality = all[(i + 1) % all.len()];
                if resolve::is_youtube_url(&self.core.current_url) {
                    let u = self.core.current_url.clone();
                    self.core.start_resolve(u);
                }
            }
            OverlayAction::CycleCodec => {
                let all = Codec::ALL;
                let i = all.iter().position(|c| *c == self.core.codec).unwrap_or(0);
                self.core.codec = all[(i + 1) % all.len()];
                if resolve::is_youtube_url(&self.core.current_url) {
                    let u = self.core.current_url.clone();
                    self.core.start_resolve(u);
                }
            }
            OverlayAction::OpenList(tab) => {
                self.list_source = match tab {
                    ListTab::Recommend => ListSource::Recommend,
                    ListTab::Subs => ListSource::Subs,
                    ListTab::Playlist => ListSource::Playlist,
                    ListTab::History => ListSource::History,
                };
                self.list_open = true;
                self.list_sel = 0;
                self.ensure_source_fetched();
            }
            OverlayAction::PlayIndex(idx) => self.play_list_index(idx),
        }
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
        if let Some((_, _, vid)) = rows.get(idx) {
            let url = format!("https://www.youtube.com/watch?v={vid}");
            self.list_open = false;
            self.url_input = url.clone();
            self.core.load(&url);
            if let Some(v) = auth::extract_video_id(&self.core.current_url) {
                self.core.start_chat(v.clone());
                self.core.start_recommend(v);
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
            // オーバーレイの操作適用・自動非表示・定期再描画。
            #[cfg(windows)]
            {
                use windows::Win32::Foundation::POINT;
                use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

                // クリックで溜まった操作をすべて適用（コントロール・動画クリック・一覧行）。
                let actions = _state
                    .overlay
                    .as_ref()
                    .map(|ov| ov.take_actions())
                    .unwrap_or_default();
                for action in actions {
                    _state.apply_overlay_action(action);
                    _state.last_activity = Instant::now();
                }

                // カーソル移動を検出して自動非表示（active 解除）を制御。
                let mut p = POINT::default();
                let _ = unsafe { GetCursorPos(&mut p) };
                if (p.x, p.y) != _state.last_cursor {
                    _state.last_cursor = (p.x, p.y);
                    _state.last_activity = Instant::now();
                }
                // コントロール描画（active）: 一覧/チャット表示中は常時、それ以外は 3 秒無操作で隠す。
                let active = _state.list_open
                    || _state.chat_open
                    || _state.last_activity.elapsed() < Duration::from_secs(3);
                // 窓の可視は「フォーカス中かつ表示すべき UI がある」時のみ。アイドル時は隠して
                // 動画全面を素通しにする（動画クリック=一時停止は winit の MouseInput で処理）。
                let show = _state.focused && active;
                if show != _state.overlay_visible {
                    _state.overlay_visible = show;
                    if let Some(ov) = _state.overlay.as_ref() {
                        ov.set_visible(show);
                    }
                }

                // 窓が可視の時のみ再描画。
                _state.render_overlay(active);
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
            // フォーカスを失ったらオーバーレイを隠す（他アプリの上に残らないように）。
            WindowEvent::Focused(focused) => {
                state.focused = focused;
                #[cfg(windows)]
                {
                    if focused {
                        state.last_activity = Instant::now();
                    } else {
                        state.overlay_visible = false;
                        if let Some(ov) = state.overlay.as_ref() {
                            ov.set_visible(false);
                        }
                    }
                }
            }
            // ウィンドウのリサイズ/移動にオーバーレイを即追従させる
            // （モーダルなドラッグループ中は about_to_wait が止まるため、ここで直接再描画）。
            WindowEvent::Resized(_) | WindowEvent::Moved(_) => {
                #[cfg(windows)]
                if state.focused {
                    state.last_activity = Instant::now();
                    state.overlay_visible = true;
                    if let Some(ov) = state.overlay.as_ref() {
                        ov.set_visible(true);
                    }
                    // リサイズ/移動直後は操作直後なので active で再描画。
                    state.render_overlay(true);
                }
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
                                if !state.core.auth_busy {
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
                            _ => {}
                        }
                    }
                }
                // Ctrl+V: クリップボードのテキストを URL 欄へ貼り付け。
                #[cfg(windows)]
                if state.ctrl {
                    if let Key::Character(c) = &event.logical_key {
                        if c.eq_ignore_ascii_case("v") {
                            if let Some(t) = crate::native_overlay::clipboard_text() {
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
                    match &event.logical_key {
                        Key::Named(NamedKey::ArrowUp) => {
                            state.list_sel = state.list_sel.saturating_sub(1);
                        }
                        Key::Named(NamedKey::ArrowDown) => {
                            let n = state.list_rows().1.len();
                            if n > 0 {
                                state.list_sel = (state.list_sel + 1).min(n - 1);
                            }
                        }
                        Key::Named(NamedKey::Enter) => {
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
                                if let Some((_, _, vid)) = rows.get(state.list_sel) {
                                    let url = format!("https://www.youtube.com/watch?v={vid}");
                                    state.list_open = false;
                                    state.url_input = url.clone();
                                    state.core.load(&url);
                                    if let Some(v) =
                                        auth::extract_video_id(&state.core.current_url)
                                    {
                                        state.core.start_chat(v.clone());
                                        state.core.start_recommend(v);
                                    }
                                }
                            }
                        }
                        Key::Named(NamedKey::Backspace) => {
                            // 再生リストの中身表示中なら一覧へ戻る。
                            if state.list_source == ListSource::Playlist
                                && !state.core.playlist_items.is_empty()
                            {
                                state.core.playlist_items.clear();
                                state.core.playlist_items_title.clear();
                                state.list_sel = 0;
                            }
                        }
                        Key::Named(NamedKey::Escape) => state.list_open = false,
                        Key::Character(c) => {
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
                                state.core.start_chat(vid.clone());
                                state.core.start_recommend(vid);
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

/// winit ウィンドウの Win32 HWND を i64 で取り出す（mpv の `wid` 用）。
fn hwnd_of(window: &Window) -> Result<i64> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle()?.as_raw() {
        RawWindowHandle::Win32(h) => Ok(h.hwnd.get() as i64),
        other => Err(anyhow::anyhow!("Win32 以外のウィンドウハンドル: {other:?}")),
    }
}
