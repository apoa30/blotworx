//! Retail MickeyPlayerMotionController ledge detection (REL 0x9a110..0x9b718)
//! and hkpCharacterStateLedgeGrab State 8 (REL 0x220a0c..0x2214b8).
use super::physics::{Collision, V};

/// Authored MickeyPlayerMotionControllerComponent ledge-grab properties.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Settings {
    pub enabled: bool,
    pub height: f32,
    pub translate_speed: f32,
    pub max_grab_distance: f32,
    pub max_save_distance: f32,
    pub max_save_distance_side: f32,
    pub max_velocity_check_backwards: f32,
    pub min_distance_off_ground: f32,
    pub only_in_front: bool,
    pub allow_sidle: bool,
    pub allow_during_double_jump: bool,
    pub min_double_jump_time: f32,
    pub use_button: bool,
    pub fit_shape_height: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            height: 0.2,
            translate_speed: 7.5,
            max_grab_distance: 0.35,
            max_save_distance: 1.0,
            max_save_distance_side: 0.01,
            max_velocity_check_backwards: 4.0,
            min_distance_off_ground: 1.5,
            only_in_front: false,
            allow_sidle: false,
            allow_during_double_jump: false,
            min_double_jump_time: 0.25,
            use_button: false,
            fit_shape_height: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// State 8 phase 1: translating from air catch point to hang point.
    Reaching,
    /// State 8 phase 2: hanging on the ledge awaiting jump, mantle, or drop.
    Hanging,
    /// State 8 phase 3 / State 12: climbing/mantling up onto the ledge top.
    Mantling,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    StartTrying,
    Reached,
    StartMantle,
    MantleFinished,
    StartLedgeJump,
    Drop,
}

#[derive(Clone, Copy, Debug)]
pub struct Active {
    pub phase: Phase,
    /// World-space target position while hanging (wall offset + ledge_y - LedgeGrabHeight).
    pub hang_position: V,
    /// World-space position on top of the ledge after mantling.
    pub top_position: V,
    /// Horizontal outward normal of the grabbed wall (points away from wall toward Mickey).
    pub wall_normal: V,
    /// Facing yaw pointing into the ledge wall (`atan2(-n.x, -n.z)`).
    pub facing_yaw: f32,
    /// Collision owner of the grabbed ledge body.
    pub owner: usize,
    /// Last sampled world position of the grabbed entity (for moving-platform carry).
    pub owner_position: Option<V>,
    /// Remaining timeout for reaching / mantling (REL State 8 +0x98).
    pub timer: f32,
}

#[derive(Clone, Debug, Default)]
pub struct State {
    pub active: Option<Active>,
    /// Brief cooldown after releasing/jumping from a ledge so Mickey does not re-grab immediately.
    pub cooldown: f32,
    /// One-tick animation event produced by the ledge state machine.
    pub event: Option<Event>,
}

/// Result of stepping an active ledge grab for one fixed simulation tick.
#[derive(Clone, Copy, Debug)]
pub enum StepOutcome {
    /// Stay in State 8: write `position`, `velocity`, and `yaw`.
    Hold { position: V, velocity: V, yaw: f32 },
    /// Transition to Jumping (State 1) with upward/forward launch velocity.
    Jump { position: V, velocity: V, yaw: f32 },
    /// Finished mantling onto top of ledge: land at `position`.
    MantleFinished { position: V, yaw: f32 },
    /// Release into InAir (State 2) with small outward push.
    Drop { velocity: V },
}

const CHARACTER_RADIUS: f32 = 0.26;
const CHARACTER_HEIGHT: f32 = 1.05;
const REACH_TIMEOUT: f32 = 0.30;
const MANTLE_DURATION: f32 = 0.22;
const RELEASE_COOLDOWN: f32 = 0.40;

fn normalize_xz(v: V) -> Option<V> {
    let len = v[0].hypot(v[2]);
    (len > 1e-4).then(|| [v[0] / len, 0.0, v[2] / len])
}

impl State {
    pub fn reset(&mut self) {
        self.active = None;
        self.cooldown = 0.0;
        self.event = None;
    }

    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub fn tick_cooldown(&mut self, dt: f32) {
        self.event = None;
        if self.cooldown > 0.0 {
            self.cooldown = (self.cooldown - dt).max(0.0);
        }
    }

    /// REL 0x9a6c4..0x9b718: probe for a grabbable ledge while airborne.
    #[allow(clippy::too_many_arguments)]
    pub fn try_detect(
        &mut self,
        world: &Collision,
        settings: &Settings,
        gate_enabled: bool,
        position: V,
        velocity: V,
        yaw: f32,
        max_slope_cos: f32,
    ) -> bool {
        if !settings.enabled || !gate_enabled || self.active.is_some() || self.cooldown > 0.0 {
            return false;
        }
        // Only grab when past apex / descending.
        if velocity[1] > 0.0 {
            return false;
        }
        // REL 0x9a7d4..0x9a888: reject if ground is within LedgeGrabMinDistanceOffGround.
        if settings.min_distance_off_ground > 0.0 {
            let ground_origin = [position[0], position[1] + 0.1, position[2]];
            if world
                .ray_hit(ground_origin, [0.0, -1.0, 0.0], settings.min_distance_off_ground)
                .is_some()
            {
                return false;
            }
        }
        let forward = [yaw.sin(), 0.0, yaw.cos()];
        let right = [yaw.cos(), 0.0, -yaw.sin()];
        let horiz_speed = velocity[0].hypot(velocity[2]);
        let grab_dist = (CHARACTER_RADIUS + settings.max_grab_distance).min(0.65);
        let side_dist = (CHARACTER_RADIUS + settings.max_grab_distance * 0.75).min(0.55);

        // Probe forward first, then sides/back only within tight arm's reach.
        let mut probes: [Option<(V, f32)>; 4] = [None; 4];
        let mut count = 0;
        probes[count] = Some((forward, grab_dist));
        count += 1;
        if !settings.only_in_front {
            probes[count] = Some((right, side_dist));
            count += 1;
            probes[count] = Some(([-right[0], 0.0, -right[2]], side_dist));
            count += 1;
            if horiz_speed <= settings.max_velocity_check_backwards {
                probes[count] = Some(([-forward[0], 0.0, -forward[2]], side_dist));
                count += 1;
            }
        }

        for probe in probes[..count].iter().flatten() {
            if let Some(found) = Self::probe_direction(
                world,
                settings,
                position,
                probe.0,
                probe.1,
                max_slope_cos,
            ) {
                self.active = Some(found);
                self.event = Some(Event::StartTrying);
                return true;
            }
        }
        false
    }

    fn probe_direction(
        world: &Collision,
        settings: &Settings,
        position: V,
        dir: V,
        max_dist: f32,
        max_slope_cos: f32,
    ) -> Option<Active> {
        // REL 0x9b430..0x9b490: horizontal wall probe at upper-body height.
        let wall_origin = [position[0], position[1] + CHARACTER_HEIGHT * 0.72, position[2]];
        let wall_hit = world.ray_hit(wall_origin, dir, max_dist)?;
        // Must be a reasonably vertical wall face.
        if wall_hit.normal[1].abs() > 0.35 {
            return None;
        }
        let wall_normal = normalize_xz(wall_hit.normal)?;
        // Wall normal must oppose the probe direction.
        if wall_normal[0] * dir[0] + wall_normal[2] * dir[2] > -0.35 {
            return None;
        }

        // REL 0x9b54c..0x9b578: downward ray just past the wall face to find the ledge top.
        let inset = 0.16;
        let top_start_y = position[1] + CHARACTER_HEIGHT + 0.55;
        let top_min_y = position[1] + CHARACTER_HEIGHT * 0.45;
        let top_probe_len = top_start_y - top_min_y;
        let top_origin = [
            wall_hit.position[0] - wall_normal[0] * inset,
            top_start_y,
            wall_hit.position[2] - wall_normal[2] * inset,
        ];
        let top_hit = world.ray_hit(top_origin, [0.0, -1.0, 0.0], top_probe_len)?;
        // Top surface must be walkable and not at the very start of the ray (which means we started inside solid geometry).
        if top_hit.distance <= 0.02 || top_hit.normal[1] < max_slope_cos {
            return None;
        }
        let ledge_y = top_hit.position[1];
        // Ledge top must sit within the grabbable vertical window around Mickey's hands.
        let dy = ledge_y - position[1];
        if !(CHARACTER_HEIGHT * 0.55..=CHARACTER_HEIGHT + 0.45).contains(&dy) {
            return None;
        }

        // REL 0x9b57c..0x9b59c: verify vertical clearance above the ledge top (LedgeGrabFitShapeHeight).
        let clearance_origin = [top_origin[0], ledge_y + 0.08, top_origin[2]];
        if settings.fit_shape_height > 0.1
            && world
                .ray_hit(clearance_origin, [0.0, 1.0, 0.0], settings.fit_shape_height)
                .is_some()
        {
            return None;
        }

        let hang_position = [
            wall_hit.position[0] + wall_normal[0] * (CHARACTER_RADIUS + 0.02),
            ledge_y - CHARACTER_HEIGHT + settings.height * 0.5,
            wall_hit.position[2] + wall_normal[2] * (CHARACTER_RADIUS + 0.02),
        ];
        let top_position = [
            wall_hit.position[0] - wall_normal[0] * (CHARACTER_RADIUS + 0.14),
            ledge_y + 0.03,
            wall_hit.position[2] - wall_normal[2] * (CHARACTER_RADIUS + 0.14),
        ];
        let facing_yaw = (-wall_normal[0]).atan2(-wall_normal[2]);

        Some(Active {
            phase: Phase::Reaching,
            hang_position,
            top_position,
            wall_normal,
            facing_yaw,
            owner: top_hit.owner,
            owner_position: None,
            timer: REACH_TIMEOUT,
        })
    }

    /// Translate the active ledge grab along with a moving ledge entity.
    /// Returns the applied delta `[dx, dy, dz]` when the ledge moved.
    pub fn carry_with_owner(&mut self, current_owner_pos: V) -> Option<V> {
        let active = self.active.as_mut()?;
        let delta = if let Some(prev) = active.owner_position {
            let d = [
                current_owner_pos[0] - prev[0],
                current_owner_pos[1] - prev[1],
                current_owner_pos[2] - prev[2],
            ];
            if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] > 1e-10 {
                for i in 0..3 {
                    active.hang_position[i] += d[i];
                    active.top_position[i] += d[i];
                }
                Some(d)
            } else {
                None
            }
        } else {
            None
        };
        active.owner_position = Some(current_owner_pos);
        delta
    }

    /// REL 0x220b7c..0x221320: advance State 8 (Reaching -> Hanging -> Mantle / Jump / Drop).
    pub fn step(
        &mut self,
        settings: &Settings,
        position: V,
        desired_dir: V,
        jump_pressed: bool,
        jump_speed: f32,
        dt: f32,
    ) -> Option<StepOutcome> {
        let active = self.active.as_mut()?;
        match active.phase {
            Phase::Reaching => {
                let dx = active.hang_position[0] - position[0];
                let dy = active.hang_position[1] - position[1];
                let dz = active.hang_position[2] - position[2];
                let dist = (dx * dx + dy * dy + dz * dz).sqrt();
                let speed = settings.translate_speed.max(1.0);
                active.timer -= dt;
                if dist <= speed * dt || active.timer <= 0.0 {
                    let hang = active.hang_position;
                    let yaw = active.facing_yaw;
                    active.phase = Phase::Hanging;
                    self.event = Some(Event::Reached);
                    Some(StepOutcome::Hold {
                        position: hang,
                        velocity: [0.0; 3],
                        yaw,
                    })
                } else {
                    let inv = speed / dist;
                    let velocity = [dx * inv, dy * inv, dz * inv];
                    let next = [
                        position[0] + velocity[0] * dt,
                        position[1] + velocity[1] * dt,
                        position[2] + velocity[2] * dt,
                    ];
                    Some(StepOutcome::Hold {
                        position: next,
                        velocity,
                        yaw: active.facing_yaw,
                    })
                }
            }
            Phase::Hanging => {
                let hang = active.hang_position;
                let yaw = active.facing_yaw;
                let into_wall = [-active.wall_normal[0], 0.0, -active.wall_normal[2]];

                // REL 0x220fcc..0x221010 + Config.ini LedgeJump_Movement=3.0, LedgeJump_Lift=6.0.
                if jump_pressed {
                    let launch = [
                        into_wall[0] * 3.0,
                        jump_speed.max(6.0),
                        into_wall[2] * 3.0,
                    ];
                    self.active = None;
                    self.cooldown = RELEASE_COOLDOWN;
                    self.event = Some(Event::StartLedgeJump);
                    return Some(StepOutcome::Jump {
                        position: [hang[0], hang[1] + 0.1, hang[2]],
                        velocity: launch,
                        yaw,
                    });
                }

                // REL 0x221034..0x2212a4: stick angle relative to wall facing.
                let stick_len = desired_dir[0].hypot(desired_dir[2]);
                if stick_len > 0.35 {
                    let dot = (desired_dir[0] * into_wall[0] + desired_dir[2] * into_wall[2])
                        / stick_len;
                    if dot > 0.5 {
                        // Pushing toward ledge (< 60 deg): mantle / climb up onto ledge.
                        active.phase = Phase::Mantling;
                        active.timer = MANTLE_DURATION;
                        self.event = Some(Event::StartMantle);
                        return Some(StepOutcome::Hold {
                            position: hang,
                            velocity: [0.0; 3],
                            yaw,
                        });
                    } else if dot < -0.5 {
                        // Pushing away from ledge (> 120 deg): drop off into InAir.
                        let away = [active.wall_normal[0] * 1.2, -0.5, active.wall_normal[2] * 1.2];
                        self.active = None;
                        self.cooldown = RELEASE_COOLDOWN;
                        self.event = Some(Event::Drop);
                        return Some(StepOutcome::Drop { velocity: away });
                    }
                }

                Some(StepOutcome::Hold {
                    position: hang,
                    velocity: [0.0; 3],
                    yaw,
                })
            }
            Phase::Mantling => {
                active.timer -= dt;
                let t = (1.0 - (active.timer / MANTLE_DURATION).clamp(0.0, 1.0)).clamp(0.0, 1.0);
                // Smoothly rise vertically first, then step forward onto the ledge top.
                let y_t = (t * 1.4).min(1.0);
                let xz_t = ((t - 0.3) / 0.7).clamp(0.0, 1.0);
                let pos = [
                    active.hang_position[0]
                        + (active.top_position[0] - active.hang_position[0]) * xz_t,
                    active.hang_position[1]
                        + (active.top_position[1] - active.hang_position[1]) * y_t,
                    active.hang_position[2]
                        + (active.top_position[2] - active.hang_position[2]) * xz_t,
                ];
                let yaw = active.facing_yaw;
                if active.timer <= 0.0 {
                    let top = active.top_position;
                    self.active = None;
                    self.cooldown = 0.25;
                    self.event = Some(Event::MantleFinished);
                    Some(StepOutcome::MantleFinished { position: top, yaw })
                } else {
                    Some(StepOutcome::Hold {
                        position: pos,
                        velocity: [0.0; 3],
                        yaw,
                    })
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blotworx::physics::StaticMesh;

    fn ledge_world(ledge_y: f32) -> Collision {
        // A solid box from z = 0..2, x = -2..2, y = 0..ledge_y (wall face at z = 0, normal -Z; top at y = ledge_y, normal +Y).
        let verts = vec![
            [-2.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, ledge_y, 0.0],
            [-2.0, ledge_y, 0.0],
            [-2.0, ledge_y, 2.0],
            [2.0, ledge_y, 2.0],
        ];
        let indices = vec![
            // Front wall facing -Z
            0, 2, 1, 0, 3, 2,
            // Top surface facing +Y
            3, 4, 5, 3, 5, 2,
        ];
        Collision::new(vec![StaticMesh {
            vertices: verts,
            indices,
            owner: 1,
            solid: true,
        }])
        .unwrap()
    }

    #[test]
    fn detects_reaches_hangs_and_mantles_ledge() {
        let world = ledge_world(3.0);
        let settings = Settings::default();
        let mut state = State::default();
        let pos = [0.0, 2.1, -0.5];
        let max_slope_cos = 39.0f32.to_radians().cos();

        assert!(state.try_detect(&world, &settings, true, pos, [0.0, -1.0, 0.0], 0.0, max_slope_cos));
        assert_eq!(state.event, Some(Event::StartTrying));
        assert_eq!(state.active.unwrap().phase, Phase::Reaching);

        // Advance until Hanging.
        let mut cur = pos;
        for _ in 0..20 {
            state.tick_cooldown(1.0 / 30.0);
            if let Some(StepOutcome::Hold { position, .. }) =
                state.step(&settings, cur, [0.0; 3], false, 9.0, 1.0 / 30.0)
            {
                cur = position;
            }
            if state.active.is_some_and(|a| a.phase == Phase::Hanging) {
                break;
            }
        }
        assert_eq!(state.active.unwrap().phase, Phase::Hanging);

        // Push forward into wall (+Z) -> Mantle.
        state.tick_cooldown(1.0 / 30.0);
        let outcome = state.step(&settings, cur, [0.0, 0.0, 1.0], false, 9.0, 1.0 / 30.0);
        assert!(matches!(outcome, Some(StepOutcome::Hold { .. })));
        assert_eq!(state.event, Some(Event::StartMantle));
        assert_eq!(state.active.unwrap().phase, Phase::Mantling);

        // Complete mantle -> lands on top of ledge (y >= 3.0).
        let mut finished = None;
        for _ in 0..20 {
            state.tick_cooldown(1.0 / 30.0);
            match state.step(&settings, cur, [0.0; 3], false, 9.0, 1.0 / 30.0) {
                Some(StepOutcome::Hold { position, .. }) => cur = position,
                Some(StepOutcome::MantleFinished { position, .. }) => {
                    finished = Some(position);
                    break;
                }
                _ => {}
            }
        }
        let top = finished.expect("Mantle did not finish");
        assert!(top[1] >= 3.0 && top[2] > 0.0, "Unexpected top pos: {top:?}");
        assert!(!state.is_active());
    }

    #[test]
    fn rejects_ledge_when_too_close_to_ground() {
        let mut world = ledge_world(3.0);
        // Add a floor at y = 1.5 (only 0.6 below player at y = 2.1, < 1.5 min_distance_off_ground).
        let _ = world.add_static_mesh(
            &[[-4.0, 1.5, -4.0], [4.0, 1.5, -4.0], [4.0, 1.5, 4.0], [-4.0, 1.5, 4.0]],
            &[0, 2, 1, 0, 3, 2],
            2,
            true,
        );
        let mut state = State::default();
        assert!(!state.try_detect(
            &world,
            &Settings::default(),
            true,
            [0.0, 2.1, -0.5],
            [0.0, -1.0, 0.0],
            0.0,
            39.0f32.to_radians().cos(),
        ));
    }
}
