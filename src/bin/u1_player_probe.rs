//! U1 feasibility スパイク: InnerTube `player` を複数 client context で叩き、
//! **PoToken 無しで再生用の直リンクが取れる client** を実測で特定する。
//!
//! inbox/native-resolver-spec.md の U1（最大の不確実性）。web client は PoToken 無しだと
//! 403/throttle になることがある。どの client context（web / tv / ios / android / android_vr /
//! web_embedded 等）なら token 無しでストリーム URL が 200/206 で取れるかをここで見極める。
//!
//! client 定義は yt-dlp の INNERTUBE_CLIENTS（2026-01 時点）に準拠。重要な含意:
//!   - android_vr / ios / android は REQUIRE_JS_PLAYER=False → 署名/nsig 不要。
//!     これらが token 無しで直リンクを返すなら U5(boa で nsig) すら不要になり得る。
//!   - web / tv は REQUIRE_JS_PLAYER=True → formats は signatureCipher で署名復号が要る(U1では復号しない)。
//!
//! 各 client について以下を報告する:
//!   - player POST の HTTP ステータス
//!   - playabilityStatus.status / reason
//!   - streamingData の formats / adaptiveFormats 数
//!   - adaptiveFormats のうち url 直書き vs signatureCipher の内訳
//!   - url 直書きの video/audio format に Range GET して 200/206(=再生可) か 403(=要token) か
//!
//! 使い方:
//!   cargo run --bin u1_player_probe                 # 既定のテスト動画(dQw4w9WgXcQ)
//!   cargo run --bin u1_player_probe -- <videoIdまたはURL>

use anyhow::Result;
use serde_json::{json, Value};
use std::io::Read;
use std::time::{Duration, Instant};

const PLAYER_ENDPOINT: &str = "https://www.youtube.com/youtubei/v1/player?prettyPrint=false";

/// テスト対象 client。yt-dlp INNERTUBE_CLIENTS(2026-01) 準拠。
struct ClientDef {
    /// 表示名（probe 出力用）。
    key: &'static str,
    /// context.client.clientName。
    client_name: &'static str,
    /// context.client.clientVersion。
    client_version: &'static str,
    /// X-Youtube-Client-Name ヘッダ値。
    client_name_id: u32,
    /// HTTP User-Agent（android/ios は専用 UA でないと弾かれる）。
    user_agent: &'static str,
    /// context.client に追加で載せるフィールド（device/os 等）。
    extra_client: Value,
    /// この client が JS player(署名/nsig)を必要とするか（U5 要否の判断材料）。
    require_js: bool,
}

fn clients() -> Vec<ClientDef> {
    vec![
        ClientDef {
            key: "web",
            client_name: "WEB",
            client_version: "2.20260114.08.00",
            client_name_id: 1,
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36",
            extra_client: json!({}),
            require_js: true,
        },
        ClientDef {
            key: "tv",
            client_name: "TVHTML5",
            client_version: "7.20260114.12.00",
            client_name_id: 7,
            user_agent: "Mozilla/5.0 (ChromiumStylePlatform) Cobalt/Version",
            extra_client: json!({}),
            require_js: true,
        },
        ClientDef {
            key: "tv_simply",
            client_name: "TVHTML5_SIMPLY",
            client_version: "1.0",
            client_name_id: 75,
            user_agent: "Mozilla/5.0 (ChromiumStylePlatform) Cobalt/Version",
            extra_client: json!({}),
            require_js: true,
        },
        ClientDef {
            key: "ios",
            client_name: "IOS",
            client_version: "21.02.3",
            client_name_id: 5,
            user_agent: "com.google.ios.youtube/21.02.3 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X)",
            extra_client: json!({
                "deviceMake": "Apple",
                "deviceModel": "iPhone16,2",
                "osName": "iPhone",
                "osVersion": "18.3.2.22D82"
            }),
            require_js: false,
        },
        ClientDef {
            key: "android",
            client_name: "ANDROID",
            client_version: "21.02.35",
            client_name_id: 3,
            user_agent: "com.google.android.youtube/21.02.35 (Linux; U; Android 11) gzip",
            extra_client: json!({
                "osName": "Android",
                "osVersion": "11",
                "androidSdkVersion": 30
            }),
            require_js: false,
        },
        ClientDef {
            key: "android_vr",
            client_name: "ANDROID_VR",
            client_version: "1.65.10",
            client_name_id: 28,
            user_agent: "com.google.android.apps.youtube.vr.oculus/1.65.10 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip",
            extra_client: json!({
                "deviceMake": "Oculus",
                "deviceModel": "Quest 3",
                "osName": "Android",
                "osVersion": "12L",
                "androidSdkVersion": 32
            }),
            require_js: false,
        },
        ClientDef {
            key: "web_embedded",
            client_name: "WEB_EMBEDDED_PLAYER",
            client_version: "1.20260115.01.00",
            client_name_id: 56,
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36",
            extra_client: json!({}),
            require_js: true,
        },
        ClientDef {
            key: "mweb",
            client_name: "MWEB",
            client_version: "2.20260115.01.00",
            client_name_id: 2,
            user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 18_3_2 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1",
            extra_client: json!({}),
            require_js: true,
        },
    ]
}

/// URL または raw ID から videoId を抽出する。
fn extract_video_id(input: &str) -> String {
    let input = input.trim();
    // 11 文字の素の ID ならそのまま。
    if input.len() == 11 && !input.contains('/') && !input.contains('=') {
        return input.to_string();
    }
    for marker in ["watch?v=", "youtu.be/", "shorts/", "live/", "embed/", "/v/"] {
        if let Some(pos) = input.find(marker) {
            let rest = &input[pos + marker.len()..];
            let id: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            if id.len() >= 11 {
                return id[..11].to_string();
            }
        }
    }
    input.to_string()
}

fn build_body(def: &ClientDef, video_id: &str) -> Value {
    let mut client = json!({
        "clientName": def.client_name,
        "clientVersion": def.client_version,
        "hl": "en",
        "gl": "US",
    });
    // extra_client をマージ。
    if let (Some(obj), Some(extra)) = (client.as_object_mut(), def.extra_client.as_object()) {
        for (k, v) in extra {
            obj.insert(k.clone(), v.clone());
        }
    }
    json!({
        "context": { "client": client },
        "videoId": video_id,
        "contentCheckOk": true,
        "racyCheckOk": true,
    })
}

/// adaptiveFormats / formats を走査し、url 直書き数 / signatureCipher 数 と
/// 最初の url 直書き video/audio を返す。
struct FormatScan {
    direct_url_count: usize,
    cipher_count: usize,
    sample_video_url: Option<String>,
    sample_audio_url: Option<String>,
    sample_video_label: String,
    sample_audio_label: String,
    hls_manifest: Option<String>,
}

fn scan_formats(streaming: &Value) -> FormatScan {
    let mut scan = FormatScan {
        direct_url_count: 0,
        cipher_count: 0,
        sample_video_url: None,
        sample_audio_url: None,
        sample_video_label: String::new(),
        sample_audio_label: String::new(),
        hls_manifest: streaming
            .get("hlsManifestUrl")
            .and_then(Value::as_str)
            .map(str::to_string),
    };

    let empty = vec![];
    let adaptive = streaming
        .get("adaptiveFormats")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let progressive = streaming
        .get("formats")
        .and_then(Value::as_array)
        .unwrap_or(&empty);

    for f in adaptive.iter().chain(progressive.iter()) {
        let has_direct = f.get("url").and_then(Value::as_str).is_some();
        let has_cipher = f.get("signatureCipher").is_some();
        if has_direct {
            scan.direct_url_count += 1;
        }
        if has_cipher {
            scan.cipher_count += 1;
        }
        let Some(url) = f.get("url").and_then(Value::as_str) else {
            continue;
        };
        let mime = f.get("mimeType").and_then(Value::as_str).unwrap_or("");
        let itag = f.get("itag").and_then(Value::as_i64).unwrap_or(0);
        let is_video = mime.starts_with("video/");
        let is_audio = mime.starts_with("audio/");
        if is_video && scan.sample_video_url.is_none() {
            let h = f.get("height").and_then(Value::as_i64).unwrap_or(0);
            scan.sample_video_url = Some(url.to_string());
            scan.sample_video_label = format!("itag={itag} {mime} {h}p");
        }
        if is_audio && scan.sample_audio_url.is_none() {
            scan.sample_audio_url = Some(url.to_string());
            scan.sample_audio_label = format!("itag={itag} {mime}");
        }
    }
    scan
}

/// スループット実測: 先頭 ~8MB を Range GET して実 download 速度を測る。
/// nsig 未処理の throttle は初回 206 は通るが速度が ~40-80KB/s に絞られる症状なので、
/// ここで「直リンクが実用速度で取れるか」を確定させる（U1 の throttle 側 / U5 要否の判断材料）。
fn probe_throughput(client: &reqwest::blocking::Client, ua: &str, url: &str) -> String {
    const CHUNK: u64 = 8 * 1024 * 1024; // 8MB
    let has_n = url.contains("&n=") || url.contains("?n=");
    let t0 = Instant::now();
    let resp = client
        .get(url)
        .header("User-Agent", ua)
        .header("Range", format!("bytes=0-{}", CHUNK - 1))
        .send();
    let mut resp = match resp {
        Ok(r) => r,
        Err(e) => return format!("GET失敗: {e}"),
    };
    let status = resp.status().as_u16();
    let mut buf = Vec::with_capacity(CHUNK as usize);
    if let Err(e) = resp.read_to_end(&mut buf) {
        return format!("HTTP {status} 読み取り失敗: {e}");
    }
    let secs = t0.elapsed().as_secs_f64();
    let bytes = buf.len() as f64;
    let mbps = (bytes / (1024.0 * 1024.0)) / secs.max(0.0001);
    // 経験則: throttle 時は ~0.05-0.08 MB/s。実用は数 MB/s 以上。
    let verdict = if mbps < 0.3 {
        "⚠ 低速(throttle疑い=要nsig)"
    } else {
        "高速(throttle無=nsig不要の可能性)"
    };
    format!(
        "HTTP {status} {:.2}MB を {:.2}s = {:.2} MB/s {verdict} [n param={}]",
        bytes / (1024.0 * 1024.0),
        secs,
        mbps,
        if has_n { "有" } else { "無" }
    )
}

/// Range GET でストリーム URL の実際の取得可否を判定する。200/206=再生可、403=要token。
fn probe_url(client: &reqwest::blocking::Client, ua: &str, url: &str) -> String {
    let resp = client
        .get(url)
        .header("User-Agent", ua)
        .header("Range", "bytes=0-1")
        .send();
    match resp {
        Ok(r) => {
            let status = r.status().as_u16();
            let len = r
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("?")
                .to_string();
            let verdict = match status {
                200 | 206 => "OK(再生可)",
                403 => "403(要token/bot)",
                _ => "他",
            };
            format!("HTTP {status} {verdict} content-length={len}")
        }
        Err(e) => format!("GET 失敗: {e}"),
    }
}

/// 1 本の動画について全 client をテストし、サマリ行を返す。
fn run_video(http: &reqwest::blocking::Client, label: &str, video_id: &str) -> Vec<(String, bool, String)> {
    println!("\n╔══════════════════════════════════════════════════════");
    println!("║ 動画カテゴリ: {label}  (video_id={video_id})");
    println!("╚══════════════════════════════════════════════════════");

    let mut summary: Vec<(String, bool, String)> = Vec::new();

    for def in clients() {
        println!("────────────────────────────────────────");
        println!("[{}] {} v{} (clientNameId={}, require_js={})",
            def.key, def.client_name, def.client_version, def.client_name_id, def.require_js);

        let body = build_body(&def, video_id);
        let resp = http
            .post(PLAYER_ENDPOINT)
            .header("Content-Type", "application/json")
            .header("User-Agent", def.user_agent)
            .header("X-Youtube-Client-Name", def.client_name_id.to_string())
            .header("X-Youtube-Client-Version", def.client_version)
            .header("Origin", "https://www.youtube.com")
            .json(&body)
            .send();

        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                println!("  player POST 失敗: {e}");
                summary.push((def.key.to_string(), false, format!("POST失敗: {e}")));
                continue;
            }
        };
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        let val: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                println!("  player HTTP {status} だが JSON 解析失敗: {e}");
                summary.push((def.key.to_string(), false, "JSON解析失敗".to_string()));
                continue;
            }
        };

        let play_status = val["playabilityStatus"]["status"].as_str().unwrap_or("?");
        let reason = val["playabilityStatus"]["reason"].as_str().unwrap_or("");
        let title = val["videoDetails"]["title"].as_str().unwrap_or("");
        let is_live = val["videoDetails"]["isLive"].as_bool().unwrap_or(false);
        println!("  player HTTP {status} / playabilityStatus={play_status} {reason}");
        println!("  title={title:?} is_live={is_live}");

        let Some(streaming) = val.get("streamingData") else {
            println!("  streamingData なし → この client では取得不可");
            summary.push((
                def.key.to_string(),
                false,
                format!("streamingData無 ({play_status} {reason})"),
            ));
            continue;
        };

        let scan = scan_formats(streaming);
        println!(
            "  formats: direct_url={} signatureCipher={} hls={}",
            scan.direct_url_count,
            scan.cipher_count,
            scan.hls_manifest.is_some()
        );

        // url 直書きの video/audio を実際に取得してみる。
        let mut verdict_ok = false;
        let mut verdict_detail = String::new();
        if let Some(vurl) = &scan.sample_video_url {
            let r = probe_url(&http, def.user_agent, vurl);
            println!("  video {} → {r}", scan.sample_video_label);
            if r.contains("OK(再生可)") {
                verdict_ok = true;
                // 206 が通った client だけ throughput を実測（throttle/nsig 要否の確定）。
                let tp = probe_throughput(&http, def.user_agent, vurl);
                println!("    └ throughput: {tp}");
                verdict_detail = format!("video:{r} | {tp}");
            } else {
                verdict_detail = format!("video:{r}");
            }
        }
        if let Some(aurl) = &scan.sample_audio_url {
            let r = probe_url(&http, def.user_agent, aurl);
            println!("  audio {} → {r}", scan.sample_audio_label);
            verdict_detail = format!("{verdict_detail} | audio:{r}");
        }
        if scan.sample_video_url.is_none() && scan.cipher_count > 0 {
            println!("  ※ 直書き url 無し・signatureCipher のみ → U1では未検証(署名復号が要る=U5/M9)");
            verdict_detail = "signatureCipherのみ(要署名復号)".to_string();
        }
        if let Some(hls) = &scan.hls_manifest {
            println!("  hlsManifestUrl: {}", &hls[..hls.len().min(80)]);
            // ライブは HLS をそのまま mpv に渡す(M13)。直リンクが無くても OK+HLS なら成立扱い。
            if !verdict_ok && play_status == "OK" {
                verdict_ok = true;
                verdict_detail = format!("HLS経路OK(ライブ向け) {verdict_detail}");
            }
        }

        summary.push((def.key.to_string(), verdict_ok, verdict_detail));
    }

    println!("\n──── [{label}] サマリ ────");
    for (key, ok, detail) in &summary {
        let mark = if *ok { "✅ OK   " } else { "❌ NG/未 " };
        println!("{mark} {key:14} {detail}");
    }
    summary
}

fn main() -> Result<()> {
    // 既定のテストセット: 通常VOD / ライブ / 年齢制限。CLI で `label=id` を渡すと上書き。
    // ライブ ID は流動的なので、実行時点で生存しているものに適宜差し替えること。
    let args: Vec<String> = std::env::args().skip(1).collect();
    let videos: Vec<(String, String)> = if args.is_empty() {
        vec![
            ("通常VOD".to_string(), "dQw4w9WgXcQ".to_string()),
            ("ライブ(LofiGirl)".to_string(), "X4VbdwhkE10".to_string()),
            ("年齢制限".to_string(), "HtVdAasjOgU".to_string()),
        ]
    } else {
        args.iter()
            .map(|a| match a.split_once('=') {
                Some((label, id)) => (label.to_string(), extract_video_id(id)),
                None => ("指定".to_string(), extract_video_id(a)),
            })
            .collect()
    };

    println!("=== U1 player probe (堅牢性確認: 複数カテゴリ) ===");

    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    // (label, [(client, ok, detail)])
    let mut all: Vec<(String, Vec<(String, bool, String)>)> = Vec::new();
    for (label, id) in &videos {
        let s = run_video(&http, label, id);
        all.push((label.clone(), s));
    }

    println!("\n════════════ U1 総合結論（カテゴリ × client） ════════════");
    println!("（✅ = PoToken無しで直リンクが 200/206 で取得できた）\n");
    // android_vr / ios / android が各カテゴリで取れたかを一覧。
    let focus = ["ios", "android", "android_vr"];
    for (label, summary) in &all {
        print!("{label:18}");
        for key in focus {
            let ok = summary.iter().find(|(k, _, _)| k == key).map(|(_, ok, _)| *ok).unwrap_or(false);
            print!("  {key}={}", if ok { "✅" } else { "❌" });
        }
        println!();
    }
    println!("\n→ 全カテゴリで no-JS client が ✅ なら U1 は堅牢に成立し、U5(nsig/boa)を回避できる。");
    println!("  ライブで android_vr が ❌ でも ios(HLS)が ✅ なら、ライブは ios に振ればよい(M13)。");
    Ok(())
}
