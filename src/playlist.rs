//! 自分の再生リスト閲覧（YouTube Data API v3）。
//!
//! OAuth 認証済みユーザーの再生リスト一覧を取得し、選択されたリストの動画を表示する。
//! - `playlists.list?mine=true` でリスト一覧
//! - `playlistItems.list?playlistId=...` でリスト内の動画

use anyhow::{bail, Result};
use serde_json::Value;
use std::sync::mpsc::Sender;

/// 再生リスト一覧の 1 件。
#[derive(Clone, Debug)]
pub struct PlaylistSummary {
    pub playlist_id: String,
    pub title: String,
    pub item_count: u64,
}

/// 再生リスト内の動画 1 件。
#[derive(Clone, Debug)]
pub struct PlaylistItem {
    pub video_id: String,
    pub title: String,
    pub channel: String,
}

/// 背景スレッドからメインスレッドへの通知。
pub enum PlaylistUpdate {
    /// 再生リスト一覧を取得完了。
    Playlists(Vec<PlaylistSummary>),
    /// 選択したリストの動画一覧を取得完了。
    Items {
        title: String,
        items: Vec<PlaylistItem>,
    },
    Error(String),
}

// ---------------------------------------------------------------------------
// 公開 API（背景スレッドから呼び出す）
// ---------------------------------------------------------------------------

/// 自分の再生リスト一覧を取得する。
pub fn fetch_my_playlists(access_token: &str, tx: &Sender<PlaylistUpdate>) {
    match fetch_playlists_inner(access_token) {
        Ok(lists) => {
            let _ = tx.send(PlaylistUpdate::Playlists(lists));
        }
        Err(e) => {
            let _ = tx.send(PlaylistUpdate::Error(e.to_string()));
        }
    }
}

/// 指定プレイリストの動画一覧を取得する。
pub fn fetch_playlist_items(
    access_token: &str,
    playlist_id: &str,
    playlist_title: &str,
    tx: &Sender<PlaylistUpdate>,
) {
    match fetch_items_inner(access_token, playlist_id) {
        Ok(items) => {
            let _ = tx.send(PlaylistUpdate::Items {
                title: playlist_title.to_string(),
                items,
            });
        }
        Err(e) => {
            let _ = tx.send(PlaylistUpdate::Error(e.to_string()));
        }
    }
}

// ---------------------------------------------------------------------------
// YouTube Data API v3 呼び出し
// ---------------------------------------------------------------------------

fn api_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| anyhow::anyhow!("HTTP クライアント作成に失敗: {e}"))
}

fn fetch_playlists_inner(access_token: &str) -> Result<Vec<PlaylistSummary>> {
    let client = api_client()?;
    let mut playlists = Vec::new();
    let mut page_token: Option<String> = None;

    // 特殊プレイリスト（「後で見る」「高く評価した動画」）を先頭に追加。
    // これらは playlists.list?mine=true には含まれないため channel から取得する。
    if let Ok(special) = fetch_special_playlists(&client, access_token) {
        playlists.extend(special);
    }

    // ユーザー作成のプレイリスト一覧。
    for _ in 0..5 {
        let mut url = String::from(
            "https://www.googleapis.com/youtube/v3/playlists\
             ?mine=true&part=snippet,contentDetails&maxResults=50",
        );
        if let Some(pt) = &page_token {
            url.push_str("&pageToken=");
            url.push_str(pt);
        }

        let resp = client.get(&url).bearer_auth(access_token).send()?;
        if !resp.status().is_success() {
            bail!("再生リスト取得に失敗 ({})", resp.status());
        }
        let body: Value = resp.json()?;

        if let Some(items) = body["items"].as_array() {
            for item in items {
                let playlist_id = item["id"].as_str().unwrap_or_default().to_string();
                let title = item["snippet"]["title"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let item_count = item["contentDetails"]["itemCount"]
                    .as_u64()
                    .unwrap_or(0);
                playlists.push(PlaylistSummary {
                    playlist_id,
                    title,
                    item_count,
                });
            }
        }

        page_token = body["nextPageToken"].as_str().map(|s| s.to_string());
        if page_token.is_none() {
            break;
        }
    }

    Ok(playlists)
}

/// 「後で見る」(WL) と「高く評価した動画」(LL) を取得する。
fn fetch_special_playlists(
    client: &reqwest::blocking::Client,
    access_token: &str,
) -> Result<Vec<PlaylistSummary>> {
    let resp = client
        .get(
            "https://www.googleapis.com/youtube/v3/channels\
             ?mine=true&part=contentDetails",
        )
        .bearer_auth(access_token)
        .send()?;
    if !resp.status().is_success() {
        bail!("チャンネル情報取得に失敗 ({})", resp.status());
    }
    let body: Value = resp.json()?;

    let related = &body["items"][0]["contentDetails"]["relatedPlaylists"];
    let mut specials = Vec::new();

    if let Some(wl) = related["watchLater"].as_str() {
        specials.push(PlaylistSummary {
            playlist_id: wl.to_string(),
            title: "後で見る".to_string(),
            item_count: 0,
        });
    }
    if let Some(ll) = related["likes"].as_str() {
        specials.push(PlaylistSummary {
            playlist_id: ll.to_string(),
            title: "高く評価した動画".to_string(),
            item_count: 0,
        });
    }

    Ok(specials)
}

fn fetch_items_inner(access_token: &str, playlist_id: &str) -> Result<Vec<PlaylistItem>> {
    let client = api_client()?;
    let mut items = Vec::new();
    let mut page_token: Option<String> = None;

    for _ in 0..10 {
        let mut url = format!(
            "https://www.googleapis.com/youtube/v3/playlistItems\
             ?playlistId={playlist_id}&part=snippet&maxResults=50"
        );
        if let Some(pt) = &page_token {
            url.push_str("&pageToken=");
            url.push_str(pt);
        }

        let resp = client.get(&url).bearer_auth(access_token).send()?;
        if !resp.status().is_success() {
            bail!("再生リスト動画取得に失敗 ({})", resp.status());
        }
        let body: Value = resp.json()?;

        if let Some(arr) = body["items"].as_array() {
            for item in arr {
                let snippet = &item["snippet"];
                let video_id = snippet["resourceId"]["videoId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                if video_id.is_empty() {
                    continue;
                }
                let title = snippet["title"].as_str().unwrap_or("").to_string();
                let channel = snippet["videoOwnerChannelTitle"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();

                items.push(PlaylistItem {
                    video_id,
                    title,
                    channel,
                });
            }
        }

        page_token = body["nextPageToken"].as_str().map(|s| s.to_string());
        if page_token.is_none() {
            break;
        }
    }

    Ok(items)
}
