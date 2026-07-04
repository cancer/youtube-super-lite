//! UI 非依存のアプリケーションコア。winit 等の UI フレームワークへの依存を持たない
//! （Cargo.toml に依存が存在しないため import 不能）。詳細は docs/design/design-principles.md。

pub mod account;
pub mod chat;
pub mod content;
pub mod flows;
pub mod gpu_usage;
pub mod image_cache;
pub mod player;
pub mod playback;
pub mod types;
pub mod yt;

/// 背景スレッドがメインループを起こすためのコールバック。bin 側で `EventLoopProxy` を
/// 包んで注入する。lib は winit を知らないため、この抽象を介して起床を要求する。
pub type Waker = std::sync::Arc<dyn Fn() + Send + Sync>;
