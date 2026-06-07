//! ネイティブ版用の透過 2D オーバーレイ（Direct2D + DirectWrite）。
//!
//! 親ウィンドウ（winit、mpv が D3D11 で動画を描く）の上に重ねる WS_EX_LAYERED の透過窓。
//! [`Player`](crate::player::Player) の再生状態を読み、上部バー（URL/タブ/ログイン/タイトル）と
//! 下部コントローラ（シーク・再生・時間・音量・ミュート・画質・コーデック・高評価・チャット）を
//! Direct2D で描画し、UpdateLayeredWindow(ULW_ALPHA) で per-pixel alpha 合成する。
//!
//! 入力モデル: フォーカス中は窓を常時可視にして WM_NCHITTEST=HTCLIENT で全クリックを捕捉する。
//! `active`（コントロール描画中）の時はクリックを各コントロール矩形へ振り分け、非ヒットや
//! 非 active 時（コントロール非描画）は「動画クリック=再生/一時停止」として TogglePause を積む。
//! 操作は [`OverlayAction`] のキューに積まれ、NativeApp が Player/Controller に適用する。

#![cfg(windows)]

use anyhow::Result;
use std::cell::RefCell;

use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap, ID2D1DCRenderTarget, ID2D1Factory, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_ELLIPSE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT,
    D2D1_RENDER_TARGET_USAGE_GDI_COMPATIBLE, D2D1_ROUNDED_RECT,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL, DWRITE_TEXT_METRICS,
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
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_POPUP, WS_VISIBLE,
};

use crate::player::Player;

/// 下部コントローラ帯の高さ（薄い半透明帯: 細いシークライン＋1 行のフラット操作）。
const BOTTOM_H: i32 = 52;
/// 上部 UI 帯の高さ（URL 行＋ナビ行＋タイトル行）。
const TOP_H: i32 = 86;
/// フラットなテキスト行の高さ（クリック判定用）。
const ROW_H: i32 = 26;

/// 上部バーのタブが指す一覧ソース（NativeApp 側の ListSource へ写す）。
#[derive(Clone, Copy)]
pub enum ListTab {
    Recommend,
    Subs,
    Playlist,
    History,
}

/// オーバーレイのクリックで発生する操作（NativeApp が Player/Controller に適用する）。
#[derive(Clone, Copy)]
pub enum OverlayAction {
    /// 再生/一時停止トグル（コントロール非ヒット＝動画クリックも含む）。
    TogglePause,
    /// シーク（0.0..=1.0 の割合。seekable 時のみ発生）。
    Seek(f64),
    /// 音量設定（0.0..=130.0）。
    SetVolume(f64),
    /// ミュートのトグル。
    ToggleMute,
    /// 現在の動画に高評価。
    Like,
    /// チャットパネルの表示トグル。
    ToggleChat,
    /// 一覧（指定タブ）を開く。
    OpenList(ListTab),
    /// 画質を巡回（→ 再生中なら取り直し）。
    CycleQuality,
    /// コーデックを巡回（→ 同上）。
    CycleCodec,
    /// ログイン開始（未ログイン時）。
    Login,
    /// 一覧の行クリック → その index を再生/ドリル。
    PlayIndex(usize),
}

/// ドラッグ中のスライダー種別（マウスキャプチャ中の連続更新対象）。
#[derive(Default, Clone, Copy, PartialEq)]
enum Drag {
    #[default]
    None,
    Seek,
    Vol,
}

/// wndproc(C コールバック) と描画/NativeApp の橋渡し。UI スレッド単一なので thread_local。
/// コントロール矩形は `active`（コントロール描画中）の時のみ有効。一覧表示中は list_* を使う。
#[derive(Default)]
struct OvShared {
    /// シーク可能か（false の時はシークバーを操作不可にする）。
    seekable: bool,
    // 下部コントローラの各ヒット矩形。
    btn: RECT,
    seek: RECT,
    vol: RECT,
    mute: RECT,
    quality: RECT,
    codec: RECT,
    like: RECT,
    chat: RECT,
    /// チャットパネル領域（クリックを無視して動画クリック=一時停止に落とさない）。
    chat_panel: RECT,
    /// クリックを捕捉する領域（上部 UI 帯・下部コントローラ帯）。この外側は HTTRANSPARENT に
    /// して親 winit ウィンドウへ通し、動画クリック=一時停止を winit 側で処理させる。
    region_top: RECT,
    region_bottom: RECT,
    // 上部バーの各ヒット矩形。
    tab_recommend: RECT,
    tab_subs: RECT,
    tab_playlist: RECT,
    tab_history: RECT,
    login: RECT,
    // 一覧（list_open 時）の行ジオメトリ。
    list_open: bool,
    list_top: i32,
    list_row_h: i32,
    list_first: usize,
    list_count: usize,
    /// ドラッグ中のスライダー（マウスキャプチャ中の連続更新対象）。render 跨ぎで保持する。
    drag: Drag,
    /// クリックで積まれた操作キュー（NativeApp が drain して適用）。
    actions: Vec<OverlayAction>,
}

thread_local! {
    static OV_STATE: RefCell<OvShared> = RefCell::new(OvShared::default());
    /// 親ウィンドウのサブクラス化用: (overlay_hwnd, parent_hwnd, 元の WndProc) を isize で保持。
    static FOLLOW: RefCell<(isize, isize, isize)> = RefCell::new((0, 0, 0));
}

#[inline]
fn in_rect(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

#[inline]
fn rf(r: RECT) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.left as f32,
        top: r.top as f32,
        right: r.right as f32,
        bottom: r.bottom as f32,
    }
}

/// 音量バーのクリック/ドラッグ位置から音量(0–130)を求める。
#[inline]
fn vol_from_x(s: &OvShared, x: i32) -> f64 {
    let f = ((x - s.vol.left) as f64 / (s.vol.right - s.vol.left).max(1) as f64).clamp(0.0, 1.0);
    f * 130.0
}

/// シークバーのクリック/ドラッグ位置から割合(0.0–1.0)を求める。
#[inline]
fn seek_from_x(s: &OvShared, x: i32) -> f64 {
    ((x - s.seek.left) as f64 / (s.seek.right - s.seek.left).max(1) as f64).clamp(0.0, 1.0)
}

/// active 時のクリックを各コントロール矩形に振り分ける。`None` は無反応（no-op）。
/// 非ヒット（バー余白・動画）は TogglePause。
fn dispatch_hit(s: &OvShared, x: i32, y: i32) -> Option<OverlayAction> {
    use OverlayAction::*;
    if s.seek.right > s.seek.left && in_rect(&s.seek, x, y) {
        // seekable 時のみシーク。非 DVR ライブは領域を吸収するだけ（一時停止に落とさない）。
        return if s.seekable {
            Some(Seek(seek_from_x(s, x)))
        } else {
            None
        };
    }
    if s.vol.right > s.vol.left && in_rect(&s.vol, x, y) {
        return Some(SetVolume(vol_from_x(s, x)));
    }
    if in_rect(&s.btn, x, y) {
        return Some(TogglePause);
    }
    if in_rect(&s.mute, x, y) {
        return Some(ToggleMute);
    }
    if in_rect(&s.quality, x, y) {
        return Some(CycleQuality);
    }
    if in_rect(&s.codec, x, y) {
        return Some(CycleCodec);
    }
    if in_rect(&s.like, x, y) {
        return Some(Like);
    }
    if in_rect(&s.chat, x, y) {
        return Some(ToggleChat);
    }
    if in_rect(&s.tab_recommend, x, y) {
        return Some(OpenList(ListTab::Recommend));
    }
    if in_rect(&s.tab_subs, x, y) {
        return Some(OpenList(ListTab::Subs));
    }
    if in_rect(&s.tab_playlist, x, y) {
        return Some(OpenList(ListTab::Playlist));
    }
    if in_rect(&s.tab_history, x, y) {
        return Some(OpenList(ListTab::History));
    }
    if in_rect(&s.login, x, y) {
        return Some(Login);
    }
    Some(TogglePause)
}

/// 画像ファイル（ディスクキャッシュ済み）を WIC でデコードして Direct2D Bitmap を作る。
unsafe fn load_wic_bitmap(dc_rt: &ID2D1DCRenderTarget, path: &str) -> Result<ID2D1Bitmap> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, IWICImagingFactory, WICBitmapDitherTypeNone,
        WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnLoad, GUID_WICPixelFormat32bppPBGRA,
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
    Ok(dc_rt.CreateBitmapFromWicBitmap(&converter, None)?)
}

/// クリップボードの Unicode テキストを取得する（URL 貼り付け用）。
pub fn clipboard_text() -> Option<String> {
    use windows::Win32::Foundation::{HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, OpenClipboard,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    use windows::Win32::System::Ole::CF_UNICODETEXT;
    unsafe {
        if OpenClipboard(None).is_err() {
            return None;
        }
        let result = (|| {
            let h: HANDLE = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let hglobal = HGLOBAL(h.0);
            let ptr = GlobalLock(hglobal) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let mut len = 0usize;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            let _ = GlobalUnlock(hglobal);
            Some(s)
        })();
        let _ = CloseClipboard();
        result
    }
}

/// 描画ヘルパー。Direct2D の塗り/テキストをまとめる（render の各所から呼ぶ）。
/// 中身は COM ポインタ（参照カウント）のクローンを持つので Overlay 本体を借用しない
/// （描画中に thumb_cache を &mut で触る draw_list と両立させるため）。
struct Painter {
    rt: ID2D1DCRenderTarget,
    /// 主フォント（タイトル・URL、22px）。
    tf: IDWriteTextFormat,
    /// 小フォント（コントロール・タブ・時間、15px）。
    tfs: IDWriteTextFormat,
    /// テキスト幅計測用（フラットボタンのヒット矩形算出）。
    dw: IDWriteFactory,
}

impl Painter {
    unsafe fn fill_round(&self, r: D2D_RECT_F, rad: f32, c: D2D1_COLOR_F) {
        if let Ok(b) = self.rt.CreateSolidColorBrush(&c, None) {
            self.rt.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: r,
                    radiusX: rad,
                    radiusY: rad,
                },
                &b,
            );
        }
    }

    unsafe fn fill_rect(&self, r: D2D_RECT_F, c: D2D1_COLOR_F) {
        if let Ok(b) = self.rt.CreateSolidColorBrush(&c, None) {
            self.rt.FillRectangle(&r, &b);
        }
    }

    /// 主フォントの左寄せテキスト（タイトル・URL）。
    unsafe fn text(&self, s: &str, r: D2D_RECT_F, c: D2D1_COLOR_F) {
        self.draw(&self.tf, s, r, c);
    }

    /// 小フォントの左寄せテキスト（コントロール・タブ・時間）。
    unsafe fn text_s(&self, s: &str, r: D2D_RECT_F, c: D2D1_COLOR_F) {
        self.draw(&self.tfs, s, r, c);
    }

    unsafe fn draw(&self, tf: &IDWriteTextFormat, s: &str, r: D2D_RECT_F, c: D2D1_COLOR_F) {
        if let Ok(b) = self.rt.CreateSolidColorBrush(&c, None) {
            let wt: Vec<u16> = s.encode_utf16().collect();
            self.rt.DrawText(
                &wt,
                tf,
                &r,
                &b,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    /// 小フォントでのテキスト幅（px）を計測する。
    unsafe fn measure_s(&self, s: &str) -> f32 {
        let wt: Vec<u16> = s.encode_utf16().collect();
        if let Ok(layout) = self.dw.CreateTextLayout(&wt, &self.tfs, 4096.0, 64.0) {
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_ok() {
                return m.width;
            }
        }
        s.chars().count() as f32 * 9.0
    }

    /// 左端 `x`・縦中心 `cy` に小フォントのフラットなテキストボタンを描き、ヒット矩形を返す。
    unsafe fn flat(&self, x: i32, cy: i32, label: &str, col: D2D1_COLOR_F) -> RECT {
        let tw = self.measure_s(label).ceil() as i32;
        let r = RECT {
            left: x,
            top: cy - ROW_H / 2,
            right: x + tw + 8,
            bottom: cy + ROW_H / 2,
        };
        self.text_s(
            label,
            D2D_RECT_F {
                left: (x + 4) as f32,
                top: (cy - 9) as f32,
                right: (x + 4 + tw) as f32,
                bottom: (cy + 9) as f32,
            },
            col,
        );
        r
    }

    /// 右端 `xr`・縦中心 `cy` に右寄せでフラットなテキストボタンを描き、ヒット矩形を返す。
    unsafe fn flat_right(&self, xr: i32, cy: i32, label: &str, col: D2D1_COLOR_F) -> RECT {
        let tw = self.measure_s(label).ceil() as i32;
        let r = RECT {
            left: xr - tw - 8,
            top: cy - ROW_H / 2,
            right: xr,
            bottom: cy + ROW_H / 2,
        };
        self.text_s(
            label,
            D2D_RECT_F {
                left: (r.left + 4) as f32,
                top: (cy - 9) as f32,
                right: (xr - 4) as f32,
                bottom: (cy + 9) as f32,
            },
            col,
        );
        r
    }
}

/// 親ウィンドウに重ねる透過 2D オーバーレイ。
pub struct Overlay {
    hwnd: HWND,
    _factory: ID2D1Factory,
    dc_rt: ID2D1DCRenderTarget,
    /// 主フォント（タイトル・URL 用、22px）。
    text_format: IDWriteTextFormat,
    /// 小フォント（コントロール・タブ・時間用、15px）。
    text_format_small: IDWriteTextFormat,
    dwrite: IDWriteFactory,
    mem_dc: HDC,
    dib: HBITMAP,
    old_obj: HGDIOBJ,
    dib_w: i32,
    dib_h: i32,
    /// 一覧サムネイルの ID2D1Bitmap キャッシュ（URL → デコード済みビットマップ）。
    thumb_cache: std::collections::HashMap<String, ID2D1Bitmap>,
}

impl Overlay {
    /// 親ウィンドウ（HWND）の上に重ねる透過オーバーレイを作る。
    pub fn new(parent: HWND) -> Result<Self> {
        unsafe {
            // WIC(CoCreateInstance) のため COM を初期化（多重呼び出しは無害）。
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

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

            // WS_EX_TOPMOST は付けない（他アプリの上に浮かせない）。所有者(親)に紐づく
            // オーバーレイとして、親がアクティブな時だけ親の上に表示する。
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_NOACTIVATE,
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
            // コントロール・タブ・時間用の小フォント（フラット表示、egui 相当のコンパクトさ）。
            let text_format_small: IDWriteTextFormat = dwrite.CreateTextFormat(
                w!("Yu Gothic UI"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                15.0,
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

            // 親ウィンドウの WndProc をサブクラス化し、ドラッグ移動(WM_MOVE)中も
            // オーバーレイを追従させる（ドラッグ中は winit のループがモーダルループに入り
            // about_to_wait が止まるため、ここで直接追従させる）。
            use windows::Win32::UI::WindowsAndMessaging::{
                SetWindowLongPtrW, GWLP_WNDPROC, WNDPROC,
            };
            let fp: WNDPROC = Some(follow_wndproc);
            let orig = SetWindowLongPtrW(parent, GWLP_WNDPROC, std::mem::transmute::<WNDPROC, isize>(fp));
            FOLLOW.with(|f| *f.borrow_mut() = (hwnd.0 as isize, parent.0 as isize, orig));

            Ok(Self {
                hwnd,
                _factory: factory,
                dc_rt,
                text_format,
                text_format_small,
                dwrite,
                mem_dc: HDC::default(),
                dib: HBITMAP::default(),
                old_obj: HGDIOBJ::default(),
                dib_w: 0,
                dib_h: 0,
                thumb_cache: std::collections::HashMap::new(),
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

    /// クリックで溜まった操作をすべて取り出す（NativeApp が Player/Controller に適用する）。
    pub fn take_actions(&self) -> Vec<OverlayAction> {
        OV_STATE.with(|s| std::mem::take(&mut s.borrow_mut().actions))
    }

    /// 表示/非表示を切り替える（フォーカス喪失時に隠す）。
    pub fn set_visible(&self, visible: bool) {
        unsafe {
            let _ = ShowWindow(self.hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
    }

    /// 親のクライアント領域に合わせて UI を Direct2D で描画し、ULW で合成する。
    ///
    /// - `active`: コントロール（上部バー＋下部コントローラ）を描くか。false なら描かず、
    ///   全クリックを動画クリック（TogglePause）として扱う。
    /// - `list_open`: 一覧（全面パネル）を描くか。
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        player: &Player,
        parent: HWND,
        url_input: &str,
        active: bool,
        list_open: bool,
        list_items: &[String],
        list_sel: usize,
        list_thumbs: &[String],
        list_header: &str,
        auth_label: &str,
        logged_in: bool,
        has_recommend: bool,
        quality_label: &str,
        codec_label: &str,
        chat_available: bool,
        chat_open: bool,
        chat_lines: &[String],
    ) {
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
            let dc_rt = self.dc_rt.clone();
            dc_rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            dc_rt.BeginDraw();
            dc_rt.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));

            let p = Painter {
                rt: dc_rt.clone(),
                tf: self.text_format.clone(),
                tfs: self.text_format_small.clone(),
                dw: self.dwrite.clone(),
            };

            // このフレームで確定する各ヒット矩形（OV_STATE へ最後に書き出す）。
            let mut hits = OvShared {
                seekable: player.seekable(),
                list_open,
                ..Default::default()
            };

            // チャットパネルは（active と無関係に）チャット表示中なら描く。一覧表示中は
            // 一覧が全面を覆うため描かない（クリックは行選択に使う）。
            if chat_open && !list_open {
                self.draw_chat(&p, w, h, chat_lines, &mut hits);
            }

            // コントロール（上部バー＋下部コントローラ）は active 時のみ描画＆ヒット登録。
            if active && !list_open {
                let title = player.media_title();
                self.draw_top_bar(
                    &p,
                    w,
                    url_input,
                    auth_label,
                    logged_in,
                    has_recommend,
                    &title,
                    &mut hits,
                );
                self.draw_controller(
                    &p,
                    w,
                    h,
                    player,
                    quality_label,
                    codec_label,
                    chat_available,
                    chat_open,
                    &mut hits,
                );
            }

            // 一覧（全面パネル）。開いている時はコントローラ等を覆う。
            if list_open {
                self.draw_list(
                    &p,
                    w,
                    h,
                    list_items,
                    list_sel,
                    list_thumbs,
                    list_header,
                    &mut hits,
                );
            }

            let _ = dc_rt.EndDraw(None, None);

            // ヒット判定用の矩形を wndproc / NativeApp と共有する。
            OV_STATE.with(|s| {
                let mut prev = s.borrow_mut();
                // キューとドラッグ状態は保持したまま矩形だけ差し替える。
                hits.actions = std::mem::take(&mut prev.actions);
                hits.drag = prev.drag;
                *prev = hits;
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

    /// 上部 UI 帯（URL 行 / ナビ行 / タイトル行）を egui 版に倣ってコンパクトに描画する。
    /// 背景は薄い半透明帯、ボタンは枠なしフラットなテキスト（背景塗りなし）で動画を覆わない。
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_top_bar(
        &self,
        p: &Painter,
        w: i32,
        url_input: &str,
        auth_label: &str,
        logged_in: bool,
        has_recommend: bool,
        title: &str,
        hits: &mut OvShared,
    ) {
        // タイトルが無ければ 2 行ぶんに縮める。
        let strip_h = if title.is_empty() { TOP_H - ROW_H } else { TOP_H };
        hits.region_top = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: strip_h,
        };
        // 薄い半透明帯（上から下へ少しフェード気味の濃さ）。
        p.fill_rect(
            D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: w as f32,
                bottom: strip_h as f32,
            },
            color(0.04, 0.04, 0.06, 0.55),
        );

        // 右端: ログイン/認証（ナビ行に右寄せ）。
        let nav_cy = 6 + ROW_H + ROW_H / 2;
        let acc_col = if logged_in {
            color(0.70, 0.88, 1.0, 1.0)
        } else {
            color(1.0, 0.92, 0.55, 1.0)
        };
        let acc_rect = p.flat_right(w - 12, nav_cy, auth_label, acc_col);
        if !logged_in {
            hits.login = acc_rect; // 未ログイン時のみクリックでログイン。
        }

        // URL 行（先頭）。
        let (txt, col) = if url_input.is_empty() {
            (
                "URL: YouTube の URL を入力して Enter（英数字キー / Ctrl+V 貼付 / Esc クリア）".to_string(),
                color(0.66, 0.66, 0.70, 1.0),
            )
        } else {
            (format!("URL: {url_input}"), color(1.0, 1.0, 1.0, 1.0))
        };
        p.text_s(
            &txt,
            D2D_RECT_F {
                left: 12.0,
                top: 6.0,
                right: w as f32 - 12.0,
                bottom: (6 + ROW_H) as f32,
            },
            col,
        );

        // ナビ行（フラットなテキストボタン）。おすすめは候補がある時のみ、
        // 再生リスト/登録チャンネル/履歴はログイン時のみ（egui 版に準拠）。
        let tab_col = color(0.85, 0.90, 1.0, 1.0);
        let mut x = 12;
        if has_recommend {
            let r = p.flat(x, nav_cy, "📋 おすすめ", tab_col);
            hits.tab_recommend = r;
            x = r.right + 10;
        }
        if logged_in {
            let r = p.flat(x, nav_cy, "📃 再生リスト", tab_col);
            hits.tab_playlist = r;
            x = r.right + 10;
            let r = p.flat(x, nav_cy, "📺 登録チャンネル", tab_col);
            hits.tab_subs = r;
            x = r.right + 10;
            let r = p.flat(x, nav_cy, "🕘 履歴", tab_col);
            hits.tab_history = r;
        }

        // タイトル行（あれば）。
        if !title.is_empty() {
            p.text_s(
                title,
                D2D_RECT_F {
                    left: 12.0,
                    top: (6 + ROW_H * 2) as f32,
                    right: w as f32 - 12.0,
                    bottom: strip_h as f32,
                },
                color(1.0, 1.0, 1.0, 1.0),
            );
        }
    }

    /// 下部コントローラ（細いシークライン＋1 行のフラット操作）を egui 版に倣って描画する。
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_controller(
        &self,
        p: &Painter,
        w: i32,
        h: i32,
        player: &Player,
        quality_label: &str,
        codec_label: &str,
        chat_available: bool,
        chat_open: bool,
        hits: &mut OvShared,
    ) {
        // 薄い半透明帯（下端いっぱい）。
        let strip = RECT {
            left: 0,
            top: h - BOTTOM_H,
            right: w,
            bottom: h,
        };
        hits.region_bottom = strip;
        p.fill_rect(rf(strip), color(0.03, 0.03, 0.05, 0.72));

        let pos = player.time_pos();
        let dur = player.duration();
        let paused = player.paused();
        let seekable = hits.seekable;

        // --- シークライン（フル幅・細い、上段）---
        let sx0 = 14.0;
        let sx1 = w as f32 - 14.0;
        let sy = (h - BOTTOM_H + 13) as f32;
        let track_h = 3.0;
        let frac = if !seekable {
            1.0 // 非 DVR ライブはバー 100% 固定（pos/dur で動き続けないように）。
        } else if dur > 0.0 {
            (pos / dur).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        p.fill_round(
            D2D_RECT_F {
                left: sx0,
                top: sy - track_h / 2.0,
                right: sx1,
                bottom: sy + track_h / 2.0,
            },
            1.5,
            color(1.0, 1.0, 1.0, 0.25),
        );
        let prog_col = if seekable {
            color(0.92, 0.20, 0.20, 1.0) // 本家風の赤。
        } else {
            color(0.55, 0.55, 0.60, 0.9) // ライブ固定はグレー。
        };
        p.fill_round(
            D2D_RECT_F {
                left: sx0,
                top: sy - track_h / 2.0,
                right: (sx0 + (sx1 - sx0) * frac).max(sx0),
                bottom: sy + track_h / 2.0,
            },
            1.5,
            prog_col,
        );
        if seekable {
            let knob_x = sx0 + (sx1 - sx0) * frac;
            if let Ok(b) = p.rt.CreateSolidColorBrush(&color(0.92, 0.20, 0.20, 1.0), None) {
                p.rt.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: D2D_POINT_2F { x: knob_x, y: sy },
                        radiusX: 6.0,
                        radiusY: 6.0,
                    },
                    &b,
                );
            }
        }
        // seek 矩形は常に登録（非 DVR ライブのクリックを吸収し一時停止に落とさない。
        // 実際にシークするかは dispatch_hit が seekable で判定）。
        hits.seek = RECT {
            left: sx0 as i32,
            top: (sy - 9.0) as i32,
            right: sx1 as i32,
            bottom: (sy + 9.0) as i32,
        };

        // --- コントロール行（フラット、下段）---
        let cy = h - 16;
        let fg = color(0.96, 0.96, 0.98, 1.0);

        // 左フロー: ▶/⏸ → 時間 → 👍 →（チャットがあれば）💬。
        let mut x = 14;
        let r = p.flat(x, cy, if paused { "▶" } else { "⏸" }, fg);
        hits.btn = r;
        x = r.right + 12;

        let time_str = format!("{} / {}", fmt_time(pos), fmt_time(dur));
        let tw = p.measure_s(&time_str).ceil() as i32;
        p.text_s(
            &time_str,
            D2D_RECT_F {
                left: x as f32,
                top: (cy - 9) as f32,
                right: (x + tw + 4) as f32,
                bottom: (cy + 9) as f32,
            },
            fg,
        );
        x += tw + 16;

        let r = p.flat(x, cy, "👍", fg);
        hits.like = r;
        x = r.right + 10;

        if chat_available {
            let r = p.flat(
                x,
                cy,
                if chat_open { "💬 非表示" } else { "💬 チャット" },
                if chat_open { color(0.55, 0.80, 1.0, 1.0) } else { fg },
            );
            hits.chat = r;
        }

        // 右フロー（右→左）: 音量バー → 🔊/🔇 → コーデック → 画質。
        let mut xr = w - 14;

        // 音量バー（幅 110）。
        let vol_w = 110;
        let vol = RECT {
            left: xr - vol_w,
            top: cy - ROW_H / 2,
            right: xr,
            bottom: cy + ROW_H / 2,
        };
        let vol_frac = (player.volume() / 130.0).clamp(0.0, 1.0) as f32;
        let vcy = cy as f32;
        p.fill_round(
            D2D_RECT_F {
                left: vol.left as f32,
                top: vcy - 2.0,
                right: vol.right as f32,
                bottom: vcy + 2.0,
            },
            2.0,
            color(1.0, 1.0, 1.0, 0.25),
        );
        let vx = vol.left as f32 + vol_w as f32 * vol_frac;
        p.fill_round(
            D2D_RECT_F {
                left: vol.left as f32,
                top: vcy - 2.0,
                right: vx.max(vol.left as f32),
                bottom: vcy + 2.0,
            },
            2.0,
            color(0.92, 0.92, 0.96, 1.0),
        );
        if let Ok(b) = p.rt.CreateSolidColorBrush(&color(1.0, 1.0, 1.0, 1.0), None) {
            p.rt.FillEllipse(
                &D2D1_ELLIPSE {
                    point: D2D_POINT_2F { x: vx, y: vcy },
                    radiusX: 5.0,
                    radiusY: 5.0,
                },
                &b,
            );
        }
        hits.vol = vol;
        xr = vol.left - 10;

        let muted = player.muted();
        let r = p.flat_right(xr, cy, if muted { "🔇" } else { "🔊" }, fg);
        hits.mute = r;
        xr = r.left - 14;

        let r = p.flat_right(xr, cy, &format!("コーデック: {codec_label}"), fg);
        hits.codec = r;
        xr = r.left - 12;

        let r = p.flat_right(xr, cy, &format!("画質: {quality_label}"), fg);
        hits.quality = r;
    }

    /// チャットパネル（右側。video-margin-ratio-right で空けた領域に重ねる）。
    /// パネル矩形を hits.chat_panel に保存し、領域内クリックを無視できるようにする。
    unsafe fn draw_chat(&self, p: &Painter, w: i32, h: i32, chat_lines: &[String], hits: &mut OvShared) {
        let pw = w as f32 * 0.28;
        let px = w as f32 - pw;
        let ptop = (TOP_H + 4) as f32;
        let pbot = (h - BOTTOM_H - 4) as f32;
        if pbot <= ptop + 40.0 {
            return;
        }
        hits.chat_panel = RECT {
            left: px as i32,
            top: ptop as i32,
            right: w,
            bottom: pbot as i32,
        };
        p.fill_rect(
            D2D_RECT_F {
                left: px,
                top: ptop,
                right: w as f32,
                bottom: pbot,
            },
            color(0.05, 0.05, 0.07, 0.82),
        );
        let line_h = 38.0;
        let avail = (((pbot - ptop - 12.0) / line_h).floor() as usize).max(1);
        let n = chat_lines.len();
        let start = n.saturating_sub(avail);
        for (rowi, line) in chat_lines[start..].iter().enumerate() {
            let y = ptop + 6.0 + rowi as f32 * line_h;
            p.text(
                line,
                D2D_RECT_F {
                    left: px + 10.0,
                    top: y,
                    right: w as f32 - 10.0,
                    bottom: y + line_h,
                },
                color(0.90, 0.90, 0.95, 1.0),
            );
        }
    }

    /// 一覧（全面パネル）を描画し、行ジオメトリを hits に保存する。
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_list(
        &mut self,
        p: &Painter,
        w: i32,
        h: i32,
        list_items: &[String],
        list_sel: usize,
        list_thumbs: &[String],
        list_header: &str,
        hits: &mut OvShared,
    ) {
        p.fill_rect(
            D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: w as f32,
                bottom: h as f32,
            },
            color(0.04, 0.04, 0.06, 0.93),
        );
        p.text(
            list_header,
            D2D_RECT_F {
                left: 24.0,
                top: 18.0,
                right: w as f32 - 24.0,
                bottom: 54.0,
            },
            color(1.0, 1.0, 1.0, 1.0),
        );
        let row_h = 48.0f32;
        let top0 = 64.0f32;
        let visible = (((h as f32 - top0 - 16.0) / row_h).floor() as usize).max(1);
        let first = if list_sel >= visible {
            list_sel - visible + 1
        } else {
            0
        };
        // 表示行のサムネイルを、ディスクキャッシュ済みのものだけ WIC デコードしてキャッシュ。
        let dc_rt_clone = self.dc_rt.clone();
        for idx in first..(first + visible).min(list_thumbs.len()) {
            let url = &list_thumbs[idx];
            if !url.is_empty() && !self.thumb_cache.contains_key(url) {
                match crate::image_cache::cached_path(url)
                    .and_then(|p| p.to_str().map(String::from))
                {
                    Some(ps) => {
                        if let Ok(bmp) = load_wic_bitmap(&dc_rt_clone, &ps) {
                            self.thumb_cache.insert(url.clone(), bmp);
                        }
                    }
                    None => crate::image_cache::ensure_cached_async(url),
                }
            }
        }
        let th = row_h - 10.0;
        let tw = th * 16.0 / 9.0;
        let text_left = 20.0 + tw + 12.0;
        for (i, item) in list_items.iter().enumerate().skip(first).take(visible) {
            let y = top0 + (i - first) as f32 * row_h;
            if i == list_sel {
                p.fill_round(
                    D2D_RECT_F {
                        left: 16.0,
                        top: y,
                        right: w as f32 - 16.0,
                        bottom: y + row_h - 4.0,
                    },
                    6.0,
                    color(0.20, 0.40, 0.85, 0.85),
                );
            }
            if let Some(bmp) = list_thumbs.get(i).and_then(|u| self.thumb_cache.get(u)) {
                let dst = D2D_RECT_F {
                    left: 20.0,
                    top: y + 3.0,
                    right: 20.0 + tw,
                    bottom: y + 3.0 + th,
                };
                p.rt.DrawBitmap(
                    bmp,
                    Some(&dst),
                    1.0,
                    windows::Win32::Graphics::Direct2D::D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                    None,
                );
            }
            let col = if i == list_sel {
                color(1.0, 1.0, 1.0, 1.0)
            } else {
                color(0.70, 0.70, 0.75, 1.0)
            };
            p.text(
                item,
                D2D_RECT_F {
                    left: text_left,
                    top: y + 6.0,
                    right: w as f32 - 28.0,
                    bottom: y + row_h,
                },
                col,
            );
        }
        if list_items.is_empty() {
            p.text(
                "（取得中… ログインが必要です）",
                D2D_RECT_F {
                    left: 28.0,
                    top: top0 + 4.0,
                    right: w as f32 - 28.0,
                    bottom: top0 + 44.0,
                },
                color(0.70, 0.70, 0.75, 1.0),
            );
        }
        hits.list_top = top0 as i32;
        hits.list_row_h = row_h as i32;
        hits.list_first = first;
        hits.list_count = list_items.len();
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

/// 親ウィンドウのサブクラス WndProc。移動・位置変更時にオーバーレイを親クライアント原点へ
/// 即座に追従させ、それ以外は元の（winit の）WndProc に委譲する。
unsafe extern "system" fn follow_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, WM_MOVE,
        WM_WINDOWPOSCHANGED, WNDPROC,
    };
    let (ov, parent, orig) = FOLLOW.with(|f| *f.borrow());
    if ov != 0 && (msg == WM_MOVE || msg == WM_WINDOWPOSCHANGED) {
        let ovh = HWND(ov as *mut core::ffi::c_void);
        let parenth = HWND(parent as *mut core::ffi::c_void);
        let mut o = POINT { x: 0, y: 0 };
        let _ = ClientToScreen(parenth, &mut o);
        let _ = SetWindowPos(
            ovh,
            None,
            o.x,
            o.y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
    let orig_proc: WNDPROC = std::mem::transmute::<isize, WNDPROC>(orig);
    CallWindowProcW(orig_proc, hwnd, msg, wparam, lparam)
}

unsafe extern "system" fn overlay_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Graphics::Gdi::ScreenToClient;
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
    use windows::Win32::UI::WindowsAndMessaging::{
        HTCLIENT, HTTRANSPARENT, MA_NOACTIVATE, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE,
        WM_MOUSEMOVE, WM_NCHITTEST,
    };
    match msg {
        // クリックされてもこの窓を activate せず、親(winit)も非アクティブ化させない。
        // これを返さないと既定動作で親が WM_KILLFOCUS → winit Focused(false) になり
        // オーバーレイが隠れてしまう（＝クリックで UI が消える）。クリック自体は食わず
        // WM_LBUTTONDOWN として配送される（MA_NOACTIVATEANDEAT ではない）。
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        // コントロール帯（上部 UI／下部コントローラ）・チャットパネル・一覧の上だけ HTCLIENT で
        // 捕捉し、それ以外は HTTRANSPARENT で親 winit ウィンドウへ通す（動画クリック=一時停止は
        // winit 側の MouseInput で処理）。
        WM_NCHITTEST => {
            let sx = (lparam.0 & 0xFFFF) as i16 as i32;
            let sy = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut pt = POINT { x: sx, y: sy };
            let _ = ScreenToClient(hwnd, &mut pt);
            let hit = OV_STATE.with(|s| {
                let s = s.borrow();
                s.list_open
                    || in_rect(&s.chat_panel, pt.x, pt.y)
                    || in_rect(&s.region_top, pt.x, pt.y)
                    || in_rect(&s.region_bottom, pt.x, pt.y)
            });
            if hit {
                LRESULT(HTCLIENT as isize)
            } else {
                LRESULT(HTTRANSPARENT as isize)
            }
        }
        // クリック: 一覧表示中は行選択、チャットパネルは無視、コントロール帯はコントロールへ
        // 振り分け（バー余白の非ヒットは TogglePause）。動画領域のクリックはここには来ない。
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut capture = false;
            OV_STATE.with(|s| {
                let mut s = s.borrow_mut();
                if s.list_open {
                    if s.list_row_h > 0 && y >= s.list_top {
                        let row = ((y - s.list_top) / s.list_row_h) as usize;
                        let idx = s.list_first + row;
                        if idx < s.list_count {
                            s.actions.push(OverlayAction::PlayIndex(idx));
                        }
                    }
                    return;
                }
                if in_rect(&s.chat_panel, x, y) {
                    return; // チャットパネル領域: クリックを無視。
                }
                // スライダー上で押したらドラッグ開始（マウスキャプチャして領域外でも追従）。
                if s.seekable && s.seek.right > s.seek.left && in_rect(&s.seek, x, y) {
                    s.drag = Drag::Seek;
                    capture = true;
                } else if s.vol.right > s.vol.left && in_rect(&s.vol, x, y) {
                    s.drag = Drag::Vol;
                    capture = true;
                }
                if let Some(act) = dispatch_hit(&s, x, y) {
                    s.actions.push(act);
                }
            });
            if capture {
                let _ = SetCapture(hwnd);
            }
            LRESULT(0)
        }
        // ドラッグ中はスライダーを連続更新（キャプチャ中なので領域外でも x を clamp して反映）。
        // `drag` はボタン押下中のみ非 None なので、これで「押しながら移動」だけを拾える。
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            OV_STATE.with(|s| {
                let mut s = s.borrow_mut();
                match s.drag {
                    Drag::Seek => {
                        let f = seek_from_x(&s, x);
                        s.actions.push(OverlayAction::Seek(f));
                    }
                    Drag::Vol => {
                        let v = vol_from_x(&s, x);
                        s.actions.push(OverlayAction::SetVolume(v));
                    }
                    Drag::None => {}
                }
            });
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            OV_STATE.with(|s| s.borrow_mut().drag = Drag::None);
            let _ = ReleaseCapture();
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
