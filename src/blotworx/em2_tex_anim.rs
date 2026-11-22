//! EM2 animated texture transforms (NiTextureTransformController: scrolling clouds, water...)
//! for draws on the game's own shaders. The shaders read TexTransformShader per NTMIndex; the
//! material asset only holds the authored (static) transform, so the animated one is sampled
//! here each simulation tick and looked up per draw.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use crate::nif_textures::UvTransform;
use crate::render_support::nif::MaterialBindings;

struct AnimatedDraw { model: String, mesh: u32, material: Arc<MaterialBindings>, key: usize }
#[derive(Default)]
struct State {
    models: HashMap<String, super::nif_runtime::Instance>,
    draws: Vec<AnimatedDraw>,
    /// (draw identity = vertex-buffer Arc pointer, shader map slot) -> current transform
    table: HashMap<(usize, u32), UvTransform>,
}
static STATE: Mutex<Option<State>> = Mutex::new(None);

pub(super) fn reset() { if let Ok(mut s) = STATE.lock() { *s = Some(State::default()); } }

/// A model's material controllers, from its node hierarchy (once per model).
pub(super) fn register_model(key: &str, graph: &super::scene_bounds::Hierarchy) {
    let Ok(Some(instance)) = super::nif_runtime::Instance::load(graph) else { return };
    if let Ok(mut s) = STATE.lock() { s.get_or_insert_with(Default::default).models.insert(key.to_owned(), instance); }
}

/// A draw of `mesh` (NiMesh block) from `model`; kept only if that mesh has texture animation.
pub(super) fn register_draw(model: &str, mesh: u32, draw: &crate::render_support::viewport3d::GpuDrawCommand) {
    let Some(material) = draw.material.clone() else { return };
    let Ok(mut s) = STATE.lock() else { return };
    let s = s.get_or_insert_with(Default::default);
    if s.models.get(model).is_some_and(|m| m.animates_textures_of(mesh)) {
        s.draws.push(AnimatedDraw { model: model.to_owned(), mesh, material, key: Arc::as_ptr(&draw.v_data) as usize });
    }
}

/// Sample every animated model at `time` (seconds of model time) and refresh the lookup table.
pub(super) fn advance(time: f32) {
    let Ok(mut s) = STATE.lock() else { return };
    let Some(s) = s.as_mut() else { return };
    if s.draws.is_empty() { return; }
    for instance in s.models.values_mut() { let _ = instance.sample_material_controllers(time); }
    let mut table = HashMap::new();
    for d in &s.draws {
        let Some(instance) = s.models.get(&d.model) else { continue };
        for ((shader_map, slot), transform) in instance.animated_texture_transforms(d.mesh, &d.material) {
            if shader_map { table.insert((d.key, slot), transform); }
        }
    }
    s.table = table;
}

/// The current animated transform for a draw's shader map `slot`, if it has one.
pub(super) fn lookup(draw: &crate::render_support::viewport3d::GpuDrawCommand, slot: u32) -> Option<UvTransform> {
    let s = STATE.lock().ok()?;
    s.as_ref()?.table.get(&(Arc::as_ptr(&draw.v_data) as usize, slot)).copied()
}

pub(super) fn count() -> usize { STATE.lock().ok().and_then(|s| s.as_ref().map(|s| s.draws.len())).unwrap_or(0) }
