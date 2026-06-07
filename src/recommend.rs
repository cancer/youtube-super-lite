//! おすすめ動画の取得（InnerTube / ytInitialData）。
//!
//! ウォッチページの ytInitialData に含まれる secondaryResults（右カラムの関連動画）を
//! パースして返す。OAuth 不要・クォータ制限なし。

use anyhow::{anyhow, Result};
use serde_json::Value;
use std::sync::mpsc::Sender;

/// おすすめ動画 1 件。
#[derive(Clone, Debug)]
pub struct VideoItem {
    pub video_id: String,
    pub title: String,
    pub channel: String,
    /// レスポンスが返すサムネ URL（最大サイズ、16:9クロップ済み）。空なら video_id から組み立て。
    pub thumbnail: String,
}

/// 背景スレッドからメインスレッドへの通知。
pub enum RecommendUpdate {
    Items(Vec<VideoItem>),
    Error(String),
}

/// おすすめ動画を背景スレッドで取得する。
pub fn fetch_recommendations(video_id: &str, tx: &Sender<RecommendUpdate>) {
    match fetch_inner(video_id) {
        Ok(items) => {
            let _ = tx.send(RecommendUpdate::Items(items));
        }
        Err(e) => {
            let _ = tx.send(RecommendUpdate::Error(e.to_string()));
        }
    }
}

fn fetch_inner(video_id: &str) -> Result<Vec<VideoItem>> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .build()?;

    let html = client
        .get(format!("https://www.youtube.com/watch?v={video_id}"))
        .send()?
        .text()?;

    let data = extract_json_var(&html, "ytInitialData")?;
    parse_recommendations(&data)
}

// ---------------------------------------------------------------------------
// ytInitialData パース
// ---------------------------------------------------------------------------

fn parse_recommendations(data: &Value) -> Result<Vec<VideoItem>> {
    let results = &data["contents"]["twoColumnWatchNextResults"]["secondaryResults"]
        ["secondaryResults"]["results"];

    let arr = results
        .as_array()
        .ok_or_else(|| anyhow!("おすすめ動画が見つかりません"))?;

    let mut items = Vec::new();
    for item in arr {
        if let Some(renderer) = item.get("compactVideoRenderer") {
            if let Some(vi) = parse_video_item(renderer) {
                items.push(vi);
            }
        }
    }
    Ok(items)
}

fn parse_video_item(renderer: &Value) -> Option<VideoItem> {
    let video_id = renderer["videoId"].as_str()?.to_string();
    let title = extract_text(&renderer["title"])?;
    let channel = extract_text(&renderer["shortBylineText"]).unwrap_or_default();
    let thumbnail = crate::subscriptions::pick_largest_thumbnail(renderer.get("thumbnail"));

    Some(VideoItem {
        video_id,
        title,
        channel,
        thumbnail,
    })
}

/// simpleText または runs[] からテキストを取り出す。
fn extract_text(v: &Value) -> Option<String> {
    if let Some(s) = v["simpleText"].as_str() {
        return Some(s.to_string());
    }
    if let Some(runs) = v["runs"].as_array() {
        let text: String = runs.iter().filter_map(|r| r["text"].as_str()).collect();
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// HTML から ytInitialData を抽出（chat.rs と同じロジック）
// ---------------------------------------------------------------------------

fn extract_json_var(html: &str, var_name: &str) -> Result<Value> {
    let marker = format!("var {var_name} = ");
    let start = html
        .find(&marker)
        .ok_or_else(|| anyhow!("{var_name} が見つかりません"))?;
    let rest = &html[start + marker.len()..];
    let end = find_json_end(rest)?;
    serde_json::from_str(&rest[..end])
        .map_err(|e| anyhow!("{var_name} の JSON 解析に失敗: {e}"))
}

fn find_json_end(s: &str) -> Result<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (i, ch) in s.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        match ch {
            '\\' if in_string => escape = true,
            '"' => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i + 1);
                }
            }
            _ => {}
        }
    }
    anyhow::bail!("JSON の終端が見つかりません")
}
