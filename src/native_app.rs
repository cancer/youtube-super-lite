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
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::controller::Controller;
use crate::player::Player;
use crate::{auth, gpu_usage, UserEvent};

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
    /// 登録チャンネル新着の一覧表示中か、および選択位置。
    list_open: bool,
    list_sel: usize,
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
        self.core.poll_channel();
        self.core.poll_gpu_usage();
        self.core.poll_resolve();
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
            // オーバーレイの操作適用・自動非表示・定期再描画（~10fps）。
            #[cfg(windows)]
            {
                use crate::native_overlay::OverlayAction;
                use windows::Win32::Foundation::{HWND, POINT};
                use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

                // クリックで溜まった操作を Player に適用。
                let action = _state.overlay.as_ref().and_then(|ov| ov.take_action());
                if let Some(action) = action {
                    let player = &_state.core.player;
                    match action {
                        OverlayAction::TogglePause => player.set_paused(!player.paused()),
                        OverlayAction::Seek(frac) => {
                            let dur = player.duration();
                            if dur > 0.0 {
                                player.set_time_pos(frac * dur);
                            }
                        }
                    }
                    _state.last_activity = Instant::now();
                }

                // カーソル移動を検出して自動非表示を制御。
                let mut p = POINT::default();
                let _ = unsafe { GetCursorPos(&mut p) };
                if (p.x, p.y) != _state.last_cursor {
                    _state.last_cursor = (p.x, p.y);
                    _state.last_activity = Instant::now();
                }
                // 一覧表示中は常に表示。それ以外は 3 秒無操作で自動非表示。
                let show = _state.list_open || _state.last_activity.elapsed() < Duration::from_secs(3);
                if show != _state.overlay_visible {
                    _state.overlay_visible = show;
                    if let Some(ov) = _state.overlay.as_ref() {
                        ov.set_visible(show);
                    }
                }

                // 表示中のみ再描画（シークバー/時間・一覧を更新）。
                if _state.overlay_visible {
                    let parent = HWND(_state.parent_wid as *mut core::ffi::c_void);
                    let url = _state.url_input.clone();
                    let list_open = _state.list_open;
                    let list_sel = _state.list_sel;
                    let (titles, thumbs): (Vec<String>, Vec<String>) = if list_open {
                        (
                            _state
                                .core
                                .sub_feed
                                .iter()
                                .map(|v| format!("{}   |   {}", v.title, v.channel))
                                .collect(),
                            _state
                                .core
                                .sub_feed
                                .iter()
                                .map(|v| v.thumbnail.clone())
                                .collect(),
                        )
                    } else {
                        (Vec::new(), Vec::new())
                    };
                    if let Some(ov) = _state.overlay.as_mut() {
                        ov.render(
                            &_state.core.player,
                            parent,
                            &url,
                            list_open,
                            &titles,
                            list_sel,
                            &thumbs,
                        );
                    }
                }
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(100),
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
            WindowEvent::KeyboardInput { event, .. } => {
                if !event.state.is_pressed() {
                    return;
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
                // Tab: 登録チャンネル新着の一覧を開閉。
                if let Key::Named(NamedKey::Tab) = event.logical_key {
                    state.list_open = !state.list_open;
                    if state.list_open {
                        state.list_sel = 0;
                        if state.core.sub_feed.is_empty() && !state.core.sub_busy {
                            state.core.start_subs();
                        }
                    }
                    #[cfg(windows)]
                    {
                        state.last_activity = Instant::now();
                    }
                    return;
                }
                // 一覧表示中はキーをナビゲーションに使う。
                if state.list_open {
                    match event.logical_key {
                        Key::Named(NamedKey::ArrowUp) => {
                            state.list_sel = state.list_sel.saturating_sub(1);
                        }
                        Key::Named(NamedKey::ArrowDown) => {
                            let n = state.core.sub_feed.len();
                            if n > 0 {
                                state.list_sel = (state.list_sel + 1).min(n - 1);
                            }
                        }
                        Key::Named(NamedKey::Enter) => {
                            if let Some(v) = state.core.sub_feed.get(state.list_sel) {
                                let url =
                                    format!("https://www.youtube.com/watch?v={}", v.video_id);
                                state.list_open = false;
                                state.url_input = url.clone();
                                state.core.load(&url);
                                if let Some(vid) =
                                    auth::extract_video_id(&state.core.current_url)
                                {
                                    state.core.start_chat(vid.clone());
                                    state.core.start_recommend(vid);
                                }
                            }
                        }
                        Key::Named(NamedKey::Escape) => state.list_open = false,
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
