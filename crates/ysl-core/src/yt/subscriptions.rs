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
#[derive(Clone, Debug, Default)]
pub struct SubVideo {
    pub video_id: String,
    pub title: String,
    pub channel: String,
    /// InnerTube が返すサムネ URL（最大サイズ。実体は 16:9 にクロップ済み）。
    pub thumbnail: String,
    /// 再生時間（秒）。ライブ中は None。
    pub duration: Option<f64>,
    pub live: bool,
    /// 視聴回数＋投稿時期（tile の 2 行目。例 "4907万回視聴 • 4 日前"）。
    pub meta: Option<String>,
    /// ケバブメニュー用データ（実データ未確認の surface のため通常は既定値＝全 None）。
    pub menu: CardMenu,
}

// ---------------------------------------------------------------------------
// 1. 新着フィード（InnerTube FEsubscriptions）
// ---------------------------------------------------------------------------

/// 全登録チャンネルの新着動画を背景スレッドで取得する。
pub fn fetch_subscription_feed(access_token: &str, tx: &Sender<crate::content::FeedUpdate<SubVideo>>) {
    match fetch_feed_inner(access_token) {
        Ok(items) => {
            let _ = tx.send(crate::content::FeedUpdate::Items(items));
        }
        Err(e) => {
            let _ = tx.send(crate::content::FeedUpdate::Error(e.to_string()));
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

/// レスポンス JSON から「すべて」タブの動画タイルを抽出し、投稿時刻の新しい順に並べ替える。
///
/// TVHTML5 の FEsubscriptions「すべて」タブは 2026 時点でアルゴリズム推薦になっており、
/// 先頭に「関連が強い」shelf（15 件・非時系列）が並ぶ。以降の shelf も配信予定/live/最近アップ
/// の混在で、shelf 順にそのまま並べると 1〜2 日前の動画が list 上位に来る（＝「新着なのに古い」）。
///
/// 対策:
///  1. 「関連が強い」shelf を丸ごとスキップする（時系列でない推薦は「新着」の趣旨に反する）。
///  2. 残りの動画 tile を meta 文字列から経過時間へ解釈し、新しい順に安定ソート。
///     - "N 分/時間/日/週間/か月/年前" → 経過秒
///     - "人が視聴中"（配信中）→ 0（最上位に相当）
///     - "公開予定"（プレミア/配信予定）→ i64::MAX（末尾に落とす。UI は "新着" 一覧）
///     - 解釈不能 → i64::MAX（末尾）
///  同じ video_id が複数 shelf に含まれるケースは dedup する。
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
    let mut out: Vec<(i64, SubVideo)> = Vec::new();
    for shelf in shelves {
        if shelf_is_algorithmic_recommendation(shelf) {
            continue;
        }
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
            let (duration, live) = tile_duration_live(tile);
            let meta = tile_meta(tile);
            let menu = tile_menu(tile);

            let key = meta_recency_key(meta.as_deref().unwrap_or(""), live);
            out.push((
                key,
                SubVideo {
                    video_id: video_id.to_string(),
                    title,
                    channel,
                    thumbnail,
                    duration,
                    live,
                    meta,
                    menu,
                },
            ));
        }
    }
    out.sort_by_key(|(k, _)| *k);
    Ok(out.into_iter().map(|(_, v)| v).collect())
}

/// shelf ヘッダのタイトルが「関連が強い」（TVHTML5 の推薦系 shelf）なら true。
/// 「新着」の趣旨とは無関係なアルゴリズム推薦なので一覧から除外する。
fn shelf_is_algorithmic_recommendation(shelf: &Value) -> bool {
    let title = shelf
        .pointer("/shelfRenderer/headerRenderer/shelfHeaderRenderer/avatarLockup/avatarLockupRenderer/title");
    let text = title
        .and_then(|t| {
            t.get("simpleText")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    t.get("runs").and_then(|v| v.as_array()).map(|runs| {
                        runs.iter()
                            .filter_map(|r| r.get("text").and_then(|v| v.as_str()))
                            .collect::<String>()
                    })
                })
        })
        .unwrap_or_default();
    text == "関連が強い"
}

/// tile の meta 文字列（例 "19万回視聴•1 日前 に配信済み" / "3.2万 人が視聴中" /
/// "2026/07/25 6:00 に公開予定"）から並び替え用のキーを作る。小さいほど新しい。
///
/// - 配信中(`人が視聴中` または tile 側の live フラグ) → 0
/// - "N 分/時間/日/週間/か月/年前" → 経過秒
/// - "公開予定"（プレミア）→ i64::MAX
/// - 解釈不能 → i64::MAX
pub(crate) fn meta_recency_key(meta: &str, live: bool) -> i64 {
    if live || meta.contains("人が視聴中") {
        return 0;
    }
    if meta.contains("公開予定") {
        return i64::MAX;
    }
    // 長い接尾辞から順に照合（"時間前" と "週間前" は末尾が "間前" で衝突しうるので、
    // "週間前" を先に置く必要はないが、意味的に長い単位から書いている）。
    const UNITS: &[(&str, i64)] = &[
        ("分前", 60),
        ("時間前", 3600),
        ("日前", 86400),
        ("週間前", 604800),
        ("か月前", 2_592_000),
        ("年前", 31_536_000),
    ];
    for (unit, secs) in UNITS {
        let Some(pos) = meta.find(unit) else { continue };
        // 単位直前の末尾連続数字を取り出す（"8 時間前" の "8" / "17 時間前" の "17"）。
        let before = meta[..pos].trim_end();
        let n_str: String = before.chars().rev().take_while(|c| c.is_ascii_digit()).collect();
        let n_str: String = n_str.chars().rev().collect();
        if let Ok(n) = n_str.parse::<i64>() {
            if n > 0 {
                return n * secs;
            }
        }
    }
    i64::MAX
}

/// チャンネル名からアバター URL を引く（無認証 WEB 検索の channelRenderer）。
///
/// TV tile（subs/history/home）はチャンネルアバターを持たないため、名前で検索して補完する。
/// OAuth は TV クライアント限定なので WEB は無認証で叩く（検索は無認証で可）。`params` は
/// 「チャンネル」フィルタ（EgIQAg==）。先頭一致の channelRenderer を採用するベストエフォート。
pub fn fetch_channel_avatar(name: &str) -> Option<String> {
    let ch = search_first_channel(name)?;
    let url = pick_largest_thumbnail(ch.get("thumbnail"));
    if url.is_empty() {
        None
    } else {
        Some(url)
    }
}

/// チャンネル名から channelId(UC...) を引く（無認証 WEB 検索）。チャンネルを開く時に使う。
pub fn fetch_channel_id(name: &str) -> Option<String> {
    let ch = search_first_channel(name)?;
    ch.get("channelId").and_then(|v| v.as_str()).map(str::to_string)
}

/// 無認証 WEB 検索（チャンネルフィルタ）で先頭の channelRenderer を取得する。
fn search_first_channel(name: &str) -> Option<Value> {
    if name.trim().is_empty() {
        return None;
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;
    let body = serde_json::json!({
        "context": { "client": {
            "clientName": "WEB", "clientVersion": "2.20260114.08.00", "hl": "ja", "gl": "JP"
        }},
        "query": name,
        "params": "EgIQAg%3D%3D"
    });
    let resp = client
        .post("https://www.youtube.com/youtubei/v1/search?prettyPrint=false")
        .header("X-YouTube-Client-Name", "1")
        .header("X-YouTube-Client-Version", "2.20260114.08.00")
        .json(&body)
        .send()
        .ok()?
        .error_for_status()
        .ok()?;
    let v: Value = resp.json().ok()?;
    find_first_channel_renderer(&v).cloned()
}

/// レスポンス中から最初の `channelRenderer` を再帰的に探す。
fn find_first_channel_renderer(v: &Value) -> Option<&Value> {
    match v {
        Value::Object(map) => {
            if let Some(c) = map.get("channelRenderer") {
                return Some(c);
            }
            map.values().find_map(find_first_channel_renderer)
        }
        Value::Array(arr) => arr.iter().find_map(find_first_channel_renderer),
        _ => None,
    }
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

/// tileRenderer（TV レイアウトの動画タイル。subs/history/home で共通）から再生時間(秒)と
/// ライブフラグを取り出す。`thumbnailOverlayTimeStatusRenderer.style == "LIVE"` はライブ。
pub fn tile_duration_live(tile: &Value) -> (Option<f64>, bool) {
    let Some(overlays) = tile
        .pointer("/header/tileHeaderRenderer/thumbnailOverlays")
        .and_then(|v| v.as_array())
    else {
        return (None, false);
    };
    for ov in overlays {
        let Some(ts) = ov.get("thumbnailOverlayTimeStatusRenderer") else {
            continue;
        };
        if ts.get("style").and_then(|v| v.as_str()) == Some("LIVE") {
            return (None, true);
        }
        let text = ts
            .pointer("/text/simpleText")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        return (parse_duration(text), false);
    }
    (None, false)
}

/// tileRenderer の指定行（0=チャンネル名, 1=視聴回数＋時期）のテキストを連結して返す。
pub fn tile_line(tile: &Value, line_idx: usize) -> String {
    extract_line(tile, line_idx)
}

/// tileRenderer の 2 行目（視聴回数＋投稿時期）を meta 文字列として取り出す。空なら None。
pub fn tile_meta(tile: &Value) -> Option<String> {
    let s = extract_line(tile, 1);
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// "9:26" / "1:23:45" 形式の再生時間表示を秒数へ変換する。
pub fn parse_duration(text: &str) -> Option<f64> {
    let parts: Vec<&str> = text.trim().split(':').collect();
    if parts.is_empty() || parts.len() > 3 || parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    let mut secs = 0.0;
    for p in &parts {
        secs = secs * 60.0 + p.parse::<f64>().ok()?;
    }
    Some(secs)
}

/// カードのケバブメニュー用データ（tile の `onLongPressCommand` から抽出）。
/// 認証済みホームフィード(FEwhat_to_watch)の tile にのみ実在を確認済み（実データで検証）。
/// 無認証のチャンネルページ tile 等には無いので、全フィールド None になり得る。
#[derive(Clone, Debug, Default)]
pub struct CardMenu {
    /// 「チャンネルへ」の実 channelId(UC...)。無ければ名前検索へフォールバックする。
    pub channel_id: Option<String>,
    /// 「興味なし」の feedbackToken。
    pub not_interested_token: Option<String>,
    /// 「チャンネルをおすすめに表示しない」の feedbackToken
    /// （実データでは確認ポップアップの奥にネストされている）。
    pub not_channel_token: Option<String>,
}

/// tile の `onLongPressCommand`（ケバブ長押しメニュー）から [`CardMenu`] を抽出する。
/// ラベルの文言で項目種別を判定するベストエフォート（YouTube 側の文言変更には追従できない）。
pub fn tile_menu(tile: &Value) -> CardMenu {
    let mut menu = CardMenu::default();
    let Some(items) = tile
        .pointer("/onLongPressCommand/showMenuCommand/menu/menuRenderer/items")
        .and_then(|v| v.as_array())
    else {
        return menu;
    };
    for item in items {
        if let Some(id) = item
            .pointer("/menuNavigationItemRenderer/navigationEndpoint/browseEndpoint/browseId")
            .and_then(|v| v.as_str())
        {
            if id.starts_with("UC") {
                menu.channel_id = Some(id.to_string());
            }
        }
        let label = menu_item_label(item);
        if label.contains("興味なし") {
            menu.not_interested_token = find_feedback_token(item);
        } else if label.contains("表示しない") {
            menu.not_channel_token = find_feedback_token(item);
        }
    }
    menu
}

/// メニュー項目（`menuNavigationItemRenderer`/`menuServiceItemRenderer`）の表示ラベルを取り出す。
fn menu_item_label(item: &Value) -> String {
    for key in ["menuNavigationItemRenderer", "menuServiceItemRenderer"] {
        let Some(text) = item.pointer(&format!("/{key}/text")) else {
            continue;
        };
        if let Some(s) = text.get("simpleText").and_then(|v| v.as_str()) {
            return s.to_string();
        }
        if let Some(runs) = text.get("runs").and_then(|v| v.as_array()) {
            return runs
                .iter()
                .filter_map(|r| r.get("text").and_then(|v| v.as_str()))
                .collect();
        }
    }
    String::new()
}

/// サブツリーを再帰探索し、最初に見つかった `feedbackToken` を返す（ネストされたポップアップの
/// 奥にあるケースに対応するため再帰にしている）。
fn find_feedback_token(v: &Value) -> Option<String> {
    match v {
        Value::Object(map) => {
            if let Some(t) = map.get("feedbackToken").and_then(|v| v.as_str()) {
                return Some(t.to_string());
            }
            map.values().find_map(find_feedback_token)
        }
        Value::Array(arr) => arr.iter().find_map(find_feedback_token),
        _ => None,
    }
}

/// 動画を「後で見る」に追加する。`playlistId: "WL"` は YouTube 標準の Watch Later 固定 ID
/// （youtube.js `PlaylistEditEndpoint` と同じ `browse/edit_playlist` を叩く。ペイロード形式は
/// tile が実際に持つ `playlistEditEndpoint` と同型で、動画ID以外は tile 非依存の汎用リクエスト）。
pub fn add_to_watch_later(access_token: &str, video_id: &str) -> Result<()> {
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(10)).build()?;
    let body = serde_json::json!({
        "context": { "client": {
            "clientName": "TVHTML5", "clientVersion": "7.20260114.12.00", "hl": "ja", "gl": "JP"
        }},
        "actions": [{ "action": "ACTION_ADD_VIDEO", "addedVideoId": video_id }],
        "playlistId": "WL"
    });
    client
        .post("https://www.youtube.com/youtubei/v1/browse/edit_playlist")
        .bearer_auth(access_token)
        .header("X-YouTube-Client-Name", "7")
        .header("X-YouTube-Client-Version", "7.20260114.12.00")
        .json(&body)
        .send()?
        .error_for_status()?;
    Ok(())
}

/// `feedbackToken` を送信する（興味なし／チャンネルをおすすめに表示しない等）。
/// リクエスト形式は youtube.js `FeedbackEndpoint.buildRequest()` と同型
/// （`actions` の再送は不要。トークン自体がサーバ側の動作を署名込みで内包している）。
pub fn send_feedback(access_token: &str, token: &str) -> Result<()> {
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(10)).build()?;
    let body = serde_json::json!({
        "context": { "client": {
            "clientName": "TVHTML5", "clientVersion": "7.20260114.12.00", "hl": "ja", "gl": "JP"
        }},
        "feedbackTokens": [token],
        "isFeedbackTokenUnencrypted": false,
        "shouldMerge": false
    });
    client
        .post("https://www.youtube.com/youtubei/v1/feedback")
        .bearer_auth(access_token)
        .header("X-YouTube-Client-Name", "7")
        .header("X-YouTube-Client-Version", "7.20260114.12.00")
        .json(&body)
        .send()?
        .error_for_status()?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recency_live_now_ranks_first() {
        assert_eq!(meta_recency_key("3.2万 人が視聴中", false), 0);
        assert_eq!(meta_recency_key("", true), 0);
    }

    #[test]
    fn recency_relative_time_units() {
        assert_eq!(meta_recency_key("6.1万回視聴•10 分前", false), 10 * 60);
        assert_eq!(meta_recency_key("6.1万回視聴•8 時間前 に配信済み", false), 8 * 3600);
        assert_eq!(meta_recency_key("14万回視聴•1 日前", false), 86400);
        assert_eq!(meta_recency_key("視聴•2 週間前", false), 2 * 604800);
        assert_eq!(meta_recency_key("視聴•3 か月前", false), 3 * 2_592_000);
        assert_eq!(meta_recency_key("視聴•4 年前", false), 4 * 31_536_000);
    }

    #[test]
    fn recency_multi_digit() {
        assert_eq!(meta_recency_key("15万回視聴•17 時間前 に配信済み", false), 17 * 3600);
    }

    #[test]
    fn recency_upcoming_and_unknown_go_last() {
        assert_eq!(meta_recency_key("2026/07/25 6:00 に公開予定", false), i64::MAX);
        assert_eq!(meta_recency_key("よくわからない meta", false), i64::MAX);
        assert_eq!(meta_recency_key("", false), i64::MAX);
    }

    fn shelf_json(title: Option<&str>, tile_ids_and_meta: &[(&str, &str)]) -> Value {
        let items: Vec<Value> = tile_ids_and_meta
            .iter()
            .map(|(id, meta)| {
                serde_json::json!({
                    "tileRenderer": {
                        "contentId": id,
                        "metadata": { "tileMetadataRenderer": {
                            "title": { "simpleText": format!("title-{id}") },
                            "lines": [
                                { "lineRenderer": { "items": [
                                    { "lineItemRenderer": { "text": { "simpleText": "channel" } } }
                                ]}},
                                { "lineRenderer": { "items": [
                                    { "lineItemRenderer": { "text": { "simpleText": *meta } } }
                                ]}},
                            ]
                        }}
                    }
                })
            })
            .collect();
        let mut shelf = serde_json::json!({
            "shelfRenderer": {
                "content": { "horizontalListRenderer": { "items": items }}
            }
        });
        if let Some(t) = title {
            shelf["shelfRenderer"]["headerRenderer"] = serde_json::json!({
                "shelfHeaderRenderer": {
                    "avatarLockup": { "avatarLockupRenderer": {
                        "title": { "runs": [ { "text": t } ] }
                    }}
                }
            });
        }
        shelf
    }

    fn build_subs_response(shelves: Vec<Value>) -> Value {
        serde_json::json!({
            "contents": { "tvBrowseRenderer": { "content": {
                "tvSecondaryNavRenderer": { "sections": [{
                    "tvSecondaryNavSectionRenderer": { "tabs": [{
                        "tabRenderer": {
                            "title": "すべて",
                            "selected": true,
                            "content": { "tvSurfaceContentRenderer": { "content": {
                                "sectionListRenderer": { "contents": shelves }
                            }}}
                        }
                    }]}
                }]}
            }}}
        })
    }

    fn ids(items: &[SubVideo]) -> Vec<String> {
        items.iter().map(|v| v.video_id.clone()).collect()
    }

    #[test]
    fn extract_skips_algorithmic_recommendation_shelf() {
        // 「関連が強い」shelf の 1 日前の動画 old0000000A が上位に来てはいけない。
        // 他 shelf の 8 時間前 fresh000000A が上に来ること。
        let resp = build_subs_response(vec![
            shelf_json(
                Some("関連が強い"),
                &[("old0000000A", "1 日前 に配信済み"), ("old0000000B", "1 日前")],
            ),
            shelf_json(None, &[("fresh000000A", "8 時間前 に配信済み")]),
        ]);
        assert_eq!(ids(&extract_videos(&resp).unwrap()), vec!["fresh000000A"]);
    }

    #[test]
    fn extract_sorts_across_shelves_newest_first() {
        // 実データ準拠のシナリオ: shelf 順は live-now → 6h → 1h → 上映予定 だが、
        // 出力は 0 → 3600 → 21600 → MAX で並ぶこと。
        let resp = build_subs_response(vec![
            shelf_json(None, &[("aaaaaaaaaaa", "6 時間前 に配信済み")]),
            shelf_json(None, &[
                ("bbbbbbbbbbb", "人が視聴中"),
                ("ccccccccccc", "1 時間前"),
                ("ddddddddddd", "2026/07/26 に公開予定"),
            ]),
        ]);
        assert_eq!(
            ids(&extract_videos(&resp).unwrap()),
            vec!["bbbbbbbbbbb", "ccccccccccc", "aaaaaaaaaaa", "ddddddddddd"]
        );
    }

    #[test]
    fn extract_dedups_across_shelves() {
        // 同じ video_id が複数 shelf に含まれても 1 度だけ出す。
        let resp = build_subs_response(vec![
            shelf_json(None, &[("dupe0000001", "3 時間前")]),
            shelf_json(None, &[("dupe0000001", "3 時間前"), ("uniq0000001", "1 時間前")]),
        ]);
        let out = extract_videos(&resp).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(ids(&out), vec!["uniq0000001", "dupe0000001"]);
    }
}
