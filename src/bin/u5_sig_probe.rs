//! U5 feasibility スパイク Phase A（診断）: TVHTML5+OAuth Bearer が返す認証付き format の
//! 実構造を dump し、403 を 200 にするのに必要な処理（署名適用 / nsig 変換）を確定する。
//!
//! 本タスク U5: TVHTML5(JS player必須)経路で解錠された stream URL は sig/n を持ち未処理だと 403。
//! Phase A では「url 直書きか signatureCipher か」「url クエリに n / sig / sp / pot 等が有るか」を実際に見る。
//! Phase B（別途）で base.js から署名/nsig 関数を抽出し boa_engine で適用して 403→200 を確認する。
//!
//! 使い方: cargo run --bin u5_sig_probe -- [videoId|url]   (既定: sbs4fauCXco メンバー限定)

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::time::Duration;

const PLAYER_ENDPOINT: &str = "https://www.youtube.com/youtubei/v1/player?prettyPrint=false";
const DEFAULT_BACKEND: &str = "https://youtube-super-lite-backend.cancer6.workers.dev";

fn auth_json_path() -> std::path::PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(base).join("YouTubeSuperLite").join("auth.json")
}

fn get_access_token(http: &reqwest::blocking::Client) -> Result<String> {
    let path = auth_json_path();
    let data = std::fs::read_to_string(&path)
        .map_err(|_| anyhow!("auth.json が読めません({})", path.display()))?;
    let v: Value = serde_json::from_str(&data)?;
    let refresh = v["refresh_token"].as_str().ok_or_else(|| anyhow!("refresh_token なし"))?;
    let backend = std::env::var("TLV_BACKEND").unwrap_or_else(|_| DEFAULT_BACKEND.to_string());
    let resp = http.post(format!("{backend}/refresh"))
        .json(&json!({ "refresh_token": refresh })).send()
        .map_err(|_| anyhow!("backend 接続失敗"))?;
    let tr: Value = resp.json()?;
    tr["access_token"].as_str().map(str::to_string).ok_or_else(|| anyhow!("access_token なし"))
}

fn extract_video_id(input: &str) -> String {
    let input = input.trim();
    if input.len() == 11 && !input.contains('/') && !input.contains('=') {
        return input.to_string();
    }
    for marker in ["watch?v=", "youtu.be/", "shorts/", "live/", "embed/"] {
        if let Some(pos) = input.find(marker) {
            let rest = &input[pos + marker.len()..];
            let id: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
            if id.len() >= 11 { return id[..11].to_string(); }
        }
    }
    input.to_string()
}

/// TVHTML5 + Bearer で player を叩く。
fn tv_player(http: &reqwest::blocking::Client, token: &str, video_id: &str) -> Result<Value> {
    let body = json!({
        "context": {"client": {"clientName": "TVHTML5", "clientVersion": "7.20260114.12.00", "hl": "en", "gl": "US"}},
        "videoId": video_id, "contentCheckOk": true, "racyCheckOk": true
    });
    let resp = http.post(PLAYER_ENDPOINT)
        .header("Content-Type", "application/json")
        .header("User-Agent", "Mozilla/5.0 (ChromiumStylePlatform) Cobalt/Version")
        .header("X-Youtube-Client-Name", "7")
        .header("X-Youtube-Client-Version", "7.20260114.12.00")
        .header("Origin", "https://www.youtube.com")
        .bearer_auth(token)
        .json(&body).send()?;
    let text = resp.text()?;
    let val: Value = serde_json::from_str(&text)?;
    if val.get("streamingData").is_none() {
        bail!("streamingData なし: {}", val["playabilityStatus"]["status"]);
    }
    Ok(val)
}

/// URL のクエリパラメータ名を列挙する。
fn query_keys(url: &str) -> Vec<String> {
    let q = url.split('?').nth(1).unwrap_or("");
    q.split('&').filter_map(|p| p.split('=').next().map(str::to_string)).filter(|s| !s.is_empty()).collect()
}

fn main() -> Result<()> {
    let arg = std::env::args().nth(1).unwrap_or_else(|| "sbs4fauCXco".to_string());
    let video_id = extract_video_id(&arg);
    println!("=== U5 Phase A: TVHTML5+Bearer 認証 format 診断 (video_id={video_id}) ===\n");

    let http = reqwest::blocking::Client::builder().timeout(Duration::from_secs(30)).build()?;
    let token = get_access_token(&http)?;
    let player = tv_player(&http, &token, &video_id)?;

    // base.js の URL ヒント（player レスポンスに assets があるか）。
    if let Some(js) = player["assets"]["js"].as_str() {
        println!("assets.js (base.js URL hint): {js}");
    } else {
        println!("assets.js: なし（base.js URL は watch ページ/iframe_api から取得が必要）");
    }
    println!();

    let streaming = &player["streamingData"];
    let empty = vec![];
    let adaptive = streaming["adaptiveFormats"].as_array().unwrap_or(&empty);
    println!("adaptiveFormats 数: {}", adaptive.len());

    // 最初の video format と最初の audio format を詳しく見る。
    for kind in ["video", "audio"] {
        if let Some(f) = adaptive.iter().find(|f| {
            f["mimeType"].as_str().map(|m| m.starts_with(kind)).unwrap_or(false)
        }) {
            println!("\n──── 最初の {kind} format ────");
            let itag = f["itag"].as_i64().unwrap_or(0);
            let mime = f["mimeType"].as_str().unwrap_or("");
            println!("itag={itag} mime={mime}");
            // キー一覧。
            if let Some(obj) = f.as_object() {
                let keys: Vec<&str> = obj.keys().map(String::as_str).collect();
                println!("keys: {keys:?}");
            }
            // url 直書きか signatureCipher か。
            if let Some(url) = f["url"].as_str() {
                println!("形態: url 直書き");
                println!("  query keys: {:?}", query_keys(url));
                println!("  has n=  : {}", url.contains("&n=") || url.contains("?n="));
                println!("  has sig=: {}", url.contains("&sig="));
                println!("  has pot=: {}", url.contains("pot="));
            } else if let Some(sc) = f["signatureCipher"].as_str() {
                println!("形態: signatureCipher（要署名復号）");
                // signatureCipher は urlencoded な s=...&sp=...&url=... 。
                let mut s_present = false; let mut sp = String::new(); let mut inner_url = String::new();
                for pair in sc.split('&') {
                    let mut it = pair.splitn(2, '=');
                    let k = it.next().unwrap_or(""); let v = it.next().unwrap_or("");
                    match k { "s" => s_present = true, "sp" => sp = v.to_string(), "url" => inner_url = v.to_string(), _ => {} }
                }
                println!("  s(署名)有: {s_present}  sp(param名): {sp}");
                // inner url を urldecode して n の有無を見る。
                let dec = urldecode(&inner_url);
                println!("  inner url query keys: {:?}", query_keys(&dec));
                println!("  inner has n=: {}", dec.contains("&n=") || dec.contains("?n="));
            } else {
                println!("形態: 不明（url も signatureCipher も無い）");
            }
        }
    }

    println!("\n→ この診断で必要処理が確定: ");
    println!("  ・signatureCipher なら s を base.js 署名関数で復号→&sp=値 で付与(M9)");
    println!("  ・url/inner に n= があれば base.js nsig 関数で変換(M10)");
    println!("  Phase B で boa_engine による抽出/実行/適用→403が200になるか検証する。");
    Ok(())
}

fn urldecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i+1..i+3]).unwrap_or(""), 16) {
                    out.push(v); i += 3;
                } else { out.push(b[i]); i += 1; }
            }
            b'+' => { out.push(b' '); i += 1; }
            x => { out.push(x); i += 1; }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
