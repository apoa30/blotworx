//! Original D3D sampler state, independent of NIF texture flags and images.
//! sRGB decode belongs to the image view; it is not a GLSL gamma approximation.
use anyhow::{ensure,Context,Result};
use glow::{self,HasContext};
use super::d3d9_sampler::State;

#[derive(Debug,Clone,PartialEq,Eq,PartialOrd,Ord)]
pub(super) struct Device {values:[u32;13]}
struct Plan {address:[u32;3],border:[f32;4],min:u32,mag:u32,bias:f32,anisotropy:f32,srgb:bool}
fn address(value:u32)->Result<u32> {Ok(match value {
    1=>glow::REPEAT,2=>glow::MIRRORED_REPEAT,3=>glow::CLAMP_TO_EDGE,4=>glow::CLAMP_TO_BORDER,
    _=>anyhow::bail!("D3D texture address mode {value} requires its adapter"),
})}
impl Device {
    /// Original executable initialization table. This is an initial device
    /// state, NOT a replacement for later renderer/material/pass writes.
    pub fn native_initial()->Result<Self> {
        Self::from_snapshot(&[State {state:1,value:1},State {state:2,value:1},State {state:3,value:1},State {state:4,value:0},State {state:5,value:1},State {state:6,value:1},State {state:7,value:0},State {state:8,value:0},State {state:9,value:0},State {state:10,value:1},State {state:11,value:0},State {state:12,value:0},State {state:13,value:256}])
    }
    pub fn needs_srgb(&self)->Result<bool>{Ok(self.plan()?.srgb)}
    pub fn from_snapshot(snapshot:&[State])->Result<Self> {
        let mut values=[0;13];let mut found=[false;13];
        for state in snapshot {ensure!((1..=13).contains(&state.state),"Unknown D3D sampler state");let at=state.state as usize-1;values[at]=state.value;found[at]=true;}
        ensure!(found.iter().all(|v|*v),"Incomplete initial D3D sampler snapshot");
        let result=Self {values};result.plan()?;Ok(result)
    }
    pub fn write(&mut self,writes:&[State])->Result<()> {
        let mut next=self.clone();
        for state in writes {ensure!((1..=13).contains(&state.state),"Unknown D3D sampler state");next.values[state.state as usize-1]=state.value;}
        next.plan()?;*self=next;Ok(())
    }
    fn plan(&self)->Result<Plan> {
        let v=|id:usize|self.values[id-1];
        let min=match v(6){1=>glow::NEAREST,2|3=>glow::LINEAR,_=>anyhow::bail!("D3D minimum filter requires its adapter")};
        let min=match (min,v(7)) {
            (value,0)=>value,(glow::NEAREST,1)=>glow::NEAREST_MIPMAP_NEAREST,
            (glow::NEAREST,2)=>glow::NEAREST_MIPMAP_LINEAR,
            (glow::LINEAR,1)=>glow::LINEAR_MIPMAP_NEAREST,
            (glow::LINEAR,2)=>glow::LINEAR_MIPMAP_LINEAR,
            _=>anyhow::bail!("D3D mip filter requires its adapter"),
        };
        let mag=match v(5){1=>glow::NEAREST,2=>glow::LINEAR,_=>anyhow::bail!("D3D magnification filter requires its adapter")};
        let bias=f32::from_bits(v(8));ensure!(bias.is_finite(),"Nonfinite D3D texture LOD bias");
        ensure!(v(9)==0,"D3D maximum mip level requires a restricted image view");
        ensure!(v(10)>0,"Invalid D3D anisotropy");ensure!(v(11)<=1,"Invalid D3D sRGB sampler flag");
        ensure!(v(12)==0,"D3D element sampler state requires its adapter");
        // DMAPOFFSET (state13) belongs exclusively to D3DDMAPSAMPLER.
        // This binding accepts only ordinary PS/VS sampler banks, so that
        // state is dormant here. Native initialization is 256, not zero.
        // Keep its value for state inheritance; do not reinterpret it as a
        // texture coordinate offset or reject a valid ordinary sampler.
        let color=v(4);
        Ok(Plan {address:[address(v(1))?,address(v(2))?,address(v(3))?],
            border:[(color>>16)&255,(color>>8)&255,color&255,color>>24].map(|v|v as f32/255.),
            min,mag,bias,anisotropy:if v(6)==3 {v(10) as f32}else{1.},srgb:v(11)!=0})
    }
}
/// The image owner must supply a compatible native-format view. A second
/// upload/view can share its original bytes; texture-wide decode must not be
/// toggled on an image simultaneously used by linear and sRGB samplers.
#[derive(Debug,Clone,Copy)]
pub(super) struct ImageView {pub target:u32,pub linear:glow::Texture,pub srgb:Option<glow::Texture>}
pub(super) struct Binding {pub handle:glow::Sampler,target:u32,srgb:bool}
impl Binding {
    /// Verified D3D9 pixel-shader tex2D(NULL) is (0,0,0,1). Core GL's
    /// incomplete default 2D texture has the same result; no dummy image owns it.
    pub unsafe fn bind_null_2d(&self,gl:&glow::Context,unit:u32)->Result<()> {
        ensure!(self.target==glow::TEXTURE_2D&&unit<16,"Null texture requires a verified pixel 2D sampler");
        gl.active_texture(glow::TEXTURE0+unit);gl.bind_texture(glow::TEXTURE_2D,None);gl.bind_sampler(unit,Some(self.handle));Ok(())
    }
    pub unsafe fn new(gl:&glow::Context,device:&Device,dimension:i32)->Result<Self> {
        let plan=device.plan()?;
        let target=match dimension {0=>glow::TEXTURE_2D,1=>glow::TEXTURE_CUBE_MAP,2=>glow::TEXTURE_3D,_=>anyhow::bail!("Unknown original sampler dimensionality")};
        let anisotropic=gl.supported_extensions().contains("GL_EXT_texture_filter_anisotropic")||gl.supported_extensions().contains("GL_ARB_texture_filter_anisotropic");
        if plan.anisotropy>1. {
            ensure!(anisotropic,"Original material requires anisotropic texture sampling");
            ensure!(plan.anisotropy<=gl.get_parameter_f32(0x84ff),"Original anisotropy exceeds device capability");
        }
        ensure!(plan.bias.abs()<=gl.get_parameter_f32(glow::MAX_TEXTURE_LOD_BIAS),"Original LOD bias exceeds device capability");
        let handle=gl.create_sampler().map_err(anyhow::Error::msg)?;
        for (name,value) in [(glow::TEXTURE_WRAP_S,plan.address[0]),(glow::TEXTURE_WRAP_T,plan.address[1]),(glow::TEXTURE_WRAP_R,plan.address[2]),(glow::TEXTURE_MIN_FILTER,plan.min),(glow::TEXTURE_MAG_FILTER,plan.mag)] {gl.sampler_parameter_i32(handle,name,value as i32);}
        gl.sampler_parameter_f32_slice(handle,glow::TEXTURE_BORDER_COLOR,&plan.border);
        gl.sampler_parameter_f32(handle,glow::TEXTURE_LOD_BIAS,plan.bias);
        gl.sampler_parameter_f32(handle,glow::TEXTURE_MIN_LOD,-1000.);
        gl.sampler_parameter_f32(handle,glow::TEXTURE_MAX_LOD,1000.);
        gl.sampler_parameter_i32(handle,glow::TEXTURE_COMPARE_MODE,glow::NONE as i32);
        if anisotropic {gl.sampler_parameter_f32(handle,0x84fe,plan.anisotropy);}
        Ok(Self {handle,target,srgb:plan.srgb})
    }
    /// Unit comes from the linked original shader (PS0..15, VS16..19), not
    /// the declaration's 256 marker or a NIF map ID. Missing resources must be
    /// resolved by their owner rather than silently sampling GL texture zero.
    pub unsafe fn bind(&self,gl:&glow::Context,unit:u32,image:&ImageView)->Result<()> {
        ensure!(unit<20,"Original shader sampler outside stage banks");
        ensure!(image.target==self.target,"Original sampler/image dimensionality differs");
        let texture=if self.srgb {image.srgb.context("Original sRGB texture view is absent")?}else{image.linear};
        gl.active_texture(glow::TEXTURE0+unit);gl.bind_texture(self.target,Some(texture));gl.bind_sampler(unit,Some(self.handle));Ok(())
    }
    pub unsafe fn destroy(self,gl:&glow::Context){gl.delete_sampler(self.handle);}
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture()->Vec<State>{[1,1,1,0,1,1,0,0,0,1,0,0,0].into_iter().enumerate().map(|(i,value)|State {state:i as u32+1,value}).collect()}
    #[test]
    fn original_executable_initial_table()->Result<()> {
        let reference:serde_json::Value=serde_json::from_str(include_str!("../../tools/blotworx/fixtures/pc_initial_device_tables.json"))?;
        let states=reference["tables"]["sampler"]["states"].as_array().unwrap().iter().map(|s|State {state:s["state"].as_u64().unwrap() as u32,value:s["value"].as_u64().unwrap() as u32}).collect::<Vec<_>>();
        let device=Device::from_snapshot(&states)?;let initial=Device::native_initial()?;
        assert_eq!(device.values,initial.values);assert_eq!(device.values[12],256);
        assert_eq!(device.plan()?.min,glow::NEAREST);
        Ok(())
    }
    #[test]
    fn mip_filters_and_inheritance_follow_original_enums()->Result<()> {
        let mut device=Device::from_snapshot(&fixture())?;
        assert_eq!(device.plan()?.min,glow::NEAREST);
        device.write(&[State {state:6,value:2},State {state:7,value:1}])?;
        assert_eq!(device.plan()?.min,glow::LINEAR_MIPMAP_NEAREST);
        device.write(&[State {state:7,value:2}])?;assert_eq!(device.plan()?.min,glow::LINEAR_MIPMAP_LINEAR);Ok(())
    }
    #[test]
    fn border_color_is_argb_and_srgb_is_independent()->Result<()> {
        let mut device=Device::from_snapshot(&fixture())?;
        device.write(&[State {state:4,value:0x80402010},State {state:11,value:1}])?;
        let p=device.plan()?;assert_eq!(p.border,[64.,32.,16.,128.].map(|v|v/255.));assert!(p.srgb);Ok(())
    }
    #[test]
    fn unsupported_updates_preserve_inherited_sampler_state()->Result<()> {
        let mut device=Device::from_snapshot(&fixture())?;let before=device.values;
        assert!(device.write(&[State {state:1,value:2},State {state:9,value:1}]).is_err());assert_eq!(device.values,before);
        assert!(Device::from_snapshot(&fixture()[1..]).is_err());
        assert!(device.write(&[State {state:8,value:f32::NAN.to_bits()}]).is_err());Ok(())
    }
}
