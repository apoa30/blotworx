//! Shared OpenGL submission boundary for original PC effect programs.
//! Owns linked interfaces; scene resources, pass states and vertex streams
//! must still be supplied by the renderer before any draw is submitted.
use anyhow::{ensure,Result};
use glow::{self,HasContext};
use std::sync::Arc;
use super::{d3d9_shader::{PassConstants,TranslatedPass},d3d9_translate::{PackedUniforms,Translation}};
#[derive(Debug)]
pub(super) struct Attribute {pub usage:i32,pub index:i32,pub location:u32}
#[derive(Debug)]
pub(super) struct Sampler {pub register:u32,pub unit:u32,pub dimension:i32,pub location:Option<glow::UniformLocation>}
struct StageLocations {floats:Option<glow::UniformLocation>,integers:Option<glow::UniformLocation>,booleans:Option<glow::UniformLocation>}
#[derive(Default)]
pub(super) struct Scratch {pub vertex:PackedUniforms,pub pixel:PackedUniforms}
/// Explicit framebuffer convention. Leaving vposFlip at its GL default zero
/// collapses shader screen-coordinate reads to zero on the vertical axis.
pub(super) struct RasterCoordinates {pub vpos_flip:[f32;2],pub clip_y:f32,pub d3d_depth:bool}
pub(super) struct Program {
    pub handle:glow::Program,pub attributes:Vec<Attribute>,
    pub vertex_samplers:Vec<Sampler>,pub pixel_samplers:Vec<Sampler>,
    vertex:StageLocations,pixel:StageLocations,pass:Arc<TranslatedPass>,
    vpos_flip:Option<glow::UniformLocation>,clip_y_flip:Option<glow::UniformLocation>,
    alpha_test:Option<glow::UniformLocation>,alpha_reference:Option<glow::UniformLocation>,
    clip_depth:Option<glow::UniformLocation>,
}
fn vertex_source(vertex:&Translation)->Result<String>{
    let source=vertex.desktop_source()?;
    ensure!(source.matches("void main()").count()==1,"Original vertex entry point is ambiguous");
    let mut result=source.replacen("void main()","void bw_original_vertex_main()",1);
    result.push_str("\nuniform int bw_d3d_clip_depth;\nvoid main(){bw_original_vertex_main();if(bw_d3d_clip_depth!=0)gl_Position.z=2.0*gl_Position.z-gl_Position.w;}\n");
    Ok(result)
}
fn fragment_source(pixel:&Translation)->Result<String>{
    let source=pixel.desktop_source()?;
    let Some(color)=pixel.outputs.iter().find(|output|output.kind==10&&output.index==0).and_then(|output|output.name.as_deref()) else{return Ok(source)};
    ensure!(source.matches("void main()").count()==1,"Original fragment entry point is ambiguous");
    // Run the original program first, including its discard/depth writes.
    // The original effect's ALPHATEST state then tests COLOR0, not texture alpha.
    // No quantization is added here: hardware alpha precision remains a frame
    // comparison requirement before claiming D3D9 raster parity.
    let mut result=source.replacen("void main()","void bw_original_main()",1);
    result.push_str(&format!(r#"
uniform int bw_alpha_comparison;
uniform float bw_alpha_reference;
void main() {{
    bw_original_main();
    float a = {color}.a;
    int c = bw_alpha_comparison;
    bool keep = c == 0 || c == 8 ||
        (c == 2 && a < bw_alpha_reference) ||
        (c == 3 && a == bw_alpha_reference) ||
        (c == 4 && a <= bw_alpha_reference) ||
        (c == 5 && a > bw_alpha_reference) ||
        (c == 6 && a != bw_alpha_reference) ||
        (c == 7 && a >= bw_alpha_reference);
    if (!keep) discard;
}}
"#));
    Ok(result)
}
unsafe fn compile(gl:&glow::Context,kind:u32,source:&str)->Result<glow::Shader>{
    let shader=gl.create_shader(kind).map_err(anyhow::Error::msg)?;
    gl.shader_source(shader,source);gl.compile_shader(shader);
    if !gl.get_shader_compile_status(shader){let error=gl.get_shader_info_log(shader);gl.delete_shader(shader);anyhow::bail!("Original effect GPU compilation: {error}");}
    Ok(shader)
}
impl Program {
    /// Caller owns a current GL context and must call destroy on that context.
    pub unsafe fn link(gl:&glow::Context,pass:Arc<TranslatedPass>)->Result<Self>{
        let vertex_source=vertex_source(&pass.vertex)?;let pixel_source=fragment_source(&pass.pixel)?;
        for sampler in &pass.vertex.samplers {ensure!((0..4).contains(&sampler.index),"Vertex sampler outside D3D9 stage bank");}
        for sampler in &pass.pixel.samplers {ensure!((0..16).contains(&sampler.index),"Pixel sampler outside D3D9 stage bank");}
        let vertex=compile(gl,glow::VERTEX_SHADER,&vertex_source)?;
        let pixel=match compile(gl,glow::FRAGMENT_SHADER,&pixel_source){Ok(shader)=>shader,Err(error)=>{gl.delete_shader(vertex);return Err(error);}};
        let handle=match gl.create_program(){Ok(handle)=>handle,Err(error)=>{gl.delete_shader(vertex);gl.delete_shader(pixel);return Err(anyhow::anyhow!(error));}};
        gl.attach_shader(handle,vertex);gl.attach_shader(handle,pixel);gl.link_program(handle);
        gl.detach_shader(handle,vertex);gl.detach_shader(handle,pixel);gl.delete_shader(vertex);gl.delete_shader(pixel);
        if !gl.get_program_link_status(handle){let error=gl.get_program_info_log(handle);gl.delete_program(handle);anyhow::bail!("Original effect GPU linking: {error}");}
        let locations=|prefix:&str|StageLocations {
            floats:gl.get_uniform_location(handle,&format!("{prefix}_uniforms_vec4")),
            integers:gl.get_uniform_location(handle,&format!("{prefix}_uniforms_ivec4")),
            booleans:gl.get_uniform_location(handle,&format!("{prefix}_uniforms_bool")),
        };
        let samplers=|stage:&Translation,base:u32|stage.samplers.iter().map(|s|Sampler {register:s.index as u32,unit:base+s.index as u32,dimension:s.kind,location:s.name.as_deref().and_then(|name|gl.get_uniform_location(handle,name))}).collect();
        let attributes=pass.vertex.inputs.iter().filter_map(|input|Some(Attribute {usage:input.kind,index:input.index,location:gl.get_attrib_location(handle,input.name.as_deref()?)?})).collect();
        Ok(Self {handle,attributes,vertex:locations("vs"),pixel:locations("ps"),vertex_samplers:samplers(&pass.vertex,16),pixel_samplers:samplers(&pass.pixel,0),vpos_flip:gl.get_uniform_location(handle,"vposFlip"),clip_y_flip:gl.get_uniform_location(handle,"vpFlip"),alpha_test:gl.get_uniform_location(handle,"bw_alpha_comparison"),alpha_reference:gl.get_uniform_location(handle,"bw_alpha_reference"),clip_depth:gl.get_uniform_location(handle,"bw_d3d_clip_depth"),pass})
    }
    /// Requires this program to be current. Call after inheriting/applying the
    /// pass's device states, including passes which turn alpha testing off.
    pub fn validate_alpha_test(&self,state:&super::d3d9_state::AlphaTest)->Result<()> {
        ensure!((1..=8).contains(&state.comparison),"Invalid alpha comparison");
        ensure!(!state.enabled||self.alpha_test.is_some(),"Alpha-tested shader has no COLOR0 output");
        Ok(())
    }
    pub unsafe fn upload_alpha_test(&self,gl:&glow::Context,state:&super::d3d9_state::AlphaTest)->Result<()> {
        self.validate_alpha_test(state)?;
        gl.uniform_1_i32(self.alpha_test.as_ref(),if state.enabled {state.comparison as i32}else{0});
        gl.uniform_1_f32(self.alpha_reference.as_ref(),state.reference as f32/255.);
        Ok(())
    }
    /// Only uniforms and sampler-unit routing are submitted here. Binding the
    /// actual textures, geometry and original pass states is a separate step.
    pub unsafe fn upload(&self,gl:&glow::Context,banks:&PassConstants,raster:&RasterCoordinates,scratch:&mut Scratch)->Result<()> {
        ensure!(raster.vpos_flip.iter().chain([&raster.clip_y]).all(|v|v.is_finite())&&matches!(raster.vpos_flip[0],-1.|1.)&&matches!(raster.clip_y,-1.|1.),"Invalid effect raster-coordinate convention");
        self.pass.vertex.pack_uniforms(&banks.vertex.floats,&banks.vertex.integers,&banks.vertex.booleans,&mut scratch.vertex)?;
        self.pass.pixel.pack_uniforms(&banks.pixel.floats,&banks.pixel.integers,&banks.pixel.booleans,&mut scratch.pixel)?;
        gl.use_program(Some(self.handle));
        for (locations,values) in [(&self.vertex,&scratch.vertex),(&self.pixel,&scratch.pixel)] {
            if !values.floats.is_empty(){gl.uniform_4_f32_slice(locations.floats.as_ref(),&values.floats);}
            if !values.integers.is_empty(){gl.uniform_4_i32_slice(locations.integers.as_ref(),&values.integers);}
            if !values.booleans.is_empty(){gl.uniform_1_i32_slice(locations.booleans.as_ref(),&values.booleans);}
        }
        for sampler in self.vertex_samplers.iter().chain(&self.pixel_samplers){gl.uniform_1_i32(sampler.location.as_ref(),sampler.unit as i32);}
        gl.uniform_2_f32(self.vpos_flip.as_ref(),raster.vpos_flip[0],raster.vpos_flip[1]);
        gl.uniform_1_f32(self.clip_y_flip.as_ref(),raster.clip_y);
        gl.uniform_1_i32(self.clip_depth.as_ref(),raster.d3d_depth as i32);
        Ok(())
    }
    pub unsafe fn destroy(self,gl:&glow::Context){gl.delete_program(self.handle);}
}
