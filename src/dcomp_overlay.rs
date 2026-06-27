//! 新オーバーレイホスト（子窓 + DirectComposition）。ゼロベース実装。
//!
//! 旧 `native_overlay`（WS_EX_LAYERED トップレベル窓を ULW で合成し、親をサブクラス化して
//! 手動追従）の負債を断つための置き換え。設計の核:
//! - オーバーレイは winit 親窓の **WS_CHILD**。位置・クリップ・移動は OS が面倒を見る（follow 不要）。
//! - 合成は **DirectComposition**（D3D11→DXGI→DComp/D2D）。per-pixel alpha を DComp サーフェスで持つ。
//! - 子窓が**全入力を所有**（HTTRANSPARENT 貫通はしない）。
//! - クリッカブル UI は**コンポーネント**（[`Control`]）で構成する。各部品が自分の描画とクリック
//!   挙動を内包し、描画とヒットが drift しない。どの部品にも当たらないクリック＝動画域＝pause、
//!   コントローラ帯の余白は帯パネルが吸収（無反応）。座標の catch-all フォールバックは持たない。
//! - wndproc 連携はグローバル(thread_local)を使わず、窓ごとに `GWLP_USERDATA` へ状態ポインタを置く。
//!
//! 移植状況: コントローラ帯コア（再生/一時停止・シーク・時間・音量）＋自動非表示まで。
//! 画質/コーデック/ミュート/Like・上部バー・一覧・チャットは後続で部品を足す。
//! レイアウト/色は旧 `native_overlay::draw_controller`（egui 踏襲）を参照して合わせる。

#![cfg(windows)]

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use windows::core::Interface;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{ID2D1Bitmap1, ID2D1DeviceContext};
use windows::Win32::Graphics::DirectComposition::{
    IDCompositionDevice, IDCompositionSurface, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::DirectWrite::{IDWriteFactory, IDWriteTextFormat};

/// 子窓への入力で積まれる行動（コアへ渡す）。UI 移植に合わせて拡張する。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OverlayAction {
    /// 再生/一時停止トグル（再生ボタン or 動画域クリック）。
    TogglePause,
    /// シーク（0.0..=1.0 の割合。seekable 時のみ）。
    Seek(f64),
    /// 音量設定（0.0..=130.0）。
    SetVolume(f64),
    /// 音量を相対変更（ホイール。± の量）。
    VolumeStep(f64),
    /// ミュートのトグル。
    ToggleMute,
    /// ライブ配信の先端へ追いつく。
    LiveEdge,
    /// 現在の動画に高評価。
    Like,
    /// 画質を巡回（→ 再生中なら取り直し）。
    CycleQuality,
    /// コーデックを巡回（→ 同上）。
    CycleCodec,
    /// ログイン開始（未ログイン時）。
    Login,
    /// 一覧（指定タブ）を開く。
    OpenList(ListTab),
    /// 一覧の行クリック → その index を再生/ドリル。
    PlayIndex(usize),
    /// 一覧を閉じる（✕ ボタン）。
    CloseList,
    /// 一覧をスクロール（選択を ± 行動かす。ホイール）。
    ListScroll(i32),
    /// チャットパネルの表示トグル。
    ToggleChat,
    /// チャットのスクロール（+ で過去へ、- で最新へ。メッセージ数）。
    ChatScroll(i32),
}

/// チャット行のセグメント（テキスト or インライン絵文字画像）。
pub enum ChatSeg {
    Text(String),
    Emoji { url: String, alt: String },
}

/// チャット 1 行（投稿者種別＋投稿者＋本文セグメント列）。
pub struct ChatLine {
    pub kind: crate::chat::AuthorKind,
    pub author: String,
    pub segs: Vec<ChatSeg>,
}

/// 折返し描画用のトークン（色付きテキスト or 絵文字画像）。
enum ChatTok {
    Text(String, D2D1_COLOR_F),
    Emoji { url: String, alt: String },
}

/// 著者種別ごとの強調色とバッジ記号（Normal は通常色・バッジ無し）。旧 native_overlay と同値。
fn author_accent(kind: crate::chat::AuthorKind) -> Option<(D2D1_COLOR_F, &'static str)> {
    use crate::chat::AuthorKind::*;
    match kind {
        Owner => Some((color(1.0, 0.80, 0.25, 1.0), "👑 ")),
        Moderator => Some((color(0.42, 0.70, 1.0, 1.0), "🔧 ")),
        Member => Some((color(0.45, 0.85, 0.5, 1.0), "★ ")),
        Verified => Some((color(0.75, 0.75, 0.8, 1.0), "✔ ")),
        Normal => None,
    }
}

/// テキストを色付きトークンへ分解（ASCII 連続=1語、空白=独立、非ASCII=1文字＝文字単位折返し可）。
fn push_text_tokens(out: &mut Vec<ChatTok>, t: &str, c: D2D1_COLOR_F) {
    let mut cur = String::new();
    for ch in t.chars() {
        if ch == ' ' {
            if !cur.is_empty() {
                out.push(ChatTok::Text(std::mem::take(&mut cur), c));
            }
            out.push(ChatTok::Text(" ".to_string(), c));
        } else if ch.is_ascii() && !ch.is_control() {
            cur.push(ch);
        } else if !ch.is_control() {
            if !cur.is_empty() {
                out.push(ChatTok::Text(std::mem::take(&mut cur), c));
            }
            out.push(ChatTok::Text(ch.to_string(), c));
        }
    }
    if !cur.is_empty() {
        out.push(ChatTok::Text(cur, c));
    }
}

/// チャット 1 行をトークン列にする（著者種別でバッジ＋名前を強調色に）。
fn tokenize_line(line: &ChatLine, normal: D2D1_COLOR_F) -> Vec<ChatTok> {
    let mut out: Vec<ChatTok> = Vec::new();
    let accent = author_accent(line.kind);
    let author_col = accent.map(|(c, _)| c).unwrap_or(normal);
    if let Some((_, badge)) = accent {
        push_text_tokens(&mut out, badge, author_col);
    }
    push_text_tokens(&mut out, &format!("{}: ", line.author), author_col);
    for seg in &line.segs {
        match seg {
            ChatSeg::Text(t) => push_text_tokens(&mut out, t, normal),
            ChatSeg::Emoji { url, alt } => out.push(ChatTok::Emoji { url: url.clone(), alt: alt.clone() }),
        }
    }
    out
}

/// トークン幅（px）。Emoji は em+2。
fn chat_tok_width(t: &ChatTok, em: f32, measure: &impl Fn(&str) -> f32) -> f32 {
    match t {
        ChatTok::Text(s, _) => measure(s),
        ChatTok::Emoji { .. } => em + 2.0,
    }
}

/// 与えた幅で折返したときの行数（描画しない。tail 算出用）。
fn chat_line_count(toks: &[ChatTok], em: f32, left: f32, right: f32, measure: &impl Fn(&str) -> f32) -> usize {
    let mut cx = left;
    let mut lines = 1usize;
    for t in toks {
        let w = chat_tok_width(t, em, measure);
        if cx > left && cx + w > right {
            lines += 1;
            cx = left;
        }
        cx += w;
    }
    lines
}

/// 一覧のソースタブ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ListTab {
    Recommend,
    Subs,
    Playlist,
    History,
}

/// 描画に必要な再生/UI 状態（コアから毎フレーム渡す）。
pub struct PlaybackView {
    pub paused: bool,
    pub pos: f64,
    pub dur: f64,
    pub seekable: bool,
    pub volume: f64,
    pub muted: bool,
    pub is_live: bool,
    pub quality: String,
    pub codec: String,
    // --- 上部バー ---
    pub url_input: String,
    pub auth_label: String,
    pub logged_in: bool,
    pub title: String,
    pub has_recommend: bool,
    // --- 一覧（list_open 時のみ有効）---
    pub list_open: bool,
    pub list_items: Vec<String>,
    pub list_thumbs: Vec<String>,
    pub list_sel: usize,
    pub list_header: String,
    // --- チャット ---
    pub chat_available: bool,
    pub chat_open: bool,
    pub chat_lines: Vec<ChatLine>,
    pub chat_scroll: usize,
    pub chat_width_ratio: f32,
}

/// 上部バー・下部コントローラ帯の高さ・行高（旧 native_overlay と同値）。
const TOP_H: i32 = 86;
const BOTTOM_H: i32 = 52;
const ROW_H: i32 = 26;
const VOL_W: i32 = 110;

#[derive(Default, Clone, Copy, PartialEq, Debug)]
enum Drag {
    #[default]
    None,
    Seek,
    Vol,
}

/// クリックを当てた部品が返す挙動。
enum Hit {
    /// 操作を生む。
    Act(OverlayAction),
    /// ドラッグ開始（連続更新。初期アクションも伴う）。
    Drag(Drag, OverlayAction),
    /// 領域内だが操作なし（pause を出さずに吸収。例: 時間ラベル）。
    Absorb,
}

/// コントローラを構成するクリッカブル/表示部品。各々が自分の描画とクリック挙動を内包する。
enum Control {
    /// 再生/一時停止ボタン（グリフ）。
    PlayPause { rect: RECT, paused: bool },
    /// シークライン（フル幅）。`enabled=false`（非シーカブル）はグレー固定・操作不可。
    Seek { rect: RECT, frac: f32, enabled: bool },
    /// 音量バー。
    Volume { rect: RECT, frac: f32 },
    /// 時間表示（描画のみ・クリックは吸収）。
    Time { rect: RECT, text: String },
    /// フラットなテキストボタン（ミュート/Like/画質/コーデック/ライブ最新など）。
    Button { rect: RECT, label: String, col: D2D1_COLOR_F, action: OverlayAction },
}

impl Control {
    fn rect(&self) -> RECT {
        match self {
            Control::PlayPause { rect, .. }
            | Control::Seek { rect, .. }
            | Control::Volume { rect, .. }
            | Control::Time { rect, .. }
            | Control::Button { rect, .. } => *rect,
        }
    }

    /// (x,y) がこの部品に当たればその挙動を返す。外れなら None。
    fn press(&self, x: i32, y: i32) -> Option<Hit> {
        if !in_rect(&self.rect(), x, y) {
            return None;
        }
        Some(match self {
            Control::PlayPause { .. } => Hit::Act(OverlayAction::TogglePause),
            Control::Seek { rect, enabled, .. } => {
                if *enabled {
                    Hit::Drag(Drag::Seek, OverlayAction::Seek(frac_x(rect, x)))
                } else {
                    Hit::Absorb
                }
            }
            Control::Volume { rect, .. } => {
                Hit::Drag(Drag::Vol, OverlayAction::SetVolume(frac_x(rect, x) * 130.0))
            }
            Control::Time { .. } => Hit::Absorb,
            Control::Button { action, .. } => Hit::Act(*action),
        })
    }

    unsafe fn draw(&self, p: &Painter) {
        let fg = color(0.96, 0.96, 0.98, 1.0);
        match self {
            Control::PlayPause { rect, paused } => {
                let glyph = if *paused { "▶" } else { "⏸" };
                let cy = (rect.top + rect.bottom) / 2;
                p.text(glyph, rf((rect.left + 4) as f32, (cy - 9) as f32, rect.right as f32, (cy + 9) as f32), fg);
            }
            Control::Time { rect, text } => {
                let cy = (rect.top + rect.bottom) / 2;
                p.text(text, rf(rect.left as f32, (cy - 9) as f32, rect.right as f32, (cy + 9) as f32), fg);
            }
            Control::Seek { rect, frac, enabled } => {
                let cy = ((rect.top + rect.bottom) / 2) as f32;
                let (x0, x1) = (rect.left as f32, rect.right as f32);
                let th = 3.0;
                p.fill_round(rf(x0, cy - th / 2.0, x1, cy + th / 2.0), 1.5, color(1.0, 1.0, 1.0, 0.25));
                let prog_col = if *enabled {
                    color(0.92, 0.20, 0.20, 1.0)
                } else {
                    color(0.55, 0.55, 0.60, 0.9)
                };
                let px = (x0 + (x1 - x0) * *frac).max(x0);
                p.fill_round(rf(x0, cy - th / 2.0, px, cy + th / 2.0), 1.5, prog_col);
                if *enabled {
                    p.fill_ellipse(px, cy, 6.0, color(0.92, 0.20, 0.20, 1.0));
                }
            }
            Control::Volume { rect, frac } => {
                let cy = ((rect.top + rect.bottom) / 2) as f32;
                let (x0, x1) = (rect.left as f32, rect.right as f32);
                p.fill_round(rf(x0, cy - 2.0, x1, cy + 2.0), 2.0, color(1.0, 1.0, 1.0, 0.25));
                let vx = x0 + (x1 - x0) * *frac;
                p.fill_round(rf(x0, cy - 2.0, vx.max(x0), cy + 2.0), 2.0, color(0.92, 0.92, 0.96, 1.0));
                p.fill_ellipse(vx, cy, 5.0, color(1.0, 1.0, 1.0, 1.0));
            }
            Control::Button { rect, label, col, .. } => {
                let cy = (rect.top + rect.bottom) / 2;
                p.text(label, rf((rect.left + 4) as f32, (cy - 9) as f32, (rect.right - 4) as f32, (cy + 9) as f32), *col);
            }
        }
    }
}

/// wndproc から触る窓ごとの状態。`GWLP_USERDATA` に *mut で置く（グローバル不使用）。
#[derive(Default)]
struct WndState {
    actions: Vec<OverlayAction>,
    /// オーバーレイ上でマウスが動いたか（自動非表示タイマのリセット用）。
    moved: bool,
    cw: i32,
    ch: i32,
    /// コントロール表示中か（false の間は全クリックを TogglePause として扱う）。
    active: bool,
    /// コントローラ帯・上部バー（この矩形内の非部品クリックは吸収＝pause を出さない）。
    panel: RECT,
    top_panel: RECT,
    /// 一覧表示中か、と行ジオメトリ（クリック行算出用）。
    list_open: bool,
    list_top: i32,
    list_row_h: i32,
    list_first: usize,
    list_count: usize,
    /// チャットパネル矩形（ホイールでのスクロール判定用）。
    chat_panel: RECT,
    /// 帯のクリッカブル/表示部品。クリックはここを探索する。
    controls: Vec<Control>,
    drag: Drag,
    /// ドラッグ中の対象矩形（連続更新の割合算出に使う）。
    drag_rect: RECT,
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
    /// 一覧サムネの URL→ビットマップキャッシュ（ディスクキャッシュ済みを WIC デコード）。
    thumb_cache: HashMap<String, ID2D1Bitmap1>,
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
                thumb_cache: HashMap::new(),
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

    /// dev-tools 用: クライアント座標へ左クリックを注入する。子窓へ WM_LBUTTONDOWN/UP を
    /// PostMessage し、実 wndproc の振り分けをそのまま通す。
    pub fn inject_click(&self, x: i32, y: i32) {
        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_LBUTTONDOWN, WM_LBUTTONUP};
        let lparam = LPARAM((((y & 0xFFFF) << 16) | (x & 0xFFFF)) as isize);
        unsafe {
            let _ = PostMessageW(self.hwnd, WM_LBUTTONDOWN, WPARAM(0), lparam);
            let _ = PostMessageW(self.hwnd, WM_LBUTTONUP, WPARAM(0), lparam);
        }
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

    /// コントローラ帯の部品リストを現在サイズ・再生状態から組み立てる（描画とヒットの単一の素）。
    /// レイアウトは旧 draw_controller を踏襲: 左フロー（▶/⏸→時間/ライブ→👍）、
    /// 右フロー右→左（音量→🔊/🔇→コーデック→画質）。
    fn build_controls(&self, w: i32, h: i32, v: &PlaybackView) -> Vec<Control> {
        let cy = h - 16;
        let top = cy - ROW_H / 2;
        let bot = cy + ROW_H / 2;
        let fg = color(0.96, 0.96, 0.98, 1.0);
        let row = |l: i32, r: i32| RECT { left: l, top, right: r, bottom: bot };
        let mut controls: Vec<Control> = Vec::new();

        // --- シークライン（フル幅・上段）---
        let sy = h - BOTTOM_H + 13;
        let seek_frac = if !v.seekable {
            1.0
        } else if v.dur > 0.0 {
            (v.pos / v.dur).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        controls.push(Control::Seek {
            rect: RECT { left: 14, top: sy - 9, right: w - 14, bottom: sy + 9 },
            frac: seek_frac,
            enabled: v.seekable,
        });

        // --- 左フロー ---
        // 再生/一時停止。
        let glyph = if v.paused { "▶" } else { "⏸" };
        let gw = unsafe { self.measure(glyph) }.ceil() as i32;
        let btn = row(14, 14 + gw + 8);
        let mut x = btn.right + 12;
        controls.push(Control::PlayPause { rect: btn, paused: v.paused });

        if v.is_live {
            // ライブ: 時間の代わりに「● ライブ」（先端なら赤、遅れていれば白＝追いつける合図）。
            let at_live = !v.seekable || v.dur <= 0.0 || (v.pos / v.dur) >= 0.99;
            let col = if at_live { color(1.0, 0.30, 0.30, 1.0) } else { fg };
            let label = "● ライブ".to_string();
            let lw = unsafe { self.measure(&label) }.ceil() as i32;
            let r = row(x, x + lw + 8);
            x = r.right + 16;
            controls.push(Control::Button { rect: r, label, col, action: OverlayAction::LiveEdge });
        } else {
            let time_str = format!("{} / {}", fmt_time(v.pos), fmt_time(v.dur));
            let tw = unsafe { self.measure(&time_str) }.ceil() as i32;
            let r = row(x, x + tw + 4);
            x = r.right + 16;
            controls.push(Control::Time { rect: r, text: time_str });
        }

        // 👍 高評価。
        let like = "👍".to_string();
        let lw = unsafe { self.measure(&like) }.ceil() as i32;
        controls.push(Control::Button { rect: row(x, x + lw + 8), label: like, col: fg, action: OverlayAction::Like });
        x += lw + 8 + 10;

        // 💬 チャット（接続中 or メッセージがある時のみ）。
        if v.chat_available {
            let (label, col) = if v.chat_open {
                ("💬 非表示".to_string(), color(0.55, 0.80, 1.0, 1.0))
            } else {
                ("💬 チャット".to_string(), fg)
            };
            let cw_ = unsafe { self.measure(&label) }.ceil() as i32;
            controls.push(Control::Button { rect: row(x, x + cw_ + 8), label, col, action: OverlayAction::ToggleChat });
        }

        // --- 右フロー（右→左）---
        let mut xr = w - 14;
        // 音量バー。
        controls.push(Control::Volume {
            rect: row(xr - VOL_W, xr),
            frac: (v.volume / 130.0).clamp(0.0, 1.0) as f32,
        });
        xr -= VOL_W + 10;
        // 🔊/🔇 ミュート。
        let mute = if v.muted { "🔇" } else { "🔊" }.to_string();
        let mw = unsafe { self.measure(&mute) }.ceil() as i32;
        controls.push(Control::Button { rect: row(xr - mw - 8, xr), label: mute, col: fg, action: OverlayAction::ToggleMute });
        xr -= mw + 8 + 14;
        // コーデック。
        let codec = format!("コーデック: {}", v.codec);
        let cw = unsafe { self.measure(&codec) }.ceil() as i32;
        controls.push(Control::Button { rect: row(xr - cw - 8, xr), label: codec, col: fg, action: OverlayAction::CycleCodec });
        xr -= cw + 8 + 12;
        // 画質。
        let quality = format!("画質: {}", v.quality);
        let qw = unsafe { self.measure(&quality) }.ceil() as i32;
        controls.push(Control::Button { rect: row(xr - qw - 8, xr), label: quality, col: fg, action: OverlayAction::CycleQuality });

        controls
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
        let nav_cy = 6 + ROW_H + ROW_H / 2;
        let strip_h = if view.title.is_empty() { TOP_H - ROW_H } else { TOP_H };
        let row_h: i32 = 48;
        let top0: i32 = 64;
        let nav = |x: i32, w: i32| RECT { left: x, top: nav_cy - ROW_H / 2, right: x + w, bottom: nav_cy + ROW_H / 2 };

        // 部品（描画とヒットで同じ素）と各パネルを組み立てる。
        let mut controls: Vec<Control> = Vec::new();
        let mut panel = RECT::default();
        let mut top_panel = RECT::default();
        let (mut list_first, mut list_count) = (0usize, 0usize);

        // チャットパネル（一覧表示中以外で、チャット表示中なら右に出す。active と無関係）。
        let chat_panel = if view.chat_open && !view.list_open {
            let pw = (cw as f32 * view.chat_width_ratio.clamp(0.15, 0.6)) as i32;
            let (ptop, pbot) = (TOP_H + 4, ch - BOTTOM_H - 4);
            if pbot > ptop + 40 {
                RECT { left: cw - pw, top: ptop, right: cw, bottom: pbot }
            } else {
                RECT::default()
            }
        } else {
            RECT::default()
        };
        // チャット表示中はインライン絵文字を（ディスクキャッシュ済みのみ）先にデコードしておく。
        if chat_panel.right > chat_panel.left {
            let urls: Vec<String> = view
                .chat_lines
                .iter()
                .flat_map(|l| {
                    l.segs.iter().filter_map(|s| match s {
                        ChatSeg::Emoji { url, .. } if !url.is_empty() => Some(url.clone()),
                        _ => None,
                    })
                })
                .collect();
            self.ensure_thumbs(&urls);
        }

        if view.list_open {
            // 一覧モード: 全面パネル（余白クリックは吸収）。行クリックは wndproc が PlayIndex 化。
            panel = RECT { left: 0, top: 0, right: cw, bottom: ch };
            let visible = (((ch - top0 - 16) / row_h).max(1)) as usize;
            list_first = if view.list_sel >= visible { view.list_sel - visible + 1 } else { 0 };
            list_count = view.list_items.len();
            // 表示行のサムネを（ディスクキャッシュ済みのみ）デコードしておく。
            let end = (list_first + visible).min(view.list_thumbs.len());
            if list_first < end {
                let urls = view.list_thumbs[list_first..end].to_vec();
                self.ensure_thumbs(&urls);
            }
            // ✕ 閉じるボタン（右上）。
            let xw = unsafe { self.measure("✕") }.ceil() as i32;
            controls.push(Control::Button {
                rect: RECT { left: cw - 16 - xw - 12, top: 14, right: cw - 16, bottom: 14 + ROW_H },
                label: "✕".to_string(),
                col: color(0.95, 0.95, 0.98, 1.0),
                action: OverlayAction::CloseList,
            });
        } else if active {
            controls = self.build_controls(cw, ch, view);
            panel = RECT { left: 0, top: ch - BOTTOM_H, right: cw, bottom: ch };
            top_panel = RECT { left: 0, top: 0, right: cw, bottom: strip_h };
            // 認証（未ログイン=右寄せログインボタン）。
            if !view.logged_in {
                let lw = unsafe { self.measure(&view.auth_label) }.ceil() as i32;
                controls.push(Control::Button {
                    rect: RECT { left: cw - 12 - lw - 8, top: nav_cy - ROW_H / 2, right: cw - 12, bottom: nav_cy + ROW_H / 2 },
                    label: view.auth_label.clone(),
                    col: color(1.0, 0.92, 0.55, 1.0),
                    action: OverlayAction::Login,
                });
            }
            // ナビタブ（左フロー）: おすすめは候補がある時のみ、再生リスト/登録/履歴はログイン時のみ。
            let tab_col = color(0.85, 0.90, 1.0, 1.0);
            let mut tx = 12;
            if view.has_recommend {
                let l = "📋 おすすめ";
                let lw = unsafe { self.measure(l) }.ceil() as i32;
                controls.push(Control::Button { rect: nav(tx, lw + 8), label: l.to_string(), col: tab_col, action: OverlayAction::OpenList(ListTab::Recommend) });
                tx += lw + 18;
            }
            if view.logged_in {
                for (l, tab) in [("📃 再生リスト", ListTab::Playlist), ("📺 登録チャンネル", ListTab::Subs), ("🕘 履歴", ListTab::History)] {
                    let lw = unsafe { self.measure(l) }.ceil() as i32;
                    controls.push(Control::Button { rect: nav(tx, lw + 8), label: l.to_string(), col: tab_col, action: OverlayAction::OpenList(tab) });
                    tx += lw + 18;
                }
            }
        }

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
            // 描画・幾何はすべてクライアント座標。ヒット矩形もクライアント座標で保存する。
            ctx.SetTransform(&Matrix3x2::translation(offset.x as f32, offset.y as f32));
            ctx.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));

            let p = Painter { ctx: &ctx, dwrite: &self.dwrite, tf: &self.tf_small };
            if view.list_open {
                // 一覧（全面パネル）。
                p.fill_rect(rf(0.0, 0.0, cw as f32, ch as f32), color(0.04, 0.04, 0.06, 0.93));
                p.text(&view.list_header, rf(24.0, 18.0, cw as f32 - 24.0, 54.0), color(1.0, 1.0, 1.0, 1.0));
                let visible = (((ch - top0 - 16) / row_h).max(1)) as usize;
                let th = (row_h - 10) as f32;
                let tw = th * 16.0 / 9.0;
                let text_left = 20.0 + tw + 12.0;
                for (i, item) in view.list_items.iter().enumerate().skip(list_first).take(visible) {
                    let y = top0 + (i - list_first) as i32 * row_h;
                    if i == view.list_sel {
                        p.fill_round(rf(16.0, y as f32, cw as f32 - 16.0, (y + row_h - 4) as f32), 6.0, color(0.20, 0.40, 0.85, 0.85));
                    }
                    // サムネ（デコード済みのみ。16:9）。
                    if let Some(bmp) = view.list_thumbs.get(i).and_then(|u| self.thumb_cache.get(u)) {
                        use windows::Win32::Graphics::Direct2D::D2D1_INTERPOLATION_MODE_LINEAR;
                        let dst = rf(20.0, (y + 3) as f32, 20.0 + tw, (y + 3) as f32 + th);
                        ctx.DrawBitmap(bmp, Some(&dst), 1.0, D2D1_INTERPOLATION_MODE_LINEAR, None, None);
                    }
                    let col = if i == view.list_sel { color(1.0, 1.0, 1.0, 1.0) } else { color(0.70, 0.70, 0.75, 1.0) };
                    p.text(item, rf(text_left, (y + 6) as f32, cw as f32 - 28.0, (y + row_h) as f32), col);
                }
                if view.list_items.is_empty() {
                    p.text("（取得中… ログインが必要です）", rf(28.0, (top0 + 4) as f32, cw as f32 - 28.0, (top0 + 44) as f32), color(0.70, 0.70, 0.75, 1.0));
                }
                // ✕ 閉じるボタン等（一覧モードの部品）。
                for c in &controls {
                    c.draw(&p);
                }
            } else if active {
                // 下部コントローラ帯の背景。
                p.fill_rect(rf(panel.left as f32, panel.top as f32, panel.right as f32, panel.bottom as f32), color(0.03, 0.03, 0.05, 0.72));
                // 上部バーの背景。
                p.fill_rect(rf(0.0, 0.0, cw as f32, strip_h as f32), color(0.04, 0.04, 0.06, 0.55));
                // URL 行（先頭）。空なら入力ガイドをグレーで。
                let (url_txt, url_col) = if view.url_input.is_empty() {
                    (
                        "URL: YouTube の URL を入力して Enter（英数字キー / Ctrl+V 貼付 / Esc クリア）".to_string(),
                        color(0.66, 0.66, 0.70, 1.0),
                    )
                } else {
                    (format!("URL: {}", view.url_input), color(1.0, 1.0, 1.0, 1.0))
                };
                p.text(&url_txt, rf(12.0, 6.0, cw as f32 - 12.0, (6 + ROW_H) as f32), url_col);
                // 認証ラベル（ログイン済みは右寄せテキスト。未ログインは Button 追加済み）。
                if view.logged_in {
                    let lw = self.measure(&view.auth_label);
                    p.text(
                        &view.auth_label,
                        rf(cw as f32 - 12.0 - lw, (nav_cy - 9) as f32, cw as f32 - 12.0, (nav_cy + 9) as f32),
                        color(0.70, 0.88, 1.0, 1.0),
                    );
                }
                // タイトル行（あれば）。
                if !view.title.is_empty() {
                    p.text(&view.title, rf(12.0, (6 + ROW_H * 2) as f32, cw as f32 - 12.0, strip_h as f32), color(1.0, 1.0, 1.0, 1.0));
                }
                // 各部品（下部コントロール＋ナビタブ＋ログインボタン）。
                for c in &controls {
                    c.draw(&p);
                }
            }

            // チャットパネル（一覧以外で chat_open のとき。active と無関係に出す）。
            // テキストは語/文字単位で手動折返し、絵文字はインライン画像（未デコード時は alt）。
            if chat_panel.right > chat_panel.left {
                use windows::Win32::Graphics::Direct2D::D2D1_INTERPOLATION_MODE_LINEAR;
                let (px, ptop, pbot) = (chat_panel.left as f32, chat_panel.top as f32, chat_panel.bottom as f32);
                p.fill_rect(rf(px, ptop, cw as f32, pbot), color(0.05, 0.05, 0.07, 0.82));
                p.text("コメント", rf(px + 10.0, ptop + 2.0, cw as f32 - 10.0, ptop + 24.0), color(0.70, 0.70, 0.75, 1.0));

                let normal = color(0.90, 0.90, 0.95, 1.0);
                let left = px + 10.0;
                let right_lim = cw as f32 - 10.0;
                let fs = 15.0f32; // tf_small のサイズ。
                let line_h = (fs * 1.6).max(fs + 8.0);
                let em = fs * 1.5;
                let body_top = ptop + 28.0;
                let avail_h = pbot - body_top - 4.0;
                let n = view.chat_lines.len();
                let end = n.saturating_sub(view.chat_scroll).max(if n > 0 { 1 } else { 0 });
                let measure = |s: &str| self.measure(s);

                // パス1: end から折返し行数を数え、画面に収まる開始 index を決める。
                let mut acc_lines = 0usize;
                let mut start = end;
                for i in (0..end).rev() {
                    let toks = tokenize_line(&view.chat_lines[i], normal);
                    let lc = chat_line_count(&toks, em, left, right_lim, &measure);
                    if start != end && (acc_lines + lc) as f32 * line_h > avail_h {
                        break;
                    }
                    acc_lines += lc;
                    start = i;
                    if acc_lines as f32 * line_h >= avail_h {
                        break;
                    }
                }

                // パス2: start から手動ワードラップで描画。
                let mut y = body_top;
                'outer: for line in &view.chat_lines[start..end] {
                    let toks = tokenize_line(line, normal);
                    let mut cx = left;
                    let mut ln = 0usize;
                    for t in &toks {
                        let tw = chat_tok_width(t, em, &measure);
                        if cx > left && cx + tw > right_lim {
                            ln += 1;
                            cx = left;
                        }
                        let ty = y + ln as f32 * line_h;
                        if ty >= pbot {
                            break 'outer;
                        }
                        match t {
                            ChatTok::Text(s, tc) => {
                                p.text_clip(s, rf(cx, ty, right_lim, ty + line_h), *tc);
                            }
                            ChatTok::Emoji { url, alt } => {
                                if let Some(bmp) = self.thumb_cache.get(url) {
                                    let top = ty + (line_h - em) / 2.0;
                                    ctx.DrawBitmap(bmp, Some(&rf(cx, top, cx + em, top + em)), 1.0, D2D1_INTERPOLATION_MODE_LINEAR, None, None);
                                } else {
                                    p.text_clip(alt, rf(cx, ty, right_lim, ty + line_h), normal);
                                }
                            }
                        }
                        cx += tw;
                    }
                    y += (ln + 1) as f32 * line_h;
                    if y >= pbot {
                        break;
                    }
                }
            }

            let _ = ctx.EndDraw(None, None);
            ctx.SetTarget(None);
            let _ = surface.EndDraw();
            let _ = self.dcomp.Commit();
        }

        // wndproc 用に状態を反映。
        self.state.active = active;
        self.state.cw = cw;
        self.state.ch = ch;
        self.state.panel = panel;
        self.state.top_panel = top_panel;
        self.state.controls = controls;
        self.state.list_open = view.list_open;
        self.state.list_top = top0;
        self.state.list_row_h = row_h;
        self.state.list_first = list_first;
        self.state.list_count = list_count;
        self.state.chat_panel = chat_panel;
    }

    /// 表示中サムネ URL のうち未キャッシュのものを、ディスクキャッシュ済みなら WIC デコードして
    /// ビットマップ化（未キャッシュは非同期取得を起動。ネットワーク取得はしない）。
    fn ensure_thumbs(&mut self, urls: &[String]) {
        for url in urls {
            if url.is_empty() || self.thumb_cache.contains_key(url) {
                continue;
            }
            match crate::image_cache::cached_path(url).and_then(|p| p.to_str().map(String::from)) {
                Some(ps) => {
                    if let Some(bmp) = unsafe { self.load_wic(&ps) } {
                        self.thumb_cache.insert(url.clone(), bmp);
                    }
                }
                None => crate::image_cache::ensure_cached_async(url),
            }
        }
    }

    /// ローカル画像ファイルを WIC でデコードし、d2d_ctx の ID2D1Bitmap1 にする。
    unsafe fn load_wic(&self, path: &str) -> Option<ID2D1Bitmap1> {
        use windows::core::HSTRING;
        use windows::Win32::Foundation::GENERIC_READ;
        use windows::Win32::Graphics::Imaging::{
            CLSID_WICImagingFactory, IWICImagingFactory, WICBitmapDitherTypeNone,
            WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnLoad,
            GUID_WICPixelFormat32bppPBGRA,
        };
        use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
        let wic: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let decoder = wic
            .CreateDecoderFromFilename(&HSTRING::from(path), None, GENERIC_READ, WICDecodeMetadataCacheOnLoad)
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let converter = wic.CreateFormatConverter().ok()?;
        converter
            .Initialize(&frame, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)
            .ok()?;
        self.d2d_ctx.CreateBitmapFromWicBitmap(&converter, None).ok()
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

/// Direct2D 描画の薄いヘルパ（部品の draw が使う）。
struct Painter<'a> {
    ctx: &'a ID2D1DeviceContext,
    dwrite: &'a IDWriteFactory,
    tf: &'a IDWriteTextFormat,
}

impl<'a> Painter<'a> {
    unsafe fn fill_rect(&self, r: D2D_RECT_F, c: D2D1_COLOR_F) {
        if let Ok(b) = self.ctx.CreateSolidColorBrush(&c, None) {
            self.ctx.FillRectangle(&r, &b);
        }
    }
    unsafe fn fill_round(&self, r: D2D_RECT_F, rad: f32, c: D2D1_COLOR_F) {
        use windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT;
        if let Ok(b) = self.ctx.CreateSolidColorBrush(&c, None) {
            self.ctx.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: rad, radiusY: rad }, &b);
        }
    }
    unsafe fn fill_ellipse(&self, x: f32, y: f32, rad: f32, c: D2D1_COLOR_F) {
        use windows::Win32::Graphics::Direct2D::Common::D2D_POINT_2F;
        use windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE;
        if let Ok(b) = self.ctx.CreateSolidColorBrush(&c, None) {
            self.ctx.FillEllipse(
                &D2D1_ELLIPSE { point: D2D_POINT_2F { x, y }, radiusX: rad, radiusY: rad },
                &b,
            );
        }
    }
    unsafe fn text(&self, s: &str, r: D2D_RECT_F, c: D2D1_COLOR_F) {
        use windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_NONE;
        use windows::Win32::Graphics::DirectWrite::DWRITE_MEASURING_MODE_NATURAL;
        let _ = self.dwrite; // 計測は DcompOverlay::measure 側。ここでは描画のみ。
        if let Ok(b) = self.ctx.CreateSolidColorBrush(&c, None) {
            let wt: Vec<u16> = s.encode_utf16().collect();
            self.ctx.DrawText(&wt, self.tf, &r, &b, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        }
    }

    /// 矩形でクリップするテキスト（チャット行の縦/横はみ出し防止）。
    unsafe fn text_clip(&self, s: &str, r: D2D_RECT_F, c: D2D1_COLOR_F) {
        use windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_CLIP;
        use windows::Win32::Graphics::DirectWrite::DWRITE_MEASURING_MODE_NATURAL;
        if let Ok(b) = self.ctx.CreateSolidColorBrush(&c, None) {
            let wt: Vec<u16> = s.encode_utf16().collect();
            self.ctx.DrawText(&wt, self.tf, &r, &b, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        }
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

/// クライアント x を矩形内の割合 0.0..=1.0 に直す。
#[inline]
fn frac_x(r: &RECT, x: i32) -> f64 {
    let w = (r.right - r.left).max(1) as f64;
    ((x - r.left) as f64 / w).clamp(0.0, 1.0)
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
/// クリックは「部品 → 帯余白(吸収) → 動画域(pause)」の順で解決し、catch-all を持たない。
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
                    Drag::Seek => s.actions.push(OverlayAction::Seek(frac_x(&s.drag_rect, lo))),
                    Drag::Vol => s
                        .actions
                        .push(OverlayAction::SetVolume(frac_x(&s.drag_rect, lo) * 130.0)),
                    Drag::None => {}
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let mut capture = false;
            if let Some(s) = state_of(hwnd) {
                if s.list_open {
                    // 一覧モード: まず部品（✕ 閉じる等）、無ければ行クリック → PlayIndex。余白は吸収。
                    let mut handled = false;
                    for c in &s.controls {
                        if let Some(Hit::Act(a)) = c.press(lo, hi) {
                            s.actions.push(a);
                            handled = true;
                            break;
                        }
                    }
                    if !handled && s.list_row_h > 0 && hi >= s.list_top {
                        let row = ((hi - s.list_top) / s.list_row_h) as usize;
                        let idx = s.list_first + row;
                        if idx < s.list_count {
                            s.actions.push(OverlayAction::PlayIndex(idx));
                        }
                    }
                } else if !s.active {
                    // 帯非表示中は全面が動画。クリックで pause（同時に活動として帯が出る）。
                    s.actions.push(OverlayAction::TogglePause);
                } else {
                    // 部品を探索（重なりなし。最初に当たったものを採用）。
                    let mut handled = false;
                    for c in &s.controls {
                        if let Some(hit) = c.press(lo, hi) {
                            match hit {
                                Hit::Act(a) => s.actions.push(a),
                                Hit::Drag(kind, a) => {
                                    s.drag = kind;
                                    s.drag_rect = c.rect();
                                    s.actions.push(a);
                                    capture = true;
                                }
                                Hit::Absorb => {}
                            }
                            handled = true;
                            break;
                        }
                    }
                    if !handled && !in_rect(&s.panel, lo, hi) && !in_rect(&s.top_panel, lo, hi) {
                        // どの部品にも当たらず上下バーの外＝動画域 → pause。バー余白は吸収（無反応）。
                        s.actions.push(OverlayAction::TogglePause);
                    }
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
            // ホイール座標はスクリーン。チャットパネル上か判定するためクライアントへ変換。
            use windows::Win32::Foundation::POINT;
            use windows::Win32::Graphics::Gdi::ScreenToClient;
            let sx = (lparam.0 & 0xFFFF) as i16 as i32;
            let sy = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut pt = POINT { x: sx, y: sy };
            let _ = ScreenToClient(hwnd, &mut pt);
            let delta = ((wparam.0 >> 16) & 0xFFFF) as i16;
            if let Some(s) = state_of(hwnd) {
                let over_chat = s.chat_panel.right > s.chat_panel.left && in_rect(&s.chat_panel, pt.x, pt.y);
                if over_chat {
                    // 上スクロール(delta>0)=過去へ(+)、下=最新へ(-)。1 ノッチ 3 メッセージ。
                    s.actions.push(OverlayAction::ChatScroll(if delta > 0 { 3 } else { -3 }));
                } else if s.list_open {
                    // 一覧表示中はホイールで選択を上下（3 行/ノッチ）。上=過去方向(-)。
                    s.actions.push(OverlayAction::ListScroll(if delta > 0 { -3 } else { 3 }));
                } else {
                    s.actions
                        .push(OverlayAction::VolumeStep(if delta > 0 { 5.0 } else { -5.0 }));
                }
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
