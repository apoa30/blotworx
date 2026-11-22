//! Source static-body publication into the shared Box3D query backend.
//! Version-specific file decoding stays outside the simulation implementation.
use crate::render_support::scene;
use anyhow::{ensure, Context, Result};
use epic_mickey_lib_rs::{packfile::VirtualFile, scene_file::SceneFile};
use std::{collections::{HashMap,HashSet}, sync::Arc};

pub struct Loaded {
    pub collision: super::physics::Collision,
    pub report: serde_json::Value,
    /// Retain source identities/metadata for contact and script integration.
    pub owners: Vec<Owner>,
    /// Decoded dynamic sources retained for lifecycle/solver admission. These
    /// are not published as fixed query obstacles or stepped prematurely.
    pub dynamic_sources: Vec<Owner>,
    pub hidden_static_sources: Vec<Owner>,
    retired_owners:HashSet<usize>,
}
pub struct Owner {
    /// Value-stage snapshot only; activation must still run the full lifecycle.
    pub prepared_motion: Option<super::havok_2012_motion::Motion>,
    pub system: Option<Arc<super::physics_system::Imported>>,
    pub scene: usize,
    pub entity_link: u32,
    pub start_hidden: bool,
    pub entity: String,
    pub resource: Arc<Vec<super::havok_2012_geometry::Body>>,
    pub body: usize,
    pub entity_transform: [f32; 16],
}
impl Loaded {
    /// Body-owner keys belong to this collision world, not the scene registry.
    /// Resolve every body of a source before releasing its stable shape IDs.
    pub fn retire_source(&mut self,scene:usize,entity:u32)->Result<usize> {
        let mut count=0;
        for (index,owner) in self.owners.iter_mut().enumerate() {
            if owner.scene!=scene||owner.entity_link!=entity||self.retired_owners.contains(&index){continue;}
            self.collision.retire_owner(index)?;
            self.retired_owners.insert(index);
            owner.resource=Arc::new(Vec::new());owner.system=None;
            count+=1;
        }
        self.report["retired_static_bodies"]=serde_json::json!(self.retired_owners.len());
        Ok(count)
    }
    /// Source geometry overlay: triangle edges and convex support points.
    /// Radii are retained by physics, not approximated by a preview triangle fan.
    pub fn debug_lines(&self) -> Vec<crate::render_support::viewport3d::GpuLineSegment> {
        use crate::render_support::viewport3d::GpuLineSegment;
        let mut lines = Vec::new();
        let mut edges = std::collections::HashSet::new();
        for (index,owner) in self.owners.iter().enumerate() {
            if self.retired_owners.contains(&index){continue;}
            let body = &owner.resource[owner.body];
            for part in &body.parts {
                let vertices = part
                    .vertices
                    .iter()
                    .map(|&p| {
                        super::havok_2012_geometry::point(
                            &owner.entity_transform,
                            super::havok_2012_geometry::point(&body.transform, p),
                        )
                    })
                    .collect::<Vec<_>>();
                if part.convex {
                    for vertex in vertices {
                        for axis in 0..3 {
                            let mut start = vertex;
                            let mut end = vertex;
                            start[axis] -= 0.02;
                            end[axis] += 0.02;
                            lines.push(GpuLineSegment {
                                start,
                                end,
                                color: [1., 0.65, 0.1, 0.8],
                            });
                        }
                    }
                } else {
                    for ids in part.indices.chunks_exact(3) {
                        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
                            let start = vertices[ids[a] as usize];
                            let end = vertices[ids[b] as usize];
                            let first = start.map(f32::to_bits);
                            let second = end.map(f32::to_bits);
                            let key = if first <= second {
                                (first, second)
                            } else {
                                (second, first)
                            };
                            if first != second && edges.insert(key) {
                                lines.push(GpuLineSegment {
                                    start,
                                    end,
                                    color: [0.15, 0.9, 0.35, 0.65],
                                });
                            }
                        }
                    }
                }
            }
        }
        lines
    }
}
fn key(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches('/')
        .to_ascii_lowercase()
}
pub fn load(
    scenes: &[SceneFile],
    files: &[VirtualFile],
    hull: &super::character_hull::ConvexHull,
    spawn: [f32; 3],
) -> Result<Loaded> {
    let mut collision = super::physics::Collision::new(Vec::new())?;
    collision.set_player_hull(hull)?;
    let catalog = files
        .iter()
        .map(|file| (key(&file.path), file))
        .collect::<HashMap<_, _>>();
    let mut cache =
        HashMap::<String, Result<Arc<super::havok_2012_geometry::Resource>, String>>::new();
    let mut owners = Vec::new();
    let mut dynamic_sources = Vec::new();
    let mut hidden_static_sources = Vec::new();
    let mut dynamic_report = Vec::new();
    let mut errors = Vec::new();
    let mut deferred_dynamic = Vec::new();
    let mut triangles = 0usize;
    let mut convex_parts = 0usize;
    let mut scaled_entities = Vec::new();
    for (scene_index, source) in scenes.iter().enumerate() {
        for entity in &source.objects {
            if !source.scene.contains(&entity.link_id) {
                continue;
            }
            let components = scene::get_effective_components(entity, &source.objects);
            let startup =
                super::em2_entity_startup::State::decode(entity.unknown, entity.unknown_em2);
            let dynamic = components
                .iter()
                .any(|c| c.class_name == "PhysicsRigidBodyDynamicComponent");
            if dynamic {
                deferred_dynamic.push(entity.name.clone());
            }
            let Some(component) = components.iter().find(|c| {
                c.class_name == "PhysicsRigidBodyStaticComponent"
                    || c.class_name == "PhysicsRigidBodyDynamicComponent"
            }) else {
                continue;
            };
            let path = component
                .properties
                .iter()
                .find(|p| p.name == "RB Hull File Path")
                .and_then(|p| p.value.as_str())
                .unwrap_or("");
            if path.is_empty() {
                continue;
            }
            let resource_key = key(path);
            let loaded = cache.entry(resource_key.clone()).or_insert_with(|| {
                (|| -> Result<_> {
                    let file = catalog
                        .get(&resource_key)
                        .with_context(|| format!("Missing collision resource {path}"))?;
                    let file = epic_mickey_lib_rs::hkx_file::HkxFile::load_bytes(&file.data)
                        .map_err(anyhow::Error::msg)?;
                    Ok(Arc::new(super::havok_2012_geometry::load_selected(&file)?))
                })()
                .map_err(|e| format!("{e:#}"))
            });
            let selected = match loaded {
                Ok(resource) => resource.clone(),
                Err(error) => {
                    errors.push(
                        serde_json::json!({"entity":entity.name,"resource":path,"error":error}),
                    );
                    continue;
                }
            };
            let resource = selected.bodies.clone();
            let matrix = scene::runtime_matrix(entity, &source.objects);
            if dynamic {
                let settings = super::havok_2012_body_properties::Settings::read(component);
                let mut live_mass = settings.as_ref().map(|s| s.mass).unwrap_or(0.);
                for (body_index, body) in resource.iter().enumerate() {
                    let motion = &body.motion;
                    let prepared =
                        settings
                            .as_ref()
                            .map_err(|e| format!("{e:#}"))
                            .and_then(|settings| {
                                settings
                                    .prepare(motion, &mut live_mass)
                                    .map_err(|e| format!("{e:#}"))
                            });
                    dynamic_report.push(serde_json::json!({
                        "entity":entity.name,"resource":path,"body":body.name,
                        "motion_type":motion.motion_type,
                        "inverse_inertia_mass":motion.inverse_inertia_mass,
                        "damping":[motion.linear_damping,motion.angular_damping],
                        "gravity_factor":motion.gravity_factor,
                        "velocity_limit_codes":motion.velocity_limit_codes,
                        "velocity_limits":motion.velocity_limits,
                        "prepared_values":prepared.as_ref().ok().map(|m|serde_json::json!({"inverse_inertia_mass":m.inverse_inertia_mass,
                            "motion_type":m.motion_type,"damping":[m.linear_damping,m.angular_damping],
                            "gravity_factor":m.gravity_factor,"velocity_limits":m.velocity_limits})),
                        "preparation_error":prepared.as_ref().err(),
                        "live_component_mass":live_mass,"full_constructor_executed":false,
                        "solver_admitted":false
                        ,"start_hidden":startup.start_hidden
                    }));
                    dynamic_sources.push(Owner {
                        prepared_motion: prepared.ok(),
                        system: selected.system.clone(),
                        scene: scene_index,
                        entity_link: entity.link_id.to_u32(),
                        start_hidden: startup.start_hidden,
                        entity: entity.name.clone(),
                        resource: resource.clone(),
                        body: body_index,
                        entity_transform: matrix,
                    });
                }
                continue;
            }
            if startup.start_hidden {
                for body in 0..resource.len() {
                    hidden_static_sources.push(Owner {
                        prepared_motion: None,
                        system: selected.system.clone(),
                        scene: scene_index,
                        entity_link: entity.link_id.to_u32(),
                        start_hidden: true,
                        entity: entity.name.clone(),
                        resource: resource.clone(),
                        body,
                        entity_transform: matrix,
                    });
                }
                continue;
            }
            let scales: [f32; 3] = std::array::from_fn(|axis| {
                (0..3)
                    .map(|r| matrix[axis * 4 + r].powi(2))
                    .sum::<f32>()
                    .sqrt()
            });
            if scales.iter().any(|s| (*s - 1.).abs() > 1e-5) {
                scaled_entities.push(serde_json::json!({"entity":entity.name,"scale":scales}));
            }
            for (body_index, body) in resource.iter().enumerate() {
                // EM2 Studio playtests: a moving (non-fixed) body under a static component (the
                // Mean Street fire station bell, for one) is left out instead of failing the whole
                // level's collision. BLOTWORX_STRICT_EM2=1 keeps the original rejection.
                if body.motion_type != 5 && std::env::var_os("BLOTWORX_STRICT_EM2").is_none() {
                    continue;
                }
                let result = (|| -> Result<()> {
                    ensure!(body.motion_type==5,"Static component references non-fixed body {}; runtime override requires verification",body.motion_type);
                    let owner = owners.len();
                    // Validate every part before publishing any of this body's shapes.
                    let parts = body
                        .parts
                        .iter()
                        .map(|part| {
                            let vertices = part
                                .vertices
                                .iter()
                                .map(|&p| {
                                    super::havok_2012_geometry::point(
                                        &matrix,
                                        super::havok_2012_geometry::point(&body.transform, p),
                                    )
                                })
                                .collect::<Vec<_>>();
                            ensure!(
                                vertices.iter().flatten().all(|v| v.is_finite()),
                                "Nonfinite placed collision"
                            );
                            Ok((vertices, part))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    for (vertices, part) in parts {
                        if part.convex {
                            collision.add_static_convex(&vertices, part.radius, owner)?;
                            convex_parts += 1;
                        } else {
                            let source_triangles = part
                                .indices
                                .chunks_exact(3)
                                .map(|ids| super::physics::Triangle {
                                    p: [
                                        vertices[ids[0] as usize],
                                        vertices[ids[1] as usize],
                                        vertices[ids[2] as usize],
                                    ],
                                })
                                .collect::<Vec<_>>();
                            collision.add_static_triangles(
                                &source_triangles,
                                part.radius,
                                owner,
                            )?;
                            triangles += source_triangles.len();
                        }
                    }
                    // Every flattened shape still belongs to this source rigid
                    // body. Preserve its filter for body-level query consumers;
                    // per-child routing remains separate from this body word.
                    let shapes=collision.owner_shapes(owner).collect::<Vec<_>>();
                    for shape in shapes {collision.set_simulation_filter(shape,body.filter)?;}
                    owners.push(Owner {
                        prepared_motion: None,
                        system: selected.system.clone(),
                        scene: scene_index,
                        entity_link: entity.link_id.to_u32(),
                        start_hidden: false,
                        entity: entity.name.clone(),
                        resource: resource.clone(),
                        body: body_index,
                        entity_transform: matrix,
                    });
                    Ok(())
                })();
                if let Err(error) = result {
                    // A failed native admission may have published preceding parts.
                    // Refuse the world rather than silently retaining a partial body.
                    return Err(error).with_context(|| {
                        format!("Static body {} in {path} ({})", body.offset, entity.name)
                    });
                }
            }
        }
    }
    let ground = collision.ray_hit([spawn[0], spawn[1] + 1., spawn[2]], [0., -1., 0.], 20.);
    let ground=ground.map(|hit|serde_json::json!({"position":hit.position,"normal":hit.normal,"distance":hit.distance,
        "entity":owners.get(hit.owner).map(|owner|owner.entity.as_str()),"body":owners.get(hit.owner).map(|owner|owner.resource[owner.body].offset)}));
    let report = serde_json::json!({"backend":"shared Box3D","source_bodies":owners.len(),"triangles":triangles,"convex_parts":convex_parts,
        "decoded_resources":cache.values().filter(|v|v.is_ok()).count(),"resource_errors":errors,"deferred_dynamic_entities":deferred_dynamic,
        "dynamic_bodies":dynamic_report,
        "hidden_static_bodies":hidden_static_sources.len(),
        "selected_systems":cache.iter().filter_map(|(path,result)|result.as_ref().ok().map(|resource|
            serde_json::json!({"resource":path,"system_count":resource.system_count,"selected":resource.system.as_ref().map(|s|s.offset),
                "body_slots":resource.system.as_ref().map(|s|&s.system.bodies),
                "constraints":resource.system.as_ref().map(|s|&s.system.constraints),
                "actions":resource.system.as_ref().map(|s|&s.system.actions),
                "phantoms":resource.system.as_ref().map(|s|&s.system.phantoms),
                "active":resource.system.as_ref().map(|s|s.system.active)}))).collect::<Vec<_>>(),
        "scaled_entities":scaled_entities,"player_hull_admitted":true,"spawn_ground_query":ground,
        "player_movement_active":false,"solver_body_properties_applied":false,"placement_parity_verified":false});
    Ok(Loaded {
        collision,
        report,
        owners,
        dynamic_sources,
        hidden_static_sources,
        retired_owners:HashSet::new(),
    })
}
