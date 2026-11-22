//! Demo combat/controller adapter. Uses real character bodies, health,
//! paint counters and script events. Collision-aware local steering is a demo
//! adapter; it is not the original navigation database or behavior selection.
use super::assets::World;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
#[derive(serde::Serialize)]
pub struct Decision {
    pause_reason: Option<&'static str>,
    dead: bool,
    hostile: bool,
    in_range: bool,
    movement_requested: bool,
    scripted_patrol: bool,
    target: usize,
    distance: f32,
    source: [f32; 3],
    destination: [f32; 3],
    direction: [f32; 2],
    navigation_floor: Option<(usize,super::ai_runtime_mesh::Floor)>,
    navigation_position: Option<[f32;3]>,
}
#[derive(Default)]
pub struct State {
    patrol: BTreeMap<usize, usize>,
    attack_at: BTreeMap<usize, f32>,
    moving: BTreeMap<usize, bool>,
    pub(super) stunned_until: BTreeMap<usize, f32>,
    steering: BTreeMap<usize, Steering>,
    attacking: BTreeMap<usize, Attack>,
    detonations: BTreeMap<usize, bool>,
    perception_profiles: BTreeMap<String, Perception>,
    last_seen: BTreeMap<usize, ([f32; 3], f32)>,
    sight_checks: BTreeMap<usize, (bool, f32)>,
    home: BTreeMap<usize, ([f32; 3], f32)>,
    pub(super) player_footstep: Option<([f32; 3], f32)>,
    pub attacks: u64,
    pub charge_windups: u64,
    pub charge_rolls: u64,
    pub charge_contacts: u64,
    pub charge_recoveries: u64,
    pub movement_updates: u64,
    pub decisions: BTreeMap<usize, Decision>,
}
impl State {
    pub(super) fn set_patrol_destination(&mut self,owner:usize,target:Option<usize>) {
        if let Some(target)=target {self.patrol.insert(owner,target);}else{self.patrol.remove(&owner);}
    }
}

#[derive(Clone, Copy)]
struct Perception {
    cones: [(f32, f32); 3],
    memory: f32,
    hearing_range: f32,
    hearing_memory: f32,
}

impl Perception {
    fn from_settings(settings: &BTreeMap<String, String>, sensor: &str, kind: &str) -> Self {
        let read = |sensor: &str, property: &str, fallback| {
            let mut scope = kind;
            loop {
                let key = format!("SENS_{sensor}_{scope}_{property}");
                if let Some(value) = settings.get(&key) {
                    if let Ok(value) = super::config_value::decimal_float(value) {
                        if value.is_finite() { return value; }
                    }
                }
                let Some((parent, _)) = scope.rsplit_once(':') else { break; };
                scope = parent;
            }
            settings.get(&format!("SENS_{sensor}_Default_{property}"))
                .and_then(|value| super::config_value::decimal_float(value).ok())
                .filter(|value| value.is_finite()).unwrap_or(fallback)
        };
        let cone = |distance, angle, range, degrees| {
            let angle = read(sensor, angle, degrees).clamp(0., 360.);
            (read(sensor, distance, range), (angle.to_radians() * 0.5).cos())
        };
        Self {
            cones: [
                cone("ViewConeDistance", "ViewConeAngle", 15., 155.),
                cone("ViewConeDistance1", "ViewConeAngle1", 0., 0.),
                cone("ViewConeDistance2", "ViewConeAngle2", 0., 0.),
            ],
            memory: read(sensor, "FactExistsDuration", 4.).max(0.),
            hearing_range: read("HearEnemyFootsteps", "HearFootstepDistance", 5.).max(0.),
            hearing_memory: read("HearEnemyFootsteps", "FactExistsDuration", 4.).max(0.),
        }
    }
}

#[derive(Clone, Copy)]
struct Steering {
    until: f32,
    source: [f32; 3],
    destination: [f32; 3],
    direction: [f32; 2],
}
#[derive(Clone, Copy)]
struct Attack {
    windup_end: Option<f32>,
    windup_idle: f32,
    weapon: usize,
    ranged_target: Option<usize>,
    ranged_aim: Option<[f32; 3]>,
    charge: Option<Charge>,
    phase: AttackPhase,
    contact_open: bool,
    hit: bool,
    uses_slot: bool,
    walk_attack: bool,
    abort_at: f32,
}
#[derive(Clone, Copy)]
struct Charge {
    distance: f32,
    move_speed: f32,
    top_speed: f32,
    ramp_time: f32,
    blocked_time: f32,
    started: f32,
    origin: [f32; 3],
    direction: [f32; 2],
    progress: f32,
    stopped: f32,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum WeaponKind {
    Melee = 0,
    Ranged = 1,
    Charge = 2,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum AttackPhase {
    Windup,
    AwaitStrike,
    Striking,
    Recovery,
}
impl World {
    pub(super) fn uses_demo_combat(&self, actor: usize, kind: &str) -> bool {
        let low = kind.to_ascii_lowercase();
        super::boss_ai::BossKind::of(kind).is_some()
            || low.contains("spatter")
            || low.contains("fodder")
            || low.contains("kamikaze")
            || low.contains("beetleworx")
            || low.contains("theblob")
            || low.contains("slobber")
            || (self.music_team(actor).is_some_and(|(team, _)| team == 2 || team == 3)
                && self.entities[actor].components.iter().any(|c| c.class_name == "ArsenalComponent"))
    }

    fn uses_attack_slot(&self, entity: usize) -> bool {
        let mut name = self.ai_brains[&entity].ai_type.as_str();
        loop {
            let key = format!("AttackConfig_{name}_AttacksAlwaysAllowed");
            if let Some(value) = self.runtime_settings.get(&key) {
                return value.eq_ignore_ascii_case("false") || value == "0";
            }
            let Some((parent, _)) = name.rsplit_once(':') else {
                return false;
            };
            name = parent;
        }
    }

    fn demo_weapon(&self, actor: usize, distance: f32, max_reach: f32, kind: WeaponKind) -> Option<(usize, f32)> {
        let active = self.entities[actor].components.iter()
            .find(|c| c.class_name == "ArsenalComponent")?
            .properties.iter().find(|p| p.name == "ActiveWeapon")?
            .value.as_u64().and_then(|index| usize::try_from(index).ok())?;
        self.live_child_slots.get(&actor)?.iter().flatten().copied()
            .filter(|child| !self.removed_entities.contains(child))
            .filter_map(|child| self.entities[child].components.iter()
                .find(|c| c.class_name == "WeaponComponent").map(|component| (child, component)))
            .enumerate()
            .filter_map(|(index, (weapon, component))| {
                let field = |name| component.properties.iter().find(|p| p.name == name).map(|p| &p.value);
                let ranged = kind == WeaponKind::Ranged;
                let component_name = if ranged { "ShooterComponent" } else { "TriggerDamageComponent" };
                if field("WeaponShots").and_then(|shots| shots.as_i64()) == Some(0)
                    || field("WeaponType")?.as_u64()? != kind as u64
                    || field("WeaponFiringMethod")?.as_u64()? != u64::from(!ranged)
                    || !self.entities[weapon].components.iter().any(|c| c.class_name == component_name)
                {
                    return None;
                }
                let minimum = field("WeaponMinRange")?.as_f64()? as f32;
                let maximum = field("WeaponMaxRange")?.as_f64()? as f32;
                let windup = field("Weapon Windup Idle Time")?.as_f64()? as f32;
                (distance >= minimum && distance <= maximum.min(max_reach))
                    .then_some((index != active, weapon, windup))
            })
            .min_by_key(|&(alternate, _, _)| alternate)
            .map(|(_, weapon, windup)| (weapon, windup))
    }
    pub(super) fn demo_melee_trigger_enabled(&self, weapon: usize) -> Option<bool> {
        let parent = self.transform_hierarchy.get(&weapon)?.parent;
        if let Some(attack) = self.demo_story.ai.attacking.get(&parent) {
            if attack.weapon == weapon {
                return Some(attack.contact_open);
            }
        }
        let brain = self.ai_brains.get(&parent)?;
        if !self.uses_demo_combat(parent, &brain.ai_type) {
            return None;
        }
        let component = self.entities[weapon].components.iter().find(|c| c.class_name == "WeaponComponent")?;
        let event = match component.properties.iter().find(|p| p.name == "WeaponType")?.value.as_u64()? {
            0 => "EVENT_Start_AttackMelee",
            2 => "EVENT_Start_AttackCharge",
            _ => return None,
        };
        (self.authored_havok.has_event(parent, event)
            || event == "EVENT_Start_AttackMelee" && self.authored_havok.has_event(parent, "EVENT_WalkAttack_Start"))
            .then_some(false)
    }
    pub(super) fn record_demo_melee_contact(&mut self, weapon: usize) {
        if self.demo_melee_trigger_enabled(weapon) != Some(true) {
            return;
        }
        let parent = self.transform_hierarchy[&weapon].parent;
        let attack = self.demo_story.ai.attacking.get_mut(&parent).unwrap();
        if !attack.hit {
            attack.hit = true;
            self.demo_story.ai.attacks += 1;
            self.demo_story.ai.charge_contacts += u64::from(attack.charge.is_some());
        }
    }
    pub(super) fn advance_demo_attack_animation(&mut self, actor: usize, event: u32) -> Result<()> {
        if self.demo_story.ai.detonations.get(&actor) == Some(&false)
            && self.animation_events.as_ref().ok()
                .and_then(|catalog| catalog.id("EVENT_Done_Explode")) == Some(event)
        {
            return self.finish_demo_detonation(actor);
        }
        // Latch the ranged aim on the authored prepare cue. The projectile
        // release cue can come later, after Mickey has already dodged.
        let aim = if matches!(event, 0x98 | 0x99) {
            let target = self.demo_story.ai.attacking.get(&actor).and_then(|attack| {
                if attack.phase == AttackPhase::AwaitStrike { attack.ranged_target } else { None }
            });
            match target {
                Some(target) => self.entity_position_or_bound_center(target)?,
                None => None,
            }
        } else {
            None
        };
        let Some(attack) = self.demo_story.ai.attacking.get_mut(&actor) else {
            return Ok(());
        };
        if attack.charge.is_some() {
            if event == 0xc1 && attack.phase == AttackPhase::Recovery {
                self.demo_story.ai.attacking.remove(&actor);
            }
            return Ok(());
        }
        if attack.ranged_target.is_some() {
            match (event, attack.phase) {
                (0x95, AttackPhase::Windup) if attack.windup_end.is_none() => {
                    attack.windup_end = Some(self.animation_time + attack.windup_idle);
                }
                (0x98 | 0x99, AttackPhase::AwaitStrike) => attack.ranged_aim = aim,
                (0x7a | 0x9a, AttackPhase::AwaitStrike) => {
                    if let Some(aim) = attack.ranged_aim {
                        self.demo_story.projectiles.queue(actor, attack.weapon, aim);
                    }
                    attack.phase = AttackPhase::Recovery;
                }
                (0x97 | 0xc1, AttackPhase::AwaitStrike | AttackPhase::Recovery) => {
                    self.demo_story.ai.attacking.remove(&actor);
                }
                _ => {}
            }
            return Ok(());
        }
        match (event, attack.phase) {
            (0x75, AttackPhase::Windup) if attack.windup_end.is_none() => {
                attack.windup_end = Some(self.animation_time + attack.windup_idle);
            }
            (0x7a, AttackPhase::AwaitStrike) => {
                attack.phase = AttackPhase::Striking;
                attack.contact_open = true;
            }
            // Some graphs stop the weapon in the follow-through clip,
            // while others stop it before branching to hit or miss.
            (0x7b, _) => attack.contact_open = false,
            (0xc1, AttackPhase::Recovery) => {
                self.demo_story.ai.attacking.remove(&actor);
            }
            (0xc1, AttackPhase::AwaitStrike | AttackPhase::Striking) if attack.walk_attack => {
                self.demo_story.ai.attacking.remove(&actor);
            }
            _ => {}
        }
        Ok(())
    }
    pub(super) fn clear_ai_combat_target(&mut self, entity: usize) {
        let Some(brain) = self.ai_brains.get_mut(&entity) else {
            return;
        };
        brain.combat_target = None;
        self.demo_story.ai.steering.remove(&entity);
        self.demo_story.ai.last_seen.remove(&entity);
        self.demo_story.ai.sight_checks.remove(&entity);
        if self.demo_story.ai.attacking.remove(&entity).is_some() {
            let _ = self.authored_havok.send_event(entity, "EVENT_ForceNormalIdle");
        }
    }
    pub(super) fn demo_melee_clear(
        &mut self,
        shape: usize,
        source: [f32; 3],
        destination: [f32; 3],
        target: usize,
    ) -> Result<bool> {
        let collision = &self.collision;
        let actor_filter = collision
            .simulation_filter(shape)
            .context("AI source body has no collision filter")?;
        let target_owner = if target == self.player_entity {
            self.player_physics.as_ref().map(|b| b.owner)
        } else {
            self.character_physics
                .get(&target)
                .and_then(|b| b.as_ref().ok())
                .map(|b| b.owner)
        };
        // Compare simulation body filters. Query override mode instead treats
        // its first word as a layer bitmask, not the actor's packed filter.
        self.dynamic_runtime_filter.query(None, |filter| {
            collision.with_query_body_excluded(shape, |collision| {
                let origin = [source[0], source[1] + 0.75, source[2]];
                let translation = [
                    destination[0] - source[0],
                    destination[1] - source[1],
                    destination[2] - source[2],
                ];
                Ok(!collision.ray_hits(origin, translation)?.iter().any(|hit| {
                    hit.fraction < 0.98
                        && Some(hit.owner) != target_owner
                        && collision.owner_is_solid(hit.owner)
                        && collision.simulation_filter(hit.shape).is_some_and(|word| {
                            filter.collision_info(actor_filter, word) >> 24 != 0
                        })
                }))
            })?
        })
    }
    pub(super) fn demo_steer(
        &mut self,
        entity: usize,
        shape: usize,
        source: [f32; 3],
        destination: [f32; 3],
        prefer_patrol_target: bool,
    ) -> Result<[f32; 2]> {
        const LOOKAHEAD: f32 = 1.1;
        let previous = self.demo_story.ai.steering.get(&entity).copied();
        if let Some(cached) = previous {
            if cached.until > self.animation_time
                && (source[0] - cached.source[0]).hypot(source[2] - cached.source[2]) < LOOKAHEAD * 0.5
                && super::physics::length(super::physics::sub(cached.destination, destination))
                    < 0.8
            {
                return Ok(cached.direction);
            }
        }
        // Route through the level's authored Kynapse graph before applying
        // local collision avoidance. If no usable route is available, retain
        // the local steering fallback so unpathed encounters still function.
        let route_target = self
            .ai_navigation
            .waypoint(source, destination)
            .unwrap_or(destination);
        let dx = route_target[0] - source[0];
        let dz = route_target[2] - source[2];
        let length = (dx * dx + dz * dz).sqrt();
        if length < 0.001 {
            return Ok([0.; 2]);
        }
        let forward = [dx / length, dz / length];
        let to_goal = [destination[0] - source[0], destination[2] - source[2]];
        let goal_distance = to_goal[0].hypot(to_goal[1]);
        let direct = (prefer_patrol_target && goal_distance > 0.001)
            .then(|| [to_goal[0] / goal_distance, to_goal[1] / goal_distance]);
        let stuck = self.demo_story.ai.moving.get(&entity) == Some(&true) && previous.is_some_and(|old| {
            old.direction != [0.; 2]
                && super::physics::length(super::physics::sub(source, old.source)) < 0.035
        });
        let steering_trace = log::log_enabled!(target: "blotworx_ai_steering", log::Level::Trace);
        if steering_trace {
            log::trace!(target: "blotworx_ai_steering", "{} t={:.2} source={source:?} waypoint={route_target:?} goal={destination:?} stuck={stuck}",
                self.entities[entity].name, self.animation_time);
        }
        let mut best = ([0.; 2], f32::NEG_INFINITY);
        let collision = &self.collision;
        let actor_filter = collision
            .simulation_filter(shape)
            .context("AI steering source has no collision filter")?;
        let mut characters = self
            .character_physics
            .values()
            .filter_map(|b| b.as_ref().ok().map(|b| b.owner))
            .collect::<std::collections::HashSet<_>>();
        if let Some(player) = &self.player_physics {
            characters.insert(player.owner);
        }
        let mut steering_hits = Vec::new();
        let dynamic_sources = &self.dynamic_runtime_sources;
        let constructions = &self.dynamic_constructions;
        self.dynamic_runtime_filter.query(None, |filter| {
            collision.with_query_body_excluded(shape, |collision| -> Result<()> {
                // A discrete patrol target can remain locally reachable even
                // when a nearby graph node points into the swept path of a
                // moving door. Test the direct heading with the same clearance
                // and floor checks, then retain graph steering around obstacles.
                let candidates = direct.filter(|_| !stuck)
                    .map(|heading| (heading, 0., goal_distance.min(LOOKAHEAD), true)).into_iter()
                    .chain([0f32, 45., -45., 90., -90., 135., -135.]
                        .map(|angle| (forward, angle, LOOKAHEAD, false)));
                let mut direct_obstruction = None;
                'headings: for (heading, degrees, distance, toward_goal) in candidates {
                    if stuck && degrees == 0. {
                        continue;
                    }
                    let (sin, cos) = degrees.to_radians().sin_cos();
                    let direction = [
                        heading[0] * cos - heading[1] * sin,
                        heading[0] * sin + heading[1] * cos,
                    ];
                    let travel = [direction[0] * distance, 0., direction[1] * distance];
                    let origin = [source[0], source[1] + 0.7, source[2]];
                    let mut blocked = false;
                    for side in [-0.27f32, 0., 0.27] {
                        let start = [
                            origin[0] - direction[1] * side,
                            origin[1],
                            origin[2] + direction[0] * side,
                        ];
                        if let Some(hit) = collision.ray_hits(start, travel)?.into_iter().find(|hit| {
                            collision.owner_is_solid(hit.owner)
                                && collision.simulation_filter(hit.shape).is_some_and(|word| {
                                    filter.collision_info(actor_filter, word) >> 24 != 0
                                })
                        }) {
                            if steering_trace {
                                steering_hits.push((degrees, hit.owner, hit.position));
                            }
                            if toward_goal {
                                direct_obstruction = Some(hit.owner);
                            } else if degrees == 0. && direct_obstruction == Some(hit.owner)
                                && dynamic_sources.iter().any(|(source, owner)| {
                                    *owner == hit.owner && constructions[source].bodies.iter()
                                        .zip(&constructions[source].node_bindings).any(|(body, binding)| {
                                            binding.is_some() && body.motion.current.motion_type() == 4
                                                && body.shapes.contains(&hit.shape)
                                        })
                                })
                            {
                                // Both approaches reach the same animated
                                // collision. Hold at the gate until it clears.
                                break 'headings;
                            }
                            blocked = true;
                            break;
                        }
                    }
                    if blocked {
                        continue;
                    }
                    let probe = [
                        source[0] + travel[0],
                        source[1] + 0.65,
                        source[2] + travel[2],
                    ];
                    let ground = collision
                        .ray_hits(probe, [0., -1.4, 0.])?
                        .into_iter()
                        .filter(|hit| {
                            collision.owner_is_solid(hit.owner)
                                && hit.normal[1] > 0.55
                                && !characters.contains(&hit.owner)
                                && collision.simulation_filter(hit.shape).is_some_and(|word| {
                                    filter.collision_info(actor_filter, word) >> 24 != 0
                                })
                        })
                        .map(|hit| hit.position[1])
                        .filter(|height| *height - source[1] <= 0.45 && source[1] - *height <= 0.65)
                        .max_by(f32::total_cmp);
                    let Some(height) = ground else {
                        if steering_trace {
                            log::trace!(target: "blotworx_ai_steering", "angle={degrees} no walkable support at {probe:?}");
                        }
                        continue;
                    };
                    let persistence = previous.map_or(0., |old| {
                        old.direction[0] * direction[0] + old.direction[1] * direction[1]
                    });
                    let score = cos * 2. + persistence * 0.35 - (height - source[1]).abs() * 0.2;
                    if score > best.1 {
                        best = (direction, score);
                    }
                    if degrees == 0. {
                        break;
                    }
                }
                Ok(())
            })?
        })?;
        if steering_trace {
            for (angle, owner, point) in steering_hits {
                let name = self.collision_entity(owner).map(|entity| entity.name.as_str());
                log::trace!(target: "blotworx_ai_steering", "angle={angle} blocked by {owner} {name:?} at {point:?}");
            }
            log::trace!(target: "blotworx_ai_steering", "selected direction {:?}", best.0);
        }
        self.demo_story.ai.steering.insert(
            entity,
            Steering {
                until: self.animation_time + 0.2,
                source,
                // Cache validity compares against the moving gameplay target,
                // not the intermediate graph waypoint.
                destination,
                direction: best.0,
            },
        );
        Ok(best.0)
    }
}

#[cfg(any(test, debug_assertions))]
pub(in crate::blotworx) mod tests {
    use super::*;
    use anyhow::{ensure, Context};
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn scripted_patrol_is_not_rejected_by_demo_combat_perception_range()->Result<()> {
        let root=std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world=super::super::assets::load(std::path::Path::new(&root),"Levels/GV_ZoneA_Start.level",&egui::Context::default())?;
        world.enable_demo_story()?;
        // Isolate route admission from the authored intro. This is a
        // controlled World test, not an intro playthrough.
        while world.cutscene_input.count!=0 {world.run_callback_fixture("UnrestrictCutSceneInput()")?;}
        world.run_callback_fixture("UnpauseAllAI(); AIController_SetDisabled('gremlinvillage_zonea.SpatterMelee 02',false); EnableMotion('gremlinvillage_zonea.SpatterMelee 02')")?;
        let entity=world.scripts.resolve_name("gremlinvillage_zonea.SpatterMelee 02").context("Original Spatter missing")?;
        world.advance_demo_ai(super::super::STEP)?;
        ensure!(!world.demo_story.ai.decisions[&entity].movement_requested,"Distant player unexpectedly triggered movement");
        world.run_callback_fixture("SetNextPatrolNode('gremlinvillage_zonea.SpatterMelee 02','gremlinvillage_zonea.SpatterMelee 01')")?;
        ensure!(!world.demo_story.ai.patrol.contains_key(&entity),"Non-PatrolNode entity was accepted as a node");
        let source=world.entities[entity].transform;
        let target=world.entities.iter().find(|target| {
            let dx=target.transform[12]-source[12];let dz=target.transform[14]-source[14];
            dx*dx+dz*dz>18.*18. && target.components.iter().any(|c|c.class_name=="PatrolNodeComponent")
        }).context("Original distant PatrolNode missing")?.name.clone();
        world.run_callback_fixture(&format!("SetNextPatrolNode('gremlinvillage_zonea.SpatterMelee 02',{target:?})"))?;
        world.advance_demo_ai(super::super::STEP)?;
        let decision=&world.demo_story.ai.decisions[&entity];
        ensure!(decision.scripted_patrol && decision.distance>18. && !decision.in_range,"Original fixture did not exercise distant scripted movement");
        ensure!(decision.movement_requested,"Authored patrol was discarded by combat perception: {:?}",decision.pause_reason);
        // Controlled original-data fixture: establish a heading error, then
        // require the actual Lua/cache path to hold physical orientation at0.
        let direction=world.demo_story.ai.decisions[&entity].direction;
        let shape=world.character_physics[&entity].as_ref().unwrap().shape;
        let position=world.collision.simulation_pose(shape)?.position;
        let yaw=(-direction[0]).atan2(direction[1])+1.;
        let (s,c)=yaw.sin_cos();
        world.warp_character_body(entity,super::super::native_hk_transform::from_entity(
            [c,0.,-s,0.,1.,0.,s,0.,c],position))?;
        world.publish_character_physics(super::super::STEP)?;
        let held=world.collision.simulation_pose(shape)?.orientation;
        world.run_callback_fixture("SetOverrideTurnRate('gremlinvillage_zonea.SpatterMelee 02',0)")?;
        world.advance_demo_ai(super::super::STEP)?;
        let after=world.collision.simulation_pose(shape)?.orientation;
        let sign=if held.iter().zip(after).map(|(a,b)|a*b).sum::<f32>()<0. {-1.}else{1.};
        // Two single-precision basis/quaternion conversions, not a trajectory tolerance.
        ensure!((0..4).all(|i|(held[i]-sign*after[i]).abs()<=2e-6),"Turn override did not reach physical NPC facing");
        world.run_callback_fixture("ResetOverrideTurnRate('gremlinvillage_zonea.SpatterMelee 02')")?;
        world.run_callback_fixture("RestrictCutSceneInput()")?;
        world.advance_demo_ai(super::super::STEP)?;
        ensure!(world.cutscene_input.restricted,"Player restriction was not applied");
        ensure!(world.demo_story.ai.decisions[&entity].movement_requested,"Player input restriction incorrectly froze an authored NPC patrol");
        world.run_callback_fixture("PauseAllAI()")?;
        world.advance_demo_ai(super::super::STEP)?;
        ensure!(!world.demo_story.ai.decisions[&entity].movement_requested,"Explicit AI pause did not stop the patrol");
        ensure!(world.cutscene_input.restricted,"AI pause changed player input ownership");
        world.run_callback_fixture("UnpauseAllAI()")?;
        world.advance_demo_ai(super::super::STEP)?;
        ensure!(world.demo_story.ai.decisions[&entity].movement_requested,"AI did not resume while the player remained restricted");
        world.run_callback_fixture("UnrestrictCutSceneInput()")?;
        world.run_callback_fixture("ClearNextPatrolNode('gremlinvillage_zonea.SpatterMelee 02')")?;
        world.advance_demo_ai(super::super::STEP)?;
        let decision=&world.demo_story.ai.decisions[&entity];
        ensure!(!decision.scripted_patrol && !decision.movement_requested,"Clearing patrol did not restore combat perception");
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn inactive_scene_does_not_steer_or_animate_retained_characters() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::super::assets::load(
            std::path::Path::new(&root), "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        world.enable_demo_story()?;
        let entity = world.scripts.resolve_name("gremlinvillage_zonea.SpatterMelee 02")
            .context("Original Spatter missing")?;
        let shape = world.character_physics[&entity].as_ref()
            .map_err(|e| anyhow::anyhow!("{e}"))?.shape;
        ensure!(!world.hidden_entities.contains(&entity), "Fixture must exercise scene activation independently of hiding");
        let scene = world.entities[entity].scene.clone();
        ensure!(world.entity_scene_active(entity), "Fixture scene was not active");
        world.scene_resources.deactivated(&scene);
        world.collision.set_simulation_velocity(shape, [0.25, -0.5, 0.75])?;
        let before = world.collision.simulation_pose(shape)?;
        world.advance_demo_ai(super::super::STEP)?;
        ensure!(!world.demo_story.ai.decisions.contains_key(&entity), "Inactive NPC received steering/attack updates");
        ensure!(!world.demo_story.ai.moving.contains_key(&entity), "Inactive NPC received a locomotion animation request");
        let after = world.collision.simulation_pose(shape)?;
        ensure!(after.linear == before.linear && after.position == before.position,
            "Inactive NPC's retained physics state changed");
        world.scene_resources.published(&scene);
        world.advance_demo_ai(super::super::STEP)?;
        ensure!(world.demo_story.ai.decisions.contains_key(&entity), "Reactivated NPC did not resume updates");
        Ok(())
    }
    #[cfg_attr(test, test)]
    #[cfg_attr(test, ignore = "requires original EM1 files in BLOTWORX_TEST_DATA")]
    pub(in crate::blotworx) fn original_steam_callback_pushes_live_player_toward_its_marker_without_damage(
    ) -> Result<()> {
        use super::super::{
            controller::{Controller, Input, TickPhase},
            knockback,
        };
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::super::assets::load(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        world.enable_demo_story()?;
        world.advance_models(0.)?;
        world.attach_player_body()?;
        let mut player = world.take_startup_player();
        // Isolated receiver fixture at the actual trigger contact from the
        // normal-input capture. This is not evidence of a continuous traversal
        // to the steam hazard; controller preparation/completion are retained.
        player.position = [55.545353, 0.0424811, 15.26334];
        player.previous = player.position;
        world.entities[world.player_entity].set_controller_pose(player.position, player.yaw);
        player.prepare_input_step(
            &world.collision,
            [0.; 2],
            [0., 0., 1.],
            false,
            super::super::STEP,
        );
        let before_velocity = player.velocity;
        let marker = world
            .scripts
            .resolve_name("gremlinvillage_zonea.PositionMarker 09")
            .context("Steam direction marker missing")?;
        let marker_position = world.entities[marker].transform[12..15].try_into()?;
        let direction =
            super::super::script_stimulus::direction(player.position, marker_position, 0.)?
                .unwrap();
        let initial = player.position;
        let health = world.runtime_health[&world.player_entity].health;
        world.with_live_player(&mut player, |world| {
            world.run_callback_fixture("GV_ZoneA_KnockBackTest()")
        })?;
        assert_eq!(
            player.velocity, before_velocity,
            "Stimulus callback bypassed the native phase0/1 launch schedule"
        );
        let runtime = world
            .player_knockback
            .as_ref()
            .context("Steam callback did not admit native knockback")?;
        assert!(runtime.state.active && player.native_knockback);
        assert_eq!(
            runtime.state.launch_velocity,
            knockback::launch(direction, 667., &runtime.settings)?
        );
        assert!(
            (super::super::physics::length([
                runtime.state.launch_velocity[0],
                0.,
                runtime.state.launch_velocity[2]
            ]) - 12.)
                .abs()
                < 0.00001,
            "Original player cap1.2 did not produce native12m/s horizontal launch"
        );
        assert_eq!(
            player.position, initial,
            "Knockback must not teleport the player"
        );
        assert_eq!(
            world.runtime_health[&world.player_entity].health, health,
            "Knockback was treated as damage"
        );
        let admissions = world.player_knockback.as_ref().unwrap().state.admissions;
        world.with_live_player(&mut player,|world|world.run_callback_fixture("StimulusEntityWithAliasInEntityDirectionInRadius('gremlinvillage_zonea.Trigger 10', ST_KNOCKBACK, 667, 0.00001, GetPlayer(), 'gremlinvillage_zonea.PositionMarker 09', '')"))?;
        assert_eq!(
            world.player_knockback.as_ref().unwrap().state.admissions,
            admissions,
            "Out-of-radius directional stimulus was delivered"
        );
        let mut preview = super::super::authored_preview::Preview::new(&mut world)?;
        let mut controller = Controller::new(player);
        let mut clips = std::collections::BTreeSet::new();
        let mut timeline = Vec::new();
        let mut last_phase = None;
        let mut maximum_distance = 0f32;
        for _ in 0..360 {
            world.frame_clock.publish(true, true);
            controller.advance_with_tick_phases(&mut world,|w|&w.collision,1./30.,true,Input::default(),
                |phase,player,tick,_,_,world|->Result<()> {
                    if phase==TickPhase::BeforeMovement {
                        world.prepare_scene_motion(super::super::STEP)?;
                        world.publish_player_ground_record(player)?;
                        world.advance_player_knockback(player,super::super::STEP)?;
                    }
                    if phase==TickPhase::SimulatePhysics {world.advance_player_physics(player,super::super::STEP)?;}
                    if phase==TickPhase::AfterMovement {
                        world.player_controller_tick(&mut preview,player)?;
                        let frames=std::mem::take(&mut preview.playback.emitted_frames);
                        world.with_live_player(player,|world|world.export_player_animation_frames(frames))?;
                        world.synchronize_player_knockback(player);
                        for node in &world.player_graph.plan.nodes {if let Some(clip)=world.player_graph.state.clips.get(&node.object){clips.insert(clip.settings.animation_name.clone());}}
                        let state=world.player_knockback.as_ref().unwrap().state.clone();
                        let now=(state.phase,state.active);
                        if Some(now)!=last_phase {timeline.push(serde_json::json!({"tick":tick,"state":state,"position":player.position,
                            "velocity":player.velocity,"support":player.support_state.map(|s|s as u32)}));last_phase=Some(now);}
                    }
                    Ok(())
                })?;
            maximum_distance = maximum_distance.max(super::super::physics::length(
                super::super::physics::sub(controller.player.position, initial),
            ));
            if world.player_knockback.as_ref().unwrap().state.recoveries > 0 {
                break;
            }
        }
        let state = &world.player_knockback.as_ref().unwrap().state;
        std::fs::write(
            "target/player-knockback-gameplay-validation.json",
            serde_json::to_vec_pretty(&serde_json::json!({
            "ticks":controller.ticks,"state":state,"timeline":timeline,"clips":clips,"maximum_distance":maximum_distance,
            "respawns":controller.player.respawns,"health_before":health,"health_after":world.runtime_health[&world.player_entity].health,
            "scope":"Original steam callback and original player graph in the live shared solver; controlled placement, not an ordinary playthrough or original-runtime solver comparison."}))?,
        )?;
        anyhow::ensure!(
            state.recoveries == 1 && !controller.player.native_knockback,
            "Original get-up animation did not release native motion10"
        );
        anyhow::ensure!(
            controller.player.respawns == 0,
            "Knockback fixture fell out of the authored level"
        );
        anyhow::ensure!(
            maximum_distance > 0.5,
            "Native knockback did not move its shared character body"
        );
        anyhow::ensure!(
            clips
                .iter()
                .any(|clip| clip.to_ascii_lowercase().contains("knockback")),
            "Original knockback clip was not played"
        );
        anyhow::ensure!(
            clips
                .iter()
                .any(|clip| clip.eq_ignore_ascii_case("Mickey_GetUp.hkx")),
            "Original get-up clip was not played"
        );
        assert_eq!(
            world.runtime_health[&world.player_entity].health, health,
            "Knockback alone subtracted health"
        );
        assert_eq!(
            world
                .collision
                .simulation_pose(world.player_physics.as_ref().unwrap().shape)?
                .position,
            controller.player.position
        );
        println!("PASS: original steam stimulus enters native motion10, moves the shared body, plays knockback/get-up and releases through animation feedback");
        Ok(())
    }
}
impl World {
    /// Functional demo receiver. The Lua direction/radius and vulnerability
    /// filtering are recovered separately; native character action admission,
    /// NPC reaction timing still follows the compatibility stun interval.
    pub(super) fn demo_character_knockback(
        &mut self,
        entity: usize,
        direction: [f32; 3],
        amount: f32,
    ) -> Result<()> {
        if entity == self.player_entity {
            return self.begin_player_knockback(direction, amount);
        } else if let Some(Ok(body)) = self.character_physics.get(&entity) {
            self.collision
                .apply_owner_impulse(body.owner, direction.map(|v| v * amount))?;
            self.begin_npc_knockback_animation(entity)?;
        } else {
            anyhow::bail!("Demo knockback target lacks its character body");
        }
        Ok(())
    }
    fn npc_knockback_event(&mut self, entity: usize, name: &str) -> Result<bool> {
        let id = self.animation_events.as_ref().ok().and_then(|catalog| catalog.id(name));
        match id {
            Some(id) if self.authored_havok.contains(entity) => self.authored_havok.send_native_event(entity, id),
            _ => Ok(false),
        }
    }
    fn begin_npc_knockback_animation(&mut self, entity: usize) -> Result<()> {
        let previous = self.demo_story.ai.stunned_until.insert(entity, self.animation_time + 0.9);
        self.demo_story.ai.attacking.remove(&entity);
        if previous.is_none_or(|end| end <= self.animation_time) {
            self.npc_knockback_event(entity, "EVENT_Start_Knockback_FallDown")?;
        }
        Ok(())
    }
    pub(super) fn demo_spin_hit(&mut self, entity: usize, origin: [f32; 3]) -> Result<()> {
        let Some(brain) = self.ai_brains.get(&entity) else {
            return Ok(());
        };
        let family = brain.ai_type.to_ascii_lowercase();
        if !(family.contains("spatter") || family.contains("fodder")) || family.contains("kamikaze") {
            return Ok(());
        }
        if let Some(Ok(body)) = self.character_physics.get(&entity) {
            let pose = self.collision.simulation_pose(body.shape)?;
            let dx = pose.position[0] - origin[0];
            let dz = pose.position[2] - origin[2];
            let length = (dx * dx + dz * dz).sqrt().max(0.01);
            self.collision
                .set_simulation_velocity(body.shape, [dx / length * 6., 3., dz / length * 6.])?;
        }
        self.begin_npc_knockback_animation(entity)?;
        if !self
            .paint_counters
            .get(&entity)
            .is_some_and(|c| c.status() == 1)
        {
            let emission = super::projectile_stimulus::Emission {
                stimulus: 8,
                amount: 1.,
                position: origin,
                direction: [0.; 3],
                target: Some(entity as u64),
                owner: Some(self.player_entity as u64),
                tag: "Demo spin".into(),
                delivery: super::projectile_stimulus::Delivery::Target,
            };
            self.receive_combat(entity, &[emission])?;
        }
        Ok(())
    }
    pub(super) fn demo_ai_lua(
        &mut self,
        lua: &mlua::Lua,
        name: &str,
        args: mlua::MultiValue,
    ) -> mlua::Result<mlua::MultiValue> {
        let Some(value) = args.front() else {
            return Ok(mlua::MultiValue::new());
        };
        let Some(entity) = self.scripts.resolve_world_argument(value)? else {
            return Ok(mlua::MultiValue::new());
        };
        match name {
            "GetCurrentTeam" => {
                let team = self.entities[entity]
                    .components
                    .iter()
                    .find(|c| c.class_name == "TeamComponent")
                    .and_then(|component| {
                        let team = |name| component.properties.iter()
                            .find(|property| property.name == name)
                            .and_then(|property| property.value.as_i64());
                        // Seer stores zero in the cooked CurrentTeam slot.
                        // The authored Team supplies the initial allegiance.
                        team("CurrentTeam").filter(|team| *team != 0).or_else(|| team("Team"))
                    })
                    .unwrap_or(0);
                // EM1's authored Spatter encounter distinguishes hostile (2)
                // from converted (3). Mickey's own team (1) is not this state.
                let team = if team == 2
                    && self
                        .paint_counters
                        .get(&entity)
                        .is_some_and(|c| c.status() == 1)
                {
                    3
                } else {
                    team
                };
                return Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Integer(team)]));
            }
            _ => {}
        }
        let _ = lua;
        Ok(mlua::MultiValue::new())
    }
    fn seer_charge(&self, weapon: usize, windup: f32) -> Result<(Charge, f32)> {
        let component = self.entities[weapon].components.iter().find(|c| c.class_name == "WeaponComponent")
            .context("Charge weapon component missing")?;
        let distance = component.properties.iter().find(|p| p.name == "Weapon Charge Distance")
            .and_then(|p| p.value.as_f64()).context("Charge weapon distance missing")? as f32;
        let setting = |key: &str| -> Result<f32> {
            let key = format!("BEH_ChargeAttack_FodderBlotling_{key}");
            let value = self.runtime_settings.get(&key).with_context(|| format!("Missing {key}"))?;
            super::config_value::decimal_float(value).map_err(|error| anyhow::anyhow!("{key}: {error}"))
        };
        Ok((Charge {
            distance,
            move_speed: setting("MoveSpeed")?,
            top_speed: setting("TopMoveSpeed")?,
            ramp_time: setting("RampUpTime")?,
            blocked_time: setting("TimeStoppedToDetectHit")?,
            started: 0.,
            origin: [0.; 3],
            direction: [0.; 2],
            progress: 0.,
            stopped: 0.,
        }, if windup < 0. { setting("WindupIdleTime")? } else { windup }))
    }
    fn finish_demo_detonation(&mut self, entity: usize) -> Result<()> {
        use super::projectile_stimulus::{Delivery, Emission};
        let Some(component) = self.entities[entity].components.iter().find(|c| c.class_name == "ExplodeComponent") else {
            return Ok(());
        };
        let field = |name| component.properties.iter().find(|p| p.name == name).map(|p| &p.value);
        let numbers = |name| field(name).and_then(|value| value.as_array()).into_iter().flatten();
        let stimuli = numbers("StimulusTypeList").zip(numbers("StimulusAmountList"))
            .zip(numbers("StimulusRadiusList"))
            .filter_map(|((kind, amount), radius)| Some((u32::try_from(kind.as_u64()?).ok()?,
                amount.as_f64()? as f32, radius.as_f64()? as f32)))
            .filter(|&(_, amount, radius)| amount.is_finite() && radius.is_finite() && radius > 0.)
            .collect::<Vec<_>>();
        let check_sight = field("Check Line Of Sight").and_then(|v| v.as_bool()).unwrap_or(false);
        let retire = field("RemoveEntity").and_then(|v| v.as_bool()).unwrap_or(false);
        let source = self.entities[entity].transform[12..15].try_into()?;
        let shape = self.character_physics.get(&entity).and_then(|body| body.as_ref().ok()).map(|body| body.shape);
        self.demo_story.ai.detonations.insert(entity, true);
        let mut pending = BTreeMap::<usize, Vec<Emission>>::new();
        for &(stimulus, amount, radius) in &stimuli {
            for target in self.area_stimulus_recipients(source, radius, Some(entity))? {
                if target == entity { continue; }
                let Some(position) = self.entity_position_or_bound_center(target)? else { continue; };
                if let Some(shape) = shape {
                    if check_sight && !self.demo_melee_clear(shape, source, position, target)? { continue; }
                }
                let offset = super::physics::sub(position, source);
                let length = super::physics::length(offset).max(0.001);
                pending.entry(target).or_default().push(Emission {
                    stimulus, amount, position: source, direction: offset.map(|value| value / length),
                    target: Some(target as u64), owner: Some(entity as u64),
                    tag: String::new(), delivery: Delivery::Target,
                });
            }
        }
        // Retail Spladoosh CSG thinning (Spatter_TimeBeforeExplodeCausesCSG):
        // if the explosion carries thinner stimulus (0), thin overlapped paintable jigsaw geometry.
        if let Some(&(_, _, thin_radius)) = stimuli.iter().find(|&&(k, _, _)| k == 0) {
            if let Ok(overlaps) = self.collision.overlap_sphere(source, thin_radius) {
                let owners = overlaps
                    .into_iter()
                    .map(|hit| hit.owner)
                    .collect::<std::collections::BTreeSet<_>>();
                for owner in owners {
                    let _ = self.brush_jigsaw_contact(owner, false);
                }
            }
        }
        for (target, emissions) in pending {
            self.receive_combat(target, &emissions)?;
        }
        self.emit_entity_output(entity, "OnExplode", entity);
        if retire {
            self.emit_entity_output(entity, "OnDeath", entity);
            self.emit_entity_output(entity, "OnDefeated", entity);
            self.queue_entity_retirement(entity)?;
        }
        Ok(())
    }

    pub(super) fn advance_demo_ai(&mut self, delta: f32) -> Result<()> {
        if !self.demo_story.enabled {
            return Ok(());
        }
        let finished: Vec<_> = self.demo_story.ai.stunned_until.iter()
            .filter_map(|(&entity, &end)| (end <= self.animation_time).then_some(entity))
            .collect();
        for entity in finished {
            self.demo_story.ai.stunned_until.remove(&entity);
            if !self.removed_entities.contains(&entity) && self.entity_scene_active(entity)
                && !self.npc_knockback_event(entity, "EVENT_Stop_Knockback_InAir")?
            {
                self.npc_knockback_event(entity, "EVENT_Start_Knockback_Recover")?;
            }
        }
        let melee_setting = |name: &str, default| self.runtime_settings.get(name)
            .and_then(|value| super::config_value::decimal_float(value).ok())
            .unwrap_or(default);
        let melee_radius = melee_setting("SpatterMelee_AttackRadius", 1.25);
        let melee_chase = melee_setting("SpatterMelee_ChaseSpeed", 5.);
        let melee_walk = melee_setting("SpatterMelee_DefaultSpeed", 3.);
        let friendly_distance = melee_setting("SpatterFriendlyFollowTolerance", 2.5);
        let kamikaze_reach = melee_setting("BEH_KamikazeChase_Default_ChaseTolerance", 1.5);
        let kamikaze_speed = melee_setting("BEH_KamikazeChase_Default_ChaseSpeed", 5.8);
        let attack_cap = |key, default| self.runtime_settings.get(key)
            .and_then(|value| value.trim().parse::<usize>().ok()).unwrap_or(default);
        let melee_slots = attack_cap("AttackConfig_MaxConcurrentMeleeAttackers", 1);
        let ranged_slots = attack_cap("AttackConfig_MaxConcurrentRangedAttackers", 2);
        self.demo_story.ai.detonations.retain(|entity, _| !self.removed_entities.contains(entity));
        self.demo_story.ai.last_seen.retain(|entity, (_, expiry)| {
            *expiry >= self.animation_time && !self.removed_entities.contains(entity)
        });
        self.demo_story.ai.home.retain(|entity, _| !self.removed_entities.contains(entity));
        self.demo_story.ai.sight_checks.retain(|entity, (_, next)| {
            *next + 1. >= self.animation_time && !self.removed_entities.contains(entity)
        });
        let ids = self
            .ai_brains
            .iter()
            .filter(|&(&entity, brain)| self.uses_demo_combat(entity, &brain.ai_type))
            .map(|(&id, _)| id)
            .collect::<Vec<_>>();
        for entity in ids {
            // Loaded resources are not necessarily active scenes. In particular,
            // streaming can retain entities before activation; neither steering,
            // attack clocks nor animation requests may run for those entities.
            if !self.entity_scene_active(entity)
                || self.removed_entities.contains(&entity)
                || self.hidden_entities.contains(&entity)
            {
                continue;
            }
            let Some(Ok(body)) = self.character_physics.get(&entity) else {
                continue;
            };
            let (shape, owner) = (body.shape, body.owner);
            let brain = &self.ai_brains[&entity];
            let scripted_move = brain.scripted_move.values().any(|move_request| move_request.pending)
                || brain.selector.current.and_then(|slot| brain.behaviors.get(slot))
                    .is_some_and(|behavior| behavior == "ScriptedMove");
            if scripted_move && self.demo_npc_active(entity) {
                // The scripted task owns velocity and facing until its arrival event.
                self.demo_story.ai.attacking.remove(&entity);
                continue;
            }
            if !self.collision.owner_in_world(owner)? {
                continue;
            }
            let pose = self.collision.simulation_pose(shape)?;
            if self.animation_time
                < self
                    .demo_story
                    .ai
                    .stunned_until
                    .get(&entity)
                    .copied()
                    .unwrap_or(0.)
            {
                self.demo_story.ai.attacking.remove(&entity);
                continue;
            }
            // RELd3e0 -> b2ec ->324bbc restricts the current player's
            // controller byte31. It does not pause NPCs. Authored encounter
            // motion must continue while the player watches a sequence.
            let exposed = self.ai_manager_enabled != Some(false) && self.advance_enemy_armor(entity)?;
            let pause_reason = if exposed {
                Some("exposed weak point")
            } else if self.demo_story.conversation.contains(&entity) {
                Some("conversation")
            } else if self.ai_manager_enabled == Some(false) {
                Some("AI manager paused")
            } else if self.ai_component_disabled(entity,"BasicAIControllerComponent") {
                Some("AI controller disabled")
            } else if self.ai_component_disabled(entity,"AIBrainComponent") {
                Some("AI brain disabled")
            } else if self.basic_motion_suspended(entity) == Some(true) {
                Some("motion component disabled")
            } else if self.ai_brains[&entity].motion_suspended {
                Some("brain motion suspended")
            } else if self.ai_brains[&entity].paused.values().any(|p| p.active) {
                Some("paused behavior")
            } else {
                None
            };
            let paused = pause_reason.is_some();
            let status = self.paint_counters.get(&entity).map_or(0, |c| c.status());
            let dead = self
                .runtime_health
                .get(&entity)
                .is_some_and(|h| h.health <= 0)
                || status == 2;
            let source = pose.position;
            if !self.demo_story.ai.home.contains_key(&entity) {
                let radius = if self.ai_brains[&entity].behaviors.iter().any(|name| name == "RunHome") {
                    self.entities[entity].components.iter().find(|c| c.class_name == "AIBrainComponent")
                        .and_then(|c| c.properties.iter().find(|p| p.name == "Tether Radius"))
                        .and_then(|p| p.value.as_f64()).unwrap_or(0.) as f32
                } else {
                    0.
                };
                self.demo_story.ai.home.insert(entity, (source, radius));
            }
            self.publish_ai_actor_position(entity)?;
            let navigation_position=self.npc_relocations.get(&entity).and_then(|state|state.navigation_position);
            let navigation_floor=self.npc_relocations.get(&entity).and_then(|state|state.navigation_floor);
            let player = self.player_entity;
            let player_position: [f32; 3] = self.entities[player].transform[12..15].try_into()?;
            let player_dx = player_position[0] - source[0];
            let player_dz = player_position[2] - source[2];
            let player_distance = player_dx.hypot(player_dz);
            let rotation = self.entities[entity].runtime_rotation;
            let facing = [rotation[2], rotation[8]];
            let ai_type = self.ai_brains[&entity].ai_type.as_str();
            let stationary = ai_type.eq_ignore_ascii_case("KamikazeBlotlingStationary");
            let profiles = &mut self.demo_story.ai.perception_profiles;
            if !profiles.contains_key(ai_type) {
                let sensor = if stationary { "SeePlayer" } else { "SeeEnemy" };
                profiles.insert(ai_type.to_owned(), Perception::from_settings(&self.runtime_settings, sensor, ai_type));
            }
            let profile = profiles[ai_type];
            let dot = (facing[0] * player_dx + facing[1] * player_dz) / player_distance.max(0.001);
            let close = player_distance <= 1.5;
            let in_range = (player_position[1] - source[1]).abs() < 4.
                && (close || profile.cones.iter().any(|&(range, cosine)| player_distance <= range && dot >= cosine));
            let previous_target = self.ai_brains[&entity].combat_target;
            let (home, tether) = self.demo_story.ai.home[&entity];
            let distance_to_home = (source[0] - home[0]).hypot(source[2] - home[2]);
            let player_outside_tether = tether > 0.
                && (player_position[0] - home[0]).hypot(player_position[2] - home[2]) > tether;
            let returning = tether > 0. && status != 1 && distance_to_home > 0.5
                && (distance_to_home > tether || player_outside_tether && previous_target.is_none());
            let can_attack = status != 1 && !dead && !paused && !returning && !player_outside_tether
                && self.music_team(entity).is_none_or(|(team, _)| team == 2);
            let sees_player = if can_attack && in_range {
                match self.demo_story.ai.sight_checks.get(&entity) {
                    Some(&(clear, next)) if next > self.animation_time => clear,
                    _ => {
                        let clear = self.demo_melee_clear(shape, source, player_position, player)?;
                        self.demo_story.ai.sight_checks.insert(entity, (clear, self.animation_time + 0.12));
                        clear
                    }
                }
            } else {
                self.demo_story.ai.sight_checks.remove(&entity);
                false
            };
            if sees_player {
                self.demo_story.ai.last_seen.insert(entity, (player_position, self.animation_time + profile.memory));
            } else if can_attack {
                if let Some((position, time)) = self.demo_story.ai.player_footstep {
                    let expiry = time + profile.hearing_memory;
                    let audible = super::physics::length(super::physics::sub(position, source)) <= profile.hearing_range;
                    let newer = self.demo_story.ai.last_seen.get(&entity).is_none_or(|&(_, previous)| expiry > previous);
                    if audible && newer && self.animation_time < time + 0.2 && self.animation_time < expiry {
                        self.demo_story.ai.last_seen.insert(entity, (position, expiry));
                    }
                }
            }
            let remembered = self.demo_story.ai.last_seen.get(&entity).map(|&(position, _)| position);
            let hostile = can_attack && (remembered.is_some()
                || previous_target == Some(player) && self.demo_story.ai.attacking.contains_key(&entity));
            self.ai_brains.get_mut(&entity).unwrap().combat_target = hostile.then_some(player);
            if hostile && previous_target != Some(player) {
                self.emit_entity_output(entity, "OnPlayerSighted", player);
                self.emit_entity_output(entity, "OnEnterCombat", player);
                self.prepare_cannon_weapons(entity)?;
            } else if !hostile && previous_target == Some(player) && !exposed {
                self.emit_entity_output(entity, "OnExitCombat", player);
            }
            // Retain the patrol node so the actor can resume it after losing
            // the target. A current patrol must not suppress acquisition.
            let patrol_target = (!hostile).then(|| self.demo_story.ai.patrol.get(&entity).copied()).flatten()
                .filter(|&id| id < self.entities.len() && !self.removed_entities.contains(&id));
            let target = patrol_target.unwrap_or(player);
            let destination: [f32; 3] = if hostile {
                remembered.unwrap_or(player_position)
            } else if returning && patrol_target.is_none() {
                home
            } else {
                self.entities[target].transform[12..15].try_into()?
            };
            let dx = destination[0] - source[0];
            let dz = destination[2] - source[2];
            let distance = dx.hypot(dz);
            let ai_type = self.ai_brains[&entity].ai_type.to_ascii_lowercase();
            let family = ai_type.split([':', '_']).next().unwrap_or(&ai_type);
            let melee_spatter = family == "spattermelee";
            let stationary = stationary || family == "kamikazeblotlingstationary";
            let spatter = family.starts_with("spatter");
            let kamikaze = ai_type.contains("kamikaze")
                && self.entities[entity].components.iter().any(|c| c.class_name == "ExplodeComponent")
                && self.authored_havok.has_event(entity, "EVENT_Start_Explode");
            if kamikaze && sees_player && distance <= kamikaze_reach
                && (destination[1] - source[1]).abs() < 1.5
                && !self.demo_story.ai.detonations.contains_key(&entity)
                && self.demo_melee_clear(shape, source, destination, target)?
            {
                self.authored_havok.send_event(entity, "EVENT_Start_Explode")?;
                self.demo_story.ai.detonations.insert(entity, false);
            }
            if let Some(&done) = self.demo_story.ai.detonations.get(&entity) {
                if !done && self.authored_havok.active_state_clip_finished(entity, "Explode_Default") {
                    self.finish_demo_detonation(entity)?;
                }
                self.collision.set_simulation_velocity(shape, [0., pose.linear[1], 0.])?;
                continue;
            }
            let follows_mickey = status == 1 && !stationary && patrol_target.is_none()
                && spatter && player_distance < 18.;
            let attack_expired = self.demo_story.ai.attacking.get(&entity)
                .is_some_and(|attack| self.animation_time >= attack.abort_at);
            if paused || dead || !hostile || attack_expired {
                if self.demo_story.ai.attacking.remove(&entity).is_some()
                    && !dead && !exposed && self.authored_havok.contains(entity)
                {
                    self.authored_havok.send_event(entity, "EVENT_ForceNormalIdle")?;
                }
            }
            let mut charge_motion = None;
            if let Some(attack) = self.demo_story.ai.attacking.get_mut(&entity) {
                if let Some(charge) = &mut attack.charge {
                    if attack.phase == AttackPhase::Windup
                        && ["ChargeAttack_WindupIdle", "ChargeAttack_WindupIdle00"].iter().any(|name|
                            self.authored_havok.active_state_time(entity, name)
                                .is_some_and(|time| time >= attack.windup_idle))
                    {
                        charge.direction = [dx / distance.max(0.001), dz / distance.max(0.001)];
                        charge.origin = source;
                        charge.started = self.animation_time;
                        attack.phase = AttackPhase::Striking;
                        self.authored_havok.send_event(entity, "EVENT_Finish_AttackChargeWindup")?;
                    }
                    if attack.phase == AttackPhase::Striking {
                        if !attack.contact_open {
                            attack.contact_open = self.authored_havok
                                .active_state_time(entity, "ChargeAttack_Attack").is_some()
                                || self.authored_havok.active_state_time(entity, "ChargeAttack_Spin00").is_some();
                            self.demo_story.ai.charge_rolls += u64::from(attack.contact_open);
                        }
                        if attack.contact_open {
                            let progress = (source[0] - charge.origin[0]) * charge.direction[0]
                                + (source[2] - charge.origin[2]) * charge.direction[1];
                            if progress - charge.progress < 0.01 {
                                charge.stopped += delta;
                            } else {
                                charge.stopped = 0.;
                            }
                            charge.progress = progress;
                            if attack.hit || progress >= charge.distance || charge.stopped > charge.blocked_time {
                                attack.phase = AttackPhase::Recovery;
                                self.demo_story.ai.charge_recoveries += 1;
                                attack.contact_open = false;
                                self.authored_havok.send_event(entity, "EVENT_Finish_AttackCharge")?;
                            } else {
                                let elapsed = self.animation_time - charge.started;
                                let amount = if charge.ramp_time > 0. { (elapsed / charge.ramp_time).min(1.) } else { 1. };
                                let speed = charge.move_speed + (charge.top_speed - charge.move_speed) * amount;
                                charge_motion = Some((charge.direction, speed));
                            }
                        }
                    }
                }
            }
            let attacking = self.demo_story.ai.attacking.contains_key(&entity);
            let walk_attack = family == "beetleworxcrawler"
                && self.authored_havok.has_event(entity, "EVENT_WalkAttack_Start");
            if self.advance_story_boss(
                entity,
                paused,
                dead,
                shape,
                source,
                player_position,
                delta,
            )? {
                continue;
            }
            if self.advance_blob_vacuum(entity, sees_player && !attacking, paused || dead,
                shape, source, player_position, facing)?
            {
                self.collision.set_simulation_velocity(shape, [0., pose.linear[1], 0.])?;
                self.authored_havok.write_native_variable(entity, "VAR_Forward_Speed", 0)?;
                continue;
            }
            let mut selected = None;
            if sees_player && !attacking
                && self.authored_havok.contains(entity) && self.prepare_child_slots(entity)?
            {
                let reach = if melee_spatter { melee_radius } else { f32::INFINITY };
                if (destination[1] - source[1]).abs() < 1.5
                    && (walk_attack || self.authored_havok.has_event(entity, "EVENT_Start_AttackMelee"))
                {
                    selected = self.demo_weapon(entity, distance, reach, WeaponKind::Melee).map(|v| (v, WeaponKind::Melee));
                }
                if selected.is_none() && self.authored_havok.has_event(entity, "EVENT_Start_AttackRanged") {
                    selected = self.demo_weapon(entity, distance, f32::INFINITY, WeaponKind::Ranged).map(|v| (v, WeaponKind::Ranged));
                }
                if selected.is_none() && distance > 0.001 && (destination[1] - source[1]).abs() < 1.5
                    && self.authored_havok.has_event(entity, "EVENT_Start_AttackCharge")
                    && self.authored_havok.has_event(entity, "EVENT_Finish_AttackChargeWindup")
                {
                    selected = self.demo_weapon(entity, distance, f32::INFINITY, WeaponKind::Charge)
                        .map(|v| (v, WeaponKind::Charge));
                }
            }
            let target_heading = (distance > 0.001).then(|| [dx / distance, dz / distance]);
            let facing_target = target_heading.is_some_and(|heading| {
                facing[0] * heading[0] + facing[1] * heading[1] >= 0.9
            });
            let wants_move = !paused && (!stationary || patrol_target.is_some())
                && self.npc_relocations.get(&entity).is_none_or(|s|s.input_flags&2!=0)
                && !dead
                && !attacking
                && selected.is_none()
                // Perception limits belong to this provisional combat planner,
                // not an explicit authored patrol destination. Applying them to
                // both silently discards real routes (the airlock requests29m).
                && (patrol_target.is_some() || hostile || follows_mickey || returning)
                && distance > if follows_mickey { friendly_distance } else if !hostile { 0.25 }
                    else if melee_spatter { 0.55 } else { 1.3 };
            let direction = if let Some((direction, _)) = charge_motion {
                direction
            } else if wants_move {
                self.demo_steer(entity, shape, source, destination, false)?
            } else {
                [0.; 2]
            };
            self.demo_story.ai.decisions.insert(
                entity,
                Decision {
                    pause_reason,
                    dead,
                    hostile,
                    in_range,
                    movement_requested: wants_move || charge_motion.is_some(),
                    scripted_patrol: patrol_target.is_some(),
                    target,
                    distance,
                    source,
                    destination,
                    direction,
                    navigation_floor,
                    navigation_position,
                },
            );
            let follows_route = wants_move && direction != [0.; 2];
            let desired = if follows_route || charge_motion.is_some() {
                Some(direction)
            } else if selected.is_some() || self.demo_story.ai.attacking.get(&entity)
                .is_some_and(|attack| attack.phase == AttackPhase::Windup)
            {
                target_heading
            } else {
                None
            };
            let requested_speed = if let Some((_, speed)) = charge_motion {
                speed
            } else if follows_route {
                let patrol_speed = patrol_target.and_then(|_| {
                    self.entities[entity].components.iter()
                        .find(|component| component.class_name == "BasicAIControllerComponent")?
                        .properties.iter().find(|property| property.name == "PatrolSpeed")?
                        .value.as_f64().map(|speed| speed as f32)
                });
                patrol_speed.unwrap_or_else(|| {
                    if kamikaze && hostile { kamikaze_speed }
                    else if follows_mickey { melee_walk.min((distance - friendly_distance) * 2.) }
                    else if melee_spatter {
                        if hostile { melee_chase } else { melee_walk }
                    } else if distance > 3. { 3. } else { 2. }
                })
            } else {
                0.
            };
            // Keep velocity on the validated path. Slow down when the body
            // needs to catch up to a new heading, so a reversal doesn't slide
            // the enemy backwards at its full chase speed.
            let speed = if hostile && follows_route {
                let alignment = facing[0] * direction[0] + facing[1] * direction[1];
                let projected = (pose.linear[0] * direction[0] + pose.linear[2] * direction[1]).max(0.);
                let acceleration = self.entities[entity].components.iter()
                    .find(|c| c.class_name == "BasicMotionControllerComponent")
                    .and_then(|c| c.properties.iter().find(|p| p.name == "GroundControlAccel"))
                    .and_then(|p| p.value.as_f64())
                    .filter(|v| *v > 0. && v.is_finite())
                    .map_or(16., |v| v as f32);
                let corner_speed = requested_speed * ((alignment - 0.5) * 2.).clamp(0., 1.);
                (projected + acceleration * delta).min(corner_speed)
            } else {
                requested_speed
            };
            let moving = speed > 0.05;
            // The planner below remains a demo adapter. Its requests now
            // reach the real BasicAI input setters instead of leaving that
            // controller at its constructor's zero-speed input forever.
            if self.entities[entity].components.iter().any(|c|c.class_name=="BasicAIControllerComponent") {
                if let Some(heading) = desired {
                    self.publish_npc_ai_facing(entity, [heading[0], 0., heading[1]])?;
                }
                self.set_npc_ai_speed(entity,false,speed)?;
            } else {
                let limitation="Demo AI without a BasicAI controller retains provisional movement; its native controller output is not implemented.";
                if !self.report.unsupported.iter().any(|line|line==limitation) {self.report.unsupported.push(limitation.into());}
            }
            if self.basic_motion_suspended(entity) != Some(true) {
                self.collision.set_simulation_velocity(
                    shape,
                    [direction[0] * speed, pose.linear[1], direction[1] * speed],
                )?;
            }
            let mut turn_speed = 0.;
            if moving {
                self.demo_story.ai.movement_updates += 1;
            }
            if let Some(desired) = desired {
                // Runtime NiMatrix3 below maps the model's +Z forward axis
                // to [-sin(yaw),0,cos(yaw)]. The old angle aimed -Z along
                // the velocity, so characters chased their target backwards.
                let yaw = (-desired[0]).atan2(desired[1]);
                let current = (-facing[0]).atan2(facing[1]);
                // This planner's intent remains provisional; native history
                // selection is separate. Consume the real initialized cache
                // and shared angular limiter instead of a fixed4.5 rate.
                let rates=&self.npc_relocations.get(&entity).context("NPC motion record missing")?.turn_rates;
                let rate=f32::from_bits(if moving {rates.moving}else{rates.stationary}
                    .context("NPC turn-rate cache is uninitialized")?);
                anyhow::ensure!(rate.is_finite() && rate>=0.,"NPC angular solver requires validation for negative/nonfinite turn rates");
                let step=super::physics::facing_delta(current,yaw,rate,delta);
                turn_speed = step / delta.max(0.00001);
                if step.abs() > 0.0001 {
                    let (s, c) = (current + step).sin_cos();
                    let transform = super::native_hk_transform::from_entity(
                        [c, 0., -s, 0., 1., 0., s, 0., c],
                        source,
                    );
                    self.warp_character_body(entity, transform)?;
                }
            }
            if self.authored_havok.contains(entity) {
                let run_reference = if melee_spatter { melee_chase } else { 3. };
                let normalized_speed = (speed / run_reference.max(f32::EPSILON)).clamp(0., 1.);
                for (name, value) in [
                    ("VAR_Normalized_Forward_Speed", normalized_speed),
                    ("VAR_Forward_Speed", speed),
                    ("VAR_Scaled_Run_Speed", speed / if melee_spatter { 5. } else { 3. }),
                    ("VAR_Scaled_Walk_Speed", speed),
                    ("VAR_Turn_Speed", turn_speed),
                    ("VAR_Attack_Speed", 1.),
                ] {
                    self.authored_havok
                        .write_native_variable(entity, name, value.to_bits())?;
                }
                self.authored_havok.write_native_variable(
                    entity,
                    "VAR_Run_Type",
                    u32::from(hostile),
                )?;
                let previous = self.demo_story.ai.moving.insert(entity, moving);
                if previous != Some(moving) && charge_motion.is_none() {
                    // Graphs filter absent events; this adapter does not inject
                    // unknown native event IDs or fabricate feedback callbacks.
                    let event = if moving {
                        "EVENT_Start_Walking_On_Ground"
                    } else {
                        "EVENT_Stop_Walking_On_Ground"
                    };
                    let _ = self.authored_havok.send_event(entity, event);
                }
            }
            if let Some(attack) = self.demo_story.ai.attacking.get_mut(&entity) {
                if attack.charge.is_none() && attack.ranged_target.is_none()
                    && matches!(attack.phase, AttackPhase::AwaitStrike | AttackPhase::Striking)
                    && (attack.hit || self.authored_havok.active_state_clip_finished(entity,
                        if attack.walk_attack { "WalkAttack_Strike" } else { "MeleeAttack_Attack" }))
                {
                    // FinishPrep can be authored at time zero. The weapon
                    // opens later, so resolve from contact or the clip end.
                    attack.phase = AttackPhase::Recovery;
                    attack.contact_open = false;
                    let event = if attack.hit {
                        "EVENT_Finish_AttackMeleeSuccess"
                    } else {
                        "EVENT_Finish_AttackMeleeFail"
                    };
                    self.authored_havok.send_event(entity, event)?;
                } else if attack.phase == AttackPhase::Windup
                    && (attack.windup_end.is_some_and(|end| self.animation_time >= end)
                        || attack.charge.is_none() && attack.ranged_target.is_none()
                            && self.authored_havok.active_state_time(entity, "MeleeAttack_Windup_Idle00")
                                .is_some_and(|time| time >= attack.windup_idle))
                {
                    attack.phase = AttackPhase::AwaitStrike;
                    let event = if attack.ranged_target.is_some() {
                        "EVENT_Finish_AttackRangedWindup"
                    } else {
                        "EVENT_Finish_AttackMeleeWindup"
                    };
                    self.authored_havok.send_event(entity, event)?;
                }
            } else if let Some(((weapon, windup), kind)) = selected {
                if !facing_target || self.animation_time
                    < self
                        .demo_story
                        .ai
                        .attack_at
                        .get(&entity)
                        .copied()
                        .unwrap_or(0.)
                {
                    continue;
                }
                if !self.demo_melee_clear(shape, source, destination, target)? {
                    continue;
                }
                let uses_slot = self.uses_attack_slot(entity);
                let ranged = kind == WeaponKind::Ranged;
                if uses_slot && self.demo_story.ai.attacking.values()
                    .filter(|attack| attack.uses_slot && attack.ranged_target.is_some() == ranged)
                    .count() >= if ranged { ranged_slots } else { melee_slots }
                {
                    continue;
                }
                let weapon_component = self.entities[weapon].components.iter_mut()
                    .find(|component| component.class_name == "WeaponComponent")
                    .context("Selected attack has no weapon component")?;
                let variant = weapon_component.properties.iter()
                    .find(|p| p.name == "WeaponAnimationVariable")
                    .and_then(|p| p.value.as_i64()).unwrap_or(0);
                let attack_cooldown = weapon_component.properties.iter()
                    .find(|p| p.name == "WeaponMinRefireTime")
                    .and_then(|p| p.value.as_f64()).unwrap_or(0.) as f32;
                if let Some(shots) = weapon_component.properties.iter_mut().find(|p| p.name == "WeaponShots") {
                    if let Some(remaining) = shots.value.as_i64().filter(|&remaining| remaining > 0) {
                        shots.value = serde_json::json!(remaining - 1);
                    }
                }
                self.authored_havok.write_native_variable(entity, "VAR_Attack_Type", variant as u32)?;
                self.demo_story.ai.attack_at.insert(entity, self.animation_time + attack_cooldown.max(1.6));
                let (charge, windup) = if kind == WeaponKind::Charge {
                    let (charge, windup) = match self.seer_charge(weapon, windup) {
                        Ok(values) => values,
                        Err(error) => {
                            let reason = format!("Charge weapon {}: {error:#}", self.entities[weapon].name);
                            if !self.report.unsupported.contains(&reason) {
                                self.report.unsupported.push(reason);
                            }
                            continue;
                        }
                    };
                    (Some(charge), windup)
                } else {
                    (None, windup.max(0.))
                };
                let walk_attack = kind == WeaponKind::Melee && walk_attack;
                self.demo_story.ai.attacking.insert(
                    entity,
                    Attack {
                        windup_end: None,
                        windup_idle: windup,
                        weapon,
                        ranged_target: (kind == WeaponKind::Ranged).then_some(target),
                        charge,
                        ranged_aim: None,
                        uses_slot,
                        walk_attack,
                        abort_at: self.animation_time + 8. + windup,
                        phase: if walk_attack { AttackPhase::AwaitStrike } else { AttackPhase::Windup },
                        contact_open: false,
                        hit: false,
                    },
                );
                let event = match kind {
                    WeaponKind::Melee if walk_attack => "EVENT_WalkAttack_Start",
                    WeaponKind::Melee => "EVENT_Start_AttackMelee",
                    WeaponKind::Ranged => "EVENT_Start_AttackRanged",
                    WeaponKind::Charge => {
                        self.demo_story.ai.charge_windups += 1;
                        "EVENT_Start_AttackCharge"
                    }
                };
                self.authored_havok.send_event(entity, event)?;
            }
        }
        Ok(())
    }
}
