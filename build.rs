use std::env;
use std::path::PathBuf;

// libmpv2-sys は `cargo:rustc-link-lib=mpv` を出すだけで検索パスを設定しない。
// ここで同梱の libmpv 開発ファイル (tools/mpv-dev/mpv.lib) を探せるようにする。
fn main() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let mpv_dir = manifest_dir.join("tools").join("mpv-dev");

    println!("cargo:rustc-link-search=native={}", mpv_dir.display());
    println!("cargo:rerun-if-changed=build.rs");
}
