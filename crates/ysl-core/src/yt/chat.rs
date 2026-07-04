//! YouTube ライブチャットの取得（InnerTube API）。
//!
//! 公式 Data API v3 ではなく、YouTube Web が内部で使う InnerTube エンドポイントを利用する。
//! OAuth 不要・クォータ制限なし。ただし非公式のため YouTube 側の変更で壊れる可能性がある。
//!
//! フロー:
//!   1. ウォッチページ HTML から ytInitialData（continuation トークン）と INNERTUBE_API_KEY を抽出
//!   2. 配信中ライブ → /youtubei/v1/live_chat/get_live_chat
//!      終了済みライブ（リプレイ）→ /youtubei/v1/live_chat/get_live_chat_replay
//!      （リクエストに `currentPlayerState.playerOffsetMs` を載せて再生位置に同期させる）
//!   3. レスポンスの timeoutMs 間隔でポーリング（continuation を更新しながらループ）

use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

/// チャットメッセージを構成する 1 区間。テキストか画像（カスタム絵文字）。
#[derive(Clone, Debug)]
pub enum ChatRun {
    Text(String),
    /// メンバーシップスタンプ等のカスタム絵文字。`url` の画像をインライン描画する
    /// （未取得時やデコード失敗時は `alt` テキストにフォールバック）。
    Image { alt: String, url: String },
}

/// 著者の種別（バッジ）。表示の強調に使う。優先度: Owner > Moderator > Verified > Member > Normal。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorKind {
    Normal,
    Member,
    Verified,
    Moderator,
    Owner,
}

/// ライブチャットの 1 メッセージ。
#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub author: String,
    pub kind: AuthorKind,
    pub runs: Vec<ChatRun>,
}

/// 背景スレッドからメインスレッドへの通知。
pub enum ChatUpdate {
    /// 新着メッセージ。
    Messages(Vec<ChatMessage>),
    /// エラー発生（リトライ可能）。
    Error(String),
    /// ライブ配信ではない（チャットが存在しない）。
    NotLive,
}

/// ポーリング停止フラグ。
pub struct ChatStop(Arc<AtomicBool>);

impl ChatStop {
    pub fn new() -> (Self, Arc<AtomicBool>) {
        let flag = Arc::new(AtomicBool::new(false));
        (Self(flag.clone()), flag)
    }

    pub fn stop(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// 背景スレッドのエントリポイント
// ---------------------------------------------------------------------------

/// ライブチャットのポーリングループ。背景スレッドで呼び出す。
///
/// `player_offset_ms` はリプレイの場合のみ参照する（メインスレッドが mpv の time-pos を
/// 継続的に store する想定）。ライブ配信では無視される。
pub fn run_chat_poll(
    video_id: &str,
    tx: &Sender<ChatUpdate>,
    stop: &Arc<AtomicBool>,
    player_offset_ms: &Arc<AtomicI64>,
) {
    let ctx = match fetch_initial_data(video_id) {
        Ok(ctx) => ctx,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("continuation が見つかりません") {
                let _ = tx.send(ChatUpdate::NotLive);
            } else {
                let _ = tx.send(ChatUpdate::Error(msg));
            }
            return;
        }
    };

    let mut continuation = ctx.continuation;

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }

        let result = if ctx.is_replay {
            let offset = player_offset_ms.load(Ordering::Relaxed).max(0);
            poll_chat_replay(&ctx.api_key, &continuation, offset)
        } else {
            poll_chat_live(&ctx.api_key, &continuation)
        };

        match result {
            Ok((messages, next_cont, timeout_ms)) => {
                if !messages.is_empty() {
                    let _ = tx.send(ChatUpdate::Messages(messages));
                }
                match next_cont {
                    Some(c) => continuation = c,
                    None => break, // チャット終了
                }
                // ポーリング間隔を待つ（stop チェックのため小刻みに sleep）。
                // YouTube が返す timeoutMs は 5〜10 秒と長く更新が遅いため、1〜2 秒に詰める。
                sleep_interruptible(Duration::from_millis(timeout_ms.clamp(1000, 2000)), stop);
            }
            Err(e) => {
                let _ = tx.send(ChatUpdate::Error(e.to_string()));
                sleep_interruptible(Duration::from_secs(5), stop);
            }
        }
    }
}

fn sleep_interruptible(total: Duration, stop: &Arc<AtomicBool>) {
    let step = Duration::from_millis(200);
    let mut elapsed = Duration::ZERO;
    while elapsed < total {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        std::thread::sleep(step.min(total - elapsed));
        elapsed += step;
    }
}

// ---------------------------------------------------------------------------
// InnerTube API
// ---------------------------------------------------------------------------

/// ウォッチページから抽出した InnerTube 情報。
struct InnerTubeContext {
    api_key: String,
    continuation: String,
    /// 終了済みライブ配信（リプレイ）なら true。get_live_chat_replay を使う。
    is_replay: bool,
}

fn http_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .build()
        .map_err(|e| anyhow!("HTTP クライアント作成に失敗: {e}"))
}

/// ウォッチページ HTML から API キーとライブチャットの continuation トークンを抽出する。
fn fetch_initial_data(video_id: &str) -> Result<InnerTubeContext> {
    let client = http_client()?;
    let html = client
        .get(format!("https://www.youtube.com/watch?v={video_id}"))
        .send()?
        .text()?;

    let initial_data = extract_json_var(&html, "ytInitialData")?;
    let api_key = extract_api_key(&html)?;
    let (continuation, is_replay) = extract_chat_continuation(&initial_data)?;

    Ok(InnerTubeContext {
        api_key,
        continuation,
        is_replay,
    })
}

/// 配信中ライブ用ポーリング。(メッセージ一覧, 次の continuation, 待機 ms) を返す。
fn poll_chat_live(
    api_key: &str,
    continuation: &str,
) -> Result<(Vec<ChatMessage>, Option<String>, u64)> {
    let client = http_client()?;

    let body = serde_json::json!({
        "context": {
            "client": {
                "clientName": "WEB",
                "clientVersion": "2.20241001.00.00"
            }
        },
        "continuation": continuation
    });

    let resp: Value = client
        .post(format!(
            "https://www.youtube.com/youtubei/v1/live_chat/get_live_chat?key={api_key}"
        ))
        .json(&body)
        .send()?
        .json()?;

    let live_chat = &resp["continuationContents"]["liveChatContinuation"];

    // メッセージを抽出。
    let mut messages = Vec::new();
    if let Some(actions) = live_chat["actions"].as_array() {
        for action in actions {
            if let Some(msg) = parse_chat_action(action) {
                messages.push(msg);
            }
        }
    }

    // 次の continuation とポーリング間隔。
    let (next_continuation, timeout_ms) =
        extract_next_continuation(live_chat, &["timedContinuationData", "invalidationContinuationData", "reloadContinuationData"]);

    Ok((messages, next_continuation, timeout_ms))
}

/// 終了済みライブ（リプレイ）用ポーリング。
///
/// 配信中と違いリクエストに `currentPlayerState.playerOffsetMs` を載せ、再生位置に対応する
/// メッセージをサーバから受け取る。レスポンスは `replayChatItemAction` で 1 段ラップされる。
fn poll_chat_replay(
    api_key: &str,
    continuation: &str,
    player_offset_ms: i64,
) -> Result<(Vec<ChatMessage>, Option<String>, u64)> {
    let client = http_client()?;

    let body = serde_json::json!({
        "context": {
            "client": {
                "clientName": "WEB",
                "clientVersion": "2.20241001.00.00"
            }
        },
        "continuation": continuation,
        "currentPlayerState": {
            "playerOffsetMs": player_offset_ms.to_string()
        }
    });

    let resp: Value = client
        .post(format!(
            "https://www.youtube.com/youtubei/v1/live_chat/get_live_chat_replay?key={api_key}"
        ))
        .json(&body)
        .send()?
        .json()?;

    let live_chat = &resp["continuationContents"]["liveChatContinuation"];

    let mut messages = Vec::new();
    if let Some(actions) = live_chat["actions"].as_array() {
        for action in actions {
            // リプレイは replayChatItemAction.actions[] でラップされている。
            if let Some(inner) = action
                .get("replayChatItemAction")
                .and_then(|r| r.get("actions"))
                .and_then(|a| a.as_array())
            {
                for sub in inner {
                    if let Some(msg) = parse_chat_action(sub) {
                        messages.push(msg);
                    }
                }
            } else if let Some(msg) = parse_chat_action(action) {
                // 念のためラップ無しもサポート。
                messages.push(msg);
            }
        }
    }

    let (next_continuation, timeout_ms) = extract_next_continuation(
        live_chat,
        &["liveChatReplayContinuationData", "timedContinuationData"],
    );

    Ok((messages, next_continuation, timeout_ms))
}

/// `liveChatContinuation.continuations[]` から、指定キーのいずれかを優先して
/// (continuation 文字列, timeoutMs) を取り出す。
fn extract_next_continuation(live_chat: &Value, keys: &[&str]) -> (Option<String>, u64) {
    let Some(continuations) = live_chat["continuations"].as_array() else {
        return (None, 5000);
    };
    for cont in continuations {
        for key in keys {
            if let Some(data) = cont.get(key) {
                let next = data["continuation"].as_str().map(|s| s.to_string());
                let timeout = data["timeoutMs"].as_u64().unwrap_or(5000);
                if next.is_some() {
                    return (next, timeout);
                }
            }
        }
    }
    (None, 5000)
}

// ---------------------------------------------------------------------------
// HTML / JSON パース
// ---------------------------------------------------------------------------

/// HTML 内の `var NAME = {...};` から JSON を抽出する。
fn extract_json_var(html: &str, var_name: &str) -> Result<Value> {
    let marker = format!("var {var_name} = ");
    let start = html
        .find(&marker)
        .ok_or_else(|| anyhow!("{var_name} が見つかりません"))?;
    let json_start = start + marker.len();
    let rest = &html[json_start..];
    let end = find_json_end(rest)?;

    serde_json::from_str(&rest[..end])
        .map_err(|e| anyhow!("{var_name} の JSON 解析に失敗: {e}"))
}

/// 文字列先頭の JSON オブジェクトの終端位置（`}` の次）を返す。
fn find_json_end(s: &str) -> Result<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;

    for (i, ch) in s.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        match ch {
            '\\' if in_string => escape = true,
            '"' => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i + 1);
                }
            }
            _ => {}
        }
    }
    bail!("JSON の終端が見つかりません")
}

/// `"INNERTUBE_API_KEY":"..."` を抽出する。
fn extract_api_key(html: &str) -> Result<String> {
    let marker = "\"INNERTUBE_API_KEY\":\"";
    let start = html
        .find(marker)
        .ok_or_else(|| anyhow!("INNERTUBE_API_KEY が見つかりません"))?;
    let rest = &html[start + marker.len()..];
    let end = rest
        .find('"')
        .ok_or_else(|| anyhow!("INNERTUBE_API_KEY の終端が見つかりません"))?;
    Ok(rest[..end].to_string())
}

/// ytInitialData からチャットの continuation トークンと配信種別 (live / replay) を抽出する。
///
/// - ライブ: `liveChatRenderer.continuations[].reloadContinuationData.continuation`
/// - リプレイ: `liveChatRenderer.header.liveChatHeaderRenderer.viewSelector
///             .sortFilterSubMenuRenderer.subMenuItems[].continuation.reloadContinuationData.continuation`
///   subMenuItems は通常「上位のチャット」「すべてのチャット」の 2 要素を持つ。
///   `selected: true` の項目（既定では「上位のチャット」が多い）を優先し、無ければ末尾を使う。
fn extract_chat_continuation(data: &Value) -> Result<(String, bool)> {
    let renderer = &data["contents"]["twoColumnWatchNextResults"]["conversationBar"]
        ["liveChatRenderer"];

    if renderer.is_null() {
        bail!("ライブチャットの continuation が見つかりません（ライブ配信ではない可能性）");
    }

    let is_replay = renderer["isReplay"].as_bool().unwrap_or(false);

    if !is_replay {
        if let Some(arr) = renderer["continuations"].as_array() {
            for item in arr {
                for key in ["reloadContinuationData", "invalidationContinuationData"] {
                    if let Some(c) = item[key]["continuation"].as_str() {
                        return Ok((c.to_string(), false));
                    }
                }
            }
        }
        bail!("ライブチャットの continuation が見つかりません（ライブ配信ではない可能性）");
    }

    let sub_items = &renderer["header"]["liveChatHeaderRenderer"]["viewSelector"]
        ["sortFilterSubMenuRenderer"]["subMenuItems"];
    let arr = sub_items
        .as_array()
        .ok_or_else(|| anyhow!("リプレイチャットの subMenuItems が見つかりません"))?;

    let pick = arr
        .iter()
        .find(|item| item["selected"].as_bool().unwrap_or(false))
        .or_else(|| arr.last())
        .ok_or_else(|| anyhow!("リプレイチャットの subMenuItems が空です"))?;

    if let Some(c) = pick["continuation"]["reloadContinuationData"]["continuation"].as_str() {
        return Ok((c.to_string(), true));
    }
    bail!("リプレイチャットの reloadContinuationData が見つかりません")
}

/// addChatItemAction からメッセージを抽出する。
fn parse_chat_action(action: &Value) -> Option<ChatMessage> {
    let item = action.get("addChatItemAction")?.get("item")?;

    // 通常メッセージ / Super Chat のいずれかを試す。
    for key in [
        "liveChatTextMessageRenderer",
        "liveChatPaidMessageRenderer",
    ] {
        if let Some(renderer) = item.get(key) {
            return parse_text_message(renderer);
        }
    }
    None
}

fn parse_text_message(renderer: &Value) -> Option<ChatMessage> {
    let author = renderer["authorName"]["simpleText"].as_str()?.to_string();
    let runs = extract_runs(&renderer["message"]);
    if runs.is_empty() {
        return None;
    }
    let kind = parse_author_kind(&renderer["authorBadges"]);
    Some(ChatMessage { author, kind, runs })
}

/// authorBadges[] から著者種別を判定する。
/// 各 badge は `liveChatAuthorBadgeRenderer.icon.iconType`（OWNER/MODERATOR/VERIFIED）か、
/// メンバーバッジは `customThumbnail` を持つ。最も強い種別を返す。
fn parse_author_kind(badges: &Value) -> AuthorKind {
    let mut kind = AuthorKind::Normal;
    let Some(arr) = badges.as_array() else {
        return kind;
    };
    let rank = |k: AuthorKind| match k {
        AuthorKind::Normal => 0,
        AuthorKind::Member => 1,
        AuthorKind::Verified => 2,
        AuthorKind::Moderator => 3,
        AuthorKind::Owner => 4,
    };
    for b in arr {
        let r = &b["liveChatAuthorBadgeRenderer"];
        let cur = match r["icon"]["iconType"].as_str() {
            Some("OWNER") => AuthorKind::Owner,
            Some("MODERATOR") => AuthorKind::Moderator,
            Some("VERIFIED") => AuthorKind::Verified,
            _ if r.get("customThumbnail").is_some() => AuthorKind::Member,
            _ => AuthorKind::Normal,
        };
        if rank(cur) > rank(kind) {
            kind = cur;
        }
    }
    kind
}

/// message.runs[] を ChatRun の列に変換する。
///
/// 絵文字 run の構造（YouTube InnerTube）:
///   - 標準 Unicode 絵文字: `emojiId` に Unicode 文字（例: "🔥"）、`isCustomEmoji: false`
///     → `ChatRun::Text(emoji_char)` としてフォントで描画
///   - カスタム絵文字（メンバーシップスタンプ等のチャンネル固有絵文字）: `emojiId` が内部 ID、
///     `image.thumbnails[].url` に PNG 等の画像 URL、`isCustomEmoji: true`
///     → `ChatRun::Image` で URL から動的にダウンロードして描画
fn extract_runs(message: &Value) -> Vec<ChatRun> {
    let mut out: Vec<ChatRun> = Vec::new();
    let push_text = |out: &mut Vec<ChatRun>, t: &str| {
        // 連続するテキストはまとめて 1 つの Text にする（描画時のレイアウトを安定化）。
        if let Some(ChatRun::Text(last)) = out.last_mut() {
            last.push_str(t);
        } else {
            out.push(ChatRun::Text(t.to_string()));
        }
    };

    let Some(runs) = message["runs"].as_array() else {
        return out;
    };
    for run in runs {
        if let Some(t) = run["text"].as_str() {
            push_text(&mut out, t);
        } else if let Some(emoji) = run.get("emoji") {
            let is_custom = emoji["isCustomEmoji"].as_bool().unwrap_or(false);
            if !is_custom {
                if let Some(id) = emoji["emojiId"].as_str() {
                    push_text(&mut out, id);
                    continue;
                }
            }
            // カスタム絵文字 → 画像 URL を取り出して Image run に。
            let alt = emoji["shortcuts"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if let Some(url) = pick_emoji_image_url(emoji) {
                // カスタム絵文字。画像 URL を保持してインライン描画する。
                out.push(ChatRun::Image { alt, url });
            } else if !alt.is_empty() {
                // 画像 URL が無い場合は shortcut にフォールバック。
                push_text(&mut out, &alt);
            }
        }
    }
    out
}

/// emoji.image.thumbnails から適度なサイズの URL を選ぶ。
fn pick_emoji_image_url(emoji: &Value) -> Option<String> {
    let thumbs = emoji["image"]["thumbnails"].as_array()?;
    // 幅が 24-32 あたりに最も近いものを優先（無ければ最初のもの）。
    let pick = thumbs
        .iter()
        .min_by_key(|t| {
            let w = t["width"].as_u64().unwrap_or(24) as i64;
            (w - 24).abs()
        })
        .or_else(|| thumbs.first())?;
    pick["url"].as_str().map(|s| s.to_string())
}
