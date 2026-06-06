//! P3a/P3b 実証バイナリ: 透過オーバーレイを Direct2D + DirectWrite + WIC で描く実動コントローラ。
//!
//! inbox/opengl-to-native-migration.md の P3。P2(overlay_probe) の GDI 描画を Direct2D に置換し、
//! 製品 UI で必要になる 2D 描画スタックを実証する。
//! P3a（描画基盤）: ①AA 角丸矩形(Direct2D) ②日本語テキスト(DirectWrite) ③JPEG デコード(WIC→Bitmap)。
//! P3b（実動コントローラ）: mpv の再生状態(time-pos/duration/pause)を読み、再生/一時停止ボタン・
//!   シークバー(トラック/進捗/ノブ)・時間表示を描画し、クリックでトグル/絶対シークする。
//! すべて per-pixel alpha（半透明）で mpv(D3D11)動画の上に重ねる。
//!
//! 合成方式: P2 と同じ「mpv 子窓(D3D11) ＋ 透過オーバーレイ窓」。オーバーレイは
//! WS_EX_LAYERED で、Direct2D の DCRenderTarget をメモリ DC(32bpp premultiplied DIB)に
//! バインドして描画し、UpdateLayeredWindow(ULW_ALPHA) で per-pixel alpha 合成する。
//! （製品版では DirectComposition への移行も視野に入るが、本 probe は 2D 描画内容の検証が目的。）
//!
//! 使い方: cargo run --bin d2d_overlay_probe -- <video|url> [thumbnail.jpg]

#[cfg(not(windows))]
fn main() {
    eprintln!("d2d_overlay_probe は Windows 専用です。");
}

#[cfg(windows)]
use libmpv2::Mpv;
#[cfg(windows)]
use std::cell::RefCell;
#[cfg(windows)]
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
#[cfg(windows)]
use windows::Win32::Graphics::Direct2D::{ID2D1Bitmap, ID2D1DCRenderTarget, ID2D1Factory};
#[cfg(windows)]
use windows::Win32::Graphics::DirectWrite::{IDWriteFactory, IDWriteTextFormat};
#[cfg(windows)]
use windows::Win32::Graphics::Gdi::{HBITMAP, HDC, HGDIOBJ};

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
    bar: RECT,
    /// 再生/一時停止ボタン・シークバーのクリック判定用矩形（オーバーレイクライアント座標）。
    btn: RECT,
    seek: RECT,
    // Direct2D / DirectWrite / WIC
    factory: Option<ID2D1Factory>,
    dc_rt: Option<ID2D1DCRenderTarget>,
    dwrite: Option<IDWriteFactory>,
    text_format: Option<IDWriteTextFormat>,
    bitmap: Option<ID2D1Bitmap>,
    // メモリ DC + DIB（per-pixel alpha 用）
    mem_dc: HDC,
    dib: HBITMAP,
    old_obj: HGDIOBJ,
    dib_w: i32,
    dib_h: i32,
    dib_bits: usize, // *mut u8 を usize で保持
    frames: u32,
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
const BAR_H: i32 = 72;

/// 秒数を mm:ss / h:mm:ss にする。
#[cfg(windows)]
fn fmt_time(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "--:--".to_string();
    }
    let t = secs as u64;
    let (h, m, s) = (t / 3600, (t % 3600) / 60, t % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use anyhow::anyhow;
    use windows::core::w;
    use windows::Win32::Graphics::Direct2D::{
        D2D1CreateFactory, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_RENDER_TARGET_PROPERTIES,
        D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_GDI_COMPATIBLE,
        D2D1_FEATURE_LEVEL_DEFAULT,
    };
    use windows::Win32::Graphics::Direct2D::Common::{
        D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT,
    };
    use windows::Win32::Graphics::DirectWrite::{
        DWriteCreateFactory, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
        DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    };
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetClientRect, GetMessageW, LoadCursorW, RegisterClassW,
        SetTimer, SetWindowLongPtrW, ShowWindow, TranslateMessage, CW_USEDEFAULT, GWLP_USERDATA,
        IDC_ARROW, MSG, SW_SHOWNOACTIVATE, WNDCLASSW, WS_CLIPCHILDREN, WS_EX_LAYERED,
        WS_EX_NOACTIVATE, WS_EX_TOPMOST, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE,
    };

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let hinstance = GetModuleHandleW(None)?;

        // --- ベース窓（mpv 埋め込み先） ---
        let base_class = w!("D2DProbeBase");
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
            w!("d2d overlay probe (mpv D3D11 + Direct2D/DirectWrite/WIC)"),
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

        // --- オーバーレイ窓（透過レイヤード、ULW で描画） ---
        let ov_class = w!("D2DProbeLayer");
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
        let overlay = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
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

        // --- mpv を D3D11 でベース窓へ埋め込み（OpenGL 不使用） ---
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

        let mut args = std::env::args().skip(1);
        let media = args.next();
        let thumb = args.next();
        if let Some(path) = &media {
            eprintln!("[d2d] loadfile {path}");
            let _ = mpv.command("loadfile", &[path.as_str()]);
        }

        SetWindowLongPtrW(overlay, GWLP_USERDATA, mpv as *const Mpv as isize);

        // --- Direct2D / DirectWrite ファクトリ ---
        let factory: ID2D1Factory =
            D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let text_format: IDWriteTextFormat = dwrite.CreateTextFormat(
            w!("Yu Gothic UI"),
            None,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            22.0,
            w!("ja-jp"),
        )?;

        // DCRenderTarget（GDI 互換、premultiplied alpha）。
        let rt_props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 0.0,
            dpiY: 0.0,
            usage: D2D1_RENDER_TARGET_USAGE_GDI_COMPATIBLE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let dc_rt: ID2D1DCRenderTarget = factory.CreateDCRenderTarget(&rt_props)?;

        // --- WIC で JPEG をデコード → Direct2D Bitmap ---
        let bitmap: Option<ID2D1Bitmap> = match &thumb {
            Some(p) => match load_wic_bitmap(&dc_rt, p) {
                Ok(b) => {
                    eprintln!("[d2d] WIC: サムネイルをデコードしました ({p})");
                    Some(b)
                }
                Err(e) => {
                    eprintln!("[d2d] WIC デコード失敗 ({p}): {e}");
                    None
                }
            },
            None => {
                eprintln!("[d2d] サムネ未指定（WIC 検証はスキップ）");
                None
            }
        };

        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.mpv = Some(mpv);
            s.base = Some(base);
            s.overlay = Some(overlay);
            s.visible = true;
            s.factory = Some(factory);
            s.dc_rt = Some(dc_rt);
            s.dwrite = Some(dwrite);
            s.text_format = Some(text_format);
            s.bitmap = bitmap;
        });

        SetTimer(base, TIMER_ID, TICK_MS, None);
        let _ = ShowWindow(overlay, SW_SHOWNOACTIVATE);

        // 初回レイアウト＆描画。
        let mut rc = RECT::default();
        let _ = GetClientRect(base, &mut rc);
        layout(base, &rc);
        render();

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

/// オーバーレイのサイズ・位置・コントローラ帯矩形を base のクライアント領域に合わせて更新する。
#[cfg(windows)]
unsafe fn layout(base: HWND, client: &RECT) {
    let w = (client.right - client.left).max(1);
    let h = (client.bottom - client.top).max(1);
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.bar = RECT {
            left: 0,
            top: (h - BAR_H).max(0),
            right: w,
            bottom: h,
        };
    });
    let _ = base;
}

/// WIC で画像ファイルをデコードして Direct2D Bitmap を生成する。
#[cfg(windows)]
unsafe fn load_wic_bitmap(
    dc_rt: &ID2D1DCRenderTarget,
    path: &str,
) -> anyhow::Result<ID2D1Bitmap> {
    use anyhow::anyhow;
    use windows::core::HSTRING;
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, IWICImagingFactory, WICBitmapDitherTypeNone,
        WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnLoad,
        GUID_WICPixelFormat32bppPBGRA,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

    let wic: IWICImagingFactory =
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
    let decoder = wic.CreateDecoderFromFilename(
        &HSTRING::from(path),
        None,
        GENERIC_READ,
        WICDecodeMetadataCacheOnLoad,
    )?;
    let frame = decoder.GetFrame(0)?;
    let converter = wic.CreateFormatConverter()?;
    converter.Initialize(
        &frame,
        &GUID_WICPixelFormat32bppPBGRA,
        WICBitmapDitherTypeNone,
        None,
        0.0,
        WICBitmapPaletteTypeMedianCut,
    )?;
    let bmp = dc_rt
        .CreateBitmapFromWicBitmap(&converter, None)
        .map_err(|e| anyhow!("CreateBitmapFromWicBitmap: {e}"))?;
    Ok(bmp)
}

/// per-pixel alpha 用のメモリ DC + 32bpp top-down DIB を（必要なら）サイズに合わせて作り直す。
#[cfg(windows)]
unsafe fn ensure_dib(w: i32, h: i32) {
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC,
    };
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.mem_dc.0 != std::ptr::null_mut() && s.dib_w == w && s.dib_h == h {
            return;
        }
        // 既存を破棄。
        if !s.mem_dc.0.is_null() {
            if !s.old_obj.0.is_null() {
                SelectObject(s.mem_dc, s.old_obj);
            }
            if !s.dib.0.is_null() {
                let _ = DeleteObject(s.dib);
            }
            let _ = DeleteDC(s.mem_dc);
        }
        let mem_dc = CreateCompatibleDC(None);
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
        let dib =
            CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap_or(HBITMAP(std::ptr::null_mut()));
        let old = SelectObject(mem_dc, dib);
        let _ = HDC::default();
        s.mem_dc = mem_dc;
        s.dib = dib;
        s.old_obj = old;
        s.dib_w = w;
        s.dib_h = h;
        s.dib_bits = bits as usize;
    });
}

/// オーバーレイを Direct2D で描画し、UpdateLayeredWindow で per-pixel alpha 合成する。
#[cfg(windows)]
unsafe fn render() {
    use windows::core::w;
    use windows::Win32::Foundation::{COLORREF, POINT, RECT, SIZE};
    use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_POINT_2F, D2D_RECT_F};
    use windows::Win32::Graphics::Direct2D::{
        D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_ROUNDED_RECT,
        D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
    };
    use windows::Win32::Graphics::DirectWrite::DWRITE_MEASURING_MODE_NATURAL;
    use windows::Win32::Graphics::Gdi::{ClientToScreen, AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION};
    use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

    let (base, overlay, bar) = STATE.with(|s| {
        let s = s.borrow();
        (s.base, s.overlay, s.bar)
    });
    let (Some(base), Some(overlay)) = (base, overlay) else {
        return;
    };
    let w = bar.right.max(1);
    let h = bar.bottom.max(1);
    ensure_dib(w, h);

    let (dc_rt, text_format, bitmap, mem_dc, dib_bits) = STATE.with(|s| {
        let s = s.borrow();
        (
            s.dc_rt.clone(),
            s.text_format.clone(),
            s.bitmap.clone(),
            s.mem_dc,
            s.dib_bits,
        )
    });
    let (Some(dc_rt), Some(text_format)) = (dc_rt, text_format) else {
        return;
    };

    let full = RECT {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    if dc_rt.BindDC(mem_dc, &full).is_err() {
        return;
    }
    dc_rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
    dc_rt.BeginDraw();
    // 全面を透明にクリア。
    dc_rt.Clear(Some(&D2D1_COLOR_F {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    }));

    // コントローラ帯（半透明の濃いグレー、角丸、アンチエイリアス）。
    let bar_f = D2D_RECT_F {
        left: 12.0,
        top: bar.top as f32 + 8.0,
        right: w as f32 - 12.0,
        bottom: h as f32 - 8.0,
    };
    if let Ok(brush) = dc_rt.CreateSolidColorBrush(
        &D2D1_COLOR_F {
            r: 0.10,
            g: 0.10,
            b: 0.12,
            a: 0.78,
        },
        None,
    ) {
        dc_rt.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: bar_f,
                radiusX: 14.0,
                radiusY: 14.0,
            },
            &brush,
        );
    }

    // --- 実動コントローラ: mpv の再生状態を読んで描画 ---
    let mpv = STATE.with(|s| s.borrow().mpv);
    let (pos, dur, paused, _title) = if let Some(m) = mpv {
        (
            m.get_property::<f64>("time-pos").unwrap_or(0.0),
            m.get_property::<f64>("duration").unwrap_or(0.0),
            m.get_property::<bool>("pause").unwrap_or(false),
            m.get_property::<String>("media-title").unwrap_or_default(),
        )
    } else {
        (0.0, 0.0, false, String::new())
    };
    STATE.with(|s| s.borrow_mut().paused = paused);

    let cy = (bar_f.top + bar_f.bottom) / 2.0;
    let mut x = bar_f.left + 12.0;

    // サムネイル（WIC デコード結果。あれば左端に「アートワーク」として）。
    if let Some(bmp) = &bitmap {
        let size = bmp.GetSize();
        let th = (BAR_H - 28) as f32;
        let tw = if size.height > 0.0 {
            th * (size.width / size.height)
        } else {
            th * 16.0 / 9.0
        };
        let dst = D2D_RECT_F {
            left: x,
            top: cy - th / 2.0,
            right: x + tw,
            bottom: cy + th / 2.0,
        };
        dc_rt.DrawBitmap(
            bmp,
            Some(&dst),
            1.0,
            windows::Win32::Graphics::Direct2D::D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
            None,
        );
        x = dst.right + 14.0;
    }

    // 白ブラシ（グリフ/ノブ/時間テキスト）。
    let white = dc_rt
        .CreateSolidColorBrush(
            &D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            },
            None,
        )
        .ok();

    // 再生/一時停止ボタン（DirectWrite グリフ）。
    let bs = 36.0;
    let btn_f = D2D_RECT_F {
        left: x,
        top: cy - bs / 2.0,
        right: x + bs,
        bottom: cy + bs / 2.0,
    };
    if let Some(b) = &white {
        let glyph: Vec<u16> = (if paused { "▶" } else { "⏸" }).encode_utf16().collect();
        let gr = D2D_RECT_F {
            left: btn_f.left + 4.0,
            top: btn_f.top + 2.0,
            right: btn_f.right,
            bottom: btn_f.bottom,
        };
        dc_rt.DrawText(
            &glyph,
            &text_format,
            &gr,
            b,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }
    x = btn_f.right + 16.0;

    // シークバー: トラック → 進捗 → ノブ。右端に時間テキスト分を確保。
    let time_str: Vec<u16> = format!("{} / {}", fmt_time(pos), fmt_time(dur))
        .encode_utf16()
        .collect();
    let time_w = 160.0;
    let seek_l = x;
    let seek_r = (bar_f.right - 16.0 - time_w).max(seek_l + 24.0);
    let track_h = 6.0;
    let frac = if dur > 0.0 {
        (pos / dur).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    let knob_x = seek_l + (seek_r - seek_l) * frac;
    if let Ok(track_brush) = dc_rt.CreateSolidColorBrush(
        &D2D1_COLOR_F {
            r: 0.45,
            g: 0.45,
            b: 0.5,
            a: 0.9,
        },
        None,
    ) {
        dc_rt.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: seek_l,
                    top: cy - track_h / 2.0,
                    right: seek_r,
                    bottom: cy + track_h / 2.0,
                },
                radiusX: 3.0,
                radiusY: 3.0,
            },
            &track_brush,
        );
    }
    if let Ok(prog_brush) = dc_rt.CreateSolidColorBrush(
        &D2D1_COLOR_F {
            r: 0.30,
            g: 0.60,
            b: 1.0,
            a: 1.0,
        },
        None,
    ) {
        dc_rt.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: seek_l,
                    top: cy - track_h / 2.0,
                    right: knob_x.max(seek_l),
                    bottom: cy + track_h / 2.0,
                },
                radiusX: 3.0,
                radiusY: 3.0,
            },
            &prog_brush,
        );
    }
    if let Some(b) = &white {
        dc_rt.FillEllipse(
            &D2D1_ELLIPSE {
                point: D2D_POINT_2F { x: knob_x, y: cy },
                radiusX: 8.0,
                radiusY: 8.0,
            },
            b,
        );
        // 時間テキスト（右）。
        let layout = D2D_RECT_F {
            left: seek_r + 12.0,
            top: cy - 14.0,
            right: bar_f.right - 8.0,
            bottom: cy + 14.0,
        };
        dc_rt.DrawText(
            &time_str,
            &text_format,
            &layout,
            b,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }

    // 入力用にボタン/シーク矩形を保存（オーバーレイクライアント座標）。
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.btn = RECT {
            left: btn_f.left as i32,
            top: btn_f.top as i32,
            right: btn_f.right as i32,
            bottom: btn_f.bottom as i32,
        };
        s.seek = RECT {
            left: seek_l as i32,
            top: (cy - 12.0) as i32,
            right: seek_r as i32,
            bottom: (cy + 12.0) as i32,
        };
    });
    let _ = w!("");

    let _ = dc_rt.EndDraw(None, None);

    // 検証: 帯中心のピクセル(BGRA premultiplied)を 1 回だけ読み出してログ。
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.frames = s.frames.saturating_add(1);
        if s.frames == 2 && dib_bits != 0 {
            let px = (w / 2).clamp(0, w - 1);
            let py = ((bar.top + h) / 2).clamp(0, h - 1);
            let off = ((py * w + px) * 4) as usize;
            let p = dib_bits as *const u8;
            let b = *p.add(off);
            let g = *p.add(off + 1);
            let r = *p.add(off + 2);
            let a = *p.add(off + 3);
            eprintln!("[d2d] 帯中心ピクセル BGRA=({b},{g},{r},{a})  alpha>0 なら Direct2D 描画成功");
        }
        if s.frames == 20 {
            if let Some(m) = s.mpv {
                let pos = m.get_property::<f64>("time-pos").unwrap_or(-1.0);
                let dur = m.get_property::<f64>("duration").unwrap_or(-1.0);
                let pause = m.get_property::<bool>("pause").unwrap_or(false);
                eprintln!("[d2d] mpv state(再生中): time-pos={pos:.2} duration={dur:.2} pause={pause}");
            }
        }
    });

    // per-pixel alpha 合成。
    let mut origin = POINT { x: 0, y: 0 };
    let _ = ClientToScreen(base, &mut origin);
    let size = SIZE { cx: w, cy: h };
    let src = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let _ = UpdateLayeredWindow(
        overlay,
        None,
        Some(&origin),
        Some(&size),
        mem_dc,
        Some(&src),
        COLORREF(0),
        Some(&blend),
        ULW_ALPHA,
    );
}

#[cfg(windows)]
unsafe extern "system" fn base_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, GetClientRect, GetCursorPos, KillTimer, PostQuitMessage, ShowWindow,
        SW_HIDE, SW_SHOWNOACTIVATE, WM_DESTROY, WM_SIZE, WM_TIMER,
    };
    match msg {
        WM_TIMER => {
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            let (overlay, should_toggle_show, show) = STATE.with(|s| {
                let mut s = s.borrow_mut();
                let moved = p.x != s.last_cursor.x || p.y != s.last_cursor.y;
                s.last_cursor = p;
                if moved {
                    s.idle_ticks = 0;
                } else {
                    s.idle_ticks = s.idle_ticks.saturating_add(1);
                }
                let should_hide = s.idle_ticks * TICK_MS >= HIDE_AFTER_MS;
                let mut toggle = false;
                let mut show = false;
                if should_hide && s.visible {
                    s.visible = false;
                    toggle = true;
                    show = false;
                } else if !should_hide && !s.visible {
                    s.visible = true;
                    toggle = true;
                    show = true;
                }
                (s.overlay, toggle, show)
            });
            if should_toggle_show {
                if let Some(ov) = overlay {
                    let _ = ShowWindow(ov, if show { SW_SHOWNOACTIVATE } else { SW_HIDE });
                }
            }
            render();
            LRESULT(0)
        }
        WM_SIZE => {
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            layout(hwnd, &rc);
            render();
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
    use windows::Win32::Graphics::Gdi::ScreenToClient;
    use windows::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, HTCLIENT, HTTRANSPARENT, WM_LBUTTONDOWN, WM_NCHITTEST,
    };
    match msg {
        WM_NCHITTEST => {
            let sx = (lparam.0 & 0xFFFF) as i16 as i32;
            let sy = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let in_bar = STATE.with(|s| {
                let s = s.borrow();
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
            // lparam はオーバーレイのクライアント座標（=描画座標）。
            let cx = (lparam.0 & 0xFFFF) as i16 as i32;
            let cy = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let (mpv, btn, seek) = STATE.with(|s| {
                let s = s.borrow();
                (s.mpv, s.btn, s.seek)
            });
            if let Some(mpv) = mpv {
                let hit = |r: &RECT| cx >= r.left && cx < r.right && cy >= r.top && cy < r.bottom;
                if hit(&btn) {
                    // 再生/一時停止トグル。
                    let paused = STATE.with(|s| {
                        let mut s = s.borrow_mut();
                        s.paused = !s.paused;
                        s.paused
                    });
                    let _ = mpv.set_property("pause", paused);
                } else if hit(&seek) && seek.right > seek.left {
                    // シークバークリック位置へ絶対シーク。
                    let frac =
                        ((cx - seek.left) as f64 / (seek.right - seek.left) as f64).clamp(0.0, 1.0);
                    let pct = format!("{:.3}", frac * 100.0);
                    let _ = mpv.command("seek", &[pct.as_str(), "absolute-percent"]);
                }
                render();
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
