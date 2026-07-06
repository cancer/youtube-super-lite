//! ユーザー設定の永続化（UI 状態と再生の好み）。
//!
//! 認証トークンと同じ設定ディレクトリ（`%APPDATA%\YouTubeSuperLite` 等）に
//! `settings.json` として保存し、次回起動時に引き継ぐ。

use std::path::PathBuf;

/// 引き継ぐ UI 設定。
#[derive(Clone, Copy, PartialEq)]
pub struct Settings {
    pub chat_font_px: f32,
    pub chat_width_ratio: f32,
    pub eq: ysl_core::types::EqParams,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            chat_font_px: 16.0,
            chat_width_ratio: 0.28,
            eq: ysl_core::types::EqParams::default(),
        }
    }
}

fn store_path() -> PathBuf {
    ysl_core::yt::auth::config_dir().join("settings.json")
}

/// 設定を読み込む（無ければ既定値）。値は妥当な範囲にクランプする。
pub fn load() -> Settings {
    let mut s = Settings::default();
    if let Ok(data) = std::fs::read_to_string(store_path()) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
            if let Some(f) = v["chat_font_px"].as_f64() {
                s.chat_font_px = (f as f32).clamp(10.0, 28.0);
            }
            if let Some(w) = v["chat_width_ratio"].as_f64() {
                s.chat_width_ratio = (w as f32).clamp(0.15, 0.6);
            }
            let mut eq = s.eq;
            if let Some(g) = v["eq_voice_gain_db"].as_f64() {
                eq.voice_gain_db = g;
            }
            eq.lowpass_hz = v["eq_lowpass_hz"].as_f64(); // 欠落/null → None（オフ）
            eq.highpass_hz = v["eq_highpass_hz"].as_f64();
            s.eq = eq.clamped();
        }
    }
    s
}

/// 設定を保存する（ディレクトリが無ければ作成）。
pub fn save(s: Settings) {
    let path = store_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let json = serde_json::json!({
        "chat_font_px": s.chat_font_px,
        "chat_width_ratio": s.chat_width_ratio,
        "eq_voice_gain_db": s.eq.voice_gain_db,
        "eq_lowpass_hz": s.eq.lowpass_hz,
        "eq_highpass_hz": s.eq.highpass_hz,
    });
    let _ = std::fs::write(&path, json.to_string());
}
