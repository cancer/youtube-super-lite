//! 再生のドメイン層。装置(mpv)・常駐リゾルバ・ユーザー設定(quality/codec)はアプリ寿命
//! （作り直したら設計意図が消える）。再生セッションは 1 URL : 1 インスタンスで丸ごと差し替える
//! （design-principles.md「寿命は現実の寿命に合わせる」）。

use crate::types::{Codec, EqParams, Quality};
use crate::yt::resolve;
use crate::{gpu_usage, player};
use crate::Waker;
use std::sync::atomic::AtomicI64;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// `poll_resolve` から shell への「今回の解決結果はコアの mpv 経路では処理しきれない」旨の合図
/// （issue #16 PR3）。返り値で経路要求を出すことで、shell 層に閉じている WebView2 の
/// 具体的な操作（`navigate_embed`）をコア側に持ち込まずに済む。
pub enum PendingRoute {
    /// SABR 詰みライブが検知され、公式 IFrame プレーヤー(WebView2)へ切替を要求する。
    Webview { video_id: String },
}

/// UI 非依存の再生状態。
pub struct Playback {
    // --- 装置と好み（アプリ寿命）---
    player: player::Player,
    resolve_handle: resolve::ResolverHandle,
    quality: Quality,
    codec: Codec,
    eq: EqParams,
    player_offset_ms: Arc<AtomicI64>,
    gpu_monitor: Option<gpu_usage::Monitor>,
    // --- 現在の再生（丸ごと差し替え）---
    current_url: String,
    is_live: bool,
    session: Option<PlaySession>,
    /// 自動ログイン完了待ちで解決を保留している URL（auth レース対策。§flows::play 参照）。
    pending_resolve: Option<String>,
}

/// 再生ごとに丸ごと差し替える状態。手動リセットの儀式（旧 load() の5フィールド初期化）は
/// この構造ごと消える — `Playback::begin_load` が `Some(new_session)` で置き換えるだけ。
struct PlaySession {
    reply_rx: Receiver<resolve::ResolveUpdate>,
    /// 並列解決の予備（ローカル中継＝サイドカー）。native の再生が mpv で失敗
    /// （403/開けない）したとき即座に切り替えるために控える。
    pending_fallback: Option<resolve::Resolved>,
    /// 直近の native ロード時刻。一定時間内に再生が始まらず idle なら失敗とみなす。
    native_load_at: Option<Instant>,
    /// native ロード後、再生開始 or 失敗を監視中か（フォールバック起動の対象）。
    fallback_armed: bool,
}

impl Playback {
    pub fn new(player: player::Player, waker: &Waker) -> Self {
        Self {
            resolve_handle: resolve::ResolverHandle::spawn(waker.clone()),
            player,
            quality: Quality::Auto,
            codec: Codec::Auto,
            eq: EqParams::default(),
            player_offset_ms: Arc::new(AtomicI64::new(0)),
            gpu_monitor: gpu_usage::start_monitoring(),
            current_url: String::new(),
            is_live: false,
            session: None,
            pending_resolve: None,
        }
    }

    /// player への直接操作（pause/seek/volume 等）用。Player 自体は閉じた API なので
    /// ラップし直さない。
    pub fn player(&self) -> &player::Player {
        &self.player
    }

    pub fn current_url(&self) -> &str {
        &self.current_url
    }

    pub fn is_live(&self) -> bool {
        self.is_live
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }

    pub fn codec(&self) -> Codec {
        self.codec
    }

    pub fn eq(&self) -> EqParams {
        self.eq
    }

    pub fn player_offset_ms(&self) -> &Arc<AtomicI64> {
        &self.player_offset_ms
    }

    /// GPU 使用率監視が有効か（Windows のみ）。ログ表示用。
    pub fn has_gpu_monitor(&self) -> bool {
        self.gpu_monitor.is_some()
    }
}

/// system: 画質設定を変更する（再解決の判断は呼び出し側。PR B で apply_action に一本化）。
pub fn set_quality(pb: &mut Playback, q: Quality) {
    pb.quality = q;
}

/// system: コーデック設定を変更する。
pub fn set_codec(pb: &mut Playback, c: Codec) {
    pb.codec = c;
}

/// system: EQ 設定を変更し、再生バックエンドへ即時反映する（クランプ込み）。
/// バックエンド分岐（#16: mpv / webview）を将来足すのはこの関数の中だけ。
/// 同値なら af の再設定をスキップする（ドラッグ中の連続 MOUSEMOVE で毎回 mpv の
/// lavfi フィルタグラフを再構築させない。初回の neutral→neutral も、Playback::new の
/// 既定値が af 未設定の mpv と一致しているため正しく弾かれる）。
pub fn set_eq(pb: &mut Playback, eq: EqParams) {
    let eq = eq.clamped();
    if pb.eq == eq {
        return;
    }
    pb.eq = eq;
    pb.player.set_af(&pb.eq.mpv_af());
}

/// system: ログイン待ちで解決を保留する（auth レース対策）。旧 `Controller::load` は保留する
/// か否かに関わらず先頭で current_url 等の帳簿を取っていたため、ここでも `begin_load` を通す
/// （さもないと current_url が古いまま残り、ログイン確定時の mark_watched/chat 接続が
/// 前の動画に誤って向く）。
pub fn hold(pb: &mut Playback, url: String) {
    begin_load(pb, url.clone());
    pb.pending_resolve = Some(url);
}

/// system: 保留していた URL を取り出す（ログイン確定/失敗時に呼ぶ）。
pub fn take_pending(pb: &mut Playback) -> Option<String> {
    pb.pending_resolve.take()
}

/// 新しい再生の帳簿（current_url 更新・前セッションの破棄）をする。
/// 前セッションを破棄するだけで、旧実装の「5フィールド手動初期化」の儀式は構造ごと消える。
fn begin_load(pb: &mut Playback, url: String) {
    pb.current_url = url;
    pb.is_live = false; // 解決完了（poll_resolve）で確定する。
    pb.pending_resolve = None;
    pb.session = None;
}

/// system: YouTube 以外の URL（直リンク等）をそのまま mpv に渡す。
pub fn load_direct(pb: &mut Playback, url: String) {
    begin_load(pb, url.clone());
    mpv_loadfile(pb, &url, None, None);
}

/// system: 解決を常駐ワーカーに依頼し、再生セッションを開始する。ログイン中なら access_token を
/// 渡し、members 限定/年齢制限も解錠できるようにする（M17）。
pub fn start_resolve(pb: &mut Playback, url: String, token: Option<&str>) {
    begin_load(pb, url.clone());
    let (reply, reply_rx) = std::sync::mpsc::channel();
    pb.session = Some(PlaySession {
        reply_rx,
        pending_fallback: None,
        native_load_at: None,
        fallback_armed: false,
    });
    // ワーカーは resolve_handle 生成時に waker を受け取り済み（リクエストごとの再注入は不要）。
    pb.resolve_handle.request(resolve::ResolveRequest {
        url,
        quality: pb.quality,
        codec: pb.codec,
        access_token: token.map(|t| t.to_string()),
        reply,
    });
}

/// system: 解決結果を取り込み、mpv に loadfile する（従来経路）。
/// SABR 詰みライブなど mpv では扱えないケースは `Some(PendingRoute)` を返し、
/// 呼び出し側（shell）が WebView2 等の別経路へ実際の再生要求を委譲する。
pub fn poll_resolve(pb: &mut Playback) -> Option<PendingRoute> {
    let session = pb.session.as_mut()?;
    let mut route: Option<PendingRoute> = None;
    while let Ok(update) = session.reply_rx.try_recv() {
        match update {
            resolve::ResolveUpdate::Ready(r) => {
                // URL が取れ次第すぐ再生（タイトルは後追いの Meta で反映）。
                let (video_url, audio_url) = (r.video_url, r.audio_url);
                session.native_load_at = Some(Instant::now());
                session.fallback_armed = true;
                session.pending_fallback = None;
                match pb.player.loadfile(&video_url, audio_url.as_deref(), None) {
                    Ok(_) => println!("loadfile: {video_url}"),
                    Err(e) => eprintln!("loadfile failed: {e}"),
                }
            }
            resolve::ResolveUpdate::Fallback(r) => {
                // 並列に用意された予備（ローカル中継）。再生失敗時まで控える。
                session.pending_fallback = Some(r);
            }
            resolve::ResolveUpdate::Meta { title, is_live } => {
                pb.is_live = is_live;
                if let Some(t) = title {
                    pb.player.set_force_media_title(&t);
                }
            }
            resolve::ResolveUpdate::UseWebview { video_id, title, is_live } => {
                // SABR 詰みライブ。mpv には loadfile せず、shell 側で WebView2 へ委譲する。
                // Meta と同じ状態反映（is_live/title）はここでも行っておく（Meta 送出順に依存しない）。
                pb.is_live = is_live;
                if let Some(t) = title.as_deref() {
                    pb.player.set_force_media_title(t);
                }
                // PR4: 直前まで mpv で別動画を再生していた場合、そのまま音が残る事故を防ぐため
                // 明示的に停止する。loadfile 経路（Ready）は replace で自然に置き換わるので不要。
                pb.player.stop();
                // native ロード監視は不要（そもそも loadfile しない）。fallback 監視も止める。
                session.native_load_at = None;
                session.fallback_armed = false;
                session.pending_fallback = None;
                route = Some(PendingRoute::Webview { video_id });
            }
            resolve::ResolveUpdate::Error(e) => {
                eprintln!("resolve failed: {e}");
            }
        }
    }
    route
}

/// system: native 再生が mpv で失敗（403/開けない）していないか監視し、失敗していれば並列に
/// 用意した予備（ローカル中継＝サイドカー）へ即切替する。メインループから毎ティック呼ぶ。
pub fn check_fallback(pb: &mut Playback) {
    let Some(session) = pb.session.as_mut() else { return };
    if !session.fallback_armed {
        return;
    }
    // 再生が始まっていれば（time-pos が進めば）監視終了＝native 成功。
    if pb.player.time_pos() > 0.5 {
        session.fallback_armed = false;
        return;
    }
    // ロード直後はバッファリング/起動の猶予を与える。
    match session.native_load_at {
        Some(at) if at.elapsed() >= Duration::from_secs(3) => {}
        _ => return,
    }
    // ファイル未ロードのまま idle = native はそのストリームを開けなかった（403 等）。
    // 予備が届いていれば中継へ切替える（届くまでは待つ）。
    if pb.player.idle_active() {
        if let Some(fb) = session.pending_fallback.take() {
            eprintln!("[fallback] native 再生失敗 → ローカル中継(サイドカー)へ切替");
            session.fallback_armed = false;
            mpv_loadfile(pb, &fb.video_url, fb.audio_url.as_deref(), None);
        }
    }
}

/// system: GPU 使用率監視スレッドからの hwdec 切替決定を取り込んで mpv に反映する。
pub fn poll_gpu(pb: &mut Playback) {
    let Some(monitor) = pb.gpu_monitor.as_ref() else {
        return;
    };
    while let Some(decision) = monitor.try_recv() {
        match decision {
            gpu_usage::HwdecDecision::UseSoftware => {
                eprintln!("[auto-hwdec] GPU 高負荷検出 → SW デコードへ切替");
                pb.player.set_hwdec("no");
            }
            gpu_usage::HwdecDecision::UseHardware => {
                eprintln!("[auto-hwdec] GPU 負荷低下 → HW デコードへ復帰");
                pb.player.set_hwdec("auto");
            }
        }
    }
}

/// Player に解決済み URL を渡して再生開始する（load_error は移植しない — 旧実装の全機能で
/// 誰にも読まれていないデッドフィールドだったため）。
fn mpv_loadfile(pb: &mut Playback, video_url: &str, audio_url: Option<&str>, title: Option<&str>) {
    match pb.player.loadfile(video_url, audio_url, title) {
        Ok(_) => println!("loadfile: {video_url}"),
        Err(e) => eprintln!("loadfile failed: {e}"),
    }
}
