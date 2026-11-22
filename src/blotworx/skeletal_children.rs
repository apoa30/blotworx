//! Live actor attachment bridge using the reversed Qs query and transform
//! cache routines. NiNode sockets use current controller poses and owner-space
//! conversion; missing sockets use the native default Qs branch.
use super::{
    actor_bone_query::{self, Pose},
    native_transform_record::Transform,
};
use anyhow::{ensure, Context, Result};
use epic_mickey_lib_rs::{hkx_file::HkxBone, scene_file::Component};
#[derive(Clone)]
pub struct Socket {
    pub name: String,
    pub offset: [f32; 3],
    pub multiplier: f32,
    pub mode: i32,
}
impl Socket {
    pub fn load(component: &Component) -> Result<Option<Self>> {
        let name = component.properties.iter()
            .find(|property| property.name == "Bone Attach Name")
            .and_then(|property| property.value.as_str())
            .unwrap_or("");
        Self::load_with_name(component, name)
    }

    fn load_with_name(component: &Component, name: &str) -> Result<Option<Self>> {
        let value = |name| {
            component
                .properties
                .iter()
                .find(|p| p.name == name)
                .map(|p| &p.value)
        };
        if name.is_empty() {
            return Ok(None);
        }
        let mode = i32::try_from(value("Bone Attach Rotate Mode")
            .map(|mode| mode.as_i64().context("Invalid attachment mode"))
            .transpose()?.unwrap_or(0))?;
        let offset = value("Bone Attach Offset")
            .map(|offset| crate::render_support::scene::parse_point3(offset)
                .context("Invalid attachment offset"))
            .transpose()?
            .unwrap_or([0.; 3]);
        // Native constructor default for the optional Bone Scale Multiplier.
        let multiplier = value("Bone Scale Multiplier")
            .map(|value| value.as_f64().context("Invalid bone scale multiplier"))
            .transpose()?
            .unwrap_or(0.) as f32;
        ensure!(
            offset
                .iter()
                .chain(std::iter::once(&multiplier))
                .all(|v| v.is_finite()),
            "Nonfinite attachment parameters"
        );
        Ok(Some(Self {
            // REL .text 356fa4..356fd0 copies the string, calls NiString::ToLower,
            // then stores that normalized name before binding the parent.
            name: name.to_ascii_lowercase(),
            offset,
            multiplier,
            mode,
        }))
    }
}
pub fn bone_pose(
    resources: &super::animation_resources::Resources,
    bones: &[HkxBone],
    name: &str,
) -> Result<Pose> {
    ensure!(
        bones.len() == resources.reference_qs.len(),
        "Attachment pose differs from rig"
    );
    let mut matches = resources
        .rig
        .bones
        .iter()
        .enumerate()
        .filter(|(_, bone)| bone.name.eq_ignore_ascii_case(name));
    // REL354b68..354bc0 replaces an invalid actor query with the default Qs
    // record. It does not try a NiNode lookup for an actor's missing bone.
    let Some((index, _)) = matches.next() else {
        return Ok(actor_bone_query::DEFAULT);
    };
    ensure!(matches.next().is_none(), "Ambiguous attachment bone {name}");
    let mut local = resources.reference_qs.clone();
    for (record, bone) in local.iter_mut().zip(bones) {
        record[..3].copy_from_slice(&bone.local_translation);
        record[4..8].copy_from_slice(&bone.local_rotation);
        record[8..11].copy_from_slice(&bone.local_scale);
    }
    struct Query {
        pose: super::actor_pose_storage::Storage,
    }
    impl actor_bone_query::Backend for Query {
        fn skeleton(&mut self) -> Result<usize> {
            Ok(0)
        }
        fn bone_count(&mut self, _: usize) -> Result<i32> {
            Ok(i32::try_from(self.pose.local.len())?)
        }
        fn flags(&mut self, index: usize) -> Result<u32> {
            self.pose
                .flags
                .get(index)
                .copied()
                .context("Missing attachment bone flags")
        }
        fn model_pose(&mut self, index: usize) -> Result<Pose> {
            self.pose
                .model
                .get(index)
                .copied()
                .context("Missing attachment model pose")
        }
        fn resolve_model_pose(&mut self, index: usize) -> Result<Pose> {
            self.pose
                .resolve(index, &mut super::actor_pose_compose::Native)
        }
        fn reference_pose(&mut self, _: usize, _: usize) -> Result<Pose> {
            anyhow::bail!("Unexpected reference attachment query")
        }
    }
    let mut query = Query {
        pose: super::actor_pose_storage::Storage {
            parents: resources
                .rig
                .bones
                .iter()
                .map(|bone| bone.parent_index)
                .collect(),
            model: vec![[0.; 12]; bones.len()],
            flags: vec![2; bones.len()],
            local,
        },
    };
    let mut output = [0.; 12];
    ensure!(
        actor_bone_query::query(&mut query, i32::try_from(index)?, false, &mut output)?,
        "Attachment bone query declined"
    );
    Ok(output)
}
const ID: [f32; 9] = [1., 0., 0., 0., 1., 0., 0., 0., 1.];
pub fn cache(parent: Transform, bone: Pose, socket: &Socket) -> Result<Transform> {
    cache_with_point(parent,bone,socket,None)
}
pub(super) fn cache_with_point(parent:Transform,bone:Pose,socket:&Socket,point:Option<Transform>)->Result<Transform> {
    ensure!(
        matches!(socket.mode, 0 | 1 | 3 | 4 | 5 | 6),
        "Native skeletal attachment mode {} is not connected",
        socket.mode
    );
    struct Prepare {
        multiplier: f32,
    }
    impl super::attachment_pose::Backend for Prepare {
        fn default_transform(&mut self) -> Result<Transform> {
            Ok(std::array::from_fn(|i| {
                if i < 9 {
                    ID[i]
                } else if i == 12 {
                    1.
                } else {
                    0.
                }
            }))
        }
        fn attachment_scale(&mut self) -> Result<f32> {
            Ok(self.multiplier)
        }
        fn store_scale_override_128(&mut self, _: u8) -> Result<()> {
            Ok(())
        }
    }
    struct Mode;
    impl super::attachment_pose::ModeBackend for Mode {
        fn store_override(&mut self, _: usize, _: u8) -> Result<()> {
            Ok(())
        }
        fn directional(&mut self, _: Pose, _: &mut Transform) -> Result<()> {
            anyhow::bail!("Directional child mode not connected")
        }
    }
    struct Offset([f32; 3]);
    impl super::attachment_pose::OffsetBackend for Offset {
        fn offset(&mut self) -> Result<[f32; 3]> {
            Ok(self.0)
        }
        fn identity_rotation(&mut self) -> Result<[f32; 9]> {
            Ok(ID)
        }
    }
    struct Publish(Transform);
    impl super::attachment_publish::Backend for Publish {
        fn store_cache_word(&mut self, index: usize, bits: u32) -> Result<()> {
            self.0[index] = f32::from_bits(bits);
            Ok(())
        }
        fn one(&mut self) -> Result<f32> {
            Ok(1.)
        }
    }
    let mut prepared = super::attachment_pose::prepare(
        bone,
        &mut Prepare {
            multiplier: socket.multiplier,
        },
    )?;
    let rotate =
        super::attachment_pose::mode_rotation(socket.mode, bone, &mut prepared, &mut Mode)?;
    let work=if let Some(point)=point.filter(|_|socket.mode==6) {
        let (position,rotation)=super::native_transform_record::world_pose(&prepared,point[9..12].try_into().unwrap(),&point[..9].try_into().unwrap());
        std::array::from_fn(|i|if i<9{rotation[i]}else if i<12{position[i-9]}else{prepared[12]*point[12]})
    }else{prepared};
    let work = super::attachment_pose::offset_work(
        socket.mode,
        work,
        rotate,
        &mut Offset(socket.offset),
    )?;
    let mut out = Publish([0.; 13]);
    super::attachment_publish::publish(parent, work, prepared, socket.multiplier, &mut out)?;
    Ok(out.0)
}
impl super::assets::World {
    /// REL .text 35392c stores a changed name and refreshes the parent's
    /// binding. The parent-binding path normalizes the name at 356fa4.
    pub(super) fn set_bone_attachment_name(
        &mut self,
        entity: usize,
        component: usize,
        field: Option<usize>,
        name: Option<&str>,
    ) -> Result<()> {
        if self.removed_entities.contains(&entity) {
            return Ok(());
        }
        let parent = self.transform_hierarchy.get(&entity).map(|local| local.parent);
        let component = &mut self.entities[entity].components[component];
        if field.and_then(|field| component.properties[field].value.as_str()) == name {
            return Ok(());
        }
        let socket = if parent.is_some() {
            Socket::load_with_name(component, name.unwrap_or_default())?
        } else {
            None
        };
        let value = serde_json::json!(socket.as_ref().map(|socket| socket.name.as_str()).or(name));
        if let Some(field) = field {
            component.properties[field].value = value;
        } else {
            component.properties.push(epic_mickey_lib_rs::scene_file::Property::new(
                "String".into(), "Bone Attach Name".into(), false, false, false, value,
            ));
        }
        if let Some(parent) = parent {
            let attached_to_bone = socket.is_some();
            self.transform_hierarchy.get_mut(&entity).unwrap().socket = socket;
            if self.entity_scene_active(entity) {
                if attached_to_bone {
                    self.update_skeletal_children_range_strict(entity..entity + 1)?;
                } else {
                    super::rotator_runtime::propagate(
                        parent, &self.transform_hierarchy, &mut self.entities, &mut self.draws,
                    )?;
                }
                self.force_dynamic_entity_pose(entity)?;
            }
            self.scene_motion_prepared_at = None;
        }
        Ok(())
    }

    fn node_attachment_pose(&self, owner: usize, name: &str, parent: Transform) -> Result<Pose> {
        use epic_mickey_lib_rs::nif_file::{Matrix3, NiTransform, Vector3};
        let entity = self
            .entities
            .get(owner)
            .context("Attachment model owner missing")?;
        let model = entity
            .model_asset
            .as_ref()
            .context("NiNode attachment parent has no model")?;
        let hierarchy = self
            .model_bounds
            .get(model)
            .context("Attachment NIF hierarchy missing")?;
        let mut found = None;
        for root in hierarchy.roots.iter().flatten() {
            if let Some(node) = hierarchy.find_attachment_node(*root, name)? {
                found = Some(node);
                break;
            }
        }
        let Some(node) = found else {
            return Ok(actor_bone_query::DEFAULT);
        };
        let parent_node = NiTransform {
            rotation: Matrix3 {
                rows: std::array::from_fn(|row| std::array::from_fn(|col| parent[row * 3 + col])),
            },
            translation: Vector3 {
                x: parent[9],
                y: parent[10],
                z: parent[11],
            },
            scale: parent[12],
        };
        let empty = std::collections::BTreeMap::new();
        let poses = if let Some(actor) = self
            .actor_playback
            .as_ref()
            .filter(|actor| actor.entity == owner)
        {
            &actor.pose.transforms
        } else if let Some(pose) = self
            .actors
            .get(&owner)
            .and_then(|actor| actor.pose.as_ref())
        {
            &pose.transforms
        } else if let Some(animation) = self.model_animation.get(&owner) {
            &animation.poses
        } else {
            &empty
        };
        let world = hierarchy.base_world_transforms_with_poses(Some(&parent_node), poses)?;
        let transform = world
            .get(&node)
            .context("Named attachment node has no current pose")?;
        let node_pose = std::array::from_fn(|i| {
            if i < 9 {
                transform.rotation.rows[i / 3][i % 3]
            } else if i < 12 {
                [
                    transform.translation.x,
                    transform.translation.y,
                    transform.translation.z,
                ][i - 9]
            } else {
                transform.scale
            }
        });
        // Primary nonactor branch354c60..354e8c uses inverse(owner) * the
        // selected node's current world transform, then raw Qs conversion.
        super::attachment_direction::node_pose(parent, node_pose)
    }
    pub(super) fn update_skeletal_children(
        &mut self,
        player_pose: Option<&[HkxBone]>,
    ) -> Result<()> {
        if let Some(pose) = player_pose {
            self.player_attachment_pose.clear();
            self.player_attachment_pose.extend_from_slice(pose);
        }
        self.update_skeletal_children_range(0..self.entities.len())
    }
    pub(super) fn update_skeletal_children_range(
        &mut self,
        range: std::ops::Range<usize>,
    ) -> Result<()> {
        self.update_skeletal_children_range_checked(range, false)
    }
    pub(super) fn update_skeletal_children_range_strict(
        &mut self,
        range: std::ops::Range<usize>,
    ) -> Result<()> {
        self.update_skeletal_children_range_checked(range, true)
    }
    fn update_skeletal_children_range_checked(
        &mut self,
        range: std::ops::Range<usize>,
        strict: bool,
    ) -> Result<()> {
        let mut pending = self
            .transform_hierarchy
            .iter()
            .filter(|(child, _)| range.contains(child))
            .filter(|(child, _)| self.entity_scene_active(**child))
            .filter_map(|(&child, local)| {
                local
                    .socket
                    .as_ref()
                    .map(|socket| (child, local.parent, socket.clone()))
            })
            .collect::<Vec<_>>();
        let admitted = pending
            .iter()
            .map(|(child, _, _)| *child)
            .collect::<std::collections::HashSet<_>>();
        let mut updated = std::collections::HashSet::new();
        while !pending.is_empty() {
            let before = pending.len();
            let mut remaining = Vec::new();
            for (child, parent, socket) in pending {
                if self.removed_entities.contains(&child) || self.removed_entities.contains(&parent)
                {
                    updated.insert(child);
                    continue;
                }
                if admitted.contains(&parent) && !updated.contains(&parent) {
                    remaining.push((child, parent, socket));
                    continue;
                }
                let result = (|| -> Result<()> {
                    let root = &self.entities[parent];
                    let parent_pose = std::array::from_fn(|i| {
                        if i < 9 {
                            root.runtime_rotation[i]
                        } else if i < 12 {
                            root.transform[3 + i]
                        } else {
                            root.authored_scale
                        }
                    });
                    let bone = if parent == self.player_entity {
                        super::skeletal_children::bone_pose(
                            &self.player_animations,
                            &self.player_attachment_pose,
                            &socket.name,
                        )?
                    } else if self.authored_havok.contains(parent) {
                        self.authored_havok.bone_pose(parent, &socket.name)?
                    } else if let Some((template, actor)) = self.havok_actors.get(&parent) {
                        let animation = self.sketch_templates.templates[template]
                            .animation
                            .as_ref()
                            .map_err(|e| anyhow::anyhow!(e.clone()))?
                            .as_ref()
                            .context("Spawned attachment parent has no animation")?;
                        super::skeletal_children::bone_pose(
                            &animation.resources,
                            &actor.playback.pose,
                            &socket.name,
                        )?
                    } else {
                        // Rigid shell parts still belong to the actor's
                        // rig when mesh or graph playback cannot initialize.
                        let reference = root.components.iter()
                            .find(|c| c.class_name == "AnimationComponent")
                            .map(|component| {
                                let value = |name| component.properties.iter()
                                    .find(|p| p.name == name).and_then(|p| p.value.as_str()).unwrap_or("");
                                self.authored_havok.reference_attachment_pose(
                                    value("Behavior File Path"), value("CharacterName"), &socket.name,
                                )
                            }).transpose()?.flatten();
                        match reference {
                            Some(pose) => pose,
                            None => self.node_attachment_pose(parent, &socket.name, parent_pose)?,
                        }
                    };
                    let pose = cache(parent_pose, bone, &socket)?;
                    let local = &self.transform_hierarchy[&child];
                    let rotation = pose[..9].try_into().unwrap();
                    let rotation = if matches!(socket.mode, 3 | 4) {
                        super::native_matrix::multiply(&rotation, &local.rotation)
                    } else {
                        rotation
                    };
                    let entity = &mut self.entities[child];
                    let previous = entity.transform;
                    entity.runtime_rotation = rotation;
                    let scale = if socket.multiplier > f32::EPSILON {
                        pose[12]
                    } else {
                        entity.authored_scale
                    };
                    entity.transform = super::render_transform::from_entity(
                        entity.runtime_rotation,
                        pose[9..12].try_into().unwrap(),
                        scale,
                    );
                    for draw in entity.draws.clone() {
                        self.draws[draw].trans_mat = entity.transform;
                    }
                    if entity.transform != previous && !entity.collision_parts.is_empty() {
                        self.publish_static_collision_pose(child, pose[9..12].try_into().unwrap(), rotation)?;
                    }
                    super::rotator_runtime::propagate(
                        child,
                        &self.transform_hierarchy,
                        &mut self.entities,
                        &mut self.draws,
                    )?;
                    Ok(())
                })();
                if let Err(error) = result {
                    let message =
                        format!("Skeletal child {}: {error:#}", self.entities[child].name);
                    if !self.report.unsupported.contains(&message) {
                        self.report.unsupported.push(message);
                    }
                    if strict {
                        return Err(error);
                    }
                }
                updated.insert(child);
            }
            ensure!(
                remaining.len() < before,
                "Unresolved skeletal attachment ownership"
            );
            pending = remaining;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_gv_nif_sockets_follow_model_controllers_and_parent_relocation() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::super::assets::load(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let nozzle = world
            .entities
            .iter()
            .position(|entity| entity.name == "mk_hose_nozzle")
            .context("Original brush nozzle child missing")?;
        let local = world
            .transform_hierarchy
            .get(&nozzle)
            .context("Nozzle has no owner")?;
        let owner = local.parent;
        let socket = local
            .socket
            .as_ref()
            .context("Nozzle has no socket")?
            .clone();
        assert_eq!(socket.name, "paintbrush");
        let before = world.entities[nozzle].transform;
        let position = world.entities[owner].transform[12..15].try_into().unwrap();
        let moved = super::super::physics::add(position, [3., 4., 5.]);
        let rotation = world.entities[owner].runtime_rotation;
        world.entities[owner].transform =
            super::super::native_hk_transform::from_entity(rotation, moved);
        for column in 0..3 {
            for row in 0..3 {
                world.entities[owner].transform[column * 4 + row] *=
                    world.entities[owner].authored_scale;
            }
        }
        world.entities[owner].transform[15] = 1.;
        world.update_skeletal_children_range(nozzle..nozzle + 1)?;
        let after = world.entities[nozzle].transform;
        for axis in 0..3 {
            ensure!(
                (after[12 + axis] - before[12 + axis] - [3., 4., 5.][axis]).abs() < 0.0001,
                "Nozzle did not follow owner relocation"
            );
        }
        ensure!(
            !world
                .report
                .unsupported
                .iter()
                .any(|message| message.starts_with("Skeletal child mk_hose_nozzle:")),
            "Nozzle attachment failed"
        );
        let animated = world.model_animation.keys().copied().find(|owner| {
            world
                .transform_hierarchy
                .values()
                .any(|local| local.parent == *owner && local.socket.is_some())
        });
        ensure!(
            animated.is_some(),
            "Expected animated original pickup socket owner"
        );
        let animated = animated.unwrap();
        let node = world
            .transform_hierarchy
            .iter()
            .find(|(_, local)| local.parent == animated && local.socket.is_some())
            .unwrap()
            .0;
        let node = *node;
        let before = world.entities[node].transform;
        world.advance_models(0.5)?;
        world.update_skeletal_children_range(node..node + 1)?;
        ensure!(
            world.entities[node].transform != before,
            "Authored model controller did not reach child socket"
        );
        println!("Original paintbrush socket follows relocated owner; original pickup controller changes its child transform");
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_gv_child_templates_have_independent_visibility_and_live_handheld_transforms(
    ) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::super::assets::load(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let mut player =
            super::super::physics::Player::new(world.report.spawn, world.report.motion.clone());
        // The airlock Gremlin arrives through the original startup streaming
        // request, rather than being part of the initial descriptor's scenes.
        for tick in 1..=90 {
            world.advance_player_models(&mut player, tick as f32 * super::super::STEP)?;
        }
        let owners = world
            .entities
            .iter()
            .enumerate()
            .filter(|(_, entity)| entity.name.contains("Gremlin_Variant4 01"))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        ensure!(owners.len() >= 2, "Expected two placed original Gremlins");
        let children = owners
            .iter()
            .map(|&owner| {
                world
                    .child_entity_by_name(owner, "GremlinWrench")?
                    .context("Original wrench child missing")
            })
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            children[0] != children[1],
            "Owners share a mutable wrench instance"
        );
        for (&owner, &child) in owners.iter().zip(&children) {
            assert_eq!(
                world.entities[child].transform_parent_id()?,
                Some(world.entities[owner].id)
            );
            ensure!(
                !world.entities[child].draws.is_empty(),
                "Wrench model not imported"
            );
            ensure!(
                world.hidden_entities.contains(&child),
                "Authored OnSpawned/OnMapLoad Hide did not reach wrench"
            );
            world.set_entity_hidden(owner, false)?;
            world.set_entity_hidden(child, true)?;
            world.dispatch_script_callback("UnhideGremlinWrench", owner)?;
            ensure!(
                !world.hidden_entities.contains(&child),
                "Original UnhideGremlinWrench failed"
            );
        }
        world.dispatch_script_callback("HideGremlinWrench", owners[0])?;
        assert!(world.hidden_entities.contains(&children[0]));
        assert!(!world.hidden_entities.contains(&children[1]));
        for tick in 91..=180 {
            world.advance_player_models(&mut player, tick as f32 * super::super::STEP)?;
        }
        for (&owner, &child) in owners.iter().zip(&children) {
            let socket = world.transform_hierarchy[&child]
                .socket
                .as_ref()
                .context("Wrench has no live socket")?;
            assert_eq!(socket.name, "Handheld");
            let parent = &world.entities[owner];
            let parent_pose = std::array::from_fn(|i| {
                if i < 9 {
                    parent.runtime_rotation[i]
                } else if i < 12 {
                    parent.transform[3 + i]
                } else {
                    parent.authored_scale
                }
            });
            let expected = cache(
                parent_pose,
                world.authored_havok.bone_pose(owner, "Handheld")?,
                socket,
            )?;
            assert_eq!(&world.entities[child].transform[12..15], &expected[9..12]);
            for draw in world.entities[child].draws.clone() {
                assert_eq!(world.draws[draw].trans_mat, world.entities[child].transform);
            }
        }
        ensure!(
            !world
                .report
                .unsupported
                .iter()
                .any(|message| message.contains("Skeletal child gremlinwrench")
                    || message.contains("Child template GremlinWrench")
                    || message.contains("global 'GetChildEntityByName'")),
            "{:?}",
            world.report.unsupported
        );
        println!("{} separate original wrench children; source Hide/Unhide callbacks and 90 live attachment ticks verified",children.len());
        Ok(())
    }
}
