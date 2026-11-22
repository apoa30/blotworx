//! Preview routes use authored knot links, segment times, and curve handles.
use super::assets::{RuntimeEntity, World};
use anyhow::{ensure, Context, Result};
use std::collections::BTreeMap;
#[path = "spline_curve.rs"]
mod spline_curve;
#[derive(Default)]
pub struct State {
    movers: BTreeMap<usize, Mover>,
    // Live speed can be written before a preview route starts, or with
    // preview traversal disabled altogether.
    spline_speed_overrides: BTreeMap<usize, f32>,
}
impl State {
    pub(super) fn retire(&mut self, owner: usize) {
        self.movers.remove(&owner);
        self.spline_speed_overrides.remove(&owner);
    }
}
struct Mover {
    passes: super::spline_events::PassState,
    target: usize,
    speed: f32,
    hold: f32,
    stop: Option<Stop>,
    segment: Option<Segment>,
    reverse: bool,
    disabled: bool,
    stopped: bool,
}

struct Segment {
    start: usize,
    end: usize,
    authored_duration: f32,
    phase: f32,
    path: spline_curve::Path,
}

#[derive(Clone, Copy)]
struct Stop {
    start: usize,
    end: usize,
    position: f32,
}
impl Stop {
    fn destination(self, start: [f32; 3], end: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|i| start[i] + (end[i] - start[i]) * self.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stop_position_retains_fraction_and_does_not_collapse_to_end_knot() {
        let mut stop = Stop {
            start: 0,
            end: 1,
            position: 0.,
        };
        let a = [-2., 1., 4.];
        let b = [6., 5., 0.];
        assert_eq!(stop.destination(a, b), a);
        stop.position = 1.;
        assert_eq!(stop.destination(a, b), b);
        stop.position = 0.25;
        assert_eq!(stop.destination(a, b), [0., 2., 3.]);
        stop.position = 1.5;
        assert_eq!(stop.destination(a, b), [10., 7., -2.]);
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_cog_doors_follow_paint_and_thinner_stop_coordinates() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::super::assets::load(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        world.enable_demo_story()?;
        fn advance(world: &mut World, count: usize) -> Result<()> {
            for _ in 0..count {
                if world.demo_story.dialogue.is_some() {
                    world.acknowledge_demo_story(0);
                }
                world.advance_demo_motion(super::super::STEP)?;
                world.advance_models_with_delta(
                    world.animation_time + super::super::STEP,
                    super::super::STEP,
                )?;
                world.advance_dynamic_physics(super::super::STEP)?;
            }
            Ok(())
        }
        advance(&mut world, 600)?;
        let resolve = |name: &str| {
            world
                .scripts
                .resolve_name(name)
                .with_context(|| format!("Missing {name}"))
        };
        let gear = resolve("gremlinvillage_zonea.GV_ZoneA_Gear03_Toon 02")?;
        let doors = [
            resolve("gremlinvillage_zonea.GV_ZoneA_CogDoor1 01")?,
            resolve("gremlinvillage_zonea.GV_ZoneA_CogDoor2 01")?,
        ];
        let open = [
            resolve("gremlinvillage_zonea.SplineKnot 11")?,
            resolve("gremlinvillage_zonea.SplineKnot 09")?,
        ];
        let closed = [
            resolve("gremlinvillage_zonea.SplineKnot 12")?,
            resolve("gremlinvillage_zonea.SplineKnot 10")?,
        ];
        // CogDoor1 deliberately has no authored hull. Verify the retained body
        // of CogDoor2 rather than inventing collision for the other half.
        let source = world
            .dynamic_bodies
            .iter()
            .position(|b| b.entity == doors[1])
            .context("CogDoor2 body missing")?;
        let shape = world.dynamic_constructions[&source].bodies[0].shapes[0];
        world.demo_counter_stimulus(gear, &[(0, 100.)], false)?;
        advance(&mut world, 120)?;
        let closed_pose = world.collision.simulation_pose(shape)?.position;
        world.demo_counter_stimulus(gear, &[(1, 100.)], false)?;
        advance(&mut world, 120)?;
        assert_eq!(world.globals.get("GV_ZoneA_SecondSpatterRunoff"), Some(2));
        for i in 0..2 {
            for axis in 12..15 {
                assert!(
                    (world.entities[doors[i]].transform[axis]
                        - world.entities[open[i]].transform[axis])
                        .abs()
                        < 0.001,
                    "Door {i} did not reach its open knot"
                );
            }
        }
        let open_pose = world.collision.simulation_pose(shape)?.position;
        for axis in 0..3 {
            let expected = world.entities[open[1]].transform[12 + axis]
                - world.entities[closed[1]].transform[12 + axis];
            assert!(
                (open_pose[axis] - closed_pose[axis] - expected).abs() < 0.005,
                "Door collision did not follow its open motion"
            );
        }
        world.demo_counter_stimulus(gear, &[(0, 100.)], false)?;
        advance(&mut world, 120)?;
        assert_eq!(world.globals.get("GV_ZoneA_SecondSpatterRunoff"), Some(3));
        for i in 0..2 {
            for axis in 12..15 {
                assert!(
                    (world.entities[doors[i]].transform[axis]
                        - world.entities[closed[i]].transform[axis])
                        .abs()
                        < 0.001,
                    "Door {i} did not return to its closed knot"
                );
            }
        }
        let restored = world.collision.simulation_pose(shape)?.position;
        for axis in 0..3 {
            assert!((restored[axis] - closed_pose[axis]).abs() < 0.005);
        }
        Ok(())
    }
}
fn property<'a>(e: &'a RuntimeEntity, class: &str, name: &str) -> Option<&'a serde_json::Value> {
    e.components
        .iter()
        .find(|c| c.class_name == class)?
        .properties
        .iter()
        .find(|p| p.name == name)
        .map(|p| &p.value)
}
impl World {
    fn move_demo_spline_owner(
        &mut self,
        owner: usize,
        position: [f32; 3],
        rotation: [f32; 9],
    ) -> Result<()> {
        let e = &self.entities[owner];
        let dynamic = e
            .components
            .iter()
            .any(|c| c.class_name == "PhysicsRigidBodyDynamicComponent");
        if !dynamic && e.jigsaw_asset.is_none() {
            return self.relocate_entity(owner, position, rotation);
        }
        if e.jigsaw_asset.is_some() {
            let initial = e.initial_physics_transform();
            let identity = super::native_hk_transform::from_entity(
                [1., 0., 0., 0., 1., 0., 0., 0., 1.],
                [0.; 3],
            );
            let inverse = super::native_hk_transform::relative(&initial, &identity);
            let next = super::native_hk_transform::from_entity(rotation, position);
            let delta = super::native_hk_transform::compose(&next, &inverse);
            let quaternion = super::native_hk_transform::rotation_quaternion(&delta)?;
            let translation = delta[12..15].try_into()?;
            let owners = self
                .jigsaw_instances
                .iter()
                .filter(|i| i.entity == owner)
                .flat_map(|i| i.collision.owners.iter().flatten().copied())
                .collect::<std::collections::BTreeSet<_>>();
            for body in owners {
                self.collision
                    .set_owner_transform(body, translation, quaternion)?;
            }
        }
        let e = &mut self.entities[owner];
        e.runtime_rotation = rotation;
        e.transform = super::render_transform::from_entity(rotation, position, e.authored_scale);
        for draw in e.draws.clone() {
            self.draws[draw].trans_mat = e.transform;
        }
        super::rotator_runtime::accept_physics_pose(
            owner,
            &mut self.transform_hierarchy,
            &mut self.entities,
            &mut self.draws,
        )?;
        // Dynamic keyframed bodies consume this owner pose in the existing
        // prepare_scene_motion phase, preserving velocity/support publication.
        Ok(())
    }
    fn demo_link(&self, owner: usize, class: &str, name: &str, id: &str) -> Option<usize> {
        if let Some(name) = property(&self.entities[owner], class, name)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            if let Some(entity) = self.scripts.resolve_name(name) {
                return Some(entity);
            }
        }
        let id = property(&self.entities[owner], class, id)?.as_u64()? as u32;
        (id != 0)
            .then(|| {
                self.entities
                    .iter()
                    .position(|e| e.scene == self.entities[owner].scene && e.id == id)
            })
            .flatten()
    }
    pub(super) fn initialize_demo_motion(&mut self) {
        self.initialize_demo_motion_range(0..self.entities.len());
    }
    pub(super) fn initialize_demo_motion_range(&mut self, range: std::ops::Range<usize>) {
        for owner in range {
            if self.demo_story.motion.movers.contains_key(&owner) {
                continue;
            }
            let Some(speed) = self.demo_story.motion.spline_speed_overrides.get(&owner)
                .copied()
                .or_else(|| property(&self.entities[owner], "SplineFollowerComponent", "InitialSpeed")
                    .and_then(|value| value.as_f64())
                    .map(|value| value as f32))
            else {
                continue;
            };
            let disabled = property(
                &self.entities[owner],
                "SplineFollowerComponent",
                "SFCStartDisabled",
            )
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
            if let Some(target) = self.demo_link(
                owner,
                "SplineFollowerComponent",
                "InitialKnotName",
                "InitialKnotEntity",
            ) {
                self.demo_story.motion.spline_speed_overrides.remove(&owner);
                let hold = property(
                    &self.entities[owner],
                    "SplineFollowerComponent",
                    "InitialHoldTime",
                )
                .and_then(|v| v.as_f64())
                .unwrap_or(0.) as f32;
                let reverse = property(
                    &self.entities[owner],
                    "SplineFollowerComponent",
                    "InitialReversed",
                )
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
                self.demo_story.motion.movers.insert(
                    owner,
                    Mover {
                        passes: Default::default(),
                        target,
                        speed,
                        hold,
                        stop: None,
                        segment: None,
                        reverse,
                        disabled,
                        stopped: false,
                    },
                );
            }
        }
    }
    /// REL .text 26445c calls the same live setter as the reflected
    /// SplineFollowerComponent.InitialSpeed field (3bb204 -> 3ba5d8).
    pub(super) fn spline_initial_speed_lua(
        &mut self,
        lua: &mlua::Lua,
        args: mlua::MultiValue,
    ) -> mlua::Result<mlua::MultiValue> {
        if args.is_empty() {
            return Ok(mlua::MultiValue::new());
        }
        let speed = args
            .get(1)
            .cloned()
            .and_then(|v| lua.coerce_number(v).ok().flatten())
            .unwrap_or(0.) as f32;
        let Some(owner) = self.scripts.resolve_native_entity_argument(lua, args[0].clone())?
            .filter(|owner| !self.removed_entities.contains(owner))
        else {
            return Ok(mlua::MultiValue::new());
        };
        if self.entities[owner].components.iter()
            .any(|component| component.class_name == "SplineFollowerComponent") {
            self.set_live_spline_speed(owner, speed)
                .map_err(mlua::Error::external)?;
        }
        Ok(mlua::MultiValue::new())
    }

    pub(super) fn live_spline_speed(&self, owner: usize) -> Option<f32> {
        self.demo_story.motion.movers.get(&owner).map(|mover| mover.speed)
            .or_else(|| self.demo_story.motion.spline_speed_overrides.get(&owner).copied())
    }

    pub(super) fn set_live_spline_speed(&mut self, owner: usize, speed: f32) -> Result<()> {
        ensure!(speed.is_finite(), "Spline speed must be finite");
        if let Some(mover) = self.demo_story.motion.movers.get_mut(&owner) {
            // 3ba5d8 clears the previous OnPass knot at +0x6c only when
            // the signed product is negative. Zero keeps that latch.
            if mover.speed * speed < 0. {
                mover.passes = Default::default();
            }
            mover.speed = speed;
            mover.stopped = false;
        } else {
            self.demo_story.motion.spline_speed_overrides.insert(owner, speed);
            if self.demo_story.enabled {
                self.initialize_demo_motion_range(owner..owner + 1);
            }
        }
        Ok(())
    }

    pub(super) fn demo_motion_lua(
        &mut self,
        lua: &mlua::Lua,
        name: &str,
        args: mlua::MultiValue,
    ) -> mlua::Result<mlua::MultiValue> {
        if name == "SplineFollower_ClearStopPosition" {
            let owner = self.scripts.resolve_native_entity_argument(
                lua, args.back().cloned().unwrap_or(mlua::Value::Nil),
            )?;
            let follower = owner.filter(|owner| {
                !self.removed_entities.contains(owner) && self.entities[*owner].components
                    .iter().any(|component| component.class_name == "SplineFollowerComponent")
            });
            if let Some(mover) = follower.and_then(|owner| self.demo_story.motion.movers.get_mut(&owner)) {
                // REL3bdbbc clears the stop flags. Speed is set separately.
                mover.stop = None;
            }
            return Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Boolean(follower.is_some())]));
        }
        if name == "SplineFollower_StopAtPosition" && args.len() != 4 {
            return Err(mlua::Error::external(
                "SplineFollower_StopAtPosition requires owner, start knot, end knot and position",
            ));
        }
        if args.is_empty() || args.len() < 2 && name != "Reverse" {
            return Err(mlua::Error::external(format!(
                "{name} requires entity and value"
            )));
        }
        let Some(owner) = self.scripts.resolve_world_argument(&args[0])? else {
            return Ok(if name == "SplineFollower_StopAtPosition" {
                mlua::MultiValue::from_vec(vec![mlua::Value::Boolean(false)])
            } else {
                mlua::MultiValue::new()
            });
        };
        if name == "Reverse" {
            if let Some(m) = self.demo_story.motion.movers.get_mut(&owner) {
                m.reverse = !m.reverse;
                // Retail Reverse (REL 263774..26378c) also clears the
                // previous OnPass knot and the terminal traversal latch.
                m.passes = Default::default();
                m.stopped = false;
            }
            for ((entity, _), rotator) in &mut self.rotators {
                if *entity == owner {
                    rotator.state.reversed = !rotator.state.reversed;
                }
            }
        } else if name == "SplineFollower_SetDisabled" {
            let disabled = super::actor_scripts::native_boolean(lua, args[1].clone())?;
            if let Some(m) = self.demo_story.motion.movers.get_mut(&owner) {
                m.disabled = disabled;
            }
            if let Some(index) = self.entities[owner]
                .components
                .iter()
                .position(|c| c.class_name == "SplineFollowerComponent")
            {
                self.component_control
                    .disabled
                    .entry(owner)
                    .or_default()
                    .insert(index, disabled);
            }
        } else if name == "SplineFollower_TeleportToKnot" {
            let Some(knot) = self.scripts.resolve_world_argument(&args[1])? else {
                return Ok(mlua::MultiValue::new());
            };
            let mut position: [f32; 3] = self.entities[knot].transform[12..15]
                .try_into()
                .map_err(mlua::Error::external)?;
            position[1] += self.pressure_plates.displacement(owner);
            self.move_demo_spline_owner(owner, position, self.entities[owner].runtime_rotation)
                .map_err(mlua::Error::external)?;
            let speed = self.live_spline_speed(owner).unwrap_or_else(|| property(
                &self.entities[owner],
                "SplineFollowerComponent",
                "InitialSpeed",
            )
            .and_then(|v| v.as_f64())
            .unwrap_or(0.) as f32);
            self.demo_story.motion.spline_speed_overrides.remove(&owner);
            let mover = self.demo_story.motion.movers.entry(owner).or_insert(Mover {
                passes: Default::default(),
                target: knot,
                speed,
                hold: 0.,
                stop: None,
                segment: None,
                reverse: false,
                disabled: false,
                stopped: false,
            });
            mover.target = knot;
            mover.hold = 0.;
            mover.stop = None;
            mover.segment = None;
            mover.stopped = false;
        } else if name == "SetSpeed" {
            let speed = lua
                .coerce_number(args[1].clone())?
                .context("Spline speed must be numeric")
                .map_err(mlua::Error::external)? as f32;
            if !speed.is_finite() {
                return Err(mlua::Error::external("Spline speed must be finite"));
            }
            if self.entities[owner].components.iter()
                .any(|c| c.class_name == "RotatorComponent")
            {
                self.set_rotator_speed(owner, speed)
                    .map_err(mlua::Error::external)?;
            } else if !self.demo_story.motion.movers.contains_key(&owner) {
                if let Some(target) = self.demo_link(
                    owner,
                    "SplineFollowerComponent",
                    "InitialKnotName",
                    "InitialKnotEntity",
                ) {
                    self.demo_story.motion.spline_speed_overrides.remove(&owner);
                    self.demo_story.motion.movers.insert(
                        owner,
                        Mover {
                            passes: Default::default(),
                            target,
                            speed,
                            hold: 0.,
                            stop: None,
                            segment: None,
                            reverse: false,
                            disabled: false,
                            stopped: false,
                        },
                    );
                }
            }
            if self.demo_story.motion.movers.contains_key(&owner) {
                self.set_live_spline_speed(owner, speed).map_err(mlua::Error::external)?;
            }
        } else {
            // EM1 REL3be468 takes exactly four arguments. REL3bdadc stores
            // both knot owners and the float position; it is not a boolean
            // or an instruction to always stop at the second knot. GV's open
            // and close callbacks use the same pair with positions 0 and 1.
            let position = lua
                .coerce_number(args[3].clone())?
                .context("Spline stop position must be numeric")
                .map_err(mlua::Error::external)? as f32;
            if !position.is_finite() {
                return Err(mlua::Error::external("Spline stop position must be finite"));
            }
            let start = self.scripts.resolve_world_argument(&args[1])?;
            let end = self.scripts.resolve_world_argument(&args[2])?;
            let accepted = if let (Some(start), Some(end), Some(m)) =
                (start, end, self.demo_story.motion.movers.get_mut(&owner))
            {
                m.stop = Some(Stop {
                    start,
                    end,
                    position,
                });
                m.target = if position == 0. { start } else { end };
                m.segment = None;
                true
            } else {
                false
            };
            return Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Boolean(
                accepted,
            )]));
        }
        Ok(mlua::MultiValue::new())
    }
    fn queue_spline_knot_event(&mut self,owner:usize,knot:usize,kind:u16)->Result<()> {
        let event=super::entity_events::Basic{event_kind:kind,source:Some(knot),header:Some(owner)}.into();
        self.entity_event_queue.enqueue_with(&event,None,&mut |_,_|anyhow::bail!("Spline basic event has no auxiliary reference"))?;
        Ok(())
    }
    pub(super) fn advance_demo_motion(&mut self, delta: f32) -> Result<()> {
        if !self.demo_story.enabled {
            return Ok(());
        }
        ensure!(delta.is_finite() && delta >= 0., "Invalid spline preview timestep");
        let mut movers = std::mem::take(&mut self.demo_story.motion.movers);
        let result = (|| -> Result<()> {
            for (&owner, m) in &mut movers {
                // Scene activation gates component updates, including their
                // hold clocks, knot outputs and branch RNG consumption.
                if !self.entity_scene_active(owner) {continue;}
                let disabled = self.entities[owner]
                    .components
                    .iter()
                    .position(|c| c.class_name == "SplineFollowerComponent")
                    .and_then(|i| {
                        self.component_control
                            .disabled
                            .get(&owner)
                            .and_then(|v| v.get(&i))
                    })
                    .copied()
                    .unwrap_or(m.disabled);
                if disabled
                    || m.stopped
                    || m.speed == 0.
                    || self.removed_entities.contains(&owner)
                    || self.removed_entities.contains(&m.target)
                    || m.stop.is_some_and(|s| {
                        self.removed_entities.contains(&s.start)
                            || self.removed_entities.contains(&s.end)
                    })
                {
                    continue;
                }
                let mut remaining = delta;
                loop {
                    if self.removed_entities.contains(&m.target) {
                        break;
                    }
                    let was_held = m.hold > 0.;
                    let travel_delta = (remaining - m.hold.max(0.)).max(0.);
                    m.hold = (m.hold - remaining).max(0.);
                    if m.hold > 0. {
                        break;
                    }
                    if was_held {
                        // REL 3bc6ac..3bc74c sends from the canonical start
                        // (+0x70), including reverse traversal and zero remainder.
                        let knot = if let Some(segment) = &m.segment {
                            Some(segment.start)
                        } else {
                            m.stop.is_none().then_some(m.target)
                        };
                        if let Some(knot) = knot.filter(|&knot| {
                            !self.removed_entities.contains(&knot)
                                && self.entities[knot].components.iter()
                                    .any(|c| c.class_name == "SplineKnotComponent")
                        }) {
                            self.queue_spline_knot_event(owner, knot, 0x1e)?;
                        }
                    }
                    let source = &self.entities[owner];
                    let plate_displacement = self.pressure_plates.displacement(owner);
                    let mut position: [f32; 3] = source.transform[12..15].try_into()?;
                    position[1] -= plate_displacement;
                    let rotation = source.runtime_rotation;
                    // Explicit stops still use the supplied, unclamped coordinate.
                    let target: [f32; 3] = if let Some(stop) = m.stop {
                        stop.destination(
                            self.entities[stop.start].transform[12..15].try_into()?,
                            self.entities[stop.end].transform[12..15].try_into()?,
                        )
                    } else {
                        self.entities[m.target].transform[12..15].try_into()?
                    };
                    if m.segment.as_ref().is_some_and(|segment| {
                        self.removed_entities.contains(&segment.start)
                            || self.removed_entities.contains(&segment.end)
                    }) {
                        m.segment = None;
                    }
                    let previous_phase = m.segment.as_ref().map(|segment| segment.phase);
                    let (mut destination, arrived, reverse, residual) = if let Some(segment) = &mut m.segment {
                        // The arrival cursor can change at a terminal knot.
                        // Keep both authored knots on the segment so Reverse
                        // can traverse the same geometry from either endpoint.
                        let (start, end) = (segment.start, segment.end);
                        let start_pos = self.entities[start].transform[12..15].try_into()?;
                        let end_pos = self.entities[end].transform[12..15].try_into()?;
                        segment.path.update_endpoints(start_pos, end_pos)?;
                        let previous = segment.phase;
                        let length = segment.path.length();
                        let duration = segment.path.duration(segment.authored_duration, m.speed);
                        ensure!(length.is_finite(), "Nonfinite demo platform route");
                        let step = spline_curve::phase_step(previous, travel_delta, length, duration, m.reverse);
                        segment.phase = step.phase;
                        if step.boundary != 0 {
                            m.target = if step.boundary < 0 { start } else { end };
                        }
                        let destination = if step.phase == 0. {
                            start_pos
                        } else if step.phase == 1. {
                            end_pos
                        } else {
                            let phase = step.phase;
                            let phase = if property(&self.entities[owner], "SplineFollowerComponent", "EaseInOut")
                                .and_then(serde_json::Value::as_bool).unwrap_or(false)
                            {
                                let no_ease = |knot: usize| {
                                    property(&self.entities[knot], "SplineKnotComponent", "Do Not Ease In/Out")
                                        .and_then(serde_json::Value::as_bool).unwrap_or(false)
                                };
                                spline_curve::ease_phase(phase, no_ease(start), no_ease(end))
                            } else {
                                phase
                            };
                            segment.path.sample(phase)
                        };
                        (destination, step.boundary != 0, step.boundary < 0, step.residual)
                    } else {
                        let difference = std::array::from_fn::<_, 3, _>(|i| target[i] - position[i]);
                        let distance = difference.iter().map(|v| v * v).sum::<f32>().sqrt();
                        ensure!(distance.is_finite(), "Nonfinite demo platform route");
                        let step = m.speed.abs() * travel_delta;
                        let fraction = if distance > step { step / distance } else { 1. };
                        let destination = if fraction == 1. {
                            target
                        } else {
                            std::array::from_fn(|i| position[i] + difference[i] * fraction)
                        };
                        (destination, fraction == 1., m.reverse, 0.)
                    };
                    // Retain plate travel when the spline publishes a new base
                    // pose. The plate advances its own depth later this tick.
                    destination[1] += plate_displacement;
                    self.move_demo_spline_owner(owner, destination, rotation)?;
                    if arrived {
                        self.emit_entity_output(owner, "OnReachedPosition", owner);
                        if m.stop.is_none_or(|s| s.position == 0. || s.position == 1.) {
                            if let Some(knot)=m.passes.pass(Some(m.target)) {
                                self.queue_spline_knot_event(owner,knot,0x1c)?;
                            }
                        }
                        if m.stop.is_some() {
                            m.stopped = true;
                            break;
                        }
                        let next = if reverse {
                            (0..self.entities.len()).find(|&i| {
                                self.demo_link(
                                    i,
                                    "SplineKnotComponent",
                                    "NextStopTarget",
                                    "NextStopEntity",
                                ) == Some(m.target)
                            }).map(|target| (target, false))
                        } else {
                            let normal=self.demo_link(m.target,"SplineKnotComponent","NextStopTarget","NextStopEntity");
                            if self.entities[m.target].components.iter().any(|c|c.class_name=="SplineKnotComponent") {
                                let branch=self.demo_link(m.target,"SplineKnotComponent","BranchPathTarget","BranchPathEntity");
                                let chance=property(&self.entities[m.target],"SplineKnotComponent","BranchPathChance").and_then(|v|v.as_f64()).unwrap_or(0.) as f32;
                                // Native consumes rand even with a null branch/zero
                                // chance. Select self means no further segment.
                                let (selected, branched)=super::spline_branch::select(m.target,normal,branch,chance,self.game_random.draw());
                                (selected != m.target).then_some((selected, branched))
                            } else {
                                normal.map(|target| (target, false))
                            }
                        };
                        // Native terminal traversal returns before reading HoldTime
                        // or emitting OnHold. Keep the arrived knot until its hold
                        // event has been admitted, then retain the onward target.
                        if super::spline_events::enter_hold(&mut m.hold, next.is_some(), || {
                            property(&self.entities[m.target], "SplineKnotComponent", "HoldTime")
                                .and_then(|v| v.as_f64()).unwrap_or(0.) as f32
                        }) {
                            self.queue_spline_knot_event(owner,m.target,0x1d)?;
                        }
                        if let Some((next, branched)) = next {
                            // Main and branch durations live on the departure
                            // knot of the authored edge, even in reverse.
                            let (start, end) = if reverse { (next, m.target) } else { (m.target, next) };
                            let key = if branched { "BranchPathTime" } else { "MainPathTime" };
                            let duration = property(&self.entities[start], "SplineKnotComponent", key)
                                .and_then(|value| value.as_f64())
                                .map(|value| value as f32)
                                .filter(|&duration| duration.is_finite())
                                .unwrap_or(0.);
                            let handle = |knot: usize, name: &str| {
                                property(&self.entities[knot], "SplineKnotComponent", name)
                                    .map(|value| crate::render_support::scene::parse_point3(value)
                                        .with_context(|| format!("Invalid spline {name} on {}", self.entities[knot].name)))
                                    .transpose()
                                    .map(|value| value.unwrap_or([0.; 3]))
                            };
                            let path = spline_curve::Path::new(
                                self.entities[start].transform[12..15].try_into()?,
                                handle(start, "OutgoingHandle")?,
                                handle(end, "IncomingHandle")?,
                                self.entities[end].transform[12..15].try_into()?,
                                !branched,
                            )?;
                            m.segment = Some(Segment {
                                start,
                                end,
                                authored_duration: duration,
                                phase: if reverse { 1. } else { 0. },
                                path,
                            });
                            m.target = next;
                        } else {
                            m.stopped = true;
                        }
                    }
                    // Retail 3bc534 keeps stepping only while time remains and
                    // either the residual or the phase has changed.
                    let phase = m.segment.as_ref().map(|segment| segment.phase);
                    if m.stopped || residual <= 0. || (residual == remaining && phase == previous_phase) {
                        break;
                    }
                    remaining = residual;
                }
            }
            Ok(())
        })();
        self.demo_story.motion.movers = movers;
        result
    }
}

#[cfg(debug_assertions)]
pub(super) fn validate_inactive_motion()->Result<()> {
    let root=std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
    let mut world=super::assets::load(std::path::Path::new(&root),"Levels/GV_ZoneA_Start.level",&egui::Context::default())?;
    world.enable_demo_story()?;
    let owner=world.entities.iter().position(|e|e.name=="gv_zonea_airlock.GV_ZoneA_Door02 01").context("Original airlock spline door missing")?;
    let scene=world.entities[owner].scene.clone();
    world.run_callback_fixture("DeactivateScene('Environments/ThePark/FantasyLand/SmallWorld/GSA/gv_zonea_airlock.gsa'); SetSpeed('gv_zonea_airlock.GV_ZoneA_Door02 01',10); SplineFollower_SetDisabled('gv_zonea_airlock.GV_ZoneA_Door02 01',false)")?;
    ensure!(!world.entity_scene_active(owner),"Original door scene did not deactivate");
    let mover=world.demo_story.motion.movers.get_mut(&owner).context("Original door has no motion state")?;
    // Controlled runtime hold isolates the component clock from route geometry.
    mover.hold=0.5;
    let target=mover.target;
    let pose=world.entities[owner].transform;
    let random=world.game_random;
    world.advance_demo_motion(0.25)?;
    world.advance_demo_motion(0.25)?;
    ensure!(world.demo_story.motion.movers[&owner].hold==0.5 && world.demo_story.motion.movers[&owner].target==target && world.entities[owner].transform==pose,"Inactive spline advanced its hold, knot or pose");
    // Other active movers may legitimately draw RNG; don't claim a global
    // stream freeze. This owner has not reached its branch selection.
    let inactive=serde_json::json!({"hold":world.demo_story.motion.movers[&owner].hold,"pose_preserved":world.entities[owner].transform==pose,"target_preserved":world.demo_story.motion.movers[&owner].target==target,"global_draws_before":random.draws,"global_draws_after":world.game_random.draws});
    world.activate_streamed_scene(&scene)?;
    world.advance_demo_motion(0.25)?;
    ensure!(world.demo_story.motion.movers[&owner].hold==0.25,"Reactivated spline did not resume its retained hold");
    std::fs::write("target/inactive-motion-validation.json",serde_json::to_vec_pretty(&serde_json::json!({"owner":owner,"scene":scene,"inactive":inactive,"resumed_hold":world.demo_story.motion.movers[&owner].hold,"scope":"Original door and real scene/speed/disable APIs; controlled runtime hold. Native curved traversal and original-runtime phase equivalence remain unverified."}))?)?;
    println!("PASS: original airlock spline door preserves pose/knot/hold while scene is inactive and resumes its retained hold after activation");
    Ok(())
}

#[cfg(debug_assertions)]
pub(super) fn validate_spline_events()->Result<()> {
    spline_curve::validate()?;
    let root=std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
    let mut world=super::assets::load(std::path::Path::new(&root),"Levels/GV_ZoneA_Start.level",&egui::Context::default())?;
    world.enable_demo_story()?;
    let owner=world.entities.iter().position(|e|e.name=="gv_zonea_airlock.GV_ZoneA_Door02 01").context("Original door missing")?;
    let knot=world.entities.iter().position(|e|e.name=="gv_zonea_airlock.SplinePortalClear02").context("Original cleanup knot missing")?;
    let scene=world.entities[owner].scene.clone();world.activate_streamed_scene(&scene)?;
    world.run_callback_fixture("SplineFollower_SetDisabled('gv_zonea_airlock.GV_ZoneA_Door02 01',false)")?;
    world.move_demo_spline_owner(owner,world.entities[knot].transform[12..15].try_into()?,world.entities[owner].runtime_rotation)?;
    let mover=world.demo_story.motion.movers.get_mut(&owner).context("Door spline missing")?;
    mover.target=knot;mover.speed=10.;mover.hold=0.;mover.stop=None;mover.segment=None;
    let authored = property(&world.entities[owner], "SplineFollowerComponent", "InitialSpeed").cloned();
    mover.passes.pass(Some(knot));
    world.set_live_spline_speed(owner, 0.)?;
    ensure!(world.demo_story.motion.movers.get_mut(&owner).unwrap().passes.pass(Some(knot)).is_none(),
        "A zero speed write cleared the last OnPass knot");
    world.set_live_spline_speed(owner, -10.)?;
    ensure!(world.demo_story.motion.movers.get_mut(&owner).unwrap().passes.pass(Some(knot)).is_none(),
        "Starting from zero cleared the last OnPass knot");
    world.set_live_spline_speed(owner, 10.)?;
    ensure!(property(&world.entities[owner], "SplineFollowerComponent", "InitialSpeed") == authored.as_ref(),
        "A live speed write modified the authored property");
    world.advance_demo_motion(0.)?;
    ensure!(world.entity_event_queue.entries().iter().any(|entry|matches!(&entry.event,super::entity_events::Event::Basic(event) if event.event_kind==0x1c && event.source==Some(knot) && event.header==Some(owner))),"Actual arrival did not enqueue OnPass with moving-door context");
    let audio_before=world.audio_playback.game_post_requests;
    world.process_deferred_entity_events()?;
    ensure!(world.audio_playback.game_post_requests>audio_before,"Original enabled OnPass audio row did not reach game audio ingress");
    for _ in 0..3 {world.advance_models_with_delta(world.animation_time+super::STEP,super::STEP)?;}
    // The original cleanup row has IoFlags3: once plus disabled. It must not
    // execute just because the enabled audio row shares its OnPass event.
    ensure!(!world.removed_entities.contains(&owner),"Original disabled OnPass cleanup incorrectly retired the moving door");
    ensure!(!world.removed_entities.contains(&knot),"OnPass destroyed its knot instead of the activator");
    ensure!(world.deferred_event_runtime.spline_io_deliveries>0,"Spline IO receiver was not used");
    println!("PASS: original door arrival queues native OnPass context, posts its enabled authored audio row and preserves the disabled cleanup row and source knot");
    drop(world);
    let root = std::path::Path::new(&root);
    validate_spline_residual(root)?;
    validate_space_mountain_lift_reverse(root)
}

#[cfg(debug_assertions)]
fn validate_space_mountain_lift_reverse(root: &std::path::Path) -> Result<()> {
    let mut world = super::assets::load(root, "Levels/TL_SpaceMountain.Level", &egui::Context::default())?;
    world.enable_demo_story()?;
    let lift = "sm_tl_zonebvisit1only.TL_SM_Gear1 03";
    let owner = world.scripts.resolve_name(lift).context("Space Mountain lift missing")?;
    let scene = world.entities[owner].scene.clone();
    world.activate_streamed_scene(&scene)?;
    world.demo_story.motion.movers.retain(|&entity, _| entity == owner);
    world.run_callback_fixture(&format!("SplineFollower_SetDisabled('{lift}',false)"))?;
    let position = |w: &World| w.entities[owner].transform[13];
    let initial = position(&world);
    for _ in 0..600 {
        world.advance_demo_motion(super::STEP)?;
    }
    let bottom = position(&world);
    ensure!(bottom < initial - 55., "Space Mountain lift did not descend to the lower knot: {initial} to {bottom}");
    ensure!(world.demo_story.motion.movers[&owner].stopped, "Lift did not stop at the lower terminal knot");
    world.emit_entity_output(owner, "OnStop", owner);
    world.dispatch_actor_inputs(world.animation_time + 10.);
    world.advance_demo_motion(super::STEP)?;
    let rising = position(&world);
    ensure!(rising > bottom && rising < bottom + 1., "Authored OnStop did not move the lift continuously back up: {bottom} to {rising}");
    for _ in 0..600 {
        world.advance_demo_motion(super::STEP)?;
    }
    ensure!((position(&world) - initial).abs() < 0.01, "Lift did not return to the original upper knot");
    println!("PASS: Space Mountain lift descends, stops and returns along the same authored spline after OnStop");
    Ok(())
}

#[cfg(debug_assertions)]
fn validate_spline_residual(root: &std::path::Path) -> Result<()> {
    let mut world = super::assets::load(root, "Levels/GV_ZoneI_Start.level", &egui::Context::default())?;
    world.enable_demo_story()?;
    let find = |name| world.entities.iter().position(|entity| entity.name == name)
        .with_context(|| format!("Missing imported spline entity {name}"));
    let owner = find("gv_zonei_dynamic.GV_ZoneI_Cloud 01")?;
    let departure = find("gv_zonei_dynamic.CloudSK 01")?;
    let arrival = find("gv_zonei_dynamic.CloudSK 02")?;
    let scene = world.entities[owner].scene.clone();
    world.activate_streamed_scene(&scene)?;
    world.run_callback_fixture(r#"
        local cloud = GetEntity("gv_zonei_dynamic.GV_ZoneI_Cloud 01")
        SetSplineFollowerInitialSpeed(cloud, 0.2)
        SplineFollower_TeleportToKnot(cloud, GetEntity("gv_zonei_dynamic.CloudSK 01"))
        SplineFollower_SetDisabled(cloud, false)
    "#)?;
    world.demo_story.motion.movers.retain(|&entity, _| entity == owner);
    world.advance_demo_motion(0.)?;
    world.process_deferred_entity_events()?;
    world.audio_playback.poll()?;

    // Split the first hold to exercise the exact-zero boundary with no
    // time remaining for movement. This tick should post only OnHoldEnd.
    world.advance_demo_motion(0.5)?;
    ensure!(spline_events_for(&world, owner).is_empty(), "Hold ended early");
    world.advance_demo_motion(0.5)?;
    ensure!(spline_events_for(&world, owner) == [(0x1e, departure)]
        && world.demo_story.motion.movers[&owner].segment.as_ref().is_some_and(|s| s.phase == 0.),
        "Hold expiration must enqueue exactly once before movement");
    let audio = &world.audio_playback;
    let audio_before = audio.game_post_requests;
    let submitted_before = audio.submitted;
    world.process_deferred_entity_events()?;
    world.audio_playback.poll()?;
    let audio = &world.audio_playback;
    let cue = "Play_sfx_GV_EBR_Cloud_Move_OnHoldEnd";
    ensure!(audio.game_post_requests == audio_before + 1,
        "Hold end did not post the authored cloud cue exactly once");
    ensure!(!audio.rejected_posts.contains_key(&(owner, cue.into())) && audio.submitted > submitted_before,
        "Cloud departure cue did not submit audio: {:?}", audio.rejected_posts.get(&(owner, cue.into())));
    world.advance_demo_motion(0.)?;
    ensure!(spline_events_for(&world, owner).is_empty(), "Expired hold fired again at zero delta");

    // Eight seconds outbound, four seconds at the next knot, then two
    // seconds along the eight-second return in the same update.
    world.advance_demo_motion(14.)?;
    let mover = &world.demo_story.motion.movers[&owner];
    ensure!(mover.target == departure && mover.hold == 0.
        && mover.segment.as_ref().is_some_and(|segment| segment.phase == 0.25),
        "Spline did not carry two seconds onto the return edge");
    let events = spline_events_for(&world, owner);
    ensure!(events == [(0x1c, arrival), (0x1d, arrival), (0x1e, arrival)],
        "Spline crossing and hold events were out of order: {events:?}");
    let position = &world.entities[owner].transform[12..15];
    ensure!(position.iter().zip([117.32754, 49.47174, -290.47675])
        .all(|(actual, expected)| (actual - expected).abs() < 0.001),
        "Spline return position after carrying time is wrong: {position:?}");
    println!("PASS: imported cloud posted {cue}, carried two residual seconds onto its return edge and queued its knot events in order");
    Ok(())
}

#[cfg(debug_assertions)]
fn spline_events_for(world: &World, owner: usize) -> Vec<(u16, usize)> {
    world.entity_event_queue.entries().iter().filter_map(|entry| match &entry.event {
        super::entity_events::Event::Basic(e)
            if (0x1c..=0x1e).contains(&e.event_kind) && e.header == Some(owner) =>
                e.source.map(|source| (e.event_kind, source)),
        _ => None,
    }).collect()
}
