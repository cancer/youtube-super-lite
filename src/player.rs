//! 動画プレイヤー（libmpv ラッパー）。
//!
//! mpv に関する状態（インスタンス・RenderContext・描画先 FBO/テクスチャ）を
//! すべてこの構造体内に閉じ込める。UI 層からは:
//!   - `Player::render(w, h)` で動画フレームを内部テクスチャに描画
//!   - `Player::texture_id()` でそのテクスチャ ID（OpenGL）を取得
//!   - 残りはプロパティ get/set / loadfile / seek 等の薄いラッパー
//! を呼ぶ。UI バックエンドを差し替えても Player は変更不要。
//!
//! 設計メモ:
//!   - mpv は `Box::leak` で `'static` 化し `RenderContext<'static>` を成立させる。
//!     `RenderContext` が `Mpv` を借用するため自己参照を回避するこの形が必要。
//!   - 動画はデフォルト FBO ではなく自前 FBO に描画し、UI 層が任意の方法で
//!     テクスチャを使えるようにする（egui の `PaintCallback`、ネイティブ UI の
//!     OpenGL レイヤ、シェーダで合成、いずれにも対応可能）。

use anyhow::{anyhow, bail, Result};
use glow::HasContext;
use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};
use libmpv2::Mpv;
use std::ffi::{c_void, CString};
use std::sync::Arc;

/// 動画プレイヤー。
pub struct Player {
    mpv: &'static Mpv,
    render_context: RenderContext<'static>,

    gl: Arc<glow::Context>,
    fbo: glow::NativeFramebuffer,
    texture: glow::NativeTexture,
    /// 現在の FBO/テクスチャのサイズ。描画要求サイズと異なれば再生成する。
    size: (i32, i32),
}

impl Player {
    /// プレイヤーを初期化する。
    /// - `gl`: egui_glow と共有する glow::Context（FBO/テクスチャ作成に必要）
    /// - `gl_display`: mpv の `get_proc_address` に渡す glutin::Display
    /// - `on_update`: mpv が新フレームを生成したときに呼ばれるコールバック
    pub fn new<F>(
        gl: Arc<glow::Context>,
        gl_display: glutin::display::Display,
        verbose: bool,
        on_update: F,
    ) -> Result<Self>
    where
        F: Fn() + Send + 'static,
    {
        // mpv 初期化。YouTube URL の解決はアプリ側で行うので `ytdl` は無効化。
        let mpv = Mpv::with_initializer(|init| {
            init.set_property("vo", "libmpv")?;
            init.set_property("ytdl", false)?;
            // hwdec は mpv 既定のまま（明示設定しない）。
            if verbose {
                init.set_property("terminal", true)?;
                init.set_property("msg-level", "all=status")?;
            }
            Ok(())
        })
        .map_err(|e| anyhow!("mpv init failed: {e}"))?;

        // RenderContext が Mpv を借用するためリークして 'static にする。
        let mpv: &'static Mpv = Box::leak(Box::new(mpv));

        let mut render_context = mpv
            .create_render_context(vec![
                RenderParam::ApiType(RenderParamApiType::OpenGl),
                RenderParam::InitParams(OpenGLInitParams {
                    get_proc_address: get_proc_address,
                    ctx: gl_display,
                }),
            ])
            .map_err(|e| anyhow!("mpv render context failed: {e}"))?;
        render_context.set_update_callback(on_update);

        // 初期サイズは適当に小さな値で作っておく。最初の render() でリサイズされる。
        let (fbo, texture) = unsafe { create_fbo_texture(&gl, 16, 16)? };

        Ok(Self {
            mpv,
            render_context,
            gl,
            fbo,
            texture,
            size: (16, 16),
        })
    }

    /// 動画フレームを内部テクスチャに描画する。
    /// 要求サイズが現在の FBO サイズと異なる場合は再生成する。
    pub fn render(&mut self, width: i32, height: i32) {
        if width <= 0 || height <= 0 {
            return;
        }
        if self.size != (width, height) {
            unsafe {
                self.gl.delete_framebuffer(self.fbo);
                self.gl.delete_texture(self.texture);
                let (fbo, texture) = create_fbo_texture(&self.gl, width, height)
                    .expect("FBO/texture recreate failed");
                self.fbo = fbo;
                self.texture = texture;
            }
            self.size = (width, height);
        }

        // mpv の render に渡す FBO ID は i32。glow の NativeFramebuffer は NonZeroU32 内包なので
        // u32 に取り出してキャスト。flip=true は OpenGL の Y 軸方向を mpv に伝える。
        let fbo_id: u32 = native_fbo_id(self.fbo);
        if let Err(e) = self
            .render_context
            .render::<()>(fbo_id as i32, width, height, true)
        {
            eprintln!("mpv render error: {e}");
        }
    }

    /// 動画テクスチャを取得する（UI 層が合成に使う）。
    pub fn texture(&self) -> glow::NativeTexture {
        self.texture
    }

    // --- mpv プロパティアクセス（薄いラッパー）---

    pub fn paused(&self) -> bool {
        self.mpv.get_property("pause").unwrap_or(false)
    }
    pub fn set_paused(&self, paused: bool) {
        let _ = self.mpv.set_property("pause", paused);
    }
    pub fn time_pos(&self) -> f64 {
        self.mpv.get_property("time-pos").unwrap_or(0.0)
    }
    pub fn set_time_pos(&self, t: f64) {
        let _ = self.mpv.set_property("time-pos", t);
    }
    pub fn duration(&self) -> f64 {
        self.mpv.get_property("duration").unwrap_or(0.0)
    }
    /// シーク可能か（VOD/DVR あり = true、DVR なしライブ = false）。
    pub fn seekable(&self) -> bool {
        self.mpv.get_property("seekable").unwrap_or(false)
    }
    pub fn volume(&self) -> f64 {
        self.mpv.get_property("volume").unwrap_or(100.0)
    }
    pub fn set_volume(&self, v: f64) {
        let _ = self.mpv.set_property("volume", v);
    }
    pub fn muted(&self) -> bool {
        self.mpv.get_property("mute").unwrap_or(false)
    }
    pub fn set_muted(&self, m: bool) {
        let _ = self.mpv.set_property("mute", m);
    }
    pub fn media_title(&self) -> String {
        self.mpv
            .get_property::<String>("media-title")
            .unwrap_or_default()
    }
    pub fn seek_relative(&self, secs: f64) {
        let _ = self
            .mpv
            .command("seek", &[&secs.to_string(), "relative"]);
    }

    /// HW デコードの設定を動的に変更する。`"auto"` で HW 利用、`"no"` で SW 強制。
    /// GPU 使用率監視（gpu_usage）から、使用率が高い時に呼ばれる。
    pub fn set_hwdec(&self, mode: &str) {
        let _ = self.mpv.set_property("hwdec", mode);
    }

    /// 動画を読み込む（loadfile replace）。
    /// `audio_url` / `title` は loadfile のオプションとして `audio-file=` / `force-media-title=` を渡す。
    pub fn loadfile(
        &self,
        video_url: &str,
        audio_url: Option<&str>,
        title: Option<&str>,
    ) -> Result<()> {
        let mut options = Vec::new();
        if let Some(a) = audio_url {
            options.push(format!("audio-file={a}"));
        }
        if let Some(t) = title {
            options.push(format!("force-media-title={t}"));
        }
        if options.is_empty() {
            self.mpv
                .command("loadfile", &[video_url])
                .map_err(|e| anyhow!("loadfile failed: {e}"))
        } else {
            let opts = options.join(",");
            self.mpv
                .command("loadfile", &[video_url, "replace", "-1", &opts])
                .map_err(|e| anyhow!("loadfile failed: {e}"))
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        unsafe {
            self.gl.delete_framebuffer(self.fbo);
            self.gl.delete_texture(self.texture);
        }
    }
}

// ---------------------------------------------------------------------------
// 内部ヘルパー
// ---------------------------------------------------------------------------

/// mpv が GL 関数ポインタを解決するためのコールバック。
/// ctx には glutin の Display を渡しておき、ここで名前解決する。
fn get_proc_address(display: &glutin::display::Display, name: &str) -> *mut c_void {
    use glutin::display::GlDisplay;
    let cname = match CString::new(name) {
        Ok(c) => c,
        Err(_) => return std::ptr::null_mut(),
    };
    display.get_proc_address(cname.as_c_str()) as *mut c_void
}

/// glow の NativeFramebuffer から OpenGL の生 ID（u32）を取り出す。
fn native_fbo_id(fbo: glow::NativeFramebuffer) -> u32 {
    // glow::NativeFramebuffer は内部に NonZeroU32 を持つ。
    // safe な変換は無いので transmute（同サイズ・同レイアウト）。
    unsafe { std::mem::transmute::<glow::NativeFramebuffer, std::num::NonZeroU32>(fbo) }.get()
}

unsafe fn create_fbo_texture(
    gl: &glow::Context,
    width: i32,
    height: i32,
) -> Result<(glow::NativeFramebuffer, glow::NativeTexture)> {
    let texture = gl
        .create_texture()
        .map_err(|e| anyhow!("create texture failed: {e}"))?;
    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
    gl.tex_image_2d(
        glow::TEXTURE_2D,
        0,
        glow::RGBA8 as i32,
        width,
        height,
        0,
        glow::RGBA,
        glow::UNSIGNED_BYTE,
        None,
    );
    gl.tex_parameter_i32(
        glow::TEXTURE_2D,
        glow::TEXTURE_MIN_FILTER,
        glow::LINEAR as i32,
    );
    gl.tex_parameter_i32(
        glow::TEXTURE_2D,
        glow::TEXTURE_MAG_FILTER,
        glow::LINEAR as i32,
    );
    gl.tex_parameter_i32(
        glow::TEXTURE_2D,
        glow::TEXTURE_WRAP_S,
        glow::CLAMP_TO_EDGE as i32,
    );
    gl.tex_parameter_i32(
        glow::TEXTURE_2D,
        glow::TEXTURE_WRAP_T,
        glow::CLAMP_TO_EDGE as i32,
    );

    let fbo = gl
        .create_framebuffer()
        .map_err(|e| anyhow!("create framebuffer failed: {e}"))?;
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
    gl.framebuffer_texture_2d(
        glow::FRAMEBUFFER,
        glow::COLOR_ATTACHMENT0,
        glow::TEXTURE_2D,
        Some(texture),
        0,
    );
    let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
    gl.bind_texture(glow::TEXTURE_2D, None);

    if status != glow::FRAMEBUFFER_COMPLETE {
        bail!("framebuffer incomplete: 0x{:X}", status);
    }
    Ok((fbo, texture))
}
