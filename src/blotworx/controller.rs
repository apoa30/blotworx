//! Playable controller orchestration shared by the viewport and headless checks.
//! The orbit camera is provisional. This does not yet execute the authored
//! animation graph, camera behaviors, ability gates or scripted input ownership.
use super::{
    input,
    physics::{add, mul, Collision, Player, V},
    STEP,
};

#[derive(Debug, PartialEq)]
pub enum AdvanceError<E> {
    Input(String),
    Collision(String),
    Dependent(E),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickPhase {
    BeforeMovement,
    SimulatePhysics,
    AfterMovement,
}
impl<E: std::fmt::Display> std::fmt::Display for AdvanceError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input(error) => write!(f, "Native input: {error}"),
            Self::Collision(error) => write!(f, "Character collision: {error}"),
            Self::Dependent(error) => write!(f, "Dependent gameplay: {error:#}"),
        }
    }
}
impl<E: std::fmt::Debug + std::fmt::Display> std::error::Error for AdvanceError<E> {}

#[derive(Default, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Input {
    /// Native device axes: right, forward. Filtering happens at simulation rate.
    pub movement: [f32; 2],
    pub camera_stick: [f32; 2],
    /// Already converted to radians; accumulated until a simulation tick.
    pub camera_delta: [f32; 2],
    pub jump_pressed: bool,
    pub jump_held: bool,
}

pub struct Controller {
    pub player: Player,
    /// Compatibility physics gate; script input restriction remains separate.
    pub motion_enabled:bool,
    pub camera_yaw: f32,
    pub camera_pitch: f32,
    /// Camera pitch range in radians (positive = above, looking down).
    pub pitch_limits: [f32; 2],
    accumulator: f64,
    jump_pending: bool,
    camera_pending: [f32; 2],
    pub ticks: u64,
    /// EM2 skid / 180 turn (360 MickeyPlayerMotionController), when the player authors it.
    pub moves: Option<super::locomotion_actions::Em2Driver>,
}

impl Controller {
    pub fn new(player: Player) -> Self {
        let moves = player.motion.em2_moves.clone().map(|s| super::locomotion_actions::Em2Driver::new(s, player.motion.turn_min_allowed_speed));
        Self {
            moves,
            player,
            motion_enabled:true,
            camera_yaw: 1.6,
            camera_pitch: 0.28,
            pitch_limits: [-0.15, 1.25],
            accumulator: 0.,
            jump_pending: false,
            camera_pending: [0.; 2],
            ticks: 0,
        }
    }

    pub fn respawn(&mut self) {
        self.player.respawn();
        self.suspend();
    }

    fn suspend(&mut self) {
        self.accumulator = 0.;
        self.jump_pending = false;
        self.camera_pending = [0.; 2];
        self.player.previous = self.player.position;
    }

    /// Host pause/focus suspension freezes simulation and discards pending
    /// actions. This is NOT the game's conversation/cutscene input gate:
    /// those must continue simulation under the original ownership rules.
    pub fn advance(&mut self, world: &Collision, elapsed: f64, active: bool, input: Input) {
        let result = self.advance_with(world, elapsed, active, input, |_, _| {
            Ok::<_, std::convert::Infallible>(())
        });
        if let Err(error) = result {
            panic!("{error}");
        }
    }

    /// Execute dependent character work after EACH committed movement tick,
    /// before the next movement tick. A failed dependent phase stops catch-up;
    /// the already committed tick is not replayed on a subsequent call.
    pub fn advance_with<E>(
        &mut self,
        world: &Collision,
        elapsed: f64,
        active: bool,
        input: Input,
        mut after_tick: impl FnMut(&mut Player, u64) -> Result<(), E>,
    ) -> Result<(), AdvanceError<E>> {
        self.advance_with_camera(world, elapsed, active, input, |player, tick, _, _| {
            after_tick(player, tick)
        })
    }
    pub fn advance_with_camera<E>(
        &mut self,
        world: &Collision,
        elapsed: f64,
        active: bool,
        input: Input,
        mut after_tick: impl FnMut(&mut Player, u64, f32, f32) -> Result<(), E>,
    ) -> Result<(), AdvanceError<E>> {
        self.advance_with_context(
            &mut &*world,
            |world| *world,
            elapsed,
            active,
            input,
            |player, tick, yaw, pitch, _| after_tick(player, tick, yaw, pitch),
        )
    }
    /// Release the collision borrow between ticks so gameplay may change the
    /// world before the next catch-up step, independently of presentation rate.
    pub fn advance_with_context<E, T>(
        &mut self,
        context: &mut T,
        collision: impl Fn(&T) -> &Collision,
        elapsed: f64,
        active: bool,
        input: Input,
        mut after_tick: impl FnMut(&mut Player, u64, f32, f32, &mut T) -> Result<(), E>,
    ) -> Result<(), AdvanceError<E>> {
        self.advance_with_tick_phases(
            context,
            collision,
            elapsed,
            active,
            input,
            |phase, player, tick, yaw, pitch, context| {
                if phase == TickPhase::AfterMovement {
                    after_tick(player, tick, yaw, pitch, context)
                } else {
                    Ok(())
                }
            },
        )
    }
    /// Action changes which affect movement belong to the same fixed tick as
    /// movement and its dependent gameplay, never to a presentation-only frame.
    /// This host orchestration does not establish the original native scheduler.
    pub fn advance_with_tick_phases<E, T>(
        &mut self,
        context: &mut T,
        collision: impl Fn(&T) -> &Collision,
        elapsed: f64,
        active: bool,
        input: Input,
        mut phase: impl FnMut(TickPhase, &mut Player, u64, f32, f32, &mut T) -> Result<(), E>,
    ) -> Result<(), AdvanceError<E>> {
        self.advance_with_tick_phases_and_camera(
            context,
            collision,
            elapsed,
            active,
            input,
            |_| Ok(None),
            |stage, player, tick, yaw, pitch, context| {
                phase(stage, player, tick, yaw, pitch, context)
            },
        )
    }
    /// Query the selected gameplay camera after pre-movement work on every
    /// simulation tick. None retains the explicitly provisional orbit camera.
    pub fn advance_with_tick_phases_and_camera<E, T>(
        &mut self,
        context: &mut T,
        collision: impl Fn(&T) -> &Collision,
        elapsed: f64,
        active: bool,
        input: Input,
        camera_forward: impl Fn(&T) -> Result<Option<V>, E>,
        mut phase: impl FnMut(TickPhase, &mut Player, u64, f32, f32, &mut T) -> Result<(), E>,
    ) -> Result<(), AdvanceError<E>> {
        if !active {
            self.suspend();
            return Ok(());
        }
        if !elapsed.is_finite() || elapsed < 0. {
            return Err(AdvanceError::Input(
                "Elapsed time must be finite and nonnegative".into(),
            ));
        }
        self.jump_pending |= input.jump_pressed;
        for (pending, delta) in self.camera_pending.iter_mut().zip(input.camera_delta) {
            *pending += delta;
        }
        // Retain every elapsed simulation tick. Truncating a rendering stall
        // changes movement, animation and script time relative to normal frames.
        self.accumulator += elapsed;
        let period = 1. / 30.;
        // Only compensates f64 host-clock accumulation at exact tick boundaries;
        // gameplay still receives the measured f32 STEP, unchanged.
        while self.accumulator + 1e-12 >= period {
            phase(
                TickPhase::BeforeMovement,
                &mut self.player,
                self.ticks + 1,
                self.camera_yaw,
                self.camera_pitch,
                context,
            )
            .map_err(AdvanceError::Dependent)?;
            let mut tick_input = if self.player.script_input_restricted {
                self.jump_pending = false;
                self.camera_pending = [0.; 2];
                Input::default()
            } else {
                input
            };
            if self.player.side_scroll_plane.is_some() {
                tick_input.movement[1]=0.;
                tick_input.camera_stick=[0.;2];
                self.camera_pending=[0.;2];
            }
            self.camera_yaw += self.camera_pending[0] - tick_input.camera_stick[0] * STEP * 2.5;
            self.camera_pitch = (self.camera_pitch + self.camera_pending[1]
                - tick_input.camera_stick[1] * STEP * 2.)
                .clamp(self.pitch_limits[0], self.pitch_limits[1]);
            self.camera_pending = [0.; 2];
            let (sin, cos) = self.camera_yaw.sin_cos();
            let (sp, cp) = self.camera_pitch.sin_cos();
            let forward = camera_forward(context)
                .map_err(AdvanceError::Dependent)?
                .unwrap_or([-cos * cp, -sp, -sin * cp]);
            if forward.iter().any(|value| !value.is_finite()) {
                return Err(AdvanceError::Input(
                    "Active camera direction must be finite".into(),
                ));
            }
            let reference = input::camera_movement_reference(forward)
                .map_err(|error| AdvanceError::Input(error.to_string()))?;
            if self.motion_enabled {
            let filtered = self
                .player
                .try_filter_stick(tick_input.movement.map(|v| -v), STEP)
                .map_err(|error| AdvanceError::Input(error.to_string()))?;
            if let Some(moves) = self.moves.as_mut() {
                // Skid / 180 read the raw stick: the filtered stick rotates too slowly (InputRotationVelocity)
                // to ever reach The180Cosine inside The180TimeToCheck.
                let raw = tick_input.movement.map(|v| -v);
                let desired = input::movement_direction(raw, reference);
                moves.tick(&mut self.player, desired, STEP);
            }
            self.player.prepare_input_step(
                collision(context),
                filtered,
                reference,
                (self.jump_pending, tick_input.jump_held),
                STEP,
            );
            }else{self.player.previous=self.player.position;}
            phase(
                TickPhase::SimulatePhysics,
                &mut self.player,
                self.ticks + 1,
                self.camera_yaw,
                self.camera_pitch,
                context,
            )
            .map_err(AdvanceError::Dependent)?;
            if self.motion_enabled {self.player.finish_input_step(collision(context), STEP);}
            if let Some(error) = collision(context).take_character_contact_error() {
                return Err(AdvanceError::Collision(error));
            }
            self.jump_pending = false;
            self.accumulator = (self.accumulator - period).max(0.);
            self.ticks += 1;
            phase(
                TickPhase::AfterMovement,
                &mut self.player,
                self.ticks,
                self.camera_yaw,
                self.camera_pitch,
                context,
            )
            .map_err(AdvanceError::Dependent)?;
        }
        Ok(())
    }

    pub fn presentation_position(&self) -> V {
        let alpha = (self.accumulator * 30.).clamp(0., 1.) as f32;
        add(
            mul(self.player.previous, 1. - alpha),
            mul(self.player.position, alpha),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::physics::{Motion, Triangle};
    use super::*;

    fn ground() -> Collision {
        Collision::new(vec![
            Triangle {
                p: [[-100., 0., -100.], [-100., 0., 100.], [100., 0., 100.]],
            },
            Triangle {
                p: [[-100., 0., -100.], [100., 0., 100.], [100., 0., -100.]],
            },
        ])
        .unwrap()
    }
    fn settled(world: &Collision) -> Controller {
        let mut c = Controller::new(Player::new([0., 1., 0.], Motion::default()));
        for _ in 0..60 {
            c.advance(world, 1. / 30., true, Input::default());
        }
        assert!(c.player.grounded);
        c
    }
    #[test]
    fn side_scroll_input_keeps_jump_and_horizontal_motion_without_depth_or_orbit() {
        // This host-only fixture has no World/Box3D plane constraint. Compare
        // against clean horizontal input to isolate input suppression; native
        // captures separately validate the actual solver's depth constraint.
        let run=|noisy:bool| {
            let world=ground();let mut controller=settled(&world);
            controller.camera_yaw=std::f32::consts::FRAC_PI_2;
            controller.camera_pitch=0.;
            controller.player.side_scroll_plane=Some(([0.;3],[0.,0.,1.]));
            let yaw=controller.camera_yaw;let mut highest=0f32;
            for tick in 0..60 {
                controller.advance(&world,1./30.,true,Input {
                    movement:[0.75,if noisy {1.}else{0.}],
                    camera_stick:if noisy {[1.,1.]}else{[0.;2]},
                    camera_delta:if noisy {[0.2,0.2]}else{[0.;2]},
                    jump_pressed:tick==10,jump_held:(10..20).contains(&tick),
                });
                highest=highest.max(controller.player.position[1]);
            }
            assert!(controller.player.position[0]>1.);
            assert!(highest>0.5);
            assert_eq!(controller.camera_yaw,yaw);
            assert_eq!(controller.camera_pitch,0.);
            (controller.player.position,controller.player.velocity,highest)
        };
        assert_eq!(run(true),run(false));
    }
    #[test]
    fn active_camera_is_sampled_each_tick_after_pre_movement_changes() {
        struct Context {
            collision: Collision,
            forward: V,
            observed: Vec<V>,
        }
        let run = |frames: &[f64]| {
            let mut context = Context {
                collision: ground(),
                forward: [0.; 3],
                observed: vec![],
            };
            let mut controller = settled(&context.collision);
            for &elapsed in frames {
                controller
                    .advance_with_tick_phases_and_camera(
                        &mut context,
                        |context| &context.collision,
                        elapsed,
                        true,
                        Input {
                            movement: [0., 1.],
                            ..Default::default()
                        },
                        |context| Ok::<_, ()>(Some(context.forward)),
                        |phase, player, tick, _, _, context| {
                            if phase == TickPhase::BeforeMovement {
                                context.forward = if tick % 2 == 1 {
                                    [1., 0.5, 0.]
                                } else {
                                    [0., -0.5, 1.]
                                };
                            }
                            if phase == TickPhase::AfterMovement {
                                let desired = player.ground_vectors.unwrap().desired;
                                if tick % 2 == 1 {
                                    assert!(desired[0] > 0.);
                                    assert!(desired[2].abs() < 1e-6);
                                } else {
                                    assert!(desired[2] > 0.);
                                    assert!(desired[0].abs() < 1e-6);
                                }
                                context.observed.push(desired);
                            }
                            Ok(())
                        },
                    )
                    .unwrap();
            }
            (
                context.observed,
                controller.player.position,
                controller.ticks,
            )
        };
        let catchup = run(&[0.2]);
        let steady = run(&[1. / 30.; 6]);
        assert_eq!(catchup, steady);
        assert_eq!(catchup.0.len(), 6);
    }
    #[test]
    fn camera_sampling_failure_does_not_commit_movement_or_fall_back() {
        let mut world = ground();
        let mut controller = settled(&world);
        let start = (controller.ticks, controller.player.position);
        let result = controller.advance_with_tick_phases_and_camera(
            &mut world,
            |world| world,
            1. / 30.,
            true,
            Input {
                movement: [0., 1.],
                ..Default::default()
            },
            |_| Err("Camera pose unavailable"),
            |phase, _, _, _, _, _| {
                assert_eq!(phase, TickPhase::BeforeMovement);
                Ok(())
            },
        );
        assert_eq!(
            result,
            Err(AdvanceError::Dependent("Camera pose unavailable"))
        );
        assert_eq!((controller.ticks, controller.player.position), start);
    }
    #[test]
    fn action_phases_run_only_on_ticks_and_precede_movement() {
        let mut world = ground();
        let mut c = settled(&world);
        let start = c.ticks;
        let mut phases = Vec::new();
        let mut run = |c: &mut Controller, elapsed, phases: &mut Vec<_>| {
            c.advance_with_tick_phases(
                &mut world,
                |w| w,
                elapsed,
                true,
                Input::default(),
                |phase, player, tick, _, _, _| {
                    phases.push((phase, tick));
                    if phase == TickPhase::BeforeMovement {
                        player.begin_brush_motion(2.5, 5.23598).unwrap();
                        // Ground locomotion owns vertical speed unless an
                        // external impulse explicitly takes ownership.
                        player.knockback_velocity_override = true;
                        player.velocity[1] = 12.;
                    } else if phase == TickPhase::AfterMovement {
                        assert!(
                            player.position[1] > 0.1,
                            "Action must affect this tick's movement"
                        );
                    } else {
                        assert_eq!(
                            player.position, player.previous,
                            "Physics boundary must precede translation"
                        );
                        assert!(
                            player.velocity[1] > 0.,
                            "Physics boundary must see the computed command"
                        );
                    }
                    Ok::<_, ()>(())
                },
            )
            .unwrap();
        };
        run(&mut c, 1. / 120., &mut phases);
        assert!(
            phases.is_empty(),
            "Presentation-only frame must not change actions"
        );
        run(&mut c, 1. / 40., &mut phases);
        assert_eq!(
            phases,
            vec![
                (TickPhase::BeforeMovement, start + 1),
                (TickPhase::SimulatePhysics, start + 1),
                (TickPhase::AfterMovement, start + 1)
            ]
        );
        phases.clear();
        run(&mut c, 0.1, &mut phases);
        assert_eq!(
            phases,
            (start + 2..=start + 4)
                .flat_map(|tick| [
                    (TickPhase::BeforeMovement, tick),
                    (TickPhase::SimulatePhysics, tick),
                    (TickPhase::AfterMovement, tick)
                ])
                .collect::<Vec<_>>()
        );
    }
    #[test]
    fn collision_changes_between_ticks_match_separate_frames() {
        let run = |hz| {
            let mut world = ground();
            let mut c = settled(&world);
            let start = c.ticks;
            for _ in 0..hz {
                c.advance_with_context(
                    &mut world,
                    |w| w,
                    1. / hz as f64,
                    true,
                    Input::default(),
                    |_, tick, _, _, world| {
                        if tick == start + 1 {
                            world.set_owner_solid(0, false).unwrap();
                        }
                        Ok::<_, ()>(())
                    },
                )
                .unwrap();
            }
            (
                c.ticks,
                c.player.position,
                c.player.velocity,
                c.player.grounded,
            )
        };
        let baseline = run(30);
        assert!(!baseline.3, "Player must fall when the floor is erased");
        for hz in [10, 60, 144] {
            assert_eq!(run(hz), baseline);
        }
    }
    #[test]
    fn rendering_stall_retains_movement_camera_and_dependent_ticks() {
        let world = ground();
        let mut stalled = settled(&world);
        let mut steady = settled(&world);
        let input = Input {
            movement: [0., 1.],
            camera_stick: [0.4, 0.1],
            ..Default::default()
        };
        let mut stalled_trace = Vec::new();
        stalled
            .advance_with(&world, 0.4, true, input, |player, tick| {
                stalled_trace.push((tick, player.position, player.velocity));
                Ok::<_, ()>(())
            })
            .unwrap();
        let mut steady_trace = Vec::new();
        for _ in 0..12 {
            steady
                .advance_with(&world, 1. / 30., true, input, |player, tick| {
                    steady_trace.push((tick, player.position, player.velocity));
                    Ok::<_, ()>(())
                })
                .unwrap();
        }
        assert_eq!(stalled_trace, steady_trace);
        assert_eq!(stalled.ticks, 72);
        assert_eq!(
            (stalled.camera_yaw, stalled.camera_pitch),
            (steady.camera_yaw, steady.camera_pitch)
        );
        let before = stalled.ticks;
        assert!(stalled
            .advance_with(
                &world,
                f64::NAN,
                true,
                Input::default(),
                |_, _| Ok::<_, ()>(())
            )
            .is_err());
        assert_eq!(stalled.ticks, before);
    }

    #[test]
    fn dependent_updates_observe_each_tick_and_failure_stops_catchup() {
        let world = ground();
        let mut c = settled(&world);
        let mut reference = settled(&world);
        let start = c.ticks;
        let input = Input {
            movement: [0., 1.],
            ..Default::default()
        };
        let mut observed = vec![];
        c.advance_with(&world, 0.1, true, input, |p, tick| {
            observed.push((tick, p.position, p.velocity));
            Ok::<_, ()>(())
        })
        .unwrap();
        for (tick, position, velocity) in observed {
            reference.advance(&world, 1. / 30., true, input);
            assert_eq!(
                (tick, position, velocity),
                (
                    reference.ticks,
                    reference.player.position,
                    reference.player.velocity
                )
            );
        }
        assert_eq!(c.ticks, start + 3);
        let failed = c.advance_with(&world, 0.1, true, input, |_, _| Err("animation failed"));
        assert_eq!(failed, Err(AdvanceError::Dependent("animation failed")));
        assert_eq!(c.ticks, start + 4);
        c.advance_with(&world, 0., true, input, |_, _| Ok::<_, ()>(()))
            .unwrap();
        assert_eq!(c.ticks, start + 6);
        c.advance_with(&world, 0.1, false, input, |_, _| panic!("Paused callback"))
            .unwrap_or_else(|_: AdvanceError<()>| unreachable!());
        assert_eq!(c.ticks, start + 6);
    }

    #[test]
    fn moving_and_rotating_camera_share_the_same_ticks_at_all_render_rates() {
        let world = ground();
        let run = |hz| {
            let mut c = settled(&world);
            for _ in 0..hz * 2 {
                c.advance(
                    &world,
                    1. / hz as f64,
                    true,
                    Input {
                        movement: [0., 1.],
                        camera_stick: [0.7, 0.1],
                        ..Default::default()
                    },
                );
            }
            (
                c.ticks,
                c.player.position,
                c.player.velocity,
                c.player.yaw,
                c.camera_yaw,
                c.camera_pitch,
            )
        };
        let baseline = run(30);
        assert_eq!(baseline.0, 120);
        for hz in [60, 144] {
            assert_eq!(run(hz), baseline);
        }
    }

    #[test]
    fn short_jump_is_retained_until_tick_and_not_repeated_on_catchup() {
        let world = ground();
        let mut c = settled(&world);
        let mut reference = settled(&world);
        c.advance(
            &world,
            1. / 120.,
            true,
            Input {
                jump_pressed: true,
                ..Default::default()
            },
        );
        c.advance(&world, 1. / 120., true, Input::default());
        assert!(c.player.grounded);
        c.advance(&world, 1. / 60., true, Input::default());
        reference.advance(
            &world,
            1. / 30.,
            true,
            Input {
                jump_pressed: true,
                ..Default::default()
            },
        );
        assert!(!c.player.grounded);
        c.advance(&world, 0.1, true, Input::default());
        for _ in 0..3 {
            reference.advance(&world, 1. / 30., true, Input::default());
        }
        assert_eq!(c.player.position, reference.player.position);
        assert_eq!(c.player.velocity, reference.player.velocity);
    }

    #[test]
    fn pause_discards_pending_jump_and_camera_without_simulating_elapsed_time() {
        let world = ground();
        let mut c = settled(&world);
        c.advance(
            &world,
            1. / 120.,
            true,
            Input {
                jump_pressed: true,
                camera_delta: [1., 0.],
                ..Default::default()
            },
        );
        let position = c.player.position;
        let ticks = c.ticks;
        c.advance(&world, 10., false, Input::default());
        assert_eq!(c.player.position, position);
        assert_eq!(c.presentation_position(), position);
        assert_eq!(c.ticks, ticks);
        c.advance(&world, 1. / 30., true, Input::default());
        assert!(c.player.grounded);
        assert_eq!(c.camera_yaw, 1.6);
        assert_eq!(c.ticks, ticks + 1);
    }
}
