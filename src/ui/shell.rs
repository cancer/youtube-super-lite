//! ネイティブ版エントリ（`--native`）。OpenGL を一切作らず、mpv を `wid` 経由で
//! D3D11 にウィンドウへ直接描画させる。`NativeRunning`（shell）が `ysl_core` の各ドメイン
//! （`account`/`playback`/`content`/`chat`）を個別フィールドとして所有し、`flows` の跨ぎ
//! system 経由で駆動する（旧 `Controller` は Issue #11 の再編で消滅）。
//!
//! ここには Win32/winit の暗黙知（地雷）を集約する。以後ほぼ不変（Issue #11 PR U）。
//! last_activity の更新は shell（このファイル）だけが行う。`actions`/`present` は
//! 「操作があったか」を戻り値で返すだけで、跨ぎ状態には直接触れない（§9.2）。
//! 触る前に docs/design 配下の「shell 変更時の手動チェックリスト」を参照すること。

use anyhow::Result;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::window::{Window, WindowId};

use ysl_core::player::Player;
use ysl_core::yt::{auth, recommend, subscriptions, history};
use ysl_core::{account, chat, content, flows, playback};
use crate::{Codec, Quality, UserEvent};

use super::actions::UiAction;

/// 再生経路（PR4）。mpv（既定）と WebView2（SABR 詰みライブの iframe 埋め込み経路）で
/// 使う子窓が完全に排他になるため、遷移時に片方だけを可視にする判定へ使う。
/// ライブ SABR 検知のみが Mpv→Webview 遷移を起こし、新 URL の再生開始（`play`）は
/// 常に Mpv から始める（webview2 mode は同一 URL の中で完結する救済経路）。
#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PlaybackMode {
    Mpv,
    Webview,
}

/// 一覧の表示ソース。1/2/3 キーで切替。
#[derive(Clone, Copy, PartialEq)]
pub(super) enum ListSource {
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
    /// WebView2 内で Google ログイン（issue #16 PR2・`--webview-login`）を行うか。
    /// 立っている場合はオーバーレイを生成せず mpv も再生しない使い捨てログインセッションになる
    /// （cookie を固定 UserDataFolder に永続化）。
    webview_login: bool,
    state: Option<NativeRunning>,
}

pub(super) struct NativeRunning {
    /// ウィンドウは所有権保持のため抱える（drop するとウィンドウが閉じ、mpv の wid も無効になる）。
    #[allow(dead_code)]
    pub(super) window: Window,
    /// 親ウィンドウの Win32 HWND（i64）。オーバーレイの追従描画に使う。
    pub(super) parent_wid: i64,
    /// 背景スレッドがメインループを起こすためのコールバック（proxy を包む。lib は winit を知らない）。
    pub(super) waker: ysl_core::Waker,
    pub(super) playback: playback::Playback,
    pub(super) account: account::Account,
    /// 1 動画 : 1 セッション。`None` にする（≠フィールドの手動リセット）ことが「停止」の全て。
    pub(super) chat: Option<chat::ChatSession>,
    pub(super) recommend: content::Feed<recommend::VideoItem>,
    pub(super) channel_view: content::ChannelView,
    pub(super) subs: content::Feed<subscriptions::SubVideo>,
    pub(super) history: content::Feed<history::HistoryItem>,
    pub(super) playlist: content::Playlist,
    pub(super) avatars: content::AvatarCache,
    /// URL 入力欄の内容（英数字キーで編集、Enter で再生）。URL は空白を含まないため
    /// Space は再生/一時停止に温存できる（フォーカス概念は持たない）。
    pub(super) url_input: String,
    /// Ctrl 押下状態（Ctrl+V 貼り付け判定用）。
    #[allow(dead_code)]
    pub(super) ctrl: bool,
    /// 一覧表示中か、選択位置、表示ソース。
    pub(super) list_open: bool,
    pub(super) list_sel: usize,
    pub(super) list_source: ListSource,
    /// ケバブで開いているカードメニューの index（無ければ None）。
    pub(super) card_menu_open: Option<usize>,
    /// チャット（右パネル）表示中か。
    pub(super) chat_open: bool,
    /// EQ パネル（コントローラ帯の EQ ボタンで開閉）表示中か。
    pub(super) eq_open: bool,
    /// チャット（コメント）の文字サイズ（px）。UI（A-/A+）で増減する。
    pub(super) chat_font_px: f32,
    /// チャット欄の幅（ウィンドウ幅比 0.15..=0.6）。左端ドラッグで変更する。
    pub(super) chat_width_ratio: f32,
    /// チャットのスクロール量（最新から遡ったメッセージ数。0=最新に追従）。
    pub(super) chat_scroll: usize,
    /// アプリ窓がフォーカスを持っているか。失っている間はオーバーレイを隠す
    /// （他アプリの上にオーバーレイが残らないようにする）。
    pub(super) focused: bool,
    /// 動画に重ねる透過オーバーレイ（子窓 + DirectComposition）。Windows のみ。
    /// init 失敗時のみ None。
    #[cfg(windows)]
    pub(super) dcomp_overlay: Option<crate::dcomp_overlay::DcompOverlay>,
    /// WebView2 ホスト子窓（issue #16 PR1）。ライブ SABR 詰み救済（公式 IFrame 埋め込み）の
    /// 土台。Windows のみ。init 失敗時のみ None。**現時点では mpv と併存させるだけで、
    /// 経路切替・hide 配線はしない**（PR3/PR4 の範疇）。
    #[cfg(windows)]
    pub(super) webview_host: Option<crate::webview_host::WebviewHost>,
    /// 自動非表示用: 最後に操作（マウス移動/キー/クリック）があった時刻。
    #[cfg(windows)]
    pub(super) last_activity: Instant,
    #[cfg(windows)]
    pub(super) overlay_visible: bool,
    /// 現在の再生経路（PR4）。SABR 詰みで WebView2 に切替わったかを保持し、
    /// 描画スキップ・子窓 hide の判定に使う。
    #[cfg(windows)]
    pub(super) mode: PlaybackMode,
    /// mpv が親窓 `parent_wid` 直下に自動生成する VO 子窓の HWND リスト（PR4）。
    /// Mpv⇄Webview 遷移時に EnumChildWindows で列挙し直し、
    /// overlay/webview 以外を「mpv の出力」とみなして ShowWindow で切替える。
    /// mpv 側は VO 子窓を直接公開しないためここで捕捉する。
    #[cfg(windows)]
    pub(super) mpv_child_hwnds: Vec<isize>,
    /// dev-tools（--enable-dev-tools）からの要求受信口。None なら無効。
    pub(super) devtools_rx: Option<std::sync::mpsc::Receiver<crate::devtools::Command>>,
    /// 保留中のスクリーンショット返信先。前面化＋再描画を待ってからキャプチャするため遅延させる。
    pub(super) pending_shot: Option<std::sync::mpsc::Sender<Vec<u8>>>,
    /// スクショ前に待つフレーム数（前面化と合成の反映待ち）。
    pub(super) shot_delay: u32,
    /// 最後に永続化した設定スナップショット（現在値と異なれば保存する）。
    pub(super) saved_settings: crate::settings::Settings,
    /// 最後に設定を保存した時刻（保存をデバウンスするため）。
    pub(super) last_settings_save: Instant,
}

impl NativeApp {
    // CLI フラグ（main.rs の CliArgs）が1つずつ増えるたびにこのコンストラクタ引数も増える。
    // 引数はいずれも独立した起動オプションでグルーピングに意味が薄いため、束ねずに列挙する。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        proxy: EventLoopProxy<UserEvent>,
        initial_url: Option<String>,
        verbose: bool,
        backend: String,
        initial_volume: Option<f64>,
        enable_dev_tools: bool,
        webview_login: bool,
    ) -> Self {
        Self {
            proxy,
            initial_url,
            verbose,
            backend,
            initial_volume,
            enable_dev_tools,
            webview_login,
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
        // ただし --webview-login は使い捨てログインセッションで通常再生をしない。mpv に loadfile
        // しないのはもちろん、URL 欄にも入れない（入れると「欄に出ているのに再生されない」不整合に
        // なる。login は URL を扱わないモード。issue #16 PR2）。
        let mut url_input = String::new();
        if self.webview_login {
            // login は URL を扱わないモード。CLI 指定 URL は破棄する（欄にも入れず再生もしない）。
            self.initial_url = None;
        } else if let Some(url) = self.initial_url.take() {
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
        // --webview-login 時は生成しない: オーバーレイ子窓は WS_CHILD|WS_VISIBLE で全入力を所有し
        // ensure_topmost が毎フレーム最前面へ引くため、WebView2 のログインフォームにクリック/キーが
        // 届かず手動ログインできなくなる（issue #16 PR2・ガイド §4）。
        #[cfg(windows)]
        let dcomp_overlay = if self.webview_login {
            None
        } else {
            match crate::dcomp_overlay::DcompOverlay::new(wid) {
                Ok(o) => {
                    eprintln!("[native] dcomp overlay (子窓+DirectComposition) を使用");
                    Some(o)
                }
                Err(e) => {
                    eprintln!("[native] dcomp overlay init failed: {e:#}");
                    None
                }
            }
        };

        // WebView2 ホスト子窓（issue #16 PR1/PR2）は常時生成し、PlaybackMode（mpv/webview）で
        // 可視制御する。`--webview-login` 時のみ Login モードで生成し、通常起動は Probe モード
        // （SABR 詰みライブの iframe 埋め込み用の待機ホスト）。
        #[cfg(windows)]
        let webview_host = {
            use crate::webview_host::{WebviewHost, WebviewMode};
            let mode = if self.webview_login {
                WebviewMode::Login
            } else {
                WebviewMode::Probe
            };
            match WebviewHost::new(wid, mode) {
                Ok(w) => Some(w),
                Err(e) => {
                    eprintln!("[native] webview2 host init failed: {e:#}");
                    None
                }
            }
        };

        // 前回保存した UI 設定（文字サイズ・チャット幅）を引き継ぐ。
        let settings = crate::settings::load();

        // 前回の EQ 設定を mpv に反映（af はグローバルプロパティなので再生開始前でも有効）。
        playback::set_eq(&mut playback_state, settings.eq);

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
            eq_open: false,
            chat_font_px: settings.chat_font_px,
            chat_width_ratio: settings.chat_width_ratio,
            chat_scroll: 0,
            focused: true,
            #[cfg(windows)]
            dcomp_overlay,
            #[cfg(windows)]
            webview_host,
            #[cfg(windows)]
            last_activity: Instant::now(),
            #[cfg(windows)]
            overlay_visible: true,
            #[cfg(windows)]
            mode: PlaybackMode::Mpv,
            #[cfg(windows)]
            mpv_child_hwnds: Vec::new(),
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
    pub(super) fn player(&self) -> &Player {
        self.playback.player()
    }

    pub(super) fn current_url(&self) -> &str {
        self.playback.current_url()
    }

    pub(super) fn is_live(&self) -> bool {
        self.playback.is_live()
    }

    pub(super) fn quality(&self) -> Quality {
        self.playback.quality()
    }

    pub(super) fn codec(&self) -> Codec {
        self.playback.codec()
    }

    pub(super) fn eq(&self) -> ysl_core::types::EqParams {
        self.playback.eq()
    }

    /// 背景スレッド（認証/API/解決）の結果を取り込む。proxy 起床時に呼ぶ。
    pub(super) fn poll_all(&mut self) {
        // リプレイチャット用に再生位置を共有。
        self.playback
            .player_offset_ms()
            .store((self.playback.player().time_pos() * 1000.0) as i64, std::sync::atomic::Ordering::Relaxed);
        // アクセストークンが失効していたら自動更新（ログインセッションの継続）。
        account::ensure_fresh_token(&mut self.account, &self.waker);
        self.poll_auth();
        self.poll_chat();
        content::poll_feed(&mut self.recommend, &mut self.avatars, &self.waker);
        content::poll_feed(&mut self.subs, &mut self.avatars, &self.waker);
        content::poll_feed(&mut self.history, &mut self.avatars, &self.waker);
        content::poll_channel_view(&mut self.channel_view, &mut self.avatars, &self.waker);
        content::poll_avatars(&mut self.avatars);
        content::poll_playlist(&mut self.playlist);
        playback::poll_gpu(&mut self.playback);
        // resolve 結果が SABR 詰みライブなら WebView2 経路へ委譲する（issue #16 PR3）。
        // 経路要求はコア側でなく shell が受け取り、shell に閉じている WebView2 の
        // 具体的な操作（navigate_embed）はここで発行する（コアを WebView2 に汚染させない）。
        if let Some(playback::PendingRoute::Webview { video_id }) =
            playback::poll_resolve(&mut self.playback)
        {
            #[cfg(windows)]
            {
                // navigate_embed 成功時のみ mode を Webview に倒す。失敗（webview_host 生成失敗や
                // ナビゲーション失敗）なら mpv/オーバーレイの可視は現状維持（子窓 hide の偏りを避ける）。
                // borrowck: webview_host の可変借用と self.apply_mode_visibility() が
                // 衝突するため、遷移可否を bool に落としてから借用を切って self を再取得する。
                let route_taken = match self.webview_host.as_mut() {
                    Some(w) => match w.navigate_embed(&video_id) {
                        Ok(()) => true,
                        Err(e) => {
                            eprintln!("[route] navigate_embed failed: {e:#}");
                            false
                        }
                    },
                    None => {
                        eprintln!(
                            "[route] WebView2 経路が要求されたが webview_host が初期化されていない (video_id={video_id})"
                        );
                        false
                    }
                };
                if route_taken {
                    self.mode = PlaybackMode::Webview;
                    self.apply_mode_visibility();
                }
            }
            #[cfg(not(windows))]
            {
                eprintln!(
                    "[route] WebView2 経路は Windows 専用（video_id={video_id}）"
                );
            }
        }
        // native 直 URL が mpv で再生失敗していれば、並列に用意した中継(サイドカー)へ切替える。
        playback::check_fallback(&mut self.playback);
    }

    /// dev-tools（--enable-dev-tools）からの要求を処理する。毎フレーム呼ぶ。
    /// last_activity の更新はここ（shell）だけが行う。`devtools_action` は
    /// 「操作が既知だったか」を返すだけで、自分では触らない（§9.2）。
    pub(super) fn poll_devtools(&mut self) {
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
                    #[cfg(windows)]
                    if known {
                        self.last_activity = Instant::now();
                    }
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

    /// 文字サイズ・チャット幅・EQ に変更があれば保存する。`force` 時はデバウンスを無視（終了時用）。
    pub(super) fn maybe_save_settings(&mut self, force: bool) {
        let cur = crate::settings::Settings {
            chat_font_px: self.chat_font_px,
            chat_width_ratio: self.chat_width_ratio,
            eq: self.playback.eq(),
        };
        if cur == self.saved_settings {
            return;
        }
        if !force && self.last_settings_save.elapsed() < Duration::from_millis(800) {
            return; // デバウンス（ドラッグ中の連続変更で書きすぎない）。
        }
        crate::settings::save(cur);
        self.saved_settings = cur;
        self.last_settings_save = Instant::now();
    }

    /// mpv/オーバーレイ vs WebView2 の可視を排他的に切替える（PR4）。
    ///
    /// mpv 側は自動生成された VO 子窓を直接公開しないため、`parent_wid` 直下の
    /// 子窓を [`refresh_mpv_child_hwnds`] で列挙して overlay/webview 以外を hide/show する。
    /// 描画そのものの停止（`render` の呼出しスキップ）は `about_to_wait` の
    /// mode 判定に任せる（ここでは可視のみ）。
    #[cfg(windows)]
    pub(super) fn apply_mode_visibility(&mut self) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE, SW_SHOWNA};

        let mpv_mode = self.mode == PlaybackMode::Mpv;

        if let Some(o) = self.dcomp_overlay.as_ref() {
            o.set_visible(mpv_mode);
        }
        if let Some(w) = self.webview_host.as_ref() {
            if let Err(e) = w.set_visible(!mpv_mode) {
                eprintln!("[route] webview set_visible failed: {e:#}");
            }
            if !mpv_mode {
                // Mpv→Webview 遷移直後の1回だけ z-order を引き上げる。
                // 以後 mpv の VO 子窓は hide 済みなので毎フレーム再主張しなくてよい。
                w.bring_to_top();
            }
        }

        self.refresh_mpv_child_hwnds();
        for hwnd in &self.mpv_child_hwnds {
            unsafe {
                let _ = ShowWindow(
                    HWND(*hwnd as *mut core::ffi::c_void),
                    if mpv_mode { SW_SHOWNA } else { SW_HIDE },
                );
            }
        }
    }

    /// `parent_wid` 直下の子窓を1階層列挙し、overlay/webview 以外を
    /// mpv の VO 子窓とみなして `mpv_child_hwnds` に集める（PR4）。
    ///
    /// EnumChildWindows は全孫を列挙するため、`GetAncestor(hwnd, GA_PARENT) == parent_wid` で
    /// 直下だけに絞る（mpv の VO は親の直下、DComp/WebView2 のさらに孫は対象外）。
    #[cfg(windows)]
    fn refresh_mpv_child_hwnds(&mut self) {
        use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{EnumChildWindows, GetAncestor, GA_PARENT};

        struct Ctx {
            parent: isize,
            overlay: Option<isize>,
            webview: Option<isize>,
            out: Vec<isize>,
        }

        unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let ctx = &mut *(lparam.0 as *mut Ctx);
            let parent = GetAncestor(hwnd, GA_PARENT);
            if parent.0 as isize != ctx.parent {
                return BOOL(1); // 続行（直下でない孫は無視）
            }
            let h = hwnd.0 as isize;
            if Some(h) == ctx.overlay || Some(h) == ctx.webview {
                return BOOL(1);
            }
            ctx.out.push(h);
            BOOL(1)
        }

        let mut ctx = Ctx {
            parent: self.parent_wid as isize,
            overlay: self.dcomp_overlay.as_ref().map(|o| o.hwnd_raw()),
            webview: self.webview_host.as_ref().map(|w| w.hwnd_raw()),
            out: Vec::new(),
        };
        unsafe {
            let _ = EnumChildWindows(
                HWND(self.parent_wid as *mut core::ffi::c_void),
                Some(cb),
                LPARAM(&mut ctx as *mut Ctx as isize),
            );
        }
        self.mpv_child_hwnds = ctx.out;
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
            // Webview モード中はオーバーレイ子窓を hide 済みで、実マウス入力も届かない。
            // ensure_topmost（render 内）を呼ぶと WebView2 の下から z-order を奪ってしまうので、
            // 描画自体を skip する（overlay の内部状態と mode の二重管理を避けるため）。
            #[cfg(windows)]
            if _state.mode == PlaybackMode::Mpv && _state.dcomp_overlay.is_some() {
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
                    || _state.eq_open
                    || _state.last_activity.elapsed() < Duration::from_secs(3);
                let logged_in = _state.account.channel_name().is_some_and(|c| !c.is_empty());
                let auth_label = if logged_in {
                    format!("👤 {}", _state.account.channel_name().unwrap_or(""))
                } else {
                    format!("🔑 {}", _state.account.status())
                };
                let list_sel = _state.list_sel;
                let list_busy = list_open && _state.list_busy();
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
                    use ysl_core::yt::chat::ChatRun;
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
                    list_busy,
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
                    eq_open: _state.eq_open,
                    eq: _state.eq(),
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
                #[cfg(windows)]
                if let Some(w) = state.webview_host.as_mut() {
                    w.resize(size.width as i32, size.height as i32);
                }
                let _ = size;
            }
            WindowEvent::KeyboardInput { event, .. } => {
                // last_activity の更新はここ（shell）だけが行う。handle_keyboard は
                // 「操作があったか」を返すだけで、自分では触らない（Issue #11 PR U §9.2）。
                let acted = state.handle_keyboard(event);
                #[cfg(windows)]
                if acted {
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
