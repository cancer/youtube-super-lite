//! YouTube アカウントの再生履歴 (`/feed/history` 相当) を取得する。
//!
//! Data API v3 には履歴を読むエンドポイントがないので、InnerTube `/youtubei/v1/browse`
//! を `browseId: "FEhistory"` で叩く。OAuth Bearer を受理するクライアントが必要で、
//! WEB クライアントは 400 を返すため **TVHTML5 クライアント**を使う（mark_watched と同じ理由）。
//!
//! レスポンスは TV 専用レイアウトの `tvBrowseRenderer` で返ってくる:
//! contents
//!  └ tvBrowseRenderer.content.tvSurfaceContentRenderer.content.gridRenderer.items[]
//!     └ tileRenderer
//!        - contentId          = 動画 ID
//!        - header.tileHeaderRenderer.thumbnail.thumbnails[].url
//!        - header.tileHeaderRenderer.thumbnailOverlays[].thumbnailOverlayTimeStatusRenderer
//!            .text.simpleText = 動画長 (例 "25:50")
//!        - header.tileHeaderRenderer.thumbnailOverlays[].thumbnailOverlayResumePlaybackRenderer
//!            .percentDurationWatched = 視聴済%
//!        - metadata.tileMetadataRenderer.title.simpleText
//!        - metadata.tileMetadataRenderer.lines[0].lineRenderer.items[0]
//!            .lineItemRenderer.text.runs[0].text = チャンネル名

use anyhow::{anyhow, Result};
use serde_json::Value;
use std::sync::mpsc::Sender;
use std::time::Duration;

/// 履歴 1 件。recommend / subscription と揃えて owned String のみ。
#[derive(Clone, Debug)]
pub struct HistoryItem {
    pub video_id: String,
    pub title: String,
    pub channel: String,
    /// "mm:ss" など。サムネ右下バッジ用。
    pub duration: String,
    /// 視聴回数の表示（例 "4.6万回視聴"）。空文字なら非表示。
    pub view_count: String,
}

pub enum HistoryUpdate {
    Items(Vec<HistoryItem>),
    Error(String),
}

pub fn fetch_history(access_token: &str, tx: &Sender<HistoryUpdate>) {
    match fetch_inner(access_token) {
        Ok(items) => {
            let _ = tx.send(HistoryUpdate::Items(items));
        }
        Err(e) => {
            let _ = tx.send(HistoryUpdate::Error(e.to_string()));
        }
    }
}

fn fetch_inner(access_token: &str) -> Result<Vec<HistoryItem>> {
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
        "browseId": "FEhistory"
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

    let items_arr = v
        .pointer("/contents/tvBrowseRenderer/content/tvSurfaceContentRenderer/content/gridRenderer/items")
        .and_then(|x| x.as_array())
        .ok_or_else(|| anyhow!("履歴の gridRenderer.items を取れません"))?;

    let mut out = Vec::with_capacity(items_arr.len());
    for it in items_arr {
        let Some(tile) = it.get("tileRenderer") else { continue };
        let Some(video_id) = tile.get("contentId").and_then(|v| v.as_str()) else { continue };
        // ヘッダから duration を抽出。
        let mut duration = String::new();
        if let Some(overlays) = tile
            .pointer("/header/tileHeaderRenderer/thumbnailOverlays")
            .and_then(|x| x.as_array())
        {
            for ov in overlays {
                if let Some(s) = ov
                    .pointer("/thumbnailOverlayTimeStatusRenderer/text/simpleText")
                    .and_then(|v| v.as_str())
                {
                    duration = s.to_string();
                    break;
                }
            }
        }
        let title = tile
            .pointer("/metadata/tileMetadataRenderer/title/simpleText")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // lines[0] がチャンネル名、lines[1] が視聴回数。runs か simpleText のどちらか。
        let channel = extract_line(tile, 0);
        let view_count = extract_line(tile, 1);

        out.push(HistoryItem {
            video_id: video_id.to_string(),
            title,
            channel,
            duration,
            view_count,
        });
    }
    Ok(out)
}

fn extract_line(tile: &Value, line_idx: usize) -> String {
    let ptr = format!(
        "/metadata/tileMetadataRenderer/lines/{line_idx}/lineRenderer/items/0/lineItemRenderer/text"
    );
    let text = match tile.pointer(&ptr) {
        Some(t) => t,
        None => return String::new(),
    };
    if let Some(s) = text.get("simpleText").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    if let Some(runs) = text.get("runs").and_then(|v| v.as_array()) {
        return runs
            .iter()
            .filter_map(|r| r.get("text").and_then(|v| v.as_str()))
            .collect::<String>();
    }
    String::new()
}
