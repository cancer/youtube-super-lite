mod auth;
mod chat;
mod devtools;
mod gl_quad;
mod gpu_usage;
mod history;
mod image_cache;
mod mark_watched;
mod player;
mod playlist;
mod recommend;
mod resolve;
mod subscriptions;

use anyhow::{anyhow, bail, Result};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glutin::config::ConfigTemplateBuilder;
use glutin::context::{ContextApi, ContextAttributesBuilder, GlProfile, Version};
use glutin::display::GetGlDisplay;
use glutin::prelude::*;
use glutin::surface::{Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow};
use raw_window_handle::HasWindowHandle;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

/// イベントループを起こす要求（mpv の新フレーム / 背景スレッド完了）。
#[derive(Debug, Clone, Copy)]
enum UserEvent {
    MpvRedraw,
    Background,
}

/// 背景スレッド（OAuth / API 呼び出し）からの結果。
enum AuthMsg {
    LoggedIn {
        tokens: auth::Tokens,
        channel: Option<String>,
    },
    Like {
        ok: bool,
        msg: String,
        tokens: Option<auth::Tokens>,
    },
    Failed(String),
}

/// この時間だけ操作がなければ UI（URL欄・コントロール・カーソル）を隠す。
const UI_HIDE_AFTER: Duration = Duration::from_secs(3);

/// チャットパネルに保持するメッセージの上限。
const CHAT_MAX_MESSAGES: usize = 200;

/// チャットサイドパネルの幅（display points）。動画描画領域の計算にも使う。
const CHAT_PANEL_WIDTH: f32 = 320.0;

/// 既定ブラウザで URL を開く。
fn open_in_browser(url: &str) {
    if url.is_empty() {
        return;
    }
    #[cfg(target_os = "windows")]
    {
        // `cmd /C start "" <url>` は URL 中の `&`（OAuth URL に多数ある）を cmd が
        // コマンド区切りと解釈して URL が途中で切れてしまう。rundll32 の
        // FileProtocolHandler は URL を単一引数として受け取るため安全に既定ブラウザで開ける。
        let _ = std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
}

/// 秒数を mm:ss / h:mm:ss 形式の文字列にする。
fn format_time(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "--:--".to_string();
    }
    let total = secs as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// 画質（最大の縦解像度）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Quality {
    Auto,
    P2160,
    P1440,
    P1080,
    P720,
    P480,
    P360,
}

impl Quality {
    const ALL: [Quality; 7] = [
        Quality::Auto,
        Quality::P2160,
        Quality::P1440,
        Quality::P1080,
        Quality::P720,
        Quality::P480,
        Quality::P360,
    ];
    fn height(self) -> Option<u32> {
        match self {
            Quality::Auto => None,
            Quality::P2160 => Some(2160),
            Quality::P1440 => Some(1440),
            Quality::P1080 => Some(1080),
            Quality::P720 => Some(720),
            Quality::P480 => Some(480),
            Quality::P360 => Some(360),
        }
    }
    fn label(self) -> &'static str {
        match self {
            Quality::Auto => "自動",
            Quality::P2160 => "2160p",
            Quality::P1440 => "1440p",
            Quality::P1080 => "1080p",
            Quality::P720 => "720p",
            Quality::P480 => "480p",
            Quality::P360 => "360p",
        }
    }
}

/// 映像コーデック。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Codec {
    Auto,
    H264,
    Vp9,
    Av1,
}

impl Codec {
    const ALL: [Codec; 4] = [Codec::Auto, Codec::H264, Codec::Vp9, Codec::Av1];
    /// yt-dlp フォーマットフィルタの vcodec 条件。
    fn vfilter(self) -> &'static str {
        match self {
            Codec::Auto => "",
            Codec::H264 => "[vcodec^=avc1]",
            Codec::Vp9 => "[vcodec^=vp09]",
            Codec::Av1 => "[vcodec^=av01]",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Codec::Auto => "自動",
            Codec::H264 => "H.264",
            Codec::Vp9 => "VP9",
            Codec::Av1 => "AV1",
        }
    }
}

/// 画質・コーデック指定から yt-dlp の `-f` フォーマット文字列を組み立てる。
/// 厳しい条件 → 緩い条件 → 既定 の順でフォールバックし、必ず再生できるようにする。
fn build_ytdlp_format(quality: Quality, codec: Codec) -> String {
    let hf = quality
        .height()
        .map(|h| format!("[height<={h}]"))
        .unwrap_or_default();
    let cf = codec.vfilter();
    if hf.is_empty() && cf.is_empty() {
        return "bestvideo+bestaudio/best".to_string();
    }
    format!("bestvideo{hf}{cf}+bestaudio/bestvideo{hf}+bestaudio/best{hf}/bestvideo+bestaudio/best")
}

/// 指定パス候補のいずれかからフォントを読み込む。
fn load_font_from(paths: &[&str]) -> Option<Vec<u8>> {
    for p in paths {
        if let Ok(bytes) = std::fs::read(p) {
            println!("font loaded: {p}");
            return Some(bytes);
        }
    }
    None
}

/// システムの日本語フォントを探す。
fn load_system_japanese_font() -> Option<Vec<u8>> {
    #[cfg(target_os = "macos")]
    let paths: &[&str] = &[
        "/System/Library/Fonts/ヒラギノ角ゴシック W4.ttc",
        "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
    ];
    #[cfg(target_os = "windows")]
    let paths: &[&str] = &[
        r"C:\Windows\Fonts\YuGothR.ttc",
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\msgothic.ttc",
    ];
    load_font_from(paths)
}

/// システムの絵文字フォントを探す。
/// 注: ab_glyph はカラー絵文字（sbix/COLR）のビットマップ描画には完全対応していないが、
/// グリフのアウトラインがある絵文字や OS フォントのカバレッジは egui 同梱フォントより広いため、
/// 豆腐になる絵文字を減らす効果がある。
fn load_system_emoji_font() -> Option<Vec<u8>> {
    #[cfg(target_os = "macos")]
    let paths: &[&str] = &[
        "/System/Library/Fonts/Apple Color Emoji.ttc",
    ];
    #[cfg(target_os = "windows")]
    let paths: &[&str] = &[
        r"C:\Windows\Fonts\seguiemj.ttf",
    ];
    load_font_from(paths)
}

/// 画像キャッシュの保存先（パッケージ外のキャッシュディレクトリ）。
/// auth の設定ディレクトリとは別に、OS のキャッシュ領域へ置く
/// （Windows は Roaming ではなく Local、macOS は ~/Library/Caches）。
fn image_cache_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var("LOCALAPPDATA")
            .or_else(|_| std::env::var("APPDATA"))
            .unwrap_or_else(|_| ".".to_string());
        PathBuf::from(base)
            .join("YouTubeSuperLite")
            .join("image-cache")
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join("Library")
            .join("Caches")
            .join("YouTubeSuperLite")
            .join("images")
    }
}

/// egui コンテキストに日本語フォントと絵文字フォントをフォールバックとして登録する。
fn setup_japanese_font(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut to_append: Vec<String> = Vec::new();

    if let Some(bytes) = load_system_emoji_font() {
        fonts
            .font_data
            .insert("emoji".to_owned(), egui::FontData::from_owned(bytes));
        to_append.push("emoji".to_owned());
    }
    if let Some(bytes) = load_system_japanese_font() {
        fonts
            .font_data
            .insert("ja".to_owned(), egui::FontData::from_owned(bytes));
        to_append.push("ja".to_owned());
    }
    if to_append.is_empty() {
        eprintln!("warning: no system fonts found; CJK/emoji may render as tofu");
        return;
    }
    // 既存ファミリーの末尾に追加してフォールバックとして利用する。
    // 絵文字フォントを日本語フォントより前に置くと、CJK 範囲外の絵文字 codepoint で
    // 先に hit するため CJK 文字がカラー絵文字フォントの不適切なグリフで上書きされない。
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let entry = fonts.families.entry(family).or_default();
        for name in &to_append {
            entry.push(name.clone());
        }
    }
    ctx.set_fonts(fonts);
}

/// yt-dlp が PATH 上にあるか確認し、なければ同梱ディレクトリを PATH 先頭に追加する。
fn ensure_ytdlp_on_path() {
    let ytdlp_name = if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" };
    let path_sep = if cfg!(windows) { ";" } else { ":" };

    // システム PATH 上にあればそのまま使う。
    if let Ok(output) = std::process::Command::new("which")
        .arg("yt-dlp")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout);
            println!("yt-dlp found: {}", path.trim());
            return;
        }
    }

    // 同梱ディレクトリを探す。
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.to_path_buf());
            candidates.push(dir.join("tools"));
        }
    }
    candidates.push(PathBuf::from("tools"));

    for dir in &candidates {
        if dir.join(ytdlp_name).exists() {
            let current = std::env::var("PATH").unwrap_or_default();
            std::env::set_var("PATH", format!("{}{path_sep}{current}", dir.display()));
            println!("yt-dlp found: {}", dir.join(ytdlp_name).display());
            return;
        }
    }
    eprintln!("warning: yt-dlp not found; YouTube URLs may fail");
}

/// dev-tools `POST /action/<name>` で立てる intent flag の集合体。
/// redraw 開始時に取り出し、UI クリック由来の flag と OR して使う。
#[derive(Default)]
struct DevToolsPending {
    toggle_chat: bool,
    toggle_recommend: bool,
    toggle_subs: bool,
    toggle_playlist: bool,
    toggle_history: bool,
    play_pause: bool,
    login: bool,
    like: bool,
    close_overlay: bool,
}

/// 初期化済み（ウィンドウ・GL・Player がそろった）状態。
struct Running {
    egui_glow: egui_glow::EguiGlow,
    /// 動画プレイヤー（mpv + 描画先テクスチャを内包）。
    player: player::Player,
    /// 動画テクスチャを背景として描画するクワッド。
    quad: gl_quad::FullscreenQuad,
    /// 直接 GL 操作（背景クリア等）用。
    gl: Arc<glow::Context>,
    gl_context: glutin::context::PossiblyCurrentContext,
    gl_surface: Surface<WindowSurface>,
    window: Window,
    url_input: String,
    // 現在再生中の URL（ブラウザでYouTubeを開くナビゲーションに使う）。
    current_url: String,
    frames: u64,
    verbose: bool,
    // 最後に操作（マウス/キー）があった時刻と、現在 UI を表示しているか。
    last_activity: Instant,
    ui_visible: bool,
    // 画質・コーデック指定（yt-dlp のフォーマット選択に使う）。
    quality: Quality,
    codec: Codec,
    // --- 認証 / API ---
    proxy: EventLoopProxy<UserEvent>,
    backend: String,
    tokens: Option<auth::Tokens>,
    channel: Option<String>,
    auth_status: String,
    auth_busy: bool,
    auth_tx: Sender<AuthMsg>,
    auth_rx: Receiver<AuthMsg>,
    // --- ライブチャット ---
    chat_messages: Vec<chat::ChatMessage>,
    chat_tx: Sender<chat::ChatUpdate>,
    chat_rx: Receiver<chat::ChatUpdate>,
    chat_stop: Option<chat::ChatStop>,
    chat_status: String,
    chat_visible: bool,
    /// リプレイチャット用: メインスレッドが mpv の time-pos (ms) を継続的に store し、
    /// チャットスレッドが get_live_chat_replay リクエストに乗せる。
    player_offset_ms: Arc<AtomicI64>,
    // --- おすすめ動画 ---
    recommend_items: Vec<recommend::VideoItem>,
    recommend_tx: Sender<recommend::RecommendUpdate>,
    recommend_rx: Receiver<recommend::RecommendUpdate>,
    recommend_visible: bool,
    recommend_status: String,
    // --- 登録チャンネルタブ ---
    /// 左のチャンネルリスト。
    sub_channels: Vec<subscriptions::SubChannel>,
    /// 右ペイン既定: 全登録チャンネルの新着フィード。
    sub_feed: Vec<subscriptions::SubVideo>,
    sub_tx: Sender<subscriptions::SubUpdate>,
    sub_rx: Receiver<subscriptions::SubUpdate>,
    sub_visible: bool,
    sub_status: String,
    sub_busy: bool,
    // --- 再生履歴 ---
    history_items: Vec<history::HistoryItem>,
    history_tx: Sender<history::HistoryUpdate>,
    history_rx: Receiver<history::HistoryUpdate>,
    history_visible: bool,
    history_status: String,
    history_busy: bool,
    // --- 再生リスト ---
    playlist_lists: Vec<playlist::PlaylistSummary>,
    playlist_items: Vec<playlist::PlaylistItem>,
    playlist_items_title: String,
    playlist_tx: Sender<playlist::PlaylistUpdate>,
    playlist_rx: Receiver<playlist::PlaylistUpdate>,
    playlist_visible: bool,
    playlist_status: String,
    playlist_busy: bool,
    // --- チャンネル動画（登録チャンネルから開くアップロード一覧。再生リストではないのでカードUI）---
    channel_videos: Vec<playlist::PlaylistItem>,
    channel_tx: Sender<playlist::PlaylistUpdate>,
    channel_rx: Receiver<playlist::PlaylistUpdate>,
    channel_visible: bool,
    channel_status: String,
    channel_busy: bool,
    // --- ストリーム解決（yt-dlp）---
    resolve_tx: Sender<resolve::ResolveUpdate>,
    resolve_rx: Receiver<resolve::ResolveUpdate>,
    resolve_busy: bool,
    load_error: Option<String>,
    // --- dev-tools (--enable-dev-tools 時のみ Some) ---
    devtools_rx: Option<Receiver<devtools::Command>>,
    /// dev-tools の `POST /action/<name>` で立てられた intent flag を redraw 開始時に吸い上げる。
    /// 既存の UI クリック由来の flag と同じローカル変数に OR される。
    devtools_pending: DevToolsPending,
    /// `--auto-hwdec-fallback` 時のみ Some。GPU 使用率を見て mpv の hwdec を切り替える。
    gpu_monitor: Option<gpu_usage::Monitor>,
}

impl Running {
    /// 動画を読み込む。YouTube URL は背景で yt-dlp 解決してから mpv に渡す。
    fn load(&mut self, url: &str) {
        let url = url.trim().to_string();
        if url.is_empty() {
            return;
        }
        self.current_url = url.clone();
        self.load_error = None;

        // ログイン済みなら再生履歴に載せる。CLI 引数経由の起動直後は auto-login が
        // 完了する前にここに来るため tokens=None になりがちで、その場合は
        // poll_auth で LoggedIn を受け取った時点で改めて起動する。
        self.start_mark_watched_if_logged_in();

        if resolve::is_youtube_url(&url) {
            self.start_resolve(url);
        } else {
            // YouTube 以外の URL（直リンク等）はそのまま mpv に渡す。
            self.mpv_loadfile(&url, None, None);
        }
    }

    /// 現在の `current_url` の動画を再生履歴に載せる（背景スレッドで投げっぱなし）。
    /// ログインしていない、または URL から video_id を取れなければ何もしない。
    fn start_mark_watched_if_logged_in(&self) {
        let Some(tokens) = self.tokens.as_ref() else {
            eprintln!("[mark_watched] skip: not logged in (current_url={})", self.current_url);
            return;
        };
        let Some(video_id) = auth::extract_video_id(&self.current_url) else {
            eprintln!("[mark_watched] skip: no video_id in url={}", self.current_url);
            return;
        };
        eprintln!("[mark_watched] spawn for {video_id}");
        let access_token = tokens.access_token.clone();
        std::thread::spawn(move || {
            match mark_watched::mark_watched(&access_token, &video_id) {
                Ok(_) => eprintln!("[mark_watched] ok ({video_id})"),
                Err(e) => eprintln!("[mark_watched] fail ({video_id}): {e}"),
            }
        });
    }

    /// yt-dlp による解決を背景スレッドで開始する。
    fn start_resolve(&mut self, url: String) {
        self.resolve_busy = true;
        let tx = self.resolve_tx.clone();
        let proxy = self.proxy.clone();
        let format = build_ytdlp_format(self.quality, self.codec);
        std::thread::spawn(move || {
            resolve::resolve(&url, &format, &tx);
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// 解決結果を取り込み、mpv に loadfile する。
    fn poll_resolve(&mut self) {
        while let Ok(update) = self.resolve_rx.try_recv() {
            self.resolve_busy = false;
            match update {
                resolve::ResolveUpdate::Ready(r) => {
                    self.mpv_loadfile(
                        &r.video_url,
                        r.audio_url.as_deref(),
                        r.title.as_deref(),
                    );
                }
                resolve::ResolveUpdate::Error(e) => {
                    self.load_error = Some(e.clone());
                    eprintln!("resolve failed: {e}");
                }
            }
        }
    }

    /// Player に解決済み URL を渡して再生開始する。
    fn mpv_loadfile(&mut self, video_url: &str, audio_url: Option<&str>, title: Option<&str>) {
        match self.player.loadfile(video_url, audio_url, title) {
            Ok(_) => println!("loadfile: {video_url}"),
            Err(e) => {
                eprintln!("loadfile failed: {e}");
                self.load_error = Some(e.to_string());
            }
        }
    }

    /// 背景スレッドからの結果を取り込む。
    fn poll_auth(&mut self) {
        while let Ok(msg) = self.auth_rx.try_recv() {
            match msg {
                AuthMsg::LoggedIn { tokens, channel } => {
                    if let Some(rt) = &tokens.refresh_token {
                        auth::save_refresh_token(rt);
                    }
                    self.tokens = Some(tokens);
                    self.channel = channel;
                    self.auth_busy = false;
                    self.auth_status = match &self.channel {
                        Some(name) => format!("ログイン中: {name}"),
                        None => "ログイン済み".to_string(),
                    };
                    // CLI 引数経由で既に load() を通った動画がここで履歴に載る。
                    self.start_mark_watched_if_logged_in();
                }
                AuthMsg::Like { ok, msg, tokens } => {
                    if let Some(t) = tokens {
                        self.tokens = Some(t);
                    }
                    self.auth_busy = false;
                    self.auth_status = msg;
                    let _ = ok;
                }
                AuthMsg::Failed(e) => {
                    self.auth_busy = false;
                    self.auth_status = format!("エラー: {e}");
                }
            }
        }
    }

    /// ログイン（ブラウザで承認 → バックエンドでトークン取得 → チャンネル名取得）を背景で開始。
    fn start_login(&mut self) {
        self.auth_busy = true;
        self.auth_status = "ブラウザで承認してください…".to_string();
        let backend = self.backend.clone();
        let tx = self.auth_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let result = auth::login(&backend).map(|tokens| {
                let channel = auth::my_channel_title(&tokens.access_token).ok();
                (tokens, channel)
            });
            let _ = match result {
                Ok((tokens, channel)) => tx.send(AuthMsg::LoggedIn { tokens, channel }),
                Err(e) => tx.send(AuthMsg::Failed(e.to_string())),
            };
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// 保存済みリフレッシュトークンで自動ログインを背景で開始。
    fn start_silent_login(&mut self, refresh_token: String) {
        self.auth_busy = true;
        self.auth_status = "ログイン情報を復元中…".to_string();
        let backend = self.backend.clone();
        let tx = self.auth_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let result = auth::refresh(&backend, &refresh_token).map(|tokens| {
                let channel = auth::my_channel_title(&tokens.access_token).ok();
                (tokens, channel)
            });
            let _ = match result {
                Ok((tokens, channel)) => tx.send(AuthMsg::LoggedIn { tokens, channel }),
                Err(e) => tx.send(AuthMsg::Failed(e.to_string())),
            };
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// チャット更新を取り込む。
    fn poll_chat(&mut self) {
        while let Ok(update) = self.chat_rx.try_recv() {
            match update {
                chat::ChatUpdate::Messages(msgs) => {
                    self.chat_messages.extend(msgs);
                    // 上限を超えたら古いメッセージを捨てる。
                    if self.chat_messages.len() > CHAT_MAX_MESSAGES {
                        let excess = self.chat_messages.len() - CHAT_MAX_MESSAGES;
                        self.chat_messages.drain(..excess);
                    }
                    self.chat_status = format!("チャット ({} 件)", self.chat_messages.len());
                }
                chat::ChatUpdate::Error(e) => {
                    self.chat_status = format!("チャットエラー: {e}");
                }
                chat::ChatUpdate::NotLive => {
                    self.chat_status.clear();
                    self.stop_chat();
                }
            }
        }
    }

    /// ライブチャットのポーリングを背景スレッドで開始する。
    fn start_chat(&mut self, video_id: String) {
        self.stop_chat();
        self.chat_messages.clear();
        self.chat_status = "チャット接続中…".to_string();
        self.chat_visible = true;

        let (stopper, stop_flag) = chat::ChatStop::new();
        self.chat_stop = Some(stopper);

        let tx = self.chat_tx.clone();
        let proxy = self.proxy.clone();
        let offset = Arc::clone(&self.player_offset_ms);
        std::thread::spawn(move || {
            chat::run_chat_poll(&video_id, &tx, &stop_flag, &offset);
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// おすすめ動画の更新を取り込む。
    fn poll_recommend(&mut self) {
        while let Ok(update) = self.recommend_rx.try_recv() {
            match update {
                recommend::RecommendUpdate::Items(items) => {
                    self.recommend_status = format!("おすすめ ({} 件)", items.len());
                    self.recommend_items = items;
                }
                recommend::RecommendUpdate::Error(e) => {
                    self.recommend_status = format!("取得エラー: {e}");
                }
            }
        }
    }

    /// おすすめ動画を背景スレッドで取得する。
    fn start_recommend(&mut self, video_id: String) {
        self.recommend_items.clear();
        self.recommend_status = "おすすめ取得中…".to_string();
        let tx = self.recommend_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            recommend::fetch_recommendations(&video_id, &tx);
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// 登録チャンネルタブの更新を取り込む（新着フィード + チャンネルリスト）。
    fn poll_subs(&mut self) {
        while let Ok(update) = self.sub_rx.try_recv() {
            match update {
                subscriptions::SubUpdate::Feed(items) => {
                    // 新着フィードの取得完了でスピナーを止める（こちらが右ペイン主役）。
                    self.sub_busy = false;
                    self.sub_status = "新着".to_string();
                    self.sub_feed = items;
                }
                subscriptions::SubUpdate::Channels(channels) => {
                    self.sub_channels = channels;
                }
                subscriptions::SubUpdate::Error(e) => {
                    self.sub_busy = false;
                    self.sub_status = format!("取得エラー: {e}");
                }
            }
        }
    }

    /// 登録チャンネルタブのデータを背景スレッドで取得する。
    /// 新着フィード（右ペイン既定）とチャンネルリスト（左）を並行取得する。
    fn start_subs(&mut self) {
        let Some(tokens) = &self.tokens else {
            self.sub_status = "先にログインしてください".to_string();
            return;
        };
        if self.sub_busy {
            return;
        }
        self.sub_busy = true;
        self.sub_status = "新着を取得中…".to_string();
        self.sub_visible = true;

        let access_token = tokens.access_token.clone();

        // 1. 新着フィード（InnerTube FEsubscriptions）。
        let tx = self.sub_tx.clone();
        let proxy = self.proxy.clone();
        let token = access_token.clone();
        std::thread::spawn(move || {
            subscriptions::fetch_subscription_feed(&token, &tx);
            let _ = proxy.send_event(UserEvent::Background);
        });

        // 2. 左のチャンネルリスト（Data API subscriptions.list）。
        let tx = self.sub_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            subscriptions::fetch_subscribed_channels(&access_token, &tx);
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// GPU 使用率監視スレッドからの hwdec 切替決定を取り込んで mpv に反映する。
    fn poll_gpu_usage(&mut self) {
        let Some(monitor) = self.gpu_monitor.as_ref() else {
            return;
        };
        while let Some(decision) = monitor.try_recv() {
            match decision {
                gpu_usage::HwdecDecision::UseSoftware => {
                    eprintln!("[auto-hwdec] GPU 高負荷検出 → SW デコードへ切替");
                    self.player.set_hwdec("no");
                }
                gpu_usage::HwdecDecision::UseHardware => {
                    eprintln!("[auto-hwdec] GPU 負荷低下 → HW デコードへ復帰");
                    self.player.set_hwdec("auto");
                }
            }
        }
    }

    /// 再生履歴の更新を取り込む。
    fn poll_history(&mut self) {
        while let Ok(update) = self.history_rx.try_recv() {
            self.history_busy = false;
            match update {
                history::HistoryUpdate::Items(items) => {
                    self.history_status = format!("再生履歴 ({} 件)", items.len());
                    self.history_items = items;
                }
                history::HistoryUpdate::Error(e) => {
                    self.history_status = format!("取得エラー: {e}");
                }
            }
        }
    }

    /// 再生履歴を背景スレッドで取得する。
    fn start_history(&mut self) {
        let Some(tokens) = &self.tokens else {
            self.history_status = "先にログインしてください".to_string();
            return;
        };
        if self.history_busy {
            return;
        }
        self.history_busy = true;
        self.history_status = "再生履歴を取得中…".to_string();
        self.history_visible = true;

        let access_token = tokens.access_token.clone();
        let tx = self.history_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            history::fetch_history(&access_token, &tx);
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// 再生リストの更新を取り込む。
    fn poll_playlist(&mut self) {
        while let Ok(update) = self.playlist_rx.try_recv() {
            self.playlist_busy = false;
            match update {
                playlist::PlaylistUpdate::Playlists(lists) => {
                    self.playlist_status = format!("再生リスト ({} 件)", lists.len());
                    self.playlist_lists = lists;
                    // リスト一覧に戻ったので動画一覧をクリア。
                    self.playlist_items.clear();
                    self.playlist_items_title.clear();
                }
                playlist::PlaylistUpdate::Items { title, items } => {
                    self.playlist_status = format!("{title} ({} 件)", items.len());
                    self.playlist_items_title = title;
                    self.playlist_items = items;
                }
                playlist::PlaylistUpdate::Error(e) => {
                    self.playlist_status = format!("取得エラー: {e}");
                }
            }
        }
    }

    /// 自分の再生リスト一覧を背景スレッドで取得する。
    fn start_playlist_list(&mut self) {
        let Some(tokens) = &self.tokens else {
            self.playlist_status = "先にログインしてください".to_string();
            return;
        };
        if self.playlist_busy {
            return;
        }
        self.playlist_busy = true;
        self.playlist_lists.clear();
        self.playlist_items.clear();
        self.playlist_items_title.clear();
        self.playlist_status = "再生リスト取得中…".to_string();
        self.playlist_visible = true;

        let access_token = tokens.access_token.clone();
        let tx = self.playlist_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            playlist::fetch_my_playlists(&access_token, &tx);
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// 選択した再生リストの動画一覧を背景スレッドで取得する。
    fn start_playlist_items(&mut self, playlist_id: String, title: String) {
        let Some(tokens) = &self.tokens else {
            return;
        };
        if self.playlist_busy {
            return;
        }
        self.playlist_busy = true;
        self.playlist_status = format!("{title} を読み込み中…");

        let access_token = tokens.access_token.clone();
        let tx = self.playlist_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            playlist::fetch_playlist_items(&access_token, &playlist_id, &title, &tx);
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    fn poll_channel(&mut self) {
        while let Ok(update) = self.channel_rx.try_recv() {
            self.channel_busy = false;
            match update {
                playlist::PlaylistUpdate::Items { title, items } => {
                    self.channel_status = title;
                    self.channel_videos = items;
                }
                playlist::PlaylistUpdate::Error(e) => {
                    self.channel_status = format!("取得エラー: {e}");
                }
                // チャンネルのアップロード取得では Playlists は来ない。
                playlist::PlaylistUpdate::Playlists(_) => {}
            }
        }
    }

    /// 登録チャンネルのアップロード動画一覧をカード UI 用に背景スレッドで取得する。
    /// 内部的にはアップロード再生リスト(UU…)を `fetch_playlist_items` で取るが、
    /// これは「再生リスト」ではないので すべて再生/シャッフル は出さず、専用オーバーレイで
    /// カードグリッド表示する。
    fn start_channel_uploads(&mut self, uploads_id: String, title: String) {
        let Some(tokens) = &self.tokens else {
            self.channel_status = "先にログインしてください".to_string();
            return;
        };
        if self.channel_busy {
            return;
        }
        self.channel_busy = true;
        self.channel_visible = true;
        self.channel_videos.clear();
        self.channel_status = format!("{title} を読み込み中…");

        let access_token = tokens.access_token.clone();
        let tx = self.channel_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            playlist::fetch_playlist_items(&access_token, &uploads_id, &title, &tx);
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// ライブチャットのポーリングを停止する。
    fn stop_chat(&mut self) {
        if let Some(stopper) = self.chat_stop.take() {
            stopper.stop();
        }
    }

    /// 現在の動画に高評価を付ける（必要ならトークンを更新してから）を背景で開始。
    fn start_like(&mut self, video_id: String) {
        let Some(tokens) = self.tokens.clone() else {
            self.auth_status = "先にログインしてください".to_string();
            return;
        };
        self.auth_busy = true;
        self.auth_status = "高評価を送信中…".to_string();
        let backend = self.backend.clone();
        let tx = self.auth_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<auth::Tokens> {
                let mut t = tokens;
                if t.is_expired() {
                    if let Some(rt) = t.refresh_token.clone() {
                        t = auth::refresh(&backend, &rt)?;
                    }
                }
                auth::rate_video(&t.access_token, &video_id, "like")?;
                Ok(t)
            })();
            let _ = match result {
                Ok(t) => tx.send(AuthMsg::Like {
                    ok: true,
                    msg: "👍 高評価しました".to_string(),
                    tokens: Some(t),
                }),
                Err(e) => tx.send(AuthMsg::Like {
                    ok: false,
                    msg: format!("高評価に失敗: {e}"),
                    tokens: None,
                }),
            };
            let _ = proxy.send_event(UserEvent::Background);
        });
    }

    /// 1 フレーム描画する：mpv 動画 → egui UI の順に重ねて表示。
    fn redraw(&mut self) {
        // リプレイチャット用に現在の再生位置 (ms) を共有する。チャット非表示でも軽量なので毎回更新。
        self.player_offset_ms
            .store((self.player.time_pos() * 1000.0) as i64, Ordering::Relaxed);

        self.poll_auth();
        self.poll_chat();
        self.poll_recommend();
        self.poll_subs();
        self.poll_history();
        self.poll_playlist();
        self.poll_channel();
        self.poll_gpu_usage();
        self.poll_resolve();

        let _ = self.gl_context.make_current(&self.gl_surface);

        let size = self.window.inner_size();
        let (w, h) = (size.width.max(1) as i32, size.height.max(1) as i32);

        // チャットパネルが表示される条件と一致させて、動画の描画領域を決める。
        // CHAT_PANEL_WIDTH は egui の論理ポイント単位。
        // self.window.inner_size() は物理ピクセル単位（Retina では 2 倍）なので
        // scale_factor をかけて物理ピクセルに変換する必要がある。
        let scale = self.window.scale_factor() as f32;
        let chat_panel_w: i32 = (CHAT_PANEL_WIDTH * scale) as i32;
        let chat_visible_now = self.chat_visible && !self.chat_messages.is_empty();
        let video_w = if chat_visible_now {
            (w - chat_panel_w).max(1)
        } else {
            w
        };

        // 背景を黒でクリア（動画が描かれない領域のため）。
        unsafe {
            use glow::HasContext;
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            self.gl.viewport(0, 0, w, h);
            self.gl.clear_color(0.0, 0.0, 0.0, 1.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }

        // 動画フレームを Player 内のテクスチャに描画（動画領域のアスペクトで）。
        // egui の Y 軸はウィンドウ上から下、OpenGL は下から上なので、
        // チャットが右側なら動画ビューポートは (0, 0, video_w, h) で左下原点から左半分を覆う。
        self.player.render(video_w, h);
        self.quad.draw(self.player.texture(), (0, 0, video_w, h));

        // 現在の再生状態を Player から取得。
        let paused = self.player.paused();
        let time_pos = self.player.time_pos();
        let duration = self.player.duration();
        let seekable_stream = self.player.seekable();
        let volume = self.player.volume();
        let muted = self.player.muted();
        let title = self.player.media_title();

        // 一定時間操作がなければ UI を隠す。表示状態が変わったらカーソルも合わせる。
        let show_ui = self.last_activity.elapsed() < UI_HIDE_AFTER;
        if show_ui != self.ui_visible {
            self.window.set_cursor_visible(show_ui);
            self.ui_visible = show_ui;
        }

        // 上に egui の UI を重ねる。mpv は 'static 参照なのでクロージャ内から直接操作できる。
        // 認証 UI 用の状態。
        let logged_in = self.tokens.is_some();
        let auth_busy = self.auth_busy;
        let auth_status = self.auth_status.clone();
        let channel = self.channel.clone();
        let video_id = auth::extract_video_id(&self.current_url);

        let chat_messages = &self.chat_messages;
        let chat_status = self.chat_status.clone();
        let chat_visible = self.chat_visible;

        let recommend_items = &self.recommend_items;
        let recommend_visible = self.recommend_visible;
        let recommend_status = self.recommend_status.clone();

        let sub_channels = &self.sub_channels;
        let sub_feed = &self.sub_feed;
        let sub_visible = self.sub_visible;
        let sub_busy = self.sub_busy;

        let history_items = &self.history_items;
        let history_visible = self.history_visible;
        let history_status = self.history_status.clone();
        let history_busy = self.history_busy;

        let playlist_lists = &self.playlist_lists;
        let playlist_items = &self.playlist_items;
        let playlist_items_title = self.playlist_items_title.clone();
        let playlist_visible = self.playlist_visible;
        let playlist_status = self.playlist_status.clone();
        let playlist_busy = self.playlist_busy;

        let channel_videos = &self.channel_videos;
        let channel_visible = self.channel_visible;
        let channel_status = self.channel_status.clone();
        let channel_busy = self.channel_busy;

        let resolve_busy = self.resolve_busy;
        let load_error = self.load_error.clone();
        // 中央オーバーレイ判定用: URL がまだ一度も渡されていない初期状態では
        // 「再生準備中…」のような誤った表示を抑制する。
        let url_set = !self.current_url.is_empty();

        let player = &self.player;
        let window = &self.window;
        let url_input = &mut self.url_input;
        let mut to_load: Option<String> = None;
        // dev-tools 由来の intent flag を吸い上げる（取り出した側はクリアして次フレームに残さない）。
        let pending = std::mem::take(&mut self.devtools_pending);
        let mut login_clicked = pending.login;
        let mut like_clicked = pending.like;
        let mut toggle_chat = pending.toggle_chat;
        let mut toggle_recommend = pending.toggle_recommend;
        let mut toggle_subs = pending.toggle_subs;
        let mut toggle_history = pending.toggle_history;
        let mut toggle_playlist = pending.toggle_playlist;
        let mut pick_playlist: Option<(String, String)> = None; // (id, title)
        let mut pick_channel: Option<(String, String)> = None; // (channel_id, title)
        let mut sub_back_to_feed = false; // チャンネル絞り込み → 新着一覧へ戻る
        let mut playlist_back = false;
        let devtools_close_overlay = pending.close_overlay;
        let devtools_play_pause = pending.play_pause;
        // loading オーバーレイの spinner をアニメさせるため、closure 内で立てる。
        let mut loading_spinning = false;
        let mut pick_video: Option<String> = None;
        // 画質・コーデックの選択（コンボボックスで書き換え、run 後に変更検出）。
        let mut sel_quality = self.quality;
        let mut sel_codec = self.codec;
        self.egui_glow.run(window, |ctx| {
            // キーボードショートカットは UI 非表示中も有効（URL 欄入力中のみ無効）。
            if !ctx.wants_keyboard_input() {
                ctx.input(|i| {
                    if i.key_pressed(egui::Key::Space) || devtools_play_pause {
                        player.set_paused(!paused);
                    }
                    if i.key_pressed(egui::Key::ArrowRight) {
                        player.seek_relative(5.0);
                    }
                    if i.key_pressed(egui::Key::ArrowLeft) {
                        player.seek_relative(-5.0);
                    }
                    if i.key_pressed(egui::Key::ArrowUp) {
                        player.set_volume((volume + 5.0).min(130.0));
                    }
                    if i.key_pressed(egui::Key::ArrowDown) {
                        player.set_volume((volume - 5.0).max(0.0));
                    }
                    // Escape で最前面のオーバーレイを閉じる。dev-tools の close_overlay も同じ経路。
                    if i.key_pressed(egui::Key::Escape) || devtools_close_overlay {
                        if playlist_visible {
                            toggle_playlist = true;
                        } else if history_visible {
                            toggle_history = true;
                        } else if sub_visible {
                            toggle_subs = true;
                        } else if recommend_visible {
                            toggle_recommend = true;
                        } else if chat_visible {
                            toggle_chat = true;
                        }
                    }
                });
            }

            // おすすめ動画オーバーレイ（プレーヤー全体を覆う）。
            if recommend_visible && !recommend_items.is_empty() {
                let screen = ctx.screen_rect();
                egui::Area::new(egui::Id::new("recommend_overlay"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(screen.min)
                    .show(ctx, |ui| {
                        let frame = egui::Frame::none()
                            .fill(egui::Color32::from_black_alpha(220))
                            .inner_margin(16.0);
                        frame.show(ui, |ui| {
                            // Frame の inner_margin (16) を考慮して内部コンテンツサイズを固定。
                            let inner = screen.size() - egui::vec2(32.0, 32.0);
                            ui.set_min_size(inner);
                            ui.set_max_size(inner);

                            if draw_overlay_header(ui, &recommend_status, false) {
                                toggle_recommend = true;
                            }
                            ui.separator();

                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    let cards: Vec<GridCard> = recommend_items
                                        .iter()
                                        .map(|item| GridCard {
                                            video_id: item.video_id.clone(),
                                            title: item.title.clone(),
                                            channel: item.channel.clone(),
                                            duration: item.duration.clone(),
                                            meta: item.view_count.clone(),
                                            channel_icon: String::new(),
                                        })
                                        .collect();
                                    if let Some(id) = draw_video_grid(ui, &cards) {
                                        pick_video = Some(format!(
                                            "https://www.youtube.com/watch?v={id}"
                                        ));
                                        toggle_recommend = true;
                                    }
                                });
                        });
                    });
            }

            // 登録チャンネル一覧（タブ＝全画面オーバーレイ。ファーストビューには重ねない）。
            if sub_visible {
                let screen = ctx.screen_rect();
                egui::Area::new(egui::Id::new("subs_overlay"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(screen.min)
                    .show(ctx, |ui| {
                        let frame = egui::Frame::none()
                            .fill(egui::Color32::from_black_alpha(230))
                            .inner_margin(16.0);
                        frame.show(ui, |ui| {
                            let inner = screen.size() - egui::vec2(32.0, 32.0);
                            ui.set_min_size(inner);
                            ui.set_max_size(inner);

                            // 見出しテキスト（「新着」）は不要。閉じるボタンと
                            // ローディングのスピナーだけ出す。
                            if draw_overlay_header(ui, "", sub_busy) {
                                toggle_subs = true;
                            }
                            ui.separator();

                            // 左: 登録チャンネルリスト（狭いカラム、常時表示）。
                            // 右: 選択したチャンネルのアップロード動画をカード UI で表示。
                            // 「再生リスト」ではないので すべて再生/シャッフル は出さない。
                            const CHANNEL_LIST_WIDTH: f32 = 240.0;
                            ui.horizontal_top(|ui| {
                                // --- 左ペイン: チャンネルリスト ---
                                ui.allocate_ui_with_layout(
                                    egui::vec2(CHANNEL_LIST_WIDTH, ui.available_height()),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        ui.set_min_width(CHANNEL_LIST_WIDTH);
                                        ui.set_max_width(CHANNEL_LIST_WIDTH);

                                        if sub_channels.is_empty() && !sub_busy {
                                            ui.label(
                                                egui::RichText::new("登録チャンネルがありません")
                                                    .color(egui::Color32::GRAY),
                                            );
                                        }

                                        egui::ScrollArea::vertical()
                                            .id_source("subs_channel_list")
                                            .auto_shrink([false, false])
                                            .show(ui, |ui| {
                                                for ch in sub_channels {
                                                    let resp = ui
                                                        .horizontal(|ui| {
                                                            ui.set_min_width(
                                                                ui.available_width(),
                                                            );
                                                            if ch.icon.is_empty() {
                                                                ui.add_space(28.0);
                                                            } else {
                                                                ui.add(
                                                                    egui::Image::new(&ch.icon)
                                                                        .fit_to_exact_size(
                                                                            egui::vec2(
                                                                                28.0, 28.0,
                                                                            ),
                                                                        )
                                                                        .rounding(
                                                                            egui::Rounding::same(
                                                                                14.0,
                                                                            ),
                                                                        ),
                                                                );
                                                            }
                                                            ui.add_space(8.0);
                                                            ui.add(
                                                                egui::Label::new(
                                                                    egui::RichText::new(
                                                                        &ch.title,
                                                                    )
                                                                    .color(egui::Color32::WHITE),
                                                                )
                                                                .truncate()
                                                                .selectable(false),
                                                            );
                                                        })
                                                        .response;
                                                    if resp
                                                        .interact(egui::Sense::click())
                                                        .on_hover_cursor(
                                                            egui::CursorIcon::PointingHand,
                                                        )
                                                        .clicked()
                                                    {
                                                        pick_channel = Some((
                                                            ch.channel_id.clone(),
                                                            ch.title.clone(),
                                                        ));
                                                    }
                                                    ui.add_space(6.0);
                                                }
                                            });
                                    },
                                );

                                ui.separator();

                                // --- 右ペイン ---
                                // 既定: 全登録チャンネルの新着一覧。
                                // チャンネル選択時: そのチャンネルのアップロード一覧に絞り込み。
                                ui.vertical(|ui| {
                                    // 新着モードでは見出しを出さない（タブ自体が新着なので冗長）。
                                    // チャンネル絞り込み時のみ「← 新着」とチャンネル名を出す。
                                    if channel_visible {
                                        ui.horizontal(|ui| {
                                            if ui.button("← 新着").clicked() {
                                                sub_back_to_feed = true;
                                            }
                                            ui.label(
                                                egui::RichText::new(&channel_status)
                                                    .color(egui::Color32::WHITE)
                                                    .heading(),
                                            );
                                            if channel_busy {
                                                ui.spinner();
                                            }
                                        });
                                        ui.add_space(4.0);
                                    }

                                    let cards: Vec<GridCard> = if channel_visible {
                                        channel_videos
                                            .iter()
                                            .map(|item| GridCard {
                                                video_id: item.video_id.clone(),
                                                title: item.title.clone(),
                                                channel: item.channel.clone(),
                                                duration: String::new(),
                                                meta: String::new(),
                                                channel_icon: String::new(),
                                            })
                                            .collect()
                                    } else {
                                        sub_feed
                                            .iter()
                                            .map(|item| GridCard {
                                                video_id: item.video_id.clone(),
                                                title: item.title.clone(),
                                                channel: item.channel.clone(),
                                                duration: item.duration.clone(),
                                                meta: item.meta.clone(),
                                                channel_icon: item.channel_icon.clone(),
                                            })
                                            .collect()
                                    };

                                    egui::ScrollArea::vertical()
                                        .id_source("subs_right_pane")
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            if let Some(id) = draw_video_grid(ui, &cards) {
                                                pick_video = Some(format!(
                                                    "https://www.youtube.com/watch?v={id}"
                                                ));
                                                toggle_subs = true;
                                            }
                                        });
                                });
                            });
                        });
                    });
            }

            // 再生履歴オーバーレイ。
            if history_visible {
                let screen = ctx.screen_rect();
                egui::Area::new(egui::Id::new("history_overlay"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(screen.min)
                    .show(ctx, |ui| {
                        let frame = egui::Frame::none()
                            .fill(egui::Color32::from_black_alpha(220))
                            .inner_margin(16.0);
                        frame.show(ui, |ui| {
                            let inner = screen.size() - egui::vec2(32.0, 32.0);
                            ui.set_min_size(inner);
                            ui.set_max_size(inner);

                            if draw_overlay_header(ui, &history_status, history_busy) {
                                toggle_history = true;
                            }
                            ui.separator();

                            if history_items.is_empty() && !history_busy {
                                ui.label(
                                    egui::RichText::new("再生履歴がありません")
                                        .color(egui::Color32::GRAY),
                                );
                            }

                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    let cards: Vec<GridCard> = history_items
                                        .iter()
                                        .map(|item| GridCard {
                                            video_id: item.video_id.clone(),
                                            title: item.title.clone(),
                                            channel: item.channel.clone(),
                                            duration: item.duration.clone(),
                                            meta: item.view_count.clone(),
                                            channel_icon: String::new(),
                                        })
                                        .collect();
                                    if let Some(id) = draw_video_grid(ui, &cards) {
                                        pick_video = Some(format!(
                                            "https://www.youtube.com/watch?v={id}"
                                        ));
                                        toggle_history = true;
                                    }
                                });
                        });
                    });
            }

            // 再生リストオーバーレイ（2段階: 一覧 → 動画リスト）。
            if playlist_visible {
                let screen = ctx.screen_rect();
                egui::Area::new(egui::Id::new("playlist_overlay"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(screen.min)
                    .show(ctx, |ui| {
                        let frame = egui::Frame::none()
                            .fill(egui::Color32::from_black_alpha(220))
                            .inner_margin(16.0);
                        frame.show(ui, |ui| {
                            // Frame の inner_margin (16) を考慮して内部コンテンツサイズを固定。
                            let inner = screen.size() - egui::vec2(32.0, 32.0);
                            ui.set_min_size(inner);
                            ui.set_max_size(inner);

                            // ヘッダー（左に戻る・タイトル・スピナー、右に「✕ 閉じる」）。
                            ui.horizontal(|ui| {
                                if !playlist_items.is_empty()
                                    && ui.button("← 一覧").clicked()
                                {
                                    playlist_back = true;
                                }
                                ui.label(
                                    egui::RichText::new(&playlist_status)
                                        .color(egui::Color32::WHITE)
                                        .heading(),
                                );
                                if playlist_busy {
                                    ui.spinner();
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.button("閉じる").clicked() {
                                            toggle_playlist = true;
                                        }
                                    },
                                );
                            });
                            ui.separator();

                            if !playlist_items.is_empty() {
                                // --- 2 ペイン表示: 左に概要、右に動画行リスト ---
                                let first_id = playlist_items
                                    .first()
                                    .map(|i| i.video_id.clone())
                                    .unwrap_or_default();

                                ui.horizontal_top(|ui| {
                                    // 左ペイン: プレイリスト概要 + アクション。
                                    let left_w: f32 = 280.0;
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(left_w, ui.available_height()),
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            ui.set_min_width(left_w);
                                            ui.set_max_width(left_w);

                                            if !first_id.is_empty() {
                                                let thumb_h = left_w * 9.0 / 16.0;
                                                ui.add(
                                                    egui::Image::new(format!(
                                                        "https://i.ytimg.com/vi/{first_id}/mqdefault.jpg"
                                                    ))
                                                    .rounding(8.0)
                                                    .fit_to_exact_size(egui::vec2(
                                                        left_w, thumb_h,
                                                    )),
                                                );
                                            }

                                            ui.add_space(12.0);
                                            ui.label(
                                                egui::RichText::new(&playlist_items_title)
                                                    .color(egui::Color32::WHITE)
                                                    .heading(),
                                            );
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{} 本",
                                                    playlist_items.len()
                                                ))
                                                .color(egui::Color32::from_rgb(170, 170, 170))
                                                .small(),
                                            );

                                            ui.add_space(12.0);
                                            ui.horizontal(|ui| {
                                                if ui.button("▶ すべて再生").clicked() {
                                                    if let Some(item) = playlist_items.first() {
                                                        pick_video = Some(format!(
                                                            "https://www.youtube.com/watch?v={}",
                                                            item.video_id
                                                        ));
                                                        toggle_playlist = true;
                                                    }
                                                }
                                                if ui.button("🔀 シャッフル").clicked() {
                                                    let nanos = std::time::SystemTime::now()
                                                        .duration_since(std::time::UNIX_EPOCH)
                                                        .map(|d| d.subsec_nanos())
                                                        .unwrap_or(0);
                                                    let idx = (nanos as usize)
                                                        % playlist_items.len();
                                                    pick_video = Some(format!(
                                                        "https://www.youtube.com/watch?v={}",
                                                        playlist_items[idx].video_id
                                                    ));
                                                    toggle_playlist = true;
                                                }
                                            });
                                        },
                                    );

                                    ui.add_space(16.0);

                                    // 右ペイン: 動画行リスト。
                                    ui.vertical(|ui| {
                                        egui::ScrollArea::vertical()
                                            .auto_shrink([false, false])
                                            .show(ui, |ui| {
                                                for (i, item) in
                                                    playlist_items.iter().enumerate()
                                                {
                                                    if draw_playlist_row(ui, item, i + 1) {
                                                        pick_video = Some(format!(
                                                            "https://www.youtube.com/watch?v={}",
                                                            item.video_id
                                                        ));
                                                        toggle_playlist = true;
                                                    }
                                                    ui.add_space(4.0);
                                                }
                                            });
                                    });
                                });
                            } else if !playlist_lists.is_empty() {
                                // --- 再生リスト一覧表示 ---
                                egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    for pl in playlist_lists {
                                        let resp = ui
                                            .horizontal(|ui| {
                                                ui.set_min_width(ui.available_width());
                                                ui.vertical(|ui| {
                                                    ui.label(
                                                        egui::RichText::new(&pl.title)
                                                            .color(egui::Color32::WHITE)
                                                            .strong(),
                                                    );
                                                    if pl.item_count > 0 {
                                                        ui.label(
                                                            egui::RichText::new(format!(
                                                                "{} 本",
                                                                pl.item_count
                                                            ))
                                                            .color(egui::Color32::from_rgb(
                                                                150, 150, 150,
                                                            )),
                                                        );
                                                    }
                                                });
                                            })
                                            .response;

                                        if resp.interact(egui::Sense::click()).clicked() {
                                            pick_playlist = Some((
                                                pl.playlist_id.clone(),
                                                pl.title.clone(),
                                            ));
                                        }
                                        ui.separator();
                                    }
                                });
                            }
                        });
                    });
            }

            // ライブチャットサイドパネル（右）。動画と並べて表示し、動画には重ねない。
            // UI 非表示中も表示する（独立した恒常パネルとして扱う）。
            if chat_visible && !chat_messages.is_empty() {
                egui::SidePanel::right("chat_panel")
                    .resizable(false)
                    .exact_width(CHAT_PANEL_WIDTH)
                    .frame(egui::Frame::none().fill(egui::Color32::from_rgb(20, 20, 20)))
                    .show(ctx, |ui| {
                        if draw_chat(ui, chat_messages, &chat_status) {
                            toggle_chat = true;
                        }
                    });
            }

            // 動画ロード状態の中央オーバーレイ。
            // auto-hide で URL バー以下が消える状態でも、画面が完全に黒くなって
            // 何が起きているか分からなくなる事態を避けるため show_ui に依存せず描画する。
            // 表示優先度: エラー > 解決中 > 再生準備中（mpv loadfile 後で初フレーム前）。
            let loading_overlay: Option<(String, bool, bool)> = if let Some(err) = &load_error {
                // (本文, 赤色か, spinner を出すか)
                Some((format!("読み込み失敗\n{err}"), true, false))
            } else if resolve_busy {
                Some(("動画を解決中…".to_string(), false, true))
            } else if url_set && time_pos == 0.0 {
                // mpv はメタデータ取得（duration > 0）と初フレーム描画の間に
                // バッファリングで停滞する。duration を条件に入れると、その間
                // テクスチャが空のまま画面が真っ黒になるため、time_pos が進む
                // までは「再生準備中…」を出し続ける。
                Some(("再生準備中…".to_string(), false, true))
            } else {
                None
            };
            if let Some((text, is_error, with_spinner)) = loading_overlay {
                if with_spinner {
                    loading_spinning = true;
                }
                egui::Area::new(egui::Id::new("loading_overlay"))
                    .order(egui::Order::Foreground)
                    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                    .show(ctx, |ui| {
                        egui::Frame::none()
                            .fill(egui::Color32::from_black_alpha(200))
                            .inner_margin(egui::Margin::symmetric(24.0, 16.0))
                            .rounding(8.0)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    if with_spinner {
                                        ui.add(egui::Spinner::new().size(20.0));
                                        ui.add_space(8.0);
                                    }
                                    let color = if is_error {
                                        egui::Color32::from_rgb(255, 120, 120)
                                    } else {
                                        egui::Color32::WHITE
                                    };
                                    ui.label(
                                        egui::RichText::new(&text)
                                            .color(color)
                                            .size(16.0),
                                    );
                                });
                            });
                    });
            }

            if !show_ui {
                return;
            }

            egui::TopBottomPanel::top("urlbar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("URL:");
                    let resp = ui.add(
                        egui::TextEdit::singleline(url_input)
                            .desired_width(f32::INFINITY)
                            .hint_text("YouTube の URL を入力して Enter"),
                    );
                    let entered = resp.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if entered {
                        to_load = Some(url_input.clone());
                    }
                });

                // 機能ナビとアカウント。アカウントは右端に寄せる。
                ui.horizontal(|ui| {
                    if !recommend_items.is_empty() {
                        if ui.button("📋 おすすめ").clicked() {
                            toggle_recommend = true;
                        }
                    }

                    if logged_in {
                        if ui.button("📃 再生リスト").clicked() {
                            toggle_playlist = true;
                        }
                        if ui.button("📺 登録チャンネル").on_hover_text("登録チャンネル一覧").clicked() {
                            toggle_subs = true;
                        }
                        if ui.button("🕘 履歴").on_hover_text("再生履歴").clicked() {
                            toggle_history = true;
                        }
                    }

                    // 右端のアカウント表示 / ログインボタン。
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if logged_in {
                                let who = channel.as_deref().unwrap_or("ログイン済み");
                                ui.label(
                                    egui::RichText::new(format!("👤 {who}"))
                                        .color(egui::Color32::WHITE),
                                );
                            } else if ui
                                .add_enabled(!auth_busy, egui::Button::new("🔑 ログイン"))
                                .clicked()
                            {
                                login_clicked = true;
                            }
                            if !auth_status.is_empty() {
                                ui.label(
                                    egui::RichText::new(&auth_status)
                                        .color(egui::Color32::from_rgb(170, 170, 170))
                                        .small(),
                                );
                            }
                        },
                    );
                });

                // 動画タイトル（上部に表示）。高評価ボタンは下部コントローラーへ。
                if !title.is_empty() {
                    ui.label(
                        egui::RichText::new(&title)
                            .color(egui::Color32::WHITE)
                            .size(16.0)
                            .strong(),
                    );
                }
            });

            // コントローラーを UI 最下部に置く（タイトル/高評価は上部 urlbar へ移動済み）。
            egui::TopBottomPanel::bottom("controls")
                .frame(
                    egui::Frame::none()
                        .fill(egui::Color32::from_black_alpha(200))
                        .inner_margin(egui::Margin::symmetric(16.0, 8.0)),
                )
                .show(ctx, |ui| {
                    ui.vertical(|ui| {
                        // シークバー (フル幅、上)
                        let mut pos = time_pos;
                        let seekable = seekable_stream;
                        // 動画読込済みでシーク不可 ＝ DVR なしライブとみなしてバーを 100% 固定。
                        let live_fixed = !seekable_stream && url_set;
                        if seek_bar(ui, &mut pos, duration, seekable, live_fixed).changed() {
                            player.set_time_pos(pos);
                        }

                        ui.add_space(4.0);

                        // ボタン行
                        ui.horizontal(|ui| {
                            // 再生 / 一時停止 (フラット白)
                            let label = if paused { "▶" } else { "⏸" };
                            let btn = egui::Button::new(
                                egui::RichText::new(label)
                                    .color(egui::Color32::WHITE)
                                    .size(18.0),
                            )
                            .frame(false);
                            if ui.add(btn).clicked() {
                                player.set_paused(!paused);
                            }

                            // 時刻 (経過 / 全体)
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} / {}",
                                    format_time(time_pos),
                                    format_time(duration),
                                ))
                                .color(egui::Color32::WHITE)
                                .size(13.0),
                            );

                            // 高評価（コントローラーの一部）。
                            let can_like = logged_in && !auth_busy && video_id.is_some();
                            if ui
                                .add_enabled(can_like, egui::Button::new("👍 高評価"))
                                .on_hover_text("この動画に高評価を付けます")
                                .clicked()
                            {
                                like_clicked = true;
                            }

                            // チャット表示切り替え（接続中 or メッセージがあるとき）。
                            if !chat_status.is_empty() {
                                let label =
                                    if chat_visible { "💬 チャット非表示" } else { "💬 チャット表示" };
                                if ui.button(label).clicked() {
                                    toggle_chat = true;
                                }
                            }

                            // 右寄せ: 音量
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let mut vol = volume;
                                    if volume_bar(ui, &mut vol, 130.0).changed() {
                                        player.set_volume(vol);
                                    }
                                    // スピーカーアイコン: クリックでミュート切替。
                                    let icon = if muted { "🔇" } else { "🔊" };
                                    let spk = egui::Button::new(
                                        egui::RichText::new(icon).color(egui::Color32::WHITE),
                                    )
                                    .frame(false);
                                    if ui.add(spk).on_hover_text("ミュート切替").clicked() {
                                        player.set_muted(!muted);
                                    }

                                    // 画質・コーデック（変更で現在の動画を取り直す）。
                                    ui.add_space(12.0);
                                    egui::ComboBox::from_id_salt("codec_combo")
                                        .selected_text(format!("コーデック: {}", sel_codec.label()))
                                        .show_ui(ui, |ui| {
                                            for c in Codec::ALL {
                                                ui.selectable_value(&mut sel_codec, c, c.label());
                                            }
                                        });
                                    egui::ComboBox::from_id_salt("quality_combo")
                                        .selected_text(format!("画質: {}", sel_quality.label()))
                                        .show_ui(ui, |ui| {
                                            for q in Quality::ALL {
                                                ui.selectable_value(&mut sel_quality, q, q.label());
                                            }
                                        });
                                },
                            );
                        });
                    });
                });

            // 動画領域（egui のパネル/オーバーレイ以外）のクリックで再生/一時停止をトグルする。
            // is_pointer_over_area() が false ＝ どの egui エリアにも乗っていない＝動画上。
            if ctx.input(|i| i.pointer.primary_clicked()) && !ctx.is_pointer_over_area() {
                player.set_paused(!paused);
            }
        });
        self.egui_glow.paint(window);

        // dev-tools のスクショ要求を処理（swap 前の back buffer から読み出す）。
        self.handle_devtools_commands();

        let _ = self.gl_surface.swap_buffers(&self.gl_context);

        self.frames += 1;
        if self.verbose && self.frames % 120 == 0 {
            eprintln!("[frames] {}", self.frames);
        }

        // 一時停止中は mpv が再描画を駆動しないので、UI 表示中は自前で次フレームを要求して
        // 非表示への移行（カウントダウン）を進める。vsync で頻度は抑えられる。
        // また、ロード状態オーバーレイの spinner を回し続けるためにも自前で再描画する。
        if (paused && show_ui) || loading_spinning {
            self.window.request_redraw();
        }

        if let Some(url) = pick_video {
            self.url_input = url.clone();
            self.load(&url);
            if let Some(vid) = auth::extract_video_id(&self.current_url) {
                self.start_chat(vid.clone());
                self.start_recommend(vid);
            }
            // 再生リスト・新着から選んだ場合はリスト自体はリセットしない。
        } else if let Some(url) = to_load {
            self.load(&url);
            if let Some(vid) = auth::extract_video_id(&self.current_url) {
                self.start_chat(vid.clone());
                self.start_recommend(vid);
            }
            // URL 手入力時は再生リストの動画一覧をリセット（一覧は保持）。
            self.playlist_items.clear();
            self.playlist_items_title.clear();
        }

        // 画質・コーデックが変更されたら、再生中の YouTube 動画を新指定で取り直す。
        if sel_quality != self.quality || sel_codec != self.codec {
            self.quality = sel_quality;
            self.codec = sel_codec;
            if !self.current_url.is_empty() && resolve::is_youtube_url(&self.current_url) {
                let u = self.current_url.clone();
                self.start_resolve(u);
            }
        }
        if toggle_chat {
            self.chat_visible = !self.chat_visible;
        }
        if toggle_recommend {
            self.recommend_visible = !self.recommend_visible;
        }
        if toggle_subs {
            self.sub_visible = !self.sub_visible;
            // 閉じるときは右ペイン（チャンネル動画）もリセットしてリスト主体に戻す。
            if !self.sub_visible {
                self.channel_visible = false;
            }
            // 表示時かつ未取得なら取得開始。
            if self.sub_visible && self.sub_channels.is_empty() && !self.sub_busy {
                self.start_subs();
            }
        }
        if toggle_history {
            self.history_visible = !self.history_visible;
            // 履歴は古くなるので「再表示時は毎回取り直す」のが履歴 UI として自然だが、
            // ここは subs と同じく「未取得ならフェッチ」だけにし、再フェッチは
            // overlay ヘッダのリロードボタン（共通の閉じるボタン横、未実装）に委ねる方針。
            if self.history_visible && self.history_items.is_empty() && !self.history_busy {
                self.start_history();
            }
        }
        if toggle_playlist {
            self.playlist_visible = !self.playlist_visible;
            // 表示時かつ未取得なら一覧取得開始。
            if self.playlist_visible
                && self.playlist_lists.is_empty()
                && self.playlist_items.is_empty()
                && !self.playlist_busy
            {
                self.start_playlist_list();
            }
        }
        if let Some((pl_id, pl_title)) = pick_playlist {
            self.start_playlist_items(pl_id, pl_title);
        }
        // 登録チャンネルをクリック → 右ペインにそのチャンネルのアップロード一覧（カード UI）を出す。
        // 登録チャンネルリストは左に残したままにする。
        if let Some((channel_id, title)) = pick_channel {
            let uploads_id = if channel_id.starts_with("UC") && channel_id.len() > 2 {
                format!("UU{}", &channel_id[2..])
            } else {
                channel_id.clone()
            };
            self.start_channel_uploads(uploads_id, title);
        }
        if sub_back_to_feed {
            // チャンネル絞り込みを解除して新着一覧へ戻す。
            self.channel_visible = false;
        }
        if playlist_back {
            self.playlist_items.clear();
            self.playlist_items_title.clear();
            self.playlist_status = format!("再生リスト ({} 件)", self.playlist_lists.len());
        }
        if login_clicked {
            self.start_login();
        }
        if like_clicked {
            if let Some(id) = video_id {
                self.start_like(id);
            }
        }
    }

    /// dev-tools サーバからの要求を 1 件処理する（swap 前に呼ぶこと）。
    /// 要求が無ければ何もしない。複数たまっていても次の redraw で次が処理される。
    fn handle_devtools_commands(&mut self) {
        // 試行回数を制限すれば飢餓は起きないが、1 件ずつでも次の redraw で次が処理される。
        let Some(rx) = self.devtools_rx.as_ref() else {
            return;
        };
        let Ok(cmd) = rx.try_recv() else {
            return;
        };
        match cmd {
            devtools::Command::Screenshot(reply) => {
                let _ = reply.send(self.capture_framebuffer_png());
            }
            devtools::Command::Action(name, reply) => {
                let known = self.apply_devtools_action(&name);
                let _ = reply.send(known);
            }
            devtools::Command::Click { x, y, reply } => {
                self.inject_click(x, y);
                let _ = reply.send(true);
            }
            devtools::Command::Type { text, enter, reply } => {
                self.inject_type(&text, enter);
                let _ = reply.send(true);
            }
        }
    }

    /// dev-tools の `POST /click` を受け、指定座標（物理px）へ左クリックを合成して egui に注入する。
    /// 次フレームの egui_glow.run でボタン/動画クリック等として処理される。
    fn inject_click(&mut self, x: f32, y: f32) {
        // /screenshot は物理ピクセル。egui のイベントは論理ポイントなので scale_factor で割る。
        let ppp = self.window.scale_factor() as f32;
        let pos = egui::pos2(x / ppp, y / ppp);
        let modifiers = egui::Modifiers::default();
        let events = &mut self.egui_glow.egui_winit.egui_input_mut().events;
        events.push(egui::Event::PointerMoved(pos));
        events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers,
        });
        events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers,
        });
        // 操作扱いにして UI を表示し、次フレームを描画させる。
        self.last_activity = Instant::now();
        self.window.request_redraw();
    }

    /// dev-tools の `POST /type`：フォーカス中のウィジェットへ貼り付け、必要なら Enter を送る。
    fn inject_type(&mut self, text: &str, enter: bool) {
        let modifiers = egui::Modifiers::default();
        let events = &mut self.egui_glow.egui_winit.egui_input_mut().events;
        if !text.is_empty() {
            events.push(egui::Event::Paste(text.to_string()));
        }
        if enter {
            events.push(egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            });
            events.push(egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers,
            });
        }
        self.last_activity = Instant::now();
        self.window.request_redraw();
    }

    /// dev-tools の `POST /action/<name>` を受けて intent flag を立てる。
    /// 既知のアクションなら true、未知なら false（HTTP 400 で返るよう devtools 側で扱う）。
    fn apply_devtools_action(&mut self, name: &str) -> bool {
        let p = &mut self.devtools_pending;
        match name {
            "toggle_chat" => p.toggle_chat = true,
            "toggle_recommend" => p.toggle_recommend = true,
            "toggle_subs" => p.toggle_subs = true,
            "toggle_playlist" => p.toggle_playlist = true,
            "toggle_history" => p.toggle_history = true,
            "play_pause" => p.play_pause = true,
            "login" => p.login = true,
            "like" => p.like = true,
            "close_overlay" => p.close_overlay = true,
            _ => return false,
        }
        // 次フレームで吸い上げる必要があるので、redraw を要求しておく。
        self.window.request_redraw();
        true
    }

    /// 現在の back buffer を PNG にエンコードして返す。
    /// GL 読み出しはメインスレッド (= GL コンテキスト所属) のみで可能。
    fn capture_framebuffer_png(&self) -> Vec<u8> {
        use glow::HasContext;
        let size = self.window.inner_size();
        let (w, h) = (size.width as i32, size.height as i32);
        if w <= 0 || h <= 0 {
            return Vec::new();
        }

        let mut data = vec![0u8; (w * h * 4) as usize];
        unsafe {
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            self.gl.read_pixels(
                0,
                0,
                w,
                h,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(&mut data),
            );
        }

        // OpenGL は左下原点、PNG は左上原点。上下反転して並べ直す。
        let row = (w * 4) as usize;
        let mut flipped = vec![0u8; data.len()];
        for y in 0..h as usize {
            let src = (h as usize - 1 - y) * row;
            let dst = y * row;
            flipped[dst..dst + row].copy_from_slice(&data[src..src + row]);
        }

        let buf: image::RgbaImage =
            match image::ImageBuffer::from_raw(w as u32, h as u32, flipped) {
                Some(b) => b,
                None => return Vec::new(),
            };
        let mut png = Vec::new();
        if buf
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .is_err()
        {
            return Vec::new();
        }
        png
    }
}

/// チャットパネルの描画。閉じるボタンが押されたら true を返す。
/// オーバーレイ共通のヘッダーを描画する。「✕ 閉じる」が押されたら true を返す。
/// 1 行ぶんの高さで、左にタイトル＋スピナー、右に閉じるボタンを配置する。
/// 動画グリッドのカード 1 つ分。
///
/// すべて owned String なのは、毎フレーム再構築される egui の immediate mode に合わせ、
/// 呼び出し側 (recommend / subscriptions) の異なるデータ構造から統一的に渡せるようにするため。
struct GridCard {
    video_id: String,
    title: String,
    channel: String,
    /// 再生時間（mm:ss など）。サムネ右下にバッジ表示。空文字なら非表示。
    duration: String,
    /// 視聴数 / 公開日などの追加メタ情報。空文字なら非表示。
    meta: String,
    /// チャンネルアイコン URL。空ならアイコン非表示。
    channel_icon: String,
}

const CARD_TARGET_WIDTH: f32 = 320.0;
const CARD_GAP: f32 = 8.0;

/// YouTube ホーム風の動画カードグリッドを描画する。クリックされたカードの video_id を返す。
fn draw_video_grid(ui: &mut egui::Ui, cards: &[GridCard]) -> Option<String> {
    let avail = ui.available_width();
    let cols = (((avail + CARD_GAP) / (CARD_TARGET_WIDTH + CARD_GAP)).floor() as usize).max(1);
    let card_w = (avail - CARD_GAP * (cols.saturating_sub(1) as f32)) / cols as f32;

    let mut clicked: Option<String> = None;
    for chunk in cards.chunks(cols) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            for card in chunk {
                if let Some(id) = draw_video_card(ui, card, card_w) {
                    clicked = Some(id);
                }
            }
        });
        ui.add_space(CARD_GAP);
    }
    clicked
}

/// 1 枚のカードを描画する。サムネ + 再生時間バッジ + タイトル + チャンネル + メタ。
fn draw_video_card(ui: &mut egui::Ui, card: &GridCard, w: f32) -> Option<String> {
    // サムネ枠は 16:9 固定（カードサイズを揃え、バッジ位置を安定させる）。
    let thumb_h = w * 9.0 / 16.0;

    let inner = ui.allocate_ui_with_layout(
        egui::vec2(w, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_min_width(w);
            ui.set_max_width(w);
            // サムネ → テキストの縦隙間を詰める（egui 既定の item_spacing.y は広め）。
            ui.spacing_mut().item_spacing.y = 0.0;

            // サムネ画像。サムネのアスペクト比は保証されない（mqdefault は 16:9(320×180)、
            // 動画自体が 4:3 のものは内側に黒帯が焼き込まれている等）。どんな比でも歪ませず、
            // 16:9 枠の中央にアスペクト維持で配置し、余った領域は黒帯にする防御的レイアウト。
            //   - 16:9 画像 → 枠いっぱい
            //   - 4:3 等 → 左右に黒帯（センタリング）
            // ソースは 16:9 に素直な mqdefault を使う（hqdefault は 480×360 の 4:3 固定で、
            // 16:9 動画でも上下黒帯が焼き込まれており 16:9 枠だと絵が黒に浮く）。
            let thumb_url = format!("https://i.ytimg.com/vi/{}/mqdefault.jpg", card.video_id);
            // 16:9 フレームを確保（cursor もこの分だけ進む）。
            let (frame_rect, _) =
                ui.allocate_exact_size(egui::vec2(w, thumb_h), egui::Sense::hover());
            // 黒背景（足りない領域がそのまま黒帯になる）。
            ui.painter()
                .rect_filled(frame_rect, 8.0, egui::Color32::BLACK);
            // 実画像をアスペクト維持でフレーム内にセンタリング描画。
            let image = egui::Image::new(thumb_url)
                .maintain_aspect_ratio(true)
                .rounding(8.0);
            // ロード済みなら実比から内接サイズを得てセンタリング。未ロード時はフレーム全体
            // （paint_at がローディングスピナーを中央に出す）。
            let thumb_rect = image
                .load_and_calc_size(ui, frame_rect.size())
                .map(|sz| egui::Rect::from_center_size(frame_rect.center(), sz))
                .unwrap_or(frame_rect);
            image.paint_at(ui, thumb_rect);

            // 再生時間バッジ（サムネ右下に重ね描き）。
            if !card.duration.is_empty() {
                let painter = ui.painter();
                let galley = painter.layout_no_wrap(
                    card.duration.clone(),
                    egui::FontId::proportional(11.0),
                    egui::Color32::WHITE,
                );
                let pad = egui::vec2(6.0, 3.0);
                let badge_size = galley.size() + pad * 2.0;
                let margin = 6.0;
                let badge_rect = egui::Rect::from_min_size(
                    thumb_rect.right_bottom() - badge_size - egui::vec2(margin, margin),
                    badge_size,
                );
                painter.rect_filled(badge_rect, 3.0, egui::Color32::from_black_alpha(204));
                painter.galley(badge_rect.min + pad, galley, egui::Color32::WHITE);
            }

            // サムネ直下にタイトルを置く。わずかな余白だけ空ける。
            ui.add_space(4.0);

            // YouTube 風レイアウト: 左にチャンネルアイコン（丸）、右に
            // タイトル / チャンネル名 / 視聴数・公開時刻 を縦に積む。
            // horizontal で並べることで、テキスト側の wrap 幅が確定する。
            const ICON_SIZE: f32 = 36.0;
            const ICON_GAP: f32 = 6.0;
            let text_w = if card.channel_icon.is_empty() {
                w
            } else {
                w - ICON_SIZE - ICON_GAP
            };
            ui.horizontal_top(|ui| {
                // 既定の item_spacing.x ぶん余計に空くのを防ぎ、ICON_GAP だけで詰める。
                ui.spacing_mut().item_spacing.x = 0.0;
                if !card.channel_icon.is_empty() {
                    ui.add(
                        egui::Image::new(&card.channel_icon)
                            .fit_to_exact_size(egui::vec2(ICON_SIZE, ICON_SIZE))
                            .rounding(ICON_SIZE / 2.0),
                    );
                    ui.add_space(ICON_GAP);
                }
                ui.vertical(|ui| {
                    ui.set_max_width(text_w);
                    // タイトル/チャンネル名/メタ の行間を詰める。
                    ui.spacing_mut().item_spacing.y = 1.0;
                    // wrap_mode を明示しないと horizontal layout 配下では Extend に
                    // なって 1 行に伸び、カード幅を超えて他カードに食い込む。
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&card.title)
                                .color(egui::Color32::WHITE)
                                .strong(),
                        )
                        .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                    if !card.channel.is_empty() {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&card.channel)
                                    .color(egui::Color32::from_rgb(170, 170, 170))
                                    .small(),
                            )
                            .wrap_mode(egui::TextWrapMode::Wrap),
                        );
                    }
                    if !card.meta.is_empty() {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&card.meta)
                                    .color(egui::Color32::from_rgb(170, 170, 170))
                                    .small(),
                            )
                            .wrap_mode(egui::TextWrapMode::Wrap),
                        );
                    }
                });
            });
        },
    );

    // カード全体をクリック可能に。
    let id = ui.id().with("card").with(&card.video_id);
    let resp = ui.interact(inner.response.rect, id, egui::Sense::click());
    if resp.clicked() {
        Some(card.video_id.clone())
    } else {
        None
    }
}

/// 本家風シークバー: 細い赤線 + ホバー時にハンドル円。クリック / ドラッグで pos を更新。
/// pos が変更されたら Response.changed() = true を返す。
fn seek_bar(
    ui: &mut egui::Ui,
    pos: &mut f64,
    duration: f64,
    seekable: bool,
    live_fixed: bool,
) -> egui::Response {
    let desired_size = egui::vec2(ui.available_width(), 16.0);
    let sense = if seekable {
        egui::Sense::click_and_drag()
    } else {
        egui::Sense::hover()
    };
    let (rect, mut response) = ui.allocate_exact_size(desired_size, sense);

    let bar_h: f32 = if response.hovered() || response.dragged() {
        6.0
    } else {
        4.0
    };
    let bar_rect =
        egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), bar_h));

    let painter = ui.painter();
    painter.rect_filled(bar_rect, 2.0, egui::Color32::from_white_alpha(64));

    // DVR なしライブは常に最先端＝100% 固定。それ以外は再生位置 / 全体。
    let progress = if live_fixed {
        1.0
    } else if duration > 0.0 {
        (*pos / duration).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    let red = egui::Color32::from_rgb(229, 9, 20);
    let progress_rect = egui::Rect::from_min_size(
        bar_rect.min,
        egui::vec2(bar_rect.width() * progress, bar_h),
    );
    painter.rect_filled(progress_rect, 2.0, red);

    if seekable && (response.hovered() || response.dragged()) {
        let handle_x = bar_rect.left() + bar_rect.width() * progress;
        painter.circle_filled(egui::pos2(handle_x, rect.center().y), 7.0, red);
    }

    if seekable && (response.clicked() || response.dragged()) {
        if let Some(p) = response.interact_pointer_pos() {
            let ratio = ((p.x - bar_rect.left()) / bar_rect.width()).clamp(0.0, 1.0);
            let new_pos = ratio as f64 * duration;
            if (*pos - new_pos).abs() > 0.001 {
                *pos = new_pos;
                response.mark_changed();
            }
        }
    }

    response
}

/// 本家風音量バー: 細い白線 + ホバー時にハンドル円。
fn volume_bar(ui: &mut egui::Ui, vol: &mut f64, max: f64) -> egui::Response {
    let desired_size = egui::vec2(80.0, 16.0);
    let (rect, mut response) =
        ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());

    let bar_h: f32 = 4.0;
    let bar_rect =
        egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), bar_h));

    let painter = ui.painter();
    painter.rect_filled(bar_rect, 2.0, egui::Color32::from_white_alpha(64));

    let progress = (*vol / max).clamp(0.0, 1.0) as f32;
    let progress_rect = egui::Rect::from_min_size(
        bar_rect.min,
        egui::vec2(bar_rect.width() * progress, bar_h),
    );
    painter.rect_filled(progress_rect, 2.0, egui::Color32::WHITE);

    if response.hovered() || response.dragged() {
        let handle_x = bar_rect.left() + bar_rect.width() * progress;
        painter.circle_filled(egui::pos2(handle_x, rect.center().y), 6.0, egui::Color32::WHITE);
    }

    if response.clicked() || response.dragged() {
        if let Some(p) = response.interact_pointer_pos() {
            let ratio = ((p.x - bar_rect.left()) / bar_rect.width()).clamp(0.0, 1.0);
            let new_vol = ratio as f64 * max;
            if (*vol - new_vol).abs() > 0.01 {
                *vol = new_vol;
                response.mark_changed();
            }
        }
    }

    response
}

/// 再生リスト 1 行を描画する。順序番号 + サムネ小 + タイトル + チャンネル。
/// クリックされたら true を返す。
fn draw_playlist_row(ui: &mut egui::Ui, item: &playlist::PlaylistItem, position: usize) -> bool {
    let row_h: f32 = 84.0;
    let thumb_h = row_h - 8.0;
    let thumb_w = thumb_h * 16.0 / 9.0;

    let inner = ui.horizontal(|ui| {
        ui.set_min_height(row_h);
        ui.set_max_height(row_h);

        // 順序番号（固定幅で揃える）。
        ui.add_sized(
            egui::vec2(32.0, row_h),
            egui::Label::new(
                egui::RichText::new(format!("{position}"))
                    .color(egui::Color32::from_rgb(150, 150, 150))
                    .monospace()
                    .size(13.0),
            ),
        );

        // サムネ。
        ui.add(
            egui::Image::new(format!(
                "https://i.ytimg.com/vi/{}/mqdefault.jpg",
                item.video_id
            ))
            .rounding(4.0)
            .fit_to_exact_size(egui::vec2(thumb_w, thumb_h)),
        );

        // タイトル / チャンネル。
        ui.vertical(|ui| {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(&item.title)
                    .color(egui::Color32::WHITE)
                    .strong(),
            );
            if !item.channel.is_empty() {
                ui.label(
                    egui::RichText::new(&item.channel)
                        .color(egui::Color32::from_rgb(170, 170, 170))
                        .small(),
                );
            }
        });
    });

    let id = ui.id().with("plrow").with(&item.video_id).with(position);
    ui.interact(inner.response.rect, id, egui::Sense::click()).clicked()
}

fn draw_overlay_header(ui: &mut egui::Ui, title: &str, busy: bool) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(title)
                .color(egui::Color32::WHITE)
                .heading(),
        );
        if busy {
            ui.spinner();
        }
        // 残りの幅を右寄せレイアウトで使い、閉じるボタンを右端に。
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                if ui.button("閉じる").clicked() {
                    close = true;
                }
            },
        );
    });
    close
}

fn draw_chat(ui: &mut egui::Ui, messages: &[chat::ChatMessage], status: &str) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(status)
                .color(egui::Color32::WHITE)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("閉じる").on_hover_text("チャットを閉じる").clicked() {
                close = true;
            }
        });
    });
    ui.separator();

    // 仮想スクロール: 幅固定なので各メッセージ高さは安定。実測値をキャッシュし、
    // 可視範囲に重なるメッセージだけを描画する（毎フレーム全件 for を避ける）。
    egui::ScrollArea::vertical()
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let width = ui.available_width();
            let row_gap = ui.spacing().item_spacing.y;
            // 未実測メッセージの推定高さ（最初の表示時に実測へ置き換わる）。
            let est = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
            let origin = ui.min_rect().min;

            // 各メッセージの先頭 y オフセット（累積）と総高さ。
            let mut offsets = Vec::with_capacity(messages.len() + 1);
            let mut acc = 0.0_f32;
            for m in messages {
                offsets.push(acc);
                let h = m.cached_height.get();
                acc += (if h > 0.0 { h } else { est }) + row_gap;
            }
            offsets.push(acc);
            // スクロールバー用に総高さを確保。
            ui.allocate_space(egui::vec2(width, acc));
            if messages.is_empty() {
                return;
            }

            // 可視範囲 [viewport.min.y, viewport.max.y] に重なるメッセージだけ描画。
            let first = offsets
                .partition_point(|&o| o <= viewport.min.y)
                .saturating_sub(1)
                .min(messages.len() - 1);
            let mut i = first;
            while i < messages.len() && offsets[i] < viewport.max.y {
                let msg = &messages[i];
                let cached = msg.cached_height.get();
                // 折返しで伸びても収まるよう、推定/実測に余裕を足した箱を与える。
                let box_h = if cached > 0.0 { cached } else { est } + 40.0;
                let rect = egui::Rect::from_min_size(
                    origin + egui::vec2(0.0, offsets[i]),
                    egui::vec2(width, box_h),
                );
                let resp = ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(&msg.author)
                                .color(egui::Color32::from_rgb(100, 180, 255))
                                .strong(),
                        );
                        for run in &msg.runs {
                            match run {
                                chat::ChatRun::Text(t) => {
                                    ui.label(egui::RichText::new(t).color(egui::Color32::WHITE));
                                }
                                chat::ChatRun::Image { url, alt } => {
                                    let size = ui.text_style_height(&egui::TextStyle::Body);
                                    let img = egui::Image::new(url)
                                        .max_height(size)
                                        .fit_to_original_size(1.0);
                                    ui.add(img).on_hover_text(alt);
                                }
                            }
                        }
                    });
                });
                // 実測高さをキャッシュ → 次フレームのオフセットに反映される。
                msg.cached_height.set(resp.response.rect.height());
                i += 1;
            }
        });
    close
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    initial_url: Option<String>,
    verbose: bool,
    backend: String,
    enable_dev_tools: bool,
    initial_volume: Option<f64>,
    state: Option<Running>,
}

impl App {
    fn new(
        proxy: EventLoopProxy<UserEvent>,
        initial_url: Option<String>,
        verbose: bool,
        backend: String,
        enable_dev_tools: bool,
        initial_volume: Option<f64>,
    ) -> Self {
        Self {
            proxy,
            initial_url,
            verbose,
            backend,
            enable_dev_tools,
            initial_volume,
            state: None,
        }
    }

    /// ウィンドウ・GL コンテキスト・mpv・RenderContext を構築する。
    fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<Running> {
        // --- ウィンドウ + GL コンフィグ ---
        let window_attributes = Window::default_attributes()
            .with_title("YouTube Super Lite")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));

        let template = ConfigTemplateBuilder::new().with_alpha_size(8);
        let display_builder =
            DisplayBuilder::new().with_window_attributes(Some(window_attributes));

        let (window, gl_config) = display_builder
            .build(event_loop, template, |configs| {
                configs
                    .reduce(|acc, c| {
                        if c.num_samples() > acc.num_samples() {
                            c
                        } else {
                            acc
                        }
                    })
                    .expect("no GL config")
            })
            .map_err(|e| anyhow!("display build failed: {e}"))?;
        let window = window.ok_or_else(|| anyhow!("no window created"))?;

        let gl_display = gl_config.display();
        let raw_window_handle = window.window_handle()?.as_raw();

        // --- GL コンテキスト + サーフェス ---
        // mpv の OpenGL バックエンドに合わせ、デスクトップ GL Core 3.3 を明示する。
        let context_attributes = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::OpenGl(Some(Version::new(3, 3))))
            .with_profile(GlProfile::Core)
            .build(Some(raw_window_handle));
        let not_current = unsafe { gl_display.create_context(&gl_config, &context_attributes)? };

        let attrs: glutin::surface::SurfaceAttributes<WindowSurface> =
            window.build_surface_attributes(SurfaceAttributesBuilder::<WindowSurface>::new())?;
        let gl_surface = unsafe { gl_display.create_window_surface(&gl_config, &attrs)? };
        let gl_context = not_current.make_current(&gl_surface)?;

        // vsync。一時停止中に UI を消すための自前再描画ループの暴走を防ぐ。
        if let Some(one) = NonZeroU32::new(1) {
            let _ = gl_surface.set_swap_interval(&gl_context, SwapInterval::Wait(one));
        }

        // --- glow + egui + Player ---
        let gl = Arc::new(unsafe {
            glow::Context::from_loader_function_cstr(|s| gl_display.get_proc_address(s).cast())
        });
        let egui_glow = egui_glow::EguiGlow::new(event_loop, gl.clone(), None, None, true);
        setup_japanese_font(&egui_glow.egui_ctx);
        // メンバーシップスタンプ等のカスタム絵文字を URL から動的に描画するため画像ローダを登録。
        egui_extras::install_image_loaders(&egui_glow.egui_ctx);
        // HTTP 画像（サムネ/アイコン/絵文字）の永続キャッシュ。install_image_loaders の後に
        // 登録することで try_load_bytes の後入れ優先（.rev()）により本ローダが先に当たる。
        egui_glow.egui_ctx.add_bytes_loader(std::sync::Arc::new(
            image_cache::DiskImageCache::new(image_cache_dir(), egui_glow.egui_ctx.clone()),
        ));

        let proxy_for_mpv = self.proxy.clone();
        let player = player::Player::new(gl.clone(), gl_display.clone(), self.verbose, move || {
            let _ = proxy_for_mpv.send_event(UserEvent::MpvRedraw);
        })?;
        // デバッグ用の初期音量指定（例: --volume 0 で無音起動）。
        if let Some(v) = self.initial_volume {
            player.set_volume(v);
        }
        let quad = gl_quad::FullscreenQuad::new(gl.clone())?;

        // 認証まわりの初期化。
        let backend = self.backend.clone();
        let (auth_tx, auth_rx) = std::sync::mpsc::channel();
        let auth_status = "未ログイン".to_string();

        // チャットまわりの初期化。
        let (chat_tx, chat_rx) = std::sync::mpsc::channel();
        // おすすめ動画の初期化。
        let (recommend_tx, recommend_rx) = std::sync::mpsc::channel();
        // 登録チャンネル新着の初期化。
        let (sub_tx, sub_rx) = std::sync::mpsc::channel();
        // 再生履歴の初期化。
        let (history_tx, history_rx) = std::sync::mpsc::channel();
        // 再生リストの初期化。
        let (playlist_tx, playlist_rx) = std::sync::mpsc::channel();
        // チャンネル動画一覧の初期化。
        let (channel_tx, channel_rx) = std::sync::mpsc::channel();
        // yt-dlp ストリーム解決の初期化。
        let (resolve_tx, resolve_rx) = std::sync::mpsc::channel();

        let mut running = Running {
            egui_glow,
            player,
            quad,
            gl: gl.clone(),
            gl_context,
            gl_surface,
            window,
            url_input: String::new(),
            current_url: String::new(),
            frames: 0,
            verbose: self.verbose,
            last_activity: Instant::now(),
            ui_visible: true,
            quality: Quality::Auto,
            codec: Codec::Auto,
            proxy: self.proxy.clone(),
            backend,
            tokens: None,
            channel: None,
            auth_status,
            auth_busy: false,
            auth_tx,
            auth_rx,
            chat_messages: Vec::new(),
            chat_tx,
            chat_rx,
            chat_stop: None,
            chat_status: String::new(),
            chat_visible: false,
            player_offset_ms: Arc::new(AtomicI64::new(0)),
            recommend_items: Vec::new(),
            recommend_tx,
            recommend_rx,
            recommend_visible: false,
            recommend_status: String::new(),
            sub_channels: Vec::new(),
            sub_tx,
            sub_rx,
            sub_visible: false,
            sub_feed: Vec::new(),
            sub_status: String::new(),
            sub_busy: false,
            history_items: Vec::new(),
            history_tx,
            history_rx,
            history_visible: false,
            history_status: String::new(),
            history_busy: false,
            playlist_lists: Vec::new(),
            playlist_items: Vec::new(),
            playlist_items_title: String::new(),
            playlist_tx,
            playlist_rx,
            playlist_visible: false,
            playlist_status: String::new(),
            playlist_busy: false,
            channel_videos: Vec::new(),
            channel_tx,
            channel_rx,
            channel_visible: false,
            channel_status: String::new(),
            channel_busy: false,
            resolve_tx,
            resolve_rx,
            resolve_busy: false,
            load_error: None,
            devtools_rx: None,
            devtools_pending: DevToolsPending::default(),
            gpu_monitor: None,
        };

        // 保存済みリフレッシュトークンがあれば自動ログインを試みる。
        if let Some(rt) = auth::load_refresh_token() {
            running.start_silent_login(rt);
        }

        // --enable-dev-tools 時のみ devtools サーバを起動。
        if self.enable_dev_tools {
            let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
            match devtools::start(cmd_tx, self.proxy.clone()) {
                Ok(port) => {
                    running.devtools_rx = Some(cmd_rx);
                    eprintln!("[dev-tools] http://127.0.0.1:{port}");
                }
                Err(e) => {
                    eprintln!("[dev-tools] サーバ起動失敗: {e}");
                }
            }
        }

        // 外部アプリ (ゲーム等) に GPU を譲るため、GPU 使用率の監視を常時起動する。
        // 現状 Windows のみで動作し、それ以外のプラットフォームでは NOP。
        if let Some(m) = gpu_usage::start_monitoring() {
            running.gpu_monitor = Some(m);
            eprintln!("[auto-hwdec] GPU 使用率の監視を開始");
        }

        Ok(running)
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match self.init(event_loop) {
            Ok(mut running) => {
                if let Some(url) = self.initial_url.take() {
                    running.url_input = url.clone();
                    running.load(&url);
                    if let Some(vid) = auth::extract_video_id(&running.current_url) {
                        running.start_chat(vid.clone());
                        running.start_recommend(vid);
                    }
                }
                self.state = Some(running);
            }
            Err(e) => {
                eprintln!("initialization failed: {e:#}");
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: UserEvent) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = &mut self.state else {
            return;
        };

        let response = state.egui_glow.on_window_event(&state.window, &event);

        // マウス/キー操作があれば UI を再表示し、非表示までの時間を計り直す。
        match &event {
            WindowEvent::CursorMoved { .. }
            | WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
            | WindowEvent::KeyboardInput { .. } => {
                state.last_activity = Instant::now();
                if !state.ui_visible {
                    state.window.request_redraw();
                }
            }
            _ => {}
        }

        match event {
            WindowEvent::CloseRequested => {
                if let Some(s) = &mut self.state {
                    s.stop_chat();
                }
                self.state = None;
                event_loop.exit();
                return;
            }
            WindowEvent::Resized(size) => {
                if let (Some(w), Some(h)) =
                    (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                {
                    state.gl_surface.resize(&state.gl_context, w, h);
                }
                state.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                state.redraw();
            }
            _ => {}
        }

        if response.repaint {
            state.window.request_redraw();
        }
    }
}

/// CLI 引数のパース結果。
struct CliArgs {
    url: Option<String>,
    verbose: bool,
    backend: String,
    enable_dev_tools: bool,
    volume: Option<f64>,
}

fn parse_args() -> Result<CliArgs> {
    let mut verbose = false;
    let mut backend = auth::DEFAULT_BACKEND.to_string();
    let mut url: Option<String> = None;
    let mut enable_dev_tools = false;
    let mut volume: Option<f64> = None;

    let parse_volume = |s: &str| -> Result<f64> {
        let v: f64 = s
            .parse()
            .map_err(|_| anyhow!("--volume の値が不正です: {s}"))?;
        Ok(v.clamp(0.0, 130.0))
    };

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-v" | "--verbose" => verbose = true,
            "--debug-backend" => {
                backend = args
                    .next()
                    .ok_or_else(|| anyhow!("--debug-backend に URL を指定してください"))?;
            }
            "--enable-dev-tools" => enable_dev_tools = true,
            // 初期音量（デバッグ用。例: --volume 0 で無音起動）。`--volume=0` 形式も可。
            "--volume" => {
                let v = args
                    .next()
                    .ok_or_else(|| anyhow!("--volume に値を指定してください"))?;
                volume = Some(parse_volume(&v)?);
            }
            s if s.starts_with("--volume=") => {
                volume = Some(parse_volume(&s["--volume=".len()..])?);
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other if other.starts_with('-') => {
                bail!("不明なオプション: {other}");
            }
            _ => {
                if url.is_some() {
                    bail!("URL が複数指定されています");
                }
                url = Some(a);
            }
        }
    }

    Ok(CliArgs {
        url,
        verbose,
        backend: backend.trim_end_matches('/').to_string(),
        enable_dev_tools,
        volume,
    })
}

fn print_help() {
    println!(
        "YouTube Super Lite\n\
         \n\
         Usage: youtube-super-lite [OPTIONS] [URL]\n\
         \n\
         Options:\n\
         \x20\x20-v, --verbose             mpv の詳細ログを出力\n\
         \x20\x20    --debug-backend URL   認証バックエンドを上書き（デバッグ用、デフォルト: {}）\n\
         \x20\x20    --enable-dev-tools    デバッグ用のローカル HTTP サーバを起動\n\
         \x20\x20                          (GET /screenshot 等。listen ポートは stderr に出力)\n\
         \x20\x20    --volume N            初期音量 0-130（デバッグ用。例: --volume 0 で無音）\n\
         \x20\x20-h, --help                このヘルプを表示",
        auth::DEFAULT_BACKEND
    );
}

fn main() -> Result<()> {
    let args = parse_args()?;

    if let Some(url) = &args.url {
        println!("YouTube Super Lite - playing: {url}");
    } else {
        println!("YouTube Super Lite - URL 欄に貼り付けて Enter で再生");
    }

    ensure_ytdlp_on_path();

    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    let mut app = App::new(
        proxy,
        args.url,
        args.verbose,
        args.backend,
        args.enable_dev_tools,
        args.volume,
    );
    event_loop.run_app(&mut app)?;
    Ok(())
}
