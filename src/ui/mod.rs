//! UI 層（Issue #11 PR U）。`shell` が地雷（Win32/winit の暗黙知）を集約し、以後ほぼ不変。
//! `actions` は全入力の合流点（`apply_action` / devtools コマンド）、`present` は
//! 状態→描画データへの純関数（`list_rows` / `state_json` 等）。

mod actions;
mod present;
pub mod shell;

pub use shell::NativeApp;
