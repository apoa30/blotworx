use super::assets::World;
use anyhow::Result;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct State {
    vacuum: BTreeMap<usize, Vacuum>,
    boss_phase: BTreeMap<usize, BossState>,
    pub boss_attacks: u64,
    pub blotling_spawns: u64,
}

#[derive(Default)]
struct BossState {
    next_attack: f32,
    next_spawn: f32,
    stunned_until: f32,
    phase: u32,
}

#[derive(Default)]
struct Vacuum {
    until: f32,
    next_attack: f32,
    effective_thinner: f32,
}

impl State {
    pub(super) fn retire(&mut self, entity: usize) {
        self.vacuum.remove(&entity);
        self.boss_phase.remove(&entity);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BossKind {
    TronPete,
    AnimatronicHook,
    ShadowBlot,
    ShadowBlotGround,
    ShadowBlotHands,
}

impl BossKind {
    pub(super) fn of(ai_type: &str) -> Option<Self> {
        let low = ai_type.to_ascii_lowercase();
        if low.contains("tronpete") {
            Some(Self::TronPete)
        } else if low.contains("animatronichook") {
            Some(Self::AnimatronicHook)
        } else if low.contains("shadowblothands") {
            Some(Self::ShadowBlotHands)
        } else if low.contains("shadowblotground") {
            Some(Self::ShadowBlotGround)
        } else if low.contains("shadowblot") {
            Some(Self::ShadowBlot)
        } else {
            None
        }
    }
}

impl World {
    fn is_blob(&self, actor: usize) -> bool {
        self.ai_brains.get(&actor).is_some_and(|brain| {
            brain.behaviors.iter().any(|behavior| behavior == "TheBlobVacuum")
        })
    }

    fn blob_vacuum_setting(&self, name: &str, fallback: f32) -> f32 {
        self.runtime_settings.get(&format!("BEH_TheBlobVacuum_Default_{name}"))
            .and_then(|value| super::config_value::decimal_float(value).ok())
            .filter(|value| value.is_finite() && *value >= 0.).unwrap_or(fallback)
    }

    pub(super) fn record_blob_vacuum_stimuli(&mut self, target: usize, stimuli: &[(u32, f32)]) {
        let actor = if self.entities[target].name.to_ascii_lowercase().contains("theblobmouth") {
            self.transform_hierarchy.get(&target).map_or(target, |node| node.parent)
        } else {
            target
        };
        if let Some(vacuum) = self.demo_story.boss_ai.vacuum.get_mut(&actor) {
            if vacuum.until > self.animation_time {
                vacuum.effective_thinner += stimuli.iter().filter(|&&(kind, amount)| kind == 0 && amount > 0.)
                    .map(|&(_, amount)| amount).sum::<f32>();
            }
        }
    }

    pub(super) fn advance_blob_vacuum(
        &mut self, actor: usize, can_start: bool, paused: bool, shape: usize,
        pose: [f32; 3], aim: [f32; 3], facing: [f32; 2],
    ) -> Result<bool> {
        if !self.is_blob(actor) || !self.authored_havok.has_event(actor, "EVENT_Start_AttackVacuum") {
            return Ok(false);
        }
        let now = self.animation_time;
        let x = pose[0] - aim[0];
        let z = pose[2] - aim[2];
        let distance = x.hypot(z);
        let forward = (-x * facing[0] - z * facing[1]) / distance.max(0.001);
        let cone = self.blob_vacuum_setting("ConeHalfAngle", 45.).to_radians().cos();
        let abort_distance = self.blob_vacuum_setting("AbortDistance", 4.);
        let choke_damage = self.blob_vacuum_setting("DamageToChoke", 8.);
        let state = self.demo_story.boss_ai.vacuum.entry(actor).or_default();
        if state.until > 0. {
            let loop_active = self.authored_havok.active_state_time(actor, "Vacuum_loop").is_some();
            let choke = state.effective_thinner >= choke_damage && choke_damage > 0.;
            if choke || now >= state.until || paused || (loop_active && distance <= abort_distance) {
                state.until = 0.;
                let event = if choke { "EVENT_Start_AttackVacuumChoke" } else { "EVENT_Finish_AttackVacuum" };
                self.authored_havok.send_event(actor, event)?;
            } else if loop_active && forward >= cone && distance > 0.001 && self.script_player_available
                && self.demo_melee_clear(shape, pose, aim, self.player_entity)?
            {
                let near = self.blob_vacuum_setting("SuctionMaxSpeed", 10.);
                let far = self.blob_vacuum_setting("SuctionMinSpeed", 6.);
                let weight = ((distance - abort_distance) / 7.).clamp(0., 1.);
                let pull = near + (far - near) * weight;
                let velocity = &mut self.script_player.velocity;
                let direction = [x / distance, z / distance];
                let along = velocity[0] * direction[0] + velocity[2] * direction[1];
                let add = (pull - along).max(0.);
                velocity[0] += direction[0] * add;
                velocity[2] += direction[1] * add;
            }
            return Ok(true);
        }
        if self.authored_havok.active_state_time(actor, "VacuumAttack").is_some() {
            return Ok(true);
        }
        if !can_start || paused || now < state.next_attack || forward < cone {
            return Ok(false);
        }
        let Some(weapon) = self.child_entity_by_name(actor, "theblobvacuum")? else { return Ok(false); };
        let Some(component) = self.entities[weapon].components.iter().find(|c| c.class_name == "WeaponComponent") else {
            return Ok(false);
        };
        let value = |name| component.properties.iter().find(|p| p.name == name)
            .and_then(|p| p.value.as_f64()).map(|value| value as f32);
        let (Some(minimum), Some(maximum), Some(refire)) = (value("WeaponMinRange"), value("WeaponMaxRange"), value("WeaponMinRefireTime")) else {
            return Ok(false);
        };
        if !(minimum..=maximum).contains(&distance) {
            return Ok(false);
        }
        self.authored_havok.send_event(actor, "EVENT_Start_AttackVacuum")?;
        self.demo_story.boss_ai.vacuum.insert(actor, Vacuum {
            until: now + 6.,
            next_attack: now + refire,
            effective_thinner: 0.,
        });
        Ok(true)
    }

    pub(super) fn boss_kind(&self, actor: usize) -> Option<BossKind> {
        BossKind::of(&self.ai_brains.get(&actor)?.ai_type)
    }

    pub(super) fn record_boss_stimuli(&mut self, actor: usize, stimuli: &[(u32, f32)]) {
        let Some(kind) = self.boss_kind(actor) else {
            return;
        };
        let now = self.animation_time;
        let hit_spin = stimuli.iter().any(|&(k, a)| k == 8 && a > 0.);
        let hit_ink = stimuli.iter().any(|&(k, a)| matches!(k, 0 | 1) && a > 0.);
        let state = self.demo_story.boss_ai.boss_phase.entry(actor).or_default();
        match kind {
            BossKind::TronPete => {
                // Spin or deflected hit stuns Petetronic (TronPeteStunned) so Mickey can paint/thin his back.
                if hit_spin {
                    state.stunned_until = state.stunned_until.max(now + 4.0);
                    if self.authored_havok.contains(actor) {
                        let _ = self.authored_havok.send_event(actor, "EVENT_Start_Stun");
                    }
                }
            }
            BossKind::AnimatronicHook => {
                if hit_spin {
                    state.stunned_until = state.stunned_until.max(now + 1.5);
                    if self.authored_havok.contains(actor) {
                        let _ = self.authored_havok.send_event(actor, "EVENT_Start_HitReact");
                    }
                }
            }
            BossKind::ShadowBlot | BossKind::ShadowBlotGround | BossKind::ShadowBlotHands => {
                if hit_ink && self.authored_havok.contains(actor) {
                    let _ = self.authored_havok.send_event(actor, "EVENT_Start_HitReact");
                }
            }
        }
    }

    /// Drives dedicated retail boss behaviors for Petetronic (`TronPete`),
    /// Animatronic Captain Hook (`AnimatronicHook`), and Shadow Blot (`ShadowBlot*`).
    /// Returns `Ok(true)` when the boss controller handled this actor's tick.
    pub(super) fn advance_story_boss(
        &mut self,
        actor: usize,
        paused: bool,
        dead: bool,
        shape: usize,
        source: [f32; 3],
        player_pos: [f32; 3],
        delta: f32,
    ) -> Result<bool> {
        let Some(kind) = self.boss_kind(actor) else {
            return Ok(false);
        };
        if paused || dead {
            return Ok(false);
        }
        let now = self.animation_time;
        let dx = player_pos[0] - source[0];
        let dy = player_pos[1] - source[1];
        let dz = player_pos[2] - source[2];
        let distance = dx.hypot(dz);
        let state = self.demo_story.boss_ai.boss_phase.entry(actor).or_default();
        if now < state.stunned_until {
            let _ = self.collision.set_simulation_velocity(shape, [0., 0., 0.]);
            return Ok(true);
        }
        // Face Mickey smoothly.
        if distance > 0.05 {
            let desired_yaw = (-dx).atan2(dz);
            let rot = self.entities[actor].runtime_rotation;
            let current = rot[6].atan2(rot[8]);
            let step = super::physics::facing_delta(current, desired_yaw, 5.0, delta);
            let (s, c) = (current + step).sin_cos();
            let transform = super::native_hk_transform::from_entity(
                [c, 0., -s, 0., 1., 0., s, 0., c],
                source,
            );
            let _ = self.warp_character_body(actor, transform);
            let _ = self.publish_npc_ai_facing(actor, [dx / distance, 0., dz / distance]);
        }
        let _ = self.prepare_child_slots(actor);
        let mut ranged_weapon = None;
        let mut melee_weapon = None;
        if let Some(slots) = self.live_child_slots.get(&actor) {
            for child in slots.iter().flatten().copied() {
                if self.removed_entities.contains(&child) {
                    continue;
                }
                if ranged_weapon.is_none()
                    && self.entities[child]
                        .components
                        .iter()
                        .any(|c| c.class_name == "ShooterComponent")
                {
                    ranged_weapon = Some(child);
                }
                if melee_weapon.is_none()
                    && self.entities[child]
                        .components
                        .iter()
                        .any(|c| c.class_name == "TriggerDamageComponent")
                {
                    melee_weapon = Some(child);
                }
            }
        }
        let setting = |key: &str, default: f32| -> f32 {
            self.runtime_settings
                .get(key)
                .and_then(|v| super::config_value::decimal_float(v).ok())
                .filter(|v| v.is_finite())
                .unwrap_or(default)
        };
        let state_next_attack = self.demo_story.boss_ai.boss_phase[&actor].next_attack;
        let state_next_spawn = self.demo_story.boss_ai.boss_phase[&actor].next_spawn;
        match kind {
            BossKind::TronPete => {
                // ConfigFiles.ini [BEH_TronPete_TronPete]: IdleAndTrackPhase1TimeLimit=1.0, RateAttackTimeLimit=2.0
                let rate_limit = setting("BEH_TronPete_TronPete_RateAttackTimeLimit", 2.0);
                if distance <= 25.0 && now >= state_next_attack {
                    let st = self.demo_story.boss_ai.boss_phase.get_mut(&actor).unwrap();
                    st.next_attack = now + rate_limit;
                    st.phase = (st.phase + 1) % 3;
                    self.demo_story.boss_ai.boss_attacks += 1;
                    if distance <= 3.5 && melee_weapon.is_some() {
                        let _ = self.authored_havok.send_event(actor, "EVENT_Start_AttackMelee");
                    } else if let Some(weapon) = ranged_weapon {
                        let _ = self.authored_havok.send_event(actor, "EVENT_Start_AttackRanged");
                        self.demo_story.projectiles.queue(actor, weapon, player_pos);
                    }
                }
                // Close in moderately when far from Mickey.
                if distance > 7.0 && distance <= 25.0 {
                    let dir = [dx / distance, dz / distance];
                    let speed = 4.0;
                    let _ = self.collision.set_simulation_velocity(shape, [dir[0] * speed, 0., dir[1] * speed]);
                    let _ = self.authored_havok.write_native_variable(actor, "VAR_Forward_Speed", speed.to_bits());
                } else {
                    let _ = self.collision.set_simulation_velocity(shape, [0., 0., 0.]);
                    let _ = self.authored_havok.write_native_variable(actor, "VAR_Forward_Speed", 0u32);
                }
                Ok(true)
            }
            BossKind::AnimatronicHook => {
                // ConfigFiles.ini [SENS_SeeEnemy_AnimatronicHook]: 100m / 360 deg.
                if distance <= 40.0 && now >= state_next_attack {
                    if distance <= 3.2 {
                        self.demo_story.boss_ai.boss_phase.get_mut(&actor).unwrap().next_attack = now + 1.8;
                        self.demo_story.boss_ai.boss_attacks += 1;
                        let _ = self.authored_havok.send_event(actor, "EVENT_Start_AttackMelee");
                    } else if let Some(weapon) = ranged_weapon.filter(|_| distance <= 18.0) {
                        self.demo_story.boss_ai.boss_phase.get_mut(&actor).unwrap().next_attack = now + 2.4;
                        self.demo_story.boss_ai.boss_attacks += 1;
                        let _ = self.authored_havok.send_event(actor, "EVENT_Start_AttackRanged");
                        self.demo_story.projectiles.queue(actor, weapon, player_pos);
                    }
                }
                // Let standard steering handle Hook's deck pursuit when outside melee reach.
                Ok(false)
            }
            BossKind::ShadowBlot | BossKind::ShadowBlotGround | BossKind::ShadowBlotHands => {
                // ConfigFiles.ini [BEH_ShadowBlot_Default]: AttackRangeNear=19, Mid=20, Far=100, MoveSpeed=25.
                let range_near = setting("BEH_ShadowBlot_Default_AttackRangeNear", 19.0);
                let range_far = setting("BEH_ShadowBlot_Default_AttackRangeFar", 100.0);
                let swoop_speed = setting("BEH_ChargeAttack_ShadowBlot_MoveSpeed", 25.0).min(14.0);
                if distance <= range_far && now >= state_next_attack {
                    let st = self.demo_story.boss_ai.boss_phase.get_mut(&actor).unwrap();
                    st.next_attack = now + if kind == BossKind::ShadowBlotHands { 2.2 } else { 3.0 };
                    st.phase = (st.phase + 1) % 3;
                    self.demo_story.boss_ai.boss_attacks += 1;
                    if distance <= range_near {
                        if kind == BossKind::ShadowBlot {
                            let _ = self.authored_havok.send_event(actor, "EVENT_Start_AttackCharge");
                        } else {
                            let _ = self.authored_havok.send_event(actor, "EVENT_Start_AttackMelee");
                        }
                    } else if let Some(weapon) = ranged_weapon {
                        let _ = self.authored_havok.send_event(actor, "EVENT_Start_AttackRanged");
                        self.demo_story.projectiles.queue(actor, weapon, player_pos);
                    }
                }
                // Periodic Blotling spawn event (REL OnShadowBlotSpawnBlotlingStart).
                if kind == BossKind::ShadowBlot && distance <= range_far && now >= state_next_spawn {
                    self.demo_story.boss_ai.boss_phase.get_mut(&actor).unwrap().next_spawn = now + 12.0;
                    self.demo_story.boss_ai.blotling_spawns += 1;
                    self.emit_entity_output(actor, "OnShadowBlotSpawnBlotlingStart", actor);
                }
                // 3D aerial pursuit for flying ShadowBlot when above/away from arena.
                if kind == BossKind::ShadowBlot && distance > 8.0 && distance <= range_far {
                    let dist3 = (dx * dx + dy * dy + dz * dz).sqrt().max(0.01);
                    let target_y_offset = 6.0;
                    let dy_hover = (player_pos[1] + target_y_offset) - source[1];
                    let speed = swoop_speed * 0.45;
                    let _ = self.collision.set_simulation_velocity(
                        shape,
                        [
                            dx / dist3 * speed,
                            (dy_hover * 0.5).clamp(-4.0, 4.0),
                            dz / dist3 * speed,
                        ],
                    );
                    return Ok(true);
                }
                Ok(false)
            }
        }
    }
}
