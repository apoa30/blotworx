//! Depth-tested retail bicone mask adapter. Native TEV mask color120/255,
//! additive front faces and subtractive back faces, then scene*(1-mask).
//! Target precision/order and original-frame parity remain separate validation.
use glow::{self, HasContext};
use std::sync::Arc;
#[derive(Clone, Debug)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Report {
    pub submitted: usize,
    pub frames: u64,
    pub target_allocations: u64,
    pub size: [i32; 2],
    pub error: Option<String>,
}
pub struct Pass {
    program: glow::Program,
    composite: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    ebo: glow::Buffer,
    quad: glow::VertexArray,
    view: glow::UniformLocation,
    model: glow::UniformLocation,
    mask: glow::UniformLocation,
    target: Option<(glow::Framebuffer, glow::Texture, glow::Renderbuffer)>,
    size: [i32; 2],
    mesh: std::sync::Weak<Mesh>,
    count: i32,
}
unsafe fn program(gl: &glow::Context, vs: &str, fs: &str) -> Result<glow::Program, String> {
    let p = gl.create_program()?;
    for (kind, source) in [(glow::VERTEX_SHADER, vs), (glow::FRAGMENT_SHADER, fs)] {
        let s = gl.create_shader(kind)?;
        gl.shader_source(s, &crate::render_support::shader_source(source));
        gl.compile_shader(s);
        if !gl.get_shader_compile_status(s) {
            let error = gl.get_shader_info_log(s);
            gl.delete_shader(s);
            gl.delete_program(p);
            return Err(error);
        }
        gl.attach_shader(p, s);
        gl.delete_shader(s);
    }
    gl.link_program(p);
    if !gl.get_program_link_status(p) {
        let e = gl.get_program_info_log(p);
        gl.delete_program(p);
        return Err(e);
    }
    Ok(p)
}
impl Pass {
    pub unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let program = program(
            gl,
            r#"#version 330 core
layout(location=0) in vec3 p;uniform mat4 view_projection;uniform mat4 model;
void main(){gl_Position=view_projection*model*vec4(p,1.);}"#,
            r#"#version 330 core
out vec4 color;void main(){color=vec4(vec3(120./255.),0.);}"#,
        )?;
        let composite = match self::program(
            gl,
            r#"#version 330 core
out vec2 uv;void main(){vec2 p=vec2((gl_VertexID<<1)&2,gl_VertexID&2);uv=p;gl_Position=vec4(p*2.-1.,0.,1.);}"#,
            r#"#version 330 core
in vec2 uv;uniform sampler2D mask_image;out vec4 color;
void main(){color=vec4(0.,0.,0.,texture(mask_image,uv).r);}"#,
        ) {
            Ok(p) => p,
            Err(e) => {
                gl.delete_program(program);
                return Err(e);
            }
        };
        let view = gl
            .get_uniform_location(program, "view_projection")
            .ok_or("Shadow view uniform missing")?;
        let model = gl
            .get_uniform_location(program, "model")
            .ok_or("Shadow model uniform missing")?;
        let mask = gl
            .get_uniform_location(composite, "mask_image")
            .ok_or("Shadow mask uniform missing")?;
        Ok(Self {
            program,
            composite,
            vao: gl.create_vertex_array()?,
            vbo: gl.create_buffer()?,
            ebo: gl.create_buffer()?,
            quad: gl.create_vertex_array()?,
            view,
            model,
            mask,
            target: None,
            size: [0; 2],
            mesh: Default::default(),
            count: 0,
        })
    }
    unsafe fn resize(&mut self, gl: &glow::Context, size: [i32; 2]) -> Result<bool, String> {
        if self.size == size {
            return Ok(false);
        }
        if let Some((f, t, d)) = self.target.take() {
            gl.delete_framebuffer(f);
            gl.delete_texture(t);
            gl.delete_renderbuffer(d);
        }
        self.size = [0; 2];
        let f = gl.create_framebuffer()?;
        let t = gl.create_texture()?;
        let d = gl.create_renderbuffer()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(t));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            size[0],
            size[1],
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        for (k, v) in [
            (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
            (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
        ] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, k, v as i32);
        }
        gl.bind_renderbuffer(glow::RENDERBUFFER, Some(d));
        gl.renderbuffer_storage(
            glow::RENDERBUFFER,
            glow::DEPTH_COMPONENT24,
            size[0],
            size[1],
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(f));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(t),
            0,
        );
        gl.framebuffer_renderbuffer(
            glow::FRAMEBUFFER,
            glow::DEPTH_ATTACHMENT,
            glow::RENDERBUFFER,
            Some(d),
        );
        let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
        self.target = Some((f, t, d));
        if status != glow::FRAMEBUFFER_COMPLETE {
            return Err(format!("Shadow framebuffer incomplete: {status:#x}"));
        }
        self.size = size;
        Ok(true)
    }
    pub unsafe fn draw(
        &mut self,
        gl: &glow::Context,
        mesh: &Arc<Mesh>,
        models: &[[f32; 16]],
        vp: &[f32; 16],
        rect: [i32; 4],
        report: &mut Report,
    ) -> Result<(), String> {
        let [left, bottom, width, height] = rect;
        report.submitted = models.len();
        let old = gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING);
        let old = std::num::NonZeroU32::new(old as u32).map(glow::NativeFramebuffer);
        // The engine requests a24-bit, non-multisampled depth buffer.
        if gl.get_parameter_i32(glow::SAMPLES) != 0 {
            return Err(
                "Shadow depth transfer requires the engine's non-multisampled target".into(),
            );
        }
        let result = (|| -> Result<(), String> {
            if self.resize(gl, [width, height])? {
                report.target_allocations += 1;
            }
            if self
                .mesh
                .upgrade()
                .is_none_or(|old| !Arc::ptr_eq(&old, mesh))
            {
                self.count =
                    i32::try_from(mesh.indices.len()).map_err(|_| "Shadow index count overflow")?;
                gl.bind_vertex_array(Some(self.vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
                let p = std::slice::from_raw_parts(
                    mesh.positions.as_ptr() as *const u8,
                    mesh.positions.len() * 12,
                );
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, p, glow::STATIC_DRAW);
                gl.enable_vertex_attrib_array(0);
                gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 12, 0);
                gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.ebo));
                let i = std::slice::from_raw_parts(
                    mesh.indices.as_ptr() as *const u8,
                    mesh.indices.len() * 4,
                );
                gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, i, glow::STATIC_DRAW);
                self.mesh = Arc::downgrade(mesh);
            }
            let (f, _, _) = self.target.ok_or("Shadow target missing")?;
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, old);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(f));
            gl.blit_framebuffer(
                left,
                bottom,
                left + width,
                bottom + height,
                0,
                0,
                width,
                height,
                glow::DEPTH_BUFFER_BIT,
                glow::NEAREST,
            );
            let error = gl.get_error();
            if error != glow::NO_ERROR {
                return Err(format!("Shadow depth transfer GL error {error:#x}"));
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(f));
            gl.viewport(0, 0, width, height);
            gl.color_mask(true, true, true, true);
            gl.clear_color(0., 0., 0., 0.);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.depth_mask(false);
            gl.enable(glow::CULL_FACE);
            gl.front_face(glow::CCW);
            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE);
            gl.use_program(Some(self.program));
            gl.uniform_matrix_4_f32_slice(Some(&self.view), false, vp);
            gl.bind_vertex_array(Some(self.vao));
            for model in models {
                gl.uniform_matrix_4_f32_slice(Some(&self.model), false, model);
                gl.cull_face(glow::BACK);
                gl.blend_equation(glow::FUNC_ADD);
                gl.draw_elements(glow::TRIANGLES, self.count, glow::UNSIGNED_INT, 0);
                gl.cull_face(glow::FRONT);
                gl.blend_equation(glow::FUNC_REVERSE_SUBTRACT);
                gl.draw_elements(glow::TRIANGLES, self.count, glow::UNSIGNED_INT, 0);
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, old);
            gl.viewport(left, bottom, width, height);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.blend_equation(glow::FUNC_ADD);
            gl.blend_func_separate(
                glow::SRC_ALPHA,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ZERO,
                glow::ONE,
            );
            gl.use_program(Some(self.composite));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, self.target.map(|(_, t, _)| t));
            gl.uniform_1_i32(Some(&self.mask), 0);
            gl.bind_vertex_array(Some(self.quad));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            report.frames += 1;
            report.size = [width, height];
            Ok(())
        })();
        gl.bind_framebuffer(glow::FRAMEBUFFER, old);
        gl.viewport(left, bottom, width, height);
        gl.enable(glow::SCISSOR_TEST);
        gl.scissor(left, bottom, width, height);
        gl.depth_mask(true);
        gl.depth_func(glow::LEQUAL);
        gl.disable(glow::BLEND);
        gl.blend_equation(glow::FUNC_ADD);
        gl.disable(glow::CULL_FACE);
        gl.bind_vertex_array(None);
        gl.use_program(None);
        result
    }
}
