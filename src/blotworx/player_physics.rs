//! Controller body admission into the shared compatibility solver. Native
//! contact listeners, welding and solver numerical parity remain incomplete.
use super::{
    assets::World, native_hk_transform as transform, native_motion_record::MotionRecord,
    physics::Player,
};
use anyhow::{ensure, Context, Result};
pub struct Body {
    pub shape: usize,
    pub owner: usize,
    pub record: MotionRecord,
    pub orientation: [f32; 4],
    pub response_limit: f32,
    pub suspended_motion: Option<super::body_motion::BodyMotion>,
    material: super::body_material::Coefficients,
}
impl World {
    /// Attach when starting gameplay, after resources and other body filters
    /// have loaded. Asset inspection alone does not start a player simulation.
    pub fn attach_player_body(&mut self) -> Result<()> {
        ensure!(
            self.player_physics.is_none(),
            "Player body already attached"
        );
        let entity = &self.entities[self.player_entity];
        let components = entity
            .components
            .iter()
            .filter(|c| c.class_name == "PhysicsRigidBodyCharacterComponent")
            .collect::<Vec<_>>();
        ensure!(
            components.len() == 1,
            "Player requires one resolved character body component"
        );
        let component = components[0];
        let value = |name: &str| {
            component
                .properties
                .iter()
                .find(|p| p.name == name)
                .map(|p| &p.value)
                .with_context(|| format!("Player body lacks {name}"))
        };
        let mut mass = value("Character Mass")?
            .as_f64()
            .context("Invalid character mass")? as f32;
        // REL379c4c..60: zero selects the source constructor's1000 default.
        if mass == 0. {
            mass = 1000.;
        }
        ensure!(
            mass.is_finite() && mass > 0.,
            "Character mass requires native compatibility handling"
        );
        let friction = value("Friction")?
            .as_f64()
            .context("Invalid character friction")? as f32;
        let layer = value("Collision Layer")?
            .as_i64()
            .context("Invalid character layer")?;
        ensure!(
            (0..32).contains(&layer),
            "Invalid character collision layer"
        );
        let real = u8::from(
            value("Is Real")?
                .as_bool()
                .context("Invalid character real flag")?,
        );
        let painted = u8::from(
            value("Is Painted")?
                .as_bool()
                .context("Invalid character painted flag")?,
        );
        // Preserve existing imported group identities. Full original startup
        // allocation order has not been reconstructed; this admission reserves
        // a unique group after those already present in the shared world.
        let group = self.collision.maximum_filter_group() + 1;
        ensure!(
            group < 32768,
            "Character admission needs a free native filter group"
        );
        let word = super::body_filter::pack(super::body_filter::Fields {
            layer: layer as u32,
            group,
            sub_a: 1,
            sub_b: 0,
            painted,
            real,
        });
        let initial = entity.physics_transform();
        let orientation = transform::rotation_quaternion_raw(&initial)?;
        let hull = self
            .report
            .player_collision
            .as_ref()
            .context("Missing original player collision hull")?;
        let owner = self.collision.next_owner()?;
        let shape = self
            .collision
            .add_static_convex(&hull.vertices, hull.radius, owner)?;
        self.collision
            .set_shape_transform(shape, self.report.spawn, orientation)?;
        self.collision
            .set_native_body_mass(&[shape], 1. / mass, [0.; 3], [0.; 3])?;
        let mut record = MotionRecord([0; 0x120]);
        record.0[8] = 7;
        record.0[0xcc..0xd0].copy_from_slice(&(1. / mass).to_be_bytes());
        // REL379ca0 reads4cd728+1c8=20 for the linear limit. The
        // response limit is +208=1000 (379dac), not this velocity limit.
        record.0[0xbc] = super::native_motion_limits::encode(20.)?;
        record.0[0xbd] = super::native_motion_limits::encode(200.)?;
        record.0[0x11e..].copy_from_slice(&0x3f80u16.to_be_bytes());
        // Original body cinfo retains linear damping0/angular damping0.05.
        record.0[0xb8..0xbc].copy_from_slice(&0.05f32.to_be_bytes());
        record.set_swept_state(&super::native_motion_step::MotionState {
            transform: initial,
            previous_center: [
                self.report.spawn[0],
                self.report.spawn[1],
                self.report.spawn[2],
                0.,
            ],
            center: [
                self.report.spawn[0],
                self.report.spawn[1],
                self.report.spawn[2],
                0.,
            ],
            previous_orientation: orientation,
            orientation,
            local_center: [0.; 4],
            delta_angle: [0.; 4],
        });
        self.collision.set_simulation_body(
            &[shape],
            &record,
            super::body_material::Coefficients {
                friction,
                restitution: 0.,
            },
            true,
        )?;
        self.collision.set_simulation_filter(shape, word)?;
        self.collision.player_body = Some(shape);
        self.player_physics = Some(Body {
            shape,
            owner,
            record,
            orientation,
            response_limit: 1000.,
            suspended_motion: None,
            material: super::body_material::Coefficients {friction,restitution:0.},
        });
        self.report.geometry.push(
            serde_json::json!({"kind":"shared_character_body","entity":entity.name,
            "shape":shape,"owner":owner,"mass":mass,"motion_type":7,"quality":8,
            "locked_rotation":true,"source_gravity_factor":1.,"world_gravity_applied":false,
            "filter_word":word,"havok_reference_validated":false}),
        );
        self.report.unsupported.push("Character uses the shared Box3D body with source mass, locked inertia and controller gravity. Original contact listeners, rigid-body welding, quality agents, startup group order and solver/controller numerical parity remain incomplete.".into());
        Ok(())
    }
    /// Functional demo delivery for primary-player DisableMotion/EnableMotion.
    /// Retains the same character snapshot replacement used by NPC bodies.
    /// Native modifier-slot/flag delivery remains compatibility work.
    pub(super) fn suspend_player_motion(&mut self, suspended:bool)->Result<()> {
        let body=self.player_physics.as_mut().context("Player motion requires its owned body")?;
        if body.suspended_motion.is_some()==suspended {return Ok(())}
        let mut motion=match body.suspended_motion.take() {
            Some(mut motion)=>{motion.current=body.record.clone();motion},
            None=>super::body_motion::BodyMotion::new(body.record.clone(),8,None)?,
        };
        motion.replace(if suspended {4}else{7},0x3f800000)?;
        body.record=motion.current.clone();
        if !suspended {
            let (inertia,mass)=body.record.solver_inverse_mass()?;
            let center=body.record.swept_state().local_center;
            self.collision.set_native_body_mass(&[body.shape],mass,inertia,[center[0],center[1],center[2]])?;
        }
        self.collision.set_simulation_body(&[body.shape],&body.record,body.material,true)?;
        body.suspended_motion=if suspended {Some(motion)}else{None};
        Ok(())
    }
    /// Called before player movement consumes jump clearance. The native
    /// supporting-body pointer is assigned by23a554..628 in contact traversal
    /// order; do not replace it with a weighted velocity or highest normal.
    pub(super) fn publish_player_ground_record(&self, player: &mut Player) -> Result<()> {
        if self.player_physics.is_none() {
            return Ok(());
        }
        let previous_motion = player.motion_state();
        player.ledge_gate_enabled = self
            .ledge_controls
            .get(&self.player_entity)
            .is_none_or(|state| state.flags_admit_query());
        // Native308a54 publishes the fresh checkSupport result before the
        // ground record. Scene changes can invalidate last tick's support.
        let support = self
            .collision
            .with_player_queries(|| player.query_support(&self.collision, super::STEP))
            .context("Character support query unavailable for motion ground publication")?;
        player.accept_motion_surface(support, || {
            let surface = self
                .collision
                .active_support_motion(false)
                .context("Supported character lacks its motion surface output")?;
            Ok([
                surface.normal[0],
                surface.normal[1],
                surface.normal[2],
                surface.normal_w,
            ])
        })?;
        //802f6aec stores the support classification even when it skips surface
        //aggregation.308a54..68 publishes state/retained normal before308b20.
        player.accept_ground_support(support);
        self.update_player_stairs(player, previous_motion, support)
    }

    /// Box3D can end the step beyond a tread with no hull contact. Classify
    /// that pose before animation consumes the motion state, so a valid lower
    /// tread retains ground motion on the same tick that support is lost.
    pub(super) fn refresh_player_stairs_after_physics(&self, player: &mut Player) -> Result<()> {
        use super::support::State;
        if !player.motion.stairs.enabled || self.player_physics.is_none() {
            return Ok(());
        }
        let Some(support) = player.support_state else {
            player.stairs_ground_retained = false;
            return Ok(());
        };
        if support == State::Supported
            || player.ground_vectors.is_none()
            || !matches!(player.motion_state(), 0 | 2 | 3)
        {
            player.stairs_ground_retained = false;
            return Ok(());
        }
        self.update_player_stairs(player, 0, support)
    }

    fn update_player_stairs(
        &self,
        player: &mut Player,
        previous_motion: i32,
        support: super::support::State,
    ) -> Result<()> {
        let input = super::motion_ground::Input {
            position: [
                player.position[0],
                player.position[1],
                player.position[2],
                0.,
            ],
            radius: player.motion.ground_probe_start_offset,
            maximum: player.motion.max_relevant_ground_distance,
            slope_cosine: player.motion.max_slope_degrees.to_radians().cos(),
            enabled: true,
            support: support as u32,
            supporting_body: self.collision.supporting_body(),
        };
        let state = player.motion_ground.get_or_insert_with(Default::default);
        self.collision.update_motion_ground(state, input)?;
        player.stairs_ground_retained = state.distance.is_some_and(|distance| {
            player
                .motion
                .stairs
                .retains(previous_motion, support as u32, distance)
        });
        Ok(())
    }
    pub fn advance_player_physics(&mut self, player: &mut Player, dt: f32) -> Result<()> {
        let Some(body) = &self.player_physics else {
            return self.advance_dynamic_physics(dt);
        };
        let shape = body.shape;
        let orientation = body.orientation;
        // Carry Mickey along moving platforms/doors while hanging on a ledge.
        if let Some(ledge_owner) = player.ledge.active.map(|active| active.owner) {
            if let Some(entity_idx) = self.collision_entity_index(ledge_owner) {
                if let Ok(owner_pos) = self.entities[entity_idx].transform[12..15].try_into() {
                    if let Some(delta) = player.ledge.carry_with_owner(owner_pos) {
                        for i in 0..3 {
                            player.position[i] += delta[i];
                        }
                    }
                }
            }
        }
        // Retail dynamic wall-pushing (PushingMaxAngle = 75 deg): when Mickey
        // runs into a movable dynamic body on the ground, push it horizontally.
        if player.grounded && !player.ledge.is_active() && !player.native_knockback {
            let hspeed = player.velocity[0].hypot(player.velocity[2]);
            let wall_push_allowed = !self.wall_pushing_disabled.contains(&self.player_entity);
            if wall_push_allowed && hspeed > 0.5 && player.motion.pushing_max_angle > 0. {
                let dir = [player.velocity[0] / hspeed, 0., player.velocity[2] / hspeed];
                let probe = [player.position[0], player.position[1] + 0.5, player.position[2]];
                if let Ok(Some(hit)) = self.collision.with_player_queries(|| {
                    Ok::<_, anyhow::Error>(self.collision.ray_hit(probe, dir, 0.55))
                }) {
                    let min_cos = player.motion.pushing_max_angle.to_radians().cos();
                    let into_dot = -(hit.normal[0] * dir[0] + hit.normal[2] * dir[2]);
                    if into_dot >= min_cos {
                        let is_dynamic = self.dynamic_runtime_sources.iter().any(|(src, &owner)| {
                            owner == hit.owner
                                && self.dynamic_constructions.get(src).is_some_and(|c| {
                                    c.bodies.iter().any(|b| {
                                        matches!(b.motion.current.motion_type(), 2 | 3)
                                    })
                                })
                        });
                        if is_dynamic {
                            let impulse = [dir[0] * hspeed * dt * 2.5, 0., dir[2] * hspeed * dt * 2.5];
                            let _ = self.collision.apply_owner_impulse(hit.owner, impulse);
                        }
                    }
                }
            }
        }
        if body.suspended_motion.is_some() || player.ledge.is_active() {
            player.velocity = [0.; 3];
        }
        if let Some((origin,normal))=player.side_scroll_plane {
            player.velocity=super::side_scroller::project(player.velocity,normal);
            let relative=super::physics::sub(player.position,origin);
            player.position=super::physics::add(origin,super::side_scroller::project(relative,normal));
        }
        let pose = self.collision.simulation_pose(shape)?;
        // Explicit respawns/scripted relocations warp the body; ordinary
        // movement uses its last solver pose and does not reset contacts.
        if pose.position != player.position {
            self.collision
                .set_shape_transform(shape, player.position, orientation)?;
        }
        self.collision
            .set_simulation_velocity(shape, player.velocity)?;
        self.collision
            .set_character_response(player.velocity, body.response_limit)?;
        let start = self.dynamic_runtime_time;
        self.advance_dynamic_physics(dt)?;
        let mut pose = self.collision.simulation_pose(shape)?;
        if player.ledge.is_active() {
            pose.position = player.position;
            pose.linear = [0.; 3];
            self.collision
                .set_shape_transform(shape, player.position, orientation)?;
            self.collision.set_simulation_velocity(shape, [0.; 3])?;
        }
        if let Some((origin,normal))=player.side_scroll_plane {
            let relative=super::physics::sub(pose.position,origin);
            let constrained=super::physics::add(origin,super::side_scroller::project(relative,normal));
            let correction=super::physics::sub(constrained,pose.position);
            if correction.iter().any(|v|v.abs()>0.00001) {
                self.collision.set_shape_transform(shape,constrained,orientation)?;
                pose.position=constrained;
                pose.center=super::physics::add(pose.center,correction);
            }
            let constrained_velocity = super::side_scroller::project(pose.linear, normal);
            if constrained_velocity != pose.linear {
                self.collision.set_simulation_velocity(shape, constrained_velocity)?;
                pose.linear = constrained_velocity;
            }
        }
        let body = self.player_physics.as_mut().unwrap();
        let mut state = body.record.swept_state();
        state.previous_center = state.center;
        state.previous_center[3] = start;
        state.previous_orientation = state.orientation;
        state.center = [pose.center[0], pose.center[1], pose.center[2], 1. / dt];
        state.orientation = pose.orientation;
        state.transform =
            transform::from_entity([1., 0., 0., 0., 1., 0., 0., 0., 1.], pose.position);
        transform::set_rotation(&mut state.transform, pose.orientation);
        body.record.set_swept_state(&state);
        body.record
            .set_linear_velocity([pose.linear[0], pose.linear[1], pose.linear[2], 0.]);
        self.entities[self.player_entity].transform[12..15].copy_from_slice(&pose.position);
        player.accept_physics_pose(pose);
        // Retail NPC head-bounce (Config.ini FriendlySpatter_StompJumpHeightBonus = 2.0):
        // when Mickey lands/descends onto another character capsule's head, bounce him upward.
        if !player.ledge.is_active()
            && !player.native_knockback
            && !player.damage_leaping
            && pose.linear[1] <= 1.0
        {
            let bounce_target = self.character_physics.iter().find_map(|(&npc, res)| {
                if npc == self.player_entity
                    || self.removed_entities.contains(&npc)
                    || self.hidden_entities.contains(&npc)
                {
                    return None;
                }
                let npc_body = res.as_ref().ok()?;
                let npc_pos: [f32; 3] = self.entities[npc].transform[12..15].try_into().ok()?;
                let dx = pose.position[0] - npc_pos[0];
                let dz = pose.position[2] - npc_pos[2];
                let dy = pose.position[1] - npc_pos[1];
                let head_h = npc_body.navigation_height.clamp(0.6, 2.2);
                (dx.hypot(dz) <= 0.65 && dy >= head_h * 0.45 && dy <= head_h + 0.45)
                    .then_some(npc)
            });
            if let Some(npc) = bounce_target {
                let ai_type = self
                    .ai_brains
                    .get(&npc)
                    .map(|b| b.ai_type.as_str())
                    .unwrap_or("");
                let is_spatter = ai_type.to_ascii_lowercase().contains("spatter");
                let is_slobber = ai_type.to_ascii_lowercase().contains("theblob")
                    || ai_type.to_ascii_lowercase().contains("slobber");
                let is_kamikaze = ai_type.to_ascii_lowercase().contains("kamikaze")
                    || ai_type.to_ascii_lowercase().contains("spladoosh");
                let stunned = self
                    .demo_story
                    .ai
                    .stunned_until
                    .get(&npc)
                    .is_some_and(|&until| self.animation_time < until);
                let friendly = self.paint_counters.get(&npc).is_some_and(|c| c.status() == 1)
                    || self.music_team(npc).is_some_and(|(team, _)| team == 3);
                // Config.ini / ConfigFiles.ini: StunnedSpatter_StompJumpHeightBonus = 3.6,
                // FriendlySpatter_StompJumpHeightBonus = 2.0, Spatter_StompStunDuration = 2.0,
                // BEH_Stomped_SpatterKamikaze = 0.2, BEH_Stomped_TheBlob = 0.0.
                let bonus = if is_spatter && stunned {
                    3.6
                } else if is_spatter && friendly {
                    2.0
                } else {
                    if !friendly && !is_slobber {
                        let dur = if is_kamikaze { 0.2 } else { 2.0 };
                        self.demo_story
                            .ai
                            .stunned_until
                            .insert(npc, self.animation_time + dur);
                        let _ = self.authored_havok.send_event(npc, "EVENT_Start_Stun");
                    }
                    1.0
                };
                player.bounce_off_head(bonus);
                self.collision
                    .set_shape_transform(shape, player.position, orientation)?;
                self.collision
                    .set_simulation_velocity(shape, player.velocity)?;
                self.entities[self.player_entity].transform[12..15]
                    .copy_from_slice(&player.position);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_wheel_platform_carries_the_shared_player_through_a_full_cycle() -> Result<()> {
        use super::super::controller::{Controller, Input, TickPhase};
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::super::assets::load(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let platform = world
            .scripts
            .resolve_name("gremlinvillage_zonea_dynamic.GV_ZoneA_Platform01 02")
            .context("Original wheel platform missing")?;
        let source = world
            .dynamic_bodies
            .iter()
            .position(|body| body.entity == platform)
            .context("Platform body missing")?;
        let owner = *world
            .dynamic_runtime_sources
            .get(&source)
            .context("Platform body not admitted to shared physics")?;
        let origin: [f32; 3] = world.entities[platform].transform[12..15].try_into()?;
        // Isolated support fixture: place the initial character on the actual
        // platform surface. This does not establish boarding or progression.
        let hit = world
            .collision
            .ray_hits([origin[0], origin[1] + 10., origin[2] - 3.], [0., -25., 0.])?
            .into_iter()
            .filter(|hit| hit.owner == owner && hit.normal[1] > 0.7)
            .max_by(|a, b| a.position[1].total_cmp(&b.position[1]))
            .context("No original platform top surface")?;
        let spawn = [hit.position[0], hit.position[1] + 0.03, hit.position[2]];
        let initial_rotation = world.entities[platform].runtime_rotation;
        let offset: [f32; 3] = std::array::from_fn(|column| {
            (0..3)
                .map(|row| initial_rotation[row * 3 + column] * (spawn[row] - origin[row]))
                .sum()
        });
        world.attach_player_body()?;
        let shape = world.player_physics.as_ref().unwrap().shape;
        let mut controller = Controller::new(Player::new(spawn, world.report.motion.clone()));
        let mut max_error = 0f32;
        let mut min_y = f32::MAX;
        let mut max_y = f32::MIN;
        let mut history = std::collections::VecDeque::new();
        for _ in 0..900 {
            let mut command = serde_json::Value::Null;
            controller.advance_with_tick_phases(&mut world,|world|&world.collision,1./30.,true,Input::default(),
                |phase,player,_,_,_,world|->Result<()> {
                    if phase==TickPhase::BeforeMovement {
                        world.prepare_scene_motion(super::super::STEP)?;
                        world.publish_player_ground_record(player)?;
                    }
                    if phase==TickPhase::SimulatePhysics {
                        command=serde_json::json!({"velocity":player.velocity,
                            "motion":player.motion_state(),"stairs":player.stairs_ground_retained,
                            "grounded":player.grounded,"support":player.support_state.map(|s|s as u32),
                            "surface":world.collision.active_support_motion(false).map(|s|
                                serde_json::json!({"normal":s.normal,"velocity":s.velocity,"distance":s.distance})),
                            "owners":world.collision.contact_owners()});
                        world.advance_player_physics(player,super::super::STEP)?;
                    }
                    Ok(())
                })?;
            let platform_pose = &world.entities[platform];
            min_y = min_y.min(platform_pose.transform[13]);
            max_y = max_y.max(platform_pose.transform[13]);
            let expected: [f32; 3] = std::array::from_fn(|row| {
                platform_pose.transform[12 + row]
                    + (0..3)
                        .map(|column| {
                            platform_pose.runtime_rotation[row * 3 + column] * offset[column]
                        })
                        .sum::<f32>()
            });
            let error = (0..3)
                .map(|i| (controller.player.position[i] - expected[i]).powi(2))
                .sum::<f32>()
                .sqrt();
            let body_shape = world.dynamic_constructions[&source].bodies[0].shapes[0];
            let body_pose = world.collision.simulation_pose(body_shape)?;
            history.push_back(serde_json::json!({"tick":controller.ticks,"player":controller.player.position,
                "velocity":controller.player.velocity,"grounded":controller.player.grounded,
                "support":controller.player.support_state.map(|state|state as u32),
                "ground":controller.player.motion_ground,"expected":expected,"error":error,
                "platform_body_position":body_pose.position,"platform_body_velocity":body_pose.linear,
                "command":command,"contact_planes":world.collision.contact_planes()}));
            if history.len() > 12 {
                history.pop_front();
            }
            if controller.ticks > 30 {
                max_error = max_error.max(error);
                if error >= 0.15 || !controller.player.grounded {
                    std::fs::write(
                        "target/demo-wheel-carry-failure.json",
                        serde_json::to_vec_pretty(&history)?,
                    )?;
                }
                // Functional carry gate, not a Havok comparison: one frame at
                // the authored ~2.3m/s tangential speed is ~0.08m; allow another
                // 0.07m for the initial gap/contact margins, without changing it
                // in response to failures. Exact native parity remains open.
                ensure!(error<0.15,"Wheel carry drift at tick{}: player {:?}, platform-relative target {:?}, error{}",
                    controller.ticks,controller.player.position,expected,error);
                ensure!(
                    controller.player.grounded,
                    "Wheel support lost at tick{}",
                    controller.ticks
                );
            }
            assert_eq!(
                world.collision.simulation_pose(shape)?.position,
                controller.player.position
            );
            ensure!(controller.player.respawns == 0, "Wheel rider respawned");
        }
        ensure!(
            max_y - min_y > 16.,
            "Original wheel did not traverse its vertical cycle: {min_y}..{max_y}"
        );
        let before_jump = controller.player.position;
        controller.advance_with_tick_phases(
            &mut world,
            |world| &world.collision,
            1. / 30.,
            true,
            Input {
                jump_pressed: true,
                jump_held: true,
                ..Default::default()
            },
            |phase, player, _, _, _, world| -> Result<()> {
                if phase == TickPhase::BeforeMovement {
                    world.prepare_scene_motion(super::super::STEP)?;
                    world.publish_player_ground_record(player)?;
                }
                if phase == TickPhase::SimulatePhysics {
                    world.advance_player_physics(player, super::super::STEP)?;
                }
                Ok(())
            },
        )?;
        ensure!(
            controller.player.ground_jump_started && !controller.player.grounded,
            "Wheel rider could not jump off"
        );
        ensure!(
            controller.player.position[1] > before_jump[1],
            "Wheel rider was snapped back after jump input"
        );
        std::fs::write(
            "target/demo-wheel-carry-owned.json",
            serde_json::to_vec_pretty(&serde_json::json!({
            "platform":world.entities[platform].name,"ticks":controller.ticks,"max_relative_error_m":max_error,
            "platform_y_range":[min_y,max_y],"initial_player":spawn,"final_player":controller.player.position,
            "respawns":controller.player.respawns,"jump_departure":true,"scope":"Isolated original-platform support and jump-departure fixture; boarding, landing elsewhere and full-level traversal are not established.",
            "original_game_reference_comparison":false}))?,
        )?;
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_player_body_runs_in_the_shared_solver() -> Result<()> {
        use super::super::{
            controller::{Controller, Input, TickPhase},
            physics::Player,
        };
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::super::assets::load(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        world.attach_player_body()?;
        let shape = world.player_physics.as_ref().unwrap().shape;
        assert_eq!(world.collision.player_body, Some(shape));
        assert_eq!(
            world
                .player_physics
                .as_ref()
                .unwrap()
                .record
                .solver_inverse_mass()?,
            ([0.; 3], 0.01)
        );
        assert_eq!(
            world.report.motion.ground_probe_start_offset, 0.39,
            "Original PlayerEntityRadius override was ignored"
        );
        let mut controller =
            Controller::new(Player::new(world.report.spawn, world.report.motion.clone()));
        for tick in 0..180 {
            controller.advance_with_tick_phases(
                &mut world,
                |w| &w.collision,
                1. / 30.,
                true,
                Input {
                    movement: if tick > 120 { [0., 1.] } else { [0.; 2] },
                    jump_pressed: tick == 90,
                    jump_held: (90..96).contains(&tick),
                    ..Default::default()
                },
                |phase, player, _, _, _, world| -> Result<()> {
                    if phase == TickPhase::BeforeMovement {
                        world.publish_player_ground_record(player)?;
                    }
                    if phase == TickPhase::SimulatePhysics {
                        world.advance_player_physics(player, super::super::STEP)?;
                    }
                    Ok(())
                },
            )?;
            ensure!(
                controller
                    .player
                    .motion_ground
                    .as_ref()
                    .is_some_and(|state| state.distance.is_some()),
                "Original World did not publish a motion ground record"
            );
            let ground = controller.player.motion_ground.as_ref().unwrap();
            if ground.hit_this_update {
                ensure!(
                    ground.surface_velocity.is_some(),
                    "Original floor lost its retained body velocity"
                );
            }
            let pose = world.collision.simulation_pose(shape)?;
            assert_eq!(
                controller.player.position, pose.position,
                "Manual translation ran after solver publication"
            );
            assert_eq!(
                &world
                    .player_physics
                    .as_ref()
                    .unwrap()
                    .record
                    .swept_state()
                    .transform[12..15],
                &pose.position
            );
            assert_eq!(
                &world.entities[world.player_entity].transform[12..15],
                &pose.position,
                "Entity registry retained authored player spawn"
            );
            assert_eq!(pose.angular, [0.; 3]);
            ensure!(
                controller.player.respawns == 0,
                "Attached player left original level collision"
            );
        }
        ensure!(
            controller.player.grounded,
            "Attached player did not retain ground support"
        );
        ensure!(
            controller.player.position != world.report.spawn,
            "Attached player never moved"
        );
        ensure!(
            controller.player.motion_surface_normal.is_some(),
            "Character support did not publish its retained motion surface"
        );
        let mut camera = vec![0; 0x800];
        camera[0x14..0x18].copy_from_slice(
            &world
                .camera_profiles
                .controller
                .selection
                .current
                .to_be_bytes(),
        );
        for (off, value) in [
            (0x350, 5f32),
            (0x354, 0.4),
            (0x358, -0.3),
            (0x5a0, 5.),
            (0x5a4, 0.4),
            (0x5a8, -0.3),
            (0x5c4, f32::MAX),
        ] {
            camera[off..off + 4].copy_from_slice(&value.to_be_bytes());
        }
        camera[0x738] = 1;
        let gates = super::super::camera_yaw_pass::Gates {
            target_moved: false,
            input_hold_elapsed: true,
        };
        world.prepare_player_camera_polar(&mut camera, gates, &controller.player)?;
        ensure!(
            f32::from_be_bytes(camera[0x358..0x35c].try_into().unwrap()).is_finite(),
            "Live motion surface produced invalid camera pitch"
        );
        let retained = controller.player.motion_surface_normal.take();
        ensure!(
            world
                .prepare_player_camera_polar(&mut camera, gates, &controller.player)
                .is_err(),
            "AutoLevel silently substituted an unpublished motion surface"
        );
        controller.player.motion_surface_normal = retained;
        Ok(())
    }
}
