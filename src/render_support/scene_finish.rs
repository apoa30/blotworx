//! Optional scene-only finishing. Runs before the HUD; Classic bypasses it.
use glow::{self, HasContext};
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub gamma: f32,
    pub bloom: f32,
    pub threshold: f32,
    pub vignette: f32,
    pub sharpen: f32,
    pub radius: f32,
    pub warmth: f32,
    pub rolloff: f32,
    pub smoothing: f32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            exposure: 0.,
            contrast: 1.,
            saturation: 1.,
            gamma: 1.,
            bloom: 0.,
            threshold: 0.75,
            vignette: 0.,
            sharpen: 0.,
            radius: 3.,
            warmth: 0.,
            rolloff: 0.,
            smoothing: 0.,
        }
    }
}
impl Settings {
    pub fn active(self) -> bool {
        self.exposure != 0.
            || self.contrast != 1.
            || self.saturation != 1.
            || self.gamma != 1.
            || self.bloom != 0.
            || self.vignette != 0.
            || self.sharpen != 0.
            || self.warmth != 0.
            || self.rolloff != 0.
            || self.smoothing != 0.
    }
}
pub struct Pass {
    program: glow::Program,
    vao: glow::VertexArray,
    texture: glow::Texture,
    size: std::cell::Cell<[i32; 2]>,
}
impl Pass {
    pub unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let program = gl.create_program()?;
        for (kind, source) in [
            (
                glow::VERTEX_SHADER,
                r#"#version 330 core
out vec2 uv;
void main(){ vec2 p=vec2((gl_VertexID<<1)&2,gl_VertexID&2); uv=p; gl_Position=vec4(p*2.-1.,0.,1.); }
"#,
            ),
            (
                glow::FRAGMENT_SHADER,
                r#"#version 330 core
in vec2 uv; out vec4 color;
uniform sampler2D scene;uniform sampler2D indirect_scene;uniform sampler2D ambient_visibility;uniform sampler2D scene_depth;uniform bool has_ao;uniform vec3 depth_projection; uniform vec4 sample_rect;uniform int hdr_style; uniform vec4 grade; uniform vec4 finish; uniform vec4 quality;
vec3 sampleAt(vec2 p){return texture(scene,sample_rect.xy+clamp(p,vec2(0.),vec2(1.))*sample_rect.zw).rgb;}
float contactVisibility(vec2 p){
 vec2 q=sample_rect.xy+p*sample_rect.zw;float depth=texture(scene_depth,q).r;float z=depth_projection.z!=0.?-depth_projection.y/(depth*2.-1.+depth_projection.x):(depth*2.-1.-depth_projection.y)/depth_projection.x;
 vec2 size=vec2(textureSize(ambient_visibility,0)),at=q*size-.5;vec2 origin=floor(at),fraction=fract(at);float sum=0.,weight=0.;
 for(int y=0;y<2;y++)for(int x=0;x<2;x++){vec2 o=vec2(x,y);vec2 probe=texture(ambient_visibility,(origin+o+.5)/size).rg;float w=(x==0?1.-fraction.x:fraction.x)*(y==0?1.-fraction.y:fraction.y)/(1.+abs(probe.y-z)*64.);sum+=probe.x*w;weight+=w;}
 return weight>1e-6?sum/weight:1.;
}
void main(){
 vec2 texel=1./(vec2(textureSize(scene,0))*sample_rect.zw); vec3 base=sampleAt(uv);
 if(has_ao){vec3 ambient=texture(indirect_scene,sample_rect.xy+uv*sample_rect.zw).rgb;base=max(base-ambient*(1.-contactVisibility(uv)),vec3(0.));}
 vec3 blur=(sampleAt(uv+texel*vec2(1,0))+sampleAt(uv-texel*vec2(1,0))+sampleAt(uv+texel*vec2(0,1))+sampleAt(uv-texel*vec2(0,1)))*.25;
 float edge=length(base-blur);
 vec3 c=max(vec3(0.),mix(base,blur,quality.w*smoothstep(.06,.3,edge))+(base-blur)*finish.w);
 if(finish.x>0.){ vec3 glowColor=vec3(0.); float weight=0.;
  for(int y=-2;y<=2;y++)for(int x=-2;x<=2;x++){float w=1./(1.+float(x*x+y*y)); vec3 s=sampleAt(uv+vec2(x,y)*texel*quality.x); glowColor+=max(s-vec3(finish.y),vec3(0.))*w;weight+=w;}
  c+=glowColor/weight*finish.x;
 }
 c*=exp2(grade.x);
 if(hdr_style!=0){
  if(hdr_style==2){c=clamp((c*(2.51*c+.03))/(c*(2.43*c+.59)+.14),0.,1.);}
  else {float lum=dot(c,vec3(.2126,.7152,.0722));if(lum>.7){float mapped=.7+.3*(1.-exp(-(lum-.7)/.3));c*=mapped/max(lum,.0001);}}
  c=pow(max(c,vec3(0.)),vec3(1./2.2));
 }
 c*=vec3(1.+quality.y,1.,1.-quality.y);c=c/(1.+c*quality.z);
 float l=dot(c,vec3(.2126,.7152,.0722)); c=mix(vec3(l),c,grade.z);
 c=(c-.5)*grade.y+.5; c=pow(max(c,vec3(0.)),vec3(1./grade.w));
 vec2 v=uv*2.-1.; c*=1.-finish.z*smoothstep(.2,1.6,dot(v,v)); color=vec4(clamp(c,0.,1.),1.);
}
"#,
            ),
        ] {
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
        let vao = gl.create_vertex_array()?;
        let texture = gl.create_texture()?;
        Ok(Self {
            program,
            vao,
            texture,
            size: std::cell::Cell::new([0, 0]),
        })
    }
    pub unsafe fn draw(&self, gl: &glow::Context, rect: [i32; 4], s: Settings) {
        let [x, y, w, h] = rect;
        if w <= 0 || h <= 0 {
            return;
        }
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
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
        if self.size.get() != [w, h] {
            gl.copy_tex_image_2d(glow::TEXTURE_2D, 0, glow::RGB, x, y, w, h, 0);
            self.size.set([w, h]);
        } else {
            gl.copy_tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, x, y, w, h);
        }
        self.draw_bound(gl, rect, s, 0, [0., 0., 1., 1.], false);
    }
    pub unsafe fn draw_hdr(
        &self,
        gl: &glow::Context,
        rect: [i32; 4],
        s: Settings,
        style: u32,
        texture: glow::Texture,
        size: [i32; 2],
        indirect: glow::Texture,
        depth: glow::Texture,
        ao: Option<glow::Texture>,
        projection: [f32; 16],
    ) {
        gl.active_texture(glow::TEXTURE0);
        gl.bind_sampler(0, None);
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.use_program(Some(self.program));
        for (unit, texture, name) in [
            (1, Some(indirect), "indirect_scene"),
            (2, ao, "ambient_visibility"),
            (3, Some(depth), "scene_depth"),
        ] {
            gl.active_texture(glow::TEXTURE0 + unit);
            gl.bind_sampler(unit, None);
            gl.bind_texture(glow::TEXTURE_2D, texture);
            gl.uniform_1_i32(
                gl.get_uniform_location(self.program, name).as_ref(),
                unit as i32,
            );
        }
        gl.uniform_3_f32(
            gl.get_uniform_location(self.program, "depth_projection")
                .as_ref(),
            projection[10],
            projection[14],
            projection[11],
        );
        gl.active_texture(glow::TEXTURE0);
        let [x, y, w, h] = rect;
        self.draw_bound(
            gl,
            rect,
            s,
            style as i32,
            [
                x as f32 / size[0] as f32,
                y as f32 / size[1] as f32,
                w as f32 / size[0] as f32,
                h as f32 / size[1] as f32,
            ],
            ao.is_some(),
        );
    }
    unsafe fn draw_bound(
        &self,
        gl: &glow::Context,
        rect: [i32; 4],
        s: Settings,
        hdr: i32,
        sample_rect: [f32; 4],
        has_ao: bool,
    ) {
        let [x, y, w, h] = rect;
        gl.viewport(x, y, w, h);
        gl.disable(glow::DEPTH_TEST);
        gl.depth_mask(false);
        gl.disable(glow::BLEND);
        gl.disable(glow::CULL_FACE);
        gl.use_program(Some(self.program));
        gl.uniform_1_i32(
            gl.get_uniform_location(self.program, "has_ao").as_ref(),
            i32::from(has_ao),
        );
        gl.uniform_1_i32(gl.get_uniform_location(self.program, "scene").as_ref(), 0);
        gl.uniform_1_i32(
            gl.get_uniform_location(self.program, "hdr_style").as_ref(),
            hdr,
        );
        gl.uniform_4_f32_slice(
            gl.get_uniform_location(self.program, "sample_rect")
                .as_ref(),
            &sample_rect,
        );
        gl.uniform_4_f32(
            gl.get_uniform_location(self.program, "grade").as_ref(),
            s.exposure,
            s.contrast,
            s.saturation,
            s.gamma,
        );
        gl.uniform_4_f32(
            gl.get_uniform_location(self.program, "finish").as_ref(),
            s.bloom,
            s.threshold,
            s.vignette,
            s.sharpen,
        );
        gl.uniform_4_f32(
            gl.get_uniform_location(self.program, "quality").as_ref(),
            s.radius,
            s.warmth,
            s.rolloff,
            s.smoothing,
        );
        gl.bind_vertex_array(Some(self.vao));
        gl.draw_arrays(glow::TRIANGLES, 0, 3);
        gl.bind_vertex_array(None);
        gl.use_program(None);
        gl.depth_mask(true);
    }
}
