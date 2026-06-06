//! P2 移行実証バイナリ: mpv(D3D11)動画の上に透過 2D オーバーレイを重ねる。
//!
//! inbox/opengl-to-native-migration.md の P2「2D レイヤ＋合成」。検証する 3 点:
//!   1. 動画(mpv の D3D11 出力)の上に透過 2D レイヤを重ねて表示できる
//!   2. 無操作が続くとオーバーレイ(コントローラ)を自動で隠す／カーソルが動いたら再表示
//!   3. 入力振り分け: コントローラ帯はオーバーレイが受け取り、それ以外は下の動画へ透過
//!
//! 合成方式の判断: libmpv2 の render API は OpenGL/SW のみで、mpv の D3D11 出力を
//! DirectComposition の visual へ直接バインドする公開 API が無い。そこで計画が想定する
//! 「mpv 子窓＋透過オーバーレイ窓」の構成を採る:
//!   - ベース窓: mpv を `wid` で埋め込み D3D11 直描画（OpenGL 不使用）
//!   - オーバーレイ窓: WS_EX_LAYERED のトップレベル透過窓。GDI で最小コントローラを描画
//! 製品版では透過 2D を Direct2D + DirectComposition に置き換える想定だが、レイヤ合成・
//! 自動非表示・入力振り分けのモデル検証には本構成で十分。
//!
//! 使い方: cargo run --bin overlay_probe -- av://lavfi:testsrc2=size=1280x720:rate=30

#[cfg(not(windows))]
fn main() {
    eprintln!("overlay_probe は Windows 専用です。");
}

#[cfg(windows)]
use std::cell::RefCell;
#[cfg(windows)]
use libmpv2::Mpv;
#[cfg(windows)]
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};

#[cfg(windows)]
#[derive(Default)]
struct ProbeState {
    mpv: Option<&'static Mpv>,
    base: Option<HWND>,
    overlay: Option<HWND>,
    paused: bool,
    last_cursor: POINT,
    idle_ticks: u32,
    visible: bool,
    /// オーバーレイクライアント座標でのコントローラ帯の矩形。
    bar: RECT,
}

#[cfg(windows)]
thread_local! {
    static STATE: RefCell<ProbeState> = RefCell::new(ProbeState::default());
}

#[cfg(windows)]
const TIMER_ID: usize = 1;
#[cfg(windows)]
const TICK_MS: u32 = 200;
#[cfg(windows)]
const HIDE_AFTER_MS: u32 = 3000;
#[cfg(windows)]
const BAR_H: i32 = 64;

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use anyhow::anyhow;
    use windows::core::w;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, GetClientRect, LoadCursorW, RegisterClassW, SetLayeredWindowAttributes,
        SetTimer, SetWindowLongPtrW, ShowWindow, CW_USEDEFAULT, GWLP_USERDATA, IDC_ARROW,
        LWA_COLORKEY, SW_SHOW, WNDCLASSW, WS_CLIPCHILDREN, WS_EX_LAYERED, WS_EX_NOACTIVATE,
        WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE,
    };

    unsafe {
        let hinstance = GetModuleHandleW(None)?;

        // --- ベース窓（mpv 埋め込み先） ---
        let base_class = w!("OverlayProbeBase");
        let wc_base = WNDCLASSW {
            lpfnWndProc: Some(base_wndproc),
            hInstance: hinstance.into(),
            lpszClassName: base_class,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        if RegisterClassW(&wc_base) == 0 {
            return Err(anyhow!("RegisterClassW(base) failed"));
        }

        let base = CreateWindowExW(
            Default::default(),
            base_class,
            w!("overlay probe (mpv D3D11 + 透過2D)"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1280,
            720,
            None,
            None,
            hinstance,
            None,
        )?;

        // --- オーバーレイ窓（透過レイヤード） ---
        let ov_class = w!("OverlayProbeLayer");
        let wc_ov = WNDCLASSW {
            lpfnWndProc: Some(overlay_wndproc),
            hInstance: hinstance.into(),
            lpszClassName: ov_class,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        if RegisterClassW(&wc_ov) == 0 {
            return Err(anyhow!("RegisterClassW(overlay) failed"));
        }

        // WS_EX_LAYERED: 透過。WS_EX_TOPMOST/NOACTIVATE: 常に最前面・フォーカスを奪わない。
        // クリックスルーは WS_EX_TRANSPARENT ではなく WM_NCHITTEST で帯のみ受ける方式にする
        // （帯=オーバーレイ、それ以外=下の動画へ透過）。ただし最初は TRANSPARENT も付けず、
        // NCHITTEST で制御する。
        let overlay = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            ov_class,
            w!("overlay"),
            WS_POPUP | WS_VISIBLE,
            0,
            0,
            10,
            10,
            base,
            None,
            hinstance,
            None,
        )?;
        // カラーキー: 黒(0x000000)を透明に。コントローラ帯は黒以外で描く。
        SetLayeredWindowAttributes(overlay, COLORREF(0x000000), 0, LWA_COLORKEY)?;

        // mpv を D3D11 でベース窓へ埋め込み（OpenGL 不使用、P1 と同じ）。
        let wid: i64 = base.0 as isize as i64;
        let mpv = Mpv::with_initializer(|init| {
            init.set_property("wid", wid)?;
            init.set_property("vo", "gpu-next")?;
            init.set_property("gpu-api", "d3d11")?;
            init.set_property("hwdec", "auto")?;
            init.set_property("ytdl", false)?;
            init.set_property("force-window", "yes")?;
            init.set_property("idle", "yes")?;
            Ok(())
        })
        .map_err(|e| anyhow!("mpv init failed: {e}"))?;
        let mpv: &'static Mpv = Box::leak(Box::new(mpv));

        if let Some(path) = std::env::args().nth(1) {
            eprintln!("[overlay_probe] loadfile {path}");
            let _ = mpv.command("loadfile", &[&path]);
        }

        // overlay の wndproc から mpv を引けるよう USERDATA に格納。
        SetWindowLongPtrW(overlay, GWLP_USERDATA, mpv as *const Mpv as isize);

        // 状態初期化。
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.mpv = Some(mpv);
            s.base = Some(base);
            s.overlay = Some(overlay);
            s.visible = true;
        });

        // ベース窓に定期タイマ: カーソル移動検出・自動非表示・オーバーレイ追従。
        SetTimer(base, TIMER_ID, TICK_MS, None);
        let _ = ShowWindow(overlay, SW_SHOW);

        // 初回レイアウト。
        let mut rc = RECT::default();
        let _ = GetClientRect(base, &mut rc);
        reposition_overlay(base, overlay, &rc);

        // メッセージループ。
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, GetMessageW, TranslateMessage, MSG,
        };
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    Ok(())
}

/// オーバーレイをベース窓のクライアント領域に一致させ、コントローラ帯の矩形を更新する。
#[cfg(windows)]
unsafe fn reposition_overlay(base: HWND, overlay: HWND, client: &RECT) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE};
    let w = client.right - client.left;
    let h = client.bottom - client.top;
    let mut origin = POINT { x: 0, y: 0 };
    let _ = ClientToScreen(base, &mut origin);
    let _ = SetWindowPos(
        overlay,
        None,
        origin.x,
        origin.y,
        w,
        h,
        SWP_NOACTIVATE,
    );
    // コントローラ帯: 下端に幅いっぱい、高さ BAR_H。
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.bar = RECT {
            left: 0,
            top: (h - BAR_H).max(0),
            right: w,
            bottom: h,
        };
    });
}

#[cfg(windows)]
unsafe extern "system" fn base_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    use windows::Win32::Graphics::Gdi::InvalidateRect;
    use windows::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, GetClientRect, GetCursorPos, KillTimer, PostQuitMessage, ShowWindow,
        SW_HIDE, SW_SHOWNOACTIVATE, WM_DESTROY, WM_SIZE, WM_TIMER,
    };
    match msg {
        WM_TIMER => {
            // カーソル移動でアクティビティ検出 → 自動非表示の制御。
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                let moved = p.x != s.last_cursor.x || p.y != s.last_cursor.y;
                s.last_cursor = p;
                if moved {
                    s.idle_ticks = 0;
                } else {
                    s.idle_ticks = s.idle_ticks.saturating_add(1);
                }
                let should_hide = s.idle_ticks * TICK_MS >= HIDE_AFTER_MS;
                if let Some(ov) = s.overlay {
                    if should_hide && s.visible {
                        let _ = ShowWindow(ov, SW_HIDE);
                        s.visible = false;
                    } else if !should_hide && !s.visible {
                        let _ = ShowWindow(ov, SW_SHOWNOACTIVATE);
                        s.visible = true;
                    }
                }
            });
            LRESULT(0)
        }
        WM_SIZE => {
            // ベース窓リサイズ → オーバーレイ追従。
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            STATE.with(|s| {
                let s = s.borrow();
                if let Some(ov) = s.overlay {
                    reposition_overlay(hwnd, ov, &rc);
                    let _ = InvalidateRect(ov, None, true);
                }
            });
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, TIMER_ID);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(windows)]
unsafe extern "system" fn overlay_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, SetBkMode, SetTextColor,
        TextOutW, PAINTSTRUCT, TRANSPARENT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, GetWindowLongPtrW, GWLP_USERDATA, HTCLIENT, HTTRANSPARENT, WM_LBUTTONDOWN,
        WM_NCHITTEST, WM_PAINT,
    };
    match msg {
        WM_NCHITTEST => {
            // 入力振り分け: コントローラ帯の中だけ HTCLIENT（オーバーレイが受ける）、
            // それ以外は HTTRANSPARENT で下の動画へ透過させる。
            let sx = (lparam.0 & 0xFFFF) as i16 as i32;
            let sy = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let in_bar = STATE.with(|s| {
                let s = s.borrow();
                // bar はオーバーレイクライアント座標。スクリーン座標を変換して判定。
                use windows::Win32::Graphics::Gdi::ScreenToClient;
                let mut pt = POINT { x: sx, y: sy };
                let _ = ScreenToClient(hwnd, &mut pt);
                pt.x >= s.bar.left && pt.x < s.bar.right && pt.y >= s.bar.top && pt.y < s.bar.bottom
            });
            if in_bar {
                LRESULT(HTCLIENT as isize)
            } else {
                LRESULT(HTTRANSPARENT as isize)
            }
        }
        WM_LBUTTONDOWN => {
            // 帯クリック → 再生/一時停止トグル。
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Mpv;
            if !ptr.is_null() {
                let mpv = &*ptr;
                let paused = STATE.with(|s| {
                    let mut s = s.borrow_mut();
                    s.paused = !s.paused;
                    s.paused
                });
                let _ = mpv.set_property("pause", paused);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            // 全面を黒(=カラーキーで透明)で塗る。
            let mut rc = RECT::default();
            let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rc);
            let black = CreateSolidBrush(COLORREF(0x000000));
            FillRect(hdc, &rc, black);
            let _ = DeleteObject(black);
            // コントローラ帯（不透明・濃いグレー）。
            let (bar, paused) = STATE.with(|s| {
                let s = s.borrow();
                (s.bar, s.paused)
            });
            let bar_brush = CreateSolidBrush(COLORREF(0x00202020));
            FillRect(hdc, &bar, bar_brush);
            let _ = DeleteObject(bar_brush);
            // ラベル。
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, COLORREF(0x00FFFFFF));
            let label: Vec<u16> = if paused {
                "⏸ 停止中  ｜ 帯クリックで再生/一時停止  ｜ 無操作3秒で自動非表示"
            } else {
                "▶ 再生中  ｜ 帯クリックで再生/一時停止  ｜ 無操作3秒で自動非表示"
            }
            .encode_utf16()
            .collect();
            let _ = TextOutW(hdc, bar.left + 16, bar.top + 20, &label);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
