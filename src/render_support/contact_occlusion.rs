//! Half-resolution geometry contact visibility, with view-space radius and
//! depth-aware reconstruction. Consumed by the indirect-light resolve only.
use glow::{self, HasContext};
pub struct Pass {
    program: glow::Program,
    vao: glow::VertexArray,
    target: Option<(glow::Framebuffer, glow::Texture, [i32; 2])>,
}
impl Pass {
    pub unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let program = gl.create_program()?;
        for (kind, source) in [
            (
                glow::VERTEX_SHADER,
                r#"#version 330 core
out vec2 uv;void main(){vec2 p=vec2((gl_VertexID<<1)&2,gl_VertexID&2);uv=p;gl_Position=vec4(p*2.-1.,0.,1.);}
"#,
            ),
            (
                glow::FRAGMENT_SHADER,
                r#"#version 330 core
in vec2 uv;out vec2 visibility_depth;uniform sampler2D depth_image;uniform vec4 projection_a;uniform vec4 projection_b;uniform vec4 scene_rect;uniform vec2 settings;
vec3 positionAt(vec2 p,float depth){vec2 ndc=(p-scene_rect.xy)/scene_rect.zw*2.-1.;float z;if(projection_b.z!=0.){z=-projection_b.y/(depth*2.-1.+projection_b.x);return vec3(-(ndc+projection_a.zw)*z/projection_a.xy,z);}z=(depth*2.-1.-projection_b.y)/projection_b.x;return vec3((ndc-projection_a.zw)/projection_a.xy,z);}
vec3 positionAt(vec2 p){return positionAt(p,texture(depth_image,clamp(p,scene_rect.xy,scene_rect.xy+scene_rect.zw)).r);}
void main(){
 vec2 local=(uv-scene_rect.xy)/scene_rect.zw;float depth=texture(depth_image,uv).r;
 if(any(lessThan(local,vec2(0.)))||any(greaterThan(local,vec2(1.)))||depth>=.999999){visibility_depth=vec2(1.,1e6);return;}
 vec3 P=positionAt(uv,depth);vec2 pixel=1./vec2(textureSize(depth_image,0));
 vec3 left=P-positionAt(uv-vec2(pixel.x,0.)),right=positionAt(uv+vec2(pixel.x,0.))-P;
 vec3 down=P-positionAt(uv-vec2(0.,pixel.y)),up=positionAt(uv+vec2(0.,pixel.y))-P;
 vec3 dx=dot(left,left)<dot(right,right)?left:right,dy=dot(down,down)<dot(up,up)?down:up;
 vec3 N=normalize(cross(dx,dy));if(dot(N,-P)<0.)N=-N;
 float radius=settings.y;vec2 projected=radius*.5*projection_a.xy*scene_rect.zw/(projection_b.z!=0.?max(-P.z,.01):1.);
 float occlusion=0.;float count=0.;
 for(int i=0;i<12;i++){float angle=float(i)*2.39996323;float scale=sqrt((float(i)+.5)/12.);vec2 q=uv+vec2(cos(angle),sin(angle))*projected*scale;
  vec3 D=positionAt(q)-P;float distance=length(D);if(distance>.0001&&distance<radius){float horizon=max(dot(N,D/distance)-.12,0.);occlusion+=horizon*(1.-distance/radius);}count+=1.;}
 visibility_depth=vec2(clamp(1.-settings.x*4.*occlusion/count,.05,1.),P.z);
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
        Ok(Self {
            program,
            vao: gl.create_vertex_array()?,
            target: None,
        })
    }
    pub unsafe fn draw(
        &mut self,
        gl: &glow::Context,
        depth: glow::Texture,
        size: [i32; 2],
        rect: [i32; 4],
        projection: [f32; 16],
        strength: f32,
        radius: f32,
    ) -> Result<glow::Texture, String> {
        let resolution = [(size[0] + 1) / 2, (size[1] + 1) / 2];
        let old =
            std::num::NonZeroU32::new(gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING) as u32)
                .map(glow::NativeFramebuffer);
        if self.target.as_ref().is_some_and(|t| t.2 != resolution) {
            let (f, t, _) = self.target.take().unwrap();
            gl.delete_framebuffer(f);
            gl.delete_texture(t);
        }
        if self.target.is_none() {
            let f = gl.create_framebuffer()?;
            let t = gl.create_texture()?;
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(t));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RG16F as i32,
                resolution[0],
                resolution[1],
                0,
                glow::RG,
                glow::FLOAT,
                glow::PixelUnpackData::Slice(None),
            );
            for p in [glow::TEXTURE_MIN_FILTER, glow::TEXTURE_MAG_FILTER] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, p, glow::NEAREST as i32);
            }
            for p in [glow::TEXTURE_WRAP_S, glow::TEXTURE_WRAP_T] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, p, glow::CLAMP_TO_EDGE as i32);
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(f));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(t),
                0,
            );
            gl.draw_buffer(glow::COLOR_ATTACHMENT0);
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            gl.bind_framebuffer(glow::FRAMEBUFFER, old);
            if status != glow::FRAMEBUFFER_COMPLETE {
                gl.delete_framebuffer(f);
                gl.delete_texture(t);
                return Err("Contact visibility target incomplete".into());
            }
            self.target = Some((f, t, resolution));
        }
        let (f, t, _) = self.target.unwrap();
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(f));
        gl.viewport(0, 0, resolution[0], resolution[1]);
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::CULL_FACE);
        gl.disable(glow::BLEND);
        gl.depth_mask(false);
        gl.use_program(Some(self.program));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_sampler(0, None);
        gl.bind_texture(glow::TEXTURE_2D, Some(depth));
        gl.uniform_1_i32(
            gl.get_uniform_location(self.program, "depth_image")
                .as_ref(),
            0,
        );
        gl.uniform_4_f32(
            gl.get_uniform_location(self.program, "projection_a")
                .as_ref(),
            projection[0],
            projection[5],
            if projection[11] != 0. {
                projection[8]
            } else {
                projection[12]
            },
            if projection[11] != 0. {
                projection[9]
            } else {
                projection[13]
            },
        );
        gl.uniform_4_f32(
            gl.get_uniform_location(self.program, "projection_b")
                .as_ref(),
            projection[10],
            projection[14],
            projection[11],
            0.,
        );
        gl.uniform_4_f32(
            gl.get_uniform_location(self.program, "scene_rect").as_ref(),
            rect[0] as f32 / size[0] as f32,
            rect[1] as f32 / size[1] as f32,
            rect[2] as f32 / size[0] as f32,
            rect[3] as f32 / size[1] as f32,
        );
        gl.uniform_2_f32(
            gl.get_uniform_location(self.program, "settings").as_ref(),
            strength,
            radius,
        );
        gl.bind_vertex_array(Some(self.vao));
        gl.draw_arrays(glow::TRIANGLES, 0, 3);
        gl.bind_vertex_array(None);
        gl.use_program(None);
        gl.depth_mask(true);
        gl.bind_framebuffer(glow::FRAMEBUFFER, old);
        gl.viewport(rect[0], rect[1], rect[2], rect[3]);
        gl.enable(glow::SCISSOR_TEST);
        gl.scissor(rect[0], rect[1], rect[2], rect[3]);
        Ok(t)
    }
    pub unsafe fn destroy(self, gl: &glow::Context) {
        if let Some((f, t, _)) = self.target {
            gl.delete_framebuffer(f);
            gl.delete_texture(t);
        }
        gl.delete_vertex_array(self.vao);
        gl.delete_program(self.program);
    }
}
