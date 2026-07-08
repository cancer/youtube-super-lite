//! P0 実証プローブ (issue #16): ログイン済みライブの `serverAbrStreamingUrl` に
//! **Bearer 付きの最小 `VideoPlaybackAbrRequest` を POST し、PoToken 無しでメディアが返るか**を実測する。
//!
//! go/no-go の判定:
//!   - 白(GO)  : Bearer だけで UMP の MEDIA パート（実メディアバイト）が返る
//!               → 案1(SABR プロトコル実装)へそのまま進める。
//!   - 黒(NO-GO): メディアが返らず STREAM_PROTECTION_STATUS が attestation を要求 / SABR_ERROR / 403
//!               → 案2(PoToken プロバイダ統合)が案1の前提条件に昇格し、見積もりを再計算する。
//!
//! 背景（ysl-live-botgate / issue #16）: 7/4 に YouTube がログイン済み TV client の
//! ライブ応答を SABR 化し、`hlsManifestUrl` が消えて `serverAbrStreamingUrl`+`adaptiveFormats`
//! のみになった。SABR は `serverAbrStreamingUrl` に protobuf(`VideoPlaybackAbrRequest`)を POST し、
//! UMP(varint フレーミング)でメディアを受け取るプロトコル。本プローブは案1の最大リスク
//! （PoToken 要求の有無）だけを最小コストで潰す。
//!
//! proto フィールド番号・UMP パート ID・リクエスト構成は LuanRT/googlevideo 準拠:
//!   VideoPlaybackAbrRequest{ clientAbrState=1, ustreamerConfig=5, prefAudio=16, prefVideo=17, streamerContext=19 }
//!   StreamerContext{ clientInfo=1, poToken=2(本プローブでは付けない) }
//!   ClientAbrState{ playerTimeMs=28, enabledTrackTypesBitfield=40 }
//!   ClientInfo{ clientName=16, clientVersion=17, osName=18, osVersion=19 }
//!   FormatId{ itag=1, lastModified=2, xtags=3 }
//!   UMP part id: MEDIA_HEADER=20, MEDIA=21, MEDIA_END=22, FORMAT_INITIALIZATION_METADATA=42,
//!                SABR_REDIRECT=43, SABR_ERROR=44, STREAM_PROTECTION_STATUS=58
//!
//! 使い方（**ログイン済みの実機で実行**。auth.json の refresh_token が要る）:
//!   cargo run --release --bin p0_sabr_probe -- <liveVideoId|watchURL>
//!   現行ライブ ID: curl -sL https://www.youtube.com/@NASA/live | grep -oE 'v=[A-Za-z0-9_-]{11}'

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;
// P0.5: serverAbrStreamingUrl の n= を nsig 変換する。boa 実装は本クレートに同梱（nsig.rs）。
// （元は ysl-core の同名モジュールを流用していたが、本体を汚さないよう独立クレートへ複製した）
mod nsig;
use nsig::NsigSolver;

/// player 取得と serverAbrStreamingUrl への POST で使う HTTP クライアント。
/// **IPv4 egress を固定**する: googlevideo の stream URL は player を取得した送信元 IP に
/// バインドされ、POST が別 IP/ファミリ(dual-stack で IPv6 等)から出ると 403 になる
/// （resolver-sidecar が local_address(Ipv4::UNSPECIFIED) する理由と同一）。
/// cookie_store も有効化し、player 応答が張る cookie を後続 POST に載せる。
fn build_http() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .local_address(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
        .cookie_store(true)
        .build()
        .map_err(Into::into)
}

const PLAYER_ENDPOINT: &str = "https://www.youtube.com/youtubei/v1/player?prettyPrint=false";
const DEFAULT_BACKEND: &str = "https://youtube-super-lite-backend.cancer6.workers.dev";

// TVHTML5 client（Bearer を受け付ける唯一の層。ライブの SABR 応答はここから取る）。
const TV_CLIENT_NAME: &str = "TVHTML5";
const TV_CLIENT_VERSION: &str = "7.20260114.12.00";
const TV_CLIENT_NAME_ID: i64 = 7;
const TV_USER_AGENT: &str = "Mozilla/5.0 (ChromiumStylePlatform) Cobalt/Version";

// ── UMP part id（LuanRT/googlevideo ump_part_id.proto）─────────────────────────
const UMP_MEDIA_HEADER: u64 = 20;
const UMP_MEDIA: u64 = 21;
const UMP_MEDIA_END: u64 = 22;
const UMP_FORMAT_INIT_METADATA: u64 = 42;
const UMP_SABR_REDIRECT: u64 = 43;
const UMP_SABR_ERROR: u64 = 44;
const UMP_STREAM_PROTECTION_STATUS: u64 = 58;

// ─────────────────────────────────────────────────────────────────────────────
// auth: refresh_token → access_token（u7_auth_probe と同じ経路。トークンは出力しない）。
// ─────────────────────────────────────────────────────────────────────────────
fn auth_json_path() -> std::path::PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(base).join("YouTubeSuperLite").join("auth.json")
}

fn get_access_token(http: &reqwest::blocking::Client) -> Result<String> {
    let path = auth_json_path();
    let data = std::fs::read_to_string(&path).map_err(|_| {
        anyhow!("auth.json が読めません({})。先にアプリでログインしてください。", path.display())
    })?;
    let v: Value = serde_json::from_str(&data)?;
    let refresh = v["refresh_token"]
        .as_str()
        .ok_or_else(|| anyhow!("refresh_token がありません"))?;

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
    tr["access_token"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("access_token が返りませんでした"))
}

fn extract_video_id(input: &str) -> String {
    let input = input.trim();
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

// ─────────────────────────────────────────────────────────────────────────────
// player 取得（TVHTML5 + Bearer）。SABR に必要な 3 点を取り出す。
// ─────────────────────────────────────────────────────────────────────────────
struct SabrInputs {
    server_abr_url: String,
    ustreamer_config_b64: String,
    video_fmt: FormatId,
    audio_fmt: FormatId,
}

#[derive(Clone)]
struct FormatId {
    itag: i64,
    last_modified: u64,
    xtags: Option<String>,
    label: String,
}

fn fetch_player_tv(
    http: &reqwest::blocking::Client,
    video_id: &str,
    token: &str,
) -> Result<(Value, bool, String)> {
    let body = json!({
        "context": { "client": {
            "clientName": TV_CLIENT_NAME,
            "clientVersion": TV_CLIENT_VERSION,
            "hl": "en", "gl": "US",
        }},
        "videoId": video_id,
        "contentCheckOk": true,
        "racyCheckOk": true,
    });
    let resp = http
        .post(PLAYER_ENDPOINT)
        .header("Content-Type", "application/json")
        .header("User-Agent", TV_USER_AGENT)
        .header("X-Youtube-Client-Name", TV_CLIENT_NAME_ID.to_string())
        .header("X-Youtube-Client-Version", TV_CLIENT_VERSION)
        .header("Origin", "https://www.youtube.com")
        .bearer_auth(token)
        .json(&body)
        .send()?;
    parse_player(resp.text()?)
}

/// 案2 PoC(本命): TVHTML5+Bearer の player を、visitorData + PoToken 付きで叩く。
/// PoToken を player 要求に載せることで、YouTube が「その token で認可済み」の
/// serverAbrStreamingUrl を発行する（reference と同じ前提を満たす）。
fn fetch_player_tv_pot(
    http: &reqwest::blocking::Client,
    video_id: &str,
    token: &str,
    visitor_data: &str,
    po_token: &str,
) -> Result<(Value, bool, String)> {
    let body = json!({
        "context": { "client": {
            "clientName": TV_CLIENT_NAME,
            "clientVersion": TV_CLIENT_VERSION,
            "hl": "en", "gl": "US",
            "visitorData": visitor_data,
        }},
        "videoId": video_id,
        "contentCheckOk": true,
        "racyCheckOk": true,
        "serviceIntegrityDimensions": { "poToken": po_token },
    });
    let resp = http
        .post(PLAYER_ENDPOINT)
        .header("Content-Type", "application/json")
        .header("User-Agent", TV_USER_AGENT)
        .header("X-Youtube-Client-Name", TV_CLIENT_NAME_ID.to_string())
        .header("X-Youtube-Client-Version", TV_CLIENT_VERSION)
        .header("X-Goog-Visitor-Id", visitor_data)
        .header("Origin", "https://www.youtube.com")
        .bearer_auth(token)
        .json(&body)
        .send()?;
    parse_player(resp.text()?)
}

// 案2 PoC 用 WEB client 定数（PoToken を束ねる先＝visitorData と一致させる）。
const WEB_CLIENT_NAME: &str = "WEB";
const WEB_CLIENT_VERSION: &str = "2.20260114.08.00";
const WEB_CLIENT_NAME_ID: i64 = 1;
const WEB_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36";

/// 案2 PoC: 匿名 WEB + visitorData + PoToken で player を叩く（PoToken が bot ゲートを突破するか）。
/// PoToken は player 要求の serviceIntegrityDimensions と context.client.visitorData に載せる。
fn fetch_player_web(
    http: &reqwest::blocking::Client,
    video_id: &str,
    visitor_data: &str,
    po_token: &str,
) -> Result<(Value, bool, String)> {
    let body = json!({
        "context": { "client": {
            "clientName": WEB_CLIENT_NAME,
            "clientVersion": WEB_CLIENT_VERSION,
            "hl": "en", "gl": "US",
            "visitorData": visitor_data,
        }},
        "videoId": video_id,
        "contentCheckOk": true,
        "racyCheckOk": true,
        "serviceIntegrityDimensions": { "poToken": po_token },
    });
    let resp = http
        .post(PLAYER_ENDPOINT)
        .header("Content-Type", "application/json")
        .header("User-Agent", WEB_USER_AGENT)
        .header("X-Youtube-Client-Name", WEB_CLIENT_NAME_ID.to_string())
        .header("X-Youtube-Client-Version", WEB_CLIENT_VERSION)
        .header("X-Goog-Visitor-Id", visitor_data)
        .header("Origin", "https://www.youtube.com")
        .json(&body)
        .send()?;
    parse_player(resp.text()?)
}

fn parse_player(text: String) -> Result<(Value, bool, String)> {
    let val: Value = serde_json::from_str(&text).map_err(|e| anyhow!("player JSON 解析失敗: {e}"))?;
    let status = val["playabilityStatus"]["status"].as_str().unwrap_or("?").to_string();
    let reason = val["playabilityStatus"]["reason"].as_str().unwrap_or("");
    let is_live = val["videoDetails"]["isLive"].as_bool().unwrap_or(false);
    let status = if reason.is_empty() { status } else { format!("{status} ({reason})") };
    Ok((val, is_live, status))
}

fn parse_format_id(f: &Value) -> Option<FormatId> {
    let itag = f["itag"].as_i64()?;
    // lastModified は数値または数値文字列で返る。
    let last_modified = f["lastModified"]
        .as_u64()
        .or_else(|| f["lastModified"].as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0);
    let xtags = f["xtags"].as_str().map(str::to_string);
    let mime = f["mimeType"].as_str().unwrap_or("");
    let h = f["height"].as_i64().unwrap_or(0);
    let br = f["bitrate"].as_i64().unwrap_or(0);
    let label = format!("itag={itag} {mime} {h}p {br}bps lmt={last_modified}");
    Some(FormatId { itag, last_modified, xtags, label })
}

fn extract_sabr_inputs(player: &Value) -> Result<SabrInputs> {
    let streaming = player
        .get("streamingData")
        .ok_or_else(|| anyhow!("streamingData なし（この応答では SABR 入力が取れない）"))?;

    let server_abr_url = streaming["serverAbrStreamingUrl"]
        .as_str()
        .ok_or_else(|| anyhow!("serverAbrStreamingUrl なし（SABR 応答ではない = HLS 経路か bot ゲート）"))?
        .to_string();

    let ustreamer_config_b64 = player["playerConfig"]["mediaCommonConfig"]
        ["mediaUstreamerRequestConfig"]["videoPlaybackUstreamerConfig"]
        .as_str()
        .ok_or_else(|| anyhow!("videoPlaybackUstreamerConfig なし"))?
        .to_string();

    let empty = vec![];
    let adaptive = streaming["adaptiveFormats"].as_array().unwrap_or(&empty);
    let mut best_video: Option<FormatId> = None;
    let mut best_audio: Option<FormatId> = None;
    let mut best_v_h = -1i64;
    let mut best_a_br = -1i64;
    for f in adaptive {
        let mime = f["mimeType"].as_str().unwrap_or("");
        if mime.starts_with("video/") {
            let h = f["height"].as_i64().unwrap_or(0);
            if h > best_v_h {
                if let Some(fid) = parse_format_id(f) {
                    best_v_h = h;
                    best_video = Some(fid);
                }
            }
        } else if mime.starts_with("audio/") {
            let br = f["bitrate"].as_i64().unwrap_or(0);
            if br > best_a_br {
                if let Some(fid) = parse_format_id(f) {
                    best_a_br = br;
                    best_audio = Some(fid);
                }
            }
        }
    }
    let video_fmt = best_video.ok_or_else(|| anyhow!("video format が adaptiveFormats に無い"))?;
    let audio_fmt = best_audio.ok_or_else(|| anyhow!("audio format が adaptiveFormats に無い"))?;
    Ok(SabrInputs { server_abr_url, ustreamer_config_b64, video_fmt, audio_fmt })
}

// ─────────────────────────────────────────────────────────────────────────────
// protobuf エンコード（標準 varint）。VideoPlaybackAbrRequest を手組みする。
// ─────────────────────────────────────────────────────────────────────────────
fn pb_varint(buf: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            buf.push(b | 0x80);
        } else {
            buf.push(b);
            break;
        }
    }
}
fn pb_tag(buf: &mut Vec<u8>, field: u64, wire: u64) {
    pb_varint(buf, (field << 3) | wire);
}
/// wire=0（varint）フィールド。
fn pb_uint(buf: &mut Vec<u8>, field: u64, v: u64) {
    pb_tag(buf, field, 0);
    pb_varint(buf, v);
}
/// wire=2（length-delimited）フィールド。
fn pb_bytes(buf: &mut Vec<u8>, field: u64, data: &[u8]) {
    pb_tag(buf, field, 2);
    pb_varint(buf, data.len() as u64);
    buf.extend_from_slice(data);
}

fn encode_format_id(f: &FormatId) -> Vec<u8> {
    let mut b = Vec::new();
    pb_uint(&mut b, 1, f.itag as u64); // itag
    pb_uint(&mut b, 2, f.last_modified); // last_modified
    if let Some(x) = &f.xtags {
        pb_bytes(&mut b, 3, x.as_bytes()); // xtags
    }
    b
}

fn encode_client_info_for(client_name_id: i64, client_version: &str) -> Vec<u8> {
    let mut b = Vec::new();
    pb_uint(&mut b, 16, client_name_id as u64); // client_name
    pb_bytes(&mut b, 17, client_version.as_bytes()); // client_version
    b
}

/// streamer_context。po_token=None なら P0/P0.5（PoToken 無し）、Some なら案2 PoC。
fn encode_streamer_context(po_token: Option<&[u8]>, client_name_id: i64, client_version: &str) -> Vec<u8> {
    let mut b = Vec::new();
    pb_bytes(&mut b, 1, &encode_client_info_for(client_name_id, client_version)); // client_info
    if let Some(pot) = po_token {
        pb_bytes(&mut b, 2, pot); // po_token（案2: PoToken を束ねる）
    }
    b
}

fn encode_client_abr_state() -> Vec<u8> {
    let mut b = Vec::new();
    pb_uint(&mut b, 28, 0); // player_time_ms = 0（先頭から）
    pb_uint(&mut b, 40, 0); // enabled_track_types_bitfield = 0（VIDEO_AND_AUDIO）
    b
}

fn build_abr_request(
    inp: &SabrInputs,
    ustreamer_config: &[u8],
    po_token: Option<&[u8]>,
    client_name_id: i64,
    client_version: &str,
) -> Vec<u8> {
    let mut b = Vec::new();
    pb_bytes(&mut b, 1, &encode_client_abr_state()); // client_abr_state
    pb_bytes(&mut b, 5, ustreamer_config); // video_playback_ustreamer_config
    pb_bytes(&mut b, 16, &encode_format_id(&inp.audio_fmt)); // preferred_audio_format_ids
    pb_bytes(&mut b, 17, &encode_format_id(&inp.video_fmt)); // preferred_video_format_ids
    pb_bytes(&mut b, 19, &encode_streamer_context(po_token, client_name_id, client_version)); // streamer_context
    b
}

// ─────────────────────────────────────────────────────────────────────────────
// base64 デコード（標準 + URL-safe、パディング/空白に寛容）。
// ─────────────────────────────────────────────────────────────────────────────
fn b64_decode(s: &str) -> Result<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' | b'-' => Some(62),
            b'/' | b'_' => Some(63),
            _ => None,
        }
    }
    let mut acc = 0u32;
    let mut bits = 0u32;
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for &c in s.as_bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        let v = val(c).ok_or_else(|| anyhow!("base64 に不正文字: {}", c as char))?;
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// UMP レスポンスのデコード（LuanRT/googlevideo UmpReader の可変長整数）。
//   part = [varint partType][varint partSize][partSize bytes payload]
// ─────────────────────────────────────────────────────────────────────────────
/// UMP 独自 varint を読む。返り値 (value, 消費バイト数)。不足なら None。
fn ump_varint(buf: &[u8], off: usize) -> Option<(u64, usize)> {
    let first = *buf.get(off)?;
    let len = if first < 128 { 1 } else if first < 192 { 2 } else if first < 224 { 3 } else if first < 240 { 4 } else { 5 };
    if off + len > buf.len() {
        return None;
    }
    let b = |i: usize| buf[off + i] as u64;
    let value = match len {
        1 => b(0),
        2 => (b(0) & 0x3f) + 64 * b(1),
        3 => (b(0) & 0x1f) + 32 * (b(1) + 256 * b(2)),
        4 => (b(0) & 0x0f) + 16 * (b(1) + 256 * (b(2) + 256 * b(3))),
        _ => b(1) + 256 * (b(2) + 256 * (b(3) + 256 * b(4))), // 5: 先頭バイトは捨て、続く 4 バイト LE
    };
    Some((value, len))
}

struct UmpPart {
    part_type: u64,
    payload: Vec<u8>,
}

fn parse_ump(buf: &[u8]) -> Vec<UmpPart> {
    let mut parts = Vec::new();
    let mut off = 0usize;
    while off < buf.len() {
        let Some((part_type, n1)) = ump_varint(buf, off) else { break };
        off += n1;
        let Some((size, n2)) = ump_varint(buf, off) else { break };
        off += n2;
        let size = size as usize;
        if off + size > buf.len() {
            // 途中で切れている（本プローブは 1 レスポンスしか読まないので許容）。
            let avail = buf.len() - off;
            parts.push(UmpPart { part_type, payload: buf[off..off + avail].to_vec() });
            break;
        }
        parts.push(UmpPart { part_type, payload: buf[off..off + size].to_vec() });
        off += size;
    }
    parts
}

/// protobuf から最初の指定フィールド(wire=0)の varint 値を読む（StreamProtectionStatus.status 用）。
fn pb_first_varint_field(buf: &[u8], want_field: u64) -> Option<u64> {
    let mut off = 0usize;
    while off < buf.len() {
        let (tag, n) = pb_read_varint(buf, off)?;
        off += n;
        let field = tag >> 3;
        let wire = tag & 7;
        match wire {
            0 => {
                let (v, n) = pb_read_varint(buf, off)?;
                off += n;
                if field == want_field {
                    return Some(v);
                }
            }
            2 => {
                let (len, n) = pb_read_varint(buf, off)?;
                off += n + len as usize;
            }
            5 => off += 4,
            1 => off += 8,
            _ => return None,
        }
    }
    None
}

/// protobuf から最初の指定フィールド(wire=2)のバイト列を読む（SabrRedirect.url 用）。
fn pb_first_bytes_field(buf: &[u8], want_field: u64) -> Option<Vec<u8>> {
    let mut off = 0usize;
    while off < buf.len() {
        let (tag, n) = pb_read_varint(buf, off)?;
        off += n;
        let field = tag >> 3;
        let wire = tag & 7;
        match wire {
            0 => {
                let (_v, n) = pb_read_varint(buf, off)?;
                off += n;
            }
            2 => {
                let (len, n) = pb_read_varint(buf, off)?;
                off += n;
                let end = off + len as usize;
                if field == want_field {
                    return buf.get(off..end).map(|s| s.to_vec());
                }
                off = end;
            }
            5 => off += 4,
            1 => off += 8,
            _ => return None,
        }
    }
    None
}

fn pb_read_varint(buf: &[u8], off: usize) -> Option<(u64, usize)> {
    let mut v = 0u64;
    let mut shift = 0u32;
    let mut i = off;
    loop {
        let b = *buf.get(i)?;
        v |= ((b & 0x7f) as u64) << shift;
        i += 1;
        if b & 0x80 == 0 {
            return Some((v, i - off));
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

fn part_name(t: u64) -> &'static str {
    match t {
        UMP_MEDIA_HEADER => "MEDIA_HEADER",
        UMP_MEDIA => "MEDIA",
        UMP_MEDIA_END => "MEDIA_END",
        UMP_FORMAT_INIT_METADATA => "FORMAT_INITIALIZATION_METADATA",
        UMP_SABR_REDIRECT => "SABR_REDIRECT",
        UMP_SABR_ERROR => "SABR_ERROR",
        UMP_STREAM_PROTECTION_STATUS => "STREAM_PROTECTION_STATUS",
        31 => "LIVE_METADATA",
        35 => "NEXT_REQUEST_POLICY",
        57 => "SABR_CONTEXT_UPDATE",
        _ => "(other)",
    }
}

fn protection_status_name(v: u64) -> &'static str {
    // googlevideo/yt-dlp: 1=OK, 2=ATTESTATION_PENDING, 3=ATTESTATION_REQUIRED
    match v {
        1 => "OK(PoToken不要)",
        2 => "ATTESTATION_PENDING(要注意)",
        3 => "ATTESTATION_REQUIRED(PoToken必須=黒)",
        _ => "(unknown)",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SABR に 1 回 POST してレスポンスを解析する。
// ─────────────────────────────────────────────────────────────────────────────
struct AbrResult {
    http_status: u16,
    content_type: String,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
}

fn post_abr(
    http: &reqwest::blocking::Client,
    url: &str,
    token: &str,
    req_body: &[u8],
) -> Result<AbrResult> {
    attempt(http, url, Some(token), Some(req_body))
}

/// URL/メソッド/Bearer を変えて 1 回叩く汎用関数（403 の原因切り分け用）。
/// req_body=None なら GET、Some なら POST。token=None なら Bearer 無し。
fn attempt(
    http: &reqwest::blocking::Client,
    url: &str,
    token: Option<&str>,
    req_body: Option<&[u8]>,
) -> Result<AbrResult> {
    let mut rb = match req_body {
        Some(b) => http.post(url).header("Content-Type", "application/x-protobuf").body(b.to_vec()),
        None => http.get(url),
    };
    rb = rb
        .header("User-Agent", TV_USER_AGENT)
        .header("Origin", "https://www.youtube.com")
        // 圧縮を無効化（reqwest は gzip 機能未有効。生の UMP を確実に受ける）。
        .header("Accept-Encoding", "identity");
    if let Some(t) = token {
        rb = rb.bearer_auth(t);
    }
    let mut resp = rb.send()?;
    let http_status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    // 診断のため応答ヘッダを全部拾う（403 の理由が独自ヘッダに載ることがある）。
    let mut headers = Vec::new();
    for (k, v) in resp.headers().iter() {
        headers.push((k.as_str().to_ascii_lowercase(), v.to_str().unwrap_or("(binary)").to_string()));
    }
    let mut body = Vec::new();
    resp.read_to_end(&mut body)?;
    Ok(AbrResult { http_status, content_type, body, headers })
}

/// URL からクエリパラメータ 1 個を取り除く（`n` 除去テスト用）。
fn strip_query_param(url: &str, key: &str) -> String {
    let Some((base, query)) = url.split_once('?') else { return url.to_string() };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|p| {
            let name = p.split('=').next().unwrap_or("");
            name != key
        })
        .collect();
    if kept.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", kept.join("&"))
    }
}

/// レスポンス解析 → (メディアバイト総数, 保護ステータス, SABR_REDIRECT の URL, SABR_ERROR 有無) を出力しつつ返す。
struct Verdict {
    media_bytes: usize,
    protection_status: Option<u64>,
    redirect_url: Option<String>,
    sabr_error: bool,
    format_init: bool,
}

fn analyze(res: &AbrResult) -> Verdict {
    println!("  HTTP {} / content-type: {}", res.http_status, if res.content_type.is_empty() { "(なし)" } else { &res.content_type });
    for (k, v) in &res.headers {
        if k == "content-type" || k == "content-length" { continue; }
        let vv = if v.len() > 120 { format!("{}…", &v[..120]) } else { v.clone() };
        println!("    header {k}: {vv}");
    }
    println!("  レスポンス長: {} bytes", res.body.len());
    if !res.body.is_empty() && res.body.len() <= 400 && !res.content_type.contains("ump") {
        println!("  body(先頭): {}", String::from_utf8_lossy(&res.body[..res.body.len().min(400)]));
    }
    let parts = parse_ump(&res.body);
    println!("  UMP パート数: {}", parts.len());

    let mut v = Verdict {
        media_bytes: 0,
        protection_status: None,
        redirect_url: None,
        sabr_error: false,
        format_init: false,
    };
    // パート種別ごとの集計。
    let mut counts: std::collections::BTreeMap<u64, (usize, usize)> = std::collections::BTreeMap::new();
    for p in &parts {
        let e = counts.entry(p.part_type).or_insert((0, 0));
        e.0 += 1;
        e.1 += p.payload.len();
        match p.part_type {
            UMP_MEDIA => v.media_bytes += p.payload.len(),
            UMP_FORMAT_INIT_METADATA => v.format_init = true,
            UMP_STREAM_PROTECTION_STATUS => {
                if let Some(st) = pb_first_varint_field(&p.payload, 1) {
                    v.protection_status = Some(st);
                }
            }
            UMP_SABR_REDIRECT => {
                if let Some(bytes) = pb_first_bytes_field(&p.payload, 1) {
                    v.redirect_url = String::from_utf8(bytes).ok();
                }
            }
            UMP_SABR_ERROR => {
                v.sabr_error = true;
            }
            _ => {}
        }
    }
    for (t, (n, sz)) in &counts {
        println!("    part {:>3} {:32} ×{} ({} bytes)", t, part_name(*t), n, sz);
    }
    if let Some(st) = v.protection_status {
        println!("  → STREAM_PROTECTION_STATUS.status = {} {}", st, protection_status_name(st));
    }
    if v.sabr_error {
        if let Some(p) = parts.iter().find(|p| p.part_type == UMP_SABR_ERROR) {
            let n = p.payload.len().min(64);
            println!("  → SABR_ERROR payload[0..{}]: {}", n, hex(&p.payload[..n]));
        }
    }
    v
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join("")
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // 案2 PoC モード: --potoken <token.json> <liveVideoId>
    //   token.json = gen.mjs 出力 {"visitorData":..,"poToken":..}（bgutils-js で生成）。
    //   匿名 WEB + visitorData + PoToken で player→SABR を通し、403→200 反転を確認する。
    if args.first().map(String::as_str) == Some("--potoken") {
        let json_path = args.get(1).cloned().unwrap_or_default();
        let raw = args.get(2).cloned().unwrap_or_default();
        if json_path.is_empty() || raw.is_empty() {
            eprintln!("usage: p0_sabr_probe --potoken <token.json> <liveVideoId>");
            std::process::exit(2);
        }
        let http = build_http()?;
        return run_potoken_poc(&http, &json_path, &extract_video_id(&raw));
    }

    // 案2 PoC（本命）: --potoken-tv <gen.mjs> <liveVideoId>
    //   TVHTML5+Bearer(=SABR を返す唯一の経路)の player を取り、その session の visitorData に束ねた
    //   PoToken を node(gen.mjs) で即時発行し、streamer_context に載せて POST。403→200 反転を見る。
    if args.first().map(String::as_str) == Some("--potoken-tv") {
        let gen = args.get(1).cloned().unwrap_or_default();
        let raw = args.get(2).cloned().unwrap_or_default();
        if gen.is_empty() || raw.is_empty() {
            eprintln!("usage: p0_sabr_probe --potoken-tv <gen.mjs> <liveVideoId>");
            std::process::exit(2);
        }
        let http = build_http()?;
        return run_potoken_tv_poc(&http, &gen, &extract_video_id(&raw));
    }

    let raw = match args.first() {
        Some(a) => a.clone(),
        None => {
            eprintln!("usage: p0_sabr_probe <liveVideoId|watchURL>");
            eprintln!("       p0_sabr_probe --potoken <token.json> <liveVideoId>   (案2 PoC)");
            eprintln!("  現行ライブ ID: curl -sL https://www.youtube.com/@NASA/live | grep -oE 'v=[A-Za-z0-9_-]{{11}}'");
            std::process::exit(2);
        }
    };
    let video_id = extract_video_id(&raw);

    println!("=== P0 SABR 実証プローブ (issue #16) ===");
    println!("video_id = {video_id}");
    println!("目的: PoToken 無し(Bearer のみ)で serverAbrStreamingUrl からメディアが返るか（go/no-go）\n");

    let http = build_http()?;

    print!("[1/4] OAuth access_token を取得中... ");
    let token = match get_access_token(&http) {
        Ok(t) => { println!("OK"); t }
        Err(e) => { println!("失敗"); return Err(e); }
    };

    println!("[2/4] TVHTML5 + Bearer で player 取得...");
    let (player, is_live, status) = fetch_player_tv(&http, &video_id, &token)?;
    println!("  playabilityStatus = {status} / is_live = {is_live}");
    if !is_live {
        println!("  ⚠ is_live=false。ライブ ID を渡しているか確認（終了済み配信は誤診の元）。");
    }
    let has_hls = player["streamingData"]["hlsManifestUrl"].is_string();
    println!("  hlsManifestUrl 有無 = {} （有ならこのリクエストは非SABR＝従来経路で再生可）", has_hls);

    let inp = match extract_sabr_inputs(&player) {
        Ok(i) => i,
        Err(e) => {
            println!("\n❌ SABR 入力が取れません: {e}");
            if has_hls {
                println!("   （hlsManifestUrl があるので今回は SABR 応答ではない。段階ロールアウトの別バケット。");
                println!("    数回リトライすると serverAbrStreamingUrl のみの応答を引ける場合がある。）");
            }
            return Err(e);
        }
    };
    println!("  serverAbrStreamingUrl: {}...", &inp.server_abr_url[..inp.server_abr_url.len().min(90)]);
    println!("  ustreamerConfig(b64) 長: {} 文字", inp.ustreamer_config_b64.len());
    println!("  video: {}", inp.video_fmt.label);
    println!("  audio: {}", inp.audio_fmt.label);
    let n_flag = inp.server_abr_url.contains("&n=") || inp.server_abr_url.contains("?n=");
    if n_flag {
        println!("  ⚠ serverAbrStreamingUrl に n= パラメータあり（nsig 変換が要る可能性。P0 では未変換で叩く）。");
    }
    // 403 切り分け: URL の全クエリキー + 署名関連パラメータの有無を表示。
    let all_keys: Vec<&str> = inp.server_abr_url
        .split('?').nth(1).unwrap_or("")
        .split('&').filter_map(|p| p.split('=').next()).filter(|s| !s.is_empty()).collect();
    println!("    url query keys: {}", all_keys.join(","));
    for key in ["ip", "source", "gcr", "sparams", "sig", "lsig", "lsparams", "pot", "sabr"] {
        if let Some(v) = query_get_param(&inp.server_abr_url, key) {
            let vv = if v.len() > 90 { format!("{}…", &v[..90]) } else { v };
            println!("    url param {key} = {vv}");
        }
    }

    println!("[3/4] VideoPlaybackAbrRequest を構築（PoToken 無し）...");
    let ustreamer = b64_decode(&inp.ustreamer_config_b64)?;
    let req_body = build_abr_request(&inp, &ustreamer, None, TV_CLIENT_NAME_ID, TV_CLIENT_VERSION);
    println!("  リクエスト protobuf 長: {} bytes", req_body.len());

    println!("[4/4] serverAbrStreamingUrl に POST（Bearer, PoToken 無し）...");
    let mut res = post_abr(&http, &inp.server_abr_url, &token, &req_body)?;
    let mut v = analyze(&res);

    // SABR_REDIRECT が返ったら 1 回だけ追従して本当の応答を見る。
    if v.media_bytes == 0 {
        if let Some(url) = v.redirect_url.clone() {
            println!("\n  ↪ SABR_REDIRECT を検出。リダイレクト先へ 1 回追従して再 POST...");
            res = post_abr(&http, &url, &token, &req_body)?;
            v = analyze(&res);
        }
    }

    let go = res.http_status == 200 && v.media_bytes > 0;

    // GO でなければ 403 の原因を切り分ける（n=未変換 / auth / URL レベルのどれか）。
    if !go {
        run_diagnostics(&http, &inp.server_abr_url, &token, &req_body);
    }

    // ── P0.5: n= を正しく nsig 変換して再 POST（n交絡の除去 = 白黒の最終確定）─────────
    // 未変換 n という唯一の交絡を潰す。既存 boa 実装(NsigSolver)を再利用。
    let mut nsig_probe: Option<NsigOutcome> = None;
    if !go && v.protection_status != Some(3) && !v.sabr_error {
        println!("\n[P0.5] serverAbrStreamingUrl の n= を nsig 変換して再 POST...");
        nsig_probe = Some(run_nsig_reprobe(&http, &inp.server_abr_url, &token, &req_body));
    }

    // ── 判定 ────────────────────────────────────────────────────────────────
    println!("\n════════════ P0/P0.5 判定 ════════════");
    if go {
        println!("✅ 白 (GO): PoToken 無し（Bearer のみ）で MEDIA {} bytes を受信。", v.media_bytes);
        println!("   → 案1(SABR プロトコル実装)へそのまま進める。P1(protobuf層)〜 に着手可。");
        if v.protection_status == Some(2) {
            println!("   ※ ただし protection_status=ATTESTATION_PENDING。継続再生で 3(要求)へ昇格しないか要監視。");
        }
    } else if v.protection_status == Some(3) || v.sabr_error {
        println!("❌ 黒 (NO-GO・確定): SABR レイヤが明示拒否（{}）。",
            if v.sabr_error { "SABR_ERROR" } else { "STREAM_PROTECTION_STATUS=ATTESTATION_REQUIRED" });
        println!("   → 案2(PoToken プロバイダ統合)が案1の前提条件に昇格。見積もりに +1週間以上を加算。");
    } else {
        match nsig_probe {
            Some(NsigOutcome::TransformFailed(e)) => {
                println!("△ 灰 (未確定): nsig 変換自体が失敗 → n 交絡を潰せなかった。");
                println!("   原因: {e}");
                println!("   含意: 現行 base.js に対し既存 nsig 抽出(nsig.rs)が壊れている可能性。");
                println!("   → 案1 の初手は nsig の保守復旧。復旧後に本プローブを再実行して PoToken 要否を確定する。");
            }
            Some(NsigOutcome::Reposted { changed, media_bytes, http_status, protection_status, sabr_error }) => {
                if media_bytes > 0 {
                    println!("✅ 白 (GO・n が原因だった): nsig 変換後の再 POST で MEDIA {media_bytes} bytes を受信。");
                    println!("   → PoToken は（少なくとも初回取得には）不要。案1 を nsig 込みで進められる。");
                    println!("   ※ 継続ポーリングで protection_status が昇格しないかは実装時に要確認。");
                } else if protection_status == Some(3) || sabr_error {
                    println!("❌ 黒 (NO-GO・確定): nsig 変換後も SABR レイヤが明示拒否（{}）。",
                        if sabr_error { "SABR_ERROR" } else { "ATTESTATION_REQUIRED" });
                    println!("   → n は無関係だった。案2(PoToken)が案1の前提条件で確定。");
                } else if http_status == 403 && changed {
                    println!("❌ 黒 (NO-GO・ほぼ確定): n を正しく変換(値が変化)して再 POST しても HTTP 403 / 0 bytes。");
                    println!("   → 唯一の交絡だった n= を潰しても通らない。Bearer だけでは不可＝PoToken 必須が濃厚。");
                    println!("   → 案2(PoToken プロバイダ統合)を案1の前提条件として見積もりを再計算する。");
                } else if http_status == 403 && !changed {
                    println!("△ 灰 (未確定): nsig 変換で n の値が変わらなかった（抽出が no-op の疑い）。");
                    println!("   → nsig の正しさが担保できない。案1 初手で nsig を検証してから再確定する。");
                } else {
                    println!("△ 灰: nsig 変換後 HTTP {http_status} / media 0。上のログで状態を確認する。");
                }
            }
            None => {
                println!("△ 灰: 判定材料不足（上のログ参照）。");
            }
        }
    }
    println!("\n（注記）本プローブは初回リクエストのみ。SABR セッション継続・PoToken 生成は範囲外。");
    Ok(())
}

/// 案2 PoC: 匿名 WEB + visitorData + PoToken で player→SABR を通し、403→200 反転を確認する。
fn run_potoken_poc(http: &reqwest::blocking::Client, json_path: &str, video_id: &str) -> Result<()> {
    println!("=== 案2 PoC: PoToken 付き SABR (issue #16) ===");
    println!("video_id = {video_id}");
    println!("目的: PoToken を束ねると serverAbrStreamingUrl が 403→200(メディア) に反転するか（最優先 go/no-go）\n");

    let data = std::fs::read_to_string(json_path)
        .map_err(|e| anyhow!("token.json が読めません({json_path}): {e}"))?;
    let tok: Value = serde_json::from_str(&data)?;
    let visitor_data = tok["visitorData"].as_str().ok_or_else(|| anyhow!("visitorData が無い"))?;
    let po_token = tok["poToken"].as_str().ok_or_else(|| anyhow!("poToken が無い"))?;
    println!("[1/5] PoToken 読込: visitorData {}文字 / poToken {}文字（WEB, visitorData 束ね）",
        visitor_data.len(), po_token.len());

    println!("[2/5] 匿名 WEB + visitorData + PoToken で player 取得...");
    let (player, is_live, status) = fetch_player_web(http, video_id, visitor_data, po_token)?;
    println!("  playabilityStatus = {status} / is_live = {is_live}");
    let has_hls = player["streamingData"]["hlsManifestUrl"].is_string();
    println!("  hlsManifestUrl 有無 = {has_hls}");
    if has_hls {
        println!("\n✅ 参考: PoToken 付き匿名 WEB で hlsManifestUrl が返った。");
        println!("   → ライブは HLS 経路で復旧可能（SABR 実装不要）。ただし HLS セグメントが 403 で封印されていないかは別途要確認。");
    }

    let inp = match extract_sabr_inputs(&player) {
        Ok(i) => i,
        Err(e) => {
            println!("\n△ SABR 入力が取れない: {e}");
            println!("   含意: PoToken を付けても匿名 WEB は live の SABR 応答を返さない（bot ゲート突破せず or 別応答）。");
            println!("   playabilityStatus を確認（LOGIN_REQUIRED なら PoToken だけでは live ゲートを越えられない）。");
            return Ok(());
        }
    };
    println!("  serverAbrStreamingUrl 取得 OK / video={} / audio={}", inp.video_fmt.label, inp.audio_fmt.label);

    println!("[3/5] serverAbrStreamingUrl の n= を nsig 変換...");
    let url_n = {
        let mut solver = NsigSolver::new();
        match solver.transform_url(http, &inp.server_abr_url) {
            Ok(u) => { println!("  nsig 変換 OK"); u }
            Err(e) => { println!("  ⚠ nsig 変換失敗({e})。未変換 URL で続行。"); inp.server_abr_url.clone() }
        }
    };

    println!("[4/5] VideoPlaybackAbrRequest を構築（PoToken を streamer_context に束ねる）...");
    let ustreamer = b64_decode(&inp.ustreamer_config_b64)?;
    let po_bytes = b64_decode(po_token)?; // websafe base64 → bytes（field2=po_token）
    let req_body = build_abr_request(&inp, &ustreamer, Some(&po_bytes), WEB_CLIENT_NAME_ID, WEB_CLIENT_VERSION);
    println!("  protobuf 長: {} bytes（poToken {} bytes 埋込）", req_body.len(), po_bytes.len());

    println!("[5/5] 匿名(Bearer無) で POST → メディアが返るか...");
    let mut res = attempt(http, &url_n, None, Some(&req_body))?;
    let mut v = analyze(&res);
    // SABR_REDIRECT を1回追従。
    if v.media_bytes == 0 {
        if let Some(u) = v.redirect_url.clone() {
            println!("\n  ↪ SABR_REDIRECT 追従...");
            res = attempt(http, &u, None, Some(&req_body))?;
            v = analyze(&res);
        }
    }
    // まだ 403 なら poToken を URL の pot= にも付けて再試行（束ね方の差を潰す）。
    if !(res.http_status == 200 && v.media_bytes > 0) && res.http_status == 403 {
        let sep = if url_n.contains('?') { '&' } else { '?' };
        let url_pot = format!("{url_n}{sep}pot={po_token}");
        println!("\n  ↪ pot= を URL にも付けて再 POST...");
        res = attempt(http, &url_pot, None, Some(&req_body))?;
        v = analyze(&res);
    }

    println!("\n════════════ 案2 PoC 判定 ════════════");
    if res.http_status == 200 && v.media_bytes > 0 {
        println!("✅ 白 (GO): PoToken を付けたら MEDIA {} bytes を受信＝403→200 反転を確認。", v.media_bytes);
        println!("   → ライブ復旧は技術的に可能。案1(SABR)+案2(PoToken)で実装に進める。");
        println!("   → 本番の PoToken は WebView2 で BotGuard を使い捨て実行して調達する方針（設計メモ）。");
    } else if v.protection_status == Some(3) || v.sabr_error {
        println!("❌ 黒: PoToken を付けても SABR レイヤが明示拒否（{}）。",
            if v.sabr_error { "SABR_ERROR" } else { "ATTESTATION_REQUIRED" });
        println!("   → この PoToken 種別/束ね方では通らない。token の context(web/tv)・content束ね・session束ねを要検討。");
    } else {
        println!("❌/△ 通らず: HTTP {} / メディア 0 bytes。", res.http_status);
        println!("   → PoToken(WEB/visitorData束ね)では反転せず。yt-dlp #16082 と整合（ライブ SABR は業界的に未確立）。");
        println!("   検討: ①content束ねトークン(videoId)②TVHTML5+Bearer 経路に web PoToken は不整合の可能性");
        println!("        ③player 要求と POST の visitorData/セッション一致 ④必須クエリ(rn/cpn 等)の不足。");
    }
    println!("\n（注記）PoToken は数時間で失効。反転しても継続再生/protection昇格は実装時に要確認。");
    Ok(())
}

/// 案2 PoC（本命）: TVHTML5+Bearer の SABR に、同一 session の visitorData に束ねた PoToken を載せて POST。
fn run_potoken_tv_poc(http: &reqwest::blocking::Client, gen_script: &str, video_id: &str) -> Result<()> {
    println!("=== 案2 PoC (TVHTML5+Bearer): PoToken 付き SABR (issue #16) ===");
    println!("video_id = {video_id}");
    println!("目的: SABR を返す TV+Bearer 経路に、同一 session visitorData 束ねの PoToken を載せ 403→200 反転を確認\n");

    print!("[1/6] OAuth access_token 取得... ");
    let token = match get_access_token(http) { Ok(t) => { println!("OK"); t } Err(e) => { println!("失敗"); return Err(e); } };

    // 先に visitorData+PoToken を発行（この visitorData を player 要求にも使い一貫させる）。
    println!("[2/6] PoToken を発行（node {gen_script}）...");
    let out = std::process::Command::new("node").arg(gen_script).output()
        .map_err(|e| anyhow!("node 実行失敗: {e}"))?;
    if !out.status.success() {
        bail!("PoToken 発行失敗: {}", String::from_utf8_lossy(&out.stderr));
    }
    let tok: Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| anyhow!("PoToken JSON 解析失敗: {e} / raw={}", String::from_utf8_lossy(&out.stdout)))?;
    let visitor_data = tok["visitorData"].as_str().ok_or_else(|| anyhow!("visitorData 無し"))?;
    let po_token = tok["poToken"].as_str().ok_or_else(|| anyhow!("poToken 無し"))?;
    println!("  visitorData {}文字 / poToken {}文字 発行 OK", visitor_data.len(), po_token.len());

    println!("[3/6] TVHTML5 + Bearer + visitorData + PoToken で player 取得...");
    let (player, is_live, status) = fetch_player_tv_pot(http, video_id, &token, visitor_data, po_token)?;
    println!("  playabilityStatus = {status} / is_live = {is_live}");
    let inp = match extract_sabr_inputs(&player) {
        Ok(i) => i,
        Err(e) => { println!("\n△ SABR 入力なし: {e}（hlsManifestUrl 経路のバケットかもしれない。数回リトライ）"); return Ok(()); }
    };
    println!("  serverAbrStreamingUrl OK（token 付き player 要求で発行＝認可済みのはず）");

    println!("[4/6] n= を nsig 変換...");
    let url_n = {
        let mut solver = NsigSolver::new();
        match solver.transform_url(http, &inp.server_abr_url) {
            Ok(u) => { println!("  OK"); u }
            Err(e) => { println!("  ⚠ 失敗({e})。未変換で続行"); inp.server_abr_url.clone() }
        }
    };

    println!("[5/6] VideoPlaybackAbrRequest 構築（TV client_info + PoToken 束ね）...");
    let ustreamer = b64_decode(&inp.ustreamer_config_b64)?;
    let po_bytes = b64_decode(po_token)?;
    let req_body = build_abr_request(&inp, &ustreamer, Some(&po_bytes), TV_CLIENT_NAME_ID, TV_CLIENT_VERSION);
    println!("  protobuf {} bytes（poToken {} bytes 埋込）", req_body.len(), po_bytes.len());

    println!("[6/6] Bearer 付きで POST...");
    let mut res = post_abr(http, &url_n, &token, &req_body)?;
    let mut v = analyze(&res);
    if v.media_bytes == 0 {
        if let Some(u) = v.redirect_url.clone() {
            println!("\n  ↪ SABR_REDIRECT 追従...");
            res = post_abr(http, &u, &token, &req_body)?;
            v = analyze(&res);
        }
    }
    if !(res.http_status == 200 && v.media_bytes > 0) && res.http_status == 403 {
        let sep = if url_n.contains('?') { '&' } else { '?' };
        let url_pot = format!("{url_n}{sep}pot={po_token}");
        println!("\n  ↪ pot= を URL にも付けて再 POST...");
        res = post_abr(http, &url_pot, &token, &req_body)?;
        v = analyze(&res);
    }

    println!("\n════════════ 案2 PoC (TV+Bearer) 判定 ════════════");
    if res.http_status == 200 && v.media_bytes > 0 {
        println!("✅ 白 (GO): PoToken を束ねたら MEDIA {} bytes を受信＝403→200 反転を確認。", v.media_bytes);
        println!("   → ライブ復旧は技術的に可能。案1(SABR)+案2(PoToken)で実装に進める。本番 PoToken は WebView2 で調達。");
    } else if v.protection_status == Some(3) || v.sabr_error {
        println!("❌ 黒: PoToken を束ねても SABR レイヤが明示拒否（{}）。",
            if v.sabr_error { "SABR_ERROR" } else { "ATTESTATION_REQUIRED" });
        println!("   → WEB requestKey の BotGuard token は TVHTML5 context では通らない可能性大。TV 用 attestation が要る。");
    } else {
        println!("❌/△ 通らず: HTTP {} / メディア 0 bytes（PoToken 束ね後も不変）。", res.http_status);
        println!("   → WEB(visitorData束ね)PoToken では TV+Bearer SABR の 403 は反転しない。");
        println!("   有力な解釈: PoToken の client context 不一致（WEB token≠TV context）。TVHTML5 の SABR は");
        println!("   TV 専用 attestation を要し、web BotGuard(requestKey O43z0…)では満たせない公算。yt-dlp #16082 とも整合。");
        println!("   次の検討: ①TV 用 requestKey/BotGuard 経路の有無 ②content束ね(videoId)token ③cpn/rn 等必須パラメータ。");
    }
    println!("\n（注記）PoToken は数時間で失効。visitorData は TV+Bearer 応答の responseContext から採取し束ねを一致させた。");
    Ok(())
}

enum NsigOutcome {
    TransformFailed(String),
    Reposted {
        changed: bool,
        media_bytes: usize,
        http_status: u16,
        protection_status: Option<u64>,
        sabr_error: bool,
    },
}

/// P0.5: serverAbrStreamingUrl の n= を nsig 変換し、変換後 URL へ再 POST して結果を返す。
fn run_nsig_reprobe(
    http: &reqwest::blocking::Client,
    url: &str,
    token: &str,
    req_body: &[u8],
) -> NsigOutcome {
    let orig_n = query_get_n(url);
    let mut solver = NsigSolver::new();
    let new_url = match solver.transform_url(http, url) {
        Ok(u) => u,
        Err(e) => return NsigOutcome::TransformFailed(e.to_string()),
    };
    let new_n = query_get_n(&new_url);
    let changed = orig_n != new_n && new_n.is_some();
    println!(
        "  n: {} → {} （{}）",
        orig_n.as_deref().map(short).unwrap_or_else(|| "(なし)".into()),
        new_n.as_deref().map(short).unwrap_or_else(|| "(なし)".into()),
        if changed { "変化あり" } else { "変化なし=要注意" }
    );

    let res = match post_abr(http, &new_url, token, req_body) {
        Ok(r) => r,
        Err(e) => return NsigOutcome::TransformFailed(format!("再 POST 失敗: {e}")),
    };
    let v = analyze(&res);
    NsigOutcome::Reposted {
        changed,
        media_bytes: v.media_bytes,
        http_status: res.http_status,
        protection_status: v.protection_status,
        sabr_error: v.sabr_error,
    }
}

fn query_get_n(url: &str) -> Option<String> {
    query_get_param(url, "n")
}

/// URL クエリから key の値を取り出し、%xx を簡易デコードして返す（ip= 等の診断表示用）。
fn query_get_param(url: &str, key: &str) -> Option<String> {
    let q = url.split('?').nth(1)?;
    let prefix = format!("{key}=");
    for pair in q.split('&') {
        if let Some(val) = pair.strip_prefix(prefix.as_str()) {
            // %3A(:) %25(%) 等を最低限デコード（IPv6 の ip= は %3A で来る）。
            let mut out = String::with_capacity(val.len());
            let b = val.as_bytes();
            let mut i = 0;
            while i < b.len() {
                if b[i] == b'%' && i + 2 < b.len() {
                    if let Ok(c) = u8::from_str_radix(&val[i + 1..i + 3], 16) {
                        out.push(c as char);
                        i += 3;
                        continue;
                    }
                }
                out.push(b[i] as char);
                i += 1;
            }
            return Some(out);
        }
    }
    None
}

fn short(s: &str) -> String {
    if s.len() > 14 { format!("{}…", &s[..14]) } else { s.to_string() }
}

struct Diag {
    stripped_status: u16,
    stripped_note: String,
    get_status: u16,
    nobearer_status: u16,
    any_media: bool,
}

/// 403 切り分け: n 除去 POST / GET / Bearer 無し POST を試し、状態を集める。
fn run_diagnostics(
    http: &reqwest::blocking::Client,
    url: &str,
    token: &str,
    req_body: &[u8],
) -> Diag {
    println!("\n──── 切り分け（403 の原因: n=未変換 / auth / URL レベル）────");
    let mut any_media = false;

    let stripped_url = strip_query_param(url, "n");
    let (stripped_status, stripped_note) = match attempt(http, &stripped_url, Some(token), Some(req_body)) {
        Ok(r) => {
            let media = parse_ump(&r.body).iter().filter(|p| p.part_type == UMP_MEDIA).map(|p| p.payload.len()).sum::<usize>();
            if media > 0 { any_media = true; }
            println!("  [n除去 POST] HTTP {} ct={} len={} media={}B", r.http_status, r.content_type, r.body.len(), media);
            let note = if r.http_status == 200 { "（=n がスロットル要因だった。nsig 変換で解決の可能性）".to_string() } else { String::new() };
            (r.http_status, note)
        }
        Err(e) => { println!("  [n除去 POST] エラー: {e}"); (0, String::new()) }
    };

    let get_status = match attempt(http, url, Some(token), None) {
        Ok(r) => { println!("  [GET body無] HTTP {} ct={} len={}", r.http_status, r.content_type, r.body.len()); r.http_status }
        Err(e) => { println!("  [GET body無] エラー: {e}"); 0 }
    };

    let nobearer_status = match attempt(http, url, None, Some(req_body)) {
        Ok(r) => { println!("  [Bearer無 POST] HTTP {} ct={} len={}", r.http_status, r.content_type, r.body.len()); r.http_status }
        Err(e) => { println!("  [Bearer無 POST] エラー: {e}"); 0 }
    };

    Diag { stripped_status, stripped_note, get_status, nobearer_status, any_media }
}
