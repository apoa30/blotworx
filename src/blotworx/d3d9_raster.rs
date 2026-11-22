//! D3D9 device-state ownership and core GL submission for original effects.
//! The initial snapshot belongs to the render owner, not an effect pass:
//! omitted assignments inherit device state rather than invented defaults.
use anyhow::{ensure, Context, Result};
use glow::{self, HasContext};
use std::collections::BTreeMap;
use super::d3d9_state::{AlphaTest, State};

const REQUIRED:[u32;27]=[7,14,15,19,20,22,23,24,25,27,52,53,54,55,56,57,58,59,168,171,175,194,195,206,207,208,209];

#[derive(Debug,Clone)]
pub(super) struct Device {values:BTreeMap<u32,u32>}
/// A render-target-specific conversion must be supplied for nonzero depth
/// bias. D3D's normalized bias cannot be passed as OpenGL's depth-buffer units.
pub(super) struct Target {
    pub front_face:u32,pub depth_bias_units:Option<f32>,
    /// Active render-target extent, independently of the camera viewport.
    pub extent:[u32;2],
}
struct Plan {
    depth:bool,depth_write:bool,depth_func:u32,alpha:AlphaTest,
    blend:bool,source:u32,destination:u32,alpha_source:u32,alpha_destination:u32,
    color_op:u32,alpha_op:u32,cull:u32,stencil:bool,
    stencil_fail:u32,stencil_zfail:u32,stencil_pass:u32,stencil_func:u32,
    stencil_ref:i32,stencil_read:u32,stencil_write:u32,color_mask:u32,
    slope:f32,bias:f32,srgb_write:bool,
}
fn comparison(value:u32)->Result<u32> {Ok(match value {
    1=>glow::NEVER,2=>glow::LESS,3=>glow::EQUAL,4=>glow::LEQUAL,
    5=>glow::GREATER,6=>glow::NOTEQUAL,7=>glow::GEQUAL,8=>glow::ALWAYS,
    _=>anyhow::bail!("Invalid D3D comparison {value}"),
})}
fn factor(value:u32)->Result<u32> {Ok(match value {
    1=>glow::ZERO,2=>glow::ONE,3=>glow::SRC_COLOR,4=>glow::ONE_MINUS_SRC_COLOR,
    5=>glow::SRC_ALPHA,6=>glow::ONE_MINUS_SRC_ALPHA,7=>glow::DST_ALPHA,
    8=>glow::ONE_MINUS_DST_ALPHA,9=>glow::DST_COLOR,10=>glow::ONE_MINUS_DST_COLOR,
    11=>glow::SRC_ALPHA_SATURATE,
    _=>anyhow::bail!("D3D blend factor {value} requires its adapter"),
})}
fn operation(value:u32)->Result<u32> {Ok(match value {
    1=>glow::FUNC_ADD,2=>glow::FUNC_SUBTRACT,3=>glow::FUNC_REVERSE_SUBTRACT,
    4=>glow::MIN,5=>glow::MAX,_=>anyhow::bail!("Invalid D3D blend operation {value}"),
})}
fn stencil(value:u32)->Result<u32> {Ok(match value {
    1=>glow::KEEP,2=>glow::ZERO,3=>glow::REPLACE,4=>glow::INCR,
    5=>glow::DECR,6=>glow::INVERT,7=>glow::INCR_WRAP,8=>glow::DECR_WRAP,
    _=>anyhow::bail!("Invalid D3D stencil operation {value}"),
})}
impl Device {
    /// Original executable initialization table. This is an initial device
    /// state, NOT a replacement for later renderer/material/pass writes.
    pub fn native_initial()->Result<Self> {
        Self::from_snapshot(&[State {state:7,value:0},State {state:14,value:1},State {state:15,value:0},State {state:19,value:2},State {state:20,value:1},State {state:22,value:3},State {state:23,value:4},State {state:24,value:0},State {state:25,value:8},State {state:27,value:0},State {state:52,value:0},State {state:53,value:1},State {state:54,value:1},State {state:55,value:1},State {state:56,value:8},State {state:57,value:0},State {state:58,value:4294967295},State {state:59,value:4294967295},State {state:168,value:15},State {state:171,value:1},State {state:175,value:0},State {state:194,value:0},State {state:195,value:0},State {state:206,value:0},State {state:207,value:2},State {state:208,value:1},State {state:209,value:1}])
    }
    pub fn alpha_test(&self)->Result<AlphaTest>{Ok(self.plan()?.alpha)}
    pub fn from_snapshot(snapshot:&[State])->Result<Self> {
        let values=snapshot.iter().map(|s|(s.state,s.value)).collect();
        let device=Self {values};device.plan()?;Ok(device)
    }
    /// Validation precedes publication: rejected writes cannot partially
    /// corrupt the inherited state for the next pass.
    pub fn write(&mut self,writes:&[State])->Result<()> {
        let mut next=self.clone();
        for state in writes {ensure!(REQUIRED.contains(&state.state),"Unowned D3D raster state {}",state.state);next.values.insert(state.state,state.value);}
        next.plan()?;*self=next;Ok(())
    }
    fn plan(&self)->Result<Plan> {
        for state in REQUIRED {ensure!(self.values.contains_key(&state),"Missing initial D3D raster state {state}");}
        let v=|state|self.values[&state];
        let flag=|state|->Result<bool>{let value=v(state);ensure!(value<=1,"D3D state {state} is not boolean");Ok(value!=0)};
        let depth=match v(7){0=>false,1=>true,_=>anyhow::bail!("D3D W-buffer requires its depth adapter")};
        ensure!((1..=3).contains(&v(22)),"Invalid D3D cull mode");
        ensure!(v(24)<=255,"D3D alpha reference outside byte range");comparison(v(25))?;
        ensure!(v(168)&!15==0,"Invalid D3D color write mask");
        let slope=f32::from_bits(v(175));let bias=f32::from_bits(v(195));
        ensure!(slope.is_finite()&&bias.is_finite(),"Nonfinite D3D depth bias");
        let source=factor(v(19))?;let destination=factor(v(20))?;let color_op=operation(v(171))?;
        let separate=flag(206)?;
        Ok(Plan {depth,depth_write:flag(14)?,depth_func:comparison(v(23))?,
            alpha:AlphaTest {enabled:flag(15)?,comparison:v(25),reference:v(24) as u8},
            blend:flag(27)?,source,destination,
            alpha_source:if separate {factor(v(207))?}else{source},
            alpha_destination:if separate {factor(v(208))?}else{destination},
            color_op,alpha_op:if separate {operation(v(209))?}else{color_op},cull:v(22),
            stencil:flag(52)?,stencil_fail:stencil(v(53))?,stencil_zfail:stencil(v(54))?,
            stencil_pass:stencil(v(55))?,stencil_func:comparison(v(56))?,stencil_ref:i32::try_from(v(57)).context("Unsigned stencil reference requires target precision handling")?,
            stencil_read:v(58),stencil_write:v(59),color_mask:v(168),slope,bias,srgb_write:flag(194)?})
    }
    /// Caller owns a current context and a compatible render target. Submit
    /// the entire owned state because shared UI/viewport draws can change GL.
    /// Returns the shader-side alpha operation, which must also be uploaded.
    pub fn validate_target(&self,target:&Target)->Result<()> {self.units(target)?;Ok(())}
    fn units(&self,target:&Target)->Result<f32> {
        let p=self.plan()?;
        ensure!(matches!(target.front_face,glow::CW|glow::CCW),"Invalid render-target winding");
        let units=if p.bias==0. {0.}else {
            let scale=target.depth_bias_units.context("Nonzero D3D depth bias requires target conversion")?;
            ensure!(scale.is_finite()&&scale>0.,"Invalid depth-bias unit conversion");
            let units=p.bias*scale;ensure!(units.is_finite(),"Depth-bias conversion overflow");units
        };
        Ok(units)
    }
    pub unsafe fn submit(&self,gl:&glow::Context,target:&Target)->Result<AlphaTest> {
        let p=self.plan()?;let units=self.units(target)?;
        let toggle=|cap,on|if on {gl.enable(cap)}else{gl.disable(cap)};
        toggle(glow::FRAMEBUFFER_SRGB,p.srgb_write);
        toggle(glow::DEPTH_TEST,p.depth);gl.depth_mask(p.depth_write);gl.depth_func(p.depth_func);
        toggle(glow::BLEND,p.blend);gl.blend_func_separate(p.source,p.destination,p.alpha_source,p.alpha_destination);
        gl.blend_equation_separate(p.color_op,p.alpha_op);
        gl.front_face(target.front_face);toggle(glow::CULL_FACE,p.cull!=1);
        // With CCW treated as front, CW culling removes back faces. The
        // target winding handles projection/framebuffer orientation changes.
        gl.cull_face(if p.cull==3 {glow::FRONT}else{glow::BACK});
        toggle(glow::STENCIL_TEST,p.stencil);gl.stencil_func(p.stencil_func,p.stencil_ref,p.stencil_read);
        gl.stencil_mask(p.stencil_write);gl.stencil_op(p.stencil_fail,p.stencil_zfail,p.stencil_pass);
        gl.color_mask(p.color_mask&1!=0,p.color_mask&2!=0,p.color_mask&4!=0,p.color_mask&8!=0);
        toggle(glow::POLYGON_OFFSET_FILL,p.slope!=0.||p.bias!=0.);gl.polygon_offset(p.slope,units);
        Ok(p.alpha)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn baseline()->Vec<State> {
        // Explicit fixture snapshot; the production owner must supply its own.
        [(7,0),(14,1),(15,0),(19,2),(20,1),(22,3),(23,4),(24,0),(25,8),
            (27,0),(52,0),(53,1),(54,1),(55,1),(56,8),(57,0),(58,u32::MAX),
            (59,u32::MAX),(168,15),(171,1),(175,0),(194,0),(195,0),(206,0),(207,2),
            (208,1),(209,1)].into_iter().map(|(state,value)|State {state,value}).collect()
    }
    #[test]
    fn original_executable_initial_table()->Result<()> {
        let reference:serde_json::Value=serde_json::from_str(include_str!("../../tools/blotworx/fixtures/pc_initial_device_tables.json"))?;
        let states=reference["tables"]["raster"]["states"].as_array().unwrap().iter().map(|s|State {state:s["state"].as_u64().unwrap() as u32,value:s["value"].as_u64().unwrap() as u32}).collect::<Vec<_>>();
        let device=Device::from_snapshot(&states)?;let initial=Device::native_initial()?;
        for state in REQUIRED {assert_eq!(device.values[&state],initial.values[&state],"state {state}");}
        assert!(!initial.plan()?.depth);
        Ok(())
    }
    #[test]
    fn inherited_state_and_rejected_updates_are_atomic()->Result<()> {
        let mut device=Device::from_snapshot(&baseline())?;
        device.write(&[State {state:27,value:1},State {state:19,value:5}])?;
        device.write(&[State {state:14,value:0}])?;
        let before=device.values.clone();assert!(device.plan()?.blend);
        assert_eq!(device.plan()?.source,glow::SRC_ALPHA);
        assert!(device.write(&[State {state:27,value:0},State {state:25,value:9}]).is_err());
        assert_eq!(device.values,before);
        assert!(device.write(&[State {state:7,value:2}]).is_err());
        assert_eq!(device.values,before);Ok(())
    }
    #[test]
    fn incomplete_snapshot_and_nonfinite_bias_are_rejected()->Result<()> {
        let mut snapshot=baseline();snapshot.pop();assert!(Device::from_snapshot(&snapshot).is_err());
        let mut device=Device::from_snapshot(&baseline())?;
        assert!(device.write(&[State {state:175,value:f32::NAN.to_bits()}]).is_err());
        assert!(device.write(&[State {state:195,value:f32::INFINITY.to_bits()}]).is_err());Ok(())
    }
    #[test]
    fn alpha_blending_inherits_color_operation_until_separate_enabled()->Result<()> {
        let mut device=Device::from_snapshot(&baseline())?;
        device.write(&[State {state:19,value:5},State {state:20,value:6},State {state:171,value:2}])?;
        let p=device.plan()?;assert_eq!(p.alpha_source,glow::SRC_ALPHA);assert_eq!(p.alpha_destination,glow::ONE_MINUS_SRC_ALPHA);assert_eq!(p.alpha_op,glow::FUNC_SUBTRACT);
        device.write(&[State {state:206,value:1}])?;
        let p=device.plan()?;assert_eq!(p.alpha_source,glow::ONE);assert_eq!(p.alpha_destination,glow::ZERO);assert_eq!(p.alpha_op,glow::FUNC_ADD);Ok(())
    }
}
