//! Application target writes and ordinary animation-feedback dispatch.
use super::{
    action_event_dispatch::{EventRegistry, Host},
    assets::World,
    camera_feedback,
    entity_events::{Animation, Event},
};
use anyhow::{ensure, Context, Result};
pub(super) const LISTENER: u32 = 1;
impl World {
    pub(super) fn assign_camera_target(&mut self, target: Option<usize>, all: bool) -> Result<()> {
        self.camera_profiles.assign_target(target, all)?;
        let current = usize::try_from(self.camera_profiles.controller.selection.current)?;
        let target = self
            .camera_profiles
            .extras
            .get(current)
            .context("Camera profile missing")?
            .target;
        let resolved = super::camera_target::resolve(
            target,
            &self.entities,
            &self.removed_entities,
            self.mickey_tracker.as_ref(),
        )?;
        self.camera_target
            .refresh(resolved, &mut self.entity_event_registry, LISTENER)
    }
    pub(super) fn update_camera_tracker_gate(&mut self) {
        // d97bc..e0 is the tracker-dependent prefix of the default generator.
        // A selected authored NIF controller does not run this prefix.
        if self.nif_camera.is_some() {
            return;
        }
        let eligible = usize::try_from(self.camera_profiles.controller.selection.current)
            .ok()
            .and_then(|current| self.camera_profiles.extras.get(current))
            .and_then(|profile| profile.target)
            .and_then(|owner| {
                self.mickey_tracker.as_ref().filter(|tracker| {
                    tracker.owner == owner
                        && !self.removed_entities.contains(&owner)
                        && tracker.error.is_none()
                })
            })
            .map(|tracker| tracker.state.eligible);
        self.camera_feedback.observe_tracker(eligible);
    }
    pub(super) fn dispatch_camera_animation_feedback(&mut self, event: Animation) -> Result<()> {
        struct Receivers<'a> {
            camera:Bindings<'a>, actions:&'a mut super::cutscene_actions::State,
            brains:&'a mut std::collections::BTreeMap<usize,super::ai_behavior_events::Brain>,
        }
        impl Host<Event> for Receivers<'_> {
            fn receive(&mut self,registry:&mut EventRegistry<Event>,listener:u32,event:&Event)->Result<()> {
                if listener==LISTENER {self.camera.receive(registry,listener,event)}
                else {self.actions.receive_deferred(listener,event,self.brains,&mut |_|anyhow::bail!("Animation dispatch has no position resolver"))}
            }
        }
        let bindings = Bindings {
            profiles: &self.camera_profiles,
            tracker: self.mickey_tracker.as_mut(),
            settings: &self.runtime_settings,
            state: &mut self.camera_feedback,
        };
        self.entity_event_registry
            .dispatch(&event.into(), None, &mut Receivers {camera:bindings,actions:&mut self.cutscene_actions,brains:&mut self.ai_brains})
            .map(|_| ())
    }
    /// Accepted Spin follows b7b34/b7b7c ->2f3400: submit the behavior
    /// event, then synchronously emit ordinary feedback with flag0.
    pub(super) fn player_controller_tick(
        &mut self,
        preview: &mut super::authored_preview::Preview,
        player: &mut super::physics::Player,
    ) -> Result<()> {
        for (event, emitted) in std::mem::take(&mut self.player_script_animation_events) {
            preview.playback.record_external_delivery(event, emitted);
        }
        let source = self.player_entity;
        let catalog = self
            .animation_events
            .as_ref()
            .map_err(|error| anyhow::anyhow!(error.clone()))?;
        let mut bindings = Bindings {
            profiles: &self.camera_profiles,
            tracker: self.mickey_tracker.as_mut(),
            settings: &self.runtime_settings,
            state: &mut self.camera_feedback,
        };
        let registry = &mut self.entity_event_registry;
        let mut brain = self.ai_brains.get_mut(&source);
        let limitation="Mickey idle/fidget timers use native stationary-state and action-reset rules. Combat-threat collection remains unimplemented; no zero-threat result is fabricated.";
        if !self
            .report
            .unsupported
            .iter()
            .any(|message| message == limitation)
        {
            self.report.unsupported.push(limitation.into());
        }
        preview.idle.stay_combat = self.player_stay_combat_idle;
        preview.controller_tick(
            player,
            &self.player_behavior,
            &mut self.player_graph,
            &mut self.player_animations,
            &mut self.game_random,
            None,
            |name| {
                let id = catalog
                    .id(name)
                    .context("Controller command has no native event mapping")?;
                ensure!(
                    matches!(id, 0x54 | 0x55 | 101 | 102),
                    "Controller command requires the native ordinary input route"
                );
                registry.dispatch(
                    &Animation {
                        source: Some(source),
                        header: None,
                        id,
                        flag: 0,
                    }
                    .into(),
                    None,
                    &mut bindings,
                )?;
                if let Some(brain) = &mut brain {
                    brain.receive_known(
                        source,
                        super::ai_behavior_events::Event {
                            kind: 9,
                            source: Some(source),
                            action: id,
                        },
                    );
                }
                //31f43c..31f514 has no footstep/noise branch for these commands.
                Ok(())
            },
        )
    }
}

pub(super) struct Bindings<'a> {
    pub(super) profiles: &'a super::camera_catalog::Catalog,
    pub(super) tracker: Option<&'a mut super::entity_tracker::Runtime>,
    pub(super) settings: &'a std::collections::BTreeMap<String, String>,
    pub(super) state: &'a mut camera_feedback::State,
}
impl Host<Event> for Bindings<'_> {
    fn receive(
        &mut self,
        _: &mut EventRegistry<Event>,
        listener: u32,
        event: &Event,
    ) -> Result<()> {
        ensure!(
            listener == LISTENER,
            "Unimplemented native entity-event listener {listener}"
        );
        let Event::Animation(event) = event else {
            anyhow::bail!("Camera receiver requires ordinary kind9 payload")
        };
        struct Effects<'a> {
            settings: &'a std::collections::BTreeMap<String, String>,
            tracker: Option<&'a mut super::entity_tracker::Runtime>,
        }
        impl camera_feedback::Effects for Effects<'_> {
            fn spin_enabled(&mut self) -> Result<bool> {
                camera_feedback::spin_enabled(self.settings)
            }
            fn follow_tracker(&mut self) -> Result<()> {
                if let Some(tracker) = &mut self.tracker {
                    tracker.state.follow_jump = true;
                }
                Ok(())
            }
            fn ledge_transition(&mut self) -> Result<()> {
                if let Some(tracker) = &mut self.tracker {
                    tracker.state.follow_jump = true;
                }
                Ok(())
            }
        }
        let owner = self
            .profiles
            .extras
            .get(usize::try_from(self.profiles.controller.selection.current)?)
            .context("Current camera profile missing")?
            .target;
        let tracker = self
            .tracker
            .as_deref_mut()
            .filter(|tracker| Some(tracker.owner) == owner);
        self.state.receive(
            event.id,
            event.flag,
            &mut Effects {
                settings: self.settings,
                tracker,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_camera_target_routes_actual_air_spin_exports_and_respects_configuration(
    ) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::super::assets::load(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let owner = world
            .mickey_tracker
            .as_ref()
            .context("Tracker missing")?
            .owner;
        let player = world.player_entity;
        assert!(world
            .camera_profiles
            .extras
            .iter()
            .all(|profile| profile.target == Some(owner)));
        assert_eq!(world.camera_target.resolved_target, Some(player));
        assert!(world
            .entity_event_registry
            .contains_for(9, LISTENER, Some(player))?);
        assert!(
            !camera_feedback::spin_enabled(&world.runtime_settings)?,
            "Retail Spin flag was overridden"
        );
        let local = world.player_behavior.single("hkbBehaviorGraphStringData")?["eventNames"]
            .as_array()
            .context("Missing graph event names")?
            .iter()
            .position(|name| name.as_str() == Some("EVENT_Start_SpinAttack_Air"))
            .context("Original graph lacks air spin export")?;
        let export = |world: &mut World| {
            world.export_player_animation_frames(vec![vec![super::super::state_machine::Event {
                id: local as i32,
                payload: None,
            }]])
        };
        world.select_nif_camera(None, None, 1, 0.)?;
        world.mickey_tracker.as_mut().unwrap().state.eligible = false;
        world.update_camera_tracker_gate();
        let count = world.camera_feedback.received;
        export(&mut world)?;
        assert_eq!(world.camera_feedback.received, count + 1);
        assert_eq!(world.camera_feedback.last_id, Some(85));
        assert!(!world.mickey_tracker.as_ref().unwrap().state.follow_jump);
        world
            .runtime_settings
            .insert("FreeCamera_FollowJump_Spin".into(), "true".into());
        export(&mut world)?;
        assert!(world.camera_feedback.spin_follow_active);
        assert!(world.mickey_tracker.as_ref().unwrap().state.follow_jump);
        assert_eq!(world.camera_feedback.follow_requests, 1);
        assert_eq!(world.camera_feedback.last_flag, Some(1));
        world.reset_mickey_tracker()?;
        world.mickey_tracker.as_mut().unwrap().state.eligible = false;
        world.update_camera_tracker_gate();
        let mut preview = super::super::authored_preview::Preview::new(&mut world)?;
        let mut simulated_player =
            super::super::physics::Player::new(world.report.spawn, world.report.motion.clone());
        simulated_player.grounded = false;
        preview.request_spin();
        let injected = world.camera_feedback.injected;
        world.player_controller_tick(&mut preview, &mut simulated_player)?;
        assert_eq!(world.camera_feedback.injected, injected + 1);
        assert_eq!(world.camera_feedback.last_injected_id, Some(85));
        assert_eq!(world.camera_feedback.last_flag, Some(0));
        assert!(
            world.mickey_tracker.as_ref().unwrap().state.follow_jump,
            "Accepted Spin did not reach native camera feedback"
        );
        let other = (0..world.entities.len())
            .find(|&id| id != player && id != owner)
            .context("Other source missing")?;
        let count = world.camera_feedback.received;
        world.dispatch_camera_animation_feedback(Animation {
            source: Some(other),
            header: None,
            id: 85,
            flag: 1,
        })?;
        assert_eq!(
            world.camera_feedback.received, count,
            "Camera received another actor's export"
        );
        let source = world
            .entities
            .iter()
            .position(|entity| {
                entity.name == "gremlinvillage_zonea.seq_projector 01.ExitProjectorCamera"
            })
            .context("Camera source missing")?;
        world.select_nif_camera(Some(source), None, 1, 0.)?;
        world.mickey_tracker.as_mut().unwrap().state.eligible = true;
        world.update_camera_tracker_gate();
        assert!(
            world.camera_feedback.airborne,
            "Inactive default controller advanced its tracker gate"
        );
        world.select_nif_camera(None, None, 1, 0.)?;
        world.update_camera_tracker_gate();
        assert!(!world.camera_feedback.airborne && !world.camera_feedback.spin_follow_active);
        let current = world.camera_profiles.controller.selection.current as usize;
        world.assign_camera_target(Some(other), false)?;
        for (index, profile) in world.camera_profiles.extras.iter().enumerate() {
            assert_eq!(
                profile.target,
                Some(if index == current { other } else { owner })
            );
        }
        assert_eq!(world.camera_target.resolved_target, Some(other));
        let count = world.camera_feedback.received;
        export(&mut world)?;
        assert_eq!(world.camera_feedback.received, count);
        world.assign_camera_target(None, true)?;
        assert!(!world.camera_target.subscribed);
        assert!(world
            .camera_profiles
            .extras
            .iter()
            .all(|profile| profile.target.is_none()));
        world.assign_camera_target(Some(owner), true)?;
        let tracker = world.mickey_tracker.take();
        assert!(world.assign_camera_target(Some(owner), false).is_err());
        assert_eq!(
            world.camera_target.resolved_target,
            Some(player),
            "Failed proxy resolution removed prior subscription"
        );
        world.mickey_tracker = tracker;
        Ok(())
    }
}
