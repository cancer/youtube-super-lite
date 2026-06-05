//! 登録チャンネル一覧の取得。
//!
//! YouTube Data API v3 `subscriptions.list?part=snippet&mine=true` を OAuth で叩き、
//! 登録しているチャンネル（名前 + アイコン + チャンネル ID）を取得する。
//! 50 件ごとに `nextPageToken` でページングする。
//!
//! スコープは `youtube.force-ssl`（ログインで取得済み）で読み取り可能。

use anyhow::{anyhow, Result};
use serde_json::Value;
use std::sync::mpsc::Sender;
use std::time::Duration;

/// 登録チャンネル 1 件。
#[derive(Clone, Debug)]
pub struct SubChannel {
    pub channel_id: String,
    pub title: String,
    /// チャンネルアイコン URL。空のこともある。
    pub icon: String,
}

/// 背景スレッドからメインスレッドへの通知。
pub enum SubUpdate {
    Channels(Vec<SubChannel>),
    Error(String),
}

/// 登録チャンネル一覧を背景スレッドで取得する。
pub fn fetch_subscribed_channels(access_token: &str, tx: &Sender<SubUpdate>) {
    match fetch_inner(access_token) {
        Ok(channels) => {
            let _ = tx.send(SubUpdate::Channels(channels));
        }
        Err(e) => {
            let _ = tx.send(SubUpdate::Error(e.to_string()));
        }
    }
}

fn fetch_inner(access_token: &str) -> Result<Vec<SubChannel>> {
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
