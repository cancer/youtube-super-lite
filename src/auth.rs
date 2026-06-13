//! YouTube ログイン（OAuth2 認可コードフロー / ループバック）と Data API 呼び出し。
//!
//! client_secret は配布アプリに持たせない。secret が必要な処理（トークン交換・更新）は
//! バックエンド（auth-worker）に中継させる。ここが持つのは「バックエンドの URL」だけ。
//! 認証の同意画面のみブラウザに委譲し、高評価などの操作は API で行う。

use anyhow::{anyhow, bail, Result};
use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const SCOPE: &str = "https://www.googleapis.com/auth/youtube.force-ssl";
pub const DEFAULT_BACKEND: &str = "https://youtube-super-lite-backend.cancer6.workers.dev";

/// アクセストークン一式。
#[derive(Clone)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expiry: Instant,
}

impl Tokens {
    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.expiry
    }
}

#[derive(Deserialize)]
struct TokenResp {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: u64,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

impl TokenResp {
    fn into_tokens(self, fallback_refresh: Option<String>) -> Result<Tokens> {
        if let Some(err) = self.error {
            bail!(
                "認証エラー: {err}{}",
                self.error_description
                    .map(|d| format!(" ({d})"))
                    .unwrap_or_default()
            );
        }
        if self.access_token.is_empty() {
            bail!("access_token が返りませんでした");
        }
        Ok(Tokens {
            access_token: self.access_token,
            refresh_token: self.refresh_token.or(fallback_refresh),
            // 期限の少し手前で失効扱いにする。
            expiry: Instant::now() + Duration::from_secs(self.expires_in.saturating_sub(60).max(1)),
        })
    }
}

/// バックエンドから client_id を取得する。
fn fetch_client_id(backend: &str) -> Result<String> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(format!("{backend}/client_id"))
        .send()
        .map_err(|_| anyhow!("認証バックエンドに接続できません"))?;
    if !resp.status().is_success() {
        bail!("client_id 取得に失敗 ({})", resp.status());
    }
    let v: serde_json::Value = resp.json()?;
    v["client_id"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("client_id がレスポンスにありません"))
}

/// ログイン（ブラウザで同意 → ループバックで code 受領 → バックエンドでトークン交換）。ブロッキング。
pub fn login(backend: &str) -> Result<Tokens> {
    let client_id = fetch_client_id(backend)?;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let redirect = format!("http://127.0.0.1:{port}");

    let state = format!(
        "{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );

    let auth_url = format!(
        "{AUTH_URL}?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent&state={}",
        urlencode(&client_id),
        urlencode(&redirect),
        urlencode(SCOPE),
        urlencode(&state),
    );

    crate::open_in_browser(&auth_url);

    let (mut stream, _) = listener.accept()?;
    let mut request_line = String::new();
    {
        let mut reader = BufReader::new(stream.try_clone()?);
        reader.read_line(&mut request_line)?;
    }

    let (code, got_state) = parse_redirect(&request_line)?;

    let body = "<!doctype html><html><head><meta charset=\"utf-8\"><title>YouTube Super Lite</title></head>\
                <body style=\"font-family:sans-serif\"><h2>ログインが完了しました</h2>\
                <p>このタブを閉じてアプリに戻ってください。</p></body></html>";
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );

    if got_state.as_deref() != Some(state.as_str()) {
        bail!("state が一致しません（CSRF の疑い）");
    }

    exchange_code(backend, &code, &redirect)
}

/// 認可コードをバックエンド経由でトークンに交換する。
fn exchange_code(backend: &str, code: &str, redirect: &str) -> Result<Tokens> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(format!("{backend}/token"))
        .json(&serde_json::json!({ "code": code, "redirect_uri": redirect }))
        .send()
        .map_err(|_| anyhow!("認証バックエンドに接続できません"))?;
    let tr: TokenResp = resp.json()?;
    tr.into_tokens(None)
}

/// リフレッシュトークンからアクセストークンを更新する（バックエンド経由）。
pub fn refresh(backend: &str, refresh_token: &str) -> Result<Tokens> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(format!("{backend}/refresh"))
        .json(&serde_json::json!({ "refresh_token": refresh_token }))
        .send()
        .map_err(|_| anyhow!("認証バックエンドに接続できません"))?;
    let tr: TokenResp = resp.json()?;
    // 更新レスポンスには refresh_token が含まれないので元の値を保持する。
    tr.into_tokens(Some(refresh_token.to_string()))
}

/// 動画に評価を付ける（rating = "like" / "dislike" / "none"）。アクセストークンのみで可能。
pub fn rate_video(access_token: &str, video_id: &str, rating: &str) -> Result<()> {
    let client = reqwest::blocking::Client::new();
    let url = format!(
        "https://www.googleapis.com/youtube/v3/videos/rate?id={}&rating={}",
        urlencode(video_id),
        urlencode(rating)
    );
    let resp = client.post(&url).bearer_auth(access_token).send()?;
    if !resp.status().is_success() {
        let status = resp.status();
        bail!("高評価に失敗 ({status}): {}", resp.text().unwrap_or_default());
    }
    Ok(())
}

/// ログイン中アカウントのチャンネル名を取得する。
pub fn my_channel_title(access_token: &str) -> Result<String> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get("https://www.googleapis.com/youtube/v3/channels?part=snippet&mine=true")
        .bearer_auth(access_token)
        .send()?;
    if !resp.status().is_success() {
        bail!("チャンネル情報の取得に失敗 ({})", resp.status());
    }
    let v: serde_json::Value = resp.json()?;
    Ok(v["items"][0]["snippet"]["title"]
        .as_str()
        .unwrap_or("(unknown)")
        .to_string())
}

/// YouTube の URL から video id を取り出す。
pub fn extract_video_id(url: &str) -> Option<String> {
    let take = |s: &str| -> String {
        s.chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect()
    };
    if let Some(rest) = url.split("youtu.be/").nth(1) {
        let id = take(rest);
        if !id.is_empty() {
            return Some(id);
        }
    }
    for marker in ["v=", "/shorts/", "/embed/", "/live/"] {
        if let Some(rest) = url.split(marker).nth(1) {
            let id = take(rest);
            if !id.is_empty() {
                return Some(id);
            }
        }
    }
    None
}

// --- リフレッシュトークンの保存/読み込み（パッケージ外の設定ディレクトリ）---

pub(crate) fn config_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(base).join("YouTubeSuperLite")
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("YouTubeSuperLite")
    }
}

fn token_store_path() -> PathBuf {
    config_dir().join("auth.json")
}

pub fn save_refresh_token(refresh_token: &str) {
    let path = token_store_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let json = serde_json::json!({ "refresh_token": refresh_token });
    let _ = std::fs::write(&path, json.to_string());
}

pub fn load_refresh_token() -> Option<String> {
    let data = std::fs::read_to_string(token_store_path()).ok()?;
    let v: serde_json::Value = serde_json::from_str(&data).ok()?;
    v["refresh_token"].as_str().map(|s| s.to_string())
}

// --- URL ユーティリティ ---

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_redirect(request_line: &str) -> Result<(String, Option<String>)> {
    // 例: "GET /?code=XXX&state=YYY HTTP/1.1"
    let target = request_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow!("リクエスト行を解析できません"))?;
    let query = target.split('?').nth(1).unwrap_or("");

    let mut code = None;
    let mut state = None;
    let mut error = None;
    for pair in query.split('&') {
        let mut it = pair.splitn(2, '=');
        let key = it.next().unwrap_or("");
        let val = it.next().unwrap_or("");
        match key {
            "code" => code = Some(urldecode(val)),
            "state" => state = Some(urldecode(val)),
            "error" => error = Some(urldecode(val)),
            _ => {}
        }
    }

    if let Some(e) = error {
        bail!("認証が拒否されました: {e}");
    }
    let code = code.ok_or_else(|| anyhow!("認可コードが返りませんでした"))?;
    Ok((code, state))
}
