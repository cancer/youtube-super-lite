//! 新オーバーレイホスト（子窓 + DirectComposition）。ゼロベース実装。
//!
//! 旧 `native_overlay`（WS_EX_LAYERED トップレベル窓を ULW で合成し、親をサブクラス化して
//! 手動追従）の負債を断つための置き換え。設計の核:
//! - オーバーレイは winit 親窓の **WS_CHILD**。位置・クリップ・移動は OS が面倒を見る（follow 不要）。
//! - 合成は **DirectComposition**（D3D11→DXGI→DComp/D2D）。per-pixel alpha を DComp サーフェスで持つ。
//! - 子窓が**全入力を所有**（HTTRANSPARENT 貫通はしない）。動画域クリックは自前で行動に変換する。
//! - wndproc 連携はグローバル(thread_local)を使わず、窓ごとに `GWLP_USERDATA` へ状態ポインタを置く。
//!
//! 移植状況: コントローラ帯コア（再生/一時停止・シーク・時間・音量）＋自動非表示まで。
//! 上部バー(URL/認証)・一覧・チャット・画質/コーデック/ミュート/Like は後続で本ファイルへ足す。
//! レイアウト/色は旧 `native_overlay::draw_controller`（egui 踏襲）を参照して合わせる。

#![cfg(windows)]

use anyhow::{anyhow, Result};
use windows::core::Interface;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::ID2D1DeviceContext;
use windows::Win32::Graphics::DirectComposition::{
    IDCompositionDevice, IDCompositionSurface, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::DirectWrite::{IDWriteFactory, IDWriteTextFormat};

/// 子窓への入力で積まれる行動（コアへ渡す）。UI 移植に合わせて拡張する。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OverlayAction {
    /// 再生/一時停止トグル（コントロール非ヒット＝動画クリックも含む）。
    TogglePause,
    /// シーク（0.0..=1.0 の割合。seekable 時のみ）。
    Seek(f64),
    /// 音量設定（0.0..=130.0）。
    SetVolume(f64),
    /// 音量を相対変更（ホイール。± の量）。
    VolumeStep(f64),
}

/// 描画に必要な再生状態（コアから毎フレーム渡す。借用を持ち込まない値のコピー）。
#[derive(Clone, Copy)]
pub struct PlaybackView {
    pub paused: bool,
    pub pos: f64,
    pub dur: f64,
    pub seekable: bool,
    pub volume: f64,
}

/// 下部コントローラ帯の高さ・行高（旧 native_overlay と同値）。
const BOTTOM_H: i32 = 52;
const ROW_H: i32 = 26;
/// 音量バーの幅（px）。
const VOL_W: i32 = 110;

#[derive(Default, Clone, Copy, PartialEq)]
enum Drag {
    #[default]
    None,
    Seek,
    Vol,
}

/// wndproc から触る窓ごとの状態。`GWLP_USERDATA` に *mut で置く（グローバル不使用）。
#[derive(Default)]
struct WndState {
    actions: Vec<OverlayAction>,
    /// オーバーレイ上でマウスが動いたか（自動非表示タイマのリセット用）。
    moved: bool,
    /// 現在のクライアントサイズ。
    cw: i32,
    ch: i32,
    /// コントロール描画中か（false の間は全クリックを TogglePause として扱う）。
    active: bool,
    seekable: bool,
    /// ヒット矩形（active 時のみ有効。クライアント座標）。
    btn: RECT,
    seek: RECT,
    vol: RECT,
    drag: Drag,
}

/// 子窓 + DComp の合成オーバーレイ。`render` で描画＆Commit、`take_actions` で入力を取り出す。
pub struct DcompOverlay {
    hwnd: HWND,
    /// `GWLP_USERDATA` が指す状態。Box でヒープ固定し、struct が move してもアドレス不変。
    state: Box<WndState>,
    _d3d: windows::Win32::Graphics::Direct3D11::ID3D11Device,
    dcomp: IDCompositionDevice,
    _target: IDCompositionTarget,
    visual: IDCompositionVisual,
    surface: Option<IDCompositionSurface>,
    d2d_ctx: ID2D1DeviceContext,
    dwrite: IDWriteFactory,
    /// 小フォント（コントロール・時間、15px）。
    tf_small: IDWriteTextFormat,
    cw: i32,
    ch: i32,
}

impl DcompOverlay {
    /// winit 親窓（HWND を i64 で受ける）の子として作成する。
    pub fn new(parent_wid: i64) -> Result<Self> {
        use windows::core::w;
        use windows::Win32::Graphics::Direct2D::{
            D2D1CreateDevice, ID2D1Device, D2D1_DEVICE_CONTEXT_OPTIONS_NONE,
        };
        use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL};
        use windows::Win32::Graphics::Direct3D11::{
            D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
        };
        use windows::Win32::Graphics::DirectComposition::DCompositionCreateDevice;
        use windows::Win32::Graphics::DirectWrite::{
            DWriteCreateFactory, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
            DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
        };
        use windows::Win32::Graphics::Dxgi::IDXGIDevice;
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, GetClientRect, LoadCursorW, RegisterClassW, SetWindowLongPtrW,
            GWLP_USERDATA, IDC_ARROW, WNDCLASSW, WS_CHILD, WS_CLIPSIBLINGS, WS_VISIBLE,
        };

        let parent = HWND(parent_wid as *mut core::ffi::c_void);
        unsafe {
            let hinstance = GetModuleHandleW(None)?;
            let class = w!("TalavaDcompOverlay");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance.into(),
                lpszClassName: class,
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                ..Default::default()
            };
            // 既に登録済みでも 0 を返すだけ（複数窓/再起動向け）。問題視しない。
            let _ = RegisterClassW(&wc);

            let mut rc = RECT::default();
            let _ = GetClientRect(parent, &mut rc);
            let cw = (rc.right - rc.left).max(1);
            let ch = (rc.bottom - rc.top).max(1);

            let hwnd = CreateWindowExW(
                Default::default(),
                class,
                w!("overlay"),
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

            let mut state = Box::new(WndState {
                cw,
                ch,
                ..Default::default()
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state.as_mut() as *mut WndState as isize);

            // D3D11 → DXGI → DComp / D2D / DirectWrite。
            let mut d3d: Option<ID3D11Device> = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                windows::Win32::Foundation::HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut d3d),
                Some(&mut D3D_FEATURE_LEVEL::default()),
                None,
            )?;
            let d3d = d3d.ok_or_else(|| anyhow!("D3D11CreateDevice returned null"))?;
            let dxgi: IDXGIDevice = d3d.cast()?;

            let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
            let target: IDCompositionTarget = dcomp.CreateTargetForHwnd(hwnd, true)?;
            let visual: IDCompositionVisual = dcomp.CreateVisual()?;
            target.SetRoot(&visual)?;

            let d2d_device: ID2D1Device = D2D1CreateDevice(&dxgi, None)?;
            let d2d_ctx: ID2D1DeviceContext =
                d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;

            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let tf_small: IDWriteTextFormat = dwrite.CreateTextFormat(
                w!("Yu Gothic UI"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                15.0,
                w!("ja-jp"),
            )?;

            let mut me = DcompOverlay {
                hwnd,
                state,
                _d3d: d3d,
                dcomp,
                _target: target,
                visual,
                surface: None,
                d2d_ctx,
                dwrite,
                tf_small,
                cw,
                ch,
            };
            me.rebuild_surface()?;
            Ok(me)
        }
    }

    /// 親クライアントサイズの変化に合わせて子窓とサーフェスを更新する（位置は OS 追従＝不要）。
    pub fn resize(&mut self, w: i32, h: i32) {
        use windows::Win32::UI::WindowsAndMessaging::MoveWindow;
        let w = w.max(1);
        let h = h.max(1);
        if w == self.cw && h == self.ch {
            return;
        }
        self.cw = w;
        self.ch = h;
        self.state.cw = w;
        self.state.ch = h;
        unsafe {
            let _ = MoveWindow(self.hwnd, 0, 0, w, h, true);
        }
        if let Err(e) = self.rebuild_surface() {
            eprintln!("[dcomp] rebuild_surface 失敗: {e:#}");
        }
    }

    /// 入力で積まれた行動を取り出す（コアが適用）。
    pub fn take_actions(&mut self) -> Vec<OverlayAction> {
        std::mem::take(&mut self.state.actions)
    }

    /// オーバーレイ上でマウスが動いたかを取り出してクリアする（自動非表示の活動源）。
    pub fn take_moved(&mut self) -> bool {
        std::mem::replace(&mut self.state.moved, false)
    }

    /// DComp サーフェスを現在サイズで作り直し、visual に割り当てる。
    fn rebuild_surface(&mut self) -> Result<()> {
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM,
        };
        unsafe {
            let surface = self.dcomp.CreateSurface(
                self.cw.max(1) as u32,
                self.ch.max(1) as u32,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_ALPHA_MODE_PREMULTIPLIED,
            )?;
            self.visual.SetContent(&surface)?;
            self.dcomp.Commit()?;
            self.surface = Some(surface);
        }
        Ok(())
    }

    /// DComp サーフェスへ Direct2D で描画して Commit する。
    /// `active` が false の間はコントロールを描かず透明（動画素通し）。クリックは TogglePause。
    pub fn render(&mut self, active: bool, view: &PlaybackView) {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT};
        use windows::Win32::Graphics::Direct2D::{
            ID2D1Bitmap1, D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_BITMAP_PROPERTIES1,
        };
        use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
        use windows::Win32::Graphics::Dxgi::IDXGISurface;
        use windows::Foundation::Numerics::Matrix3x2;

        let Some(surface) = self.surface.clone() else {
            return;
        };
        let (cw, ch) = (self.cw, self.ch);
        unsafe {
            let mut offset = POINT::default();
            let dxgi_surface: IDXGISurface = match surface.BeginDraw(None, &mut offset) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[dcomp] surface.BeginDraw 失敗: {e:#}");
                    return;
                }
            };
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: std::mem::ManuallyDrop::new(None),
            };
            let bitmap: ID2D1Bitmap1 =
                match self.d2d_ctx.CreateBitmapFromDxgiSurface(&dxgi_surface, Some(&props)) {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!("[dcomp] CreateBitmapFromDxgiSurface 失敗: {e:#}");
                        let _ = surface.EndDraw();
                        return;
                    }
                };
            let ctx = self.d2d_ctx.clone();
            ctx.SetTarget(&bitmap);
            ctx.BeginDraw();
            // BeginDraw の offset 分だけ平行移動（サーフェスはアトラスの一部のことがある）。
            // 以降の描画・幾何はすべてクライアント座標で行い、ヒット矩形もクライアント座標で保存する。
            ctx.SetTransform(&Matrix3x2::translation(offset.x as f32, offset.y as f32));
            ctx.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));

            // 既定はヒット無効化（active=false や非コントロール領域のクリックは TogglePause）。
            self.state.active = active;
            self.state.seekable = view.seekable;
            self.state.cw = cw;
            self.state.ch = ch;
            self.state.btn = RECT::default();
            self.state.seek = RECT::default();
            self.state.vol = RECT::default();

            if active {
                self.draw_controller(&ctx, cw, ch, view);
            }

            let _ = ctx.EndDraw(None, None);
            ctx.SetTarget(None);
            let _ = surface.EndDraw();
            let _ = self.dcomp.Commit();
        }
    }

    /// 下部コントローラ帯（半透明帯・シークライン・再生/一時停止・時間・音量）を描く。
    /// 幾何・色は旧 `native_overlay::draw_controller` を踏襲。ヒット矩形を WndState に保存する。
    unsafe fn draw_controller(&mut self, ctx: &ID2D1DeviceContext, w: i32, h: i32, v: &PlaybackView) {
        // 半透明帯（下端いっぱい）。
        fill_rect(
            ctx,
            rf(0.0, (h - BOTTOM_H) as f32, w as f32, h as f32),
            color(0.03, 0.03, 0.05, 0.72),
        );

        // --- シークライン（フル幅・細い、上段）---
        let sx0 = 14.0f32;
        let sx1 = w as f32 - 14.0;
        let sy = (h - BOTTOM_H + 13) as f32;
        let track_h = 3.0f32;
        let frac = if !v.seekable {
            1.0
        } else if v.dur > 0.0 {
            (v.pos / v.dur).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        fill_round(ctx, rf(sx0, sy - track_h / 2.0, sx1, sy + track_h / 2.0), 1.5, color(1.0, 1.0, 1.0, 0.25));
        let prog_col = if v.seekable {
            color(0.92, 0.20, 0.20, 1.0)
        } else {
            color(0.55, 0.55, 0.60, 0.9)
        };
        fill_round(
            ctx,
            rf(sx0, sy - track_h / 2.0, (sx0 + (sx1 - sx0) * frac).max(sx0), sy + track_h / 2.0),
            1.5,
            prog_col,
        );
        if v.seekable {
            let knob_x = sx0 + (sx1 - sx0) * frac;
            fill_ellipse(ctx, knob_x, sy, 6.0, color(0.92, 0.20, 0.20, 1.0));
        }
        self.state.seek = RECT {
            left: sx0 as i32,
            top: (sy - 9.0) as i32,
            right: sx1 as i32,
            bottom: (sy + 9.0) as i32,
        };

        // --- コントロール行（下段）---
        let cy = h - 16;
        let fg = color(0.96, 0.96, 0.98, 1.0);

        // 再生/一時停止グリフ。
        let glyph = if v.paused { "▶" } else { "⏸" };
        let gw = self.measure(glyph).ceil() as i32;
        let btn = RECT {
            left: 14,
            top: cy - ROW_H / 2,
            right: 14 + gw + 8,
            bottom: cy + ROW_H / 2,
        };
        self.text(ctx, glyph, rf(18.0, (cy - 9) as f32, (18 + gw) as f32, (cy + 9) as f32), fg);
        self.state.btn = btn;
        let x = btn.right + 12;

        // 時間表示（mm:ss / mm:ss）。
        let time_str = format!("{} / {}", fmt_time(v.pos), fmt_time(v.dur));
        let tw = self.measure(&time_str).ceil() as i32;
        self.text(ctx, &time_str, rf(x as f32, (cy - 9) as f32, (x + tw + 4) as f32, (cy + 9) as f32), fg);

        // 音量バー（右端）。
        let xr = w - 14;
        let vol = RECT {
            left: xr - VOL_W,
            top: cy - ROW_H / 2,
            right: xr,
            bottom: cy + ROW_H / 2,
        };
        let vol_frac = (v.volume / 130.0).clamp(0.0, 1.0) as f32;
        let vcy = cy as f32;
        fill_round(ctx, rf(vol.left as f32, vcy - 2.0, vol.right as f32, vcy + 2.0), 2.0, color(1.0, 1.0, 1.0, 0.25));
        let vx = vol.left as f32 + VOL_W as f32 * vol_frac;
        fill_round(ctx, rf(vol.left as f32, vcy - 2.0, vx.max(vol.left as f32), vcy + 2.0), 2.0, color(0.92, 0.92, 0.96, 1.0));
        fill_ellipse(ctx, vx, vcy, 5.0, color(1.0, 1.0, 1.0, 1.0));
        self.state.vol = vol;
    }

    /// 小フォントの左寄せテキストを描く。
    unsafe fn text(&self, ctx: &ID2D1DeviceContext, s: &str, r: D2D_RECT_F, c: D2D1_COLOR_F) {
        use windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_NONE;
        use windows::Win32::Graphics::DirectWrite::DWRITE_MEASURING_MODE_NATURAL;
        if let Ok(b) = ctx.CreateSolidColorBrush(&c, None) {
            let wt: Vec<u16> = s.encode_utf16().collect();
            ctx.DrawText(&wt, &self.tf_small, &r, &b, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        }
    }

    /// 小フォントでのテキスト幅（px）を計測する。
    unsafe fn measure(&self, s: &str) -> f32 {
        use windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_METRICS;
        let wt: Vec<u16> = s.encode_utf16().collect();
        if let Ok(layout) = self.dwrite.CreateTextLayout(&wt, &self.tf_small, 8192.0, 64.0) {
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_ok() {
                return m.widthIncludingTrailingWhitespace;
            }
        }
        s.chars().count() as f32 * 9.0
    }
}

#[inline]
fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

#[inline]
fn rf(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F { left, top, right, bottom }
}

unsafe fn fill_rect(ctx: &ID2D1DeviceContext, r: D2D_RECT_F, c: D2D1_COLOR_F) {
    if let Ok(b) = ctx.CreateSolidColorBrush(&c, None) {
        ctx.FillRectangle(&r, &b);
    }
}

unsafe fn fill_round(ctx: &ID2D1DeviceContext, r: D2D_RECT_F, rad: f32, c: D2D1_COLOR_F) {
    use windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT;
    if let Ok(b) = ctx.CreateSolidColorBrush(&c, None) {
        ctx.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: rad, radiusY: rad }, &b);
    }
}

unsafe fn fill_ellipse(ctx: &ID2D1DeviceContext, x: f32, y: f32, rad: f32, c: D2D1_COLOR_F) {
    use windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE;
    use windows::Win32::Graphics::Direct2D::Common::D2D_POINT_2F;
    if let Ok(b) = ctx.CreateSolidColorBrush(&c, None) {
        ctx.FillEllipse(
            &D2D1_ELLIPSE {
                point: D2D_POINT_2F { x, y },
                radiusX: rad,
                radiusY: rad,
            },
            &b,
        );
    }
}

/// 秒数を mm:ss / h:mm:ss にする。
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

#[inline]
fn in_rect(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// 窓ごとの状態（GWLP_USERDATA）。null の間は触らない。
unsafe fn state_of<'a>(hwnd: HWND) -> Option<&'a mut WndState> {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, GWLP_USERDATA};
    let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WndState;
    if p.is_null() {
        None
    } else {
        Some(&mut *p)
    }
}

/// オーバーレイ子窓の WndProc。全クライアントを HTCLIENT で受ける（既定動作）。
unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
    use windows::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, MA_NOACTIVATE, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE,
        WM_MOUSEMOVE, WM_MOUSEWHEEL,
    };
    let lo = (lparam.0 & 0xFFFF) as i16 as i32;
    let hi = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
    match msg {
        // この子窓はアクティブ化しない（winit 親のキーボードフォーカスを奪わない）。
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            if let Some(s) = state_of(hwnd) {
                s.moved = true;
                match s.drag {
                    Drag::Seek if s.seek.right > s.seek.left => {
                        s.actions.push(OverlayAction::Seek(frac_x(&s.seek, lo)));
                    }
                    Drag::Vol if s.vol.right > s.vol.left => {
                        s.actions.push(OverlayAction::SetVolume(frac_x(&s.vol, lo) * 130.0));
                    }
                    _ => {}
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let mut capture = false;
            if let Some(s) = state_of(hwnd) {
                if !s.active {
                    s.actions.push(OverlayAction::TogglePause);
                } else if s.seekable && in_rect(&s.seek, lo, hi) {
                    s.drag = Drag::Seek;
                    s.actions.push(OverlayAction::Seek(frac_x(&s.seek, lo)));
                    capture = true;
                } else if in_rect(&s.vol, lo, hi) {
                    s.drag = Drag::Vol;
                    s.actions.push(OverlayAction::SetVolume(frac_x(&s.vol, lo) * 130.0));
                    capture = true;
                } else {
                    // ボタン上も、バー余白も、動画域も：再生/一時停止トグル。
                    s.actions.push(OverlayAction::TogglePause);
                }
            }
            if capture {
                let _ = SetCapture(hwnd);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if let Some(s) = state_of(hwnd) {
                s.drag = Drag::None;
            }
            let _ = ReleaseCapture();
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as i16;
            if let Some(s) = state_of(hwnd) {
                s.actions
                    .push(OverlayAction::VolumeStep(if delta > 0 { 5.0 } else { -5.0 }));
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// クライアント x を矩形内の割合 0.0..=1.0 に直す。
#[inline]
fn frac_x(r: &RECT, x: i32) -> f64 {
    let w = (r.right - r.left).max(1) as f64;
    ((x - r.left) as f64 / w).clamp(0.0, 1.0)
}
