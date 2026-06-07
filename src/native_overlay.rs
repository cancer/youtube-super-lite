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
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER,
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

/// 下部コントローラ帯の高さ（2 段: シーク行＋コントロール行）。
const BAR_H: i32 = 104;

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

/// active 時のクリックを各コントロール矩形に振り分ける。非ヒットは TogglePause（動画クリック）。
fn dispatch_hit(s: &OvShared, x: i32, y: i32) -> OverlayAction {
    use OverlayAction::*;
    if s.seekable && s.seek.right > s.seek.left && in_rect(&s.seek, x, y) {
        let f = ((x - s.seek.left) as f64 / (s.seek.right - s.seek.left) as f64).clamp(0.0, 1.0);
        return Seek(f);
    }
    if s.vol.right > s.vol.left && in_rect(&s.vol, x, y) {
        let f = ((x - s.vol.left) as f64 / (s.vol.right - s.vol.left) as f64).clamp(0.0, 1.0);
        return SetVolume(f * 130.0);
    }
    if in_rect(&s.btn, x, y) {
        return TogglePause;
    }
    if in_rect(&s.mute, x, y) {
        return ToggleMute;
    }
    if in_rect(&s.quality, x, y) {
        return CycleQuality;
    }
    if in_rect(&s.codec, x, y) {
        return CycleCodec;
    }
    if in_rect(&s.like, x, y) {
        return Like;
    }
    if in_rect(&s.chat, x, y) {
        return ToggleChat;
    }
    if in_rect(&s.tab_recommend, x, y) {
        return OpenList(ListTab::Recommend);
    }
    if in_rect(&s.tab_subs, x, y) {
        return OpenList(ListTab::Subs);
    }
    if in_rect(&s.tab_playlist, x, y) {
        return OpenList(ListTab::Playlist);
    }
    if in_rect(&s.tab_history, x, y) {
        return OpenList(ListTab::History);
    }
    if in_rect(&s.login, x, y) {
        return Login;
    }
    TogglePause
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

/// 描画ヘルパー。Direct2D の塗り/テキスト/ボタンをまとめる（render の各所から呼ぶ）。
/// 中身は COM ポインタ（参照カウント）のクローンを持つので Overlay 本体を借用しない
/// （描画中に thumb_cache を &mut で触る draw_list と両立させるため）。
struct Painter {
    rt: ID2D1DCRenderTarget,
    /// 左寄せテキスト用フォーマット。
    tf: IDWriteTextFormat,
    /// 中央寄せ（ボタンラベル用）フォーマット。
    tf_c: IDWriteTextFormat,
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

    /// 左寄せテキスト。
    unsafe fn text(&self, s: &str, r: D2D_RECT_F, c: D2D1_COLOR_F) {
        if let Ok(b) = self.rt.CreateSolidColorBrush(&c, None) {
            let wt: Vec<u16> = s.encode_utf16().collect();
            self.rt.DrawText(
                &wt,
                &self.tf,
                &r,
                &b,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    /// 中央寄せテキスト。
    unsafe fn text_center(&self, s: &str, r: D2D_RECT_F, c: D2D1_COLOR_F) {
        if let Ok(b) = self.rt.CreateSolidColorBrush(&c, None) {
            let wt: Vec<u16> = s.encode_utf16().collect();
            self.rt.DrawText(
                &wt,
                &self.tf_c,
                &r,
                &b,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    /// ボタン（角丸背景＋中央ラベル）。`hot` で強調色。
    unsafe fn button(&self, r: RECT, label: &str, hot: bool) {
        let bg = if hot {
            color(0.26, 0.42, 0.72, 0.95)
        } else {
            color(0.22, 0.22, 0.27, 0.88)
        };
        self.fill_round(rf(r), 8.0, bg);
        self.text_center(label, rf(r), color(0.95, 0.95, 0.98, 1.0));
    }
}

/// 親ウィンドウに重ねる透過 2D オーバーレイ。
pub struct Overlay {
    hwnd: HWND,
    _factory: ID2D1Factory,
    dc_rt: ID2D1DCRenderTarget,
    text_format: IDWriteTextFormat,
    text_format_center: IDWriteTextFormat,
    _dwrite: IDWriteFactory,
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
            // ボタンラベル用に中央寄せ（水平・垂直）したフォーマットを別途作る。
            let text_format_center: IDWriteTextFormat = dwrite.CreateTextFormat(
                w!("Yu Gothic UI"),
                None,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                20.0,
                w!("ja-jp"),
            )?;
            let _ = text_format_center.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
            let _ = text_format_center.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
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
                text_format_center,
                _dwrite: dwrite,
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
        quality_label: &str,
        codec_label: &str,
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
                tf_c: self.text_format_center.clone(),
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
                self.draw_top_bar(&p, w, url_input, auth_label, logged_in, &title, &mut hits);
                self.draw_controller(
                    &p,
                    w,
                    h,
                    player,
                    quality_label,
                    codec_label,
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
                // キューは保持したまま矩形だけ差し替える。
                hits.actions = std::mem::take(&mut prev.actions);
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

    /// 上部バー（URL 欄 / タブ / ログイン・認証状態 / 動画タイトル）を描画する。
    unsafe fn draw_top_bar(
        &self,
        p: &Painter,
        w: i32,
        url_input: &str,
        auth_label: &str,
        logged_in: bool,
        title: &str,
        hits: &mut OvShared,
    ) {
        // 上部 UI 帯（URL バー＋タブ行＋タイトル行）のクリック捕捉領域。
        hits.region_top = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: 140,
        };
        // 上部バー帯（URL 欄＋ログイン）。
        let top = RECT {
            left: 12,
            top: 10,
            right: w - 12,
            bottom: 54,
        };
        p.fill_round(rf(top), 10.0, color(0.10, 0.10, 0.12, 0.78));

        // ログイン/認証（右寄せ）。未ログイン時はボタンとして機能（Login）。
        let login = RECT {
            left: w - 12 - 240,
            top: 13,
            right: w - 16,
            bottom: 51,
        };
        if logged_in {
            // ログイン済みはチャンネル名を表示するだけ（クリック不可）。
            p.text(auth_label, rf(shrink(login, 8, 6)), color(0.60, 0.85, 1.0, 1.0));
        } else {
            p.button(login, auth_label, false);
            hits.login = login;
        }

        // URL 欄テキスト（ログイン領域の手前まで）。
        let (txt, col) = if url_input.is_empty() {
            (
                "URL を入力して Enter で再生（英数字キー / Ctrl+V 貼付 / Esc クリア）".to_string(),
                color(0.62, 0.62, 0.65, 1.0),
            )
        } else {
            (format!("URL: {url_input}"), color(1.0, 1.0, 1.0, 1.0))
        };
        let url_rect = D2D_RECT_F {
            left: top.left as f32 + 14.0,
            top: top.top as f32 + 10.0,
            right: login.left as f32 - 12.0,
            bottom: top.bottom as f32,
        };
        p.text(&txt, url_rect, col);

        // タブ行（おすすめ / 登録チャンネル / 再生リスト / 履歴）。
        let ty = 60;
        let th = 36;
        let gap = 8;
        let mut x = 12;
        let mut place = |label: &str, width: i32| -> RECT {
            let r = RECT {
                left: x,
                top: ty,
                right: x + width,
                bottom: ty + th,
            };
            p.button(r, label, false);
            x += width + gap;
            r
        };
        hits.tab_recommend = place("おすすめ", 96);
        hits.tab_subs = place("登録チャンネル", 150);
        hits.tab_playlist = place("再生リスト", 120);
        hits.tab_history = place("履歴", 84);

        // 動画タイトル行（タブ行の下、左寄せ・薄め）。
        if !title.is_empty() {
            let r = D2D_RECT_F {
                left: 16.0,
                top: 102.0,
                right: w as f32 - 16.0,
                bottom: 134.0,
            };
            p.text(&title, r, color(0.92, 0.92, 0.96, 0.96));
        }
    }

    /// 下部コントローラ（シーク行＋コントロール行）を描画する。
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_controller(
        &self,
        p: &Painter,
        w: i32,
        h: i32,
        player: &Player,
        quality_label: &str,
        codec_label: &str,
        chat_open: bool,
        hits: &mut OvShared,
    ) {
        let bar = RECT {
            left: 12,
            top: h - BAR_H + 8,
            right: w - 12,
            bottom: h - 8,
        };
        // 下部コントローラ帯のクリック捕捉領域（バーいっぱい）。
        hits.region_bottom = bar;
        p.fill_round(rf(bar), 14.0, color(0.10, 0.10, 0.12, 0.80));

        let pos = player.time_pos();
        let dur = player.duration();
        let paused = player.paused();
        let seekable = hits.seekable;

        // --- シーク行 ---
        let seek_cy = (bar.top + 22) as f32;
        let seek_l = (bar.left + 20) as f32;
        let time_w = 150.0;
        let seek_r = ((bar.right - 20) as f32 - time_w).max(seek_l + 24.0);
        let track_h = 6.0;
        let frac = if dur > 0.0 {
            (pos / dur).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        // トラック（背景）。
        p.fill_round(
            D2D_RECT_F {
                left: seek_l,
                top: seek_cy - track_h / 2.0,
                right: seek_r,
                bottom: seek_cy + track_h / 2.0,
            },
            3.0,
            if seekable {
                color(0.45, 0.45, 0.5, 0.9)
            } else {
                color(0.55, 0.20, 0.20, 0.9)
            },
        );
        if seekable {
            let knob_x = seek_l + (seek_r - seek_l) * frac;
            p.fill_round(
                D2D_RECT_F {
                    left: seek_l,
                    top: seek_cy - track_h / 2.0,
                    right: knob_x.max(seek_l),
                    bottom: seek_cy + track_h / 2.0,
                },
                3.0,
                color(0.30, 0.60, 1.0, 1.0),
            );
            if let Ok(b) = p.rt.CreateSolidColorBrush(&color(1.0, 1.0, 1.0, 1.0), None) {
                p.rt.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: D2D_POINT_2F {
                            x: knob_x,
                            y: seek_cy,
                        },
                        radiusX: 8.0,
                        radiusY: 8.0,
                    },
                    &b,
                );
            }
            hits.seek = RECT {
                left: seek_l as i32,
                top: (seek_cy - 12.0) as i32,
                right: seek_r as i32,
                bottom: (seek_cy + 12.0) as i32,
            };
        } else {
            // DVR 無しライブ: 固定表示で操作無効。LIVE 表示。
            p.text_center(
                "● LIVE",
                D2D_RECT_F {
                    left: seek_l,
                    top: seek_cy - 14.0,
                    right: seek_l + 90.0,
                    bottom: seek_cy + 14.0,
                },
                color(1.0, 0.45, 0.45, 1.0),
            );
        }
        // 時間表示。
        let time_str = format!("{} / {}", fmt_time(pos), fmt_time(dur));
        p.text(
            &time_str,
            D2D_RECT_F {
                left: seek_r + 12.0,
                top: seek_cy - 14.0,
                right: bar.right as f32 - 8.0,
                bottom: seek_cy + 14.0,
            },
            color(0.95, 0.95, 0.98, 1.0),
        );

        // --- コントロール行 ---
        let cy = bar.top + 66;
        let bh = 36;
        let row = |x: i32, width: i32| -> RECT {
            RECT {
                left: x,
                top: cy - bh / 2,
                right: x + width,
                bottom: cy + bh / 2,
            }
        };
        let mut x = bar.left + 20;

        // 再生/一時停止。
        let btn = row(x, 44);
        p.button(btn, if paused { "▶" } else { "⏸" }, false);
        hits.btn = btn;
        x += 44 + 12;

        // ミュート。
        let muted = player.muted();
        let mute = row(x, 44);
        p.button(mute, if muted { "🔇" } else { "🔊" }, muted);
        hits.mute = mute;
        x += 44 + 8;

        // 音量バー（0–130）。
        let vol = row(x, 130);
        let vol_frac = (player.volume() / 130.0).clamp(0.0, 1.0) as f32;
        let vcy = cy as f32;
        p.fill_round(
            D2D_RECT_F {
                left: vol.left as f32,
                top: vcy - 3.0,
                right: vol.right as f32,
                bottom: vcy + 3.0,
            },
            3.0,
            color(0.45, 0.45, 0.5, 0.9),
        );
        let vx = vol.left as f32 + (vol.right - vol.left) as f32 * vol_frac;
        p.fill_round(
            D2D_RECT_F {
                left: vol.left as f32,
                top: vcy - 3.0,
                right: vx.max(vol.left as f32),
                bottom: vcy + 3.0,
            },
            3.0,
            color(0.30, 0.60, 1.0, 1.0),
        );
        if let Ok(b) = p.rt.CreateSolidColorBrush(&color(1.0, 1.0, 1.0, 1.0), None) {
            p.rt.FillEllipse(
                &D2D1_ELLIPSE {
                    point: D2D_POINT_2F { x: vx, y: vcy },
                    radiusX: 7.0,
                    radiusY: 7.0,
                },
                &b,
            );
        }
        hits.vol = vol;
        x += 130 + 18;

        // 画質。
        let quality = row(x, 96);
        p.button(quality, quality_label, false);
        hits.quality = quality;
        x += 96 + 8;

        // コーデック。
        let codec = row(x, 88);
        p.button(codec, codec_label, false);
        hits.codec = codec;
        x += 88 + 18;

        // 高評価。
        let like = row(x, 48);
        p.button(like, "👍", false);
        hits.like = like;
        x += 48 + 8;

        // チャットトグル。
        let chat = row(x, 48);
        p.button(chat, "💬", chat_open);
        hits.chat = chat;
    }

    /// チャットパネル（右側。video-margin-ratio-right で空けた領域に重ねる）。
    /// パネル矩形を hits.chat_panel に保存し、領域内クリックを無視できるようにする。
    unsafe fn draw_chat(&self, p: &Painter, w: i32, h: i32, chat_lines: &[String], hits: &mut OvShared) {
        let bar_top = (h - BAR_H + 8) as f32;
        let pw = w as f32 * 0.28;
        let px = w as f32 - pw;
        let ptop = 60.0;
        let pbot = bar_top - 8.0;
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

/// 矩形を内側に縮める（テキストのパディング用）。
#[inline]
fn shrink(r: RECT, dx: i32, dy: i32) -> RECT {
    RECT {
        left: r.left + dx,
        top: r.top + dy,
        right: r.right - dx,
        bottom: r.bottom - dy,
    }
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
    use windows::Win32::UI::WindowsAndMessaging::{
        HTCLIENT, HTTRANSPARENT, WM_LBUTTONDOWN, WM_NCHITTEST,
    };
    match msg {
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
                } else if in_rect(&s.chat_panel, x, y) {
                    // チャットパネル領域: クリックを無視。
                } else {
                    let act = dispatch_hit(&s, x, y);
                    s.actions.push(act);
                }
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
