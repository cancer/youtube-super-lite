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
//! レイアウト/色は egui 版 redraw 踏襲（旧 `native_overlay` の ULW 実装は撤去済み。
//! 必要なら git 履歴 / tag `legacy-ulw-overlay` を参照）。

#![cfg(windows)]

use crate::design as ds;
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
/// `String` を持つ variant があるため `Copy` は導出しない。
#[derive(Debug, Clone, PartialEq)]
pub enum OverlayAction {
    /// 再生/一時停止トグル（再生ボタン or 動画域クリック）。
    TogglePause,
    /// シーク（0.0..=1.0 の割合。seekable 時のみ。シークバードラッグ用）。
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
    /// 一覧の行クリック → その video_id を再生する（描画順の座席番号ではなく実 ID。
    /// クリックと適用の間に一覧が更新されても別の動画を指さない）。
    Play { video_id: String },
    /// カードのアバター/チャンネル名クリック → 実 channelId（無ければ名前検索）でチャンネルを開く。
    OpenChannel { id: Option<String>, name: String },
    /// カードのケバブ(⋮)クリック → その index のコンテキストメニューを開く（表示位置の話なので
    /// index のままでよい）。
    OpenCardMenu(usize),
    /// 開いているカードメニューを閉じる（メニュー外クリック / ✕ 相当）。
    CloseCardMenu,
    /// メニュー項目「後で見るに保存」（対象の video_id）。
    SaveWatchLater(String),
    /// メニュー項目「興味なし」（feedbackToken）。
    NotInterested(String),
    /// メニュー項目「チャンネルをおすすめに表示しない」（feedbackToken）。
    NotRecommendChannel(String),
    /// 一覧を閉じる（✕ ボタン）。
    CloseList,
    /// 一覧をスクロール（選択を ± 行動かす。ホイール）。
    ListScroll(i32),
    /// チャットパネルの表示トグル。
    ToggleChat,
    /// チャットのスクロール（+ で過去へ、- で最新へ。メッセージ数）。
    ChatScroll(i32),
    /// チャット欄の幅（ウィンドウ幅比 0.15..=0.6）を設定（左端ドラッグ）。
    SetChatWidth(f64),
    /// チャット文字サイズを小さく / 大きく。
    ChatFontDec,
    ChatFontInc,
    /// EQ パネルの表示トグル（コントローラ帯の EQ ボタン）。
    ToggleEq,
    /// EQ: ボイス帯域ゲインを dB で絶対設定（スライダードラッグ用）。
    SetEqVoice(f64),
    /// EQ: ローパスカットオフを絶対設定（スライダードラッグ用。None=オフ）。
    SetEqLowpass(Option<f64>),
    /// EQ: ハイパスカットオフを絶対設定（スライダードラッグ用。None=オフ）。
    SetEqHighpass(Option<f64>),
    /// EQ: 全ニュートラル（リセットボタン）。
    EqReset,
}

/// EQ パネルのスライダー3本を識別する。
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum EqBand {
    Voice,
    Low,
    High,
}

/// チャット行のセグメント（テキスト or インライン絵文字画像）。
pub enum ChatSeg {
    Text(String),
    Emoji { url: String, alt: String },
}

/// チャット 1 行（投稿者種別＋投稿者＋本文セグメント列）。
pub struct ChatLine {
    pub kind: ysl_core::yt::chat::AuthorKind,
    pub author: String,
    pub segs: Vec<ChatSeg>,
}

/// 折返し描画用のトークン（色付きテキスト or 絵文字画像）。
enum ChatTok {
    Text(String, D2D1_COLOR_F),
    Emoji { url: String, alt: String },
}

/// 著者種別ごとの強調色とバッジ記号（Normal は通常色・バッジ無し）。旧 native_overlay と同値。
fn author_accent(kind: ysl_core::yt::chat::AuthorKind) -> Option<(D2D1_COLOR_F, &'static str)> {
    use ysl_core::yt::chat::AuthorKind::*;
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

/// 一覧の 1 項目（動画カード or 再生リスト行）。
///
/// `title`/`channel`/`thumb`/`id` は現状のデータ源から常に埋まる。`avatar`/`duration`/`live`/
/// `meta`/`verified` は [`ysl_core::yt::recommend::VideoItem`]（おすすめ）では常に埋まるが、
/// `subscriptions`/`history` 側はまだ未対応で既定値のまま（あれば描く）。
#[derive(Clone, Default)]
pub struct Card {
    /// 再生対象の video_id（再生リスト一覧では playlist_id）。
    pub id: String,
    pub title: String,
    /// チャンネル名（再生リスト一覧では件数などのサブ情報を入れる）。
    pub channel: String,
    /// サムネ URL（空なら未指定＝プレースホルダ）。
    pub thumb: String,
    /// チャンネルアバター URL（空なら未指定＝プレースホルダ円）。
    pub avatar: String,
    /// 再生時間（秒）。あれば時間バッジを描く。
    pub duration: Option<f64>,
    /// ライブ配信フラグ。true なら LIVE バッジ。
    pub live: bool,
    /// 視聴回数/経過時間などのメタ行（任意）。
    pub meta: Option<String>,
    /// 認証チャンネルの✔（任意）。
    pub verified: bool,
    /// ケバブメニュー用データ（実チャンネルID／興味なし・非表示の feedbackToken）。
    pub menu: ysl_core::yt::subscriptions::CardMenu,
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
    // --- 一覧（list_open 時のみ有効）---
    pub list_open: bool,
    pub list_cards: Vec<Card>,
    /// 現在の一覧ソースの取得が進行中か（空一覧の「取得中…」表示用）。
    pub list_busy: bool,
    pub list_sel: usize,
    pub list_header: String,
    /// 現在の一覧ソース（サイドバーのアクティブ表示用）。
    pub list_tab: ListTab,
    /// ケバブで開いているカードメニューの index（無ければ None）。
    pub card_menu_open: Option<usize>,
    // --- チャット ---
    pub chat_available: bool,
    pub chat_open: bool,
    pub chat_lines: Vec<ChatLine>,
    pub chat_scroll: usize,
    pub chat_width_ratio: f32,
    pub chat_font_px: f32,
    // --- EQ パネル ---
    pub eq_open: bool,
    pub eq: ysl_core::types::EqParams,
}

/// 上部バー・下部コントローラ帯の高さ・行高（旧 native_overlay と同値）。
const TOP_H: i32 = 86;
const BOTTOM_H: i32 = 52;
const ROW_H: i32 = 26;
const VOL_W: i32 = 110;

/// 一覧（おすすめグリッド）レイアウト定数（DESIGN / おすすめ.dc.html 準拠）。
const TITLEBAR_H: f32 = 44.0; // 一覧パネル上部の見出し帯
const SIDEBAR_W: f32 = 224.0; // 左サイドバー幅
const NAV_ROW_H: f32 = 40.0; // サイドバーナビ行高

/// おすすめグリッドの算出済みジオメトリ。描画・サムネ先読み・ヒット判定で共有する。
#[derive(Clone, Copy)]
struct BrowseGrid {
    cols: usize,
    card_w: f32,
    thumb_h: f32,
    card_h: f32,
    row_pitch: f32,
    content_x: f32,
    grid_y0: f32,
    /// 先頭に描くカード index（縦スクロール相当。選択が見える位置に寄せる）。
    first: usize,
    /// 画面に収まるカード行数。
    visible_rows: usize,
    heading_y: f32,
    chips_y: f32,
}

/// list_open 時のグリッド寸法を、クライアント幅・件数・選択から算出する。
fn browse_grid(cw: i32, ch: i32, count: usize, sel: usize) -> BrowseGrid {
    let pad = ds::SPACE_SECTION; // 24
    let gap = ds::GAP_LOOSE; // 16
    let content_x = SIDEBAR_W + pad;
    let content_w = (cw as f32 - content_x - pad).max(200.0);
    // 目標カード幅 ~300px。列数は 2..=4 でクランプ（デザイン基準 3）。
    let cols = (((content_w + gap) / (300.0 + gap)).floor() as usize).clamp(2, 4);
    let card_w = ((content_w - gap * (cols - 1) as f32) / cols as f32).max(120.0);
    let thumb_h = card_w * 9.0 / 16.0;
    let info_h = 64.0; // タイトル2行 + チャンネル + メタ
    let card_h = thumb_h + ds::GAP_TIGHT + info_h;
    let row_pitch = card_h + ds::SPACE_SECTION;

    let heading_y = TITLEBAR_H + ds::GAP_LOOSE;
    let chips_y = heading_y + ds::SIZE_3XL + ds::GAP_TIGHT;
    let grid_y0 = chips_y + 32.0 + ds::GAP_LOOSE; // チップ行(32) の下

    let grid_h = (ch as f32 - grid_y0 - ds::GAP_LOOSE).max(row_pitch);
    let visible_rows = ((grid_h + ds::SPACE_SECTION) / row_pitch).floor().max(1.0) as usize;

    let sel_row = if cols > 0 { sel / cols } else { 0 };
    let first_row = sel_row.saturating_sub(visible_rows.saturating_sub(1));
    let first = first_row * cols.max(1);
    let _ = count;

    BrowseGrid {
        cols,
        card_w,
        thumb_h,
        card_h,
        row_pitch,
        content_x,
        grid_y0,
        first,
        visible_rows,
        heading_y,
        chips_y,
    }
}

#[derive(Default, Clone, Copy, PartialEq, Debug)]
enum Drag {
    #[default]
    None,
    Seek,
    Vol,
    /// チャット欄の左端をドラッグして幅変更中。
    ChatW,
    /// EQ パネルのスライダーをドラッグ中（対象バンド）。
    Eq(EqBand),
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
    /// EQ スライダー（ラベル付き。Volume と同じトラック＋つまみ描画・ドラッグ挙動）。
    EqSlider { rect: RECT, frac: f32, band: EqBand, label: String },
}

impl Control {
    fn rect(&self) -> RECT {
        match self {
            Control::PlayPause { rect, .. }
            | Control::Seek { rect, .. }
            | Control::Volume { rect, .. }
            | Control::Time { rect, .. }
            | Control::Button { rect, .. }
            | Control::EqSlider { rect, .. } => *rect,
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
            Control::Button { action, .. } => Hit::Act(action.clone()),
            Control::EqSlider { rect, band, .. } => {
                Hit::Drag(Drag::Eq(*band), eq_action_from_frac(*band, frac_x(rect, x)))
            }
        })
    }

    unsafe fn draw(&self, p: &Painter) {
        let fg = ds::TEXT_PRIMARY;
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
                p.fill_round(rf(x0, cy - th / 2.0, x1, cy + th / 2.0), 1.5, ds::alpha(ds::TEXT_PRIMARY, 0.25));
                let prog_col = if *enabled {
                    ds::ACCENT_BRAND
                } else {
                    ds::alpha(ds::TEXT_DISABLED, 0.9)
                };
                let px = (x0 + (x1 - x0) * *frac).max(x0);
                p.fill_round(rf(x0, cy - th / 2.0, px, cy + th / 2.0), 1.5, prog_col);
                if *enabled {
                    p.fill_ellipse(px, cy, 6.0, ds::ACCENT_BRAND);
                }
            }
            Control::Volume { rect, frac } => {
                let cy = ((rect.top + rect.bottom) / 2) as f32;
                draw_slider_track(p, rect.left as f32, rect.right as f32, cy, *frac);
            }
            Control::Button { rect, label, col, .. } => {
                let cy = (rect.top + rect.bottom) / 2;
                p.text(label, rf((rect.left + 4) as f32, (cy - 9) as f32, (rect.right - 4) as f32, (cy + 9) as f32), *col);
            }
            Control::EqSlider { rect, frac, label, .. } => {
                let cy = ((rect.top + rect.bottom) / 2) as f32;
                // ラベルはトラック左に固定幅で確保し、Control::Time と同じ縦センタリングで描く。
                let label_w = EQ_SLIDER_LABEL_W;
                p.text(label, rf(rect.left as f32, cy - 9.0, rect.left as f32 + label_w - 8.0, cy + 9.0), fg);
                draw_slider_track(p, rect.left as f32 + label_w, rect.right as f32, cy, *frac);
            }
        }
    }
}

/// スライダーのトラック＋つまみを描く（Volume と EqSlider で共通の見た目）。
unsafe fn draw_slider_track(p: &Painter, x0: f32, x1: f32, cy: f32, frac: f32) {
    p.fill_round(rf(x0, cy - 2.0, x1, cy + 2.0), 2.0, ds::alpha(ds::TEXT_PRIMARY, 0.25));
    let vx = x0 + (x1 - x0) * frac;
    p.fill_round(rf(x0, cy - 2.0, vx.max(x0), cy + 2.0), 2.0, ds::TEXT_PRIMARY);
    p.fill_ellipse(vx, cy, 5.0, ds::TEXT_ON_ACCENT);
}

/// EQ スライダーのラベル領域幅（px。トラックはこの右側から始まる）。
const EQ_SLIDER_LABEL_W: f32 = 132.0;

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
    /// 一覧（おすすめグリッド）表示中か。
    list_open: bool,
    /// おすすめグリッドのカード矩形→video_id（クリック→Play）。位置(index)ではなく実 ID を持つ
    /// ため、描画とクリックの間に一覧が更新されても別の動画を指さない。
    card_hits: Vec<(RECT, String)>,
    /// カード内の小領域（アバター/チャンネル→OpenChannel, ケバブ→OpenCardMenu）。card_hits より優先。
    card_extra_hits: Vec<(RECT, OverlayAction)>,
    /// カードメニューが開いているか、と各項目の矩形→アクション。開いている間はこれを最優先で
    /// 判定し、当たらなければ CloseCardMenu を発火してそのクリックを消費する（吸収）。
    menu_open: bool,
    menu_hits: Vec<(RECT, OverlayAction)>,
    /// サイドバーナビ矩形→切替アクション（クリック→OpenList）。
    nav_hits: Vec<(RECT, OverlayAction)>,
    /// 現在のグリッド列数（ホイール/キーの 1 行移動量）。
    grid_cols: usize,
    /// チャットパネル矩形（ホイールでのスクロール判定用）と左端リサイズハンドル。
    chat_panel: RECT,
    chat_resize: RECT,
    /// EQ パネル矩形（ヒットテスト用。余白クリックで pause させないための除外領域）。
    eq_panel: RECT,
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
    /// 見出しフォント（一覧ページ見出し、36px/700）。
    tf_title: IDWriteTextFormat,
    /// チャット用フォント（ユーザー調整サイズ。NO_WRAP。size 変化時に作り直す）。
    chat_tf: Option<IDWriteTextFormat>,
    chat_tf_px: f32,
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
            DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL,
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
            // 一覧ページ見出し用（3xl / bold）。DESIGN.md の page-title 相当。
            let tf_title: IDWriteTextFormat = dwrite.CreateTextFormat(
                w!("Yu Gothic UI"),
                None,
                DWRITE_FONT_WEIGHT_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                ds::SIZE_3XL,
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
                tf_title,
                chat_tf: None,
                chat_tf_px: 0.0,
                thumb_cache: HashMap::new(),
                cw,
                ch,
            };
            me.rebuild_surface()?;
            Ok(me)
        }
    }

    /// 現在のおすすめグリッドの列数（キーボードの 1 行移動量。未描画時は 1）。
    pub fn grid_cols(&self) -> usize {
        self.state.grid_cols.max(1)
    }

    /// オーバーレイ子窓を兄弟の最前面（入力の受け口）に保つ。
    ///
    /// mpv は `wid`(親窓)に d3d11 VO 用の子窓を動画ロード時に生成し、それが z-order 最前面に
    /// 入るため、放置すると実マウス入力を mpv 子窓が先取りしてしまう（DirectComposition は
    /// 見た目だけ DWM が最前面に合成するので、視覚と入力の最前面がズレる）。ここで毎フレーム
    /// トップを再主張して入力を確実にオーバーレイへ入れる。`SWP_NOACTIVATE` で winit 親窓の
    /// キーボードフォーカスは奪わない。透過は子窓の per-pixel alpha でそのまま効く。
    fn ensure_topmost(&self) {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindow, SetWindowPos, GW_HWNDPREV, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        };
        unsafe {
            // 兄弟の最前面なら GW_HWNDPREV は無い。上に別窓（mpv 子窓等）がある時だけ引き上げる
            // （毎フレームの冗長な SetWindowPos と z-order の取り合いを避ける）。
            if GetWindow(self.hwnd, GW_HWNDPREV).is_err() {
                return;
            }
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

    /// オーバーレイ子窓が現在、兄弟の最前面（＝実マウス入力を受けられる位置）にあるかを返す。
    /// 入力は一切動かさない読み取り専用の z-order 確認（dev-tools `/state` の検証用）。
    pub fn is_topmost(&self) -> bool {
        use windows::Win32::UI::WindowsAndMessaging::{GetWindow, GW_HWNDPREV};
        unsafe { GetWindow(self.hwnd, GW_HWNDPREV).is_err() }
    }

    /// オーバーレイ子窓の可視状態を切り替える（PR4 WebView2 経路切替用）。
    /// SW_SHOWNA を使いフォーカスを奪わない。ここでは可視のみで、
    /// 描画自体の抑止（render 呼出しの skip）は呼び出し側が担う。
    #[allow(dead_code)]
    pub fn set_visible(&self, visible: bool) {
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE, SW_SHOWNA};
        unsafe {
            let _ = ShowWindow(self.hwnd, if visible { SW_SHOWNA } else { SW_HIDE });
        }
    }

    /// オーバーレイ子窓の HWND を isize で返す（子窓列挙で mpv VO を識別する際に、
    /// 「これは overlay なので除外」と判定するために使う）。
    #[allow(dead_code)]
    pub fn hwnd_raw(&self) -> isize {
        self.hwnd.0 as isize
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
        let fg = ds::TEXT_PRIMARY;
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
            let col = if at_live { ds::ACCENT_BRAND } else { fg };
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
                ("💬 非表示".to_string(), ds::ACCENT_BRAND)
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
        // EQ トグル（有効時 or パネル開時はアクセント色）。
        let eq_active = v.eq_open || !v.eq.is_neutral();
        let eq_col = if eq_active { ds::ACCENT_BRAND } else { fg };
        let eq_label_w = unsafe { self.measure("EQ") }.ceil() as i32;
        controls.push(Control::Button {
            rect: row(xr - eq_label_w - 8, xr),
            label: "EQ".to_string(),
            col: eq_col,
            action: OverlayAction::ToggleEq,
        });
        xr -= eq_label_w + 8 + 10;
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
        // mpv の映像子窓より上に居続けて実マウス入力を受ける（視覚は DComp が最前面合成）。
        self.ensure_topmost();
        let (cw, ch) = (self.cw, self.ch);
        let nav_cy = 6 + ROW_H + ROW_H / 2;
        let strip_h = if view.title.is_empty() { TOP_H - ROW_H } else { TOP_H };
        let nav = |x: i32, w: i32| RECT { left: x, top: nav_cy - ROW_H / 2, right: x + w, bottom: nav_cy + ROW_H / 2 };

        // 部品（描画とヒットで同じ素）と各パネルを組み立てる。
        let mut controls: Vec<Control> = Vec::new();
        let mut panel = RECT::default();
        let mut top_panel = RECT::default();
        let mut eq_panel = RECT::default();
        let mut grid: Option<BrowseGrid> = None;
        let mut card_hits: Vec<(RECT, String)> = Vec::new();
        let mut nav_hits: Vec<(RECT, OverlayAction)> = Vec::new();
        // カード内の小領域（アバター/チャンネル名→OpenChannelOf, ケバブ→OpenCardMenu）。
        // カード本体(card_hits=再生)より優先して判定する。
        let mut card_extra_hits: Vec<(RECT, OverlayAction)> = Vec::new();
        let mut menu_hits: Vec<(RECT, OverlayAction)> = Vec::new();

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
        // チャット表示中: フォント用意・インライン絵文字の事前デコード・左端リサイズハンドル。
        let mut chat_resize = RECT::default();
        if chat_panel.right > chat_panel.left {
            self.ensure_chat_format(view.chat_font_px);
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
            chat_resize = RECT { left: chat_panel.left, top: chat_panel.top, right: chat_panel.left + 8, bottom: chat_panel.bottom };
        }

        if view.list_open {
            // 一覧モード: 全面パネル（余白クリックは吸収）。カードクリックは wndproc が PlayIndex 化。
            panel = RECT { left: 0, top: 0, right: cw, bottom: ch };
            let g = browse_grid(cw, ch, view.list_cards.len(), view.list_sel);
            // グリッドに収まる範囲のサムネを（ディスクキャッシュ済みのみ）デコードしておく。
            let end = (g.first + g.cols * g.visible_rows).min(view.list_cards.len());
            if g.first < end {
                let urls: Vec<String> = view.list_cards[g.first..end]
                    .iter()
                    .flat_map(|c| [c.thumb.clone(), c.avatar.clone()])
                    .filter(|u| !u.is_empty())
                    .collect();
                self.ensure_thumbs(&urls);
            }
            grid = Some(g);
            // ✕ 閉じるボタン（右上）。
            let xw = unsafe { self.measure("✕") }.ceil() as i32;
            controls.push(Control::Button {
                rect: RECT { left: cw - 16 - xw - 12, top: 14, right: cw - 16, bottom: 14 + ROW_H },
                label: "✕".to_string(),
                col: ds::TEXT_PRIMARY,
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
                    col: ds::TEXT_PRIMARY,
                    action: OverlayAction::Login,
                });
            }
            // ナビタブ（左フロー）: おすすめは常時、再生リスト/登録/履歴はログイン時のみ。
            let tab_col = ds::TEXT_PRIMARY;
            let mut tx = 12;
            {
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
            // チャット文字サイズ A-/A+（チャットヘッダ右）。
            if chat_panel.right > chat_panel.left {
                let hcy = chat_panel.top + 14;
                let acc = ds::TEXT_PRIMARY;
                let aw = unsafe { self.measure("A+") }.ceil() as i32;
                let ainc = RECT { left: cw - 10 - aw - 8, top: hcy - ROW_H / 2, right: cw - 10, bottom: hcy + ROW_H / 2 };
                controls.push(Control::Button { rect: ainc, label: "A+".to_string(), col: acc, action: OverlayAction::ChatFontInc });
                let ap = unsafe { self.measure("A-") }.ceil() as i32;
                let adec = RECT { left: ainc.left - 6 - ap - 8, top: hcy - ROW_H / 2, right: ainc.left - 6, bottom: hcy + ROW_H / 2 };
                controls.push(Control::Button { rect: adec, label: "A-".to_string(), col: acc, action: OverlayAction::ChatFontDec });
            }
            // EQ パネル（下帯直上・右寄せ）: スライダー3本＋リセット。list_open 中は出さない
            // （呼び出し元の active && !list_open が保証済みだが、ここでは view.eq_open だけ見る）。
            if view.eq_open {
                let eq = view.eq;
                let panel_w = 360;
                let slider_h = ROW_H;
                let reset_h = ROW_H;
                let pad = 12;
                let panel_h = slider_h * 3 + reset_h + pad * 2;
                let right = cw - 14;
                let bottom = ch - BOTTOM_H - 8;
                eq_panel = RECT { left: right - panel_w, top: bottom - panel_h, right, bottom };
                let mut sy = eq_panel.top + pad;
                for band in [EqBand::Voice, EqBand::Low, EqBand::High] {
                    let r = RECT { left: eq_panel.left + pad, top: sy, right: eq_panel.right - pad, bottom: sy + slider_h };
                    controls.push(Control::EqSlider { rect: r, frac: eq_frac(band, eq), band, label: eq_label(band, eq) });
                    sy += slider_h;
                }
                let reset_w = unsafe { self.measure("リセット") }.ceil() as i32 + 16;
                let reset_r = RECT { left: eq_panel.right - pad - reset_w, top: sy, right: eq_panel.right - pad, bottom: sy + reset_h };
                controls.push(Control::Button { rect: reset_r, label: "リセット".to_string(), col: ds::TEXT_PRIMARY, action: OverlayAction::EqReset });
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

            // チャットパネルを先に描く（後でバー/コントロール＝A-/A+ 等が上に乗るように）。
            // テキストは語/文字単位で手動折返し、絵文字はインライン画像（未デコード時は alt）。
            if chat_panel.right > chat_panel.left {
                use windows::Win32::Graphics::Direct2D::D2D1_INTERPOLATION_MODE_LINEAR;
                let ctf = self.chat_tf.clone();
                let cf_ref = ctf.as_ref().unwrap_or(&self.tf_small);
                let pc = Painter { ctx: &ctx, dwrite: &self.dwrite, tf: cf_ref };
                let (px, ptop, pbot) = (chat_panel.left as f32, chat_panel.top as f32, chat_panel.bottom as f32);
                p.fill_rect(rf(px, ptop, cw as f32, pbot), ds::alpha(ds::BG_CANVAS, 0.82));
                // 左端リサイズハンドル（境界線＋中央グリップ）。
                p.fill_rect(rf(px, ptop, px + 2.0, pbot), ds::alpha(ds::ICON_MUTED, 0.8));
                let grip_cy = (ptop + pbot) / 2.0;
                p.fill_round(rf(px + 1.0, grip_cy - 16.0, px + 5.0, grip_cy + 16.0), 2.0, ds::alpha(ds::ICON_DEFAULT, 0.95));
                p.text("コメント", rf(px + 10.0, ptop + 2.0, cw as f32 - 10.0, ptop + 24.0), ds::TEXT_SECONDARY);

                let normal = ds::TEXT_PRIMARY;
                let left = px + 10.0;
                let right_lim = cw as f32 - 10.0;
                let fs = view.chat_font_px.clamp(10.0, 28.0);
                let line_h = (fs * 1.6).max(fs + 8.0);
                let em = fs * 1.5;
                let body_top = ptop + 28.0;
                let avail_h = pbot - body_top - 4.0;
                let n = view.chat_lines.len();
                let end = n.saturating_sub(view.chat_scroll).max(if n > 0 { 1 } else { 0 });
                let cmeasure = |s: &str| self.measure_tf(cf_ref, s);

                // パス1: end から折返し行数を数え、画面に収まる開始 index を決める。
                let mut acc_lines = 0usize;
                let mut start = end;
                for i in (0..end).rev() {
                    let toks = tokenize_line(&view.chat_lines[i], normal);
                    let lc = chat_line_count(&toks, em, left, right_lim, &cmeasure);
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
                        let tw = chat_tok_width(t, em, &cmeasure);
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
                                pc.text_clip(s, rf(cx, ty, right_lim, ty + line_h), *tc);
                            }
                            ChatTok::Emoji { url, alt } => {
                                if let Some(bmp) = self.thumb_cache.get(url) {
                                    let top = ty + (line_h - em) / 2.0;
                                    ctx.DrawBitmap(bmp, Some(&rf(cx, top, cx + em, top + em)), 1.0, D2D1_INTERPOLATION_MODE_LINEAR, None, None);
                                } else {
                                    pc.text_clip(alt, rf(cx, ty, right_lim, ty + line_h), normal);
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

            if view.list_open {
                let g = grid.unwrap_or_else(|| browse_grid(cw, ch, view.list_cards.len(), view.list_sel));
                let cwf = cw as f32;
                let chf = ch as f32;

                // 全面の地（動画上の暗幕）。
                p.fill_rect(rf(0.0, 0.0, cwf, chf), ds::alpha(ds::BG_CANVAS, 0.93));

                // --- タイトルバー（44）: ロゴ＋アプリ名。閉じる✕は controls 側。 ---
                p.fill_rect(rf(0.0, 0.0, cwf, TITLEBAR_H), ds::alpha(ds::BG_SURFACE, 0.92));
                p.fill_rect(rf(0.0, TITLEBAR_H - 1.0, cwf, TITLEBAR_H), ds::BORDER_SUBTLE);
                p.fill_round(rf(12.0, 11.0, 34.0, 33.0), ds::RADIUS_CONTROL_SOFT, ds::ACCENT_BRAND);
                p.text("▶", rf(17.0, 13.0, 34.0, 33.0), ds::TEXT_ON_ACCENT);
                p.text("YouTube Super Lite", rf(44.0, 12.0, 320.0, 34.0), ds::TEXT_PRIMARY);

                // --- サイドバー（224）: 右境界＋ナビ 4 行。 ---
                p.fill_rect(rf(SIDEBAR_W - 1.0, TITLEBAR_H, SIDEBAR_W, chf), ds::BORDER_SUBTLE);
                let navs = [
                    ("📺 登録チャンネル", ListTab::Subs),
                    ("🕘 履歴", ListTab::History),
                    ("📃 再生リスト", ListTab::Playlist),
                    ("📋 おすすめ", ListTab::Recommend),
                ];
                let mut ny = TITLEBAR_H + ds::SPACE_INSET;
                for (label, tab) in navs {
                    let r = RECT {
                        left: 8,
                        top: ny as i32,
                        right: (SIDEBAR_W - 8.0) as i32,
                        bottom: (ny + NAV_ROW_H) as i32,
                    };
                    let active_nav = tab == view.list_tab;
                    if active_nav {
                        p.fill_round(
                            rf(r.left as f32, r.top as f32, r.right as f32, r.bottom as f32),
                            ds::RADIUS_CONTROL_SOFT,
                            ds::alpha(ds::BG_ELEVATED, 0.95),
                        );
                    }
                    let tcol = if active_nav { ds::TEXT_PRIMARY } else { ds::TEXT_SECONDARY };
                    p.text_clip(label, rf(r.left as f32 + 14.0, ny + 9.0, SIDEBAR_W - 12.0, ny + NAV_ROW_H - 6.0), tcol);
                    nav_hits.push((r, OverlayAction::OpenList(tab)));
                    ny += NAV_ROW_H + 4.0;
                }

                // --- コンテンツ: ページ見出し（3xl）。 ---
                let page_title = view.list_header.split('（').next().unwrap_or("").trim();
                p.text_font(&self.tf_title, page_title, rf(g.content_x, g.heading_y, cwf - ds::SPACE_SECTION, g.heading_y + ds::SIZE_3XL + 8.0), ds::TEXT_PRIMARY);

                // --- フィルタチップ列（現状は見た目のみ・非機能）。 ---
                let chips = ["すべて", "ゲーム", "音楽", "ライブ", "ミックス", "視聴済み"];
                let mut chx = g.content_x;
                let chip_h = 32.0;
                for (ci, label) in chips.iter().enumerate() {
                    let tw = self.measure(label);
                    let w = tw + ds::SPACE_INSET * 2.0;
                    if chx + w > cwf - ds::SPACE_SECTION {
                        break;
                    }
                    let selected = ci == 0;
                    let (bg, fgc) = if selected {
                        (ds::BG_INVERSE, ds::TEXT_INVERSE)
                    } else {
                        (ds::alpha(ds::BG_ELEVATED, 0.95), ds::TEXT_PRIMARY)
                    };
                    p.fill_round(rf(chx, g.chips_y, chx + w, g.chips_y + chip_h), ds::RADIUS_CONTROL_SOFT, bg);
                    p.text_clip(label, rf(chx + ds::SPACE_INSET, g.chips_y + 6.0, chx + w, g.chips_y + chip_h - 4.0), fgc);
                    chx += w + ds::GAP_TIGHT;
                }

                // --- 動画グリッド。 ---
                let gap = ds::GAP_LOOSE;
                let first_row = g.first / g.cols.max(1);
                let end = (g.first + g.cols * g.visible_rows).min(view.list_cards.len());
                // 開いているカードメニューのケバブ位置（(右端, 情報列top)）。可視範囲内の時だけ Some。
                let mut menu_anchor: Option<(f32, f32)> = None;
                for i in g.first..end {
                    let card = &view.list_cards[i];
                    let col = i % g.cols;
                    let row = i / g.cols;
                    let cx = g.content_x + col as f32 * (g.card_w + gap);
                    let cy = g.grid_y0 + (row - first_row) as f32 * g.row_pitch;
                    if cy + g.card_h > chf {
                        break;
                    }
                    let cright = cx + g.card_w;
                    let tb = cy + g.thumb_h; // サムネ下端

                    // 選択カードの下地（キーボード選択のフォーカス）。
                    if i == view.list_sel {
                        p.fill_round(
                            rf(cx - 8.0, cy - 8.0, cright + 8.0, cy + g.card_h + 8.0),
                            ds::RADIUS_CONTAINER,
                            ds::alpha(ds::BG_SELECTED, 0.6),
                        );
                    }
                    // サムネ（プレースホルダ地＋デコード済みビットマップ。どちらも角丸クリップ）。
                    p.fill_round(rf(cx, cy, cright, tb), ds::RADIUS_CONTAINER, ds::alpha(ds::BG_SURFACE, 0.9));
                    if !card.thumb.is_empty() {
                        if let Some(bmp) = self.thumb_cache.get(&card.thumb) {
                            p.fill_round_bitmap(bmp, rf(cx, cy, cright, tb), ds::RADIUS_CONTAINER);
                        }
                    }
                    // 時間バッジ / LIVE バッジ（データがあれば）。
                    if card.live {
                        let bw = self.measure("● ライブ") + 10.0;
                        p.fill_round(rf(cx + 8.0, tb - 26.0, cx + 8.0 + bw, tb - 6.0), ds::RADIUS_OVERLAY, ds::ACCENT_LIVE);
                        p.text_clip("● ライブ", rf(cx + 13.0, tb - 25.0, cx + 8.0 + bw, tb - 6.0), ds::TEXT_ON_ACCENT);
                    } else if let Some(d) = card.duration {
                        let t = fmt_time(d);
                        let bw = self.measure(&t) + 10.0;
                        p.fill_round(rf(cright - 8.0 - bw, tb - 26.0, cright - 8.0, tb - 6.0), ds::RADIUS_OVERLAY, ds::BG_SCRIM);
                        p.text_clip(&t, rf(cright - 3.0 - bw, tb - 25.0, cright - 8.0, tb - 6.0), ds::TEXT_ON_ACCENT);
                    }

                    // アバター（36px円。配信中は赤リング）＋ 情報列（タイトル2行/チャンネル/メタ）＋ケバブ。
                    let info_y = tb + ds::GAP_TIGHT;
                    let avatar_size = ds::SIZE_AVATAR_CHANNEL;
                    let (avatar_cx, avatar_cy) = (cx + avatar_size / 2.0, info_y + avatar_size / 2.0);
                    if card.live {
                        p.fill_ellipse(avatar_cx, avatar_cy, avatar_size / 2.0 + 2.0, ds::ACCENT_BRAND);
                    }
                    p.fill_ellipse(avatar_cx, avatar_cy, avatar_size / 2.0, ds::alpha(ds::BG_SURFACE, 0.9));
                    if !card.avatar.is_empty() {
                        if let Some(bmp) = self.thumb_cache.get(&card.avatar) {
                            p.fill_round_bitmap(bmp, rf(cx, info_y, cx + avatar_size, info_y + avatar_size), avatar_size / 2.0);
                        }
                    }
                    let text_x = cx + avatar_size + ds::GAP_TIGHT;
                    let kebab_w = 16.0;
                    p.text_clip("⋮", rf(cright - kebab_w, info_y, cright, info_y + 20.0), ds::ICON_MUTED);
                    let title_col = if i == view.list_sel { ds::TEXT_PRIMARY } else { ds::TEXT_PRIMARY };
                    // タイトル: 2 行ぶんの高さで自動折返し＋クリップ。
                    p.text_clip(&card.title, rf(text_x, info_y, cright - kebab_w - 4.0, info_y + 38.0), title_col);
                    if !card.channel.is_empty() {
                        let mut chline = card.channel.clone();
                        if card.verified {
                            chline.push_str(" ✔");
                        }
                        p.text_clip(&chline, rf(text_x, info_y + 40.0, cright, info_y + 58.0), ds::TEXT_SECONDARY);
                    }
                    if let Some(meta) = &card.meta {
                        p.text_clip(meta, rf(text_x, info_y + 58.0, cright, info_y + 76.0), ds::TEXT_SECONDARY);
                    }

                    // カード内の小領域を、カード本体(=再生)より優先で判定する。
                    // アバター＋チャンネル行 → チャンネルを開く。
                    card_extra_hits.push((
                        RECT { left: cx as i32, top: info_y as i32, right: (cright - kebab_w - 4.0) as i32, bottom: (info_y + avatar_size) as i32 },
                        OverlayAction::OpenChannel { id: card.menu.channel_id.clone(), name: card.channel.clone() },
                    ));
                    // ケバブ(⋮) → コンテキストメニュー。
                    card_extra_hits.push((
                        RECT { left: (cright - kebab_w - 6.0) as i32, top: info_y as i32, right: cright as i32, bottom: (info_y + 24.0) as i32 },
                        OverlayAction::OpenCardMenu(i),
                    ));
                    if view.card_menu_open == Some(i) {
                        menu_anchor = Some((cright, info_y));
                    }

                    card_hits.push((
                        RECT { left: cx as i32, top: cy as i32, right: cright as i32, bottom: (cy + g.card_h) as i32 },
                        card.id.clone(),
                    ));
                }

                if view.list_cards.is_empty() {
                    // 空一覧の理由を区別して表示する（取得中 / 未ログイン / 本当に空）。
                    let msg = if view.list_busy {
                        "取得中…"
                    } else if !view.logged_in {
                        "ログインが必要です"
                    } else {
                        "表示できる動画がありません"
                    };
                    p.text(msg, rf(g.content_x, g.grid_y0, cwf - ds::SPACE_SECTION, g.grid_y0 + 40.0), ds::TEXT_SECONDARY);
                }

                // --- カードのケバブメニュー（開いている時のみ。他の全てより上に描く）。 ---
                if let (Some(mi), Some((anchor_right, anchor_top))) = (view.card_menu_open, menu_anchor) {
                    if let Some(card) = view.list_cards.get(mi) {
                        let menu_w = 268.0;
                        let item_h = 40.0;
                        let mut items: Vec<(&str, OverlayAction)> = vec![
                            ("後で見るに保存", OverlayAction::SaveWatchLater(card.id.clone())),
                        ];
                        if let Some(token) = card.menu.not_interested_token.clone() {
                            items.push(("興味なし", OverlayAction::NotInterested(token)));
                        }
                        if let Some(token) = card.menu.not_channel_token.clone() {
                            items.push(("チャンネルをおすすめに表示しない", OverlayAction::NotRecommendChannel(token)));
                        }
                        if card.menu.channel_id.is_some() || !card.channel.is_empty() {
                            items.push(("チャンネルへ", OverlayAction::OpenChannel { id: card.menu.channel_id.clone(), name: card.channel.clone() }));
                        }
                        let menu_h = items.len() as f32 * item_h + ds::SPACE_INSET;
                        let mx = (anchor_right - menu_w).clamp(ds::GAP_TIGHT, cwf - menu_w - ds::GAP_TIGHT);
                        let my = anchor_top.clamp(ds::GAP_TIGHT, chf - menu_h - ds::GAP_TIGHT);
                        p.fill_round(rf(mx, my, mx + menu_w, my + menu_h), ds::RADIUS_CONTAINER, ds::alpha(ds::BG_ELEVATED, 0.98));
                        for (idx, (label, action)) in items.iter().enumerate() {
                            let iy = my + ds::SPACE_INSET / 2.0 + idx as f32 * item_h;
                            p.text_clip(label, rf(mx + ds::SPACE_INSET, iy + 11.0, mx + menu_w - ds::SPACE_INSET, iy + item_h - 8.0), ds::TEXT_PRIMARY);
                            menu_hits.push((
                                RECT { left: mx as i32, top: iy as i32, right: (mx + menu_w) as i32, bottom: (iy + item_h) as i32 },
                                action.clone(),
                            ));
                        }
                    }
                }

                // ✕ 閉じるボタン等（一覧モードの部品）。
                for c in &controls {
                    c.draw(&p);
                }
            } else if active {
                // 下部コントローラ帯の背景。
                p.fill_rect(rf(panel.left as f32, panel.top as f32, panel.right as f32, panel.bottom as f32), ds::alpha(ds::BG_CANVAS, 0.72));
                // 上部バーの背景。
                p.fill_rect(rf(0.0, 0.0, cw as f32, strip_h as f32), ds::alpha(ds::BG_CANVAS, 0.55));
                // URL 行（先頭）。空なら入力ガイドをグレーで。
                let (url_txt, url_col) = if view.url_input.is_empty() {
                    (
                        "URL: YouTube の URL を入力して Enter（英数字キー / Ctrl+V 貼付 / Esc クリア）".to_string(),
                        ds::TEXT_SECONDARY,
                    )
                } else {
                    (format!("URL: {}", view.url_input), ds::TEXT_PRIMARY)
                };
                p.text(&url_txt, rf(12.0, 6.0, cw as f32 - 12.0, (6 + ROW_H) as f32), url_col);
                // 認証ラベル（ログイン済みは右寄せテキスト。未ログインは Button 追加済み）。
                if view.logged_in {
                    let lw = self.measure(&view.auth_label);
                    p.text(
                        &view.auth_label,
                        rf(cw as f32 - 12.0 - lw, (nav_cy - 9) as f32, cw as f32 - 12.0, (nav_cy + 9) as f32),
                        ds::TEXT_SECONDARY,
                    );
                }
                // タイトル行（あれば）。
                if !view.title.is_empty() {
                    p.text(&view.title, rf(12.0, (6 + ROW_H * 2) as f32, cw as f32 - 12.0, strip_h as f32), ds::TEXT_PRIMARY);
                }
                // EQ パネルの背景（下帯と同じ地。部品より先に描いてスライダーを上に乗せる）。
                if eq_panel.right > eq_panel.left {
                    p.fill_rect(
                        rf(eq_panel.left as f32, eq_panel.top as f32, eq_panel.right as f32, eq_panel.bottom as f32),
                        ds::alpha(ds::BG_CANVAS, 0.72),
                    );
                }
                // 各部品（下部コントロール＋ナビタブ＋ログイン＋A-/A+・EQ パネル。チャットの上に乗る）。
                for c in &controls {
                    c.draw(&p);
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
        self.state.card_hits = card_hits;
        self.state.card_extra_hits = card_extra_hits;
        self.state.menu_open = view.card_menu_open.is_some();
        self.state.menu_hits = menu_hits;
        self.state.nav_hits = nav_hits;
        self.state.grid_cols = grid.map(|g| g.cols).unwrap_or(1);
        self.state.chat_panel = chat_panel;
        self.state.chat_resize = chat_resize;
        self.state.eq_panel = eq_panel;
    }

    /// 表示中サムネ URL のうち未キャッシュのものを、ディスクキャッシュ済みなら WIC デコードして
    /// ビットマップ化（未キャッシュは非同期取得を起動。ネットワーク取得はしない）。
    fn ensure_thumbs(&mut self, urls: &[String]) {
        for url in urls {
            if url.is_empty() || self.thumb_cache.contains_key(url) {
                continue;
            }
            match ysl_core::image_cache::cached_path(url).and_then(|p| p.to_str().map(String::from)) {
                Some(ps) => {
                    if let Some(bmp) = unsafe { self.load_wic(&ps) } {
                        self.thumb_cache.insert(url.clone(), bmp);
                    }
                }
                None => ysl_core::image_cache::ensure_cached_async(url),
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

    /// 小フォントでのテキスト幅（px）。
    unsafe fn measure(&self, s: &str) -> f32 {
        self.measure_tf(&self.tf_small, s)
    }

    /// 指定フォントでのテキスト幅（px）。
    unsafe fn measure_tf(&self, tf: &IDWriteTextFormat, s: &str) -> f32 {
        use windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_METRICS;
        let wt: Vec<u16> = s.encode_utf16().collect();
        if let Ok(layout) = self.dwrite.CreateTextLayout(&wt, tf, 8192.0, 64.0) {
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_ok() {
                return m.widthIncludingTrailingWhitespace;
            }
        }
        s.chars().count() as f32 * 9.0
    }

    /// チャット用フォントを指定 px で用意する（変化時のみ作り直す）。NO_WRAP（折返しは手動）。
    fn ensure_chat_format(&mut self, px: f32) {
        let px = px.clamp(10.0, 28.0);
        if self.chat_tf.is_some() && (self.chat_tf_px - px).abs() < 0.5 {
            return;
        }
        use windows::core::w;
        use windows::Win32::Graphics::DirectWrite::{
            DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_WORD_WRAPPING_NO_WRAP,
        };
        unsafe {
            if let Ok(tf) = self.dwrite.CreateTextFormat(
                w!("Yu Gothic UI"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                px,
                w!("ja-jp"),
            ) {
                let _ = tf.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                self.chat_tf = Some(tf);
                self.chat_tf_px = px;
            }
        }
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
    /// ビットマップを角丸矩形にクリップして描く（`DrawBitmap` は矩形のみのため、
    /// ビットマップブラシ＋`FillRoundedRectangle` でカードサムネを角丸にする）。
    unsafe fn fill_round_bitmap(&self, bmp: &ID2D1Bitmap1, r: D2D_RECT_F, rad: f32) {
        use windows::Foundation::Numerics::Matrix3x2;
        use windows::Win32::Graphics::Direct2D::{
            D2D1_BITMAP_BRUSH_PROPERTIES1, D2D1_EXTEND_MODE_CLAMP,
            D2D1_INTERPOLATION_MODE_LINEAR, D2D1_ROUNDED_RECT,
        };
        let sz = bmp.GetSize();
        if sz.width <= 0.0 || sz.height <= 0.0 {
            return;
        }
        // object-fit: cover 相当。縦横同じ倍率（大きい方）で拡大し、はみ出た分は
        // FillRoundedRectangle の角丸クリップで切り捨てる。サムネの実アスペクト比は
        // 16:9 とは限らない（旧形式の 4:3 レターボックス/任意アスペクトの投稿サムネがある）ため、
        // 縦横別倍率の引き伸ばしは元画像の黒帯ごと潰して見せてしまう。cover なら常に枠を実写で
        // 埋め、黒帯は基本的にクロップアウトされる。
        let dst_w = r.right - r.left;
        let dst_h = r.bottom - r.top;
        let scale = (dst_w / sz.width).max(dst_h / sz.height);
        let scaled_w = sz.width * scale;
        let scaled_h = sz.height * scale;
        let offset_x = r.left + (dst_w - scaled_w) / 2.0;
        let offset_y = r.top + (dst_h - scaled_h) / 2.0;
        let props = D2D1_BITMAP_BRUSH_PROPERTIES1 {
            extendModeX: D2D1_EXTEND_MODE_CLAMP,
            extendModeY: D2D1_EXTEND_MODE_CLAMP,
            interpolationMode: D2D1_INTERPOLATION_MODE_LINEAR,
        };
        if let Ok(brush) = self.ctx.CreateBitmapBrush(bmp, Some(&props as *const _), None) {
            brush.SetTransform(&Matrix3x2 {
                M11: scale,
                M12: 0.0,
                M21: 0.0,
                M22: scale,
                M31: offset_x,
                M32: offset_y,
            });
            self.ctx.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT { rect: r, radiusX: rad, radiusY: rad },
                &brush,
            );
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

    /// 指定フォントで描くテキスト（見出し=tf_title 等、既定 tf 以外を使う場合）。
    unsafe fn text_font(&self, tf: &IDWriteTextFormat, s: &str, r: D2D_RECT_F, c: D2D1_COLOR_F) {
        use windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_NONE;
        use windows::Win32::Graphics::DirectWrite::DWRITE_MEASURING_MODE_NATURAL;
        if let Ok(b) = self.ctx.CreateSolidColorBrush(&c, None) {
            let wt: Vec<u16> = s.encode_utf16().collect();
            self.ctx.DrawText(&wt, tf, &r, &b, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
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

/// ローパス/ハイパスのラダー刻み（オフの1区分込みで段数+1）。
const EQ_LADDER_BUCKETS: usize = ysl_core::types::LOWPASS_STEPS.len() + 1;
const _: () = assert!(ysl_core::types::LOWPASS_STEPS.len() == ysl_core::types::HIGHPASS_STEPS.len());

/// frac(0.0..=1.0) を 0..=buckets-1 の区分 index に量子化する。
fn frac_bucket(frac: f64, buckets: usize) -> usize {
    ((frac * buckets as f64) as usize).min(buckets - 1)
}

/// 区分 index を区分中央の frac に戻す（表示用の逆変換で使う）。
fn bucket_frac(idx: usize, buckets: usize) -> f32 {
    ((idx as f64 + 0.5) / buckets as f64) as f32
}

/// EQ スライダーのドラッグ/クリック位置(frac)を `OverlayAction` に直す純関数。
/// ラダー定数は devtools のステップ操作と同じ離散値（`ysl_core::types` 参照）に量子化する。
fn eq_action_from_frac(band: EqBand, frac: f64) -> OverlayAction {
    use ysl_core::types::{HIGHPASS_STEPS, LOWPASS_STEPS, VOICE_GAIN_MAX_DB};
    match band {
        EqBand::Voice => {
            // クランプは set_eq(EqParams::clamped) に委譲する（値域の定義を二重化しない）。
            let db = (frac * VOICE_GAIN_MAX_DB * 2.0 - VOICE_GAIN_MAX_DB).round();
            OverlayAction::SetEqVoice(db)
        }
        EqBand::Low => {
            let b = frac_bucket(frac, EQ_LADDER_BUCKETS);
            let hz = if b >= LOWPASS_STEPS.len() { None } else { Some(LOWPASS_STEPS[b]) };
            OverlayAction::SetEqLowpass(hz)
        }
        EqBand::High => {
            let b = frac_bucket(frac, EQ_LADDER_BUCKETS);
            // 最左区分(0)＝オフ、区分1..=9 が HIGHPASS_STEPS[0..=8]（左右逆）。
            let hz = if b == 0 { None } else { Some(HIGHPASS_STEPS[b - 1]) };
            OverlayAction::SetEqHighpass(hz)
        }
    }
}

/// 現在の EQ 値からスライダーのつまみ位置(frac)を出す純関数（`eq_action_from_frac` の逆変換）。
fn eq_frac(band: EqBand, eq: ysl_core::types::EqParams) -> f32 {
    use ysl_core::types::{ladder_idx, HIGHPASS_STEPS, LOWPASS_STEPS, VOICE_GAIN_MAX_DB};
    match band {
        EqBand::Voice => ((eq.voice_gain_db + VOICE_GAIN_MAX_DB) / (VOICE_GAIN_MAX_DB * 2.0)) as f32,
        EqBand::Low => match eq.lowpass_hz {
            None => bucket_frac(EQ_LADDER_BUCKETS - 1, EQ_LADDER_BUCKETS),
            Some(hz) => {
                let idx = ladder_idx(&LOWPASS_STEPS, hz);
                bucket_frac(idx, EQ_LADDER_BUCKETS)
            }
        },
        EqBand::High => match eq.highpass_hz {
            None => bucket_frac(0, EQ_LADDER_BUCKETS),
            Some(hz) => {
                let idx = ladder_idx(&HIGHPASS_STEPS, hz);
                bucket_frac(idx + 1, EQ_LADDER_BUCKETS)
            }
        },
    }
}

/// EQ スライダーのラベル文言（有効/オフで書式を変える）。
fn eq_label(band: EqBand, eq: ysl_core::types::EqParams) -> String {
    match band {
        EqBand::Voice if eq.voice_gain_db == 0.0 => "声 0dB".to_string(),
        EqBand::Voice => format!("声 {:+.0}dB", eq.voice_gain_db),
        // ローパス＝高域を削る、ハイパス＝低域を削る。ラベルは「何を削るか」で表記する
        // （フィルタ名のままだと直感と逆に読める）。
        EqBand::Low => match eq.lowpass_hz {
            Some(hz) => format!("高域カット {}", fmt_hz(hz)),
            None => "高域カット オフ".to_string(),
        },
        EqBand::High => match eq.highpass_hz {
            Some(hz) => format!("低域カット {}", fmt_hz(hz)),
            None => "低域カット オフ".to_string(),
        },
    }
}

/// Hz をラベル用に整形する（1000 以上は k 単位。例: 8000 → "8kHz"、100 → "100Hz"）。
fn fmt_hz(hz: f64) -> String {
    if hz >= 1000.0 {
        let k = hz / 1000.0;
        if k.fract() == 0.0 {
            format!("{k:.0}kHz")
        } else {
            format!("{k:.1}kHz")
        }
    } else {
        format!("{hz:.0}Hz")
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
                    Drag::ChatW => {
                        // 左端を x に動かす → 幅比率 = (cw - x) / cw。
                        let ratio = ((s.cw - lo) as f64 / s.cw.max(1) as f64).clamp(0.15, 0.6);
                        s.actions.push(OverlayAction::SetChatWidth(ratio));
                    }
                    Drag::Eq(band) => {
                        s.actions.push(eq_action_from_frac(band, frac_x(&s.drag_rect, lo)));
                    }
                    Drag::None => {}
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let mut capture = false;
            if let Some(s) = state_of(hwnd) {
                if s.list_open && s.menu_open {
                    // カードメニュー表示中はメニュー項目のみ判定（モーダル扱い）。
                    // 当たらなければ閉じてこのクリックを吸収する（下のカード等には落とさない）。
                    let mut handled = false;
                    for (r, a) in &s.menu_hits {
                        if in_rect(r, lo, hi) {
                            s.actions.push(a.clone());
                            handled = true;
                            break;
                        }
                    }
                    if !handled {
                        s.actions.push(OverlayAction::CloseCardMenu);
                    }
                } else if s.list_open {
                    // 一覧モード: 部品（✕ 閉じる等）→ サイドバーナビ → カード の順。余白は吸収。
                    let mut handled = false;
                    for c in &s.controls {
                        if let Some(Hit::Act(a)) = c.press(lo, hi) {
                            s.actions.push(a);
                            handled = true;
                            break;
                        }
                    }
                    if !handled {
                        for (r, a) in &s.nav_hits {
                            if in_rect(r, lo, hi) {
                                s.actions.push(a.clone());
                                handled = true;
                                break;
                            }
                        }
                    }
                    // カード内の小領域（アバター/チャンネル・ケバブ）を本体より優先。
                    if !handled {
                        for (r, a) in &s.card_extra_hits {
                            if in_rect(r, lo, hi) {
                                s.actions.push(a.clone());
                                handled = true;
                                break;
                            }
                        }
                    }
                    if !handled {
                        for (r, video_id) in &s.card_hits {
                            if in_rect(r, lo, hi) {
                                s.actions.push(OverlayAction::Play { video_id: video_id.clone() });
                                break;
                            }
                        }
                    }
                } else if s.chat_resize.right > s.chat_resize.left && in_rect(&s.chat_resize, lo, hi) {
                    // チャット欄の左端ハンドル → 幅ドラッグ開始。
                    s.drag = Drag::ChatW;
                    capture = true;
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
                    if !handled
                        && !in_rect(&s.panel, lo, hi)
                        && !in_rect(&s.top_panel, lo, hi)
                        && !in_rect(&s.eq_panel, lo, hi)
                    {
                        // どの部品にも当たらず上下バー・EQ パネルの外＝動画域 → pause。
                        // バー/パネル余白は吸収（無反応）。
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
                    // 一覧表示中はホイールで選択を 1 行（＝列数ぶん）上下。上=過去方向(-)。
                    let step = s.grid_cols.max(1) as i32;
                    s.actions.push(OverlayAction::ListScroll(if delta > 0 { -step } else { step }));
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
