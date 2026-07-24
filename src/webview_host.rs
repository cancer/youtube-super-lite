//! WebView2 ホスト子窓（issue #16 PR1/PR2）。
//!
//! ライブ配信が SABR 詰みで mpv 再生できないケースの救済として、公式 IFrame プレーヤーを
//! WebView2 で埋め込むための土台。子窓生成・Environment/Controller 生成・固定
//! UserDataFolder（`%APPDATA%\YouTubeSuperLite\webview2`）への cookie 永続化は
//! [`WebviewMode`] の両モード共通で、ナビゲーション部分だけがモードで分岐する:
//!
//! - **PR1 (`WebviewMode::Probe`)**: 自前 HTML(iframe embed) を仮想ホスト
//!   （`https://ysl.embed.example/player.html`）から正規 origin で配信し、エラー153なしで
//!   プレーヤーが描画されることを確認する（[inbox/issue16-implementation-guide.md] §4）。
//! - **PR2 (`WebviewMode::Login`)**: 仮想ホストマッピングも player.html 書き出しもせず、
//!   トップレベルで Google ログイン URL へ `Navigate` する。ユーザーが対話的に Google
//!   ログインを完了すると、固定 UserDataFolder に cookie が永続化され、以後 Probe や本体が
//!   この cookie を使い回す（bot ゲート突破の下地。§5 PR2）。
//!
//! 経路切替（mpv⇄WebView2, PR3）・オーバーレイ/mpv の hide（PR4）・onError fallback（PR5）は
//! スコープ外。
//!
//! 構成は [`crate::dcomp_overlay::DcompOverlay`] と同じ要領（winit 親窓 `wid` を親に
//! `WS_CHILD` 子窓を作る）。ただし描画は WebView2 自身が行うため、DComp/D3D11/D2D は
//! 一切使わない。wndproc も自前ロジックを持たず `DefWindowProcW` に委譲するだけでよい
//! （このPRでは子窓への入力ハンドリングを配線しない。§PR4 の範疇）。
//!
//! エラー153(Video Player Configuration Error) は「embed をトップレベル文書として開く」
//! 「Referer が付かない」の2つで踏む。ここでは両方を避ける:
//! 1. トップレベル navigate せず、自前 HTML に `<iframe src="…/embed/<id>">` を置く。
//! 2. `NavigateToString`(null origin) ではなく `SetVirtualHostNameToFolderMapping` で
//!    仮想ホスト（`https://ysl.embed.example/`）にマップしたローカルフォルダの
//!    `player.html` を `Navigate` する（正規 origin/Referer が付く）。

#![cfg(windows)]

use anyhow::{anyhow, Result};
use std::path::PathBuf;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::WinRT::EventRegistrationToken;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, MoveWindow, RegisterClassW, WNDCLASSW,
    WS_CHILD, WS_CLIPSIBLINGS, WS_VISIBLE,
};

use webview2_com::Microsoft::Web::WebView2::Win32::{
    CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2Controller, ICoreWebView2Environment,
    ICoreWebView2EnvironmentOptions, ICoreWebView2_3, COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_ALLOW,
    COREWEBVIEW2_WEB_ERROR_STATUS,
};
use webview2_com::{
    take_pwstr, CoreWebView2EnvironmentOptions, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, NavigationCompletedEventHandler,
    NavigationStartingEventHandler,
};

/// 匿名検証用の固定 video_id（issue #16 PR1）。経路切替（resolve→WebView2 配線）は PR3 の
/// 範疇でここではまだ配線しない。「lofi hip hop radio - beats to relax/study to」は
/// 常時ライブの定番配信で、動作確認用の固定 ID として長期間安定して存在する。
const DEV_FIXED_VIDEO_ID: &str = "jfKfPfyJRdk";

/// 自前 HTML を配信する仮想ホスト名。`.local` は mDNS 予約のため避ける
/// （[inbox/issue16-implementation-guide.md] §4.4）。
const VIRTUAL_HOST: &str = "ysl.embed.example";

/// WebView2 子窓の動作モード（issue #16）。子窓・Environment・Controller の生成は両モード共通で、
/// ナビゲーション部分だけがモードで分岐する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebviewMode {
    /// PR1: 自前 HTML(iframe embed) を仮想ホストから配信し 153 回避を確認する（匿名でよい）。
    Probe,
    /// PR2: トップレベルで Google ログイン URL へナビゲートし、固定 UserDataFolder に
    /// cookie を永続化する（ユーザーが対話的にログインを完了する導線）。
    Login,
}

/// Login モードでトップレベルにナビゲートする Google ログイン URL（issue #16 PR2）。
///
/// `continue` を `www.youtube.com` に向けることで、ログイン後に YouTube へ着地して cookie 文脈を
/// 保つ。`youtube-nocookie.com` は cookie（＝ログイン文脈）を捨てるため使わない。
/// 純粋関数として切り出しているのは、副作用の無いこの生成ロジックをユニットテストするため。
fn login_url() -> &'static str {
    "https://accounts.google.com/ServiceLogin?continue=https%3A%2F%2Fwww.youtube.com%2F&hl=ja"
}

/// WebView2 を貼った子窓（winit 親窓 `wid` の3枚目の子窓）。
/// `environment`/`controller` は COM の強参照。フィールドとして保持し続けないと
/// ブラウザプロセスが解放されて描画が止まる。
pub struct WebviewHost {
    hwnd: HWND,
    /// 生存させ続けるためだけに保持（COM 参照）。直接は使わない。
    #[allow(dead_code)]
    environment: ICoreWebView2Environment,
    controller: ICoreWebView2Controller,
    cw: i32,
    ch: i32,
    /// 生成時のモード（issue #16 PR3）。`navigate_embed` は Probe モードで
    /// 仮想ホストマッピング済みの前提でしか呼べないので、モードを保持して弾けるようにする。
    mode: WebviewMode,
    /// player.html を書き出すローカルフォルダ（`<user_data_dir>/www`）。
    /// Probe モードでのみ実体を持つ（Login では未使用）。navigate_embed で再書き出しに使う。
    www_dir: PathBuf,
}

impl Drop for WebviewHost {
    fn drop(&mut self) {
        // WebView2 のブラウザプロセスを明示的に終了させる（wravery/webview2-rs 準拠）。
        unsafe {
            let _ = self.controller.Close();
        }
    }
}

impl WebviewHost {
    /// winit 親窓（HWND を i64 で受ける）の子として作成する。
    /// `mode` でナビゲーション挙動が分岐する（[`WebviewMode`]）。子窓・Environment・
    /// Controller の生成は両モード共通。
    pub fn new(parent_wid: i64, mode: WebviewMode) -> Result<Self> {
        // WebView2 は STA 前提の COM API。多重初期化は S_FALSE で成功扱いになる
        // （`HRESULT::ok()` は失敗 HRESULT のみ Err を返すので、既に別スレッドが
        // 同じ STA で初期化済みなら実質ここは黙って通る）。
        unsafe {
            if let Err(e) = CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok() {
                eprintln!("[webview2] CoInitializeEx: {e:?}（続行。既に別モードで初期化済みの可能性）");
            }
        }

        let parent = HWND(parent_wid as *mut core::ffi::c_void);

        // 子窓生成（DcompOverlay::new と同じ要領）。描画は WebView2 自身が行うため、
        // wndproc は DefWindowProcW に委譲するだけでよい（このPRでは入力を配線しない）。
        let (hwnd, cw, ch) = unsafe {
            let hinstance = GetModuleHandleW(None)?;
            let class = w!("YslWebView2Host");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance.into(),
                lpszClassName: class,
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);

            let mut rc = RECT::default();
            let _ = GetClientRect(parent, &mut rc);
            let cw = (rc.right - rc.left).max(1);
            let ch = (rc.bottom - rc.top).max(1);

            let hwnd = CreateWindowExW(
                Default::default(),
                class,
                w!("webview2"),
                WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
                0,
                0,
                cw,
                ch,
                parent,
                None,
                hinstance,
                None,
            )?;
            (hwnd, cw, ch)
        };

        // UserDataFolder は固定パス（起動ごとに変えない＝ cookie を使い回す前提。PR2 の核）。
        // 両モード共通: ここに溜まった cookie を Probe/本体が使い回す。
        let user_data_dir = webview2_user_data_dir()?;
        std::fs::create_dir_all(&user_data_dir)?;

        // Environment 生成オプション。`--autoplay-policy` は生成時にしか渡せない
        // （条件C。後付け不可なのでここに同梱する）。
        let options = CoreWebView2EnvironmentOptions::default();
        unsafe {
            options.set_additional_browser_arguments(
                "--autoplay-policy=no-user-gesture-required".to_string(),
            );
            // 条件Bの下地（PR2 で本格運用）: OS ログインは使わず専用プロファイルに限定する。
            options.set_exclusive_user_data_folder_access(true);
        }
        let options: ICoreWebView2EnvironmentOptions = options.into();

        let user_data_dir_hstring = HSTRING::from(user_data_dir.as_os_str());

        let environment: ICoreWebView2Environment = {
            let (tx, rx) = std::sync::mpsc::channel();
            CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
                Box::new(move |handler| unsafe {
                    CreateCoreWebView2EnvironmentWithOptions(
                        PCWSTR::null(),
                        &user_data_dir_hstring,
                        &options,
                        &handler,
                    )
                    .map_err(webview2_com::Error::WindowsError)
                }),
                Box::new(move |result, env| {
                    result?;
                    tx.send(env).expect("send over mpsc channel");
                    Ok(())
                }),
            )
            .map_err(|e| anyhow!("CreateCoreWebView2EnvironmentWithOptions 失敗: {e:?}"))?;
            rx.recv()
                .map_err(|_| anyhow!("webview2 environment 受信チャネルが閉じた"))?
                .ok_or_else(|| anyhow!("webview2 environment が None"))?
        };

        let controller: ICoreWebView2Controller = {
            let (tx, rx) = std::sync::mpsc::channel();
            let env = environment.clone();
            CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
                Box::new(move |handler| unsafe {
                    env.CreateCoreWebView2Controller(hwnd, &handler)
                        .map_err(webview2_com::Error::WindowsError)
                }),
                Box::new(move |result, controller| {
                    result?;
                    tx.send(controller).expect("send over mpsc channel");
                    Ok(())
                }),
            )
            .map_err(|e| anyhow!("CreateCoreWebView2Controller 失敗: {e:?}"))?;
            rx.recv()
                .map_err(|_| anyhow!("webview2 controller 受信チャネルが閉じた"))?
                .ok_or_else(|| anyhow!("webview2 controller が None"))?
        };

        unsafe {
            controller.SetBounds(RECT {
                left: 0,
                top: 0,
                right: cw,
                bottom: ch,
            })?;
            controller.SetIsVisible(true)?;
        }

        let webview = unsafe { controller.CoreWebView2()? };

        // 自前 HTML（iframe embed）を配信するローカルフォルダ。Probe モードで実体化する
        // （navigate_embed で再書き出しするためモード外でもパスは記録する）。
        let www_dir = user_data_dir.join("www");

        // ここからがモード分岐。子窓・Environment・Controller・SetBounds/SetIsVisible までは共通。
        match mode {
            WebviewMode::Probe => {
                // 起動のたびに書き直す（バイナリ側のテンプレート更新をそのまま反映させるため）。
                std::fs::create_dir_all(&www_dir)?;
                std::fs::write(www_dir.join("player.html"), player_html(DEV_FIXED_VIDEO_ID))?;

                // 正規 origin/Referer を与えるため、仮想ホストにローカルフォルダをマップして
                // その URL を Navigate する（NavigateToString や data/blob URL は Referer が
                // 付かず153を踏むので不可）。
                let webview3: ICoreWebView2_3 = windows::core::Interface::cast(&webview)?;

                // ナビゲーション診断（issue #16 PR1 のゴール確認）。真っ白の原因が
                // (A)iframe ロード成功だが匿名 bot ゲート/待機画面（＝実質成功） か
                // (B)iframe ロード自体が失敗（153相当・origin/Referer 拒否） かを確定させるため、
                // 主フレームと iframe のナビゲーション結果を probe.log に追記する。
                // 登録は Navigate 前に行う（イベントを取りこぼさないため）。ハンドラ内は panic させず握る。
                let log_path = user_data_dir.join("probe.log");
                register_nav_diagnostics(&webview, log_path);

                let www_dir_hstring = HSTRING::from(www_dir.as_os_str());
                unsafe {
                    webview3.SetVirtualHostNameToFolderMapping(
                        w!("ysl.embed.example"),
                        &www_dir_hstring,
                        COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_ALLOW,
                    )?;
                    webview.Navigate(w!("https://ysl.embed.example/player.html"))?;
                }

                eprintln!(
                    "[native] webview2 子窓を生成（Probe・video_id={DEV_FIXED_VIDEO_ID} 固定・仮想ホスト https://{VIRTUAL_HOST}/）"
                );
            }
            WebviewMode::Login => {
                // 仮想ホストマッピングも player.html 書き出しもしない。トップレベルで
                // Google ログイン画面へ Navigate し、ユーザーが対話的にログインを完了すると
                // 固定 UserDataFolder に cookie が永続化される（以後 Probe/本体が使い回す）。
                // ログイン完了の自動判定はしない（手動運用）。
                let log_path = user_data_dir.join("probe.log");
                register_main_nav_completed(&webview, log_path, "LoginNavigationCompleted");

                let login_url_hstring = HSTRING::from(login_url());
                unsafe {
                    webview.Navigate(&login_url_hstring)?;
                }

                eprintln!("[native] webview2 子窓を生成（Login・{}）", login_url());
            }
        }

        Ok(Self {
            hwnd,
            environment,
            controller,
            cw,
            ch,
            mode,
            www_dir,
        })
    }

    /// 経路切替（issue #16 PR3）でライブ SABR 詰みが検知されたとき、指定 `video_id` の
    /// 公式 IFrame プレーヤーを WebView2 上で再ロードする。
    ///
    /// 手順:
    /// 1. `player_html(video_id)` で HTML を生成し `<user_data_dir>/www/player.html` に上書き
    /// 2. `Navigate(https://ysl.embed.example/player.html)` を呼ぶ
    ///
    /// 仮想ホストマッピングは `WebviewMode::Probe` での `new()` で登録済みの前提。
    /// `WebviewMode::Login` で生成したホストではマッピングが張られていないため、
    /// 呼び出し自体を誤りとして `bail!` で明示的に落とす（呼び出し側は Probe 起動時のみ想定）。
    pub fn navigate_embed(&mut self, video_id: &str) -> Result<()> {
        if self.mode != WebviewMode::Probe {
            return Err(anyhow!(
                "navigate_embed は Probe モードでのみ呼び出せる（現在: {:?}）",
                self.mode
            ));
        }

        // player.html を video_id 差し替えで上書き（テンプレート・仮想ホストは既存を流用）。
        std::fs::create_dir_all(&self.www_dir)?;
        std::fs::write(self.www_dir.join("player.html"), player_html(video_id))?;

        // 仮想ホストマッピングは new() の Probe 分岐で登録済み。同 URL への再 Navigate は
        // 上書きした player.html を再取得させる（iframe の embed URL も再構築される）。
        let webview = unsafe { self.controller.CoreWebView2()? };
        unsafe {
            webview.Navigate(w!("https://ysl.embed.example/player.html"))?;
        }
        eprintln!("[native] webview2 navigate_embed (video_id={video_id})");
        Ok(())
    }

    /// WebView2 ホスト子窓の可視を切替える（PR4 経路切替用）。
    /// Controller の SetIsVisible と子窓 ShowWindow の両方を揃える必要がある:
    /// 前者だけだと子窓は残って入力を吸い続け、後者だけだと WebView 側が
    /// 「非可視」と認識できず描画/アニメーションを止められない。
    #[allow(dead_code)]
    pub fn set_visible(&self, visible: bool) -> Result<()> {
        use windows::Win32::Foundation::BOOL;
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE, SW_SHOWNA};
        unsafe {
            self.controller.SetIsVisible(BOOL(visible as i32))?;
            let _ = ShowWindow(self.hwnd, if visible { SW_SHOWNA } else { SW_HIDE });
        }
        Ok(())
    }

    /// WebView2 ホスト子窓を兄弟の最前面へ引き上げる（PR4 経路切替時のみ）。
    /// フォーカスは奪わない（SWP_NOACTIVATE）。DcompOverlay の ensure_topmost と違い、
    /// 毎フレーム呼ぶのではなく Mpv→Webview 遷移時に一度だけ呼ぶ。
    #[allow(dead_code)]
    pub fn bring_to_top(&self) {
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        };
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// WebView2 ホスト子窓の HWND を isize で返す（親直下の子窓列挙で
    /// 「WebView2 は除外」と判定するために使う）。
    #[allow(dead_code)]
    pub fn hwnd_raw(&self) -> isize {
        self.hwnd.0 as isize
    }

    /// 親窓のリサイズに追従する（DcompOverlay::resize と同じ流儀）。
    pub fn resize(&mut self, w: i32, h: i32) {
        let w = w.max(1);
        let h = h.max(1);
        if w == self.cw && h == self.ch {
            return;
        }
        self.cw = w;
        self.ch = h;
        unsafe {
            let _ = MoveWindow(self.hwnd, 0, 0, w, h, true);
            if let Err(e) = self.controller.SetBounds(RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            }) {
                eprintln!("[webview2] SetBounds 失敗: {e:?}");
            }
        }
    }
}

/// probe.log（と stderr）へ1行追記する。診断専用（issue #16 PR1）。失敗は握る。
fn probe_log(path: &std::path::Path, line: &str) {
    use std::io::Write;
    eprintln!("[webview2-probe] {line}");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

/// 主フレームの `NavigationCompleted` と iframe の `FrameNavigationStarting`/
/// `FrameNavigationCompleted` を購読し、URI・IsSuccess・WebErrorStatus を probe.log に出す。
/// これで「真っ白」の原因が iframe ロード成功後の匿名 bot ゲート(A)か、iframe ロード失敗(B)かを
/// 切り分ける。ハンドラ内は panic させず、取得失敗は握って続行する。
///
/// 診断専用（issue #16 PR1 のゴール計測）。ここで再生挙動・経路・UI は一切変えない。
fn register_nav_diagnostics(webview: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2, log_path: PathBuf) {
    // 主フレーム（player.html 自体）のロード完了。origin/Referer が効いているか、
    // youtube 側が何を返したか（DocumentTitle）の傍証にもなる。Login 側と共通のヘルパで購読する。
    register_main_nav_completed(webview, log_path.clone(), "MainNavigationCompleted");

    // iframe（youtube.com/embed）のナビゲート開始。決め手となる「どの URI へ行こうとしたか」。
    {
        let log_path = log_path.clone();
        let handler = NavigationStartingEventHandler::create(Box::new(move |_sender, args| {
            let mut uri = String::new();
            unsafe {
                if let Some(args) = args.as_ref() {
                    let mut p = windows::core::PWSTR::null();
                    if args.Uri(&mut p).is_ok() && !p.is_null() {
                        uri = take_pwstr(p);
                    }
                }
            }
            probe_log(&log_path, &format!("FrameNavigationStarting uri={uri}"));
            Ok(())
        }));
        let mut token = EventRegistrationToken::default();
        unsafe {
            if let Err(e) = webview.add_FrameNavigationStarting(&handler, &mut token) {
                eprintln!("[webview2-probe] add_FrameNavigationStarting 登録失敗: {e:?}");
            }
        }
    }

    // iframe のナビゲート完了。IsSuccess=false かつ WebErrorStatus が原因（=B）、
    // IsSuccess=true なら中身は表示されている（真っ白なら bot ゲート/待機画面=A）。
    {
        let log_path = log_path.clone();
        let handler = NavigationCompletedEventHandler::create(Box::new(move |_sender, args| {
            let mut is_success = false;
            let mut status = COREWEBVIEW2_WEB_ERROR_STATUS(0);
            unsafe {
                if let Some(args) = args.as_ref() {
                    let mut b = windows::Win32::Foundation::BOOL(0);
                    if args.IsSuccess(&mut b).is_ok() {
                        is_success = b.as_bool();
                    }
                    let _ = args.WebErrorStatus(&mut status);
                }
            }
            probe_log(
                &log_path,
                &format!("FrameNavigationCompleted IsSuccess={is_success} WebErrorStatus={status:?}"),
            );
            Ok(())
        }));
        let mut token = EventRegistrationToken::default();
        unsafe {
            if let Err(e) = webview.add_FrameNavigationCompleted(&handler, &mut token) {
                eprintln!("[webview2-probe] add_FrameNavigationCompleted 登録失敗: {e:?}");
            }
        }
    }

    probe_log(&log_path, "--- webview2 probe start (handlers registered) ---");
}

/// 主フレームの `NavigationCompleted` を `label` 付きで購読する共通ヘルパ（issue #16 PR1/PR2）。
/// IsSuccess/WebErrorStatus/Source/DocumentTitle を抽出し `<label> uri=… IsSuccess=… …` 形式で
/// probe.log/stderr に出す。Probe（"MainNavigationCompleted"）と Login（"LoginNavigationCompleted"）で
/// 差分はログ接頭辞だけなので、本体をここに集約する。ハンドラ内は panic させず取得失敗は握る。
///
/// Login モードでは embed 用の iframe ハンドラ（[`register_nav_diagnostics`] の
/// `FrameNavigation*`）は登録せず、この主フレーム購読1本だけで
/// 「youtube.com にログイン状態で戻った」ことを目視/ログで追う（自動判定はしない・手動運用）。
fn register_main_nav_completed(
    webview: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
    log_path: PathBuf,
    label: &'static str,
) {
    let handler = NavigationCompletedEventHandler::create(Box::new(move |sender, args| {
        let mut is_success = false;
        let mut status = COREWEBVIEW2_WEB_ERROR_STATUS(0);
        let mut source = String::new();
        let mut title = String::new();
        unsafe {
            if let Some(args) = args.as_ref() {
                let mut b = windows::Win32::Foundation::BOOL(0);
                if args.IsSuccess(&mut b).is_ok() {
                    is_success = b.as_bool();
                }
                let _ = args.WebErrorStatus(&mut status);
            }
            if let Some(wv) = sender.as_ref() {
                let mut p = windows::core::PWSTR::null();
                if wv.Source(&mut p).is_ok() && !p.is_null() {
                    source = take_pwstr(p);
                }
                let mut t = windows::core::PWSTR::null();
                if wv.DocumentTitle(&mut t).is_ok() && !t.is_null() {
                    title = take_pwstr(t);
                }
            }
        }
        probe_log(
            &log_path,
            &format!(
                "{label} uri={source} IsSuccess={is_success} WebErrorStatus={status:?} DocumentTitle={title:?}"
            ),
        );
        Ok(())
    }));
    let mut token = EventRegistrationToken::default();
    unsafe {
        if let Err(e) = webview.add_NavigationCompleted(&handler, &mut token) {
            eprintln!("[webview2-probe] {label} add_NavigationCompleted 登録失敗: {e:?}");
        }
    }
}

/// WebView2 の固定 UserDataFolder（`%APPDATA%\YouTubeSuperLite\webview2`）。
/// 起動ごとに変えない＝ cookie を使い回す前提（PR2 の核）。
fn webview2_user_data_dir() -> Result<PathBuf> {
    let appdata = std::env::var_os("APPDATA")
        .ok_or_else(|| anyhow!("%APPDATA% 環境変数が見つかりません"))?;
    Ok(PathBuf::from(appdata)
        .join("YouTubeSuperLite")
        .join("webview2"))
}

/// 自前 HTML（構成B: トップレベルではなく iframe に embed を置く）。
/// `allow="autoplay"` と Environment 側の `--autoplay-policy` フラグは条件Cの下地として残すが、
/// **プローブ時に音を鳴らさない**ため URL 側では `autoplay=1` を付けず `mute=1` を付ける
/// （[[ysl-debug-mute]] と整合。PR1 のゴールは「153なしで描画」で自動再生は不要）。
fn player_html(video_id: &str) -> String {
    format!(
        r#"<!doctype html>
<html>
<head>
<meta charset="utf-8" />
<style>
  html, body {{ margin: 0; padding: 0; width: 100%; height: 100%; background: #000; overflow: hidden; }}
  iframe {{ display: block; border: 0; width: 100%; height: 100%; }}
</style>
</head>
<body>
<iframe
  src="https://www.youtube.com/embed/{video_id}?enablejsapi=1&mute=1"
  referrerpolicy="strict-origin-when-cross-origin"
  allow="autoplay; encrypted-media; fullscreen; picture-in-picture"
  allowfullscreen
></iframe>
</body>
</html>
"#
    )
}

/// 子窓の WndProc。描画は WebView2 自身が行うため、既定処理に委譲するだけでよい
/// （このPRでは子窓への入力ハンドリングを配線しない。§PR4 の範疇）。
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Login モードのナビゲート先 URL（issue #16 PR2）が Google ログインで、`continue` が
    /// YouTube に戻る形になっていること。Win32/COM に依存しない唯一の純粋ロジック。
    #[test]
    fn login_url_targets_google_login_returning_to_youtube() {
        let u = login_url();
        assert!(u.starts_with("https://"), "https:// で始まること: {u}");
        assert!(u.contains("accounts.google.com"), "Google ログインであること: {u}");
        assert!(u.contains("continue"), "continue パラメータを持つこと: {u}");
        assert!(u.contains("youtube"), "continue が youtube に戻ること: {u}");
    }
}
