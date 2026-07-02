//! おすすめ（YouTube ホームフィード）の取得。
//!
//! YouTube トップに出る「おすすめ」は、動画再生とは無関係なホームフィード。InnerTube の
//! `browseId: FEwhat_to_watch` を **TVHTML5 client + OAuth Bearer** で叩いて取得する
//! （subscriptions=FEsubscriptions / history=FEhistory と同型）。無認証では中身が返らず
//! ログイン誘導だけになるため、ログイン必須。レスポンスは TV レイアウトの `tileRenderer`
//! （subs/history と同じ構造）なので、tile 用の共通ヘルパを流用する。
//!
//! 注意: TV tile にはチャンネルアバターが含まれない（サムネのみ）ため `avatar` は空になる。

use anyhow::{anyhow, Result};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::mpsc::Sender;
use std::time::Duration;

/// おすすめ動画 1 件。
#[derive(Clone, Debug, Default)]
pub struct VideoItem {
    pub video_id: String,
    pub title: String,
    pub channel: String,
    pub thumbnail: String,
    /// 再生時間（秒）。ライブ中は None。
    pub duration: Option<f64>,
    pub live: bool,
    /// 視聴回数＋投稿時期（例 "4907万回視聴 • 4 日前"）。
    pub meta: Option<String>,
    pub verified: bool,
    /// ケバブメニュー用データ（実チャンネルID／興味なし・非表示の feedbackToken）。
    /// 認証済みホームフィードの tile にのみ実在。無ければ既定値（全 None）。
    pub menu: crate::subscriptions::CardMenu,
}

/// 背景スレッドからメインスレッドへの通知。
pub enum RecommendUpdate {
    Items(Vec<VideoItem>),
    Error(String),
}

/// ホームフィード（おすすめ）を背景スレッドで取得する。要 OAuth access_token。
pub fn fetch_home_feed(access_token: &str, tx: &Sender<RecommendUpdate>) {
    match fetch_inner(access_token) {
        Ok(items) => {
            let _ = tx.send(RecommendUpdate::Items(items));
        }
        Err(e) => {
            let _ = tx.send(RecommendUpdate::Error(e.to_string()));
        }
    }
}

/// 指定チャンネル(UC...)の動画一覧を取得する（TVHTML5, 無認証で可）。tile を再帰収集。
pub fn fetch_channel_videos(channel_id: &str) -> Result<Vec<VideoItem>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;
    let body = serde_json::json!({
        "context": { "client": {
            "clientName": "TVHTML5", "clientVersion": "7.20260114.12.00", "hl": "ja", "gl": "JP"
        }},
        "browseId": channel_id
    });
    let resp = client
        .post("https://www.youtube.com/youtubei/v1/browse")
        .header("X-YouTube-Client-Name", "7")
        .header("X-YouTube-Client-Version", "7.20260114.12.00")
        .json(&body)
        .send()?
        .error_for_status()?;
    let v: Value = resp.json()?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    collect_tiles(&v, &mut seen, &mut out);
    Ok(out)
}

fn fetch_inner(access_token: &str) -> Result<Vec<VideoItem>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;

    let body = serde_json::json!({
        "context": {
            "client": {
                "clientName": "TVHTML5",
                "clientVersion": "7.20260114.12.00",
                "hl": "ja",
                "gl": "JP"
            }
        },
        "browseId": "FEwhat_to_watch"
    });
    let resp = client
        .post("https://www.youtube.com/youtubei/v1/browse")
        .bearer_auth(access_token)
        .header("X-YouTube-Client-Name", "7")
        .header("X-YouTube-Client-Version", "7.20260114.12.00")
        .json(&body)
        .send()?
        .error_for_status()?;
    let v: Value = resp.json()?;
    parse_home(&v)
}

// ---------------------------------------------------------------------------
// パース
// ---------------------------------------------------------------------------

/// ホームフィードは shelf / grid など複数の器に tileRenderer が散らばるため、レスポンス全体を
/// 再帰的に走査して動画タイルを集める（コンテナ構造の差異に強い）。video_id で dedup。
fn parse_home(v: &Value) -> Result<Vec<VideoItem>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    collect_tiles(v, &mut seen, &mut out);
    if out.is_empty() {
        return Err(anyhow!("おすすめ動画が見つかりません（ログインが必要な場合があります）"));
    }
    Ok(out)
}

fn collect_tiles(v: &Value, seen: &mut HashSet<String>, out: &mut Vec<VideoItem>) {
    match v {
        Value::Object(map) => {
            if let Some(tile) = map.get("tileRenderer") {
                if let Some(vi) = parse_tile(tile) {
                    if seen.insert(vi.video_id.clone()) {
                        out.push(vi);
                    }
                }
            }
            for (_, child) in map {
                collect_tiles(child, seen, out);
            }
        }
        Value::Array(arr) => {
            for child in arr {
                collect_tiles(child, seen, out);
            }
        }
        _ => {}
    }
}

fn parse_tile(tile: &Value) -> Option<VideoItem> {
    // 動画タイルのみ（contentType が動画、または contentId が 11 桁の video_id）。
    let video_id = tile.get("contentId").and_then(|v| v.as_str())?.to_string();
    if video_id.len() != 11 {
        return None;
    }
    let title = tile
        .pointer("/metadata/tileMetadataRenderer/title/simpleText")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if title.is_empty() {
        return None;
    }
    let channel = crate::subscriptions::tile_line(tile, 0);
    let thumbnail =
        crate::subscriptions::pick_largest_thumbnail(tile.pointer("/header/tileHeaderRenderer/thumbnail"));
    let (duration, live) = crate::subscriptions::tile_duration_live(tile);
    let meta = crate::subscriptions::tile_meta(tile);
    let menu = crate::subscriptions::tile_menu(tile);

    Some(VideoItem {
        video_id,
        title,
        channel,
        thumbnail,
        duration,
        live,
        meta,
        verified: false,
        menu,
    })
}
