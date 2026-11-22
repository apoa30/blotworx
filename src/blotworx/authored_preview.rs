//! Provisional playable controller/animation integration. Uses authored graph
//! selection, retained clip clocks and transition output. Full graph scheduling,
//! synchronized timesteps and controller notification parity remain unfinished.
use super::assets::World;
use anyhow::{ensure, Context, Result};

pub struct Preview {
    pub playback: super::havok_playback::Playback,
    controller_animation: super::controller_animation::Bindings,
    pub diagnostic_brush_direction: bool,
    pub ground_history: super::ground_history::History,
    airborne_animation: bool,
    respawns: u32,
    pub spin_lifecycle: super::spin::Lifecycle,
    spin_config: Option<(f32, f32, f32)>,
    spin_stop_id: i32,
    spin_requested: bool,
    ground_reset_pending: bool,
    pub spin_hits: std::collections::HashSet<usize>,
    /// Queries awaiting the world's fixed-tick combat dispatcher.
    pub pending_combat_queries: u64,
    brush_animation: super::brush::AnimationBindings,
    brush_jump_active: bool,
    pub idle: super::mickey_idle::State,
    idle_settings: Option<super::mickey_idle::Settings>,
}
impl Preview {
    /// Caller owns the action latch and verified motion-history/timestep inputs.
    /// Its callback runs synchronously after the latch change, like retail.
    pub fn brush_shuffle(
        &mut self,
        latch: &mut super::brush::AimShuffle,
        yaw: f32,
        history: [f32; 3],
        rates: [f32; 2],
        dt: f32,
        graph: &super::behavior::Document,
        prepared: &mut super::generator_plan::Prepared,
        resources: &mut super::animation_resources::Resources,
    ) -> Result<()> {
        let bindings = self.brush_animation.shuffle;
        latch.update(yaw, history, rates[0], rates[1], dt, |event| {
            let index = match event {
                super::brush::ShuffleEvent::Right => 0,
                super::brush::ShuffleEvent::Left => 1,
                super::brush::ShuffleEvent::Stop => 2,
            };
            self.deliver_event(
                graph,
                prepared,
                resources,
                super::state_machine::Event {
                    id: bindings[index],
                    payload: None,
                },
            )
        })
    }
    pub fn brush_animation_inputs(&self, state: &super::behavior_state::State) -> (f32, f32) {
        (
            f32::from_bits(state.words()[self.brush_animation.elevation]),
            f32::from_bits(state.words()[self.brush_animation.yaw]),
        )
    }
    pub fn brush_aim(
        &self,
        state: &mut super::behavior_state::State,
        elevation_dot: f32,
        yaw_radians: f32,
    ) -> Result<()> {
        self.brush_animation
            .apply_aim(state, elevation_dot, yaw_radians)
    }
    /// Animation portion of accepted brush start/stop. Dispatch is synchronous:
    /// the native action writes VAR_StrafingState only after SendEvent returns.
    /// This does not perform input eligibility, aiming, FX or projectile work.
    pub fn brush_transition(
        &mut self,
        active: bool,
        graph: &super::behavior::Document,
        prepared: &mut super::generator_plan::Prepared,
        resources: &mut super::animation_resources::Resources,
    ) -> Result<()> {
        let binding = self.brush_animation;
        self.deliver_event(
            graph,
            prepared,
            resources,
            super::state_machine::Event {
                id: if active { binding.start } else { binding.stop },
                payload: None,
            },
        )?;
        prepared.state.set_integer(binding.state, i32::from(active))
    }
    fn deliver_event(
        &mut self,
        graph: &super::behavior::Document,
        prepared: &mut super::generator_plan::Prepared,
        resources: &mut super::animation_resources::Resources,
        event: super::state_machine::Event,
    ) -> Result<()> {
        self.playback
            .deliver_event(graph, prepared, resources, event)
    }
    pub fn queue_named_event(
        &mut self,
        graph: &super::behavior::Document,
        name: &str,
    ) -> Result<()> {
        self.playback.queue_named_event(graph, name)
    }
    pub(super) fn brush_jump_transition(
        &mut self, active: bool, graph: &super::behavior::Document,
    ) -> Result<()> {
        if active == self.brush_jump_active {
            return Ok(());
        }
        self.brush_jump_active = active;
        let name = if active { "EVENT_Start_ThinnerJump" } else { "EVENT_Stop_ThinnerJump" };
        if graph.single("hkbBehaviorGraphStringData")?["eventNames"].as_array()
            .is_some_and(|names| names.iter().any(|candidate| candidate.as_str() == Some(name)))
        {
            self.queue_named_event(graph, name)?;
        }
        Ok(())
    }
    pub fn new(world: &mut World) -> Result<Self> {
        let mut preview = Self::initialize(
            &world.player_behavior,
            &mut world.player_graph,
            &mut world.player_animations,
        )?;
        let settings = world
            .report
            .spin_settings
            .as_ref()
            .context("Missing spin configuration")?;
        preview.spin_config = Some((
            settings.initial_preview_wait(),
            settings.stimulus_period,
            settings.gravity,
        ));
        let idle_settings = super::mickey_idle::Settings::load(&world.runtime_settings)?;
        preview.idle.reset(&idle_settings, &mut world.game_random);
        preview.idle_settings = Some(idle_settings);
        Ok(preview)
    }
    pub fn request_spin(&mut self) {
        self.spin_requested = true;
    }
    fn initialize(
        graph: &super::behavior::Document,
        prepared: &mut super::generator_plan::Prepared,
        resources: &mut super::animation_resources::Resources,
    ) -> Result<Self> {
        let playback = super::havok_playback::Playback::initialize(graph, prepared, resources)?;
        Self::from_playback(playback,graph)
    }
    pub(super) fn from_playback(playback:super::havok_playback::Playback,graph:&super::behavior::Document)->Result<Self> {
        let spin_stop_id = graph.single("hkbBehaviorGraphStringData")?["eventNames"]
            .as_array()
            .context("Missing event names")?
            .iter()
            .position(|v| v.as_str() == Some("EVENT_Stop_SpinAttack"))
            .context("Missing spin stop event")? as i32;
        Ok(Self {
            playback,
            controller_animation: super::controller_animation::Bindings::load(graph)?,
            diagnostic_brush_direction: false,
            brush_animation: super::brush::AnimationBindings::load(graph)?,
            brush_jump_active: false,
            ground_history: Default::default(),
            spin_lifecycle: Default::default(),
            spin_config: None,
            spin_stop_id,
            spin_requested: false,
            ground_reset_pending: false,
            pending_combat_queries: 0,
            spin_hits: Default::default(),
            airborne_animation: false,
            respawns: 0,
            idle: Default::default(),
            idle_settings: None,
        })
    }
    /// Shared motion-only phase for explicit playtests without action/idle
    /// settings. Keeps the existing graph, clips, history and transition path.
    pub(super) fn playtest_motion_tick(&mut self,player:&super::physics::Player,graph:&super::behavior::Document,prepared:&mut super::generator_plan::Prepared,resources:&mut super::animation_resources::Resources)->Result<()> {
        self.record_ground_motion(player)?;
        self.controller_animation.apply(player,&mut prepared.state)?;
        self.apply_controller_motion(player,graph,&mut prepared.state)?;
        self.advance_character(graph,prepared,resources,1)
    }
    pub fn advance(&mut self, world: &mut World, ticks: u64) -> Result<()> {
        self.advance_character(
            &world.player_behavior,
            &mut world.player_graph,
            &mut world.player_animations,
            ticks,
        )?;
        world.export_player_animation_frames(std::mem::take(&mut self.playback.emitted_frames))?;
        world.update_skeletal_children(Some(&self.playback.pose))
    }
    /// Feed the ground-state outputs before they are changed by collision.
    /// This records the prototype's ground update; scripted state routing and
    /// moving-surface compatibility still require the original state lifecycle.
    pub fn record_ground_motion(&mut self, player: &super::physics::Player) -> Result<()> {
        if let Some(vectors) = player.ground_vectors {
            if self.ground_reset_pending {
                // StopSpin sets motion+0x211. The next ground update calls
                // REL 0x307250, which records zero vectors via 0x308254.
                self.ground_history.push([0.; 3], [0.; 3], super::STEP)?;
            } else {
                self.ground_history
                    .push(vectors.movement, vectors.desired, super::STEP)?;
            }
        } else {
            self.ground_history.clear();
        }
        Ok(())
    }
    /// One provisional controller tick, shared by interactive play and probes.
    /// R/fall recovery recreates the preview character, not the retail death or
    /// checkpoint lifecycle. Keep decoded immutable animation resources cached.
    pub fn synchronize_player(
        &mut self,
        player: &super::physics::Player,
        graph: &super::behavior::Document,
        prepared: &mut super::generator_plan::Prepared,
        resources: &mut super::animation_resources::Resources,
    ) -> Result<bool> {
        if self.respawns != player.respawns {
            let mut replacement = super::generator_plan::prepare(
                graph,
                super::behavior_state::State::new(graph)?,
                0,
            )?;
            let mut preview = Self::initialize(graph, &mut replacement, resources)?;
            preview.respawns = player.respawns;
            preview.spin_config = self.spin_config;
            preview.idle_settings = self.idle_settings;
            preview.diagnostic_brush_direction = self.diagnostic_brush_direction;
            // Commit only after startup activation and output succeeded.
            *prepared = replacement;
            *self = preview;
            return Ok(true);
        }
        Ok(false)
    }
    pub fn controller_tick(
        &mut self,
        player: &mut super::physics::Player,
        graph: &super::behavior::Document,
        prepared: &mut super::generator_plan::Prepared,
        resources: &mut super::animation_resources::Resources,
        random: &mut super::game_random::State,
        combat_intensity: Option<f32>,
        mut ordinary_feedback: impl FnMut(&str) -> Result<()>,
    ) -> Result<()> {
        if self.synchronize_player(player, graph, prepared, resources)? {
            if let Some(settings) = self.idle_settings {
                self.idle.reset(&settings, random);
            }
        }
        self.record_ground_motion(player)?;
        self.controller_animation
            .apply(player, &mut prepared.state)?;
        if self.diagnostic_brush_direction {
            // The recovered angle kernel is native Rust. This diagnostic
            // scheduling still needs original action-scheduler validation.
            if let Some(yaw) =
                super::brush::moving_aim_yaw(self.ground_history.latest(), player.yaw, |x, z| {
                    Ok(super::native_angle::atan2(x, z))
                })?
            {
                prepared
                    .state
                    .set_real(self.brush_animation.yaw, 57.2957763671875_f32 * yaw)?;
            }
        }

        self.apply_controller_motion(player, graph, &mut prepared.state)?;
        if let Some((_, period, _)) = self.spin_config {
            if self.spin_lifecycle.update(super::STEP, period) {
                self.pending_combat_queries += 1;
            }
        }
        if std::mem::take(&mut self.spin_requested) {
            ensure!(
                self.spin_config.is_some(),
                "Spin preview has no imported settings"
            );
            if let Some(event) = self.spin_lifecycle.start(player.motion_state(), true) {
                self.spin_hits.clear();
                player.begin_spin_motion(self.spin_config.unwrap().2);
                self.queue_named_event(graph, event)?;
                ordinary_feedback(event)?;
            }
        }
        if let Some(settings) = self.idle_settings {
            let playback = &mut self.playback;
            self.idle.advance(
                player.motion_state(),
                self.ground_history
                    .latest()
                    .map_or(0., |sample| sample.speed),
                player.reverse_motion_enabled(),
                super::STEP,
                combat_intensity,
                &settings,
                random,
                |name| {
                    let id = graph.single("hkbBehaviorGraphStringData")?["eventNames"]
                        .as_array()
                        .context("Missing idle event names")?
                        .iter()
                        .position(|value| value.as_str() == Some(name))
                        .with_context(|| format!("Missing native Mickey event {name}"))?
                        as i32;
                    //2f3400 dispatches synchronously, before ordinary kind9 feedback.
                    let event = super::state_machine::Event { id, payload: None };
                    let emitted = super::havok_playback::Playback::dispatch_event(
                        graph, prepared, resources, &event,
                    )?;
                    playback.record_external_delivery(event, emitted);
                    ordinary_feedback(name)
                },
            )?;
        }
        self.advance_character(graph, prepared, resources, 1)?;
        for event in &self.playback.delivered_events {
            if event.id == self.spin_stop_id {
                if let Some((wait, _, _)) = self.spin_config {
                    self.spin_lifecycle.animation_stopped(wait);
                }
                player.end_spin_motion();
                self.ground_reset_pending = true;
            }
        }
        Ok(())
    }
    /// Provisional controller-to-graph phase routing. The events select authored
    /// branches; native notification timing and landing-type selection remain
    /// to be compared with the original controller.
    pub fn apply_controller_motion(
        &mut self,
        player: &super::physics::Player,
        graph: &super::behavior::Document,
        state: &mut super::behavior_state::State,
    ) -> Result<()> {
        if player.native_knockback {
            self.airborne_animation = false;
            return Ok(());
        }
        if player.damage_leaping || player.damage_leap_landed {
            self.airborne_animation = false;
            if player.damage_leap_landed {
                self.queue_named_event(graph, "EVENT_Damaged_Leap_Land")?;
            }
            return Ok(());
        }
        if let Some(ledge_event) = player.ledge.event {
            match ledge_event {
                super::ledge_grab::Event::StartTrying => {
                    self.queue_named_event(graph, "EVENT_Start_Trying_Ledge_Grab")?;
                    self.airborne_animation = true;
                }
                super::ledge_grab::Event::Reached => {
                    self.queue_named_event(graph, "EVENT_Start_Ledge_Grab")?;
                    self.queue_named_event(graph, "EVENT_Done_Ledge_Grab_Reach")?;
                    self.airborne_animation = true;
                }
                super::ledge_grab::Event::StartMantle => {
                    self.queue_named_event(graph, "EVENT_Start_Ledge_Jump")?;
                    self.airborne_animation = true;
                }
                super::ledge_grab::Event::MantleFinished => {
                    self.queue_named_event(graph, "EVENT_Stop_Ledge_Jump")?;
                    self.queue_named_event(graph, "EVENT_Start_Land")?;
                    self.airborne_animation = false;
                    return Ok(());
                }
                super::ledge_grab::Event::StartLedgeJump => {
                    self.queue_named_event(graph, "EVENT_Stop_Ledge_Jump")?;
                    self.queue_named_event(graph, "EVENT_Start_Jump")?;
                    self.airborne_animation = true;
                    return Ok(());
                }
                super::ledge_grab::Event::Drop => {
                    self.queue_named_event(graph, "EVENT_Stop_Ledge_Jump")?;
                    self.queue_named_event(graph, "EVENT_Start_InAir")?;
                    self.airborne_animation = true;
                    return Ok(());
                }
            }
        }
        if player.ledge.is_active() {
            return Ok(());
        }
        if player.slide_started {
            self.queue_named_event(graph, "EVENT_Start_Slide_Transition")?;
            self.queue_named_event(graph, "EVENT_Start_Slide")?;
        } else if player.slide_stopped {
            self.queue_named_event(graph, "EVENT_Stop_Slide")?;
        }
        self.apply_ground_motion(player, state)?;
        if player.ground_jump_started || player.air_first_jump_started {
            self.queue_named_event(graph, "EVENT_Start_Jump")?;
            self.airborne_animation = true;
        } else if player.second_jump_started {
            self.queue_named_event(graph, "EVENT_Start_Second_Jump")?;
            self.airborne_animation = true;
        } else if player.motion_state() == 2 && player.ground_vectors.is_some() {
            // Ground vectors identify a tick which began on support. Use the
            // authored external airborne event, not the graph-internal Fall
            // event.
            self.queue_named_event(graph, "EVENT_Start_InAir")?;
            self.airborne_animation = true;
        } else if self.airborne_animation && player.grounded {
            // REL 0x307c6c..0x307de8: select death/hard/stationary/walk/run landing event.
            let severity = player.landed_fall_severity.unwrap_or(0);
            if severity >= 3 {
                if self.queue_named_event(graph, "EVENT_Start_Death_Fall_Rescue").is_err() {
                    let _ = self.queue_named_event(graph, "EVENT_Start_HardLand");
                }
            } else if severity >= 2 {
                self.queue_named_event(graph, "EVENT_Start_HardLand")?;
            } else {
                let horiz = player.velocity[0].hypot(player.velocity[2]);
                let event = if horiz < 0.1 || self.attributes.values.len() < 2 {
                    "EVENT_Start_Land"
                } else {
                    let scale = player.max_move_speed() / self.attributes.values[1].max(1e-4);
                    let walk = self.attributes.values[0] * scale;
                    let run = self.attributes.values[1] * scale;
                    let mid = walk + 0.5 * (run - walk);
                    if horiz < mid {
                        "EVENT_Start_Land_Walk"
                    } else {
                        "EVENT_Start_Land_Run"
                    }
                };
                self.queue_named_event(graph, event)?;
            }
            self.airborne_animation = false;
        }
        Ok(())
    }
    /// Feed the recovered ground-animation bridge. Call once after recording
    /// the controller's pre-collision vectors, before graph event processing.
    pub fn apply_ground_motion(
        &mut self,
        player: &super::physics::Player,
        state: &mut super::behavior_state::State,
    ) -> Result<()> {
        if self.ground_reset_pending && player.ground_vectors.is_some() {
            self.ground_reset_pending = false;
            let output = super::locomotion_animation::update(0., 0., 0., 0., 0., 0, true)?
                .context("Missing locomotion reset output")?;
            if let Some(id) = state.apply_locomotion(&output)? {
                self.pending_events.push_back(super::state_machine::Event {
                    id: id.try_into()?,
                    payload: None,
                });
            }
            return Ok(());
        }
        let Some(current) = self.ground_history.latest() else {
            return Ok(());
        };
        // Airborne ticks clear the ground history. The first supported sample
        // must still produce a locomotion entry event even when momentum has
        // already carried speed above the start threshold. Dropping that sample
        // leaves the landing graph without any subsequent threshold crossing.
        let previous_speed = self.ground_history.previous().map_or(0., |v| v.speed);
        ensure!(
            self.attributes.on_fraction > 0. && self.attributes.values.len() >= 2,
            "Ground animation requires authored walk/run attributes"
        );
        let output = super::locomotion_animation::ground(
            previous_speed,
            current.speed,
            player.max_move_speed(),
            self.attributes.values[1],
            self.attributes.values[0],
        )?;
        if let Some(id) = state.apply_locomotion(&output)? {
            self.pending_events.push_back(super::state_machine::Event {
                id: id.try_into()?,
                payload: None,
            });
        }
        Ok(())
    }
    pub fn advance_character(
        &mut self,
        graph: &super::behavior::Document,
        prepared: &mut super::generator_plan::Prepared,
        resources: &mut super::animation_resources::Resources,
        ticks: u64,
    ) -> Result<()> {
        self.playback
            .advance_character(graph, prepared, resources, ticks)
    }
}

impl std::ops::Deref for Preview {
    type Target = super::havok_playback::Playback;
    fn deref(&self) -> &Self::Target {
        &self.playback
    }
}
impl std::ops::DerefMut for Preview {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.playback
    }
}
