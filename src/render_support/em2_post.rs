//! EM2 PC post-processing, ported from the game's own effects in shaders.pak:
//! pp_bloom (PostProcess_BloomThresh / GaussBlurH / GaussBlurV / BloomComp) and
//! pp_colourcorrection (PostProcess_ColorCorrection). The active render profile
//! (RenderSceneEffectsComponent) supplies the values each frame via `set`.
use egui_glow::glow::{self, HasContext};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// g_bloomHighlightThreshold = Gen3BloomThreshold (360 0x829ce670, unscaled)
    pub bloom_threshold: f32,
    /// g_bloomGlowIntensity = Gen3BloomIntensity
    pub bloom_intensity: f32,
    /// g_gaussBlurWidth = min(|Gen3BloomBlur|, 1.5)
    pub blur_width: f32,
    /// g_gen3ToneBrightness = Gen3ToneBrightness/100 (profile default 20 <-> effect default 0.2)
    pub brightness: f32,
    /// g_gen3ToneContrast = 1 + Gen3ToneContrast/100 (15 <-> 1.15)
    pub contrast: f32,
    /// g_gen3ToneSaturation = 1 + Gen3ToneSaturation/100 (5 <-> 1.05)
    pub saturation: f32,
}
static PARAMS: std::sync::Mutex<Option<Params>> = std::sync::Mutex::new(None);
pub fn set(params: Option<Params>) {
    if let Ok(mut slot) = PARAMS.lock() {
        *slot = params;
    }
}
static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
/// F10 menu switch (Tools tab).
pub fn set_enabled(on: bool) { ENABLED.store(on, std::sync::atomic::Ordering::Relaxed); }
pub fn enabled() -> bool { ENABLED.load(std::sync::atomic::Ordering::Relaxed) }
pub fn get() -> Option<Params> {
    if std::env::var_os("BLOTWORX_EM2_POST").is_some_and(|v| v == "0") || !enabled() {
        return None;
    }
    PARAMS.lock().ok().and_then(|p| *p)
}

const VERTEX: &str = r#"#version 330 core
out vec2 uv;
void main(){ vec2 p=vec2((gl_VertexID<<1)&2,gl_VertexID&2); uv=p; gl_Position=vec4(p*2.-1.,0.,1.); }
"#;
// pp_bloom PostProcess_BloomThresh (the extra bloom/toon buffers it adds are empty here)
const THRESHOLD: &str = r#"#version 330 core
in vec2 uv; out vec4 color; uniform sampler2D scene; uniform float threshold;
void main(){
 vec3 c=texture(scene,uv).rgb;
 float w=clamp((dot(c,vec3(.3,.59,.11))-threshold+.1)*5.,0.,1.);
 w=w*w*(3.-2.*w);
 color=vec4(c*w,1.);
}
"#;
// pp_bloom GaussBlurH/V: 7 taps, weights .05 .1 .2 .3 .2 .1 .05
const BLUR: &str = r#"#version 330 core
in vec2 uv; out vec4 color; uniform sampler2D source; uniform vec2 step_uv;
void main(){
 float w[7]=float[7](.05,.1,.2,.3,.2,.1,.05);
 vec4 sum=vec4(0.);
 for(int i=0;i<7;i++){sum+=texture(source,uv+step_uv*float(i-3))*w[i];}
 color=sum;
}
"#;
// pp_bloom BloomComp (scene*g_bloomSceneIntensity + bloom*g_bloomGlowIntensity), then
// pp_colourcorrection with g_ColorSwapWeight=1 and g_Brightness=1 (their effect defaults).
const FINAL: &str = r#"#version 330 core
in vec2 uv; out vec4 color; uniform sampler2D scene; uniform sampler2D bloom;
uniform float intensity; uniform vec3 tone;
void main(){
 vec3 c=texture(scene,uv).rgb+texture(bloom,uv).rgb*intensity;
 vec3 r=(c-.5)*tone.y+tone.x+.5;
 float l=dot(r,vec3(.299,.587,.114));
 r=mix(vec3(l),r,tone.z);
 color=vec4(clamp(r,0.,1.),1.);
}
"#;

pub struct Pass {
    threshold: glow::Program,
    blur: glow::Program,
    finish: glow::Program,
    vao: glow::VertexArray,
    scene: glow::Texture,
    half: [glow::Texture; 2],
    fbo: [glow::Framebuffer; 2],
    size: std::cell::Cell<[i32; 2]>,
}
unsafe fn program(gl: &glow::Context, fragment: &str) -> Result<glow::Program, String> {
    let program = gl.create_program()?;
    for (kind, source) in [(glow::VERTEX_SHADER, VERTEX), (glow::FRAGMENT_SHADER, fragment)] {
        let shader = gl.create_shader(kind)?;
        gl.shader_source(shader, &crate::render_support::shader_source(source));
        gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader) {
            let e = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            gl.delete_program(program);
            return Err(e);
        }
        gl.attach_shader(program, shader);
        gl.delete_shader(shader);
    }
    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        let e = gl.get_program_info_log(program);
        gl.delete_program(program);
        return Err(e);
    }
    Ok(program)
}
unsafe fn linear(gl: &glow::Context) {
    for (k, v) in [
        (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
        (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
        (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
        (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
    ] {
        gl.tex_parameter_i32(glow::TEXTURE_2D, k, v as i32);
    }
}
impl Pass {
    pub unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        Ok(Self {
            threshold: program(gl, THRESHOLD)?,
            blur: program(gl, BLUR)?,
            finish: program(gl, FINAL)?,
            vao: gl.create_vertex_array()?,
            scene: gl.create_texture()?,
            half: [gl.create_texture()?, gl.create_texture()?],
            fbo: [gl.create_framebuffer()?, gl.create_framebuffer()?],
            size: std::cell::Cell::new([0, 0]),
        })
    }
    /// Reads the finished frame from the bound draw framebuffer's `rect` and writes it back processed.
    pub unsafe fn draw(&self, gl: &glow::Context, rect: [i32; 4], p: Params) {
        let [x, y, w, h] = rect;
        if w <= 1 || h <= 1 {
            return;
        }
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            eprintln!("[post] EM2 bloom + colour correction on {w}x{h}: {p:?}");
        }
        let destination = std::num::NonZeroU32::new(gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING) as u32).map(glow::NativeFramebuffer);
        let (hw, hh) = ((w / 2).max(1), (h / 2).max(1));
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::BLEND);
        gl.disable(glow::CULL_FACE);
        gl.disable(glow::SCISSOR_TEST);
        gl.depth_mask(false);
        gl.bind_vertex_array(Some(self.vao));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.scene));
        linear(gl);
        if self.size.get() != [w, h] {
            gl.copy_tex_image_2d(glow::TEXTURE_2D, 0, glow::RGB, x, y, w, h, 0);
            for (texture, fbo) in self.half.iter().zip(&self.fbo) {
                gl.bind_texture(glow::TEXTURE_2D, Some(*texture));
                linear(gl);
                gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA16F as i32, hw, hh, 0, glow::RGBA, glow::FLOAT, glow::PixelUnpackData::Slice(None));
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(*fbo));
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(*texture), 0);
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, destination);
            self.size.set([w, h]);
        } else {
            gl.copy_tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, x, y, w, h);
        }
        // threshold -> half[0]
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo[0]));
        gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);
        gl.viewport(0, 0, hw, hh);
        gl.use_program(Some(self.threshold));
        gl.bind_texture(glow::TEXTURE_2D, Some(self.scene));
        gl.uniform_1_i32(gl.get_uniform_location(self.threshold, "scene").as_ref(), 0);
        gl.uniform_1_f32(gl.get_uniform_location(self.threshold, "threshold").as_ref(), p.bloom_threshold);
        gl.draw_arrays(glow::TRIANGLES, 0, 3);
        // blur H: half[0] -> half[1]; blur V: half[1] -> half[0]
        gl.use_program(Some(self.blur));
        gl.uniform_1_i32(gl.get_uniform_location(self.blur, "source").as_ref(), 0);
        let step = gl.get_uniform_location(self.blur, "step_uv");
        for (from, to, dir) in [(0usize, 1usize, [1f32, 0.]), (1, 0, [0., 1.])] {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo[to]));
            gl.bind_texture(glow::TEXTURE_2D, Some(self.half[from]));
            gl.uniform_2_f32(step.as_ref(), dir[0] * p.blur_width / hw as f32, dir[1] * p.blur_width / hh as f32);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
        }
        // composite + colour correction -> destination rect
        gl.bind_framebuffer(glow::FRAMEBUFFER, destination);
        gl.viewport(x, y, w, h);
        gl.use_program(Some(self.finish));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.scene));
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.half[0]));
        gl.uniform_1_i32(gl.get_uniform_location(self.finish, "scene").as_ref(), 0);
        gl.uniform_1_i32(gl.get_uniform_location(self.finish, "bloom").as_ref(), 1);
        gl.uniform_1_f32(gl.get_uniform_location(self.finish, "intensity").as_ref(), p.bloom_intensity);
        gl.uniform_3_f32(gl.get_uniform_location(self.finish, "tone").as_ref(), p.brightness, p.contrast, p.saturation);
        gl.draw_arrays(glow::TRIANGLES, 0, 3);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.use_program(None);
        gl.bind_vertex_array(None);
        gl.depth_mask(true);
    }
}
