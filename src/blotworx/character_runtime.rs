//! Shared authored Havok graph/skin playback, usable before a full world is admitted.
use crate::render_support::{scene::CachedNifMesh, viewport3d::GpuDrawCommand};
use anyhow::{ensure, Context, Result};
use epic_mickey_lib_rs::{
    nif_file::{NiObject, NifFile},
    packfile::VirtualFile,
    scene_file::Component,
};
use std::{collections::HashMap, sync::Arc};
struct Binding {
    draw: usize,
    skin: usize,
    template: Arc<Vec<f32>>,
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    sources: Arc<Vec<u32>>,
    attributes:Option<Result<Arc<crate::nif_vertex_attributes::Source>,String>>,
    transform: [[f32; 4]; 4],
}
pub struct Character {
    graph: super::behavior::Document,
    prepared: super::generator_plan::Prepared,
    resources: super::animation_resources::Resources,
    playback: Option<super::havok_playback::Playback>,
    controlled:Option<super::authored_preview::Preview>,
    motion_prepared:bool,
    skins: Vec<Arc<super::skin::Skin>>,
    model: NifFile,model_path:String,split_catalog:Option<Arc<super::streamed_animation_resources::Catalog>>,
    native_clip_writes:u64,last_native_clip:Option<serde_json::Value>,
    bindings: Vec<Binding>,
    pub ticks: u64,
    native_bool_writes:u64,
    native_event_writes:u64,last_native_event:Option<String>,last_native_event_local:Option<i32>,last_native_event_delivered:bool,
    animation_culling:Option<bool>,
    last_native_bool:Option<(u32,bool,Option<usize>)>,
}
fn asset<'a>(files: &'a [VirtualFile], path: &str) -> Option<&'a VirtualFile> {
    let key = path
        .replace('\\', "/")
        .trim_start_matches('/')
        .to_ascii_lowercase();
    files
        .iter()
        .find(|f| f.path.replace('\\', "/").eq_ignore_ascii_case(&key))
}
impl Character {
    pub fn load(components: &[Component], files: &[VirtualFile]) -> Result<Self> {
        let animation = components
            .iter()
            .find(|c| c.class_name == "AnimationComponent")
            .context("Character has no Havok animation component")?;
        let prop = |name| {
            animation
                .properties
                .iter()
                .find(|p| p.name == name)
                .and_then(|p| p.value.as_str())
        };
        let project = prop("Behavior File Path").context("Character behavior project is absent")?;
        let default_name;
        let name = match prop("CharacterName").filter(|name| !name.is_empty()) {
            Some(name) => name,
            None => {
                default_name = super::behavior::default_character_name(project)?;
                &default_name
            }
        };
        let model_path = prop("NIF File Path").context("Character model is absent")?;
        let (contract, graph) = super::behavior::character_assets(project, name, |path| {
            asset(files, path).map(|f| f.data.as_slice())
        })?;
        let mut resources = super::animation_resources::Resources::load(&contract, |path| {
            asset(files, path).map(|f| f.data.clone())
        })?;
        let mut state = super::behavior_state::State::new(&graph)?;
        // Runtime linking assigns indices; the serialized -1 is not the live
        // value. hkbBehaviorLinkingUtils::linkClipBindings uses the character's
        // authored name-table ordinals and leaves missing symbols unbound.
        for clip in state.clips.values_mut() {
            clip.settings.animation_binding_index=match contract.animation_binding_names.iter().rposition(|name|name==&clip.settings.animation_name) {
                Some(index)=>index.try_into().context("Character animation binding index exceeds signed16-bit field")?,None=>-1,
            };
        }
        let mut prepared = super::generator_plan::prepare(&graph, state, 0)?;
        let playback =
            super::havok_playback::Playback::initialize(&graph, &mut prepared, &mut resources)?;
        let bytes = &asset(files, model_path)
            .context("Character costume model was not mounted")?
            .data;
        let model = NifFile::load_bytes(bytes).map_err(anyhow::Error::msg)?;
        let skins = super::skin::read_nif(bytes)?;
        ensure!(!skins.is_empty(), "Character costume has no supported skin");
        Ok(Self {
            graph,
            prepared,
            resources,
            playback:Some(playback),controlled:None,motion_prepared:false,
            skins:skins.into_iter().map(Arc::new).collect(),
            model,model_path:model_path.to_owned(),split_catalog:None,native_clip_writes:0,last_native_clip:None,
            bindings: Vec::new(),
            ticks: 0,
            native_bool_writes:0,native_event_writes:0,last_native_event:None,last_native_event_local:None,last_native_event_delivered:false,last_native_bool:None,animation_culling:None,
        })
    }
    pub fn bind(&mut self, draw: usize, mesh: &CachedNifMesh) -> Result<()> {
        let skin = self
            .skins
            .iter()
            .position(|s| s.block == mesh.mesh_index)
            .context("Character mesh has no skin binding")?;
        let raw = match self.model.blocks.get(mesh.mesh_index) {
            Some(NiObject::NiMesh(m)) => m,
            _ => anyhow::bail!("Character source is not NiMesh"),
        };
        let decoded = crate::wii_mesh::extract(&self.model, raw)?;
        ensure!(
            mesh.source_indices.as_ref() == Some(&decoded.source_indices),
            "Character render/source vertex mapping differs"
        );
        self.bindings.push(Binding {
            draw,
            skin,
            template: mesh.v_data.clone(),
            positions: decoded.mesh.positions,
            normals: decoded
                .mesh
                .normals
                .context("Character source has no normals")?,
            sources: Arc::new(decoded.source_indices),
            attributes:mesh.source_attributes.clone(),
            transform: mesh.mesh_to_model,
        });
        Ok(())
    }
    pub fn apply(&self, draws: &mut [GpuDrawCommand]) -> Result<()> {
        let mut palettes = HashMap::new();
        for binding in &self.bindings {
            if !palettes.contains_key(&binding.skin) {
                palettes.insert(
                    binding.skin,
                    Arc::new(super::skin::bone_palette(&self.skins[binding.skin], &self.active_playback()?.pose)?),
                );
            }
            let (vertices, min, max) = super::skin::render_skinned_pose(
                &self.skins[binding.skin],
                &palettes[&binding.skin],
                &binding.template,
                &binding.positions,
                &binding.normals,
                &binding.sources,
                binding.transform,
            )?;
            let draw = draws
                .get_mut(binding.draw)
                .context("Character draw was retired")?;
            draw.v_data = Arc::new(vertices);
            draw.source_attributes=binding.attributes.as_ref().map(|source|match source {
                Ok(source)=>{
                    let mut snapshot=(**source).clone();
                    snapshot.pose=Some(Arc::new(super::skin::DirectionSnapshot {skin:self.skins[binding.skin].clone(),palette:palettes[&binding.skin].clone(),sources:binding.sources.clone()}));
                    Ok(Arc::new(snapshot))
                },
                Err(error)=>Err(error.clone()),
            });
            draw.aabb_min = min;
            draw.aabb_max = max;
        }
        Ok(())
    }
    fn active_playback(&self)->Result<&super::havok_playback::Playback> {
        self.controlled.as_ref().map(|p|&p.playback).or(self.playback.as_ref()).context("Character playback ownership unavailable")
    }
    pub fn apply_playtest_motion(&mut self,player:&super::physics::Player)->Result<()> {
        ensure!(!self.motion_prepared,"Character motion already advanced this tick");
        if self.controlled.is_none(){
            let playback=self.playback.take().context("Character authored playback unavailable")?;
            self.controlled=Some(super::authored_preview::Preview::from_playback(playback,&self.graph)?);
        }
        self.controlled.as_mut().context("Shared motion playback unavailable")?.playtest_motion_tick(player,&self.graph,&mut self.prepared,&mut self.resources)?;
        self.motion_prepared=true;Ok(())
    }
    pub fn advance(&mut self, ticks: u64, draws: &mut [GpuDrawCommand]) -> Result<()> {
        if self.motion_prepared {ensure!(ticks==1,"Prepared motion requires one animation tick");self.motion_prepared=false;}
        else if let Some(preview)=self.controlled.as_mut(){preview.advance_character(&self.graph,&mut self.prepared,&mut self.resources,ticks)?;}
        else {self.playback.as_mut().context("Character playback unavailable")?.advance_character(&self.graph,&mut self.prepared,&mut self.resources,ticks)?;}
        if let Some(id)=self.last_native_event_local {
            if self.active_playback()?.delivered_events.iter().any(|e|e.id==id){self.last_native_event_delivered=true;}
        }
        self.ticks+=ticks;self.apply(draws)
    }
    /// Wwise event names carried by this tick's delivered behavior events: EM2 attaches sounds to
    /// animation clips as clip triggers with a string payload (e.g. Mickey_Jump.hkx fires
    /// "Play_sfx_Mickey_Jump_Core"), so the game's own timing decides when they play.
    pub fn sound_triggers(&self)->Vec<String> {
        let Ok(playback)=self.active_playback() else {return Vec::new()};
        playback.delivered_events.iter().filter_map(|e|e.payload.as_ref()).filter_map(|r|self.graph.objects.get(&r.id))
            .filter(|o|o.klass=="hkbStringEventPayload").filter_map(|o|o.fields["data"].as_str().map(str::to_owned))
            .filter(|s|!s.is_empty()).collect()
    }
    /// Names of this tick's delivered behavior events (diagnostics).
    pub fn delivered_event_names(&self)->Vec<String> {
        let Ok(playback)=self.active_playback() else {return Vec::new()};
        let names=self.graph.single("hkbBehaviorGraphStringData").ok().and_then(|d|d["eventNames"].as_array().cloned()).unwrap_or_default();
        playback.delivered_events.iter().map(|e|names.get(e.id.max(0) as usize).and_then(|v|v.as_str()).map(str::to_owned).unwrap_or_else(||format!("#{}",e.id))).collect()
    }
    /// PC registrationa28d80 assigns native13 to VAR_Has_Tool. Settera25fb0
    /// writes its mapped word and legitimately ignores an absent local symbol.
    pub fn bone_pose(&self,name:&str)->Result<super::actor_bone_query::Pose> {
        let lower=name.to_ascii_lowercase();
        let skeleton=lower.strip_prefix("skeleton:").map(str::to_owned).unwrap_or_else(||format!("skeleton:{lower}"));
        let skel=lower.strip_prefix("skel_").map(str::to_owned).unwrap_or_else(||format!("skel_{lower}"));
        for candidate in [lower,skeleton,skel] {
            if self.resources.rig.bones.iter().any(|b|b.name.eq_ignore_ascii_case(&candidate)) {return super::skeletal_children::bone_pose(&self.resources,&self.active_playback()?.pose,&candidate);}
        }Ok(super::actor_bone_query::DEFAULT)
    }
    pub fn set_pc_bool(&mut self,id:u32,value:bool)->Result<()> {
        ensure!(id==13,"PC animation bool symbol {id} is not mapped");
        let local=self.prepared.state.write_native_variable("VAR_Has_Tool",u32::from(value));
        self.native_bool_writes+=1;self.last_native_bool=Some((id,value,local));Ok(())
    }
    pub(super) fn set_split_catalog(&mut self,catalog:Arc<super::streamed_animation_resources::Catalog>){self.split_catalog=Some(catalog);}
    pub(super) fn link_pc_clip(&mut self,resource:&str,node:&str)->Result<()> {
        let mut definitions=self.prepared.state.clips.values().filter(|clip|clip.settings.name.starts_with("Dynamic")&&clip.settings.name==node);
        let Some(definition)=definitions.next() else{return Ok(())};
        ensure!(definitions.next().is_none(),"Ambiguous dynamic clip node {node}");
        let index=definition.settings.animation_binding_index;if index==-1{return Ok(());}
        ensure!(!resource.contains(':')&&!resource.starts_with(['/', '\\'])&&!resource.split(['/', '\\']).any(|s|s==".."),"Unsupported dynamic animation resource path");
        let model=self.model_path.replace('\\',"/");let base=model.rsplit_once('/').context("Character model has no source directory")?.0;
        let catalog=self.split_catalog.as_ref().context("Dynamic animation archive provider is unavailable")?;
        let split=resource.to_ascii_lowercase().ends_with("_streamed.hkx");
        for prefix in ["Animations","animations/igcs"] {
            let logical=format!("{base}/{prefix}/{resource}");
            let path=if split{super::streamed_animation_resources::cache_key(&logical)?}else{logical.to_ascii_lowercase()};
            if let Some(bytes)=catalog.read(&path)? {
                if split {self.resources.bind_dynamic_streamed(index,&path,&bytes,catalog)?;}else{self.resources.bind_dynamic(index,&path,&bytes,false)?;}
                self.native_clip_writes+=1;self.last_native_clip=Some(serde_json::json!({"node":node,"resource":resource,"cache":path,"binding_index":index}));return Ok(());
            }
        }
        anyhow::bail!("Dynamic animation {resource} is absent from mounted archives; global lipsync search/default fallback are not connected")
    }
    pub(super) fn queue_pc_event(&mut self,name:&str)->Result<()> {
        ensure!(super::em2_animation_events::native_id(name).is_some(),"Unknown PC native animation event {name}");
        let playback=if let Some(preview)=self.controlled.as_mut(){&mut preview.playback}else{self.playback.as_mut().context("Character playback unavailable")?};
        // Exact local graph symbol lookup; unmatched symbols remain explicit
        // until native PC missing-symbol policy has reference verification.
        playback.queue_named_event(&self.graph,name)?;
        let strings=self.graph.single("hkbBehaviorGraphStringData")?;
        self.last_native_event_local=Some(strings["eventNames"].as_array().context("Missing graph event symbols")?.iter().position(|n|n.as_str()==Some(name)).context("Queued event has no local symbol")?.try_into()?);
        self.last_native_event_delivered=false;
        self.native_event_writes+=1;self.last_native_event=Some(name.to_owned());Ok(())
    }
    pub(super) fn set_animation_culling(&mut self,enabled:bool)->Result<()> {
        ensure!(!enabled,"Visibility-owned animation culling is not connected");
        // This runtime already advances every admitted actor, including actors
        // outside the viewport. Disabled native animation culling uses that path.
        self.animation_culling=Some(false);Ok(())
    }
    pub fn summary(&self) -> serde_json::Value {
        let dynamic=self.prepared.plan.nodes.iter().filter_map(|node|{
            let clip=self.prepared.state.clips.get(&node.object)?;
            let path=self.resources.binding_path(clip.settings.animation_binding_index)?;
            let clock=self.prepared.clips.get(&node.object).ok().map(|i|i.clock.local_time);
            Some(serde_json::json!({"node":clip.settings.name,"binding_index":clip.settings.animation_binding_index,"resource":path,"local_time":clock}))
        }).collect::<Vec<_>>();
        serde_json::json!({"active_dynamic_clips":dynamic,"ticks":self.ticks,"bones":self.active_playback().map_or(0,|p|p.pose.len()),"bound_meshes":self.bindings.len(),
            "last_native_event_delivered":self.last_native_event_delivered,"last_native_event_local":self.last_native_event_local,
            "native_clip_writes":self.native_clip_writes,"last_native_clip":self.last_native_clip,
            "native_event_writes":self.native_event_writes,"last_native_event":self.last_native_event,
            "native_bool_writes":self.native_bool_writes,"last_native_bool":self.last_native_bool,
            "animation_culling_request":self.animation_culling,
            "active_nodes":self.prepared.plan.nodes.iter().map(|n|n.object.as_str()).collect::<Vec<_>>(),
            "playback":"shared authored Havok graph","shared_motion_driver":self.controlled.is_some(),"original_game_parity_verified":false})
    }
}
