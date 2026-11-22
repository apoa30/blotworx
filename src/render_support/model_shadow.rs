//! Retained model-to-texture target. Solid120 gray becomes119 after I4
//! expansion (7*17); white remains255. EFB/raster parity is not yet accepted.
use glow::{self, HasContext};
use std::sync::Arc;
#[derive(Clone, Debug)]
pub struct Caster {
    pub vertices: Arc<Vec<f32>>,
    pub indices: Arc<Vec<u32>>,
    pub model: [f32; 16],
}
#[derive(Clone, Debug)]
pub struct Frame {
    pub view_projection: [f32; 16],
    pub projection: [f32; 12],
    pub clip: [f32; 12],
    pub casters: Vec<Caster>,
}
impl Frame {
    pub fn receiver_matrices(&self, model: &[f32; 16]) -> anyhow::Result<[[f32; 16]; 2]> {
        let m = std::array::from_fn(|i| model[(i % 4) * 4 + i / 4]);
        let mut p = crate::blotworx::texture_effect::concat(&self.projection, &m)?;
        let c = crate::blotworx::texture_effect::concat(&self.clip, &m)?;
        for i in 4..7 {
            p[i] = -p[i];
        }
        p[7] = 1. - p[7];
        let gl = |r: [f32; 12]| {
            std::array::from_fn(|i| {
                if i % 4 == 3 {
                    if i == 15 {
                        1.
                    } else {
                        0.
                    }
                } else {
                    r[(i % 4) * 4 + i / 4]
                }
            })
        };
        Ok([gl(p), gl(c)])
    }
}
#[derive(Default, Clone, Debug, serde::Serialize)]
pub struct Report {
    pub frames: u64,
    pub target_allocations: u64,
    pub caster_draws: usize,
    pub receiver_draws: usize,
    pub error: Option<String>,
    pub original_frame_parity_validated: bool,
}
pub struct Pass {
    program: glow::Program,
    view: glow::UniformLocation,
    model: glow::UniformLocation,
    framebuffer: glow::Framebuffer,
    pub image: glow::Texture,
    pub clip_image: glow::Texture,
    depth: glow::Renderbuffer,
}
unsafe fn image(
    gl: &glow::Context,
    w: i32,
    h: i32,
    data: Option<&[u8]>,
) -> Result<glow::Texture, String> {
    let t = gl.create_texture()?;
    gl.bind_texture(glow::TEXTURE_2D, Some(t));
    gl.tex_image_2d(
        glow::TEXTURE_2D,
        0,
        glow::R8 as i32,
        w,
        h,
        0,
        glow::RED,
        glow::UNSIGNED_BYTE,
        glow::PixelUnpackData::Slice(data),
    );
    for (n, v) in [
        (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
        (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
        (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
        (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
        (glow::TEXTURE_SWIZZLE_R, glow::RED),
        (glow::TEXTURE_SWIZZLE_G, glow::RED),
        (glow::TEXTURE_SWIZZLE_B, glow::RED),
        (glow::TEXTURE_SWIZZLE_A, glow::RED),
    ] {
        gl.tex_parameter_i32(glow::TEXTURE_2D, n, v as i32);
    }
    Ok(t)
}
impl Pass {
    pub unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let p = gl.create_program()?;
        for (k,s) in [(glow::VERTEX_SHADER,"#version 330 core\nlayout(location=0)in vec3 p;uniform mat4 vp;uniform mat4 model;void main(){gl_Position=vp*model*vec4(p,1.);}"),
            (glow::FRAGMENT_SHADER,"#version 330 core\nout vec4 color;void main(){color=vec4(vec3(119./255.),0.);}")] {
            let shader=gl.create_shader(k)?;gl.shader_source(shader, &crate::render_support::shader_source(s));gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader){let e=gl.get_shader_info_log(shader);gl.delete_shader(shader);gl.delete_program(p);return Err(e);}
            gl.attach_shader(p,shader);gl.delete_shader(shader);
        }
        gl.link_program(p);
        if !gl.get_program_link_status(p) {
            let e = gl.get_program_info_log(p);
            gl.delete_program(p);
            return Err(e);
        }
        let view = gl
            .get_uniform_location(p, "vp")
            .ok_or("Projector camera uniform missing")?;
        let model = gl
            .get_uniform_location(p, "model")
            .ok_or("Projector model uniform missing")?;
        let old =
            std::num::NonZeroU32::new(gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING) as u32)
                .map(glow::NativeFramebuffer);
        let t = image(gl, 256, 256, None)?;
        let clip = image(gl, 4, 2, Some(&[0, 255, 255, 0, 0, 255, 255, 0]))?;
        let f = gl.create_framebuffer()?;
        let d = gl.create_renderbuffer()?;
        gl.bind_renderbuffer(glow::RENDERBUFFER, Some(d));
        gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH_COMPONENT24, 256, 256);
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
        let complete = gl.check_framebuffer_status(glow::FRAMEBUFFER);
        gl.bind_framebuffer(glow::FRAMEBUFFER, old);
        if complete != glow::FRAMEBUFFER_COMPLETE {
            gl.delete_framebuffer(f);
            gl.delete_renderbuffer(d);
            gl.delete_texture(t);
            gl.delete_texture(clip);
            gl.delete_program(p);
            return Err(format!("Projector framebuffer incomplete: {complete:#x}"));
        }
        Ok(Self {
            program: p,
            view,
            model,
            framebuffer: f,
            image: t,
            clip_image: clip,
            depth: d,
        })
    }
    pub unsafe fn begin(&self, gl: &glow::Context, frame: &Frame) {
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
        gl.viewport(0, 0, 256, 256);
        gl.disable(glow::SCISSOR_TEST);
        gl.color_mask(true, true, true, true);
        gl.depth_mask(true);
        gl.clear_color(1., 1., 1., 1.);
        gl.clear_depth(1.);
        gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
        gl.disable(glow::BLEND);
        gl.enable(glow::DEPTH_TEST);
        gl.depth_func(glow::LEQUAL);
        gl.enable(glow::CULL_FACE);
        gl.front_face(glow::CCW);
        gl.cull_face(glow::BACK);
        gl.use_program(Some(self.program));
        gl.uniform_matrix_4_f32_slice(Some(&self.view), false, &frame.view_projection);
    }
    pub unsafe fn model(&self, gl: &glow::Context, model: &[f32; 16]) {
        gl.uniform_matrix_4_f32_slice(Some(&self.model), false, model);
    }
    pub unsafe fn destroy(self, gl: &glow::Context) {
        gl.delete_program(self.program);
        gl.delete_framebuffer(self.framebuffer);
        gl.delete_texture(self.image);
        gl.delete_texture(self.clip_image);
        gl.delete_renderbuffer(self.depth);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projector_receiver_matrices_preserve_native_concat_bits() -> anyhow::Result<()> {
        let r: serde_json::Value = serde_json::from_str(include_str!(
            "../../tools/blotworx/fixtures/projector_texture_matrices.json"
        ))?;
        let floats = |v: &serde_json::Value| {
            v.as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap() as f32)
                .collect::<Vec<_>>()
        };
        for c in r["cases"].as_array().unwrap() {
            let rows = floats(&c["model_matrix"]);
            let model = std::array::from_fn(|i| {
                if i % 4 == 3 {
                    if i == 15 {
                        1.
                    } else {
                        0.
                    }
                } else {
                    rows[(i % 4) * 4 + i / 4]
                }
            });
            let plane = floats(&c["world_clip_matrix"]);
            let frame = Frame {
                view_projection: model,
                projection: plane.clone().try_into().unwrap(),
                clip: plane.try_into().unwrap(),
                casters: vec![],
            };
            let matrices = frame.receiver_matrices(&model)?;
            let expected = floats(&c["local_clip_matrix"]);
            for row in 0..3 {
                for col in 0..4 {
                    assert_eq!(
                        matrices[1][col * 4 + row].to_bits(),
                        expected[row * 4 + col].to_bits()
                    );
                }
            }
        }
        Ok(())
    }
}
