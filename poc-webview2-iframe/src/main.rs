//! Issue #16 PoC: WebView2 に YouTube IFrame embed をロードし、
//! ライブ配信が実際に再生されるかを確認する使い捨て検証プログラム。
//!
//! 決定事項の詳細は inbox/issue16-webview2-poc-plan.md を参照。
//! 本実装ではなく go/no-go を確定させるための最小コードなので、
//! 本体(youtube-super-lite)側のコードとの共通化・重複排除はしない。

use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;

use webview2_com::{
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
};

use windows::core::*;
use windows::Win32::Foundation::{E_POINTER, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BitBlt, ClientToScreen, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC,
    ReleaseDC, SelectObject, UpdateWindow, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    HBITMAP, SRCCOPY,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{SetProcessDpiAwareness, PROCESS_PER_MONITOR_DPI_AWARE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
    GetMessageW, GetWindowLongPtrW, PostQuitMessage, RegisterClassW, SetForegroundWindow,
    SetTimer, SetWindowLongPtrW, ShowWindow, TranslateMessage, CW_USEDEFAULT, GWLP_USERDATA, MSG,
    SW_SHOW, WM_CLOSE, WM_DESTROY, WM_TIMER, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

use webview2_com::Microsoft::Web::WebView2::Win32::{
    CreateCoreWebView2Environment, ICoreWebView2Controller,
};

/// キャプチャの間隔・回数・強制終了までの時間は inbox/issue16-webview2-poc-plan.md の決定値。
const CAPTURE_INTERVAL_MS: u32 = 5_000;
const CAPTURE_COUNT: u32 = 5;
const EXIT_AFTER_MS: u32 = 30_000;

const TIMER_ID_CAPTURE: usize = 1;
const TIMER_ID_EXIT: usize = 2;

const WINDOW_WIDTH: i32 = 960;
const WINDOW_HEIGHT: i32 = 540;

struct AppState {
    // 読み出しはしないが、Drop されると WebView2 の描画が止まるため保持し続ける。
    #[allow(dead_code)]
    controller: ICoreWebView2Controller,
    out_dir: PathBuf,
    capture_index: u32,
}

fn main() {
    let arg = match env::args().nth(1) {
        Some(a) => a,
        None => {
            eprintln!("usage: poc-webview2-iframe <channel_id>");
            eprintln!("       poc-webview2-iframe login   # WebView2内でGoogleにログインし、cookieをWebView2既定プロファイルに残す");
            std::process::exit(1);
        }
    };

    let result = if arg == "login" { run_login() } else { run(&arg) };

    if let Err(err) = result {
        eprintln!("poc-webview2-iframe failed: {err:?}");
        std::process::exit(1);
    }
}

/// WebView2の既定ユーザーデータフォルダ（exeのパスに紐づき、プロセスをまたいで永続する）で
/// 実際にGoogleへログインするためのモード。認証情報の入力は本人が行う必要があるため
/// （合成入力での自動ログインは行わない）、自動終了タイマーは張らずウィンドウを手動で
/// 閉じるまで待つ。閉じた時点でログイン後のcookieがプロファイルに保存されている。
/// 通常モード(run)は同じ既定プロファイルを使うため、ここでのログインがそのまま引き継がれる。
fn run_login() -> Result<()> {
    println!("[poc] login モード: 開いたウィンドウでGoogleアカウントにログインしてください。");
    println!("[poc] ログインが完了したらウィンドウを閉じてください（自動終了はしません）。");

    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE)?;
    }

    let hwnd = create_window()?;
    let controller = create_webview2_controller(hwnd)?;

    unsafe {
        let mut rect = RECT::default();
        GetClientRect(hwnd, &mut rect)?;
        controller.SetBounds(rect)?;
        controller.SetIsVisible(true)?;
    }

    let webview = unsafe { controller.CoreWebView2()? };
    unsafe {
        webview.Navigate(&HSTRING::from(
            "https://accounts.google.com/ServiceLogin?service=youtube&continue=https://www.youtube.com/",
        ))?;
    }

    let state = Box::new(AppState {
        controller,
        out_dir: PathBuf::new(),
        capture_index: CAPTURE_COUNT, // loginモードはキャプチャ不要（タイマー自体を張らない）
    });
    let state_ptr = Box::into_raw(state);

    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
        // 自動終了・自動キャプチャのタイマーは張らない。ユーザーが手動で閉じるまで待つ。
    }

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    println!("[poc] login モード終了。プロファイルにログイン状態が保存されました。");
    Ok(())
}

fn run(channel_id: &str) -> Result<()> {
    // WebView2のトップレベルnavigationで直接 /embed/live_stream?channel=... を開くと
    // Refererが送られず「エラー153(PLAYABILITY_ERROR_CODE_EMBEDDER_IDENTITY_MISSING_REFERRER)」
    // でサーバー側に拒否される（検証済み。WebView2固有の問題ではなくcurlでも同じ結果になる）。
    // 実サイトが埋め込む形を再現するため、127.0.0.1のローカルHTTPサーバーから配信した
    // 親HTML内のiframeとしてロードし、実URLのRefererを送らせる。
    let iframe_src =
        format!("https://www.youtube.com/embed/live_stream?channel={channel_id}&autoplay=1&mute=1");
    let html = format!(
        r#"<!doctype html><html><body style="margin:0;background:#000">
<iframe width="100%" height="100%" frameborder="0"
  referrerpolicy="strict-origin-when-cross-origin"
  allow="autoplay"
  src="{iframe_src}"></iframe>
</body></html>"#
    );

    let out_dir = PathBuf::from("poc-shots").join(channel_id);
    fs::create_dir_all(&out_dir).expect("create output dir for screenshots");
    println!("[poc] channel={channel_id}");
    println!("[poc] iframe_src={iframe_src}");

    let server = tiny_http::Server::http("127.0.0.1:0")
        .expect("start local http server for referrer test");
    let port = server.server_addr().to_ip().expect("ip addr").port();
    let embed_url = format!("http://127.0.0.1:{port}/");
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let response = tiny_http::Response::from_string(html.clone()).with_header(
                tiny_http::Header::from_bytes(
                    &b"Content-Type"[..],
                    &b"text/html; charset=utf-8"[..],
                )
                .unwrap(),
            );
            let _ = request.respond(response);
        }
    });
    println!("[poc] embed_url={embed_url}");
    println!("[poc] out_dir={}", out_dir.display());

    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE)?;
    }

    let hwnd = create_window()?;

    // WebView2の既定ユーザーデータフォルダを使う（exeのパスに紐づき、プロセスをまたいで
    // 永続する）。`login` モードで一度ログインしておけば、ここで同じプロファイルの
    // cookieが引き継がれる（inbox/issue16-webview2-poc-plan.md の追加検証）。
    let controller = create_webview2_controller(hwnd)?;

    unsafe {
        let mut rect = RECT::default();
        GetClientRect(hwnd, &mut rect)?;
        controller.SetBounds(rect)?;
        controller.SetIsVisible(true)?;
    }

    let webview = unsafe { controller.CoreWebView2()? };
    unsafe {
        webview.Navigate(&HSTRING::from(embed_url.as_str()))?;
    }

    let state = Box::new(AppState {
        controller,
        out_dir,
        capture_index: 0,
    });
    let state_ptr = Box::into_raw(state);

    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
        SetTimer(Some(hwnd), TIMER_ID_CAPTURE, CAPTURE_INTERVAL_MS, None);
        SetTimer(Some(hwnd), TIMER_ID_EXIT, EXIT_AFTER_MS, None);
    }

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // AppState の解放は WM_DESTROY 側(window_proc)が担う。ここで再度 drop すると
        // 二重解放になる（state_ptr は WM_DESTROY 後は既に無効なポインタ）。
    }

    Ok(())
}

fn create_window() -> Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class_name = w!("PocWebView2IframeWindow");

        let window_class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassW(&window_class);

        let hwnd = CreateWindowExW(
            Default::default(),
            class_name,
            w!("poc-webview2-iframe"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            WINDOW_WIDTH,
            WINDOW_HEIGHT,
            None,
            None,
            Some(instance.into()),
            None,
        )?;

        Ok(hwnd)
    }
}

fn create_webview2_controller(hwnd: HWND) -> Result<ICoreWebView2Controller> {
    let environment = {
        let (tx, rx) = mpsc::channel();
        CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
            Box::new(move |environmentcreatedhandler| unsafe {
                CreateCoreWebView2Environment(&environmentcreatedhandler)
                    .map_err(webview2_com::Error::WindowsError)
            }),
            Box::new(move |error_code, environment| {
                error_code?;
                tx.send(environment.ok_or_else(|| Error::from(E_POINTER)))
                    .expect("send environment over mpsc channel");
                Ok(())
            }),
        )
        .map_err(webview2_error_to_windows)?;

        rx.recv()
            .expect("receive environment over mpsc channel")?
    };

    let controller = {
        let (tx, rx) = mpsc::channel();
        CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
            Box::new(move |handler| unsafe {
                environment
                    .CreateCoreWebView2Controller(hwnd, &handler)
                    .map_err(webview2_com::Error::WindowsError)
            }),
            Box::new(move |error_code, controller| {
                error_code?;
                tx.send(controller.ok_or_else(|| Error::from(E_POINTER)))
                    .expect("send controller over mpsc channel");
                Ok(())
            }),
        )
        .map_err(webview2_error_to_windows)?;

        rx.recv().expect("receive controller over mpsc channel")?
    };

    Ok(controller)
}

fn webview2_error_to_windows(err: webview2_com::Error) -> Error {
    match err {
        webview2_com::Error::WindowsError(e) => e,
        other => Error::new(E_POINTER, format!("{other:?}")),
    }
}

extern "system" fn window_proc(hwnd: HWND, msg: u32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    unsafe {
        let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;

        match msg {
            WM_TIMER => {
                if state_ptr.is_null() {
                    return LRESULT(0);
                }
                let state = &mut *state_ptr;
                match w_param.0 {
                    TIMER_ID_CAPTURE => {
                        if state.capture_index < CAPTURE_COUNT {
                            let path = state
                                .out_dir
                                .join(format!("shot-{:02}.png", state.capture_index));
                            // 画面座標BitBltのため、他ウィンドウに覆われていると別画面が写る。
                            // 撮影前に自ウィンドウを前面化する（本体devtoolsの/screenshotと同じ手法。
                            // 合成入力(SendInput等)は使わず、自プロセスの自ウィンドウのz-orderのみ操作）。
                            let _ = SetForegroundWindow(hwnd);
                            match capture_client_png(hwnd) {
                                Some(png) => {
                                    if let Err(e) = fs::write(&path, png) {
                                        eprintln!("[poc] スクリーンショット保存失敗: {e}");
                                    } else {
                                        println!("[poc] saved {}", path.display());
                                    }
                                }
                                None => eprintln!("[poc] スクリーンショット取得失敗"),
                            }
                            state.capture_index += 1;
                        }
                    }
                    TIMER_ID_EXIT => {
                        let _ = DestroyWindow(hwnd);
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                if !state_ptr.is_null() {
                    drop(Box::from_raw(state_ptr));
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, w_param, l_param),
        }
    }
}

/// ウィンドウのクライアント領域を画面から BitBlt で取り込み、PNG バイト列にする。
/// src/ui/shell.rs の capture_client_png と同じ手法（画面座標BitBlt）だが、
/// PoC専用として意図的に重複実装している（inbox/issue16-webview2-poc-plan.md）。
unsafe fn capture_client_png(hwnd: HWND) -> Option<Vec<u8>> {
    let mut rc = RECT::default();
    GetClientRect(hwnd, &mut rc).ok()?;
    let w = (rc.right - rc.left).max(1);
    let h = (rc.bottom - rc.top).max(1);
    let mut org = POINT { x: 0, y: 0 };
    let _ = ClientToScreen(hwnd, &mut org);

    let screen = GetDC(None);
    let mem = CreateCompatibleDC(Some(screen));
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
    let dib = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
        .ok()
        .filter(|b: &HBITMAP| !b.0.is_null());

    let result = (|| {
        let dib = dib?;
        let old = SelectObject(mem, dib.into());
        let _ = BitBlt(mem, 0, 0, w, h, Some(screen), org.x, org.y, SRCCOPY);
        let n = (w * h * 4) as usize;
        let src = std::slice::from_raw_parts(bits as *const u8, n);
        // BGRA(top-down) → RGB（PNG保存はアルファ不要のためRGBに落とす）。
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for chunk in src.chunks_exact(4) {
            rgb.push(chunk[2]);
            rgb.push(chunk[1]);
            rgb.push(chunk[0]);
        }
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut wtr = enc.write_header().ok()?;
            wtr.write_image_data(&rgb).ok()?;
        }
        SelectObject(mem, old);
        let _ = DeleteObject(dib.into());
        Some(out)
    })();

    let _ = DeleteDC(mem);
    ReleaseDC(None, screen);
    result
}
