//! 新オーバーレイホスト（子窓 + DirectComposition）。ゼロベース実装。
//!
//! 旧 `native_overlay`（WS_EX_LAYERED トップレベル窓を ULW で合成し、親をサブクラス化して
//! 手動追従）の負債を断つための置き換え。設計の核:
//! - オーバーレイは winit 親窓の **WS_CHILD**。位置・クリップ・移動は OS が面倒を見る（follow 不要）。
//! - 合成は **DirectComposition**（D3D11→DXGI→DComp/D2D）。per-pixel alpha を DComp サーフェスで持つ。
//! - 子窓が**全入力を所有**（HTTRANSPARENT 貫通はしない）。動画域クリックは自前で行動に変換する。
//! - wndproc 連携はグローバル(thread_local)を使わず、窓ごとに `GWLP_USERDATA` へ状態ポインタを置く。
//!
//! 本ファイルは骨組み段階: 子窓＋合成＋追従＋入力受領までを通し、描画はプレースホルダ。
//! 実 UI（コントローラ/一覧/チャット等）の移植は後続で本ファイルへ足す。

#![cfg(windows)]

use anyhow::{anyhow, Result};
use windows::core::Interface;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::ID2D1DeviceContext;
use windows::Win32::Graphics::DirectComposition::{
    IDCompositionDevice, IDCompositionSurface, IDCompositionTarget, IDCompositionVisual,
};

/// 子窓への入力で積まれる行動（コアへ渡す）。最小から始め、UI 移植に合わせて拡張する。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OverlayAction {
    /// 動画域クリック＝再生/一時停止トグル。
    TogglePause,
}

/// 高さ（下部コントロール帯のプレースホルダ）。
const BAR_H: i32 = 72;

/// wndproc から触る窓ごとの状態。`GWLP_USERDATA` に *mut で置く（グローバル不使用）。
struct WndState {
    actions: Vec<OverlayAction>,
    /// オーバーレイ上でマウスが動いたか（自動非表示タイマのリセット用）。
    moved: bool,
    /// 現在のクライアント高さ（領域判定用）。
    ch: i32,
}

/// 子窓 + DComp の合成オーバーレイ。`render` で描画＆Commit、`take_actions` で入力を取り出す。
pub struct DcompOverlay {
    hwnd: HWND,
    /// `GWLP_USERDATA` が指す状態。Box でヒープ固定し、struct が move してもアドレス不変。
    state: Box<WndState>,
    // 合成スタック（drop 順は問わない。デバイスは Box/COM 参照カウント管理）。
    _d3d: windows::Win32::Graphics::Direct3D11::ID3D11Device,
    dcomp: IDCompositionDevice,
    _target: IDCompositionTarget,
    visual: IDCompositionVisual,
    surface: Option<IDCompositionSurface>,
    d2d_ctx: ID2D1DeviceContext,
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
            // 既に登録済みでも RegisterClassW は 0 を返すだけ（複数窓/再起動向け）。問題視しない。
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

            // 窓ごとの状態を Box で確保し、ポインタを GWLP_USERDATA へ。
            let mut state = Box::new(WndState {
                actions: Vec::new(),
                moved: false,
                ch,
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state.as_mut() as *mut WndState as isize);

            // D3D11 → DXGI → DComp / D2D。
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

            let mut me = DcompOverlay {
                hwnd,
                state,
                _d3d: d3d,
                dcomp,
                _target: target,
                visual,
                surface: None,
                d2d_ctx,
                cw,
                ch,
            };
            me.rebuild_surface()?;
            me.render();
            Ok(me)
        }
    }

    /// 親クライアントサイズの変化に合わせて子窓とサーフェスを更新する。
    /// 位置追従は子窓なので不要（OS 任せ）。サイズだけ合わせる。
    pub fn resize(&mut self, w: i32, h: i32) {
        use windows::Win32::UI::WindowsAndMessaging::MoveWindow;
        let w = w.max(1);
        let h = h.max(1);
        if w == self.cw && h == self.ch {
            return;
        }
        self.cw = w;
        self.ch = h;
        self.state.ch = h;
        unsafe {
            let _ = MoveWindow(self.hwnd, 0, 0, w, h, true);
        }
        if let Err(e) = self.rebuild_surface() {
            eprintln!("[dcomp] rebuild_surface 失敗: {e:#}");
        }
        self.render();
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

    /// DComp サーフェスへ Direct2D で描画して Commit する。骨組みではプレースホルダ。
    pub fn render(&mut self) {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::Graphics::Direct2D::Common::{
            D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F,
        };
        use windows::Win32::Graphics::Direct2D::{
            ID2D1Bitmap1, D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_BITMAP_PROPERTIES1, D2D1_ROUNDED_RECT,
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
            let ctx = &self.d2d_ctx;
            ctx.SetTarget(&bitmap);
            ctx.BeginDraw();
            ctx.SetTransform(&Matrix3x2::translation(offset.x as f32, offset.y as f32));
            ctx.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
            // プレースホルダ: 下部に半透明バー（合成と透過の在席確認用）。
            if let Ok(brush) = ctx.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 0.10,
                    g: 0.10,
                    b: 0.12,
                    a: 0.78,
                },
                None,
            ) {
                let bar = D2D_RECT_F {
                    left: 12.0,
                    top: (ch - BAR_H) as f32 + 8.0,
                    right: cw as f32 - 12.0,
                    bottom: ch as f32 - 8.0,
                };
                ctx.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: bar,
                        radiusX: 14.0,
                        radiusY: 14.0,
                    },
                    &brush,
                );
            }
            let _ = ctx.EndDraw(None, None);
            ctx.SetTarget(None);
            let _ = surface.EndDraw();
            let _ = self.dcomp.Commit();
        }
    }
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
    use windows::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, MA_NOACTIVATE, WM_LBUTTONDOWN, WM_MOUSEACTIVATE, WM_MOUSEMOVE,
    };
    match msg {
        // この子窓はアクティブ化しない（winit 親のキーボードフォーカスを奪わない）。
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            if let Some(s) = state_of(hwnd) {
                s.moved = true;
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let cy = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            if let Some(s) = state_of(hwnd) {
                // 骨組み: バー帯以外（＝動画域）クリックは再生/一時停止トグルに変換。
                // バー帯のコントロール割り当ては UI 移植時に追加する。
                if cy < s.ch - BAR_H {
                    s.actions.push(OverlayAction::TogglePause);
                }
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
