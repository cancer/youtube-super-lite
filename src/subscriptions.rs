//! 登録チャンネルの新着動画取得。
//!
//! フロー:
//!   1. YouTube Data API v3 `subscriptions.list?mine=true` で登録チャンネル一覧を取得（OAuth 必須）
//!   2. 各チャンネルの公開 RSS フィード（クォータ不要）から最新動画を取得
//!   3. 日付でソートして返す

use anyhow::{bail, Result};
use serde_json::Value;
use std::sync::mpsc::Sender;

/// 新着動画 1 件。
#[derive(Clone, Debug)]
pub struct SubVideo {
    pub video_id: String,
    pub title: String,
    pub channel: String,
    /// ISO 8601 の日付部分（例: "2026-05-31"）。
    pub published: String,
}

/// 背景スレッドからメインスレッドへの通知。
pub enum SubUpdate {
    Items(Vec<SubVideo>),
    Error(String),
}

/// 登録チャンネルの新着動画を背景スレッドで取得する。
pub fn fetch_subscription_feed(access_token: &str, tx: &Sender<SubUpdate>) {
    match fetch_inner(access_token) {
        Ok(items) => {
            let _ = tx.send(SubUpdate::Items(items));
        }
        Err(e) => {
            let _ = tx.send(SubUpdate::Error(e.to_string()));
        }
    }
}

fn fetch_inner(access_token: &str) -> Result<Vec<SubVideo>> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    // 1. 登録チャンネル一覧を取得（最大 50 件 × 複数ページ、上限 150 チャンネル）。
    let channels = fetch_subscribed_channels(&client, access_token)?;

    // 2. 各チャンネルの RSS フィードから最新動画を取得。
    let mut all_videos = Vec::new();
    for (ch_id, ch_name) in &channels {
        let rss_url = format!("https://www.youtube.com/feeds/videos.xml?channel_id={ch_id}");
        if let Ok(resp) = client.get(&rss_url).send() {
            if let Ok(xml) = resp.text() {
                // 各チャンネルから最新 3 件のみ取得。
                let mut videos = parse_rss_feed(&xml, ch_name);
                videos.truncate(3);
                all_videos.extend(videos);
            }
        }
    }

    // 3. 日付の降順でソートし、上位 50 件に絞る。
    all_videos.sort_by(|a, b| b.published.cmp(&a.published));
    all_videos.truncate(50);

    Ok(all_videos)
}

// ---------------------------------------------------------------------------
// YouTube Data API v3
// ---------------------------------------------------------------------------

fn fetch_subscribed_channels(
    client: &reqwest::blocking::Client,
    access_token: &str,
) -> Result<Vec<(String, String)>> {
    let mut channels = Vec::new();
    let mut page_token: Option<String> = None;
    let max_pages = 3; // 最大 150 チャンネル

    for _ in 0..max_pages {
        let mut url = String::from(
            "https://www.googleapis.com/youtube/v3/subscriptions\
             ?mine=true&part=snippet&maxResults=50&order=relevance",
        );
        if let Some(pt) = &page_token {
            url.push_str("&pageToken=");
            url.push_str(pt);
        }

        let resp = client.get(&url).bearer_auth(access_token).send()?;
        if !resp.status().is_success() {
            let status = resp.status();
            bail!("登録チャンネル取得に失敗 ({status})");
        }
        let body: Value = resp.json()?;

        if let Some(items) = body["items"].as_array() {
            for item in items {
                if let Some(ch_id) = item["snippet"]["resourceId"]["channelId"].as_str() {
                    let ch_name = item["snippet"]["title"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    channels.push((ch_id.to_string(), ch_name));
                }
            }
        }

        page_token = body["nextPageToken"].as_str().map(|s| s.to_string());
        if page_token.is_none() {
            break;
        }
    }

    Ok(channels)
}

// ---------------------------------------------------------------------------
// RSS フィードのパース
// ---------------------------------------------------------------------------

fn parse_rss_feed(xml: &str, fallback_channel: &str) -> Vec<SubVideo> {
    let mut videos = Vec::new();

    for entry_chunk in xml.split("<entry>").skip(1) {
        let end = entry_chunk.find("</entry>").unwrap_or(entry_chunk.len());
        let entry = &entry_chunk[..end];

        let video_id = extract_xml_tag(entry, "yt:videoId");
        let title = extract_xml_tag(entry, "title");
        let published_full = extract_xml_tag(entry, "published");
        let channel = extract_xml_tag(entry, "name")
            .unwrap_or_else(|| fallback_channel.to_string());

        if let (Some(video_id), Some(title)) = (video_id, title) {
            // ISO 8601 の先頭 10 文字（YYYY-MM-DD）を日付として使う。
            let published = published_full
                .as_deref()
                .map(|s| s.chars().take(10).collect::<String>())
                .unwrap_or_default();

            videos.push(SubVideo {
                video_id,
                title,
                channel,
                published,
            });
        }
    }

    videos
}

fn extract_xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(html_unescape(&xml[start..end]))
}

/// 最低限の HTML エンティティのアンエスケープ。
fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}
