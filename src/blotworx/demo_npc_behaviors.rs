//! USER-AUTHORIZED TEMPORARY DEMO FALLBACK. This is not native AI parity.
//! Reuses the verified common selector/task clock, scripted flags, animation
//! waits and actual authored events. Navigation, ambient patrol admission and
//! immediate action draining are provisional. Strict mode never enters here.
use super::{assets::World,ai_selection::{self,Selector,Lifecycle},ai_task_runtime::Context};
use anyhow::{Context as _,Result,ensure};
use std::collections::{BTreeMap,BTreeSet};
#[derive(Default,serde::Serialize)]
pub struct State {
    pub enabled:bool,
    pub actors:BTreeSet<usize>,
    moves:BTreeMap<usize,Move>,
    patrol_wait:BTreeMap<usize,(usize,f32)>,
    moving:BTreeMap<usize,bool>,
    pub updates:u64,
    pub selections:u64,
    pub errors:Vec<String>,
    #[serde(skip)] delta:f32,
}
#[derive(Clone,Copy,serde::Serialize)]
struct Move {destination:[f32;3],radius:f32,turn:bool}
impl State {
    pub fn retire(&mut self,owner:usize){self.actors.remove(&owner);self.moves.remove(&owner);self.patrol_wait.remove(&owner);self.moving.remove(&owner);}
}
impl World {
    pub(super) fn demo_npc_active(&self,owner:usize)->bool {
        self.demo_story.enabled && self.demo_story.npc_fallback.enabled
            && !self.removed_entities.contains(&owner)
            && !self.spawner_sources.contains(&owner)
            && self.ai_brains.get(&owner).is_some_and(|brain| {
                !self.uses_demo_combat(owner, &brain.ai_type)
                    && (brain.ai_type.eq_ignore_ascii_case("Pete:SmallWorld")
                        || brain.ai_type.to_ascii_lowercase().starts_with("gremlin")
                        || (!brain.behaviors.iter().any(|name| name == "RobotArmGeneral")
                            && (!brain.scripted_move.is_empty() || !brain.patrol.is_empty())
                            && self.entities[owner].components.iter().any(|c| c.class_name == "BasicAIControllerComponent")))
            })
            && self.authored_havok.contains(owner)
            && self.character_physics.get(&owner).is_some_and(|b|b.is_ok())
    }
    pub(super) fn refresh_demo_npc_behavior(&mut self,owner:usize)->Result<()> {
        if !self.demo_npc_active(owner) || !self.entity_scene_active(owner) {return Ok(())}
        self.demo_story.npc_fallback.actors.insert(owner);
        if self.ai_brains[&owner].selector.updating {
            return ai_selection::execute_live(&mut Driver{world:self,owner});
        }
        // Explicit demo timing fallback: drain a completed scripted behavior
        // before a following Lua cutscene request writes its new variant.
        let previous=self.demo_story.npc_fallback.delta;
        self.demo_story.npc_fallback.delta=0.;
        let result=super::ai_brain_update::advance_live(Context{owner,delta:0.},&mut Driver{world:self,owner});
        self.demo_story.npc_fallback.delta=previous;result
    }
    pub(super) fn advance_demo_npc_behaviors(&mut self,delta:f32)->Result<()> {
        if !self.demo_story.npc_fallback.enabled || !self.demo_story.enabled || self.ai_manager_enabled==Some(false){return Ok(())}
        let actors=self.ai_brains.keys().copied().filter(|&id|self.demo_npc_active(id)).collect::<Vec<_>>();
        self.demo_story.npc_fallback.delta=delta;
        for owner in actors {
            if !self.entity_scene_active(owner) || self.ai_component_disabled(owner,"AIBrainComponent"){continue;}
            self.demo_story.npc_fallback.actors.insert(owner);
            let result=super::ai_brain_update::advance_live(Context{owner,delta},&mut Driver{world:self,owner});
            if let Err(error)=result {
                let text=format!("Temporary NPC fallback {}: {error:#}",self.entities[owner].name);
                if !self.demo_story.npc_fallback.errors.contains(&text){self.demo_story.npc_fallback.errors.push(text.clone());self.report.unsupported.push(text);}
            }
            self.demo_story.npc_fallback.updates+=1;
        }Ok(())
    }
    fn stop_demo_npc_motion(&mut self,owner:usize,hold:bool)->Result<()> {
        let body=self.character_physics.get(&owner).and_then(|b|b.as_ref().ok()).context("Fallback NPC body missing")?;
        let shape=body.shape;let mut velocity=self.collision.simulation_pose(shape)?.linear;
        velocity[0]=0.;velocity[2]=0.;if hold{velocity[1]=0.;}
        self.collision.set_simulation_velocity(shape,velocity)?;
        self.set_npc_ai_speed(owner,false,0.)?;Ok(())
    }
    fn demo_npc_turn_toward(&mut self,owner:usize,destination:[f32;3],delta:f32)->Result<bool> {
        let body=self.character_physics.get(&owner).and_then(|b|b.as_ref().ok()).context("Fallback turn body missing")?;
        let pose=self.collision.simulation_pose(body.shape)?;
        let dx=destination[0]-pose.position[0];let dz=destination[2]-pose.position[2];
        if dx*dx+dz*dz<0.000001{return Ok(true)}
        let desired=(-dx).atan2(dz);let r=self.entities[owner].runtime_rotation;let current=r[6].atan2(r[8]);
        let rate=self.npc_relocations.get(&owner).and_then(|s|s.turn_rates.stationary).map(f32::from_bits).unwrap_or(std::f32::consts::TAU);
        let step=super::physics::facing_delta(current,desired,rate,delta);
        let (s,c)=(current+step).sin_cos();
        self.warp_character_body(owner,super::native_hk_transform::from_entity([c,0.,-s,0.,1.,0.,s,0.,c],pose.position))?;
        let length=(dx*dx+dz*dz).sqrt();
        self.publish_npc_ai_facing(owner,[dx/length,0.,dz/length])?;
        Ok(super::physics::facing_delta(current+step,desired,1.,1.).abs()<0.03)
    }
    pub(super) fn demo_npc_face_player(&mut self,owner:usize)->Result<()> {
        let p=self.entities[self.player_entity].transform;
        self.demo_npc_turn_toward(owner,[p[12],p[13],p[14]],self.demo_story.npc_fallback.delta)?;Ok(())
    }
    fn drive_demo_npc_move(&mut self,owner:usize,movement:Move,delta:f32,prefer_patrol_target:bool)->Result<bool> {
        if movement.turn {return self.demo_npc_turn_toward(owner,movement.destination,delta)}
        let body=self.character_physics.get(&owner).and_then(|b|b.as_ref().ok()).context("Fallback navigation body missing")?;
        let shape=body.shape;let pose=self.collision.simulation_pose(shape)?;
        let dx=movement.destination[0]-pose.position[0];let dz=movement.destination[2]-pose.position[2];
        if dx*dx+dz*dz<=movement.radius*movement.radius {self.stop_demo_npc_motion(owner,false)?;self.demo_npc_walking(owner,0.)?;return Ok(true)}
        if self.basic_motion_suspended(owner)==Some(true)||self.ai_component_disabled(owner,"BasicAIControllerComponent"){self.stop_demo_npc_motion(owner,false)?;return Ok(false)}
        let direction=self.demo_steer(owner,shape,pose.position,movement.destination,prefer_patrol_target)?;
        let speed=self.entities[owner].components.iter().find(|c|c.class_name=="BasicMotionControllerComponent")
            .and_then(|c|c.properties.iter().find(|p|p.name=="Max Move Speed")).and_then(|p|p.value.as_f64()).unwrap_or(2.) as f32;
        ensure!(speed.is_finite() && speed>=0.,"Invalid fallback NPC movement speed");
        let speed=if direction==[0.;2]{0.}else{speed.min(((dx*dx+dz*dz).sqrt()-movement.radius).max(0.)/delta.max(0.000001))};
        self.set_npc_ai_speed(owner,false,speed)?;
        self.collision.set_simulation_velocity(shape,[direction[0]*speed,pose.linear[1],direction[1]*speed])?;
        self.demo_npc_turn_toward(owner,[pose.position[0]+direction[0],pose.position[1],pose.position[2]+direction[1]],delta)?;
        self.demo_npc_walking(owner,speed)?;Ok(false)
    }
    fn demo_npc_walking(&mut self,owner:usize,speed:f32)->Result<()> {
        self.authored_havok.write_native_variable(owner,"VAR_Forward_Speed",speed.to_bits())?;
        let moving=speed>0.;let old=self.demo_story.npc_fallback.moving.insert(owner,moving);
        // Gremlin graphs do not declare Mickey's Start/Stop_Walking events.
        // Their authored locomotion reads VAR_Forward_Speed after Force_Idle
        // leaves the prior scripted branch. Resolve through the real catalog.
        if old!=Some(moving){self.send_native_actor_animation_event(owner,0xc2)?;}
        Ok(())
    }
    fn demo_patrol_notice(&mut self,owner:usize,target:usize,kind:u16)->Result<()> {
        self.deferred_event_runtime.dispatch_ordinary_output(&mut self.entity_event_registry,
            super::entity_events::Basic{event_kind:kind,source:Some(target),header:Some(owner)},
            &mut self.outputs,&self.entities,&self.scene_resources,self.animation_time,&mut self.ai_brains,&self.robot_arm_controls,&self.cutscene_actions)
    }
    fn update_demo_npc_patrol(&mut self,owner:usize,slot:usize,delta:f32)->Result<u32> {
        let target=self.ai_brains[&owner].patrol[&slot].target.filter(|&t|t<self.entities.len()&&!self.removed_entities.contains(&t));
        let Some(target)=target else{self.stop_demo_npc_motion(owner,false)?;self.demo_npc_walking(owner,0.)?;return Ok(1)};
        if let Some((last,until))=self.demo_story.npc_fallback.patrol_wait.get(&owner).copied(){
            if last==target {
                if self.animation_time<until {return Ok(1)}
                self.demo_story.npc_fallback.patrol_wait.remove(&owner);self.demo_patrol_notice(owner,target,0x8e)?;
                if self.ai_brains[&owner].patrol[&slot].target==Some(target){self.advance_native_patrol_target(owner,slot)?;}return Ok(1)
            }
            self.demo_story.npc_fallback.patrol_wait.remove(&owner);
        }
        let properties=self.entities[target].components.iter().find(|c|c.class_name=="PatrolNodeComponent").map(|c|&c.properties);
        let number=|name:&str,default:f32|properties.and_then(|p|p.iter().find(|p|p.name==name)).and_then(|p|p.value.as_f64()).map(|v|v as f32).unwrap_or(default);
        let radius=number("GotoTolerance",0.5).max(0.05);let low=number("HoldTimeMin",0.);let high=number("HoldTimeMax",low);
        let p=self.entities[target].transform;let movement=Move{destination:[p[12],p[13],p[14]],radius,turn:false};
        if self.drive_demo_npc_move(owner,movement,delta,true)? {
            let wait=if high>low{low+(high-low)*(self.game_random.draw() as f32/32767.)}else{low};
            self.demo_story.npc_fallback.patrol_wait.insert(owner,(target,self.animation_time+wait.max(0.)));
            self.demo_patrol_notice(owner,target,0x8d)?;
        }Ok(1)
    }
    pub(super) fn activate_demo_npc_wait(&mut self,owner:usize,task:usize)->Result<()> {
        let definition=self.ai_brains[&owner].animation_waits[&task].task.definition;
        self.stop_demo_npc_motion(owner,false)?;
        definition.activate(owner,&mut PreviewAnimation{world:self,task})
    }
}
struct PreviewAnimation<'a>{world:&'a mut World,task:usize}
impl super::ai_animation_wait::Host for PreviewAnimation<'_>{
    fn animation_controller(&mut self,o:usize)->Result<Option<usize>>{Ok(self.world.authored_havok.contains(o).then_some(o))}
    fn send_animation_event(&mut self,o:usize,e:u32)->Result<()>{self.world.send_native_actor_animation_event(o,e)?;Ok(())}
    fn subscribe(&mut self,o:usize,k:u16,any:bool)->Result<()>{ensure!(k==9&&!any,"Invalid preview animation subscription");let task=self.world.ai_brains.get_mut(&o).and_then(|b|b.animation_waits.get_mut(&self.task)).context("Preview animation task missing")?;let fresh=!task.subscribed;task.subscribed=true;if fresh{self.world.cutscene_actions.bind_animation_task(o,&mut self.world.entity_event_registry)?;}Ok(())}
    // This separate demo host does not invent native agents or change strict
    // lookup semantics. Its missing agent40 stop is performed above.
    fn has_agent(&mut self,_:usize)->Result<bool>{Ok(false)}
    fn agent(&mut self,_:usize)->Result<usize>{anyhow::bail!("Preview does not fabricate a native navigation agent")}
    fn agent_heading(&mut self,_:usize,_:f32)->Result<()>{anyhow::bail!("No native preview agent")}
    fn agent_40(&mut self,_:usize)->Result<()>{anyhow::bail!("No native preview agent")}
    fn has_basic_controller(&mut self,o:usize)->Result<bool>{Ok(self.world.npc_relocations.contains_key(&o))}
    fn basic_facing(&mut self,o:usize,f:[f32;3])->Result<()>{self.world.publish_npc_ai_facing(o,f)}
}
struct Driver<'a>{world:&'a mut World,owner:usize}
impl Driver<'_>{
    fn brain(&self)->Result<&super::ai_behavior_events::Brain>{self.world.ai_brains.get(&self.owner).context("Preview brain retired")}
    fn brain_mut(&mut self)->Result<&mut super::ai_behavior_events::Brain>{self.world.ai_brains.get_mut(&self.owner).context("Preview brain retired")}
    fn name(&self,b:usize)->Result<&str>{self.brain()?.behaviors.get(b).map(String::as_str).context("Preview behavior absent")}
    fn finish(&mut self,b:usize)->Result<()>{
        if let Some(child)=self.brain()?.behavior_tasks[&b].child{self.world.stop_ai_child(self.owner,child)?;if let Some(child)=self.brain()?.behavior_tasks[&b].child{self.world.destroy_ai_child(self.owner,child)?;}self.brain_mut()?.behavior_tasks.get_mut(&b).unwrap().child=None;}
        if self.name(b)?=="Conversation"{self.brain_mut()?.conversations.get_mut(&b).unwrap().exit();}
        self.world.demo_story.npc_fallback.moves.remove(&self.owner);
        let name=self.name(b)?.to_string();
        self.world.entity_event_queue.enqueue(&super::entity_events::Event::BehaviorFinished(super::entity_events::BehaviorFinished{source:Some(self.owner),header:None,name}),None)?;Ok(())
    }
}
impl ai_selection::LiveHost for Driver<'_>{fn selector(&self)->Result<&Selector>{Ok(&self.brain()?.selector)}fn selector_mut(&mut self)->Result<&mut Selector>{Ok(&mut self.brain_mut()?.selector)}}
impl ai_selection::LifecycleHost for Driver<'_>{
    fn lifecycle(&self)->Result<&Lifecycle>{Ok(&self.brain()?.selection_lifecycle)}fn lifecycle_mut(&mut self)->Result<&mut Lifecycle>{Ok(&mut self.brain_mut()?.selection_lifecycle)}
    fn selection_running(&self)->Result<bool>{Ok(self.brain()?.selector.updating)}fn set_selection_running(&mut self,v:bool)->Result<()>{self.brain_mut()?.selector.updating=v;Ok(())}
    fn lifecycle_random(&mut self)->Result<i32>{Ok(self.world.game_random.draw() as i32)}
    fn selection_phase(&mut self)->Result<()>{ai_selection::choose_live(self)?;self.brain_mut()?.reevaluation_requested=false;self.world.demo_story.npc_fallback.selections+=1;Ok(())}
}
impl ai_selection::Host for Driver<'_>{
    fn score_20(&mut self,b:usize)->Result<i32>{Ok(match self.name(b)?{"ScriptedMove"=>44,"ScriptedIdle"=>43,"Conversation"=>42,"Paused"=>40,"Patrol"=>4,_=>-1})}
    fn current_gate_0c(&mut self,_:usize)->Result<bool>{Ok(false)}fn candidate_gate_28(&mut self,_:usize)->Result<bool>{Ok(false)}fn current_gate_2c(&mut self,_:usize)->Result<bool>{Ok(false)}
    fn candidate_gate_24(&mut self,b:usize)->Result<bool>{let brain=self.brain()?;Ok(match self.name(b)?{"ScriptedMove"=>brain.scripted_move[&b].pending,"ScriptedIdle"=>brain.motion_suspended,"Conversation"=>brain.conversations[&b].eligible(),"Paused"=>brain.paused[&b].active,"Patrol"=>!self.world.uses_demo_combat(self.owner,&brain.ai_type),_=>false})}
    fn random(&mut self)->Result<i32>{Ok(self.world.game_random.draw() as i32)}
    fn leave_and_notify(&mut self,b:usize)->Result<()>{self.finish(b)}
    fn enter_4c(&mut self,b:usize,_:bool)->Result<()>{
        let task=self.brain_mut()?.behavior_tasks.get_mut(&b).context("Preview common task absent")?;task.mode=0;task.timer=0.;
        match self.name(b)? {
            "ScriptedIdle"|"Paused"|"Patrol"=>{let held=self.brain()?.motion_suspended;self.world.stop_demo_npc_motion(self.owner,held)?;self.world.send_native_actor_animation_event(self.owner,0xc2)?;}
            "Conversation"=>self.brain_mut()?.conversations.get_mut(&b).unwrap().enter(),
            "ScriptedMove"=>{let state=self.brain_mut()?.scripted_move.get_mut(&b).unwrap();state.pending=false;let p=state.destination_bits.context("Preview move destination absent")?;let movement=Move{destination:[f32::from_bits(p[0]),f32::from_bits(p[1]),f32::from_bits(p[2])],radius:f32::from_bits(state.radius_bits.unwrap_or(0x3f000000)),turn:matches!(state.action,Some(2|3))};self.world.demo_story.npc_fallback.moves.insert(self.owner,movement);}
            _=>{},
        }Ok(())
    }
}
impl super::ai_brain_update::LiveHost for Driver<'_>{
    fn selector(&self)->Result<&Selector>{Ok(&self.brain()?.selector)}fn selector_mut(&mut self)->Result<&mut Selector>{Ok(&mut self.brain_mut()?.selector)}
    fn lifecycle(&self)->Result<&Lifecycle>{Ok(&self.brain()?.selection_lifecycle)}fn lifecycle_mut(&mut self)->Result<&mut Lifecycle>{Ok(&mut self.brain_mut()?.selection_lifecycle)}
    fn update_allowed(&mut self,_:usize,_:usize)->Result<bool>{Ok(true)}
    fn update_current(&mut self,b:usize,c:Context)->Result<u32>{self.world.update_ai_behavior_task(self.owner,b,c.delta,update,completed)}
    fn child(&mut self,b:usize)->Result<Option<usize>>{Ok(self.brain()?.behavior_tasks[&b].child)}
    fn stop_child(&mut self,_:usize,t:usize,o:usize)->Result<()>{self.world.stop_ai_child(o,t)}fn destroy_child(&mut self,_:usize,t:usize)->Result<()>{self.world.destroy_ai_child(self.owner,t)}
    fn clear_child(&mut self,b:usize)->Result<()>{self.brain_mut()?.behavior_tasks.get_mut(&b).unwrap().child=None;Ok(())}
    fn exit(&mut self,b:usize,_:Context)->Result<()>{if self.name(b)?=="Conversation"{self.brain_mut()?.conversations.get_mut(&b).unwrap().exit();}Ok(())}
    fn owner_entity(&mut self,o:usize)->Result<Option<usize>>{Ok(Some(o))}fn name(&mut self,b:usize)->Result<String>{Ok(Driver::name(self,b)?.into())}
    fn enqueue_finished(&mut self,e:super::entity_events::BehaviorFinished)->Result<()>{self.world.entity_event_queue.enqueue(&super::entity_events::Event::BehaviorFinished(e),None)?;Ok(())}
    fn selection_requested(&mut self,_:usize)->Result<bool>{Ok(self.brain()?.reevaluation_requested)}fn select(&mut self,_:usize)->Result<()>{ai_selection::execute_live(self)}
    fn background_active(&mut self,_:usize)->Result<bool>{Ok(false)}fn background_update(&mut self,_:usize,_:Context)->Result<()>{Ok(())}
}
fn completed(_:&mut World,_:usize,_:Context)->Result<()>{Ok(())}
fn update(w:&mut World,b:usize,c:Context)->Result<u32>{
    let brain=&w.ai_brains[&c.owner];
    match brain.behaviors[b].as_str(){
        "ScriptedIdle"=>{let held=brain.motion_suspended;w.stop_demo_npc_motion(c.owner,held)?;Ok(if held{1}else{2})},
        "Paused"=>{let active=brain.paused[&b].active;w.stop_demo_npc_motion(c.owner,false)?;Ok(if active{1}else{2})},
        "Conversation"=>w.update_selected_ai_conversation(c.owner),
        "Patrol"=>w.update_demo_npc_patrol(c.owner,b,c.delta),
        "ScriptedMove"=>{
            if let Some(movement)=w.demo_story.npc_fallback.moves.get(&c.owner).copied(){if !w.drive_demo_npc_move(c.owner,movement,c.delta,false)?{return Ok(1)}}
            w.demo_story.npc_fallback.moves.remove(&c.owner);super::ai_scripted_move::CompletionHost::reached_position(w,c.owner)?;Ok(2)
        },
        _=>{let _=w.stop_demo_npc_motion(c.owner,false);Ok(1)},
    }
}
