//! YouTube アカウントの再生履歴に動画を載せるための ping。
//!
//! 公式 Data API v3 には履歴に書き込む手段がなく、`/api/stats/playback` と
//! `/api/stats/watchtime` という InnerTube 内部エンドポイントを叩く必要がある。
//! 実装は yt-dlp の `_mark_watched` 関数を参考にした:
//! https://github.com/yt-dlp/yt-dlp/blob/master/yt_dlp/extractor/youtube/_video.py#L2273
//!
//! 手順:
//! 1. `/youtubei/v1/player` を **TVHTML5 クライアント**（client name 7）+ OAuth Bearer で叩く。
//!    WEB クライアントは OAuth を 400 で拒否し、TVHTML5 だけが Bearer を受理する。
//! 2. レスポンスから `playbackTracking.videostatsPlaybackUrl.baseUrl` と
//!    `videostatsWatchtimeUrl.baseUrl` を取り出す。これらは既にクエリで動画を識別する
//!    ei / docid / plid 等を含む完全 URL。
//! 3. CPN（Client Playback Nonce、16 文字ランダム）を生成して、`ver=2`, `cpn`, `cmt=<len-1>`,
//!    `el=detailpage` を追加（`el=leanback` の上書き）、watchtime には更に `st=0` / `et=<len-1>`。
//! 4. それぞれ GET（OAuth Bearer 付き）。両方 204 を返せば履歴に載る。

use anyhow::{anyhow, Result};
use rand::Rng;
use serde_json::Value;
use std::time::Duration;

const TV_CLIENT_NAME: &str = "TVHTML5";
const TV_CLIENT_VERSION: &str = "7.20260114.12.00";
const TV_CLIENT_NAME_NUM: &str = "7";
/// Cobalt (Smart TV ブラウザ) の UA。reqwest のデフォルト UA だと TVHTML5 と矛盾するので
/// 明示的に上書きする。yt-dlp の TVHTML5 クライアント定義から拝借。
const TV_USER_AGENT: &str =
    "Mozilla/5.0 (ChromiumStylePlatform) Cobalt/25.lts.30.1034943-gold (unlike Gecko), Unknown_TV_Unknown_0/Unknown (Unknown, Unknown)";
const CPN_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_";

/// 動画を再生履歴に載せる。OAuth トークンが必要。失敗しても致命的ではない。
pub fn mark_watched(access_token: &str, video_id: &str) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(TV_USER_AGENT)
        .build()?;

    let player_resp = fetch_player_response(&client, access_token, video_id)?;
    let pt = player_resp
        .get("playbackTracking")
        .ok_or_else(|| anyhow!("playbackTracking が player response にない（非公開動画？）"))?;

    let playback_url = pt
        .get("videostatsPlaybackUrl")
        .and_then(|v| v.get("baseUrl"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("videostatsPlaybackUrl.baseUrl がない"))?;
    let watchtime_url = pt
        .get("videostatsWatchtimeUrl")
        .and_then(|v| v.get("baseUrl"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("videostatsWatchtimeUrl.baseUrl がない"))?;

    let cpn = generate_cpn();
    send_ping(&client, access_token, playback_url, &cpn, false)?;
    send_ping(&client, access_token, watchtime_url, &cpn, true)?;
    Ok(())
}

fn fetch_player_response(
    client: &reqwest::blocking::Client,
    access_token: &str,
    video_id: &str,
) -> Result<Value> {
    let body = serde_json::json!({
        "context": {
            "client": {
                "clientName": TV_CLIENT_NAME,
                "clientVersion": TV_CLIENT_VERSION,
                "hl": "en",
                "gl": "US"
            }
        },
        "videoId": video_id
    });
    let resp = client
        .post("https://www.youtube.com/youtubei/v1/player")
        .bearer_auth(access_token)
        .header("X-YouTube-Client-Name", TV_CLIENT_NAME_NUM)
        .header("X-YouTube-Client-Version", TV_CLIENT_VERSION)
        .json(&body)
        .send()?
        .error_for_status()?;
    Ok(resp.json()?)
}

fn generate_cpn() -> String {
    let mut rng = rand::thread_rng();
    (0..16)
        .map(|_| {
            let i: usize = rng.gen_range(0..CPN_ALPHABET.len());
            CPN_ALPHABET[i] as char
        })
        .collect()
}

/// `baseUrl` のクエリに ver/cpn/cmt/el を追加・上書きして GET する。
/// `is_watchtime` のとき更に st=0 / et=<len-1> を追加する。
fn send_ping(
    client: &reqwest::blocking::Client,
    access_token: &str,
    base_url: &str,
    cpn: &str,
    is_watchtime: bool,
) -> Result<()> {
    let mut url = url::Url::parse(base_url)?;

    // 既存クエリから len を取り、yt-dlp に倣って cmt = len - 1 を作る。
    // len が無い・パース不能のときは 1.5（yt-dlp のフォールバック）。
    // Python の `str(float)` は常に小数点を含む（"80.0"）。Rust の `format!("{}", 80.0)` は
    // "80" になってしまうので `{:.1}` で揃える（YouTube サーバ側のパースに揺らぎが
    // あるかもしれないので yt-dlp と完全に同じ書式で送る）。
    let len_val: f64 = url
        .query_pairs()
        .find(|(k, _)| k == "len")
        .and_then(|(_, v)| v.parse::<f64>().ok())
        .unwrap_or(1.5);
    let cmt = format!("{:.1}", len_val - 1.0);

    // 既存の el を捨てて detailpage に置き換える。url::Url の query_pairs_mut は
    // 重複キーを許すため、一度 el を除外したペア列を再構築する必要がある。
    let preserved: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| k != "el" && k != "ver" && k != "cpn" && k != "cmt" && k != "st" && k != "et")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    {
        let mut qp = url.query_pairs_mut();
        qp.clear();
        for (k, v) in &preserved {
            qp.append_pair(k, v);
        }
        qp.append_pair("ver", "2");
        qp.append_pair("cpn", cpn);
        qp.append_pair("cmt", &cmt);
        qp.append_pair("el", "detailpage");
        if is_watchtime {
            qp.append_pair("st", "0");
            qp.append_pair("et", &cmt);
        }
    }

    eprintln!("[mark_watched] GET {}", url.as_str());
    let resp = client
        .get(url.as_str())
        .bearer_auth(access_token)
        .header("X-YouTube-Client-Name", TV_CLIENT_NAME_NUM)
        .header("X-YouTube-Client-Version", TV_CLIENT_VERSION)
        .send()?;
    let status = resp.status();
    let body = resp.text().unwrap_or_default();
    eprintln!("[mark_watched]   -> {status} body_len={}", body.len());
    if !status.is_success() {
        return Err(anyhow!("ping 失敗: HTTP {status} body={}", body));
    }
    Ok(())
}
