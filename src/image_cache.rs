//! 画像（サムネ等）のディスクキャッシュ。
//!
//! ネイティブ版（`native_overlay`）が一覧サムネを表示するために使う。URL → FNV ハッシュの
//! ファイル名で OS のキャッシュ領域へ保存し、WIC でデコードする。取得は自前で行う
//! （[`ensure_cached_async`] が背景スレッドで download → 保存）。

use std::path::PathBuf;
use std::sync::Mutex;

/// 画像ディスクキャッシュのディレクトリ。
pub fn cache_dir() -> PathBuf {
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
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        PathBuf::from(".").join(".ysl-image-cache")
    }
}

/// 既にディスクキャッシュ済みなら、その画像ファイルのパスを返す（WIC デコード用）。
pub fn cached_path(uri: &str) -> Option<PathBuf> {
    let p = cache_dir().join(hash_uri(uri));
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

/// 未キャッシュなら背景スレッドでダウンロードして保存する。同一 URL の多重取得は
/// in-flight セットで抑止。取得後は [`cached_path`] が当たるようになる。
pub fn ensure_cached_async(uri: &str) {
    use std::collections::HashSet;
    use std::sync::OnceLock;
    static INFLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    if uri.is_empty() {
        return;
    }
    let dir = cache_dir();
    let path = dir.join(hash_uri(uri));
    if path.is_file() {
        return;
    }
    let set = INFLIGHT.get_or_init(|| Mutex::new(HashSet::new()));
    {
        let mut g = set.lock().unwrap();
        if g.contains(uri) {
            return;
        }
        g.insert(uri.to_string());
    }
    let uri = uri.to_string();
    std::thread::spawn(move || {
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(client) = reqwest::blocking::Client::builder()
            .user_agent("Mozilla/5.0")
            .build()
        {
            if let Ok(resp) = client.get(&uri).send() {
                if let Ok(bytes) = resp.bytes() {
                    if !bytes.is_empty() {
                        let tmp = path.with_extension("tmp");
                        if std::fs::write(&tmp, &bytes).is_ok() {
                            let _ = std::fs::rename(&tmp, &path);
                        }
                    }
                }
            }
        }
        if let Some(set) = INFLIGHT.get() {
            let _ = set.lock().map(|mut g| g.remove(&uri));
        }
    });
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
