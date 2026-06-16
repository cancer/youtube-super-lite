//! InnerTube `player` 呼び出しと、client context / フォーマット選択。
//!
//! PoC(U1/U7)で確定した経路:
//!   - 匿名 VOD     : ANDROID_VR（署名/nsig 不要・2160p adaptive）
//!   - 匿名 ライブ  : ANDROID（hlsManifestUrl）
//!   - 認証(members/年齢制限): TVHTML5 + OAuth Bearer（解錠。URL は nsig 変換が要る）
//!
//! client 定義は yt-dlp INNERTUBE_CLIENTS(2026-01) 準拠。バージョンは保守対象(U4)。

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::{Codec, Quality};

const PLAYER_ENDPOINT: &str = "https://www.youtube.com/youtubei/v1/player?prettyPrint=false";

/// InnerTube client 定義。
pub struct ClientDef {
    pub key: &'static str,
    client_name: &'static str,
    client_version: &'static str,
    client_name_id: u32,
    user_agent: &'static str,
    /// context.client に追加する device/os フィールド（JSON オブジェクト文字列）。
    extra_client: &'static str,
    /// この client は OAuth Bearer を付けて呼ぶ（認証経路）。
    pub use_bearer: bool,
}

pub const ANDROID_VR: ClientDef = ClientDef {
    key: "android_vr",
    client_name: "ANDROID_VR",
    client_version: "1.65.10",
    client_name_id: 28,
    user_agent: "com.google.android.apps.youtube.vr.oculus/1.65.10 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip",
    extra_client: r#"{"deviceMake":"Oculus","deviceModel":"Quest 3","osName":"Android","osVersion":"12L","androidSdkVersion":32}"#,
    use_bearer: false,
};

pub const ANDROID: ClientDef = ClientDef {
    key: "android",
    client_name: "ANDROID",
    client_version: "21.02.35",
    client_name_id: 3,
    user_agent: "com.google.android.youtube/21.02.35 (Linux; U; Android 11) gzip",
    extra_client: r#"{"osName":"Android","osVersion":"11","androidSdkVersion":30}"#,
    use_bearer: false,
};

pub const TVHTML5: ClientDef = ClientDef {
    key: "tv",
    client_name: "TVHTML5",
    client_version: "7.20260114.12.00",
    client_name_id: 7,
    user_agent: "Mozilla/5.0 (ChromiumStylePlatform) Cobalt/Version",
    extra_client: "{}",
    use_bearer: true,
};

/// player レスポンスから取り出した必要情報。
pub struct PlayerInfo {
    pub status: String,
    pub title: Option<String>,
    pub is_live: bool,
    /// streamingData（無い＝再生不可）。
    pub streaming: Option<Value>,
}

/// URL または raw ID から videoId(11 文字)を抽出する。
pub fn extract_video_id(input: &str) -> Option<String> {
    let input = input.trim();
    let valid = |id: &str| id.len() == 11 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if valid(input) {
        return Some(input.to_string());
    }
    for marker in ["watch?v=", "youtu.be/", "shorts/", "live/", "embed/", "/v/"] {
        if let Some(pos) = input.find(marker) {
            let rest = &input[pos + marker.len()..];
            let id: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            if id.len() >= 11 {
                return Some(id[..11].to_string());
            }
        }
    }
    None
}

/// 指定 client で player を叩き、必要情報を取り出す。
pub fn fetch_player(
    http: &reqwest::blocking::Client,
    def: &ClientDef,
    video_id: &str,
    access_token: Option<&str>,
) -> Result<PlayerInfo> {
    let mut client = json!({
        "clientName": def.client_name,
        "clientVersion": def.client_version,
        "hl": "en",
        "gl": "US",
    });
    if let Ok(Value::Object(extra)) = serde_json::from_str::<Value>(def.extra_client) {
        if let Some(obj) = client.as_object_mut() {
            for (k, v) in extra {
                obj.insert(k, v);
            }
        }
    }
    let body = json!({
        "context": { "client": client },
        "videoId": video_id,
        "contentCheckOk": true,
        "racyCheckOk": true,
    });

    let mut req = http
        .post(PLAYER_ENDPOINT)
        .header("Content-Type", "application/json")
        .header("User-Agent", def.user_agent)
        .header("X-Youtube-Client-Name", def.client_name_id.to_string())
        .header("X-Youtube-Client-Version", def.client_version)
        .header("Origin", "https://www.youtube.com");
    if def.use_bearer {
        if let Some(tok) = access_token {
            req = req.bearer_auth(tok);
        }
    }

    let resp = req.json(&body).send()?;
    let text = resp.text()?;
    let val: Value = serde_json::from_str(&text)
        .map_err(|e| anyhow!("player レスポンス解析失敗({}): {e}", def.key))?;

    let status = val["playabilityStatus"]["status"]
        .as_str()
        .unwrap_or("?")
        .to_string();
    let title = val["videoDetails"]["title"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let is_live = val["videoDetails"]["isLive"].as_bool().unwrap_or(false);
    let streaming = val.get("streamingData").cloned();

    Ok(PlayerInfo {
        status,
        title,
        is_live,
        streaming,
    })
}

/// streamingData から hlsManifestUrl を取り出す（ライブ用・M13）。
pub fn hls_manifest(streaming: &Value) -> Option<String> {
    streaming
        .get("hlsManifestUrl")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// 1 つの format から再生 URL を取り出す（url 直書き or signatureCipher の url 部）。
/// nsig 変換は呼び出し側が後段で適用する。署名(sig)は解錠 URL では適用済み。
fn format_url(f: &Value) -> Option<String> {
    if let Some(u) = f.get("url").and_then(Value::as_str) {
        return Some(u.to_string());
    }
    // signatureCipher の場合は url= 部分を取り出す（署名適用は別途必要だが本アプリの経路では未使用）。
    if let Some(sc) = f.get("signatureCipher").and_then(Value::as_str) {
        for pair in sc.split('&') {
            if let Some(v) = pair.strip_prefix("url=") {
                return Some(crate::resolve::urldecode(v));
            }
        }
    }
    None
}

/// mimeType の codecs がコーデック指定にマッチするか。
fn codec_matches(mime: &str, codec: Codec) -> bool {
    match codec {
        Codec::Auto => true,
        Codec::H264 => mime.contains("avc1"),
        Codec::Vp9 => mime.contains("vp09") || mime.contains("vp9"),
        Codec::Av1 => mime.contains("av01"),
    }
}

/// adaptiveFormats から Quality/Codec に沿って video+audio を選ぶ（M12）。
/// muxed しか無い場合は (video_url, None) を返す。
pub fn select_streams(
    streaming: &Value,
    quality: Quality,
    codec: Codec,
) -> Result<(String, Option<String>)> {
    let empty = vec![];
    let adaptive = streaming
        .get("adaptiveFormats")
        .and_then(Value::as_array)
        .unwrap_or(&empty);

    // video / audio に分ける。
    let videos: Vec<&Value> = adaptive
        .iter()
        .filter(|f| {
            f["mimeType"].as_str().map(|m| m.starts_with("video/")).unwrap_or(false)
        })
        .collect();
    let audios: Vec<&Value> = adaptive
        .iter()
        .filter(|f| {
            f["mimeType"].as_str().map(|m| m.starts_with("audio/")).unwrap_or(false)
        })
        .collect();

    if !videos.is_empty() && !audios.is_empty() {
        let max_h = quality.height();
        let height_ok = |f: &&Value| max_h.map_or(true, |h| f["height"].as_i64().unwrap_or(0) as u32 <= h);
        let codec_ok = |f: &&Value| {
            f["mimeType"].as_str().map(|m| codec_matches(m, codec)).unwrap_or(false)
        };
        let bitrate = |f: &&Value| f["bitrate"].as_i64().unwrap_or(0);

        // 段階的フォールバック: height+codec → height のみ → 制約なし。
        let pick_video = |filt: &dyn Fn(&&Value) -> bool| -> Option<&Value> {
            videos
                .iter()
                .filter(|f| filt(f))
                .max_by_key(|f| (f["height"].as_i64().unwrap_or(0), bitrate(f)))
                .copied()
        };

        let video = pick_video(&|f| height_ok(f) && codec_ok(f))
            .or_else(|| pick_video(&|f| height_ok(f)))
            .or_else(|| pick_video(&|_| true))
            .ok_or_else(|| anyhow!("video format が選べません"))?;

        // audio: ビットレート最大（言語トラックは default を優先したいが簡略化）。
        let audio = audios
            .iter()
            .max_by_key(|f| bitrate(f))
            .copied()
            .ok_or_else(|| anyhow!("audio format が選べません"))?;

        let vurl = format_url(video).ok_or_else(|| anyhow!("video URL が取れません"))?;
        let aurl = format_url(audio).ok_or_else(|| anyhow!("audio URL が取れません"))?;
        return Ok((vurl, Some(aurl)));
    }

    // muxed(formats[]) フォールバック。
    let progressive = streaming
        .get("formats")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    if let Some(best) = progressive
        .iter()
        .max_by_key(|f| f["height"].as_i64().unwrap_or(0))
    {
        if let Some(u) = format_url(best) {
            return Ok((u, None));
        }
    }

    bail!("再生可能な format がありません")
}
