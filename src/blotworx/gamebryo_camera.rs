//! Scene Gamebryo actors and authored camera views share one playback owner.
//! IGC admission and camera-controller blends are separate owners.
use anyhow::{ensure,Context,Result};
use std::collections::{BTreeMap,HashSet};
use epic_mickey_lib_rs::{nif_file::{NifFile,NiTransform,Matrix3,Vector3},packfile::VirtualFile};
use super::{actor_runtime::Actor,nif_sequence::{ActorBindings,SequenceResources},scene_bounds::Hierarchy};

struct DrawBinding {draw:usize,mesh:u32,inverse:[f32;16],model_bind:[[f32;4];4],sources:Option<Vec<u32>>,vertices:std::sync::Arc<Vec<f32>>,indices:std::sync::Arc<Vec<u32>>,path:Option<Vec<u32>>}
struct Camera {
    name:String,graph:Hierarchy,resources:SequenceResources,bindings:ActorBindings,
    actor:Actor,parent:NiTransform,node:Option<u32>,time:f32,model_path:String,draws:Vec<DrawBinding>,
}
impl Camera {
    fn load(entity:&super::assets::RuntimeEntity,files:&[VirtualFile])->Result<Self> {
        let component=entity.components.iter().find(|c|c.class_name=="AnimationComponentGB").context("Camera has no Gamebryo animation component")?;
        let path=|name:&str|component.properties.iter().find(|p|p.name.replace(' ',"")==name).and_then(|p|p.value.as_str()).filter(|p|!p.is_empty()).with_context(||format!("Camera is missing {name}"));
        let nif_path=path("NIFFilePath")?;
        let model=files.iter().find(|f|f.path.replace('\\',"/").eq_ignore_ascii_case(&nif_path.replace('\\',"/"))).with_context(||format!("Missing camera model {nif_path}"))?;
        let nif=NifFile::load_bytes(&model.data).map_err(anyhow::Error::msg)?;
        let graph=Hierarchy::load_with_source(&nif,&model.data)?;
        ensure!(graph.cameras.len()<=1,"Actor {} contains multiple camera views",entity.name);
        let node=graph.cameras.keys().next().copied();
        ensure!(node.is_some()||!nif.block_types.iter().any(|name|name.contains("SkinningMeshModifier")),"Gamebryo model skin palette publication is not connected");
        let root=graph.roots.iter().flatten().copied().next().context("Camera NIF has no root")?;
        let resources=SequenceResources::load_from_assets(path("KFMFilePath")?,files)?;
        let bindings=ActorBindings::prepare(&resources,&nif,&graph,root)?;
        ensure!(bindings.other_evaluators.is_empty(),"Unsupported camera evaluators: {:?}",bindings.other_evaluators);
        let scale=entity.authored_scale;
        ensure!(scale.is_finite()&&scale>0.,"Invalid camera entity scale");
        let parent=NiTransform {rotation:Matrix3{rows:std::array::from_fn(|r|std::array::from_fn(|c|entity.transform[c*4+r]/scale))},
            translation:Vector3{x:entity.transform[12],y:entity.transform[13],z:entity.transform[14]},scale};
        let boolean=|name:&str,default:bool|->Result<bool>{component.properties.iter().find(|p|p.name==name).map(|p|p.value.as_bool().context("Invalid camera clock property")).transpose().map(|v|v.unwrap_or(default))};
        let mut actor=Actor::new(boolean("Use Delta Time",true)?,boolean("Start Using Global Time",false)?);
        // Camera startup remains on its existing explicit selection path.
        // Model actors publish their authored default pose before a named request.
        if node.is_none() {
            let initial=component.properties.iter().find(|p|p.name=="Active Sequence ID").map(|p|p.value.as_u64().context("Invalid initial Gamebryo sequence")).transpose()?.unwrap_or(0);
            let initial=u32::try_from(initial)?;actor.retain_target(initial);
            if resources.get(initial).is_some(){actor.pose=Some(bindings.sample(initial,0.)?);}
            if boolean("Start Active",false)? {actor.request(initial,&resources,&bindings)?;actor.update(&resources,&bindings,0.,0.)?;}
        }
        Ok(Self{name:entity.name.clone(),graph,resources,bindings,actor,parent,node,time:0.,model_path:nif_path.replace('\\',"/"),draws:Vec::new()})
    }
    fn frame(&self)->Result<super::nif_camera::Frame> {
        let empty=BTreeMap::new();let poses=self.actor.pose.as_ref().map(|p|&p.transforms).unwrap_or(&empty);
        let world=self.graph.base_world_transforms_with_poses(Some(&self.parent),poses)?;
        let node=self.node.context("Selected Gamebryo model has no camera view")?;
        let camera=&self.graph.cameras[&node];
        Ok(super::nif_camera::Frame::from_transform(&world[&node],camera.frustum,camera.orthographic!=0))
    }
}
#[derive(Default)]
pub(super) struct State {
    cameras:BTreeMap<usize,Camera>,errors:Vec<serde_json::Value>,deferred:Vec<String>,selected:Option<usize>,
    frame:Option<super::nif_camera::Frame>,requests:u64,
}
impl State {
    pub fn load(entities:&[super::assets::RuntimeEntity],files:&[VirtualFile],targets:impl Iterator<Item=String>)->Self {
        let mut state=Self::default();let mut seen=HashSet::new();
        for name in targets.filter(|name|!name.is_empty()) {
            if !seen.insert(name.clone()){continue;}
            let Some(owner)=entities.iter().position(|e|e.name==name) else{state.deferred.push(name);continue;};
            let result=Camera::load(&entities[owner],files).map(|camera|(owner,camera));
            match result {Ok((owner,camera))=>{state.cameras.insert(owner,camera);},Err(error)=>state.errors.push(serde_json::json!({"entity":name,"error":format!("{error:#}")}))}
        }
        state
    }
    pub fn contains(&self,owner:usize)->bool {self.cameras.get(&owner).is_some_and(|actor|actor.node.is_some())}
    pub fn bind_draw(&mut self,owner:usize,path:&str,draw:usize,mesh:&crate::render_support::scene::CachedNifMesh)->Result<()> {
        let Some(actor)=self.cameras.get_mut(&owner)else{return Ok(())};
        if !actor.model_path.eq_ignore_ascii_case(&path.replace('\\',"/")){return Ok(());}
        ensure!(actor.graph.nodes.contains_key(&(mesh.mesh_index as u32)),"Gamebryo draw has no source node");
        actor.draws.push(DrawBinding {draw,mesh:mesh.mesh_index as u32,inverse:super::nif_runtime::inverse(mesh.mesh_to_model)?,model_bind:mesh.mesh_to_model,sources:mesh.source_indices.clone(),vertices:mesh.v_data.clone(),indices:mesh.indices.clone(),path:mesh.hierarchy_path.clone()});Ok(())
    }
    pub fn apply_models(&self,entities:&[super::assets::RuntimeEntity],draws:&mut [crate::render_support::viewport3d::GpuDrawCommand])->Result<()> {
        for (&owner,actor) in &self.cameras {
            if actor.draws.is_empty(){continue;}
            let Some(pose)=&actor.actor.pose else{continue;};
            let entity=entities.get(owner).context("Gamebryo render owner is absent")?;
            let scale=entity.authored_scale;ensure!(scale.is_finite()&&scale>0.,"Invalid Gamebryo owner scale");
            let parent=NiTransform {rotation:Matrix3 {rows:std::array::from_fn(|r|std::array::from_fn(|c|entity.transform[c*4+r]/scale))},translation:Vector3 {x:entity.transform[12],y:entity.transform[13],z:entity.transform[14]},scale};
            let world=actor.graph.base_world_transforms_with_poses(Some(&parent),&pose.transforms)?;
            for binding in &actor.draws {
                let draw=draws.get_mut(binding.draw).context("Gamebryo render draw is absent")?;
                let matrix=super::nif_runtime::matrix(world.get(&binding.mesh).context("Animated mesh frame absent")?);
                draw.trans_mat=super::skin::matrix_product(&matrix,&binding.inverse);
                ensure!(pose.visibility.is_empty()||binding.path.is_some(),"Gamebryo visibility draw has no source ancestry");
                let hidden=pose.visibility.iter().any(|(node,value)|*value<0.5&&binding.path.as_ref().is_some_and(|path|path.contains(node)));
                draw.indices=if hidden{std::sync::Arc::new(Vec::new())}else{binding.indices.clone()};
                let morphs=pose.morphs.iter().filter(|((mesh,_),_)|*mesh==binding.mesh).collect::<Vec<_>>();
                if !morphs.is_empty(){
                    let sources=binding.sources.as_ref().context("Gamebryo morph lacks source indices")?;
                    let matched=morphs.into_iter().filter(|(_, (start,positions))|sources.iter().all(|&index|index>=*start&&(index-*start)<positions.len()as u32)).collect::<Vec<_>>();
                    ensure!(matched.len()==1,"Gamebryo morph region is absent or ambiguous");let (_, (start,positions))=matched[0];
                    let (vertices,lo,hi)=super::nif_sequence::render_positions(&binding.vertices,sources,*start,positions,binding.model_bind)?;
                    draw.v_data=std::sync::Arc::new(vertices);draw.aabb_min=lo;draw.aabb_max=hi;
                }
                for (&(mesh,slot),&color) in &pose.colors {if mesh==binding.mesh {
                    let registers=draw.material_registers.as_mut().context("Gamebryo color channel lacks a material register owner")?;
                    ensure!(slot<registers.len(),"Gamebryo material color slot absent");registers[slot]=super::nif_properties::packed_color(color)?;
                }}
            }
        }Ok(())
    }
    pub fn prepared(&self,owner:usize)->bool {self.cameras.contains_key(&owner)}
    pub fn request(&mut self,owner:usize,name:&str)->Result<()> {
        let camera=self.cameras.get_mut(&owner).context("Gamebryo actor has no prepared source animation owner")?;
        let id=camera.resources.find_name(name);
        ensure!(camera.resources.get(id).is_some(),"Unknown camera sequence {name}");
        camera.actor.request(id,&camera.resources,&camera.bindings)?;
        camera.actor.update(&camera.resources,&camera.bindings,camera.time,0.)?;
        self.requests=self.requests.wrapping_add(1);
        if self.selected==Some(owner){self.frame=Some(camera.frame()?);}
        Ok(())
    }
    pub fn select_immediate(&mut self,owner:usize)->Result<()> {
        let frame=self.cameras.get(&owner).context("NIF camera is not prepared")?.frame()?;
        self.selected=Some(owner);self.frame=Some(frame);Ok(())
    }
    pub fn advance(&mut self,delta:f32)->Result<()> {
        ensure!(delta.is_finite()&&delta>=0.,"Invalid Gamebryo camera delta");
        for (&owner,camera) in &mut self.cameras {
            if camera.actor.active_id().is_some(){camera.time+=delta;camera.actor.update(&camera.resources,&camera.bindings,camera.time,delta)?;}
            if self.selected==Some(owner){self.frame=Some(camera.frame()?);}
        }
        Ok(())
    }
    pub fn retire(&mut self,owner:usize) {self.cameras.remove(&owner);if self.selected==Some(owner){self.selected=None;self.frame=None;}}
    pub fn frame(&self)->Option<super::nif_camera::Frame>{self.frame}
    pub fn summary(&self)->serde_json::Value {
        serde_json::json!({"cameras":self.cameras.iter().filter(|(_,c)|c.node.is_some()).map(|(owner,c)|serde_json::json!({"owner":owner,"entity":c.name,"active_sequence":c.actor.active_id(),"time":c.time,"sequences":c.resources.sequences().map(|(id,s)|serde_json::json!({"id":id,"name":s.name,"duration":s.duration})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
            "model_actors":self.cameras.iter().filter(|(_,c)|c.node.is_none()).map(|(owner,c)|serde_json::json!({"owner":owner,"entity":c.name,"active_sequence":c.actor.active_id(),"time":c.time,"draws":c.draws.len(),"pose_transforms":c.actor.pose.as_ref().map_or(0,|p|p.transforms.len())})).collect::<Vec<_>>(),
            "selected":self.selected,"frame":self.frame,"errors":self.errors,"deferred_entity_bindings":self.deferred,"requests":self.requests,
            "scope":"Shared KFM/actor bindings and hierarchy propagation for source cameras and rigid/morph model draws; immediate raw-frustum playtest selection; full native camera-controller admission, blends, lens/viewport policy and IGC lifecycle pending"})
    }
}
