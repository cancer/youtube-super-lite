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
    /// 動画に重ねる透過 2D オーバーレイ（コントローラ表示）。Windows のみ。
    #[cfg(windows)]
    overlay: Option<crate::native_overlay::Overlay>,
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

        // CLI で URL 指定があれば再生開始。
        if let Some(url) = self.initial_url.take() {
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
            #[cfg(windows)]
            overlay,
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
            // オーバーレイを定期再描画してシークバー/時間表示を更新する（~10fps）。
            #[cfg(windows)]
            {
                let parent =
                    windows::Win32::Foundation::HWND(_state.parent_wid as *mut core::ffi::c_void);
                if let Some(ov) = _state.overlay.as_mut() {
                    ov.render(&_state.core.player, parent);
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
            WindowEvent::KeyboardInput { event, .. } => {
                if !event.state.is_pressed() {
                    return;
                }
                let player = &state.core.player;
                match event.logical_key {
                    Key::Named(NamedKey::Space) => player.set_paused(!player.paused()),
                    Key::Named(NamedKey::ArrowRight) => player.seek_relative(5.0),
                    Key::Named(NamedKey::ArrowLeft) => player.seek_relative(-5.0),
                    Key::Named(NamedKey::ArrowUp) => {
                        player.set_volume((player.volume() + 5.0).min(130.0))
                    }
                    Key::Named(NamedKey::ArrowDown) => {
                        player.set_volume((player.volume() - 5.0).max(0.0))
                    }
                    _ => {}
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
