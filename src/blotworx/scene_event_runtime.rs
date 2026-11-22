//! PC scene-event integration using the shared entity, IO, Lua and Box3D
//! implementations. Unsupported world services stop at the owning delivery;
//! pending work is retained, never reported as successful gameplay.
use super::{
    actor_io::{Delivery, Outputs},
    actor_scripts::{CallbackContext, Scripts},
    assets::RuntimeEntity,
};
use anyhow::{ensure, Context, Result};
use epic_mickey_lib_rs::scene_file::{Component, SceneFile};
use std::collections::{HashMap, HashSet, VecDeque};
struct Volume {
    owner: usize,
    shape: usize,
    inside: bool,
    disabled: bool,
}
#[derive(Clone, Copy, serde::Serialize)]
struct PlayerSlot {
    enabled: bool,
    control_type: i32,
    entity: Option<usize>,
}
/// PC SwapPlayer emits CE synchronously after clearing/replacing the slot.
/// Retain an unsupported event, including the old source identity, rather than
/// returning success to the original script before its listeners are wired.
#[derive(serde::Serialize)]
struct PlayerChange {
    event: u16,
    index: usize,
    old_entity: Option<usize>,
    new_entity: Option<usize>,
}
struct PcCameraReceiver<'a> {tracker:Option<&'a super::em2_camera_tracking::Owned>}
struct PendingCameraWorld;
impl super::em2_camera_yaw::World for PendingCameraWorld {
    fn check_snap(&mut self,_:usize,_:f32)->Result<()> {anyhow::bail!("Default-pass hint interpolators are not initialized")}
    fn snap(&mut self,_:usize,_:f32)->Result<()> {anyhow::bail!("Default-pass hint interpolators are not initialized")}
    fn target_live(&mut self,_:u32)->Result<bool> {anyhow::bail!("Camera focus handle lifecycle is not connected")}
    fn yaw_constrained(&mut self)->Result<bool> {anyhow::bail!("Default-pass yaw constraint query is not connected")}
}
impl super::em2_camera_data::Receiver for PcCameraReceiver<'_> {
    fn check_dataset(&mut self,_:&super::em2_camera_profiles::Row)->Result<bool> {
        let tracker=self.tracker.context("PC camera-owned tracker is absent")?;
        ensure!(tracker.reset_complete,"PC camera tracker {} is not initialized: {}",tracker.owner,tracker.phase);
        anyhow::bail!("PC camera CheckDataSet requires initialized working target/yaw, visibility and filtered sphere-overlap receivers")
    }
    fn precision(&mut self)->Result<super::em2_camera_profiles::Precision> {
        anyhow::bail!("Original PC camera-thread x87 precision is not reference-verified")
    }
    fn subscribe_target(&mut self)->Result<()> {
        anyhow::bail!("PC camera target transform-event subscription is not connected")
    }
}
pub struct Runtime {
    pc_render_profiles:std::sync::Arc<std::sync::Mutex<super::render_profiles::Registry>>,
    pc_light_sources:super::d3d9_scene_lights::Registry,
    pub entities: Vec<RuntimeEntity>,
    pub movies: super::movie::Requests,
    pub hud:super::em2_hud::State,
    abilities:super::em2_abilities::State,
    tools:super::em2_player_tools::State,
    child_links:Vec<(usize,usize,String)>,
    source_owners:HashMap<(usize,u32),usize>,
    drip_request_count:u64,last_drip_request:Option<serde_json::Value>,
    child_failures:HashMap<usize,Vec<String>>,
    previous_hidden:HashSet<usize>,
    pending_hide_hooks:Vec<(usize,bool)>,
    cameras:[super::camera_overrides::Controller;2],
    camera_manager:Option<super::em2_camera_manager::Manager>,
    pending_camera_clear:Option<super::em2_camera_reset::Pending>,
    player_constraints:super::player_constraints::State,
    guardians:super::guardians::State,
    camera_trackers:Vec<super::em2_camera_tracking::Owned>,
    templates:Option<std::sync::Arc<super::em2_children::Templates>>,
    maps:super::map_info::Volumes,
    quest_path:super::em2_quest_path::State,
    music:super::em2_music::Cache,
    level:super::em2_level_manager::Manager,
    sequences:super::em2_sequences::Catalog,
    igc:super::igc::Catalog,
    igc_sound_owners:HashMap<usize,Option<usize>>,
    igc_animation_owners:HashMap<usize,Option<usize>>,
    igc_animation_dispatches:u64,
    igc_dialogue_owners:HashMap<usize,Option<usize>>,igc_dialogue_requests:u64,last_igc_dialogue:Option<serde_json::Value>,
    igc_control_owners:HashMap<usize,[Option<usize>;3]>,
    igc_player_writes:VecDeque<super::em2_igc_control::PlayerWrite>,
    igc_control_dispatches:u64,igc_control_writes_applied:u64,
    igc_origin_token:usize,
    gamebryo_cameras:super::gamebryo_camera::State,
    music_overrides:super::em2_music::Overrides,
    audio:super::em2_audio::Catalog,
    audio_posts:super::audio_post72::State,
    hidden: HashSet<usize>,
    ids: HashMap<u32, usize>,
    queries: super::physics::Collision,
    volumes: Vec<Volume>,
    outputs: Outputs,
    pending: VecDeque<Delivery>,
    retained_commands: Vec<super::actor_scripts::Command>,
    player: usize,
    players: [PlayerSlot; 2],
    cutscene_input:super::em2_cutscene_input::State,
    game_state_events:super::em2_game_state_events::State,
    source_keys: Vec<(usize,u32)>,
    removal_queue: VecDeque<usize>,
    retired: HashSet<usize>,
    pending_player_event: Option<PlayerChange>,
    player_events_delivered: u64,
    last_player_change: Option<PlayerChange>,
    entered: u64,
    exited: u64,
    callbacks: u64,
    skipped: u64,
    error: Option<String>,
    time: f32,
    source_errors: Vec<serde_json::Value>,
    disabled_usables:HashSet<usize>,
    usable_owners:Vec<usize>,
    uses_started:u64,
    last_usable:Option<usize>,
    pending_collision_retirements:VecDeque<(usize,u32)>,
}
fn property<'a>(component: &'a Component, name: &str) -> Option<&'a serde_json::Value> {
    component
        .properties
        .iter()
        .find(|p| p.name == name)
        .map(|p| &p.value)
}
fn point(value: &serde_json::Value) -> Result<[f32; 3]> {
    let mut out = [0.; 3];
    for (i, key) in ["x", "y", "z"].iter().enumerate() {
        out[i] = value
            .get(*key)
            .and_then(|v| v.as_f64())
            .context("Invalid trigger size")? as f32;
    }
    ensure!(
        out.iter().all(|v| v.is_finite() && *v >= 0.),
        "Invalid trigger dimensions"
    );
    Ok(out)
}
impl Runtime {
    fn resolve_igc_entity_binding(&self,name:&str,binding:[u32;2])->Result<Option<usize>> {
        if binding[1]==u32::MAX || name.is_empty(){return Ok(None);}
        let hash=super::named_effects::lookup_hash(name)?;
        if hash==super::named_effects::lookup_hash("PLAYER_MICKEY")? {return Ok(self.players[0].entity);}
        if hash==super::named_effects::lookup_hash("PLAYER_OSWALD")? {return Ok(self.players[1].entity);}
        ensure!(binding[1]!=1,"IGC overloaded entity parameter is not connected");
        ensure!(!name.contains(':'),"IGC qualified entity lookup is not connected");
        let mut matches=self.entities.iter().enumerate().filter(|(o,e)|e.name==name&&!self.retired.contains(o));
        let owner=matches.next().map(|(o,_)|o);
        ensure!(matches.next().is_none(),"IGC entity name {name} has ambiguous live owners");Ok(owner)
    }
    /// PC b2fd60 binds roles before considering an overloaded parameter.
    /// Retain package scene and cleanup handles rather than
    /// reselecting a player when the timed sound event fires.
    fn bind_igc_sound_owners(&mut self)->Result<()> {
        let mut owners=HashMap::new();let mut controls=HashMap::new();let mut animations=HashMap::new();let mut dialogues=HashMap::new();
        for event in self.igc.prepared_scenes()?.flat_map(|scene|&scene.hooks).flat_map(|h|&h.events) {
            if let Some(dialogue)=&event.dialogue {
                let name=event.strings.get(&0x38).context("IGC dialogue owner string is absent")?;
                dialogues.insert(event.meta.offset,self.resolve_igc_entity_binding(name,dialogue.entity_binding)?);
            }
            if let Some(animation)=&event.gamebryo_animation {
                let name=event.strings.get(&0x38).context("IGC Gamebryo owner string is absent")?;
                animations.insert(event.meta.offset,self.resolve_igc_entity_binding(name,animation.entity_binding)?);
            }
            if let Some(animation)=&event.animation {
                let name=event.strings.get(&0x38).context("IGC animation owner string is absent")?;
                animations.insert(event.meta.offset,self.resolve_igc_entity_binding(name,animation.bindings[0])?);
            }
            let Some(sound)=&event.sound else{continue};
            if !matches!(sound.mode,0|2|3|5){continue;}
            let name=event.strings.get(&0x38).context("IGC sound entity string is absent")?;
            let owner=self.resolve_igc_entity_binding(name,sound.entity_binding)?;
            owners.insert(event.meta.offset,owner);
        }
        for control in self.igc.prepared_scenes()?.flat_map(|scene|&scene.hooks).flat_map(|h|&h.controls) {
            let mut bound=[None;3];
            for (i,offset) in [4,28,52].into_iter().enumerate() {
                bound[i]=self.resolve_igc_entity_binding(control.strings.get(&offset).context("IGC control entity string is absent")?,control.bindings[i])?;
            }
            controls.insert(control.offset,bound);
        }
        self.igc_sound_owners=owners;self.igc_control_owners=controls;self.igc_animation_owners=animations;self.igc_dialogue_owners=dialogues;Ok(())
    }
    fn dispatch_igc_control(&mut self,control:&super::igc::EntityControl)->Result<()> {
        use super::em2_igc_control::{Operation,PlayerWrite};
        let operations=super::em2_igc_control::operations(control.flags,control.values);
        if operations.is_empty(){return Ok(());}
        let owner=self.igc_control_owners.get(&control.offset).context("IGC control entities were not bound")?[0];
        let Some(owner)=owner.filter(|o|!self.retired.contains(o)) else{return Ok(())};
        ensure!(owner==self.player,"Non-player IGC control physics/pose receivers are not connected");
        let mut rotation=None;
        // Preflight all receivers before committing any of this control's
        // ordered effects. Missing services must not leave a half-applied warp.
        for operation in &operations {
            match operation {
                Operation::Motion(_)|Operation::AnimationCulling(false)=>{},
                Operation::Location(options)=>{
                    
                    rotation=Some(super::em2_igc_control::rotation(control.rotation,self.entities[owner].runtime_rotation,options[2])?);
                },
                other=>anyhow::bail!("IGC control receiver {other:?} is not connected"),
            }
        }
        for operation in operations {
            self.igc_player_writes.push_back(match operation {
                Operation::Motion(enabled)=>PlayerWrite::Motion(enabled),
                Operation::Location(options)=>{
                    let rotation=rotation.context("IGC teleport rotation was not prepared")?;
                    PlayerWrite::Pose {position:control.position,rotation,ground_snap:options[0]}
                },
                Operation::AnimationCulling(enabled)=>PlayerWrite::AnimationCulling(enabled),
                _=>unreachable!("IGC control receiver was preflighted"),
            });
        }
        self.igc_control_dispatches+=1;Ok(())
    }
    pub fn apply_playtest_igc_controls(&mut self,mut apply:impl FnMut(super::em2_igc_control::PlayerWrite)->Result<Option<[f32;3]>>)->Result<usize> {
        let mut applied=0;
        while let Some(write)=self.igc_player_writes.front().cloned() {
            let position=match apply(write.clone()) {Ok(position)=>position,Err(error)=>{self.error.get_or_insert_with(||format!("IGC player control publication: {error:#}"));return Err(error);}};
            if let super::em2_igc_control::PlayerWrite::Pose{rotation,..}=write {
                let position=position.context("IGC pose publication returned no final location")?;
                let entity=&mut self.entities[self.player];entity.runtime_rotation=rotation;
                entity.transform=super::render_transform::from_entity(rotation,position,entity.authored_scale);
            }
            self.igc_player_writes.pop_front();self.igc_control_writes_applied+=1;applied+=1;
        }Ok(applied)
    }
    /// Explicit shared-controller playtest publication; native transform
    /// notifications and authored EM2 motion lifecycle remain separate.
    pub fn source_description(&self,scene:usize,id:u32)->serde_json::Value {
        self.source_owners.get(&(scene,id)).map(|&owner|{
            let e=&self.entities[owner];
            let models=e.components.iter().flat_map(|c|&c.properties).filter(|p|p.name.to_ascii_lowercase().contains("nif")&&p.value.is_string()).map(|p|p.value.clone()).collect::<Vec<_>>();
            serde_json::json!({"name":e.name,"models":models})
        }).unwrap_or(serde_json::Value::Null)
    }
    fn dispatch_igc(&mut self,hook:&super::igc::Hook,control:Option<&super::igc::EntityControl>,event:Option<&super::igc::Event>)->Result<()> {
        if let Some(control)=control {
            return self.dispatch_igc_control(control).with_context(||format!("IGC control {}",hook.meta.name));
        }
        let event=event.context("IGC dispatch has no payload")?;
        if let Some(animation)=&event.animation {
            use super::em2_igc_animation::Operation;
            let offsets=[0x50,0x64,0x78,0x8c,0xa0];
            let valid=std::array::from_fn(|i|animation.bindings[i+1][1]!=u32::MAX&&event.strings.get(&offsets[i]).is_some_and(|s|!s.is_empty()));
            let operations=super::em2_igc_animation::operations(animation.flags,valid);
            if operations.is_empty(){return Ok(());}
            let owner=self.igc_animation_owners.get(&event.meta.offset).context("IGC animation owner was not bound")?;
            let Some(owner)=owner.filter(|o|!self.retired.contains(o)) else{return Ok(())};
            ensure!(owner==self.player,"IGC non-player animation publication is not connected");
            // Preflight the entire event before publishing any side effect.
            for op in &operations {match op {
                Operation::Event=>{
                    ensure!(animation.bindings[3][1]==0,"IGC overloaded animation event string is not connected");
                    ensure!(super::em2_animation_events::native_id(&event.strings[&0x78]).is_some(),"IGC animation event is absent from original PC registry");
                },
                Operation::Link=>ensure!(animation.bindings[1][1]==0&&animation.bindings[2][1]==0,"IGC overloaded dynamic clip strings are not connected"),
                other=>anyhow::bail!("IGC animation {other:?} receiver is not connected"),
            }}
            for operation in operations {
                self.igc_player_writes.push_back(match operation {
                    Operation::Event=>super::em2_igc_control::PlayerWrite::AnimationEvent(event.strings[&0x78].clone()),
                    Operation::Link=>super::em2_igc_control::PlayerWrite::AnimationClip{resource:event.strings[&0x64].clone(),node:event.strings[&0x50].clone()},
                    _=>unreachable!("Animation receiver was preflighted"),
                });
            }
            self.igc_animation_dispatches+=1;return Ok(());
        }
        if let Some(sound)=&event.sound {
            // Original PC af24c0: six operations, all other modes have no
            // effects. Posting/state changes are asynchronous queue ingress.
            match sound.mode {
                0=>{
                    ensure!(sound.name_binding[1]==0,"IGC sound overloaded event string is not connected");
                    let owner=self.igc_sound_owners.get(&event.meta.offset).context("IGC sound entity was not bound before playback")?;
                    if let Some(owner)=owner.filter(|o|!self.retired.contains(o)) {
                        self.audio_posts.post(&self.audio,owner,event.strings.get(&0x50).context("IGC sound event name is absent")?)?;
                    }
                },
                1=>{
                    ensure!(sound.name_binding[1]==0&&sound.state_binding[1]==0,"IGC sound overloaded state strings are not connected");
                    let group=super::em2_audio::event_id(event.strings.get(&0x50).context("IGC sound state group is absent")?)?;
                    let state=super::em2_audio::event_id(event.strings.get(&0x64).context("IGC sound state name is absent")?)?;
                    self.audio_posts.set_state(group,state);
                },
                2..=5=>anyhow::bail!("IGC sound mode {} needs switch/parameter/stop receivers",sound.mode),
                _=>{},
            }
            return Ok(());
        }
        if let Some(system)=&event.system_control {
            ensure!(system.flags&!7==0,"IGC system control flags {:#x} need additional receivers",system.flags);
            if system.flags&1!=0 {self.hud.start_in(system.duration,false)?;}
            else if system.flags&2!=0 {self.hud.start_in(system.duration,true)?;}
            else if system.flags&4!=0 {self.hud.start_out(system.duration)?;}
            return Ok(());
        }
        if let Some(camera)=&event.camera {
            // af27a0 has no effects when none of its five operation bits is set.
            if camera.flags&31==0{return Ok(());}
            if camera.flags&1==0&&camera.flags&2!=0 {anyhow::bail!("IGC camera release/transition receiver is not connected");}
            let name=event.strings.get(&0x38).context("IGC camera target is absent")?;
            let owner=self.entities.iter().position(|e|e.name==*name).filter(|o|!self.retired.contains(o))
                .with_context(||format!("IGC camera target {name} is not live"))?;
            if camera.flags&1!=0 {
                ensure!(camera.duration==0.&&camera.transition==0&&event.strings.get(&0x50).is_none_or(|s|s.is_empty()),"IGC camera blend/focus receiver is not connected");
                return self.gamebryo_cameras.select_immediate(owner);
            }
            if camera.flags&2==0&&camera.flags&4!=0 {
                ensure!(camera.parameter_78==0.&&camera.parameter_7c==0.&&camera.option_8c==1&&camera.option_8e==0&&camera.option_90==0,"IGC camera animation options require additional receivers");
                return self.gamebryo_cameras.request(owner,event.strings.get(&0x68).context("IGC camera sequence is absent")?);
            }
            anyhow::bail!("IGC camera flags {:#x} require release/activation receivers",camera.flags);
        }
        if let Some(animation)=&event.gamebryo_animation {
            // PC af38d0 mode0 forwards to the same af6390 utility used by
            // IGCCamera animation. Other modes own actor-update flags.
            if animation.mode>2 {return Ok(());}
            let owner=*self.igc_animation_owners.get(&event.meta.offset).context("IGC Gamebryo owner was not bound")?;
            let Some(owner)=owner.filter(|owner|!self.retired.contains(owner))else{return Ok(())};
            ensure!(animation.mode==0,"IGC Gamebryo actor flag receiver is not connected");
            ensure!(animation.name_binding[1]==0,"IGC overloaded Gamebryo sequence name is not connected");
            let name=event.strings.get(&0x50).context("IGC Gamebryo sequence is absent")?;
            if name.is_empty(){return Ok(());}

            ensure!(animation.scalar60==0.&&animation.scalar64==0.&&animation.options==[1,0,0],"IGC Gamebryo animation options require additional receivers");
            return self.gamebryo_cameras.request(owner,name);
        }
        let parameters=event.strings.values().filter(|s|!s.is_empty()).cloned().collect::<Vec<_>>();
        anyhow::bail!("IGC event receiver type {} ({}) is not connected: {:?}",event.meta.kind,event.meta.name,parameters)
    }
    fn advance_igc(&mut self,delta:f32,execution:&mut Scripts,registry:&mut Scripts,globals:&mut super::em2_global_state::State)->Result<()> {
        let mut catalog=std::mem::take(&mut self.igc);
        let result=catalog.advance_playtest(delta,|hook,control,event|{
            if control.is_none() {if let Some(event)=event {if let Some(callback)=&event.lua {
                ensure!(callback.binding[1]==0,"IGC overloaded Lua callback strings are not connected");
                let source=event.strings.get(&0x38).context("IGC Lua callback string is absent")?;
                let origin=self.igc_origin_token;
                execution.with_world_host(
                    |lua,name,args|self.call_native(lua,name,args,registry,globals),
                    |execution|{execution.invoke_igc_callback(source,origin)?;Ok(())},
                )?;
                return Ok(());
            }}}
            if control.is_none() {if let Some(event)=event {if let Some(dialogue)=&event.dialogue {
                // PC af6090 treats selectors as unsigned, routes0/1/2, and
                // ignores all others. GetPlayer returns slots without enabled filtering.
                if dialogue.selector>2{return Ok(());}
                ensure!(dialogue.name_binding[1]==0,"IGC overloaded dialogue name is not connected");
                let owner=*self.igc_dialogue_owners.get(&event.meta.offset).context("IGC dialogue marker was not bound")?;
                let name=event.strings.get(&0x50).context("IGC dialogue sequence name is absent")?;
                let context=match dialogue.selector{1=>self.players[0].entity,2=>self.players[1].entity,_=>None};
                let index=context.and_then(|owner|self.players.iter().position(|p|p.entity==Some(owner)));
                let admitted=self.sequences.prepare(owner,Some(name),context,index,globals)?;
                let presented=if admitted {
                    let presented=self.sequences.present_prepared(globals,|source,owner|execution.schedule_story_source(source,owner))?;
                    ensure!(presented,"IGC dialogue admitted but native/playtest presentation is unavailable");presented
                }else{false};
                self.igc_dialogue_requests+=1;
                self.last_igc_dialogue=Some(serde_json::json!({"owner":owner,"name":name,"selector":dialogue.selector,"context":context,"admitted":admitted,"presented":presented,
                    "presentation":"shared manual text panel; native voice,UI,speaking animation and completion remain pending"}));
                return Ok(());
            }}}
            self.dispatch_igc(hook,control,event)
        });
        self.igc=catalog;result
    }
    pub fn playtest_player_source(&self)->(usize,u32){self.source_keys[self.player]}
    pub fn bind_gamebryo_draw(&mut self,scene:usize,entity:u32,path:&str,draw:usize,mesh:&crate::render_support::scene::CachedNifMesh)->Result<()> {
        let Some(&owner)=self.source_owners.get(&(scene,entity))else{return Ok(())};
        self.gamebryo_cameras.bind_draw(owner,path,draw,mesh)
    }
    pub fn apply_gamebryo_models(&mut self,draws:&mut [crate::render_support::viewport3d::GpuDrawCommand])->Result<()> {
        if let Err(error)=self.gamebryo_cameras.apply_models(&self.entities,draws){
            let message=format!("Gamebryo model pose publication: {error:#}");self.error=Some(message.clone());anyhow::bail!("{message}");
        }Ok(())
    }
    pub fn playtest_camera_frame(&self)->Option<super::nif_camera::Frame>{self.gamebryo_cameras.frame()}
    pub fn camera_source(&self,scene:usize,id:u32)->bool {self.source_owners.get(&(scene,id)).is_some_and(|&owner|self.gamebryo_cameras.contains(owner))}

    pub fn playtest_use_target(&self,position:[f32;3])->Result<Option<usize>> {
        if self.error.is_some() || !self.players[0].enabled || !self.abilities.basic_enabled(self.player,"Use")? {return Ok(None);}
        let mut selected=super::usable_selection::Selection::default();
        for &owner in &self.usable_owners {
            let entity=&self.entities[owner];
            if self.hidden.contains(&owner)||self.retired.contains(&owner)||self.disabled_usables.contains(&owner){continue;}
            let Some(component)=entity.components.iter().find(|c|c.class_name=="UsableComponent") else{continue};
            let properties=super::em2_usable::Properties::read(component)?;
            if !properties.allows_player(Some(0)) || self.sequences.blocks_playtest_input()&&!properties.during_conversation {continue;}
            selected.consider(position,owner,[entity.transform[12],entity.transform[13],entity.transform[14]],properties.radius);
        }
        Ok(selected.owner())
    }
    /// Immediate marker-use phase in explicit shared playtests. Physical grabs,
    /// delayed use, cooperative actions and native feedback/release events still
    /// need their owners. Admit authored IO, never substitute a story outcome.
    pub fn try_playtest_use(&mut self,position:[f32;3],time:f32)->Result<bool> {
        let Some(owner)=self.playtest_use_target(position)? else{return Ok(false)};
        ensure!(time.is_finite()&&time>=self.time,"Invalid use-event time");
        let entity=&self.entities[owner];
        ensure!(entity.components.iter().all(|c|matches!(c.class_name.as_str(),
            "JPSTransformationComponent"|"InputOutputComponent"|"UsableComponent"|"ChildComponent"|"FXComponent")),
            "{} requires non-marker use/grab component receivers",entity.name);
        let usable=entity.components.iter().find(|c|c.class_name=="UsableComponent").unwrap();
        let delay=property(usable,"UseDelay").map(|v|v.as_f64().context("Invalid use delay")).transpose()?.unwrap_or(0.);
        ensure!(delay==0.,"{} requires delayed-use animation/effect receivers",entity.name);
        let once=property(usable,"UsableOnce").map(|v|v.as_bool().context("Invalid usable-once flag")).transpose()?.unwrap_or(false);
        self.outputs.emit(owner,"OnUse",self.player,time);
        if once{self.disabled_usables.insert(owner);}
        self.uses_started+=1;self.last_usable=Some(owner);Ok(true)
    }
    pub fn playtest_player_matrix(&self)->[f32;16] {
        let e=&self.entities[self.player];super::render_transform::from_entity(e.runtime_rotation,self.player_position(),1.)
    }
    pub fn publish_playtest_player_pose(&mut self,position:[f32;3],yaw:f32)->Result<()> {
        ensure!(position.iter().all(|v|v.is_finite())&&yaw.is_finite(),"Nonfinite playtest player pose");
        self.entities[self.player].set_controller_pose(position,yaw);Ok(())
    }
    pub fn enable_playtest_dialogue(&mut self,root:&std::path::Path,language:&str)->Result<()> {self.sequences.enable_playtest(root,language)?;self.igc.enable_playtest();Ok(())}
    pub fn playtest_dialogue_blocks_input(&self)->bool {self.sequences.blocks_playtest_input()}
    pub fn answer_playtest_dialogue(&mut self,answer:usize,globals:&mut super::em2_global_state::State,scripts:&mut Scripts)->Result<()> {self.sequences.answer_playtest(answer,globals,|source,owner|scripts.schedule_story_source(source,owner))}
    pub fn draw_playtest_dialogue(&mut self,ui:&egui::Ui,pad_edges:u16,globals:&mut super::em2_global_state::State,scripts:&mut Scripts) {
        if let Err(error)=self.sequences.draw_playtest(ui,pad_edges,globals,|source,owner|scripts.schedule_story_source(source,owner)){self.error.get_or_insert_with(||format!("Dialogue presentation: {error:#}"));}
    }
    /// An animation clip's sound trigger, posted on the actor's own entity like the game does.
    pub fn post_actor_sound(&mut self,scene:usize,entity:u32,name:&str)->Result<()> {
        let Some(&owner)=self.source_owners.get(&(scene,entity)) else {return Ok(())};
        if self.retired.contains(&owner) {return Ok(());}
        self.audio_posts.post(&self.audio,owner,name)?;Ok(())
    }
    /// Queued audio commands for the EM2 playback layer, and the bank files they refer to.
    pub fn take_audio(&mut self)->Vec<super::audio_post72::Pending> {self.audio_posts.drain()}
    /// Raw dump of every JPSLightComponent (BLOTWORX_LIGHT_DUMP=path): name, position and all properties.
    pub fn dump_lights(&self,path:&std::path::Path) {
        let rows:Vec<_>=self.entities.iter().enumerate().filter(|(owner,_)|!self.retired.contains(owner)).flat_map(|(_,e)|e.components.iter().filter(|c|c.class_name=="JPSLightComponent"||c.class_name=="JPSLightBakingComponent"||c.class_name=="RenderSceneEffectsComponent").map(move |c|serde_json::json!({
            "entity":e.name,"class":c.class_name,"position":&e.transform[12..15],
            "properties":c.properties.iter().map(|p|(p.name.clone(),p.value.clone())).collect::<serde_json::Map<_,_>>()}))).collect();
        let _=std::fs::write(path,serde_json::to_string_pretty(&rows).unwrap_or_default());
    }
    /// The runtime SH light list (360 0x827a34f8 / 0x8279ffc8): every JPSLightBakingComponent with
    /// GenerateRuntimeLight, in registration order. Raycasts stay off (the 360 only reads the raycast
    /// flag when a global switch is set). Invalid sources give an empty list plus the error text.
    pub fn sh_light_records(&mut self)->(Vec<std::sync::Arc<super::d3d9_sh::LightRecord>>,Option<String>) {
        match self.pc_light_sources.records(super::pc_float::Precision::Extended,false) {
            Ok(rows)=>(rows.into_iter().map(|(_,record)|record).collect(),None),
            Err(error)=>(Vec::new(),Some(format!("{error:#}"))),
        }
    }
    pub fn audio_bank_files(&self)->Vec<std::path::PathBuf> {self.audio.bank_files()}
    /// World position of a placed entity (audio emitters); None for the global object.
    pub fn entity_position(&self,owner:usize)->Option<[f32;3]> {
        let m=&self.entities.get(owner)?.transform;Some([m[12],m[13],m[14]])
    }
    pub fn player_position(&self) -> [f32; 3] {
        let m = &self.entities[self.player].transform;
        [m[12], m[13], m[14]]
    }
    pub fn halted(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn load_pc(
        scenes: &[SceneFile],
        scene_names: &[String],
        hull: &super::character_hull::ConvexHull,
        player: usize,
        scripts: &mut Scripts,
        settings:&std::collections::BTreeMap<String,String>,
        audio:super::em2_audio::Catalog,
        level:super::em2_level_manager::Manager,
        files:&[epic_mickey_lib_rs::packfile::VirtualFile],
    ) -> Result<Self> {
        ensure!(
            scenes.len() == scene_names.len(),
            "Scene event source identities differ"
        );
        let mut entities = Vec::new();
        let mut source_keys = Vec::new();
        let mut hidden = HashSet::new();
        let mut ids = HashMap::new();
        for (scene_index, scene) in scenes.iter().enumerate() {
            let layers = super::actor_io::component_index(&scene.objects);
            for source in scene
                .objects
                .iter()
                .filter(|source| scene.scene.contains(&source.link_id))
            {
                let mut components =
                    crate::render_support::scene::get_effective_components(source, &scene.objects);
                super::actor_io::inherit_connections(&mut components, &layers)?;
                let owner = entities.len();
                source_keys.push((scene_index, source.link_id.to_u32()));
                ensure!(
                    ids.insert(source.link_id.to_u32(), owner).is_none(),
                    "Ambiguous placed entity GUID"
                );
                if super::em2_entity_startup::State::decode(source.unknown, source.unknown_em2)
                    .start_hidden
                {
                    hidden.insert(owner);
                }
                for component in components
                    .iter()
                    .filter(|c| c.class_name == "InputOutputComponent")
                {
                    if let Some(groups) = property(component, "EntityGroups") {
                        for group in groups.as_array().context("Invalid entity groups")? {
                            let name = group.as_str().context("Invalid entity group name")?;
                            if !name.is_empty() {
                                scripts.register_existing_alias(owner, name)?;
                            }
                        }
                    }
                }
                let pose = crate::render_support::scene::get_entity_transform(source, &scene.objects);
                let matrix = crate::render_support::scene::runtime_matrix(source, &scene.objects);
                entities.push(RuntimeEntity {
                    authored_scale: pose.scale[0],
                    authored_rotation: std::array::from_fn(|i| pose.rotation[i / 3][i % 3]),
                    runtime_rotation: std::array::from_fn(|i| pose.rotation[i / 3][i % 3]),
                    model_asset: crate::render_support::scene::find_entity_nif_path_with_master(
                        source,
                        &scene.objects,
                    ),
                    mesh_draws: HashMap::new(),
                    jigsaw_asset: None,
                    jigsaw_states: Vec::new(),
                    scene: scene_names[scene_index].clone(),
                    id: source.link_id.to_u32(),
                    name: source.name.clone(),
                    transform: matrix,
                    authored_transform: matrix,
                    components,
                    draws: 0..0,
                    collision_parts: 0..0,
                });
            }
        }
        ensure!(player < entities.len(), "Scene event player is missing");
        let layer = entities[player]
            .components
            .iter()
            .find(|c| c.class_name == "PhysicsRigidBodyCharacterComponent")
            .and_then(|c| property(c, "Collision Layer"))
            .and_then(|v| v.as_u64())
            .context("Player collision layer missing")?;
        let mut outputs = Outputs::default();
        for (owner, entity) in entities.iter().enumerate() {
            outputs.bind_resolved(owner, &entity.name, &entity.components, |row| {
                match row.instigator.as_deref().unwrap_or("") {
                    "_self" => Ok(Some(owner)),
                    "_player" => Ok(Some(player)),
                    // Human player two and the AI companion are distinct
                    // selectors even when they share the second manager slot.
                    "_player2" => scripts.companion_entity(Some(false)),
                    "_buddyAI" => scripts.companion_entity(Some(true)),
                    "_player_or_ai1" => Ok(Some(player)),
                    "_player_or_ai2" => scripts.companion_entity(None),
                    "" => Ok(None),
                    name if !name.starts_with('_') => Ok(scripts.resolve_name(name)),
                    name => anyhow::bail!("Unsupported remote IO source {name}"),
                }
            })?;
        }
        let mut queries = super::physics::Collision::new(Vec::new())?;
        queries.set_player_hull(hull)?;
        let mut volumes = Vec::new();
        let mut source_errors = Vec::new();
        for (owner, entity) in entities.iter().enumerate() {
            for component in entity
                .components
                .iter()
                .filter(|c| c.class_name == "PhysicsTriggerComponent")
            {
                let result = (|| -> Result<Option<usize>> {
                    let layers = property(component, "Trigger Collision Layers")
                        .map(|v| v.as_array().context("Invalid trigger layers"))
                        .transpose()?;
                    if !layers
                        .is_some_and(|layers| layers.iter().any(|v| v.as_u64() == Some(layer)))
                    {
                        return Ok(None);
                    }
                    ensure!(
                        !property(component, "Trigger Is Dynamic")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                        "Dynamic trigger pose tracking is pending"
                    );
                    ensure!(
                        property(component, "Trigger Proxy NIF File Path")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .is_empty(),
                        "Model-based trigger geometry is pending"
                    );
                    let size = point(
                        property(component, "Trigger Size").context("Trigger size missing")?,
                    )?;
                    if size.contains(&0.) {
                        return Ok(None);
                    }
                    let corners = (0..8)
                        .map(|i| {
                            super::havok_2012_geometry::point(
                                &entity.transform,
                                std::array::from_fn(|axis| {
                                    if i & (1 << axis) == 0 {
                                        -size[axis] * 0.5
                                    } else {
                                        size[axis] * 0.5
                                    }
                                }),
                            )
                        })
                        .collect::<Vec<_>>();
                    // Native LoadTriggerData halves Trigger Size; its convex
                    // box generator receives radius zero (4153682/4153508).
                    Ok(Some(queries.add_static_convex(&corners, 0., owner)?))
                })();
                let result=result.and_then(|shape| {
                    // PC property descriptor9dad30..9dad67 defaults false.
                    // Keep disabled shapes for subsequent enable/lifecycle work;
                    // their authored bounds must not produce initial contacts.
                    let disabled=property(component,"Trigger Start Disabled")
                        .map(|v|v.as_bool().context("Invalid trigger disabled property")).transpose()?.unwrap_or(false);
                    Ok(shape.map(|shape|(shape,disabled)))
                });
                match result {Ok(Some((shape,disabled)))=>volumes.push(Volume {owner,shape,inside:false,disabled}),Ok(None)=>{},
                    Err(error)=>source_errors.push(serde_json::json!({"entity":entity.name,"component":component.class_name,"error":format!("{error:#}")})),}
            }
        }
        Self::prepare_prefabs(&entities,scripts,&mut source_errors)?;
        let sequences=super::em2_sequences::Catalog::load(&entities,files)?;
        let igc=super::igc::Catalog::load(files);
        let gamebryo_cameras=super::gamebryo_camera::State::load(&entities,files,igc.camera_targets());
        let mut abilities=super::em2_abilities::State::default();
        for (owner,entity) in entities.iter().enumerate() {
            let human=entity.components.iter().any(|c|matches!(c.class_name.as_str(),"BasicPlayerControllerComponent"|"MickeyPlayerControllerComponent"));
            let ai=entity.components.iter().any(|c|c.class_name=="AIAbilitiesControllerComponent");
            if human||ai {abilities.register(owner,human,ai,!entity.components.iter().any(|c|c.class_name=="MickeyPlayerControllerComponent"))?;}
        }
        let mut cameras=[super::camera_overrides::Controller::new(settings)?,super::camera_overrides::Controller::new(settings)?];
        // PC97acb0 slot constructor; ee1e19 (Mickey base+fc = outer+10c)
        // and f797fe (buddy+150) initialize their retained handles to null.
        let mut player_constraints=super::player_constraints::State::new(vec![super::player_constraints::Slot {active:false,handle:0};2]);
        for (owner,entity) in entities.iter().enumerate() {
            let mut components=Vec::new();
            for (class,kind) in [("AIBuddyBrainComponent",super::player_constraints::Kind::Buddy),("MickeyPlayerControllerComponent",super::player_constraints::Kind::Player)] {
                ensure!(entity.components.iter().filter(|c|c.class_name==class).count()<=1,"Ambiguous player constraint component {class}");
                if entity.components.iter().any(|c|c.class_name==class) {components.push((kind,0));}
            }
            player_constraints.register(owner,&components)?;
        }
        for camera in &mut cameras {camera.data.initialize_profiles(settings)?;}
        let usable_owners=entities.iter().enumerate().filter(|(_,e)|e.components.iter().any(|c|c.class_name=="UsableComponent")).map(|(owner,_)|owner).collect();
        let pc_render_profiles=std::sync::Arc::new(std::sync::Mutex::new(super::render_profiles::Registry::default()));
        Self::refresh_pc_render_profiles(&pc_render_profiles,&entities,&HashSet::new())?;
        let mut pc_light_sources=super::d3d9_scene_lights::Registry::default();
        Self::refresh_pc_light_sources(&mut pc_light_sources,&entities,&source_keys,&HashSet::new());
        Ok(Self {
            pc_render_profiles,pc_light_sources,hud:super::em2_hud::State::new(settings)?,tools:super::em2_player_tools::State::new(settings),child_links:Vec::new(),child_failures:HashMap::new(),previous_hidden:HashSet::new(),pending_hide_hooks:Vec::new(),source_owners:source_keys.iter().enumerate().map(|(i,k)|(*k,i)).collect(),drip_request_count:0,last_drip_request:None,abilities,sequences,igc,igc_sound_owners:HashMap::new(),igc_animation_owners:HashMap::new(),igc_animation_dispatches:0,igc_dialogue_owners:HashMap::new(),igc_dialogue_requests:0,last_igc_dialogue:None,igc_control_owners:HashMap::new(),igc_player_writes:VecDeque::new(),igc_control_dispatches:0,igc_control_writes_applied:0,igc_origin_token:0,gamebryo_cameras,entities,
            level,movies:Default::default(),maps:Default::default(),quest_path:Default::default(),music:Default::default(),music_overrides:Default::default(),audio,audio_posts:Default::default(),
            cameras,camera_manager:None,pending_camera_clear:None,player_constraints,guardians:Default::default(),camera_trackers:Vec::new(),templates:None,
            hidden,
            ids,
            queries,
            volumes,
            outputs,
            pending: VecDeque::new(),
            retained_commands: Vec::new(),
            player,
            cutscene_input:super::em2_cutscene_input::State::main_playtest(),
            game_state_events:Default::default(),
            players: [PlayerSlot {enabled:true,control_type:0,entity:Some(player)},
                PlayerSlot {enabled:true,control_type:1,entity:scripts.companion_entity(None)?}],
            source_keys,removal_queue:VecDeque::new(),retired:HashSet::new(),pending_player_event:None,player_events_delivered:0,last_player_change:None,
            entered: 0,
            exited: 0,
            callbacks: 0,
            skipped: 0,
            error: None,
            time: 0.,
            source_errors,disabled_usables:HashSet::new(),usable_owners,uses_started:0,last_usable:None,pending_collision_retirements:VecDeque::new(),
        })
    }
    /// Prefab Lua data and membership, as EM1's entity initialization does: GetPrefabData needs
    /// each JPSPrefabComponent's table, and members share their root's table. Nested prefabs:
    /// smaller prefabs bind first and a member keeps the first (innermost) prefab it joins.
    fn prefab_members(entity:&RuntimeEntity)->Result<Vec<u32>> {
        let mut out=Vec::new();
        for component in entity.components.iter().filter(|c|c.class_name=="JPSPrefabComponent") {
            for p in component.properties.iter().filter(|p|p.name=="Prefab Entities") {
                for id in p.value.as_array().context("Prefab Entities must be an array")? {
                    let id=u32::try_from(id.as_u64().context("Invalid prefab member identity")?)?;
                    if id!=0 {out.push(id);}
                }
            }
        }
        Ok(out)
    }
    fn prepare_prefabs(entities:&[RuntimeEntity],scripts:&mut Scripts,errors:&mut Vec<serde_json::Value>)->Result<()> {
        let mut roots=Vec::new();
        for (owner,entity) in entities.iter().enumerate() {
            let prefabs=entity.components.iter().filter(|c|c.class_name=="JPSPrefabComponent").collect::<Vec<_>>();
            let Some(component)=prefabs.first() else {continue};
            let result=(||->Result<()> {
                ensure!(prefabs.len()==1,"Multiple prefab components on one entity");
                let rows=|name:&str|->Result<Vec<String>> {
                    let Some(p)=component.properties.iter().find(|p|p.name==name) else {return Ok(Vec::new())};
                    p.value.as_array().with_context(||format!("{name} must be an array"))?.iter()
                        .map(|row|row.as_str().map(str::to_owned).with_context(||format!("{name} row must be a string"))).collect()
                };
                scripts.prepare_prefab_data(owner,&rows("Prefab Data")?,&rows("Prefab Data Override")?)
            })();
            match result {
                Ok(())=>roots.push(owner),
                Err(error)=>errors.push(serde_json::json!({"entity":entity.name,"component":"JPSPrefabComponent","error":format!("Prefab Lua data: {error:#}")})),
            }
        }
        let identities=entities.iter().enumerate().map(|(i,e)|((e.scene.as_str(),e.id),i)).collect::<HashMap<_,_>>();
        let mut sized=Vec::new();
        for owner in roots {
            match Self::prefab_members(&entities[owner]) {
                Ok(members)=>sized.push((members.len(),owner,members)),
                Err(error)=>errors.push(serde_json::json!({"entity":entities[owner].name,"component":"JPSPrefabComponent","error":format!("Prefab Lua ownership: {error:#}")})),
            }
        }
        sized.sort_by_key(|(len,owner,_)|(*len,*owner));
        for (_,owner,members) in sized {
            for id in members {
                // a member in another loaded scene, or not loaded in this state, has no table
                let Some(&member)=identities.get(&(entities[owner].scene.as_str(),id)) else {continue};
                if member==owner || scripts.prefab_owner(member)?.is_some() {continue;}
                if let Err(error)=scripts.bind_prefab_member(member,owner) {
                    errors.push(serde_json::json!({"entity":entities[member].name,"component":"JPSPrefabComponent","error":format!("Prefab Lua ownership: {error:#}")}));
                }
            }
        }
        Ok(())
    }
    /// GetRelativePrefabEntity(owner, suffix): the owner's prefab member whose IO EntityGroups
    /// holds exactly `suffix` (EM1 REL34d8b0 semantics; the rendered name isn't compared).
    fn relative_prefab_entity(&self,registry:&Scripts,owner:usize,suffix:&str)->Result<Option<usize>> {
        let root=registry.prefab_owner(owner)?.unwrap_or(owner);
        let Some(root_entity)=self.entities.get(root) else {return Ok(None)};
        for id in Self::prefab_members(root_entity)? {
            let Some(index)=self.entities.iter().position(|e|e.scene==root_entity.scene&&e.id==id) else {continue};
            if self.retired.contains(&index) {continue;}
            for io in self.entities[index].components.iter().filter(|c|c.class_name=="InputOutputComponent") {
                let Some(groups)=property(io,"EntityGroups") else {continue};
                for group in groups.as_array().context("Prefab member EntityGroups must be an array")? {
                    if group.as_str()==Some(suffix) {return Ok(Some(index));}
                }
            }
        }
        Ok(None)
    }
    pub fn install_children(&mut self,graph:&super::em2_children::Graph)->Result<()> {
        self.templates=Some(graph.templates.clone());
        let owners=self.source_keys.iter().enumerate().map(|(i,k)|(*k,i)).collect::<HashMap<_,_>>();
        for link in &graph.links {
            let parent=*owners.get(&(link.parent_scene,link.parent)).context("Child parent is not admitted")?;
            let child=*owners.get(&(link.child_scene,link.child)).context("Child is not admitted")?;
            self.child_links.push((parent,child,link.template.clone()));
        }
        for e in &graph.errors {if let Some(owner)=owners.get(&(e.scene,e.parent)){self.child_failures.entry(*owner).or_default().push(e.message.clone());}}
        Ok(())
    }
    pub fn install_camera_trackers(&mut self,bindings:&[super::em2_camera_tracking::Binding])->Result<()> {
        ensure!(self.camera_trackers.is_empty(),"PC camera trackers already installed");
        for binding in bindings {
            let owner=*self.source_owners.get(&(binding.scene,binding.entity)).context("Camera tracker source is not admitted")?;
            let target=*self.source_owners.get(&(binding.target_scene,binding.target)).context("Camera tracker target source is not admitted")?;
            self.camera_trackers.push(super::em2_camera_tracking::Owned::new(binding.clone(),owner,target,&self.entities[owner].components)?);
        }
        Ok(())
    }
    pub fn install_default_cameras(&mut self,bindings:&[super::em2_camera_manager::Binding],settings:&std::collections::BTreeMap<String,String>)->Result<()> {
        ensure!(self.camera_manager.is_none()&&bindings.len()==2,"PC default cameras already installed or malformed");
        let mut owners=[0;2];
        for (slot,binding) in bindings.iter().enumerate() {
            ensure!(binding.slot==slot,"Default camera slot order differs");
            owners[slot]=*self.source_owners.get(&(binding.scene,binding.entity)).context("Default camera source is not admitted")?;
        }
        self.camera_manager=Some(super::em2_camera_manager::Manager::new(bindings.to_vec(),owners,settings)?);Ok(())
    }
    pub fn publish_camera_renderer(&mut self,aspect:f32)->Result<()> {
        self.camera_manager.as_mut().context("Default camera manager is not installed")?.publish(aspect,&mut self.entities)
    }
    pub fn source_live(&self,scene:usize,id:u32)->bool {self.ids.get(&id).is_some_and(|o|self.source_keys[*o]==(scene,id)&&!self.retired.contains(o))}
    pub fn source_pose(&self,scene:usize,id:u32)->Result<super::native_transform_record::Transform> {
        let owner=*self.ids.get(&id).context("Child parent is not live")?;
        ensure!(self.source_keys[owner]==(scene,id),"Child parent identity mismatch");let e=&self.entities[owner];
        Ok(std::array::from_fn(|i|if i<9 {e.runtime_rotation[i]}else if i<12 {e.transform[3+i]}else{e.authored_scale}))
    }
    pub fn publish_child_pose(&mut self,write:&super::em2_child_pose::Write) {
        if let Some(&owner)=self.ids.get(&write.source.1) {if self.source_keys[owner]==write.source {
            let e=&mut self.entities[owner];e.runtime_rotation=write.pose[..9].try_into().unwrap();e.transform=super::render_transform::from_entity(e.runtime_rotation,write.pose[9..12].try_into().unwrap(),write.pose[12]);
        }}
    }
    pub fn hidden_source(&self,scene:usize,id:u32)->bool {
        self.source_owners.get(&(scene,id)).is_some_and(|o|self.hidden.contains(o)||self.retired.contains(o))
    }
    fn set_hidden_recursive(&mut self,owner:usize,hidden:bool)->Result<()> {
        ensure!(!self.child_failures.contains_key(&owner),"Visibility owner has failed child template clones");
                struct Backend<'a> {hidden:&'a mut HashSet<usize>,previous:&'a mut HashSet<usize>,links:&'a [(usize,usize,String)],hooks:&'a mut Vec<(usize,bool)>}
                impl super::visibility_children::Backend for Backend<'_> {
                    fn local_hidden(&mut self,e:usize,h:bool)->Result<()> {self.hooks.push((e,h));Ok(())}
                    fn children(&mut self,e:usize)->Result<Vec<usize>> {Ok(self.links.iter().filter(|(p,_,_)|*p==e).map(|(_,c,_)|*c).collect())}
                    fn hidden(&self,e:usize)->bool {self.hidden.contains(&e)}
                    fn previous_hidden(&self,e:usize)->bool {self.previous.contains(&e)}
                    fn snapshot_hidden(&mut self,e:usize){if self.hidden.contains(&e){self.previous.insert(e);}else{self.previous.remove(&e);}}
                    fn commit_hidden(&mut self,e:usize,h:bool){if h{self.hidden.insert(e);}else{self.hidden.remove(&e);}}
                }
        if self.hidden.contains(&owner)!=hidden {
            super::visibility_children::set(&mut Backend {hidden:&mut self.hidden,previous:&mut self.previous_hidden,links:&self.child_links,hooks:&mut self.pending_hide_hooks},owner,hidden)?;
        }Ok(())
    }
    fn apply_tool_visibility(&mut self)->Result<()> {
        while let Some(request)=self.tools.pending_visibility.front().cloned() {
            ensure!(!self.child_failures.contains_key(&request.owner),"Tool child owner has failed template clones");
            let name=super::child_entities::requested_name(&request.template)?;
            let child=self.child_links.iter().find(|(parent,child,template)|*parent==request.owner&&!self.retired.contains(child)&&template.eq_ignore_ascii_case(name)).map(|(_,child,_)|*child);
            if let Some(child)=child {
                self.set_hidden_recursive(child,request.hidden)?;
            }
            self.tools.pending_visibility.pop_front();
        }Ok(())
    }
    fn targets(&self, delivery: &Delivery, scripts: &Scripts) -> Result<Vec<usize>> {
        let row = &delivery.connection;
        Ok(match row.target.as_str() {
            "_self" => vec![row.owner],
            "_player" | "_player_or_ai1" => self.players[0].entity.into_iter().collect(),
            "_player2" => scripts.companion_entity(Some(false))?.into_iter().collect(),
            "_buddyAI" => scripts.companion_entity(Some(true))?.into_iter().collect(),
            "_player_or_ai2" => scripts.companion_entity(None)?.into_iter().collect(),
            "_activator" => delivery.activator.into_iter().collect(),
            "_target_group" => scripts
                .group_entity_names(&row.group)?
                .iter()
                .filter_map(|name| scripts.resolve_name(name))
                .collect(),
            "" => Vec::new(),
            name if !name.starts_with('_') => row
                .target_guid
                .and_then(|id| self.ids.get(&id).copied())
                .into_iter()
                .collect(),
            name => anyhow::bail!("Unsupported IO action target {name}"),
        })
    }
    pub fn is_retired_source(&self,scene:usize,entity:u32)->bool {
        self.source_owners.get(&(scene,entity)).is_some_and(|owner|self.retired.contains(owner))
    }
    pub fn flush_collision_retirements(&mut self,mut retire:impl FnMut(usize,u32)->Result<()>)->Result<usize> {
        let mut count=0;
        while let Some(&(scene,entity))=self.pending_collision_retirements.front() {
            if let Err(error)=retire(scene,entity){
                self.error.get_or_insert_with(||format!("Collision retirement: {error:#}"));return Err(error);
            }
            self.pending_collision_retirements.pop_front();count+=1;
        }Ok(count)
    }
    fn disable_player(&mut self,index:usize,registry:&mut Scripts)->Result<()> {
        // PC 978130 changes the enable byte before SwapPlayer (979970).
        // The replacement path is deliberately not approximated by unhiding
        // the previous entity: it must create/finalize a new archetype.
        ensure!(index==1 && self.players[index].control_type==1,
            "Primary/human player removal requires camera and input ownership handoff");
        self.players[index].enabled=false;
        let old=self.players[index].entity;
        if let Some(owner)=old {
            let entity=&mut self.entities[owner];
            // 9799e4..9799fb sets Collision Layer=13, not an event notification.
            for component in &mut entity.components {
                for property in component.properties.iter_mut().filter(|p|p.name=="Collision Layer") {
                    property.value=13.into();
                }
            }
            // World::MarkEntityForRemovalFromWorld (95fad0) queues one owner;
            // source handle retirement belongs to the later world boundary.
            if !self.removal_queue.contains(&owner) && !self.retired.contains(&owner) {
                self.removal_queue.push_back(owner);
            }
        }
        self.players[index].entity=None;
        registry.clear_em2_player_slot(index)?;
        self.pending_player_event=Some(PlayerChange {event:0xce,index,old_entity:old,new_entity:None});
        // PC CE camera listener f45400 compares new/old source handles with
        // its tracker; unaffected primary trackers return at f45552/f4579e.
        // The source CE camera listener only acts when its tracker is affected.
        // This admitted single-player branch removes the AI secondary, leaving
        // the primary tracker intact. Do not generalize this to human cameras.
        let change=self.pending_player_event.as_ref().unwrap();
        ensure!(change.old_entity!=Some(self.player) && change.new_entity!=Some(self.player),
            "Primary camera player-change dispatch is not implemented");
        self.player_events_delivered+=1;
        self.last_player_change=self.pending_player_event.take();
        Ok(())
    }
    /// Flush only the admitted runtime owners at the world boundary. Other
    /// source component initialization and replacement remain explicitly absent.
    fn flush_removals(&mut self,registry:&mut Scripts)->Result<()> {
        while let Some(&owner)=self.removal_queue.front() {
            if self.retired.contains(&owner){self.removal_queue.pop_front();continue;}
            let result=(||->Result<()> {
            for (_,child,_) in self.child_links.iter().filter(|(p,_,_)|*p==owner) {if !self.retired.contains(child)&&!self.removal_queue.contains(child){self.removal_queue.push_back(*child);}}
            self.abilities.retire(owner);self.tools.retire(owner);self.gamebryo_cameras.retire(owner);
            self.child_links.retain(|(parent,child,_)|*parent!=owner&&*child!=owner);
            self.child_failures.remove(&owner);
            self.previous_hidden.remove(&owner);
            self.pending_hide_hooks.retain(|(entity,_)|*entity!=owner);
            self.usable_owners.retain(|entity|*entity!=owner);
            self.disabled_usables.remove(&owner);
            self.outputs.remove_owner(owner);
            self.queries.retire_owner(owner)?;
            self.pending.retain(|delivery|delivery.connection.owner!=owner);
            self.volumes.retain(|volume|volume.owner!=owner);
            self.ids.remove(&self.entities[owner].id);
            registry.unregister_entity(owner)?;
            self.entities[owner].components=Vec::new();
            self.hidden.remove(&owner);
            self.retired.insert(owner);
            self.pending_collision_retirements.push_back(self.source_keys[owner]);
            Ok(())})();
            if let Err(error)=result{self.error.get_or_insert_with(||format!("Entity retirement: {error:#}"));return Err(error);}
            self.removal_queue.pop_front();
        }
        Ok(())
    }
    pub fn playtest_input_restricted(&self)->bool {self.cutscene_input.restricted(0)}
    fn set_supported_disabled(&mut self,owner:usize,disabled:bool)->Result<()> {
        let entity=&self.entities[owner];
        let trigger=entity.components.iter().any(|c|c.class_name=="PhysicsTriggerComponent");
        let usable=entity.components.iter().any(|c|c.class_name=="UsableComponent");
        // Triggers and usables are enabled/disabled here. Other component kinds have no enable
        // receiver in the playtest yet: they keep running, and each kind is reported once instead
        // of stopping the level's scripts. Triggers that don't touch the player have no volume.
        static SEEN:std::sync::Mutex<Option<HashSet<String>>>=std::sync::Mutex::new(None);
        for c in &entity.components {
            if !matches!(c.class_name.as_str(),"JPSTransformationComponent"|"InputOutputComponent"|"PhysicsTriggerComponent"|"UsableComponent"|"ChildComponent"|"FXComponent")
                && SEEN.lock().map(|mut s|s.get_or_insert_with(Default::default).insert(c.class_name.clone())).unwrap_or(false) {
                log::warn!("EM2 world service not implemented (ignored): Enable/Disable.{}",c.class_name);
            }
        }
        let position=self.player_position();
        for volume in self.volumes.iter_mut().filter(|v|v.owner==owner) {
            if volume.disabled==disabled{continue;}
            let inside=!disabled&&self.players[0].enabled&&!self.hidden.contains(&owner)
                && self.queries.shape_penetration(volume.shape,position)?>0.;
            // PhysicsTriggerComponent::SetDisabled reconciles existing touches
            // before returning. Keep these transitions in the shared IO queue.
            if inside!=volume.inside {
                let output=if inside{self.entered+=1;"OnStartTouch"}else{self.exited+=1;"OnEndTouch"};
                self.outputs.emit(owner,output,self.player,self.time);
                volume.inside=inside;
            }
            volume.disabled=disabled;
        }
        if usable {if disabled{self.disabled_usables.insert(owner);}else{self.disabled_usables.remove(&owner);}}
        Ok(())
    }
    fn set_camera_attributes(&mut self,registry:&Scripts,lua:&mlua::Lua,args:mlua::MultiValue,for_player:bool)->Result<()> {
        // PC f17160 pops the last entity argument. It applies to the current
        // script entity only if that entity is one of PlayerManager's owners.
        // ForPlayer (PC f17260) pops player first only with two or more
        // arguments, then the override. A null player selects both slots.
        let mut args=args;
        let target=if for_player && args.len()>1 {registry.resolve_native_entity_argument(lua,args.pop_back().unwrap())?}else{None};
        let Some(owner)=registry.resolve_native_entity_argument(lua,args.pop_back().unwrap_or(mlua::Value::Nil))? else{return Ok(());};
        if !self.entities.get(owner).is_some_and(|entity|entity.components.iter().any(|c|c.class_name=="CameraOverrideComponent")){return Ok(());}
        let context=if for_player {target}else{registry.current_callback_context()?.entity};
        let selected=self.players.iter().position(|slot|slot.entity.is_some() && slot.entity==context);
        if (!for_player || target.is_some()) && selected.is_none(){return Ok(());}
        for index in 0..self.players.len() {
        if selected.is_some_and(|slot|slot!=index) || self.players[index].entity.is_none(){continue;}
        let entities=&self.entities;let retired=&self.retired;
        let changed=self.cameras[index].overrides.push(owner,|candidate| {
            if retired.contains(&candidate) || !registry.has_entity(candidate){return Ok(None);}
            let component=entities[candidate].components.iter().find(|c|c.class_name=="CameraOverrideComponent")
                .context("Camera stack owner lost its override component")?;
            Ok(Some(super::camera_overrides::priority(component)?))
        })?;
        if changed {
            if let Some(selected)=self.cameras[index].overrides.selected {
                let component=self.entities[selected].components.iter().find(|c|c.class_name=="CameraOverrideComponent")
                    .context("Selected camera override is absent")?;
                let ids=&self.ids;
                self.cameras[index].apply(selected,component,|id|ids.get(&id).copied().map(Some)
                    .with_context(||format!("Unresolved camera auto-level spline GUID {id}")))?;
            }
        }
        }
        Ok(())
    }
    fn set_camera_values(&mut self,registry:&Scripts,lua:&mlua::Lua,name:&str,args:mlua::MultiValue)->mlua::Result<mlua::MultiValue> {
        let reset=name.starts_with("Reset");
        let request=if reset {super::em2_camera_data::Request::parse_reset(lua,args)?}else{super::em2_camera_data::Request::parse(lua,args)?};
        let target=registry.resolve_native_entity_argument(lua,request.target.clone())?;
        let selected=if target.is_none() {None}else{self.players.iter().position(|p|p.entity==target)};
        // PCf15280: null handle selects both slots; an entity which matches
        // neither player selects none. Disabled slots lack camera owners.
        if target.is_some()&&selected.is_none(){return Ok(mlua::MultiValue::new());}
        let dataset=if name.contains("Travel"){2}else if name.contains("Combat"){3}else{1};
        for index in 0..2 {
            if selected.is_some_and(|slot|slot!=index)||self.players[index].entity.is_none(){continue;}
            let camera=&mut self.cameras[index];
            if reset {camera.data.reset(dataset,request.duration)}else{camera.data.write(dataset,&request)}.map_err(mlua::Error::external)?;
            let tracker=self.camera_trackers.iter().find(|t|t.binding.slot==index);
            camera.data.dispatch(&mut PcCameraReceiver {tracker}).map_err(mlua::Error::external)?;
        }
        Ok(mlua::MultiValue::new())
    }
    fn drain_camera_clear(&mut self)->Result<()> {
        loop {
            let pending=self.pending_camera_clear.as_ref().context("Camera reset request is absent")?;
            let Some(&index)=pending.slots.get(pending.next_slot) else {
                self.pending_camera_clear=None;return Ok(());
            };
            let duration=pending.duration;
            let player=self.players[index].entity.filter(|owner|!self.retired.contains(owner));
            let component_present=player.is_some_and(|owner|self.entities[owner].components.iter().any(|c|c.class_name=="MickeyPlayerControllerComponent"));
            if self.cameras[index].clear_plan.is_none() {
                let manager=self.camera_manager.as_ref().context("Camera reset requires the initialized default-camera manager")?;
                let tracker=self.camera_trackers.iter().find(|t|t.binding.slot==index)
                    .is_some_and(|t|!self.retired.contains(&t.owner)&&self.entities[t.owner].components.iter().any(|c|c.class_name=="EntityTrackerComponent"));
                self.cameras[index].begin_clear(super::em2_camera_reset::Input {
                    duration,player:component_present,tracker,hud2d:self.hud.hud2d,faded:self.hud.faded,desired_cursor:self.hud.desired_cursor,
                    // Controller resolves these from the original registered CVars.
                    look_ahead:false,ledge_check:false,auto_level:false,default_fov:manager.lens.default_fov,
                })?;
            }
            let hud=&mut self.hud;
            let constraints=&mut self.player_constraints;
            let guardians=&mut self.guardians;
            self.cameras[index].drain_clear(|operation| {
                use super::em2_camera_reset::Operation::*;
                match operation {
                    DesiredCursor(value)=>hud.set_desired_cursor(value),
                    Hud2D(value)=>hud.set_hud2d(value),
                    Cursor(value)=>hud.set_cursor(value),
                    Constraint2D(value)=>constraints.set_active(index,value,player,|owner,kind,before,after| {
                        // Original f79b00/edb7f0 return immediately for an
                        // unchanged handle. Changed handles require retained
                        // subscriptions/attachment ownership, never a fake ack.
                        ensure!(before==after,"Constraint {kind:?} owner {owner}: changed-handle attachment/subscription receiver is not connected ({before}->{after})");
                        Ok(())
                    })?,
                    GuardianVisible(value)=>{
                        struct Effects;
                        impl super::guardians::Effects for Effects {
                            fn recompute(&mut self)->Result<bool>{anyhow::bail!("PC guardian population recomputation is not connected")}
                            fn present(&mut self,_:bool)->Result<()>{anyhow::bail!("PC primary-player guardian entity/audio presentation is not connected")}
                        }
                        guardians.set_visible(value,&mut Effects)?;
                    },
                    _=>anyhow::bail!("Camera world reset receiver {operation:?} is not connected"),
                }
                Ok(())
            },|restore|restore.restore(component_present,|group,_|anyhow::bail!("Camera {group:?} ability-group decrement receiver is not connected")),
            &mut PendingCameraWorld,None,None).with_context(||format!("Camera reset slot {index}"))?;
            self.pending_camera_clear.as_mut().unwrap().next_slot+=1;
        }
    }
    fn call_native(
        &mut self,
        lua: &mlua::Lua,
        name: &str,
        args: mlua::MultiValue,
        registry: &mut Scripts,
        globals: &mut super::em2_global_state::State,
    ) -> mlua::Result<mlua::MultiValue> {
        use mlua::{FromLua,Value};
        match name {
            "AnimGBSequence"=>{
                // Optional restart/speed/start-time arguments (e.g. Prefab_Animate_Doors) are accepted;
                // the sequence still starts from its beginning at normal speed.
                let owner=registry.resolve_native_entity_argument(lua,args.front().cloned().unwrap_or(Value::Nil))?;
                let name=lua.coerce_string(args.get(1).cloned().unwrap_or(Value::Nil))?
                    .map(|s|s.to_str().map(|s|s.to_owned())).transpose()?.unwrap_or_default();
                if let Some(owner)=owner.filter(|_|!name.is_empty()) {
                    if self.gamebryo_cameras.prepared(owner) {self.gamebryo_cameras.request(owner,&name).map_err(mlua::Error::external)?;}
                    else {
                        // Only NIF cameras are animated by this runtime so far; props (doors etc.)
                        // keep their pose instead of halting the level's scripts.
                        static ONCE:std::sync::Once=std::sync::Once::new();
                        ONCE.call_once(||log::warn!("EM2 world service not implemented (ignored): AnimGBSequence(non-camera)"));
                    }
                }
                Ok(mlua::MultiValue::new())
            }
            "GrabCameraNif"=>{
                if args.len()>2{return Err(mlua::Error::external("PC NIF camera transition/player options are not connected"));}
                let owner=registry.resolve_native_entity_argument(lua,args.front().cloned().unwrap_or(Value::Nil))?;
                let focus=registry.resolve_native_entity_argument(lua,args.get(1).cloned().unwrap_or(Value::Nil))?;
                if focus.is_some(){return Err(mlua::Error::external("PC NIF camera focus ownership is not connected"));}
                if let Some(owner)=owner {self.gamebryo_cameras.select_immediate(owner).map_err(mlua::Error::external)?;}
                Ok(mlua::MultiValue::new())
            }
            "PlayIGC"=>{
                if args.is_empty(){return Ok(mlua::MultiValue::new());}
                if matches!(self.cutscene_input.current(),1|7){return Ok(mlua::MultiValue::new());}
                if args.len()>2{return Err(mlua::Error::external("IGC overloaded parameter bindings are not connected"));}
                let name=lua.coerce_string(args.front().cloned().unwrap_or(Value::Nil))?
                    .map(|s|s.to_str().map(|s|s.to_owned())).transpose()?.unwrap_or_default();
                if name.is_empty(){return Ok(mlua::MultiValue::new());}
                // PC90bc40 ->6f1c10 pops a raw pointer, not an entity-name
                // query. Preserve a supplied handle even after retirement.
                let token=match args.get(1) {
                    Some(Value::LightUserData(value))=>value.0 as usize,
                    Some(value @ (Value::Table(_)|Value::Function(_)|Value::Thread(_)|Value::UserData(_)))=>value.to_pointer() as usize,
                    Some(Value::Error(_)|Value::Other(_))=>return Err(mlua::Error::external("PC foreign-value IGC origin conversion is not connected")),
                    _=>0,
                };
                self.igc.prepare(&name).map_err(mlua::Error::external)?;
                self.igc_origin_token=token;
                self.bind_igc_sound_owners().map_err(mlua::Error::external)?;
                self.igc.start_playtest().map_err(mlua::Error::external)?;
                Ok(mlua::MultiValue::new())
            }
            "RestrictCutSceneInput"|"UnrestrictCutSceneInput"=>{
                let owner=registry.resolve_native_entity_argument(lua,args.back().cloned().unwrap_or(Value::Nil))?;
                for index in 0..2 {
                    let slot=self.players[index];
                    if !slot.enabled||slot.entity.is_none()||owner.is_some_and(|o|slot.entity!=Some(o)){continue;}
                    self.cutscene_input.apply(index,name=="RestrictCutSceneInput").map_err(mlua::Error::external)?;
                }
                Ok(mlua::MultiValue::new())
            }
            "Enable"|"Disable"=>{
                let owner=registry.resolve_native_entity_argument(lua,args.back().cloned().unwrap_or(Value::Nil))?;
                if let Some(owner)=owner.filter(|o|!self.retired.contains(o)) {
                    self.set_supported_disabled(owner,name=="Disable").map_err(mlua::Error::external)?;
                }
                Ok(mlua::MultiValue::new())
            }
            "Hide"|"Unhide"|"Show"=>{
                // PC100ab00/100ab50 call96c090 with true/false; Show aliases
                // the exact Unhide wrapper. Share tool-child visibility state.
                let owner=registry.resolve_native_entity_argument(lua,args.back().cloned().unwrap_or(Value::Nil))?;
                if let Some(owner)=owner.filter(|o|!self.retired.contains(o)) {
                    self.set_hidden_recursive(owner,name=="Hide").map_err(mlua::Error::external)?;
                }
                Ok(mlua::MultiValue::new())
            }
            "DestroyEntity"=>{
                // PC955b50 protects active human/AI players before95fc40 queues
                // world removal. Use the existing boundary and child lifecycle.
                let owner=registry.resolve_native_entity_argument(lua,args.back().cloned().unwrap_or(Value::Nil))?;
                if let Some(owner)=owner.filter(|o|!self.retired.contains(o)) {
                    if !self.players.iter().any(|p|p.enabled&&p.entity==Some(owner))&&!self.removal_queue.contains(&owner) {
                        self.removal_queue.push_back(owner);
                    }
                }
                Ok(mlua::MultiValue::new())
            }
            // PC 977840 consumes its unused bool; Wii U differs.
            "EnableSplitScreen"=>Ok(mlua::MultiValue::new()),
            "PlayerSetDripsVisible"=>{
                let mut values=args.into_vec();
                let visible=super::em2_hud::pc_boolean(lua,values.pop().unwrap_or(Value::Nil))?;
                let owner=registry.resolve_native_entity_argument(lua,values.pop().unwrap_or(Value::Nil))?
                    .filter(|o|self.entities[*o].components.iter().any(|c|c.class_name=="MickeyPlayerControllerComponent"));
                if let Some(owner)=owner {
                    if self.child_failures.contains_key(&owner){return Err(mlua::Error::external("Drip owner has failed child clones"));}
                    let visited=self.child_links.iter().filter(|(p,c,_)|*p==owner&&!self.retired.contains(c)).map(|(_,c,_)|*c).collect::<Vec<_>>();
                    let mut matched=Vec::new();
                    for &child in &visited {if self.entities[child].name.contains("mk_") {
                        //eda7f0 uses the direct entity hidden setter, not the
                        //recursive EntityUtils::SetHidden hook dispatch.
                        if visible{self.hidden.remove(&child);}else{self.hidden.insert(child);}matched.push(child);
                    }}
                    self.drip_request_count=self.drip_request_count.wrapping_add(1);self.last_drip_request=Some(serde_json::json!({"owner":owner,"visible":visible,"visited_children":visited.len(),"matched":matched,"force_restart":false}));
                }
                Ok(mlua::MultiValue::new())
            },
            "HidePlayerTool"|"UnhidePlayerTool"=>{
                let index=i32::from_lua(args.back().cloned().unwrap_or(Value::Integer(0)),lua)?;
                let index=usize::try_from(index).ok().filter(|i|*i<2).ok_or_else(||mlua::Error::external("Invalid PC tool player slot"))?;
                let owner=self.players[index].entity;
                let has_animation=owner.is_some_and(|o|self.entities[o].components.iter().any(|c|c.class_name=="AnimationComponent"));
                self.tools.set_hidden(index,name=="HidePlayerTool",owner,has_animation).map_err(mlua::Error::external)?;
                Ok(mlua::MultiValue::new())
            },
            "UnlockAbility"|"LockAbility"=>{
                let mut values=args.into_vec();
                let ability=lua.coerce_string(values.pop().unwrap_or(Value::Nil))?;
                let ability=ability.map(|v|v.as_bytes().to_vec()).unwrap_or_default();
                let ability=std::str::from_utf8(ability.split(|b|*b==0).next().unwrap_or_default()).map_err(mlua::Error::external)?;
                let owner=registry.resolve_native_entity_argument(lua,values.pop().unwrap_or(Value::Nil))?;
                if let Some(owner)=owner {
                    let index=self.players.iter().position(|p|p.entity==Some(owner))
                        .ok_or_else(||mlua::Error::external("PC ability operation has no valid player slot; original would index outside its slot array"))?;
                    self.abilities.set_lock(index,owner,ability,name=="UnlockAbility",[true;2]).map_err(mlua::Error::external)?;
                }
                Ok(mlua::MultiValue::new())
            },
            // No health/costume systems in the EM2 playtest yet. Return safe values so scripts
            // that compare them don't fail on nil: the entity's authored health if it has one
            // (else 1, i.e. alive), and an empty costume name (the default outfit).
            "GetHealth"=>{
                let owner=registry.resolve_native_entity_argument(lua,args.front().cloned().unwrap_or(Value::Nil))?;
                let authored=owner.and_then(|o|self.entities.get(o)).and_then(|e|e.components.iter().flat_map(|c|c.properties.iter())
                    .find(|p|p.name.contains("Health")&&p.value.is_number()).and_then(|p|p.value.as_f64()));
                Ok(mlua::MultiValue::from_vec(vec![Value::Number(authored.filter(|h|*h>0.).unwrap_or(1.))]))
            }
            "GetCurrentCostumeName"=>Ok(mlua::MultiValue::from_vec(vec![Value::String(lua.create_string("")?)])),
            "GetRelativePrefabEntity"=>{
                let mut values=args.into_vec();
                if values.len()!=2 {return Err(mlua::Error::external("Relative prefab lookup requires owner and suffix"));}
                let suffix=lua.coerce_string(values.pop().unwrap())?.ok_or_else(||mlua::Error::external("Relative prefab suffix is not a string"))?;
                let suffix=suffix.to_str()?.split('\0').next().unwrap_or("").to_owned();
                let owner=registry.resolve_native_entity_argument(lua,values.pop().unwrap())?.filter(|o|!self.retired.contains(o));
                let found=match owner {Some(owner)=>self.relative_prefab_entity(registry,owner,&suffix).map_err(mlua::Error::external)?,None=>None};
                // EM1 returns entity name 0 (not nil) when nothing matches
                Ok(mlua::MultiValue::from_vec(vec![match found {Some(index)=>registry.entity_value(index)?,None=>Value::Integer(0)}]))
            }
            "LevelManager_GetCurrentLevelName"|"LevelManager_GetCurrentState"|"LevelManager_GetCurrentInterior"|"LevelManager_SetCurrentState"=>self.level.call(lua,name,args),
            "FireSequence"=>{
                let mut args=args.into_vec();let count=args.len();
                let selector=if count>2 {i32::from_lua(args.pop().unwrap(),lua)?}else{0};
                let name=if count>1 {match args.pop().unwrap() {Value::Nil=>None,value=>Some(mlua::String::from_lua(value,lua)?)}}else{None};
                let bytes=name.as_ref().map(|s|s.as_bytes().to_vec());
                let name=bytes.as_ref().map(|bytes|std::str::from_utf8(bytes.split(|b|*b==0).next().unwrap_or_default())).transpose().map_err(mlua::Error::external)?;
                let owner=registry.resolve_native_entity_argument(lua,args.pop().unwrap_or(Value::Nil))?.filter(|owner|!self.retired.contains(owner));
                let context=match selector {1=>self.players[0].entity.filter(|_|self.players[0].enabled),2=>self.players[1].entity.filter(|_|self.players[1].enabled),_=>None};
                let index=context.and_then(|owner|self.players.iter().position(|p|p.enabled && p.entity==Some(owner)));
                if self.sequences.prepare(owner,name,context,index,globals).map_err(mlua::Error::external)? {
                    if self.sequences.present_prepared(globals,|source,owner|registry.schedule_story_source(source,owner)).map_err(mlua::Error::external)? {return Ok(mlua::MultiValue::new());}
                    return Err(mlua::Error::external(self.sequences.pending_description()));
                }
                Ok(mlua::MultiValue::new())
            },
            "StartFadeIn"|"StartFadeInNoHUD"|"StartFadeOut"|"HideHud"|"ShowHud"|"SetCursorOnly"=>self.hud.call(lua,name,args),
            "BwxQueueMovie"=>self.movies.lua(lua,args),
            "SetMusicEvent"|"SetMusicTarget"|"GetMusicEvent"|"GetMusicTarget"=>self.music.call(lua,name,args),
            "AddMusicStateOverride"|"ReplaceMusicStateOverride"|"RemoveMusicStateOverride"|"RemoveAllMusicStateOverride"=>{
                let posts=&mut self.audio_posts;
                self.music_overrides.call(lua,name,args,|group,state|posts.set_state(group,state))
            }
            "AudioPostEventOn"=>{
                let mut args=args.into_vec();
                let name=String::from_lua(args.pop().unwrap_or(Value::Nil),lua)?;
                let owner=registry.resolve_native_entity_argument(lua,args.pop().unwrap_or(Value::Nil))?;
                if owner.is_none_or(|owner|self.retired.contains(&owner)){return Ok(mlua::MultiValue::new());}
                let name=name.split('\0').next().unwrap_or_default();
                let playing=self.audio_posts.post(&self.audio,owner.unwrap(),name).map_err(mlua::Error::external)?;
                Ok(mlua::MultiValue::from_vec(vec![Value::Integer(i64::from(playing))]))
            }
            "AudioSetState"|"AudioSetSwitch"|"AudioStopPlaying"|"AudioStopPlayingOn"|"AudioStopPlayingAll"|"AudioSetParameter"=>{
                // names (states, switches, parameters, events) use the same Wwise name hash as events
                use super::audio_post72::Pending;
                let args=args.into_vec();
                let hash=|text:&str|super::em2_audio::event_id(text.split('\0').next().unwrap_or_default()).map_err(mlua::Error::external);
                let strings:Vec<String>=args.iter().filter_map(|v|match v {Value::String(s)=>Some(s.to_string_lossy().to_string()),_=>None}).collect();
                let number=args.iter().rev().find_map(|v|match v {Value::Number(n)=>Some(*n),Value::Integer(i)=>Some(*i as f64),_=>None});
                let owner=match args.iter().find(|v|!matches!(v,Value::String(_)|Value::Number(_)|Value::Nil)) {
                    Some(v)=>registry.resolve_native_entity_argument(lua,v.clone())?.filter(|o|!self.retired.contains(o)),
                    None=>None,
                };
                let pending=match name {
                    "AudioSetState" if strings.len()>=2=>Pending::State {group:hash(&strings[strings.len()-2])?,state:{let s=hash(&strings[strings.len()-1])?;if s==0x2ca33bdb {0}else{s}},skip_transition:0,skip_extension:0},
                    "AudioSetSwitch" if strings.len()>=2=>Pending::Switch {owner:owner.unwrap_or(0x7fffffff),group:hash(&strings[strings.len()-2])?,value:hash(&strings[strings.len()-1])?},
                    "AudioStopPlayingAll"=>Pending::Stop {owner:None,playing_id:None,event:None},
                    "AudioStopPlaying"|"AudioStopPlayingOn"=>Pending::Stop {owner,
                        playing_id:args.iter().find_map(|v|match v {Value::Integer(i)=>Some(*i as u32),Value::Number(n)=>Some(*n as u32),_=>None}),
                        event:strings.last().map(|s|hash(s)).transpose()?},
                    "AudioSetParameter" if !strings.is_empty()=>Pending::Parameter {owner,id:hash(&strings[strings.len()-1])?,value:number.unwrap_or(0.) as f32},
                    _=>return Ok(mlua::MultiValue::new()),
                };
                self.audio_posts.push(pending);
                Ok(mlua::MultiValue::new())
            }
            "Quest_SetCriticalPath"|"Quest_GetCriticalPath"=>self.quest_path.call(lua,name,args),
            "SetPropertyFloat"|"GetPropertyFloat"=>{
                let request=super::em2_float_property::Request::parse(lua,name=="SetPropertyFloat",args)?;
                let Some(owner)=registry.resolve_native_entity_argument(lua,request.entity.clone())?
                    .filter(|owner|!self.retired.contains(owner)) else{return Ok(mlua::MultiValue::new());};
                let entity=self.entities.get_mut(owner).ok_or_else(||mlua::Error::external("Float property owner is absent"))?;
                request.apply(lua,&mut entity.components)
            }
            "SetActiveSubMap"|"SetActiveFullMap"=>{
                let owner=registry.resolve_native_entity_argument(lua,args.back().cloned().unwrap_or(Value::Nil))?
                    .filter(|&owner|self.entities.get(owner).is_some_and(|entity|entity.components.iter().any(|component|component.class_name=="MapVolumeComponent")));
                if name=="SetActiveSubMap" {self.maps.set_sub_map(owner);}else{self.maps.set_full_map(owner);}
                Ok(mlua::MultiValue::new())
            }
            "ClearCameraAttributes"|"ClearCameraAttributesForPlayer"=>{
                // Inverse of SetCameraAttributes: drop this override from every slot's stack and
                // apply the next one. When the stack empties, return to the default camera the same
                // way ClearAllCameraAttributes does (default duration).
                let mut args=args;
                let target=if name=="ClearCameraAttributesForPlayer"&&args.len()>1 {registry.resolve_native_entity_argument(lua,args.pop_back().unwrap())?}else{None};
                let Some(owner)=registry.resolve_native_entity_argument(lua,args.pop_back().unwrap_or(Value::Nil))? else {return Ok(mlua::MultiValue::new());};
                let mut emptied=Vec::new();
                for index in 0..2 {
                    if target.is_some()&&self.players[index].entity!=target {continue;}
                    if !self.cameras[index].overrides.owners.contains(&owner) {continue;}
                    self.cameras[index].overrides.owners.retain(|o|*o!=owner);
                    let entities=&self.entities;let retired=&self.retired;
                    let changed=self.cameras[index].overrides.refresh(|candidate| {
                        if retired.contains(&candidate)||!registry.has_entity(candidate){return Ok(None);}
                        let component=entities[candidate].components.iter().find(|c|c.class_name=="CameraOverrideComponent").context("Camera stack owner lost its override component")?;
                        Ok(Some(super::camera_overrides::priority(component)?))
                    }).map_err(mlua::Error::external)?;
                    match self.cameras[index].overrides.selected {
                        Some(selected) if changed=>{
                            let component=self.entities[selected].components.iter().find(|c|c.class_name=="CameraOverrideComponent").context("Selected camera override is absent").map_err(mlua::Error::external)?;
                            let ids=&self.ids;
                            self.cameras[index].apply(selected,component,|id|ids.get(&id).copied().map(Some).with_context(||format!("Unresolved camera auto-level spline GUID {id}"))).map_err(mlua::Error::external)?;
                        }
                        None=>emptied.push(index),
                        _=>{}
                    }
                }
                if !emptied.is_empty() && self.pending_camera_clear.is_none() && self.camera_manager.is_some() {
                    self.pending_camera_clear=Some(super::em2_camera_reset::Pending {slots:emptied,duration:-1.,next_slot:0});
                    self.drain_camera_clear().map_err(|error|mlua::Error::external(format!("{error:#}")))?;
                }
                Ok(mlua::MultiValue::new())
            }
            // App-level viewport culling (used by building-exit cameras). Blotworx draws one
            // full-screen viewport and culls by camera already, so there is nothing to change.
            "SetAppCulledViewport"|"ClearAppCulledViewport"=>Ok(mlua::MultiValue::new()),
            "SetCameraAttributes"|"SetCameraAttributesForPlayer"=>{self.set_camera_attributes(registry,lua,args,name=="SetCameraAttributesForPlayer").map_err(mlua::Error::external)?;Ok(mlua::MultiValue::new())},
            "ClearAllCameraAttributes"=>{
                let request=super::em2_camera_reset::Request::parse(lua,args)?;
                let target=registry.resolve_native_entity_argument(lua,request.target)?;
                let slots=(0..2).filter(|&index|target.is_none()||self.players[index].entity==target).collect::<Vec<_>>();
                if slots.is_empty(){return Ok(mlua::MultiValue::new());}
                if let Some(pending)=self.pending_camera_clear.as_ref() {
                    if pending.slots!=slots||pending.duration.to_bits()!=request.duration.to_bits() {
                        return Err(mlua::Error::external("A different camera reset is still pending"));
                    }
                }else{
                    self.pending_camera_clear=Some(super::em2_camera_reset::Pending {slots,duration:request.duration,next_slot:0});
                }
                self.drain_camera_clear().map_err(|error|mlua::Error::external(format!("{error:#}")))?;
                Ok(mlua::MultiValue::new())
            },
            "SetDefaultCameraValues"|"SetTravelCameraValues"|"SetCombatCameraValues"|"ResetDefaultCameraValues"|"ResetTravelCameraValues"|"ResetCombatCameraValues"=>self.set_camera_values(registry,lua,name,args),
            "EnablePlayer"=>{
                let index=i32::from_lua(args.front().cloned().unwrap_or(Value::Integer(0)),lua)?;
                let enabled=bool::from_lua(args.get(1).cloned().unwrap_or(Value::Boolean(false)),lua)?;
                let slot=usize::try_from(index).ok().filter(|&index|index<2)
                    .ok_or_else(||mlua::Error::external("Invalid EM2 player index"))?;
                if self.players[slot].enabled==enabled {return Ok(mlua::MultiValue::new());}
                if enabled {
                    return Err(mlua::Error::external("EM2 EnablePlayer requires source archetype replacement/finalization; previous entity cannot be reused"));
                }
                self.disable_player(slot,registry).map_err(mlua::Error::external)?;
                Ok(mlua::MultiValue::new())
            }
            _=>globals.call(lua,name,args),
        }
    }
    pub fn step(
        &mut self,
        time: f32,
        delta: f32,
        player_position: [f32; 3],
        scripts: &mut Scripts,
        globals: &mut super::em2_global_state::State,
    ) -> Result<()> {
        if self.error.is_some() {
            return Ok(());
        }
        // Keep one VM and one scheduler. The registry facade remains available
        // to synchronous native handlers without aliasing the executing Scripts.
        let mut execution = scripts.detach_scheduler()?;
        let result = (|| -> Result<()> {
            ensure!(
                time.is_finite() && time >= self.time && delta.is_finite() && delta>=0.,
                "Invalid scene event clock"
            );
            // The simulation owns its timestep. Reconstructing it from
            // rounded absolute timestamps makes IGC/GB clocks diverge from
            // the behavior graph, despite all three running in the same tick.
            self.time = time;
            let commands = execution.with_world_host(
                |lua, name, args| self.call_native(lua, name, args, scripts, globals),
                |execution| execution.advance(time),
            )?;
            self.retained_commands.extend(commands);
            if let Some(error)=execution.errors.first() {
                anyhow::bail!("Script coroutine failed: {error}");
            }
            ensure!(
                self.retained_commands.is_empty(),
                "Script world commands await lifecycle dispatch"
            );
            for volume in &mut self.volumes {
                let inside = !volume.disabled && self.players[0].enabled && !self.hidden.contains(&volume.owner)
                    && self
                        .queries
                        .shape_penetration(volume.shape, player_position)?
                        > 0.;
                if inside != volume.inside {
                    self.outputs.emit(
                        volume.owner,
                        if inside {
                            self.entered += 1;
                            "OnStartTouch"
                        } else {
                            self.exited += 1;
                            "OnEndTouch"
                        },
                        self.player,
                        time,
                    );
                    volume.inside = inside;
                }
            }
            self.pending.extend(self.outputs.ready(time));
            while let Some(delivery) = self.pending.front().cloned() {
                let row = &delivery.connection;
                ensure!(row.flags & !7 == 0, "Unrecovered EM2 IO flags");
                if execution.io_callback_admitted(&row.input)? {
                    for target in self.targets(&delivery, scripts)? {
                        execution.with_world_host(
                            |lua, name, args| self.call_native(lua, name, args, scripts, globals),
                            |execution| execution.call_io_from(
                                &row.input,
                                CallbackContext {
                                    entity: Some(target),
                                    activator: delivery.activator,
                                },
                                &row.parameters,
                                row.owner,
                                false,
                                delivery.payload.as_ref(),
                            ),
                        )
                            .with_context(|| {
                                format!(
                                    "{}: {} -> {}",
                                    self.entities[row.owner].name, row.output, row.input
                                )
                            })?;
                        self.callbacks += 1;
                    }
                } else {
                    anyhow::bail!("{}: {} -> {} requires component input dispatch; delivery retained",
                        self.entities[row.owner].name, row.output, row.input);
                }
                self.pending.pop_front();
            }
            self.advance_igc(delta,&mut execution,scripts,globals)?;
            self.gamebryo_cameras.advance(delta)?;
            Ok(())
        })();
        // Restore even when a callback failed: tasks, errors and cached handles
        // remain in the live scheduler instead of the temporary facade.
        scripts.restore_scheduler(execution)?;
        self.flush_removals(scripts)?;
        if let Err(error)=self.apply_tool_visibility(){if self.error.is_none(){self.error=Some(format!("Tool visibility: {error:#}"));}}
        if let Err(error)=Self::refresh_pc_render_profiles(&self.pc_render_profiles,&self.entities,&self.retired) {if self.error.is_none(){self.error=Some(format!("Render profiles: {error:#}"));}}
        Self::refresh_pc_light_sources(&mut self.pc_light_sources,&self.entities,&self.source_keys,&self.retired);
        self.audio_posts.prepare_pending(&self.audio);
        if let Err(error) = result {
            self.error = Some(format!("{error:#}"));
        }
        Ok(())
    }
    pub fn apply_animation_writes(&mut self,mut write:impl FnMut(usize,u32,u32,bool)->Result<()>){
        while let Some(request)=self.tools.pending_animation.front().cloned() {
            let result=(||->Result<()> {
                let &(scene,entity)=self.source_keys.get(request.owner).context("Animation write lost its source owner")?;
                write(scene,entity,request.id,request.value)
            })();
            if let Err(error)=result {
                let error=format!("Player tool animation: {error:#}");
                if self.error.is_none(){self.error=Some(error);}else{let record=serde_json::json!({"error":error});if !self.source_errors.contains(&record){self.source_errors.push(record);}}
                break;
            }
            self.tools.animation_applied();
        }
    }
    pub fn advance_presentation(&mut self,real:f32,game:f32) {
        if self.error.is_some(){return;}
        // Camera fade color and scaled PC TimeManager publication remain
        // separate compatibility work. The active camera constructor color is0.
        let current=self.cutscene_input.current();
        let game_state=&mut self.game_state_events;
        if let Err(error)=self.hud.advance(real,game,false,0.,|effect|match effect {
            super::em2_hud::Effect::Unmute=>{self.audio_posts.unmute(&self.audio)?;Ok(())},
            // Explicit playtest delivery to its owned GameState listener;
            // complete native global registration remains separate boot work.
            super::em2_hud::Effect::FadeOutComplete=>game_state.receive(current,0x3d),
        }) {self.error=Some(format!("HUD presentation: {error:#}"));}
    }
    pub fn pc_render_profiles(&self)->std::sync::Arc<std::sync::Mutex<super::render_profiles::Registry>> {self.pc_render_profiles.clone()}
    fn refresh_pc_render_profiles(registry:&std::sync::Arc<std::sync::Mutex<super::render_profiles::Registry>>,entities:&[RuntimeEntity],retired:&HashSet<usize>)->Result<()> {
        registry.lock().map_err(|_|anyhow::anyhow!("Render-profile state lock poisoned"))?.refresh_profiles(entities.iter().enumerate().filter(|(owner,_)|!retired.contains(owner)).flat_map(|(owner,entity)|entity.components.iter().enumerate().filter(|(_,component)|component.class_name=="RenderSceneEffectsComponent").map(move |(index,component)|{
            let id=(owner,index);let result=(||->Result<_>{
                let candidate=super::render_profiles::Candidate::from_properties(id,entity.transform[12..15].try_into().unwrap(),|name|property(component,name).cloned(),true)?;
                let settings=super::render_profile_settings::Settings::read(|name|property(component,name).cloned(),true)?;
                Ok((candidate,std::sync::Arc::new(settings)))
            })().with_context(||format!("Render profile {}:{}",entity.scene,entity.name));(id,result)
        })));Ok(())
    }
    fn refresh_pc_light_sources(registry:&mut super::d3d9_scene_lights::Registry,entities:&[RuntimeEntity],source_keys:&[(usize,u32)],retired:&HashSet<usize>) {
        registry.refresh_scoped(entities.iter().enumerate().filter(|(owner,_)|!retired.contains(owner)).flat_map(|(owner,entity)| {
            entity.components.iter().enumerate().filter(|(_,c)|c.class_name=="JPSLightBakingComponent").map(move |(index,baking)| {
                let scene=source_keys.get(owner).map(|key|key.0);
                let result=(||->Result<Option<super::d3d9_sh_cache::Source>> {
                    ensure!(scene.is_some(),"PC SH source lost its scene identity");
                    let mut lights=entity.components.iter().filter(|c|c.class_name=="JPSLightComponent");
                    let light=lights.next().context("PC baking component has no light component")?;
                    ensure!(lights.next().is_none(),"PC baking owner has multiple light components; native association is unresolved");
                    super::d3d9_sh_cache::Source::from_properties(|name|property(light,name),|name|property(baking,name),
                        entity.transform[12..15].try_into().unwrap(),entity.runtime_rotation,true)
                })().with_context(||format!("PC SH source {}:{}",entity.scene,entity.name));
                ((owner,index),scene,entity.name.clone(),result)
            })
        }));
    }
    pub fn summary(&self) -> serde_json::Value {
        let marker_use=serde_json::json!({"requests_admitted":self.uses_started,"last_owner":self.last_usable,"disabled":self.disabled_usables.len(),"source_markers":self.usable_owners.len(),"scope":"Shared nearest selection, PC fast fields, basic lock and immediate marker OnUse; physical/cooperative/delayed use, native feedback/release and full input-state gates remain pending"});
        let mut report=serde_json::json!({"entities":self.entities.len(),"box_triggers":self.volumes.len(),"disabled_triggers":self.volumes.iter().filter(|v|v.disabled).map(|v|&self.entities[v.owner].name).collect::<Vec<_>>(),"entered":self.entered,"exited":self.exited,
            "callbacks_admitted":self.callbacks,"noncallable_inputs":self.skipped,"pending_deliveries":self.pending.len(),"delayed_deliveries":self.outputs.pending_count(),
            "retained_world_commands":self.retained_commands.len(),"time":self.time,"halted":self.error,
            "blocked_delivery":self.pending.front().map(|d|serde_json::json!({"owner":self.entities[d.connection.owner].name,"output":d.connection.output,"input":d.connection.input})),
            "players":self.players,"retired_entities":self.retired.len(),"pending_player_event":self.pending_player_event,"player_events_delivered":self.player_events_delivered,"last_player_change":self.last_player_change,
            "children":serde_json::json!({"links":self.child_links,"clone_failures":self.child_failures,"pending_component_hide_hooks":self.pending_hide_hooks,"drip_requests":serde_json::json!({"count":self.drip_request_count,"last":self.last_drip_request})}),"player_tools":self.tools.summary(),"abilities":self.abilities.summary(),"hud":self.hud.summary(),"sequences":self.sequences.summary(),"level_manager":self.level.summary(),"movies":self.movies.summary(),"music_cache":self.music.summary(),"music_overrides":self.music_overrides.summary(),"audio":self.audio.summary(),"audio_posts":self.audio_posts.summary(),"cameras":self.cameras,
            "quest_critical_path":self.quest_path,"quest_scope":"verified PC critical-path string selection; progression, constructor and save restoration remain unconnected",
            "maps":self.maps,"map_scope":"verified PC volume selection; HUD rendering, markers and volume lifecycle remain unconnected",
            "camera_scope":"override stack, AutoLevel, PC profile construction and custom/reset writers; transition dispatch requires CheckDataSet geometry and target-event receivers; base blending verified at explicit x87 precisions, actual render-thread precision and PC camera generator/presentation unverified",
            "player_ground_properties":self.entities[self.player].components.iter().find(|c|c.class_name=="OnGroundStateComponent").map(|c|c.properties.iter().filter(|p|p.name=="Max Move Speed").map(|p|(&p.name,&p.value)).collect::<std::collections::BTreeMap<_,_>>()),
            "float_property_scope":"verified PC OnGroundState Max Move Speed scalar; other descriptors and player locomotion remain unconnected",
            "player_manager_scope":"single-player AI companion disable; primary/human handoff and replacement factory unsupported",
            "source_errors":self.source_errors,"trigger_contact_parity_verified":false,"full_scene_lifecycle_connected":false});
        report["pc_light_sources"]=self.pc_light_sources.summary();
        report["pc_render_profiles"]=match self.pc_render_profiles.lock(){Ok(state)=>state.summary(),Err(_)=>serde_json::json!({"error":"Render-profile state lock poisoned"})};
        report["cutscene_input"]=serde_json::json!({"state":self.cutscene_input,"scope":"PC restriction/release counters and main-game/cutscene stack connected to shared controller; diagnostic MainGame initial context; native state-entry HUD, events, lotcheck and arbitrary GameState transitions pending"});
        report["pending_camera_clear"]=serde_json::json!(self.pending_camera_clear);
        report["player_constraints"]=serde_json::json!(self.player_constraints);
        report["guardians"]=serde_json::json!(self.guardians);
        report["camera_manager"]=self.camera_manager.as_ref().map(|manager|manager.summary()).unwrap_or(serde_json::Value::Null);
        report["igc"]=self.igc.summary();
        report["igc_sound_bindings"]=serde_json::json!(self.igc_sound_owners);
        report["igc_origin_token"]=serde_json::json!(self.igc_origin_token);
        report["game_state_events"]=self.game_state_events.summary();
        report["igc_dialogue"]=serde_json::json!({"requests":self.igc_dialogue_requests,"last":self.last_igc_dialogue,"bindings":self.igc_dialogue_owners});
        report["igc_controls"]=serde_json::json!({"animation_dispatches":self.igc_animation_dispatches,"dispatches":self.igc_control_dispatches,"writes_applied":self.igc_control_writes_applied,"pending":self.igc_player_writes,"bindings":self.igc_control_owners,
            "scope":"Native control order/rotation; explicit shared-playtest primary-player relocation/reset and motion gate. Native EM2 proxy/contact lifecycle, non-player control, AI/child/target controls and enabled animation culling remain incomplete"});
        report["gamebryo_cameras"]=self.gamebryo_cameras.summary();
        report["marker_use"]=marker_use;
        report["retirement"]=serde_json::json!({"pending_collision_sources":self.pending_collision_retirements,"scope":"Registered aliases, scripts, owned children, IO, trigger queries and static collision retirement; complete native per-component/world-event cleanup remains pending"});
        report["camera_trackers"]=serde_json::json!(self.camera_trackers);
        report
    }
}
