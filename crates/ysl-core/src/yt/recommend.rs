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
    pub menu: super::subscriptions::CardMenu,
}

/// ホームフィード（おすすめ）を背景スレッドで取得する。要 OAuth access_token。
pub fn fetch_home_feed(access_token: &str, tx: &Sender<crate::content::FeedUpdate<VideoItem>>) {
    match fetch_inner(access_token) {
        Ok(items) => {
            let _ = tx.send(crate::content::FeedUpdate::Items(items));
        }
        Err(e) => {
            let _ = tx.send(crate::content::FeedUpdate::Error(e.to_string()));
        }
    }
}

/// 指定チャンネル(UC...)の動画一覧を取得する（TVHTML5, 無認証で可）。
///
/// TVHTML5 の `browseId=UCxxx`（params 無し）はチャンネルの **ホームタブ** を返す。
/// ホームタブには複数の shelf が並び、その中には「人気の動画」（数年前の動画が並ぶ）や
/// 「過去のライブ配信」も含まれるため、全 shelf の tile を素朴に集約すると新着のつもりが
/// 古い動画の混じった一覧になる。「動画」shelf のみを新着扱いで使う（新しい順に並び、Shorts は除外）。
/// この shelf が見つからない場合は従来動作にフォールバックして tile を再帰収集する。
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
    Ok(pick_channel_videos(&v))
}

/// ホームタブのレスポンスから「動画」shelf（新しい順のアップロード）を取り出す。
/// 見つからなければ従来動作（全 shelf の tile を再帰収集）にフォールバックする。
fn pick_channel_videos(v: &Value) -> Vec<VideoItem> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    if let Some(items) = find_videos_shelf_items(v) {
        for it in items {
            if let Some(tile) = it.get("tileRenderer") {
                if let Some(vi) = parse_tile(tile) {
                    if seen.insert(vi.video_id.clone()) {
                        out.push(vi);
                    }
                }
            }
        }
        return out;
    }
    collect_tiles(v, &mut seen, &mut out);
    out
}

/// レスポンスを再帰的に探索し、「動画」shelf の `items` 配列を返す。
///
/// 判定は次の順で行う（`hl=ja` で叩くため 1. がまず効き、locale 変更や YouTube 側の表記揺れは
/// 2. の params プレフィックスで拾う）:
///  1. shelf ヘッダのタイトルテキストが完全一致で "動画"。
///  2. shelf ヘッダの `endpoint.browseEndpoint.params` が `EgZ2aWRlb3MYAyAA` で始まる。
///     - `EgZ2aWRlb3M` = protobuf field 1 = "videos"（Videos タブ系列）。
///     - `YAyAA` は「Videos タブの新着順」に対応する識別子で、「人気の動画」(`YAS...`) や
///       「過去のライブ配信」(`YAyAC...`) とはここで分岐する。
fn find_videos_shelf_items(v: &Value) -> Option<&Vec<Value>> {
    fn walk(node: &Value) -> Option<&Vec<Value>> {
        match node {
            Value::Object(map) => {
                if let Some(shelf) = map.get("shelfRenderer") {
                    if is_videos_shelf(shelf) {
                        if let Some(items) = shelf
                            .pointer("/content/horizontalListRenderer/items")
                            .and_then(|x| x.as_array())
                        {
                            return Some(items);
                        }
                    }
                }
                map.values().find_map(walk)
            }
            Value::Array(arr) => arr.iter().find_map(walk),
            _ => None,
        }
    }
    walk(v)
}

fn is_videos_shelf(shelf: &Value) -> bool {
    let title = shelf_header_title(shelf);
    if title == "動画" {
        return true;
    }
    let params = shelf
        .pointer("/endpoint/browseEndpoint/params")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    params.starts_with("EgZ2aWRlb3MYAyAA")
}

/// TVHTML5 の shelfHeaderRenderer は `avatarLockup.avatarLockupRenderer.title` に
/// `simpleText` か `runs` でタイトルを持つ。両形式のテキストを連結して返す。
fn shelf_header_title(shelf: &Value) -> String {
    let title = shelf.pointer("/headerRenderer/shelfHeaderRenderer/avatarLockup/avatarLockupRenderer/title");
    let Some(title) = title else { return String::new() };
    if let Some(s) = title.get("simpleText").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    if let Some(runs) = title.get("runs").and_then(|v| v.as_array()) {
        return runs
            .iter()
            .filter_map(|r| r.get("text").and_then(|v| v.as_str()))
            .collect();
    }
    String::new()
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
    let channel = super::subscriptions::tile_line(tile, 0);
    let thumbnail =
        super::subscriptions::pick_largest_thumbnail(tile.pointer("/header/tileHeaderRenderer/thumbnail"));
    let (duration, live) = super::subscriptions::tile_duration_live(tile);
    let meta = super::subscriptions::tile_meta(tile);
    let menu = super::subscriptions::tile_menu(tile);

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

#[cfg(test)]
mod tests {
    use super::*;

    /// tvSurfaceContentRenderer 直下の sectionListRenderer に、任意の shelf を並べたレスポンスを
    /// 組み立てる。shelf は `shelf_json(title, params, video_ids)` で作った Value を渡す。
    fn build_home(shelves: Vec<Value>) -> Value {
        serde_json::json!({
            "contents": {
                "tvBrowseRenderer": { "content": {
                    "tvSurfaceContentRenderer": { "content": {
                        "sectionListRenderer": { "contents": shelves }
                    }}
                }}
            }
        })
    }

    /// shelfRenderer 1 個ぶんの JSON。ヘッダは "動画" / "人気の動画" 等の TVHTML5 レイアウトに
    /// 合わせて `avatarLockup.avatarLockupRenderer.title.runs[0].text` に置く。
    fn shelf_json(title: &str, params: Option<&str>, video_ids: &[&str]) -> Value {
        let items: Vec<Value> = video_ids
            .iter()
            .map(|id| {
                serde_json::json!({
                    "tileRenderer": {
                        "contentId": id,
                        "metadata": { "tileMetadataRenderer": {
                            "title": { "simpleText": format!("title-{id}") }
                        }}
                    }
                })
            })
            .collect();
        let mut endpoint = serde_json::json!({});
        if let Some(p) = params {
            endpoint = serde_json::json!({
                "browseEndpoint": { "browseId": "UCxxx", "params": p }
            });
        }
        serde_json::json!({
            "shelfRenderer": {
                "endpoint": endpoint,
                "headerRenderer": { "shelfHeaderRenderer": {
                    "avatarLockup": { "avatarLockupRenderer": {
                        "title": { "runs": [ { "text": title } ] }
                    }}
                }},
                "content": { "horizontalListRenderer": { "items": items }}
            }
        })
    }

    fn ids(items: &[VideoItem]) -> Vec<String> {
        items.iter().map(|v| v.video_id.clone()).collect()
    }

    #[test]
    fn picks_videos_shelf_by_japanese_title_and_skips_popular() {
        // TVHTML5 hl=ja のホームタブ想定: 「人気の動画」「動画」の 2 shelf。
        // 「動画」だけを採用し、「人気の動画」の古い動画は入らないことを検証する。
        let home = build_home(vec![
            shelf_json(
                "人気の動画",
                Some("EgZ2aWRlb3MYASAAcAPyBg0KCzoEIgIIAqIBAggB"),
                &["oldvid00001", "oldvid00002"],
            ),
            shelf_json(
                "動画",
                Some("EgZ2aWRlb3MYAyAAcADyBg0KCzoEIgIIBKIBAggB"),
                &["newvid00001", "newvid00002"],
            ),
        ]);
        assert_eq!(
            ids(&pick_channel_videos(&home)),
            vec!["newvid00001", "newvid00002"]
        );
    }

    #[test]
    fn picks_videos_shelf_by_params_when_title_locale_differs() {
        // hl 変更や表記揺れで title が "動画" でなくなっても、params プレフィックスで拾えること。
        let home = build_home(vec![
            shelf_json(
                "Popular videos",
                Some("EgZ2aWRlb3MYASAAcAPyBg0KCzoEIgIIAqIBAggB"),
                &["oldvid00001"],
            ),
            shelf_json(
                "Videos",
                Some("EgZ2aWRlb3MYAyAAcADyBg0KCzoEIgIIBKIBAggB"),
                &["newvid00001", "newvid00002"],
            ),
        ]);
        assert_eq!(
            ids(&pick_channel_videos(&home)),
            vec!["newvid00001", "newvid00002"]
        );
    }

    #[test]
    fn does_not_pick_past_live_streams_shelf() {
        // 「過去のライブ配信」も params プレフィックスは "EgZ2aWRlb3MYAy" まで共通だが、
        // その次が "AC..."（Videos-新着は "AA..."）なので混入しないこと。
        let home = build_home(vec![shelf_json(
            "過去のライブ配信",
            Some("EgZ2aWRlb3MYAyACOARwAPIGCQoHegCiAQIIAQ=="),
            &["livevid0001"],
        )]);
        // フォールバック（全 tile 再帰収集）に落ちて livevid0001 は拾われるが、
        // それはこのテストの関心事ではない。ここでは Past Live shelf が "Videos" 判定に
        // 引っかからないことを直接検証する。
        let shelves = home
            .pointer("/contents/tvBrowseRenderer/content/tvSurfaceContentRenderer/content/sectionListRenderer/contents")
            .and_then(|v| v.as_array())
            .expect("test fixture missing shelves");
        let past_live_shelf = shelves[0].get("shelfRenderer").expect("shelfRenderer");
        assert!(!is_videos_shelf(past_live_shelf));
    }

    #[test]
    fn falls_back_to_recursive_collection_when_no_videos_shelf() {
        // 「動画」shelf が無い（小さいチャンネル等）の場合、素朴な再帰収集で拾える動画がそのまま返る。
        let home = build_home(vec![shelf_json("おすすめ", None, &["someviii001"])]);
        assert_eq!(ids(&pick_channel_videos(&home)), vec!["someviii001"]);
    }
}
