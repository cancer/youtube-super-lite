use std::env;
use std::path::PathBuf;

// libmpv2-sys は `cargo:rustc-link-lib=mpv` を出すだけで検索パスを設定しない。
// プラットフォームに応じてライブラリの検索パスを追加する。
fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_os == "macos" {
        // macOS: pkg-config で libmpv のパスを解決する。
        if let Ok(output) = std::process::Command::new("pkg-config")
            .args(["--libs-only-L", "mpv"])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for flag in stdout.split_whitespace() {
                if let Some(path) = flag.strip_prefix("-L") {
                    println!("cargo:rustc-link-search=native={path}");
                }
            }
        }
    } else {
        // Windows: 同梱の libmpv 開発ファイル (tools/mpv-dev/mpv.lib) を探す。
        let manifest_dir =
            PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
        let mpv_dir = manifest_dir.join("tools").join("mpv-dev");
        println!("cargo:rustc-link-search=native={}", mpv_dir.display());
    }

    println!("cargo:rerun-if-changed=build.rs");
}
