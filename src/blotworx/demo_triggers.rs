//! Demo overlap production for authored triggers and pickups. Bounds-based
//! volume queries are intentionally separate from native Havok parity.
use super::assets::{RuntimeEntity, World};
use anyhow::Result;
use std::collections::BTreeMap;
#[derive(Default)]
pub struct State {
    visits: BTreeMap<(usize, usize), Visit>,
    // Track the player's last tick independently of component activation.
    // Retail 3cedec reads the tracked body's motion segment; a plane does
    // not have to have observed the previous tick to detect a crossing.
    previous_player_position: Option<[f32; 3]>,
    pending: Vec<(f32, usize, String)>,
    pub entries: u64,
    pub pickups: u64,
}
#[derive(Default)]
struct Visit {
    inside: bool,
    previous: Option<[f32; 3]>,
    count: u32,
}
impl State {
    pub(super) fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "previous_player_position":self.previous_player_position,
            "visits":self.visits.iter().map(|(&(entity,component),visit)|
                serde_json::json!({"entity":entity,"component":component,
                    "inside":visit.inside,"previous_local":visit.previous,"count":visit.count}))
                .collect::<Vec<_>>()
        })
    }
}
fn value<'a>(e: &'a RuntimeEntity, class: &str, name: &str) -> Option<&'a serde_json::Value> {
    e.components
        .iter()
        .find(|c| c.class_name == class)?
        .properties
        .iter()
        .find(|p| p.name == name)
        .map(|p| &p.value)
}
fn local(rotation: [f32; 9], origin: [f32; 3], point: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|col| {
        (0..3)
            .map(|row| rotation[row * 3 + col] * (point[row] - origin[row]))
            .sum()
    })
}
fn plane_cross(previous: [f32; 3], current: [f32; 3], size: [f32; 2]) -> bool {
    let Some(fraction) = super::trigger_plane_segment::fraction(previous[2], current[2]) else {
        return false;
    };
    let crossing =
        std::array::from_fn::<_, 2, _>(|i| current[i] + (previous[i] - current[i]) * fraction);
    crossing[0].abs() <= size[0] * 0.5 + 0.3 && crossing[1].abs() <= size[1] * 0.5 + 0.6
}
// Retail REL3cf0d4..3cf144 sends one direction-specific event (69/6a).
// A receiver's list of connected outputs never changes the crossing direction.
fn trigger_output_matches(output: &str, entered: bool, plane: bool, forward: bool) -> bool {
    let name = output.to_ascii_lowercase();
    if entered {
        matches!(
            name.as_str(),
            "onenter"
                | "onentered"
                | "ontrigger"
                | "ontriggerenter"
                | "ontouch"
                | "onvolumeentered"
                | "onstarttouch"
        ) || plane
            && (matches!(name.as_str(), "oncross" | "oncrossed" | "onplanecrossed")
                || name
                    == if forward {
                        "ontriggerplaneforward"
                    } else {
                        "ontriggerplanebackward"
                    })
    } else {
        matches!(
            name.as_str(),
            "onexit"
                | "onexited"
                | "ontriggerexit"
                | "onleave"
                | "onvolumeexited"
                | "onendtouch"
                | "ontriggerempty"
        )
    }
}

impl World {
    /// Functional inventory dispatch for the demo. Imported defaults stay immutable;
    /// checkpoint/UI consumers read the same live InventoryComponent properties.
    pub(super) fn player_inventory_command(&mut self, command: &str) -> Result<bool> {
        let mut words = command
            .split('\0')
            .next()
            .unwrap_or("")
            .split(' ')
            .filter(|word| !word.is_empty());
        if words.next() != Some("Add") {
            return Ok(false);
        }
        let (Some(item), Some(amount)) = (words.next(), words.next()) else {
            return Ok(false);
        };
        let amount = super::config_value::decimal_float(amount).map_err(anyhow::Error::msg)?;
        self.player_inventory_add(item, amount)
    }
    pub(super) fn player_inventory_add(&mut self, item: &str, amount: f32) -> Result<bool> {
        let limitation="Inventory Add updates live balances; native global item/paint events, pickup messages and powerup UI remain unimplemented.";
        if !self
            .report
            .unsupported
            .iter()
            .any(|entry| entry == limitation)
        {
            self.report.unsupported.push(limitation.into());
        }
        let default_maximum = self
            .runtime_settings
            .get("SketchDefaultMax")
            .map(String::as_str)
            .unwrap_or("2.0");
        let Some(inventory) = self.entities[self.player_entity]
            .components
            .iter_mut()
            .find(|c| c.class_name == "InventoryComponent")
        else {
            return Ok(false);
        };
        if item
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("Sketch"))
        {
            let slot = inventory
                .properties
                .iter()
                .find(|p| p.name == "Sketch")
                .and_then(|p| p.value.as_array())
                .and_then(|a| a.iter().position(|v| v.as_str() == Some(item)));
            let Some(slot) = slot else {
                super::inventory_properties::add_new_sketch(
                    inventory,
                    item,
                    amount,
                    default_maximum,
                )?;
                return Ok(true);
            };
            let maximum = inventory
                .properties
                .iter()
                .find(|p| p.name == "SketchMax")
                .and_then(|p| p.value.get(slot))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.) as f32;
            let Some(count) = inventory
                .properties
                .iter_mut()
                .find(|p| p.name == "SketchCount")
                .and_then(|p| p.value.get_mut(slot))
            else {
                return Ok(false);
            };
            let previous = count.as_i64().unwrap_or(0) as i32;
            if previous as f32 >= maximum {
                return Ok(false);
            }
            // REL343c1c converts the single-precision sum, not the amount
            // before addition (observable with fractional maxima/counts).
            let maximum = super::inventory_commands::checked_fctiwz(maximum)?;
            let next =
                super::inventory_commands::checked_fctiwz(previous as f32 + amount)?.min(maximum);
            *count = serde_json::json!(next);
            return Ok(true);
        }
        let maximum = inventory
            .properties
            .iter()
            .find(|p| p.name == format!("{item}Max"))
            .and_then(|p| p.value.as_f64());
        let Some(property) = inventory
            .properties
            .iter_mut()
            .find(|p| p.name == item && p.class_name == "Float")
        else {
            return Ok(false);
        };
        let previous = match item {
            "Paint" => self
                .ammo
                .inventory
                .as_ref()
                .map(|a| f64::from(a.amounts[0])),
            "Thinner" => self
                .ammo
                .inventory
                .as_ref()
                .map(|a| f64::from(a.amounts[1])),
            _ => None,
        }
        .unwrap_or_else(|| property.value.as_f64().unwrap_or(0.));
        let next = super::inventory_commands::preview_scalar_balance(
            previous as f32,
            amount,
            maximum.unwrap_or(0.) as f32,
        );
        if next == previous as f32 {
            return Ok(false);
        }
        property.value = serde_json::json!(next);
        if item == "ETicket" && next > previous as f32 {
            let serial = self.ticket_hud_gain.map_or(1, |(serial, _, _)| serial.wrapping_add(1));
            self.ticket_hud_gain = Some((serial, previous as f32, next));
        } else if item == "ETicket" {
            self.ticket_hud_gain = None;
        }
        if let Some(ammo) = &mut self.ammo.inventory {
            if item == "Paint" {
                ammo.amounts[0] = next as f32;
            } else if item == "Thinner" {
                ammo.amounts[1] = next as f32;
            }
        }
        Ok(true)
    }
    fn demo_trigger_outputs(
        &self,
        entity: usize,
        entered: bool,
        plane: bool,
        forward: bool,
    ) -> Vec<String> {
        self.entities[entity]
            .components
            .iter()
            .filter(|c| c.class_name == "InputOutputComponent")
            .flat_map(|c| c.properties.iter())
            .filter(|p| p.name == "Output")
            .filter_map(|p| p.value.as_array())
            .flatten()
            .filter_map(|v| v.as_str())
            .filter(|output| trigger_output_matches(output, entered, plane, forward))
            .map(str::to_owned)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    pub(super) fn demo_contains(&self, entity: usize, position: [f32; 3]) -> Result<Option<bool>> {
        if let Some(hull) = self.rigid_trigger_hulls.get(&entity) {
            return hull.overlaps_player(self.entities[entity].physics_transform(), position)
                .map(Some);
        }
        let bounds = if let Some(bounds) = self.phantom_bounds(entity)? {
            Some(bounds)
        } else if self.resource_phantom_shapes.contains_key(&entity) {
            let aabb = self.resource_phantom_aabb(entity, 0.)?;
            Some(super::phantom_bounds::Bounds {
                min: aabb[0][..3].try_into()?,
                max: aabb[1][..3].try_into()?,
            })
        } else if let Some(component) = self.entities[entity].components.iter()
            .find(|component| {
                component.class_name == "PhysicsRigidBodyPhantomShapeComponent"
                    && component.properties.iter().any(|property| property.name == "Phantom Size")
                    && !component.properties.iter().any(|property| {
                        property.name == "RB Hull File Path"
                            && property.value.as_str().is_some_and(|path| !path.is_empty())
                    })
            })
        {
            let owner = &self.entities[entity];
            return super::phantom_bounds::SizeVolume::from_size_property(component)?
                .map(|volume| volume.overlaps_rigid_box(owner.physics_transform(), position))
                .transpose();
        } else {
            None
        };
        Ok(bounds.map(|bounds| bounds.overlaps_player(position)))
    }
    pub(super) fn suspend_inactive_trigger_overlaps(&mut self) {
        let entities = &self.entities;
        let resources = &self.scene_resources;
        for ((owner, _), visit) in &mut self.demo_story.triggers.visits {
            if entities
                .get(*owner)
                .is_none_or(|entity| !resources.is_active(&entity.scene))
            {
                // Do this at deactivation too: off/on in one update must not
                // sweep between positions across the inactive interval.
                visit.inside = false;
                visit.previous = None;
            }
        }
    }
    pub(super) fn advance_demo_triggers(&mut self, position: [f32; 3], delta: f32) -> Result<()> {
        if !self.demo_story.enabled {
            return Ok(());
        }
        self.suspend_inactive_trigger_overlaps();
        let mut state = std::mem::take(&mut self.demo_story.triggers);
        let previous_player_position = state.previous_player_position.replace(position);
        let result = (|| -> Result<()> {
            state
                .visits
                .retain(|(owner, _), _| !self.removed_entities.contains(owner));
            let pending = std::mem::take(&mut state.pending);
            for (due, entity, output) in pending {
                if self.removed_entities.contains(&entity) {
                    continue;
                }
                if !self.entity_scene_active(entity) {
                    // Hold delivery without fabricating native timer/unload
                    // semantics: due time remains on the existing game clock.
                    state.pending.push((due, entity, output));
                    continue;
                }
                if due <= self.animation_time {
                    if !self.removed_entities.contains(&entity) {
                        self.emit_entity_output(entity, &output, self.player_entity);
                    }
                } else {
                    state.pending.push((due, entity, output));
                }
            }
            for entity in 0..self.entities.len() {
                if self.removed_entities.contains(&entity) || !self.entity_scene_active(entity) {
                    continue;
                }
                let triggers = self.entities[entity]
                    .components
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| {
                        matches!(
                            c.class_name.as_str(),
                            "TriggerVolumeComponent"
                                | "TriggerPlaneComponent"
                                | "TriggerDamageComponent"
                        )
                    })
                    .map(|(i, c)| (i, c.class_name.clone()))
                    .collect::<Vec<_>>();
                for (index, class) in triggers {
                    let component_value = |name: &str| {
                        self.entities[entity].components[index]
                            .properties
                            .iter()
                            .find(|p| p.name == name)
                            .map(|p| &p.value)
                    };
                    let melee_enabled = (class == "TriggerDamageComponent"
                        && self.entities[entity].components.iter().any(|c| c.class_name == "WeaponComponent"))
                        .then(|| self.demo_melee_trigger_enabled(entity))
                        .flatten();
                    let disabled = melee_enabled.map(|enabled| !enabled).unwrap_or_else(|| self
                        .component_control
                        .disabled
                        .get(&entity)
                        .and_then(|v| v.get(&index))
                        .copied()
                        .unwrap_or_else(|| {
                            component_value("TVCStartDisabled")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false)
                        }));
                    if disabled {
                        if let Some(visit) = state.visits.get_mut(&(entity, index)) {
                            visit.inside = false;
                            visit.previous = None;
                        }
                        continue;
                    }
                    let e = &self.entities[entity];
                    if let Some(groups) = component_value("TrackedEntityGroups")
                        .and_then(|v| v.as_array())
                        .filter(|v| !v.is_empty())
                    {
                        let mut accepted = false;
                        for group in groups.iter().filter_map(|v| v.as_str()) {
                            if group.eq_ignore_ascii_case("Player")
                                || self
                                    .scripts
                                    .group_entity_names(group)?
                                    .iter()
                                    .any(|name| name == &self.entities[self.player_entity].name)
                            {
                                accepted = true;
                                break;
                            }
                        }
                        if !accepted {
                            continue;
                        }
                    }
                    if let Some(layers) = component_value("Collidable Collision Layers")
                        .and_then(|v| v.as_array())
                        .filter(|v| !v.is_empty())
                    {
                        let player_layer = self
                            .player_physics
                            .as_ref()
                            .and_then(|body| self.collision.simulation_filter(body.shape))
                            .map(|word| word & 31)
                            .or_else(|| {
                                value(
                                    &self.entities[self.player_entity],
                                    "PhysicsRigidBodyCharacterComponent",
                                    "Collision Layer",
                                )
                                .and_then(|v| v.as_u64())
                                .and_then(|v| u32::try_from(v).ok())
                            });
                        if let Some(layer) = player_layer {
                            let bit = if layer < 32 { 1u32 << layer } else { 0 };
                            let admitted_layer = layers
                                .iter()
                                .filter_map(|v| v.as_u64())
                                .filter_map(|v| u32::try_from(v).ok())
                                .any(|selector| super::trigger_damage::layer_mask(selector) & bit != 0);
                            if !admitted_layer {
                                continue;
                            }
                        }
                    }
                    let visit = state.visits.entry((entity, index)).or_default();
                    let plane = class == "TriggerPlaneComponent";
                    let midair_platform_jump = class == "TriggerVolumeComponent"
                        && self.script_player_available
                        && !self.script_player.grounded
                        && !self.script_player.stairs_ground_retained
                        && !self.script_player.ledge.is_active()
                        && (self.script_player.jump_stage.is_some()
                            || self
                                .script_player
                                .motion_ground
                                .as_ref()
                                .and_then(|g| g.distance)
                                .is_some_and(|d| d <= 4.5));
                    let (inside, entered, forward) = if plane {
                        let center = e.transform[12..15].try_into()?;
                        let current = local(
                            e.runtime_rotation,
                            center,
                            [position[0], position[1] + 0.6, position[2]],
                        );
                        let size = component_value("Trigger Plane Size");
                        let size = [
                            size.and_then(|s| s.get("x"))
                                .and_then(|v| v.as_f64())
                                .unwrap_or(5.) as f32,
                            size.and_then(|s| s.get("y"))
                                .and_then(|v| v.as_f64())
                                .unwrap_or(5.) as f32,
                        ];
                        // Both endpoints use this tick's plane frame. Retaining
                        // last tick's local coordinates also creates spurious
                        // crossings when the plane itself moves or rotates.
                        let previous = previous_player_position.map(|previous| {
                            local(
                                e.runtime_rotation,
                                center,
                                [previous[0], previous[1] + 0.6, previous[2]],
                            )
                        });
                        let crossed = previous.is_some_and(|previous| plane_cross(previous, current, size));
                        visit.previous = Some(current);
                        (false, crossed, previous.is_some_and(|previous|
                            super::trigger_plane_segment::forward(previous[2], current[2])))
                    } else if midair_platform_jump {
                        (visit.inside, false, true)
                    } else {
                        let inside = self.demo_contains(entity, position)?.unwrap_or(false);
                        (inside, inside && !visit.inside, true)
                    };
                    let exited = !plane && !inside && visit.inside;
                    visit.inside = inside;
                    let limit = component_value("Number Times")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let admitted = entered && (limit == 0 || u64::from(visit.count) < limit);
                    if admitted {
                        visit.count += 1;
                        state.entries += 1;
                        let delay = component_value("Delay")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.) as f32;
                        for output in self.demo_trigger_outputs(entity, true, plane, forward) {
                            state
                                .pending
                                .push((self.animation_time + delay, entity, output));
                        }
                    }
                    if exited {
                        for output in self.demo_trigger_outputs(entity, false, false, true) {
                            state.pending.push((self.animation_time, entity, output));
                        }
                    }
                    if class == "TriggerDamageComponent" && inside {
                        let component = &e.components[index];
                        let damage = super::trigger_damage::Authored::load(component)?;
                        let layer = self
                            .player_physics
                            .as_ref()
                            .and_then(|body| self.collision.simulation_filter(body.shape))
                            .map(|word| word & 31)
                            .or_else(|| {
                                value(
                                    &self.entities[self.player_entity],
                                    "PhysicsRigidBodyCharacterComponent",
                                    "Collision Layer",
                                )
                                .and_then(|v| v.as_u64())
                                .and_then(|v| u32::try_from(v).ok())
                            });
                        if (!damage.entry_only || entered)
                            && layer.is_none_or(|layer| damage.admits_layer(layer))
                        {
                            let samples = damage
                                .samples(if damage.entry_only { 1. } else { delta }, layer)?;
                            if !samples.is_empty() {
                                let destination = self
                                    .entity_position_or_bound_center(self.player_entity)?
                                    .unwrap_or(position);
                                let direction =
                                    self.emitter_stimulus_direction(entity, self.player_entity)?;
                                let emissions = samples
                                    .into_iter()
                                    .map(|(stimulus, amount)| {
                                        super::projectile_stimulus::Emission {
                                            stimulus,
                                            amount,
                                            position: destination,
                                            direction,
                                            target: Some(self.player_entity as u64),
                                            owner: Some(entity as u64),
                                            tag: damage.alias.clone(),
                                            delivery: super::projectile_stimulus::Delivery::Target,
                                        }
                                    })
                                    .collect::<Vec<_>>();
                                self.receive_combat(self.player_entity, &emissions)?;
                                if melee_enabled == Some(true) {
                                    self.record_demo_melee_contact(entity);
                                }
                            }
                        }
                    }
                    if admitted
                        && class == "TriggerVolumeComponent"
                        && self
                            .demo_trigger_outputs(entity, true, plane, forward)
                            .is_empty()
                    {
                        self.request_demo_conversation(entity)?;
                    }
                }
            }
            Ok(())
        })();
        self.demo_story.triggers = state;
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plane_endpoint_crossings_keep_their_motion_direction() {
        for (previous, current, forward) in [(-1., 0., true), (0., 1., true),
            (1., 0., false), (0., -1., false)] {
            assert!(plane_cross([0., 0., previous], [0., 0., current], [2., 2.]));
            let direction = super::super::trigger_plane_segment::forward(previous, current);
            assert_eq!(direction, forward);
            assert!(trigger_output_matches(if forward {"OnTriggerPlaneForward"} else {"OnTriggerPlaneBackward"}, true, true, direction));
            assert!(!trigger_output_matches(if forward {"OnTriggerPlaneBackward"} else {"OnTriggerPlaneForward"}, true, true, direction));
        }
        assert!(!plane_cross([0., 0., 1.], [0., 0., 2.], [2., 2.]));
        assert!(!plane_cross([0., 0., 0.], [0., 0., 0.], [2., 2.]));
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_airlock_activation_does_not_discard_entry_crossing() -> Result<()> {
        use anyhow::{ensure, Context};
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::super::assets::load(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        world.enable_demo_story()?;
        let gate = world
            .scripts
            .resolve_name("gv_zonea_airlock.PlaneTrigger 01")
            .context("Original encounter plane missing")?;
        let scene = world.entities[gate].scene.clone();
        let entity = &world.entities[gate];
        let center: [f32; 3] = entity.transform[12..15].try_into()?;
        let rotation = entity.runtime_rotation;
        let point = |sign: f32| {
            std::array::from_fn(|axis| {
                center[axis] + rotation[axis * 3 + 2] * sign - if axis == 1 { 0.6 } else { 0. }
            })
        };
        world.deactivate_streamed_scene(&scene)?;
        world.advance_demo_triggers(point(-1.), super::super::STEP)?;
        ensure!(
            !world
                .demo_story
                .triggers
                .visits
                .keys()
                .any(|(owner, _)| *owner == gate),
            "Inactive plane unexpectedly collected local visit history"
        );
        world.scene_requests.request(&scene);
        world.prepare_gameplay_scene_requests()?;
        world.advance_demo_triggers(point(1.), super::super::STEP)?;
        ensure!(
            world
                .demo_story
                .triggers
                .pending
                .iter()
                .any(|(_, owner, output)| *owner == gate && output == "OnTriggerPlaneForward"),
            "Scene activation discarded the player's entry segment"
        );
        // Deliver through the original IO and Lua, rather than invoking the
        // encounter callback or a spawner in this regression.
        world.advance_demo_triggers(point(1.), super::super::STEP)?;
        for _ in 0..900 {
            if world.demo_story.dialogue.is_some() {
                world.acknowledge_demo_story(0);
            }
            world.advance_models_with_delta(
                world.animation_time + super::super::STEP,
                super::super::STEP,
            )?;
            world.advance_dynamic_physics(super::super::STEP)?;
        }
        ensure!(
            world.globals.get("GV_ZoneA_CombatTraining") == Some(1),
            "Original encounter callback did not start"
        );
        // Construction alone is insufficient: a cloned template can exist
        // while remaining hidden, in an inactive scene, or outside physics.
        // Check the same admission gates the interactive host consumes.
        let mut draws = world.draws.clone();
        world.apply_model_poses(&mut draws)?;
        // This entry fixture covers the first spawned enemy. Do not require
        // another wave before delivering its prerequisite encounter events.
        for suffix in ["05"] {
            // The scene name identifies the source object; the authored
            // Spawn Group is the alias used by the encounter's unfreeze Lua.
            let name = format!("gremlinvillage_zonea.SpatterMelee {suffix}");
            let index = world
                .scripts
                .resolve_name(&name)
                .with_context(|| format!("Encounter has no live alias for {name}"))?;
            ensure!(
                world
                    .spawners
                    .values()
                    .any(|state| state.live.contains(&index)),
                "Encounter alias selects a source template: {name}"
            );
            ensure!(
                world.entity_scene_active(index)
                    && !world.removed_entities.contains(&index)
                    && !world.hidden_entities.contains(&index),
                "Encounter enemy is not admitted to the active scene: {name}"
            );
            ensure!(
                world.entities[index]
                    .draws
                    .clone()
                    .any(|draw| !world.hidden_entity_draws.contains(&draw)
                        && !draws[draw].indices.is_empty()),
                "Encounter enemy has no visible geometry: {name}"
            );
            let body = world
                .character_physics
                .get(&index)
                .with_context(|| format!("Encounter enemy has no character body: {name}"))?
                .as_ref()
                .map_err(|error| anyhow::anyhow!("{name}: {error}"))?;
            ensure!(
                world.collision.owner_in_world(body.owner)?
                    && body.motion.current.motion_type() == 7,
                "Encounter enemy was not unfrozen into gameplay physics: {name}"
            );
        }
        Ok(())
    }
    #[test]
    fn plane_crossing_requires_crossing_inside_its_rectangle() {
        assert!(plane_cross([0., 0., -1.], [0., 0., 1.], [5., 5.]));
        assert!(!plane_cross([4., 0., -1.], [4., 0., 1.], [5., 5.]));
        assert!(!plane_cross([0., 0., 1.], [0., 0., 2.], [5., 5.]));
        assert!(!plane_cross([0., 4., -1.], [0., 4., 1.], [5., 5.]));
    }
}

#[cfg(any(test, debug_assertions))]
pub(super) fn validate_original_pickups_and_effects() -> Result<()> {
    use anyhow::{ensure, Context};
    let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
    let mut world = super::assets::load(
        std::path::Path::new(&root),
        "Levels/GV_ZoneA_Start.level",
        &egui::Context::default(),
    )?;
    world.enable_demo_story()?;
    world.advance_models(0.)?;
    world.attach_player_body()?;
    world.run_callback_fixture("PlayerSetDripsVisible(GetPlayer(),false)")?;
    ensure!(
        !world.demo_story.drips_visible,
        "Authored player drip call did not hide drips"
    );
    world.run_callback_fixture("PlayerSetDripsVisible(GetPlayer(),true)")?;
    ensure!(
        world.demo_story.drips_visible,
        "Authored player drip call did not restore drips"
    );
    let static_entity = world
        .entities
        .iter()
        .position(|e| {
            !e.collision_parts.is_empty()
                && e.components
                    .iter()
                    .any(|c| c.class_name == "PhysicsRigidBodyStaticComponent")
        })
        .context("Original static-body fixture missing")?;
    let component = world.entities[static_entity]
        .components
        .iter()
        .position(|c| c.class_name == "PhysicsRigidBodyStaticComponent")
        .unwrap();
    let old = world.entities[static_entity].components[component]
        .properties
        .iter()
        .find(|p| p.name == "Collision Layer")
        .context("Missing static layer")?
        .value
        .as_i64()
        .context("Invalid static layer")?;
    let name = world.entities[static_entity].name.clone();
    world.run_callback_fixture(&format!("SetPropertyInt({name:?},'Collision Layer',7)"))?;
    for part in world.entities[static_entity].collision_parts.clone() {
        for shape in world
            .collision
            .owner_shapes(world.report.collision_parts[part].owner)
        {
            ensure!(
                world.collision.simulation_filter_word(shape)? & 31 == 7,
                "Static layer setter did not update solver shape"
            );
        }
    }
    world.run_callback_fixture(&format!(
        "SetPropertyInt({name:?},'Collision Layer',{old}); SetAppCulled({name:?})"
    ))?;
    ensure!(
        world.entities[static_entity].draws.clone().all(|d| world
            .demo_story
            .scene
            .culled_draws
            .contains(&d)),
        "App-cull did not reach model draws"
    );
    ensure!(
        !world.hidden_entities.contains(&static_entity),
        "App-cull changed physical visibility"
    );
    world.run_callback_fixture(&format!("ClearAppCulled({name:?})"))?;
    ensure!(
        world.demo_story.scene.culled_draws.is_empty(),
        "App-cull did not clear"
    );
    for name in [
        "ability_watchsketchpaint_nif",
        "ability_tvsketchpaint_nif",
        "eticket_c_nif",
        "ability_anvilsketch_nif",
    ] {
        let index = world
            .scripts
            .resolve_name(name)
            .with_context(|| format!("Missing original spawner source {name}"))?;
        ensure!(
            world.hidden_entities.contains(&index),
            "Spawner source leaked into world: {name}"
        );
    }
    let ticket = world
        .scripts
        .resolve_name("gremlinvillage_zonea.ETicket_C_Nif 01")
        .context("Original placed ticket missing")?;
    ensure!(
        !world.hidden_entities.contains(&ticket),
        "Actual placed ticket hidden with templates"
    );
    let inventory = |world: &World| {
        value(
            &world.entities[world.player_entity],
            "InventoryComponent",
            "ETicket",
        )
        .and_then(|v| v.as_f64())
        .unwrap()
    };
    // A partially spent tank must receive its authored increment, not reset
    // from the import-time InventoryComponent value after brush debits.
    if let Some(ammo) = &mut world.ammo.inventory {
        ammo.amounts[0] = 2.;
    }
    ensure!(
        world.player_inventory_command("Add Paint 1")?,
        "Paint increment rejected"
    );
    ensure!(
        world.ammo.inventory.as_ref().unwrap().amounts[0] == 3.,
        "Pickup reset a spent tank from stale authored ammo"
    );
    ensure!(
        super::actor_io::parameters("VAR_Mood_Type,1") == ["VAR_Mood_Type", "1"],
        "Native animation IO parameters were not unpacked"
    );
    ensure!(
        super::actor_io::parameters(r"literal\,comma") == ["literal,comma"],
        "Escaped IO comma was not preserved"
    );
    let previous = inventory(&world);
    let mut player = world.take_startup_player();
    player.position = world.entities[ticket].transform[12..15].try_into()?;
    world.entities[world.player_entity].set_controller_pose(player.position, player.yaw);
    let position = player.position;
    world.advance_pickup_overlaps(position)?;
    ensure!(
        world.removed_entities.contains(&ticket),
        "Overlapping the placed ticket did not collect it"
    );
    ensure!(
        inventory(&world) == previous + 10.,
        "Authored Add ETicket 10 did not reach live inventory"
    );
    let at = inventory(&world);
    world.advance_pickup_overlaps(position)?;
    ensure!(inventory(&world) == at, "Collected ticket granted twice");
    let mut first = world.draws.clone();
    let mut second = world.draws.clone();
    world.material_animation.apply(0., &mut first)?;
    world.material_animation.apply(2., &mut second)?;
    let animated = first
        .iter()
        .zip(&second)
        .filter(|(a, _)| {
            a.material
                .as_ref()
                .and_then(|m| m.shader.as_deref())
                .is_some_and(|s| s.eq_ignore_ascii_case("thinner_stagnent_bottom"))
        })
        .any(|(a, b)| {
            a.material_images
                .iter()
                .zip(b.material_images.iter())
                .any(|(a, b)| a.matrix != b.matrix)
        });
    ensure!(
        animated,
        "Original thinner material did not animate: {}",
        world.material_animation.summary()
    );
    let top_animated = first
        .iter()
        .zip(&second)
        .filter(|(a, _)| {
            a.material
                .as_ref()
                .and_then(|m| m.shader.as_deref())
                .is_some_and(|s| s.eq_ignore_ascii_case("thinner_stagnent_top"))
        })
        .any(|(a, b)| a.material_indirect != b.material_indirect);
    ensure!(
        top_animated,
        "Original thinner indirect distortion did not animate"
    );
    let barrel = world
        .scripts
        .resolve_name("gremlinvillage_zonea.Breakable Barrel Ammo 01")
        .context("Original barrel missing")?;
    let center: [f32; 3] = world.entities[barrel].transform[12..15].try_into()?;
    for offset in [[0.9, 0., 0.], [-0.9, 0., 0.], [0., 0., 0.9], [0., 0., -0.9]] {
        if world.runtime_health[&barrel].health <= 0 {
            break;
        }
        let origin = std::array::from_fn(|i| center[i] + offset[i]);
        world.dispatch_spin(origin, &mut std::collections::HashSet::new())?;
    }
    ensure!(
        world.runtime_health[&barrel].health <= 0,
        "Original barrel did not receive spin damage through collision queries"
    );
    world.advance_models_with_delta(world.animation_time + super::STEP, super::STEP)?;
    ensure!(
        world.removed_entities.contains(&barrel) || world.hidden_entities.contains(&barrel),
        "Dead barrel remained solid and visible"
    );
    let steam = world
        .scripts
        .resolve_name("gremlinvillage_zonea.Steam_Blocker_01 05")
        .context("Original steam blocker missing")?;
    world.admit_scene_particles();
    ensure!(
        world.scene_particles.live.contains_key(&steam),
        "Steam blocker failed: {:?}",
        world.scene_particles.errors.get(&steam)
    );
    world.scene_particles.set_enabled(steam, true)?;
    let mut emitted = false;
    for _ in 0..60 {
        world.advance_scene_particles(super::STEP)?;
        emitted |= world.scene_particles.live[&steam]
            .particles
            .particle_count()
            > 0;
    }
    ensure!(
        emitted,
        "Original multi-emitter steam produced no particles"
    );
    std::fs::write(
        "target/demo-blockers-original-assets.json",
        serde_json::to_vec_pretty(
            &serde_json::json!({"placed_ticket_collected":true,"ticket_amount":inventory(&world),"source_pickups_hidden":true,"thinner_material_animated":animated,"thinner_distortion_animated":top_animated,"barrel_breakable":true,"steam_emitted":emitted,"material_animation":world.material_animation.summary(),"scene_particles":world.scene_particles.summary()}),
        )?,
    )?;
    println!("PASS: original ticket collection, hidden spawner sources, thinner texture animation and multi-emitter steam");
    Ok(())
}

#[cfg(debug_assertions)]
pub(super) fn validate_streamed_triggers() -> Result<()> {
    use anyhow::{ensure, Context};
    let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
    let mut world = super::assets::load(
        std::path::Path::new(&root),
        "Levels/GV_ZoneA_Start.level",
        &egui::Context::default(),
    )?;
    world.enable_demo_story()?;
    world.advance_models(0.)?;
    world.attach_player_body()?;
    world.pickup_receivers.clear();
    let owner = world
        .scripts
        .resolve_name("gremlinvillage_zonea.SteamTrigger 09")
        .context("Original steam trigger missing")?;
    let component = world.entities[owner]
        .components
        .iter()
        .position(|c| c.class_name == "TriggerDamageComponent")
        .context("Steam damage component missing")?;
    for (entity, item) in world.entities.iter().enumerate() {
        for (index, c) in item.components.iter().enumerate() {
            if matches!(
                c.class_name.as_str(),
                "TriggerDamageComponent" | "TriggerVolumeComponent" | "TriggerPlaneComponent"
            ) {
                world
                    .component_control
                    .disabled
                    .entry(entity)
                    .or_default()
                    .insert(index, entity != owner || index != component);
            }
        }
    }
    let bounds = world
        .phantom_bounds(owner)?
        .context("Original steam phantom missing")?;
    let position = std::array::from_fn(|axis| (bounds.min[axis] + bounds.max[axis]) * 0.5);
    let mut player = world.take_startup_player();
    player.position = position;
    world.entities[world.player_entity].set_controller_pose(position, player.yaw);
    let scene = world.entities[owner].scene.clone();
    world.demo_story.triggers.visits.insert(
        (owner, component),
        Visit {
            inside: true,
            previous: Some(position),
            count: 3,
        },
    );
    world.deactivate_streamed_scene(&scene)?;
    ensure!(
        !world.entity_scene_active(owner),
        "Steam scene did not deactivate"
    );
    let visit = &world.demo_story.triggers.visits[&(owner, component)];
    ensure!(
        !visit.inside && visit.previous.is_none() && visit.count == 3,
        "Deactivation did not immediately invalidate overlap history"
    );
    world
        .demo_story
        .triggers
        .pending
        .push((0., owner, "StreamingProbe".into()));
    let health = world.runtime_health[&world.player_entity].health;
    world.with_live_player(&mut player, |world| {
        world.advance_demo_triggers(position, super::STEP)
    })?;
    ensure!(
        world.runtime_health[&world.player_entity].health == health,
        "Inactive scene damaged the live player"
    );
    ensure!(
        world
            .demo_story
            .triggers
            .pending
            .iter()
            .any(|(_, _, name)| name == "StreamingProbe"),
        "Inactive scene delivered its delayed output"
    );
    let visit = &world.demo_story.triggers.visits[&(owner, component)];
    ensure!(
        !visit.inside && visit.previous.is_none() && visit.count == 3,
        "Inactive overlap reset lost lifetime count or kept stale position"
    );
    world.activate_streamed_scene(&scene)?;
    ensure!(
        world.entity_scene_active(owner),
        "Steam scene did not reactivate"
    );
    world.with_live_player(&mut player, |world| {
        world.advance_demo_triggers(position, super::STEP)
    })?;
    ensure!(
        world.runtime_health[&world.player_entity].health == health - 1,
        "Reactivated original steam trigger failed to damage player"
    );
    ensure!(
        !world
            .demo_story
            .triggers
            .pending
            .iter()
            .any(|(_, _, name)| name == "StreamingProbe"),
        "Reactivated scene did not release held output"
    );
    ensure!(
        world.demo_story.triggers.visits[&(owner, component)].inside,
        "Reactivation did not query a fresh overlap"
    );
    // Two independently configured components on one owner must never share
    // the first component's properties. Use the original steam fixture for
    // collision admission and combat delivery, rather than a mock dispatcher.
    let second = world.entities[owner].components.len();
    let mut copy = world.entities[owner].components[component].clone();
    for property in &mut copy.properties {
        if property.name == "Damage per second" {
            property.value = serde_json::json!([30., 0.]);
        }
    }
    for property in &mut world.entities[owner].components[component].properties {
        if property.name == "Damage per second" {
            property.value = serde_json::json!([0., 0.]);
        }
    }
    world.entities[owner].components.push(copy);
    world
        .component_control
        .disabled
        .entry(owner)
        .or_default()
        .insert(component, true);
    world
        .component_control
        .disabled
        .entry(owner)
        .or_default()
        .insert(second, false);
    world
        .runtime_health
        .get_mut(&world.player_entity)
        .unwrap()
        .invulnerable_until = 0.;
    let before = world.runtime_health[&world.player_entity].health;
    world.with_live_player(&mut player, |world| {
        world.advance_demo_triggers(position, super::STEP)
    })?;
    ensure!(
        world.runtime_health[&world.player_entity].health == before - 1,
        "Second damage component incorrectly read the disabled first component's zero damage"
    );
    let previous_count = world.demo_story.triggers.visits[&(owner, second)].count;
    world
        .component_control
        .disabled
        .entry(owner)
        .or_default()
        .insert(second, true);
    world.with_live_player(&mut player, |world| {
        world.advance_demo_triggers(position, super::STEP)
    })?;
    let suspended = &world.demo_story.triggers.visits[&(owner, second)];
    ensure!(
        !suspended.inside && suspended.previous.is_none() && suspended.count == previous_count,
        "Disabled trigger retained stale contact or reset its lifetime count"
    );
    world
        .component_control
        .disabled
        .entry(owner)
        .or_default()
        .insert(second, false);
    world.with_live_player(&mut player, |world| {
        world.advance_demo_triggers(position, super::STEP)
    })?;
    ensure!(
        world.demo_story.triggers.visits[&(owner, second)].count == previous_count + 1,
        "Re-enabled trigger failed to produce a fresh entry"
    );
    let (steam, steam_component) = world
        .entities
        .iter()
        .enumerate()
        .find_map(|(entity, item)| {
            if !item.name.contains(".SteamTrigger") {
                return None;
            }
            item.components.iter().enumerate().find_map(|(index, c)| {
                if c.class_name != "TriggerDamageComponent" {
                    return None;
                }
                let damage = super::trigger_damage::Authored::load(c).ok()?;
                damage
                    .samples(super::STEP, Some(3))
                    .ok()?
                    .iter()
                    .any(|(kind, amount)| *kind == 3 && *amount > 0.)
                    .then_some((entity, index))
            })
        })
        .context("Original knockback steam trigger missing")?;
    // Isolate the authored hazard in a controllable gameplay state. Entrance
    // input locks and other overlapping hazards are outside this probe.
    for (entity, item) in world.entities.iter().enumerate() {
        for (index, c) in item.components.iter().enumerate() {
            if matches!(
                c.class_name.as_str(),
                "TriggerDamageComponent" | "TriggerVolumeComponent" | "TriggerPlaneComponent"
            ) {
                world
                    .component_control
                    .disabled
                    .entry(entity)
                    .or_default()
                    .insert(index, entity != steam || index != steam_component);
            }
            if entity == world.player_entity
                && matches!(
                    c.class_name.as_str(),
                    "MickeyPlayerMotionControllerComponent" | "BasicMotionControllerComponent"
                )
            {
                world
                    .component_control
                    .disabled
                    .entry(entity)
                    .or_default()
                    .insert(index, false);
            }
        }
    }
    let bounds = world
        .phantom_bounds(steam)?
        .context("Original knockback steam phantom missing")?;
    let position = std::array::from_fn(|axis| (bounds.min[axis] + bounds.max[axis]) * 0.5);
    player.position = position;
    world.entities[world.player_entity].set_controller_pose(position, player.yaw);
    let before = world
        .player_knockback
        .as_ref()
        .map_or(0, |state| state.state.admissions);
    world.with_live_player(&mut player, |world| {
        world.advance_demo_triggers(position, super::STEP)
    })?;
    ensure!(
        world
            .player_knockback
            .as_ref()
            .is_some_and(|state| state.state.active && state.state.admissions > before),
        "Original steam overlap did not enter knockback"
    );
    println!(
        "PASS: original {} overlap admits player knockback",
        world.entities[steam].name
    );
    println!("PASS: inactive original GV steam scene causes no player damage/output; overlap history resets while count survives; reactivation restores damage and held delivery; same-class damage components use independent properties; disabled triggers re-query entry without resetting lifetime limits");
    Ok(())
}

#[cfg(debug_assertions)]
pub(super) fn validate_dbc_arm_trigger() -> Result<()> {
    use anyhow::Context as _;
    let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
    let mut world = super::assets::load(
        std::path::Path::new(&root),
        "Levels/DBC_Start_1st.Level",
        &egui::Context::default(),
    )?;
    world.enable_demo_story()?;
    let trigger = world.scripts.resolve_name("dbc_1st_zonea_clawstages.ArmTriggerZone0 01")
        .context("Missing authored DBC arm trigger")?;
    let arm = world.scripts.resolve_name("dbc_1st_zonea_dynamic.Robot_Arm")
        .context("Missing authored DBC mechanical arm")?;
    let center = world.rigid_trigger_hulls.get(&trigger)
        .with_context(|| format!("Missing arm trigger hull: {:?}", world.report.unsupported.iter()
            .filter(|line| line.contains("ArmTriggerZone0 01")).collect::<Vec<_>>()))?
        .interior_player_position();
    let outside = [center[0], center[1] + 100., center[2]];
    let bit = 1 << 6; // Authored OnStartTouch/OnEndTouch both pass attack type 6.
    for (position, enabled) in [(outside, false), (center, true), (outside, false)] {
        world.advance_demo_triggers(position, super::STEP)?;
        world.advance_demo_triggers(position, super::STEP)?;
        world.advance_models_with_delta(world.animation_time + super::STEP, super::STEP)?;
        anyhow::ensure!((world.robot_arm_controls[&arm].enabled_mask & bit != 0) == enabled,
            "Authored arm trigger did not set attack 6 to {enabled} at {position:?}");
    }
    println!("PASS: entering and leaving the authored DBC arm hull enables and disables attack 6; center {center:?}");
    Ok(())
}
