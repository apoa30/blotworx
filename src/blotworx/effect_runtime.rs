//! Named FX instances own actual World handles, IO, timers and mutable particle
//! state. Immutable decoded models/materials are shared across invocations.
//! Complete native manager scheduling and particle simulation remain open.
use super::{
    assets::{Report, RuntimeEntity, World},
    brush_assets::{Assets, Definition},
    brush_particles::Hose,
    named_effects::{self, Backend, Pose},
    scene_bounds::Hierarchy,
};
use anyhow::{ensure, Context, Result};
use epic_mickey_lib_rs::{nif_file::NifFile, packfile::VirtualFile, scene_file::Component};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
    sync::Arc,
};

fn value<'a>(
    components: &'a [Component],
    class: &str,
    name: &str,
) -> Option<&'a serde_json::Value> {
    components
        .iter()
        .find(|c| c.class_name == class)?
        .properties
        .iter()
        .find(|p| p.name == name)
        .map(|p| &p.value)
}
fn flag(components: &[Component], name: &str) -> Result<bool> {
    value(components, "ParticleSystemControllerComponent", name)
        .and_then(|v| v.as_bool())
        .with_context(|| format!("Missing particle-controller boolean {name}"))
}
fn number(components: &[Component], name: &str) -> Result<f32> {
    let v = value(components, "ParticleSystemControllerComponent", name)
        .and_then(|v| v.as_f64())
        .with_context(|| format!("Missing particle-controller float {name}"))? as f32;
    ensure!(v.is_finite(), "Nonfinite particle-controller {name}");
    Ok(v)
}
struct Mesh {
    node: u32,
    path: Vec<u32>,
    draw: crate::render_support::viewport3d::GpuDrawCommand,
    inverse: [f32; 16],
    billboards: Vec<(u32,u16)>,
    lighting_slot: Option<u8>,
}
struct Prepared {
    visibility: super::effect_visibility::Runtime,
    definition: Arc<Definition>,
    prototype: Hose,
    hierarchy: Arc<Hierarchy>,
    meshes: Arc<Vec<Mesh>>,
    lifecycle: Arc<super::controller_lifecycle::Plan>,
    bind_world: Arc<BTreeMap<u32, epic_mickey_lib_rs::nif_file::NiTransform>>,
}
pub struct Live {
    visibility: super::effect_visibility::Runtime,
    pub owner: usize,
    pub template: String,
    pub fire_and_forget: bool,
    pub particles: Hose,
    animation: Option<super::nif_runtime::Instance>,
    hierarchy: Arc<Hierarchy>,
    lifecycle: Arc<super::controller_lifecycle::Plan>,
    pub started_at: Option<f32>,
    meshes: Arc<Vec<Mesh>>,
    bind_world: Arc<BTreeMap<u32, epic_mickey_lib_rs::nif_file::NiTransform>>,
    enabled: bool,
    wait: bool,
    pub drain: Option<u32>,
    pub ready: bool,
    pub age: f32,
    removal_locked: bool,
}
pub struct Library {
    resources: super::resource_store::Store,
    names: BTreeMap<String, usize>,
    definitions: Vec<Result<Arc<Definition>, String>>,
    prepared: BTreeMap<usize, Result<Arc<Prepared>, String>>,
    pub live: BTreeMap<usize, Live>,
    pub created: u64,
    pub completed: u64,
}
impl Live {
    fn validate_controllers(&self) -> Result<()> {
        for entry in &self.lifecycle.entries {
            let transform = self
                .animation
                .as_ref()
                .is_some_and(|a| a.owns_controller(entry.block));
            let particle = self.particles.owns_controller(entry.block);
            let owners = usize::from(transform)
                + usize::from(particle)
                + usize::from(self.visibility.owns(entry.block));
            ensure!(
                owners <= 1,
                "Named FX {} controller {} #{} (property {:?}) has multiple playback owners",
                self.template,
                entry.class_name,
                entry.block,
                entry.property
            );
        }
        Ok(())
    }
    fn restart(&mut self, time: f32) -> Result<()> {
        ensure!(time.is_finite(), "Nonfinite named-FX entity clock");
        self.validate_controllers()?;
        //36939c clears removal state and prepares/starts the existing model.
        // Never clone a replacement system or discard particles in flight.
        let ids = self.lifecycle.controller_ids();
        self.particles.prepare_effect_template(&ids);
        if let Some(animation) = &mut self.animation {
            animation.prepare_effect_template(&ids);
        }
        self.lifecycle.start(time, |id, time| {
            if let Some(animation) = self.animation.as_mut().filter(|a| a.owns_controller(id)) {
                animation.start_controller(id, time)
            } else if self.visibility.owns(id) {
                self.visibility.start(id)
            } else if self.particles.owns_controller(id) {
                self.particles.start_controller(id, time)
            } else {
                Ok(())
            }
        })?;
        //36a724(0) does not clear the counter once removal byte41 is set.
        //36939c still starts the controllers in that case.
        self.enabled = true;
        if !self.removal_locked {
            self.drain = None;
        }
        self.started_at = Some(time);
        Ok(())
    }
    fn stop_model(&mut self) -> Result<()> {
        self.validate_controllers()?;
        for entry in &self.lifecycle.entries {
            if let Some(animation) = self
                .animation
                .as_mut()
                .filter(|a| a.owns_controller(entry.block))
            {
                animation.stop_controller(entry.block)?;
            } else if self.visibility.owns(entry.block) {
                self.visibility.stop(entry.block)?;
            } else if self.particles.owns_controller(entry.block) {
                self.particles.stop_controller(entry.block)?;
            }
        }
        self.enabled = false;
        Ok(())
    }
    pub(super) fn begin_removal(&mut self, lock: bool) -> Result<()> {
        if !self.removal_locked && self.drain.is_none() {
            //36a724 non-ribbon branch stops the complete model, irrespective
            // of WaitForParticlesOnRemove. That property gates readiness only.
            self.stop_model()?;
            self.drain = Some(1);
        }
        //36a8ec locks after requesting Stop;31bd58's ordinary emitter Stop
        //does not. A lifetime/entity removal must not become cancellable.
        if lock {
            self.removal_locked = true;
        }
        Ok(())
    }
    pub fn append(
        &self,
        entity: &RuntimeEntity,
        eye: [f32; 3],
        right: [f32; 3],
        up: [f32; 3],
        lights: &[super::spherical_lighting::RegisteredLight],
        draws: &mut Vec<crate::render_support::viewport3d::GpuDrawCommand>,
    ) -> Result<()> {
        self.particles.append(right, up, draws)?;
        let world = self
            .animation
            .as_ref()
            .map(|a| &a.world)
            .unwrap_or(&self.bind_world);
        let back = super::physics::cross(right, up);
        let view = [
            right[0], up[0], back[0], 0.,
            right[1], up[1], back[1], 0.,
            right[2], up[2], back[2], 0.,
            0., 0., 0., 1.,
        ];
        for mesh in self.meshes.iter() {
            if !self.visibility.visible(&mesh.path) {continue;}
            let mut matrix = super::skin::matrix_product(
                &entity.transform,
                &super::nif_runtime::matrix(world.get(&mesh.node).context("FX mesh pose missing")?),
            );
            let mut accumulated=[1.,0.,0.,0.,0.,1.,0.,0.,0.,0.,1.,0.,0.,0.,0.,1.];
            for &(billboard,mode) in &mesh.billboards {
                let node=world.get(&billboard).context("FX billboard pose missing")?;
                let before=super::skin::matrix_product(&accumulated,&super::skin::matrix_product(&entity.transform,&super::nif_runtime::matrix(node)));
                let delta=super::model_billboard::camera_delta(mode,before,entity.authored_scale*node.scale,eye,right,up)?;
                matrix=super::skin::matrix_product(&delta,&matrix);
                accumulated=super::skin::matrix_product(&delta,&accumulated);
            }
            let mut draw = mesh.draw.clone();
            draw.trans_mat = super::skin::matrix_product(&matrix, &mesh.inverse);
            self.particles.publish_material(mesh.node, &mut draw)?;
            if let Some(animation) = &self.animation {
                animation.apply_material(mesh.node, &mut draw)?;
            }
            if let Some(slot) = mesh.lighting_slot {
                let images = Arc::make_mut(&mut draw.material_images);
                if let Some(image) = images.iter_mut().find(|image| image.slot == slot) {
                    let center = std::array::from_fn(|axis| {
                        (draw.aabb_min[axis] + draw.aabb_max[axis]) * 0.5
                    });
                    let position = super::physics::point(&draw.trans_mat, center);
                    image.rgba = Arc::new(super::spherical_lighting::view_texture(
                        lights.iter().map(|light| (&light.light, 1.)), position, &view,
                    )?);
                }
            }
            draws.push(draw);
        }
        Ok(())
    }
}
impl Library {
    pub(super) fn resource_store(&mut self) -> &mut super::resource_store::Store {
        &mut self.resources
    }
    pub(super) fn set_demo_enabled(
        &mut self,
        entity: usize,
        enabled: bool,
        time: f32,
    ) -> Result<bool> {
        let Some(effect) = self.live.get_mut(&entity) else {
            return Ok(false);
        };
        if enabled {
            effect.restart(time)?;
        } else {
            effect.stop_model()?;
        }
        Ok(true)
    }
    pub fn load(
        root: &Path,
        report: &Report,
        files: &[VirtualFile],
        entities: &[RuntimeEntity],
    ) -> Result<Self> {
        let mut library=Self {resources:super::resource_store::Store::new(root,&report.packs)?,names:BTreeMap::new(),definitions:Vec::new(),prepared:BTreeMap::new(),live:BTreeMap::new(),created:0,completed:0};
        library.admit(&report.packs,&report.scenes,files,entities)?;
        library.resources.palette_scenes()?;
        Ok(library)
    }
    pub(super) fn admit(&mut self,packs:&[String],scene_paths:&[String],files:&[VirtualFile],entities:&[RuntimeEntity])->Result<()> {
        if self.resources.admit_packs(packs)?>0 {self.prepared.retain(|_,prepared|prepared.is_ok());}
        let mut needed=false;
        for entity in entities {
            if let Some(array)=value(&entity.components,"FXComponent","FXTemplate") {
                for name in array.as_array().context("Invalid FXTemplate array")? {
                    let key=named_effects::lookup_key(name.as_str().context("Invalid FXTemplate name")?)?;
                    if !self.names.get(&key).is_some_and(|&id|self.definitions[id].is_ok()) {needed=true;}
                }
            }
        }
        if !needed {return Ok(());}
        let selected = scene_paths
            .iter()
            .map(|p| p.replace('\\', "/").to_ascii_lowercase())
            .collect::<HashSet<_>>();
        let mut seen = HashSet::new();
        let scenes = files
            .iter()
            .filter(|f| {
                let path = f.path.replace('\\', "/").to_ascii_lowercase();
                path.ends_with(".bin")
                    && (path.starts_with("palettes/") || selected.contains(&path))
                    && seen.insert(path)
            })
            .map(|f| {
                Ok((
                    f.path.clone(),
                    super::scene_loading::parse_scene(&f.path, &f.data)?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut palette_scenes: Option<Arc<Vec<(String, epic_mickey_lib_rs::scene_file::SceneFile)>>> = None;
        for entity in entities {
            if let Some(array) = value(&entity.components, "FXComponent", "FXTemplate") {
                for name in array.as_array().context("Invalid FXTemplate array")? {
                    let name = name.as_str().context("Invalid FXTemplate name")?;
                    let key = named_effects::lookup_key(name)?;
                    if self.names.get(&key).is_some_and(|&id|self.definitions[id].is_ok()) {
                        continue;
                    }
                    let mut definition = Assets::declarations(name, &scenes)
                        .and_then(|mut graph| {
                            graph.remove(&key).context("FX root declaration missing")
                        });
                    if definition.is_err() {
                        if palette_scenes.is_none() {
                            palette_scenes = self.resources.palette_scenes().ok();
                        }
                        if let Some(palettes) = &palette_scenes {
                            if let Ok(mut graph) = Assets::declarations(name, palettes) {
                                if let Some(found) = graph.remove(&key) {
                                    definition = Ok(found);
                                }
                            }
                        }
                    }
                    let definition = definition.map(Arc::new).map_err(|e| format!("{e:#}"));
                    if let Some(&id)=self.names.get(&key) {
                        if definition.is_ok() {self.definitions[id]=definition;self.prepared.remove(&id);}
                    } else {
                        self.names.insert(key,self.definitions.len());self.definitions.push(definition);
                    }
                }
            }
        }
        let death_effects = entities
            .iter()
            .filter_map(|entity| value(&entity.components, "HealthComponent", "NoHealthEffect"))
            .filter_map(|property| property.as_str())
            .filter(|name| !name.is_empty())
            .filter_map(|name| named_effects::lookup_key(name).ok())
            .filter_map(|name| self.names.get(&name).copied())
            .collect::<HashSet<_>>();
        for effect in death_effects {
            // Cache decoded one-shot resources in the scene loading phase.
            // Instance creation still reports any errors at the call site.
            let _ = self.prepare(effect);
        }
        Ok(())
    }
    fn prepare(&mut self, index: usize) -> Result<Arc<Prepared>> {
        if let Some(result) = self.prepared.get(&index) {
            return result.clone().map_err(anyhow::Error::msg);
        }
        let result = (|| -> Result<Arc<Prepared>> {
            let definition = self.definitions[index]
                .clone()
                .map_err(anyhow::Error::msg)?;
            if !definition.children.is_empty() {
                log::debug!(
                    "FX {} has {} child templates; instantiating root visual",
                    definition.name,
                    definition.children.len()
                );
            }
            for component in &definition.components {
                if !matches!(
                    component.class_name.as_str(),
                    "JPSTransformationComponent"
                        | "JPSSceneGraphComponent"
                        | "ParticleSystemControllerComponent"
                        | "LifetimeComponent"
                        | "LODComponent"
                        | "GameEventsComponent"
                        | "InputOutputComponent"
                        | "DynamicallyLitComponent"
                ) {
                    log::debug!(
                        "FX {} auxiliary component {} ignored during visual preparation",
                        definition.name,
                        component.class_name
                    );
                }
            }
            let _ = number(&definition.components, "InheritVelocityMultiplier");
            let _ = flag(&definition.components, "InheritVelocityFromParentEntity");
            let _ = number(&definition.components, "RestartTimeMin");
            let _ = number(&definition.components, "RestartTimeMax");
            let model = self.resources.read(
                definition
                    .model
                    .as_deref()
                    .context("FX root has no model")?,
            )?;
            let nif = NifFile::load_bytes(&model.data).map_err(anyhow::Error::msg)?;
            let files = self.resources.particle_files(&model.data)?;
            let mut prototype = Hose::load_attached(&model.data, &files)?;
            let graph = Hierarchy::load(&nif)?;
            let root = nif
                .roots
                .first()
                .copied()
                .flatten()
                .context("FX model has no first streamed root")?;
            ensure!(
                super::nif_types::is_a(super::nif_types::block_name(&nif, root)?, "NiAVObject")?,
                "FX model root is not a NiAVObject"
            );
            let mut hierarchy = graph.subtree(root)?;
            let lifecycle = Arc::new(super::controller_lifecycle::Plan::load(
                &nif, &hierarchy, root,
            )?);
            let ids = lifecycle.controller_ids();
            for node in hierarchy.nodes.values_mut() {
                for c in &mut node.controllers {
                    if ids.contains(&c.block) {
                        c.flags = super::nif_runtime::effect_template_flags(c.flags);
                    }
                }
            }
            prototype.prepare_effect_template(&ids);
            let hierarchy = Arc::new(hierarchy);
            let visibility = super::effect_visibility::Runtime::load(&nif, &hierarchy)?;
            let geometry = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::render_support::nif::build_cached_scene(&nif)
            }))
            .map_err(|_| anyhow::anyhow!("FX geometry parser panicked: {}", definition.name))?;
            let raw = super::jigsaw::raw_blocks(&model.data)?;
            let parents = hierarchy
                .nodes
                .iter()
                .flat_map(|(&parent, node)| {
                    node.children
                        .iter()
                        .flatten()
                        .map(move |child| (*child, parent))
                })
                .collect::<BTreeMap<_, _>>();
            let mut meshes = Vec::new();
            let mut textures = HashMap::new();
            for (node, block) in nif
                .blocks
                .iter()
                .enumerate()
                .filter(|(_, b)| matches!(b, epic_mickey_lib_rs::nif_file::NiObject::NiMesh(_)))
            {
                if !hierarchy.nodes.contains_key(&(node as u32)) {
                    continue;
                }
                let mut path = Vec::new();
                let mut current = Some(node as u32);
                while let Some(id) = current {
                    ensure!(!path.contains(&id), "Cyclic FX mesh hierarchy");
                    path.push(id);
                    current = parents.get(&id).copied();
                }
                // The outward effect's five meshes are hidden surface emitters,
                // not additional rendered copies of the gremlin.
                if path.iter().any(|id| hierarchy.nodes[id].flags & 1 != 0)
                    && !path.iter().any(|id|hierarchy.nodes[id].controllers.iter().any(|c|c.class_name=="NiVisController")) {
                    continue;
                }
                ensure!(
                    path.iter().all(|id| hierarchy.nodes[id]
                        .controllers
                        .iter()
                        .all(|c| matches!(c.class_name.as_str(), "NiTransformController" | "NiVisController")
                            || c.color.is_some() || c.texture_transform.is_some() || c.indirect_scale.is_some())),
                    "FX mesh visibility/property controller is not connected: {}", definition.name
                );
                let billboards = path
                    .iter().rev()
                    .filter(|id| {
                        nif.block_types[nif.block_type_indices[**id as usize] as usize]
                            == "NiBillboardNode"
                    })
                    .copied()
                    .collect::<Vec<_>>();
                let mut billboard_modes=Vec::new();
                for &id in &billboards {
                    let bytes = raw[id as usize]
                        .get(raw[id as usize].len() - 2..)
                        .context("Billboard mode missing")?;
                    let mode = if nif.source_is_little_endian {
                        u16::from_le_bytes(bytes.try_into()?)
                    } else {
                        u16::from_be_bytes(bytes.try_into()?)
                    } & 7;
                    ensure!(mode <= 4, "Unknown FX billboard mode {mode}");
                    billboard_modes.push((id,mode));
                }
                let parts = geometry
                    .submeshes
                    .iter()
                    .filter(|mesh| mesh.mesh_index == node)
                    .collect::<Vec<_>>();
                ensure!(
                    !parts.is_empty(),
                    "FX mesh geometry failed to decode: {}:{}",
                    definition.name,
                    block.name()
                );
                for mesh in parts {
                    let shader = mesh
                        .material
                        .shader
                        .as_ref()
                        .context("FX mesh has no shader")?;
                    let filename = format!("{shader}.r3mt");
                    let source = files
                        .iter()
                        .rev()
                        .find(|f| {
                            f.path
                                .replace('\\', "/")
                                .rsplit('/')
                                .next()
                                .is_some_and(|name| name.eq_ignore_ascii_case(&filename))
                        })
                        .context("FX mesh shader source missing")?;
                    let registers = super::brush_particles::authored_registers(
                        &nif,
                        &block.as_av_object().unwrap().base.extra_data_links,
                    )?;
                    let program = Arc::new(
                        crate::render_support::material_preview::compile_native_with_registers(
                            std::str::from_utf8(&source.data)?,
                            &mesh.material,
                            &registers,
                        )
                        .map_err(anyhow::Error::msg)?,
                    );
                    let lighting_slot = program.first_runtime_sh_slot(&mesh.material)
                        .map(u8::try_from).transpose()?;
                    let runtime_lighting = lighting_slot.map(|slot| {
                        use super::spherical_lighting::SIZE;
                        crate::render_support::material_preview::MaterialImage {
                            slot,
                            rgba: Arc::new(vec![0; SIZE * SIZE * 4]),
                            width: SIZE as u32,
                            height: SIZE as u32,
                            sampler: Default::default(),
                            matrix: [1., 0., 0., 0., 1., 0., 0., 0., 1.],
                        }
                    });
                    let images = crate::render_support::material_preview::images_with_runtime_sh(
                        &program,
                        &mesh.material,
                        &files,
                        &mut textures,
                        runtime_lighting.as_ref(),
                    )
                    .map_err(anyhow::Error::msg)?;
                    let mut min = [f32::INFINITY; 3];
                    let mut max = [f32::NEG_INFINITY; 3];
                    for p in &mesh.positions {
                        for i in 0..3 {
                            min[i] = min[i].min(p[i]);
                            max[i] = max[i].max(p[i]);
                        }
                    }
                    meshes.push(Mesh {
                        node: node as u32,
                        path: path.clone(),
                        inverse: super::nif_runtime::inverse(mesh.mesh_to_model)?,
                        billboards: billboard_modes.clone(),
                        lighting_slot,
                        draw: crate::render_support::viewport3d::GpuDrawCommand {
                            shader_time:None,source_attributes: mesh.source_attributes.clone(),
                            extra_uvs: mesh.extra_uvs.clone(),
                            projection_bound: None,
                            projected_shadow: false,
                            skybox: false,
                            material_registers: None,
                            material_indirect: None,
                            material_images: Arc::new(images),
                            baked: false,
                            baked_error: None,
                            preview: Some(Ok(program)),
                            uv_transform: mesh.uv_transform,
                            material: Some(mesh.material.clone()),
                            sampler: mesh.sampler,
                            v_data: mesh.v_data.clone(),
                            indices: mesh.indices.clone(),
                            trans_mat: [
                                1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
                            ],
                            color: [1.; 4],
                            tex_bytes: None,
                            aabb_min: min,
                            aabb_max: max,
                            light_group: Arc::from(""),
                        },
                    });
                }
            }
            let bind_world = Arc::new(hierarchy.base_world_transforms(None)?);
            // Validate direct model controllers before publishing an entity.
            let animation = super::nif_runtime::Instance::load(&hierarchy)?;
            for entry in &lifecycle.entries {
                let transform = animation
                    .as_ref()
                    .is_some_and(|a| a.owns_controller(entry.block));
                ensure!(
                    usize::from(transform) + usize::from(prototype.owns_controller(entry.block)) + usize::from(visibility.owns(entry.block))
                        == 1,
                    "Named FX {} controller {} #{} needs one actual playback owner",
                    definition.name,
                    entry.class_name,
                    entry.block
                );
            }
            Ok(Arc::new(Prepared {
                visibility,
                definition,
                prototype,
                hierarchy,
                lifecycle,
                meshes: Arc::new(meshes),
                bind_world,
            }))
        })();
        self.prepared.insert(
            index,
            result
                .as_ref()
                .map(Arc::clone)
                .map_err(|e| format!("{e:#}")),
        );
        result
    }
    pub fn retire(&mut self, entity: usize) {
        if self.live.remove(&entity).is_some() {
            self.completed += 1;
        }
    }
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({"created":self.created,"completed":self.completed,"resources":self.resources.summary(),
            "prepared_models":self.prepared.values().filter(|v|v.is_ok()).count(),
            "errors":self.prepared.iter().filter_map(|(i,v)|v.as_ref().err().map(|e|serde_json::json!({"template":self.definitions[*i].as_ref().ok().map(|d|&d.name),"error":e}))).collect::<Vec<_>>(),
            "live":self.live.iter().map(|(entity,e)|serde_json::json!({"entity":entity,"owner":e.owner,"template":e.template,
                "age":e.age,"fire_and_forget":e.fire_and_forget,"drain_updates":e.drain,"particles":e.particles.particle_count(),
                "controller_started_at":e.started_at,"controller_clock":"default entity scaled seconds", "random_owner":"shared game stream",
                "removal_locked":e.removal_locked,
                "mesh_draws":e.meshes.len(),"mesh_emitters":e.particles.mesh_emission_summary(),
                "colors":e.particles.color_summary(),"render_probe":e.particles.render_probe()})).collect::<Vec<_>>()})
    }
}

struct Factory<'a> {
    world: &'a mut World,
    owner: usize,
}
impl Backend for Factory<'_> {
    fn child_factory(&mut self) -> Result<Option<usize>> {
        Ok(self.world.entities[self.owner]
            .components
            .iter()
            .any(|c| c.class_name == "ChildComponent")
            .then_some(self.owner))
    }
    fn existing_child(&mut self, _: usize, name: &str) -> Result<Option<usize>> {
        self.world.child_entity_by_name(self.owner, name)
    }
    fn particle_controller(&mut self, entity: usize) -> Result<Option<usize>> {
        if self.world.named_fx.live.contains_key(&entity) {
            return Ok(Some(entity));
        }
        ensure!(
            !self.world.entities[entity]
                .components
                .iter()
                .any(|c| c.class_name == "ParticleSystemControllerComponent"),
            "Existing particle entity has not been initialized by the native FX factory"
        );
        Ok(None)
    }
    fn set_pose(&mut self, entity: usize, pose: &Pose) -> Result<()> {
        let e = &mut self.world.entities[entity];
        e.runtime_rotation = pose[..9].try_into().unwrap();
        e.authored_scale = pose[12];
        e.transform = super::render_transform::from_entity(
            e.runtime_rotation,
            pose[9..12].try_into().unwrap(),
            pose[12],
        );
        Ok(())
    }
    fn restart_particles(&mut self, entity: usize) -> Result<()> {
        let time = self.world.frame_clock.clock.entity_seconds(true);
        self.world
            .named_fx
            .live
            .get_mut(&entity)
            .context("Persistent FX model is not initialized")?
            .restart(time)
    }
    fn retain_child(&mut self, entity: usize) -> Result<()> {
        //31bc24 checks the owner's current hidden byte34. The child callback
        //1ecf9c snapshots its own current state into byte35; its zero argument
        //does not clear either flag.297844(1) then hides the retained child.
        if self.world.hidden_entities.contains(&self.owner) {
            self.world.snapshot_entity_hidden(entity);
            self.world.set_entity_hidden(entity, true)?;
        }
        Ok(())
    }
    fn template(&mut self, name: &str) -> Result<Option<usize>> {
        let key = named_effects::lookup_key(name)?;
        let Some(array) = value(
            &self.world.entities[self.owner].components,
            "FXComponent",
            "FXTemplate",
        ) else {
            return Ok(None);
        };
        let mut admitted = false;
        for name in array.as_array().context("Invalid FXTemplate array")? {
            if named_effects::lookup_key(name.as_str().context("Invalid FXTemplate name")?)? == key
            {
                admitted = true;
                break;
            }
        }
        if !admitted {
            return Ok(None);
        }
        Ok(Some(*self.world.named_fx.names.get(&key).with_context(||
            format!("FX template from an incoming scene has not been admitted to the runtime resource store: {name}"))?))
    }
    fn template_lifetime(&mut self, index: usize) -> Result<Option<f32>> {
        let definition = self.world.named_fx.definitions[index]
            .as_ref()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(
            value(&definition.components, "LifetimeComponent", "Lifetime")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32),
        )
    }
    fn create_child(
        &mut self,
        _: usize,
        index: usize,
        fire_and_forget: bool,
    ) -> Result<Option<usize>> {
        // Resource/controller admission completes before any entity is published.
        self.world
            .create_named_effect(self.owner, index, fire_and_forget)
            .map(Some)
    }
}

impl World {
    pub(super) fn stop_named_effect(&mut self, owner: usize, name: &str) -> Result<()> {
        if !self.entities[owner]
            .components
            .iter()
            .any(|c| c.class_name == "FXComponent")
        {
            return Ok(());
        }
        if let Some(child) = self.child_entity_by_name(owner, name)? {
            if let Some(controller) = (Factory { world: self, owner }).particle_controller(child)? {
                self.named_fx
                    .live
                    .get_mut(&controller)
                    .context("Named particle-controller owner missing")?
                    .begin_removal(false)?;
            }
        }
        Ok(())
    }
    ///2f0b04/2f0bb4 route by the payload string without the ordinary kind9
    ///fallback.31b974 obtains fire-and-forget from the owner's FX table.
    pub(super) fn dispatch_animation_emitter(
        &mut self,
        owner: usize,
        id: u32,
        name: &str,
    ) -> Result<()> {
        ensure!(
            matches!(id, 0x214 | 0x215 | 0x217),
            "Invalid animation emitter command"
        );
        ensure!(
            owner < self.entities.len() && !self.removed_entities.contains(&owner),
            "Invalid animation emitter owner"
        );
        if !self.entities[owner]
            .components
            .iter()
            .any(|c| c.class_name == "FXComponent")
        {
            return Ok(());
        }
        if id == 0x215 {
            return self.stop_named_effect(owner, name);
        }
        let key = named_effects::lookup_key(name)?;
        let Some(templates) = value(
            &self.entities[owner].components,
            "FXComponent",
            "FXTemplate",
        ) else {
            return Ok(());
        };
        let templates = templates.as_array().context("Invalid FXTemplate array")?;
        let mut selected = None;
        for (index, template) in templates.iter().enumerate() {
            if named_effects::lookup_key(template.as_str().context("Invalid FXTemplate name")?)?
                == key
            {
                selected = Some(index);
                break;
            }
        }
        let Some(index) = selected else { return Ok(()) }; //31b788 missing-entry admission.
                                                           //2f0b54..88: push only after template lookup, before attempting Start.
                                                           // Ten is the original retained-name limit, not an emission limit:
                                                           // the eleventh and later requests still start their effects.
        if id == 0x217 {
            let stack = self
                .controller_feedback
                .pushed_emitters
                .entry(owner)
                .or_default();
            if stack.len() < 10 {
                stack.push(name.to_owned());
            }
        }
        if !self.entities[owner]
            .components
            .iter()
            .any(|c| c.class_name == "ChildComponent")
        {
            return Ok(());
        }
        let flags = value(
            &self.entities[owner].components,
            "FXComponent",
            "FXTemplateFireAndForget",
        )
        .and_then(|v| v.as_array())
        .context("Missing FXTemplateFireAndForget array")?;
        let forget = flags
            .get(index)
            .and_then(|v| v.as_bool())
            .context("Missing named-FX fire-and-forget flag")?;
        named_effects::start(&mut Factory { world: self, owner }, name, None, forget)?;
        Ok(())
    }
    /// Detached one-shot effects survive their source entity's hide/removal.
    pub(super) fn start_one_shot_effect(&mut self, owner: usize, name: &str) -> Result<()> {
        named_effects::start(&mut Factory { world: self, owner }, name, None, true)?;
        Ok(())
    }
    /// Starts a palette-defined FX template (such as `BlastConePaint` or
    /// `BlastConeThinner` from `_Dynamic.pak`) directly at an explicit pose.
    pub(super) fn start_palette_effect(
        &mut self,
        owner: usize,
        name: &str,
        pose: Option<&Pose>,
        fallback_lifetime: Option<f32>,
    ) -> Result<Option<usize>> {
        let key = named_effects::lookup_key(name)?;
        let index = if let Some(&id) = self.named_fx.names.get(&key) {
            if self.named_fx.definitions[id].is_ok() {
                id
            } else {
                let palettes = self.named_fx.resources.palette_scenes()?;
                let Ok(mut graph) = Assets::declarations(name, &palettes) else {
                    return Ok(None);
                };
                let Some(found) = graph.remove(&key) else {
                    return Ok(None);
                };
                self.named_fx.definitions[id] = Ok(Arc::new(found));
                self.named_fx.prepared.remove(&id);
                id
            }
        } else {
            let palettes = self.named_fx.resources.palette_scenes()?;
            let Ok(mut graph) = Assets::declarations(name, &palettes) else {
                return Ok(None);
            };
            let Some(found) = graph.remove(&key) else {
                return Ok(None);
            };
            let id = self.named_fx.definitions.len();
            self.named_fx.names.insert(key, id);
            self.named_fx.definitions.push(Ok(Arc::new(found)));
            id
        };
        let entity = self.create_named_effect(owner, index, true)?;
        if let Some(pose) = pose {
            Factory { world: self, owner }.set_pose(entity, pose)?;
        }
        if let Some(seconds) = fallback_lifetime {
            if seconds > 0. && !self.runtime_lifetimes.contains_key(&entity) {
                self.runtime_lifetimes.insert(
                    entity,
                    super::lifetime::State {
                        timer: super::brush::Lifetime {
                            remaining: seconds,
                            ..Default::default()
                        },
                        pause_during_igc: false,
                        pause_all_during_igc: false,
                        shrink: false,
                    },
                );
            }
        }
        Ok(Some(entity))
    }
    pub(super) fn named_effect_lua(
        &mut self,
        lua: &mlua::Lua,
        api: &str,
        mut args: mlua::MultiValue,
    ) -> mlua::Result<mlua::MultiValue> {
        let forget = api == "StartEmitterByNameFireForget";
        if (!forget && args.len() != 2) || (forget && args.len() < 2) {
            return Err(mlua::Error::external(format!(
                "{api}: native argument-count diagnostic is not reconstructed"
            )));
        }
        let use_rotation = if forget && args.len() > 2 {
            super::actor_scripts::native_boolean(lua, args.pop_back().unwrap())?
        } else {
            true
        };
        let name = lua.coerce_string(args.pop_back().unwrap())?;
        let name = name
            .map(|s| {
                s.to_str()
                    .map(|s| s.split('\0').next().unwrap_or("").to_owned())
            })
            .transpose()?
            .unwrap_or_default();
        let owner = self
            .scripts
            .resolve_native_entity_argument(lua, args.pop_back().unwrap())?;
        if let Some(owner) = owner.filter(|owner| !self.removed_entities.contains(owner)) {
            if api == "StopEmitterByName" {
                self.stop_named_effect(owner, &name)
                    .map_err(mlua::Error::external)?;
                return Ok(mlua::MultiValue::new());
            }
            let pose = (!use_rotation).then(|| {
                let mut pose = [0.; 13];
                pose[0] = 1.;
                pose[4] = 1.;
                pose[8] = 1.;
                pose[12] = 1.;
                pose[9..12].copy_from_slice(&self.entities[owner].transform[12..15]);
                pose
            });
            named_effects::start(
                &mut Factory { world: self, owner },
                &name,
                pose.as_ref(),
                forget,
            )
            .map_err(mlua::Error::external)?;
        }
        Ok(mlua::MultiValue::new())
    }
    fn create_named_effect(
        &mut self,
        owner: usize,
        index: usize,
        fire_and_forget: bool,
    ) -> Result<usize> {
        let prepared = self.named_fx.prepare(index)?;
        let definition = &prepared.definition;
        for warning in &prepared.prototype.warnings {
            if !self.report.warnings.contains(warning) {
                self.report.warnings.push(warning.clone());
            }
        }
        let source = epic_mickey_lib_rs::scene_file::Entity::new(
            "JPSGeneralEntity".into(),
            definition.name.clone(),
            epic_mickey_lib_rs::scene_file::ID::new(1),
            epic_mickey_lib_rs::scene_file::ID::new(0),
            0,
            0,
            definition.components.clone(),
        );
        let authored = crate::render_support::scene::get_entity_transform(&source, &[]);
        let scale = authored.scale[0];
        ensure!(
            scale.is_finite() && authored.scale == [scale; 3],
            "Nonuniform effect entity scale"
        );
        let particles = prepared
            .prototype
            .instantiate_subtree(prepared.hierarchy.clone())?;
        let animation = super::nif_runtime::Instance::load(&prepared.hierarchy)?;
        let enabled = flag(&definition.components, "StartEmittersEnabled").unwrap_or(true);
        let wait = flag(&definition.components, "WaitForParticlesOnRemove").unwrap_or(false);
        let transform_component = definition
            .components
            .iter()
            .find(|c| c.class_name == "JPSTransformationComponent")
            .context("Effect has no transform")?;
        let socket = super::skeletal_children::Socket::load(transform_component)?;
        let local_position = value(
            &definition.components,
            "JPSTransformationComponent",
            "Local Translation",
        )
        .and_then(crate::render_support::scene::parse_point3)
        .unwrap_or([0.; 3]);
        let local_rotation = value(
            &definition.components,
            "JPSTransformationComponent",
            "Local Rotation",
        )
        .and_then(crate::render_support::scene::parse_matrix3)
        .map(|m| std::array::from_fn(|i| m[i / 3][i % 3]))
        .unwrap_or([1., 0., 0., 0., 1., 0., 0., 0., 1.]);
        let mut live = Live {
            visibility: prepared.visibility.clone(),
            owner,
            template: definition.name.clone(),
            fire_and_forget,
            particles,
            animation,
            lifecycle: prepared.lifecycle.clone(),
            started_at: None,
            hierarchy: prepared.hierarchy.clone(),
            meshes: prepared.meshes.clone(),
            bind_world: prepared.bind_world.clone(),
            enabled,
            wait,
            drain: None,
            ready: false,
            age: 0.,
            removal_locked: false,
        };
        if enabled {
            live.restart(self.frame_clock.clock.entity_seconds(true))?;
        } else {
            live.stop_model()?;
        }
        let entity = self.entities.len();
        let parent = &self.entities[owner];
        let id = self
            .entities
            .iter()
            .map(|e| e.id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .context("Effect entity ID exhausted")?;
        let rotation = parent.runtime_rotation;
        let transform = super::render_transform::from_entity(
            rotation,
            parent.transform[12..15].try_into().unwrap(),
            scale,
        );
        let asset = definition
            .model
            .as_ref()
            .unwrap()
            .replace('\\', "/")
            .to_ascii_lowercase();
        self.entities.push(RuntimeEntity {
            authored_scale: scale,
            authored_rotation: rotation,
            runtime_rotation: rotation,
            model_asset: Some(asset.clone()),
            mesh_draws: HashMap::new(),
            jigsaw_asset: None,
            jigsaw_states: Vec::new(),
            scene: parent.scene.clone(),
            id,
            name: definition.name.clone(),
            transform,
            authored_transform: transform,
            components: definition.components.clone(),
            draws: self.draws.len()..self.draws.len(),
            collision_parts: 0..0,
        });
        self.model_bounds
            .entry(asset)
            .or_insert_with(|| (*prepared.hierarchy).clone());
        self.named_fx.live.insert(entity, live);
        self.named_fx.created += 1;
        let result = (|| -> Result<()> {
            // Native312aa0 attaches both forms, then detaches fire-and-forget.
            self.transform_hierarchy.insert(
                entity,
                super::rotator_runtime::Local {
                    parent: owner,
                    position: local_position,
                    rotation: local_rotation,
                    socket,
                },
            );
            super::rotator_runtime::propagate(
                owner,
                &self.transform_hierarchy,
                &mut self.entities,
                &mut self.draws,
            )?;
            self.update_skeletal_children_range_strict(entity..entity + 1)?;
            if fire_and_forget {
                self.transform_hierarchy.remove(&entity);
            }
            self.scripts
                .register_entity(entity, &definition.name, &[])?;
            self.outputs
                .bind(entity, &definition.name, &definition.components)?;
            self.bind_entity_lifetime(entity)?;
            for c in &definition.components {
                if c.class_name == "GameEventsComponent" {
                    self.component_events.bind(entity, c)?;
                }
                if c.class_name == "LODComponent" {
                    self.lod_states
                        .insert(entity, super::lod::State::constructed());
                }
            }
            self.emit_entity_output(entity, "OnSpawned", entity);
            // Use the actual World callback host. A detached Lua scheduler
            // queues these deliveries with the real owner/activator context.
            let deliveries = self.outputs.ready_for_owner(self.animation_time, entity);
            self.dispatch_actor_deliveries(deliveries);
            Ok(())
        })();
        if let Err(error) = result {
            self.named_fx.live.get_mut(&entity).unwrap().ready = true;
            self.retire_entity(entity)?;
            return Err(error);
        }
        let warning="Named FX use actual entities, lifetimes, hidden-state retention, controller start/stop, shared scaled clock and shared game RNG. Complete model/material types, entity clock-selector changes, native manager/update ordering and particle simulation parity remain incomplete.";
        if !self.report.unsupported.iter().any(|s| s == warning) {
            self.report.unsupported.push(warning.into());
        }
        Ok(entity)
    }
    pub(super) fn advance_named_effects(&mut self, dt: f32) -> Result<()> {
        let mut finished = Vec::new();
        let time = self.frame_clock.clock.entity_seconds(true);
        let active = self
            .named_fx
            .live
            .keys()
            .copied()
            .filter(|&entity| self.entity_scene_active(entity))
            .collect::<std::collections::BTreeSet<_>>();
        for (&entity, effect) in &mut self.named_fx.live {
            if !active.contains(&entity) {
                continue;
            }
            effect.age += dt;
            effect.visibility.update(time)?;
            if let Some(animation) = &mut effect.animation {
                animation.update(&effect.hierarchy, time)?;
                effect
                    .particles
                    .publish_model_transforms(&animation.world)?;
            }
            effect.particles.advance_attached_at(
                time,
                self.entities[entity].transform,
                &mut self.game_random,
            )?;
            if let Some(counter) = &mut effect.drain {
                *counter += 1;
                // REL36a600 uses an update count, not a guessed seconds timer.
                effect.ready =
                    *counter > 90 || !effect.wait || effect.particles.particle_count() == 0;
                if effect.ready {
                    finished.push(entity);
                }
            }
        }
        for entity in finished {
            self.retire_entity(entity)?;
        }
        Ok(())
    }
}

#[cfg(any(test, debug_assertions))]
fn fixture_frame(world: &mut World) -> Result<()> {
    // Explicit reference driver: model/controller time comes from the same
    // shared clock as production, never an implicit per-effect delta clock.
    world.frame_clock.publish(true, true);
    world.advance_models_with_delta(world.animation_time + super::STEP, super::STEP)
}
#[cfg(any(test, debug_assertions))]
pub fn validate_original_animation_emitter() -> Result<()> {
    let reference: serde_json::Value = serde_json::from_str(include_str!(
        "../../tools/blotworx/fixtures/effect_restart.json"
    ))?;
    let mut clock =
        super::game_clock::Clock::new(super::game_clock::Rate::from_bus_clock(0x0e7be2c0)?, 0);
    clock.unscaled_seconds = 12.345678901;
    clock.scaled_seconds = 23.456789012;
    for case in reference["cases"].as_array().unwrap() {
        if let Some(call) = case["calls"]
            .as_array()
            .unwrap()
            .iter()
            .find(|call| call[0] == "start_animations")
        {
            ensure!(
                u64::from(
                    clock
                        .entity_seconds(case["scaled"].as_bool().unwrap())
                        .to_bits()
                ) == call[1].as_u64().unwrap(),
                "Native entity clock conversion differs from1eff10"
            );
        }
    }
    let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
    let mut world = super::assets::load(
        Path::new(&root),
        "Levels/GV_ZoneA_Start.level",
        &egui::Context::default(),
    )?;
    let owner = world
        .entities
        .iter()
        .position(|e| e.name == "gremlinvillage_zonea.SpatterMelee 02")
        .context("Original Spatter absent")?;
    let graph = world.authored_havok.graph(owner)?;
    let payload = graph
        .objects
        .iter()
        .find(|(_, o)| {
            o.klass == "hkbStringEventPayload"
                && o.fields["data"].as_str() == Some("BeetleworxFodderKnockbackDust")
        })
        .map(|(id, _)| super::state_machine::Reference { id: id.clone() })
        .context("Original dust animation payload absent")?;
    let event = super::state_machine::Event {
        id: 0x214,
        payload: Some(payload.clone()),
    };
    world.frame_clock.publish(true, true);
    let owner_name = serde_json::to_string(&world.entities[owner].name)?;
    let before = world.named_fx.created;
    world.dispatch_exported_animation_event(owner, &event)?;
    ensure!(
        world.named_fx.created == before + 1,
        "Animation event did not instantiate original dust effect"
    );
    let entity = world.entities.len() - 1;
    ensure!(
        world.named_fx.live[&entity].fire_and_forget,
        "Animation FX ignored its owner's authored fire-and-forget flag"
    );
    ensure!(
        !world.transform_hierarchy.contains_key(&entity),
        "Transient animation FX retained its owner attachment"
    );
    ensure!(
        world.runtime_lifetimes.contains_key(&entity),
        "Animation FX did not bind original Lifetime"
    );
    world.dispatch_exported_animation_event(
        owner,
        &super::state_machine::Event {
            id: 0x215,
            payload: Some(payload.clone()),
        },
    )?;
    ensure!(
        world.named_fx.live[&entity].drain.is_none(),
        "Emitter stop found a detached fire-and-forget child"
    );
    world.dispatch_exported_animation_event(
        owner,
        &super::state_machine::Event {
            id: 0x214,
            payload: None,
        },
    )?;
    ensure!(
        world.named_fx.created == before + 1,
        "Null animation payload created an effect"
    );
    world.dispatch_animation_emitter(owner, 0x214, "[FX]SpatterSurprisedFX")?;
    ensure!(
        world.named_fx.created == before + 1,
        "Emitter lookup stripped an authored prefix or searched unrelated templates"
    );
    let mut saw_particles = false;
    for _ in 0..180 {
        fixture_frame(&mut world)?;
        if let Some(live) = world.named_fx.live.get(&entity) {
            saw_particles |= live.particles.particle_count() > 0;
        } else {
            break;
        }
    }
    ensure!(
        saw_particles,
        "Authored animation dust emitted no particles"
    );
    ensure!(
        world.removed_entities.contains(&entity) && !world.named_fx.live.contains_key(&entity),
        "Animation FX did not retire through its original lifetime/particle drain"
    );
    // The real Lua persistent path now constructs, reuses and restarts one
    // attached instance. Its initial lifetime is not reset by a model restart.
    world.set_entity_hidden(owner, true)?;
    let start=format!("assert(select('#',StartEmitterByName(GetEntity({owner_name}),'BeetleworxFodderKnockbackDust'))==0)");
    world.run_callback_fixture(&start)?;
    let child = world.entities.len() - 1;
    ensure!(
        !world.named_fx.live[&child].fire_and_forget
            && world.transform_hierarchy.contains_key(&child),
        "Persistent FX was detached or replaced by a burst"
    );
    ensure!(
        world.hidden_entities.contains(&child) && !world.previous_hidden_entities.contains(&child),
        "FX child did not inherit hidden owner with its own visible snapshot"
    );
    world.set_entity_hidden(owner, false)?;
    ensure!(
        !world.hidden_entities.contains(&child),
        "Parent Unhide did not restore retained visible FX child"
    );
    let random_before = world.game_random;
    for _ in 0..12 {
        fixture_frame(&mut world)?;
    }
    let count = world.named_fx.live[&child].particles.particle_count();
    ensure!(
        count > 0 && world.game_random.draws > random_before.draws,
        "Persistent particles did not use shared clock/RNG"
    );
    let entities = world.entities.len();
    let created = world.named_fx.created;
    let reads = world.named_fx.resources.reads;
    let random = world.game_random;
    world.run_callback_fixture(&start)?;
    ensure!(
        world.entities.len() == entities
            && world.named_fx.created == created
            && world.named_fx.resources.reads == reads,
        "Persistent restart duplicated an entity or reloaded its resources"
    );
    ensure!(
        world.named_fx.live[&child].particles.particle_count() == count
            && world.game_random == random,
        "Restart cleared particles or consumed birth RNG"
    );
    let now = world.frame_clock.clock.entity_seconds(true);
    ensure!(
        world.named_fx.live[&child].started_at == Some(now)
            && world.named_fx.live[&child]
                .particles
                .emitter_command_state()
                .iter()
                .all(|(_, active, time, driven)| *active && *time == Some(now) && *driven),
        "Restart did not start actual controller owners at native entity time"
    );
    world.dispatch_exported_animation_event(
        owner,
        &super::state_machine::Event {
            id: 0x215,
            payload: Some(payload),
        },
    )?;
    ensure!(
        world.named_fx.live[&child].drain == Some(1)
            && world.named_fx.live[&child].particles.particle_count() == count
            && world.named_fx.live[&child]
                .particles
                .emitter_command_state()
                .iter()
                .all(|(_, active, _, _)| !*active),
        "Stop did not stop controllers while retaining live particles"
    );
    world.run_callback_fixture(&start)?;
    ensure!(
        world.entities.len() == entities
            && world.named_fx.live[&child].drain.is_none()
            && world.named_fx.live[&child].particles.particle_count() == count,
        "Restart did not cancel draining on the same instance"
    );
    // Repeated updates at a frozen shared clock must not emit or draw RNG.
    let random = world.game_random;
    world.advance_named_effects(super::STEP)?;
    ensure!(
        world.game_random == random
            && world.named_fx.live[&child].particles.particle_count() == count,
        "Frozen particle clock advanced births"
    );
    world.run_callback_fixture(&format!("assert(select('#',StopEmitterByName(GetEntity({owner_name}),'BeetleworxFodderKnockbackDust'))==0)"))?;
    ensure!(
        world.named_fx.live[&child].drain == Some(1) && !world.named_fx.live[&child].removal_locked,
        "Lua emitter Stop incorrectly locked entity removal"
    );
    world.run_callback_fixture(&start)?;
    ensure!(
        world.named_fx.live[&child].drain.is_none(),
        "Lua Stop could not be restarted"
    );
    world.set_entity_hidden(owner, true)?;
    //31bc24's zero callback argument must not clear a hidden child's snapshot.
    world.run_callback_fixture(&start)?;
    ensure!(
        world.previous_hidden_entities.contains(&child),
        "Retained hidden FX child lost its prior hidden snapshot"
    );
    world.set_entity_hidden(owner, false)?;
    ensure!(
        world.hidden_entities.contains(&child),
        "Parent Unhide ignored native child snapshot veto"
    );
    world.retire_entity(child)?;
    let drain = world.named_fx.live[&child].drain;
    world.run_callback_fixture(&start)?;
    ensure!(
        world.named_fx.live[&child].removal_locked && world.named_fx.live[&child].drain == drain,
        "Restart cancelled a lifetime/entity removal lock"
    );
    world.named_fx.live.get_mut(&child).unwrap().ready = true;
    world.retire_entity(child)?;
    ensure!(
        !world.previous_hidden_entities.contains(&child),
        "Retired FX leaked its hidden snapshot"
    );
    // This is the actual authored persistent splash animation payload, not a
    // guessed generic effect. Validate that it survives repeated starts too.
    let graph = world.authored_havok.graph(owner)?;
    let splash = graph
        .objects
        .iter()
        .find(|(_, o)| {
            o.klass == "hkbStringEventPayload" && o.fields["data"].as_str() == Some("puddleSplash")
        })
        .map(|(id, _)| super::state_machine::Reference { id: id.clone() })
        .context("Original splash animation payload absent")?;
    let event = super::state_machine::Event {
        id: 0x214,
        payload: Some(splash),
    };
    world.dispatch_exported_animation_event(owner, &event)?;
    let splash = world
        .child_entity_by_name(owner, "puddleSplash")?
        .context("Authored splash did not create or retain its child")?;
    let created = world.named_fx.created;
    let mut splash_particles = false;
    ensure!(
        !world.named_fx.live[&splash].fire_and_forget
            && world.transform_hierarchy.contains_key(&splash),
        "Authored splash lost its persistent attachment"
    );
    world.dispatch_exported_animation_event(owner, &event)?;
    ensure!(
        world.named_fx.created == created,
        "Persistent splash animation duplicated its child"
    );
    for _ in 0..12 {
        fixture_frame(&mut world)?;
        if let Some(effect) = world.named_fx.live.get(&splash) {
            splash_particles |= effect.particles.particle_count() > 0;
        }
    }
    ensure!(
        splash_particles,
        "Original persistent splash emitted no particles"
    );
    let mut pushed = event.clone();
    pushed.id = 0x217;
    let before = world.named_fx.created;
    for _ in 0..12 {
        world.dispatch_exported_animation_event(owner, &pushed)?;
    }
    let stack = world
        .controller_feedback
        .pushed_emitters
        .get(&owner)
        .context("Emitter_Push lost its retained stack")?;
    ensure!(
        stack.len() == 10 && stack.iter().all(|name| name == "puddleSplash"),
        "Native push limit/order or duplicate retention differs"
    );
    ensure!(
        world.named_fx.created == before,
        "Emitter_Push replaced an existing persistent child"
    );
    world.dispatch_exported_animation_event(
        owner,
        &super::state_machine::Event {
            id: 0x218,
            payload: None,
        },
    )?;
    let original_ignore =
        super::entity_status::read_ignore_audio(&world.entities[owner].components)?
            .context("Spatter animation component absent")?;
    let sound = world
        .authored_havok
        .graph(owner)?
        .objects
        .iter()
        .find(|(_, o)| {
            o.klass == "hkbStringEventPayload"
                && o.fields["data"].as_str() == Some("Play_sfx_Spatter_Squeak")
        })
        .map(|(id, _)| super::state_machine::Reference { id: id.clone() })
        .context("Original Spatter audio payload absent")?;
    let audio = super::state_machine::Event {
        id: 0x212,
        payload: Some(sound),
    };
    world.run_callback_fixture(&format!("local e=GetEntity({owner_name}); SetPropertyBool(e,'IgnoreAudioEvents',true); assert(GetPropertyBool(e,'IgnoreAudioEvents')==true)"))?;
    let posts = world.audio_playback.game_post_requests;
    world.dispatch_exported_animation_event(owner, &audio)?;
    ensure!(
        world.audio_playback.game_post_requests == posts,
        "IgnoreAudioEvents still posted animation audio"
    );
    world.run_callback_fixture(&format!("local e=GetEntity({owner_name}); SetPropertyBool(e,'IgnoreAudioEvents',false); assert(GetPropertyBool(e,'IgnoreAudioEvents')==false)"))?;
    world.dispatch_exported_animation_event(owner, &audio)?;
    ensure!(
        world.audio_playback.game_post_requests == posts + 1,
        "Audio_Start did not reach the owned playback backend"
    );
    world.dispatch_exported_animation_event(
        owner,
        &super::state_machine::Event {
            id: 0x212,
            payload: None,
        },
    )?;
    ensure!(
        world.audio_playback.game_post_requests == posts + 1,
        "Null audio payload posted an invented event"
    );
    super::entity_status::write_ignore_audio(
        &mut world.entities[owner].components,
        original_ignore,
    )?;
    ensure!(
        !world
            .controller_feedback
            .pushed_emitters
            .contains_key(&owner)
            && world.named_fx.live[&splash].drain == Some(1),
        "Pop_Effects did not stop and release the retained names"
    );
    // Repeated Pop has no retained effects, but still sends native kind9.
    world.dispatch_exported_animation_event(
        owner,
        &super::state_machine::Event {
            id: 0x218,
            payload: None,
        },
    )?;
    std::fs::write(
        "target/animation-emitter-gameplay-validation.json",
        serde_json::to_vec_pretty(&serde_json::json!({
        "owner":world.entities[owner].name,"effect":world.entities[entity].name,"native_event":0x214,
        "original_payload":true,"source_fire_and_forget":true,"particles_observed":saw_particles,
        "lifetime_and_drain_retired":true,"null_payload_no_creation":true,"stop_does_not_find_detached_child":true,
        "missing_owner_template_declines_without_prefix_rewrite":true,
        "persistent_lua_constructs_and_reuses_one_attached_child":true,
        "restart_preserves_particles_rng_and_resource_reads":true,
        "stop_preserves_particles_and_restart_cancels_drain":true,
        "lua_stop_restarts_but_entity_removal_stays_locked":true,
        "shared_clock_rng_and_frozen_clock_validated":true,
        "authored_persistent_splash_emits":splash_particles,
        "emitter_push_preserves_duplicates_native_limit_and_child_identity":true,
        "pop_stops_and_releases_retained_names":true,
        "authored_audio_start_reaches_backend_and_obeys_live_lua_ignore_property":true,
        "audio_scope":"Bare string payload dispatch only; backend rejections stay explicit. No Wwise mixer/spatial parity claim.",
        "retained_fx_inherits_owner_hide_and_restores_visible_snapshot":true,
        "retained_hidden_fx_vetoes_parent_unhide":true,
        "retired_fx_releases_hidden_snapshot":true,
        "scope":"Controlled export of the original graph payload through real World FX creation and lifecycle; not native event timing or rendering parity"}))?,
    )?;
    println!("PASS: original Spatter FX lifecycle, push/pop stacks, shared clock/RNG, splash and owned Audio_Start/IgnoreAudioEvents dispatch");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_named_effect_creates_world_entity_runs_material_io_and_retires_after_particles(
    ) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::super::assets::load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let owner = world
            .entities
            .iter()
            .position(|e| e.name == "gremlinvillage_zonea.Gremlin_Variant4 01")
            .context("Original gremlin absent")?;
        let name = serde_json::to_string(&world.entities[owner].name)?;
        let before = world.entities.len();
        world.run_callback_fixture(&format!("assert(select('#',StartEmitterByNameFireForget(GetEntity({name}),'GremlinVariant4TeleportInFX'))==0)"))?;
        assert_eq!(world.entities.len(), before + 1);
        let entity = before;
        assert!(world.scripts.has_entity(entity));
        assert!(world.runtime_lifetimes.contains_key(&entity));
        assert!(
            !world.transform_hierarchy.contains_key(&entity),
            "Fire-and-forget must detach"
        );
        let colors = world.named_fx.live[&entity].particles.color_summary();
        assert_eq!(
            colors.as_array().unwrap().len(),
            2,
            "OnSpawned callback did not reach actual effect material state"
        );
        for node in colors.as_array().unwrap() {
            assert_eq!(
                node["constant_colors"][1],
                serde_json::json!([85, 119, 78, 0])
            );
        }
        let detached = world.entities[entity].transform;
        world.run_callback_fixture(&format!(
            "MoveToEntity(GetEntity({name}),GetPlayer(),false,false)"
        ))?;
        assert_eq!(world.entities[entity].transform, detached);
        // Unknown per-owner names do not consult arbitrary global templates.
        world.run_callback_fixture(&format!(
            "StartEmitterByNameFireForget(GetEntity({name}),'BwxMissingEffect')"
        ))?;
        assert_eq!(world.entities.len(), before + 1);
        let mut saw_particles = false;
        let mut saw_drain = false;
        let mut mesh_poses = HashSet::new();
        for frame in 0..150 {
            fixture_frame(&mut world)?;
            if let Some(effect) = world.named_fx.live.get(&entity) {
                saw_particles |= effect.particles.particle_count() > 0;
                saw_drain |= effect.drain.is_some();
                let mut draws = Vec::new();
                effect
                    .particles
                    .append([1., 0., 0.], [0., 1., 0.], &mut draws)?;
                for draw in draws {
                    assert_eq!(
                        draw.material_registers.unwrap()[4],
                        [85., 119., 78., 0.].map(|v| v / 255.)
                    );
                }
                let mut all = Vec::new();
                effect.append(
                    &world.entities[entity],
                    [5., 2., 5.],
                    [1., 0., 0.],
                    [0., 1., 0.],
                    &world.report.spherical_lights,
                    &mut all,
                )?;
                let flash = all.last().context("No FX mesh draw")?;
                assert!(!flash.v_data.is_empty());
                assert!(
                    super::super::render_transform::is_affine(&flash.trans_mat),
                    "Billboard mesh transform must retain homogeneous W"
                );
                mesh_poses.insert(flash.trans_mat.map(f32::to_bits));
            } else {
                break;
            }
            if frame == 15 && saw_particles {
                world.retire_entity(entity)?;
                assert!(
                    world.named_fx.live.contains_key(&entity),
                    "Live particles must survive the removal request"
                );
                assert_eq!(world.named_fx.live[&entity].drain, Some(1));
                saw_drain = true;
            }
        }
        assert!(saw_particles);
        assert!(saw_drain);
        assert!(world.removed_entities.contains(&entity));
        assert!(
            mesh_poses.len() > 1,
            "Authored flash transform controller must affect rendering"
        );
        assert!(!world.scripts.has_entity(entity));
        assert!(!world.runtime_lifetimes.contains_key(&entity));
        assert!(!world.lod_states.contains_key(&entity));
        assert!(!world.named_fx.live.contains_key(&entity));
        assert!(world.named_fx.completed > 0);
        // False overrides the attachment result with identity/world position.
        let before = world.entities.len();
        let reads = world.named_fx.resources.reads;
        world.run_callback_fixture(&format!(
            "StartEmitterByNameFireForget(GetEntity({name}),'GremlinVariant4TeleportInFX',false)"
        ))?;
        assert_eq!(world.entities.len(), before + 1);
        assert_eq!(
            world.entities[before].runtime_rotation,
            [1., 0., 0., 0., 1., 0., 0., 0., 1.]
        );
        assert_eq!(world.entities[before].authored_scale, 1.);
        assert!(super::super::render_transform::is_affine(
            &world.entities[before].transform
        ));
        assert_eq!(
            world.entities[before].transform[12..15],
            world.entities[owner].transform[12..15]
        );
        // Material/model payloads are cached, not inflated for each instance.
        let template =
            world.named_fx.names[&named_effects::lookup_key("GremlinVariant4TeleportInFX")?];
        assert!(world.named_fx.prepared[&template].is_ok());
        assert_eq!(
            world.named_fx.resources.reads, reads,
            "Another instance must reuse decoded resources"
        );
        assert_eq!(
            world.named_fx.live[&before].meshes.len(),
            1,
            "Original teleport flash is a real mesh"
        );
        assert_eq!(
            world.named_fx.prepared[&template]
                .as_ref()
                .unwrap()
                .prototype
                .color_summary(),
            serde_json::json!([])
        );
        // Late creation plus rounded absolute tick times reproduces the live
        // capture that previously left a 1.5-second lifetime at exactly zero.
        let start_tick = (world.animation_time / super::super::STEP).ceil() as u32;
        for tick in start_tick + 1..=start_tick + 150 {
            world.frame_clock.publish(true, true);
            world
                .advance_models_with_delta(tick as f32 * super::super::STEP, super::super::STEP)?;
        }
        assert!(
            world.removed_entities.contains(&before),
            "Lifetime expiry must also retire the effect"
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_outward_named_effect_emits_from_hidden_mesh_surfaces() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::super::assets::load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let owner = world
            .entities
            .iter()
            .position(|e| e.name == "gremlinvillage_zonea.Gremlin_Variant4 01")
            .context("Original gremlin absent")?;
        let name = serde_json::to_string(&world.entities[owner].name)?;
        world.run_callback_fixture(&format!(
            "StartEmitterByNameFireForget(GetEntity({name}),'GremlinVariant4TeleportOutFX')"
        ))?;
        let entity = world.entities.len() - 1;
        assert_eq!(
            world.named_fx.live[&entity].meshes.len(),
            0,
            "Hidden surface emitters must not render as extra gremlins"
        );
        let sources = world.named_fx.live[&entity]
            .particles
            .mesh_emission_summary();
        assert_eq!(sources.len(), 5);
        let nodes = sources
            .iter()
            .flat_map(|s| s["sources"].as_array().unwrap())
            .map(|s| s["node"].as_u64().unwrap())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(nodes, std::collections::BTreeSet::from([8, 18, 25, 32, 39]));
        let mut emitting = std::collections::BTreeSet::new();
        for _ in 0..120 {
            fixture_frame(&mut world)?;
            if let Some(live) = world.named_fx.live.get(&entity) {
                for layer in live.particles.mesh_emission_summary() {
                    if layer["particles"].as_u64().unwrap() > 0 {
                        emitting.insert(layer["system"].as_u64().unwrap());
                    }
                }
                let mut draws = Vec::new();
                live.particles
                    .append([1., 0., 0.], [0., 1., 0.], &mut draws)?;
                assert!(draws
                    .iter()
                    .flat_map(|d| d.v_data.iter())
                    .all(|v| v.is_finite()));
            }
        }
        assert_eq!(
            emitting.len(),
            5,
            "Every original body-part emitter must produce particles"
        );
        assert!(world.removed_entities.contains(&entity));
        assert!(!world.named_fx.live.contains_key(&entity));
        assert!(!world.scripts.has_entity(entity));
        assert!(!world.runtime_lifetimes.contains_key(&entity));
        Ok(())
    }
}
