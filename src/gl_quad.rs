//! テクスチャを画面いっぱいに描画するためのシンプルな OpenGL クワッドレンダラ。
//!
//! `Player` が動画フレームを内部テクスチャに描画する。`FullscreenQuad` は
//! そのテクスチャをデフォルトフレームバッファ（ウィンドウ）にフルスクリーンで
//! 描画する。UI（egui）はこの上に重ねて合成される。

use anyhow::{anyhow, bail, Result};
use glow::HasContext;
use std::sync::Arc;

/// フルスクリーンクワッドのレンダラ。
pub struct FullscreenQuad {
    gl: Arc<glow::Context>,
    program: glow::NativeProgram,
    vao: glow::NativeVertexArray,
    sampler_loc: Option<glow::UniformLocation>,
}

impl FullscreenQuad {
    pub fn new(gl: Arc<glow::Context>) -> Result<Self> {
        let program = unsafe { build_program(&gl)? };
        let vao = unsafe {
            gl.create_vertex_array()
                .map_err(|e| anyhow!("create VAO failed: {e}"))?
        };
        let sampler_loc = unsafe { gl.get_uniform_location(program, "u_tex") };

        Ok(Self {
            gl,
            program,
            vao,
            sampler_loc,
        })
    }

    /// 指定テクスチャをデフォルトフレームバッファの指定領域に描画する。
    /// `viewport`: (x, y, width, height) — OpenGL 規約で左下原点。
    pub fn draw(&self, texture: glow::NativeTexture, viewport: (i32, i32, i32, i32)) {
        let gl = &self.gl;
        unsafe {
            // 既定の FBO（ウィンドウ）に描画。
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.viewport(viewport.0, viewport.1, viewport.2, viewport.3);

            // egui が後で描画するため、深度テスト・ブレンド等は外しておく。
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.disable(glow::CULL_FACE);

            gl.use_program(Some(self.program));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            if let Some(loc) = &self.sampler_loc {
                gl.uniform_1_i32(Some(loc), 0);
            }

            gl.bind_vertex_array(Some(self.vao));
            // 頂点バッファ無しでシェーダ内で gl_VertexID から座標生成する。
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
        }
    }
}

impl Drop for FullscreenQuad {
    fn drop(&mut self) {
        unsafe {
            self.gl.delete_program(self.program);
            self.gl.delete_vertex_array(self.vao);
        }
    }
}

// ---------------------------------------------------------------------------
// シェーダ
// ---------------------------------------------------------------------------

/// 大きな三角形 1 枚で画面を覆い、UV を全画面に展開するパターン。
/// 頂点バッファ不要（gl_VertexID から座標を生成）。
const VS: &str = r#"#version 330 core
out vec2 v_uv;
void main() {
    // 3 頂点で画面外まで覆う大三角形（gl_VertexID = 0,1,2）。
    //   id=0: (-1,-1) uv=(0,0)
    //   id=1: ( 3,-1) uv=(2,0)
    //   id=2: (-1, 3) uv=(0,2)
    // クリップで画面の四角だけが残る。
    vec2 pos = vec2((gl_VertexID == 1) ? 3.0 : -1.0,
                    (gl_VertexID == 2) ? 3.0 : -1.0);
    v_uv = (pos + 1.0) * 0.5;
    gl_Position = vec4(pos, 0.0, 1.0);
}
"#;

const FS: &str = r#"#version 330 core
in vec2 v_uv;
out vec4 frag;
uniform sampler2D u_tex;
void main() {
    frag = texture(u_tex, v_uv);
}
"#;

unsafe fn build_program(gl: &glow::Context) -> Result<glow::NativeProgram> {
    let vs = compile_shader(gl, glow::VERTEX_SHADER, VS)?;
    let fs = compile_shader(gl, glow::FRAGMENT_SHADER, FS)?;
    let program = gl
        .create_program()
        .map_err(|e| anyhow!("create program failed: {e}"))?;
    gl.attach_shader(program, vs);
    gl.attach_shader(program, fs);
    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        let log = gl.get_program_info_log(program);
        bail!("program link failed: {log}");
    }
    gl.detach_shader(program, vs);
    gl.detach_shader(program, fs);
    gl.delete_shader(vs);
    gl.delete_shader(fs);
    Ok(program)
}

unsafe fn compile_shader(
    gl: &glow::Context,
    kind: u32,
    src: &str,
) -> Result<glow::NativeShader> {
    let shader = gl
        .create_shader(kind)
        .map_err(|e| anyhow!("create shader failed: {e}"))?;
    gl.shader_source(shader, src);
    gl.compile_shader(shader);
    if !gl.get_shader_compile_status(shader) {
        let log = gl.get_shader_info_log(shader);
        bail!("shader compile failed: {log}");
    }
    Ok(shader)
}
