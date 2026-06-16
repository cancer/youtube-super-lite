//! U7 feasibility スパイク: **ログイン状態(OAuth bearer)で members限定/年齢制限が解錠できるか**を実測する。
//!
//! 新 MUST: 「ログインしている状態では members限定・年齢制限も視聴できる」。
//! 現状アプリ(src/auth.rs)が保持するのは cookie ではなく OAuth2 アクセストークン
//! (scope=youtube.force-ssl, Google OAuth bearer)。yt-dlp は OAuth を廃止し cookie 方式へ移行済みで、
//! 「Data API スコープの bearer が InnerTube player 認証で通るか」は不確実 → ここで見極める。
//!
//! 手順:
//!   1. %APPDATA%\YouTubeSuperLite\auth.json の refresh_token を読む。
//!   2. backend(/refresh) で access_token に更新（トークンは一切出力しない）。
//!   3. Authorization: Bearer を付けて youtubei/v1/player を各 client で叩き、
//!      members(sbs4fauCXco)/age(HtVdAasjOgU) が playabilityStatus=OK + streamingData を返すか確認。
//!   4. 解錠できた client では直リンクを Range GET して 200/206 を確認。
//!
//! 注意: members 動画はログイン中アカウントが当該チャンネルのメンバーである場合のみ解錠される。
//!
//! 使い方:
//!   cargo run --bin u7_auth_probe                 # 既定(members + age)
//!   cargo run --bin u7_auth_probe -- <id|url> ... # 任意動画

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::time::Duration;

const PLAYER_ENDPOINT: &str = "https://www.youtube.com/youtubei/v1/player?prettyPrint=false";
const DEFAULT_BACKEND: &str = "https://youtube-super-lite-backend.cancer6.workers.dev";

struct ClientDef {
    key: &'static str,
    client_name: &'static str,
    client_version: &'static str,
    client_name_id: u32,
    user_agent: &'static str,
    extra_client: Value,
}

/// bearer 認証で試す価値のある client 群（web系/tv系=従来 cookie 認証が効く層 + 無認証で取れた mobile 系）。
fn clients() -> Vec<ClientDef> {
    vec![
        ClientDef {
            key: "web",
            client_name: "WEB",
            client_version: "2.20260114.08.00",
            client_name_id: 1,
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36",
            extra_client: json!({}),
        },
        ClientDef {
            key: "web_embedded",
            client_name: "WEB_EMBEDDED_PLAYER",
            client_version: "1.20260115.01.00",
            client_name_id: 56,
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36",
            extra_client: json!({}),
        },
        ClientDef {
            key: "tv",
            client_name: "TVHTML5",
            client_version: "7.20260114.12.00",
            client_name_id: 7,
            user_agent: "Mozilla/5.0 (ChromiumStylePlatform) Cobalt/Version",
            extra_client: json!({}),
        },
        ClientDef {
            key: "mweb",
            client_name: "MWEB",
            client_version: "2.20260115.01.00",
            client_name_id: 2,
            user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 18_3_2 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1",
            extra_client: json!({}),
        },
        ClientDef {
            key: "android",
            client_name: "ANDROID",
            client_version: "21.02.35",
            client_name_id: 3,
            user_agent: "com.google.android.youtube/21.02.35 (Linux; U; Android 11) gzip",
            extra_client: json!({"osName":"Android","osVersion":"11","androidSdkVersion":30}),
        },
        ClientDef {
            key: "ios",
            client_name: "IOS",
            client_version: "21.02.3",
            client_name_id: 5,
            user_agent: "com.google.ios.youtube/21.02.3 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X)",
            extra_client: json!({"deviceMake":"Apple","deviceModel":"iPhone16,2","osName":"iPhone","osVersion":"18.3.2.22D82"}),
        },
        ClientDef {
            key: "android_vr",
            client_name: "ANDROID_VR",
            client_version: "1.65.10",
            client_name_id: 28,
            user_agent: "com.google.android.apps.youtube.vr.oculus/1.65.10 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip",
            extra_client: json!({"deviceMake":"Oculus","deviceModel":"Quest 3","osName":"Android","osVersion":"12L","androidSdkVersion":32}),
        },
    ]
}

fn auth_json_path() -> std::path::PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(base).join("YouTubeSuperLite").join("auth.json")
}

/// refresh_token を読み、backend で access_token に更新する。トークンは出力しない。
fn get_access_token(http: &reqwest::blocking::Client) -> Result<String> {
    let path = auth_json_path();
    let data = std::fs::read_to_string(&path)
        .map_err(|_| anyhow!("auth.json が読めません({})。先にアプリでログインしてください。", path.display()))?;
    let v: Value = serde_json::from_str(&data)?;
    let refresh = v["refresh_token"].as_str().ok_or_else(|| anyhow!("refresh_token がありません"))?;

    let backend = std::env::var("TLV_BACKEND").unwrap_or_else(|_| DEFAULT_BACKEND.to_string());
    let resp = http
        .post(format!("{backend}/refresh"))
        .json(&json!({ "refresh_token": refresh }))
        .send()
        .map_err(|_| anyhow!("認証バックエンドに接続できません"))?;
    let tr: Value = resp.json()?;
    if let Some(err) = tr["error"].as_str() {
        bail!("トークン更新エラー: {err}");
    }
    tr["access_token"].as_str().map(str::to_string).ok_or_else(|| anyhow!("access_token が返りませんでした"))
}

fn extract_video_id(input: &str) -> String {
    let input = input.trim();
    if input.len() == 11 && !input.contains('/') && !input.contains('=') {
        return input.to_string();
    }
    for marker in ["watch?v=", "youtu.be/", "shorts/", "live/", "embed/", "/v/"] {
        if let Some(pos) = input.find(marker) {
            let rest = &input[pos + marker.len()..];
            let id: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
            if id.len() >= 11 {
                return id[..11].to_string();
            }
        }
    }
    input.to_string()
}

fn build_body(def: &ClientDef, video_id: &str) -> Value {
    let mut client = json!({"clientName": def.client_name, "clientVersion": def.client_version, "hl": "en", "gl": "US"});
    if let (Some(obj), Some(extra)) = (client.as_object_mut(), def.extra_client.as_object()) {
        for (k, v) in extra { obj.insert(k.clone(), v.clone()); }
    }
    json!({"context": {"client": client}, "videoId": video_id, "contentCheckOk": true, "racyCheckOk": true})
}

/// player を1回叩いて (HTTPステータス, playabilityStatus, streamingData有無, direct_url数, 最初のvideo直リンク) を返す。
fn call_player(
    http: &reqwest::blocking::Client,
    def: &ClientDef,
    video_id: &str,
    access_token: Option<&str>,
) -> (String, String, bool, usize, Option<String>) {
    let body = build_body(def, video_id);
    let mut req = http
        .post(PLAYER_ENDPOINT)
        .header("Content-Type", "application/json")
        .header("User-Agent", def.user_agent)
        .header("X-Youtube-Client-Name", def.client_name_id.to_string())
        .header("X-Youtube-Client-Version", def.client_version)
        .header("Origin", "https://www.youtube.com");
    if let Some(tok) = access_token {
        req = req.bearer_auth(tok);
    }
    let resp = match req.json(&body).send() {
        Ok(r) => r,
        Err(e) => return (format!("POST失敗:{e}"), String::new(), false, 0, None),
    };
    let http_status = resp.status().to_string();
    let text = resp.text().unwrap_or_default();
    let val: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return (http_status, "JSON解析失敗".to_string(), false, 0, None),
    };
    let play = val["playabilityStatus"]["status"].as_str().unwrap_or("?").to_string();
    let reason = val["playabilityStatus"]["reason"].as_str().unwrap_or("");
    let play = if reason.is_empty() { play } else { format!("{play} ({reason})") };

    let streaming = val.get("streamingData");
    let mut direct = 0usize;
    let mut sample_video = None;
    if let Some(s) = streaming {
        let empty = vec![];
        let adaptive = s.get("adaptiveFormats").and_then(Value::as_array).unwrap_or(&empty);
        let prog = s.get("formats").and_then(Value::as_array).unwrap_or(&empty);
        for f in adaptive.iter().chain(prog.iter()) {
            if let Some(u) = f.get("url").and_then(Value::as_str) {
                direct += 1;
                let mime = f.get("mimeType").and_then(Value::as_str).unwrap_or("");
                if mime.starts_with("video/") && sample_video.is_none() {
                    sample_video = Some(u.to_string());
                }
            }
        }
    }
    (http_status, play, streaming.is_some(), direct, sample_video)
}

fn probe_url(http: &reqwest::blocking::Client, ua: &str, url: &str, bearer: Option<&str>) -> String {
    let mut req = http.get(url).header("User-Agent", ua).header("Range", "bytes=0-1");
    if let Some(t) = bearer {
        req = req.bearer_auth(t);
    }
    match req.send() {
        Ok(r) => {
            let s = r.status().as_u16();
            let v = match s { 200 | 206 => "OK(再生可)", 403 => "403(要token)", _ => "他" };
            format!("HTTP {s} {v}")
        }
        Err(e) => format!("GET失敗:{e}"),
    }
}

/// stream URL のクエリから診断フラグを抜く（403 の原因切り分け用）。
fn url_flags(url: &str) -> String {
    let has = |k: &str| url.contains(k);
    format!(
        "n={} pot={} sig={} ratebypass={} expire={}",
        yn(has("&n=") || has("?n=")),
        yn(has("pot=")),
        yn(has("&sig=") || has("&lsig=")),
        yn(has("ratebypass")),
        yn(has("expire=")),
    )
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let videos: Vec<(String, String)> = if args.is_empty() {
        vec![
            ("メンバー限定".to_string(), "sbs4fauCXco".to_string()),
            ("年齢制限".to_string(), "HtVdAasjOgU".to_string()),
        ]
    } else {
        args.iter().map(|a| ("指定".to_string(), extract_video_id(a))).collect()
    };

    println!("=== U7 認証付き player probe (members/age 解錠の可否) ===\n");

    let http = reqwest::blocking::Client::builder().timeout(Duration::from_secs(30)).build()?;

    print!("OAuth access_token を取得中... ");
    let token = match get_access_token(&http) {
        Ok(t) => { println!("OK (scope=youtube.force-ssl, bearer)"); t }
        Err(e) => { println!("失敗"); return Err(e); }
    };

    for (label, id) in &videos {
        println!("\n╔══════════════════════════════════════════════════════");
        println!("║ {label} (video_id={id})");
        println!("╚══════════════════════════════════════════════════════");
        for def in clients() {
            // 無認証(baseline)と Bearer認証を比較。
            let (_hs0, play0, sd0, _d0, _v0) = call_player(&http, &def, id, None);
            let (hs1, play1, sd1, d1, v1) = call_player(&http, &def, id, Some(&token));
            println!("[{:11}] 無認証: sd={} {}", def.key, yn(sd0), play0);
            println!("              Bearer : HTTP {hs1} sd={} direct_url={d1} {}", yn(sd1), play1);
            if let Some(vurl) = &v1 {
                println!("              └ url flags: {}", url_flags(vurl));
                let plain = probe_url(&http, def.user_agent, vurl, None);
                let authed = probe_url(&http, def.user_agent, vurl, Some(&token));
                println!("              └ video直リンク: 無認証GET={plain} / BearerGET={authed}");
            }
        }
    }

    println!("\n════════════ U7 結論 ════════════");
    println!("各動画で Bearer 時に sd=✅(streamingData有) になった client があれば、");
    println!("既存 OAuth ログインだけで members/age を解錠でき、新MUSTを満たせる。");
    println!("全て sd=❌ なら OAuth bearer では不可 → cookie(SAPISID)方式の検討が必要。");
    Ok(())
}

fn yn(b: bool) -> &'static str { if b { "✅" } else { "❌" } }
