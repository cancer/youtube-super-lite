//! ネイティブ版用の透過 2D オーバーレイ（Direct2D + DirectWrite）。
//!
//! 親ウィンドウ（winit、mpv が D3D11 で動画を描く）の上に重ねる WS_EX_LAYERED の透過窓。
//! [`Player`](crate::player::Player) の再生状態を読み、コントローラ（再生/一時停止グリフ・
//! シークバー・時間表示）を Direct2D で描画し、UpdateLayeredWindow(ULW_ALPHA) で
//! per-pixel alpha 合成する。probe(src/bin/d2d_overlay_probe.rs) の描画を構造体化したもの。
//!
//! この段階ではクリックスルー（WS_EX_TRANSPARENT）の表示専用。操作はキーボード（NativeApp 側）。
//! クリックによる入力振り分けは後続フェーズで追加する。

#![cfg(windows)]

use anyhow::Result;
use std::cell::RefCell;

use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_ELLIPSE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT,
    D2D1_RENDER_TARGET_USAGE_GDI_COMPATIBLE, D2D1_ROUNDED_RECT,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_MEASURING_MODE_NATURAL,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
    HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, LoadCursorW, RegisterClassW, ShowWindow,
    UpdateLayeredWindow, IDC_ARROW, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};

use crate::player::Player;

const BAR_H: i32 = 72;

/// オーバーレイのクリックで発生する操作（NativeApp が Player に適用する）。
#[derive(Clone, Copy)]
pub enum OverlayAction {
    /// 再生/一時停止トグル。
    TogglePause,
    /// シーク（0.0..=1.0 の割合）。
    Seek(f64),
}

/// wndproc(C コールバック) と描画/NativeApp の橋渡し。UI スレッド単一なので thread_local。
#[derive(Default)]
struct OvShared {
    bar: RECT,
    btn: RECT,
    seek: RECT,
    pending: Option<OverlayAction>,
}

thread_local! {
    static OV_STATE: RefCell<OvShared> = RefCell::new(OvShared::default());
}

#[inline]
fn in_rect(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// 親ウィンドウに重ねる透過 2D オーバーレイ。
pub struct Overlay {
    hwnd: HWND,
    _factory: ID2D1Factory,
    dc_rt: ID2D1DCRenderTarget,
    text_format: IDWriteTextFormat,
    _dwrite: IDWriteFactory,
    mem_dc: HDC,
    dib: HBITMAP,
    old_obj: HGDIOBJ,
    dib_w: i32,
    dib_h: i32,
}

impl Overlay {
    /// 親ウィンドウ（HWND）の上に重ねる透過オーバーレイを作る。
    pub fn new(parent: HWND) -> Result<Self> {
        unsafe {
            let hinstance = GetModuleHandleW(None)?;
            let class_name = w!("YSL_NativeOverlay");
            // クラス登録は失敗（既登録）しても続行。
            let wc = WNDCLASSW {
                lpfnWndProc: Some(overlay_wndproc),
                hInstance: hinstance.into(),
                lpszClassName: class_name,
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);

            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                class_name,
                w!("overlay"),
                WS_POPUP | WS_VISIBLE,
                0,
                0,
                10,
                10,
                parent,
                None,
                hinstance,
                None,
            )?;

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

            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);

            Ok(Self {
                hwnd,
                _factory: factory,
                dc_rt,
                text_format,
                _dwrite: dwrite,
                mem_dc: HDC::default(),
                dib: HBITMAP::default(),
                old_obj: HGDIOBJ::default(),
                dib_w: 0,
                dib_h: 0,
            })
        }
    }

    /// per-pixel alpha 用のメモリ DC + 32bpp top-down DIB を（必要なら）作り直す。
    unsafe fn ensure_dib(&mut self, w: i32, h: i32) {
        if !self.mem_dc.0.is_null() && self.dib_w == w && self.dib_h == h {
            return;
        }
        if !self.mem_dc.0.is_null() {
            if !self.old_obj.0.is_null() {
                SelectObject(self.mem_dc, self.old_obj);
            }
            if !self.dib.0.is_null() {
                let _ = DeleteObject(self.dib);
            }
            let _ = DeleteDC(self.mem_dc);
        }
        let mem_dc = CreateCompatibleDC(None);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
            .unwrap_or(HBITMAP(std::ptr::null_mut()));
        let old = SelectObject(mem_dc, dib);
        self.mem_dc = mem_dc;
        self.dib = dib;
        self.old_obj = old;
        self.dib_w = w;
        self.dib_h = h;
    }

    /// クリックで溜まった操作を取り出す（NativeApp が Player に適用する）。
    pub fn take_action(&self) -> Option<OverlayAction> {
        OV_STATE.with(|s| s.borrow_mut().pending.take())
    }

    /// 表示/非表示を切り替える（自動非表示用）。
    pub fn set_visible(&self, visible: bool) {
        unsafe {
            let _ = ShowWindow(self.hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
    }

    /// 親のクライアント領域に合わせて URL バー＋コントローラを Direct2D で描画し、ULW で合成する。
    pub fn render(&mut self, player: &Player, parent: HWND, url_input: &str) {
        unsafe {
            let mut rc = RECT::default();
            if GetClientRect(parent, &mut rc).is_err() {
                return;
            }
            let w = (rc.right - rc.left).max(1);
            let h = (rc.bottom - rc.top).max(1);
            self.ensure_dib(w, h);

            let full = RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            };
            if self.dc_rt.BindDC(self.mem_dc, &full).is_err() {
                return;
            }
            let dc_rt = &self.dc_rt;
            dc_rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            dc_rt.BeginDraw();
            dc_rt.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));

            // コントローラ帯（半透明・角丸）。
            let bar_f = D2D_RECT_F {
                left: 12.0,
                top: (h - BAR_H) as f32 + 8.0,
                right: w as f32 - 12.0,
                bottom: h as f32 - 8.0,
            };
            if let Ok(b) = dc_rt.CreateSolidColorBrush(&color(0.10, 0.10, 0.12, 0.78), None) {
                dc_rt.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: bar_f,
                        radiusX: 14.0,
                        radiusY: 14.0,
                    },
                    &b,
                );
            }

            let pos = player.time_pos();
            let dur = player.duration();
            let paused = player.paused();
            let cy = (bar_f.top + bar_f.bottom) / 2.0;
            let mut x = bar_f.left + 16.0;

            let white = dc_rt
                .CreateSolidColorBrush(&color(1.0, 1.0, 1.0, 1.0), None)
                .ok();

            // 再生/一時停止グリフ。
            let bs = 36.0;
            let btn_rect = RECT {
                left: x as i32,
                top: (cy - bs / 2.0) as i32,
                right: (x + bs) as i32,
                bottom: (cy + bs / 2.0) as i32,
            };
            if let Some(b) = &white {
                let glyph: Vec<u16> = (if paused { "▶" } else { "⏸" }).encode_utf16().collect();
                let gr = D2D_RECT_F {
                    left: x + 4.0,
                    top: cy - bs / 2.0 + 2.0,
                    right: x + bs,
                    bottom: cy + bs / 2.0,
                };
                dc_rt.DrawText(
                    &glyph,
                    &self.text_format,
                    &gr,
                    b,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
            x += bs + 16.0;

            // シークバー。
            let time_str: Vec<u16> = format!("{} / {}", fmt_time(pos), fmt_time(dur))
                .encode_utf16()
                .collect();
            let time_w = 160.0;
            let seek_l = x;
            let seek_r = (bar_f.right - 16.0 - time_w).max(seek_l + 24.0);
            let seek_rect = RECT {
                left: seek_l as i32,
                top: (cy - 12.0) as i32,
                right: seek_r as i32,
                bottom: (cy + 12.0) as i32,
            };
            let track_h = 6.0;
            let frac = if dur > 0.0 {
                (pos / dur).clamp(0.0, 1.0) as f32
            } else {
                0.0
            };
            let knob_x = seek_l + (seek_r - seek_l) * frac;
            if let Ok(tb) = dc_rt.CreateSolidColorBrush(&color(0.45, 0.45, 0.5, 0.9), None) {
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
                    &tb,
                );
            }
            if let Ok(pb) = dc_rt.CreateSolidColorBrush(&color(0.30, 0.60, 1.0, 1.0), None) {
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
                    &pb,
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
                let layout = D2D_RECT_F {
                    left: seek_r + 12.0,
                    top: cy - 14.0,
                    right: bar_f.right - 8.0,
                    bottom: cy + 14.0,
                };
                dc_rt.DrawText(
                    &time_str,
                    &self.text_format,
                    &layout,
                    b,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            // URL 入力バー（上部）。
            let top_f = D2D_RECT_F {
                left: 12.0,
                top: 10.0,
                right: w as f32 - 12.0,
                bottom: 54.0,
            };
            if let Ok(b) = dc_rt.CreateSolidColorBrush(&color(0.10, 0.10, 0.12, 0.78), None) {
                dc_rt.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: top_f,
                        radiusX: 10.0,
                        radiusY: 10.0,
                    },
                    &b,
                );
            }
            let (txt, col) = if url_input.is_empty() {
                (
                    "URL を入力して Enter で再生（英数字キーで入力 / Backspace 削除 / Esc クリア）"
                        .to_string(),
                    color(0.62, 0.62, 0.65, 1.0),
                )
            } else {
                (format!("URL: {url_input}"), color(1.0, 1.0, 1.0, 1.0))
            };
            if let Ok(b) = dc_rt.CreateSolidColorBrush(&col, None) {
                let layout = D2D_RECT_F {
                    left: top_f.left + 14.0,
                    top: top_f.top + 10.0,
                    right: top_f.right - 12.0,
                    bottom: top_f.bottom,
                };
                let wtext: Vec<u16> = txt.encode_utf16().collect();
                dc_rt.DrawText(
                    &wtext,
                    &self.text_format,
                    &layout,
                    &b,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            let _ = dc_rt.EndDraw(None, None);

            // ヒット判定用の矩形を wndproc / NativeApp と共有する。
            let bar_rect = RECT {
                left: bar_f.left as i32,
                top: bar_f.top as i32,
                right: bar_f.right as i32,
                bottom: bar_f.bottom as i32,
            };
            OV_STATE.with(|s| {
                let mut s = s.borrow_mut();
                s.bar = bar_rect;
                s.btn = btn_rect;
                s.seek = seek_rect;
            });

            // ULW で per-pixel alpha 合成。位置は親のクライアント原点（スクリーン座標）。
            let mut origin = POINT { x: 0, y: 0 };
            let _ = ClientToScreen(parent, &mut origin);
            let size = SIZE { cx: w, cy: h };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&origin),
                Some(&size),
                self.mem_dc,
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            if !self.mem_dc.0.is_null() {
                if !self.old_obj.0.is_null() {
                    SelectObject(self.mem_dc, self.old_obj);
                }
                if !self.dib.0.is_null() {
                    let _ = DeleteObject(self.dib);
                }
                let _ = DeleteDC(self.mem_dc);
            }
            use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

#[inline]
fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

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

unsafe extern "system" fn overlay_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::Graphics::Gdi::ScreenToClient;
    use windows::Win32::UI::WindowsAndMessaging::{
        HTCLIENT, HTTRANSPARENT, WM_LBUTTONDOWN, WM_NCHITTEST,
    };
    match msg {
        // 入力振り分け: コントローラ帯の中だけ overlay が受け取り、それ以外は下の動画へ透過。
        WM_NCHITTEST => {
            let sx = (lparam.0 & 0xFFFF) as i16 as i32;
            let sy = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut pt = POINT { x: sx, y: sy };
            let _ = ScreenToClient(hwnd, &mut pt);
            let in_bar = OV_STATE.with(|s| in_rect(&s.borrow().bar, pt.x, pt.y));
            if in_bar {
                LRESULT(HTCLIENT as isize)
            } else {
                LRESULT(HTTRANSPARENT as isize)
            }
        }
        // 帯クリック: ボタン=再生/一時停止、シークバー=絶対シーク。
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            OV_STATE.with(|s| {
                let mut s = s.borrow_mut();
                if in_rect(&s.btn, x, y) {
                    s.pending = Some(OverlayAction::TogglePause);
                } else if s.seek.right > s.seek.left && in_rect(&s.seek, x, y) {
                    let frac = ((x - s.seek.left) as f64 / (s.seek.right - s.seek.left) as f64)
                        .clamp(0.0, 1.0);
                    s.pending = Some(OverlayAction::Seek(frac));
                }
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
