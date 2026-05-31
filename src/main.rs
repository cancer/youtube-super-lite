mod auth;

use anyhow::{anyhow, Result};
use std::ffi::{c_void, CString};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glutin::config::ConfigTemplateBuilder;
use glutin::context::{ContextApi, ContextAttributesBuilder, GlProfile, Version};
use glutin::display::GetGlDisplay;
use glutin::prelude::*;
use glutin::surface::{Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow};
use raw_window_handle::HasWindowHandle;

use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};
use libmpv2::Mpv;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

/// イベントループを起こす要求（mpv の新フレーム / 背景スレッド完了）。
#[derive(Debug, Clone, Copy)]
enum UserEvent {
    MpvRedraw,
    Background,
}

/// 背景スレッド（OAuth / API 呼び出し）からの結果。
enum AuthMsg {
    LoggedIn {
        tokens: auth::Tokens,
        channel: Option<String>,
    },
    Like {
        ok: bool,
        msg: String,
        tokens: Option<auth::Tokens>,
    },
    Failed(String),
}

/// この時間だけ操作がなければ UI（URL欄・コントロール・カーソル）を隠す。
const UI_HIDE_AFTER: Duration = Duration::from_secs(3);

/// mpv が GL 関数ポインタを解決するためのコールバック。
/// ctx には glutin の Display を渡しておき、ここで名前解決する。
fn get_proc_address(display: &glutin::display::Display, name: &str) -> *mut c_void {
    let cname = match CString::new(name) {
        Ok(c) => c,
        Err(_) => return std::ptr::null_mut(),
    };
    display.get_proc_address(cname.as_c_str()) as *mut c_void
}

/// 既定ブラウザで URL を開く（Windows）。
fn open_in_browser(url: &str) {
    if url.is_empty() {
        return;
    }
    // cmd の start。第1引数の "" は start のウィンドウタイトル指定（URL を誤認させないため）。
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
}

/// 秒数を mm:ss / h:mm:ss 形式の文字列にする。
fn format_time(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "--:--".to_string();
    }
    let total = secs as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// 同梱の yt-dlp.exe があるディレクトリを PATH 先頭に追加する。
fn ensure_ytdlp_on_path() {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.to_path_buf());
            candidates.push(dir.join("tools"));
        }
    }
    candidates.push(PathBuf::from("tools"));

    for dir in &candidates {
        if dir.join("yt-dlp.exe").exists() {
            let current = std::env::var("PATH").unwrap_or_default();
            std::env::set_var("PATH", format!("{};{}", dir.display(), current));
            println!("yt-dlp found: {}", dir.join("yt-dlp.exe").display());
            return;
        }
    }
    eprintln!("warning: yt-dlp.exe not found near the executable; YouTube URLs may fail");
}

/// 初期化済み（ウィンドウ・GL・mpv がそろった）状態。
struct Running {
    egui_glow: egui_glow::EguiGlow,
    // RenderContext は Mpv を借用する。Mpv はリークして 'static 化しているため、
    // RenderContext<'static> として保持できる（自己参照を回避）。
    render_context: RenderContext<'static>,
    mpv: &'static Mpv,
    gl_context: glutin::context::PossiblyCurrentContext,
    gl_surface: Surface<WindowSurface>,
    window: Window,
    url_input: String,
    // 現在再生中の URL（ブラウザでYouTubeを開くナビゲーションに使う）。
    current_url: String,
    frames: u64,
    verbose: bool,
    // 最後に操作（マウス/キー）があった時刻と、現在 UI を表示しているか。
    last_activity: Instant,
    ui_visible: bool,
    // --- 認証 / API ---
    proxy: EventLoopProxy<UserEvent>,
    backend: String,
    tokens: Option<auth::Tokens>,
    channel: Option<String>,
    auth_status: String,
    auth_busy: bool,
    auth_tx: Sender<AuthMsg>,
    auth_rx: Receiver<AuthMsg>,
}

impl Running {
    /// 動画を読み込む。
    fn load(&mut self, url: &str) {
        let url = url.trim();
        if url.is_empty() {
            return;
        }
        match self.mpv.command("loadfile", &[url]) {
            Ok(_) => {
                println!("loadfile: {url}");
                self.current_url = url.to_string();
            }
            Err(e) => eprintln!("loadfile failed: {e}"),
        }
    }

    /// 背景スレッドからの結果を取り込む。
    fn poll_auth(&mut self) {
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
    fn start_login(&mut self) {
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
    fn start_silent_login(&mut self, refresh_token: String) {
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

    /// 現在の動画に高評価を付ける（必要ならトークンを更新してから）を背景で開始。
    fn start_like(&mut self, video_id: String) {
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

    /// 1 フレーム描画する：mpv 動画 → egui UI の順に重ねて表示。
    fn redraw(&mut self) {
        self.poll_auth();

        let _ = self.gl_context.make_current(&self.gl_surface);

        let size = self.window.inner_size();
        let (w, h) = (size.width.max(1) as i32, size.height.max(1) as i32);

        // 下地として mpv の映像を既定フレームバッファ(fbo 0)へ描画。flip=true は GL 座標系向け。
        if let Err(e) = self.render_context.render::<()>(0, w, h, true) {
            eprintln!("mpv render error: {e}");
        }

        // mpv の現在状態を取得（ファイル未読込時はエラーになるので既定値で受ける）。
        let paused = self.mpv.get_property::<bool>("pause").unwrap_or(false);
        let time_pos = self.mpv.get_property::<f64>("time-pos").unwrap_or(0.0);
        let duration = self.mpv.get_property::<f64>("duration").unwrap_or(0.0);
        let volume = self.mpv.get_property::<f64>("volume").unwrap_or(100.0);
        let title = self.mpv.get_property::<String>("media-title").unwrap_or_default();

        // 一定時間操作がなければ UI を隠す。表示状態が変わったらカーソルも合わせる。
        let show_ui = self.last_activity.elapsed() < UI_HIDE_AFTER;
        if show_ui != self.ui_visible {
            self.window.set_cursor_visible(show_ui);
            self.ui_visible = show_ui;
        }

        // 上に egui の UI を重ねる。mpv は 'static 参照なのでクロージャ内から直接操作できる。
        // 認証 UI 用の状態。
        let logged_in = self.tokens.is_some();
        let auth_busy = self.auth_busy;
        let auth_status = self.auth_status.clone();
        let channel = self.channel.clone();
        let video_id = auth::extract_video_id(&self.current_url);

        let mpv = self.mpv;
        let window = &self.window;
        let url_input = &mut self.url_input;
        let mut to_load: Option<String> = None;
        let mut login_clicked = false;
        let mut like_clicked = false;
        self.egui_glow.run(window, |ctx| {
            // キーボードショートカットは UI 非表示中も有効（URL 欄入力中のみ無効）。
            if !ctx.wants_keyboard_input() {
                ctx.input(|i| {
                    if i.key_pressed(egui::Key::Space) {
                        let _ = mpv.set_property("pause", !paused);
                    }
                    if i.key_pressed(egui::Key::ArrowRight) {
                        let _ = mpv.command("seek", &["5", "relative"]);
                    }
                    if i.key_pressed(egui::Key::ArrowLeft) {
                        let _ = mpv.command("seek", &["-5", "relative"]);
                    }
                    if i.key_pressed(egui::Key::ArrowUp) {
                        let _ = mpv.set_property("volume", (volume + 5.0).min(130.0));
                    }
                    if i.key_pressed(egui::Key::ArrowDown) {
                        let _ = mpv.set_property("volume", (volume - 5.0).max(0.0));
                    }
                });
            }

            if !show_ui {
                return;
            }

            egui::TopBottomPanel::top("urlbar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("URL:");
                    let resp = ui.add(
                        egui::TextEdit::singleline(url_input)
                            .desired_width(f32::INFINITY)
                            .hint_text("YouTube の URL を入力して Enter"),
                    );
                    let entered = resp.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if entered {
                        to_load = Some(url_input.clone());
                    }
                });

                // タイトル表示（動画読込後のみ）。
                if !title.is_empty() {
                    ui.label(egui::RichText::new(&title).strong());
                }

                // 認証 / 高評価。
                ui.horizontal(|ui| {
                    if logged_in {
                        let who = channel.as_deref().unwrap_or("ログイン済み");
                        ui.label(format!("👤 {who}"));
                        let can_like = !auth_busy && video_id.is_some();
                        if ui
                            .add_enabled(can_like, egui::Button::new("👍 高評価"))
                            .on_hover_text("この動画に高評価を付けます")
                            .clicked()
                        {
                            like_clicked = true;
                        }
                    } else {
                        if ui
                            .add_enabled(!auth_busy, egui::Button::new("🔑 YouTube にログイン"))
                            .clicked()
                        {
                            login_clicked = true;
                        }
                    }
                    ui.label(&auth_status);
                });
            });

            egui::TopBottomPanel::bottom("controls").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // 再生 / 一時停止
                    let label = if paused { "▶" } else { "⏸" };
                    if ui.button(label).clicked() {
                        let _ = mpv.set_property("pause", !paused);
                    }

                    ui.label(format_time(time_pos));

                    // シークバー（duration が確定したときのみ操作可能）
                    let mut pos = time_pos;
                    let seekable = duration > 0.0;
                    let slider = egui::Slider::new(&mut pos, 0.0..=duration.max(0.1))
                        .show_value(false);
                    let resp = ui.add_enabled(seekable, slider);
                    // ドラッグ終了時・クリック時にシーク（毎フレームのシーク連発を避ける）。
                    if seekable && (resp.drag_stopped() || (resp.changed() && !resp.dragged())) {
                        let _ = mpv.set_property("time-pos", pos);
                    }

                    ui.label(format_time(duration));

                    // 音量
                    ui.separator();
                    ui.label("🔊");
                    let mut vol = volume;
                    if ui
                        .add(egui::Slider::new(&mut vol, 0.0..=130.0).fixed_decimals(0))
                        .changed()
                    {
                        let _ = mpv.set_property("volume", vol);
                    }
                });
            });
        });
        self.egui_glow.paint(window);

        let _ = self.gl_surface.swap_buffers(&self.gl_context);

        self.frames += 1;
        if self.verbose && self.frames % 120 == 0 {
            eprintln!("[frames] {}", self.frames);
        }

        // 一時停止中は mpv が再描画を駆動しないので、UI 表示中は自前で次フレームを要求して
        // 非表示への移行（カウントダウン）を進める。vsync で頻度は抑えられる。
        if paused && show_ui {
            self.window.request_redraw();
        }

        if let Some(url) = to_load {
            self.load(&url);
        }
        if login_clicked {
            self.start_login();
        }
        if like_clicked {
            if let Some(id) = video_id {
                self.start_like(id);
            }
        }
    }
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    initial_url: Option<String>,
    verbose: bool,
    state: Option<Running>,
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>, initial_url: Option<String>) -> Self {
        Self {
            proxy,
            initial_url,
            verbose: std::env::var("TALAVA_VERBOSE").is_ok(),
            state: None,
        }
    }

    /// ウィンドウ・GL コンテキスト・mpv・RenderContext を構築する。
    fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<Running> {
        // --- ウィンドウ + GL コンフィグ ---
        let window_attributes = Window::default_attributes()
            .with_title("Talava Player")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));

        let template = ConfigTemplateBuilder::new().with_alpha_size(8);
        let display_builder =
            DisplayBuilder::new().with_window_attributes(Some(window_attributes));

        let (window, gl_config) = display_builder
            .build(event_loop, template, |configs| {
                configs
                    .reduce(|acc, c| {
                        if c.num_samples() > acc.num_samples() {
                            c
                        } else {
                            acc
                        }
                    })
                    .expect("no GL config")
            })
            .map_err(|e| anyhow!("display build failed: {e}"))?;
        let window = window.ok_or_else(|| anyhow!("no window created"))?;

        let gl_display = gl_config.display();
        let raw_window_handle = window.window_handle()?.as_raw();

        // --- GL コンテキスト + サーフェス ---
        // mpv の OpenGL バックエンドに合わせ、デスクトップ GL Core 3.3 を明示する。
        let context_attributes = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::OpenGl(Some(Version::new(3, 3))))
            .with_profile(GlProfile::Core)
            .build(Some(raw_window_handle));
        let not_current = unsafe { gl_display.create_context(&gl_config, &context_attributes)? };

        let attrs: glutin::surface::SurfaceAttributes<WindowSurface> =
            window.build_surface_attributes(SurfaceAttributesBuilder::<WindowSurface>::new())?;
        let gl_surface = unsafe { gl_display.create_window_surface(&gl_config, &attrs)? };
        let gl_context = not_current.make_current(&gl_surface)?;

        // vsync。一時停止中に UI を消すための自前再描画ループの暴走を防ぐ。
        if let Some(one) = NonZeroU32::new(1) {
            let _ = gl_surface.set_swap_interval(&gl_context, SwapInterval::Wait(one));
        }

        // --- glow + egui ---
        let gl = unsafe {
            glow::Context::from_loader_function_cstr(|s| gl_display.get_proc_address(s).cast())
        };
        let egui_glow = egui_glow::EguiGlow::new(event_loop, Arc::new(gl), None, None, true);

        // --- mpv（Render API 利用時は vo=libmpv）---
        let verbose = self.verbose;
        let mpv = Mpv::with_initializer(|init| {
            init.set_property("vo", "libmpv")?;
            init.set_property("ytdl", true)?;
            init.set_property("ytdl-format", "bestvideo+bestaudio/best")?;
            if verbose {
                init.set_property("terminal", true)?;
                init.set_property("msg-level", "all=status")?;
            }
            Ok(())
        })
        .map_err(|e| anyhow!("mpv init failed: {e}"))?;

        // RenderContext は Mpv を借用するため、Mpv をリークして 'static 化する。
        let mpv: &'static Mpv = Box::leak(Box::new(mpv));

        // --- mpv Render Context（OpenGL）---
        let mut render_context = mpv
            .create_render_context(vec![
                RenderParam::ApiType(RenderParamApiType::OpenGl),
                RenderParam::InitParams(OpenGLInitParams {
                    get_proc_address,
                    ctx: gl_display.clone(),
                }),
            ])
            .map_err(|e| anyhow!("mpv render context failed: {e}"))?;

        // 新フレーム到着時にイベントループを起こして再描画させる。
        let proxy = self.proxy.clone();
        render_context.set_update_callback(move || {
            let _ = proxy.send_event(UserEvent::MpvRedraw);
        });

        // 認証まわりの初期化。
        let backend = auth::backend_base();
        let (auth_tx, auth_rx) = std::sync::mpsc::channel();
        let auth_status = "未ログイン".to_string();

        let mut running = Running {
            egui_glow,
            render_context,
            mpv,
            gl_context,
            gl_surface,
            window,
            url_input: String::new(),
            current_url: String::new(),
            frames: 0,
            verbose: self.verbose,
            last_activity: Instant::now(),
            ui_visible: true,
            proxy: self.proxy.clone(),
            backend,
            tokens: None,
            channel: None,
            auth_status,
            auth_busy: false,
            auth_tx,
            auth_rx,
        };

        // 保存済みリフレッシュトークンがあれば自動ログインを試みる。
        if let Some(rt) = auth::load_refresh_token() {
            running.start_silent_login(rt);
        }

        Ok(running)
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match self.init(event_loop) {
            Ok(mut running) => {
                if let Some(url) = self.initial_url.take() {
                    running.url_input = url.clone();
                    running.load(&url);
                }
                self.state = Some(running);
            }
            Err(e) => {
                eprintln!("initialization failed: {e:#}");
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: UserEvent) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
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

        let response = state.egui_glow.on_window_event(&state.window, &event);

        // マウス/キー操作があれば UI を再表示し、非表示までの時間を計り直す。
        match &event {
            WindowEvent::CursorMoved { .. }
            | WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
            | WindowEvent::KeyboardInput { .. } => {
                state.last_activity = Instant::now();
                if !state.ui_visible {
                    state.window.request_redraw();
                }
            }
            _ => {}
        }

        match event {
            WindowEvent::CloseRequested => {
                self.state = None;
                event_loop.exit();
                return;
            }
            WindowEvent::Resized(size) => {
                if let (Some(w), Some(h)) =
                    (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                {
                    state.gl_surface.resize(&state.gl_context, w, h);
                }
                state.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                state.redraw();
            }
            _ => {}
        }

        if response.repaint {
            state.window.request_redraw();
        }
    }
}

fn main() -> Result<()> {
    let initial_url = std::env::args().nth(1);
    if let Some(url) = &initial_url {
        println!("Talava Player - playing: {url}");
    } else {
        println!("Talava Player - URL 欄に貼り付けて Enter で再生");
    }

    ensure_ytdlp_on_path();

    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    let mut app = App::new(proxy, initial_url);
    event_loop.run_app(&mut app)?;
    Ok(())
}
