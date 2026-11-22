//! Rendering and asset decoding shared by the runtime.
pub(crate) mod baked_preview;
pub(crate) mod clip_visibility;
pub(crate) mod clustered_lights;
pub(crate) mod contact_occlusion;
pub(crate) mod game_config;
pub(crate) mod gpu_timing;
pub(crate) mod material_preview;
pub(crate) mod material_vertex_color;
pub(crate) mod model_shadow;
pub(crate) mod native_material;
pub(crate) mod nif;
pub(crate) mod scene;
pub(crate) mod scene_finish;
pub(crate) mod em2_post;
#[cfg(any(test, debug_assertions))]
pub(crate) mod shader;
pub(crate) mod spatial_lights;
pub(crate) mod surface_lighting;
pub(crate) mod surface_material;
pub mod viewport3d;
pub(crate) mod volume_shadow;
pub(crate) mod wii_default;


pub(crate) fn shader_source(source: &str) -> std::borrow::Cow<'_, str> {
    if cfg!(target_os = "android") && source.trim_start().starts_with("#version 330 core") {
        return source.replacen("#version 330 core", "#version 300 es\nprecision highp float;\nprecision highp int;", 1).into();
    }
    source.into()
}
