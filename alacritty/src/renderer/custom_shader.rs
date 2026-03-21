use std::ffi::{CStr, CString};
use std::path::Path;
use std::{fmt, fs, ptr};

use alacritty_terminal::term::MAX_MULTI_CURSORS;
use log::{error, info, warn};

use crate::display::SizeInfo;
use crate::gl;
use crate::gl::types::{GLint, GLuint};

// Texture unit allocation (must not conflict with upstream alacritty):
// TEXTURE0 - glyph atlas (upstream alacritty)
// TEXTURE1 - scene FBO texture (custom shader pipeline)
// TEXTURE2 - ping-pong buffer 0 (multi-shader chains)
// TEXTURE3 - ping-pong buffer 1 (multi-shader chains)

/// Vertex shader source (compiled into binary).
const VERTEX_SHADER: &str = include_str!("../../res/glsl3/custom_shader.v.glsl");

/// Header prepended to user fragment shaders (ghostty-compatible uniforms).
///
/// Bridges OpenGL (bottom-left origin) to ghostty/Metal (top-left origin):
/// - fragCoord.y is flipped in the main() wrapper
/// - texture() is redefined to flip UV.y for correct sampling
/// - Cursor rects stay in top-left origin (matching ghostty convention)
// MAX_CURSORS must match MAX_MULTI_CURSORS in alacritty_terminal::term.
const FRAGMENT_HEADER: &str = "\
#version 330 core

uniform sampler2D iChannel0;
uniform vec2 iResolution;
// Note: iTime is f32 and loses sub-ms precision after ~4.5 hours of uptime.
// This matches ghostty/shadertoy behavior and is standard for shader uniforms.
uniform float iTime;
uniform float iTimeCursorChange;
uniform vec4 iCurrentCursor;
uniform vec4 iPreviousCursor;
uniform vec4 iCurrentCursorColor;
uniform float iCursorVisible;
uniform vec2 iCellSize;

#define MAX_CURSORS 64
uniform vec4 iCursors[MAX_CURSORS];
uniform vec4 iPreviousCursors[MAX_CURSORS];
uniform int iCursorTypes[MAX_CURSORS];
uniform int iCursorCount;
uniform int iPreviousCursorCount;
uniform float iTimeModeChange;
uniform vec2 iMousePosition;

out vec4 fragColor;

// Flip texture V for OpenGL->Metal coordinate bridge.
vec4 _tex_compat(sampler2D s, vec2 uv) {
    return textureLod(s, vec2(uv.x, 1.0 - uv.y), 0.0);
}
#define texture(s, uv) _tex_compat(s, uv)

";

/// Wrapper: flip fragCoord.y to top-left origin, undefine texture macro
/// so the real texture() is available for any post-mainImage code.
const FRAGMENT_MAIN_WRAPPER: &str = "\n#undef texture
void main() {
    mainImage(fragColor, vec2(gl_FragCoord.x, iResolution.y - gl_FragCoord.y));
}
";

/// Preprocess a user shader source file.
///
/// If the source starts with `#version` (after trimming whitespace/BOM), it is
/// used as-is (standalone shader). Otherwise, the ghostty-compatible header is
/// prepended and a `main()` wrapper calling `mainImage()` is appended.
///
/// Supports `#pragma include "filename.glsl"` directives, resolved relative to
/// the shader's directory. Includes are processed before header/wrapper logic.
fn preprocess_shader(source: &str, shader_dir: &Path) -> String {
    let expanded = expand_includes(source, shader_dir, 0);
    let trimmed = expanded.strip_prefix('\u{feff}').unwrap_or(&expanded);
    let trimmed = trimmed.trim_start();

    if trimmed.starts_with("#version") {
        expanded
    } else {
        format!("{FRAGMENT_HEADER}{expanded}{FRAGMENT_MAIN_WRAPPER}")
    }
}

/// Expand `#pragma include "filename.glsl"` directives in shader source.
/// Resolves paths relative to `base_dir`. Logs a warning and skips failed includes.
fn expand_includes(source: &str, base_dir: &Path, depth: u8) -> String {
    if depth > 16 {
        warn!("Shader include depth exceeded 16, skipping further expansion");
        return source.to_owned();
    }

    let mut result = String::with_capacity(source.len());
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("#pragma include") {
            let rest = rest.trim();
            if let Some(path_str) = rest.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                let include_path = base_dir.join(path_str);
                match fs::read_to_string(&include_path) {
                    Ok(contents) => {
                        // Recursively expand includes in the included file.
                        let expanded = expand_includes(
                            &contents,
                            include_path.parent().unwrap_or(base_dir),
                            depth + 1,
                        );
                        result.push_str(&expanded);
                        result.push('\n');
                        continue;
                    },
                    Err(e) => {
                        warn!("Failed to include shader {:?}: {e}", include_path);
                    },
                }
            }
        }
        result.push_str(line);
        result.push('\n');
    }
    result
}

/// Compile a shader from source and return its GL handle.
fn compile_shader(source: &str, shader_type: GLuint) -> Result<GLuint, String> {
    let c_source = CString::new(source).map_err(|e| e.to_string())?;
    unsafe {
        let shader = gl::CreateShader(shader_type);
        gl::ShaderSource(shader, 1, &c_source.as_ptr(), ptr::null());
        gl::CompileShader(shader);

        let mut success = 0;
        gl::GetShaderiv(shader, gl::COMPILE_STATUS, &mut success);
        if success == 0 {
            let mut len = 0;
            gl::GetShaderiv(shader, gl::INFO_LOG_LENGTH, &mut len);
            let mut log_buf = vec![0u8; len as usize];
            gl::GetShaderInfoLog(shader, len, ptr::null_mut(), log_buf.as_mut_ptr() as *mut _);
            log_buf.truncate(log_buf.iter().position(|&b| b == 0).unwrap_or(log_buf.len()));
            let log = String::from_utf8_lossy(&log_buf).to_string();
            gl::DeleteShader(shader);
            return Err(log);
        }
        Ok(shader)
    }
}

/// Link a vertex and fragment shader into a program.
///
/// The fragment shader is deleted after linking. The vertex shader is NOT
/// deleted since it is shared across multiple programs.
fn link_program(vertex: GLuint, fragment: GLuint) -> Result<GLuint, String> {
    unsafe {
        let program = gl::CreateProgram();
        gl::AttachShader(program, vertex);
        gl::AttachShader(program, fragment);
        gl::LinkProgram(program);

        // Only delete the fragment shader (vertex is shared).
        gl::DeleteShader(fragment);

        let mut success = 0;
        gl::GetProgramiv(program, gl::LINK_STATUS, &mut success);
        if success == 0 {
            let mut len = 0;
            gl::GetProgramiv(program, gl::INFO_LOG_LENGTH, &mut len);
            let mut log_buf = vec![0u8; len as usize];
            gl::GetProgramInfoLog(program, len, ptr::null_mut(), log_buf.as_mut_ptr() as *mut _);
            log_buf.truncate(log_buf.iter().position(|&b| b == 0).unwrap_or(log_buf.len()));
            let log = String::from_utf8_lossy(&log_buf).to_string();
            gl::DeleteProgram(program);
            return Err(log);
        }
        Ok(program)
    }
}

/// A single compiled custom shader program.
struct CustomShaderProgram {
    program: GLuint,

    // Uniform locations.
    u_channel0: GLint,
    u_resolution: GLint,
    u_time: GLint,
    u_time_cursor_change: GLint,
    u_current_cursor: GLint,
    u_previous_cursor: GLint,
    u_current_cursor_color: GLint,
    u_cursor_visible: GLint,
    u_cell_size: GLint,
    u_cursors: GLint,
    u_prev_cursors: GLint,
    u_cursor_types: GLint,
    u_cursor_count: GLint,
    u_prev_cursor_count: GLint,
    u_time_mode_change: GLint,
    u_mouse_pos: GLint,
}

impl CustomShaderProgram {
    /// Compile a fragment shader and link it with the shared vertex shader.
    fn new(shader_path: &Path, vertex_shader: GLuint) -> Option<Self> {
        let source = match fs::read_to_string(shader_path) {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to read custom shader {:?}: {e}", shader_path);
                return None;
            },
        };

        let shader_dir = shader_path.parent().unwrap_or(Path::new("."));
        let fragment_source = preprocess_shader(&source, shader_dir);

        unsafe {
            let fragment = match compile_shader(&fragment_source, gl::FRAGMENT_SHADER) {
                Ok(s) => s,
                Err(log) => {
                    error!("Custom shader {:?} compilation failed:\n{log}", shader_path);
                    return None;
                },
            };

            let program = match link_program(vertex_shader, fragment) {
                Ok(p) => p,
                Err(log) => {
                    error!("Custom shader {:?} link failed:\n{log}", shader_path);
                    return None;
                },
            };

            let get_loc = |name: &CStr| -> GLint { gl::GetUniformLocation(program, name.as_ptr()) };

            Some(Self {
                program,
                u_channel0: get_loc(c"iChannel0"),
                u_resolution: get_loc(c"iResolution"),
                u_time: get_loc(c"iTime"),
                u_time_cursor_change: get_loc(c"iTimeCursorChange"),
                u_current_cursor: get_loc(c"iCurrentCursor"),
                u_previous_cursor: get_loc(c"iPreviousCursor"),
                u_current_cursor_color: get_loc(c"iCurrentCursorColor"),
                u_cursor_visible: get_loc(c"iCursorVisible"),
                u_cell_size: get_loc(c"iCellSize"),
                u_cursors: get_loc(c"iCursors"),
                u_prev_cursors: get_loc(c"iPreviousCursors"),
                u_cursor_types: get_loc(c"iCursorTypes"),
                u_cursor_count: get_loc(c"iCursorCount"),
                u_prev_cursor_count: get_loc(c"iPreviousCursorCount"),
                u_time_mode_change: get_loc(c"iTimeModeChange"),
                u_mouse_pos: get_loc(c"iMousePosition"),
            })
        }
    }

    /// Bind this program and set uniforms, then draw the fullscreen triangle.
    ///
    /// `input_texture` is the GL texture to bind as iChannel0.
    /// `input_unit` is the texture unit (e.g., 1 for TEXTURE1).
    fn draw(
        &self,
        vao: GLuint,
        input_texture: GLuint,
        input_unit: GLint,
        uniforms: &ShaderUniforms,
    ) {
        unsafe {
            gl::UseProgram(self.program);

            gl::ActiveTexture(gl::TEXTURE0 + input_unit as GLuint);
            gl::BindTexture(gl::TEXTURE_2D, input_texture);
            gl::Uniform1i(self.u_channel0, input_unit);

            gl::Uniform2f(self.u_resolution, uniforms.resolution[0], uniforms.resolution[1]);
            gl::Uniform1f(self.u_time, uniforms.time);
            gl::Uniform1f(self.u_time_cursor_change, uniforms.time_cursor_change);
            gl::Uniform4f(
                self.u_current_cursor,
                uniforms.current_cursor[0],
                uniforms.current_cursor[1],
                uniforms.current_cursor[2],
                uniforms.current_cursor[3],
            );
            gl::Uniform4f(
                self.u_previous_cursor,
                uniforms.previous_cursor[0],
                uniforms.previous_cursor[1],
                uniforms.previous_cursor[2],
                uniforms.previous_cursor[3],
            );
            gl::Uniform4f(
                self.u_current_cursor_color,
                uniforms.current_cursor_color[0],
                uniforms.current_cursor_color[1],
                uniforms.current_cursor_color[2],
                uniforms.current_cursor_color[3],
            );
            gl::Uniform1f(self.u_cursor_visible, uniforms.cursor_visible);
            gl::Uniform2f(self.u_cell_size, uniforms.cell_size[0], uniforms.cell_size[1]);
            gl::Uniform1i(self.u_cursor_count, uniforms.cursor_count);
            gl::Uniform1i(self.u_prev_cursor_count, uniforms.prev_cursor_count);
            gl::Uniform1f(self.u_time_mode_change, uniforms.time_mode_change);
            gl::Uniform2f(self.u_mouse_pos, uniforms.mouse_pos[0], uniforms.mouse_pos[1]);
            let upload_count =
                uniforms.cursor_count.max(uniforms.prev_cursor_count).min(MAX_MULTI_CURSORS as i32);
            if upload_count > 0 {
                gl::Uniform4fv(
                    self.u_cursors,
                    upload_count,
                    uniforms.cursors.as_ptr() as *const f32,
                );
                gl::Uniform4fv(
                    self.u_prev_cursors,
                    upload_count,
                    uniforms.prev_cursors.as_ptr() as *const f32,
                );
                gl::Uniform1iv(self.u_cursor_types, upload_count, uniforms.cursor_types.as_ptr());
            }
            gl::BindVertexArray(vao);
            gl::DrawArrays(gl::TRIANGLES, 0, 3);
            gl::BindVertexArray(0);
        }
    }
}

impl Drop for CustomShaderProgram {
    fn drop(&mut self) {
        unsafe {
            if self.program != 0 {
                gl::DeleteProgram(self.program);
            }
        }
    }
}

/// Uniform values passed to custom shaders each frame.
#[derive(Debug)]
pub struct ShaderUniforms {
    pub resolution: [f32; 2],
    pub time: f32,
    pub time_cursor_change: f32,
    pub current_cursor: [f32; 4],
    pub previous_cursor: [f32; 4],
    pub current_cursor_color: [f32; 4],
    pub cursor_visible: f32,
    pub cell_size: [f32; 2],
    pub cursors: [[f32; 4]; MAX_MULTI_CURSORS],
    pub prev_cursors: [[f32; 4]; MAX_MULTI_CURSORS],
    pub cursor_types: [i32; MAX_MULTI_CURSORS],
    pub cursor_count: i32,
    pub prev_cursor_count: i32,
    pub time_mode_change: f32,
    pub mouse_pos: [f32; 2],
}

/// Manages a chain of custom post-process shaders with shared GL resources.
///
/// Owns the scene FBO, shared VAO, shared vertex shader, ping-pong FBOs,
/// and a list of compiled shader programs.
pub struct CustomShaderPipeline {
    programs: Vec<CustomShaderProgram>,
    vao: GLuint,
    vertex_shader: GLuint,

    // Scene FBO (lazily allocated).
    fbo: GLuint,
    fbo_texture: GLuint,
    fbo_width: i32,
    fbo_height: i32,

    // Ping-pong FBOs + textures for multi-shader chaining.
    // Each FBO has its own attached texture, avoiding per-frame re-attachment.
    // Only allocated when programs.len() >= 2.
    ping_fbos: [GLuint; 2],
    ping_textures: [GLuint; 2],
}

impl fmt::Debug for CustomShaderPipeline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CustomShaderPipeline").field("shader_count", &self.programs.len()).finish()
    }
}

impl CustomShaderPipeline {
    /// Create a new pipeline from a list of shader paths.
    ///
    /// Compiles the shared vertex shader once, then compiles each fragment
    /// shader. If any shader fails, the entire pipeline creation fails.
    pub fn new(shader_paths: &[impl AsRef<Path>]) -> Option<Self> {
        if shader_paths.is_empty() {
            return None;
        }

        let vertex_shader = match compile_shader(VERTEX_SHADER, gl::VERTEX_SHADER) {
            Ok(s) => s,
            Err(log) => {
                error!("Custom shader vertex shader compilation failed:\n{log}");
                return None;
            },
        };

        let total = shader_paths.len();
        let mut programs = Vec::with_capacity(total);

        for (i, path) in shader_paths.iter().enumerate() {
            match CustomShaderProgram::new(path.as_ref(), vertex_shader) {
                Some(program) => programs.push(program),
                None => {
                    error!(
                        "Custom shader {}/{} {:?} failed, disabling shader chain",
                        i + 1,
                        total,
                        path.as_ref()
                    );
                    // Clean up already-compiled programs.
                    drop(programs);
                    unsafe {
                        gl::DeleteShader(vertex_shader);
                    }
                    return None;
                },
            }
        }

        let mut vao = 0;
        unsafe {
            gl::GenVertexArrays(1, &mut vao);
        }

        info!("Custom shader pipeline loaded with {} shader(s)", programs.len());

        Some(Self {
            programs,
            vao,
            vertex_shader,
            fbo: 0,
            fbo_texture: 0,
            fbo_width: 0,
            fbo_height: 0,
            ping_fbos: [0; 2],
            ping_textures: [0; 2],
        })
    }

    // -- Scene FBO management --

    /// Ensure scene FBO exists and matches the given dimensions.
    pub fn ensure_fbo(&mut self, width: i32, height: i32) -> bool {
        if self.fbo != 0 && self.fbo_width == width && self.fbo_height == height {
            return true;
        }

        self.delete_fbo();

        unsafe {
            // Scene texture on TEXTURE1.
            gl::ActiveTexture(gl::TEXTURE1);
            gl::GenTextures(1, &mut self.fbo_texture);
            gl::BindTexture(gl::TEXTURE_2D, self.fbo_texture);
            gl::TexImage2D(
                gl::TEXTURE_2D,
                0,
                gl::RGBA8 as i32,
                width,
                height,
                0,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                ptr::null(),
            );
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::NEAREST as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::NEAREST as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE as i32);
            gl::BindTexture(gl::TEXTURE_2D, 0);
            gl::ActiveTexture(gl::TEXTURE0);

            gl::GenFramebuffers(1, &mut self.fbo);
            gl::BindFramebuffer(gl::FRAMEBUFFER, self.fbo);
            gl::FramebufferTexture2D(
                gl::FRAMEBUFFER,
                gl::COLOR_ATTACHMENT0,
                gl::TEXTURE_2D,
                self.fbo_texture,
                0,
            );

            let status = gl::CheckFramebufferStatus(gl::FRAMEBUFFER);
            gl::BindFramebuffer(gl::FRAMEBUFFER, 0);

            if status != gl::FRAMEBUFFER_COMPLETE {
                error!("Custom shader scene FBO incomplete: {status:#x}");
                self.delete_fbo();
                return false;
            }

            // Allocate ping-pong textures + FBO if we have 2+ shaders.
            if self.programs.len() >= 2 && !self.ensure_ping_pong(width, height) {
                self.delete_fbo();
                return false;
            }
        }

        self.fbo_width = width;
        self.fbo_height = height;
        true
    }

    /// Allocate two ping-pong FBOs with pre-attached textures for chaining.
    ///
    /// Using separate FBOs avoids per-frame `glFramebufferTexture2D` re-attachment,
    /// which can cause pipeline stalls on macOS Metal.
    fn ensure_ping_pong(&mut self, width: i32, height: i32) -> bool {
        self.delete_ping_pong();

        unsafe {
            gl::GenTextures(2, self.ping_textures.as_mut_ptr());
            gl::GenFramebuffers(2, self.ping_fbos.as_mut_ptr());

            for (i, &tex_unit) in [gl::TEXTURE2, gl::TEXTURE3].iter().enumerate() {
                gl::ActiveTexture(tex_unit);
                gl::BindTexture(gl::TEXTURE_2D, self.ping_textures[i]);
                gl::TexImage2D(
                    gl::TEXTURE_2D,
                    0,
                    gl::RGBA8 as i32,
                    width,
                    height,
                    0,
                    gl::RGBA,
                    gl::UNSIGNED_BYTE,
                    ptr::null(),
                );
                gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::NEAREST as i32);
                gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::NEAREST as i32);
                gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE as i32);
                gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE as i32);
                gl::BindTexture(gl::TEXTURE_2D, 0);

                // Attach texture to its dedicated FBO.
                gl::BindFramebuffer(gl::FRAMEBUFFER, self.ping_fbos[i]);
                gl::FramebufferTexture2D(
                    gl::FRAMEBUFFER,
                    gl::COLOR_ATTACHMENT0,
                    gl::TEXTURE_2D,
                    self.ping_textures[i],
                    0,
                );
                let status = gl::CheckFramebufferStatus(gl::FRAMEBUFFER);
                if status != gl::FRAMEBUFFER_COMPLETE {
                    error!("Custom shader ping-pong FBO {} incomplete: {status:#x}", i);
                    gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
                    self.delete_ping_pong();
                    return false;
                }
            }
            gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
            gl::ActiveTexture(gl::TEXTURE0);
        }

        true
    }

    /// Bind the scene FBO as the render target.
    pub fn bind_fbo(&self) {
        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER, self.fbo);
        }
    }

    /// Unbind the FBO (restore default framebuffer).
    pub fn unbind_fbo(&self) {
        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
        }
    }

    /// Whether a scene FBO is currently allocated.
    pub fn has_fbo(&self) -> bool {
        self.fbo != 0
    }

    /// Delete scene FBO, texture, and ping-pong resources.
    pub fn delete_fbo(&mut self) {
        unsafe {
            if self.fbo != 0 {
                gl::DeleteFramebuffers(1, &self.fbo);
                self.fbo = 0;
            }
            if self.fbo_texture != 0 {
                gl::DeleteTextures(1, &self.fbo_texture);
                self.fbo_texture = 0;
            }
        }
        self.fbo_width = 0;
        self.fbo_height = 0;
        self.delete_ping_pong();
    }

    fn delete_ping_pong(&mut self) {
        unsafe {
            for fbo in &mut self.ping_fbos {
                if *fbo != 0 {
                    gl::DeleteFramebuffers(1, fbo);
                    *fbo = 0;
                }
            }
            for tex in &mut self.ping_textures {
                if *tex != 0 {
                    gl::DeleteTextures(1, tex);
                    *tex = 0;
                }
            }
        }
    }

    /// Run the post-process shader chain.
    ///
    /// For a single shader: reads scene FBO texture, writes to screen.
    /// For multiple shaders: ping-pong chain, last shader writes to screen.
    pub fn draw(&self, size_info: &SizeInfo, uniforms: &ShaderUniforms) {
        let width = size_info.width() as i32;
        let height = size_info.height() as i32;

        unsafe {
            gl::Viewport(0, 0, width, height);
            gl::Disable(gl::BLEND);

            if self.programs.len() == 1 {
                // Single shader: read scene FBO (TEXTURE1), write to screen.
                self.programs[0].draw(self.vao, self.fbo_texture, 1, uniforms);
            } else {
                // Multi-shader ping-pong chain using pre-attached FBOs.
                // write_idx tracks which ping FBO to render into next.
                let mut write_idx: usize = 0;

                for (i, program) in self.programs.iter().enumerate() {
                    let is_last = i == self.programs.len() - 1;

                    // Determine input texture and unit.
                    let (input_texture, input_unit) = if i == 0 {
                        // First shader reads from scene FBO (TEXTURE1).
                        (self.fbo_texture, 1)
                    } else {
                        // Subsequent shaders read from the texture we just wrote to.
                        let read_idx = 1 - write_idx;
                        let unit = 2 + read_idx as i32; // TEXTURE2 or TEXTURE3
                        (self.ping_textures[read_idx], unit)
                    };

                    if is_last {
                        // Last shader writes to default framebuffer (screen).
                        gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
                    } else {
                        // Intermediate shaders write to the current ping FBO.
                        gl::BindFramebuffer(gl::FRAMEBUFFER, self.ping_fbos[write_idx]);
                    }

                    program.draw(self.vao, input_texture, input_unit, uniforms);

                    if !is_last {
                        // Swap: next shader writes to the other FBO.
                        write_idx = 1 - write_idx;
                    }
                }
            }

            // Clean up GL state.
            gl::ActiveTexture(gl::TEXTURE1);
            gl::BindTexture(gl::TEXTURE_2D, 0);
            if self.programs.len() >= 2 {
                gl::ActiveTexture(gl::TEXTURE2);
                gl::BindTexture(gl::TEXTURE_2D, 0);
                gl::ActiveTexture(gl::TEXTURE3);
                gl::BindTexture(gl::TEXTURE_2D, 0);
            }
            gl::ActiveTexture(gl::TEXTURE0);
            gl::UseProgram(0);
            gl::Enable(gl::BLEND);
        }
    }
}

impl Drop for CustomShaderPipeline {
    fn drop(&mut self) {
        self.delete_fbo();
        unsafe {
            if self.vao != 0 {
                gl::DeleteVertexArrays(1, &self.vao);
            }
            if self.vertex_shader != 0 {
                gl::DeleteShader(self.vertex_shader);
            }
        }
    }
}
