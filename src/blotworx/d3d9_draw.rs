//! Original PC effects at the shared viewport draw boundary. No scene defaults
//! are invented here: the scene owner must admit numeric and device inputs.
use anyhow::{ensure,Context,Result};
use eframe::glow::{self,HasContext};
use std::{collections::{BTreeMap,BTreeSet},sync::{Arc,Weak}};
use crate::render_support::{native_material::{Backend,Frame},nif::MaterialBindings,viewport3d::{GpuDrawCommand,GpuMeshHandle}};
use super::{d3d9_gpu as gpu,d3d9_images as images,d3d9_material::{Resources,Texture},d3d9_raster as raster,d3d9_sampling as sampling,d3d9_shader::{Effect,PassSamplers,TranslatedPass},d3d9_state,d3d9_vertices};

pub(super) type Materials=BTreeMap<usize,(Weak<MaterialBindings>,Result<Arc<Resources>,String>)>;
#[derive(Debug,Clone)]
pub(super) enum InputImage {
    /// Explicit known absence; never used to hide unresolved scene ownership.
    Null2d,
    Asset(Arc<images::Source>),
    /// Borrowed from the current scene pass (for example a projected shadow).
    /// The scene owner keeps it alive for this draw; this cache never deletes it.
    Runtime(sampling::ImageView),
}
#[derive(Debug,Clone)]
pub(super) struct AssignedTextureEffect {pub image:Option<InputImage>,pub projection:Option<super::d3d9_texture_effects::Projection>}
pub(super) struct Inputs {
    pub texture_precision:Option<super::pc_float::Precision>,
    pub lighting:Option<super::d3d9_globals::Lighting>,
    pub sh:Option<Arc<super::d3d9_sh::Snapshot>>,
    pub predefined:Option<super::d3d9_predefined::Cache>,
    /// Exact shader Object annotation and optional explicit ObjectIndex.
    /// Missing assignments stay errors; preview light ordering is unrelated.
    pub assigned_lights:Option<Arc<super::d3d9_light::State>>,
    pub object_lights:BTreeMap<(String,Option<i32>),super::d3d9_light::Light>,
    pub parameters:Params,
    /// Only authored Attribute parameters can explicitly retain FXO defaults.
    pub attribute_defaults:BTreeSet<String>,
    pub images:BTreeMap<usize,InputImage>,
    pub texture_effects:Option<Arc<super::d3d9_texture_effects::State<AssignedTextureEffect>>>,
    pub device:raster::Device,
    pub samplers:BTreeMap<(bool,u32),sampling::Device>,
    pub target:raster::Target,pub coordinates:gpu::RasterCoordinates,
    pub precision:super::d3d9_preshader::TemporaryPrecision,
}
/// Shader input values by name; shared (Arc) so cached inputs are copied without allocating.
pub(super) type Params=BTreeMap<Arc<str>,Arc<[u32]>>;
impl Inputs {
    fn put(&mut self,name:&str,words:Vec<u32>) {self.parameters.insert(name.into(),words.into());}
    fn texture_effect(&self,name:&str,index:Option<i32>)->Result<Option<&AssignedTextureEffect>> {
        use super::d3d9_texture_effects::Kind;
        let kind=Kind::named(name).with_context(||format!("Unconnected original scene texture {name}"))?;
        // World surfaces on the game's shaders: no scene texture-effect owner yet means nothing
        // is projected on them (unshadowed), not a reason to drop to the generic fallback.
        let Some(effects)=self.texture_effects.as_ref() else {
            if env_native() {return Ok(None);}
            anyhow::bail!("Missing original scene texture ownership {name}");
        };
        let index=match kind {Kind::Environment|Kind::Fog=>index.unwrap_or(0),_=>index.context("Automatic texture-effect index requires registration-order ownership")?} as u32;
        Ok(effects.select(kind,index).map(|effect|&effect.value))
    }
}
pub(super) trait InputOwner:std::fmt::Debug+Send {
    fn fallback_only(&self)->bool {false}
    fn prepare(&mut self,resources:&Resources,draw:&GpuDrawCommand,frame:&Frame<'_>)->Result<Inputs>;
}
/// Explicit temporary owner while the scene-value adapter is under repair.
/// It never submits a zero-matrix or guessed-lighting original shader draw.
#[derive(Debug)]
pub(super) struct UnboundScene;
impl InputOwner for UnboundScene {
    fn fallback_only(&self)->bool {true}
    fn prepare(&mut self,resources:&Resources,_:&GpuDrawCommand,_:&Frame<'_>)->Result<Inputs> {
        anyhow::bail!("Original scene inputs pending for {}",resources.technique.shader_name)
    }
}
/// Live demo bridge: uses the shared viewport's actual camera/object matrices.
/// Original camera lifecycle, inverse-cache rounding, global-light selection and
/// pass initialization parity remain tracked separately. Missing authored or
/// runtime inputs still reject the draw and use the visible fallback.
#[derive(Debug)]
pub(super) struct ViewportScene {pub profiles:Option<Arc<std::sync::Mutex<super::render_profiles::Registry>>>}
impl InputOwner for ViewportScene {
    fn prepare(&mut self,_:&Resources,draw:&GpuDrawCommand,frame:&Frame<'_>)->Result<Inputs> {
        use super::d3d9_predefined::{self as predefined,Matrix};
        ensure!(frame.viewport[2]>0&&frame.viewport[3]>0,"Empty live renderer viewport");
        let lighting=if let Some(profiles)=&self.profiles {
            let mut state=profiles.lock().map_err(|_|anyhow::anyhow!("Render-profile state lock poisoned"))?;
            state.observe(frame.eye,super::pc_float::Precision::Extended)?;
            // This bridge submits the common viewport's main-scene pass.
            // The separate original toon offscreen/composite pass is pending.
            state.lighting(super::d3d9_globals::ScenePass::Main).cloned()
        }else{None};
        let source=draw.source_attributes.as_ref().context("Live renderer has no mesh-space transform")?.as_ref().map_err(|e|anyhow::anyhow!("{e}"))?;
        let mesh_to_model=std::array::from_fn(|i|source.mesh_to_model[i%4][i/4]);
        let world=predefined::multiply(&mesh_to_model,&draw.trans_mat);
        // GL [-w,+w] depth -> D3D [0,+w]. The shader bridge reverses this
        // at submission, so the common viewport and native draw coincide.
        let mut depth=[0.;16];for i in [0,5,15]{depth[i]=1.;}depth[10]=0.5;depth[14]=0.5;
        let projection=predefined::multiply(frame.projection,&depth);
        let direction=[-frame.view[2],-frame.view[6],-frame.view[10],0.];
        let mut cache=predefined::Cache::compose(world,*frame.view,projection,[frame.eye[0],frame.eye[1],frame.eye[2],1.],direction,[0.,1.,1.,0.])?;
        // Functional live bridge; these are mathematical inverses, not yet
        // a claim of original 32-bit D3DX inverse-cache bit equivalence.
        for kind in [Matrix::World,Matrix::View,Matrix::Projection,Matrix::WorldView,Matrix::ViewProjection,Matrix::WorldViewProjection] {
            let matrix=cache.matrices[&kind].value;cache.set_inverse(kind,live_inverse(matrix)?)?;
        }
        let mut device=raster::Device::native_initial()?;
        device.write(&[d3d9_state::State {state:7,value:1}])?;
        let sampler=sampling::Device::native_initial()?;
        let samplers=(0..16).map(|register|((false,register),sampler.clone())).chain((0..4).map(|register|((true,register),sampler.clone()))).collect();
        // Characters (Gen3_Char*): ambient light from the scene's lights (SH) and an empty
        // projected-shadow set, so they use the game's character shader instead of the fallback.
        // opt-in until the remaining character inputs are found (they still render dark)
        let character=char_native(draw);
        let sh=if character {character_sh(draw,frame)} else {None};
        let texture_effects=character.then(||Arc::new(super::d3d9_texture_effects::State {environment:None,fog:None,projected_shadows:Vec::new(),projected_lights:Vec::new()}));
        Ok(Inputs {texture_precision:Some(super::pc_float::Precision::Extended),lighting,sh,predefined:Some(cache),assigned_lights:None,object_lights:BTreeMap::new(),parameters:BTreeMap::new(),attribute_defaults:BTreeSet::new(),images:BTreeMap::new(),texture_effects,device,samplers,target:raster::Target {front_face:glow::CCW,depth_bias_units:None,extent:[frame.viewport[2] as u32,frame.viewport[3] as u32]},coordinates:gpu::RasterCoordinates {vpos_flip:[-1.,1.],clip_y:1.,d3d_depth:true},precision:super::d3d9_preshader::TemporaryPrecision::Double})
    }
}
/// Spherical-harmonic ambient light for a character: the scene's ambient and directional
/// lights, plus point/spot lights in range, through the native SH accumulator.
/// The scene's runtime SH light records, published by the game loop each frame.
static SCENE_SH:std::sync::Mutex<Option<Arc<Vec<Arc<super::d3d9_sh::LightRecord>>>>>=std::sync::Mutex::new(None);
static EXACT_SH:std::sync::OnceLock<std::sync::atomic::AtomicBool>=std::sync::OnceLock::new();
fn exact_flag()->&'static std::sync::atomic::AtomicBool {EXACT_SH.get_or_init(||std::sync::atomic::AtomicBool::new(std::env::var("BLOTWORX_EM2_SH_EXACT").map_or(true,|v|v!="0")))}
/// Character lighting from the game's own runtime light list (F10 menu, Tools tab).
pub(super) fn exact_sh()->bool {exact_flag().load(std::sync::atomic::Ordering::Relaxed)}
pub(super) fn set_exact_sh(on:bool) {exact_flag().store(on,std::sync::atomic::Ordering::Relaxed);}
pub(super) fn set_scene_sh_lights(records:Vec<Arc<super::d3d9_sh::LightRecord>>) {
    if let Ok(mut slot)=SCENE_SH.lock() {*slot=Some(Arc::new(records));}
}
fn character_sh(draw:&GpuDrawCommand,frame:&Frame<'_>)->Option<Arc<super::d3d9_sh::Snapshot>> {
    use super::d3d9_light::Kind;use super::d3d9_sh::{Contribution,Snapshot};
    let probe=[draw.trans_mat[12],draw.trans_mat[13]+0.6,draw.trans_mat[14]];
    let precision=super::pc_float::Precision::Extended;
    let mut snapshot=Snapshot {coefficients:[[0.;3];9],specular_color:None,specular_direction:None};
    // Exact path (360 0x827a17c0): the runtime SH list sampled at the probe, nothing added.
    // BLOTWORX_EM2_SH_EXACT=0 falls back to the older approximation below.
    let exact=exact_sh();
    let records=SCENE_SH.lock().ok().and_then(|slot|slot.clone());
    if exact { if let Some(records)=records.filter(|r|!r.is_empty()) {
        static LOGGED:std::sync::atomic::AtomicU32=std::sync::atomic::AtomicU32::new(0);
        let log=std::env::var_os("BLOTWORX_SH_LOG").is_some()&&LOGGED.fetch_add(1,std::sync::atomic::Ordering::Relaxed)<2;
        for record in records.iter() {
            let used=snapshot.accumulate_sample(record,probe,None,precision);
            if log {
                let w=record.sample(probe,precision).map(|s|s.weight).unwrap_or(-1.);
                eprintln!("[sh] flags {:#06x} pos {:?} dim {} amb {:?} dif {:?} fall {:?} -> w {w} used {used:?}",record.flags,record.position,record.dimmer,record.ambient,record.diffuse,record.falloff);
            }
        }
        if log {eprintln!("[sh] probe {probe:?} c0/3.5449 = {:?}",snapshot.coefficients[0].map(|v|v/3.544_907_7));}
        return Some(Arc::new(snapshot));
    }}
    for light in frame.lights {
        let dimmer=if light.dimmer.is_finite(){light.dimmer}else{1.};
        let (kind,weight,probe_direction)=match light.light_type {
            0=>(Kind::Ambient,dimmer,[0.,1.,0.]),
            1=>(Kind::Directional,dimmer,[0.,1.,0.]),
            2|3=>{
                let d=[light.position[0]-probe[0],light.position[1]-probe[1],light.position[2]-probe[2]];
                let dist=(d[0]*d[0]+d[1]*d[1]+d[2]*d[2]).sqrt();
                let range=if light.range>0.{light.range}else{10.};
                if dist>=range||dist<1e-4 {continue;}
                let start=light.range_start.max(0.).min(range*0.999);
                let fall=if dist<=start{1.}else{1.-(dist-start)/(range-start)};
                (if light.light_type==2{Kind::Point}else{Kind::Spot},dimmer*fall,[d[0]/dist,d[1]/dist,d[2]/dist])
            }
            _=>continue,
        };
        if !(weight>0.) {continue;}
        let _=snapshot.accumulate(&Contribution {kind,ambient:light.ambient,diffuse:light.diffuse,light_direction:light.direction,probe_direction,weight},precision);
    }
    // The game lights characters mostly from baked light probes, which aren't loaded here. When
    // the scene lights leave a character dim, add a daylight base: a sky-tinted ambient fill and a
    // soft key light from above and in front (BLOTWORX_EM2_CHAR_LIGHT scales it; 0 disables).
    // The character shaders read SH in 0-255 colour units with 2x overbright (output = 2*E/255,
    // so 127.5 is full brightness). Top the ambient up to a daylight level in those units.
    static SCALE:std::sync::OnceLock<f32>=std::sync::OnceLock::new();
    let scale=*SCALE.get_or_init(||std::env::var("BLOTWORX_EM2_CHAR_LIGHT").ok().and_then(|s|s.parse::<f32>().ok()).unwrap_or(1.0));
    const TARGET:f32=72.;
    let level=snapshot.coefficients[0].iter().sum::<f32>()/3./3.544_907_7;
    if scale>0. && level<TARGET*scale {
        let need=(TARGET*scale-level)/TARGET;
        let _=snapshot.accumulate(&Contribution {kind:Kind::Ambient,ambient:[0.62*110.,0.66*110.,0.72*110.],diffuse:[0.;3],light_direction:[0.,-1.,0.],probe_direction:[0.,1.,0.],weight:need},precision);
        let _=snapshot.accumulate(&Contribution {kind:Kind::Directional,ambient:[0.;3],diffuse:[0.75*100.,0.7*100.,0.6*100.],light_direction:[0.35,-0.8,0.45],probe_direction:[0.,1.,0.],weight:need},precision);
    }
    Some(Arc::new(snapshot))
}
/// World surfaces run the game's own shaders (normal, specular, detail maps), taking inputs the
/// scene doesn't supply yet (PointLight0...) from the effect's defaults (a light that contributes
/// nothing). On by default; BLOTWORX_EM2_ENV_NATIVE=0 sends them back to the generic fallback.
fn env_native()->bool {
    static ON:std::sync::OnceLock<bool>=std::sync::OnceLock::new();
    *ON.get_or_init(||std::env::var("BLOTWORX_EM2_ENV_NATIVE").map_or(true,|v|v!="0"))
}
/// env_native for one draw. BLOTWORX_EM2_ENV_NATIVE_SKIP=a,b keeps shaders whose names contain
/// any of those parts on the fallback (diagnostics; nothing is skipped by default).
fn env_native_for(draw:&GpuDrawCommand)->bool {
    static SKIP:std::sync::OnceLock<Vec<String>>=std::sync::OnceLock::new();
    let skip=SKIP.get_or_init(||std::env::var("BLOTWORX_EM2_ENV_NATIVE_SKIP").unwrap_or_default().split(',').filter(|s|!s.is_empty()).map(str::to_owned).collect());
    env_native()&&!draw.material.as_ref().and_then(|m|m.shader.as_deref()).is_some_and(|s|skip.iter().any(|k|s.contains(k.as_str())))
}
/// Gen3_StaticEnv_*_Transparent (distant "sham" scenery, glass...) always runs on the game's own
/// shader: its opacity is alphaMaskMap.g (second UV set) x vertex alpha, while the diffuse alpha is
/// only a gloss term. The generic fallback blends by the diffuse alpha and made these ~99% clear.
fn transparent_env(draw:&GpuDrawCommand)->bool {
    draw.material.as_ref().and_then(|m|m.shader.as_deref()).is_some_and(|s|s.starts_with("Gen3_StaticEnv")&&s.ends_with("_Transparent"))
}
/// Characters (Gen3_Char*) use the game's own shader; BLOTWORX_EM2_CHAR_NATIVE=0 sends them to the fallback.
fn char_native(draw:&GpuDrawCommand)->bool {
    static ON:std::sync::OnceLock<bool>=std::sync::OnceLock::new();
    *ON.get_or_init(||std::env::var("BLOTWORX_EM2_CHAR_NATIVE").map_or(true,|v|v!="0"))&&draw.material.as_ref().and_then(|m|m.shader.as_deref()).is_some_and(|s|s.starts_with("Gen3_Char"))
}
/// A 1x1 white texture (made once), for scene textures that mean "no effect here".
unsafe fn white_view(gl:&glow::Context)->Result<sampling::ImageView> {
    thread_local!{static WHITE:std::cell::Cell<Option<glow::Texture>>=const{std::cell::Cell::new(None)};}
    let texture=match WHITE.with(|w|w.get()) {
        Some(t)=>t,
        None=>{
            let t=gl.create_texture().map_err(anyhow::Error::msg)?;
            gl.bind_texture(glow::TEXTURE_2D,Some(t));
            gl.tex_image_2d(glow::TEXTURE_2D,0,glow::RGBA8 as i32,1,1,0,glow::RGBA,glow::UNSIGNED_BYTE,glow::PixelUnpackData::Slice(Some(&[255u8,255,255,255])));
            gl.tex_parameter_i32(glow::TEXTURE_2D,glow::TEXTURE_MAX_LEVEL,0);
            gl.bind_texture(glow::TEXTURE_2D,None);
            WHITE.with(|w|w.set(Some(t)));t
        }
    };
    Ok(sampling::ImageView {target:glow::TEXTURE_2D,linear:texture,srgb:None})
}
/// A 1x1 flat tangent-space normal (0.5, 0.5, 1), made once, for missing normal maps.
unsafe fn flat_normal_view(gl:&glow::Context)->Result<sampling::ImageView> {
    thread_local!{static FLAT:std::cell::Cell<Option<glow::Texture>>=const{std::cell::Cell::new(None)};}
    let texture=match FLAT.with(|w|w.get()) {
        Some(t)=>t,
        None=>{
            let t=gl.create_texture().map_err(anyhow::Error::msg)?;
            gl.bind_texture(glow::TEXTURE_2D,Some(t));
            gl.tex_image_2d(glow::TEXTURE_2D,0,glow::RGBA8 as i32,1,1,0,glow::RGBA,glow::UNSIGNED_BYTE,glow::PixelUnpackData::Slice(Some(&[128u8,128,255,255])));
            gl.tex_parameter_i32(glow::TEXTURE_2D,glow::TEXTURE_MAX_LEVEL,0);
            gl.bind_texture(glow::TEXTURE_2D,None);
            FLAT.with(|w|w.set(Some(t)));t
        }
    };
    Ok(sampling::ImageView {target:glow::TEXTURE_2D,linear:texture,srgb:None})
}
fn live_inverse(matrix:[f32;16])->Result<[f32;16]> {
    let mut rows=[[0f64;8];4];
    for r in 0..4 {for c in 0..4 {rows[r][c]=matrix[r*4+c] as f64;}rows[r][r+4]=1.;}
    for c in 0..4 {
        let pivot=(c..4).max_by(|&a,&b|rows[a][c].abs().total_cmp(&rows[b][c].abs())).unwrap();
        ensure!(rows[pivot][c]!=0.,"Singular live renderer transform");rows.swap(c,pivot);
        let divisor=rows[c][c];for v in &mut rows[c]{*v/=divisor;}
        for r in 0..4 {if r!=c {let factor=rows[r][c];for k in 0..8 {rows[r][k]-=factor*rows[c][k];}}}
    }
    let result:[f32;16]=std::array::from_fn(|i|rows[i/4][i%4+4] as f32);
    ensure!(result.iter().all(|v|v.is_finite()),"Nonfinite live renderer inverse");Ok(result)
}

type PipelineKey=(usize,usize);
type GeometryKey=(usize,usize,usize,usize,usize);
type SamplerKey=(i32,sampling::Device);
struct Pass {source:Arc<TranslatedPass>,program:gpu::Program,samplers:PassSamplers,writes:Vec<d3d9_state::State>}
struct Pipeline {passes:Vec<Pass>}
impl Pipeline {
    unsafe fn new(gl:&glow::Context,resources:&Resources)->Result<Self> {
        let mut result=Self {passes:Vec::new()};
        let build=(||->Result<()> {
            let layout=resources.effect.layout.as_ref().map_err(|e|anyhow::anyhow!("{e}"))?;
            let technique=layout.techniques.get(resources.technique.technique).context("Registered technique was retired")?;
            for pass in &technique.passes {
                let source=Arc::new(resources.effect.translate_pass(&resources.technique.name,pass.name.as_deref().context("Original pass is unnamed")?)?);
                let samplers=resources.effect.sampler_bindings(&source)?;
                let writes=resources.effect.render_states(&source)?;
                let program=gpu::Program::link(gl,source.clone())?;
                result.passes.push(Pass {source,program,samplers,writes});
            }
            ensure!(!result.passes.is_empty(),"Registered effect has no passes");Ok(())
        })();
        if let Err(error)=build {result.destroy(gl);return Err(error);}Ok(result)
    }
    unsafe fn destroy(self,gl:&glow::Context){for pass in self.passes {pass.program.destroy(gl);}}
}
struct PipelineEntry {source:Weak<Effect>,value:Result<Pipeline,String>}
struct Geometry {vertices:Weak<Vec<f32>>,indices:Weak<Vec<u32>>,binding:d3d9_vertices::Binding}
struct Image {source:Weak<images::Source>,value:images::Image}
struct Sampler {last_seen:u64,value:sampling::Binding}
enum ReadyImage {Cached(usize),Runtime(sampling::ImageView),Null2d}
struct ReadyPass {geometry:GeometryKey,textures:Vec<(u32,SamplerKey,ReadyImage)>,device:raster::Device}
pub(super) struct Renderer {
    materials:Materials,owner:Box<dyn InputOwner>,pipelines:BTreeMap<PipelineKey,PipelineEntry>,
    geometry:BTreeMap<GeometryKey,Geometry>,images:BTreeMap<usize,Image>,samplers:BTreeMap<SamplerKey,Sampler>,
    scratch:gpu::Scratch,frame:u64,errors:BTreeMap<usize,String>,
    /// Per material/pass inputs that never change between draws (see execute).
    static_params:std::collections::HashMap<u64,Vec<(Arc<str>,Arc<[u32]>)>>,
    failed:std::collections::HashMap<usize,u64>,
}
#[derive(Debug,serde::Serialize)]
pub(super) struct Counts {pub materials:usize,pub pipelines:usize,pub geometry:usize,pub images:usize,pub samplers:usize,pub failed_materials:usize}
impl std::fmt::Debug for Renderer {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {f.debug_struct("OriginalPcRenderer").field("materials",&self.materials.len()).field("pipelines",&self.pipelines.len()).field("images",&self.images.len()).field("owner",&self.owner).finish()}
}
impl Renderer {
    pub fn counts(&self)->Counts {Counts {materials:self.materials.len(),pipelines:self.pipelines.len(),geometry:self.geometry.len(),images:self.images.len(),samplers:self.samplers.len(),failed_materials:self.errors.len()}}
    pub fn new(materials:Materials,owner:Box<dyn InputOwner>)->Self {
        Self {materials,owner,pipelines:BTreeMap::new(),geometry:BTreeMap::new(),images:BTreeMap::new(),samplers:BTreeMap::new(),scratch:gpu::Scratch::default(),frame:0,errors:BTreeMap::new(),failed:Default::default(),static_params:Default::default()}
    }
    fn record_error(&mut self,identity:usize,error:&str) {
        if self.errors.get(&identity).is_none_or(|old|old!=error) {
            log::warn!("Original PC material fallback: {error}");
            self.errors.insert(identity,error.to_owned());
        }
    }
    fn numeric_admission(resources:&Resources,pass:&TranslatedPass,draw:&GpuDrawCommand,inputs:&mut Inputs)->Result<()> {
        let layout=resources.effect.layout.as_ref().map_err(|e|anyhow::anyhow!("{e}"))?;
        let names=resources.effect.numeric_inputs(pass)?;
        for name in names {
            // already supplied (an earlier pass, or this material's cached static inputs)
            if inputs.parameters.contains_key(name) {continue;}
            let index=layout.parameters.iter().position(|p|p.value.name.as_deref()==Some(name)).context("Numeric scene parameter is absent")?;
            if let Some(kind)=resources.effect.binding_annotations[index].text("VarType")? {
                if kind.eq_ignore_ascii_case("Global")&&!inputs.parameters.contains_key(name) {
                    if let Some(value)=inputs.lighting.as_ref().and_then(|lighting|lighting.resolve(name)) {
                        inputs.put(name,value.words(&layout.parameters[index].value)?);
                    }
                }
                if kind.eq_ignore_ascii_case("Global")&&name=="g_renderTargetDimensions"&&!inputs.parameters.contains_key(name) {
                    let value=super::d3d9_predefined::Value::Vector(super::d3d9_globals::target_dimensions(inputs.target.extent)?);
                    inputs.put(name,value.words(&layout.parameters[index].value)?);
                }
                if kind.eq_ignore_ascii_case("Object")&&!inputs.parameters.contains_key(name) {
                    let annotations=&resources.effect.binding_annotations[index];
                    if let Some(object)=annotations.text("Object")? {
                        let key=(object.to_owned(),annotations.integer("ObjectIndex")?);
                        if super::d3d9_texture_effects::Kind::named(object).is_some() {
                            let property=annotations.text("ObjectProperty")?.context("Texture-effect property is absent")?;
                            if super::d3d9_texture_effects::Projection::handles(property) {
                                let source=inputs.texture_effect(object,key.1)?.map(|effect|effect.projection.as_ref().context("Assigned texture effect lost its projection properties")).transpose()?;
                                let value=super::d3d9_texture_effects::Projection::resolve(source,property).unwrap();
                                inputs.put(name,value.words(&layout.parameters[index].value)?);
                            }
                        }
                        if let Some(light)=inputs.object_lights.get(&key) {
                            let property=annotations.text("ObjectProperty")?.context("Light property is absent")?;
                            if let Some(words)=light.words(property,annotations.text("Space")?,&layout.parameters[index].value)? {
                                inputs.put(name,words);
                            }
                        }
                        if !inputs.parameters.contains_key(name) {
                            if let Some(state)=&inputs.assigned_lights {
                                let property=annotations.text("ObjectProperty")?.context("Light property is absent")?;
                                if let Some(words)=state.words(object,key.1,property,annotations.text("Space")?,&layout.parameters[index].value)? {
                                    inputs.put(name,words);
                                }
                            }
                        }
                    }
                }
                if kind.eq_ignore_ascii_case("Attribute")&&!inputs.parameters.contains_key(name) {
                    if let Some(snapshot)=&inputs.sh {
                        if let Some(words)=snapshot.words(name,&layout.parameters[index].value)? {inputs.put(name,words);}
                    }
                }
                if kind.eq_ignore_ascii_case("Attribute")&&!inputs.parameters.contains_key(name) {
                    if let Some(attributes)=draw.material.as_ref().and_then(|material|material.attributes.as_ref()) {
                        let attributes=attributes.as_ref().map_err(|error|anyhow::anyhow!("{error}"))?;
                        if let Some(attribute)=attributes.get(name) {
                            inputs.put(name,super::d3d9_attributes::words(attribute,&layout.parameters[index].value)?);
                        }
                    }
                }
                if kind.eq_ignore_ascii_case("Predefined")&&!inputs.parameters.contains_key(name) {
                    let mapping=resources.effect.binding_annotations[index].text("DefinedMapping")?.context("Predefined mapping name is absent")?;
                    if mapping.eq_ignore_ascii_case("TexCoordGenProjMatrix") {
                        let bound=draw.projection_bound.context("TexCoordGenProjMatrix requires the mesh's actual world-bound center")?;
                        let cache=inputs.predefined.as_ref().context("TexCoordGenProjMatrix requires camera cache ownership")?;
                        let precision=inputs.texture_precision.context("TexCoordGenProjMatrix requires native precision ownership")?;
                        inputs.put(name,cache.texgen_projection(bound,precision)?.words(&layout.parameters[index].value)?);
                    }
                    if let Some(material)=&draw.material {
                        if let Some(value)=super::d3d9_texture_transform::resolve_animated(material,mapping,resources.effect.binding_annotations[index].integer("NTMIndex")?,inputs.texture_precision,|slot|super::em2_tex_anim::lookup(draw,slot))? {
                            inputs.put(name,value.words(&layout.parameters[index].value)?);
                        }
                    }
                    if mapping.eq_ignore_ascii_case("time") {
                        if let Some(time)=draw.shader_time.as_ref().and_then(|clock|clock.value()) {
                            inputs.put(name,super::d3d9_attributes::shader_time_value(time,&layout.parameters[index].value)?);
                        } else if let Some(attributes)=draw.material.as_ref().and_then(|material|material.attributes.as_ref()) {
                            let attributes=attributes.as_ref().map_err(|error|anyhow::anyhow!("{error}"))?;
                            if let Some(value)=super::d3d9_attributes::shader_time(attributes,&layout.parameters[index].value)? {inputs.put(name,value);}
                        }
                    }
                    if let Some(cache)=&inputs.predefined {
                        if !inputs.parameters.contains_key(name) {if let Some(value)=cache.resolve(mapping)? {inputs.put(name,value.words(&layout.parameters[index].value)?);}}
                    }
                }
                let admitted_default=kind.eq_ignore_ascii_case("Attribute")&&inputs.attribute_defaults.contains(name);
                // Characters: inputs the EM2 scene owner doesn't supply yet (per-object environment
                // point light, Mickey's ear transforms...) take the effect's compiled default, else a
                // neutral value (identity matrices, a light that falls off to nothing, zero).
                if !inputs.parameters.contains_key(name)&&!admitted_default&&(char_native(draw)||env_native_for(draw)||transparent_env(draw)) {
                    let value=&layout.parameters[index].value;
                    let count=(value.rows.max(1)*value.columns.max(1)*value.elements.max(1)) as usize;
                    // the character's SH light, whichever kind of input the effect declares it as
                    let sh_words=inputs.sh.as_ref().and_then(|snapshot|snapshot.words(name,value).ok().flatten());
                    let words=if let Some(w)=sh_words {w} else {match &value.data {
                        super::d3d9_effect::Data::Words(w) if w.len()>=count&&w.iter().any(|x|*x!=0)=>w[..count].to_vec(),
                        _=>{
                            let mut v=vec![0f32;count];
                            if value.kind==3 {
                                if (value.class==2||value.class==3)&&value.rows==value.columns {let n=value.rows as usize;for e in 0..value.elements.max(1) as usize {for i in 0..n {v[e*n*n+i*n+i]=1.;}}}
                                else if name.to_ascii_lowercase().contains("atten") {v[0]=1.;}
                            }
                            v.iter().map(|x|x.to_bits()).collect()
                        }
                    }};
                    inputs.put(name,words);
                }
                // Per-mesh colours ("MeshData" ConstantColor / ColorRegister) only exist when an entity's
                // R3MTEffectsComponent overrides them; otherwise the effect's compiled default applies.
                if !(inputs.parameters.contains_key(name)||admitted_default)
                    && kind.eq_ignore_ascii_case("Object")
                    && resources.effect.binding_annotations[index].text("Object").ok().flatten()==Some("MeshData") {
                    let value=&layout.parameters[index].value;
                    let count=(value.rows.max(1)*value.columns.max(1)*value.elements.max(1)) as usize;
                    let property=resources.effect.binding_annotations[index].text("ObjectProperty").ok().flatten().unwrap_or("");
                    // the entity's R3MTEffectsComponent override, when enabled (em2.rs stores slots 0-3 =
                    // ConstantColor_N, 4-6 = ColorRegister_N, -1 = not enabled)
                    let slot=resources.effect.binding_annotations[index].integer("ObjectIndex").ok().flatten().unwrap_or(0).max(0) as usize;
                    let slot=if property.eq_ignore_ascii_case("ColorRegister"){4+slot}else{slot};
                    let authored=draw.material_registers.as_ref().and_then(|r|r.get(slot)).filter(|c|c[0]>=0.).copied();
                    let words=if let Some(c)=authored {(0..count).map(|i|c[i%4].to_bits()).collect()} else {match &value.data {
                        super::d3d9_effect::Data::Words(w) if w.len()>=count&&w.iter().any(|x|*x!=0)=>w[..count].to_vec(),
                        _=>{let fill:[f32;4]=if property.eq_ignore_ascii_case("ColorRegister"){[0.,0.,0.,0.]}else{[1.,1.,1.,1.]};(0..count).map(|i|fill[i%4].to_bits()).collect()}
                    }};
                    inputs.put(name,words);
                }
                if !(inputs.parameters.contains_key(name)||admitted_default) {
                    let a=&resources.effect.binding_annotations[index];
                    anyhow::bail!("Missing original {kind} input {name} (Object={:?} ObjectIndex={:?} ObjectProperty={:?} Space={:?})",
                        a.text("Object").ok().flatten(),a.integer("ObjectIndex").ok().flatten(),a.text("ObjectProperty").ok().flatten(),a.text("Space").ok().flatten());
                }
            }
        }
        Ok(())
    }
    unsafe fn execute(&mut self,gl:&glow::Context,resources:&Arc<Resources>,draw:&GpuDrawCommand,mesh:GpuMeshHandle,mut inputs:Inputs)->Result<()> {
        // Resolve scene ownership before allocating GPU state, and prepare
        // every pass before submitting any geometry for this object.
        ensure!(!self.owner.fallback_only(),"Scene owner is restricted to the legacy fallback");
        ensure!(draw.v_data.len()%12==0&&mesh.index_count>=0&&mesh.index_count as usize==draw.indices.len(),"Original draw geometry differs from shared buffers");
        let key=(Arc::as_ptr(&resources.effect) as usize,resources.technique.technique);
        if !self.pipelines.contains_key(&key) {
            let value=Pipeline::new(gl,resources).map_err(|e|format!("{e:#}"));
            self.pipelines.insert(key,PipelineEntry {source:Arc::downgrade(&resources.effect),value});
        }
        let pipeline=self.pipelines[&key].value.as_ref().map_err(|e|anyhow::anyhow!("{e}"))?;
        let mut device=inputs.device.clone();let mut ready=Vec::with_capacity(pipeline.passes.len());
        for (index,pass) in pipeline.passes.iter().enumerate() {
            // Inputs that don't change per draw or per frame (material attributes, default lights,
            // mesh colours...) are resolved once per material/pass and reused; only camera/object
            // (Predefined) and scene-lighting (Global) inputs are rebuilt for every draw.
            // Characters are excluded: their light (SH) is per draw.
            let cache_key=(!char_native(draw)).then(|| {
                use std::hash::{Hash,Hasher};let mut h=std::collections::hash_map::DefaultHasher::new();
                draw.material.as_ref().map(|m|Arc::as_ptr(m) as usize).hash(&mut h);key.hash(&mut h);index.hash(&mut h);
                if let Some(r)=&draw.material_registers {for row in r {for v in row {v.to_bits().hash(&mut h);}}}
                h.finish()
            });
            let cached=cache_key.and_then(|k|self.static_params.get(&k));
            if let Some(cached)=cached {for (n,v) in cached.iter() {if !inputs.parameters.contains_key(&**n) {inputs.parameters.insert(n.clone(),v.clone());}}}
            let fill=cache_key.filter(|_|cached.is_none());
            Self::numeric_admission(resources,&pass.source,draw,&mut inputs)?;
            if let Some(k)=fill {
                let layout=resources.effect.layout.as_ref().map_err(|e|anyhow::anyhow!("{e}"))?;
                let mut keep=Vec::new();
                for name in resources.effect.numeric_inputs(&pass.source)? {
                    let Some(i)=layout.parameters.iter().position(|p|p.value.name.as_deref()==Some(name)) else {continue};
                    let kind=resources.effect.binding_annotations[i].text("VarType")?.unwrap_or("");
                    if kind.eq_ignore_ascii_case("Predefined")||kind.eq_ignore_ascii_case("Global") {continue;}
                    if let Some((n,v))=inputs.parameters.get_key_value(name) {keep.push((n.clone(),v.clone()));}
                }
                if self.static_params.len()>50_000 {self.static_params.clear();}
                self.static_params.insert(k,keep);
            }
            let banks=resources.effect.constant_banks(&pass.source,&inputs.parameters,inputs.precision)?;
            static DUMP:std::sync::OnceLock<Option<std::ffi::OsString>>=std::sync::OnceLock::new();
            if let Some(dir)=DUMP.get_or_init(||std::env::var_os("BLOTWORX_DUMP_CHAR")) {
                // BLOTWORX_DUMP_MATCH: "sky" = skybox draws, otherwise part of a shader name (default Gen3_Char)
                let wanted=std::env::var("BLOTWORX_DUMP_MATCH").unwrap_or_else(|_|"Gen3_Char".into());
                let shader_name=draw.material.as_ref().and_then(|m|m.shader.as_deref()).unwrap_or("");
                if (wanted=="sky"&&draw.skybox)||(wanted!="sky"&&shader_name.contains(wanted.as_str())) {
                    let path=std::path::Path::new(&dir).join(format!("{}_{}.txt",resources.technique.shader_name.replace(['/','\\',':'],"_"),index));
                    if !path.exists() {
                        let mut out=String::new();
                        for (stage,t,f) in [("VERTEX",&pass.source.vertex,&banks.vertex.floats),("PIXEL",&pass.source.pixel,&banks.pixel.floats)] {
                            out+=&format!("==== {stage}\n");
                            for u in &t.uniforms {out+=&format!("uniform kind={} c{} count={} constant={} name={:?}\n",u.kind,u.index,u.count,u.constant,u.name);}
                            for s in &t.samplers {out+=&format!("sampler kind={} s{} name={:?}\n",s.kind,s.index,s.name);}
                            for r in 0..f.len()/4 {let v=&f[r*4..r*4+4];if v.iter().any(|x|*x!=0.) {out+=&format!("c{r} = {:?}\n",v);}}
                            out+=&t.source;out+="\n";
                        }
                        out+=&format!("==== PASS WRITES {:?}\n",pass.writes.iter().map(|s|(s.state,s.value)).collect::<Vec<_>>());
                        out+="==== PARAMETERS\n";
                        for (k,v) in &inputs.parameters {out+=&format!("{k} = {:?}\n",v.iter().take(16).map(|w|f32::from_bits(*w)).collect::<Vec<_>>());}
                        let _=std::fs::create_dir_all(&dir);let _=std::fs::write(&path,out);
                    }
                }
            }
            device.write(&pass.writes)?;
            // The character pass turns depth writes off (the game lays character depth down in an
            // earlier depth-only pass). Without that pass, write depth here or characters are see-through.
            if char_native(draw) {device.write(&[d3d9_state::State {state:14,value:1}])?;}
            // World passes do the same (Gen3_StaticEnv*: ZWRITEENABLE=0 after the game's depth pre-pass).
            // Without depth, later draws paint over nearer ones: floors vanish and walls turn see-through.
            // Opaque passes write depth here; alpha-blended ones (ALPHABLENDENABLE=27) stay as authored.
            else if (env_native_for(draw)||transparent_env(draw)) && !pass.writes.iter().any(|w|w.state==27&&w.value!=0) {
                device.write(&[d3d9_state::State {state:14,value:1}])?;
            }
            device.validate_target(&inputs.target)?;
            pass.program.validate_alpha_test(&device.alpha_test()?)?;
            // Omitted sampler writes inherit earlier passes, including when a
            // different sampler parameter occupies the same hardware register.
            let mut textures=Vec::new();
            for (vertex,routes,registers) in [(true,&pass.samplers.vertex,&pass.program.vertex_samplers),(false,&pass.samplers.pixel,&pass.program.pixel_samplers)] {
                for route in routes {
                    let sampler=registers.iter().find(|s|s.register==route.register as u32).context("Original shader sampler interface differs")?;
                    let parameter=route.texture_parameter;
                    if parameter.is_none() {
                        ensure!(!vertex&&sampler.dimension==0,"Null D3D texture requires its verified stage/dimension adapter");
                        let state=inputs.samplers.get_mut(&(vertex,sampler.register)).context("Original sampler device snapshot is absent")?;state.write(&route.states)?;let state=state.clone();
                        let sampler_key=(sampler.dimension,state.clone());
                        if !self.samplers.contains_key(&sampler_key){self.samplers.insert(sampler_key.clone(),Sampler {last_seen:self.frame,value:sampling::Binding::new(gl,&state,sampler.dimension)?});}
                        let binding=self.samplers.get_mut(&sampler_key).unwrap();binding.last_seen=self.frame;binding.value.bind_null_2d(gl,sampler.unit)?;
                        textures.push((sampler.unit,sampler_key,ReadyImage::Null2d));continue;
                    }
                    let parameter=parameter.unwrap();
                    let input_image=if let Some(image)=inputs.images.get(&parameter) {image.clone()}else {match resources.textures.get(&parameter) {
                        Some(Some(Texture::Image {source,..}))=>InputImage::Asset(source.clone()),
                        Some(Some(Texture::SceneObject {name,index}))=>{
                            match inputs.texture_effect(name,*index)?.and_then(|effect|effect.image.clone()) {
                                Some(image)=>image,
                                // characters and world surfaces with nothing projected on them: unshadowed (white), not tex2D(NULL)=black
                                None if name.eq_ignore_ascii_case("ProjectedShadowMap")=>InputImage::Runtime(white_view(gl)?),
                                None=>InputImage::Null2d,
                            }
                        },
                        // a missing normal map (e.g. Mickey's brush, handheld/textures/mickeyBrushNG_N_tex.nif
                        // isn't in the level's packs) only loses surface detail: use a flat normal
                        Some(Some(Texture::MissingFile {path})) if path.to_ascii_lowercase().contains("_n_tex")=>InputImage::Runtime(flat_normal_view(gl)?),
                        Some(Some(Texture::MissingFile {path}))=>anyhow::bail!("Missing original texture {path}"),
                        Some(Some(Texture::MissingMap {shader,slot}))=>anyhow::bail!("Missing original NIF map shader={shader},slot={slot}"),
                        _=>anyhow::bail!("Original texture parameter {parameter} is unbound"),
                    }};
                    let state=inputs.samplers.get_mut(&(vertex,sampler.register)).context("Original sampler device snapshot is absent")?;state.write(&route.states)?;let state=state.clone();
                    let sampler_key=(sampler.dimension,state.clone());
                    if !self.samplers.contains_key(&sampler_key){self.samplers.insert(sampler_key.clone(),Sampler {last_seen:self.frame,value:sampling::Binding::new(gl,&state,sampler.dimension)?});}
                    self.samplers.get_mut(&sampler_key).unwrap().last_seen=self.frame;
                    let image=match input_image {
                        InputImage::Null2d=>{
                            ensure!(!vertex&&sampler.dimension==0,"Known-null scene texture requires a verified pixel 2D sampler");
                            self.samplers[&sampler_key].value.bind_null_2d(gl,sampler.unit)?;
                            textures.push((sampler.unit,sampler_key,ReadyImage::Null2d));continue;
                        },
                        InputImage::Asset(source)=>{
                            let image_key=Arc::as_ptr(&source) as usize;
                            if !self.images.contains_key(&image_key){let image=images::Image::new(gl,&source)?;self.images.insert(image_key,Image {source:Arc::downgrade(&source),value:image});}
                            if state.needs_srgb()? {self.images.get_mut(&image_key).unwrap().value.ensure_srgb(gl)?;}
                            ReadyImage::Cached(image_key)
                        },
                        InputImage::Runtime(view)=>ReadyImage::Runtime(view),
                    };
                    // Validate view/target and stage binding before any pass draws.
                    let view=match &image {ReadyImage::Cached(key)=>self.images[key].value.view,ReadyImage::Runtime(view)=>*view,ReadyImage::Null2d=>unreachable!()};
                    self.samplers[&sampler_key].value.bind(gl,sampler.unit,&view)?;
                    textures.push((sampler.unit,sampler_key,image));
                }
            }
            let geometry_key=(Arc::as_ptr(&draw.v_data) as usize,Arc::as_ptr(&draw.indices) as usize,key.0,key.1,index);
            let source=match &draw.source_attributes {Some(Ok(source))=>Some(source),Some(Err(error)) if pass.program.attributes.iter().any(|a|matches!(a.usage,0|3|6|7))=>anyhow::bail!("Original mesh-space streams: {error}"),_=>None};
            if let Some(geometry)=self.geometry.get_mut(&geometry_key) {geometry.binding.refresh(gl,mesh.vbo,mesh.ebo,draw.v_data.len()/12,source,draw.extra_uvs.as_deref())?;}
            else {let binding=d3d9_vertices::Binding::new_mesh_space(gl,&pass.program,mesh.vbo,mesh.ebo,draw.v_data.len()/12,source,draw.extra_uvs.as_deref())?;self.geometry.insert(geometry_key,Geometry {vertices:Arc::downgrade(&draw.v_data),indices:Arc::downgrade(&draw.indices),binding});}
            pass.program.upload(gl,&banks,&inputs.coordinates,&mut self.scratch)?;
            ready.push(ReadyPass {geometry:geometry_key,textures,device:device.clone()});
        }
        for (pass,ready) in pipeline.passes.iter().zip(ready) {
            gl.use_program(Some(pass.program.handle));
            let alpha=ready.device.submit(gl,&inputs.target)?;pass.program.upload_alpha_test(gl,&alpha)?;
            for (unit,sampler,image) in ready.textures {let view=match image {ReadyImage::Cached(key)=>self.images[&key].value.view,ReadyImage::Runtime(view)=>view,ReadyImage::Null2d=>{self.samplers[&sampler].value.bind_null_2d(gl,unit)?;continue;}};self.samplers[&sampler].value.bind(gl,unit,&view)?;}
            gl.bind_vertex_array(Some(self.geometry[&ready.geometry].binding.vao));
            gl.draw_elements(glow::TRIANGLES,mesh.index_count,glow::UNSIGNED_INT,0);
        }
        Ok(())
    }
}
impl Backend for Renderer {
    // Draws whose game shader failed recently go to the ordinary (linear) fallback, so the usual
    // off-screen culling is safe for them; game-shader draws may move vertices and stay unculled.
    fn allows_linear_culling(&self,draw:&GpuDrawCommand)->bool {
        // Static world/prop shaders don't move vertices, so their mesh bounds are valid for
        // off-screen culling (without it every world mesh was drawn every frame: ~3x slower).
        // Wavey (vertex-animated) and skinned character shaders keep drawing unculled.
        let static_shader=draw.material.as_ref().and_then(|m|m.shader.as_deref()).is_some_and(|s|
            (s.starts_with("Gen3_StaticEnv")||s.starts_with("Gen3_StatEnv")||s.starts_with("Gen3_Prop"))&&!s.contains("Wavey"));
        static_shader||self.owner.fallback_only()||draw.material.as_ref().is_some_and(|m|self.failed.get(&(Arc::as_ptr(m) as usize)).is_some_and(|&at|self.frame.wrapping_sub(at)<240))
    }
    fn handles(&self,draw:&GpuDrawCommand)->bool {draw.material.as_ref().is_some_and(|m|self.materials.contains_key(&(Arc::as_ptr(m) as usize)))}
    unsafe fn begin_frame(&mut self,gl:&glow::Context) {
        self.frame=self.frame.wrapping_add(1);self.materials.retain(|_,(source,_)|source.strong_count()!=0);
        self.errors.retain(|key,_|self.materials.contains_key(key));
        self.failed.retain(|key,_|self.materials.contains_key(key));
        let live=self.materials.values().filter_map(|(_,value)|value.as_ref().ok()).map(|r|(Arc::as_ptr(&r.effect) as usize,r.technique.technique)).collect::<BTreeSet<_>>();
        self.pipelines.retain(|key,entry|{if live.contains(key)&&entry.source.strong_count()!=0 {true}else {if let Ok(pipeline)=std::mem::replace(&mut entry.value,Err("retired".into())){pipeline.destroy(gl);}false}});
        let retired=self.geometry.iter().filter(|(key,g)|g.vertices.strong_count()==0||g.indices.strong_count()==0||!self.pipelines.contains_key(&(key.2,key.3))).map(|(key,_)|*key).collect::<Vec<_>>();
        for key in retired {self.geometry.remove(&key).unwrap().binding.destroy(gl);}
        let retired=self.images.iter().filter(|(_,image)|self.materials.is_empty()||image.source.strong_count()==0).map(|(key,_)|*key).collect::<Vec<_>>();
        for key in retired {self.images.remove(&key).unwrap().value.destroy(gl);}
        let retired=self.samplers.iter().filter(|(_,s)|self.materials.is_empty()||s.last_seen!=self.frame.wrapping_sub(1)).map(|(key,_)|key.clone()).collect::<Vec<_>>();
        for key in retired {self.samplers.remove(&key).unwrap().value.destroy(gl);}
    }
    unsafe fn draw(&mut self,gl:&glow::Context,draw:&GpuDrawCommand,mesh:GpuMeshHandle,frame:&Frame<'_>)->Result<bool,String> {
        let Some(material)=draw.material.as_ref() else{return Ok(false)};
        let Some((_,resources))=self.materials.get(&(Arc::as_ptr(material) as usize)) else{return Ok(false)};
        let identity=Arc::as_ptr(material) as usize;
        // A material that failed recently fails the same way again: go straight to the fallback
        // instead of rebuilding inputs every frame (retried every 240 frames).
        static RETRY:std::sync::OnceLock<bool>=std::sync::OnceLock::new();
        if !*RETRY.get_or_init(||std::env::var_os("BLOTWORX_NATIVE_RETRY").is_some()) {
            if let Some(&at)=self.failed.get(&identity) {
                if self.frame.wrapping_sub(at)<240 {return Ok(false);}
                self.failed.remove(&identity);
            }
        }
        let prepared=(||->Result<_ ,String> {
            let resources=resources.as_ref().map_err(Clone::clone)?.clone();
            let inputs=self.owner.prepare(&resources,draw,frame).map_err(|e|format!("{}: {e:#}",resources.technique.shader_name))?;
            Ok((resources,inputs))
        })();
        let (resources,inputs)=match prepared {Ok(value)=>value,Err(error)=>{self.record_error(identity,&error);self.failed.insert(identity,self.frame);return Err(error);}};
        let front=gl.get_parameter_i32(glow::FRONT_FACE) as u32;
        let result=self.execute(gl,&resources,draw,mesh,inputs).map(|_|true).map_err(|e|format!("{}: {e:#}",resources.technique.shader_name));
        for unit in 0..20 {gl.bind_sampler(unit,None);}
        gl.active_texture(glow::TEXTURE0);gl.bind_vertex_array(None);gl.use_program(None);gl.front_face(front);
        gl.disable(glow::STENCIL_TEST);gl.stencil_mask(u32::MAX);gl.disable(glow::POLYGON_OFFSET_FILL);gl.color_mask(true,true,true,true);
        match &result {Ok(_)=>{self.errors.remove(&identity);},Err(error)=>{self.record_error(identity,error);self.failed.insert(identity,self.frame);}}
        result
    }
    unsafe fn destroy(&mut self,gl:&glow::Context) {
        for (_,geometry) in std::mem::take(&mut self.geometry){geometry.binding.destroy(gl);}
        for (_,image) in std::mem::take(&mut self.images){image.value.destroy(gl);}
        for (_,sampler) in std::mem::take(&mut self.samplers){sampler.value.destroy(gl);}
        for (_,pipeline) in std::mem::take(&mut self.pipelines){if let Ok(pipeline)=pipeline.value {pipeline.destroy(gl);}}
        self.materials.clear();self.errors.clear();
    }
}

