//! Functional presentation for the demo. This deliberately uses a lightweight
//! scheduler rather than claiming parity with Apprentice's native UI manager.
use crate::em1_bsq::{Document, Node};
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct State {
    pub enabled: bool,
    pub dialogue: Option<Dialogue>,
    bark: Option<Bark>,
    pub choice: usize,
    pub conversation: std::collections::BTreeSet<usize>,
    pub(super) camera_override: Option<usize>,
    saved_camera_override: Option<usize>,
    pub(super) owns_grabbed_camera: bool,
    active: Option<Playback>,
    pub(super) current_request: Option<super::story_sequences::Request>,
    pub completed: Vec<String>,
    pub errors: Vec<String>,
    cancelled:u64,
    last_cancelled:Option<String>,
    last_cancelled_detail:Option<serde_json::Value>,
    pub hud: bool,
    pub reticle_visible: bool,
    pub drips_visible: bool,
    pub motion: super::demo_gameplay::State,
    pub ai: super::demo_ai::State,
    pub(super) enemy_armor: super::enemy_armor::State,
    pub(super) boss_ai: super::boss_ai::State,
    pub(super) projectiles: super::ai_projectiles::State,
    pub npc_fallback: super::demo_npc_behaviors::State,
    pub scene: super::demo_scene::State,
    pub triggers: super::demo_triggers::State,
    pub counters: super::demo_counter::State,
    pub(super) reward_events:u64,
    pub pending_deaths: std::collections::BTreeSet<usize>,
    dictionary_paths: std::collections::BTreeSet<String>,
    dictionaries: Vec<(u32, BTreeMap<u32, String>)>,
    random_dialogue: BTreeMap<(usize,Vec<usize>),super::dialogue_selection::State>,
    random_dialogue_picks:u64,
}
#[cfg(any(test, debug_assertions))]
mod tests {
    use super::*;
    pub(super) fn validate_progression() -> Result<()> {
        original_repaired_gate_does_not_fall_through_to_broken_dialogue()?;
        original_third_pipe_keeps_nil_receiver_admission_without_losing_other_outputs()?;
        demo_opening_door_opens_from_painting_both_gears()?;
        demo_opening_door_breaks_from_spin()?;
        demo_opening_executes_authored_story_instead_of_aborting()?;
        demo_zone_a_pipes_combat_and_exit_chain()?;
        demo_zone_a_friendly_combat_and_exit_chain()?;
        demo_zone_a_mixed_combat_and_exit_chain()?;
        println!("PASS: eight original-asset demo progression scenarios (scripted stimuli; not a manual playthrough)");
        Ok(())
    }
    #[cfg_attr(test, test)]
    #[cfg_attr(test, ignore = "requires original EM1 files in BLOTWORX_TEST_DATA")]
    fn original_repaired_gate_does_not_fall_through_to_broken_dialogue() -> Result<()> {
        for broken in [false, true] {
            let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
            let mut world = super::super::assets::load(
                std::path::Path::new(&root),
                "Levels/GV_ZoneA_Start.level",
                &egui::Context::default(),
            )?;
            world.enable_demo_story()?;
            for _ in 0..600 {
                if world.demo_story.dialogue.is_some() {
                    world.acknowledge_demo_story(0);
                }
                world.advance_models_with_delta(
                    world.animation_time + super::super::STEP,
                    super::super::STEP,
                )?;
            }
            world.globals.set("GV_ZoneA_GearSmashed", i32::from(broken));
            world
                .globals
                .set("GV_ZoneA_Door01", if broken { 0 } else { 2 });
            world.globals.set("GV_ZoneA_TiestowDoorHappy_Played", 0);
            let owner = world
                .entities
                .iter()
                .position(|e| {
                    e.components
                        .iter()
                        .filter(|c| c.class_name == "ConversationComponent")
                        .flat_map(|c| &c.properties)
                        .any(|p| {
                            p.name == "Sequences"
                                && p.value.to_string().contains("GV_ZoneA_TiestowDoorHappy")
                        })
                })
                .context("Tiestow owner missing")?;
            let admission = world.request_story_sequence(owner, "GV_ZoneA_TiestowDoorHappy")?;
            anyhow::ensure!(
                matches!(admission, super::super::story_sequences::Admission::Queued),
                "Door branch was not admitted: {admission:?}"
            );
            let mut lines = Vec::new();
            for _ in 0..180 {
                if let Some(dialogue) = &world.demo_story.dialogue {
                    lines.push(dialogue.text.clone());
                    world.acknowledge_demo_story(0);
                }
                world.advance_models_with_delta(
                    world.animation_time + super::super::STEP,
                    super::super::STEP,
                )?;
            }
            let repaired = world
                .demo_story
                .localized("Gremlin Tiestow_GV_ZoneA_TiestowDoorHappy_1");
            let smashed = world
                .demo_story
                .localized("Gremlin Tiestow_GV_ZoneA_TiestowDoorHappy_2");
            assert!(
                lines.contains(if broken { &smashed } else { &repaired }),
                "Expected door line missing: {lines:?}"
            );
            assert!(
                !lines.contains(if broken { &repaired } else { &smashed }),
                "Door dialogue fell through: {lines:?}"
            );
            assert!(world.demo_story.errors.is_empty());
        }
        Ok(())
    }
    #[cfg_attr(test, test)]
    #[cfg_attr(test, ignore = "requires original EM1 files in BLOTWORX_TEST_DATA")]
    fn original_third_pipe_keeps_nil_receiver_admission_without_losing_other_outputs() -> Result<()>
    {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::super::assets::load(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        world.enable_demo_story()?;
        for _ in 0..600 {
            if world.demo_story.dialogue.is_some() {
                world.acknowledge_demo_story(0);
            }
            world.advance_models_with_delta(
                world.animation_time + super::super::STEP,
                super::super::STEP,
            )?;
        }
        let name = "gremlinvillage_zonea_static.GV_SteamPipeB1_Toon 03";
        let pipe = world
            .scripts
            .resolve_name(name)
            .context("Third pipe missing")?;
        assert!(world
            .scripts
            .resolve_name("gremlinvillage_zonea.GV_GearB2_Inert 02")
            .is_none());
        world.demo_counter_stimulus(pipe, &[(1, 100.)], false)?;
        for _ in 0..30 {
            world.advance_models_with_delta(
                world.animation_time + super::super::STEP,
                super::super::STEP,
            )?;
        }
        assert_eq!(world.globals.get("GV_ZoneA_Pipe03"), Some(1));
        assert!(
            !world
                .report
                .unsupported
                .iter()
                .any(|s| s.starts_with("Authored IO ") && s.contains(name)),
            "Third pipe IO failed: {:?}",
            world
                .report
                .unsupported
                .iter()
                .filter(|s| s.starts_with("Authored IO "))
                .collect::<Vec<_>>()
        );
        Ok(())
    }
    #[cfg_attr(test, test)]
    #[cfg_attr(test, ignore = "requires original EM1 files in BLOTWORX_TEST_DATA")]
    fn demo_opening_door_opens_from_painting_both_gears() -> Result<()> {
        opening_door_puzzle(false)
    }
    #[cfg_attr(test, test)]
    #[cfg_attr(test, ignore = "requires original EM1 files in BLOTWORX_TEST_DATA")]
    fn demo_opening_door_breaks_from_spin() -> Result<()> {
        opening_door_puzzle(true)
    }
    pub(super) fn validate_gv_encounter()->Result<()> {demo_zone_a_pipes_combat_and_exit_chain()}
    pub(super) fn validate_brush_puzzle()->Result<()> {
        opening_door_puzzle(false)?;
        println!("PASS: original opening puzzle completes through real projectile sweeps, shared contact dispatch and animated door collision");
        Ok(())
    }
    fn opening_door_puzzle(spin: bool) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::super::assets::load(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        world.checkpoint.service = Some(super::super::save_service::Service::open(
            std::path::PathBuf::from(format!("target/demo-frontdoor-save-{}", std::process::id())),
            0,
            world.frame_clock.hardware_sample(),
        )?);
        world.enable_demo_story()?;
        world.enable_demo_abilities()?;
        world.camera_lens.set_renderer_aspect(16. / 9.)?;
        fn advance(world: &mut super::super::assets::World, count: usize) -> Result<()> {
            for _ in 0..count {
                if world.demo_story.dialogue.is_some() {
                    world.acknowledge_demo_story(0);
                }
                world.advance_models_with_delta(
                    world.animation_time + super::super::STEP,
                    super::super::STEP,
                )?;
                world.advance_dynamic_physics(super::super::STEP)?;
                world.advance_jigsaw_effects(super::super::STEP)?;
            }
            Ok(())
        }
        advance(&mut world, 600)?;
        // Complete the real level-entry projector handoff before the player
        // reaches the combat gate. Otherwise Prefab_ProjectorMickeyMove waits
        // forever for actual controller motion and leaves the scene's
        // conversational coroutine suspended throughout the route.
        if let Some(marker) = world
            .entities
            .iter()
            .position(|entity| entity.name.ends_with(".ExitProjectorMarker"))
        {
            let mut player = world.take_startup_player();
            world.with_live_player(&mut player, |world| {
                world.move_player_to_entity(super::super::entity_relocation::Request {
                    entity: world.player_entity,
                    target: marker,
                    reset: false,
                    probe: false,
                })
            })?;
            world.script_player = player;
        }
        let door = world
            .scripts
            .resolve_name("gremlinvillage_zonea_dynamic.GV_ZoneA_Door03 01")
            .context("Opening door missing")?;
        let source = *world
            .dynamic_runtime_sources
            .keys()
            .find(|&&s| world.dynamic_bodies[s].entity == door)
            .context("Door bodies missing")?;
        let before = world.dynamic_constructions[&source]
            .bodies
            .iter()
            .map(|b| b.state().transform[13])
            .collect::<Vec<_>>();
        if spin {
            let gear = world
                .scripts
                .resolve_name("gremlinvillage_zonea.GV_ZoneA_SM01_Inert 01")
                .context("Breakable gear missing")?;
            let mut hits = std::collections::HashSet::new();
            world.dispatch_spin([0., 0., -3.5], &mut hits)?;
            assert!(
                hits.contains(&gear),
                "Spin query did not contact the breakable gear"
            );
            advance(&mut world, 1)?;
            assert!(
                world.hidden_entities.contains(&gear),
                "Destroyed gear remained visible during its delayed output"
            );
            assert!(
                world.demo_story.pending_deaths.contains(&gear),
                "Delayed death output lost its callback owner"
            );
            assert!(
                !world.removed_entities.contains(&gear),
                "Gear retired before its 0.3-second output could start"
            );
            advance(&mut world, 240)?;
            assert!(
                world.removed_entities.contains(&gear),
                "Spin did not break the gear"
            );
            assert_eq!(world.globals.get("GV_ZoneA_GearSmashed"), Some(1));
        } else {
            for (index, suffix) in ["01", "02"].into_iter().enumerate() {
                let name = format!("gremlinvillage_zonea.GV_ZoneA_SM01_Toon {suffix}");
                let gear = world
                    .scripts
                    .resolve_name(&name)
                    .context("Paintable gear missing")?;
                let e = &world.entities[gear];
                let p = [e.transform[12], e.transform[13], e.transform[14] + 0.8];
                let target=[p[0],p[1],p[2]-1.6];
                let mut contacts=0;
                let mut emission_time=0.;
                for _ in 0..120 {
                    if emission_time<=0. {
                        world.projectile_preview.spawn(&world.report,super::super::brush::Ink::Paint,
                            world.player_entity as u64,p,target)?;
                        emission_time+=world.report.brush_settings.as_ref().context("Brush settings missing")?.projectile_interval;
                    }
                    emission_time-=super::super::STEP;
                    world.advance_brush_projectiles(super::super::STEP)?;
                    let step=world.projectile_preview.step_contacts.clone();
                    for contact in step {
                        if world.collision_entity(contact.hit.owner).is_some_and(|e|e.name==name) {contacts+=1;}
                        let player=world.entities[world.player_entity].transform[12..15].try_into()?;
                        world.dispatch_brush_contact(&contact,player)?;
                    }
                    advance(&mut world,1)?;
                    if world.paint_counters[&gear].status()==1 {break;}
                }
                assert!(contacts>0,"Real projectiles did not contact {name}");
                assert_eq!(world.paint_counters[&gear].status(),1,"Real spray did not paint {name}");
                assert!(world.model_paint[&gear].painted,"Gear counter painted but body/render state did not follow");
                for &owner in &world.model_paint[&gear].owners {
                    assert!(world.collision.owner_is_solid(owner),"Painted gear collision remained absent");
                }
                // Wait through the visual paint effect as well as the counter.
                // Both completing on one gear must count as only one gear.
                advance(&mut world, 90)?;
                assert_eq!(
                    world.globals.get("GV_ZoneA_Door01"),
                    Some(index as i16 + 1),
                    "Gear output did not reach authored door logic"
                );
                if index == 0 {
                    assert_ne!(world.globals.get("GV_ZoneA_FrontDoorBlocker"), Some(1),
                        "Front door opened after only one gear");
                }
            }
            advance(&mut world, 180)?;
        }
        let errors = world
            .report
            .unsupported
            .iter()
            .filter(|s| s.starts_with("Game Lua:"))
            .collect::<Vec<_>>();
        assert!(errors.is_empty(), "Puzzle script errors: {errors:?}");
        assert!(
            world.demo_story.errors.is_empty(),
            "Puzzle sequence errors: {:?}",
            world.demo_story.errors
        );
        assert_eq!(world.globals.get("GV_ZoneA_FrontDoorBlocker"), Some(1));
        assert!(
            world.dynamic_constructions[&source]
                .bodies
                .iter()
                .enumerate()
                .any(|(i, b)| (b.state().transform[13] - before[i]).abs() > 1.),
            "Puzzle never animated the door collision"
        );
        Ok(())
    }
    #[cfg_attr(test, test)]
    #[cfg_attr(test, ignore = "requires original EM1 files in BLOTWORX_TEST_DATA")]
    fn demo_opening_executes_authored_story_instead_of_aborting() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::super::assets::load(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        world.checkpoint.service = Some(super::super::save_service::Service::open(
            std::path::PathBuf::from(format!("target/demo-story-save-{}", std::process::id())),
            0,
            world.frame_clock.hardware_sample(),
        )?);
        world.enable_demo_story()?;
        world.enable_demo_abilities()?;
        world.camera_lens.set_renderer_aspect(16. / 9.)?;
        let catalog=world.story_sequences.resources.iter().map(|r|->Result<_>{
            let node=sequence(&r.bytes,&r.path)?;
            Ok(serde_json::json!({"name":r.name,"events":list(&node,"Events").iter().map(|e|serde_json::json!({"kind":number(e,"_type"),"text":text(e,"Text key"),"lua":text(e,"Lua")})).collect::<Vec<_>>()}))
        }).collect::<Result<Vec<_>>>()?;
        let usable=world.entities.iter().enumerate().filter(|(_,e)|e.components.iter().any(|c|c.class_name=="UsableComponent"))
            .map(|(id,e)|serde_json::json!({"id":id,"name":e.name,"outputs":e.components.iter().filter(|c|c.class_name=="InputOutputComponent").flat_map(|c|c.properties.iter()).filter(|p|p.name=="Output").map(|p|p.value.clone()).collect::<Vec<_>>()})).collect::<Vec<_>>();
        let trigger_catalog=world.entities.iter().enumerate().filter(|(_,e)|e.components.iter().any(|c|c.class_name.starts_with("Trigger")))
            .map(|(id,e)|serde_json::json!({"id":id,"name":e.name,"position":e.transform[12..15],"shape":world.phantom_bounds(id).ok().flatten(),
                "components":e.components.iter().filter(|c|c.class_name.starts_with("Trigger")||c.class_name=="InputOutputComponent")
                    .map(|c|serde_json::json!({"class":c.class_name,"properties":c.properties.iter().map(|p|(p.name.clone(),p.value.clone())).collect::<BTreeMap<_,_>>()})).collect::<Vec<_>>()})).collect::<Vec<_>>();
        let counter_catalog=world.entities.iter().enumerate().filter(|(_,e)|e.components.iter().any(|c|c.class_name=="PaintThinnerCounterComponent"))
            .map(|(id,e)|serde_json::json!({"id":id,"name":e.name,"components":e.components.iter().filter(|c|matches!(c.class_name.as_str(),"InputOutputComponent"|"GameEventsComponent"|"PaintThinnerCounterComponent"))
                .map(|c|serde_json::json!({"class":c.class_name,"properties":c.properties.iter().map(|p|(p.name.clone(),p.value.clone())).collect::<BTreeMap<_,_>>()})).collect::<Vec<_>>()})).collect::<Vec<_>>();
        let inputs = world
            .entities
            .iter()
            .flat_map(|e| e.components.iter())
            .filter(|c| c.class_name == "InputOutputComponent")
            .flat_map(|c| c.properties.iter())
            .filter(|p| p.name == "Input")
            .filter_map(|p| p.value.as_array())
            .flatten()
            .filter_map(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .collect::<std::collections::BTreeSet<_>>();
        let missing = inputs
            .into_iter()
            .filter(|name| !world.scripts.has_callable(name).unwrap_or(false))
            .collect::<Vec<_>>();
        std::fs::write(
            "target/demo-level-wiring.json",
            serde_json::to_vec_pretty(
                &serde_json::json!({"triggers":trigger_catalog,"counters":counter_catalog,"missing_inputs":missing}),
            )?,
        )?;
        assert!(missing.is_empty(), "Unbound scene inputs: {missing:?}");
        for _ in 0..600 {
            if world.demo_story.dialogue.is_some() {
                world.acknowledge_demo_story(0);
            }
            world.advance_models_with_delta(
                world.animation_time + super::super::STEP,
                super::super::STEP,
            )?;
        }
        let input_after_intro = world.cutscene_input.restricted;
        let enemy = world
            .scripts
            .resolve_name("gremlinvillage_zonea.SpatterMelee 01")
            .context("Demo enemy missing")?;
        world.run_callback_fixture("ExitCutscene('gremlinvillage_zonea.SpatterMelee 01'); EnableMotion('gremlinvillage_zonea.SpatterMelee 01'); UnpauseAllAI()")?;
        for _ in 0..4 {
            world.advance_models_with_delta(
                world.animation_time + super::super::STEP,
                super::super::STEP,
            )?;
        }
        let player: [f32; 3] = world.entities[world.player_entity].transform[12..15].try_into()?;
        world.relocate_entity(
            enemy,
            [player[0] + 4., player[1], player[2]],
            world.entities[enemy].runtime_rotation,
        )?;
        let health_before = world.runtime_health[&world.player_entity].health;
        for _ in 0..120 {
            world.advance_demo_ai(super::super::STEP)?;
            world.advance_dynamic_physics(super::super::STEP)?;
            world.advance_models_with_delta(
                world.animation_time + super::super::STEP,
                super::super::STEP,
            )?;
            if world.runtime_health[&world.player_entity].health < health_before {
                break;
            }
        }
        let combat = serde_json::json!({"health_before":health_before,"health_after":world.runtime_health[&world.player_entity].health,
            "movement_updates":world.demo_story.ai.movement_updates,"attacks":world.demo_story.ai.attacks,"input_after_intro":input_after_intro});
        let pipe = world
            .scripts
            .resolve_name("gremlinvillage_zonea_static.GV_SteamPipeB1_Toon 01")
            .context("Pipe puzzle missing")?;
        // Exercise the actual collision -> projectile -> area stimulus path,
        // rather than calling the counter directly and hiding filter errors.
        let owner = world.model_paint[&pipe].owners[0];
        let part = &world.report.collision_parts[owner];
        let mesh = part
            .native_resource
            .get(part.native_part)
            .context("Steam-pipe source mesh missing")?;
        let mut contact = None;
        for local in 0..part.triangle_count {
            let face = &mesh.indices[local * 3..local * 3 + 3];
            let [a, b, c] = std::array::from_fn(|corner| {
                let vertex = mesh.vertices[face[corner] as usize];
                super::super::physics::point(
                    &part.entity_transform,
                    super::super::physics::point(&mesh.body_transform, vertex),
                )
            });
            let u = super::super::physics::sub(b, a);
            let v = super::super::physics::sub(c, a);
            let normal = super::super::physics::unit([
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ]);
            let center = std::array::from_fn(|i| (a[i] + b[i] + c[i]) / 3.);
            let origin =
                super::super::physics::add(center, super::super::physics::mul(normal, 0.2));
            if let Some(hit) = world.collision.with_player_queries(|| {
                world.collision.sweep_sphere(
                    origin,
                    super::super::physics::mul(normal, -0.4),
                    0.025,
                )
            })? {
                if hit.owner == owner {
                    contact = Some(hit);
                    break;
                }
            }
        }
        let hit = contact.context("Spray cannot contact the thinned puzzle pipe")?;
        let shot = world.projectile_preview.pool.acquire::<anyhow::Error>(
            super::super::brush::Ink::Paint,
            world.player_entity as u64,
            [0.; 3],
            [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            || Ok(0),
        )?;
        shot.set_launch_velocity([0., 0., 1.], false, 1.);
        let contact = super::super::brush_preview::Contact {
            ink: super::super::brush::Ink::Paint,
            projectile: 0,
            hit,
        };
        for _ in 0..30 {
            world.dispatch_projectile_game_events(&contact)?;
            world.advance_models_with_delta(
                world.animation_time + super::super::STEP,
                super::super::STEP,
            )?;
        }
        for _ in 0..30 {
            world.advance_models_with_delta(
                world.animation_time + super::super::STEP,
                super::super::STEP,
            )?;
        }
        let pipe_painted = world.globals.get("GV_ZoneA_Pipe01");
        let checkpoint_trigger = world
            .scripts
            .resolve_name("gremlinvillage_zonea.Trigger 03")
            .context("Checkpoint trigger missing")?;
        let trigger_position = world.entities[checkpoint_trigger].transform[12..15].try_into()?;
        world.advance_demo_triggers(trigger_position, super::super::STEP)?;
        world.advance_demo_triggers(trigger_position, super::super::STEP)?;
        world.advance_models_with_delta(
            world.animation_time + super::super::STEP,
            super::super::STEP,
        )?;
        std::fs::write(
            "target/demo-story-owned.json",
            serde_json::to_vec_pretty(
                &serde_json::json!({"story":world.demo_story.summary(),"combat":combat,"catalog":catalog,"usable":usable,"triggers":trigger_catalog,"script_errors":world.report.unsupported.iter().filter(|s|s.starts_with("Game Lua:")).collect::<Vec<_>>()}),
            )?,
        )?;
        assert!(
            world.demo_story.errors.is_empty(),
            "{:?}",
            world.demo_story.errors
        );
        assert!(
            world
                .demo_story
                .completed
                .iter()
                .any(|s| s == "GV_ZoneA_TiestowSeesPlayer"),
            "Opening never reached its dialogue"
        );
        assert!(!input_after_intro, "The opening left controls locked");
        assert!(
            world.demo_story.ai.movement_updates > 0,
            "Enemy steering never ran: {combat}"
        );
        assert!(
            world.runtime_health[&world.player_entity].health < health_before,
            "Enemy never dealt damage: {combat}"
        );
        assert_eq!(
            pipe_painted,
            Some(1),
            "Painting the pipe did not reach its original puzzle callback"
        );
        assert!(
            world.demo_story.triggers.entries > 0,
            "Walking into a trigger produced no overlap entry"
        );
        assert!(!world
            .report
            .unsupported
            .iter()
            .any(|s| s.contains("attempt to call global 'FireSequence'")
                || s.contains("attempt to call global 'ClearAllCameraAttributesTimed'")));
        Ok(())
    }
    #[cfg_attr(test, test)]
    #[cfg_attr(test, ignore = "requires original EM1 files in BLOTWORX_TEST_DATA")]
    fn demo_zone_a_pipes_combat_and_exit_chain() -> Result<()> {
        zone_a_encounter(false, false)
    }
    #[cfg_attr(test, test)]
    #[cfg_attr(test, ignore = "requires original EM1 files in BLOTWORX_TEST_DATA")]
    fn demo_zone_a_friendly_combat_and_exit_chain() -> Result<()> {
        zone_a_encounter(true, false)
    }
    #[cfg_attr(test, test)]
    #[cfg_attr(test, ignore = "requires original EM1 files in BLOTWORX_TEST_DATA")]
    fn demo_zone_a_mixed_combat_and_exit_chain() -> Result<()> {
        zone_a_encounter(true, true)
    }
    fn zone_a_encounter(friendly: bool, mixed: bool) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::super::assets::load(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        world.checkpoint.service = Some(super::super::save_service::Service::open(
            std::path::PathBuf::from(format!(
                "target/demo-progression-save-{}",
                std::process::id()
            )),
            0,
            world.frame_clock.hardware_sample(),
        )?);
        world.enable_demo_story()?;
        world.enable_demo_abilities()?;
        world.camera_lens.set_renderer_aspect(16. / 9.)?;
        fn advance(world: &mut super::super::assets::World, count: usize) -> Result<()> {
            let mut player = world.take_startup_player();
            for _ in 0..count {
                if world.demo_story.dialogue.is_some() {
                    world.acknowledge_demo_story(0);
                }
                world.with_live_player(&mut player, |world| {
                    world.advance_demo_motion(super::super::STEP)?;
                    world.advance_models_with_delta(
                        world.animation_time + super::super::STEP,
                        super::super::STEP,
                    )
                })?;
            }
            world.script_player = player;
            Ok(())
        }
        advance(&mut world, 600)?;
        let door = world
            .scripts
            .resolve_name("gremlinvillage_zonea.GV_ZoneA_CogDoor1 01")
            .context("Cog door missing")?;
        let end = world
            .scripts
            .resolve_name("gremlinvillage_zonea.SplineKnot 11")
            .context("Door open endpoint missing")?;
        let gear = world
            .scripts
            .resolve_name("gremlinvillage_zonea.GV_ZoneA_Gear03_Toon 02")
            .context("Door's paintable gear missing")?;
        // Open the doors through the actual puzzle owner and its authored IO,
        // rather than directly invoking the motor functions being exercised.
        world.demo_counter_stimulus(gear, &[(1, 100.)], false)?;
        advance(&mut world, 120)?;
        assert_eq!(
            world.globals.get("GV_ZoneA_SecondSpatterRunoff"),
            Some(2),
            "Painting the gear did not execute OpenGearDoor"
        );
        let door_error = (12..15)
            .map(|i| (world.entities[door].transform[i] - world.entities[end].transform[i]).powi(2))
            .sum::<f32>()
            .sqrt();
        assert!(
            door_error < 0.01,
            "Door did not reach its requested end knot: {door_error}"
        );
        for suffix in ["01", "02", "03", "04"] {
            let name = format!("gremlinvillage_zonea_static.GV_SteamPipeB1_Toon {suffix}");
            let pipe = world
                .scripts
                .resolve_name(&name)
                .with_context(|| format!("Missing {name}"))?;
            world.demo_counter_stimulus(pipe, &[(1, 100.)], false)?;
        }
        advance(&mut world, 60)?;
        for suffix in ["01", "02", "03", "04"] {
            assert_eq!(
                world.globals.get(&format!("GV_ZoneA_Pipe{suffix}")),
                Some(1)
            );
        }
        let gate = world
            .scripts
            .resolve_name("gv_zonea_airlock.PlaneTrigger 01")
            .context("Combat gate missing")?;
        anyhow::ensure!(world.entity_scene_active(gate),
            "Encounter fixture must activate the airlock before crossing its plane");
        let e = &world.entities[gate];
        let center: [f32; 3] = e.transform[12..15].try_into()?;
        let r = e.runtime_rotation;
        // Cross the authored plane, then deliver its pending IO on the next
        // trigger tick, as the interactive BeforeMovement phase does. Model
        // stepping alone does not service demo trigger pending outputs.
        for sign in [-1., 1., 1.] {
            let position = std::array::from_fn(|i| {
                center[i] + r[i * 3 + 2] * sign - if i == 1 { 0.6 } else { 0. }
            });
            world.advance_demo_triggers(position, super::super::STEP)?;
        }
        advance(&mut world, 900)?;
        world.run_callback_fixture("BwxOutcomeCounts={}; do local original=GV_ZoneA_SpatterCountMaster; function GV_ZoneA_SpatterCountMaster() table.insert(BwxOutcomeCounts,GetGlobal('GV_ZoneA_SpatterCount')); return original() end end")?;
        for (index, suffix) in ["05", "04"].into_iter().enumerate() {
            let marker=world.scripts.resolve_name("gremlinvillage_zonea.PositionMarker 09").context("Combat relocation marker absent")?;
            anyhow::ensure!(world.entities[marker].name=="gv_zonea_airlock.PositionMarker 09",
                "Combat relocation resolved stale previous-area marker: {}",world.entities[marker].name);
            // Encounter Lua addresses the Spawn Group alias, not the hidden
            // source template. Stimulating the template can run its IO and
            // falsely pass progression without a playable enemy ever existing.
            let name = format!("gremlinvillage_zonea.SpatterMelee {suffix}");
            let enemy = world
                .scripts
                .resolve_name(&name)
                .with_context(|| format!("Missing {name}"))?;
            anyhow::ensure!(world.spawners.values().any(|state| state.live.contains(&enemy)),
                "Encounter outcome targeted a template instead of a live spawn: {name}");
            anyhow::ensure!(world.entity_scene_active(enemy)
                && !world.hidden_entities.contains(&enemy)
                && !world.removed_entities.contains(&enemy),
                "Encounter enemy is not active and visible before combat: {name}");
            let body = world.character_physics.get(&enemy)
                .context("Encounter spawn has no character body")?.as_ref()
                .map_err(|error|anyhow::anyhow!("{name}: {error}"))?;
            anyhow::ensure!(world.collision.owner_in_world(body.owner)?
                && body.motion.current.motion_type()==7,
                "Encounter spawn is not admitted to gameplay physics: {name}");
            world.demo_counter_stimulus(enemy, &[(if friendly { 1 } else { 0 }, 100.)], false)?;
            advance(&mut world, 600)?;
            if friendly {
                world.run_callback_fixture(&format!("assert(GetCurrentTeam('{name}')==3,'Painted Spatter must use its converted team')"))?;
            }
            if mixed && index == 0 {
                world.run_callback_fixture("BwxOutcomeCounts={}")?;
                world.demo_counter_stimulus(enemy, &[(0, 100.)], false)?;
                advance(&mut world, 600)?;
                assert_eq!(
                    world.globals.get("GV_ZoneA_SpatterCount"),
                    Some(1),
                    "One enemy must contribute exactly one outcome"
                );
                world.run_callback_fixture("assert(BwxOutcomeCounts[1]==1,'A converted enemy dying must not prematurely finish the encounter')")?;
            }
        }
        let gremlin = world
            .scripts
            .resolve_name("gv_zonea_airlock.Gremlin_Variant4 01")
            .context("Airlock gremlin missing")?;
        // This encounter does not hand control to the gremlin for a manual
        // conversation. The authored GV_ZoneA_TiestowOpenExit coroutine
        // owns his teleport/visibility lifecycle; the authored exit trigger
        // is the next progression step. Teleport effects and cutscene phases
        // make visibility alone an unreliable completion signal.
        advance(&mut world, 600)?;
        anyhow::ensure!(
            world.globals.get("GV_ZoneA_ExitOpened") == Some(1),
            "Gremlin exit did not set its authored open flag"
        );
        let exit = world
            .scripts
            .resolve_name("gv_zonea_airlock.ConversationalTrigger_GusAirlock")
            .context("Exit conversation missing")?;
        let position = world.entities[exit].transform[12..15].try_into()?;
        world.advance_demo_triggers(position, super::super::STEP)?;
        advance(&mut world, 600)?;
        let stage = serde_json::json!({"story":world.demo_story.summary(),"combat_training":world.globals.get("GV_ZoneA_CombatTraining"),
            "spatter_count":world.globals.get("GV_ZoneA_SpatterCount"),"spatters_dead":world.globals.get("GV_ZoneA_SpattersDead"),
            "gremlin_hidden":world.hidden_entities.contains(&gremlin),"lua_task_errors":world.scripts.errors,
            "script_errors":world.report.unsupported.iter().filter(|s|s.starts_with("Game Lua:")).collect::<Vec<_>>()});
        std::fs::write(
            if mixed {
                "target/demo-progression-mixed-owned.json"
            } else if friendly {
                "target/demo-progression-friendly-owned.json"
            } else {
                "target/demo-progression-owned.json"
            },
            serde_json::to_vec_pretty(&stage)?,
        )?;
        assert_eq!(
            world.globals.get("GV_ZoneA_CombatTraining"),
            Some(1),
            "Combat gate never activated: {stage}"
        );
        assert_eq!(
            world.globals.get("GV_ZoneA_SpatterCount"),
            Some(2),
            "Both enemy outcomes must advance the encounter: {stage}"
        );
        assert!(world.demo_story.errors.is_empty(), "{stage}");
        assert!(
            !world
                .report
                .unsupported
                .iter()
                .any(|s| s.starts_with("Game Lua:")),
            "{stage}"
        );
        assert!(
            world
                .demo_story
                .completed
                .iter()
                .any(|s| s == "GV_ZoneA_GusOswaldSighting01"),
            "Exit conversation never completed: {stage}"
        );
        Ok(())
    }
}
pub struct Dialogue {
    pub speaker: String,
    pub text: String,
    pub choices: Vec<(String, String)>,
    pub portrait: Option<String>,
}
struct Bark {
    line: Dialogue,
    remaining: f32,
    from_sequence: bool,
}
#[derive(Clone)]
struct StoryEvent {node:Node,key:(usize,Vec<usize>)}
impl std::ops::Deref for StoryEvent {type Target=Node;fn deref(&self)->&Node {&self.node}}
fn events(resource:usize,prefix:&[usize],nodes:&[Node])->Vec<StoryEvent> {
    nodes.iter().enumerate().map(|(i,node)| {let mut path=prefix.to_vec();path.push(i);StoryEvent {node:node.clone(),key:(resource,path)}}).collect()
}
struct Playback {
    owner: usize,
    name: String,
    events: Vec<StoryEvent>,
    cursor: usize,
    delay: f32,
    restricted: bool,
}
fn list<'a>(n: &'a Node, key: &str) -> &'a [Node] {
    if let Node::Record { fields, .. } = n {
        for field in fields {
            if let Node::List { name, items, .. } = field {
                if name == key {
                    return items;
                }
            }
        }
    }
    &[]
}
fn text<'a>(n: &'a Node, key: &str) -> &'a str {
    n.value(key).and_then(|v| v.as_str()).unwrap_or("")
}
fn number(n: &Node, key: &str) -> i32 {
    n.value(key).and_then(|v| v.as_i64()).unwrap_or(0) as i32
}
fn sequence(bytes: &[u8], path: &str) -> Result<Node> {
    let doc = Document::load(path, bytes)?;
    let Node::Record { fields, .. } = doc.root else {
        bail!("Missing story root")
    };
    fields
        .into_iter()
        .find(|n| n.name() == "Sequence")
        .context("Missing story sequence")
}
impl State {
    pub(super) fn presented_dialogue(&self) -> Option<(&Dialogue, bool)> {
        self.dialogue.as_ref().map(|line| (line, true))
            .or_else(|| self.bark.as_ref().map(|bark| (&bark.line, false)))
    }
    pub fn load_strings(&mut self, files: &[epic_mickey_lib_rs::packfile::VirtualFile]) {
        for file in files
            .iter()
            .filter(|file| file.path.to_ascii_lowercase().ends_with(".dct"))
        {
            if !self.dictionary_paths.insert(file.path.replace('\\',"/").to_ascii_lowercase()) {continue;}
            let dct = epic_mickey_lib_rs::dct::DCT::from_binary(file.data.clone());
            self.dictionaries.push((
                dct.hash_seed,
                dct.dialog_entries
                    .into_iter()
                    .map(|entry| (entry.hashed_key, entry.text))
                    .collect(),
            ));
        }
    }
    fn localized(&self, key: &str) -> String {
        for (seed, entries) in &self.dictionaries {
            if let Some(text) = entries.get(&epic_mickey_lib_rs::dct::epic_mickey_hash(
                key.as_bytes(),
                *seed,
            )) {
                return text.clone();
            }
        }
        key.to_owned()
    }
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({"enabled":self.enabled,"reward_events":self.reward_events,"cancelled":self.cancelled,"last_cancelled":self.last_cancelled,"last_cancelled_detail":self.last_cancelled_detail,"random_dialogue_events":self.random_dialogue.len(),"random_dialogue_picks":self.random_dialogue_picks,"active":self.active.as_ref().map(|p|&p.name),
        "dictionary_count":self.dictionaries.len(),"ai_movement_updates":self.ai.movement_updates,"ai_attacks":self.ai.attacks,"ai_decisions":self.ai.decisions,
        "seer_charge":{"windups":self.ai.charge_windups,"rolls":self.ai.charge_rolls,"contacts":self.ai.charge_contacts,"recoveries":self.ai.charge_recoveries},
        "ai_projectiles": self.projectiles.summary(),
        "trigger_entries":self.triggers.entries,"trigger_state":self.triggers.summary(),"pickups_collected":self.triggers.pickups,
        "event":self.active.as_ref().map(|p|p.cursor),"delay_seconds":self.active.as_ref().map(|p|p.delay),
        "pending_deaths":self.pending_deaths.len(),"dialogue":self.dialogue.as_ref().map(|d|&d.text),
        "bark":self.bark.as_ref().map(|b|serde_json::json!({"text":b.line.text,"remaining":b.remaining,"from_sequence":b.from_sequence})),
        "completed":self.completed,"errors":self.errors,
        "effect_warnings":self.scene.effect_warnings,"emitter_requests":self.scene.emitter_requests})
    }
    pub fn acknowledge(&mut self, choice: usize) {
        if let Some(dialogue) = self.dialogue.take() {
            if let (Some(playback), Some((_, label))) =
                (&mut self.active, dialogue.choices.get(choice))
            {
                if let Some(index) = playback
                    .events
                    .iter()
                    .position(|n| text(n, "Label") == label)
                {
                    playback.cursor = index;
                }
            }
        }
    }
}
impl super::assets::World {
    // A BSQ Camera event can own its shot. A dialogue-only BSQ can run over
    // a shot acquired by a Lua coroutine, which still controls its release.
    fn release_demo_story_camera(&mut self) {
        if self.demo_story.owns_grabbed_camera {
            self.clear_grabbed_camera();
        }
    }
    /// Retail presentation adapter12fcc and instance completion2844dc.
    /// This is gameplay Status.Invisible, never render visibility or collision.
    fn story_player_status(&mut self,value:bool)->Result<()> {
        if !self.removed_entities.contains(&self.player_entity) {
            if let Some(player)=self.entities.get_mut(self.player_entity) {
                super::entity_status::write(&mut player.components,value)?;
            }
        }
        Ok(())
    }
    /// Cancel the functional presentation owner without consuming another
    /// script's nested input restriction or synthesizing successful completion.
    pub(super) fn cancel_story_owner(&mut self,owner:usize) {
        if !self.demo_story.active.as_ref().is_some_and(|p|p.owner==owner) {return;}
        let p=self.demo_story.active.take().unwrap();
        self.release_demo_story_camera();
        self.demo_story.last_cancelled_detail=Some(serde_json::json!({
            "owner":owner,"name":self.entities[owner].name,"scene":self.entities[owner].scene,
            "time":self.animation_time,"retired":self.removed_entities.contains(&owner),
            "scene_active":self.entity_scene_active(owner),"event_cursor":p.cursor,"event_count":p.events.len(),
            "previous_event_key":p.cursor.checked_sub(1).and_then(|i|p.events.get(i)).map(|event|event.key.clone()),
            "next_event_key":p.events.get(p.cursor).map(|event|event.key.clone()),
            "pending_delay":p.delay,"dialogue_active":self.demo_story.dialogue.is_some(),
            "resource":self.demo_story.current_request.and_then(|request|self.story_sequences.resources.get(request.resource)).map(|r|r.path.clone())}));
        if self.demo_story.dialogue.take().is_some() {self.cutscene_input.unrestrict();}
        if self.demo_story.bark.as_ref().is_some_and(|bark| bark.from_sequence) {
            self.demo_story.bark = None;
        }
        if p.restricted {self.cutscene_input.unrestrict();}
        if let Err(error)=self.story_player_status(false) {self.demo_story.errors.push(format!("Story cancellation status cleanup: {error:#}"));}
        self.demo_story.current_request=None;
        self.demo_story.cancelled=self.demo_story.cancelled.wrapping_add(1);
        self.demo_story.last_cancelled=Some(p.name);
    }
    pub(super) fn cancel_inactive_story(&mut self) {
        if let Some(owner)=self.demo_story.active.as_ref().map(|p|p.owner) {
            if self.removed_entities.contains(&owner)||!self.entity_scene_active(owner) {self.cancel_story_owner(owner);}
        }
    }
    pub fn enable_demo_story(&mut self) -> Result<()> {
        if self.report.level.eq_ignore_ascii_case("Levels/TL_SpaceMountain.Level") {
            // Intro_Setup releases two sets of Minihub Lua callbacks. A direct
            // level start has neither module to release. The original functions
            // only clear their globals; keep real handlers on streamed entry.
            self.scripts.ensure_global_module("@space-mountain-direct-entry.lua", br#"
                if not rawget(_G, 'Cleanup_TL_Gear_Puzzle') then
                    rawset(_G, 'Cleanup_TL_Gear_Puzzle', function()
                        rawset(_G, 'Cleanup_TL_Gear_Puzzle', nil)
                    end)
                end
                if not rawget(_G, 'Cleanup_TL_Minihub') then
                    rawset(_G, 'Cleanup_TL_Minihub', function()
                        rawset(_G, 'Cleanup_TL_Minihub', nil)
                    end)
                end
            "#)?;
        }
        if self.report.level.eq_ignore_ascii_case("Levels/PotW_JollyRoger_Start.Level") {
            self.scripts.ensure_global_module("@jolly-roger-direct-entry.lua", br#"
                if not rawget(_G, 'StartPetePanWander') then
                    rawset(_G, 'StartPetePanWander', function() end)
                end
            "#)?;
        }
        self.demo_story.enabled = true;
        self.demo_story.npc_fallback.enabled = true;
        let limitation="Temporary NPC fallback: verified scripted/conversation events and task waits with provisional Gremlin patrol/navigation and immediate action draining. Damage reactions, attraction/follow policies and full native navigation remain incomplete. Use --strict-npc-ai to disable this fallback.";
        if !self.report.unsupported.iter().any(|s|s==limitation){self.report.unsupported.push(limitation.into());}
        self.demo_story.hud = true;
        self.demo_story.reticle_visible = true;
        self.demo_story.drips_visible = true;
        self.initialize_demo_motion();
        self.initialize_demo_counters();
        let packs = super::assets::files_root(self.source_data_root()).join("packfiles");
        if let Some(path) = std::fs::read_dir(&packs)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("English_NTSC.pak"))
            })
        {
            let parsed = crate::pack_io::read(std::fs::read(path)?)?;
            self.demo_story.load_strings(&parsed.pak.files);
        }
        for name in [
            "FireSequence",
            "ClearAllCameraAttributesTimed",
            "ReleaseCamera",
            "ShowHud",
            "HideReticleOnly",
            "ShowReticleOnly",
            "PlayerSetDripsVisible",
            "SetSplineFollowerInitialSpeed",
            "SplineFollower_StopAtPosition",
            "SplineFollower_ClearStopPosition",
            "SetNextPatrolNode",
            "ClearNextPatrolNode",
            "ResumePatrol",
            "GetCurrentTeam",
            "CameraReset",
            "AudioSetState",
            "StartEmitters",
            "StopEmitters",
            "ForcePainted",
            "ForceThinned",
            "StartTrackingCurrentLevelForScoring",
            "EndTrackingCurrentLevelForScoring",
        ] {
            self.scripts.bind_missing_world_api(name)?;
        }
        for name in [
            "SetCharacterInConversation",
            "SetCameraAttributes",
            "ClearAllCameraAttributes",
            "ClearAllCameraAttributesSD",
            "ClearAllCameraAttributesTimedSD",
            "SaveCameraOverride",
            "LoadSavedCameraOverride",
        ] {
            self.scripts.bind_missing_world_api(name)?;
        }
        for name in [
            "HideHud",
            "AudioSetSwitch",
            "NativeAudioSetState",
            "NativeAudioSetSwitch",
            "CounterAdd",
            "CounterSubtract",
            "Reverse",
            "SetParentEntity",
            "ClearParent",
            "SetAppCulled",
            "ClearAppCulled",
            "SetSpeed",
            "SplineFollower_SetDisabled",
            "SplineFollower_TeleportToKnot",
        ] {
            self.scripts.bind_missing_world_api(name)?;
        }
        Ok(())
    }
    pub fn acknowledge_demo_story(&mut self, choice: usize) {
        if self
            .demo_story
            .dialogue
            .as_ref()
            .is_some_and(|d| !d.choices.is_empty() && choice >= d.choices.len())
        {
            return;
        }
        if self.demo_story.dialogue.is_some() {
            self.cutscene_input.unrestrict();
        }
        self.demo_story.acknowledge(choice);
    }
    pub fn usable_target(&self, position: [f32; 3]) -> Option<usize> {
        if self.cutscene_input.restricted || self.demo_story.dialogue.is_some() {
            return None;
        }
        super::usable_selection::nearest(position,self.entities
            .iter()
            .enumerate()
            .filter_map(|(entity, owner)| {
                if self.removed_entities.contains(&entity)
                    || self.hidden_entities.contains(&entity)
                    || !self.entity_scene_active(entity)
                {
                    return None;
                }
                let (index, usable) = owner
                    .components
                    .iter()
                    .enumerate()
                    .find(|(_, c)| c.class_name == "UsableComponent")?;
                if self
                    .component_control
                    .disabled
                    .get(&entity)
                    .and_then(|v| v.get(&index))
                    == Some(&true)
                {
                    return None;
                }
                let radius = usable
                    .properties
                    .iter()
                    .find(|p| p.name == "UsableRadius")
                    .and_then(|p| p.value.as_f64())
                    .unwrap_or(0.) as f32;
                (radius > 0.).then_some((entity,[owner.transform[12],owner.transform[13],owner.transform[14]],radius))
            }))
    }
    pub fn use_usable(&mut self, entity: usize) -> Result<()> {
        let owner = self.entities.get(entity).context("Unknown usable entity")?;
        anyhow::ensure!(
            !self.removed_entities.contains(&entity),
            "Usable entity {} has been retired",
            owner.name
        );
        anyhow::ensure!(
            !self.hidden_entities.contains(&entity),
            "Usable entity {} is hidden",
            owner.name
        );
        anyhow::ensure!(
            self.entity_scene_active(entity),
            "Usable entity {} belongs to an inactive scene {}",
            owner.name,
            owner.scene
        );
        let usable_index = owner
            .components
            .iter()
            .position(|component| component.class_name == "UsableComponent")
            .context("Entity has no UsableComponent")?;
        anyhow::ensure!(
            self.component_control
                .disabled
                .get(&entity)
                .and_then(|disabled| disabled.get(&usable_index))
                != Some(&true),
            "Usable component is disabled"
        );
        let once = owner
            .components
            .iter()
            .filter(|c| c.class_name == "UsableComponent")
            .any(|c| {
                c.properties
                    .iter()
                    .any(|p| p.name == "UsableOnce" && p.value.as_bool() == Some(true))
            });
        let routed = self.outputs.has_connections(entity,"OnUse");
        {
            // Retail3d4f2c..60: source is the usable owner, activator is
            // the interacting entity. Admission precedes use-once disablement.
            self.entity_event_queue.enqueue(&super::entity_events::Event::Basic(super::entity_events::Basic {
                event_kind:0x0e,source:Some(entity),header:Some(self.player_entity),
            }),None)?;
        }
        // Existing exploration conversation bridge; native Conversation
        // receiver construction remains separate. An ineligible sequence is
        // normal admission, not an interaction failure.
        if !routed && self.entities[entity].components.iter().any(|c|c.class_name=="ConversationComponent") {
            let _admission=self.request_conversation(entity)?;
        }
        if once {
            self.set_component_disabled(entity,usable_index,true)?;
        }
        Ok(())
    }
    pub(super) fn demo_story_lua(
        &mut self,
        lua: &mlua::Lua,
        name: &str,
        args: mlua::MultiValue,
    ) -> mlua::Result<mlua::MultiValue> {
        if name == "SetCharacterInConversation" {
            if args.len() != 2 {
                return Err(mlua::Error::external(
                    "Conversation state requires entity and enabled flag",
                ));
            }
            if let Some(entity) = self.scripts.resolve_world_argument(&args[0])? {
                if super::actor_scripts::native_boolean(lua, args[1].clone())? {
                    self.demo_story.conversation.insert(entity);
                } else {
                    self.demo_story.conversation.remove(&entity);
                }
            }
        } else if name == "SetCameraAttributes" {
            let owner = args
                .front()
                .map(|v| self.scripts.resolve_world_argument(v))
                .transpose()?
                .flatten();
            self.demo_story.camera_override = owner;
            // An authored spline override selects the gameplay camera path.
            // The DBC first-machine return uses this without ReleaseCamera.
            if owner.is_some_and(|entity| self.entities[entity].components.iter()
                .filter(|component| component.class_name == "CameraOverrideComponent")
                .flat_map(|component| &component.properties)
                .any(|property| property.name == "CameraSplineKnot"
                    && property.value.as_u64().is_some_and(|knot| knot != 0 && knot < u32::MAX as u64)))
            {
                self.clear_grabbed_camera();
            }
        } else if matches!(name, "SaveCameraOverride" | "SaveCurrentCameraOverride") {
            self.demo_story.saved_camera_override = self.demo_story.camera_override;
        } else if name == "LoadSavedCameraOverride" {
            self.demo_story.camera_override = self.demo_story.saved_camera_override;
        } else if name == "PlayerSetDripsVisible" {
            if args.len() != 2 {
                return Err(mlua::Error::external(
                    "PlayerSetDripsVisible requires player and visibility",
                ));
            }
            let Some(owner) = self.scripts.resolve_world_argument(&args[0])? else {
                return Ok(mlua::MultiValue::new());
            };
            let visible = super::actor_scripts::native_boolean(lua, args[1].clone())?;
            let player = &self.entities[owner];
            let mut children = Vec::new();
            for (index, entity) in self.entities.iter().enumerate() {
                if entity.scene == player.scene
                    && super::scene_particles::player_drip(&entity.components)
                    && entity
                        .components
                        .iter()
                        .any(|c| c.class_name == "ParticleSystemControllerComponent")
                    && entity
                        .transform_parent_id()
                        .map_err(mlua::Error::external)?
                        == Some(player.id)
                {
                    children.push(index);
                }
            }
            self.admit_scene_particles();
            for child in children {
                self.demo_story.scene.emitter_requests.insert(child, visible);
                self.scene_particles.set_enabled(child, visible).map_err(mlua::Error::external)?;
                self.set_entity_hidden(child, !visible)
                    .map_err(mlua::Error::external)?;
            }
            self.demo_story.drips_visible = visible;
        } else if name == "HideReticleOnly" || name == "ShowReticleOnly" {
            self.demo_story.reticle_visible = name == "ShowReticleOnly";
        } else if name == "ShowHud" || name == "HideHud" {
            self.demo_story.hud = name == "ShowHud"
                && super::actor_scripts::native_boolean(
                    lua,
                    args.front().cloned().unwrap_or(mlua::Value::Boolean(true)),
                )?;
            if self.demo_story.hud {
                self.screen_fade.hide_gameplay_hud = false;
            }
        } else if matches!(
            name,
            "ClearAllCameraAttributes"
                | "ClearAllCameraAttributesSD"
                | "ClearAllCameraAttributesTimedSD"
                | "ClearAllCameraAttributesTimed"
                | "ReleaseCamera"
                | "CameraReset"
        ) {
            // Demo approximation: hand control back to the existing player
            // camera immediately. Authored cameras still own their sequences.
            if name == "ReleaseCamera" && args.len() != 2 {
                return Err(mlua::Error::external("Incorrect camera release arguments"));
            }
            self.demo_story.camera_override = None;
            self.clear_grabbed_camera();
        } else {
            if args.len() < 2 || (name == "ForceSequence" && args.len() != 2) {
                return Err(mlua::Error::external(
                    "Sequence request requires an entity and a sequence",
                ));
            }
            let entity = self.scripts.resolve_world_argument(&args[0])?;
            let sequence_name = lua
                .coerce_string(args[1].clone())?
                .context("Sequence name must be text")
                .map_err(mlua::Error::external)?;
            if let Some(entity) = entity {
                let sequence = sequence_name.to_str()?;
                let admission = if name == "ForceSequence" {
                    self.force_story_sequence(entity, &sequence)
                } else {
                    self.request_story_sequence(entity, &sequence)
                }.map_err(mlua::Error::external)?;
                if self.demo_story.enabled
                    && matches!(
                        admission,
                        super::story_sequences::Admission::Unregistered
                            | super::story_sequences::Admission::Ineligible
                    )
                {
                    self.report.unsupported.push(format!(
                        "FireSequence {sequence:?} on {} was not admitted: {admission:?}",
                        self.entities[entity].name
                    ));
                }
            }
        }
        Ok(mlua::MultiValue::new())
    }
    fn conversation_portrait(&self, owner: usize, speaker: &str) -> Option<String> {
        let portrait = |entity: usize, match_role: bool| {
            let conversation = self.entities.get(entity)?.components.iter()
                .find(|component| component.class_name == "ConversationComponent")?;
            if match_role && !conversation.properties.iter()
                .find(|property| property.name == "Roles")
                .and_then(|property| property.value.as_array())
                .is_some_and(|roles| roles.iter().any(|role| {
                    role.as_str().is_some_and(|role| role.eq_ignore_ascii_case(speaker))
                }))
            {
                return None;
            }
            conversation.properties.iter().find(|property| property.name == "Portrait")
                .and_then(|property| property.value.as_str())
                .filter(|path| !path.is_empty())
                .map(str::to_owned)
        };
        if let Some(path) = portrait(owner, !speaker.is_empty()) {
            return Some(path);
        }
        if let Some(path) = self.scripts.resolve_name(speaker).and_then(|entity| portrait(entity, false)) {
            return Some(path);
        }
        // BSQ speakers commonly name a conversation role instead of a scene
        // entity. Prefer the sequence owner's scene among active role matches.
        self.entities.iter().enumerate()
            .filter(|(entity, _)| !self.removed_entities.contains(entity) && self.entity_scene_active(*entity))
            .filter_map(|(entity, actor)| portrait(entity, true).map(|path| (actor.scene != self.entities[owner].scene, path)))
            .min_by_key(|(other_scene, _)| *other_scene)
            .map(|(_, path)| path)
    }

    fn bark_display_time(&self) -> Result<f32> {
        self.runtime_settings.get("BarkDisplayTime")
            .map(|value| super::config_value::decimal_float(value).map_err(anyhow::Error::msg))
            .transpose().map(|seconds| seconds.unwrap_or(1.))
    }

    // REL1e7234 and 1e70c4. A Lua bark opens GenericDialog with SHOWA=false
    // and owns a timed presentation, with no sequence or input restriction.
    pub(super) fn bark_lua(&mut self, lua: &mlua::Lua, args: mlua::MultiValue) -> mlua::Result<mlua::MultiValue> {
        if !(2..=5).contains(&args.len()) {
            return Err(mlua::Error::external("Bark requires a target, text, and optional display time and speaker"));
        }
        let target = self.scripts.resolve_native_entity_argument(lua, args[0].clone())?;
        if target != Some(self.player_entity) {
            return Ok(mlua::MultiValue::new());
        }
        let string = |index: usize| -> mlua::Result<String> {
            args.get(index).map(|value| lua.coerce_string(value.clone()))
                .transpose().map(|value| value.flatten().map_or_else(String::new, |s| s.to_string_lossy()))
        };
        let text = string(1)?.replace("\\n", "\n");
        let remaining = if let Some(value) = args.get(2) {
            lua.coerce_number(value.clone())?.unwrap_or(0.) as f32
        } else {
            self.bark_display_time().map_err(mlua::Error::external)?
        };
        if !text.is_empty() && remaining.is_finite() && remaining > 0. {
            let speaker = string(4)?;
            let from_sequence = self.demo_story.bark.as_ref().is_some_and(|bark| bark.from_sequence);
            self.demo_story.bark = Some(Bark {
                line: Dialogue { speaker, text, choices: Vec::new(), portrait: None },
                remaining,
                from_sequence,
            });
        }
        Ok(mlua::MultiValue::new())
    }

    pub(super) fn advance_demo_story(&mut self, delta: f32) {
        if let Some(bark) = &mut self.demo_story.bark {
            bark.remaining = (bark.remaining - delta).max(0.);
            if bark.remaining == 0. {
                self.demo_story.bark = None;
            }
        }
        // The sequence queue is shared by every scene. The demo flag controls
        // only the GV-specific trigger/AI scaffolding; authored conversations
        // and their UI must advance in ordinary levels as well.
        if !self.demo_story.enabled
            && self.demo_story.active.is_none()
            && self.story_sequences.queue.is_empty()
        {
            return;
        }
        if let Err(error) = self.step_demo_story(delta) {
            self.demo_story.errors.push(format!("{error:#}"));
            if let Some(p) = self.demo_story.active.take() {
                if p.restricted {
                    self.cutscene_input.unrestrict();
                }
            }
            self.demo_story.current_request = None;
            self.demo_story.dialogue = None;
            if self.demo_story.bark.as_ref().is_some_and(|bark| bark.from_sequence) {
                self.demo_story.bark = None;
            }
        }
    }
    fn step_demo_story(&mut self, delta: f32) -> Result<()> {
        self.cancel_inactive_story();
        if self.demo_story.active.is_none() {
            self.story_sequences.queue.retain(|request|!self.removed_entities.contains(&request.entity));
            let player_midair_jump = self.script_player_available
                && !self.script_player.grounded
                && !self.script_player.stairs_ground_retained
                && self.script_player.jump_stage.is_some();
            let next=self.story_sequences.queue.iter().position(|request|{
                self.entity_scene_active(request.entity)
                    && (!player_midair_jump
                        || self.story_sequences.resources.get(request.resource).is_none_or(|r| r.flags & 1 == 0))
            });
            if let Some(request)=next.and_then(|index|self.story_sequences.queue.remove(index)) {
                let resource = &self.story_sequences.resources[request.resource];
                let node = sequence(&resource.bytes, &resource.path)?;
                let restricted = resource.flags & 1 != 0;
                let name=resource.name.clone();let played_key=resource.played_key.clone();
                //281314..281378: ordinary AI action21 precedes instance
                //activation/presentation. Queueing alone does not send it.
                self.cutscene_actions.dispatch_story_conversation(request.entity,
                    super::ai_conversation::Notification::Started,
                    &mut self.entity_event_registry, &mut self.ai_brains)?;
                if restricted {
                    self.cutscene_input.restrict();
                }
                if let Err(error)=self.story_player_status(true) {
                    if restricted {self.cutscene_input.unrestrict();}
                    return Err(error);
                }
                self.globals.set(&played_key, 1);
                self.demo_story.current_request = Some(request);
                self.demo_story.active = Some(Playback {
                    owner: request.entity,
                    name,
                    events: events(request.resource,&[],list(&node,"Events")),
                    cursor: 0,
                    delay: 0.,
                    restricted,
                });
            }
        }
        if self.demo_story.dialogue.is_some()
            || self.demo_story.bark.as_ref().is_some_and(|bark| bark.from_sequence)
        {
            return Ok(());
        }
        let Some(mut p) = self.demo_story.active.take() else {
            return Ok(());
        };
        p.delay = (p.delay - delta).max(0.);
        // One event per update permits scheduled Lua to commit its changes
        // before the following conditional/branch observes them.
        let result = (|| -> Result<bool> {
            if p.delay > 0. {
                return Ok(false);
            }
            let Some(event) = p.events.get(p.cursor).cloned() else {
                return Ok(true);
            };
            p.cursor += 1;
            match number(&event, "_type") {
                0 | 1 | 2 => {
                    let line = if number(&event, "_type") == 1 {
                        let lines=list(&event,"Lines");
                        let selected=self.demo_story.random_dialogue.entry(event.key.clone())
                            .or_insert_with(||super::dialogue_selection::State::new(lines.len()))
                            .select(number(&event,"Selection mode") as u16,||self.game_random.draw())?;
                        self.demo_story.random_dialogue_picks=self.demo_story.random_dialogue_picks.wrapping_add(1);
                        &lines[selected]
                    } else {
                        &event
                    };
                    let speaker = text(&event, "Speaker").to_owned();
                    let choices = list(&event, "Choices")
                        .iter()
                        .map(|n| {
                            (
                                self.demo_story.localized(text(n, "Text key")),
                                text(n, "Target label").to_owned(),
                            )
                        })
                        .collect();
                    let dialogue = Dialogue {
                        portrait: self.conversation_portrait(p.owner, &speaker),
                        speaker,
                        text: self.demo_story.localized(text(line, "Text key")),
                        choices,
                    };
                    // The original Wii sends some of these cues to the
                    // controller speaker. Play the authored event in the
                    // host mix so it is audible with any controller.
                    let audio_event = text(line, "Audio event").trim();
                    if !audio_event.is_empty() {
                        self.post_audio_from_game(p.owner, audio_event);
                    }
                    // REL27ab44 reads the active sequence's bit 0. The
                    // caller passes it to REL1e6da0, which chooses the modal
                    // event or a timed bark using BarkDisplayTime.
                    if p.restricted || number(&event, "_type") == 2 {
                        self.cutscene_input.restrict();
                        self.demo_story.choice = 0;
                        self.demo_story.dialogue = Some(dialogue);
                    } else {
                        let remaining = self.bark_display_time()?;
                        if remaining > 0. {
                            self.demo_story.bark = Some(Bark { line: dialogue, remaining, from_sequence: true });
                        }
                    }
                }
                8 => {
                    let item=super::story_rewards::event_item(&event)?;
                    self.apply_story_reward(item,number(&event,"Amount"))?;
                }
                3 | 4 => {
                    let quest = number(&event, "Quest ID");
                    let state = number(&event, "State") as u16;
                    if event.value("Silent").and_then(|v| v.as_bool()) == Some(true) {
                        self.quests.set_silent(quest, state);
                    } else {
                        if let Some(transition) = self.quests.begin_transition(quest, state) {
                            self.ui_notifications.extend(self.quests.notification(transition));
                        }
                    }
                }
                5 => {
                    let camera = self.scripts.resolve_name(text(&event, "Camera"));
                    let target = self.scripts.resolve_name(text(&event, "Target"));
                    self.select_entity_camera(camera, target, 1, 0.)?;
                    self.demo_story.owns_grabbed_camera = camera.is_some();
                    if event.value("Wait").and_then(|v| v.as_bool()) == Some(true) {
                        p.delay = event
                            .value("Blend seconds")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.) as f32;
                    }
                }
                6 => {
                    for (branch_index,branch) in list(&event,"Branches").iter().enumerate() {
                        if self.demo_condition(branch, p.owner)? {
                            let mut prefix=event.key.1.clone();prefix.push(branch_index);
                            p.events.splice(p.cursor..p.cursor,events(event.key.0,&prefix,list(branch,"Events")));
                            break;
                        }
                    }
                }
                7 => {
                    if self.demo_condition(&event, p.owner)? {
                        let label = text(&event, "Target label");
                        p.cursor = p
                            .events
                            .iter()
                            .position(|n| text(n, "Label") == label)
                            .with_context(|| format!("Missing story label {label}"))?;
                    }
                }
                9 => {
                    let source = text(&event, "Lua").split('\0').next().unwrap_or("");
                    if !source.is_empty() {
                        self.scripts.schedule_story_source(source, p.owner)?;
                    }
                }
                // Close presentation, then execute trailing quest/state events.
                // They are authored after End in the retail GV sequences.
                10 => {
                    self.release_demo_story_camera();
                    if p.restricted {
                        self.cutscene_input.unrestrict();
                        p.restricted = false;
                    }
                    // End terminates this presentation branch. Retail files
                    // retain contiguous quest/state bookkeeping after it, but
                    // a later label/dialogue belongs to another branch.
                    let tail = p.events[p.cursor..]
                        .iter()
                        .take_while(|event| matches!(number(event, "_type"), 3 | 4 | 11))
                        .count();
                    p.events.truncate(p.cursor + tail);
                }
                11 => {
                    for state in list(&event, "State changes") {
                        self.globals
                            .set(text(state, "Name"), number(state, "Value"));
                    }
                }
                12 => {
                    p.delay = event
                        .value("Seconds")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.) as f32
                }
                14 => {
                    let name = text(&event, "Sequence");
                    if let Some((resource_id,resource)) = self
                        .story_sequences
                        .resources
                        .iter().enumerate()
                        .find(|(_,r)| r.name == name || r.key == name)
                    {
                        let node = sequence(&resource.bytes, &resource.path)?;
                        let next = events(resource_id, &[], list(&node, "Events"));
                        self.globals.set(&resource.played_key, 1);
                        if event.value("Flag").and_then(|v| v.as_bool()) == Some(true) {
                            // The chained conversation takes over this presentation.
                            // Publish its Played state before it can branch back to
                            // the original sequence (Tim's Wrench and ThanksFull).
                            let restricted = resource.flags & 1 != 0;
                            match (p.restricted, restricted) {
                                (false, true) => self.cutscene_input.restrict(),
                                (true, false) => self.cutscene_input.unrestrict(),
                                _ => {}
                            }
                            p.restricted = restricted;
                            p.name.clone_from(&resource.name);
                            p.cursor = 0;
                            p.events = next;
                            self.demo_story.current_request = Some(super::story_sequences::Request {
                                entity: p.owner,
                                resource: resource_id,
                            });
                        } else {
                            p.events.splice(p.cursor..p.cursor, next);
                        }
                    } else {
                        bail!("Missing nested sequence {name}");
                    }
                }
                16 => self.quests.mark_status_if_incomplete(number(&event, "Quest ID") as u16),
                tag => bail!(
                    "Demo story event {tag} ({}) needs a gameplay handler",
                    event.name()
                ),
            }
            Ok(false)
        })();
        match result {
            Ok(false) => self.demo_story.active = Some(p),
            Ok(true) => {
                self.release_demo_story_camera();
                self.demo_story.current_request = None;
                if p.restricted {
                    self.cutscene_input.unrestrict();
                }
                self.story_player_status(false)?;
                //Normal manager completion2809b0 sends the matching action22.
                //Cancellation has separate native abort/release semantics.
                self.cutscene_actions.dispatch_story_conversation(p.owner,
                    super::ai_conversation::Notification::Finished { aborted: false },
                    &mut self.entity_event_registry, &mut self.ai_brains)?;
                self.emit_entity_output(p.owner, "OnSequenceFinished", p.owner);
                self.demo_story.completed.push(p.name);
            }
            Err(error) => {
                self.release_demo_story_camera();
                self.demo_story.current_request = None;
                if p.restricted {
                    self.cutscene_input.unrestrict();
                }
                if let Err(cleanup)=self.story_player_status(false) {self.demo_story.errors.push(format!("Story error status cleanup: {cleanup:#}"));}
                return Err(error.context(format!("Sequence {} event {}", p.name, p.cursor)));
            }
        }
        Ok(())
    }
    fn demo_condition(&self, node: &Node, owner: usize) -> Result<bool> {
        let Node::Record { fields, .. } = node else {
            return Ok(true);
        };
        let Some(condition) = fields.iter().find(|n| matches!(n, Node::Record { .. })) else {
            return Ok(true);
        };
        struct ContextOwner<'a>(&'a super::global_state::State);
        impl super::story_sequences::ContextOwner for ContextOwner<'_> {
            fn global(&mut self, name: &str) -> Result<Option<i16>> {
                Ok(self.0.get(name))
            }
            fn mood(&mut self, _: usize, name: &str) -> Result<bool> {
                bail!("Character mood {name} unavailable")
            }
        }
        super::story_sequences::Condition::load(condition)?
            .evaluate(owner, &mut ContextOwner(&self.globals))
    }
}

/// Debug-build integration validation, using the same original-asset scenarios
/// as the regression suite. Scripted stimuli do not validate player navigation.
#[cfg(debug_assertions)]
pub(super) fn validate_progression() -> Result<()> {
    tests::validate_progression()
}

#[cfg(debug_assertions)]
pub(super) fn validate_random_dialogue()->Result<()> {
    super::dialogue_selection::validate_reference()?;
    let root=std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
    let mut w=super::assets::load(std::path::Path::new(&root),"Levels/GV_ZoneA_Start.level",&egui::Context::default())?;
    let resource_id=w.story_sequences.resources.iter().position(|r|r.path.to_ascii_lowercase().ends_with("gv_zonea_waitsforhelp.bsq")).context("Original random dialogue resource missing")?;
    let resource=&w.story_sequences.resources[resource_id];let node=sequence(&resource.bytes,&resource.path)?;
    let original=events(resource_id,&[],list(&node,"Events")).into_iter().find(|e|number(e,"_type")==1).context("Original random event missing")?;
    let count=list(&original,"Lines").len();let mode=number(&original,"Selection mode") as u16;
    let mut expected=super::dialogue_selection::State::new(count);let initial_draws=w.game_random.draws;
    w.demo_story.enabled=true;w.cutscene_input=Default::default();
    for _ in 0..count {
        w.demo_story.dialogue=None;w.demo_story.current_request=None;
        w.demo_story.active=Some(Playback {owner:w.player_entity,name:"original-random-dialogue-fixture".into(),events:vec![original.clone()],cursor:0,delay:0.,restricted:false});
        let mut rng=w.game_random;let selected=expected.select(mode,||rng.draw())?;
        w.step_demo_story(super::STEP)?;
        let line=&list(&original,"Lines")[selected];
        anyhow::ensure!(w.demo_story.dialogue.as_ref().context("Dialogue was not presented")?.text==w.demo_story.localized(text(line,"Text key")),"Native-selected line did not reach presentation");
        anyhow::ensure!(w.game_random==rng,"Dialogue consumed wrong random draws");
    }
    anyhow::ensure!(w.demo_story.random_dialogue.len()==1 && w.demo_story.random_dialogue_picks==count as u64,"Event history was reset or duplicated between plays");
    anyhow::ensure!(w.game_random.draws-initial_draws==count as u64,"C rand draw count changed");
    println!("PASS: original GV random dialogue selects/presents all{count} lines across repeated plays with one persistent event history and retail RNG consumption");
    Ok(())
}

#[cfg(debug_assertions)]
pub(super) fn validate_story_teardown()->Result<()> {
    let root=std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
    let mut w=super::assets::load(std::path::Path::new(&root),"Levels/GV_ZoneA_Start.level",&egui::Context::default())?;
    w.enable_demo_story()?;w.story_sequences.queue.clear();w.cutscene_input=Default::default();
    let owner=w.scripts.resolve_name("gremlinvillage_zonea.Gremlin_Variant4 01").context("Original speaker missing")?;
    let hidden=w.hidden_entities.contains(&w.player_entity);
    w.force_story_sequence(owner,"GV_ZoneA_WaitsForHelp")?;
    w.step_demo_story(super::STEP)?;w.step_demo_story(super::STEP)?;
    anyhow::ensure!(super::entity_status::read(&w.entities[w.player_entity].components)?==Some(true),"Original story activation did not set gameplay Invisible");
    for _ in 0..120 {
        if w.demo_story.dialogue.is_some(){w.acknowledge_demo_story(0);}
        w.step_demo_story(super::STEP)?;
        if w.demo_story.active.is_none(){break;}
    }
    anyhow::ensure!(w.demo_story.active.is_none() && super::entity_status::read(&w.entities[w.player_entity].components)?==Some(false),"Original story completion did not clear gameplay Invisible");
    anyhow::ensure!(w.hidden_entities.contains(&w.player_entity)==hidden,"Gameplay Invisible incorrectly changed render visibility");
    let scene=w.entities[owner].scene.clone();let completed=w.demo_story.completed.len();
    w.cutscene_input.restrict(); // unrelated scripted restriction must survive.
    anyhow::ensure!(matches!(w.force_story_sequence(owner,"GV_ZoneA_WaitsForHelp")?,super::story_sequences::Admission::Queued),"Original conversation could not queue");
    w.step_demo_story(super::STEP)?;w.step_demo_story(super::STEP)?;
    anyhow::ensure!(w.demo_story.dialogue.is_some() && w.cutscene_input.count==3,"Original dialogue/sequence holds not established");
    w.deactivate_streamed_scene(&scene)?;
    anyhow::ensure!(super::entity_status::read(&w.entities[w.player_entity].components)?==Some(false),"Stream cancellation left player Invisible");
    anyhow::ensure!(w.demo_story.active.is_none() && w.demo_story.dialogue.is_none() && w.demo_story.current_request.is_none() && w.cutscene_input.count==1 && w.cutscene_input.restricted,"Scene deactivation leaked conversation state or released an unrelated input owner");
    w.force_story_sequence(owner,"GV_ZoneA_WaitsForHelp")?;
    w.step_demo_story(super::STEP)?;
    anyhow::ensure!(w.story_sequences.queue.len()==1 && w.demo_story.active.is_none(),"Inactive owner consumed its queued conversation");
    w.activate_streamed_scene(&scene)?;
    w.step_demo_story(super::STEP)?;w.step_demo_story(super::STEP)?;
    anyhow::ensure!(w.demo_story.dialogue.is_some() && w.cutscene_input.count==3,"Reactivated owner did not present its retained request");
    w.retire_entity(owner)?;
    anyhow::ensure!(super::entity_status::read(&w.entities[w.player_entity].components)?==Some(false),"Owner retirement left player Invisible");
    anyhow::ensure!(w.demo_story.active.is_none() && w.demo_story.dialogue.is_none() && w.cutscene_input.count==1 && w.cutscene_input.restricted && w.demo_story.completed.len()==completed && w.demo_story.cancelled==2,"Retirement leaked presentation state or falsely completed the sequence");
    w.cutscene_input.unrestrict();anyhow::ensure!(!w.cutscene_input.restricted,"Unrelated restriction could not release normally");
    println!("PASS: original GV BSQ cancellation on stream deactivation/actor retirement, retained inactive requests and independent nested input ownership");Ok(())
}

#[cfg(debug_assertions)]
pub(super) fn validate_brush_puzzle()->Result<()> { tests::validate_brush_puzzle() }


#[cfg(debug_assertions)]
pub(super) fn validate_gv_encounter()->Result<()> {tests::validate_gv_encounter()}
