//! コンテンツ一覧のドメイン層。おすすめ/チャンネルビュー/登録チャンネル新着/再生履歴/
//! 再生リスト/チャンネルアバターは互いに不変条件を共有しない独立した状態機械の集まりなので、
//! 束ねる `Content` 型は作らない（design-principles.md 原則1）。呼び出し側が個別フィールドで持つ。

use crate::yt::{history, playlist, recommend, subscriptions};
use crate::Waker;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

/// 一覧系 fetch の背景スレッド→メインの通知。旧 RecommendUpdate/SubUpdate/HistoryUpdate を統一。
pub enum FeedUpdate<T> {
    Items(Vec<T>),
    Error(String),
}

/// `Feed<T>` の system がアバター補完のために要求する、要素の「チャンネル名」。
pub trait HasChannel {
    fn channel_name(&self) -> &str;
}

impl HasChannel for recommend::VideoItem {
    fn channel_name(&self) -> &str {
        &self.channel
    }
}
impl HasChannel for subscriptions::SubVideo {
    fn channel_name(&self) -> &str {
        &self.channel
    }
}
impl HasChannel for history::HistoryItem {
    fn channel_name(&self) -> &str {
        &self.channel
    }
}

/// 非同期取得する一覧の共通状態。フィールドは private（書き込みは本モジュールの関数のみ）。
pub struct Feed<T> {
    items: Vec<T>,
    tx: Sender<FeedUpdate<T>>,
    rx: Receiver<FeedUpdate<T>>,
    busy: bool,
    /// エラーログの識別用（例: "recommend"）。
    label: &'static str,
}

impl<T> Feed<T> {
    pub fn new(label: &'static str) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self { items: Vec::new(), tx, rx, busy: false, label }
    }

    pub fn items(&self) -> &[T] {
        &self.items
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }
}

/// system: rx を drain して取り込む。新しい Items が来たら true。
/// Error は eprintln! で記録するだけ（status フィールドは持たない — 旧実装の status は
/// 全機能で誰にも読まれていないデッドだったため、再現しない）。
/// Items 到着→チャンネル名収集→アバター依頼、の連鎖はここで完結させる（avatars を触るので
/// シグネチャに現れる。同じ理由で waker も要る — アバター解決の背景スレッドを起こすため）。
pub fn poll_feed<T: HasChannel>(f: &mut Feed<T>, avatars: &mut AvatarCache, waker: &Waker) -> bool {
    let mut updated = false;
    while let Ok(update) = f.rx.try_recv() {
        match update {
            FeedUpdate::Items(items) => {
                updated = true;
                f.busy = false;
                let names: Vec<String> = items.iter().map(|v| v.channel_name().to_string()).collect();
                f.items = items;
                request_avatars(avatars, names, waker);
            }
            FeedUpdate::Error(e) => {
                f.busy = false;
                eprintln!("[{}] 取得エラー: {e}", f.label);
            }
        }
    }
    updated
}

/// system: 取得開始の帳簿（busy=true, items.clear()）をして、spawn 用に tx の clone を返す。
/// 旧 items は消す — 一覧は開くたびに 0 から組み立てる方針で、手元の旧データはキャッシュ
/// ではない（いつ取得したか不明な内容を取得中に一瞬見せない）。取得中の空一覧は UI 側が
/// busy を見て「取得中…」を表示する。
pub fn begin_fetch<T>(f: &mut Feed<T>) -> Sender<FeedUpdate<T>> {
    f.busy = true;
    f.items.clear();
    f.tx.clone()
}

/// チャンネル名クリックで開く、特定チャンネルの動画一覧（アバター/名前クリックで開く対）。
pub struct ChannelView {
    feed: Feed<recommend::VideoItem>,
    title: String,
}

impl ChannelView {
    pub fn new() -> Self {
        Self { feed: Feed::new("channel"), title: String::new() }
    }

    pub fn items(&self) -> &[recommend::VideoItem] {
        self.feed.items()
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn is_busy(&self) -> bool {
        self.feed.is_busy()
    }
}

impl Default for ChannelView {
    fn default() -> Self {
        Self::new()
    }
}

/// 再生リストの二階層ナビゲーション（一覧⇄選択したリストの動画一覧）という 1 つの機械。
pub struct Playlist {
    lists: Vec<playlist::PlaylistSummary>,
    items: Vec<playlist::PlaylistItem>,
    items_title: String,
    tx: Sender<playlist::PlaylistUpdate>,
    rx: Receiver<playlist::PlaylistUpdate>,
    busy: bool,
}

impl Playlist {
    pub fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self { lists: Vec::new(), items: Vec::new(), items_title: String::new(), tx, rx, busy: false }
    }

    pub fn lists(&self) -> &[playlist::PlaylistSummary] {
        &self.lists
    }

    pub fn items(&self) -> &[playlist::PlaylistItem] {
        &self.items
    }

    pub fn items_title(&self) -> &str {
        &self.items_title
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// 動画一覧が開かれているか（= リスト一覧ではなく中身を見ている）。
    /// native_app の階層判定（旧 `playlist_items.is_empty()` の否定）を置き換える。
    pub fn is_items_view(&self) -> bool {
        !self.items.is_empty()
    }
}

impl Default for Playlist {
    fn default() -> Self {
        Self::new()
    }
}

/// チャンネルアバター（名前→URL キャッシュ）。解決済み+依頼済みの整合が不変条件。
/// TV tile がアバターを持たないので、無認証 WEB 検索で名前から補完する。
pub struct AvatarCache {
    map: HashMap<String, String>,
    requested: HashSet<String>,
    tx: Sender<(String, String)>,
    rx: Receiver<(String, String)>,
}

impl AvatarCache {
    pub fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self { map: HashMap::new(), requested: HashSet::new(), tx, rx }
    }

    pub fn url_for(&self, name: &str) -> Option<&str> {
        self.map.get(name).map(String::as_str)
    }
}

impl Default for AvatarCache {
    fn default() -> Self {
        Self::new()
    }
}

/// system: 未解決のチャンネル名のアバターを無認証 WEB 検索で背景解決する（1 スレッドで順次）。
/// 二重リクエスト防止の HashSet ロジックごと移植。
pub fn request_avatars(avatars: &mut AvatarCache, names: Vec<String>, waker: &Waker) {
    let mut todo = Vec::new();
    for name in names {
        if name.is_empty() || avatars.requested.contains(&name) {
            continue;
        }
        avatars.requested.insert(name.clone());
        todo.push(name);
    }
    if todo.is_empty() {
        return;
    }
    let tx = avatars.tx.clone();
    let waker = Arc::clone(waker);
    std::thread::spawn(move || {
        for name in todo {
            if let Some(url) = subscriptions::fetch_channel_avatar(&name) {
                let _ = tx.send((name, url));
                waker();
            }
        }
    });
}

/// system: アバターの解決結果を取り込む（名前→URL）。
pub fn poll_avatars(avatars: &mut AvatarCache) {
    while let Ok((name, url)) = avatars.rx.try_recv() {
        avatars.map.insert(name, url);
    }
}

/// system: おすすめ（ホームフィード FEwhat_to_watch）を背景スレッドで取得する。要ログイン。
/// オーバーレイを開くたびに呼ばれるようになったため、多重リクエスト防止の busy ガードを持つ
/// （ログイン時の先読みと開いた直後の再取得が重なるケースを含む）。
pub fn start_recommend(f: &mut Feed<recommend::VideoItem>, token: &str, waker: &Waker) {
    if f.is_busy() {
        return;
    }
    let tx = begin_fetch(f);
    let access_token = token.to_string();
    let waker = Arc::clone(waker);
    std::thread::spawn(move || {
        recommend::fetch_home_feed(&access_token, &tx);
        waker();
    });
}

/// system: 登録チャンネルタブの新着フィードを背景スレッドで取得する。
pub fn start_subs(f: &mut Feed<subscriptions::SubVideo>, token: &str, waker: &Waker) {
    if f.is_busy() {
        return;
    }
    let tx = begin_fetch(f);
    let access_token = token.to_string();
    let waker = Arc::clone(waker);
    std::thread::spawn(move || {
        subscriptions::fetch_subscription_feed(&access_token, &tx);
        waker();
    });
}

/// system: 再生履歴を背景スレッドで取得する。
pub fn start_history(f: &mut Feed<history::HistoryItem>, token: &str, waker: &Waker) {
    if f.is_busy() {
        return;
    }
    let tx = begin_fetch(f);
    let access_token = token.to_string();
    let waker = Arc::clone(waker);
    std::thread::spawn(move || {
        history::fetch_history(&access_token, &tx);
        waker();
    });
}

/// system: チャンネル名からそのチャンネルの動画一覧を背景取得する（名前→channelId→browse）。
pub fn open_channel(cv: &mut ChannelView, name: String, waker: &Waker) {
    cv.title = name.clone();
    let tx = begin_fetch(&mut cv.feed);
    let waker = Arc::clone(waker);
    std::thread::spawn(move || {
        let result = match subscriptions::fetch_channel_id(&name) {
            Some(id) => recommend::fetch_channel_videos(&id).map_err(|e| e.to_string()),
            None => Err(format!("チャンネルが見つかりません: {name}")),
        };
        let _ = tx.send(match result {
            Ok(items) => FeedUpdate::Items(items),
            Err(e) => FeedUpdate::Error(e),
        });
        waker();
    });
}

/// system: 実 channelId(UC...) からそのチャンネルの動画一覧を背景取得する（名前検索を経由しない、
/// より確実な経路。ケバブメニューの「チャンネルへ」が実IDを持つ場合に使う）。
pub fn open_channel_by_id(cv: &mut ChannelView, id: String, title: String, waker: &Waker) {
    cv.title = title;
    let tx = begin_fetch(&mut cv.feed);
    let waker = Arc::clone(waker);
    std::thread::spawn(move || {
        let result = recommend::fetch_channel_videos(&id).map_err(|e| e.to_string());
        let _ = tx.send(match result {
            Ok(items) => FeedUpdate::Items(items),
            Err(e) => FeedUpdate::Error(e),
        });
        waker();
    });
}

/// system: チャンネルビューの取り込み（アバター補完込み）。`poll_feed` と同じ作法だが
/// `ChannelView` はフィード 1 本 + title の対なので専用の system にする。
pub fn poll_channel_view(cv: &mut ChannelView, avatars: &mut AvatarCache, waker: &Waker) -> bool {
    poll_feed(&mut cv.feed, avatars, waker)
}

/// system: 再生リストの更新を取り込む。
pub fn poll_playlist(p: &mut Playlist) {
    while let Ok(update) = p.rx.try_recv() {
        p.busy = false;
        match update {
            playlist::PlaylistUpdate::Playlists(lists) => {
                p.lists = lists;
                // リスト一覧に戻ったので動画一覧をクリア。
                p.items.clear();
                p.items_title.clear();
            }
            playlist::PlaylistUpdate::Items { title, items } => {
                p.items_title = title;
                p.items = items;
            }
            playlist::PlaylistUpdate::Error(e) => {
                eprintln!("[playlist] 取得エラー: {e}");
            }
        }
    }
}

/// system: 自分の再生リスト一覧を背景スレッドで取得する（0 から組み立てる。begin_fetch と同方針）。
pub fn start_playlist_list(p: &mut Playlist, token: &str, waker: &Waker) {
    if p.busy {
        return;
    }
    p.busy = true;
    p.lists.clear();
    p.items.clear();
    p.items_title.clear();
    let access_token = token.to_string();
    let tx = p.tx.clone();
    let waker = Arc::clone(waker);
    std::thread::spawn(move || {
        playlist::fetch_my_playlists(&access_token, &tx);
        waker();
    });
}

/// system: 選択した再生リストの動画一覧を背景スレッドで取得する。
pub fn start_playlist_items(p: &mut Playlist, playlist_id: String, title: String, token: &str, waker: &Waker) {
    if p.busy {
        return;
    }
    p.busy = true;
    let access_token = token.to_string();
    let tx = p.tx.clone();
    let waker = Arc::clone(waker);
    std::thread::spawn(move || {
        playlist::fetch_playlist_items(&access_token, &playlist_id, &title, &tx);
        waker();
    });
}

/// system: リスト一覧に戻る（動画一覧とタイトルを同時にクリア）。
pub fn back_to_lists(p: &mut Playlist) {
    p.items.clear();
    p.items_title.clear();
}
