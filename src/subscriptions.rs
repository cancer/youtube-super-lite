//! 登録チャンネルの新着動画取得。
//!
//! InnerTube `/youtubei/v1/browse?browseId=FEsubscriptions` を TVHTML5 client + OAuth で叩き、
//! 「すべて」タブの shelf から動画タイルを抽出する。
//!
//! 旧実装は YouTube Data API v3 `subscriptions.list` + 各チャンネルの RSS フィードを
//! 並列取得していたが、122 ch で 12 秒・50 ch でも 1.3 秒かかっていた。
//! こちらは 1 リクエストで完結するため大幅に高速。
//!
//! レスポンス構造:
//! contents.tvBrowseRenderer.content.tvSecondaryNavRenderer.sections[0]
//!  └ tvSecondaryNavSectionRenderer.tabs[0]   ← selected=true の「すべて」タブ
//!     └ tabRenderer.content.tvSurfaceContentRenderer.content.sectionListRenderer.contents[]
//!         └ shelfRenderer.content.horizontalListRenderer.items[]
//!             └ tileRenderer  (history.rs と同じ tile 構造)
//!
//! 同じ動画 ID が複数 shelf に含まれることがあるので、抽出時に dedup する。

use anyhow::{anyhow, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// 新着動画 1 件。history と同じ表示情報を持つ。
#[derive(Clone, Debug)]
pub struct SubVideo {
    pub video_id: String,
    pub title: String,
    pub channel: String,
    /// "mm:ss" など。サムネ右下バッジ用。
    pub duration: String,
    /// 「14万回視聴・17 時間前」など、メタ情報。
    pub meta: String,
    /// チャンネルアイコン URL (yt3.googleusercontent.com の sized 画像)。空のこともある。
    pub channel_icon: String,
}

/// 背景スレッドからメインスレッドへの通知。
pub enum SubUpdate {
    Items(Vec<SubVideo>),
    Error(String),
}

/// 登録チャンネルの新着動画を背景スレッドで取得する。
/// `log_timings` を立てると各段階の所要時間を stderr に出す（dev-tools での効果測定用）。
pub fn fetch_subscription_feed(
    access_token: &str,
    tx: &Sender<SubUpdate>,
    log_timings: bool,
) {
    match fetch_inner(access_token, log_timings) {
        Ok(items) => {
            let _ = tx.send(SubUpdate::Items(items));
        }
        Err(e) => {
            let _ = tx.send(SubUpdate::Error(e.to_string()));
        }
    }
}

fn fetch_inner(access_token: &str, log_timings: bool) -> Result<Vec<SubVideo>> {
    let t_total = Instant::now();

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
    let t_http = Instant::now();
    let resp = client
        .post("https://www.youtube.com/youtubei/v1/browse")
        .bearer_auth(access_token)
        .header("X-YouTube-Client-Name", "7")
        .header("X-YouTube-Client-Version", "7.20260114.12.00")
        .json(&body)
        .send()?
        .error_for_status()?;
    let v: Value = resp.json()?;
    if log_timings {
        eprintln!(
            "[subs] http+json: {} ms",
            t_http.elapsed().as_millis()
        );
    }

    let t_parse = Instant::now();
    let items = extract_videos(&v)?;
    if log_timings {
        eprintln!(
            "[subs] parse: {} ms ({} items)",
            t_parse.elapsed().as_millis(),
            items.len()
        );
        let with_icon = items.iter().filter(|i| !i.channel_icon.is_empty()).count();
        eprintln!("[subs] items with icon: {}/{}", with_icon, items.len());
        if let Some(first) = items.first() {
            eprintln!(
                "[subs] first item: channel={:?} icon={:?}",
                first.channel,
                first.channel_icon
            );
        }
        eprintln!(
            "[subs] fetch total: {} ms",
            t_total.elapsed().as_millis()
        );
    }
    Ok(items)
}

/// デバッグ用フラグ。マッピング不一致を切り分けるとき true にして実行する。
const DEBUG_ICONS: bool = false;

/// レスポンス JSON から「すべて」タブの動画タイルを抽出する。
/// 同じ video_id が複数 shelf に含まれるケースを dedup する。
fn extract_videos(v: &Value) -> Result<Vec<SubVideo>> {
    let tabs = v
        .pointer("/contents/tvBrowseRenderer/content/tvSecondaryNavRenderer/sections/0/tvSecondaryNavSectionRenderer/tabs")
        .and_then(|x| x.as_array())
        .ok_or_else(|| anyhow!("登録チャンネルの tabs を取れません"))?;

    // tabs[1..] は各チャンネルの専用タブ。それぞれの `thumbnail` がチャンネルアイコン。
    // チャンネル ID は browseId ではなく `endpoint.browseEndpoint.params` に
    // base64(protobuf) で埋め込まれている（browseId は "FEsubscriptions" 共通）。
    // base64 decode したバイト列から ASCII の `UC` で始まる 24 文字を取り出すと
    // それが正しい channel ID。tile からアイコンを引くためのマップを構築する。
    let mut icon_by_channel: HashMap<String, String> = HashMap::new();
    let mut dbg_no_tr = 0;
    let mut dbg_no_params = 0;
    let mut dbg_decode_fail = 0;
    let mut dbg_no_icon = 0;
    for (idx, t) in tabs.iter().enumerate() {
        let tr = match t.get("tabRenderer") {
            Some(x) => x,
            None => { dbg_no_tr += 1; continue }
        };
        let params = match tr
            .pointer("/endpoint/browseEndpoint/params")
            .and_then(|x| x.as_str())
        {
            Some(s) => s,
            None => { dbg_no_params += 1; continue }
        };
        let ch_id = match decode_channel_id_from_params(params) {
            Some(s) => s,
            None => { dbg_decode_fail += 1; continue }
        };
        // yt3.googleusercontent.com の URL は protocol-relative (`//yt3...`) で返ってくる
        // ことがある。egui の HTTP loader はスキーム必須なので https を補う。
        let icon = {
            let raw_url = tr
                .pointer("/thumbnail/thumbnails/0/url")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if raw_url.starts_with("//") {
                format!("https:{raw_url}")
            } else {
                raw_url.to_string()
            }
        };
        if icon.is_empty() {
            dbg_no_icon += 1;
            if DEBUG_ICONS && idx < 3 {
                eprintln!("[subs] tab[{}] no icon: thumbnail dump = {}",
                    idx,
                    serde_json::to_string(&tr.get("thumbnail").unwrap_or(&Value::Null)).unwrap_or_default());
            }
            continue;
        }
        icon_by_channel.insert(ch_id, icon);
    }
    if DEBUG_ICONS {
        eprintln!(
            "[subs] tab scan: no_tr={} no_params={} decode_fail={} no_icon={} mapped={}",
            dbg_no_tr, dbg_no_params, dbg_decode_fail, dbg_no_icon, icon_by_channel.len()
        );
    }
    if DEBUG_ICONS {
        eprintln!(
            "[subs] icon map size: {} (from {} tabs)",
            icon_by_channel.len(),
            tabs.len()
        );
        if let Some((k, v)) = icon_by_channel.iter().next() {
            eprintln!("[subs] icon sample: {} -> {}", k, &v[..v.len().min(80)]);
        }
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
            // メタ行は「14万回視聴・17 時間前」のように複数 item で組まれる。
            // line[1] の全 lineItem を連結する。
            let meta = extract_line(tile, 1);
            // line[0] の 1 つ目 lineItem の navigationEndpoint からチャンネル ID を得て、
            // tabs から作った map でアイコン URL を引く。
            let channel_id = tile
                .pointer("/metadata/tileMetadataRenderer/lines/0/lineRenderer/items/0/lineItemRenderer/text/runs/0/navigationEndpoint/browseEndpoint/browseId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let channel_icon = icon_by_channel
                .get(channel_id)
                .cloned()
                .unwrap_or_default();
            if DEBUG_ICONS && channel_icon.is_empty() && out.len() < 3 {
                eprintln!(
                    "[subs] no icon for channel_id={:?} channel={:?}",
                    channel_id, channel
                );
            }

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
/// (UC で始まる 24 文字の ASCII) が入っているので、それを正規表現相当の手書きで抽出する。
fn decode_channel_id_from_params(params: &str) -> Option<String> {
    use base64::Engine;
    // `%3D` 等の URL エスケープを戻す。標準ライブラリで簡易に。
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
    let raw = match base64::engine::general_purpose::URL_SAFE.decode(&unescaped) {
        Ok(r) => r,
        Err(e) => {
            if DEBUG_ICONS {
                eprintln!(
                    "[subs] base64 decode err for {:?}: {:?}",
                    &unescaped, e
                );
            }
            return None;
        }
    };
    // ASCII の連続部分から `UC` で始まる 24 文字（YouTube channel ID の形）を探す。
    // i は raw[i..i+24] にアクセスできる必要があるので i の最大値は raw.len() - 24。
    // range は半開区間なので `0..(len - 23)` で i ∈ [0, len-24] をカバーする。
    let n = raw.len();
    if n < 24 {
        return None;
    }
    for i in 0..(n - 23) {
        if raw[i] == b'U' && raw[i + 1] == b'C' {
            let slice = &raw[i..i + 24];
            if slice.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-') {
                return Some(std::str::from_utf8(slice).ok()?.to_string());
            }
        }
    }
    None
}

/// `metadata.tileMetadataRenderer.lines[line_idx]` の全 `lineItem` のテキストを連結する。
/// 「14万回視聴」「・」「17 時間前」のように複数 item に分かれているケースに対応。
fn extract_line(tile: &Value, line_idx: usize) -> String {
    let line = match tile
        .pointer(&format!("/metadata/tileMetadataRenderer/lines/{line_idx}/lineRenderer/items"))
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
