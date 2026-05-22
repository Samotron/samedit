//! OpenGL renderer implementation using `glow` bindings.

use crate::RenderFrame;
use crate::geometry::Vertex;
use crate::text_mesh::{TexturedVertex, text_mesh_from_runs};
use glow::HasContext;
use std::rc::Rc;
use thiserror::Error;

/// OpenGL renderer error.
#[derive(Debug, Error)]
pub enum RendererError {
    #[error("OpenGL creation error: {0}")]
    GlError(String),
    #[error("Shader compilation error: {0}")]
    ShaderError(String),
}

/// The GL renderer responsible for executing draw calls for a RenderFrame.
pub struct GlRenderer {
    gl: Rc<glow::Context>,
    rect_program: glow::Program,
    rect_vao: glow::VertexArray,
    rect_vbo: glow::Buffer,
    rect_ebo: glow::Buffer,
    rect_proj_loc: glow::UniformLocation,

    text_program: glow::Program,
    text_vao: glow::VertexArray,
    text_vbo: glow::Buffer,
    text_ebo: glow::Buffer,
    text_proj_loc: glow::UniformLocation,
    text_tex_loc: glow::UniformLocation,

    atlas_texture: glow::Texture,
    atlas_width: i32,
    atlas_height: i32,
}

impl GlRenderer {
    /// Initialize a new GlRenderer with a given context and texture dimensions.
    ///
    /// # Safety
    /// Assumes the provided OpenGL context is current on the calling thread.
    pub unsafe fn new(
        gl: Rc<glow::Context>,
        atlas_width: i32,
        atlas_height: i32,
    ) -> Result<Self, RendererError> {
        // SAFETY: GlRenderer initialization relies on the caller providing a valid current context.
        unsafe {
            // Compile Shaders
            let rect_program = create_program(&gl, RECT_VERT_SRC, RECT_FRAG_SRC)
                .map_err(RendererError::ShaderError)?;

            let text_program = create_program(&gl, TEXT_VERT_SRC, TEXT_FRAG_SRC)
                .map_err(RendererError::ShaderError)?;

            // Locate Uniforms
            let rect_proj_loc = gl
                .get_uniform_location(rect_program, "u_projection")
                .ok_or_else(|| {
                    RendererError::ShaderError(
                        "u_projection location not found in rect shader".into(),
                    )
                })?;

            let text_proj_loc = gl
                .get_uniform_location(text_program, "u_projection")
                .ok_or_else(|| {
                    RendererError::ShaderError(
                        "u_projection location not found in text shader".into(),
                    )
                })?;
            let text_tex_loc = gl
                .get_uniform_location(text_program, "u_texture")
                .ok_or_else(|| {
                    RendererError::ShaderError("u_texture location not found in text shader".into())
                })?;

            // VAO / VBO / EBO Setup
            let rect_vao = gl.create_vertex_array().map_err(RendererError::GlError)?;
            let rect_vbo = gl.create_buffer().map_err(RendererError::GlError)?;
            let rect_ebo = gl.create_buffer().map_err(RendererError::GlError)?;

            gl.bind_vertex_array(Some(rect_vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(rect_vbo));
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(rect_ebo));

            // Vertex Attributes: position, tex_coord, color
            let stride = std::mem::size_of::<Vertex>() as i32;

            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, stride, 0);

            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, stride, 8); // offset of tex_coord (2 * f32)

            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_f32(2, 4, glow::FLOAT, false, stride, 16); // offset of color (4 * f32)

            // Text buffers setup
            let text_vao = gl.create_vertex_array().map_err(RendererError::GlError)?;
            let text_vbo = gl.create_buffer().map_err(RendererError::GlError)?;
            let text_ebo = gl.create_buffer().map_err(RendererError::GlError)?;

            gl.bind_vertex_array(Some(text_vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(text_vbo));
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(text_ebo));

            let text_stride = std::mem::size_of::<TexturedVertex>() as i32;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, text_stride, 0);

            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, text_stride, 8);

            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_f32(2, 4, glow::FLOAT, false, text_stride, 16);

            // Texture Setup
            let atlas_texture = gl.create_texture().map_err(RendererError::GlError)?;
            gl.bind_texture(glow::TEXTURE_2D, Some(atlas_texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                atlas_width,
                atlas_height,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::NEAREST as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::NEAREST as i32,
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

            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);

            Ok(Self {
                gl,
                rect_program,
                rect_vao,
                rect_vbo,
                rect_ebo,
                rect_proj_loc,
                text_program,
                text_vao,
                text_vbo,
                text_ebo,
                text_proj_loc,
                text_tex_loc,
                atlas_texture,
                atlas_width,
                atlas_height,
            })
        }
    }

    /// Retrieve raw glow::Context reference.
    pub fn gl(&self) -> &Rc<glow::Context> {
        &self.gl
    }

    /// Draw the current frame.
    ///
    /// # Safety
    /// Assumes the OpenGL context is active and viewport size is valid.
    pub unsafe fn draw_frame(
        &mut self,
        frame: &RenderFrame,
        viewport_width: u32,
        viewport_height: u32,
    ) -> Result<(), RendererError> {
        // SAFETY: Invocation of glDrawElements and buffer binding operates on valid GL objects allocated in `new()`.
        unsafe {
            // Upload glyph texture additions
            if !frame.glyph_uploads.is_empty() {
                self.gl
                    .bind_texture(glow::TEXTURE_2D, Some(self.atlas_texture));
                for upload in &frame.glyph_uploads {
                    self.gl.tex_sub_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        upload.rect.x,
                        upload.rect.y,
                        upload.rect.width,
                        upload.rect.height,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(Some(&upload.pixels)),
                    );
                }
                self.gl.bind_texture(glow::TEXTURE_2D, None);
            }

            // Set up viewport and clearing
            self.gl
                .viewport(0, 0, viewport_width as i32, viewport_height as i32);
            let clear_color = frame.clear_color.to_array();
            self.gl.clear_color(
                clear_color[0],
                clear_color[1],
                clear_color[2],
                clear_color[3],
            );
            self.gl.clear(glow::COLOR_BUFFER_BIT);

            // Blending is crucial for transparent rectangles and anti-aliased text
            self.gl.enable(glow::BLEND);
            self.gl
                .blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);

            // Projection Matrix
            let projection = ortho_projection(viewport_width as f32, viewport_height as f32);

            // Draw Solid Rectangles
            if !frame.rect_mesh.is_empty() {
                self.gl.use_program(Some(self.rect_program));
                self.gl
                    .uniform_matrix_4_f32_slice(Some(&self.rect_proj_loc), false, &projection);

                self.gl.bind_vertex_array(Some(self.rect_vao));
                self.gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.rect_vbo));

                let vertices_bytes = std::slice::from_raw_parts(
                    frame.rect_mesh.vertices.as_ptr() as *const u8,
                    frame.rect_mesh.vertices.len() * std::mem::size_of::<Vertex>(),
                );
                self.gl.buffer_data_u8_slice(
                    glow::ARRAY_BUFFER,
                    vertices_bytes,
                    glow::DYNAMIC_DRAW,
                );

                self.gl
                    .bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.rect_ebo));
                let indices_bytes = std::slice::from_raw_parts(
                    frame.rect_mesh.indices.as_ptr() as *const u8,
                    frame.rect_mesh.indices.len() * std::mem::size_of::<u32>(),
                );
                self.gl.buffer_data_u8_slice(
                    glow::ELEMENT_ARRAY_BUFFER,
                    indices_bytes,
                    glow::DYNAMIC_DRAW,
                );

                self.gl.draw_elements(
                    glow::TRIANGLES,
                    frame.rect_mesh.indices.len() as i32,
                    glow::UNSIGNED_INT,
                    0,
                );
            }

            // Draw Text runs
            if !frame.text_runs.is_empty() {
                let text_mesh =
                    text_mesh_from_runs(&frame.text_runs, self.atlas_width, self.atlas_height)
                        .map_err(|e| {
                            RendererError::ShaderError(format!("Text mesh generation error: {e:?}"))
                        })?;

                if !text_mesh.is_empty() {
                    self.gl.use_program(Some(self.text_program));
                    self.gl.uniform_matrix_4_f32_slice(
                        Some(&self.text_proj_loc),
                        false,
                        &projection,
                    );

                    self.gl.active_texture(glow::TEXTURE0);
                    self.gl
                        .bind_texture(glow::TEXTURE_2D, Some(self.atlas_texture));
                    self.gl.uniform_1_i32(Some(&self.text_tex_loc), 0);

                    self.gl.bind_vertex_array(Some(self.text_vao));
                    self.gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.text_vbo));

                    let vertices_bytes = std::slice::from_raw_parts(
                        text_mesh.vertices.as_ptr() as *const u8,
                        text_mesh.vertices.len() * std::mem::size_of::<TexturedVertex>(),
                    );
                    self.gl.buffer_data_u8_slice(
                        glow::ARRAY_BUFFER,
                        vertices_bytes,
                        glow::DYNAMIC_DRAW,
                    );

                    self.gl
                        .bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.text_ebo));
                    let indices_bytes = std::slice::from_raw_parts(
                        text_mesh.indices.as_ptr() as *const u8,
                        text_mesh.indices.len() * std::mem::size_of::<u32>(),
                    );
                    self.gl.buffer_data_u8_slice(
                        glow::ELEMENT_ARRAY_BUFFER,
                        indices_bytes,
                        glow::DYNAMIC_DRAW,
                    );

                    self.gl.draw_elements(
                        glow::TRIANGLES,
                        text_mesh.indices.len() as i32,
                        glow::UNSIGNED_INT,
                        0,
                    );
                }
            }

            // Cleanup state
            self.gl.bind_vertex_array(None);
            self.gl.bind_texture(glow::TEXTURE_2D, None);
            self.gl.use_program(None);
            self.gl.disable(glow::BLEND);
        }

        Ok(())
    }
}

impl Drop for GlRenderer {
    fn drop(&mut self) {
        // SAFETY: GlRenderer cleanup runs safely when context remains valid.
        unsafe {
            self.gl.delete_program(self.rect_program);
            self.gl.delete_vertex_array(self.rect_vao);
            self.gl.delete_buffer(self.rect_vbo);
            self.gl.delete_buffer(self.rect_ebo);

            self.gl.delete_program(self.text_program);
            self.gl.delete_vertex_array(self.text_vao);
            self.gl.delete_buffer(self.text_vbo);
            self.gl.delete_buffer(self.text_ebo);

            self.gl.delete_texture(self.atlas_texture);
        }
    }
}

// Helper Orthographic Projection Matrix
fn ortho_projection(width: f32, height: f32) -> [f32; 16] {
    [
        2.0 / width,
        0.0,
        0.0,
        0.0,
        0.0,
        -2.0 / height,
        0.0,
        0.0,
        0.0,
        0.0,
        -1.0,
        0.0,
        -1.0,
        1.0,
        0.0,
        1.0,
    ]
}

// Helper to compile a program
unsafe fn create_program(
    gl: &glow::Context,
    vertex_src: &str,
    fragment_src: &str,
) -> Result<glow::Program, String> {
    // SAFETY: Compiles GLSL shaders within context.
    unsafe {
        let program = gl.create_program()?;

        let vs = gl.create_shader(glow::VERTEX_SHADER)?;
        gl.shader_source(vs, vertex_src);
        gl.compile_shader(vs);
        if !gl.get_shader_compile_status(vs) {
            let log = gl.get_shader_info_log(vs);
            gl.delete_shader(vs);
            return Err(format!("Vertex shader compile error: {log}"));
        }
        gl.attach_shader(program, vs);

        let fs = gl.create_shader(glow::FRAGMENT_SHADER)?;
        gl.shader_source(fs, fragment_src);
        gl.compile_shader(fs);
        if !gl.get_shader_compile_status(fs) {
            let log = gl.get_shader_info_log(fs);
            gl.delete_shader(fs);
            return Err(format!("Fragment shader compile error: {log}"));
        }
        gl.attach_shader(program, fs);

        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            let log = gl.get_program_info_log(program);
            return Err(format!("Program link error: {log}"));
        }

        gl.detach_shader(program, vs);
        gl.delete_shader(vs);
        gl.detach_shader(program, fs);
        gl.delete_shader(fs);

        Ok(program)
    }
}

// Rectangle shaders
const RECT_VERT_SRC: &str = r#"#version 330 core
layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_tex;
layout (location = 2) in vec4 a_color;
out vec4 v_color;
uniform mat4 u_projection;
void main() {
    gl_Position = u_projection * vec4(a_pos, 0.0, 1.0);
    v_color = a_color;
}
"#;

const RECT_FRAG_SRC: &str = r#"#version 330 core
in vec4 v_color;
out vec4 FragColor;
void main() {
    FragColor = v_color;
}
"#;

// Text shaders
const TEXT_VERT_SRC: &str = r#"#version 330 core
layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_tex;
layout (location = 2) in vec4 a_color;
out vec2 v_tex;
out vec4 v_color;
uniform mat4 u_projection;
void main() {
    gl_Position = u_projection * vec4(a_pos, 0.0, 1.0);
    v_tex = a_tex;
    v_color = a_color;
}
"#;

const TEXT_FRAG_SRC: &str = r#"#version 330 core
in vec2 v_tex;
in vec4 v_color;
out vec4 FragColor;
uniform sampler2D u_texture;
void main() {
    FragColor = texture(u_texture, v_tex) * v_color;
}
"#;
