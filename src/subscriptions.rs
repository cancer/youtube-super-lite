//! 登録チャンネルタブのデータ取得。
//!
//! 2 種類のデータを扱う:
//!  1. 新着フィード（`SubVideo`）: 全登録チャンネルの最近の投稿を集約したもの。
//!     InnerTube `/youtubei/v1/browse?browseId=FEsubscriptions` を TVHTML5 client + OAuth で
//!     1 リクエスト取得し「すべて」タブの shelf から動画タイルを抽出する。右ペインの既定表示。
//!  2. 登録チャンネル一覧（`SubChannel`）: Data API v3 `subscriptions.list?mine=true`。左の
//!     チャンネルリスト用。選択するとそのチャンネルのアップロード一覧に右ペインを絞り込む。
//!
//! 新着フィードは旧実装（Data API + 各チャンネル RSS 並列取得、122ch で 12 秒）を廃し、
//! InnerTube 1 リクエストに置き換えてある（大幅に高速）。
//!
//! 新着フィードのレスポンス構造:
//! contents.tvBrowseRenderer.content.tvSecondaryNavRenderer.sections[0]
//!  └ tvSecondaryNavSectionRenderer.tabs[0]   ← selected=true の「すべて」タブ
//!     └ tabRenderer.content.tvSurfaceContentRenderer.content.sectionListRenderer.contents[]
//!         └ shelfRenderer.content.horizontalListRenderer.items[]
//!             └ tileRenderer  (history.rs と同じ tile 構造)
//! 同じ動画 ID が複数 shelf に含まれることがあるので、抽出時に dedup する。

use anyhow::{anyhow, Result};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::mpsc::Sender;
use std::time::Duration;

/// 新着動画 1 件。
#[derive(Clone, Debug)]
pub struct SubVideo {
    pub video_id: String,
    pub title: String,
    pub channel: String,
    /// InnerTube が返すサムネ URL（最大サイズ。実体は 16:9 にクロップ済み）。
    pub thumbnail: String,
}

/// 背景スレッドからメインスレッドへの通知。
pub enum SubUpdate {
    /// 新着フィード。
    Feed(Vec<SubVideo>),
    Error(String),
}

// ---------------------------------------------------------------------------
// 1. 新着フィード（InnerTube FEsubscriptions）
// ---------------------------------------------------------------------------

/// 全登録チャンネルの新着動画を背景スレッドで取得する。
pub fn fetch_subscription_feed(access_token: &str, tx: &Sender<SubUpdate>) {
    match fetch_feed_inner(access_token) {
        Ok(items) => {
            let _ = tx.send(SubUpdate::Feed(items));
        }
        Err(e) => {
            let _ = tx.send(SubUpdate::Error(e.to_string()));
        }
    }
}

fn fetch_feed_inner(access_token: &str) -> Result<Vec<SubVideo>> {
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
        "browseId": "FEsubscriptions"
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
    extract_videos(&v)
}

/// レスポンス JSON から「すべて」タブの動画タイルを抽出する。
/// 同じ video_id が複数 shelf に含まれるケースを dedup する。
fn extract_videos(v: &Value) -> Result<Vec<SubVideo>> {
    let tabs = v
        .pointer("/contents/tvBrowseRenderer/content/tvSecondaryNavRenderer/sections/0/tvSecondaryNavSectionRenderer/tabs")
        .and_then(|x| x.as_array())
        .ok_or_else(|| anyhow!("登録チャンネルの tabs を取れません"))?;

    // selected=true のタブを探す。なければ先頭。
    let tab = tabs
        .iter()
        .find(|t| {
            t.pointer("/tabRenderer/selected")
                .and_then(|x| x.as_bool())
                .unwrap_or(false)
        })
        .or_else(|| tabs.first())
        .ok_or_else(|| anyhow!("tabs が空"))?;

    let shelves = tab
        .pointer("/tabRenderer/content/tvSurfaceContentRenderer/content/sectionListRenderer/contents")
        .and_then(|x| x.as_array())
        .ok_or_else(|| anyhow!("「すべて」タブの shelves を取れません"))?;

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for shelf in shelves {
        let items = match shelf
            .pointer("/shelfRenderer/content/horizontalListRenderer/items")
            .and_then(|x| x.as_array())
        {
            Some(a) => a,
            None => continue,
        };
        for it in items {
            let Some(tile) = it.get("tileRenderer") else { continue };
            let Some(video_id) = tile.get("contentId").and_then(|v| v.as_str()) else { continue };
            if !seen.insert(video_id.to_string()) {
                continue;
            }

            let title = tile
                .pointer("/metadata/tileMetadataRenderer/title/simpleText")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let channel = extract_line(tile, 0);
            // InnerTube が用意したサムネ URL（最大サイズ。サーバが 16:9 にクロップ済み）。
            let thumbnail =
                pick_largest_thumbnail(tile.pointer("/header/tileHeaderRenderer/thumbnail"));

            out.push(SubVideo {
                video_id: video_id.to_string(),
                title,
                channel,
                thumbnail,
            });
        }
    }
    Ok(out)
}

/// `thumbnail` オブジェクト（`{ "thumbnails": [ {url,width,height}, ... ] }`）から
/// 最大サイズ（配列末尾）の URL を取り出す。protocol-relative は https を補う。
/// 取れなければ空文字。
pub fn pick_largest_thumbnail(thumb: Option<&Value>) -> String {
    let url = thumb
        .and_then(|t| t.get("thumbnails"))
        .and_then(|a| a.as_array())
        .and_then(|a| a.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or("");
    if url.starts_with("//") {
        format!("https:{url}")
    } else {
        url.to_string()
    }
}

/// `metadata.tileMetadataRenderer.lines[line_idx]` の全 `lineItem` のテキストを連結する。
fn extract_line(tile: &Value, line_idx: usize) -> String {
    let line = match tile
        .pointer(&format!(
            "/metadata/tileMetadataRenderer/lines/{line_idx}/lineRenderer/items"
        ))
        .and_then(|x| x.as_array())
    {
        Some(a) => a,
        None => return String::new(),
    };
    let mut out = String::new();
    for itm in line {
        let text = match itm.pointer("/lineItemRenderer/text") {
            Some(t) => t,
            None => continue,
        };
        if let Some(s) = text.get("simpleText").and_then(|v| v.as_str()) {
            out.push_str(s);
        } else if let Some(runs) = text.get("runs").and_then(|v| v.as_array()) {
            for r in runs {
                if let Some(s) = r.get("text").and_then(|v| v.as_str()) {
                    out.push_str(s);
                }
            }
        }
    }
    out
}
