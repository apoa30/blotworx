//! Child attachment presentation using shared bone/node pose and cache owners.
//! PC creation/parent ownership is verified; numeric attachment parity remains open.
use super::{em2_children::Graph, native_transform_record::Transform, skeletal_children::Socket};
use anyhow::{ensure, Context, Result};
use epic_mickey_lib_rs::{
    nif_file::{Matrix3, NiTransform, NifFile, Vector3},
    packfile::VirtualFile,
    scene_file::SceneFile,
};
use std::{collections::HashMap, sync::Arc};
struct Binding {
    parent: (usize, u32),
    child: (usize, u32),
    scale: f32,
    socket: Option<Socket>,
    point: Option<Transform>,
    animated: bool,
    model: Option<Arc<super::scene_bounds::Hierarchy>>,
    draws: Vec<usize>,
}
#[derive(Default)]
pub(super) struct State {
    bindings: Vec<Binding>,
    pub errors: Vec<String>,
    updates: u64,
}
pub(super) struct Write {
    pub source: (usize, u32),
    pub pose: Transform,
}
impl State {
    pub fn load(
        graph: &Graph,
        scenes: &[SceneFile],
        files: &[VirtualFile],
        settings: &std::collections::BTreeMap<String, String>,
    ) -> Self {
        let mut out = Self::default();
        let mut models = HashMap::<String, Arc<super::scene_bounds::Hierarchy>>::new();
        for link in &graph.links {
            let result = (|| -> Result<Binding> {
                let child = scenes[link.child_scene]
                    .objects
                    .iter()
                    .find(|e| e.link_id.to_u32() == link.child)
                    .context("Child pose source absent")?;
                let parent = scenes[link.parent_scene]
                    .objects
                    .iter()
                    .find(|e| e.link_id.to_u32() == link.parent)
                    .context("Parent pose source absent")?;
                let components = crate::render_support::scene::get_effective_components(
                    child,
                    &scenes[link.child_scene].objects,
                );
                let component = components
                    .iter()
                    .find(|c| c.class_name == "JPSTransformationComponent");
                let value = |name: &str| {
                    component
                        .and_then(|c| c.properties.iter().find(|p| p.name == name))
                        .map(|p| &p.value)
                };
                let name = value("Bone Attach Name")
                    .map(|v| v.as_str().context("Invalid attachment bone name"))
                    .transpose()?
                    .unwrap_or("");
                let mut socket = if name.is_empty() {
                    None
                } else {
                    Some(Socket {
                        name: name.to_ascii_lowercase(),
                        offset: value("Bone Attach Offset")
                            .map(|v| {
                                crate::render_support::scene::parse_point3(v)
                                    .context("Invalid bone offset")
                            })
                            .transpose()?
                            .unwrap_or([0.; 3]),
                        mode: value("Bone Attach Rotate Mode")
                            .map(|v| {
                                v.as_i64()
                                    .context("Invalid bone rotation mode")
                                    .and_then(|v| Ok(i32::try_from(v)?))
                            })
                            .transpose()?
                            .unwrap_or(0),
                        multiplier: value("Bone Scale Multiplier")
                            .map(|v| v.as_f64().context("Invalid bone scale multiplier"))
                            .transpose()?
                            .unwrap_or(0.) as f32,
                    })
                };
                let parent_components = crate::render_support::scene::get_effective_components(
                    parent,
                    &scenes[link.parent_scene].objects,
                );
                let animated = parent_components
                    .iter()
                    .any(|c| c.class_name == "AnimationComponent");
                let mut point = None;
                if let Some(socket) = socket.as_mut() {
                    let model = crate::render_support::scene::find_entity_nif_path_with_master(
                        parent,
                        &scenes[link.parent_scene].objects,
                    );
                    if animated {
                        if let Some(p) = super::em2_attach_points::load(
                            model.as_deref().unwrap_or(""),
                            &socket.name,
                            settings,
                        )? {
                            socket.name = p.bone;
                            socket.mode = 6;
                            point = Some(p.transform);
                        } else if socket.mode == 6 {
                            socket.mode = 0;
                        }
                    } else if socket.mode == 6 {
                        socket.mode = 0;
                    }
                }

                let mut model = None;
                if socket.is_some() && !animated {
                    if let Some(path) = crate::render_support::scene::find_entity_nif_path_with_master(
                        parent,
                        &scenes[link.parent_scene].objects,
                    ) {
                        let key = path.replace('\\', "/").to_ascii_lowercase();
                        if !models.contains_key(&key) {
                            let file = files
                                .iter()
                                .find(|f| f.path.replace('\\', "/").eq_ignore_ascii_case(&key))
                                .context("Attachment parent NIF absent")?;
                            let nif =
                                NifFile::load_bytes(&file.data).map_err(anyhow::Error::msg)?;
                            models.insert(
                                key.clone(),
                                Arc::new(super::scene_bounds::Hierarchy::load_with_source(
                                    &nif, &file.data,
                                )?),
                            );
                        }
                        model = models.get(&key).cloned();
                    }
                }
                let scale = crate::render_support::scene::get_entity_transform(
                    child,
                    &scenes[link.child_scene].objects,
                )
                .scale[0];
                ensure!(scale.is_finite(), "Nonfinite child scale");
                if let Some(socket) = &socket {
                    ensure!(
                        socket
                            .offset
                            .iter()
                            .chain(std::iter::once(&socket.multiplier))
                            .all(|v| v.is_finite()),
                        "Nonfinite child socket"
                    );
                }
                Ok(Binding {
                    parent: (link.parent_scene, link.parent),
                    child: (link.child_scene, link.child),
                    scale,
                    socket,
                    point,
                    animated,
                    model,
                    draws: Vec::new(),
                })
            })();
            match result {
                Ok(binding) => out.bindings.push(binding),
                Err(e) => out.errors.push(format!("Child{}: {e:#}", link.child)),
            }
        }
        out
    }
    pub fn bind_draws(&mut self, sources: &[super::model_body_binding::DrawSource]) {
        for b in &mut self.bindings {
            b.draws = sources
                .iter()
                .enumerate()
                .filter(|(_, s)| (s.scene, s.entity) == b.child)
                .map(|(i, _)| i)
                .collect();
        }
    }
    /// Draws attached (directly or through other attachments) to `parent`.
    pub fn attached_draws(&self, parent: (usize, u32)) -> Vec<usize> {
        let mut owners = vec![parent];
        let mut out = Vec::new();
        let mut i = 0;
        while i < owners.len() {
            let owner = owners[i];
            for b in self.bindings.iter().filter(|b| b.parent == owner) {
                if !owners.contains(&b.child) {
                    owners.push(b.child);
                    out.extend(&b.draws);
                }
            }
            i += 1;
        }
        out
    }
    pub fn update(
        &mut self,
        mut alive: impl FnMut(usize, u32) -> bool,
        mut owner: impl FnMut(usize, u32) -> Result<Transform>,
        mut actor: impl FnMut(usize, u32, &str) -> Result<Option<super::actor_bone_query::Pose>>,
        draws: &mut [crate::render_support::viewport3d::GpuDrawCommand],
    ) -> Vec<Write> {
        let mut writes = Vec::new();
        let mut computed = HashMap::new();
        for binding in &self.bindings {
            if !alive(binding.child.0, binding.child.1)
                || !alive(binding.parent.0, binding.parent.1)
            {
                continue;
            }
            let result = (|| -> Result<Transform> {
                let parent = if let Some(pose) = computed.get(&binding.parent) {
                    *pose
                } else {
                    owner(binding.parent.0, binding.parent.1)?
                };
                if let Some(socket) = &binding.socket {
                    let bone = if binding.animated {
                        actor(binding.parent.0, binding.parent.1, &socket.name)?
                            .context("Attachment actor graph unavailable")?
                    } else if let Some(model) = &binding.model {
                        let mut found = None;
                        for root in model.roots.iter().flatten() {
                            if let Some(node) = model.find_attachment_node(*root, &socket.name)? {
                                found = Some(node);
                                break;
                            }
                        }
                        if let Some(node) = found {
                            let root = NiTransform {
                                rotation: Matrix3 {
                                    rows: std::array::from_fn(|r| {
                                        std::array::from_fn(|c| parent[r * 3 + c])
                                    }),
                                },
                                translation: Vector3 {
                                    x: parent[9],
                                    y: parent[10],
                                    z: parent[11],
                                },
                                scale: parent[12],
                            };
                            let poses = model.base_world_transforms(Some(&root))?;
                            let node = poses.get(&node).context("Attachment node pose absent")?;
                            let pose = std::array::from_fn(|i| {
                                if i < 9 {
                                    node.rotation.rows[i / 3][i % 3]
                                } else if i < 12 {
                                    [node.translation.x, node.translation.y, node.translation.z]
                                        [i - 9]
                                } else {
                                    node.scale
                                }
                            });
                            super::attachment_direction::node_pose(parent, pose)?
                        } else {
                            super::actor_bone_query::DEFAULT
                        }
                    } else {
                        super::actor_bone_query::DEFAULT
                    };
                    let mut pose = super::skeletal_children::cache_with_point(
                        parent,
                        bone,
                        socket,
                        binding.point,
                    )?;
                    if socket.multiplier <= f32::EPSILON {
                        pose[12] = binding.scale;
                    }
                    ensure!(
                        pose.iter().all(|v| v.is_finite()),
                        "Nonfinite child attachment"
                    );
                    Ok(pose)
                } else {
                    let mut pose = parent;
                    pose[12] = binding.scale;
                    Ok(pose)
                }
            })();
            match result {
                Ok(pose) => {
                    let matrix = super::render_transform::from_entity(
                        pose[..9].try_into().unwrap(),
                        pose[9..12].try_into().unwrap(),
                        pose[12],
                    );
                    for &draw in &binding.draws {
                        draws[draw].trans_mat = matrix;
                    }
                    computed.insert(binding.child, pose);
                    writes.push(Write {
                        source: binding.child,
                        pose,
                    });
                    self.updates += 1;
                }
                Err(e) => {
                    let error = format!("Child{}: {e:#}", binding.child.1);
                    if !self.errors.contains(&error) {
                        self.errors.push(error);
                    }
                }
            }
        }
        writes
    }
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({"bindings":self.bindings.len(),"updates":self.updates,"errors":self.errors,"original_pc_pose_parity_verified":false,"scope":"shared live bone/NIF-node attachment and render poses; animation of nonactor NIF nodes, complete transform modes and PC numeric parity remain incomplete"})
    }
}
