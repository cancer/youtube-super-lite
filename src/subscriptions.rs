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
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;
use std::time::Duration;

/// 新着動画 1 件（history と同じ表示情報）。
#[derive(Clone, Debug)]
pub struct SubVideo {
    pub video_id: String,
    pub title: String,
    pub channel: String,
    /// "mm:ss" など。サムネ右下バッジ用。
    pub duration: String,
    /// 「14万回視聴・17 時間前」など、メタ情報。
    pub meta: String,
    /// チャンネルアイコン URL。空のこともある。
    pub channel_icon: String,
}

/// 登録チャンネル 1 件（左のチャンネルリスト用）。
#[derive(Clone, Debug)]
pub struct SubChannel {
    pub channel_id: String,
    pub title: String,
    /// チャンネルアイコン URL。空のこともある。
    pub icon: String,
}

/// 背景スレッドからメインスレッドへの通知。
pub enum SubUpdate {
    /// 新着フィード（右ペイン既定）。
    Feed(Vec<SubVideo>),
    /// 登録チャンネル一覧（左リスト）。
    Channels(Vec<SubChannel>),
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

    // tabs[1..] は各チャンネルの専用タブ。それぞれの `thumbnail` がチャンネルアイコン。
    // チャンネル ID は `endpoint.browseEndpoint.params` に base64(protobuf) で埋め込まれて
    // いる（browseId は "FEsubscriptions" 共通）。decode して tile からアイコンを引く map にする。
    let mut icon_by_channel: HashMap<String, String> = HashMap::new();
    for t in tabs.iter() {
        let Some(tr) = t.get("tabRenderer") else { continue };
        let Some(params) = tr
            .pointer("/endpoint/browseEndpoint/params")
            .and_then(|x| x.as_str())
        else {
            continue;
        };
        let Some(ch_id) = decode_channel_id_from_params(params) else { continue };
        // yt3.googleusercontent.com の URL は protocol-relative (`//yt3...`) のことがある。
        // egui の HTTP loader はスキーム必須なので https を補う。
        let raw_url = tr
            .pointer("/thumbnail/thumbnails/0/url")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        let icon = if raw_url.starts_with("//") {
            format!("https:{raw_url}")
        } else {
            raw_url.to_string()
        };
        if icon.is_empty() {
            continue;
        }
        icon_by_channel.insert(ch_id, icon);
    }

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
            let channel = extract_line(tile, 0);
            // メタ行は「14万回視聴・17 時間前」のように複数 item で組まれる。連結する。
            let meta = extract_line(tile, 1);
            // line[0] の navigationEndpoint からチャンネル ID を得てアイコン map を引く。
            let channel_id = tile
                .pointer("/metadata/tileMetadataRenderer/lines/0/lineRenderer/items/0/lineItemRenderer/text/runs/0/navigationEndpoint/browseEndpoint/browseId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let channel_icon = icon_by_channel
                .get(channel_id)
                .cloned()
                .unwrap_or_default();

            out.push(SubVideo {
                video_id: video_id.to_string(),
                title,
                channel,
                duration,
                meta,
                channel_icon,
            });
        }
    }
    Ok(out)
}

/// tab の `endpoint.browseEndpoint.params` から channel ID を取り出す。
/// params は URL エスケープされた base64(protobuf)。中の文字列フィールドに channel ID
/// (UC で始まる 24 文字の ASCII) が入っているので、それを手書きで抽出する。
fn decode_channel_id_from_params(params: &str) -> Option<String> {
    use base64::Engine;
    // `%3D` 等の URL エスケープを戻す。
    let unescaped: String = {
        let mut out = String::with_capacity(params.len());
        let bytes = params.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push((hi * 16 + lo) as u8 as char);
                    i += 3;
                    continue;
                }
            }
            out.push(bytes[i] as char);
            i += 1;
        }
        out
    };
    let raw = base64::engine::general_purpose::URL_SAFE
        .decode(&unescaped)
        .ok()?;
    // ASCII の連続部分から `UC` で始まる 24 文字（channel ID の形）を探す。
    let n = raw.len();
    if n < 24 {
        return None;
    }
    for i in 0..(n - 23) {
        if raw[i] == b'U' && raw[i + 1] == b'C' {
            let slice = &raw[i..i + 24];
            if slice
                .iter()
                .all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            {
                return Some(std::str::from_utf8(slice).ok()?.to_string());
            }
        }
    }
    None
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

// ---------------------------------------------------------------------------
// 2. 登録チャンネル一覧（Data API v3 subscriptions.list）
// ---------------------------------------------------------------------------

/// 登録チャンネル一覧を背景スレッドで取得する。50 件ごとに nextPageToken でページング。
pub fn fetch_subscribed_channels(access_token: &str, tx: &Sender<SubUpdate>) {
    match fetch_channels_inner(access_token) {
        Ok(channels) => {
            let _ = tx.send(SubUpdate::Channels(channels));
        }
        Err(e) => {
            let _ = tx.send(SubUpdate::Error(e.to_string()));
        }
    }
}

fn fetch_channels_inner(access_token: &str) -> Result<Vec<SubChannel>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;

    let mut out: Vec<SubChannel> = Vec::new();
    let mut page_token: Option<String> = None;

    loop {
        let mut url = "https://www.googleapis.com/youtube/v3/subscriptions\
                       ?part=snippet&mine=true&maxResults=50&order=alphabetical"
            .to_string();
        if let Some(t) = &page_token {
            url.push_str(&format!("&pageToken={t}"));
        }

        let resp = client
            .get(&url)
            .bearer_auth(access_token)
            .send()?
            .error_for_status()
            .map_err(|e| anyhow!("subscriptions.list 失敗: {e}"))?;
        let v: Value = resp.json()?;

        if let Some(items) = v["items"].as_array() {
            for it in items {
                let sn = &it["snippet"];
                let title = sn["title"].as_str().unwrap_or("").to_string();
                let channel_id = sn
                    .pointer("/resourceId/channelId")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                // アイコンは default → medium → high の順で拾う。
                let raw_icon = sn
                    .pointer("/thumbnails/default/url")
                    .or_else(|| sn.pointer("/thumbnails/medium/url"))
                    .or_else(|| sn.pointer("/thumbnails/high/url"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                let icon = if raw_icon.starts_with("//") {
                    format!("https:{raw_icon}")
                } else {
                    raw_icon.to_string()
                };
                if !title.is_empty() {
                    out.push(SubChannel {
                        channel_id,
                        title,
                        icon,
                    });
                }
            }
        }

        match v["nextPageToken"].as_str() {
            Some(t) if !t.is_empty() => page_token = Some(t.to_string()),
            _ => break,
        }
        // 安全弁（異常な無限ページング回避）。
        if out.len() > 5000 {
            break;
        }
    }

    Ok(out)
}
