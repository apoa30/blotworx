//! Per-model direct transform controllers and their shared resolved pose.
use super::{
    nif_transform::{Playback, Pose, Update},
    scene_bounds::Hierarchy,
};
use anyhow::{ensure, Context, Result};
use epic_mickey_lib_rs::nif_file::NiTransform;
use std::collections::{BTreeMap, BTreeSet};

/// HUD registration20ec6c invokes15b620: set APP_INIT and clear Active
/// before any clone is made. This is a runtime template change, not a rewrite
/// of the source NIF's flags or a private-clock workaround.
pub fn effect_template_flags(flags: u16) -> u16 {
    (flags | 1) & !8
}

struct MaterialController {
    control: super::scene_bounds::Controller,
    clock: super::brush_particles::ControllerTime,
}

pub struct Instance {
    material_controllers: Vec<MaterialController>,
    colors: BTreeMap<(u32, usize), [f32; 4]>,
    texture_transforms: BTreeMap<(u32, bool, u32, u32), f32>,
    indirect_scales: BTreeMap<u32, [[i32; 3]; 6]>,
    pub component_clock: Option<super::nif_sequence::ComponentClock>,
    controllers: Vec<Playback>,
    controller_owners: BTreeMap<u32, usize>,
    locals: BTreeMap<u32, NiTransform>,
    pub(super) published_locals: BTreeMap<u32, NiTransform>,
    pub poses: BTreeMap<u32, Pose>,
    pub world: BTreeMap<u32, NiTransform>,
    pub affected: BTreeSet<u32>,
}
impl Instance {
    pub fn load(hierarchy: &Hierarchy) -> Result<Option<Self>> {
        let mut controllers = Vec::new();
        let mut material_controllers = Vec::new();
        let mut seen = BTreeSet::new();
        let mut seen_material_targets = BTreeSet::new();
        let mut pending: Vec<_> = hierarchy.roots.iter().rev().flatten().copied().collect();
        while let Some(id) = pending.pop() {
            let node = &hierarchy.nodes[&id];
            for c in &node.controllers {
                if c.class_name == "NiTransformController" && seen.insert(c.block) {
                    ensure!(
                        c.target
                            .is_some_and(|target| hierarchy.nodes.contains_key(&target)),
                        "Transform target outside model"
                    );
                    controllers.push(Playback::load(c)?);
                } else if (c.texture_transform.is_some() || c.color.is_some() || c.indirect_scale.is_some())
                    && c.target.is_some_and(|mesh| seen_material_targets.insert((c.block, mesh)))
                {
                    material_controllers.push(MaterialController { control: c.clone(), clock: Default::default() });
                }
            }
            pending.extend(node.children.iter().rev().flatten().copied());
        }
        if controllers.is_empty() && material_controllers.is_empty() {
            return Ok(None);
        }
        let controller_owners = controllers
            .iter()
            .enumerate()
            .map(|(index, controller)| (controller.block, index))
            .collect();
        let locals: BTreeMap<_, _> = hierarchy
            .nodes
            .iter()
            .map(|(&id, node)| (id, node.local.clone()))
            .collect();
        Ok(Some(Self {
            component_clock: None,
            material_controllers,
            colors: BTreeMap::new(),
            indirect_scales: BTreeMap::new(),
            texture_transforms: BTreeMap::new(),
            controllers,
            controller_owners,
            published_locals: locals.clone(),
            locals,
            poses: BTreeMap::new(),
            world: hierarchy.base_world_transforms(None)?,
            affected: BTreeSet::new(),
        }))
    }
    pub fn owns_controller(&self, block: u32) -> bool {
        self.controller_owners.contains_key(&block)
            || self.material_controllers.iter().any(|c| c.control.block == block)
    }
    pub fn prepare_effect_template(&mut self, ids: &BTreeSet<u32>) {
        for controller in &mut self.controllers {
            if ids.contains(&controller.block) {
                controller.flags = effect_template_flags(controller.flags);
            }
        }
        for state in &mut self.material_controllers {
            if ids.contains(&state.control.block) {
                state.control.flags = effect_template_flags(state.control.flags);
            }
        }
    }
    pub fn start_controller(&mut self, block: u32, time: f32) -> Result<()> {
        if let Some(&index) = self.controller_owners.get(&block) {
            self.controllers[index].start_at(time);
        } else {
            self.set_material_controller_active(block, true)?;
            // An effect can be drawn on the frame it spawns or restarts.
            // Replace its previous material sample before that draw.
            self.sample_materials(time, Some(block))?;
        }
        Ok(())
    }
    pub fn stop_controller(&mut self, block: u32) -> Result<()> {
        if let Some(&index) = self.controller_owners.get(&block) {
            self.controllers[index].stop();
        } else {
            self.set_material_controller_active(block, false)?;
        }
        Ok(())
    }
    fn set_material_controller_active(&mut self, block: u32, active: bool) -> Result<()> {
        ensure!(self.owns_controller(block), "Model controller {block} has no owner");
        for state in self.material_controllers.iter_mut().filter(|state| state.control.block == block) {
            state.control.flags = (state.control.flags & !8) | (u16::from(active) << 3);
            state.clock.reset();
        }
        Ok(())
    }
    pub fn update(&mut self, hierarchy: &Hierarchy, time: f32) -> Result<()> {
        self.sample_materials(time, None)?;
        self.update_with_writes(hierarchy, time, |_, _| Ok(()))
    }

    fn sample_materials(&mut self, time: f32, only: Option<u32>) -> Result<()> {
        for state in &mut self.material_controllers {
            let c = &state.control;
            if only.is_some_and(|block| c.block != block) || c.flags & 8 == 0 || c.flags & 0x20 != 0 {
                continue;
            }
            let time = state.clock.scale_parameters(time, c.flags, c.frequency, c.phase, c.start, c.stop)?;
            let target = c.target.context("Direct material target missing")?;
            if let Some(channel) = &c.color {
                if let Some(color) = channel.sample(time)? {
                    self.colors.insert((target, channel.slot), super::nif_properties::packed_color(color)?);
                }
            }
            if let Some(channel) = &c.texture_transform {
                if let Some((shader_map, slot, operation, value)) = channel.sample(time)? {
                    self.texture_transforms.insert((target, shader_map, slot, operation), value);
                }
            }
            if let Some(channel) = &c.indirect_scale {
                if let Some(scale) = channel.sample(time)? {
                    // SFX_projectionWarp uses a single unrotated indirect matrix.
                    // Retain its authored scalar, including zero and negative keys.
                    let rows = crate::render_support::material_preview::indirect::srt_matrix([scale; 2], 0., [0.; 2])?;
                    let mut matrices = [[0; 3]; 6];
                    matrices[..2].copy_from_slice(&rows);
                    self.indirect_scales.insert(target, matrices);
                }
            }
        }
        Ok(())
    }
    pub(super) fn apply_material(&self, mesh: u32, draw: &mut crate::render_support::viewport3d::GpuDrawCommand) -> Result<()> {
        if let Some(material) = &draw.material {
            let mut transforms = BTreeMap::new();
            let samples = self.texture_transforms.range((mesh, false, 0, 0)..=(mesh, true, u32::MAX, u32::MAX));
            for (&(_, shader_map, slot, operation), &sample) in samples {
                let map = (shader_map, slot);
                let Some(Ok(original)) = material.texture_transforms.get(&map) else {
                    continue;
                };
                let transform = transforms.entry(map).or_insert_with(|| {
                    original.unwrap_or(crate::nif_textures::UvTransform {
                        method: 2, // NiTextureTransformController creates a Maya transform.
                        center: [0.5; 2],
                        ..Default::default()
                    })
                });
                super::nif_properties::DirectTextureTransform::apply(operation, sample, transform);
            }
            for ((shader_map, slot), transform) in transforms {
                // The generated material resolves each slot from the last binding.
                // Standard maps can supply that slot when no shader map does.
                let supplies_shader_slot = material.textures.iter().rev()
                    .find(|binding| binding.slot == slot)
                    .is_some_and(|binding| binding.shader_id.is_some() == shader_map);
                if supplies_shader_slot && draw.material_images.iter().any(|image| u32::from(image.slot) == slot) {
                    let matrix = transform.matrix().context("Invalid animated texture transform")?;
                    for image in std::sync::Arc::make_mut(&mut draw.material_images) {
                        if u32::from(image.slot) == slot {
                            image.matrix = matrix;
                        }
                    }
                }
                if slot == 0 && (!shader_map || !material.texture_transforms.contains_key(&(false, 0))) {
                    draw.uv_transform = Some(transform);
                }
            }
        }
        if draw.material.as_ref().and_then(|material| material.shader.as_deref())
            .is_some_and(|name| name.eq_ignore_ascii_case("SFX_projectionWarp"))
        {
            if let Some(matrices) = self.indirect_scales.get(&mesh) {
                draw.material_indirect = Some(*matrices);
            }
        }
        for (&(_, slot), &color) in self.colors.range((mesh, 0)..=(mesh, 6)) {
            if let Some(Ok(program)) = &draw.preview {
                if let Some(defaults) = program.register_defaults {
                    draw.material_registers.get_or_insert(defaults)[slot] = color;
                }
            }
        }
        Ok(())
    }

    /// EM2 game-shader path: sample only the material (texture/colour) controllers.
    pub(super) fn sample_material_controllers(&mut self, time: f32) -> Result<()> {
        self.sample_materials(time, None)
    }
    pub(super) fn animates_textures_of(&self, mesh: u32) -> bool {
        self.material_controllers.iter().any(|c| c.control.texture_transform.is_some() && c.control.target == Some(mesh))
    }
    /// The mesh's animated texture transforms by (shader map, slot), as apply_material builds them.
    pub(super) fn animated_texture_transforms(&self, mesh: u32, material: &crate::render_support::nif::MaterialBindings) -> BTreeMap<(bool, u32), crate::nif_textures::UvTransform> {
        let mut transforms = BTreeMap::new();
        for (&(_, shader_map, slot, operation), &sample) in self.texture_transforms.range((mesh, false, 0, 0)..=(mesh, true, u32::MAX, u32::MAX)) {
            let map = (shader_map, slot);
            let Some(Ok(original)) = material.texture_transforms.get(&map) else { continue };
            let transform = transforms.entry(map).or_insert_with(|| original.unwrap_or(crate::nif_textures::UvTransform { method: 2, center: [0.5; 2], ..Default::default() }));
            super::nif_properties::DirectTextureTransform::apply(operation, sample, transform);
        }
        transforms
    }
    /// Publish only writes performed by this update. Reapplying the cached
    /// pose after an Unchanged outcome would overwrite later local edits.
    pub fn update_with_writes(
        &mut self,
        hierarchy: &Hierarchy,
        time: f32,
        mut write: impl FnMut(u32, &Pose) -> Result<()>,
    ) -> Result<()> {
        // Reuse mutable state instead of rebuilding a map or replaying cached
        // poses on every frame. Restore ownership even if an update fails.
        let mut locals = std::mem::take(&mut self.locals);
        let result = self.update_with_local_state(hierarchy, time, &mut locals, |target, pose| {
            write(target, pose)
        });
        self.locals = locals;
        result
    }
    /// Direct transform phase of the native downward walk. Publish each
    /// node immediately after its own controllers. A later child's write to
    /// an already visited node changes its local state, not its published
    /// world frame until that node is visited again. This is not a substitute
    /// for property controllers, mesh modifiers or particle pre/post phases.
    pub fn update_with_local_state(
        &mut self,
        hierarchy: &Hierarchy,
        time: f32,
        locals: &mut BTreeMap<u32, NiTransform>,
        mut write: impl FnMut(u32, &Pose) -> Result<()>,
    ) -> Result<()> {
        self.affected.clear();
        let mut pending: Vec<_> = hierarchy
            .roots
            .iter()
            .rev()
            .flatten()
            .map(|id| (*id, None, false))
            .collect();
        while let Some((id, parent, parent_animated)) = pending.pop() {
            let node = &hierarchy.nodes[&id];
            for attached in &node.controllers {
                let Some(&owner) = self.controller_owners.get(&attached.block) else {
                    continue;
                };
                match self.controllers[owner].update(time)? {
                    Update::Pose { target, pose } => {
                        write(target, &pose)?;
                        let local = locals
                            .get_mut(&target)
                            .context("Transform controller targets missing local state")?;
                        *local = pose.apply_to(local)?;
                        let state = self.poses.entry(target).or_insert(Pose {
                            translation: None,
                            rotation: None,
                            scale: None,
                        });
                        if pose.translation.is_some() {
                            state.translation = pose.translation;
                        }
                        if pose.rotation.is_some() {
                            state.rotation = pose.rotation;
                        }
                        if pose.scale.is_some() {
                            state.scale = pose.scale;
                        }
                    }
                    // These are native direct-update outcomes, not substitutes for
                    // sequence-manager playback. Its missing integration is reported.
                    Update::ManagerControlled
                    | Update::Inactive
                    | Update::Unchanged
                    | Update::NoInterpolator
                    | Update::NoValue => {}
                }
            }
            let parent = parent
                .map(|id| {
                    self.world
                        .get(&id)
                        .context("Parent world frame has not been published")
                })
                .transpose()?;
            let local = locals.get(&id).context("Node local state missing")?;
            let world = super::scene_bounds::compose(parent, local)?;
            self.published_locals.insert(id, local.clone());
            self.world.insert(id, world);
            let animated = parent_animated || self.poses.contains_key(&id);
            if animated {
                self.affected.insert(id);
            }
            pending.extend(
                node.children
                    .iter()
                    .rev()
                    .flatten()
                    .map(|child| (*child, Some(id), animated)),
            );
        }
        Ok(())
    }
    pub fn bounds(
        &self,
        hierarchy: &Hierarchy,
        parent: &NiTransform,
        previous: &BTreeMap<u32, super::scene_bounds::Sphere>,
    ) -> Result<BTreeMap<u32, super::scene_bounds::Sphere>> {
        // Only resolve the controller guard when every attached controller is
        // a direct transform controller. Other bound producers stay explicit.
        let world = hierarchy.base_world_transforms_with_overrides(
            Some(parent),
            &BTreeMap::new(),
            &self.published_locals,
        )?;
        hierarchy.bounds_from_world(&world, previous, true, false)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn later_child_write_does_not_republish_its_already_visited_parent() -> Result<()> {
        use epic_mickey_lib_rs::nif_file::{NiMeshData, NiNodeData, NiObject, NifFile, Vector3};
        let reference: serde_json::Value = serde_json::from_str(include_str!(
            "../../tools/blotworx/fixtures/model_update_order.json"
        ))?;
        let trace = reference["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| {
                case["nodes"]
                    == serde_json::json!([{"id":0,"kind":"node","children":[1]},
                {"id":1,"kind":"mesh","controllers":1}])
                    && case["update_controllers"] == true
            })
            .context("Missing original parent-before-child-controller trace")?;
        assert_eq!(
            trace["calls"],
            serde_json::json!([
                ["world", 0],
                ["controller", 1, 0],
                ["modifiers", 1],
                ["world", 1],
                ["bound", 1]
            ])
        );
        let mut root = NiNodeData::default();
        root.base.transform.scale = 1.;
        root.children = vec![Some(1)];
        let mut child = NiMeshData::default();
        child.base.base.transform.scale = 1.;
        child.bound_center = Vector3 {
            x: 2.,
            y: 0.,
            z: 0.,
        };
        child.bound_radius = 1.;
        let mut graph = Hierarchy::load(&NifFile {
            roots: vec![Some(0)],
            blocks: vec![NiObject::NiNode(root), NiObject::NiMesh(child)],
            ..Default::default()
        })?;
        graph
            .nodes
            .get_mut(&1)
            .unwrap()
            .controllers
            .push(super::super::scene_bounds::Controller {
                block: 10,
                class_name: "NiTransformController".into(),
                color: None,
                indirect_scale: None,
                texture_transform: None,
                flags: 0x4d,
                frequency: 1.,
                phase: 0.,
                start: 0.,
                stop: 10.,
                target: Some(0),
                transform: Some(Ok(super::super::nif_transform::Interpolator {
                    translation: [9., 0., 0.],
                    rotation: [-f32::MAX; 4],
                    scale: -f32::MAX,
                    data: None,
                    channels: None,
                })),
            });
        let mut runtime = Instance::load(&graph)?.unwrap();
        let mut locals = graph
            .nodes
            .iter()
            .map(|(&id, node)| (id, node.local.clone()))
            .collect();
        runtime.update_with_local_state(&graph, 0., &mut locals, |_, _| Ok(()))?;
        assert_eq!(locals[&0].translation.x, 9.);
        assert_eq!(
            runtime.world[&0].translation.x, 0.,
            "Child retroactively published its parent"
        );
        assert_eq!(
            runtime.world[&1].translation.x, 0.,
            "Child used unpublished parent local state"
        );
        let bounds = runtime.bounds(&graph, &graph.nodes[&0].local, &BTreeMap::new())?;
        assert_eq!(
            bounds[&1].center[0], 2.,
            "Bounds used a later local write instead of the published frame"
        );
        runtime.update_with_local_state(&graph, 0., &mut locals, |_, _| Ok(()))?;
        assert_eq!(runtime.world[&0].translation.x, 9.);
        assert_eq!(runtime.world[&1].translation.x, 9.);
        assert_eq!(
            runtime.bounds(&graph, &graph.nodes[&0].local, &BTreeMap::new())?[&1].center[0],
            11.
        );
        Ok(())
    }
    #[test]
    fn shared_transform_controller_has_one_playback_owner() -> Result<()> {
        use epic_mickey_lib_rs::nif_file::{NiNodeData, NiObject, NifFile};
        let mut root = NiNodeData::default();
        root.base.transform.scale = 1.;
        root.children = vec![Some(1)];
        let mut child = NiNodeData::default();
        child.base.transform.scale = 1.;
        let mut graph = Hierarchy::load(&NifFile {
            blocks: vec![NiObject::NiNode(root), NiObject::NiNode(child)],
            roots: vec![Some(0)],
            ..Default::default()
        })?;
        let c = super::super::scene_bounds::Controller {
            block: 10,
            class_name: "NiTransformController".into(),
            color: None,
            indirect_scale: None,
            texture_transform: None,
            flags: 0x45,
            frequency: 1.,
            phase: 0.,
            start: 0.,
            stop: 10.,
            target: Some(0),
            transform: Some(Ok(super::super::nif_transform::Interpolator {
                translation: [1., 2., 3.],
                rotation: [-f32::MAX; 4],
                scale: -f32::MAX,
                data: None,
                channels: None,
            })),
        };
        graph.nodes.get_mut(&0).unwrap().controllers.push(c.clone());
        graph.nodes.get_mut(&1).unwrap().controllers.push(c);
        let mut instance = Instance::load(&graph)?.unwrap();
        assert_eq!(instance.controllers.len(), 1);
        instance.start_controller(10, 12.25)?;
        let mut writes = 0;
        instance.update_with_writes(&graph, 12.25, |_, _| {
            writes += 1;
            Ok(())
        })?;
        assert_eq!(writes, 1);
        Ok(())
    }
    #[test]
    fn restarting_material_controller_replaces_its_sample_before_the_next_tick() -> Result<()> {
        use super::super::nif_transform::{Key, Track};
        use epic_mickey_lib_rs::nif_file::{NiNodeData, NiObject, NifFile};
        let mut graph = Hierarchy::load(&NifFile {
            blocks: vec![NiObject::NiNode(NiNodeData::default())],
            roots: vec![Some(0)],
            ..Default::default()
        })?;
        graph.nodes.get_mut(&0).unwrap().controllers.push(
            super::super::scene_bounds::Controller {
                block: 1,
                class_name: "NiColorExtraDataController".into(),
                color: Some(super::super::nif_properties::DirectColor {
                    slot: 1,
                    track: Some(Track {
                        kind: 1,
                        keys: [(0., 0.), (1., 1.)].into_iter().map(|(time, alpha)| Key {
                            time,
                            value: vec![0., 0., 0., alpha],
                            tangents: None,
                            tcb: None,
                        }).collect(),
                    }),
                    value: [0.; 4],
                }),
                indirect_scale: None,
                texture_transform: None,
                flags: 0x45,
                frequency: 1.,
                phase: 0.,
                start: 0.,
                stop: 1.,
                target: Some(0),
                transform: None,
            }
        );
        let mut instance = Instance::load(&graph)?.unwrap();
        instance.start_controller(1, 10.)?;
        assert_eq!(instance.colors[&(0, 1)][3], 0.);
        instance.update(&graph, 11.)?;
        assert_eq!(instance.colors[&(0, 1)][3], 1.);
        instance.stop_controller(1)?;
        instance.start_controller(1, 25.)?;
        assert_eq!(instance.colors[&(0, 1)][3], 0.);
        instance.update(&graph, 25.5)?;
        assert_eq!(instance.colors[&(0, 1)][3], 127. / 255.);
        Ok(())
    }

    #[test]
    fn effect_template_flags_match_native_registration() -> Result<()> {
        let data: serde_json::Value = serde_json::from_str(include_str!(
            "../../tools/blotworx/fixtures/effect_template_flags.json"
        ))?;
        assert_eq!(data["cases"].as_array().unwrap().len(), 64);
        for case in data["cases"].as_array().unwrap() {
            assert_eq!(
                effect_template_flags(case["flags"].as_u64().unwrap() as u16),
                case["output"].as_u64().unwrap() as u16
            );
        }
        Ok(())
    }
    #[test]
    fn unchanged_controller_does_not_reapply_cached_pose_over_later_local_write() -> Result<()> {
        use epic_mickey_lib_rs::nif_file::{NiNodeData, NiObject, NifFile};
        let mut node = NiNodeData::default();
        node.base.transform.scale = 1.;
        let nif = NifFile {
            blocks: vec![NiObject::NiNode(node)],
            roots: vec![Some(0)],
            ..Default::default()
        };
        let mut hierarchy = Hierarchy::load(&nif)?;
        hierarchy.nodes.get_mut(&0).unwrap().controllers.push(
            super::super::scene_bounds::Controller {
                block: 10,
                class_name: "NiTransformController".into(),
                color: None,
                indirect_scale: None,
                texture_transform: None,
                flags: 0x4d,
                frequency: 1.,
                phase: 0.,
                start: 0.,
                stop: 10.,
                target: Some(0),
                transform: Some(Ok(super::super::nif_transform::Interpolator {
                    translation: [1., 2., 3.],
                    rotation: [-f32::MAX; 4],
                    scale: -f32::MAX,
                    data: None,
                    channels: None,
                })),
            },
        );
        let mut runtime = Instance::load(&hierarchy)?.unwrap();
        let mut local = hierarchy.nodes[&0].local.clone();
        runtime.update_with_writes(&hierarchy, 0., |_, pose| {
            local = pose.apply_to(&local)?;
            Ok(())
        })?;
        assert_eq!(local.translation.x, 1.);
        local.translation.x = 9.;
        runtime.update_with_writes(&hierarchy, 0., |_, pose| {
            local = pose.apply_to(&local)?;
            Ok(())
        })?;
        assert_eq!(
            local.translation.x, 9.,
            "Cached pose overwrote a later placement without an actual controller write"
        );
        runtime.update_with_writes(&hierarchy, 1., |_, pose| {
            local = pose.apply_to(&local)?;
            Ok(())
        })?;
        assert_eq!(
            local.translation.x, 1.,
            "A subsequent real controller write must still take effect"
        );
        Ok(())
    }
    #[test]
    fn imported_bind_inverse_handles_rotation_scale_translation() -> Result<()> {
        let bind = [
            [0., -2., 0., 3.],
            [3., 0., 0., 4.],
            [0., 0., 4., 5.],
            [0., 0., 0., 1.],
        ];
        let inverse = inverse(bind)?;
        let flat = std::array::from_fn(|i| bind[i % 4][i / 4]);
        let identity = super::super::skin::matrix_product(&flat, &inverse);
        for (i, value) in identity.into_iter().enumerate() {
            assert!((value - if i % 5 == 0 { 1. } else { 0. }).abs() < 1e-6);
        }
        let mut singular = bind;
        singular[2] = [0.; 4];
        assert!(super::inverse(singular).is_err());
        Ok(())
    }
}
pub fn matrix(t: &NiTransform) -> [f32; 16] {
    std::array::from_fn(|i| {
        let (row, col) = (i % 4, i / 4);
        if row < 3 && col < 3 {
            t.rotation.rows[row][col] * t.scale
        } else if col == 3 && row < 3 {
            [t.translation.x, t.translation.y, t.translation.z][row]
        } else if row == 3 && col == 3 {
            1.
        } else {
            0.
        }
    })
}
/// Invert the actual transform baked into imported vertices, once at load.
pub fn inverse(m: [[f32; 4]; 4]) -> Result<[f32; 16]> {
    ensure!(
        m.iter().flatten().all(|v| v.is_finite()) && m[3] == [0., 0., 0., 1.],
        "Invalid mesh bind transform"
    );
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let rows: [[f64; 3]; 3] = std::array::from_fn(|r| std::array::from_fn(|c| m[r][c] as f64));
    let cof = [
        cross(rows[1], rows[2]),
        cross(rows[2], rows[0]),
        cross(rows[0], rows[1]),
    ];
    let determinant = (0..3).map(|i| rows[0][i] * cof[0][i]).sum::<f64>();
    ensure!(
        determinant != 0. && determinant.is_finite(),
        "Singular mesh bind transform requires original local vertices"
    );
    let mut out = [0.; 16];
    out[15] = 1.;
    for r in 0..3 {
        for c in 0..3 {
            out[c * 4 + r] = (cof[c][r] / determinant) as f32;
        }
        out[12 + r] = -(0..3)
            .map(|c| cof[c][r] / determinant * m[c][3] as f64)
            .sum::<f64>() as f32;
    }
    ensure!(
        out.iter().all(|v| v.is_finite()),
        "Mesh bind inverse overflow"
    );
    Ok(out)
}
