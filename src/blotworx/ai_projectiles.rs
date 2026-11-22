//! Runtime bridge for the authored mode-4 ShooterComponent projectile.
use super::{
    assets::World,
    physics::{self, SimulationContact, V},
    projectile_stimulus::{Contact, Delivery, Emitter, Stimuli},
};
use anyhow::{ensure, Context, Result};
use epic_mickey_lib_rs::scene_file::Component;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct State {
    releases: Vec<(usize, usize, V)>,
    flights: BTreeMap<usize, Flight>,
    draining: Vec<usize>,
    launched: u64,
    collided: u64,
    player_hits: u64,
    last_error: Option<String>,
}
struct Flight {
    entity: usize,
    shooter: usize,
    launch_velocity: V,
    contact: Option<usize>,
}
impl State {
    pub(super) fn queue(&mut self, actor: usize, weapon: usize, aim: V) {
        self.releases.push((actor, weapon, aim));
    }
    pub(super) fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "launched": self.launched,
            "collided": self.collided,
            "player_hits": self.player_hits,
            "in_flight": self.flights.len(),
            "last_error": self.last_error,
        })
    }
}

fn field<'a>(component: &'a Component, name: &str) -> Result<&'a serde_json::Value> {
    component
        .properties
        .iter()
        .find(|p| p.name == name)
        .map(|p| &p.value)
        .with_context(|| format!("{} lacks {name}", component.class_name))
}

fn launch_velocity(origin: V, target: V, speed: f32, gravity: f32) -> Result<V> {
    let [x, y, z] = physics::sub(target, origin);
    ensure!(
        speed.is_finite() && gravity.is_finite() && [x, y, z].iter().all(|v| v.is_finite()),
        "Nonfinite mode-4 trajectory input"
    );
    // REL39191c uses the horizontal distance, a single narrowed reciprocal
    // square-root refinement, and the world's signed vertical gravity.
    let square = x * x + z * z;
    if square == 0. {
        return Ok([0.; 3]);
    }
    let estimate = super::pose_blend::estimate_double(square) as f32;
    let refined = (-(estimate * estimate)).mul_add(square, 3.) * (0.5 * estimate);
    let distance = square * refined;
    let time = if speed.abs() < f32::EPSILON {
        0.
    } else {
        distance / speed
    };
    let vertical = if time.abs() < f32::EPSILON {
        0.
    } else {
        time.mul_add((0.5 * -gravity) * time, y) / time
    };
    let horizontal = if distance.abs() < f32::EPSILON {
        0.
    } else {
        speed / distance
    };
    Ok([x * horizontal, vertical, z * horizontal])
}

impl World {
    pub(super) fn launch_ai_projectiles(&mut self) {
        for (actor, weapon, aim) in std::mem::take(&mut self.demo_story.projectiles.releases) {
            if let Err(error) = self.launch_ai_projectile(actor, weapon, aim) {
                let message = format!("AI projectile {}: {error:#}", self.entities[actor].name);
                self.demo_story.projectiles.last_error = Some(message.clone());
                if !self.report.unsupported.contains(&message) {
                    self.report.unsupported.push(message);
                }
            }
        }
    }

    fn launch_ai_projectile(&mut self, actor: usize, weapon: usize, aim: V) -> Result<()> {
        if !self.entity_scene_active(actor)
            || self.hidden_entities.contains(&actor)
            || self.removed_entities.contains(&weapon)
        {
            return Ok(());
        }
        let muzzle = &self.entities[weapon];
        let (shooter_index, shooter) = muzzle
            .components
            .iter()
            .enumerate()
            .find(|(_, c)| c.class_name == "ShooterComponent")
            .context("Missing ShooterComponent")?;
        if self
            .component_control
            .disabled
            .get(&weapon)
            .and_then(|flags| flags.get(&shooter_index))
            .copied()
            .unwrap_or_else(|| {
                field(shooter, "Shooter Start Disabled")
                    .ok()
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
            })
        {
            return Ok(());
        }
        let method = field(shooter, "Shooter Delivery Method")
            .ok()
            .and_then(|v| v.as_u64())
            .unwrap_or(2);
        let speed = field(shooter, "Shooter Speed")?
            .as_f64()
            .context("Invalid shooter speed")? as f32;
        let indexes = field(shooter, "Projectile Spawn Index")?
            .as_array()
            .context("Invalid projectile slots")?;
        let spawner = muzzle
            .components
            .iter()
            .find(|c| c.class_name == "TemplateSpawnerComponent")
            .context("Missing projectile template spawner")?;
        let templates = field(spawner, "Templates To Spawn")?
            .as_array()
            .context("Invalid projectile templates")?;
        let names = indexes
            .iter()
            .map(|value| {
                let index =
                    usize::try_from(value.as_u64().context("Invalid projectile slot index")?)?;
                Ok(templates
                    .get(index)
                    .and_then(serde_json::Value::as_str)
                    .context("Projectile slot has no template")?
                    .to_owned())
            })
            .collect::<Result<Vec<_>>>()?;
        let origin = muzzle.transform[12..15].try_into()?;
        let rotation = muzzle.runtime_rotation;
        let use_gravity = matches!(method, 3 | 4 | 5 | 6);
        let velocity = if use_gravity {
            launch_velocity(origin, aim, speed.max(2.0), self.report.physics_world.gravity[1])?
        } else {
            let delta = physics::sub(aim, origin);
            let distance = physics::length(delta);
            ensure!(speed.is_finite() && distance.is_finite(), "Invalid direct shot");
            if distance <= f32::EPSILON { return Ok(()); }
            physics::mul(delta, speed / distance)
        };
        for name in names {
            let entity = self.spawn_palette_root(weapon, &name, origin, rotation, "")?;
            let result = (|| -> Result<()> {
                // REL 0x390c70 -> 0x38e794 -> 0x3831c8(dynamic=1): activate the
                // spawned projectile's rigid body before publishing velocity so
                // keyframed templates actually launch instead of hovering in mid-air.
                self.set_entity_motion_enabled(entity, true)?;
                let (&source, &owner) = self
                    .dynamic_runtime_sources
                    .iter()
                    .find(|(source, _)| self.dynamic_bodies[**source].entity == entity)
                    .context("Projectile body was not admitted")?;
                let construction = self
                    .dynamic_constructions
                    .get_mut(&source)
                    .context("Missing projectile body construction")?;
                let body = construction
                    .bodies
                    .first_mut()
                    .context("Projectile body missing")?;
                ensure!(
                    matches!(body.motion.current.motion_type(), 2 | 3 | 6 | 7),
                    "Projectile body stayed non-dynamic (motion_type={}) after activation",
                    body.motion.current.motion_type()
                );
                let shape = *body
                    .shapes
                    .first()
                    .context("Projectile body has no collision shape")?;
                if use_gravity {
                    body.motion.current.initialize_motion_tuning(|name| {
                        Ok(if name == "Gravity Factor" { 1. } else { 0. })
                    })?;
                }
                self.collision.set_simulation_body(
                    &body.shapes,
                    &body.motion.current,
                    construction.materials[0],
                    true,
                )?;
                self.collision.set_simulation_velocity(shape, velocity)?;
                self.demo_story.projectiles.flights.insert(
                    owner,
                    Flight {
                        entity,
                        shooter: actor,
                        launch_velocity: velocity,
                        contact: None,
                    },
                );
                self.demo_story.projectiles.launched += 1;
                self.emit_entity_output(entity, "OnProjectileLaunched", actor);
                Ok(())
            })();
            if let Err(error) = result {
                self.queue_entity_retirement(entity)?;
                return Err(error);
            }
        }
        Ok(())
    }

    pub(super) fn record_ai_projectile_contact(&mut self, contact: &SimulationContact) {
        for side in 0..2 {
            let owner = contact.owners[side];
            let other = contact.owners[1 - side];
            let Some(flight) = self.demo_story.projectiles.flights.get(&owner) else {
                continue;
            };
            if flight.contact.is_none()
                && self.collision_entity_index(other) != Some(flight.shooter)
            {
                self.demo_story
                    .projectiles
                    .flights
                    .get_mut(&owner)
                    .unwrap()
                    .contact = Some(other);
            }
        }
    }

    pub(super) fn advance_ai_projectile_impacts(&mut self) -> Result<()> {
        let mut state = std::mem::take(&mut self.demo_story.projectiles);
        let result = (|| {
            let mut still_draining = Vec::new();
            for entity in state.draining.drain(..) {
                if self.outputs.has_pending_for_owner(entity) && self.entity_scene_active(entity) {
                    still_draining.push(entity);
                } else {
                    self.queue_entity_retirement(entity)?;
                }
            }
            state.draining = still_draining;
            let settled = state
                .flights
                .iter()
                .filter_map(|(&owner, flight)| {
                    (flight.contact.is_some()
                        || !self.entity_scene_active(flight.entity)
                        || self.removed_entities.contains(&flight.entity))
                    .then_some(owner)
                })
                .collect::<Vec<_>>();
            for owner in settled {
                let flight = state.flights.remove(&owner).unwrap();
                if self.removed_entities.contains(&flight.entity) {
                    continue;
                }
                if !self.entity_scene_active(flight.entity) {
                    self.queue_entity_retirement(flight.entity)?;
                    continue;
                }
                let other = flight
                    .contact
                    .and_then(|other| self.collision_entity_index(other));
                let hit_player = self.dispatch_ai_projectile_impact(&flight, other)?;
                self.collision.set_owner_in_world(owner, false)?;
                self.emit_entity_output(
                    flight.entity,
                    "OnProjectileCollided",
                    other.unwrap_or(flight.shooter),
                );
                state.draining.push(flight.entity);
                state.collided += 1;
                state.player_hits += u64::from(hit_player);
            }
            Ok(())
        })();
        self.demo_story.projectiles = state;
        result
    }

    fn dispatch_ai_projectile_impact(
        &mut self,
        flight: &Flight,
        target: Option<usize>,
    ) -> Result<bool> {
        let entity = &self.entities[flight.entity];
        let projectile = entity
            .components
            .iter()
            .find(|c| c.class_name == "ProjectileComponent")
            .context("Projectile contact lacks ProjectileComponent")?;
        let properties = projectile
            .properties
            .iter()
            .map(|p| serde_json::json!({"name": p.name, "value": p.value}))
            .collect::<Vec<_>>();
        let position = entity.transform[12..15].try_into()?;
        let body = target.and_then(|target| self.entities[target].projectile_target_body());
        let contact = Contact::from_body(
            position,
            flight.launch_velocity,
            target.map(|v| v as u64),
            body,
        );
        let mut emitter = Emitter {
            stimuli: Stimuli::load(&properties)?,
            amount_scale: 1.,
            projectile: flight.entity as u64,
            owner: Some(flight.shooter as u64),
            tag: entity.name.clone(),
        };
        let mut player_hit = false;
        emitter.dispatch(contact, false, |_, emission| {
            let recipients = match emission.delivery {
                Delivery::Target | Delivery::Area { radius: 0., .. } => {
                    target.into_iter().collect()
                }
                Delivery::Area { radius, .. } => {
                    self.area_stimulus_recipients(position, radius, Some(flight.shooter))?
                }
            };
            for recipient in recipients
                .into_iter()
                .filter(|&recipient| recipient != flight.entity)
            {
                self.receive_combat(recipient, std::slice::from_ref(&emission))?;
                player_hit |= recipient == self.player_entity;
            }
            Ok(())
        })?;
        Ok(player_hit)
    }
}
