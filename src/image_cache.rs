//! 画像の永続キャッシュ（egui `BytesLoader` 実装）。
//!
//! egui の画像ロードは「BytesLoader(URI→生バイト) → ImageLoader(decode) →
//! TextureLoader(GPUアップロード)」の 3 段。本モジュールは **1 段目だけ** を差し替え、
//! サムネ / チャンネルアイコン / カスタム絵文字などの HTTP 画像を:
//!   1. メモリ map に命中すれば即返す
//!   2. ディスクキャッシュに命中すれば読んでメモリへ
//!   3. どちらもミスなら並列ワーカ（同時数制限つき）で取得し、その間は Pending
//! という順で解決する。`egui::Image::new(url)` の呼び出し側は変更不要。
//!
//! 設計判断（「いったんそれで」で確定したデフォルト）:
//!   - 失効は **総容量 LRU のみ**（アイコン TTL は入れない）。サムネ/アイコンの URL は
//!     実質コンテンツ固定なので基本は保持し、ディスク上限超過分のみ mtime 昇順で削除。
//!   - ハッシュは依存追加なしの **FNV-1a 64bit**（`DefaultHasher` は実行ごとにシードが
//!     変わりファイル名が安定しないため使えない）。
//!   - 同時取得数を固定ワーカ数に絞ることで、500 件グリッド初回表示の一斉フェッチによる
//!     描画ストールを防ぐ。
//!
//! egui の登録順: `install_image_loaders` の **後** に `ctx.add_bytes_loader` する。
//! `try_load_bytes` は後入れ優先（`.rev()`）で走査するため、本ローダが先に当たる。
//! `http`/`https` 以外は `NotSupported` を返し、`file://` 等は既存ローダへ委譲する。

use egui::load::{Bytes, BytesLoadResult, BytesLoader, BytesPoll, LoadError};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

/// メモリ側キャッシュの総バイト上限。超えたら LRU で古い Ready を捨てる。
/// （egui 側の decode/texture キャッシュは別管理。ここはバイト層のみ。）
const MEM_CAP_BYTES: usize = 64 * 1024 * 1024;
/// ディスクキャッシュの総バイト上限。起動時に mtime 昇順で超過分を削除。
const DISK_CAP_BYTES: u64 = 256 * 1024 * 1024;
/// 同時フェッチ数（= ワーカスレッド数）。
const WORKERS: usize = 6;

/// 1 URI 分の状態。
enum Entry {
    /// ワーカに投入済み・取得中。
    Pending,
    /// 取得完了。`last` は LRU 用のアクセス tick。
    Ready {
        bytes: Arc<[u8]>,
        mime: Option<String>,
        size: usize,
        last: u64,
    },
    /// 取得失敗（再試行は再起動 or forget まで行わない）。
    Failed,
}

struct Inner {
    map: HashMap<String, Entry>,
    /// Ready エントリの合計バイト数（byte_size / LRU 判定用）。
    total: usize,
    /// アクセス tick（単調増加）。LRU の新旧比較に使う。
    tick: u64,
}

pub struct DiskImageCache {
    inner: Arc<Mutex<Inner>>,
    /// ワーカへ URI を投げるキュー。
    job_tx: Sender<String>,
}

impl DiskImageCache {
    /// キャッシュを構築し、ワーカと起動時のディスク掃除スレッドを立ち上げる。
    /// `ctx` は取得完了時の `request_repaint` に使う（clone は安価な Arc）。
    pub fn new(dir: PathBuf, ctx: egui::Context) -> Self {
        let _ = std::fs::create_dir_all(&dir);

        // 起動時にディスク上限を超えていれば古いものから削除（バックグラウンド）。
        {
            let dir = dir.clone();
            std::thread::spawn(move || sweep_disk(&dir, DISK_CAP_BYTES));
        }

        // ワーカと load() で同一の Inner を共有する。
        let inner = Arc::new(Mutex::new(Inner {
            map: HashMap::new(),
            total: 0,
            tick: 0,
        }));

        let (job_tx, job_rx) = mpsc::channel::<String>();
        let job_rx = Arc::new(Mutex::new(job_rx));

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());

        for _ in 0..WORKERS {
            let job_rx = job_rx.clone();
            let dir = dir.clone();
            let client = client.clone();
            let inner = inner.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || loop {
                // recv はロックを取って 1 件取り出す（複数ワーカで共有する mpsc receiver）。
                let uri = {
                    let rx = job_rx.lock().unwrap();
                    match rx.recv() {
                        Ok(u) => u,
                        Err(_) => break, // 送信側が落ちた = 終了
                    }
                };
                process_job(&uri, &dir, &client, &inner);
                ctx.request_repaint();
            });
        }

        Self { inner, job_tx }
    }
}

/// FNV-1a 64bit ハッシュ → 16 桁 hex。URL→キャッシュファイル名に使う。
fn hash_uri(uri: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in uri.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// ワーカ 1 件分: ディスク命中なら読む、ミスなら取得して保存。結果をメモリへ反映。
fn process_job(
    uri: &str,
    dir: &std::path::Path,
    client: &reqwest::blocking::Client,
    inner: &Mutex<Inner>,
) {
    let path = dir.join(hash_uri(uri));

    // 1. ディスク命中。
    if let Ok(bytes) = std::fs::read(&path) {
        if !bytes.is_empty() {
            store_ready(inner, uri, bytes, None);
            return;
        }
    }

    // 2. ネットワーク取得。
    let result = client.get(uri).send().and_then(|r| r.error_for_status());
    match result {
        Ok(resp) => {
            let mime = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.split(';').next().unwrap_or(s).trim().to_string());
            match resp.bytes() {
                Ok(body) if !body.is_empty() => {
                    // atomic: tmp に書いて rename。
                    let tmp = path.with_extension("tmp");
                    if std::fs::write(&tmp, &body).is_ok() {
                        let _ = std::fs::rename(&tmp, &path);
                    }
                    store_ready(inner, uri, body.to_vec(), mime);
                }
                _ => mark_failed(inner, uri),
            }
        }
        Err(_) => mark_failed(inner, uri),
    }
}

fn store_ready(inner: &Mutex<Inner>, uri: &str, bytes: Vec<u8>, mime: Option<String>) {
    let mut g = inner.lock().unwrap();
    let size = bytes.len();
    let tick = g.tick + 1;
    g.tick = tick;
    // 既存 Ready を置き換える場合は旧サイズを差し引く。
    if let Some(Entry::Ready { size: old, .. }) = g.map.get(uri) {
        g.total = g.total.saturating_sub(*old);
    }
    g.map.insert(
        uri.to_string(),
        Entry::Ready {
            bytes: bytes.into(),
            mime,
            size,
            last: tick,
        },
    );
    g.total += size;
    evict_if_needed(&mut g);
}

fn mark_failed(inner: &Mutex<Inner>, uri: &str) {
    let mut g = inner.lock().unwrap();
    if let Some(Entry::Ready { size, .. }) = g.map.get(uri) {
        g.total = g.total.saturating_sub(*size);
    }
    g.map.insert(uri.to_string(), Entry::Failed);
}

/// メモリ上限超過時、最も古い（last 最小）Ready を捨てて total を下げる。
fn evict_if_needed(g: &mut Inner) {
    if g.total <= MEM_CAP_BYTES {
        return;
    }
    // Ready のみを (last, uri, size) で集めて昇順に削除。
    let mut readies: Vec<(u64, String, usize)> = g
        .map
        .iter()
        .filter_map(|(k, v)| match v {
            Entry::Ready { last, size, .. } => Some((*last, k.clone(), *size)),
            _ => None,
        })
        .collect();
    readies.sort_by_key(|(last, _, _)| *last);
    for (_, uri, size) in readies {
        if g.total <= MEM_CAP_BYTES {
            break;
        }
        g.map.remove(&uri);
        g.total = g.total.saturating_sub(size);
    }
}

/// ディスクキャッシュ掃除: 合計サイズが cap を超えていたら mtime 昇順で超過分を削除。
fn sweep_disk(dir: &std::path::Path, cap: u64) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    let mut total: u64 = 0;
    for ent in rd.flatten() {
        let path = ent.path();
        // tmp は対象外（書きかけ）。
        if path.extension().map(|e| e == "tmp").unwrap_or(false) {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        let Ok(meta) = ent.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let len = meta.len();
        let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
        total += len;
        files.push((mtime, len, path));
    }
    if total <= cap {
        return;
    }
    files.sort_by_key(|(mtime, _, _)| *mtime); // 古い順
    for (_, len, path) in files {
        if total <= cap {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
}

impl BytesLoader for DiskImageCache {
    fn id(&self) -> &str {
        "youtube_super_lite::image_cache::DiskImageCache"
    }

    fn load(&self, _ctx: &egui::Context, uri: &str) -> BytesLoadResult {
        // http(s) 以外は他ローダ（file:// / bytes:// 等）へ委譲。
        if !(uri.starts_with("http://") || uri.starts_with("https://")) {
            return Err(LoadError::NotSupported);
        }

        let mut g = self.inner.lock().unwrap();
        match g.map.get(uri) {
            Some(Entry::Ready { bytes, mime, .. }) => {
                let bytes = bytes.clone();
                let mime = mime.clone();
                // LRU: アクセス tick を更新。
                let tick = g.tick + 1;
                g.tick = tick;
                if let Some(Entry::Ready { last, .. }) = g.map.get_mut(uri) {
                    *last = tick;
                }
                Ok(BytesPoll::Ready {
                    size: None,
                    bytes: Bytes::Shared(bytes),
                    mime,
                })
            }
            Some(Entry::Pending) => Ok(BytesPoll::Pending { size: None }),
            Some(Entry::Failed) => {
                Err(LoadError::Loading("画像の取得に失敗".to_string()))
            }
            None => {
                // 初回ミス: Pending にして 1 度だけワーカへ投入。
                g.map.insert(uri.to_string(), Entry::Pending);
                drop(g);
                let _ = self.job_tx.send(uri.to_string());
                Ok(BytesPoll::Pending { size: None })
            }
        }
    }

    fn forget(&self, uri: &str) {
        let mut g = self.inner.lock().unwrap();
        if let Some(Entry::Ready { size, .. }) = g.map.get(uri) {
            g.total = g.total.saturating_sub(*size);
        }
        g.map.remove(uri);
    }

    fn forget_all(&self) {
        let mut g = self.inner.lock().unwrap();
        g.map.clear();
        g.total = 0;
    }

    fn byte_size(&self) -> usize {
        self.inner.lock().unwrap().total
    }
}
