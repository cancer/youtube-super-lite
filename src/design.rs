//! DESIGN.md のデザイントークンを Direct2D 向けに表現するトークン層。
//!
//! UI 実装（[`crate::dcomp_overlay`]）は **このモジュールのみ** を参照し、生の hex 値や
//! その場の rgb 即値を埋め込まない（DESIGN.md §6「実装はセマンティック層のみ参照」）。
//! プリミティブ（`--p-*`）はモジュール内 [`p`] に閉じ込め、外部にはセマンティック（`--s-*`）
//! 相当の名前だけを公開する。新色が要るときは まず [`p`] に 1 件足し、それを指すセマンティックを
//! 定義する、の順で拡張する。
//!
//! 注意（オーバーレイの半透明）: コントローラ/一覧/チャットは動画の上に重ねる **半透明**
//! Direct2D オーバーレイなので、面（surface 系）は不透明なトークン色に [`alpha`] で透明度を
//! 与えて使う。文字・アクセント・ボーダーは原則そのまま（不透明）。
#![cfg(windows)]
// 一覧グリッド未移植などで一部トークンはまだ未使用（段階的に反映する）。
#![allow(dead_code)]

use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

/// 8bit sRGB を D2D の 0..1 ストレート色（不透明）へ変換する。
const fn rgb(r: u8, g: u8, b: u8) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

/// 既存トークン色の不透明度だけ差し替えた色を返す（半透明オーバーレイ面用）。
pub const fn alpha(c: D2D1_COLOR_F, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: c.r,
        g: c.g,
        b: c.b,
        a,
    }
}

/// プリミティブトークン（`--p-*`）。意味を持たない生値。外部からは直接使わない。
pub mod p {
    use super::rgb;
    use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

    // ニュートラル・ランプ
    pub const BLACK: D2D1_COLOR_F = rgb(0x00, 0x00, 0x00);
    pub const GRAY_900: D2D1_COLOR_F = rgb(0x0F, 0x0F, 0x0F); // ページ地
    pub const GRAY_850: D2D1_COLOR_F = rgb(0x18, 0x18, 0x18);
    pub const GRAY_800: D2D1_COLOR_F = rgb(0x21, 0x21, 0x21); // カード/面
    pub const GRAY_750: D2D1_COLOR_F = rgb(0x27, 0x27, 0x27); // ホバー面/チップ地
    pub const GRAY_700: D2D1_COLOR_F = rgb(0x3F, 0x3F, 0x3F); // 罫線/選択
    pub const GRAY_600: D2D1_COLOR_F = rgb(0x60, 0x60, 0x60); // 無効テキスト
    pub const GRAY_500: D2D1_COLOR_F = rgb(0x71, 0x71, 0x71); // 弱いアイコン
    pub const GRAY_400: D2D1_COLOR_F = rgb(0xAA, 0xAA, 0xAA); // 副次テキスト
    pub const GRAY_200: D2D1_COLOR_F = rgb(0xCC, 0xCC, 0xCC); // 明アイコン
    pub const GRAY_100: D2D1_COLOR_F = rgb(0xF1, 0xF1, 0xF1); // 主テキスト
    pub const WHITE: D2D1_COLOR_F = rgb(0xFF, 0xFF, 0xFF);

    // 赤系（唯一の有彩色）
    pub const RED_600: D2D1_COLOR_F = rgb(0xCC, 0x00, 0x00); // LIVE 基準
    pub const RED_500: D2D1_COLOR_F = rgb(0xFF, 0x00, 0x00); // ブランド赤/通知
}

// ── セマンティックトークン（--s-*）。UI 実装はこちらを参照する。 ──

// サーフェス / 背景（オーバーレイでは alpha() で半透明にして使う）
pub const BG_CANVAS: D2D1_COLOR_F = p::GRAY_900;
pub const BG_SURFACE: D2D1_COLOR_F = p::GRAY_800;
pub const BG_ELEVATED: D2D1_COLOR_F = p::GRAY_750;
pub const BG_HOVER: D2D1_COLOR_F = p::GRAY_750;
pub const BG_SELECTED: D2D1_COLOR_F = p::GRAY_700;
pub const BG_INVERSE: D2D1_COLOR_F = p::GRAY_100;
/// サムネ上バッジの下地（不透明度込みのスクリム）。
pub const BG_SCRIM: D2D1_COLOR_F = alpha(p::BLACK, 0.80);

// テキスト
pub const TEXT_PRIMARY: D2D1_COLOR_F = p::GRAY_100;
pub const TEXT_SECONDARY: D2D1_COLOR_F = p::GRAY_400;
pub const TEXT_DISABLED: D2D1_COLOR_F = p::GRAY_600;
pub const TEXT_ON_ACCENT: D2D1_COLOR_F = p::WHITE;
pub const TEXT_INVERSE: D2D1_COLOR_F = p::GRAY_900;

// 罫線
pub const BORDER_SUBTLE: D2D1_COLOR_F = p::GRAY_700;

// アクセント / 状態（赤のみ）
pub const ACCENT_LIVE: D2D1_COLOR_F = p::RED_600;
pub const ACCENT_BRAND: D2D1_COLOR_F = p::RED_500;
pub const INDICATOR_NOTIFY: D2D1_COLOR_F = p::RED_500;

// アイコン
pub const ICON_DEFAULT: D2D1_COLOR_F = p::GRAY_200;
pub const ICON_MUTED: D2D1_COLOR_F = p::GRAY_500;
pub const ICON_VERIFIED: D2D1_COLOR_F = p::GRAY_400;

// ── 角丸（形の役割で束ねる、px）──
pub const RADIUS_OVERLAY: f32 = 4.0; // 時間/LIVE/件数バッジ
pub const RADIUS_CONTROL_SOFT: f32 = 8.0; // チップ/ナビ行ホバー
pub const RADIUS_CONTAINER: f32 = 12.0; // カード/サムネ/パネル
pub const RADIUS_PILL: f32 = 9999.0; // ピル操作（実用上は高さの半分で頭打ち）

// ── スペーシング（空間の関係で束ねる、px）──
pub const SPACE_INSET: f32 = 12.0; // 部品内側の左右余白
pub const SPACE_INSET_PILL: f32 = 20.0; // ピル操作の左右
pub const GAP_TIGHT: f32 = 12.0; // 関連要素の間
pub const GAP_LOOSE: f32 = 16.0; // 独立要素の間
pub const SPACE_SECTION: f32 = 24.0; // ページ/セクション外周
pub const OVERLAY_OFFSET: f32 = 8.0; // サムネ上バッジの逃げ

// ── タイプスケール（px。役割別の使い分けは dcomp_overlay 側で）──
pub const SIZE_3XL: f32 = 36.0; // page-title
pub const SIZE_2XL: f32 = 24.0;
pub const SIZE_XL: f32 = 20.0; // section
pub const SIZE_LG: f32 = 16.0;
pub const SIZE_MD: f32 = 14.0; // card-title / body
pub const SIZE_SM: f32 = 12.0; // meta / badge

// ── サイズ（役割別、px）──
pub const SIZE_AVATAR_CHANNEL: f32 = 36.0;
pub const SIZE_AVATAR_NAV: f32 = 24.0;
pub const SIZE_ICON: f32 = 24.0;
pub const SIZE_INDICATOR: f32 = 8.0;
pub const SIZE_NAV_ROW: f32 = 40.0;
