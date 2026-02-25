use std::sync::Arc;

use anyhow::Result;
use glow::{Context, HasContext};

pub struct OpenGLVideoRenderer {
    gl: Arc<Context>,
    program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    y_texture: glow::Texture,
    uv_texture: glow::Texture,
}

impl OpenGLVideoRenderer {
    pub fn new(gl: Arc<Context>) -> Result<Self> {
        unsafe {
            let vertex_shader = gl
                .create_shader(glow::VERTEX_SHADER)
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            gl.shader_source(vertex_shader, VERTEX_SHADER);
            gl.compile_shader(vertex_shader);
            if !gl.get_shader_compile_status(vertex_shader) {
                let log = gl.get_shader_info_log(vertex_shader);
                gl.delete_shader(vertex_shader);
                anyhow::bail!("VS: {}", log);
            }

            let fragment_shader = gl
                .create_shader(glow::FRAGMENT_SHADER)
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            gl.shader_source(fragment_shader, FRAGMENT_SHADER);
            gl.compile_shader(fragment_shader);
            if !gl.get_shader_compile_status(fragment_shader) {
                let log = gl.get_shader_info_log(fragment_shader);
                gl.delete_shader(fragment_shader);
                anyhow::bail!("FS: {}", log);
            }

            let program = gl.create_program().map_err(|e| anyhow::anyhow!("{}", e))?;
            gl.attach_shader(program, vertex_shader);
            gl.attach_shader(program, fragment_shader);
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                let log = gl.get_program_info_log(program);
                anyhow::bail!("Link: {}", log);
            }

            gl.delete_shader(vertex_shader);
            gl.delete_shader(fragment_shader);

            let vao = gl
                .create_vertex_array()
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            gl.bind_vertex_array(Some(vao));

            let vbo = gl.create_buffer().map_err(|e| anyhow::anyhow!("{}", e))?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            let _ = gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                unsafe {
                    std::slice::from_raw_parts(
                        VERTICES.as_ptr() as *const u8,
                        VERTICES.len() * std::mem::size_of::<f32>(),
                    )
                },
                glow::STATIC_DRAW,
            );

            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 16, 0);

            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 16, 8);

            gl.bind_vertex_array(None);

            let y_texture = gl.create_texture().map_err(|e| anyhow::anyhow!("{}", e))?;
            let uv_texture = gl.create_texture().map_err(|e| anyhow::anyhow!("{}", e))?;

            Ok(Self {
                gl,
                program,
                vao,
                vbo,
                y_texture,
                uv_texture,
            })
        }
    }

    pub fn upload_frame(&self, width: u32, height: u32, y_data: &[u8], uv_data: &[u8]) {
        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(self.y_texture));
            self.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::R8 as i32,
                width as i32,
                height as i32,
                0,
                glow::RED,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(y_data)),
            );
            self.gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            self.gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );

            let uv_width = width / 2;
            let uv_height = height / 2;
            self.gl
                .bind_texture(glow::TEXTURE_2D, Some(self.uv_texture));
            self.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RG8 as i32,
                uv_width as i32,
                uv_height as i32,
                0,
                glow::RG,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(uv_data)),
            );
            self.gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            self.gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
        }
    }

    pub fn render(&self, viewport: [f32; 4], screen_height: f32) {
        unsafe {
            // Convert from egui coordinates (top-left origin) to OpenGL coordinates (bottom-left origin)
            let x = viewport[0] as i32;
            let y = (screen_height - viewport[1] - viewport[3]) as i32;
            let w = viewport[2] as i32;
            let h = viewport[3] as i32;

            self.gl.viewport(x, y, w, h);
            self.gl.use_program(Some(self.program));
            self.gl.bind_vertex_array(Some(self.vao));
            self.gl.draw_arrays(glow::TRIANGLES, 0, 6);
            self.gl.bind_vertex_array(None);
        }
    }
}

impl Drop for OpenGLVideoRenderer {
    fn drop(&mut self) {
        unsafe {
            self.gl.delete_program(self.program);
            self.gl.delete_vertex_array(self.vao);
            self.gl.delete_buffer(self.vbo);
            self.gl.delete_texture(self.y_texture);
            self.gl.delete_texture(self.uv_texture);
        }
    }
}

const VERTICES: [f32; 24] = [
    -1.0, 1.0, 0.0, 0.0, 1.0, -1.0, 1.0, 1.0, -1.0, -1.0, 0.0, 1.0, -1.0, 1.0, 0.0, 0.0, 1.0, 1.0,
    1.0, 0.0, 1.0, -1.0, 1.0, 1.0,
];

const VERTEX_SHADER: &str = r#"
#version 330 core
layout(location = 0) in vec2 a_pos;
layout(location = 1) in vec2 a_uv;
out vec2 v_uv;
void main() {
    gl_Position = vec4(a_pos, 0.0, 1.0);
    v_uv = a_uv;
}
"#;

const FRAGMENT_SHADER: &str = r#"
#version 330 core
in vec2 v_uv;
out vec4 FragColor;
void main() {
    // Test pattern: green
    FragColor = vec4(0.0, 1.0, 0.0, 1.0);
}
"#;
