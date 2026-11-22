use super::physics::{self, Collision, Motion, Triangle, V};
use crate::render_support::{baked_preview::BakedPreview, material_preview, scene, viewport3d::*};
use anyhow::{ensure, Context, Result};
use epic_mickey_lib_rs::{
    file_manipulator::EndianType, hkx_file::HkxFile, packfile::VirtualFile, scene_file::SceneFile,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(feature = "blotworx-memory-profile")]
fn trace_load_memory(stage: &str) {
    if let Some((live, peak)) = crate::memory_profile::snapshot() {
        eprintln!(
            "[load-memory] {stage}: {:.1} MiB live · {:.1} MiB peak",
            live as f64 / 1_048_576.,
            peak as f64 / 1_048_576.
        );
    }
}
#[cfg(all(not(feature = "blotworx-memory-profile"), target_os = "horizon"))]
fn trace_load_memory(stage: &str) {
    super::horizon_3ds::debug_stage(stage);
}
#[cfg(all(not(feature = "blotworx-memory-profile"), not(target_os = "horizon")))]
fn trace_load_memory(_: &str) {}

#[path = "entity_initialize.rs"]
mod entity_initialize;
#[path = "multi_position_audio_world.rs"]
mod multi_position_audio_world;
#[path = "audio_world.rs"]
mod audio_world;
#[path = "scene_import.rs"]
mod scene_import;
#[path = "scene_publish.rs"]
mod scene_publish;
#[path = "template_spawner.rs"]
pub(super) mod template_spawner;
#[path = "template_instance.rs"]
mod template_instance;
#[path = "death_reward.rs"]
mod death_reward;
#[path = "entity_retirement.rs"]
mod entity_retirement;
#[path = "pickup_motion.rs"]
mod pickup_motion;
#[path = "vector_property.rs"]
mod vector_property;

/// Maps the shared world triangle storage back to an authored body/subpart.
/// Names alone are not identity: separate placements may share the same asset.
#[derive(Clone, serde::Serialize)]
pub struct CollisionPart {
    /// Runtime collider identity, independent of this record's report index.
    pub owner: usize,
    /// Shared original body-local geometry. Never serialize duplicate assets
    /// into diagnostics or infer native support features from baked vertices.
    #[serde(skip)]
    pub native_resource: Arc<Vec<super::collision::StaticMesh>>,
    #[serde(skip)]
    pub native_part: usize,
    #[serde(skip)]
    pub native_welding: Arc<BTreeMap<usize, super::shape_welding::Prepared>>,
    pub scene: String,
    pub entity_id: u32,
    pub asset: String,
    pub body_offset: usize,
    pub body_filter_info: u32,
    pub shape_keys: super::collision::ShapeKeys,
    pub child_filters: super::collision::ChildFilters,
    pub motion_type: u8,
    pub linear_velocity: [f32; 3],
    pub angular_velocity: [f32; 3],
    pub subpart_index: usize,
    pub body_transform: [f32; 16],
    pub entity_transform: [f32; 16],
    pub triangle_start: usize,
    pub triangle_count: usize,
}
/// Original imported data, before scene/runtime collidable filter overrides.
/// part identifies a placement; body_offset alone isn't globally unique.
#[derive(Clone, Copy, Debug)]
pub struct ImportedStaticCandidate {
    pub part: usize,
    pub body_offset: usize,
    pub shape_key: u32,
    pub serialized_body_filter: u32,
    pub serialized_child_filter: u32,
    pub serialized_quality: u8,
}
#[derive(Default, serde::Serialize)]
pub struct Report {
    pub physics_world: super::physics_world::Configuration,
    pub counter_rim_colors: super::paint_counter::RimColors,
    pub counter_impact_colors: super::paint_counter::RimColors,
    pub pickup_bob_globals: Option<super::pickup::BobGlobals>,
    pub spherical_lights: Vec<super::spherical_lighting::RegisteredLight>,
    pub jigsaw_overlap_guard_enabled: bool,
    pub jigsaw_time_scale: f32,
    pub brush_assets: Option<super::brush_assets::Assets>,
    pub spin_assets: Option<super::brush_assets::Assets>,
    pub spin_ribbons: BTreeMap<String, Vec<super::ribbon::Generator>>,
    pub ammo_policy: Option<super::ammo::Policy>,
    pub authored_ammo: Option<super::ammo::Inventory>,
    pub ammo_settings: Option<super::ammo::Settings>,
    pub brush_shapes: BTreeMap<String, super::projectile_shape::SphereBody>,
    pub brush_templates: BTreeMap<String, serde_json::Value>,
    pub brush_settings: Option<super::brush::Settings>,
    pub ear_settings: Option<super::ears::Settings>,
    pub locomotion_action_settings: Option<super::locomotion_actions::Settings>,
    pub spin_settings: Option<super::spin::Settings>,
    pub player_behavior: Option<super::behavior::CharacterAssets>,
    pub player_collision: Option<super::character_hull::ConvexHull>,
    #[serde(skip)]
    pub player_native_support: Option<super::native_gsk::ConvexSupport>,
    pub player_components: Vec<serde_json::Value>,
    pub cooked_skins: Vec<serde_json::Value>,
    pub engine: String,
    pub level: String,
    pub packs: Vec<String>,
    pub scenes: Vec<String>,
    pub placed_entities: usize,
    pub meshes: usize,
    pub render_triangles: usize,
    pub collision_triangles: usize,
    pub collision_parts: Vec<CollisionPart>,
    pub textures: usize,
    pub geometry: Vec<serde_json::Value>,
    pub global_overrides: usize,
    pub spawn: V,
    pub motion: Motion,
    pub components: BTreeMap<String, usize>,
    pub warnings: Vec<String>,
    pub unsupported: Vec<String>,
}
pub struct JigsawInstance {
    pub allow_nonoverlapping_painting: bool,
    pub transition_states: HashMap<usize, [super::jigsaw_effects::Draw; 2]>,
    pub render_states: HashMap<usize, [GpuDrawCommand; 2]>,
    pub topology: super::jigsaw_topology::Topology,
    pub entity: usize,
    pub node: usize,
    pub collision: super::jigsaw_collision::Binding,
    pub mesh_draws: HashMap<u32, Vec<usize>>,
}
#[derive(Clone, Copy)]
pub struct JigsawOwner {
    pub instance: usize,
    pub piece: usize,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct SphericalBinding {
    pub entity: usize,
    pub mesh: u32,
    pub draw: usize,
    pub property: u64,
    pub slot: u32,
}
pub struct ActorPlayback {
    pub entity: usize,
    pub sequence: u32,
    pub name: String,
    pub time: f32,
    repeat: bool,
    duration: f32,
    frequency: f32,
    cycle: u32,
    clock: super::nif_sequence::UnsynchronizedClock,
    pub(super) pose: super::nif_sequence::ActorPose,
    pub(super) world: BTreeMap<u32, epic_mickey_lib_rs::nif_file::NiTransform>,
}
/// Losslessly compressed CPU mesh and image payloads for a streamed-out scene.
/// Stable draw slots and entity identities remain in place; reactivation
/// restores their original buffers before making those draws visible.
#[derive(Default)]
pub(super) struct ColdSceneAssets {
    pub(super) vertices: HashMap<usize, CompressedSceneArray>,
    pub(super) indices: HashMap<usize, CompressedSceneArray>,
    pub(super) images: HashMap<usize, CompressedSceneArray>,
    pub(super) draw_vertices: HashMap<usize, usize>,
    pub(super) draw_indices: HashMap<usize, usize>,
    pub(super) draw_images: HashMap<usize, ColdDrawImages>,
    pub(super) spherical_images: BTreeMap<usize, ColdSphericalImages>,
    pub(super) texture_cache_entries: HashMap<String, (usize, u32, u32)>,
}
#[derive(Debug)]
pub(super) struct CompressedSceneArray {
    pub(super) elements: usize,
    pub(super) bytes: Vec<u8>,
}
pub(super) enum ColdImageRef {
    Stored(usize),
    Retained(Arc<Vec<u8>>),
}
pub(super) struct ColdMaterialImage {
    pub(super) slot: u8,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) sampler: crate::render_support::viewport3d::TextureSampler,
    pub(super) matrix: [f32; 9],
    pub(super) rgba: ColdImageRef,
}
pub(super) struct ColdDrawImages {
    pub(super) tex_bytes: Option<(ColdImageRef, u32, u32)>,
    pub(super) material_images: Vec<ColdMaterialImage>,
}
pub(super) enum ColdSphericalImages {
    Ready(
        Arc<crate::render_support::material_preview::Program>,
        Vec<ColdMaterialImage>,
    ),
    Failed(String),
}
impl ColdSceneAssets {
    pub(super) fn compressed_bytes(&self) -> usize {
        self.vertices
            .values()
            .chain(self.indices.values())
            .chain(self.images.values())
            .map(|array| array.bytes.capacity())
            .sum()
    }
    pub(super) fn retire_draw(&mut self, draw: usize) {
        if let Some(key) = self.draw_vertices.remove(&draw) {
            if !self.draw_vertices.values().any(|&candidate| candidate == key) {
                self.vertices.remove(&key);
            }
        }
        if let Some(key) = self.draw_indices.remove(&draw) {
            if !self.draw_indices.values().any(|&candidate| candidate == key) {
                self.indices.remove(&key);
            }
        }
        self.draw_images.remove(&draw);
        self.spherical_images.remove(&draw);
        let mut used_images = HashSet::new();
        for state in self.draw_images.values() {
            if let Some((ColdImageRef::Stored(image), _, _)) = &state.tex_bytes {
                used_images.insert(*image);
            }
            used_images.extend(state.material_images.iter().filter_map(|image| match &image.rgba {
                ColdImageRef::Stored(image) => Some(*image),
                ColdImageRef::Retained(_) => None,
            }));
        }
        for state in self.spherical_images.values() {
            if let ColdSphericalImages::Ready(_, images) = state {
                used_images.extend(images.iter().filter_map(|image| match &image.rgba {
                    ColdImageRef::Stored(image) => Some(*image),
                    ColdImageRef::Retained(_) => None,
                }));
            }
        }
        self.images.retain(|image, _| used_images.contains(image));
        self.texture_cache_entries
            .retain(|_, (image, _, _)| used_images.contains(image));
    }
    pub(super) fn is_empty(&self) -> bool {
        self.vertices.is_empty()
            && self.indices.is_empty()
            && self.images.is_empty()
            && self.draw_images.is_empty()
            && self.spherical_images.is_empty()
    }
}
pub struct World {
    death_rewarded: BTreeSet<usize>,
    cleanup_queue: super::entity_cleanup::Queue,
    cleanup_preparing: BTreeSet<usize>,
    cleanup_completed: u64,
    dynamic_rewards: super::dynamic_reward::State,
    pub(super) level_transition: super::level_transition::Pending,
    pub(super) pending_player_transfer: Option<super::level_session::Player>,
    pub robot_arm_idle_requests: BTreeMap<usize, super::robot_arm::Idle>,
    pub robot_arm_controls: BTreeMap<usize, super::robot_arm::Runtime>,
    pub model_shadow: super::model_shadow_scene::Runtime,
    data_root: PathBuf,
    pub movies: super::movie::Requests,
    asset_context: egui::Context,
    pub(super) scene_requests: super::scene_loading::Requests,
    pub(super) scene_requests_prepared_for_models: bool,
    pub scene_resources: super::scene_resources::State,
    pub level_resource: super::checkpoint_metadata::Level,
    texture_cache: HashMap<String, Option<DecodedImage>>,
    pub(super) startup_texture_cache_pruned_bytes: usize,
    pub(super) startup_entity_payload_compacted_bytes: usize,
    scene_geometry_cache: scene_import::GeometryCache,
    scene_collision_cache: scene_import::StaticCollisionCache,
    scene_sequence_cache: scene_import::SequenceCache,
    scene_actor_binding_cache: scene_import::ActorBindingCache,
    static_collision_owner_parts: HashMap<usize, usize>,
    pub particle_asset_preview: Option<super::particle_preview::Instance>,
    pub named_fx: super::effect_runtime::Library,
    pub scene_particles: super::scene_particles::Runtime,
    pub shadow_volumes: super::shadow_scene::Runtime,
    pub guardian_entities: super::guardians::Ownership<usize>,
    pub guardian_templates: BTreeMap<String, super::brush_assets::Assets>,
    pub guardian_costs: super::guardians::Costs,
    pub guardian_parameters: super::guardian_parameters::Library,
    pub guardians: super::guardians::State,
    pub guardian_audio: super::guardians::Audio,
    pub audio_playback: super::audio_playback::Playback,
    pub(super) music_state: super::audio_music_state::State,
    pub multi_position_audio: BTreeMap<(usize, u32), super::multi_position_audio::Controller>,
    pub screen_fade: super::screen_fade::State,
    pub map_info: super::map_info::State,
    pub camera_profiles: super::camera_catalog::Catalog,
    pub story_sequences: super::story_sequences::State,
    pub demo_story: super::demo_story::State,
    pub camera_pipeline: super::camera_pipeline::Runtime,
    pub side_scroller: super::side_scroller::State,
    pub contact_stimuli: super::contact_stimuli::State,
    pub camera_lens: super::camera_lens::Application,
    pub camera_target: super::camera_target::State,
    pub camera_feedback: super::camera_feedback::State,
    pub gameplay_camera_targets: super::camera_generator_target::Targets,
    pub mickey_tracker: Option<super::entity_tracker::Runtime>,
    pub entity_event_registry:
        super::action_event_dispatch::EventRegistry<super::entity_events::Event>,
    pub render_options: super::render_options::State,
    pub ledge_controls: BTreeMap<usize, super::ledge_control::State>,
    pub(super) wall_pushing_disabled: HashSet<usize>,
    pub nif_camera: Option<super::nif_camera::Selection>,
    pub nif_camera_transition: Option<super::nif_camera::Transition>,
    pub nif_camera_completed_frame: Option<super::nif_camera::Frame>,
    pub spawners: BTreeMap<usize, template_spawner::State>,
    pub(super) spawner_sources: HashSet<usize>,
    pub component_control: super::component_control::State,
    pub npc_relocations: BTreeMap<usize, super::npc_relocation::State>,
    pub globals: super::global_state::State,
    pub quests: super::quest_state::State,
    pub game_options: super::save_options::State,
    pub script_feedback: super::script_feedback::Runtime,
    pub instance_changes: super::save_instances::State,
    pub playstyle_points: super::playstyle_points::State,
    /// Named level scoring scopes opened by original Lua scripts. Tracking
    /// scope lifecycle prevents missing bookkeeping APIs from aborting level
    /// entrance coroutines; score-event accumulation remains separate.
    pub playstyle_tracking: BTreeSet<String>,
    pub playthrough: super::score_state::Playthrough,
    pub playtime: super::checkpoint_time::Playtime,
    pub frame_clock: super::frame_clock::Context,
    pub checkpoint: super::checkpoint::State,
    pub extras: super::extras::State,
    pub collectibles: super::collectibles::State,
    pub ui_notifications: Vec<super::collectibles::Notification>,
    pub player_spawn: super::player_spawn::State,
    pub cutscene_input: super::cutscene_input::State,
    pub(super) script_player: physics::Player,
    pub(super) script_player_available: bool,
    pub havok_actors: BTreeMap<usize, (String, super::sketch_assets::Actor)>,
    pub sketch_templates: super::sketch_assets::Library,
    pub component_events: super::component_events::Runtime,
    pub ai_brains: BTreeMap<usize, super::ai_behavior_events::Brain>,
    pub patrol_predecessors: BTreeMap<usize, Option<usize>>,
    /// Authored Kynapse navigation graphs referenced by loaded scene components.
    /// The demo AI adapter uses these as route guidance; native Kynapse policy
    /// (agent profiles, movement types and cross-sector traversal) is separate.
    pub ai_navigation: super::ai_navigation::Runtime,
    pub paint_counters: BTreeMap<usize, super::paint_counter::Counter>,
    pub counter_materials: BTreeMap<usize, super::paint_counter::MaterialRuntime>,
    pub ai_registry: super::ai_registry::Registry,
    pub entity_event_queue: super::entity_events::Queue,
    pub deferred_event_runtime:super::entity_event_runtime::Runtime,
    pub ai_manager_enabled: Option<bool>,
    pub cutscene_actions: super::cutscene_actions::State,
    pub animation_events: std::result::Result<super::animation_events::Catalog, String>,
    pub animation_variable_changes: super::animation_variables::Trace,
    pub player_script_animation_events: Vec<(
        super::state_machine::Event,
        Vec<super::state_machine::Event>,
    )>,
    pub controller_feedback: super::controller_feedback::Runtime,
    pub authored_havok: super::authored_havok::Runtime,
    pub player_physics: Option<super::player_physics::Body>,
    pub character_physics: BTreeMap<usize, Result<super::character_physics::Body, String>>,
    pub runtime_settings: BTreeMap<String, String>,
    pub(super) countdown: super::countdown::State,
    pub input_presentation: super::input_presentation::State,
    pub(super) logic_timers: BTreeMap<(usize,u32),super::logic_timer::State>,
    pub model_paint: BTreeMap<usize, super::model_paint::Model>,
    pub model_paint_owners: BTreeMap<usize, usize>,
    pub pickup_bob_globals: super::pickup::BobGlobals,
    pub pickup_settings: BTreeMap<usize, super::pickup::Settings>,
    pub pickup_update_states: BTreeMap<usize, super::pickup::UpdateState>,
    pub pickup_lifecycles: BTreeMap<usize, super::pickup_lifecycle::State>,
    pickup_collection_blocked: BTreeSet<usize>,
    pub pickup_physics_leases: BTreeMap<usize, super::pickup_world_init::Component>,
    pub pickup_reference_monitor: std::sync::Mutex<()>,
    pub authored_inventory: BTreeMap<usize, super::inventory_state::Authored>,
    pub authored_health: BTreeMap<usize, super::health::Authored>,
    pub runtime_health: BTreeMap<usize, super::health::Runtime>,
    pub removed_entities: HashSet<usize>,
    pub removed_draws: HashSet<usize>,
    /// Script visibility is instance state, separate from geometry removal and
    /// per-piece paint/thinner visibility. Unhide must not restore thinned parts.
    pub hidden_entities: HashSet<usize>,
    /// Scene deactivation visibility, kept separate so a later stream request
    /// can restore exactly the entities hidden by the scene transition.
    pub(super) stream_hidden_entities: BTreeMap<usize, String>,
    /// Deactivated scene mesh arrays use a lossless compressed cold store.
    /// This bounds resident geometry while preserving exact restore data.
    pub(super) cold_scene_assets: HashMap<String, ColdSceneAssets>,
    /// Entity byte35, copied by1ecf9c independently of current hidden byte34.
    pub previous_hidden_entities: HashSet<usize>,
    pub hidden_entity_draws: HashSet<usize>,
    /// Constructor state only: config/save restoration and action gates pending.
    pub player_abilities: BTreeMap<String, super::ability_ownership::Ownership>,
    pub player_ability_registry: super::ability_registry::Registry,
    /// MickeyPlayerController byte d0, set by retail SetStayInCombatIdle.
    pub player_stay_combat_idle: bool,
    pub ticket_hud_gain: Option<(u64, f32, f32)>,
    pub ability_hud_requests: Vec<(&'static str, &'static str, &'static str)>,
    pub diagnostic_pickup_commands: bool,
    pub pickup_receivers: BTreeMap<usize, super::pickup::Receiver>,
    pub size_phantoms: BTreeMap<usize, super::phantom_bounds::SizeVolume>,
    pub(super) rigid_trigger_hulls: BTreeMap<usize, super::phantom_bounds::RigidTriggerHull>,
    /// Imported resource geometry is retained separately from size phantoms.
    /// It must not be installed as solid/static collision or a shapeless AABB.
    pub resource_phantom_shapes: BTreeMap<usize, Arc<Vec<super::collision::StaticMesh>>>,
    /// Source constructor pose, retained before world admission. No serialized
    /// rigid-body placement or render scale is applied to a shape phantom.
    pub resource_phantom_motion: BTreeMap<usize, super::native_motion_step::MotionState>,
    pub lod_states: BTreeMap<usize, super::lod::State>,
    /// Mutable native shape caches share the resource's ownership, rather than
    /// rebuilding geometry or cached list bounds for every overlap query.
    pub resource_phantom_bounds:
        BTreeMap<usize, Arc<std::sync::Mutex<super::shape_bounds::ShapeGraph>>>,
    pub size_phantom_filters: BTreeMap<usize, u32>,
    pub physics_objects: super::phantom_objects::Objects,
    pub size_phantom_handles: BTreeMap<usize, usize>,
    pub resource_phantom_handles: BTreeMap<usize, usize>,
    pub phantom_systems: BTreeMap<usize, usize>,
    pub(super) phantom_resources: super::phantom_assets::Cache,
    pub actor_playback: Option<ActorPlayback>,
    pub actors: BTreeMap<usize, super::actor_runtime::Actor>,
    /// AnimationComponentGB +0x94 defaults false. Store only true flags;
    /// this remains component state even if its pose binding is unsupported.
    pub idle_actors: HashSet<usize>,
    pub scripts: super::actor_scripts::Scripts,
    pub outputs: super::actor_io::Outputs,
    pub(super) animation_time: f32,
    actor_draw_sources: BTreeMap<usize, (Vec<u32>, [[f32; 4]; 4])>,
    /// Shared immutable KF resources. Instance activation/pose state is separate.
    pub actor_sequences: BTreeMap<usize, Arc<super::nif_sequence::SequenceResources>>,
    /// Shared actor transform/morph bindings; activation and blending are separate.
    pub actor_bindings: BTreeMap<usize, Arc<super::nif_sequence::ActorBindings>>,
    pub dynamic_bodies: Vec<super::dynamic_body::Imported>,
    pub dynamic_body_sources: super::dynamic_body::SourceSummary,
    pub dynamic_constructions: BTreeMap<usize, super::dynamic_body::Construction>,
    /// Diagnostic query world only; never substitute frozen query geometry
    /// for native dynamic gameplay admission/contact response.
    pub dynamic_queries: Option<Collision>,
    pub dynamic_query_owners: Vec<(usize, usize)>,
    /// Supported single-body compatibility simulation in the shared gameplay
    /// collision world. Multi-body model binding and Havok parity remain open.
    pub dynamic_runtime_sources: BTreeMap<usize, usize>,
    pub dynamic_ground_contacts: std::collections::BTreeSet<usize>,
    pub(super) pressure_plates: super::pressure_plate::Plates,
    pub runtime_lifetimes: BTreeMap<usize, super::lifetime::State>,
    pub dynamic_runtime_filter: super::physics_filter::GameFilter,
    pub rigid_layer_updates: super::rigid_layer::State,
    pub dynamic_runtime_time: f32,
    pub game_random: super::game_random::State,
    pub player_knockback: Option<super::player_knockback::Runtime>,
    pub(super) transform_hierarchy: BTreeMap<usize, super::rotator_runtime::Local>,
    pub(super) live_child_slots: BTreeMap<usize,Vec<Option<usize>>>,
    pub(super) live_transform_children: BTreeMap<usize,Vec<usize>>,
    pub(super) dynamic_parent_bindings: BTreeMap<(usize,u32),Option<usize>>,
    pub(super) script_transformations: u64,
    pub(super) script_transform_lod_resets: u64,
    pub(super) parent_pose_forces: u64,
    pub(super) parent_pose_error: f32,
    pub(super) rotators: BTreeMap<(usize, u32), super::rotator_runtime::Instance>,
    pub(super) scene_motion_prepared_at: Option<(f32, f32)>,
    pub(super) model_animation: BTreeMap<usize, super::nif_runtime::Instance>,
    pub(super) render_objects: BTreeMap<(usize, u32), super::render_object::State>,
    model_mesh_inverse: BTreeMap<usize, [f32; 16]>,
    pub(super) model_local_draws: HashSet<usize>,
    spherical_materials: BTreeMap<
        usize,
        std::result::Result<
            (
                Arc<material_preview::Program>,
                Vec<material_preview::MaterialImage>,
            ),
            String,
        >,
    >,
    pub(super) spherical_bounds: BTreeMap<usize, BTreeMap<u32, super::scene_bounds::Sphere>>,
    pub spherical_properties: super::spherical_lighting::PropertyStore,
    pub spherical_bindings: Vec<SphericalBinding>,
    pub child_lighting_properties: BTreeMap<u64, u64>,
    pub model_bounds: HashMap<String, super::scene_bounds::Hierarchy>,
    pub material_animation: super::material_animation::Runtime,
    pub jigsaw_effect_queue: super::jigsaw_transition::Queue,
    pub(super) jigsaw_endpoint_pending: BTreeSet<usize>,
    pub jigsaw_effect_values: HashMap<usize, f32>,
    pub sketch_draws: HashSet<usize>,
    pub hidden_jigsaw_draws: HashSet<usize>,
    pub jigsaw_instances: Vec<JigsawInstance>,
    pub jigsaw_owners: HashMap<usize, JigsawOwner>,
    pub jigsaws: HashMap<String, Vec<super::jigsaw::Asset>>,
    pub projectile_preview: super::brush_preview::Projectiles,
    pub brush_cursor: super::cursor_runtime::Runtime,
    pub debug_font: std::result::Result<super::debug_font::Font, String>,
    pub brush_decals: BTreeMap<String, super::decal::Component>,
    pub brush_stimuli: BTreeMap<String, super::projectile_stimulus::Stimuli>,
    pub brush: super::brush_render::Brush,
    pub spin_ribbon: Option<super::brush_particles::Ribbon>,
    pub spin_particles: Option<super::brush_particles::Hose>,
    pub ammo: super::ammo::State,
    pub brush_runtime: super::brush_runtime::Runtime,
    pub entities: Vec<RuntimeEntity>,
    pub player_entity: usize,
    pub player_animations: super::animation_resources::Resources,
    pub(super) player_attachment_pose: Vec<epic_mickey_lib_rs::hkx_file::HkxBone>,
    pub player_graph: super::generator_plan::Prepared,
    pub player_behavior: super::behavior::Document,
    pub player_meshes: Vec<PlayerMesh>,
    pub skins: HashMap<String, Vec<super::skin::Skin>>,
    pub draws: Vec<GpuDrawCommand>,
    /// Neutral-color buffers are generated only for inspection lighting and
    /// shared by every placed draw using the same source mesh.
    pub(super) inspection_vertex_buffers: super::inspection_vertex_cache::Cache,
    pub player_draws: Vec<usize>,
    pub collision: Collision,
    pub lights: Vec<GpuLight>,
    pub(super) enhanced_light_registry: super::scene_light_registry::Registry,
    pub report: Report,
}
/// Resolved authored state retained for gameplay. Scene-local IDs must never
/// be conflated across streamed scenes, even when names/templates agree.
pub struct RuntimeEntity {
    pub authored_scale: f32,
    /// Original unscaled matrix entries. Do not reconstruct these by dividing
    /// the rendered basis by scale; Havok's conversion never reads that scale.
    pub authored_rotation: [f32; 9],
    /// Live unscaled transform-owner rotation; physics must never recover this
    /// by dividing a render matrix (zero scale is a valid authored value).
    pub runtime_rotation: [f32; 9],
    /// Resolved resource identity; each entity still has independent bounds.
    pub model_asset: Option<String>,
    pub mesh_draws: HashMap<u32, Vec<usize>>,
    pub jigsaw_asset: Option<String>,
    pub jigsaw_states: Vec<super::jigsaw_state::PaintState>,
    pub scene: String,
    pub id: u32,
    pub name: String,
    pub transform: [f32; 16],
    pub authored_transform: [f32; 16],
    pub components: Vec<epic_mickey_lib_rs::scene_file::Component>,
    pub draws: std::ops::Range<usize>,
    pub collision_parts: std::ops::Range<usize>,
}
fn dynamic_collision_assets(
    entities: &[RuntimeEntity],
    files: &[VirtualFile],
) -> Vec<super::dynamic_body::Imported> {
    dynamic_collision_assets_with_cache(entities, files, &[])
}
fn dynamic_collision_assets_with_cache(
    entities: &[RuntimeEntity],
    files: &[VirtualFile],
    existing: &[super::dynamic_body::Imported],
) -> Vec<super::dynamic_body::Imported> {
    type Geometry = std::result::Result<Arc<Vec<super::collision::StaticMesh>>, String>;
    type Systems = std::result::Result<Arc<Vec<super::physics_system::Imported>>, String>;
    let mut cache = existing
        .iter()
        .map(|body| {
            (
                body.asset.clone(),
                (
                    body.geometry.clone(),
                    body.systems.clone(),
                    body.selected_system.clone(),
                ),
            )
        })
        .collect::<BTreeMap<
            String,
            (
                Geometry,
                Systems,
                std::result::Result<Option<usize>, String>,
            ),
        >>();
    let mut output = Vec::new();
    for (entity_index, entity) in entities.iter().enumerate() {
        for component in entity
            .components
            .iter()
            .filter(|c| c.class_name == "PhysicsRigidBodyDynamicComponent")
        {
            // Keep lookup scoped to this component. A different component's
            // RB Hull File Path is not a substitute for a missing property.
            let requested = component
                .properties
                .iter()
                .find(|p| p.name == "RB Hull File Path")
                .and_then(|p| p.value.as_str())
                .map(|p| p.trim_matches(&['\"', '\'', ' '][..]));
            let (asset, geometry, systems, selected_system) = match requested
                .filter(|p| !p.is_empty())
            {
                None => (
                    String::new(),
                    Err("Dynamic rigid body has no collision asset".into()),
                    Err("Dynamic rigid body has no physics resource".into()),
                    Ok(None),
                ),
                Some(path) => match lookup(files, path) {
                    None => (
                        path.to_owned(),
                        Err(format!("Missing dynamic collision resource {path}")),
                        Err(format!("Missing physics resource {path}")),
                        Err(format!("Missing physics resource {path}")),
                    ),
                    Some(file) => {
                        let asset = key(&file.path);
                        let result = cache
                            .entry(asset.clone())
                            .or_insert_with(|| match HkxFile::load_bytes(&file.data) {
                                Ok(hkx) => {
                                    let systems = super::physics_system::import(&hkx)
                                        .map(Arc::new)
                                        .map_err(|e| format!("{e:#}"));
                                    let selected = systems.as_ref().map_err(Clone::clone).and_then(
                                        |systems| {
                                            super::physics_system::select_first(&hkx, systems)
                                                .map_err(|e| format!("{e:#}"))
                                        },
                                    );
                                    (
                                        super::collision::body_local_meshes(&hkx)
                                            .map(Arc::new)
                                            .map_err(|e| format!("{e:#}")),
                                        systems,
                                        selected,
                                    )
                                }
                                Err(error) => (Err(error.clone()), Err(error.clone()), Err(error)),
                            })
                            .clone();
                        (asset, result.0, result.1, result.2)
                    }
                },
            };
            output.push(super::dynamic_body::Imported {
                entity: entity_index,
                component: component.link_id.to_u32(),
                asset,
                geometry,
                systems,
                selected_system,
                initial_velocity: super::dynamic_body::InitialVelocity::from_component(component)
                    .map_err(|e| format!("{e:#}")),
            });
        }
    }
    output
}
pub(super) fn render_model_path(
    components: &[epic_mickey_lib_rs::scene_file::Component],
) -> Result<Option<String>> {
    let mut paths = BTreeMap::new();
    for component in components.iter().filter(|c| {
        matches!(
            c.class_name.as_str(),
            "JPSSceneGraphComponent"
                | "AnimationComponent"
                | "AnimationComponentGB"
                | "SkyboxComponent"
                | "BeamComponent"
        )
    }) {
        if let Some(property) = component
            .properties
            .iter()
            .find(|p| p.name == "NIF File Path")
        {
            let path = property
                .value
                .as_str()
                .context("Invalid rendering NIF File Path")?
                .trim_matches(&['"', '\'', ' '][..]);
            if !path.is_empty() {
                paths.insert(key(path), path.to_string());
            }
        }
    }
    ensure!(
        paths.len() <= 1,
        "Multiple rendering model resources need separate runtime instances: {:?}",
        paths.values().collect::<Vec<_>>()
    );
    Ok(paths.into_values().next())
}
impl RuntimeEntity {
    /// A retired entity keeps its stable world slot and authored identity, but
    /// no longer needs the component/property tree used by live gameplay.
    /// Replacing the containers (rather than clearing them) releases their
    /// nested allocations immediately and leaves the tombstone lightweight.
    fn release_retired_component_data(&mut self) {
        self.components = Vec::new();
        self.mesh_draws = HashMap::new();
    }
    pub fn initial_physics_transform(&self) -> super::native_hk_transform::Transform {
        super::native_hk_transform::from_entity(
            self.authored_rotation,
            self.authored_transform[12..15].try_into().unwrap(),
        )
    }
    pub fn physics_transform(&self) -> super::native_hk_transform::Transform {
        super::native_hk_transform::from_entity(
            self.runtime_rotation,
            self.transform[12..15].try_into().unwrap(),
        )
    }
    /// ChildComponent initialization (REL 0x3121f4) allocates a shared SH
    /// property only when this authored flag is enabled and a model exists.
    pub fn children_share_lighting(&self) -> Result<bool> {
        let Some(component) = self
            .components
            .iter()
            .find(|c| c.class_name == "ChildComponent")
        else {
            return Ok(false);
        };
        let Some(property) = component
            .properties
            .iter()
            .find(|p| p.name == "ChildrenShareSphericalHarmonicLighting")
        else {
            return Ok(false);
        };
        property.value.as_bool().with_context(|| {
            format!(
                "Invalid ChildrenShareSphericalHarmonicLighting on {}:{}",
                self.scene, self.name
            )
        })
    }
    /// Transform attachment is distinct from prefab inheritance.
    pub fn transform_parent_id(&self) -> Result<Option<u32>> {
        let Some(component) = self
            .components
            .iter()
            .find(|c| c.class_name == "JPSTransformationComponent")
        else {
            return Ok(None);
        };
        let Some(property) = component
            .properties
            .iter()
            .find(|p| p.name == "Parent Entity")
        else {
            return Ok(None);
        };
        let id = property
            .value
            .as_u64()
            .and_then(|id| u32::try_from(id).ok())
            .with_context(|| format!("Invalid Parent Entity on {}:{}", self.scene, self.name))?;
        Ok((id != 0).then_some(id))
    }
    /// Mirror the current controller pose using the same composition as rendering.
    /// Rebuild from authored basis every time: accumulating yaw would cause drift.
    pub fn set_controller_pose(&mut self, position: V, yaw: f32) {
        self.transform = self.controller_matrix(position, yaw);
        let authored_yaw = self.authored_rotation[2].atan2(self.authored_rotation[8]);
        let (s, c) = (yaw - authored_yaw).sin_cos();
        for col in 0..3 {
            self.runtime_rotation[col] =
                c * self.authored_rotation[col] + s * self.authored_rotation[6 + col];
            self.runtime_rotation[3 + col] = self.authored_rotation[3 + col];
            self.runtime_rotation[6 + col] =
                -s * self.authored_rotation[col] + c * self.authored_rotation[6 + col];
        }
    }
    pub fn controller_matrix(&self, position: V, yaw: f32) -> [f32; 16] {
        let mut matrix = self.authored_transform;
        let authored_yaw = self.authored_rotation[2].atan2(self.authored_rotation[8]);
        let (s, c) = (yaw - authored_yaw).sin_cos();
        for col in 0..3 {
            matrix[col * 4] =
                c * self.authored_transform[col * 4] + s * self.authored_transform[col * 4 + 2];
            matrix[col * 4 + 2] =
                -s * self.authored_transform[col * 4] + c * self.authored_transform[col * 4 + 2];
        }
        matrix[12..15].copy_from_slice(&position);
        matrix
    }

    pub(super) fn has_character_body(&self) -> bool {
        self.components.iter().any(|component| component.class_name == "PhysicsRigidBodyCharacterComponent")
    }

    /// Query current components, preserving native precedence rather than
    /// inferring projectile behavior from mesh motion type or collision layer.
    pub fn projectile_target_body(&self) -> Option<super::projectile_stimulus::TargetBody> {
        super::projectile_stimulus::TargetBody::find(
            self.components.iter().map(|c| c.class_name.as_str()),
        )
    }
    /// REL 0x3906e0 replaces the shape material's real flag with the selected
    /// rigid body's Is Real value. Characters and targets without such a body
    /// retain the shape flag obtained earlier in the collision handler.
    pub fn projectile_real_surface(&self, shape_real: bool) -> Result<bool> {
        use super::projectile_stimulus::TargetBody;
        match self.projectile_target_body() {
            Some(body @ (TargetBody::Dynamic | TargetBody::Phantom | TargetBody::Static)) => {
                let component = self
                    .components
                    .iter()
                    .find(|c| c.class_name == body.class_name())
                    .context("Projectile target body disappeared")?;
                component
                    .properties
                    .iter()
                    .find(|p| p.name == "Is Real")
                    .and_then(|p| p.value.as_bool())
                    .with_context(|| {
                        format!("Missing or invalid Is Real on {}:{}", self.scene, self.name)
                    })
            }
            _ => Ok(shape_real),
        }
    }
}
impl World {
    /// Drop capacity left over from scene parsing after the initial entity
    /// set has been fully initialized. Authored values stay byte-for-byte
    /// equivalent; vectors and strings can grow normally when scripts mutate
    /// them later.
    fn compact_startup_entity_payloads(&mut self) -> usize {
        let before = super::debug_view::runtime_entity_heap_bytes(self);
        for entity in &mut self.entities {
            entity.name.shrink_to_fit();
            entity.scene.shrink_to_fit();
            entity.components.shrink_to_fit();
            for component in &mut entity.components {
                component.class_name.shrink_to_fit();
                component.properties.shrink_to_fit();
                for property in &mut component.properties {
                    property.class_name.shrink_to_fit();
                    property.name.shrink_to_fit();
                    compact_json_value(&mut property.value);
                }
            }
        }
        let after = super::debug_view::runtime_entity_heap_bytes(self);
        before
            .0
            .saturating_sub(after.0)
            .saturating_add(before.1.saturating_sub(after.1))
            .saturating_add(before.2.saturating_sub(after.2))
    }

    pub(super) fn retained_source_collision_payload(&self) -> (usize, usize) {
        let mut resources = HashSet::new();
        let mut mopp_programs = HashSet::new();
        let mut shape_count = 0usize;
        let mut bytes = 0usize;
        let mut add = |shapes: &Arc<Vec<super::collision::StaticMesh>>| {
            if !resources.insert(Arc::as_ptr(shapes) as usize) {
                return;
            }
            shape_count = shape_count.saturating_add(shapes.len());
            for shape in shapes.iter() {
                bytes = bytes
                    .saturating_add(shape.vertices.capacity() * std::mem::size_of::<[f32; 3]>())
                    .saturating_add(shape.indices.capacity() * std::mem::size_of::<u32>());
                if let Some(program) = &shape.mopp {
                    if mopp_programs.insert(Arc::as_ptr(program) as usize) {
                        bytes = bytes.saturating_add(program.bytes.len());
                    }
                }
            }
        };
        for shapes in self.resource_phantom_shapes.values() {
            add(shapes);
        }
        for body in &self.dynamic_bodies {
            if let Ok(shapes) = &body.geometry {
                add(shapes);
            }
        }
        (shape_count, bytes)
    }

    /// Drop decode-cache entries which have no current asset owner. A future
    /// streamed scene can decode them again from its source packfile; keeping
    /// unreferenced pixels here merely defeats the cache's share-only role.
    fn prune_unused_texture_cache(&mut self) -> usize {
        let mut unique = HashMap::<usize, usize>::new();
        for image in self.texture_cache.values().flatten() {
            if Arc::strong_count(&image.rgba) == 1 {
                unique
                    .entry(Arc::as_ptr(&image.rgba) as usize)
                    .or_insert_with(|| image.rgba.capacity());
            }
        }
        let released = unique.values().copied().sum();
        self.texture_cache.retain(|_, image| {
            image
                .as_ref()
                .is_some_and(|image| Arc::strong_count(&image.rgba) > 1)
        });
        released
    }

    pub fn inspection_vertices(&mut self, draw: usize) -> Option<Arc<Vec<f32>>> {
        let source = self.draws.get(draw)?.v_data.clone();
        let cached = self.inspection_vertex_buffers.neutralized(&source);
        Some(cached)
    }

    pub(super) fn source_data_root(&self) -> &Path {
        &self.data_root
    }
    /// Header-owned live state only. The checkpoint coordinator separately
    /// supplies callback names, retained overrides, timing and section streams.
    pub fn write_save_header_owners(&self, header: &mut [u8]) -> Result<()> {
        ensure!(
            header.len() >= super::save_game::HEADER_SIZE,
            "Truncated save header"
        );
        let metadata = self.checkpoint_summary()?;
        let global_metadata =
            super::checkpoint_metadata::Globals::capture(|name| self.globals.get(name))?;
        let inventory_metadata = self.checkpoint_inventory_limits()?;
        self.level_resource.write_header(header)?;
        metadata.write_header(header)?;
        global_metadata.write_header(header)?;
        inventory_metadata.write_header(header)?;
        self.extras.write_save_header(header)?;
        self.collectibles.write_save_header(header)?;
        self.game_options.write_header(header)?;
        self.playthrough.write_header(header)?;
        Ok(())
    }
    pub fn set_guardian_enabled(&mut self, enabled: bool) -> Result<()> {
        self.set_guardian_presentation(enabled, false)
    }
    pub(super) fn set_guardian_visible(&mut self, visible: bool) -> Result<()> {
        self.set_guardian_presentation(visible, true)
    }
    fn set_guardian_presentation(&mut self, value: bool, visibility: bool) -> Result<()> {
        struct Effects<'a>(&'a mut World);
        impl super::guardians::Effects for Effects<'_> {
            fn recompute(&mut self) -> Result<bool> {
                anyhow::bail!("Guardian selected-state population recomputation is not implemented")
            }
            fn present(&mut self, visible: bool) -> Result<()> {
                self.0.guardian_audio.present(visible)?;
                let owned = self.0.guardian_entities.entities().to_vec();
                for entity in owned {
                    self.0.set_entity_hidden(entity, !visible)?;
                }
                Ok(())
            }
        }
        let mut state = std::mem::take(&mut self.guardians);
        let result = if visibility {
            state.set_visible(value, &mut Effects(self))
        } else {
            state.set_enabled(value, &mut Effects(self))
        };
        self.guardians = state;
        let limitation="Guardian enable/disable drives global audio actions and owned entity visibility. Audio device playback, guardian population and controller motion remain incomplete.";
        if !self
            .report
            .unsupported
            .iter()
            .any(|message| message == limitation)
        {
            self.report.unsupported.push(limitation.into());
        }
        result
    }
    pub fn set_entity_hidden(&mut self, entity: usize, hidden: bool) -> Result<()> {
        self.set_entities_hidden(&[entity], hidden).map(|_| ())
    }
    /// Apply one visibility operation to several roots while building the
    /// world-wide hierarchy only once. Scene streaming commonly toggles many
    /// entities together; rebuilding the same maps per entity made that path
    /// quadratic in world size and caused large transient allocations.
    pub(super) fn set_entities_hidden(
        &mut self,
        entities: &[usize],
        hidden: bool,
    ) -> Result<Vec<usize>> {
        for &entity in entities {
            ensure!(
                entity < self.entities.len() && !self.removed_entities.contains(&entity),
                "Visibility target is not a live entity"
            );
        }
        if entities.is_empty() {
            return Ok(Vec::new());
        }
        let mut children = BTreeMap::<usize, Vec<usize>>::new();
        let identities = self
            .entities
            .iter()
            .enumerate()
            .filter(|(index, _)| !self.removed_entities.contains(index))
            .map(|(index, entity)| ((entity.scene.as_str(), entity.id), index))
            .collect::<HashMap<_, _>>();
        for (child, entity) in self.entities.iter().enumerate() {
            if self.removed_entities.contains(&child) {
                continue;
            }
            if let Some(local) = self.transform_hierarchy.get(&child) {
                children.entry(local.parent).or_default().push(child);
            } else if let Some(id) = entity.transform_parent_id()? {
                if let Some(&parent) = identities.get(&(entity.scene.as_str(), id)) {
                    children.entry(parent).or_default().push(child);
                }
            }
        }
        struct Visibility<'a> {
            world: &'a mut World,
            children: BTreeMap<usize, Vec<usize>>,
            newly_hidden: Vec<usize>,
        }
        impl super::visibility_children::Backend for Visibility<'_> {
            fn local_hidden(&mut self, owner: usize, hidden: bool) -> Result<()> {
                // Native component hooks precede children and the final flag
                // setter. Pending construction consumes the committed state.
                self.world.set_rigid_body_visibility(owner, hidden)?;
                if hidden {
                    self.world
                        .hidden_entity_draws
                        .extend(self.world.entities[owner].draws.clone());
                } else {
                    for draw in self.world.entities[owner].draws.clone() {
                        self.world.hidden_entity_draws.remove(&draw);
                    }
                }
                Ok(())
            }
            fn children(&mut self, owner: usize) -> Result<Vec<usize>> {
                Ok(self.children.get(&owner).cloned().unwrap_or_default())
            }
            fn hidden(&self, owner: usize) -> bool {
                self.world.hidden_entities.contains(&owner)
            }
            fn previous_hidden(&self, owner: usize) -> bool {
                self.world.previous_hidden_entities.contains(&owner)
            }
            fn snapshot_hidden(&mut self, owner: usize) {
                self.world.snapshot_entity_hidden(owner);
            }
            fn commit_hidden(&mut self, owner: usize, hidden: bool) {
                if hidden {
                    if !self.world.hidden_entities.contains(&owner) {
                        self.newly_hidden.push(owner);
                    }
                    self.world.hidden_entities.insert(owner);
                } else {
                    self.world.hidden_entities.remove(&owner);
                }
            }
        }
        let mut visibility = Visibility {
            world: self,
            children,
            newly_hidden: Vec::new(),
        };
        for &entity in entities {
            super::visibility_children::set(&mut visibility, entity, hidden)?;
        }
        let newly_hidden = visibility.newly_hidden;
        let limitation="Script model/transform-child visibility, native child hidden snapshots/vetoes and supported rigid-body world membership are connected; phantom admission, native world listeners/island rebuilds, other component visibility vetoes, emitter restart, notifications and audio/light hide behavior remain unimplemented.";
        if !self
            .report
            .unsupported
            .iter()
            .any(|line| line == limitation)
        {
            self.report.unsupported.push(limitation.into());
        }
        Ok(newly_hidden)
    }
    pub(super) fn snapshot_entity_hidden(&mut self, entity: usize) {
        if self.hidden_entities.contains(&entity) {
            self.previous_hidden_entities.insert(entity);
        } else {
            self.previous_hidden_entities.remove(&entity);
        }
    }
    /// REL297844 selects rigid-body hooks before its phantom fallback. This
    /// binds existing static and admitted dynamic geometry only; it does not
    /// fabricate phantom admission or convert an unsupported physics system.
    fn set_rigid_body_visibility(&mut self, entity: usize, hidden: bool) -> Result<()> {
        if !self.entities[entity].components.iter().any(|component| {
            matches!(
                component.class_name.as_str(),
                "PhysicsRigidBodyStaticComponent"
                    | "PhysicsRigidBodyDynamicComponent"
                    | "PhysicsRigidBodyCharacterComponent"
            )
        }) {
            return Ok(());
        }
        let target = &self.entities[entity];
        let mut owners = self
            .report
            .collision_parts
            .iter()
            .filter(|part| {
                part.scene == target.scene
                    && part.entity_id == target.id
                    && self.imported_static_part(part.owner).is_some()
                    && self.collision.owner_shapes(part.owner).next().is_some()
            })
            .map(|part| part.owner)
            .collect::<std::collections::BTreeSet<_>>();
        owners.extend(
            self.dynamic_runtime_sources
                .iter()
                .filter_map(|(&source, &owner)| {
                    (self.dynamic_bodies[source].entity == entity).then_some(owner)
                }),
        );
        if let Some(Ok(body)) = self.character_physics.get(&entity) {
            owners.insert(body.owner);
        }
        if entity == self.player_entity {
            if let Some(body) = &self.player_physics {
                owners.insert(body.owner);
            }
        }
        for owner in owners {
            self.collision.set_owner_in_world(owner, !hidden)?;
        }
        Ok(())
    }
    /// Publish the original sketch root through shared rendering, actor and
    /// physics paths. Placement/action eligibility and child FX are separate
    /// lifecycle stages, still required before this is a complete ability.
    pub fn spawn_sketch_root(
        &mut self,
        name: &str,
        position: V,
        rotation: [f32; 9],
    ) -> Result<usize> {
        ensure!(
            position.iter().chain(&rotation).all(|v| v.is_finite()),
            "Invalid sketch pose"
        );
        let template_key = name.to_ascii_lowercase();
        let template = self
            .sketch_templates
            .templates
            .get(&template_key)
            .context("Sketch template missing")?;
        let definition = &template.graph.definitions[&template.graph.root];
        ensure!(
            definition.children.is_empty(),
            "Sketch child entity instantiation remains unsupported"
        );
        let path = definition
            .model
            .as_ref()
            .context("Sketch root has no model")?
            .clone();
        let asset = key(&path);
        let model = self
            .sketch_templates
            .models
            .get(&asset)
            .context("Sketch model was not prepared")?;
        let hierarchy = model.hierarchy.clone();
        let actor = template
            .animation
            .as_ref()
            .map_err(|error| anyhow::anyhow!(error.clone()))?
            .as_ref()
            .context("Sketch root has no supported Havok actor")?;
        // Ensure a complete initial pose can be rendered before publishing.
        ensure!(
            !actor.resources.rig.bones.is_empty(),
            "Sketch actor lacks skeleton"
        );
        let components = definition.components.clone();
        let source = epic_mickey_lib_rs::scene_file::Entity::new(
            "JPSGeneralEntity".into(),
            definition.name.clone(),
            epic_mickey_lib_rs::scene_file::ID::new(1),
            epic_mickey_lib_rs::scene_file::ID::new(0),
            0,
            0,
            components.clone(),
        );
        let authored = scene::get_entity_transform(&source, &[]);
        let scale = authored.scale[0];
        ensure!(scale.is_finite() && scale > 0., "Unsupported sketch scale");
        let transform = super::render_transform::from_entity(rotation, position, scale);
        let entity = self.entities.len();
        let id = self
            .entities
            .iter()
            .map(|e| e.id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .context("Runtime entity ID exhausted")?;
        let draw_start = self.draws.len();
        let mut mesh_draws = HashMap::<u32, Vec<usize>>::new();
        let mut prepared_draws = Vec::new();
        let mut lighting_slots = Vec::new();
        for mesh in &model.meshes {
            let mut draw = mesh.prepared_draw();
            draw.trans_mat = transform;
            if mesh.runtime_sh_slot.is_some() {
                draw.preview = Some(Err(
                    "Sketch lighting awaits current world/view update".into()
                ));
            }
            mesh_draws
                .entry(mesh.block)
                .or_default()
                .push(draw_start + prepared_draws.len());
            lighting_slots.push((mesh.block, mesh.runtime_sh_slot, mesh.prepared_draw()));
            prepared_draws.push(draw);
        }
        let runtime = RuntimeEntity {
            authored_scale: scale,
            authored_rotation: rotation,
            runtime_rotation: rotation,
            model_asset: Some(asset.clone()),
            mesh_draws,
            jigsaw_asset: None,
            jigsaw_states: Vec::new(),
            scene: definition.source_scene.clone(),
            id,
            name: format!("{} [runtime {id}]", definition.name),
            transform,
            authored_transform: transform,
            components,
            draws: draw_start..draw_start + prepared_draws.len(),
            collision_parts: 0..0,
        };
        ensure!(
            runtime.transform_parent_id()?.is_none(),
            "Sketch root has unsupported parent"
        );
        let mut local_files = Vec::new();
        for component in runtime
            .components
            .iter()
            .filter(|c| c.class_name == "PhysicsRigidBodyDynamicComponent")
        {
            let path = component
                .properties
                .iter()
                .find(|p| p.name == "RB Hull File Path")
                .and_then(|p| p.value.as_str())
                .context("Sketch collision path missing")?;
            let requested = key(path);
            let (identity, bytes) = self
                .sketch_templates
                .resources
                .iter()
                .find(|(identity, _)| {
                    **identity == requested || **identity == format!("{requested}_wii")
                })
                .context("Sketch collision resource missing")?;
            local_files.push(VirtualFile::new(
                epic_mickey_lib_rs::packfile::EndianDependentString::new(" HKX".into()),
                false,
                0,
                identity.clone(),
                bytes.to_vec(),
            ));
        }
        let mut imported = dynamic_collision_assets_with_cache(
            std::slice::from_ref(&runtime),
            &local_files,
            &self.dynamic_bodies,
        );
        ensure!(!imported.is_empty(), "Sketch has no dynamic body");
        for body in &mut imported {
            body.entity = entity;
        }
        let source_start = self.dynamic_bodies.len();
        self.entities.push(runtime);
        self.draws.extend(prepared_draws);
        self.model_bounds.entry(asset).or_insert(hierarchy);
        self.dynamic_bodies.extend(imported);
        let result = (|| -> Result<()> {
            self.scripts
                .register_entity(entity, &self.entities[entity].name, &[])?;
            self.outputs.bind(
                entity,
                &self.entities[entity].name,
                &self.entities[entity].components,
            )?;
            self.bind_entity_lifetime(entity)?;
            self.construct_dynamic_sources(source_start..self.dynamic_bodies.len())?;
            ensure!(
                (source_start..self.dynamic_bodies.len()).all(|source| self
                    .dynamic_constructions
                    .get(&source)
                    .is_some_and(|state| state.completed && state.bodies.len() == 1)),
                "Sketch body construction incomplete"
            );
            let mut next = self.collision.next_owner()?;
            self.admit_dynamic_sources(source_start..self.dynamic_bodies.len(), &mut next)?;
            ensure!(
                (source_start..self.dynamic_bodies.len())
                    .all(|source| self.dynamic_runtime_sources.contains_key(&source)),
                "Sketch body admission incomplete"
            );
            self.activate_sketch_actor(entity, &template_key)?;
            let mut script_paths = Vec::new();
            for component in self.entities[entity]
                .components
                .iter()
                .filter(|component| component.class_name == "ScriptComponent")
            {
                for property in component
                    .properties
                    .iter()
                    .filter(|property| property.name == "Lua File Path")
                {
                    script_paths.extend(super::actor_scripts::authored_paths(&property.value)?);
                }
            }
            for path in script_paths {
                let bytes = self
                    .sketch_templates
                    .resources
                    .get(&key(&path))
                    .context("Prepared sketch script dependency missing")?
                    .clone();
                self.with_script_world_host(Self::script_world_api, |scripts| {
                    scripts.ensure_module(&path, &bytes, entity)
                })?;
            }
            let root = *self
                .entity_bound_hierarchy(entity)?
                .context("Sketch bound hierarchy missing")?
                .roots
                .iter()
                .flatten()
                .next()
                .context("Sketch model root missing")?;
            let mut property = None;
            for (offset, (mesh, slot, mut draw)) in lighting_slots.into_iter().enumerate() {
                let Some(slot) = slot else { continue };
                let id = match property {
                    Some(id) => id,
                    None => {
                        let id = self.spherical_properties.allocate(
                            super::spherical_lighting::BoundOwner {
                                entity: entity as u64,
                                node: root,
                            },
                        )?;
                        property = Some(id);
                        id
                    }
                };
                let property = self
                    .spherical_properties
                    .attach((entity as u64, mesh), id)?;
                let program = draw
                    .preview
                    .take()
                    .context("Sketch material missing")?
                    .map_err(anyhow::Error::msg)?;
                // The actual image is produced in apply_spherical_lighting
                // before display. This slot descriptor contains no fake pixels.
                if program.texture_slots.contains(&slot) {
                    let binding = draw.material.as_ref().and_then(|material| {
                        material
                            .textures
                            .iter()
                            .rev()
                            .find(|texture| texture.slot == u32::from(slot))
                    });
                    let sampler = binding
                        .map(|b| b.sampler)
                        .or_else(|| {
                            program
                                .template_samplers
                                .get(slot as usize)
                                .copied()
                                .flatten()
                        })
                        .unwrap_or_default();
                    let matrix = binding
                        .and_then(|b| b.transform)
                        .and_then(|t| t.matrix())
                        .unwrap_or([1., 0., 0., 0., 1., 0., 0., 0., 1.]);
                    Arc::make_mut(&mut draw.material_images).push(
                        material_preview::MaterialImage {
                            slot,
                            rgba: Arc::new(Vec::new()),
                            width: 16,
                            height: 16,
                            sampler,
                            matrix,
                        },
                    );
                }
                self.spherical_materials.insert(
                    draw_start + offset,
                    Ok((program, draw.material_images.as_ref().clone())),
                );
                self.spherical_bindings.push(SphericalBinding {
                    entity,
                    mesh,
                    draw: draw_start + offset,
                    property,
                    slot: u32::from(slot),
                });
            }
            self.emit_entity_output(entity, "OnSpawned", entity);
            let immediate = self.outputs.ready_for_owner(self.animation_time, entity);
            self.dispatch_actor_deliveries(immediate);
            Ok(())
        })();
        if let Err(error) = result {
            self.retire_entity(entity)?;
            return Err(error);
        }
        let message="Sketch root creation is active with original animated meshes, physics and lifetime timers. Action/inventory placement, child FX, impact callbacks and complete script activation remain unfinished.";
        if !self.report.unsupported.iter().any(|error| error == message) {
            self.report.unsupported.push(message.into());
        }
        Ok(entity)
    }
    pub(super) fn player_checkpoint_death_available(&self) -> bool {
        self.checkpoint.latest.as_ref().is_some_and(|snapshot| snapshot.has_active_campaign())
            && self.outputs.has_connections(self.player_entity, "OnDeath")
    }

    fn advance_health_lifecycle(&mut self) -> Result<()> {
        let owners = self
            .runtime_health
            .keys()
            .copied()
            .filter(|&entity| self.entity_scene_active(entity))
            .collect::<Vec<_>>();
        let pending = owners
            .into_iter()
            .filter_map(|entity| {
                self.runtime_health
                    .get_mut(&entity)
                    .and_then(|state| state.take_death().then_some(entity))
            })
            .collect::<Vec<_>>();
        for entity in pending {
            if self.removed_entities.contains(&entity) {
                continue;
            }
            if entity == self.player_entity {
                // REL323be4..323bf8 clears the damage timer on death.
                self.runtime_health.get_mut(&entity).unwrap().invulnerable_until = 0.;
                if self.player_checkpoint_death_available() {
                    self.emit_entity_output(entity, "OnDeath", entity);
                    let deliveries = self.outputs.ready_for_owner(self.animation_time, entity);
                    self.dispatch_actor_deliveries(deliveries);
                }
                let message = "Original player death animation and fade timing are not connected".to_string();
                if !self.report.unsupported.contains(&message) {
                    self.report.unsupported.push(message);
                }
                continue;
            }
            let component = self.entities[entity]
                .components
                .iter()
                .find(|c| c.class_name == "HealthComponent")
                .context("Missing zero-health component")?;
            let field = |name: &str| {
                component
                    .properties
                    .iter()
                    .find(|p| p.name == name)
                    .map(|p| &p.value)
            };
            let action = field("NoHealthAction")
                .and_then(|v| v.as_i64())
                .context("Missing NoHealthAction")?;
            // Effect publication can grow entities, so release the property borrow first.
            let sound = field("NoHealthSFX").and_then(|v| v.as_str()).unwrap_or("").to_owned();
            let effect = field("NoHealthEffect").and_then(|v| v.as_str()).unwrap_or("").to_owned();
            if !sound.is_empty() {
                self.post_audio_from_game(entity, &sound);
            }
            if !effect.is_empty() {
                if let Err(error) = self.start_one_shot_effect(entity, &effect) {
                    self.report.unsupported.push(format!("Death effect {} -> {effect}: {error:#}", self.entities[entity].name));
                }
            }
            let actions = self.component_events.receive_zero_health(
                entity,
                matches!(action, 1 | 2),
                self.animation_time,
            )?;
            self.execute_component_events(actions)?;
            if matches!(action, 1 | 2) {
                if let Err(error)=self.receive_death_rewards(entity,2) {
                    self.report.unsupported.push(format!("Death reward receiver {}: {error:#}",self.entities[entity].name));
                }
            }
            if self.demo_story.enabled {
                self.emit_entity_output(entity, "OnDeath", entity);
                let deliveries = self.outputs.ready_for_owner(self.animation_time, entity);
                self.dispatch_actor_deliveries(deliveries);
            }
            if action == 2
                || (self.demo_story.enabled && action == 1 && !self.ai_brains.contains_key(&entity))
            {
                // Demo scenery destruction bridge; animated actor deaths remain separate.
                // Demo lifecycle bridge: a death output may deliberately have
                // a delay (the opening gear's DoorBreak uses 0.3 seconds).
                // Withdraw presentation/physics now, retaining the callback
                // owner until its admitted deliveries have started. Native
                // delayed-action reference ownership still needs validation.
                if self.demo_story.enabled && self.outputs.has_pending_for_owner(entity) {
                    self.set_entity_hidden(entity, true)?;
                    self.demo_story.pending_deaths.insert(entity);
                } else {
                    self.retire_entity(entity)?;
                }
            }
        }
        Ok(())
    }
    pub(super) fn bind_entity_lifetime(&mut self, entity: usize) -> Result<()> {
        let mut components = self.entities[entity]
            .components
            .iter()
            .filter(|component| component.class_name == "LifetimeComponent");
        let Some(component) = components.next() else {
            return Ok(());
        };
        ensure!(
            components.next().is_none(),
            "Multiple lifetime components need distinct runtime owners"
        );
        let state = super::lifetime::State::from_component(component, || {
            anyhow::bail!("Original shared lifetime RNG/startup stream is not connected")
        })?;
        if state.shrink {
            self.report.unsupported.push(format!(
                "Lifetime shrink/alpha binding is not connected: {}",
                self.entities[entity].name
            ));
        }
        if state.pause_during_igc || state.pause_all_during_igc {
            let message="Lifetime IGC pause rules are decoded, but global cutscene event routing is not connected.";
            if !self.report.unsupported.iter().any(|error| error == message) {
                self.report.unsupported.push(message.into());
            }
        }
        self.runtime_lifetimes.insert(entity, state);
        Ok(())
    }
    pub(super) fn set_entity_float_property(&mut self, entity: usize, name: &str, value: f32) -> Result<()> {
        ensure!(
            !self.removed_entities.contains(&entity) && value.is_finite(),
            "Invalid float-property target/value"
        );
        // REL2664e0 validates property type and invokes the entity's native
        // float setter. Merely editing JSON would bypass its runtime effects.
        // These descriptors instead use ordinary scalar value storage. Their
        // setters do not run mood evaluation or request a new AI destination.
        let stored_owner = match name {
            "NegativeThreshold" | "PositiveThreshold" => Some("MoodComponent"),
            "Wander Radius" => Some("AIBrainComponent"),
            "Desired Move Speed" | "Desired Vertical Speed" | "PatrolSpeed" => Some("BasicAIControllerComponent"),
            "BranchPathChance"=>Some("SplineKnotComponent"),
            _ => None,
        };
        if let Some(owner) = stored_owner {
            let property = self.entities[entity].components.iter_mut()
                .find(|component| component.class_name == owner)
                .and_then(|component| component.properties.iter_mut().find(|property|property.name==name))
                .with_context(||format!("Missing {owner} scalar {name}"))?;
            ensure!(property.class_name=="Float" && !property.value.is_array(), "Invalid stored scalar type");
            property.value = serde_json::json!(value);
            if matches!(owner,"SplineKnotComponent"|"BasicAIControllerComponent") {return Ok(());}
            let limitation = "Mood thresholds and AI wander radius support live scalar reads/writes. Mood watching/state outputs and native AI wander destination selection remain unimplemented.";
            if !self.report.unsupported.iter().any(|message|message==limitation) {
                self.report.unsupported.push(limitation.into());
            }
            return Ok(());
        }
        if name == "Max Speed" {
            return self.set_rotator_speed(entity, value);
        }
        if name == "Acceleration" {
            return self.set_rotator_acceleration(entity, value);
        }
        if name == "InitialSpeed" {
            ensure!(self.entities[entity].components.iter()
                .any(|component| component.class_name == "SplineFollowerComponent"),
                "Missing SplineFollowerComponent owner for InitialSpeed");
            return self.set_live_spline_speed(entity, value);
        }
        let scalar_owner = match name {
            "Spawn Delay" | "Spawn Radius" => Some("TemplateSpawnerComponent"),
            // REL3c764c/3c7818 -> 3c7c64 -> 123d80 stores the target value.
            "Target Value" => Some("TargetableComponent"),
            _ => None,
        };
        if let Some(owner) = scalar_owner {
            let component = self.entities[entity]
                .components
                .iter_mut()
                .find(|component| component.class_name == owner)
                .with_context(|| format!("Missing {owner} owner for {name}"))?;
            // REL3ca6a0/3ca760 construct ordinary float descriptors using
            // 6e804. Value virtual+4c ->123da0 ->descriptor+18 ->6e854
            // ->123ec4 stores the scalar only; it does not reset countdown+48.
            if let Some(property) = component
                .properties
                .iter_mut()
                .find(|property| property.name == name)
            {
                ensure!(
                    property.class_name == "Float" && !property.value.is_array(),
                    "Invalid {owner}.{name} float type"
                );
                property.value = serde_json::json!(value);
            } else {
                component
                    .properties
                    .push(epic_mickey_lib_rs::scene_file::Property::new(
                        "Float".into(),
                        name.into(),
                        false,
                        false,
                        false,
                        serde_json::json!(value),
                    ));
            }
            return Ok(());
        }
        if name == "CounterStart" {
            let component = self.entities[entity]
                .components
                .iter()
                .find_map(|component| {
                    component
                        .properties
                        .iter()
                        .find(|property| property.name == name)
                        .map(|property| (component, property))
                })
                .context("CounterStart scalar property missing")?;
            ensure!(
                component.0.class_name == "PaintThinnerCounterComponent"
                    && component.1.class_name == "Float"
                    && !component.1.value.is_array(),
                "CounterStart setter has an unsupported property owner/type"
            );
            // Descriptor callback362c9c ->3653d0 writes the current scalar,
            // then refreshes material only when363888's fraction changes.
            // The initialization anchor, cached status and hit events survive.
            let counter = self
                .paint_counters
                .get_mut(&entity)
                .context("CounterStart setter has no initialized runtime")?;
            let before = counter.toon_fraction();
            counter.value = value;
            if counter.toon_fraction() != before {
                self.advance_counter_materials(entity..entity + 1);
                if let Some(error) = self
                    .counter_materials
                    .get(&entity)
                    .and_then(|state| state.error.as_ref())
                {
                    anyhow::bail!("{error}");
                }
            }
            return Ok(());
        }
        if name == "UsableRadius" {
            let component = self.entities[entity]
                .components
                .iter_mut()
                .find(|component| component.class_name == "UsableComponent")
                .context("Usable radius target has no usable component")?;
            if let Some(property) = component
                .properties
                .iter_mut()
                .find(|property| property.name == name)
            {
                ensure!(
                    property.class_name == "Float" && !property.value.is_array(),
                    "Invalid usable radius property"
                );
                property.value = serde_json::json!(value);
            } else {
                component
                    .properties
                    .push(epic_mickey_lib_rs::scene_file::Property::new(
                        "Float".into(),
                        name.into(),
                        false,
                        false,
                        false,
                        serde_json::json!(value),
                    ));
            }
            return Ok(());
        }
        ensure!(
            name == "Lifetime",
            "Native float setter is not connected for {name}"
        );
        let component = self.entities[entity]
            .components
            .iter_mut()
            .find(|component| component.class_name == "LifetimeComponent")
            .context("Lifetime component disappeared")?;
        let property = component
            .properties
            .iter_mut()
            .find(|property| property.name == "Lifetime")
            .context("Lifetime property disappeared")?;
        let state = self
            .runtime_lifetimes
            .get_mut(&entity)
            .context("Lifetime setter has no initialized component")?;
        state.timer.remaining = value;
        property.value = serde_json::json!(value);
        Ok(())
    }
    fn advance_entity_lifetimes(&mut self, dt: f32) -> Result<()> {
        let owners = self
            .runtime_lifetimes
            .keys()
            .copied()
            .filter(|&entity| self.entity_scene_active(entity))
            .collect::<Vec<_>>();
        let mut removal = Vec::new();
        for entity in owners {
            // Hidden spawner sources are factory inputs. Their lifetime
            // starts on a spawned instance, not on its authored source.
            if self.removed_entities.contains(&entity) || self.spawner_sources.contains(&entity) {
                continue;
            }
            let Some(update) = self.runtime_lifetimes[&entity].prepare(dt)? else {
                continue;
            };
            if update.expired {
                // EM1 event0x3b is OnLifetimeExpired, not OnDeath. The default
                // backend queues removal; only the brush pool installs a
                // special callback (RELaa9cc ->3595ac), handled by its owner.
                self.emit_entity_output(entity, "OnLifetimeExpired", entity);
                let immediate = self.outputs.ready_for_owner(self.animation_time, entity);
                self.dispatch_actor_deliveries(immediate);
                removal.push(entity);
            }
            // Preserve callback/IO changes to other state, but overwrite a
            // callback's Lifetime write with the saved subtraction, as35940c.
            if let Some(state) = self.runtime_lifetimes.get_mut(&entity) {
                state.commit(&update);
            }
            if let Some(property) = self.entities[entity]
                .components
                .iter_mut()
                .find(|component| component.class_name == "LifetimeComponent")
                .and_then(|component| {
                    component
                        .properties
                        .iter_mut()
                        .find(|property| property.name == "Lifetime")
                })
            {
                property.value = serde_json::json!(update.remaining);
            }
        }
        // Do not invalidate other timer owners while iterating the update set.
        // Native manager removal is deferred; full component scheduling and
        // callback/reclamation timing remain separately reference-unvalidated.
        for entity in removal {
            self.retire_entity(entity)?;
        }
        Ok(())
    }
    pub(super) fn retire_entity(&mut self, entity: usize) -> Result<()> {
        if let Some(effect) = self.named_fx.live.get_mut(&entity) {
            if !effect.ready {
                effect.begin_removal(true)?;
                return Ok(());
            }
        }
        if self.removed_entities.contains(&entity) { return Ok(()); }
        self.deliver_entity_removal(entity)?;
        // Transformation teardown353380 runs while all parent/node bindings
        // still exist. It detaches surviving children, then its own parent.
        self.detach_retiring_transform(entity)?;
        self.removed_entities.insert(entity);
        self.demo_story.motion.retire(entity);
        self.spawner_sources.remove(&entity);
        self.live_transform_children.remove(&entity);
        self.controller_feedback.pushed_emitters.remove(&entity);
        self.demo_story.pending_deaths.remove(&entity);
        self.cancel_story_owner(entity);
        self.story_sequences.remove_owner(entity);
        self.named_fx.retire(entity);
        self.lod_states.remove(&entity);
        self.guardian_entities.remove(Some(entity));
        let draws = self.entities[entity].draws.clone();
        self.material_animation.lock(draws.clone(), None);
        self.removed_draws.extend(draws.clone());
        self.hidden_entities.remove(&entity);
        self.previous_hidden_entities.remove(&entity);
        for draw in self.entities[entity].draws.clone() {
            self.hidden_entity_draws.remove(&draw);
        }
        self.remove_entity_physics(entity)?;
        self.release_retired_entity_assets(entity, draws);
        self.actors.remove(&entity);
        self.idle_actors.remove(&entity);
        self.model_animation.remove(&entity);
        self.actor_sequences.remove(&entity);
        self.actor_bindings.remove(&entity);
        self.rotators.retain(|(owner, _), _| *owner != entity);
        self.transform_hierarchy.remove(&entity);
        self.live_child_slots.remove(&entity);
        for slots in self.live_child_slots.values_mut() {for slot in slots {if *slot==Some(entity) {*slot=None;}}}
        self.dynamic_parent_bindings.retain(|(owner,_),_|*owner!=entity);
        self.havok_actors.remove(&entity);
        self.ai_brains.remove(&entity);
        self.demo_story.npc_fallback.retire(entity);
        self.demo_story.enemy_armor.retire(entity);
        self.demo_story.boss_ai.retire(entity);
        self.patrol_predecessors.remove(&entity);
        self.robot_arm_idle_requests.remove(&entity);
        self.robot_arm_controls.remove(&entity);
        self.cutscene_actions
            .retire(entity, &mut self.entity_event_registry);
        self.ledge_controls.remove(&entity);
        self.wall_pushing_disabled.remove(&entity);
        if self
            .nif_camera
            .is_some_and(|camera| camera.entity == entity)
        {
            self.nif_camera = None;
            self.nif_camera_transition = None;
            self.nif_camera_completed_frame = None;
        }
        self.paint_counters.remove(&entity);
        self.counter_materials.remove(&entity);
        self.model_paint.remove(&entity);
        self.model_paint_owners.retain(|_, owner| *owner != entity);
        self.pickup_settings.remove(&entity);
        self.pickup_update_states.remove(&entity);
        self.pickup_collection_blocked.remove(&entity);
        self.pickup_lifecycles.remove(&entity);
        self.pickup_receivers.remove(&entity);
        self.authored_inventory.remove(&entity);
        self.authored_health.remove(&entity);
        self.runtime_health.remove(&entity);
        self.death_rewarded.remove(&entity);
        self.dynamic_rewards.cursors.remove(&entity);
        self.size_phantoms.remove(&entity);
        self.resource_phantom_shapes.remove(&entity);
        self.resource_phantom_motion.remove(&entity);
        self.resource_phantom_bounds.remove(&entity);
        self.rigid_trigger_hulls.remove(&entity);
        self.size_phantom_filters.remove(&entity);
        self.size_phantom_handles.remove(&entity);
        self.resource_phantom_handles.remove(&entity);
        self.phantom_systems.remove(&entity);
        self.dynamic_constructions.retain(|source, _| {
            self.dynamic_bodies
                .get(*source)
                .is_none_or(|body| body.entity != entity)
        });
        let mut released_dynamic_source = false;
        for source in &mut self.dynamic_bodies {
            if source.entity != entity {
                continue;
            }
            source.geometry = Err("Owning entity was retired".into());
            source.systems = Err("Owning entity was retired".into());
            released_dynamic_source = true;
        }
        if released_dynamic_source {
            self.dynamic_body_sources =
                super::dynamic_body::SourceSummary::new(&self.dynamic_bodies);
        }
        self.render_objects.retain(|(owner, _), _| *owner != entity);
        self.ai_registry.retire(entity);
        self.multi_position_audio
            .retain(|(owner, _), _| *owner != entity);
        self.authored_havok.remove(entity);
        let (released_skin_bytes, released_skin_models) = self.prune_unused_skin_payloads();
        #[cfg(feature = "blotworx-memory-profile")]
        if released_skin_bytes > 0 {
            eprintln!(
                "[runtime-memory] retired entity released {:.1} MiB of skin payload across {} models",
                released_skin_bytes as f64 / 1_048_576.,
                released_skin_models
            );
        }
        #[cfg(not(feature = "blotworx-memory-profile"))]
        let _ = (released_skin_bytes, released_skin_models);
        self.runtime_lifetimes.remove(&entity);
        if self
            .actor_playback
            .as_ref()
            .is_some_and(|playback| playback.entity == entity)
        {
            self.actor_playback = None;
        }
        self.component_events.remove(entity);
        self.component_control.disabled.remove(&entity);
        self.spawners.remove(&entity);
        self.retire_deferred_output_events(entity);
        self.outputs.remove_owner(entity);
        self.remove_entity_lighting(entity)?;
        // The world cache owns one reference; active draws/templates keep
        // shared images alive through their Arcs. Prune only after every
        // retired owner (including lighting materials) has released its refs.
        self.prune_unused_texture_cache();
        self.scene_geometry_cache.prune();
        self.scene_collision_cache.prune();
        self.scene_sequence_cache.prune();
        self.scene_actor_binding_cache.prune();
        let (released_model_bounds, released_jigsaws) =
            self.prune_unused_scene_model_metadata(entity);
        #[cfg(feature = "blotworx-memory-profile")]
        if released_model_bounds || released_jigsaws {
            eprintln!(
                "[runtime-memory] retired entity released scene metadata: model_bounds={}, jigsaw_templates={}",
                released_model_bounds as u8,
                released_jigsaws as u8
            );
        }
        #[cfg(not(feature = "blotworx-memory-profile"))]
        let _ = (released_model_bounds, released_jigsaws);
        self.scripts.unregister_entity(entity)?;
        self.entities[entity].release_retired_component_data();
        Ok(())
    }
    fn release_retired_entity_assets(
        &mut self,
        entity: usize,
        draws: std::ops::Range<usize>,
    ) {
        for draw in draws {
            self.draws[draw].release_retired_assets();
            self.actor_draw_sources.remove(&draw);
            self.model_mesh_inverse.remove(&draw);
            self.model_local_draws.remove(&draw);
            self.jigsaw_effect_values.remove(&draw);
            self.sketch_draws.remove(&draw);
            self.hidden_jigsaw_draws.remove(&draw);
            for cold in self.cold_scene_assets.values_mut() {
                cold.retire_draw(draw);
            }
        }
        self.cold_scene_assets
            .retain(|_, cold| !cold.is_empty());
        let instances = self
            .jigsaw_instances
            .iter()
            .enumerate()
            .filter_map(|(index, instance)| (instance.entity == entity).then_some(index))
            .collect::<HashSet<_>>();
        self.jigsaw_owners
            .retain(|_, owner| !instances.contains(&owner.instance));
        for (index, instance) in self.jigsaw_instances.iter_mut().enumerate() {
            if instances.contains(&index) {
                instance.transition_states.clear();
                instance.render_states.clear();
                instance.mesh_draws.clear();
            }
        }
        for part_index in self.entities[entity].collision_parts.clone() {
            let part = &mut self.report.collision_parts[part_index];
            self.static_collision_owner_parts.remove(&part.owner);
            part.native_resource = Arc::new(Vec::new());
            part.native_welding = Arc::new(BTreeMap::new());
        }
    }
    fn remove_entity_lighting(&mut self, entity: usize) -> Result<()> {
        let mut properties = HashSet::new();
        for binding in self
            .spherical_bindings
            .iter()
            .filter(|binding| binding.entity == entity)
        {
            if let Some(id) = self
                .spherical_properties
                .detach((entity as u64, binding.mesh))?
            {
                properties.insert(id);
            }
            self.spherical_materials.remove(&binding.draw);
        }
        self.spherical_bindings
            .retain(|binding| binding.entity != entity);
        if let Some(id) = self.child_lighting_properties.remove(&(entity as u64)) {
            properties.insert(id);
        }
        for id in properties {
            self.spherical_properties.collect_unreferenced(Some(id))?;
        }
        self.spherical_bounds.remove(&entity);
        Ok(())
    }
    /// Physics removal is independent of entity destruction: sketch death
    /// animations and their delayed IO continue after collision is withdrawn.
    fn remove_entity_physics(&mut self, entity: usize) -> Result<()> {
        ensure!(
            entity < self.entities.len(),
            "Unknown physics-removal target"
        );
        // Both imported static parts and separately admitted dynamic bodies
        // use collision-owner IDs. Resolve them before mutating the backend.
        let owners = self.entities[entity]
            .collision_parts
            .clone()
            .map(|part| self.report.collision_parts[part].owner)
            .chain(
                self.dynamic_runtime_sources
                    .iter()
                    .filter_map(|(&source, &owner)| {
                        (self.dynamic_bodies[source].entity == entity).then_some(owner)
                    }),
            )
            .chain(self.jigsaw_owners.iter().filter_map(|(&owner, source)| {
                (self.jigsaw_instances[source.instance].entity == entity).then_some(owner)
            }))
            .collect::<HashSet<_>>();
        for owner in owners {
            self.collision.retire_owner(owner)?;
        }
        if let Some(Ok(body)) = self.character_physics.remove(&entity) {
            self.collision.retire_owner(body.owner)?;
        }
        self.npc_relocations.remove(&entity);
        // Retired shapes keep stable handles, but must no longer publish
        // simulation poses back into a destroyed gameplay entity.
        self.dynamic_ground_contacts
            .retain(|source| self.dynamic_bodies[*source].entity != entity);
        self.dynamic_runtime_sources
            .retain(|source, _| self.dynamic_bodies[*source].entity != entity);
        Ok(())
    }
    pub(super) fn receive_combat(
        &mut self,
        target: usize,
        emissions: &[super::projectile_stimulus::Emission],
    ) -> Result<()> {
        if self.demo_story.enabled && !self.removed_entities.contains(&target) {
            self.enemy_weak_point_hit(target, emissions)?;
        }
        self.receive_combat_inner(target, emissions, None)
    }

    pub(super) fn receive_combat_inner(
        &mut self,
        target: usize,
        emissions: &[super::projectile_stimulus::Emission],
        weak_point_damage: Option<f32>,
    ) -> Result<()> {
        if self.removed_entities.contains(&target) {
            return Ok(());
        }
        let Some(first) = emissions.first() else {
            return Ok(());
        };
        let mut effective = Vec::with_capacity(emissions.len());
        let components = &self.entities[target].components;
        if let Some(component) = components
            .iter()
            .find(|c| c.class_name == "VulnerabilityComponent")
        {
            let number = |name: &str| {
                component
                    .properties
                    .iter()
                    .find(|p| p.name == name)
                    .and_then(|p| p.value.as_f64())
                    .unwrap_or(0.) as f32
            };
            let vulnerability = super::vulnerability::Authored::load(component)?;
            let target_position=self.entities[target].transform[12..15].try_into()?;
            for emission in emissions {
                let entry=*vulnerability.entries.get(emission.stimulus as usize).context("Unknown combat stimulus")?;
                let directional_factor=if super::vulnerability::rejected(emission.amount,entry.threshold,None) {1.} else {
                    super::vulnerability_direction::factor(
                        emission.amount,emission.position,target_position,self.entities[target].runtime_rotation,
                        number("InvulnerableMinAngle"),number("InvulnerableMaxAngle"),number("InvulnerableThreshold"),
                    )?
                };
                let amount=vulnerability.amount(emission.stimulus,emission.amount,entry,
                    super::vulnerability::ContextValues {
                        source_is_player:emission.owner==Some(self.player_entity as u64),
                        directional_factor,timer_sample:None,
                    });
                effective.push((emission.stimulus,amount));
            }
        } else {
            effective.extend(emissions.iter().map(|e| (e.stimulus, e.amount)));
        }
        if let Some(amount) = weak_point_damage {
            effective.clear();
            effective.push((8, amount));
        }
        // Dynamic rigid-body listener, REL3815b4..3816e0: first type-3
        // stimulus only; exclude the source entity and non-positive amounts.
        let mut character_impulse = None;
        if first.owner != Some(target as u64) {
            if let Some((_, amount)) = effective
                .iter()
                .find(|(kind, _)| *kind == 3)
                .filter(|(_, amount)| *amount > 0.)
            {
                let impulse = first.direction.map(|axis| axis * amount);
                if components
                    .iter()
                    .any(|c| c.class_name == "PhysicsRigidBodyCharacterComponent")
                    && (target == self.player_entity || self.demo_story.enabled)
                {
                    character_impulse = Some((first.direction, *amount));
                }
                let owners = self
                    .dynamic_runtime_sources
                    .iter()
                    .filter_map(|(&source, &owner)| {
                        (self.dynamic_bodies[source].entity == target).then_some(owner)
                    })
                    .collect::<Vec<_>>();
                for owner in owners {
                    self.collision.apply_owner_impulse(owner, impulse)?;
                }
            }
        }
        let ids = emissions.iter().map(|e| e.stimulus).collect::<Vec<_>>();
        let activator = first.owner.and_then(|owner| usize::try_from(owner).ok())
            .filter(|owner| *owner < self.entities.len());
        let mut damage_accepted = false;
        let mut lost_health = false;
        if let Some(authored) = self.authored_health.get(&target) {
            let component = components
                .iter()
                .find(|c| c.class_name == "HealthComponent")
                .context("Missing health owner")?;
            let field = |name: &str| {
                component
                    .properties
                    .iter()
                    .find(|p| p.name == name)
                    .map(|p| &p.value)
            };
            let force = field("ForceInvulnerable")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                || (target == self.player_entity && self.cutscene_input.restricted);
            let duration = field("InvulnerableDuration")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.) as f32;
            let amount = super::health::stimulus_damage(&effective, false)?;
            let runtime = self
                .runtime_health
                .entry(target)
                .or_insert_with(|| super::health::Runtime::new(authored));
            let previous_health = runtime.health;
            if first.owner != Some(target as u64)
                && runtime.damage(
                    amount,
                    authored.infinite,
                    force,
                    self.animation_time,
                    duration,
                )?
            {
                damage_accepted = true;
                lost_health = runtime.health < previous_health && runtime.health > 0;
                // Keep imported defaults immutable; consumers see the live value
                // in the entity's runtime property and the runtime health map.
                if let Some(property) = self.entities[target]
                    .components
                    .iter_mut()
                    .find(|c| c.class_name == "HealthComponent")
                    .and_then(|c| c.properties.iter_mut().find(|p| p.name == "Health"))
                {
                    property.value = serde_json::json!(runtime.health);
                }
                // Infinite-health targets can accept damage without changing
                // their health value. Their authored outputs still have to fire.
                self.outputs.emit_combat(target, "OnDamage", activator.or(Some(target)), self.animation_time, &effective);
                let actions = self
                    .component_events
                    .receive_damage(target, self.animation_time)?;
                self.execute_component_events(actions)?;
                if self.demo_story.enabled
                    && target != self.player_entity
                    && self.ai_brains.contains_key(&target)
                    && previous_health > 0
                    && self
                        .runtime_health
                        .get(&target)
                        .is_some_and(|h| h.health <= 0)
                {
                    let zero_actions = self.component_events.receive_zero_health(
                        target,
                        true,
                        self.animation_time,
                    )?;
                    self.execute_component_events(zero_actions)?;
                    let _ = self.receive_death_rewards(target, 2);
                    self.emit_entity_output(target, "OnDeath", activator.unwrap_or(self.player_entity));
                    self.emit_entity_output(target, "OnDefeated", activator.unwrap_or(self.player_entity));
                    if self.authored_havok.contains(target) {
                        let _ = self.authored_havok.send_event(target, "EVENT_Start_Death");
                    }
                    self.clear_ai_combat_target(target);
                    self.set_entity_hidden(target, true)?;
                }
            }
        }
        if self.demo_story.enabled {
            self.record_blob_vacuum_stimuli(target, &effective);
            self.record_boss_stimuli(target, &effective);
        }
        if let Some((direction, amount)) = character_impulse {
            self.demo_character_knockback(target, direction, amount)?;
        }
        self.outputs.emit_combat(target, "OnStimulus", activator, self.animation_time, &effective);
        if self.demo_story.enabled {
            self.demo_counter_stimulus(target, &effective, first.owner == Some(target as u64))?;
        }
        if target != self.player_entity || first.tag != "ThinnerOcean" || damage_accepted {
            let actions = self.component_events
                .receive_stimuli(target, &ids, &first.tag, self.animation_time)?;
            self.execute_component_events(actions)?;
        }
        // DamageLeap is authored as a response channel paired with ordinary
        // thinner damage. Mickey's health multiplier for channel 6 is zero,
        // so read the response from the incoming batch, after health admission.
        // River meshes and trigger volumes both go through this receiver.
        // Trigger whenever Mickey is still alive (including brief i-frames) so
        // he leaps out of thinner instead of walking along the river floor.
        let player_alive = lost_health
            || self
                .runtime_health
                .get(&target)
                .is_some_and(|runtime| runtime.health > 0);
        if target == self.player_entity && player_alive && self.script_player_available
            && emissions.iter().any(|e| e.stimulus == 6 && e.amount > 0.)
            && self.script_player.begin_damage_leap()
        {
            let event = self.animation_events.as_ref().ok()
                .and_then(|events| events.id("EVENT_Start_Damaged_Leap"));
            if let Some(event) = event {
                self.send_native_actor_animation_event(target, event)?;
            }
        }
        Ok(())
    }
    pub(super) fn dispatch_live_spin(&mut self, hit_set: &mut HashSet<usize>) -> Result<()> {
        // RELb7fec calls REL2969f0 for the attack center. This is the
        // SceneGraph root's world-bound center when one is available.
        let center = self.entity_position_or_bound_center(self.player_entity)?
            .context("Missing live spin source")?;
        self.dispatch_spin(center, hit_set)
    }

    pub fn dispatch_spin(&mut self, position: V, hit_set: &mut HashSet<usize>) -> Result<()> {
        use super::physics_filter::QueryFlags;
        let settings = self
            .report
            .spin_settings
            .as_ref()
            .context("Missing spin settings")?;
        let (range, knockback, spin) = (
            settings.preview_value("Range"),
            settings.preview_value("KnockbackStim"),
            settings.preview_value("SpinStim"),
        );
        let overlaps = self.collision.overlap_sphere(position, range)?;
        log::debug!(target: "blotworx_spin", "spin center {position:?}, radius {range}, raw overlaps {}",
            overlaps.len());
        let flags = QueryFlags {
            bytes: [1, 1, 1, 0, 1],
            excluded_group: 0,
        };
        let owners = self.dynamic_runtime_filter.query(Some(flags), |filter| {
            Ok(overlaps
                .into_iter()
                .filter(|hit| {
                    self.collision
                        .simulation_filter(hit.shape)
                        .is_some_and(|word| filter.collision_info(u32::MAX, word) >> 24 != 0)
                })
                .map(|hit| hit.owner)
                .collect::<Vec<_>>())
        })?;
        for owner in owners {
            let Some(target) = self.collision_entity_index(owner) else {
                continue;
            };
            if target == self.player_entity || hit_set.contains(&target) {
                continue;
            }
            let entity = &self.entities[target];
            let target_position = [
                entity.transform[12],
                entity.transform[13],
                entity.transform[14],
            ];
            let ray_origin = if self.demo_story.enabled {
                [position[0], position[1] + 0.7, position[2]]
            } else {
                position
            };
            let ray_target = if self.demo_story.enabled {
                [
                    target_position[0],
                    target_position[1] + 0.7,
                    target_position[2],
                ]
            } else {
                target_position
            };
            let translation = std::array::from_fn(|i| ray_target[i] - ray_origin[i]);
            let mut rays = self
                .collision
                .with_player_queries(|| self.collision.ray_hits(ray_origin, translation))?;
            // The retail collector uses the same fraction partition as casts.
            super::cast_contacts::sort_fractions(&mut rays, |hit| hit.fraction)?;
            let hits = self.dynamic_runtime_filter.query(Some(flags), |filter| {
                Ok(rays.iter().filter_map(|hit| {
                    let word = self.collision.simulation_filter(hit.shape)?;
                    (filter.collision_info(0x700003, word) >> 24 != 0)
                        .then_some((hit.owner, word))
                }).collect::<Vec<_>>())
            })?;
            if !self.ordered_entity_visibility(self.player_entity, target, &hits)? {
                log::debug!(target: "blotworx_spin", "blocked spin target {} from {ray_origin:?} to {ray_target:?}, first collision {:?}",
                    entity.name, hits.first().and_then(|&(owner, _)| self.collision_entity_index(owner))
                        .map(|entity| self.entities[entity].name.as_str()));
                continue;
            }
            log::debug!(target: "blotworx_spin", "admitted spin target {}", entity.name);
            super::spin::dispatch_target(
                self.player_entity as u64,
                target as u64,
                position,
                target_position,
                knockback,
                spin,
                "Spin",
                |emissions| self.receive_combat(target, &emissions),
            )?;
            if self.demo_story.enabled {
                self.demo_spin_hit(target, position)?;
            }
            hit_set.insert(target);
        }
        Ok(())
    }
    /// Dispatches Mickey's grounded Blast cone stimuli per `ConfigFiles.ini`
    /// `[MickeyAbilities_Blast]` (`AimMinAngleCosine=0.5`, `Paint_Stim_1=4.0`,
    /// `Paint_KnockbackStim_1=1.5`, `Thinner_Stim_1=4.0`, `Thinner_KnockbackStim_1=1.5`).
    pub(super) fn dispatch_blast_cone(
        &mut self,
        origin: V,
        forward: V,
        ink: super::brush::Ink,
    ) -> Result<()> {
        use super::physics_filter::QueryFlags;
        use super::projectile_stimulus::{Delivery, Emission};
        const RANGE: f32 = 5.0;
        const AIM_MIN_COS: f32 = 0.5;
        const INK_STIM: f32 = 4.0;
        const KNOCKBACK_STIM: f32 = 1.5;
        let overlaps = self.collision.overlap_sphere(origin, RANGE)?;
        let flags = QueryFlags {
            bytes: [1, 1, 1, 0, 1],
            excluded_group: 0,
        };
        let owners = self.dynamic_runtime_filter.query(Some(flags), |filter| {
            Ok(overlaps
                .into_iter()
                .filter(|hit| {
                    self.collision
                        .simulation_filter(hit.shape)
                        .is_some_and(|word| filter.collision_info(u32::MAX, word) >> 24 != 0)
                })
                .map(|hit| hit.owner)
                .collect::<Vec<_>>())
        })?;
        let mut hit_set = HashSet::new();
        let stimulus = if ink == super::brush::Ink::Paint { 1 } else { 0 };
        let tag = if ink == super::brush::Ink::Paint {
            "BlastPaint"
        } else {
            "BlastThinner"
        };
        for owner in owners {
            let Some(target) = self.collision_entity_index(owner) else {
                continue;
            };
            if target == self.player_entity || !hit_set.insert(target) {
                continue;
            }
            let entity = &self.entities[target];
            let target_pos = [
                entity.transform[12],
                entity.transform[13] + 0.7,
                entity.transform[14],
            ];
            let delta = [
                target_pos[0] - origin[0],
                target_pos[1] - origin[1],
                target_pos[2] - origin[2],
            ];
            let dist = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
            if dist > RANGE || dist < 1e-4 {
                continue;
            }
            let dir = [delta[0] / dist, delta[1] / dist, delta[2] / dist];
            let horiz_len = delta[0].hypot(delta[2]);
            let cos_angle = if horiz_len > 1e-4 {
                (delta[0] * forward[0] + delta[2] * forward[2]) / horiz_len
            } else {
                1.0
            };
            if cos_angle < AIM_MIN_COS {
                continue;
            }
            let emissions = [
                Emission {
                    stimulus: 3,
                    amount: KNOCKBACK_STIM,
                    position: origin,
                    direction: dir,
                    target: Some(target as u64),
                    owner: Some(self.player_entity as u64),
                    tag: tag.into(),
                    delivery: Delivery::Target,
                },
                Emission {
                    stimulus,
                    amount: INK_STIM,
                    position: origin,
                    direction: dir,
                    target: Some(target as u64),
                    owner: Some(self.player_entity as u64),
                    tag: tag.into(),
                    delivery: Delivery::Target,
                },
            ];
            let _ = self.receive_combat(target, &emissions);
        }
        Ok(())
    }
    fn stimulus_root(&self, entity: usize) -> Result<usize> {
        let mut current = entity;
        let mut visited = HashSet::new();
        loop {
            ensure!(visited.insert(current), "Cycle in stimulus owner hierarchy");
            let runtime = self
                .entities
                .get(current)
                .context("Missing stimulus owner")?;
            let Some(parent) = runtime.transform_parent_id()? else {
                return Ok(current);
            };
            current = self
                .entities
                .iter()
                .position(|e| e.scene == runtime.scene && e.id == parent)
                .with_context(|| {
                    format!("Unresolved stimulus parent {}:{parent}", runtime.scene)
                })?;
        }
    }
    pub fn area_stimulus_recipients(
        &mut self,
        position: V,
        radius: f32,
        owner: Option<usize>,
    ) -> Result<Vec<usize>> {
        use super::physics_filter::QueryFlags;
        let root = owner.map(|entity| self.stimulus_root(entity)).transpose()?;
        let overlaps = self.collision.overlap_sphere(position, radius)?;
        let mut recipients = Vec::new();
        let mut seen = HashSet::new();
        // REL28827c returns -0x1121, NOT the spin visibility mask0x700003.
        // REL2884f0..28851c selects the original override bytes below.
        let admitted = self.dynamic_runtime_filter.query(
            Some(QueryFlags {
                bytes: [1, 0, 1, 0, 0],
                excluded_group: 0,
            }),
            |filter| {
                overlaps
                    .into_iter()
                    .filter_map(|hit| {
                        let result = (|| {
                            let word = self
                                .collision
                                .simulation_filter(hit.shape)
                                .context("Area query shape lacks native filter")?;
                            Ok((filter.collision_info(0xffffeedf, word) >> 24 != 0)
                                .then_some(hit.owner))
                        })();
                        match result {
                            Ok(None) => None,
                            Ok(Some(owner)) => Some(Ok(owner)),
                            Err(error) => Some(Err(error)),
                        }
                    })
                    .collect::<Result<Vec<_>>>()
            },
        )?;
        for collision_owner in admitted {
            let Some(entity) = self.collision_entity(collision_owner) else {
                continue;
            };
            let target = self
                .entities
                .iter()
                .position(|e| std::ptr::eq(e, entity))
                .context("Area recipient lost identity")?;
            if Some(target) == owner || root == Some(self.stimulus_root(target)?) {
                continue;
            }
            if seen.insert(target) {
                recipients.push(target);
            }
        }
        Ok(recipients)
    }
    pub(super) fn execute_component_events(
        &mut self,
        actions: Vec<super::component_events::Action>,
    ) -> Result<()> {
        for action in actions {
            // REL31ed00 executes audio, effect, script, action in this order.
            // Unimplemented sinks remain explicit, rather than success no-ops.
            if !action.audio.is_empty() {
                self.post_audio_from_game(action.entity, &action.audio);
            }
            if !action.effect.is_empty() {
                if let Err(error) = self.start_one_shot_effect(action.entity, &action.effect) {
                    let message = format!(
                        "GameEvents effect {} -> {}: {error:#}",
                        self.entities[action.entity].name, action.effect,
                    );
                    if !self.report.unsupported.contains(&message) {
                        self.report.unsupported.push(message);
                    }
                }
            }
            if !action.script.is_empty() {
                self.dispatch_script_callback(&action.script, action.entity)?;
            }
            if let Some(id) = action.action {
                let message = format!(
                    "GameEvents action receiver pending: {} -> {id}",
                    self.entities[action.entity].name
                );
                if !self.report.unsupported.contains(&message) {
                    self.report.unsupported.push(message);
                }
            }
        }
        Ok(())
    }
    /// One committed contact must publish both gameplay stimuli and the
    /// paintable body's material/collision state, in the same order as play.
    pub fn dispatch_brush_contact(&mut self, contact: &super::brush_preview::Contact,
        player_position: [f32;3]) -> Result<bool> {
        self.dispatch_projectile_game_events(contact)?;
        self.brush_jigsaw_hit(&contact.hit, contact.ink==super::brush::Ink::Paint, player_position)
    }
    pub fn dispatch_projectile_game_events(
        &mut self,
        contact: &super::brush_preview::Contact,
    ) -> Result<()> {
        use super::projectile_stimulus::{Contact, Delivery, Emitter};
        let Some(target) = self.collision_entity_index(contact.hit.owner) else {
            return Ok(());
        };
        let entity = &self.entities[target];
        let body = entity.projectile_target_body();
        let settings = self
            .report
            .brush_settings
            .as_ref()
            .context("Missing brush settings")?;
        let name =
            &settings.projectile_templates[usize::from(contact.ink == super::brush::Ink::Thinner)];
        let projectile = self
            .projectile_preview
            .pool
            .entries
            .get(contact.projectile as usize)
            .context("Missing contacted projectile")?;
        let mut emitter = Emitter {
            stimuli: self
                .brush_stimuli
                .get(name)
                .context("Missing projectile stimuli")?
                .clone(),
            amount_scale: settings.value(
                contact.ink,
                "StimulusScale",
                0,
                super::ability_value::NEUTRAL_PLAYSTYLE,
            )?,
            projectile: contact.projectile,
            owner: Some(projectile.owner),
            tag: "Brush".into(),
        };
        if self.demo_story.enabled && std::env::var_os("BLOTWORX_TRACE_BRUSH").is_some() {
            let vulnerability = entity
                .components
                .iter()
                .find(|c| c.class_name == "VulnerabilityComponent")
                .map(|c| {
                    c.properties
                        .iter()
                        .filter(|p| {
                            matches!(
                                p.name.as_str(),
                                "Paint Vulnerability" | "Thinner Vulnerability"
                            )
                        })
                        .map(|p| (&p.name, &p.value))
                        .collect::<Vec<_>>()
                });
            eprintln!(
                "[demo brush] {} at {:?}; counter={}; vulnerability={:?}; stimuli={:?}",
                entity.name,
                contact.hit.position,
                self.paint_counters.contains_key(&target),
                vulnerability,
                emitter.stimuli
            );
        }
        // Ability constructor a650c..a6544 sets +18 to Brush; aa674..aa678
        // copies it into ProjectileComponent+50 via392250.
        let event = Contact::from_body(
            contact.hit.position,
            projectile.launch_velocity,
            Some(target as u64),
            body,
        );
        emitter.dispatch(event, false, |_, emission| {
            // REL288478..2884cc routes a zero-radius area request directly.
            if emission.delivery == Delivery::Target
                || matches!(emission.delivery, Delivery::Area { radius: 0., .. })
            {
                self.receive_combat(target, std::slice::from_ref(&emission))?;
            } else if let Delivery::Area { radius, .. } = emission.delivery {
                let recipients = self.area_stimulus_recipients(
                    emission.position,
                    radius,
                    emission.owner.map(usize::try_from).transpose()?,
                )?;
                for recipient in recipients {
                    self.receive_combat(recipient, std::slice::from_ref(&emission))?;
                }
            }
            Ok(())
        })
    }
    /// Owner lookup for the isolated dynamic query namespace. Never interpret
    /// its IDs as indices into imported static collision_parts.
    pub fn dynamic_query_body(
        &self,
        owner: usize,
    ) -> Option<(
        &RuntimeEntity,
        &super::dynamic_body::PlacedBody,
        &super::dynamic_body::BodyLayout,
    )> {
        let &(source, index) = self.dynamic_query_owners.get(owner)?;
        let imported = self.dynamic_bodies.get(source)?;
        if self.removed_entities.contains(&imported.entity) {
            return None;
        }
        let state = self.dynamic_constructions.get(&source)?;
        Some((
            self.entities.get(imported.entity)?,
            state.bodies.get(index)?,
            state.layouts.get(index)?,
        ))
    }
    /// REL3827e4..382f78 body loop. Caller owns the partial construction and
    /// shared native RNG; this does not imply world admission or simulation.
    pub fn initialize_dynamic_system_records(
        &mut self,
        source: usize,
        state: &mut super::dynamic_body::Construction,
        owner_token: Option<usize>,
        mut random: impl FnMut() -> u16,
    ) -> Result<()> {
        ensure!(
            !state.started,
            "Construction already started; partial state must not be blindly retried"
        );
        state.started = true;
        let imported = self
            .dynamic_bodies
            .get(source)
            .context("Missing construction resource")?;
        let parts = imported
            .geometry
            .as_ref()
            .map_err(|error| anyhow::anyhow!(error.clone()))?
            .clone();
        let parameters = imported
            .initial_velocity
            .as_ref()
            .map_err(|error| anyhow::anyhow!(error.clone()))?
            .clone();
        let entity = imported.entity;
        let component_id = imported.component;
        for index in 0..state.bodies.len() {
            // Native virtual37c is repeated once per body and updates the
            // entire system. There are no native world refreshes before add.
            let component = self.entities[entity]
                .components
                .iter()
                .find(|c| c.link_id.to_u32() == component_id)
                .context("Construction component disappeared")?;
            let value = |name: &str| {
                component
                    .properties
                    .iter()
                    .find(|p| p.name == name)
                    .map(|p| &p.value)
                    .with_context(|| format!("Missing construction {name}"))
            };
            let layer = i32::try_from(
                value("Collision Layer")?
                    .as_i64()
                    .context("Invalid Collision Layer")?,
            )? as u32;
            let real = u8::from(value("Is Real")?.as_bool().context("Invalid Is Real")?);
            let painted = u8::from(
                value("Is Painted")?
                    .as_bool()
                    .context("Invalid Is Painted")?,
            );
            state
                .filters
                .fill(super::body_filter::pack(super::body_filter::Fields {
                    layer,
                    group: 0,
                    sub_a: 0,
                    sub_b: 0,
                    real,
                    painted,
                }));
            super::dynamic_body::initialize_body_velocity(
                &mut state.bodies,
                index,
                &parameters,
                &mut random,
                |_, _| Ok(()),
            )?;
            let frame = state.bodies[index].state().transform;
            self.initialize_dynamic_body_properties(
                source,
                state.layouts[index].parts[0],
                &mut state.bodies[index].motion.current,
                &mut state.materials[index],
                &mut state.bounds,
                &mut state.mover_material_seen,
            )?;
            state.bodies[index].welding = state.layouts[index].prepare_welding(&parts)?;
            self.initialize_dynamic_body_userdata(
                source,
                &mut state.bodies[index],
                owner_token,
                frame,
            )?;
        }
        state.completed = true;
        Ok(())
    }
    /// Ordered dynamic userdata initialization against the resolved owner
    /// position (descriptor1119c). Owner token comes from entity allocation;
    /// retained transform comes from the body before entity placement.
    pub fn initialize_dynamic_body_userdata(
        &self,
        source: usize,
        body: &mut super::dynamic_body::PlacedBody,
        owner_token: Option<usize>,
        preplacement: super::native_hk_transform::Transform,
    ) -> Result<()> {
        let imported = self
            .dynamic_bodies
            .get(source)
            .context("Unknown userdata component")?;
        let entity = self
            .entities
            .get(imported.entity)
            .context("Missing userdata entity")?;
        body.initialize_dynamic_user_data(owner_token)?;
        body.bind_dynamic_user_data(
            [
                entity.transform[12],
                entity.transform[13],
                entity.transform[14],
            ],
            preplacement,
        )
    }
    /// Native construction-phase activation against this component's actual
    /// mutable scene state. Caller supplies its owned bodies in system order.
    /// This must precede native world admission; query-shape presence is not
    /// native admission/island membership.
    pub fn activate_dynamic_before_admission(
        &mut self,
        source: usize,
        bodies: &mut [super::dynamic_body::PlacedBody],
        requested_dynamic: bool,
        has_parent: bool,
        update_quality: bool,
        gravity_factor_word: u32,
    ) -> Result<()> {
        let imported = self
            .dynamic_bodies
            .get(source)
            .context("Unknown activation component")?;
        let selected = *imported
            .selected_system
            .as_ref()
            .map_err(|e| anyhow::anyhow!(e.clone()))?;
        let Some(selected) = selected else {
            ensure!(
                bodies.is_empty(),
                "Bodies supplied for a missing native physics system"
            );
            return Ok(());
        };
        let systems = imported
            .systems
            .as_ref()
            .map_err(|e| anyhow::anyhow!(e.clone()))?;
        let system = systems
            .iter()
            .find(|system| system.offset == selected)
            .context("Selected activation system missing")?;
        ensure!(
            system.system.bodies.iter().all(Option::is_some),
            "Sparse body array requires a sparse construction lifecycle"
        );
        ensure!(
            bodies.len() == system.system.bodies.len(),
            "Activation physics-system body count mismatch"
        );
        let component = self.entities[imported.entity]
            .components
            .iter_mut()
            .find(|component| component.link_id.to_u32() == imported.component)
            .context("Activation component identity changed")?;
        let mut system = super::body_activation::PreAdmission {
            bodies,
            component,
            gravity_factor_word,
        };
        super::body_activation::apply(&mut system, requested_dynamic, has_parent, update_quality)
    }
    /// Execute the native property stage on an already velocity-initialized
    /// body's owned state. Admission remains the lifecycle caller's job.
    pub fn initialize_dynamic_body_properties(
        &mut self,
        imported: usize,
        part: usize,
        motion: &mut super::native_motion_record::MotionRecord,
        material: &mut super::body_material::Coefficients,
        bounds: &mut super::shape_bounds::ShapeGraph,
        mover_material_seen: &mut bool,
    ) -> Result<()> {
        let source = self
            .dynamic_bodies
            .get(imported)
            .context("Unknown dynamic component")?;
        let geometry = source
            .geometry
            .as_ref()
            .map_err(|e| anyhow::anyhow!(e.clone()))?;
        let shape = geometry.get(part).context("Unknown dynamic body part")?;
        let root = shape.bounds_root;
        let owner = &mut self.entities[source.entity];
        let transform = super::native_hk_transform::from_entity(
            owner.runtime_rotation,
            [
                owner.transform[12],
                owner.transform[13],
                owner.transform[14],
            ],
        );
        let component = owner
            .components
            .iter_mut()
            .find(|c| c.link_id.to_u32() == source.component)
            .context("Dynamic component identity changed")?;
        let mut context = super::body_scene_context::Scene {
            component,
            settings: &self.runtime_settings,
            transform,
            mover_material_seen: *mover_material_seen,
        };
        let result =
            super::body_initialization::apply(motion, material, bounds, root, &mut context);
        *mover_material_seen = context.mover_material_seen;
        result
    }
    /// Publish the committed controller pose before this tick's script/model
    /// phase. Presentation interpolation must never become gameplay state.
    pub fn advance_player_models(&mut self, player: &mut physics::Player, time: f32) -> Result<()> {
        self.entities
            .get_mut(self.player_entity)
            .context("Player entity disappeared")?
            .set_controller_pose(player.position, player.yaw);
        self.update_mickey_tracker(player);
        self.update_gameplay_camera_targets(player);
        // The committed driver advances exactly one STEP here. Subtracting
        // rounded tick*STEP timestamps changes that delta and can land native
        // countdowns on exact zero (which intentionally disables their update).
        self.with_live_player(player, |world| {
            world.advance_models_with_delta(time, super::STEP)
        })?;
        // This entry point runs once per committed controller tick. Rendering
        // never advances actor clocks. IO events queued above reach this tick.
        self.with_live_player(player,|world| {
            let spawned=world.havok_actors.keys().copied()
                .filter(|&entity| !world.removed_entities.contains(&entity) && world.entity_scene_active(entity))
                .collect::<Vec<_>>();
            for entity in spawned {
                let (events,unsupported)={
                    let (template,actor)=world.havok_actors.get_mut(&entity).context("Spawned actor disappeared before update")?;
                    let animation=world.sketch_templates.templates.get_mut(template).context("Live actor template disappeared")?
                        .animation.as_mut().map_err(|error|anyhow::anyhow!(error.clone()))?.as_mut().context("Live actor has no animation")?;
                    animation.advance(actor,1)?;
                    let catalog=world.animation_events.as_ref().map_err(|error|anyhow::anyhow!(error.clone()))?;
                    let mut events=Vec::new();let mut unsupported=Vec::new();
                    for frame in std::mem::take(&mut actor.playback.emitted_frames) {
                        events.extend(super::animation_event_queue::normalize(frame,|local| {
                            if local<0 {return Ok(local);}
                            let name=animation.contract.events.get(local as usize).context("Spawned actor event exceeds its graph table")?;
                            if let Some(native)=catalog.id(name) {return Ok(native as i32);}
                            // DOL8022566c filters negative mapped IDs. These
                            // remain graph-local events, not failed native calls.
                            Ok(-1)
                        })?);
                    }
                    (events,unsupported)
                };
                world.deliver_actor_exports(entity,events,unsupported);
            }
            for entity in world.authored_havok.active_entities(|entity| {
                !world.removed_entities.contains(&entity) && world.entity_scene_active(entity)
            }) {
                match world.authored_havok.step_and_export(entity) {
                    Ok((events,unsupported))=>world.deliver_actor_exports(entity,events,unsupported),
                    Err(error)=>{
                        world.report.unsupported.push(format!("Authored Havok actor {entity} update/export: {error:#}"));
                        world.authored_havok.remove(entity);
                        let _ = world.prune_unused_skin_payloads();
                    },
                }
            }
            world.update_skeletal_children(None)?;
            world.launch_ai_projectiles();
            Ok(())
        })
    }
    pub(super) fn deliver_actor_exports(
        &mut self,
        entity: usize,
        events: Vec<super::state_machine::Event>,
        unsupported: Vec<String>,
    ) {
        for message in unsupported {
            if !self.report.unsupported.contains(&message) {
                self.report.unsupported.push(message);
            }
        }
        for event in events {
            self.controller_feedback.exported_events += 1;
            let result = self.dispatch_exported_animation_event(entity, &event);
            if let Err(error) = result {
                let message = format!(
                    "Actor {} animation export {}: {error:#}",
                    self.entities[entity].name, event.id
                );
                if !self.report.unsupported.contains(&message) {
                    self.report.unsupported.push(message);
                }
            }
        }
    }
    pub(super) fn dispatch_exported_animation_event(
        &mut self,
        entity: usize,
        event: &super::state_machine::Event,
    ) -> Result<()> {
        if event.id == 0x212 {
            let ignore = super::entity_status::read_ignore_audio(
                &self
                    .entities
                    .get(entity)
                    .context("Animation audio owner missing")?
                    .components,
            )?
            .context("Animation audio component missing")?;
            if ignore {
                return Ok(());
            }
        }
        if matches!(event.id, 0x212 | 0x21b | 0x214 | 0x215 | 0x217) {
            if event.payload.is_none() {
                return Ok(());
            }
            let graph = if entity == self.player_entity {
                &self.player_behavior
            } else if self.authored_havok.contains(entity) {
                self.authored_havok.graph(entity)?
            } else {
                let (template, _) = self
                    .havok_actors
                    .get(&entity)
                    .context("Animation payload target has no graph")?;
                &self
                    .sketch_templates
                    .templates
                    .get(template)
                    .context("Animation payload template missing")?
                    .animation
                    .as_ref()
                    .map_err(|error| anyhow::anyhow!(error.clone()))?
                    .as_ref()
                    .context("Animation payload template has no graph")?
                    .graph
            };
            let source = if event.id == 0x212 {
                super::controller_feedback::audio_source(graph, event.payload.as_ref())?
            } else {
                super::controller_feedback::lua_source(graph, event.payload.as_ref())?
            }
            .map(str::to_owned);
            if let Some(source) = source {
                if event.id == 0x212 {
                    self.post_audio_from_game(entity, &source);
                } else if event.id == 0x21b {
                    self.scripts.schedule_animation_source(&source, entity)?;
                } else {
                    self.dispatch_animation_emitter(entity, event.id as u32, &source)?;
                }
            }
            return Ok(());
        }
        if self.demo_story.enabled {
            self.enemy_armor_animation(entity, event.id as u32)?;
        }
        self.advance_demo_attack_animation(entity, event.id as u32)?;
        // The native ordinary kind9 fallback ignores payloads. Specialized
        // IDs still fail explicitly until their concrete controller is wired.
        self.dispatch_native_animation_feedback(entity, event.id as u32, 1)
    }
    pub(super) fn post_audio_from_game(&mut self, entity: usize, event: &str) -> u32 {
        if let Err(error)=self.prepare_audio_position(entity) {
            let message=format!("Audio source pose: {error:#}");if !self.report.unsupported.contains(&message){self.report.unsupported.push(message);}
        }
        let id = self.audio_playback.post_from_game(event, entity);
        if id == 0
            && self
                .audio_playback
                .rejected_posts
                .contains_key(&(entity, event.to_owned()))
        {
            let message=format!("Audio preview rejected: {} -> {event}. Posting returns 0; full action/mixer execution remains incomplete.",self.entities[entity].name);
            if !self.report.unsupported.contains(&message) {
                self.report.unsupported.push(message);
            }
        }
        id
    }
    pub(super) fn audio_set_switch_lua(
        &mut self,
        lua: &mlua::Lua,
        args: mlua::MultiValue,
    ) -> mlua::Result<mlua::MultiValue> {
        if args.len() != 3 {
            return Err(mlua::Error::external(
                "AudioSetSwitch requires entity, group and value",
            ));
        }
        let group = lua
            .coerce_string(args[1].clone())?
            .ok_or_else(|| mlua::Error::external("Audio switch group must be text"))?;
        let value = lua
            .coerce_string(args[2].clone())?
            .ok_or_else(|| mlua::Error::external("Audio switch value must be text"))?;
        let Some(entity) = self.scripts.resolve_world_argument(&args[0])? else {
            return Ok(mlua::MultiValue::new());
        };
        if self.removed_entities.contains(&entity) {
            return Ok(mlua::MultiValue::new());
        }
        self.audio_playback
            .set_switch_from_game(
                entity,
                group.to_str()?.split('\0').next().unwrap_or(""),
                value.to_str()?.split('\0').next().unwrap_or(""),
            )
            .map_err(mlua::Error::external)?;
        let limitation="Entity audio switch values are queued on registered objects; continuous switch voice notifications and the full mixer remain incomplete.";
        if !self
            .report
            .unsupported
            .iter()
            .any(|line| line == limitation)
        {
            self.report.unsupported.push(limitation.into());
        }
        Ok(mlua::MultiValue::new())
    }
    pub(super) fn export_player_animation_frames(
        &mut self,
        frames: Vec<Vec<super::state_machine::Event>>,
    ) -> Result<()> {
        let catalog = self
            .animation_events
            .as_ref()
            .map_err(|error| anyhow::anyhow!(error.clone()))?;
        let names = self.player_behavior.single("hkbBehaviorGraphStringData")?["eventNames"]
            .as_array()
            .context("Missing player animation event names")?;
        let mut output = Vec::new();
        let mut unsupported = Vec::new();
        for frame in frames {
            output.extend(super::animation_event_queue::normalize(frame,|local| {
                if local<0 {return Ok(local);}
                let name=names.get(local as usize).and_then(|name|name.as_str()).context("Player event exceeds graph names")?;
                if let Some(native)=catalog.id(name) {return Ok(native as i32);}
                Ok(-1) // Native export filters graph-only event IDs.
            })?);
        }
        self.deliver_actor_exports(self.player_entity, output, unsupported);
        Ok(())
    }
    pub fn activate_sketch_actor(&mut self, entity: usize, template: &str) -> Result<()> {
        ensure!(
            entity < self.entities.len() && !self.removed_entities.contains(&entity),
            "Invalid actor entity"
        );
        ensure!(
            !self.havok_actors.contains_key(&entity),
            "Havok actor already active"
        );
        let key = template.to_ascii_lowercase();
        let definition = self
            .sketch_templates
            .templates
            .get_mut(&key)
            .context("Missing actor template")?;
        let expected = &definition.graph.definitions[&definition.graph.root].model;
        ensure!(
            expected.as_ref().is_some_and(|path| self.entities[entity]
                .model_asset
                .as_ref()
                .is_some_and(|asset| asset.eq_ignore_ascii_case(path)
                    || asset.eq_ignore_ascii_case(&format!("{path}_wii")))),
            "Actor model differs from template"
        );
        let animation = definition
            .animation
            .as_mut()
            .map_err(|error| anyhow::anyhow!(error.clone()))?
            .as_mut()
            .context("Template has no Havok animation")?;
        let actor = animation.instantiate()?;
        self.havok_actors.insert(entity, (key, actor));
        Ok(())
    }
    pub(super) fn send_havok_actor_event(&mut self, entity: usize, name: &str) -> Result<()> {
        if self.authored_havok.contains(entity) {
            return self.authored_havok.send_event(entity, name);
        }
        let (template, actor) = self
            .havok_actors
            .get_mut(&entity)
            .context("Animation event target has no active Havok actor")?;
        let animation = self
            .sketch_templates
            .templates
            .get_mut(template)
            .context("Live actor template disappeared")?
            .animation
            .as_mut()
            .map_err(|error| anyhow::anyhow!(error.clone()))?
            .as_mut()
            .context("Live actor has no animation")?;
        animation.send_event(actor, name)
    }
    /// Native controller IDs require the common name table and graph-local
    /// reverse mapping. A known event absent from a graph is filtered normally.
    pub(super) fn send_native_actor_animation_event(
        &mut self,
        entity: usize,
        id: u32,
    ) -> Result<bool> {
        ensure!(
            !self.removed_entities.contains(&entity),
            "Animation event target was retired"
        );
        let owner = self
            .entities
            .get(entity)
            .context("Animation event target is not loaded")?;
        let catalog = self
            .animation_events
            .as_ref()
            .map_err(|error| anyhow::anyhow!(error.clone()))?;
        let name = catalog.name(id)?;
        if entity == self.player_entity {
            let names = self.player_behavior.single("hkbBehaviorGraphStringData")?["eventNames"]
                .as_array()
                .context("Player graph has no event names")?;
            let local = names.iter().position(|value| value.as_str() == Some(name));
            if let Some(local) = local {
                let event = super::state_machine::Event {
                    id: i32::try_from(local)?,
                    payload: None,
                };
                let emitted = super::havok_playback::Playback::dispatch_event(
                    &self.player_behavior,
                    &mut self.player_graph,
                    &mut self.player_animations,
                    &event,
                )?;
                self.player_script_animation_events.push((event, emitted));
            }
            self.dispatch_native_animation_feedback(entity, id, 0)?;
            return Ok(local.is_some());
        }
        if self.authored_havok.contains(entity) {
            let handled = self.authored_havok.send_native_event(entity, id)?;
            self.dispatch_native_animation_feedback(entity, id, 0)?;
            return Ok(handled);
        }
        if let Some((template, actor)) = self.havok_actors.get_mut(&entity) {
            let animation = self
                .sketch_templates
                .templates
                .get_mut(template)
                .context("Live actor template disappeared")?
                .animation
                .as_mut()
                .map_err(|error| anyhow::anyhow!(error.clone()))?
                .as_mut()
                .context("Live actor has no animation")?;
            if let Some(local) = animation
                .contract
                .events
                .iter()
                .position(|event| event == name)
            {
                actor.playback.deliver_event(
                    &animation.graph,
                    &mut actor.prepared,
                    &mut animation.resources,
                    super::state_machine::Event {
                        id: i32::try_from(local)?,
                        payload: None,
                    },
                )?;
                self.dispatch_native_animation_feedback(entity, id, 0)?;
                return Ok(true);
            }
            self.dispatch_native_animation_feedback(entity, id, 0)?;
            return Ok(false);
        }
        ensure!(
            !owner
                .components
                .iter()
                .any(|c| c.class_name == "AnimationComponent"),
            "Native controller animation graph is unavailable for {}",
            owner.name
        );
        Ok(false)
    }
    fn dispatch_native_animation_feedback(
        &mut self,
        entity: usize,
        id: u32,
        flag: u8,
    ) -> Result<()> {
        if id == 0x218 {
            //2f0a84: stop in insertion order, then clear the retained names.
            // Audio_Push remains explicitly unsupported; no audio handles can
            // have entered this controller's native second stack yet.
            let names = self
                .controller_feedback
                .pushed_emitters
                .get(&entity)
                .cloned()
                .unwrap_or_default();
            for name in &names {
                self.stop_named_effect(entity, name)?;
            }
            self.controller_feedback.pushed_emitters.remove(&entity);
            // Unlike Start/Stop, Pop continues through ordinary kind9 feedback,
            // preserving both exported flag1 and incoming-command flag0.
        }
        //This entry has no payload.2ee294 declines a null emitter string;
        //specialized start/stop must not leak into ordinary kind9 feedback.
        if matches!(id, 0x212 | 0x214 | 0x215 | 0x217) {
            return Ok(());
        }
        if id != 0x218 {
            super::controller_feedback::require_ordinary(id)?;
        }
        self.dispatch_camera_animation_feedback(super::entity_events::Animation {
            source: Some(entity),
            header: None,
            id,
            flag,
        })?;
        // A constructed kind9 brain listener has already run through the same
        // ordinary dispatcher as the camera. Retain the earlier partial
        // component-feedback adapter only for brains not registered there yet.
        if !self.cutscene_actions.animation_task_bound(entity,&self.entity_event_registry) {
            if let Some(brain) = self.ai_brains.get_mut(&entity) {
                brain.receive_known(
                    entity,
                    super::ai_behavior_events::Event {
                        kind: 9,
                        source: Some(entity),
                        action: id,
                    },
                );
            }
        }
        self.receive_robot_arm_animation(entity, id)?;
        if entity == self.player_entity {
            if let Some(runtime) = &mut self.player_knockback {
                runtime.state.animation(id);
            }
        }
        let variant = self.controller_feedback.variant;
        super::controller_feedback::dispatch(id, variant, |effect| {
            match effect {
                super::controller_feedback::Effect::FootstepCategory(_) => {
                    if entity == self.player_entity && self.demo_story.enabled {
                        let player = &self.entities[entity].transform;
                        self.demo_story.ai.player_footstep = Some((
                            [player[12], player[13], player[14]], self.animation_time,
                        ));
                    }
                    let actions = self.component_events.receive_footstep(
                        entity,
                        variant,
                        self.animation_time,
                    )?;
                    self.execute_component_events(actions)?;
                }
                super::controller_feedback::Effect::Noise => {
                    if self.scripts.is_execution_detached() {
                        self.scripts.queue_native_callout(
                            "#bwx_animation_noise",
                            mlua::MultiValue::from_vec(vec![mlua::Value::Integer(mlua::Integer::try_from(
                                entity,
                            )?)]),
                        )?;
                    } else {
                        self.request_animation_noise(entity);
                    }
                }
            }
            Ok(())
        })
    }
    fn request_animation_noise(&mut self, entity: usize) {
        self.controller_feedback.noise_requests += 1;
        self.controller_feedback.last_noise_source = Some(entity);
        let message="Native animation noise event0x6d requested; perception/event-manager receiver delivery remains pending".to_owned();
        if !self.report.unsupported.contains(&message) {
            self.report.unsupported.push(message);
        }
    }
    /// Native shape AABB at the retained phantom pose. The caller must supply
    /// the actual world/query tolerance; zero is not an inferred world default.
    /// This query does not admit the phantom or generate overlap events.
    pub fn resource_phantom_aabb(&self, owner: usize, tolerance: f32) -> Result<[[f32; 4]; 2]> {
        let shapes = self
            .resource_phantom_shapes
            .get(&owner)
            .context("Owner has no imported resource phantom")?;
        let root = shapes
            .first()
            .context("Resource phantom has no shape root")?
            .bounds_root;
        let motion = self
            .resource_phantom_motion
            .get(&owner)
            .context("Resource phantom has no constructor motion state")?;
        let cache = self
            .resource_phantom_bounds
            .get(&owner)
            .context("Resource phantom has no native bounds cache")?;
        let mut graph = cache
            .lock()
            .map_err(|_| anyhow::anyhow!("Resource phantom bounds cache poisoned"))?;
        // Attached pickups can be moved by an animated socket after their
        // phantom was constructed. Use the current attachment pose for the
        // overlap query. Detached owners keep their native motion record.
        let pose = if self.transform_hierarchy.contains_key(&owner) {
            self.entities[owner].physics_transform()
        } else {
            motion.transform
        };
        graph.aabb(root, &pose, tolerance)
    }
    /// Bounds prepared for native admission, not proof that admission occurred.
    /// Shape phantoms use calcAabb802beeb8's half-world tolerance; AABB phantoms
    /// return their owned stored bounds without another tolerance expansion.
    pub fn phantom_admission_bounds(&self, owner: usize) -> Result<super::phantom_bounds::Bounds> {
        let pair = if let Some(handle) = self.size_phantom_handles.get(&owner) {
            let record = self
                .physics_objects
                .phantoms
                .get(handle)
                .context("Missing box phantom record")?;
            let raw = record.bounds.context("Box phantom has no stored AABB")?;
            [
                [
                    f32::from_bits(raw[0]),
                    f32::from_bits(raw[1]),
                    f32::from_bits(raw[2]),
                    f32::from_bits(raw[3]),
                ],
                [
                    f32::from_bits(raw[4]),
                    f32::from_bits(raw[5]),
                    f32::from_bits(raw[6]),
                    f32::from_bits(raw[7]),
                ],
            ]
        } else {
            ensure!(
                self.resource_phantom_handles.contains_key(&owner),
                "Owner has no allocated phantom"
            );
            self.resource_phantom_aabb(owner, self.report.physics_world.shape_phantom_tolerance())?
        };
        ensure!(
            (0..3).all(|axis| pair[0][axis].is_finite()
                && pair[1][axis].is_finite()
                && pair[0][axis] <= pair[1][axis]),
            "Invalid phantom admission AABB"
        );
        Ok(super::phantom_bounds::Bounds {
            min: pair[0][..3].try_into().unwrap(),
            max: pair[1][..3].try_into().unwrap(),
        })
    }
    pub fn phantom_admission_sweep_bounds(
        &self,
        owner: usize,
    ) -> Result<super::physics_world::SweepAabb> {
        let bounds = self.phantom_admission_bounds(owner)?;
        self.report
            .physics_world
            .sweep_from_aabb(bounds.min, bounds.max)
    }
    fn initialize_pickup_listeners_range(&mut self, range: std::ops::Range<usize>) -> Result<()> {
        let owners = self
            .pickup_update_states
            .keys()
            .copied()
            .filter(|owner| range.contains(owner))
            .collect::<Vec<_>>();
        for owner in owners {
            let Some(&phantom) = self
                .size_phantom_handles
                .get(&owner)
                .or_else(|| self.resource_phantom_handles.get(&owner))
            else {
                self.report.unsupported.push(format!(
                    "Pickup {} requires its missing phantom/component initialization",
                    self.entities[owner].name
                ));
                continue;
            };
            let component = owner + 1; // opaque runtime lease token, separate map
            self.pickup_physics_leases.entry(component).or_insert(
                super::pickup_world_init::Component {
                    references: 1,
                    phantom,
                },
            );
            let position = super::native_hk_transform::entity_position_vector(
                self.entities[owner].transform[12..15].try_into().unwrap(),
            );
            let mut bindings = super::pickup_world_init::Bindings {
                component,
                components: &mut self.pickup_physics_leases,
                monitor: &self.pickup_reference_monitor,
                held: None,
                position,
            };
            let state = self.pickup_lifecycles.entry(owner).or_default();
            let update = self
                .pickup_update_states
                .get_mut(&owner)
                .context("Pickup update owner disappeared")?;
            super::pickup_lifecycle::initialize(
                state,
                &mut update.bob,
                &mut super::pickup_lifecycle::SharedInitialize {
                    objects: &mut self.physics_objects,
                    bindings: &mut bindings,
                },
            )?;
            self.report.geometry.push(serde_json::json!({"kind":"pickup_listener_initialized",
                "entity":self.entities[owner].name,"phantom":state.phantom,"listener":state.listener,
                "physics_world_admitted":false,"collection_scheduled":false}));
        }
        Ok(())
    }
    fn script_world_api(
        &mut self,
        lua: &mlua::Lua,
        name: &str,
        args: mlua::MultiValue,
    ) -> mlua::Result<mlua::MultiValue> {
        match name {
            "SetOverrideTurnRate" | "ResetOverrideTurnRate" => self.motion_turn_override_lua(lua,name,args),
            // Authored OnSpawned callbacks run before the optional demo story
            // adapter is enabled. Particle commands belong to the live World.
            "StartEmitters" | "StopEmitters" => self.demo_scene_lua(lua,name,args),
            "TransitionToLevel" | "CleanLoadLevel" => {
                if args.len() != 1 {
                    return Err(mlua::Error::external(format!("{name} requires one destination")));
                }
                if let Some(path) = lua.coerce_string(args[0].clone())? {
                    let path = path.to_str()?;
                    if name == "CleanLoadLevel" {
                        self.level_transition.request_clean(&path)
                    } else {
                        self.level_transition.request(&path)
                    }.map_err(mlua::Error::external)?;
                }
                Ok(mlua::MultiValue::new())
            }
            "AudioSetSwitch" => self.audio_set_switch_lua(lua, args),
            "SetCombatIntensityBias" | "CombatIntensity_SetBias" => {
                let value = args.back().cloned().unwrap_or(mlua::Value::Nil);
                if let Some(bias) = lua.coerce_number(value)? {
                    self.set_music_bias(bias as f32);
                }
                Ok(mlua::MultiValue::new())
            }
            "GetCombatIntensityBias" | "CombatIntensity_GetBias" => Ok(mlua::MultiValue::from_vec(
                vec![mlua::Value::Number(f64::from(self.music_bias()))])),
            "RequestRobotArmChangeIdle" => self.robot_arm_idle_lua(lua, args),
            "SetRobotArmAttackTypeAngleInfo"
            | "SetRobotArmAttackTypeCooldownInfo"
            | "SetRobotArmAttackTypeDelayInfo"
            | "SetRobotArmAttackTypeEnabled"
            | "SetRobotArmSpotlightEntity"
            | "SetRobotArmSpotlightDisabled"
            | "SetRobotArmSpotlightStatus"
            | "RequestRobotArmSmash"
            | "RequestRobotArmElectrocute" => self.robot_arm_control_lua(lua, name, args),
            "BwxQueueMovie" => self.movies.lua(lua, args),
            "StreamPart"
            | "StreamInteriorZone"
            | "StreamInterior"
            | "StreamZones"
            | "StreamInLevel"
            | "DeactivateScene"
            | "BwxIsLevelLoadPending" => self.demo_scene_lua(lua, name, args),
            "AudioSetState"
            | "NativeAudioSetState"
            | "NativeAudioSetSwitch"
            | "CounterAdd"
            | "CounterSubtract"
            | "SetAppCulled"
            | "ClearAppCulled"
            | "ForceThinned"
                if self.demo_story.enabled =>
            {
                self.demo_scene_lua(lua, name, args)
            }
            "SetNextPatrolNode" | "ClearNextPatrolNode" | "ResumePatrol" =>
                self.patrol_command_lua(lua,name,args),
            "GetCurrentTeam"
                if self.demo_story.enabled =>
            {
                self.demo_ai_lua(lua, name, args)
            }
            "SetSplineFollowerInitialSpeed" => self.spline_initial_speed_lua(lua, args),
            "SetSpeed"
            | "Reverse"
            | "SplineFollower_SetDisabled"
            | "SplineFollower_StopAtPosition"
            | "SplineFollower_ClearStopPosition"
            | "SplineFollower_TeleportToKnot"
                if self.demo_story.enabled =>
            {
                self.demo_motion_lua(lua, name, args)
            }
            "Bark" => self.bark_lua(lua, args),
            "SetRotatorMaxSpeed" => self.rotator_max_speed_lua(lua, args),
            "FireSequence"
            | "ForceSequence"
            | "ClearAllCameraAttributesTimed"
            | "ClearAllCameraAttributes"
            | "ClearAllCameraAttributesSD"
            | "ClearAllCameraAttributesTimedSD"
            | "ReleaseCamera"
            | "CameraReset"
            | "ShowHud"
            | "HideHud"
            | "HideReticleOnly"
            | "ShowReticleOnly"
            | "PlayerSetDripsVisible"
            | "SetCharacterInConversation"
            | "SetCameraAttributes"
            | "SaveCameraOverride"
            | "SaveCurrentCameraOverride"
            | "LoadSavedCameraOverride"
                if self.demo_story.enabled =>
            {
                self.demo_story_lua(lua, name, args)
            }
            "StartEmitterByName" | "StartEmitterByNameFireForget" | "StopEmitterByName" => {
                self.named_effect_lua(lua, name, args)
            }
            "SetR3MTLockAnimation" | "SetR3MTUnlockAnimation" => {
                let lock = name == "SetR3MTLockAnimation";
                if args.len() != 1 + usize::from(lock) {
                    return Err(mlua::Error::external(format!("Invalid {name} arguments")));
                }
                if let Some(owner) = self.scripts.resolve_native_entity_argument(lua, args[0].clone())? {
                    let frame = if lock {
                        let frame = lua.coerce_number(args[1].clone())?
                            .ok_or_else(|| mlua::Error::external("Expected a material animation frame"))? as f32;
                        if !frame.is_finite() {
                            return Err(mlua::Error::external("Invalid material animation frame"));
                        }
                        Some(frame)
                    } else {
                        None
                    };
                    self.material_animation.lock(self.entities[owner].draws.clone(), frame);
                }
                Ok(mlua::MultiValue::new())
            }
            "SetR3MTColorReg" | "SetR3MTConstReg" => {
                if args.len()!=3 { return Err(mlua::Error::external(format!("{name} requires entity, register, vector4"))); }
                let expanded=mlua::MultiValue::from_vec(vec![args[0].clone(),mlua::Value::Nil,args[1].clone(),args[2].clone()]);
                let request=super::subgraph_color::parse(lua,&self.scripts,expanded)?;
                let constant=name=="SetR3MTConstReg";
                if let Some(owner)=request.entity.filter(|owner|!self.removed_entities.contains(owner)) {
                    if (0..if constant {4}else{3}).contains(&request.slot) {
                        if constant {
                            self.set_subgraph_constant(owner,None,request.slot,request.color).map_err(mlua::Error::external)?;
                        } else if let Some(effect)=self.named_fx.live.get_mut(&owner) {
                            effect.particles.set_subgraph_regular(None,request.slot,request.color).map_err(mlua::Error::external)?;
                        } else if let Some(effect)=self.scene_particles.live.get_mut(&owner) {
                            effect.particles.set_subgraph_regular(None,request.slot,request.color).map_err(mlua::Error::external)?;
                        } else {
                            let rgba=super::paint_counter::pack_color(request.color).map_err(mlua::Error::external)?;
                            self.publish_model_color(owner,request.slot as u8,rgba,false).map_err(mlua::Error::external)?;
                        }
                    }
                }
                Ok(mlua::MultiValue::new())
            }
            "SetR3MTConstRegOnSubGraph" => {
                let request = super::subgraph_color::parse(lua, &self.scripts, args)?;
                if let Some(entity) = request.entity {
                    self.set_subgraph_constant(
                        entity,
                        request.name.as_deref(),
                        request.slot,
                        request.color,
                    )
                    .map_err(mlua::Error::external)?;
                }
                Ok(mlua::MultiValue::new())
            }
            "AnimVarInt" | "AnimVarFloat" => self.animation_variable_lua(lua, name, args),
            "SetNPCState" => self.npc_animation_state_lua(lua, args),
            "AI_ClearTarget" => {
                if let Some(owner) = self.scripts.resolve_native_entity_argument(
                    lua, args.back().cloned().unwrap_or(mlua::Value::Nil),
                )? {
                    self.clear_ai_combat_target(owner);
                }
                Ok(mlua::MultiValue::new())
            }
            "AIBB_SetReevaluateRequired" => {
                let mut args = args;
                let required = super::actor_scripts::native_boolean(
                    lua, args.pop_back().unwrap_or(mlua::Value::Nil),
                )?;
                if let Some(brain) = self.scripts.resolve_native_entity_argument(
                    lua, args.pop_back().unwrap_or(mlua::Value::Nil),
                )?.and_then(|entity| self.ai_brains.get_mut(&entity)) {
                    brain.reevaluation_requested = required;
                }
                Ok(mlua::MultiValue::new())
            }
            "StoreEndgameData" | "UpdatePins" | "ResetSaveGameSlot" => {
                if !args.is_empty() {
                    self.report.warnings.push(format!("{name} takes no arguments"));
                }
                match name {
                    "StoreEndgameData" => self.store_endgame_outcomes(),
                    "UpdatePins" => self.record_endgame_pins(),
                    _ => self.complete_save_slot().map_err(mlua::Error::external)?,
                }
                Ok(mlua::MultiValue::new())
            }
            "SetEndingGame" => {
                self.checkpoint.ending_game = super::actor_scripts::native_boolean(
                    lua, args.back().cloned().unwrap_or(mlua::Value::Nil),
                )?;
                Ok(mlua::MultiValue::new())
            }
            "GetWasEndGame" | "GetWasQuitGame" => {
                // GetWasQuitGame is false on boot and clean in-session loads.
                // The host quit action currently closes the application.
                Ok(vec![mlua::Value::Boolean(name == "GetWasEndGame" && self.checkpoint.ending_game)].into())
            }
            "StartSequence" => {
                let sequence = lua.coerce_string(args.get(1).cloned().unwrap_or(mlua::Value::Nil))?
                    .ok_or_else(|| mlua::Error::external("StartSequence needs an owner and sequence string"))?;
                let sequence = sequence.to_str()?;
                if sequence.as_ref() == "EndGame:0" {
                    let checkpoint = self.checkpoint.latest.as_ref()
                        .filter(|snapshot| !snapshot.has_active_campaign())
                        .ok_or_else(|| mlua::Error::external("The ending sequence needs a completed slot"))?;
                    let outcomes = checkpoint.bytes[0x2c5..0x2ce].try_into().unwrap();
                    let movies = super::endgame::movies(outcomes, &self.runtime_settings)
                        .map_err(mlua::Error::external)?;
                    self.movies.start_ending(movies);
                } else {
                    self.movies.start_frontend(&sequence).map_err(mlua::Error::external)?;
                }
                Ok(mlua::MultiValue::new())
            }
            "SaveGameToMemory" => self.save_game_to_memory(lua, args),
            "LoadGameFromMemory" => {
                // 3a724 binds this wrapper to REL2fcc8. Its last argument
                // selects the frontend load job instead of the memory buffer.
                if args.len() > 1 {
                    self.report.warnings.push("LoadGameFromMemory expects at most one argument".into());
                }
                let from_disk = super::actor_scripts::native_boolean(
                    lua, args.back().cloned().unwrap_or(mlua::Value::Nil),
                )?;
                if from_disk {
                    self.checkpoint.restore_requested = Some(super::checkpoint::Restore::Disk);
                } else if self.checkpoint.latest.is_some() {
                    self.checkpoint.restore_requested = Some(super::checkpoint::Restore::Memory);
                }
                Ok(mlua::MultiValue::new())
            }
            "RestartCurrentLevel" => {
                // REL2f7e8 -> 2b938 -> 2a21c submits the current descriptor
                // with both part overrides absent. It does not load MICK/WRLD.
                if !args.is_empty() {
                    self.report.warnings.push("RestartCurrentLevel takes no arguments".into());
                }
                self.level_transition.request(&self.report.level)
                    .map_err(mlua::Error::external)?;
                Ok(mlua::MultiValue::new())
            }
            "SetPlayerSpawnTransform" | "SetPlayerSpawnTransformEntity" => {
                if args.len() != 1 {
                    return Err(mlua::Error::external(format!("{name} requires one argument; native count diagnostics are not reconstructed")));
                }
                let entity = if name == "SetPlayerSpawnTransform" {
                    match lua.coerce_string(args[0].clone())? {
                        Some(name) => self
                            .scripts
                            .resolve_name(name.to_str()?.split('\0').next().unwrap_or("")),
                        None => None,
                    }
                } else {
                    self.scripts
                        .action_event_header(lua, &args)?
                        .and_then(|token| self.scripts.resolve_token(token))
                }
                .filter(|&entity| {
                    entity < self.entities.len() && !self.removed_entities.contains(&entity)
                });
                if let Some(entity) = entity {
                    let source = &self.entities[entity];
                    if !source
                        .components
                        .iter()
                        .any(|component| component.class_name == "JPSTransformationComponent")
                    {
                        return Err(mlua::Error::external(
                            "Player spawn source has no resolved transform component",
                        ));
                    }
                    self.player_spawn.set(
                        source.runtime_rotation,
                        source.transform[12..15].try_into().unwrap(),
                    );
                }
                Ok(mlua::MultiValue::new())
            }
            "SetDefaultCameraValues"
            | "SetCombatCameraValues"
            | "SetTravelCameraValues"
            | "ResetDefaultCameraValues"
            | "ResetCombatCameraValues"
            | "ResetTravelCameraValues" => {
                let reset = name.starts_with("Reset");
                let expected = if reset { 0 } else { 3 };
                if args.len() != expected {
                    return Err(mlua::Error::external(format!("{name} requires {expected} arguments; native argument diagnostics are not reconstructed")));
                }
                let values = if reset {
                    [0.; 3]
                } else {
                    [
                        lua.coerce_number(args[0].clone())?.unwrap_or(0.) as f32,
                        lua.coerce_number(args[1].clone())?.unwrap_or(0.) as f32,
                        lua.coerce_number(args[2].clone())?.unwrap_or(0.) as f32,
                    ]
                };
                let extras = &self.camera_profiles.extras;
                let target = &mut self.camera_target;
                let registry = &mut self.entity_event_registry;
                let entities = &self.entities;
                let removed = &self.removed_entities;
                let tracker = self.mickey_tracker.as_ref();
                let kind = match name {
                    "SetCombatCameraValues" | "ResetCombatCameraValues" => {
                        super::camera_defaults::Kind::Combat
                    }
                    "SetTravelCameraValues" | "ResetTravelCameraValues" => {
                        super::camera_defaults::Kind::Travel
                    }
                    _ => super::camera_defaults::Kind::Default,
                };
                let mut notify = |_: &super::camera_profiles::Library,
                                  state: &super::camera_profile_selection::State|
                 -> Result<()> {
                    let profile = extras
                        .get(usize::try_from(state.current)?)
                        .context("Selected camera profile is absent")?;
                    // Native resolves the entity/proxy before removing the old
                    // subscription. Null legitimately skips that resolution.
                    let resolved =
                        super::camera_target::resolve(profile.target, entities, removed, tracker)?;
                    // Host tag, not a Wii pointer or proof of native hash order.
                    target.refresh(resolved, registry, super::camera_feedback_world::LISTENER)
                };
                let controller = &mut self.camera_profiles.controller;
                if reset {
                    controller.reset_values(kind, &mut notify)
                } else {
                    controller.set_values(kind, values, &mut notify)
                }
                .map_err(mlua::Error::external)?;
                let limitation="Native camera profile configuration/selection and tracker target binding are connected; complete gameplay controller updates and rendered default-camera generation remain incomplete.";
                if !self
                    .report
                    .unsupported
                    .iter()
                    .any(|line| line == limitation)
                {
                    self.report.unsupported.push(limitation.into());
                }
                Ok(mlua::MultiValue::new())
            }
            "EnableComponent" | "DisableComponent" => self.named_component_lua(lua, name, args),
            "Enable" | "Disable" => {
                if args.len() != 1 {
                    return Err(mlua::Error::external(
                        "Enable/Disable requires one native entity argument",
                    ));
                }
                let entity = self
                    .scripts
                    .action_event_header(lua, &args)?
                    .and_then(|token| self.scripts.resolve_token(token))
                    .filter(|&entity| {
                        entity < self.entities.len() && !self.removed_entities.contains(&entity)
                    });
                if let Some(entity) = entity {
                    self.set_entity_disabled(entity, name == "Disable")
                        .map_err(mlua::Error::external)?;
                }
                Ok(mlua::MultiValue::new())
            }
            "DestroyEntity" => {
                if args.len() != 1 {
                    return Err(mlua::Error::external(
                        "DestroyEntity requires one native entity argument",
                    ));
                }
                // REL2699fc/278bec accepts numeric names and opaque pointers.
                // The manager resolves that token again at181f28; absent or
                // retired targets legitimately return zero values.
                let entity = self
                    .scripts
                    .action_event_header(lua, &args)?
                    .and_then(|token| self.scripts.resolve_token(token))
                    .filter(|&entity| {
                        entity < self.entities.len() && !self.removed_entities.contains(&entity)
                    });
                if let Some(entity) = entity {
                    if self.demo_story.enabled {
                        self.queue_entity_retirement(entity)
                            .map_err(mlua::Error::external)?;
                        return Ok(mlua::MultiValue::new());
                    }
                    return Err(mlua::Error::external(format!("DestroyEntity {} requires deferred component preparation/readiness, event44 and teardown; immediate retirement would violate the native lifecycle",self.entities[entity].name)));
                }
                Ok(mlua::MultiValue::new())
            }
            "Rumble" => {
                if !(2..=3).contains(&args.len()) {
                    return Err(mlua::Error::external("Rumble requires entity, duration and optional controller channel"));
                }
                // Retail consumes the entity token but dispatches through the input manager.
                let _entity = self.scripts.resolve_world_argument(&args[0])?;
                let duration = lua.coerce_number(args[1].clone())?.unwrap_or(0.) as f32;
                if !duration.is_finite() { return Err(mlua::Error::external("Nonfinite rumble duration")); }
                let channel = if args.len()==3 { super::score_state::integer_argument(lua,args[2].clone())? as u32 } else {0};
                self.script_feedback.rumble(channel,duration,self.game_options.rumble!=0);
                let limitation = "Script rumble timers are connected; physical controller haptics require an assigned host input device.";
                if self.game_options.rumble!=0 && !self.report.unsupported.iter().any(|line|line==limitation) {
                    self.report.unsupported.push(limitation.into());
                }
                Ok(mlua::MultiValue::new())
            }
            "ShakeCamera" => {
                if args.len()!=6 { return Err(mlua::Error::external("ShakeCamera requires six numeric arguments")); }
                let mut values=[0.;6];
                for (destination,value) in values.iter_mut().zip(args.iter()) {
                    *destination=lua.coerce_number(value.clone())?.ok_or_else(||mlua::Error::external("ShakeCamera argument is not numeric"))? as f32;
                }
                self.script_feedback.shake(values).map_err(mlua::Error::external)?;
                Ok(mlua::MultiValue::new())
            }
            "ToggleLetterbox" => {
                if !args.is_empty() {
                    return Err(mlua::Error::external("ToggleLetterbox takes no arguments"));
                }
                self.camera_lens.toggle_letterbox().map_err(mlua::Error::external)?;
                Ok(mlua::MultiValue::new())
            }
            "SetLetterbox" => {
                if args.len() != 1 {
                    return Err(mlua::Error::external(
                        "SetLetterbox requires one native boolean argument",
                    ));
                }
                let requested = super::actor_scripts::native_boolean(lua, args[0].clone())?;
                self.camera_lens
                    .set_letterbox(requested)
                    .map_err(mlua::Error::external)?;
                Ok(mlua::MultiValue::new())
            }
            "GrabCameraNif" | "GrabCamera" => {
                if args.len() != 4 {
                    return Err(mlua::Error::external(format!(
                        "{name} requires source, focus, transition mode and duration"
                    )));
                }
                let duration = lua.coerce_number(args[3].clone())?.unwrap_or(0.) as f32;
                let mode = super::score_state::integer_argument(lua, args[2].clone())?;
                let focus = self.scripts.resolve_world_argument(&args[1])?;
                let source = self.scripts.resolve_world_argument(&args[0])?;
                if name == "GrabCameraNif" {
                    self.select_nif_camera(source, focus, mode, duration)
                } else {
                    self.select_entity_camera(source, focus, mode, duration)
                }
                .map_err(mlua::Error::external)?;
                Ok(mlua::MultiValue::new())
            }
            "SetLedgeGrabEnabled" | "SetWallPushingEnabled" => {
                if args.len() != 2 {
                    return Err(mlua::Error::external(format!(
                        "{name} requires entity and native boolean value"
                    )));
                }
                // Native stack conversion reads the boolean before resolving
                // the entity; a missing/non-Mickey target legitimately declines.
                let enabled = super::actor_scripts::native_boolean(lua, args[1].clone())?;
                if let Some(entity) = self.scripts.resolve_world_argument(&args[0])? {
                    if !self.removed_entities.contains(&entity)
                        && self.entities[entity].components.iter().any(|component| {
                            component.class_name == "MickeyPlayerMotionControllerComponent"
                        })
                    {
                        if name == "SetWallPushingEnabled" {
                            // REL 9c87c writes the per-controller byte at
                            // +0x498. Constructor 957e0 defaults it to true.
                            if enabled {
                                self.wall_pushing_disabled.remove(&entity);
                            } else {
                                self.wall_pushing_disabled.insert(entity);
                            }
                        } else {
                            self.ledge_controls
                                .get_mut(&entity)
                                .ok_or_else(|| {
                                    mlua::Error::external("Mickey ledge gate was not initialized")
                                })?
                                .set_script_gate(enabled);
                        }
                    }
                }
                Ok(mlua::MultiValue::new())
            }
            "SetDrawSilhouettes" => {
                if args.len() != 1 {
                    return Err(mlua::Error::external(
                        "SetDrawSilhouettes requires one native boolean value",
                    ));
                }
                let enabled = super::actor_scripts::native_boolean(lua, args[0].clone())?;
                self.render_options.set_draw_silhouettes(enabled);
                Ok(mlua::MultiValue::new())
            }
            "SetMap" => {
                if args.len() != 3 {
                    return Err(mlua::Error::external("SetMap requires entity, map and map type; native argument diagnostics are not reconstructed"));
                }
                // REL1f9618 pops map type, map, entity in that order. The
                // entity is converted by278bec but is never dereferenced or
                // used to gate the two GameState string writes.
                let text = |value: mlua::Value| -> mlua::Result<String> {
                    Ok(lua
                        .coerce_string(value)?
                        .map(|text| {
                            text.to_str()
                                .map(|text| text.split('\0').next().unwrap_or("").to_owned())
                        })
                        .transpose()?
                        .unwrap_or_default())
                };
                let map_type = text(args[2].clone())?;
                let map = text(args[1].clone())?;
                self.scripts
                    .action_event_header(lua, &mlua::MultiValue::from_vec(vec![args[0].clone()]))?;
                self.map_info.map = map;
                self.map_info.map_type = map_type;
                let limitation="Pause-map GameState metadata is connected; original pause-menu/Scaleform map rendering remains unimplemented.";
                if !self
                    .report
                    .unsupported
                    .iter()
                    .any(|line| line == limitation)
                {
                    self.report.unsupported.push(limitation.into());
                }
                Ok(mlua::MultiValue::new())
            }
            "AudioPostEventOn" => {
                if args.len() != 2 {
                    return Err(mlua::Error::external(
                        "AudioPostEventOn requires entity and event name",
                    ));
                }
                let event = lua
                    .coerce_string(args[1].clone())?
                    .ok_or_else(|| mlua::Error::external("Audio event name is not a string"))?;
                let event = event.to_str()?.split('\0').next().unwrap_or("").to_owned();
                let Some(entity) = self.scripts.resolve_world_argument(&args[0])? else {
                    return Ok(mlua::MultiValue::new());
                };
                if self.removed_entities.contains(&entity) {
                    return Ok(mlua::MultiValue::new());
                }
                let id = self.post_audio_from_game(entity, &event);
                Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Integer(
                    id as mlua::Integer,
                )]))
            }
            "MultiPosSoundStart" | "MultiPosSoundStop" => {
                if args.len() != 1 {
                    return Err(mlua::Error::external(format!("{name} requires one entity")));
                }
                if let Some(entity) = self.scripts.resolve_world_argument(&args[0])? {
                    self.control_multi_position_audio(entity, name == "MultiPosSoundStart")
                        .map_err(mlua::Error::external)?;
                }
                Ok(mlua::MultiValue::new())
            }
            "StartRotateToPosition" | "StopRotateToPosition" | "InstantRotateToPosition" => {
                self.rotator_command_lua(lua, name, args)
            }
            "GetPropertyString" => self.get_script_string_property(lua, args),
            "ForcePainted"|"ForceSketched" => {
                if args.is_empty() {return Err(mlua::Error::external("Body paint command requires an entity"))}
                // Retail pops an optional source from the end, then target;
                // leading surplus arguments are ignored by these callbacks.
                let target=if args.len()>1 {args.len()-2} else {0};
                let Some(owner)=self.scripts.resolve_native_entity_argument(lua,args[target].clone())?
                    .filter(|i|!self.removed_entities.contains(i)) else {return Ok(mlua::MultiValue::new())};
                if self.model_paint.contains_key(&owner) {
                    let position=self.entities[self.player_entity].transform[12..15].try_into().unwrap();
                    self.change_whole_model_paint(owner,name=="ForcePainted",position,false).map_err(mlua::Error::external)?;
                } else if self.jigsaw_instances.iter().any(|instance|instance.entity==owner) {
                    let position=self.entities[self.player_entity].transform[12..15].try_into().unwrap();
                    self.change_whole_jigsaw_paint(owner,name=="ForcePainted",position).map_err(mlua::Error::external)?;
                } else {
                    let body=self.entities[owner].components.iter().find(|c|matches!(c.class_name.as_str(),
                        "PhysicsRigidBodyDynamicComponent"|"PhysicsRigidBodyStaticComponent"|"PhysicsRigidBodyCharacterComponent"));
                    if let Some(body)=body {
                        let real=body.properties.iter().find(|p|p.name=="Is Real").and_then(|p|p.value.as_bool());
                        if real!=Some(true) || body.class_name=="PhysicsRigidBodyCharacterComponent" {
                            return Err(mlua::Error::external("Scripted paint for this jigsaw/character or unresolved body is not connected"));
                        }
                    }
                }
                let limitation="Scripted whole-model and jigsaw body transitions are connected. Character dispatch, native event/source attribution, whole-jigsaw queue interaction parity and full transition timing parity remain incomplete.";
                if !self.report.unsupported.iter().any(|v|v==limitation) {self.report.unsupported.push(limitation.into());}
                Ok(mlua::MultiValue::new())
            }
            "TimerEnable"|"TimerDisable"|"TimerReset"|"TimerSetInterval" => self.logic_timer_lua(lua,name,args),
            "TimerStart"|"TimerStop"|"TimerPause"|"TimerUnpause"|"TimerGetTime"|"TimerGetState"|"TimerAddTime"|"TimerSetTime" => self.countdown_lua(lua,name,args),
            "AI_SetDisabled"|"AIController_SetDisabled" => self.ai_disabled_lua(lua,name,args),
            "Damage" => self.damage_lua(lua,args),
            "IsAlive"|"IsPlayer"|"IsCharacter" => self.entity_liveness_lua(lua,name,args),
            "GetHealth"|"SetHealth" => self.health_lua(lua,name,args),
            "GetPropertyInt" => self.get_script_integer_property(lua, args),
            "GetPropertyBool" | "SetPropertyBool" => self.bool_property_lua(lua, name, args),
            "GetElementCount" => self.property_count_lua(lua,args),
            "GetChildEntityByIndex" => self.child_index_lua(lua,args),
            "SetParentEntity"|"ClearParent" => self.parenting_lua(lua,name,args),
            "SetTransformation" => self.transformation_lua(lua,args),
            "GetPropertyEntity" => self.get_entity_property_lua(lua, args),
            "SetPropertyEntity" => self.set_entity_property_lua(lua, args),
            "SetPropertyString" | "SetPropertyInt" => {
                if args.len() != 3 && args.len() != 4 {
                    return Err(mlua::Error::external(format!("{name} requires entity, property, value and optional index; native argument diagnostics are not reconstructed")));
                }
                let integer = |value: mlua::Value| -> mlua::Result<i32> {
                    let value = lua.coerce_number(value)?.unwrap_or(0.).trunc();
                    if !value.is_finite() || value < i32::MIN as f64 || value > i32::MAX as f64 {
                        return Err(mlua::Error::external("Property integer conversion requires native fctiwz validation for this value"));
                    }
                    Ok(value as i32)
                };
                let index = args.get(3).cloned().map(integer).transpose()?.unwrap_or(0) as u32;
                let (value, null_string) = if name == "SetPropertyString" {
                    let text = lua.coerce_string(args[2].clone())?;
                    let null_string = text.is_none();
                    let value = serde_json::Value::String(
                        text.map(|text| {
                            text.to_str()
                                .map(|text| text.split('\0').next().unwrap_or("").to_owned())
                        })
                        .transpose()?
                        .unwrap_or_default(),
                    );
                    (value, null_string)
                } else {
                    (serde_json::json!(integer(args[2].clone())?), false)
                };
                let property = lua.coerce_string(args[1].clone())?;
                let Some(property) = property else {
                    return Ok(mlua::MultiValue::new());
                };
                let property = property
                    .to_str()?
                    .split('\0')
                    .next()
                    .unwrap_or("")
                    .to_owned();
                let entity_args = mlua::MultiValue::from_vec(vec![args[0].clone()]);
                let Some(token) = self.scripts.action_event_header(lua, &entity_args)? else {
                    return Ok(mlua::MultiValue::new());
                };
                let Some(entity) =
                    self.scripts
                        .resolve_world_argument(&mlua::Value::LightUserData(
                            mlua::LightUserData(token as *mut std::ffi::c_void),
                        ))?
                else {
                    return Ok(mlua::MultiValue::new());
                };
                let expected = if name == "SetPropertyString" {
                    "String"
                } else {
                    "Integer"
                };
                let found = self.entities[entity]
                    .components
                    .iter()
                    .enumerate()
                    .find_map(|(component_index, component)| {
                        component
                            .properties
                            .iter()
                            .enumerate()
                            .find(|(_, field)| field.name == property)
                            .map(|(field_index, field)| (component_index, field_index, field))
                    });
                if expected == "String" && property == "Bone Attach Name" {
                    let transform = match found {
                        Some((component, field, value))
                            if self.entities[entity].components[component].class_name == "JPSTransformationComponent"
                                && value.class_name == "String" && !value.value.is_array() => Some((component, Some(field))),
                        None => self.entities[entity].components.iter()
                            .position(|component| component.class_name == "JPSTransformationComponent")
                            .map(|component| (component, None)),
                        _ => None,
                    };
                    if let Some((component, field)) = transform.filter(|_| index == 0) {
                        let name = if null_string { None } else { value.as_str() };
                        self.set_bone_attachment_name(entity, component, field, name)
                            .map_err(mlua::Error::external)?;
                    }
                    return Ok(mlua::MultiValue::new());
                }
                if found.is_none() {
                    // Reflected fields exist even when the scene omitted an
                    // unchanged default. Only create the verified scalar pool
                    // fields here; other missing native setters stay explicit.
                    if let Some(component) =
                        self.entities[entity]
                            .components
                            .iter_mut()
                            .find(|component| {
                                component.class_name == "TemplateSpawnerComponent"
                                    && matches!(
                                        property.as_str(),
                                        "Spawn Group"
                                            | "Index To Spawn"
                                            | "Max Live Spawns"
                                            | "Number To Spawn"
                                            | "Total Spawns"
                                    )
                            })
                    {
                        let kind = if property == "Spawn Group" {
                            "String"
                        } else {
                            "Integer"
                        };
                        if kind == expected && index == 0 {
                            component.properties.push(
                                epic_mickey_lib_rs::scene_file::Property::new(
                                    expected.into(),
                                    property,
                                    false,
                                    false,
                                    false,
                                    value,
                                ),
                            );
                        }
                        return Ok(mlua::MultiValue::new());
                    }
                    if self.entities[entity].components.iter().any(|component| {
                        crate::component_schema::CATALOG
                            .components
                            .get(&component.class_name)
                            .is_some_and(|definition| definition.fields.contains_key(&property))
                    }) {
                        return Err(mlua::Error::external(format!(
                            "Reflected setter for omitted field {property} is not connected"
                        )));
                    }
                    return Ok(mlua::MultiValue::new());
                }
                let Some((component_index, field_index, field)) =
                    found.filter(|(_, _, field)| field.class_name == expected)
                else {
                    return Ok(mlua::MultiValue::new());
                };
                // Base scalar setter130468 declines nonzero indices. Do not
                // manufacture an array or report an unsupported operation.
                if !field.value.is_array() && index != 0 {
                    return Ok(mlua::MultiValue::new());
                }
                let component = &self.entities[entity].components[component_index];
                if expected == "Integer"
                    && component.class_name == "DeathRewardComponent"
                    && property == "RewardSpawnIndex"
                    && field.value.is_array()
                {
                    // Reward selection (REL77490) reads this indexed property
                    // when the death event arrives. Preserve the live array;
                    // invalid indices decline without resizing it.
                    if let Some(slot) = self.entities[entity].components[component_index]
                        .properties[field_index].value.get_mut(index as usize) {
                        *slot = value;
                    }
                    return Ok(mlua::MultiValue::new());
                }
                if component.class_name == "InventoryComponent"
                    && matches!((expected,property.as_str()),("Integer","SketchCount")|("String","Sketch"))
                {
                    // Indexed reflected setters decline invalid slots. They do
                    // not resize or merge these independently stored arrays.
                    if let Some(slot)=self.entities[entity].components[component_index].properties[field_index]
                        .value.get_mut(index as usize) { *slot=value; }
                    return Ok(mlua::MultiValue::new());
                }
                if expected == "Integer"
                    && matches!(
                        component.class_name.as_str(),
                        "PhysicsRigidBodyCharacterComponent" | "PhysicsRigidBodyStaticComponent"
                    )
                    && property == "Collision Layer"
                    && !field.value.is_array()
                {
                    let value = i32::try_from(value.as_i64().ok_or_else(|| {
                        mlua::Error::external("Invalid character collision layer")
                    })?)
                    .map_err(mlua::Error::external)?;
                    if component.class_name == "PhysicsRigidBodyStaticComponent" {
                        self.set_static_collision_layer(
                            entity,
                            component_index,
                            field_index,
                            value,
                        )
                        .map_err(mlua::Error::external)?;
                    } else {
                        self.set_character_collision_layer(
                            entity,
                            component_index,
                            field_index,
                            value,
                        )
                        .map_err(mlua::Error::external)?;
                    }
                    return Ok(mlua::MultiValue::new());
                }
                if expected == "Integer" && component.class_name == "HealthComponent" && property == "Health" && !field.value.is_array() {
                    self.set_script_health(entity,value.as_i64().unwrap() as i32)?;
                    return Ok(mlua::MultiValue::new());
                }
                let scalar_spawner = !field.value.is_array()
                    && ((component.class_name == "TemplateSpawnerComponent"
                        && matches!(
                            property.as_str(),
                            "Spawn Group"
                                | "Index To Spawn"
                                | "Max Live Spawns"
                                | "Number To Spawn"
                                | "Total Spawns"
                        ))
                        || (component.class_name == "WeaponComponent"
                            && matches!(
                                property.as_str(),
                                "WeaponShots" | "WeaponShotsMin" | "WeaponShotsMax" | "ActiveWeapon"
                            )));
                if !scalar_spawner {
                    return Err(mlua::Error::external(format!(
                        "Native {expected} setter is not connected for {}.{property}",
                        component.class_name
                    )));
                }
                // TemplateSpawner's reflected scalar pool is authoritative.
                // Keep source assets immutable and avoid a second property map.
                self.entities[entity].components[component_index].properties[field_index].value =
                    value;
                Ok(mlua::MultiValue::new())
            }
            "AnimGBReset" => {
                if args.len() != 1 {
                    return Err(mlua::Error::external("AnimGBReset requires one entity; native argument diagnostics are not reconstructed"));
                }
                let Some(token) = self.scripts.action_event_header(lua, &args)? else {
                    return Ok(mlua::MultiValue::new());
                };
                let Some(entity) =
                    self.scripts
                        .resolve_world_argument(&mlua::Value::LightUserData(
                            mlua::LightUserData(token as *mut std::ffi::c_void),
                        ))?
                else {
                    return Ok(mlua::MultiValue::new());
                };
                if !self.entities[entity]
                    .components
                    .iter()
                    .any(|c| c.class_name == "AnimationComponentGB")
                {
                    return Ok(mlua::MultiValue::new());
                }
                self.reset_actor_sequence(entity)
                    .map_err(mlua::Error::external)?;
                Ok(mlua::MultiValue::new())
            }
            "AnimGBSequence" => {
                if args.len() < 2 {
                    return Err(mlua::Error::external("AnimGBSequence needs entity and sequence; native argument diagnostics are not reconstructed"));
                }
                // The native wrapper pops at most six values from the stack.
                // Any earlier extra arguments remain unused, not shifted into
                // the entity/name positions (REL2f8020..80c0).
                let args = args
                    .iter()
                    .skip(args.len().saturating_sub(6))
                    .cloned()
                    .collect::<Vec<_>>();
                let temporary = args
                    .get(2)
                    .cloned()
                    .map(|value| super::actor_scripts::native_boolean(lua, value))
                    .transpose()?
                    .unwrap_or(false);
                let float = |at: usize| -> mlua::Result<f32> {
                    let Some(value) = args.get(at) else {
                        return Ok(0.);
                    };
                    // REL27883c uses Lua5.1 lua_tonumber: nonnumeric values
                    // convert to zero, without a metamethod or type error.
                    let value = lua.coerce_number(value.clone())?.unwrap_or(0.) as f32;
                    if !value.is_finite() {
                        return Err(mlua::Error::external(
                            "Nonfinite actor timing requires native reference validation",
                        ));
                    }
                    Ok(value)
                };
                let timing = float(3)?;
                let duration = float(4)?;
                let dirty = args
                    .get(5)
                    .cloned()
                    .map(|value| super::actor_scripts::native_boolean(lua, value))
                    .transpose()?
                    .unwrap_or(false);
                let sequence = lua.coerce_string(args[1].clone())?;
                let Some(sequence) = sequence else {
                    return Ok(mlua::MultiValue::new());
                };
                let sequence = sequence
                    .to_str()?
                    .split('\0')
                    .next()
                    .unwrap_or("")
                    .to_owned();
                if sequence.is_empty() {
                    return Ok(mlua::MultiValue::new());
                }
                let entity_args = mlua::MultiValue::from_vec(vec![args[0].clone()]);
                let Some(token) = self.scripts.action_event_header(lua, &entity_args)? else {
                    return Ok(mlua::MultiValue::new());
                };
                let Some(entity) =
                    self.scripts
                        .resolve_world_argument(&mlua::Value::LightUserData(
                            mlua::LightUserData(token as *mut std::ffi::c_void),
                        ))?
                else {
                    return Ok(mlua::MultiValue::new());
                };
                let Some(component) = self.entities[entity]
                    .components
                    .iter()
                    .find(|component| component.class_name == "AnimationComponentGB")
                else {
                    return Ok(mlua::MultiValue::new());
                };
                if !self.actor_sequences.contains_key(&entity) {
                    let path = component
                        .properties
                        .iter()
                        .find(|p| p.name.replace(' ', "") == "KFMFilePath")
                        .and_then(|p| p.value.as_str());
                    if path.is_none_or(str::is_empty) {
                        return Ok(mlua::MultiValue::new());
                    }
                    return Err(mlua::Error::external(
                        "AnimationComponentGB manager resources are unavailable",
                    ));
                }
                let resources = &self.actor_sequences[&entity];
                let missing = resources.get(resources.find_name(&sequence)).is_none();
                if dirty || (!missing && (timing > 0. || duration > 0.)) {
                    let message = format!(
                        "AnimGBSequence({entity}, {sequence}) requested unsupported timing/model-dirty modifiers; playing the authored manager sequence with its default transition instead"
                    );
                    if !self
                        .report
                        .unsupported
                        .iter()
                        .any(|entry| entry == &message)
                    {
                        self.report.unsupported.push(message);
                    }
                }
                self.request_actor_sequence_with_end_event(entity, &sequence, temporary)
                    .map_err(mlua::Error::external)?;
                Ok(mlua::MultiValue::new())
            }
            "EnterCutscene" | "ExitCutscene" | "IsInCutscene" => {
                self.cutscene_action_lua(lua, name, args)
            }
            "PauseAllAI" | "UnpauseAllAI" => {
                let header = self.scripts.action_event_header(lua, &args)?;
                let action = if name == "PauseAllAI" {
                    super::ai_registry::Action::Pause
                } else {
                    super::ai_registry::Action::Resume
                };
                let mut host = super::action_event_queue::AiBroadcast {
                    queue: &mut self.entity_event_queue,
                    manager_enabled: &mut self.ai_manager_enabled,
                };
                self.ai_registry
                    .broadcast(action, header, &mut host)
                    .map_err(mlua::Error::external)?;
                // REL184b14 queues;184b34 changes the manager gate;184b44
                // returns zero Lua results. Receiver delivery is a later phase:
                // do not convert successful enqueue into a synchronous Lua error.
                if self.entity_event_queue.pending() != 0 {
                    let message="Deferred AI actions use the live shared queue. Complete native component subscriptions, controller outputs and phase parity remain incomplete.";
                    if !self.report.unsupported.iter().any(|entry| entry == message) {
                        self.report.unsupported.push(message.into());
                    }
                }
                Ok(mlua::MultiValue::new())
            }
            "#bwx_math_random" => super::lua_random::random(lua, args, &mut self.game_random),
            "#bwx_math_randomseed" => super::lua_random::seed(lua, args, &mut self.game_random),
            "#bwx_animation_noise" => {
                if args.len() != 1 {
                    return Err(mlua::Error::external(
                        "Invalid internal animation-noise continuation",
                    ));
                }
                let entity = <usize as mlua::FromLua>::from_lua(args[0].clone(), lua)?;
                self.request_animation_noise(entity);
                return Ok(mlua::MultiValue::new());
            }
            "BwxNativeAnimationEvent" => {
                if args.len() != 2 {
                    return Err(mlua::Error::external(
                        "BwxNativeAnimationEvent requires entity and native event ID",
                    ));
                }
                let entity = self
                    .scripts
                    .resolve_world_argument(&args[0])?
                    .ok_or_else(|| {
                        mlua::Error::external("Native animation diagnostic target is not loaded")
                    })?;
                let id = <u32 as mlua::FromLua>::from_lua(args[1].clone(), lua)?;
                let handled = self
                    .send_native_actor_animation_event(entity, id)
                    .map_err(mlua::Error::external)?;
                return Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Boolean(
                    handled,
                )]));
            }
            "AnimEvent" => {
                if args.len() != 2 {
                    return Err(mlua::Error::external(
                        "AnimEvent requires entity and native integer event ID",
                    ));
                }
                // REL2e9be0 uses27880c, including global-name lookup.
                // IO parameters arrive as strings, not evaluated Lua tokens.
                let id = super::score_state::integer_argument(lua,args[1].clone())? as u32;
                if let Some(entity) = self.scripts.resolve_world_argument(&args[0])? {
                    if self.entities[entity]
                        .components
                        .iter()
                        .any(|component| component.class_name == "AnimationComponent")
                    {
                        self.send_native_actor_animation_event(
                            entity,
                            id,
                        )
                        .map_err(mlua::Error::external)?;
                    }
                }
                Ok(mlua::MultiValue::new())
            }
            "PlayerPickup_SetPendingActivation" => {
                if args.len() != 2 {
                    return Err(mlua::Error::external("Pending activation requires a pickup and duration"));
                }
                let duration = lua.coerce_number(args[1].clone())?
                    .map(|duration| duration as f32)
                    .filter(|duration| duration.is_finite())
                    .ok_or_else(|| mlua::Error::external("Pickup duration is not finite"))?;
                if let Some(owner) = self.scripts.resolve_world_argument(&args[0])? {
                    if let Some(pickup) = self.pickup_update_states.get_mut(&owner) {
                        pickup.gate.delay(duration);
                    }
                }
                Ok(mlua::MultiValue::new())
            }
            "ForceSpawn" => {
                if args.len() != 2 {
                    return Err(mlua::Error::external(
                        "ForceSpawn requires entity and integer count",
                    ));
                }
                let value = lua
                    .coerce_number(args[1].clone())?
                    .ok_or_else(|| mlua::Error::external("Spawn count is not numeric"))?;
                if !value.is_finite()
                    || value.trunc() < i32::MIN as f64
                    || value.trunc() > i32::MAX as f64
                {
                    return Err(mlua::Error::external(
                        "Spawn count exceeds native integer range",
                    ));
                }
                let Some(owner) = self.scripts.resolve_world_argument(&args[0])? else {
                    return Ok(mlua::MultiValue::new());
                };
                let handles = self
                    .force_spawn(owner, value.trunc() as i32)
                    .map_err(mlua::Error::external)?;
                let table = lua.create_table()?;
                for (index, entity) in handles.into_iter().enumerate() {
                    let value = entity.map(|entity|self.scripts.entity_value(entity)).transpose()?.unwrap_or(mlua::Value::Integer(0));
                    table.raw_set(index + 1, value)?;
                }
                Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Table(table)]))
            }
            "AddHeroPoints" | "AddScrapperPoints" => {
                if args.len() != 2 {
                    return Err(mlua::Error::external("Guardian points require an entity and amount"));
                }
                let amount = lua.coerce_number(args[1].clone())?
                    .map(|amount| amount as f32)
                    .filter(|amount| amount.is_finite())
                    .ok_or_else(|| mlua::Error::external("Guardian point amount must be finite"))?;
                self.scripts.resolve_native_entity_argument(lua, args[0].clone())?;
                self.guardians.add_points(name == "AddHeroPoints", amount)
                    .map_err(mlua::Error::external)?;
                Ok(mlua::MultiValue::new())
            }
            "GetHeroPercent" | "GetScrapperPercent" => {
                // Retail15bf8/15d0c resolve/pop the supplied entity, but the
                // percentage comes from the global guardian state manager.
                if args.len()!=1 {
                    self.report.warnings.push(format!("{name}: expected one argument; preserving native stack-pop order"));
                }
                self.scripts.resolve_native_entity_argument(lua,args.back().cloned().unwrap_or(mlua::Value::Nil))?;
                Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Number(
                    self.guardians.percentage(name=="GetHeroPercent") as f64)]))
            }
            "AnimGBSetIdle" => {
                if args.len() != 2 {
                    return Err(mlua::Error::external("AnimGBSetIdle needs entity and boolean; native argument diagnostics are not reconstructed"));
                }
                let idle = super::actor_scripts::native_boolean(lua, args[1].clone())?;
                if let Some(entity) = self.scripts.resolve_world_argument(&args[0])? {
                    if self.entities[entity]
                        .components
                        .iter()
                        .any(|component| component.class_name == "AnimationComponentGB")
                    {
                        if idle {
                            self.idle_actors.insert(entity);
                        } else {
                            self.idle_actors.remove(&entity);
                        }
                        if !self.actors.contains_key(&entity) {
                            let message=format!("Animation idle state stored for {}; its actor-manager pose binding is not available",self.entities[entity].name);
                            if !self.report.unsupported.contains(&message) {
                                self.report.unsupported.push(message);
                            }
                        }
                    }
                }
                Ok(mlua::MultiValue::new())
            }
            "RemovePhysicsFromWorld" => {
                if args.len()!=1 {self.report.warnings.push("RemovePhysicsFromWorld: expected one entity; preserving native stack order".into());}
                let mut args=args;
                if let Some(entity)=self.scripts.resolve_native_entity_argument(lua,args.pop_back().unwrap_or(mlua::Value::Nil))?.filter(|i|!self.removed_entities.contains(i)) {
                    self.remove_entity_physics(entity).map_err(mlua::Error::external)?;
                }
                Ok(mlua::MultiValue::new())
            }
            "EnableMotion" | "DisableMotion" => {
                if args.len() != 1 {
                    return Err(mlua::Error::external("Motion API requires one entity; native count diagnostics are not reconstructed"));
                }
                // REL263c54/263d54 ->2995a8/2995a0 ->2995b0. Names and opaque
                // entity arguments use the existing native entity conversion.
                if let Some(entity) = self.scripts.resolve_world_argument(&args[0])? {
                    self.set_entity_motion_enabled(entity, name == "EnableMotion")
                        .map_err(mlua::Error::external)?;
                }
                Ok(mlua::MultiValue::new())
            }
            "Hide" | "Unhide" => {
                if args.len() != 1 {
                    return Err(mlua::Error::external("Visibility API needs one entity; native argument diagnostics are not reconstructed"));
                }
                if let Some(entity) = self.scripts.resolve_world_argument(&args[0])? {
                    self.set_entity_hidden(entity, name == "Hide")
                        .map_err(mlua::Error::external)?;
                }
                Ok(mlua::MultiValue::new())
            }
            "IsHidden" => {
                if args.len() != 1 {
                    return Err(mlua::Error::external("IsHidden requires one entity"));
                }
                let hidden = self
                    .scripts
                    .resolve_world_argument(&args[0])?
                    .is_some_and(|entity| self.hidden_entities.contains(&entity));
                Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Boolean(
                    hidden,
                )]))
            }
            "SetPropertyVector" => self.set_vector_property_lua(lua,args),
            "SplineKnotComponent_SetBranchProbability" => {
                if args.len() != 2 {
                    return Err(mlua::Error::external("SetBranchProbability requires a spline knot and a probability"));
                }
                let chance = lua.coerce_number(args[1].clone())?
                    .context("Spline branch probability must be numeric")
                    .map_err(mlua::Error::external)? as f32;
                if let Some(entity) = self.scripts.resolve_native_entity_argument(lua, args[0].clone())?
                    .filter(|entity| !self.removed_entities.contains(entity)
                        && self.entities[*entity].components.iter()
                            .any(|component| component.class_name == "SplineKnotComponent"))
                {
                    self.set_entity_float_property(entity, "BranchPathChance", chance)
                        .map_err(mlua::Error::external)?;
                }
                Ok(mlua::MultiValue::new())
            }
            "GetPropertyFloat" | "SetPropertyFloat" => {
                let setter = name == "SetPropertyFloat";
                let required = if setter { 3 } else { 2 };
                if args.len() != required && args.len() != required + 1 {
                    return Err(mlua::Error::external(
                        "Float property API has invalid argument count",
                    ));
                }
                let index = if args.len() > required {
                    <i32 as mlua::FromLua>::from_lua(args[required].clone(), lua)?
                } else {
                    0
                };
                let property = lua
                    .coerce_string(args[1].clone())?
                    .ok_or_else(|| mlua::Error::external("Property name is not a string"))?;
                let property = property
                    .to_str()?
                    .split('\0')
                    .next()
                    .unwrap_or("")
                    .to_owned();
                let value = if setter {
                    Some(lua.coerce_number(args[2].clone())?.ok_or_else(|| {
                        mlua::Error::external("Float property value is not numeric")
                    })? as f32)
                } else {
                    None
                };
                let Some(entity) = self.scripts.resolve_world_argument(&args[0])? else {
                    return Ok(mlua::MultiValue::new());
                };
                if self.removed_entities.contains(&entity) {
                    return Ok(mlua::MultiValue::new());
                }
                let scalar_owner = match property.as_str() {
                    "Spawn Delay" | "Spawn Radius" => Some("TemplateSpawnerComponent"),
                    "UsableRadius" => Some("UsableComponent"),
                    "Target Value" => Some("TargetableComponent"),
                    "InitialSpeed" => Some("SplineFollowerComponent"),
                    _ => None,
                };
                if let Some(owner) = scalar_owner {
                    let first = self.entities[entity]
                        .components
                        .iter()
                        .find_map(|component| {
                            component
                                .properties
                                .iter()
                                .find(|candidate| candidate.name == property)
                                .map(|candidate| (component, candidate))
                        });
                    // Preserve native first-property type admission before supplying
                    // a declared scalar absent from the cooked property list.
                    if let Some((component, candidate)) = first {
                        if candidate.class_name != "Float" {
                            return Ok(mlua::MultiValue::new());
                        }
                        if component.class_name != owner || candidate.value.is_array() {
                            return Err(mlua::Error::external(format!(
                                "{property} does not belong to a supported {owner} scalar"
                            )));
                        }
                    } else if !self.entities[entity]
                        .components
                        .iter()
                        .any(|component| component.class_name == owner)
                    {
                        return Ok(mlua::MultiValue::new());
                    }
                    if index != 0 {
                        return Ok(mlua::MultiValue::new());
                    }
                    if let Some(value) = value {
                        self.set_entity_float_property(entity, &property, value)
                            .map_err(mlua::Error::external)?;
                        return Ok(mlua::MultiValue::new());
                    }
                    if property == "InitialSpeed" {
                        if let Some(speed) = self.live_spline_speed(entity) {
                            return Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Number(
                                f64::from(speed),
                            )]));
                        }
                    }
                    let scalar = first
                        .map(|(_, candidate)| {
                            candidate.value.as_f64().ok_or_else(|| {
                                mlua::Error::external(format!("Invalid {property} scalar"))
                            })
                        })
                        .transpose()?
                        .unwrap_or(0.);
                    return Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Number(
                        scalar,
                    )]));
                }
                // REL2656c8/266600 returns no Lua values for a missing property
                // or a non-float type. A successful setter also returns none.
                let found = self.entities[entity]
                    .components
                    .iter()
                    .flat_map(|component| {
                        component
                            .properties
                            .iter()
                            .map(move |candidate| (component, candidate))
                    })
                    .find(|(_, candidate)| candidate.name == property);
                let Some((component, found)) =
                    found.filter(|(_, candidate)| candidate.class_name == "Float")
                else {
                    return Ok(mlua::MultiValue::new());
                };
                if component.class_name == "InventoryComponent" {
                    if let Some(value)=value {
                        self.set_inventory_float(entity,&property,index,value).map_err(mlua::Error::external)?;
                        return Ok(mlua::MultiValue::new());
                    }
                    let value=self.inventory_float(entity,&property,index).map_err(mlua::Error::external)?;
                    return Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Number(f64::from(value))]));
                }
                if component.class_name=="PhysicsStimulusComponent" && matches!(property.as_str(),"StimulusPerSecondList"|"StimulusRadiusList") {
                    let offset=usize::try_from(index).map_err(mlua::Error::external)?;
                    if let Some(value)=value {
                        self.set_contact_float(entity,&property,offset,value).map_err(mlua::Error::external)?;
                        return Ok(mlua::MultiValue::new());
                    }
                    let value=found.value.get(offset).and_then(|v|v.as_f64()).ok_or_else(||mlua::Error::external("Contact float index out of range"))?;
                    return Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Number(value)]));
                }
                if index != 0 || found.value.is_array() {
                    return Err(mlua::Error::external(
                        "Native indexed float-property dispatch is not connected",
                    ));
                }
                if let Some(value) = value {
                    self.set_entity_float_property(entity, &property, value)
                        .map_err(mlua::Error::external)?;
                    Ok(mlua::MultiValue::new())
                } else {
                    let value = match property.as_str() {
                        "CounterStart" | "PaintThreshold" | "ThinnerThreshold" => {
                            if component.class_name != "PaintThinnerCounterComponent" {
                                return Err(mlua::Error::external(format!("{property} getter has an unsupported component owner")));
                            }
                            let counter=self.paint_counters.get(&entity).ok_or_else(||mlua::Error::external("Counter getter has no initialized runtime"))?;
                            match property.as_str() {"PaintThreshold"=>counter.paint_threshold,"ThinnerThreshold"=>counter.thinner_threshold,_=>counter.value}
                        }
                        "Lifetime" => {
                            self.runtime_lifetimes
                                .get(&entity)
                                .ok_or_else(|| {
                                    mlua::Error::external(
                                        "Lifetime getter has no initialized runtime",
                                    )
                                })?
                                .timer
                                .remaining
                        }
                        "Max Speed" => {
                            if component.class_name != "RotatorComponent"
                                || !self
                                    .rotators
                                    .contains_key(&(entity, component.link_id.to_u32()))
                            {
                                return Err(mlua::Error::external(
                                    "Max Speed getter lacks a supported initialized rotator",
                                ));
                            }
                            found
                                .value
                                .as_f64()
                                .ok_or_else(|| mlua::Error::external("Invalid rotator float"))?
                                as f32
                        }
                        "NegativeThreshold" | "PositiveThreshold" | "Wander Radius" | "BranchPathChance" | "PatrolSpeed" => {
                            let owner = match property.as_str() {
                                "Wander Radius" => "AIBrainComponent",
                                "BranchPathChance" => "SplineKnotComponent",
                                "PatrolSpeed" => "BasicAIControllerComponent",
                                _ => "MoodComponent",
                            };
                            if component.class_name!=owner {
                                return Err(mlua::Error::external("Stored scalar has an unsupported owner"));
                            }
                            found.value.as_f64().ok_or_else(||mlua::Error::external("Invalid stored float"))? as f32
                        }
                        _ => {
                            return Err(mlua::Error::external(format!(
                                "Native float getter is not connected for {property}"
                            )))
                        }
                    };
                    Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Number(
                        f64::from(value),
                    )]))
                }
            }
            "GetChildEntityByName" => {
                if args.len() != 2 {
                    return Err(mlua::Error::external(
                        "GetChildEntityByName requires owner and child name",
                    ));
                }
                let name = lua
                    .coerce_string(args[1].clone())?
                    .ok_or_else(|| mlua::Error::external("Child name is not a string"))?;
                let name = name.to_str()?;
                let found = match self.scripts.resolve_world_argument(&args[0])? {
                    Some(owner) => self
                        .child_entity_by_name(owner, &name)
                        .map_err(mlua::Error::external)?,
                    None => None,
                };
                Ok(mlua::MultiValue::from_vec(vec![match found {
                    Some(index) => self.scripts.entity_value(index)?,
                    None => mlua::Value::Integer(0),
                }]))
            }
            "GetRelativePrefabEntity" => {
                if args.len() != 2 {
                    return Err(mlua::Error::external("Relative prefab lookup requires owner and suffix; native argument diagnostics are not implemented"));
                }
                let suffix = lua.coerce_string(args[1].clone())?.ok_or_else(|| {
                    mlua::Error::external("Relative prefab suffix is not a string")
                })?;
                let suffix = suffix.to_str()?.split('\0').next().unwrap_or("").to_owned();
                let result = match self.scripts.resolve_world_argument(&args[0])? {
                    Some(owner) => self
                        .relative_prefab_entity(owner, &suffix)
                        .map_err(mlua::Error::external)?,
                    None => None,
                };
                Ok(mlua::MultiValue::from_vec(vec![match result {
                    Some(index) => self.scripts.entity_value(index)?,
                    None => mlua::Value::Integer(0),
                }]))
            }
            "StartFadeOut" | "StartFadeIn" | "StartFadeInNoHUD" => {
                if (name == "StartFadeOut" && args.len() != 1)
                    || (name != "StartFadeOut" && args.len() > 1)
                {
                    return Err(mlua::Error::external(
                        "Unsupported fade callback argument diagnostics",
                    ));
                }
                let duration = if args.is_empty() {
                    None
                } else {
                    let value = lua
                        .coerce_number(args[0].clone())?
                        .ok_or_else(|| mlua::Error::external("Fade duration is not numeric"))?
                        as f32;
                    if !value.is_finite() {
                        return Err(mlua::Error::external("Non-finite fade duration"));
                    }
                    if value >= 0. {
                        Some(value)
                    } else {
                        None
                    }
                };
                if name == "StartFadeOut" {
                    let duration = match duration {
                        Some(value) => value,
                        None => self
                            .runtime_settings
                            .get("FullScreenFadeTime")
                            .ok_or_else(|| {
                                mlua::Error::external("FullScreenFadeTime has no imported value")
                            })?
                            .parse::<f32>()
                            .map_err(|_| mlua::Error::external("Invalid FullScreenFadeTime"))?,
                    };
                    self.screen_fade
                        .start_out(duration)
                        .map_err(mlua::Error::external)?;
                } else {
                    self.screen_fade
                        .start_in_mode(duration, name == "StartFadeInNoHUD")
                        .map_err(mlua::Error::external)?;
                }
                Ok(mlua::MultiValue::new())
            }
            "StartTrackingCurrentLevelForScoring" | "EndTrackingCurrentLevelForScoring" => {
                if args.len() != 1 {
                    return Err(mlua::Error::external(format!(
                        "{name} requires one scoring-scope name"
                    )));
                }
                let scope = lua
                    .coerce_string(args[0].clone())?
                    .ok_or_else(|| mlua::Error::external("Scoring-scope name is not a string"))?;
                let scope = scope
                    .to_str()?
                    .split('\0')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_owned();
                if scope.is_empty() {
                    return Err(mlua::Error::external("Scoring-scope name is empty"));
                }
                if name == "StartTrackingCurrentLevelForScoring" {
                    self.playstyle_tracking.insert(scope);
                } else {
                    self.playstyle_tracking.remove(&scope);
                }
                let limitation = "Level scoring-scope start/end calls are accepted so they do not cancel authored sequences; score events, combo evaluation and notifications remain incomplete.";
                if !self
                    .report
                    .unsupported
                    .iter()
                    .any(|entry| entry == limitation)
                {
                    self.report.unsupported.push(limitation.into());
                }
                Ok(mlua::MultiValue::new())
            }
            "HasCollectible" | "GiveCollectible" | "IncrementCollectibleCount" => {
                if args.len() != 1 {
                    return Err(mlua::Error::external(format!(
                        "{name} requires one name; native argument diagnostics are not connected"
                    )));
                }
                let text = lua.coerce_string(args[0].clone())?;
                let key = text
                    .map(|s| {
                        s.to_str()
                            .map(|s| s.split('\0').next().unwrap_or("").to_owned())
                    })
                    .transpose()?
                    .unwrap_or_default();
                if name == "HasCollectible" {
                    return Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Boolean(
                        self.collectibles.has(&key),
                    )]));
                }
                let before = self.collectibles.notifications.len();
                if name == "GiveCollectible" {
                    self.collectibles.set_owned(&key, true, true, false)
                } else {
                    self.collectibles.increment(&key)
                }
                .map_err(mlua::Error::external)?;
                if self.collectibles.notifications.len() != before
                    && self
                        .collectibles
                        .notifications
                        .last()
                        .is_some_and(|n| n.route == super::collectibles::Route::UiEventManager)
                {
                    self.ui_notifications
                        .push(self.collectibles.notifications.pop().unwrap());
                }
                Ok(mlua::MultiValue::new())
            }
            "SetUnlockedExtra" => {
                if args.is_empty() {
                    return Err(mlua::Error::external("SetUnlockedExtra requires a name; native argument diagnostics are not connected"));
                }
                let notify = if args.len() > 1 {
                    super::actor_scripts::native_boolean(lua, args[args.len() - 1].clone())?
                } else {
                    true
                };
                // Native11328c..1132f8 pops an optional boolean, then a string;
                // earlier arguments are ignored, and only the grant trims spaces.
                let value = &args[if args.len() > 1 { args.len() - 2 } else { 0 }];
                let text = lua.coerce_string(value.clone())?;
                let key = text
                    .map(|s| {
                        s.to_str().map(|s| {
                            s.split('\0')
                                .next()
                                .unwrap_or("")
                                .trim_matches(' ')
                                .to_owned()
                        })
                    })
                    .transpose()?
                    .unwrap_or_default();
                if let Some(extra) = self.extras.unlock(&key) {
                    let (name, kind) = (extra.name.clone(), extra.kind.clone());
                    if self.globals.set(&name, 1) {
                        let pending="Global values are connected; Apprentice/quest global-change notification dispatch and save restoration remain incomplete.";
                        if !self.report.unsupported.iter().any(|s| s == pending) {
                            self.report.unsupported.push(pending.into());
                        }
                    }
                    if notify {
                        let (event, icon) = if kind == "Film" || kind == "Cartoon" {
                            (
                                "UI_NewFilmReceived",
                                r"HUDElements\Icons\ui_icon_unlockable_film_tex.nif",
                            )
                        } else {
                            (
                                "UI_ExtraContentUnlocked",
                                r"HUDElements\Icons\ui_icon_unlockable_concept_art_tex.nif",
                            )
                        };
                        self.ui_notifications.push(
                            super::collectibles::Notification::new(
                                super::collectibles::Route::UiEventManager,
                                event,
                                &name,
                                icon,
                            )
                            .map_err(mlua::Error::external)?,
                        );
                    }
                }
                Ok(mlua::MultiValue::new())
            }
            "PlayerHasUnlockedExtra" => {
                if args.len() != 1 {
                    return Err(mlua::Error::external("PlayerHasUnlockedExtra requires one name; native argument diagnostics are not connected"));
                }
                let name = lua.coerce_string(args[0].clone())?;
                let found = if let Some(name) = name {
                    self.extras
                        .has(name.to_str()?.split('\0').next().unwrap_or(""))
                } else {
                    self.extras.has("")
                };
                Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Boolean(
                    found,
                )]))
            }
            "GetGlobal" | "SetGlobal" => {
                let expected = if name == "GetGlobal" {
                    args.len() == 1
                } else {
                    matches!(args.len(), 2 | 3)
                };
                if !expected {
                    return Err(mlua::Error::external("Invalid global callback argument count; native diagnostics are not implemented"));
                }
                let key = lua
                    .coerce_string(args[0].clone())?
                    .ok_or_else(|| mlua::Error::external("Global name requires a native string"))?;
                let key = key.to_str()?.split('\0').next().unwrap_or("").to_owned();
                if name == "GetGlobal" {
                    return Ok(mlua::MultiValue::from_vec(vec![self
                        .globals
                        .get(&key)
                        .map_or(mlua::Value::Nil, |value| {
                            mlua::Value::Integer(value as mlua::Integer)
                        })]));
                }
                let number = match &args[1] {
                    mlua::Value::Boolean(value) => {
                        if *value {
                            1.
                        } else {
                            0.
                        }
                    }
                    value => lua.coerce_number(value.clone())?.ok_or_else(|| {
                        mlua::Error::external("Unsupported global integer conversion")
                    })?,
                };
                if !number.is_finite()
                    || number.trunc() < i32::MIN as f64
                    || number.trunc() > i32::MAX as f64
                {
                    return Err(mlua::Error::external(
                        "Out-of-range global integer conversion requires native fctiwz handling",
                    ));
                }
                let value = number as i32;
                let silent = if args.len() == 3 {
                    super::actor_scripts::native_boolean(lua, args[2].clone())?
                } else {
                    false
                };
                if !self.globals.set(&key, value) {
                    return Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Nil]));
                }
                if !silent {
                    let limitation="Global values are connected; Apprentice/quest global-change notification dispatch and save restoration remain incomplete.";
                    if !self
                        .report
                        .unsupported
                        .iter()
                        .any(|message| message == limitation)
                    {
                        self.report.unsupported.push(limitation.into());
                    }
                }
                Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Integer(
                    value as mlua::Integer,
                )]))
            }
            "ActivateScene" => {
                if args.len() != 1 {
                    return Err(mlua::Error::external("ActivateScene requires one scene name; native argument diagnostics are not connected"));
                }
                let name = lua
                    .coerce_string(args[0].clone())?
                    .ok_or_else(|| mlua::Error::external("Invalid scene name"))?;
                let name = name.to_str()?;
                if self.scene_requests.request(&name) == super::scene_loading::Admission::Full {
                    self.report.warnings.push(
                        "ActivateScene request rejected by the original 32-slot queue capacity"
                            .into(),
                    );
                }
                Ok(mlua::MultiValue::new())
            }
            "EnableGuardianSystem" | "DisableGuardianSystem" => {
                // REL15e20/15e8c optionally resolves argument one, but never
                // uses the result when calling the singleton at BSS+7c.
                if let Some(argument) = args.front() {
                    let _ = self.scripts.resolve_world_argument(argument)?;
                }
                self.set_guardian_enabled(name == "EnableGuardianSystem")
                    .map_err(mlua::Error::external)?;
                Ok(mlua::MultiValue::new())
            }
            "RestrictCutSceneInput" | "UnrestrictCutSceneInput" => {
                if !args.is_empty() {
                    return Err(mlua::Error::external("Cutscene input callbacks take no arguments; native argument diagnostic handling is not implemented"));
                }
                if name == "RestrictCutSceneInput" {
                    self.cutscene_input.restrict();
                } else {
                    self.cutscene_input.unrestrict();
                }
                self.script_player.script_input_restricted = self.cutscene_input.restricted;
                let limitation="Cutscene player input restrictions are connected. Original menu/input-context stack, cursor/modal ownership and native scheduler order remain unvalidated.";
                if !self
                    .report
                    .unsupported
                    .iter()
                    .any(|message| message == limitation)
                {
                    self.report.unsupported.push(limitation.into());
                }
                Ok(mlua::MultiValue::new())
            }
            "SetShadowScale" => {
                if args.len() != 2 {
                    return Err(mlua::Error::external("SetShadowScale requires entity and vector4"));
                }
                let mlua::Value::UserData(vector) = &args[1] else {
                    return Err(mlua::Error::external("Shadow scale requires vector4 userdata"));
                };
                let scale = vector.borrow::<super::lua_vector::Vector4>()?.0;
                let scale: [f32; 3] = scale[..3].try_into().unwrap();
                if !scale.iter().all(|v| v.is_finite()) {
                    return Err(mlua::Error::external("Nonfinite shadow scale"));
                }
                if let Some(owner) = self.scripts.resolve_native_entity_argument(lua, args[0].clone())?
                    .filter(|owner| !self.removed_entities.contains(owner)) {
                    self.shadow_volumes.set_scale(owner, scale);
                }
                Ok(mlua::MultiValue::new())
            }
            "RestoreShadowScale" => {
                let mut args = args;
                if let Some(owner) = self.scripts.resolve_native_entity_argument(
                    lua, args.pop_back().unwrap_or(mlua::Value::Nil),
                )?.filter(|owner| !self.removed_entities.contains(owner)) {
                    self.shadow_volumes.restore_scale(owner);
                }
                Ok(mlua::MultiValue::new())
            }
            "SetHudFor2D" => {
                if args.len()!=2 {
                    return Err(mlua::Error::external("SetHudFor2D requires owner and mode"));
                }
                // REL1f94fc consumes an owner but does not use it. 1f7978
                // writes only input manager+89; it does not show/hide HUD.
                self.input_presentation.mode_2d=u8::from(super::actor_scripts::native_boolean(lua,args[1].clone())?);
                // Demo integration follows the original Enable2DSetup/Disable2DSetup
                // call convention. This is NOT a reinterpretation of native byte89.
                if self.demo_story.enabled {
                    self.side_scroller.enabled=self.input_presentation.mode_2d==0;
                    self.side_scroller.plane=None;
                    if !self.side_scroller.enabled {self.side_scroller.owner=None;}
                }
                Ok(mlua::MultiValue::new())
            }
            "GetConfigVariableFloat" | "GetConfigVariableBool" | "GetConfigVariableString" => {
                if args.len() != 1 {
                    return Err(mlua::Error::external(
                        "Configuration getter requires a variable name",
                    ));
                }
                let key = lua
                    .coerce_string(args[0].clone())?
                    .ok_or_else(|| mlua::Error::external("Invalid configuration variable name"))?;
                let key = key.to_str()?.split('\0').next().unwrap_or("").to_owned();
                if name == "GetConfigVariableString" {
                    return Ok(mlua::MultiValue::from_vec(vec![mlua::Value::String(
                        lua.create_string(self.runtime_settings.get(&key).map(String::as_str).unwrap_or(""))?
                    )]));
                }
                if name == "GetConfigVariableBool" {
                    let value = super::config_value::boolean(self.runtime_settings.get(&key).map(String::as_str))
                        .map_err(mlua::Error::external)?;
                    return Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Boolean(value)]));
                }
                let value=self.runtime_settings.get(&key).ok_or_else(||mlua::Error::external(format!("Configuration variable {key} has no imported value; compiled defaults are not connected")))?;
                let value: f32 = value.parse().map_err(|_| {
                    mlua::Error::external(format!(
                        "Configuration variable {key} is not a supported float"
                    ))
                })?;
                if !value.is_finite() {
                    return Err(mlua::Error::external("Non-finite configuration float"));
                }
                Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Number(
                    value as f64,
                )]))
            }
            "StimulusEntityWithAliasCalculateDirectionInRadius"
            | "StimulusEntityWithAliasInEntityDirectionInRadius" => {
                let entity_direction = name == "StimulusEntityWithAliasInEntityDirectionInRadius";
                if args.len() != if entity_direction { 7 } else { 6 } {
                    return Err(mlua::Error::external("Directional stimulus requires owner, type, amount, radius, target, optional direction entity, and alias"));
                }
                let owner = self.scripts.resolve_world_argument(&args[0])?;
                let target = self.scripts.resolve_world_argument(&args[4])?;
                let (Some(owner), Some(target)) = (owner, target) else {
                    return Ok(mlua::MultiValue::new());
                };
                let kind = lua
                    .coerce_integer(args[1].clone())?
                    .ok_or_else(|| mlua::Error::external("Invalid directional stimulus type"))?;
                if !(0..15).contains(&kind) {
                    return Err(mlua::Error::external(
                        "Unsupported directional stimulus type",
                    ));
                }
                let number = |value: mlua::Value| -> mlua::Result<f32> {
                    let value = lua.coerce_number(value)?.ok_or_else(|| {
                        mlua::Error::external("Invalid directional stimulus number")
                    })? as f32;
                    if !value.is_finite() {
                        return Err(mlua::Error::external(
                            "Non-finite directional stimulus number",
                        ));
                    }
                    Ok(value)
                };
                let amount = number(args[2].clone())?;
                let radius = number(args[3].clone())?;
                let tag = lua
                    .coerce_string(args[if entity_direction { 6 } else { 5 }].clone())?
                    .ok_or_else(|| mlua::Error::external("Invalid stimulus alias"))?
                    .to_str()?
                    .split('\0')
                    .next()
                    .unwrap_or("")
                    .to_owned();
                let position = |entity: usize| {
                    std::array::from_fn(|i| self.entities[entity].transform[12 + i])
                };
                let target_position = position(target);
                // REL28a134..28a480 uses target -> direction-entity for BOTH
                // the normalized vector and the strict radius comparison.
                // The emitting owner's position is not that vector's origin.
                let (from, to) = if entity_direction {
                    let Some(direction_entity) = self.scripts.resolve_world_argument(&args[5])?
                    else {
                        return Ok(mlua::MultiValue::new());
                    };
                    (target_position, position(direction_entity))
                } else {
                    (position(owner), target_position)
                };
                let Some(direction) = super::script_stimulus::direction(from, to, radius)
                    .map_err(mlua::Error::external)?
                else {
                    return Ok(mlua::MultiValue::new());
                };
                if kind == 3
                    && !self.demo_story.enabled
                    && self.entities[target].components.iter().any(|component| {
                        component.class_name == "PhysicsRigidBodyCharacterComponent"
                    })
                {
                    return Err(mlua::Error::external(
                        "Character knockback action/motion receiver is not connected",
                    ));
                }
                let emission = super::projectile_stimulus::Emission {
                    stimulus: kind as u32,
                    amount,
                    position: target_position,
                    direction,
                    target: Some(target as u64),
                    owner: Some(owner as u64),
                    tag,
                    delivery: super::projectile_stimulus::Delivery::Target,
                };
                self.receive_combat(target, std::slice::from_ref(&emission))
                    .map_err(mlua::Error::external)?;
                Ok(mlua::MultiValue::new())
            }
            "StimulusSphereWithAlias" => {
                if args.len() != 6 {
                    return Err(mlua::Error::external("StimulusSphereWithAlias needs owner, type, amount, position, radius and alias"));
                }
                let owner = self.scripts.resolve_world_argument(&args[0])?;
                let stimulus = lua
                    .coerce_integer(args[1].clone())?
                    .ok_or_else(|| mlua::Error::external("Invalid stimulus type"))?;
                if !(0..15).contains(&stimulus) {
                    return Err(mlua::Error::external("Unsupported stimulus type"));
                }
                let number = |value: mlua::Value| -> mlua::Result<f32> {
                    let value = lua
                        .coerce_number(value)?
                        .ok_or_else(|| mlua::Error::external("Invalid stimulus number"))?
                        as f32;
                    if !value.is_finite() {
                        return Err(mlua::Error::external("Non-finite stimulus number"));
                    }
                    Ok(value)
                };
                let amount = number(args[2].clone())?;
                let radius = number(args[4].clone())?;
                if radius < 0. {
                    return Err(mlua::Error::external("Negative stimulus radius"));
                }
                let mlua::Value::UserData(position) = &args[3] else {
                    return Err(mlua::Error::external(
                        "Stimulus position must be original vector4 userdata",
                    ));
                };
                let position = position.borrow::<super::lua_vector::Vector4>()?.0;
                let tag = lua
                    .coerce_string(args[5].clone())?
                    .ok_or_else(|| mlua::Error::external("Invalid stimulus alias"))?
                    .to_str()?
                    .split('\0')
                    .next()
                    .unwrap_or("")
                    .to_owned();
                let position = [position[0], position[1], position[2]];
                if !position.iter().all(|value| value.is_finite()) {
                    return Err(mlua::Error::external("Non-finite stimulus position"));
                }
                // REL288478..4cc selects direct delivery at radius zero.
                // This sphere wrapper supplies no direct target (2899c4),
                // so it must not turn zero radius into a point-overlap attack.
                if radius == 0. {
                    return Ok(mlua::MultiValue::new());
                }
                // REL2898fc..2899e4 supplies zero direction to the shared
                // sphere emitter, retaining the owner's opaque identity/tag.
                let recipients = self
                    .area_stimulus_recipients(position, radius, owner)
                    .map_err(mlua::Error::external)?;
                for recipient in recipients {
                    let emission = super::projectile_stimulus::Emission {
                        stimulus: stimulus as u32,
                        amount,
                        position,
                        direction: [0.; 3],
                        target: Some(recipient as u64),
                        owner: owner.map(|owner| owner as u64),
                        tag: tag.clone(),
                        delivery: super::projectile_stimulus::Delivery::Target,
                    };
                    self.receive_combat(recipient, std::slice::from_ref(&emission))
                        .map_err(mlua::Error::external)?;
                }
                Ok(mlua::MultiValue::new())
            }
            "GetPlaythrough" => {
                if !args.is_empty() {
                    return Err(mlua::Error::external(
                        "GetPlaythrough argument-count diagnostics are not implemented",
                    ));
                }
                Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Integer(
                    self.playthrough.value as mlua::Integer,
                )]))
            }
            "SetPlaythrough" => {
                if args.len() != 1 {
                    return Err(mlua::Error::external("SetPlaythrough requires one argument; native count diagnostics are not implemented"));
                }
                self.playthrough.value =
                    super::score_state::integer_argument(lua, args[0].clone())?;
                Ok(mlua::MultiValue::new())
            }
            "DisableAbility_Player" | "EnableAbility_Player" => {
                let enabling = name == "EnableAbility_Player";
                let expected = if enabling { 3 } else { 2 };
                if args.len() != expected {
                    return Err(mlua::Error::external(format!("{name} requires {expected} arguments; native diagnostic continuation is not implemented")));
                }
                let ability = lua
                    .coerce_string(args[1].clone())?
                    .ok_or_else(|| mlua::Error::external("Ability name must be a native string"))?;
                let ability = ability
                    .to_str()?
                    .split('\0')
                    .next()
                    .unwrap_or("")
                    .to_owned();
                let level = if enabling {
                    let value = lua.coerce_number(args[2].clone())?.ok_or_else(|| {
                        mlua::Error::external(
                            "Ability level conversion is not implemented for this argument",
                        )
                    })?;
                    let value = value.trunc();
                    if !value.is_finite() || value < i32::MIN as f64 || value > i32::MAX as f64 {
                        return Err(mlua::Error::external(
                            "Out-of-range native ability level conversion is not implemented",
                        ));
                    }
                    value as i32
                } else {
                    0
                };
                let Some(entity) = self.scripts.resolve_world_argument(&args[0])? else {
                    return Ok(mlua::MultiValue::new());
                };
                if entity != self.player_entity {
                    if self.entities[entity]
                        .components
                        .iter()
                        .any(|c| c.class_name == "MickeyPlayerControllerComponent")
                    {
                        return Err(mlua::Error::external(
                            "Ability owner is another unimplemented Mickey controller",
                        ));
                    }
                    return Ok(mlua::MultiValue::new());
                }
                if enabling {
                    self.player_grant_ability(&ability, "All", level)
                        .map_err(mlua::Error::external)?;
                } else {
                    if !["Blast", "Brush", "Spin"].contains(&ability.as_str()) {
                        return Err(mlua::Error::external(format!("Disable {ability} requires reconstruction of its original action/inventory receiver")));
                    }
                    self.player_abilities
                        .get_mut(&ability)
                        .ok_or_else(|| {
                            mlua::Error::external(format!(
                                "Ability receiver {ability} is not reconstructed"
                            ))
                        })?
                        .revoke_all();
                }
                Ok(mlua::MultiValue::new())
            }
            "MoveToEntity" | "TeleportToEntity" => {
                let Some(request) = self.scripts.move_to_entity_request(lua, args)? else {
                    // Original callback returns no values without moving when
                    // either resolved entity is absent.
                    return Ok(mlua::MultiValue::new());
                };
                self.move_to_entity(request)
                    .map_err(mlua::Error::external)?;
                Ok(mlua::MultiValue::new())
            }
            "IsValidHandle" | "GetFacing" | "GetUp" | "GetLeft" => {
                if args.len() != 1 {
                    return Err(mlua::Error::external(format!("{name} requires one entity")));
                }
                // EM1 REL26a008/26bb34 resolve through the entity manager,
                // including name arguments, rather than treating numbers as indices.
                let owner = self.scripts.resolve_native_entity_argument(lua, args[0].clone())?
                    .filter(|index| !self.removed_entities.contains(index));
                if name == "IsValidHandle" {
                    return Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Boolean(owner.is_some())]));
                }
                let Some(owner) = owner else { return Ok(mlua::MultiValue::new()); };
                let entity = self.entities.get(owner).ok_or_else(|| mlua::Error::external("Facing owner is missing"))?;
                // REL297110 selects axis 2 from the native row-major world
                // rotation. Preserve its magnitude; render scale is separate.
                let rotation = entity.runtime_rotation;
                let axis=match name {"GetLeft"=>0,"GetUp"=>1,_=>2};
                Ok(mlua::MultiValue::from_vec(vec![mlua::Value::UserData(
                    lua.create_userdata(super::lua_vector::Vector4([
                        rotation[axis], rotation[3+axis], rotation[6+axis], 0.0,
                    ]))?,
                )]))
            }
            "EnableMovementInput" | "DisableMovementInput" => self.movement_input_lua(lua,args,name=="EnableMovementInput"),
            "GetDistanceBetweenEntities" => self.entity_distance_lua(lua,args),
            "PaintThinnerCounterReset" => self.counter_reset_lua(lua,args),
            "GetTicketCount" => self.ticket_count_lua(args),
            "SetStayInCombatIdle" => self.set_stay_combat_idle_lua(lua,args),
            "PlayerHasSketchAbilities"|"PlayerHasSketchAbilitiesSD"|"PlayerHasPaintThinnerAbilities"|"PlayerHasPaintThinnerAbilitiesSD" => self.player_sketch_availability_lua(lua,name,args),
            "RelayEnable"|"RelayDisable"|"RelayToggle"|"RelayTrigger" => self.relay_lua(lua,name,args),
            "SetVulnerability" => self.set_vulnerability_lua(lua,args),
            "ResetTicketCount" => self.reset_ticket_count_lua(args),
            "GetSketchCount" => self.sketch_count_lua(lua,args),
            "PaintThinnerCounter_GetStatus" | "PaintThinnerCounter_GetPaintedPercentage" => self.counter_query_lua(lua,name,args),
            "SetFacing" => self.facing_lua(lua,args),
            "GetPosition" => {
                if args.len() != 1 {
                    return Err(mlua::Error::external(
                        "GetPosition argument-count diagnostic handling is not implemented",
                    ));
                }
                let entity = self.scripts.resolve_world_argument(&args[0])?;
                let position = match entity {
                    Some(index) => {
                        let entity = self.entities.get(index).ok_or_else(|| {
                            mlua::Error::external("Runtime entity identity is missing")
                        })?;
                        super::native_hk_transform::entity_position_vector([
                            entity.transform[12],
                            entity.transform[13],
                            entity.transform[14],
                        ])
                    }
                    None => [0.0; 4],
                };
                Ok(mlua::MultiValue::from_vec(vec![mlua::Value::UserData(
                    lua.create_userdata(super::lua_vector::Vector4(position))?,
                )]))
            }
            _ => Err(mlua::Error::external(format!(
                "Native world API not implemented: {name}"
            ))),
        }
    }
    /// Keep one Lua scheduler while admitting a synchronous mutable world
    /// borrow. Restore task ownership before propagating errors or unwinds.
    pub fn with_script_world_host<R>(
        &mut self,
        mut host: impl FnMut(
            &mut Self,
            &mlua::Lua,
            &str,
            mlua::MultiValue,
        ) -> mlua::Result<mlua::MultiValue>,
        run: impl FnOnce(&mut super::actor_scripts::Scripts) -> Result<R>,
    ) -> Result<R> {
        let mut scripts = self.scripts.detach_scheduler()?;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            scripts.with_world_host(|lua, name, args| host(self, lua, name, args), run)
        }));
        self.scripts.restore_scheduler(scripts)?;
        match outcome {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
    /// Decoded event 0x34 addressed to Mickey. The return value requests the
    /// native clear/removal lifecycle; it does not pretend that lifecycle ran.
    /// Global scheduling, component traversal and event 0x25 remain separate.
    pub fn player_pickup_event(&mut self, event: super::pickup::CollectionEvent) -> Result<bool> {
        ensure!(
            event.recipient == self.player_entity,
            "Pickup recipient is not the player; InventoryComponent receiver needs reconstruction"
        );
        let commands = self
            .pickup_receivers
            .get(&event.pickup)
            .context("Pickup event payload has no PlayerPickupComponent")?
            .commands
            .clone();
        super::pickup::player_commands(&commands, |command| {
            if self.player_inventory_command(command)? {
                return Ok(true);
            }
            self.player_give_command(command)
        })
    }
    /// Run imported pickup volumes against the committed player pose on every
    /// gameplay tick. The volume is the authored phantom AABB (or its imported
    /// shape bounds); the small player extents account for the character hull.
    /// This is intentionally shared by ordinary play and the demo harness.
    /// Commit collection-delay admission once per component update. Overlap
    /// queries may run more than once per frame and must not advance this clock.
    /// REL3897d8 exits even on the update which clears the delayed byte.
    fn advance_pickup_collection_gates(&mut self, delta: f32) -> Result<()> {
        ensure!(delta.is_finite() && delta >= 0., "Invalid pickup component delta");
        self.pickup_collection_blocked.clear();
        let owners = self.pickup_update_states.keys().copied().collect::<Vec<_>>();
        for owner in owners {
            if self.removed_entities.contains(&owner) || !self.entity_scene_active(owner) {
                self.pickup_collection_blocked.insert(owner);
                continue;
            }
            let component = self.entities[owner].components.iter()
                .position(|c| c.class_name == "PlayerPickupComponent")
                .context("Pickup update lost its component")?;
            let inactive = self.entities[owner].components[component].properties.iter()
                .find(|p| p.name == "Inactive")
                .is_some_and(|p| p.value.as_bool().unwrap_or_else(|| p.value.as_i64().unwrap_or(0) != 0));
            let disabled = self.component_control.disabled.get(&owner)
                .and_then(|components| components.get(&component)).copied().unwrap_or(false);
            if inactive || disabled {
                self.pickup_collection_blocked.insert(owner);
                continue;
            }
            if let Err(error)=self.refresh_pickup_bob_baseline(owner) {
                let message=format!("Pickup baseline {}: {error:#}",self.entities[owner].name);
                if !self.report.unsupported.contains(&message) {self.report.unsupported.push(message);}
                self.pickup_collection_blocked.insert(owner);
                continue;
            }
            if !self.pickup_update_states.get_mut(&owner).unwrap().gate.advance(0, 0, delta) {
                self.pickup_collection_blocked.insert(owner);
            }
        }
        Ok(())
    }
    pub(super) fn advance_pickup_overlaps(&mut self, position: [f32; 3]) -> Result<()> {
        let owners = self.pickup_receivers.keys().copied().collect::<Vec<_>>();
        for owner in owners {
            if owner >= self.entities.len()
                || self.removed_entities.contains(&owner)
                || self.hidden_entities.contains(&owner)
                || !self.entity_scene_active(owner)
            {
                continue;
            }
            let inactive = self.entities[owner]
                .components
                .iter()
                .find(|component| component.class_name == "PlayerPickupComponent")
                .and_then(|component| {
                    component
                        .properties
                        .iter()
                        .find(|property| property.name == "Inactive")
                })
                .is_some_and(|property| {
                    property
                        .value
                        .as_bool()
                        .unwrap_or_else(|| property.value.as_i64().unwrap_or(0) != 0)
                });
            let inside = if inactive {
                false
            } else {
                self.demo_contains(owner, position)?.unwrap_or_else(|| {
                    let p = &self.entities[owner].transform[12..15];
                    (0..3)
                        .map(|axis| (p[axis] - position[axis]).powi(2))
                        .sum::<f32>()
                        <= 1.4 * 1.4
                })
            };
            // Maintain the same single-collector receiver semantics as the
            // native overlap callback, including clearing only this player.
            self.pickup_overlap(owner, Some(self.player_entity), inside)?;
            if !inside || self.pickup_collection_blocked.contains(&owner)
                || self.pickup_update_states.get(&owner).is_some_and(|state| state.gate.delayed)
            {
                continue;
            }
            let event = self.pickup_receivers.get(&owner).and_then(|receiver| {
                receiver.collection_event(owner, |entity| entity < self.entities.len())
            });
            let Some(event) = event else {
                continue;
            };
            if !self.player_pickup_event(event)? {
                continue;
            }
            if let Some(receiver) = self.pickup_receivers.get_mut(&owner) {
                receiver.clear();
            }
            self.emit_entity_output(owner, "OnCollected", self.player_entity);
            let deliveries = self.outputs.ready_for_owner(self.animation_time, owner);
            self.dispatch_actor_deliveries(deliveries);
            if self.demo_story.enabled {
                self.demo_story.triggers.pickups += 1;
            }
            self.retire_entity(owner)?;
        }
        Ok(())
    }
    /// Explicit playback probe using the original pickup's authored command
    /// list. It does not generate overlaps, consume or delete the pickup.
    pub fn replay_pickup_commands(&mut self, name: &str) -> Result<()> {
        let index = self
            .scripts
            .resolve_name(name)
            .context("Pickup command probe entity not found")?;
        let commands = self
            .pickup_receivers
            .get(&index)
            .context("Probe entity has no pickup receiver")?
            .commands
            .clone();
        self.diagnostic_pickup_commands = true;
        for command in &commands {
            ensure!(
                self.player_give_command(command)?,
                "Pickup probe command not handled: {command}"
            );
        }
        self.report.geometry.push(serde_json::json!({"kind":"diagnostic_pickup_command_replay",
            "entity":self.entities[index].name,"commands":commands,"ownership":self.player_abilities,
            "hud_requests":self.ability_hud_requests,"physics_overlap":false,"pickup_lifecycle":false}));
        self.report.unsupported.push("DIAGNOSTIC pickup command replay: no physics overlap, pickup lifecycle, playable ownership gates or HUD delivery implied.".into());
        Ok(())
    }
    /// Native Give command effect for reconstructed base ability ownership.
    /// This is not pickup lifecycle dispatch or completed action initialization.
    pub fn player_give_command(&mut self, command: &str) -> Result<bool> {
        let Some(give) = super::ability_ownership::parse(command) else {
            return Ok(false);
        };
        match give {
            super::ability_ownership::Give::Ability {
                name,
                selector,
                level,
            } => self.player_grant_ability(name, selector, level)?,
            super::ability_ownership::Give::Health { amount } => {
                let authored = self
                    .authored_health
                    .get(&self.player_entity)
                    .context("Health grant has no player HealthComponent")?;
                let runtime = self
                    .runtime_health
                    .get_mut(&self.player_entity)
                    .context("Health grant has no live player health")?;
                // Use the recovered signed-add/clamp admission. No reward can
                // resurrect a dead recipient through pickup collection.
                if super::health::alive(runtime.health, u8::from(authored.infinite)) {
                    if let Some(next) =
                        super::health::preview_heal(runtime.health, authored.maximum, amount, 0)
                    {
                        runtime.health = next;
                        let property = self.entities[self.player_entity]
                            .components
                            .iter_mut()
                            .find(|c| c.class_name == "HealthComponent")
                            .and_then(|c| c.properties.iter_mut().find(|p| p.name == "Health"))
                            .context("Player health property missing")?;
                        property.value = serde_json::json!(next);
                    }
                }
            }
        }
        Ok(true)
    }
    /// Explicit demo initialization until native startup pickup collection is
    /// connected. Use the real Give receiver, retaining scripted disable flags.
    /// Asset loading and inspection never call this automatically.
    pub fn enable_demo_abilities(&mut self) -> Result<()> {
        for command in ["Give Brush All 1", "Give Blast All 1", "Give Spin All 1"] {
            ensure!(
                self.player_give_command(command)?,
                "Demo ability command rejected: {command}"
            );
        }
        Ok(())
    }
    fn player_grant_ability(&mut self, name: &str, selector: &str, level: i32) -> Result<()> {
        ensure!(
            ["Blast", "Brush", "Spin"].contains(&name),
            "Give {name} requires reconstruction of its original action/inventory receiver"
        );
        let ability = self.player_abilities.get_mut(name).with_context(|| {
            format!("Give {name} requires reconstruction of its original action/inventory receiver")
        })?;
        // Blast override RELa388c tests Paint for All; Thinner has its own
        // effective ownership check. UI delivery is explicit and still pending.
        let ink = if selector == "Thinner" {
            super::brush::Ink::Thinner
        } else {
            super::brush::Ink::Paint
        };
        let before = ability.enabled(ink);
        ability.give(selector, level);
        if name == "Blast" && !before && ability.enabled(ink) {
            self.ability_hud_requests
                .push(("Show", "HudElement", "AmmoBars"));
        }
        Ok(())
    }
    pub fn player_ability_enabled(&self, name: &str, ink: super::brush::Ink) -> Result<bool> {
        Ok(self
            .player_abilities
            .get(name)
            .with_context(|| format!("Ability ownership {name} is not reconstructed"))?
            .enabled(ink))
    }
    /// One registered ability's MICK record. The full checkpoint owner must
    /// retain original primary/secondary/sketch registration order.
    pub fn player_ability_save_record(&self, name: &str) -> Result<Vec<u8>> {
        let state = self
            .player_abilities
            .get(name)
            .context("Ability save owner is not reconstructed")?;
        let mut output = Vec::new();
        super::ability_persistence::append(&mut output, name, state)?;
        Ok(output)
    }
    pub fn player_ability_save_summary(&self) -> Result<Vec<serde_json::Value>> {
        let bytes = self.player_registered_ability_records()?;
        super::save_game::records(&bytes)?.into_iter().map(|record| {
            if record.name.contains(':') {Ok(serde_json::json!({"key":record.name,"mask":i32::from_be_bytes(record.data.try_into()?)}))}
            else {Ok(serde_json::json!({"key":record.name,"sketch_maximum":f32::from_be_bytes(record.data.try_into()?)}))}
        }).collect()
    }
    /// One native reader21098 visit. Reuse the real Give receiver, including
    /// derived HUD requests; a raw overwrite would lose those side effects.
    /// This alone does not restore the other player/checkpoint sections.
    pub fn restore_player_ability_record(&mut self, name: &str, payload: &[u8]) -> Result<()> {
        let records = super::save_game::records(payload)?;
        let state = self
            .player_abilities
            .get(name)
            .context("Ability restore owner is not reconstructed")?;
        if let Some(grant) = super::ability_persistence::grant(&records, name, state)? {
            if matches!(name, "Blast" | "Brush" | "Spin") {
                self.player_grant_ability(name, grant.selector, grant.level)?;
            } else {
                self.player_abilities.get_mut(name).unwrap().give(grant.selector, grant.level);
                self.report.unsupported.push(format!(
                    "Checkpoint restored {name} ownership; ability-specific grant side effects remain unverified."
                ));
            }
        }
        Ok(())
    }
    /// Original pickup listener entry: resolve the phantom owner and the other
    /// collidable through their native ownership chains before entity dispatch.
    /// Pointer-to-runtime-entity mapping is supplied by the physics world; raw
    /// collidable addresses must never be mistaken for scene entity indices.
    pub fn pickup_phantom_overlap(
        &mut self,
        phantom_owner: usize,
        other_collidable: usize,
        entered: bool,
        records: &impl super::collidable_owner::Records,
        resolve_entity: impl Fn(usize) -> Result<Option<usize>>,
    ) -> Result<()> {
        super::pickup_listener::deliver(
            phantom_owner,
            other_collidable,
            entered,
            &self.entities,
            &mut self.pickup_receivers,
            records,
            resolve_entity,
        )
    }
    /// Synchronous registered pickup-agent ingress. Requires reconstructed
    /// ownership bindings and admitted broadphase events; generates neither.
    pub fn dispatch_pickup_phantom_overlap(
        &mut self,
        phantom: usize,
        visitor: usize,
        entered: bool,
        bindings: &impl super::pickup_listener::Bindings,
    ) -> Result<bool> {
        let record = self
            .physics_objects
            .phantoms
            .get_mut(&phantom)
            .context("Unknown pickup phantom allocation")?;
        let simple = record
            .collidable
            .is_some_and(|collidable| collidable.shape.is_some());
        let has_shape = if simple {
            super::collidable_owner::Records::collidable(bindings, visitor)?.has_shape
        } else {
            false
        };
        let mut callbacks = super::pickup_listener::Callbacks {
            entities: &self.entities,
            receivers: &mut self.pickup_receivers,
            listeners: &self.physics_objects.listeners,
            bindings,
            phantom,
            user_data: &record.user_data,
        };
        let overlaps = &mut record.overlaps;
        if simple && entered {
            overlaps.simple_add_from(phantom, visitor, has_shape, &mut callbacks)
        } else if simple {
            overlaps.simple_remove_from(phantom, visitor, has_shape, &mut callbacks)
        } else if entered {
            overlaps.add_from(phantom, visitor, &mut callbacks)
        } else {
            overlaps.remove_from(phantom, visitor, &mut callbacks)
        }
    }
    /// Consume a decoded native-style overlap event. No broadphase or grants
    /// are implied here; the physics producer and command receiver are pending.
    pub fn pickup_overlap(
        &mut self,
        owner: usize,
        other: Option<usize>,
        entered: bool,
    ) -> Result<()> {
        ensure!(
            other.is_none_or(|index| index < self.entities.len()),
            "Unknown overlap entity"
        );
        let receiver = self
            .pickup_receivers
            .get_mut(&owner)
            .context("Overlap owner has no pickup receiver")?;
        let exists = |index| index < self.entities.len();
        if entered {
            receiver.enter(other, exists, |index| {
                self.entities[index]
                    .components
                    .iter()
                    .any(|component| component.class_name == "InventoryComponent")
            });
        } else {
            receiver.exit(other, exists);
        }
        Ok(())
    }
    pub fn phantom_bounds(&self, index: usize) -> Result<Option<super::phantom_bounds::Bounds>> {
        let Some(volume) = self.size_phantoms.get(&index) else {
            return Ok(None);
        };
        let entity = self
            .entities
            .get(index)
            .context("Phantom owner not loaded")?;
        volume
            .bounds(
                entity.transform[12..15].try_into().unwrap(),
                entity.runtime_rotation,
            )
            .map(Some)
    }
    /// Publish an absolute root-owner pose to rendering and installed static
    /// queries. Native MoveToEntity's component resets, phantom/controller
    /// state and parent propagation are separate required work, not no-ops.
    /// The collision cache stores static geometry in its original world frame.
    /// Reapply the entity's rigid delta when Lua or an animated socket moves it.
    pub(super) fn publish_static_collision_pose(&mut self, index: usize, position: V, rotation: [f32; 9]) -> Result<()> {
        let entity = &self.entities[index];
        if entity.collision_parts.is_empty() { return Ok(()); }
        let initial = entity.initial_physics_transform();
        let next = super::native_hk_transform::from_entity(rotation, position);
        let identity =
            super::native_hk_transform::from_entity([1., 0., 0., 0., 1., 0., 0., 0., 1.], [0.; 3]);
        let inverse = super::native_hk_transform::relative(&initial, &identity);
        let delta = super::native_hk_transform::compose(&next, &inverse);
        let quaternion = super::native_hk_transform::rotation_quaternion(&delta)?;
        let translation = delta[12..15].try_into().unwrap();
        let owners = entity
            .collision_parts
            .clone()
            .map(|part| self.report.collision_parts[part].owner)
            .collect::<Vec<_>>();
        for &owner in &owners {
            ensure!(
                self.collision.has_owner(owner),
                "Static relocation has no installed collision owner {owner}"
            );
        }
        for owner in owners {
            self.collision
                .set_owner_transform(owner, translation, quaternion)?;
        }
        Ok(())
    }

    pub fn relocate_entity(&mut self, index: usize, position: V, rotation: [f32; 9]) -> Result<()> {
        let entity = self
            .entities
            .get(index)
            .context("Relocation target is not loaded")?;
        ensure!(
            position.iter().chain(&rotation).all(|v| v.is_finite()),
            "Invalid owner pose"
        );
        ensure!(
            index != self.player_entity,
            "Player relocation requires controller-state synchronization"
        );
        if let Some(Err(error)) = self.character_physics.get(&index) {
            anyhow::bail!("Character relocation requires its body: {error}");
        }
        ensure!(
            entity.transform_parent_id()?.is_none(),
            "Parented relocation requires local/world ownership propagation"
        );
        for child in self.entities.iter().filter(|_| !self.demo_story.enabled) {
            ensure!(
                child.scene != entity.scene || child.transform_parent_id()? != Some(entity.id),
                "Owner relocation requires attached-child propagation"
            );
        }
        ensure!(
            !entity.components.iter().any(|c| matches!(
                c.class_name.as_str(),
                "PhysicsRigidBodyDynamicComponent" | "PhysicsCharacterControllerComponent"
            )),
            "Relocation requires dynamic/phantom/controller component synchronization"
        );
        let mut resource_phantom_transform = None;
        let phantom = if entity
            .components
            .iter()
            .any(|c| c.class_name == "PhysicsPhantomComponent")
            && self.resource_phantom_motion.contains_key(&index)
        {
            ensure!(
                !self.size_phantom_handles.contains_key(&index),
                "Resource phantom has inconsistent size allocation"
            );
            if let Some(handle) = self.resource_phantom_handles.get(&index) {
                let owned = self
                    .physics_objects
                    .phantoms
                    .get(handle)
                    .context("Missing resource phantom allocation")?;
                ensure!(
                    owned.world.is_none(),
                    "Admitted resource phantom relocation requires broadphase publication"
                );
                ensure!(
                    owned.overlaps.collidables.is_empty(),
                    "Detached resource phantom retains live overlaps"
                );
            }
            // REL378564 preserves existing shape rotation; detached
            // DOL802bed50 copies only the transform, not swept endpoints.
            let motion = &self.resource_phantom_motion[&index];
            resource_phantom_transform = Some(super::native_hk_transform::relocated_shape_phantom(
                &motion.transform,
                super::native_hk_transform::entity_position_vector(position),
            ));
            None
        } else if entity
            .components
            .iter()
            .any(|c| c.class_name == "PhysicsPhantomComponent")
        {
            let volume = self
                .size_phantoms
                .get(&index)
                .context("Relocation requires a supported size phantom")?;
            let handle = *self
                .size_phantom_handles
                .get(&index)
                .context("Relocation phantom has no owned runtime record")?;
            let owned = self
                .physics_objects
                .phantoms
                .get(&handle)
                .context("Relocation phantom record disappeared")?;
            ensure!(
                owned.world.is_none(),
                "Active phantom relocation requires broadphase publication"
            );
            ensure!(
                owned.overlaps.collidables.is_empty(),
                "Detached phantom retains live overlap memberships"
            );
            Some((handle, volume.padded_bounds(position, rotation)?))
        } else {
            None
        };
        ensure!(
            entity.jigsaw_asset.is_none(),
            "Jigsaw relocation requires piece-body synchronization"
        );
        self.publish_static_collision_pose(index, position, rotation)?;
        if let Some((handle, bounds)) = phantom {
            self.physics_objects
                .phantoms
                .get_mut(&handle)
                .context("Relocation phantom disappeared")?
                .bounds = Some(bounds);
        }
        if let Some(transform) = resource_phantom_transform {
            self.resource_phantom_motion
                .get_mut(&index)
                .context("Resource phantom motion disappeared")?
                .transform = transform;
        }
        if self.character_physics.contains_key(&index) {
            let next = super::native_hk_transform::from_entity(rotation, position);
            // DOL802b7bd4 -> motion virtual44 ->8014e750. Keep the
            // previous swept base time and velocities; do not stamp now.
            self.warp_character_body(index, next)?;
        }
        let entity = &mut self.entities[index];
        entity.runtime_rotation = rotation;
        entity.transform =
            super::render_transform::from_entity(rotation, position, entity.authored_scale);
        for draw in entity.draws.clone() {
            self.draws[draw].trans_mat = entity.transform;
        }
        if self.demo_story.enabled {
            super::rotator_runtime::accept_physics_pose(
                index,
                &mut self.transform_hierarchy,
                &mut self.entities,
                &mut self.draws,
            )?;
        }
        Ok(())
    }
    /// Production root-owner integration. Native controller/AI/LOD/spline and
    /// attached-owner relocation remain explicit required work, not skipped.
    fn move_to_entity(&mut self, request: super::entity_relocation::Request) -> Result<()> {
        if request.entity == self.player_entity {
            return self.move_player_to_entity(request);
        }
        if self.entities.get(request.entity).is_some_and(|entity| {
            entity
                .components
                .iter()
                .any(|component| component.class_name == "PhysicsRigidBodyCharacterComponent")
        }) {
            return self.move_npc_to_entity(request);
        }
        let source = self
            .entities
            .get(request.entity)
            .context("MoveToEntity source identity is missing")?;
        for component in &source.components {
            ensure!(
                !matches!(
                    component.class_name.as_str(),
                    "BasicAIControllerComponent"
                        | "BasicMotionControllerComponent"
                        | "MickeyPlayerControllerComponent"
                        | "MickeyPlayerMotionControllerComponent"
                        | "PhysicsRigidBodyCharacterComponent"
                        | "PhysicsRigidBodyDynamicComponent"
                        | "PhysicsRigidBodyPhantomShapeComponent"
                        | "SplineFollowerComponent"
                        | "SplineTrackerComponent"
                ),
                "MoveToEntity {}:{} requires {} relocation effects",
                source.scene,
                source.name,
                component.class_name
            );
        }
        if source
            .components
            .iter()
            .any(|c| c.class_name == "LODComponent")
        {
            ensure!(
                self.lod_states.contains_key(&request.entity),
                "MoveToEntity LOD state is missing"
            );
        }
        let target = self
            .entities
            .get(request.target)
            .context("MoveToEntity target identity is missing")?;
        let position = target.transform[12..15].try_into().unwrap();
        let rotation = target.runtime_rotation;
        self.relocate_entity(request.entity, position, rotation)?;
        self.refresh_relocated_model(request.entity,self.animation_time)?;
        if self.lod_states.contains_key(&request.entity) {self.reset_relocated_lod(request.entity)?;}
        let limitation="MoveToEntity root-owner integration is active; original component scheduling, intermediate setter publications and active physics-world fidelity remain unvalidated.";
        if !self.report.unsupported.iter().any(|s| s == limitation) {
            self.report.unsupported.push(limitation.into());
        }
        Ok(())
    }
    pub fn run_script(&mut self, source: &str) -> Result<()> {
        self.with_script_world_host(Self::script_world_api, |scripts| scripts.evaluate(source))
    }
    #[cfg(any(test, debug_assertions))]
    pub(super) fn run_callback_fixture(&mut self, source: &str) -> Result<()> {
        self.with_script_world_host(Self::script_world_api, |scripts| {
            scripts.evaluate_callback_fixture(source)
        })
    }
    /// The known subset of AI behavior receiver execution, not tree selection
    /// or controller delivery. AI event0x0c actions are distinct from the
    /// GameEventsComponent ActionEventNames enum (REL31f1c4).
    pub(super) fn receive_known_ai_behavior_event(
        &mut self,
        owner: usize,
        event: super::ai_behavior_events::Event,
    ) -> Result<bool> {
        ensure!(
            !self.removed_entities.contains(&owner),
            "AI event owner was removed"
        );
        let brain = self
            .ai_brains
            .get_mut(&owner)
            .context("AI event target has no authored brain")?;
        Ok(brain.receive_known(owner, event))
    }
    /// Supplemental NPCPuddle callback stage, not a substitute for deferred
    /// manager delivery or the other behavior receivers.
    pub(super) fn receive_known_ai_counter_status(
        &mut self,
        owner: usize,
        event: &super::entity_events::CounterStatus,
    ) -> Result<bool> {
        ensure!(
            !self.removed_entities.contains(&owner),
            "Counter-status listener was removed"
        );
        let brain = self
            .ai_brains
            .get_mut(&owner)
            .context("Counter-status listener has no authored brain")?;
        Ok(brain.receive_counter_status(owner, event))
    }
    /// NPCPuddle slot24, REL68eb4: use the counter's current threshold result,
    /// not the render/model painted bit or whether this behavior is selected.
    pub(super) fn npc_puddle_eligible(&self, owner: usize, behavior: usize) -> Result<bool> {
        ensure!(
            !self.removed_entities.contains(&owner),
            "Puddle eligibility owner was retired"
        );
        let brain = self
            .ai_brains
            .get(&owner)
            .context("Puddle eligibility owner has no brain")?;
        ensure!(
            brain.npc_puddle.contains(&behavior),
            "Not an authored NPCPuddle slot"
        );
        let counter = self
            .paint_counters
            .get(&owner)
            .context("NPCPuddle requires its owner's paint/thinner counter")?;
        Ok(counter.status() == 2)
    }
    /// REL363fe0..364068: enqueue kind75 before updating the cached status.
    /// Callers still owe value/material writes and subsequent hit callbacks.
    pub(super) fn enqueue_counter_status(
        &mut self,
        owner: usize,
        current: u32,
        header: Option<usize>,
        auxiliary: u32,
        reference: Option<usize>,
        compare: &mut dyn FnMut(usize, usize) -> Result<bool>,
    ) -> Result<bool> {
        ensure!(
            !self.removed_entities.contains(&owner),
            "Counter status owner was removed"
        );
        let previous = self
            .paint_counters
            .get(&owner)
            .context("Counter status owner has no counter")?
            .published_status;
        if previous == current {
            return Ok(false);
        }
        let event = super::entity_events::CounterStatus {
            source: Some(owner),
            header,
            previous: previous as i32,
            current: current as i32,
            auxiliary,
            reference,
        }
        .into();
        self.entity_event_queue
            .enqueue_with(&event, None, compare)?;
        self.paint_counters
            .get_mut(&owner)
            .unwrap()
            .published_status = current;
        Ok(true)
    }
    pub(super) fn relative_prefab_entity(&self, owner: usize, suffix: &str) -> Result<Option<usize>> {
        let owner = self.scripts.prefab_owner(owner)?.unwrap_or(owner);
        let root = self
            .entities
            .get(owner)
            .context("Relative prefab owner is not loaded")?;
        let Some(component) = root
            .components
            .iter()
            .find(|component| component.class_name == "JPSPrefabComponent")
        else {
            return Ok(None);
        };
        let members = component
            .properties
            .iter()
            .find(|property| property.name == "Prefab Entities")
            .and_then(|property| property.value.as_array())
            .context("Relative prefab membership array missing")?;
        // REL34d8b0..34da2c scans Prefab Entities, then each member's
        // InputOutputComponent EntityGroups. It compares interned strings
        // exactly; the rendered entity name is not part of this lookup.
        let mut caged_actor = None;
        let mut ambiguous_caged_actor = false;
        for member in members {
            let id = u32::try_from(member.as_u64().context("Invalid prefab member ID")?)?;
            if id == 0 {
                continue;
            }
            let Some((index, entity)) = self.entities.iter().enumerate().find(|(index, entity)| {
                entity.scene == root.scene
                    && entity.id == id
                    && !self.removed_entities.contains(index)
            }) else {
                continue;
            };
            for io in entity
                .components
                .iter()
                .filter(|component| component.class_name == "InputOutputComponent")
            {
                let Some(groups) = io
                    .properties
                    .iter()
                    .find(|property| property.name == "EntityGroups")
                else {
                    continue;
                };
                for group in groups
                    .value
                    .as_array()
                    .context("Prefab member EntityGroups must be an array")?
                {
                    let group = group.as_str().context("Prefab member EntityGroups entry must be a string")?;
                    if group == suffix {
                        return Ok(Some(index));
                    }
                    // Space Voyage's three cage instances omit the dot and
                    // give both the cage and its occupant the same group.
                    if suffix == ".caged_gremlin" && group == "caged_gremlin"
                        && entity.components.iter().any(|c| c.class_name == "PhysicsRigidBodyCharacterComponent")
                    {
                        ambiguous_caged_actor |= caged_actor.is_some_and(|actor| actor != index);
                        caged_actor = Some(index);
                    }
                }
            }
        }
        Ok(caged_actor.filter(|_| !ambiguous_caged_actor))
    }
    pub(super) fn child_entity_by_name(&self, owner: usize, name: &str) -> Result<Option<usize>> {
        let requested = super::named_effects::lookup_key(name)?;
        if let Some((&entity, _)) = self.named_fx.live.iter().find(|(_, effect)| {
            effect.owner == owner
                && !effect.fire_and_forget
                && effect.template.eq_ignore_ascii_case(&requested)
        }) {
            return Ok(Some(entity));
        }
        let root = self
            .entities
            .get(owner)
            .context("Child owner is not loaded")?;
        let component = root
            .components
            .iter()
            .find(|c| c.class_name == "ChildComponent");
        let found = if let Some(slots)=self.live_child_slots.get(&owner) {
            let requested=super::child_entities::requested_name(name)?;
            slots.iter().flatten().copied().find(|&i| {
                let child = &self.entities[i];
                !self.removed_entities.contains(&i)
                    && (child.name.eq_ignore_ascii_case(requested)
                        || child.name.strip_suffix(&format!(" [spawn {}]", child.id))
                            .is_some_and(|name| name.eq_ignore_ascii_case(requested)))
            })
        } else {super::child_entities::find(component, name, |id| {
            Ok(self
                .entities
                .iter()
                .enumerate()
                .find(|(index, entity)| {
                    entity.scene == root.scene
                        && entity.id == id
                        && !self.removed_entities.contains(index)
                })
                .map(|(index, entity)| (index, entity.name.clone())))
        })?};
        if found.is_none() {
            // Do not turn an unimplemented spawn into a successful null lookup
            // followed by Hide(0). These templates are real runtime entities.
            let requested = super::child_entities::requested_name(name)?;
            if component
                .and_then(|c| c.properties.iter().find(|p| p.name == "Child Templates"))
                .and_then(|p| p.value.as_array())
                .is_some_and(|templates| {
                    templates.iter().any(|template| {
                        template
                            .as_str()
                            .is_some_and(|template| template.eq_ignore_ascii_case(requested))
                    })
                })
            {
                anyhow::bail!("Child template {requested} on {} requires native child instantiation and attachment",root.name);
            }
        }
        Ok(found)
    }
    pub fn dispatch_script_callback(&mut self, name: &str, entity: usize) -> Result<()> {
        if self.scripts.is_execution_detached() {
            return self.scripts.queue_callback(name, entity);
        }
        self.with_script_world_host(Self::script_world_api, |scripts| scripts.call(name, entity))
    }
    pub fn emit_entity_output(&mut self, entity: usize, output: &str, activator: usize) {
        self.outputs
            .emit(entity, output, activator, self.animation_time);
    }
    pub(super) fn dispatch_actor_inputs(&mut self, time: f32) {
        let deliveries = self.outputs.ready(time);
        self.dispatch_actor_deliveries(deliveries);
    }
    pub(super) fn dispatch_actor_deliveries(&mut self, deliveries: Vec<super::actor_io::Delivery>) {
        for delivery in deliveries {
            let connection = delivery.connection;
            // ready() returns a batch. An earlier command can destroy an
            // owner whose later delivery is already in that batch.
            if self.removed_entities.contains(&connection.owner)
                || !self.entity_scene_active(connection.owner)
            {
                continue;
            }
            let result = (|| {
                ensure!(
                    connection.flags & !3 == 0,
                    "IO flags {:#x} include unrecovered policy bits",
                    connection.flags
                );
                // Retail checks the global before expanding targets/parameters.
                // Missing or noncallable inputs are ignored, not failed tasks.
                if !self.scripts.io_callback_admitted(&connection.input)? {
                    let warning=format!("Authored IO input skipped by retail callable-type gate: {} -> {}",self.entities[connection.owner].name,connection.input);
                    if !self.report.warnings.contains(&warning) {self.report.warnings.push(warning);}
                    return Ok(());
                }
                // Snapshot names before callbacks, then resolve each name at
                // delivery time, as REL328aec..328bbc does. TargetGroup is
                // ignored for ordinary selectors; it is not an extra filter.
                let names = if connection.target == "_target_group" {
                    self.scripts.group_entity_names(&connection.group)?
                } else {
                    vec![connection.target.clone()]
                };
                for name in names {
                    let target = match name.as_str() {
                        "_self" => Some(connection.owner),
                        "_activator" => delivery.activator,
                        "_player" => Some(self.player_entity),
                        name => self.resolve_io_target(connection.owner,name)?,
                    };
                    let result = if let Some(target) = target {
                        self.dispatch_actor_input(target, delivery.activator, &connection, delivery.payload.as_ref())
                    } else {
                        // The retail IO dispatcher still calls the function,
                        // with nil as its first argument (REL328e28..328e64).
                        // Do not fabricate an entity or suppress the callback.
                        let context = super::actor_scripts::CallbackContext {
                            entity: Some(connection.owner),
                            activator: delivery.activator,
                        };
                        if self.scripts.is_execution_detached() {
                            self.scripts.queue_io_from(
                                &connection.input,
                                context,
                                &connection.parameters,connection.owner,true,delivery.payload.as_ref(),
                            )
                        } else {
                            self.with_script_world_host(Self::script_world_api, |scripts| {
                                scripts.call_io_from(
                                    &connection.input,
                                    context,
                                    &connection.parameters,connection.owner,true,delivery.payload.as_ref(),
                                )
                            })
                        }
                    };
                    // A missing/broken receiver must not prevent remaining
                    // snapshotted group members from receiving the event.
                    if let Err(error) = result {
                        self.report.unsupported.push(format!(
                            "Authored IO {}: {} -> {} ({name}): {error:#}",
                            self.entities[connection.owner].name,
                            connection.output,
                            connection.input
                        ));
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                self.report.unsupported.push(format!(
                    "Authored IO {}: {} -> {}: {error:#}",
                    self.entities[connection.owner].name, connection.output, connection.input
                ));
            }
        }
    }
    fn dispatch_actor_input(
        &mut self,
        target: usize,
        activator: Option<usize>,
        connection: &super::actor_io::Connection,
        payload: Option<&super::actor_io::EventPayload>,
    ) -> Result<()> {
        ensure!(
            !self.removed_entities.contains(&target),
            "IO target was removed"
        );
        match connection.input.as_str() {
            "SetPropertyFloat" => {
                self.dispatch_io_float(target,&connection.parameters)
            }
            "RemovePhysicsFromWorld" => self.remove_entity_physics(target),
            "DestroyEntity" => self.queue_entity_retirement(target).map(|_|()),
            "EnableMotion" => self.set_entity_motion_enabled(target, true),
            "DisableMotion" => self.set_entity_motion_enabled(target, false),
            input => {
                let context = super::actor_scripts::CallbackContext {
                    entity: Some(target),
                    activator,
                };
                if self.scripts.is_execution_detached() {
                    return self
                        .scripts
                        .queue_io_from(input, context, &connection.parameters,connection.owner,false,payload);
                }
                self.with_script_world_host(Self::script_world_api, |scripts| {
                    scripts.call_io_from(input, context, &connection.parameters,connection.owner,false,payload)
                })
            }
        }
    }
    pub fn request_actor_sequence(&mut self, entity: usize, name: &str) -> Result<()> {
        self.request_actor_sequence_with_end_event(entity, name, false)
    }

    fn request_actor_sequence_with_end_event(&mut self, entity: usize, name: &str, notify: bool) -> Result<()> {
        ensure!(
            !self.removed_entities.contains(&entity),
            "Animation requested on a removed entity"
        );
        let resources = self
            .actor_sequences
            .get(&entity)
            .context("Script target has no Gamebryo actor resources")?;
        // Native2f6ea8 clears the idle flag before sequence lookup, including
        // an unknown sequence. Requests must resume the component clock.
        self.idle_actors.remove(&entity);
        let id = resources.find_name(name);
        if resources.get(id).is_none() {
            return Ok(());
        }
        let actor = self.actors
            .get_mut(&entity)
            .context("Script target has no supported actor instance")?;
        actor.request(
                id,
                resources,
                self.actor_bindings
                    .get(&entity)
                    .context("Missing actor bindings")?,
            )?;
        actor.bind_end_event(id, notify, resources)
    }
    fn reset_actor_sequence(&mut self, entity: usize) -> Result<()> {
        ensure!(
            !self.removed_entities.contains(&entity),
            "Reset requested on a removed actor"
        );
        self.idle_actors.remove(&entity); // REL2f8334 clears94 before manager work.
        let actor = self
            .actors
            .get_mut(&entity)
            .context("Reset target has no supported actor instance")?;
        // The component retains its target across a manager reset. Let the
        // next update consume it, so a sequence selected immediately after
        // reset starts clean instead of crossfading from a restarted old clip.
        actor.reset();
        Ok(())
    }
    pub fn play_actor(&mut self, name: &str, sequence: &str, repeat: bool) -> Result<()> {
        let matches = self
            .entities
            .iter()
            .enumerate()
            .filter(|(_, e)| e.name == name)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        ensure!(
            matches.len() == 1,
            "Actor name {name:?} is missing or ambiguous"
        );
        let entity = matches[0];
        let resources = self
            .actor_sequences
            .get(&entity)
            .context("Actor has no Gamebryo resources")?;
        let id = resources.find_name(sequence);
        let (clip, _) = resources.get(id).context("Actor sequence not found")?;
        let bindings = self
            .actor_bindings
            .get(&entity)
            .context("Actor pose bindings unavailable")?;
        let pose = bindings.sample(id, 0.)?;
        let hierarchy = &self.model_bounds[self.entities[entity]
            .model_asset
            .as_ref()
            .context("Actor model missing")?];
        let world = hierarchy.base_world_transforms_with_poses(None, &pose.transforms)?;
        self.actor_playback = Some(ActorPlayback {
            entity,
            sequence: id,
            name: sequence.into(),
            repeat,
            time: 0.,
            duration: clip.duration,
            frequency: clip.frequency,
            cycle: clip.cycle_type,
            clock: super::nif_sequence::UnsynchronizedClock::new(0., clip.frequency)?,
            pose,
            world,
        });
        Ok(())
    }
    /// Exploration recovery, never used by the authored gameplay path. An
    /// aborted script cannot execute its matching fade-in/input release.
    pub fn recover_failed_preview_script(&mut self, since: usize) -> bool {
        let failed = self.report.unsupported.iter().skip(since).any(|message| {
            message.starts_with("Game Lua:") || message.starts_with("Game script ") ||
                // A callback lookup failure ran no Lua and cannot strand a
                // fade/control transaction. Do not cancel another live sequence.
                (message.starts_with("Authored IO ")
                    && !message.contains("Missing authored script callback "))
        });
        let failed_projector = self.report.unsupported.iter().skip(since).any(|message| {
            !message.contains("Missing authored script callback ")
                && message.contains("Mickey_Projector_Exit")
                && (message.starts_with("Game Lua:")
                    || message.starts_with("Game script ")
                    || message.starts_with("Authored IO "))
        });
        let hidden_player = failed_projector && self.hidden_entities.contains(&self.player_entity);
        let abandoned_camera = failed_projector && self.nif_camera.is_some();
        if !failed
            || !(self.screen_fade.out
                || self.screen_fade.opacity > 0.
                || self.cutscene_input.restricted
                || hidden_player
                || abandoned_camera)
        {
            return false;
        }
        // Preview-only recovery of an interrupted projector sequence. Do not
        // unhide unrelated entities or override a working script's player hide.
        if hidden_player {
            if let Err(error) = self.set_entity_hidden(self.player_entity, false) {
                self.report
                    .unsupported
                    .push(format!("Preview recovery visibility failed: {error:#}"));
                return false;
            }
        }
        if failed_projector {
            self.nif_camera = None;
            self.nif_camera_transition = None;
            self.nif_camera_completed_frame = None;
        }
        self.screen_fade = Default::default();
        self.cutscene_input = Default::default();
        self.script_player.script_input_restricted = false;
        let message="Preview recovery: an interrupted game script left the screen faded, controls locked or the projector player hidden. Exploration restored; the scripted sequence remains incomplete.";
        if !self.report.unsupported.iter().any(|entry| entry == message) {
            self.report.unsupported.push(message.into());
        }
        true
    }
    pub fn advance_models(&mut self, time: f32) -> Result<()> {
        self.advance_models_with_delta(time, time - self.animation_time)
    }
    pub(super) fn advance_models_with_delta(&mut self, time: f32, delta: f32) -> Result<()> {
        ensure!(
            time.is_finite() && time >= self.animation_time,
            "Actor time must be monotonic"
        );
        ensure!(
            delta.is_finite() && delta >= 0.,
            "Invalid committed component delta"
        );
        if !std::mem::take(&mut self.scene_requests_prepared_for_models) {
            self.process_scene_request()?;
        }
        self.animation_time = time;
        self.script_feedback.advance(delta);
        self.advance_countdown(delta);
        self.advance_logic_timers(delta)?;
        let player_position = self
            .entities
            .get(self.player_entity)
            .context("Player entity disappeared")?
            .transform[12..15]
            .try_into()?;
        self.advance_pickup_collection_gates(delta)?;
        self.advance_pickup_overlaps(player_position)?;
        self.advance_pickup_attraction(delta)?;
        self.advance_counter_materials(0..self.entities.len());
        self.advance_counter_lifecycle(delta)?;
        self.advance_health_lifecycle()?;
        // Each GameEvents component performs its first-update dispatch before
        // draining delayed events. Keep callbacks in the real World host so
        // native calls retain their owning entity and can create more actors.
        // Binding order is retained here; native manager-wide ordering remains
        // a separate fidelity gap, rather than being inferred from entity IDs.
        for owner in self.component_events.owners() {
            if self.removed_entities.contains(&owner) || !self.entity_scene_active(owner) {
                continue;
            }
            let disabled =
                self.entities[owner]
                    .components
                    .iter()
                    .enumerate()
                    .any(|(index, component)| {
                        component.class_name == "GameEventsComponent"
                            && self
                                .component_control
                                .disabled
                                .get(&owner)
                                .and_then(|flags| flags.get(&index))
                                == Some(&true)
                    });
            if disabled {
                continue;
            }
            let events = self.component_events.update(owner, time)?;
            self.execute_component_events(events)?;
        }
        self.dispatch_actor_inputs(time);
        let retired = self
            .demo_story
            .pending_deaths
            .iter()
            .copied()
            .filter(|&entity| !self.outputs.has_pending_for_owner(entity))
            .collect::<Vec<_>>();
        for entity in retired {
            self.retire_entity(entity)?;
        }
        self.advance_demo_story(delta);
        let active_script_owners = self
            .entities
            .iter()
            .enumerate()
            .filter_map(|(entity, _)| self.entity_scene_active(entity).then_some(entity))
            .collect::<std::collections::BTreeSet<_>>();
        let commands = self.with_script_world_host(Self::script_world_api, |scripts| {
            scripts.advance_active(time, |entity| active_script_owners.contains(&entity))
        })?;
        for command in commands {
            let result = match command {
                super::actor_scripts::Command::Sequence { entity, name, notify } => {
                    self.request_actor_sequence_with_end_event(entity, &name, notify)
                }
                super::actor_scripts::Command::Reset { entity } => {
                    self.reset_actor_sequence(entity)
                }
                super::actor_scripts::Command::Retire { entity } => self.queue_entity_retirement(entity).map(|_|()),
                super::actor_scripts::Command::Output { entity, name } => {
                    self.emit_entity_output(entity, &name, entity);
                    Ok(())
                }
            };
            if let Err(error) = result {
                self.report
                    .unsupported
                    .push(format!("Script animation request: {error:#}"));
            }
        }
        self.process_entity_cleanup()?;
        self.process_deferred_entity_events()?;
        self.advance_robot_arm_ai(delta)?;
        self.advance_demo_npc_behaviors(delta)?;
        self.report.unsupported.extend(
            self.scripts
                .errors
                .drain(..)
                .map(|error| format!("Game Lua: {error}")),
        );
        let mut failed = Vec::new();
        let active_scenes = self
            .entities
            .iter()
            .enumerate()
            .filter_map(|(entity, _)| self.entity_scene_active(entity).then_some(entity))
            .collect::<std::collections::BTreeSet<_>>();
        for (&entity, actor) in &mut self.actors {
            if !active_scenes.contains(&entity) {
                continue;
            }
            let result = (|| {
                if !self.idle_actors.contains(&entity) {
                    actor.update(
                        &self.actor_sequences[&entity],
                        &self.actor_bindings[&entity],
                        time,
                        delta,
                    )?;
                    if let Some(id) = actor.take_ended() {
                        let (sequence, _) = self.actor_sequences[&entity].get(id)
                            .context("Missing completed actor sequence")?;
                        if let Some(name) = &sequence.name {
                            self.outputs.emit_animation_ended(entity, time, name);
                        }
                    }
                }
                if let Some(pose) = &actor.pose {
                    let hierarchy = &self.model_bounds[self.entities[entity]
                        .model_asset
                        .as_ref()
                        .context("Actor model missing")?];
                    actor.world =
                        hierarchy.base_world_transforms_with_poses(None, &pose.transforms)?;
                }
                Ok::<_, anyhow::Error>(())
            })();
            if let Err(error) = result {
                self.report
                    .unsupported
                    .push(format!("Actor {}: {error:#}", self.entities[entity].name));
                failed.push(entity);
            }
        }
        for entity in failed {
            self.actors.remove(&entity);
        }
        for (&entity, animation) in &mut self.model_animation {
            if !active_scenes.contains(&entity) {
                continue;
            }
            let runtime = &self.entities[entity];
            let hierarchy = &self.model_bounds[runtime
                .model_asset
                .as_ref()
                .context("Animated model identity missing")?];
            let sample_time=if let Some(clock)=&mut animation.component_clock {
                let enabled=runtime.components.iter().position(|c|c.class_name=="JPSSceneGraphComponent")
                    .and_then(|component|self.component_control.disabled.get(&entity)?.get(&component)).copied()!=Some(true);
                clock.advance(enabled,delta);
                clock.periodic_update_time(time)
            } else {time};
            animation
                .update(hierarchy, sample_time)
                .with_context(|| format!("Model animation {}:{}", runtime.scene, runtime.name))?;
        }
        if let Some(playback) = &mut self.actor_playback {
            let mut clip_time = playback
                .clock
                .sample(time, playback.duration, playback.cycle)?;
            if playback.repeat
                && playback.cycle != 0
                && playback.duration > 0.
                && playback.time >= playback.duration
            {
                playback.clock =
                    super::nif_sequence::UnsynchronizedClock::new(0., playback.frequency)?;
                clip_time = playback
                    .clock
                    .sample(time, playback.duration, playback.cycle)?;
            }
            playback.time = clip_time;
            playback.pose =
                self.actor_bindings[&playback.entity].sample(playback.sequence, clip_time)?;
            let hierarchy = &self.model_bounds[self.entities[playback.entity]
                .model_asset
                .as_ref()
                .context("Actor model missing")?];
            playback.world =
                hierarchy.base_world_transforms_with_poses(None, &playback.pose.transforms)?;
        }
        self.advance_entity_lifetimes(delta)?;
        self.advance_named_effects(delta)?;
        self.advance_scene_particles(delta)?;
        self.advance_spawners(delta)?;
        self.advance_multi_position_audio();
        self.advance_nif_camera_transition(delta)?;
        self.publish_authored_camera()?;
        Ok(())
    }
    #[cfg(any(test, debug_assertions))]
    pub fn apply_model_poses(&self, draws: &mut [GpuDrawCommand]) -> Result<()> {
        self.apply_model_geometry(draws)?;
        self.apply_model_materials(draws)
    }

    pub(super) fn apply_model_geometry(&self, draws: &mut [GpuDrawCommand]) -> Result<()> {
        self.authored_havok.apply(self, draws)?;
        for (&entity, (template, actor)) in &self.havok_actors {
            if !self.entity_scene_active(entity) {
                continue;
            }
            let definition = &self.sketch_templates.templates[template];
            let path = definition.graph.definitions[&definition.graph.root]
                .model
                .as_ref()
                .context("Actor model missing")?;
            let model = self
                .sketch_templates
                .models
                .get(&key(path))
                .context("Actor prepared model missing")?;
            model.apply_pose(
                &actor.playback.pose,
                &self.entities[entity].mesh_draws,
                draws,
            )?;
        }
        for (&entity, animation) in &self.model_animation {
            if !self.entity_scene_active(entity) {
                continue;
            }
            // A sequence supplies skeletal/mesh poses. Direct extra-data
            // controllers still drive the material registers and indirect warp.
            let uses_actor_pose = self.actor_playback.as_ref().is_some_and(|p| p.entity == entity)
                || self.actors.get(&entity).is_some_and(|actor| actor.pose.is_some());
            let runtime = &self.entities[entity];
            for (&mesh, indices) in &runtime.mesh_draws {
                if uses_actor_pose || !animation.affected.contains(&mesh) {
                    continue;
                }
                let world = super::nif_runtime::matrix(
                    animation
                        .world
                        .get(&mesh)
                        .context("Animated mesh transform missing")?,
                );
                for &index in indices {
                    let draw = draws.get_mut(index).context("Animated draw missing")?;
                    if self.model_local_draws.contains(&index) {
                        draw.trans_mat = super::skin::matrix_product(&runtime.transform, &world);
                    } else {
                        let inverse = self
                            .model_mesh_inverse
                            .get(&index)
                            .context("Animated mesh bind inverse missing")?;
                        let delta = super::skin::matrix_product(&world, inverse);
                        draw.trans_mat = super::skin::matrix_product(&draw.trans_mat, &delta);
                    }
                }
            }
        }
        for (entity, pose, transforms) in self.presented_actor_poses() {
            for (&mesh, indices) in &self.entities[entity].mesh_draws {
                let world = super::nif_runtime::matrix(
                    transforms
                        .get(&mesh)
                        .context("Actor mesh transform missing")?,
                );
                for &index in indices {
                    let draw = draws.get_mut(index).context("Actor draw missing")?;
                    if self.model_local_draws.contains(&index) {
                        draw.trans_mat =
                            super::skin::matrix_product(&self.entities[entity].transform, &world);
                    } else {
                        let inverse = self
                            .model_mesh_inverse
                            .get(&index)
                            .context("Actor mesh bind inverse missing")?;
                        draw.trans_mat = super::skin::matrix_product(
                            &draw.trans_mat,
                            &super::skin::matrix_product(&world, inverse),
                        );
                    }
                    let morphs = pose
                        .morphs
                        .iter()
                        .filter(|((target, _), _)| *target == mesh)
                        .collect::<Vec<_>>();
                    if !morphs.is_empty() {
                        let (sources, bind) = self
                            .actor_draw_sources
                            .get(&index)
                            .context("Actor morph draw lacks source mapping")?;
                        let matches = morphs
                            .into_iter()
                            .filter(|(_, (first, positions))| {
                                sources.iter().all(|&source| {
                                    source >= *first && (source - *first) < positions.len() as u32
                                })
                            })
                            .collect::<Vec<_>>();
                        ensure!(
                            matches.len() == 1,
                            "Actor morph draw has ambiguous or missing source region"
                        );
                        let (_, (first, positions)) = matches[0];
                        let (vertices, lo, hi) = super::nif_sequence::render_positions(
                            &draw.v_data,
                            sources,
                            *first,
                            positions,
                            *bind,
                        )?;
                        draw.v_data = Arc::new(vertices);
                        draw.aabb_min = lo;
                        draw.aabb_max = hi;
                    }
                }
            }
            if pose.visibility.values().any(|&v| v < 0.5) {
                let hierarchy = &self.model_bounds[self.entities[entity]
                    .model_asset
                    .as_ref()
                    .context("Visibility model missing")?];
                let mut pending = pose
                    .visibility
                    .iter()
                    .filter_map(|(&node, &value)| (value < 0.5).then_some(node))
                    .collect::<Vec<_>>();
                let mut visited = HashSet::new();
                static EMPTY: std::sync::OnceLock<Arc<Vec<u32>>> = std::sync::OnceLock::new();
                while let Some(node) = pending.pop() {
                    if !visited.insert(node) {
                        continue;
                    }
                    if let Some(bound) = hierarchy.nodes.get(&node) {
                        pending.extend(bound.children.iter().flatten().copied());
                    }
                    for &index in self.entities[entity]
                        .mesh_draws
                        .get(&node)
                        .into_iter()
                        .flatten()
                    {
                        draws[index].indices = EMPTY.get_or_init(|| Arc::new(Vec::new())).clone();
                    }
                }
            }
        }
        Ok(())
    }
    fn presented_actor_poses(&self) -> impl Iterator<Item = (usize, &super::nif_sequence::ActorPose, &BTreeMap<u32, epic_mickey_lib_rs::nif_file::NiTransform>)> {
        let manual = self
            .actor_playback
            .as_ref()
            .filter(|playback| self.entity_scene_active(playback.entity))
            .map(|p| (p.entity, &p.pose, &p.world));
        let automatic = self
            .actors
            .iter()
            .filter(|(entity, _)| self.entity_scene_active(**entity))
            .filter(move |(entity, _)| manual.as_ref().is_none_or(|p| p.0 != **entity))
            .filter_map(|(&entity, actor)| {
                actor.pose.as_ref().map(|pose| (entity, pose, &actor.world))
            });
        automatic.chain(manual)
    }

    pub(super) fn apply_model_materials(&self, draws: &mut [GpuDrawCommand]) -> Result<()> {
        for (&entity, animation) in &self.model_animation {
            if !self.entity_scene_active(entity) {
                continue;
            }
            for (&mesh, indices) in &self.entities[entity].mesh_draws {
                for &index in indices {
                    let draw = draws.get_mut(index).context("Animated draw missing")?;
                    animation.apply_material(mesh, draw)?;
                }
            }
        }
        for (entity, pose, _) in self.presented_actor_poses() {
            for (&(mesh, slot), &color) in &pose.colors {
                let color = super::nif_properties::packed_color(color)?;
                for &index in self.entities[entity]
                    .mesh_draws
                    .get(&mesh)
                    .into_iter()
                    .flatten()
                {
                    let draw = &mut draws[index];
                    if let Some(defaults) = draw.preview.as_ref()
                        .and_then(|program| program.as_ref().ok())
                        .and_then(|program| program.register_defaults)
                    {
                        draw.material_registers.get_or_insert(defaults)[slot] = color;
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn actor_has_local_nif_draws(&self, entity: usize) -> bool {
        self.entities[entity]
            .draws
            .clone()
            .any(|draw| self.model_local_draws.contains(&draw))
    }
    /// Construct native model-instance ownership without fabricating a light
    /// sample point. Image updates follow once the owner's world bound exists.
    pub fn bind_spherical_properties(
        &mut self,
        entities: std::ops::Range<usize>,
        sources: &material_preview::Sources,
        files: &[VirtualFile],
        textures: &mut HashMap<String, Option<DecodedImage>>,
    ) -> Result<()> {
        use super::spherical_lighting::{inherited_property, BoundOwner};
        ensure!(
            entities.start <= entities.end && entities.end <= self.entities.len(),
            "Invalid lighting entity range"
        );
        ensure!(
            !self
                .spherical_bindings
                .iter()
                .any(|b| entities.contains(&b.entity))
                && !self
                    .child_lighting_properties
                    .keys()
                    .any(|id| entities.contains(&(*id as usize))),
            "Lighting already bound for entity range"
        );
        // SH belongs to the retained painted material, independent of initial ink state.
        let mut painted_draws = BTreeMap::new();
        for model in self.model_paint.values() {
            for (&draw, states) in &model.states {
                painted_draws.insert(draw, states[0].clone());
            }
        }
        for instance in &self.jigsaw_instances {
            for (&draw, states) in &instance.render_states {
                painted_draws.insert(draw, states[0].clone());
            }
        }
        let owner = |entity: usize| -> Result<Option<BoundOwner>> {
            let Some(hierarchy) = self.entity_bound_hierarchy(entity)? else {
                return Ok(None);
            };
            let roots: Vec<_> = hierarchy.roots.iter().flatten().copied().collect();
            ensure!(
                roots.len() == 1,
                "SH owner {} needs one model root, found {}",
                self.entities[entity].name,
                roots.len()
            );
            Ok(Some(BoundOwner {
                entity: entity as u64,
                node: roots[0],
            }))
        };
        let mut properties = self.spherical_properties.clone();
        let mut shared = self.child_lighting_properties.clone();
        for index in entities.clone() {
            let entity = &self.entities[index];
            if entity.children_share_lighting()? {
                if let Some(model) = owner(index)? {
                    shared.insert(index as u64, properties.allocate(model)?);
                }
            }
        }
        let links = self.lighting_owner_links(&shared)?;
        let mut local = BTreeMap::new();
        let mut bindings = Vec::new();
        for index in entities.clone() {
            let entity = &self.entities[index];
            let mut ordered = BTreeMap::new();
            for (&mesh, draw_indices) in &entity.mesh_draws {
                for &draw in draw_indices {
                    ensure!(
                        ordered.insert(draw, mesh).is_none(),
                        "Duplicate draw in SH model ownership"
                    );
                }
            }
            for (draw, mesh) in ordered {
                let command = painted_draws
                    .get(&draw)
                    .or_else(|| self.draws.get(draw))
                    .context("Missing SH mesh draw")?;
                let Some(material) = &command.material else {
                    continue;
                };
                // The existing material diagnostic remains authoritative
                // for unsupported templates; do not invent their map list.
                let Ok(program) = material_preview::prepare(material, sources, command.baked)
                else {
                    continue;
                };
                let Some(slot) = program.first_runtime_sh_slot(material) else {
                    continue;
                };
                let candidate = if let Some(id) = inherited_property(index as u64, None, &links)? {
                    id
                } else if let Some(&id) = local.get(&index) {
                    id
                } else {
                    let model = owner(index)?.context("SH mesh has no model owner")?;
                    let id = properties.allocate(model)?;
                    local.insert(index, id);
                    id
                };
                let property = properties.attach((index as u64, mesh), candidate)?;
                bindings.push(SphericalBinding {
                    entity: index,
                    mesh,
                    draw,
                    property,
                    slot,
                });
            }
        }
        // Commit as a unit; failed topology resolution cannot leave a partial
        // owner graph behind. Streaming teardown will use detach/collection.
        self.spherical_properties = properties;
        let placeholder = Arc::new(vec![0; super::spherical_lighting::SIZE.pow(2) * 4]);
        self.spherical_materials
            .extend(bindings.iter().map(|binding| {
                let command = painted_draws
                    .get(&binding.draw)
                    .unwrap_or(&self.draws[binding.draw]);
                let material = command.material.as_ref().unwrap();
                let prepared = material_preview::prepare(material, sources, command.baked)
                    .and_then(|program| {
                        let image = material_preview::MaterialImage {
                            slot: binding.slot.try_into().map_err(|_| "Invalid SH slot")?,
                            rgba: placeholder.clone(),
                            width: 16,
                            height: 16,
                            sampler: Default::default(),
                            matrix: [1., 0., 0., 0., 1., 0., 0., 0., 1.],
                        };
                        material_preview::images_with_runtime_sh(
                            &program,
                            material,
                            files,
                            textures,
                            Some(&image),
                        )
                        .map(|images| (program, images))
                    });
                (binding.draw, prepared)
            }));
        self.spherical_bindings.extend(bindings);
        self.child_lighting_properties = shared;
        Ok(())
    }
    /// Called once per rendered view, before draw filtering. Resolve each
    /// shared property once and retain immutable map images between frames.
    pub fn apply_spherical_lighting(&mut self, draws: &mut [GpuDrawCommand], view: &[f32; 16]) {
        let owners: BTreeMap<_, _> = self
            .spherical_bindings
            .iter()
            .filter_map(|binding| {
                self.spherical_properties
                    .get(binding.property)
                    .ok()
                    .map(|property| (binding.property, property.owner))
            })
            .collect();
        let projected_entities = self
            .entities
            .iter()
            .enumerate()
            .filter_map(|(entity, runtime)| {
                runtime
                    .mesh_draws
                    .values()
                    .flatten()
                    .any(|&draw| {
                        let active = draws
                            .get(draw)
                            .and_then(|draw| draw.preview.as_ref())
                            .and_then(|program| program.as_ref().ok());
                        // Lighting attachment can replace an initially unready material.
                        // Include its final projection requirement before resolving bounds.
                        let prepared = if !self.sketch_draws.contains(&draw)
                            && !self.jigsaw_effect_values.contains_key(&draw)
                        {
                            self.spherical_materials
                                .get(&draw)
                                .and_then(|material| material.as_ref().ok())
                                .map(|(program, _)| program)
                        } else {
                            None
                        };
                        active
                            .into_iter()
                            .chain(prepared)
                            .any(|program| program.pinned_projection)
                    })
                    .then_some(entity)
            })
            .collect::<BTreeSet<_>>();
        let required = owners
            .values()
            .map(|owner| owner.entity as usize)
            .chain(projected_entities.iter().copied())
            .collect::<BTreeSet<_>>();
        let mut bounds = BTreeMap::new();
        let mut projection_mesh_bounds = BTreeMap::new();
        for entity in required {
            if bounds.contains_key(&entity) {
                continue;
            }
            let result = (|| -> Result<BTreeMap<u32, super::scene_bounds::Sphere>> {
                let runtime = &self.entities[entity];
                // Draw transforms include the current presentation pose. A
                // sharing owner without a draw uses its runtime transform.
                let matrix = runtime
                    .draws
                    .clone()
                    .next()
                    .filter(|i| !self.model_local_draws.contains(i))
                    .and_then(|i| draws.get(i))
                    .map(|d| d.trans_mat)
                    .unwrap_or(runtime.transform);
                let scale = runtime.authored_scale;
                ensure!(
                    scale.is_finite() && scale > 0.,
                    "Unsupported SH owner scale"
                );
                let transform = epic_mickey_lib_rs::nif_file::NiTransform {
                    rotation: epic_mickey_lib_rs::nif_file::Matrix3 {
                        rows: std::array::from_fn(|r| {
                            std::array::from_fn(|c| matrix[c * 4 + r] / scale)
                        }),
                    },
                    translation: epic_mickey_lib_rs::nif_file::Vector3 {
                        x: matrix[12],
                        y: matrix[13],
                        z: matrix[14],
                    },
                    scale,
                };
                let hierarchy = self
                    .entity_bound_hierarchy(entity)?
                    .context("SH owner model unavailable")?;
                let empty = BTreeMap::new();
                let previous = self.spherical_bounds.get(&entity).unwrap_or(&empty);
                let empty_poses = BTreeMap::new();
                let animation = self.model_animation.get(&entity);
                let (poses, direct, managed) = if let Some(playback) = self
                    .actor_playback.as_ref().filter(|playback| playback.entity == entity)
                {
                    (&playback.pose.transforms, true, true)
                } else if let Some(actor) = self.actors.get(&entity) {
                    let poses = actor.pose.as_ref().map(|pose| &pose.transforms)
                        .or_else(|| animation.map(|animation| &animation.poses));
                    (poses.unwrap_or(&empty_poses), true, true)
                } else if let Some(animation) = animation {
                    (&animation.poses, true, false)
                } else {
                    (&empty_poses, false, false)
                };
                let runtime_geometry = self.scene_particles.live.get(&entity)
                    .map(|effect| effect.particles.lighting_bounds())
                    .transpose()?.unwrap_or_default();
                let result = if let Some(animation) = animation.filter(|_| !managed) {
                    hierarchy.base_world_transforms_with_overrides(
                        Some(&transform), &empty_poses, &animation.published_locals,
                    )
                } else {
                    hierarchy.base_world_transforms_with_poses(Some(&transform), poses)
                }.and_then(|mut transforms| {
                    hierarchy.face_billboard_bounds(&mut transforms, view)?;
                    hierarchy.bounds_with_runtime_geometry(
                        &transforms, previous, direct, managed, &runtime_geometry, true,
                    )
                });
                if result.is_err() && projected_entities.contains(&entity) {
                    projection_mesh_bounds.insert(
                        entity,
                        hierarchy
                            .mesh_world_bounds(&transform, poses, direct, managed)
                            .map_err(|error| format!("{error:#}")),
                    );
                }
                result
            })();
            if let Ok(value) = &result {
                self.spherical_bounds.insert(entity, value.clone());
            }
            bounds.insert(
                entity,
                result.map_err(|e| format!("SH bounds for {}: {e:#}", self.entities[entity].name)),
            );
        }
        let mut images = BTreeMap::new();
        for (id, owner) in owners {
            let image = (|| -> std::result::Result<Arc<Vec<u8>>, String> {
                let bound = *bounds[&(owner.entity as usize)]
                    .as_ref()
                    .map_err(Clone::clone)?
                    .get(&owner.node)
                    .ok_or("SH owner node has no world bound")?;
                if self
                    .report
                    .spherical_lights
                    .iter()
                    .any(|light| light.raycast)
                {
                    return Err("SH raycast visibility requires runtime integration".into());
                }
                self.spherical_properties
                    .update_image(
                        id,
                        bound,
                        self.report
                            .spherical_lights
                            .iter()
                            .map(|light| (&light.light, 1.)),
                        view,
                    )
                    .map_err(|e| format!("{e:#}"))
            })();
            images.insert(id, image);
        }
        for binding in &self.spherical_bindings {
            // An ink transition owns its shader, but its composed base still
            // samples the object's live lighting property.
            if self.jigsaw_effect_values.contains_key(&binding.draw) {
                let draw = &mut draws[binding.draw];
                if let (Some(Ok(pixels)), Some(Ok(program)), Some(material)) = (
                    images.get(&binding.property), draw.preview.as_ref(), draw.material.as_ref(),
                ) {
                    if let Some(slot) = program.first_runtime_sh_slot(material) {
                        if let Some(image) = Arc::make_mut(&mut draw.material_images).iter_mut()
                            .find(|image| u32::from(image.slot) == slot)
                        {
                            image.rgba = pixels.clone();
                        }
                    }
                }
                continue;
            }
            if self.sketch_draws.contains(&binding.draw) {
                continue;
            }
            let result = (|| -> std::result::Result<_, String> {
                let pixels = images
                    .get(&binding.property)
                    .ok_or("Missing SH property image")?
                    .as_ref()
                    .map_err(Clone::clone)?;
                let (program, templates) = self
                    .spherical_materials
                    .get(&binding.draw)
                    .ok_or("Missing SH material")?
                    .as_ref()
                    .map_err(Clone::clone)?;
                let mut images = templates.clone();
                // An unused SH slot still owns a property but needs no sampler.
                if let Some(image) = images
                    .iter_mut()
                    .find(|image| u32::from(image.slot) == binding.slot)
                {
                    image.rgba = pixels.clone();
                }
                Ok((program.clone(), images))
            })();
            let draw = &mut draws[binding.draw];
            match result {
                Ok((program, images)) => {
                    draw.preview = Some(Ok(program));
                    draw.material_images = Arc::new(images);
                }
                Err(error) => draw.preview = Some(Err(error)),
            }
        }
        // Apply projection anchors after final material selection. Otherwise a
        // newly prepared projected shader reaches the GPU with a missing bound.
        for entity in projected_entities {
            for (&mesh, indices) in &self.entities[entity].mesh_draws {
                let center = bounds[&entity]
                    .as_ref()
                    .ok()
                    .and_then(|bounds| bounds.get(&mesh))
                    .or_else(|| {
                        projection_mesh_bounds
                            .get(&entity)
                            .and_then(|result| result.as_ref().ok())
                            .and_then(|meshes| meshes.get(&mesh))
                            .and_then(|result| result.as_ref().ok())
                    })
                    .map(|sphere| sphere.center);
                for &index in indices {
                    draws[index].projection_bound = center;
                }
                if center.is_none()
                    && indices.iter().any(|&index| {
                        draws[index].preview.as_ref().is_some_and(|preview| {
                            preview
                                .as_ref()
                                .is_ok_and(|program| program.pinned_projection)
                        })
                    })
                {
                    let detail = projection_mesh_bounds
                        .get(&entity)
                        .and_then(|result| result.as_ref().ok())
                        .and_then(|meshes| meshes.get(&mesh))
                        .and_then(|result| result.as_ref().err())
                        .or_else(|| bounds[&entity].as_ref().err())
                        .map(String::as_str)
                        .unwrap_or("Mesh world bound unavailable");
                    let error = format!(
                        "Pinned projection {} mesh{mesh}: {detail}",
                        self.entities[entity].name
                    );
                    if !self.report.unsupported.contains(&error) {
                        self.report.unsupported.push(error);
                    }
                }
            }
        }
    }
    pub fn entity_bound_hierarchy(
        &self,
        entity: usize,
    ) -> Result<Option<&super::scene_bounds::Hierarchy>> {
        let entity = self
            .entities
            .get(entity)
            .context("Missing bound-owner entity")?;
        entity
            .model_asset
            .as_ref()
            .map(|asset| {
                self.model_bounds.get(asset).with_context(|| {
                    format!(
                        "Missing bound hierarchy for {}:{} ({asset})",
                        entity.scene, entity.name
                    )
                })
            })
            .transpose()
    }
    /// Native36539c/16ccf8 traverses the owner's model-root NiNode graph.
    /// Bone-parented entities have independent model roots; a transform-parent
    /// relationship alone does not add their models to this graph.
    pub(super) fn publish_model_constant_color(
        &mut self,
        owner: usize,
        update: super::paint_counter::MaterialUpdate,
    ) -> Result<usize> {
        self.publish_model_color(owner, update.constant_color, update.rgba, true)
    }
    fn publish_model_color(
        &mut self,
        owner: usize,
        slot: u8,
        rgba: [u8; 4],
        constant: bool,
    ) -> Result<usize> {
        ensure!(
            slot < if constant { 4 } else { 3 },
            "Invalid model color slot"
        );
        ensure!(
            !self.removed_entities.contains(&owner),
            "Color publication owner was removed"
        );
        let Some(hierarchy) = self.entity_bound_hierarchy(owner)? else {
            return Ok(0);
        };
        let mut order = vec![];
        let mut pending = hierarchy
            .roots
            .iter()
            .rev()
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        while let Some(node) = pending.pop() {
            let state = hierarchy
                .nodes
                .get(&node)
                .context("Color subtree node missing")?;
            if state.authored_bound.is_some() {
                order.push(node);
            }
            pending.extend(state.children.iter().rev().flatten().copied());
        }
        for &node in &order {
            self.render_objects
                .get(&(owner, node))
                .context("Color target is not an initialized render object")?;
        }
        // Native16cda8 writes userdata independently of shader readiness.
        // Retain colors even when a drawable has a material diagnostic.
        let mut published = 0;
        for node in order {
            let object = self.render_objects.get_mut(&(owner, node)).unwrap();
            published += usize::from(if constant {
                object.publish(slot, rgba)?
            } else {
                object.publish_regular(slot, rgba)?
            });
        }
        Ok(published)
    }
    pub(super) fn apply_model_constant_colors(&self, draws: &mut [GpuDrawCommand]) -> Result<()> {
        for (&(owner, node), state) in &self.render_objects {
            // Streamed-out scenes retain object color state, but their draw
            // payloads can be cold. Reapply on activation after restoration.
            if self.removed_entities.contains(&owner) || !self.entity_scene_active(owner) {
                continue;
            }
            if !state
                .constant_colors
                .iter()
                .chain(&state.regular_colors)
                .any(Option::is_some)
            {
                continue;
            }
            if let Some(indices) = self.entities[owner].mesh_draws.get(&node) {
                for &index in indices {
                    if self.removed_draws.contains(&index) {continue;}
                    state.apply(
                        draws
                            .get_mut(index)
                            .context("Published model draw missing")?,
                    ).with_context(||format!("Model {} node {node} draw {index}",self.entities[owner].name))?;
                }
            }
        }
        Ok(())
    }
    pub(super) fn advance_counter_materials(&mut self, range: std::ops::Range<usize>) {
        let owners = self
            .counter_materials
            .keys()
            .copied()
            .filter(|owner| range.contains(owner) && self.entity_scene_active(*owner))
            .collect::<Vec<_>>();
        for owner in owners {
            let result = (|| -> Result<()> {
                let runtime = &self.counter_materials[&owner];
                let component = self.entities[owner]
                    .components
                    .iter()
                    .find(|component| component.class_name == "PaintThinnerCounterComponent")
                    .context("Counter material component disappeared")?;
                let value = |name: &str| {
                    component
                        .properties
                        .iter()
                        .find(|property| property.name == name)
                        .map(|property| &property.value)
                };
                // Native constructor361db4 initializes the effect enum to0.
                let effect = value("PaintThinnerEffect")
                    .map(|value| value.as_i64().context("Invalid counter material effect"))
                    .transpose()?
                    .unwrap_or(0);
                // Descriptor3628c0 is EffectThreshold, not ToonPercentThreshold.
                // Its factory39e990 loads float1 from4cd748 when omitted.
                let threshold = value("EffectThreshold")
                    .map(|value| value.as_f64().context("Invalid counter effect threshold"))
                    .transpose()?
                    .unwrap_or(1.) as f32;
                let mut filter = runtime.filter;
                let update = filter.update_with_colors(
                    self.paint_counters[&owner].toon_fraction(),
                    threshold,
                    effect == 0,
                    runtime.blocked,
                    !runtime.initialized,
                    &self.report.counter_rim_colors,
                )?;
                if let Some(update) = update {
                    self.publish_model_constant_color(owner, update)?;
                }
                let runtime = self.counter_materials.get_mut(&owner).unwrap();
                runtime.filter = filter;
                runtime.initialized = true;
                runtime.error = None;
                if update.is_some() {
                    runtime.publications += 1;
                }
                Ok(())
            })();
            if let Err(error) = result {
                let error = format!("Counter material {}: {error:#}", self.entities[owner].name);
                self.counter_materials.get_mut(&owner).unwrap().error = Some(error.clone());
                if !self.report.unsupported.contains(&error) {
                    self.report.unsupported.push(error);
                }
            }
        }
    }
    pub(super) fn initialize_counter_transients(
        &mut self,
        range: std::ops::Range<usize>,
    ) -> Result<()> {
        let owners = self
            .counter_materials
            .keys()
            .copied()
            .filter(|owner| range.contains(owner))
            .collect::<Vec<_>>();
        for owner in owners {
            let component = self.entities[owner]
                .components
                .iter()
                .find(|component| component.class_name == "PaintThinnerCounterComponent")
                .context("Counter lifecycle component missing")?;
            let enabled = component
                .properties
                .iter()
                .find(|property| property.name == "PaintThinnerEffect")
                .map(|property| {
                    property
                        .value
                        .as_i64()
                        .context("Invalid counter effect enum")
                })
                .transpose()?
                .unwrap_or(0)
                == 0;
            if enabled {
                // Native3213e8 collects meshes in NiNode child order. The first
                // mesh supplies an existing regular1 override, not template RGB.
                let baseline = if let Some(hierarchy) = self.entity_bound_hierarchy(owner)? {
                    let mut pending = hierarchy
                        .roots
                        .iter()
                        .rev()
                        .flatten()
                        .copied()
                        .collect::<Vec<_>>();
                    let mut first = None;
                    while let Some(node) = pending.pop() {
                        let record = &hierarchy.nodes[&node];
                        if record.authored_bound.is_some() {
                            first = Some(node);
                            break;
                        }
                        pending.extend(record.children.iter().rev().flatten().copied());
                    }
                    first
                        .and_then(|node| self.render_objects.get(&(owner, node)))
                        .and_then(|state| state.regular_colors[1])
                } else {
                    None
                };
                let runtime = self.counter_materials.get_mut(&owner).unwrap();
                runtime.transient.initialize(baseline);
            }
            self.counter_materials
                .get_mut(&owner)
                .unwrap()
                .transient_initialized = true;
        }
        Ok(())
    }
    fn advance_counter_lifecycle(&mut self, delta: f32) -> Result<()> {
        ensure!(
            delta.is_finite() && delta >= 0.,
            "Invalid counter update delta"
        );
        let owners = self.counter_materials.keys().copied().collect::<Vec<_>>();
        for owner in owners {
            if !self.entity_scene_active(owner) {
                continue;
            }
            let component = self.entities[owner]
                .components
                .iter()
                .find(|component| component.class_name == "PaintThinnerCounterComponent")
                .context("Counter lifecycle component missing")?;
            let enabled = component
                .properties
                .iter()
                .find(|property| property.name == "PaintThinnerEffect")
                .map(|property| {
                    property
                        .value
                        .as_i64()
                        .context("Invalid counter effect enum")
                })
                .transpose()?
                .unwrap_or(0)
                == 0;
            let runtime = self.counter_materials.get_mut(&owner).unwrap();
            ensure!(
                runtime.transient_initialized,
                "Counter transient initialization missing"
            );
            // Native3637c4 decrements only a positive timer, clamping at0.
            if runtime.invulnerable_remaining > 0. {
                runtime.invulnerable_remaining = (runtime.invulnerable_remaining - delta).max(0.);
            }
            let color = if enabled {
                runtime.transient.advance(delta)?
            } else {
                None
            };
            if let Some(color) = color {
                self.publish_model_color(owner, 1, color, false)?;
            }
            // Native363818 publishes A2 after the transient helper.
            self.counter_materials
                .get_mut(&owner)
                .unwrap()
                .status_not_thinned = self.paint_counters[&owner].status() != 2;
        }
        Ok(())
    }
    /// Color-feedback stage364160..3641a0. Call after the native value,
    /// status, zero-health and invulnerability stages, not from a scalar setter.
    pub(super) fn activate_counter_impact(&mut self, owner: usize, amount: f32) -> Result<bool> {
        ensure!(
            !self.removed_entities.contains(&owner),
            "Counter impact owner was removed"
        );
        let color = self.report.counter_impact_colors.impact_rgba(amount)?;
        let component = self
            .entities
            .get(owner)
            .context("Counter impact owner missing")?
            .components
            .iter()
            .find(|component| component.class_name == "PaintThinnerCounterComponent")
            .context("Counter impact component missing")?;
        let effect = component
            .properties
            .iter()
            .find(|property| property.name == "PaintThinnerEffect")
            .map(|property| {
                property
                    .value
                    .as_i64()
                    .context("Invalid counter effect enum")
            })
            .transpose()?
            .unwrap_or(0);
        if effect != 0 {
            return Ok(false);
        }
        let runtime = self
            .counter_materials
            .get_mut(&owner)
            .context("Counter impact workspace missing")?;
        ensure!(
            runtime.transient_initialized,
            "Counter impact before transient initialization"
        );
        runtime.transient.activate(1., color)
    }
    pub fn entity_world_bounds(
        &self,
        entity: usize,
        parent: &epic_mickey_lib_rs::nif_file::NiTransform,
        previous: &BTreeMap<u32, super::scene_bounds::Sphere>,
    ) -> Result<BTreeMap<u32, super::scene_bounds::Sphere>> {
        let hierarchy = self
            .entity_bound_hierarchy(entity)?
            .context("World-bound owner lacks model hierarchy")?;
        let result = if let Some(playback) = self
            .actor_playback
            .as_ref()
            .filter(|playback| playback.entity == entity)
        {
            hierarchy.actor_world_bounds(parent, previous, &playback.pose.transforms)
        } else if let Some(actor) = self.actors.get(&entity) {
            let empty = BTreeMap::new();
            let poses = actor
                .pose
                .as_ref()
                .map(|pose| &pose.transforms)
                .or_else(|| {
                    self.model_animation
                        .get(&entity)
                        .map(|animation| &animation.poses)
                });
            hierarchy.actor_world_bounds(parent, previous, poses.unwrap_or(&empty))
        } else if let Some(animation) = self.model_animation.get(&entity) {
            animation.bounds(hierarchy, parent, previous)
        } else {
            hierarchy.static_world_bounds(Some(parent), previous)
        };
        if self.demo_story.enabled && result.is_err() {
            // Demo fallback for unfinished animated-bound producers. Use the
            // rendered mesh boxes at their current placements; never fabricate
            // a zero-radius bound at the entity origin.
            let mut minimum = [f32::INFINITY; 3];
            let mut maximum = [f32::NEG_INFINITY; 3];
            for draw in self.entities[entity]
                .draws
                .clone()
                .filter_map(|i| self.draws.get(i))
            {
                if !draw
                    .aabb_min
                    .iter()
                    .chain(&draw.aabb_max)
                    .all(|v| v.is_finite())
                {
                    continue;
                }
                for corner in 0..8 {
                    let point: [f32; 3] = std::array::from_fn(|i| {
                        if corner & (1 << i) == 0 {
                            draw.aabb_min[i]
                        } else {
                            draw.aabb_max[i]
                        }
                    });
                    let world: [f32; 3] = std::array::from_fn(|i| {
                        draw.trans_mat[i] * point[0]
                            + draw.trans_mat[4 + i] * point[1]
                            + draw.trans_mat[8 + i] * point[2]
                            + draw.trans_mat[12 + i]
                    });
                    for i in 0..3 {
                        minimum[i] = minimum[i].min(world[i]);
                        maximum[i] = maximum[i].max(world[i]);
                    }
                }
            }
            if minimum.iter().chain(&maximum).all(|v| v.is_finite()) {
                let center = std::array::from_fn(|i| (minimum[i] + maximum[i]) * 0.5);
                let radius = (0..3)
                    .map(|i| (maximum[i] - center[i]).powi(2))
                    .sum::<f32>()
                    .sqrt();
                let bound = super::scene_bounds::Sphere { center, radius };
                return Ok(hierarchy.nodes.keys().map(|&id| (id, bound)).collect());
            }
        }
        result
    }
    /// Native2969f0 uses the SceneGraph root NiNode's world-bound center;
    /// without that root it uses the entity position (296c44). A missing
    /// runtime bound producer is an error, not permission to aim at its origin.
    pub(super) fn camera_focus_point(&self, entity: usize) -> Result<Option<[f32; 3]>> {
        // Native1e39b0 resolves a generation-checked handle on every update.
        if self.removed_entities.contains(&entity) {
            return Ok(None);
        }
        if let Some(tracker) = self
            .mickey_tracker
            .as_ref()
            .filter(|tracker| tracker.owner == entity)
        {
            ensure!(
                tracker.error.is_none(),
                "MickeyTracker update unavailable: {}",
                tracker.error.as_deref().unwrap_or("")
            );
        }
        self.entity_position_or_bound_center(entity)
    }
    /// Shared2969f0 query used by camera focus and multi-position audio.
    /// The root NiNode's world-bound center takes precedence over translation.
    pub(super) fn entity_position_or_bound_center(
        &self,
        entity: usize,
    ) -> Result<Option<[f32; 3]>> {
        if self.removed_entities.contains(&entity) {
            return Ok(None);
        }
        let runtime = self
            .entities
            .get(entity)
            .context("Spatial source entity missing")?;
        let position: [f32; 3] = runtime.transform[12..15].try_into().unwrap();
        let Some(graph) = self.entity_bound_hierarchy(entity)? else {
            return Ok(Some(position));
        };
        let roots = graph.roots.iter().flatten().copied().collect::<Vec<_>>();
        if roots.is_empty() {
            return Ok(Some(position));
        }
        ensure!(
            roots.len() == 1,
            "Camera focus SceneGraph root selection requires a single model root"
        );
        let root = roots[0];
        let node = &graph.nodes[&root];
        //296b8c's RTTI walk requires NiNode ancestry. These are the known
        // parsed classes; unknown derived roots must establish their ancestry.
        if matches!(
            node.class_name.as_str(),
            "NiMesh" | "NiCamera" | "NiPSParticleSystem"
        ) {
            return Ok(Some(position));
        }
        ensure!(
            matches!(
                node.class_name.as_str(),
                "NiNode" | "JPSJigsawNode" | "NiSortAdjustNode" | "NiBillboardNode"
            ),
            "Unverified camera focus root ancestry: {}",
            node.class_name
        );
        let parent = epic_mickey_lib_rs::nif_file::NiTransform {
            rotation: epic_mickey_lib_rs::nif_file::Matrix3 {
                rows: std::array::from_fn(|r| {
                    std::array::from_fn(|c| runtime.runtime_rotation[r * 3 + c])
                }),
            },
            translation: epic_mickey_lib_rs::nif_file::Vector3 {
                x: position[0],
                y: position[1],
                z: position[2],
            },
            scale: runtime.authored_scale,
        };
        let empty = BTreeMap::new();
        let bounds = self.entity_world_bounds(
            entity,
            &parent,
            self.spherical_bounds.get(&entity).unwrap_or(&empty),
        )?;
        Ok(Some(
            bounds
                .get(&root)
                .context("Camera focus root world bound missing")?
                .center,
        ))
    }
    /// Resolve authored scene-local transform links into unique runtime IDs.
    /// Unresolved external parents remain errors, not accidentally shared
    /// properties on an unrelated entity with the same serialized ID.
    pub fn lighting_owner_links(
        &self,
        properties: &std::collections::BTreeMap<u64, u64>,
    ) -> Result<std::collections::BTreeMap<u64, super::spherical_lighting::OwnerLink>> {
        let mut identities = HashMap::new();
        for (index, entity) in self.entities.iter().enumerate() {
            ensure!(
                identities
                    .insert((entity.scene.as_str(), entity.id), index as u64)
                    .is_none(),
                "Duplicate runtime entity {}:{}",
                entity.scene,
                entity.id
            );
        }
        self.entities
            .iter()
            .enumerate()
            .map(|(index, entity)| {
                let parent = entity
                    .transform_parent_id()?
                    .map(|id| {
                        identities
                            .get(&(entity.scene.as_str(), id))
                            .copied()
                            .with_context(|| {
                                format!(
                                    "Unresolved transform parent {id} on {}:{}",
                                    entity.scene, entity.name
                                )
                            })
                    })
                    .transpose()?;
                let id = index as u64;
                Ok((
                    id,
                    super::spherical_lighting::OwnerLink {
                        parent,
                        property: properties.get(&id).copied(),
                    },
                ))
            })
            .collect()
    }
    /// Provisional instantaneous endpoints; native overlap gating, transition
    /// scheduling, stimulus events and animated TEV registers remain pending.
    pub fn preview_jigsaw_contact(&mut self, owner: usize, painted: bool) -> Result<bool> {
        let Some(source) = self.jigsaw_owners.get(&owner).copied() else {
            return Ok(false);
        };
        let instance = &self.jigsaw_instances[source.instance];
        if self.entities[instance.entity].jigsaw_states[instance.node].painted[source.piece]
            == painted
        {
            return Ok(false);
        }
        self.publish_jigsaw_piece_collision(owner, painted)?;
        let instance = &mut self.jigsaw_instances[source.instance];
        let entity = &mut self.entities[instance.entity];
        let asset = &self.jigsaws[entity
            .jigsaw_asset
            .as_ref()
            .context("Missing jigsaw asset")?][instance.node];
        instance.topology.set_thinned(
            &asset.pieces,
            &asset.adjacency,
            source.piece,
            !painted,
            |mesh, visible| {
                for &draw in &instance.mesh_draws[&mesh] {
                    if visible {
                        self.hidden_jigsaw_draws.remove(&draw);
                    } else {
                        self.hidden_jigsaw_draws.insert(draw);
                    }
                }
            },
        )?;
        let piece = &asset.pieces[source.piece];
        for mesh in piece.mesh_links.iter().chain(&piece.secondary_mesh_links) {
            for &index in &instance.mesh_draws[mesh] {
                let state = &instance.render_states[&index][usize::from(!painted)];
                // Preserve live transforms/geometry; swap only the material endpoint.
                let draw = &mut self.draws[index];
                draw.material = state.material.clone();
                draw.material_images = state.material_images.clone();
                draw.preview = state.preview.clone();
                if painted {
                    self.sketch_draws.remove(&index);
                } else {
                    self.sketch_draws.insert(index);
                }
            }
        }
        Ok(true)
    }

    pub(super) fn imported_static_part(&self, owner: usize) -> Option<&CollisionPart> {
        let part = self
            .report
            .collision_parts
            .get(*self.static_collision_owner_parts.get(&owner)?)?;
        (part.owner == owner).then_some(part)
    }
    pub fn collision_entity(&self, part: usize) -> Option<&RuntimeEntity> {
        self.entities.get(self.collision_entity_index(part)?)
    }
    pub(super) fn collision_entity_index(&self, part: usize) -> Option<usize> {
        if self
            .player_physics
            .as_ref()
            .is_some_and(|body| body.owner == part)
        {
            return (self.player_entity < self.entities.len()).then_some(self.player_entity);
        }
        if let Some((&entity, _)) = self
            .character_physics
            .iter()
            .find(|(_, body)| body.as_ref().is_ok_and(|body| body.owner == part))
        {
            return (entity < self.entities.len()).then_some(entity);
        }
        if let Some((&source, _)) = self
            .dynamic_runtime_sources
            .iter()
            .find(|(_, owner)| **owner == part)
        {
            let entity = self.dynamic_bodies.get(source)?.entity;
            return (entity < self.entities.len()).then_some(entity);
        }
        if let Some(source) = self.jigsaw_owners.get(&part) {
            let entity = self.jigsaw_instances.get(source.instance)?.entity;
            return (entity < self.entities.len()).then_some(entity);
        }
        let source = self.imported_static_part(part)?;
        self.entities
            .iter()
            .position(|e| e.id == source.entity_id && e.scene == source.scene)
    }
    /// Resolve an original triangle ID from the active Box3D contact snapshot.
    /// Static owner relocation uses the current unscaled transform. Dynamic
    /// bodies and jigsaws still need their own pose/shape ownership paths.
    pub fn initial_static_contact(
        &self,
        owner: usize,
        source_triangle: usize,
    ) -> Result<(
        super::collision::TriangleContactShape,
        super::native_hk_transform::Transform,
    )> {
        let part = self
            .imported_static_part(owner)
            .context("Contact is not an imported static part")?;
        let local = source_triangle
            .checked_sub(part.triangle_start)
            .context("Contact precedes static part")?;
        ensure!(
            local < part.triangle_count,
            "Contact belongs to another static part"
        );
        let mesh = part
            .native_resource
            .get(part.native_part)
            .context("Missing native static part")?;
        let welding = part
            .native_welding
            .get(&part.body_offset)
            .context("Static contact has no prepared welding")?;
        let entity = self
            .collision_entity(owner)
            .context("Static contact has no entity")?;
        let transform = super::native_hk_transform::initial_body(
            &entity.physics_transform(),
            &mesh.body_transform,
        )?;
        Ok((mesh.prepared_triangle_contact(local, welding)?, transform))
    }
    /// Resolve source identities supplied by actual Box3D candidate callbacks.
    /// Jigsaw/dynamic owners require their own binding, never a static fallback.
    pub fn imported_static_candidate(
        &self,
        candidate: physics::SphereCandidate,
    ) -> Result<ImportedStaticCandidate> {
        let part = self
            .imported_static_part(candidate.owner)
            .context("Sweep owner is not an imported static part")?;
        ensure!(
            self.collision.shape_owner(candidate.shape) == Some(candidate.owner),
            "Sweep shape does not belong to its reported static owner"
        );
        let mesh = part
            .native_resource
            .get(part.native_part)
            .context("Missing native sweep part")?;
        ensure!(
            mesh.body_offset == part.body_offset,
            "Static sweep body binding changed"
        );
        let local = match candidate.source_triangle {
            Some(source) => {
                let local = source
                    .checked_sub(part.triangle_start)
                    .context("Sweep triangle precedes static part")?;
                ensure!(
                    local < part.triangle_count,
                    "Sweep triangle belongs to another static part"
                );
                Some(local)
            }
            None => None,
        };
        Ok(ImportedStaticCandidate {
            part: candidate.owner,
            body_offset: mesh.body_offset,
            shape_key: mesh.shape_keys.key(local)?,
            serialized_body_filter: mesh.body_filter_info,
            serialized_child_filter: mesh.child_filters.at(local)?,
            serialized_quality: mesh.motion.quality,
        })
    }
}
pub struct PlayerMesh {
    pub draw: usize,
    pub asset: String,
    pub block: usize,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub sources: Vec<u32>,
    pub transform: [[f32; 4]; 4],
}
fn key(s: &str) -> String {
    super::resource_path::canonical(s)
        .unwrap_or_else(|_| s.replace('\\', "/"))
        .trim_matches('/')
        .to_ascii_lowercase()
}
fn fast_key_eq(raw: &str, target: &str) -> bool {
    let trimmed = raw.trim_matches(|c| c == '/' || c == '\\');
    if trimmed.contains("..") || trimmed.contains("//") || trimmed.contains("\\\\") || trimmed.contains("/.") || trimmed.contains("\\.") {
        return key(raw) == target;
    }
    let a = trimmed.as_bytes();
    let b = target.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).all(|(&ca, &cb)| {
        let na = if ca == b'\\' { b'/' } else { ca.to_ascii_lowercase() };
        na == cb
    })
}
fn lookup<'a>(files: &'a [VirtualFile], path: &str) -> Option<&'a VirtualFile> {
    let k = key(path);
    let kw = format!("{k}_wii");
    files
        .iter()
        .find(|f| fast_key_eq(&f.path, &k) || fast_key_eq(&f.path, &kw))
}
fn pack(root: &Path, name: &str, files: &mut Vec<VirtualFile>, report: &mut Report) -> Result<()> {
    let p = root.join("packfiles").join(name);
    let p = if p.exists() {
        p
    } else {
        std::fs::read_dir(root.join("packfiles"))?
            .filter_map(Result::ok)
            .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
            .context(format!("Missing pack {name}"))?
            .path()
    };
    if report.packs.iter().any(|n| n.eq_ignore_ascii_case(name)) {
        return Ok(());
    }
    let members = crate::pack_io::index(&p)?;
    let mut archive = std::fs::File::open(&p)?;
    let mut existing: HashSet<_> = files.iter().map(|f| key(&f.path)).collect();
    for member in members {
        if existing.insert(key(&member.path)) {
            let data = member.read_from(&mut archive)?;
            files.push(VirtualFile::new(
                epic_mickey_lib_rs::packfile::EndianDependentString::new(member.tag),
                member.size != member.packed_size,
                6,
                member.path,
                data,
            ));
        }
    }
    report.packs.push(name.into());
    Ok(())
}
fn descriptor(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect()
}
fn descriptor_bytes(bytes: &[u8]) -> Result<HashMap<String, String>> {
    // Retail descriptors contain legacy-encoded author comments. Decode only
    // the fields used for resource resolution; unrelated metadata is not a
    // reason to reject an otherwise valid level.
    let mut values = HashMap::new();
    for line in bytes.split(|&b| b == b'\n') {
        let Some(at) = line.iter().position(|&b| b == b'=') else {
            continue;
        };
        let Ok(name) = std::str::from_utf8(&line[..at]) else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        if matches!(name.as_str(), "folder" | "initial" | "shared" | "interior") {
            let value = std::str::from_utf8(&line[at + 1..])
                .with_context(|| format!("Invalid level resource field {name}"))?;
            values.insert(name, value.trim().to_owned());
        }
    }
    Ok(values)
}
fn startup_part(values: &HashMap<String, String>) -> Result<Option<String>> {
    Ok(startup_parts(values)?.into_iter().next())
}
fn startup_parts(values: &HashMap<String, String>) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    for field in ["initial", "interior"] {
        let Some(path) = startup_part_path(values, field)? else {
            continue;
        };
        if !paths
            .iter()
            .any(|existing: &String| key(existing) == key(&path))
        {
            paths.push(path);
        }
    }
    Ok(paths)
}
fn startup_part_path(values: &HashMap<String, String>, field: &str) -> Result<Option<String>> {
    let Some(part) = values.get(field).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let folder = values.get("folder").context("Level has no folder")?;
    let suffix = if key(part).ends_with(".part") {
        ""
    } else {
        ".part"
    };
    Ok(Some(super::resource_path::canonical(&format!(
        "Levels/{folder}/{part}{suffix}"
    ))?))
}
fn startup_scene_paths(
    root: &Path,
    files: &[VirtualFile],
    values: &HashMap<String, String>,
) -> Result<(Vec<String>, String, [String; 2], Vec<String>)> {
    //28fc90..28fdc0 forces both startup parts active, ignoring -i. Later
    // streamed part replacement obeys its options; keep their owners separate.
    let mut groups = [String::new(), String::new()];
    let mut cache: HashMap<String, String> = HashMap::new();
    let mut missing = Vec::new();
    for (index, field) in ["initial", "interior"].into_iter().enumerate() {
        let Some(path) = startup_part_path(values, field)? else {
            continue;
        };
        let data = if let Some(data) = cache.get(&key(&path)) {
            data.clone()
        } else {
            let bytes = match read_descriptor(root, files, &path) {
                Ok(bytes) => bytes,
                Err(error)
                    if field == "interior"
                        && !root.join(path.replace('\\', "/")).is_file()
                        && lookup(files, &path).is_none() =>
                {
                    // A retail level can reference an optional streamed start/interior
                    // part omitted by a partial extraction. Keep the shared scene
                    // playable and report the missing segment instead of refusing
                    // to open the entire level.
                    missing.push(format!("Startup part {path} is absent: {error:#}"));
                    continue;
                }
                Err(error) => return Err(error),
            };
            let data = std::str::from_utf8(&bytes)
                .with_context(|| format!("Startup part {path}"))?
                .to_owned();
            cache.insert(key(&path), data.clone());
            data
        };
        groups[index] = data;
    }
    let text = groups.join("\n");
    Ok((
        initial_scenes(values.get("shared").map(String::as_str), &text)?,
        text,
        groups,
        missing,
    ))
}
// Retain options: native bootstrap ignores -i, later part replacement does not.
fn scene_reference(line: &str) -> Result<Option<(String, Vec<String>)>> {
    let line = line.trim();
    if line.is_empty() || line.starts_with("//") || line.starts_with([';', '#']) {
        return Ok(None);
    }
    let mut words = line.split_whitespace();
    let path = words.next().unwrap();
    let normalized = key(path);
    let cooked = if normalized.ends_with(".gsa") {
        format!("{}.bin", &path[..path.len() - 4])
    } else if normalized.ends_with(".bin") {
        path.to_owned()
    } else {
        return Ok(None);
    };
    let options = words.map(str::to_owned).collect::<Vec<_>>();
    ensure!(
        options
            .iter()
            .all(|v| v.eq_ignore_ascii_case("-i") || v.eq_ignore_ascii_case("-inactive")),
        "Unsupported scene reference options: {line}"
    );
    Ok(Some((cooked, options)))
}
// A level may consist entirely of its shared scene; an initial part can contain
// only soundbanks (BuildingStandards.level / BSTEST/HotStream.part).
fn initial_scenes(shared: Option<&str>, part: &str) -> Result<Vec<String>> {
    let mut scenes = Vec::new();
    let mut seen = HashSet::new();
    for line in shared.into_iter().chain(part.lines()) {
        if let Some((path, _)) = scene_reference(line)? {
            if seen.insert(key(&path)) {
                scenes.push(path);
            }
        }
    }
    ensure!(
        !scenes.is_empty(),
        "Level shared scene and initial part contain no scene references"
    );
    Ok(scenes)
}

#[cfg(target_os = "horizon")]
pub fn default_root() -> Option<PathBuf> {
    for candidate in [
        "sdmc:/blotworx/DATA/files",
        "sdmc:/blotworx/DATA",
        "sdmc:/blotworx/files",
        "sdmc:/DATA/files",
        "sdmc:/DATA",
        "romfs:/DATA/files",
        "romfs:/DATA",
    ] {
        let p = PathBuf::from(candidate);
        let root = files_root(&p);
        if root.join("packfiles").is_dir() {
            return Some(root);
        }
    }
    None
}

#[cfg(not(target_os = "horizon"))]
pub fn default_root() -> Option<PathBuf> {
    let base = dirs::data_local_dir()?.join("BlueThinner/builds");
    let mut candidates: Vec<_> = std::fs::read_dir(base)
        .ok()?
        .flatten()
        .filter(|e| e.path().join("DATA/files/packfiles/levels.pak").exists())
        .collect();
    candidates.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
    candidates.pop().map(|e| e.path().join("DATA/files"))
}
pub(super) fn files_root(root: &Path) -> PathBuf {
    // Accept Explorer's "Copy as path" text and the packfiles folder itself,
    // as well as the disc, DATA and files directories. All consumers (including
    // the catalog) must resolve the same root as the actual importer.
    let root_text = root.to_string_lossy();
    let root = Path::new(root_text.trim().trim_matches('"'));
    if root
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("packfiles"))
        && root.is_dir()
    {
        root.parent().unwrap_or(root).to_owned()
    } else if root.join("DATA/files").is_dir() {
        root.join("DATA/files")
    } else if root.join("files/packfiles").is_dir() {
        root.join("files")
    } else {
        root.to_owned()
    }
}
/// Import the shared CLB once, preserving both owners and authored indices.
pub fn collectible_owners_from_game(
    root: &Path,
) -> Result<(super::collectibles::State, super::extras::State)> {
    let root = files_root(root);
    let mut files = Vec::new();
    let mut report = Report::default();
    pack(&root, "collectibles.pak", &mut files, &mut report)?;
    super::extras::State::load_database(
        &lookup(&files, "collectibles.clb")
            .context("Missing collectibles database")?
            .data,
    )
}
/// Read only the quest database for save inspection; no scene payload import.
pub fn quest_owner_from_game(root: &Path) -> Result<super::quest_state::State> {
    let root = files_root(root);
    let archive = std::fs::read_dir(root.join("packfiles"))?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("bsq.pak")
        })
        .context("Missing bsq.pak")?
        .path();
    let path = "Apprentice/GlobalState/ApprenticeQuests.bsq";
    let stored =
        crate::pack_io::read_filtered(std::fs::read(archive)?, |_, name| key(name) == key(path))?;
    let database = lookup(&stored.pak.files, path).context("Quest database absent from bsq.pak")?;
    super::quest_state::State::load(&database.path, &database.data)
}
/// Index descriptors without inflating scene or model payloads.
pub fn available_levels(root: &Path) -> Result<Vec<String>> {
    Ok(level_catalog(root)?
        .into_iter()
        .map(|entry| entry.path)
        .collect())
}
#[derive(Debug)]
pub struct LevelEntry {
    pub path: String,
    pub unavailable: Option<String>,
}
/// Validate descriptor dependencies, not just the presence of an old descriptor.
/// Retail archives retain development levels whose referenced parts were never shipped.
pub fn level_catalog(root: &Path) -> Result<Vec<LevelEntry>> {
    let root = files_root(root);
    let packs = std::fs::read_dir(root.join("packfiles"))?.collect::<std::io::Result<Vec<_>>>()?;
    let archive = packs
        .iter()
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("levels.pak")
        })
        .context("Missing levels.pak")?
        .path();
    let stored = crate::pack_io::read_filtered(std::fs::read(archive)?, |_, path| {
        key(path).ends_with(".level") || key(path).ends_with(".part")
    })?;
    let mut levels = stored
        .pak
        .files
        .iter()
        .filter(|file| key(&file.path).ends_with(".level"))
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    // Project builds may contain new or overridden loose descriptors.
    let mut pending = vec![root.join("Levels")];
    while let Some(folder) = pending.pop() {
        if !folder.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&folder)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("level"))
            {
                levels.push(
                    entry
                        .path()
                        .strip_prefix(&root)?
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    levels.sort_by_key(|path| key(path));
    levels.dedup_by(|a, b| key(a) == key(b));
    let mut result = Vec::new();
    for path in levels {
        let check = (|| -> Result<()> {
            let bytes = read_descriptor(&root, &stored.pak.files, &path)?;
            let values = descriptor_bytes(&bytes)?;
            for scene in startup_scene_paths(&root, &stored.pak.files, &values)?.0 {
                let stem = Path::new(&scene)
                    .file_stem()
                    .context("Scene has no filename")?
                    .to_string_lossy();
                let name = format!("{stem}.pak");
                ensure!(
                    packs.iter().any(|entry| entry
                        .file_name()
                        .to_string_lossy()
                        .eq_ignore_ascii_case(&name)),
                    "Required scene pack is absent: {name} ({scene})"
                );
            }
            Ok(())
        })();
        result.push(LevelEntry {
            path,
            unavailable: check.err().map(|error| format!("{error:#}")),
        });
    }
    Ok(result)
}
fn read_descriptor(root: &Path, files: &[VirtualFile], path: &str) -> Result<Vec<u8>> {
    let loose = root.join(path.replace('\\', "/"));
    if loose.is_file() {
        return std::fs::read(&loose).with_context(|| format!("Read {}", loose.display()));
    }
    Ok(lookup(files, path)
        .with_context(|| format!("Required descriptor is absent: {path}"))?
        .data
        .clone())
}

pub(super) fn loading_artwork(root: &Path, requested: &str) -> Result<Option<String>> {
    let archive = std::fs::read_dir(root.join("packfiles"))?
        .filter_map(Result::ok)
        .find(|entry| entry.file_name().eq_ignore_ascii_case("levels.pak"))
        .context("Missing levels.pak")?
        .path();
    let levels = crate::pack_io::read_filtered(std::fs::read(archive)?, |_, path| {
        key(path).ends_with(".level")
    })?;
    let path = resolve_level_path(root, &levels.pak.files, requested)?;
    let bytes = read_descriptor(root, &levels.pak.files, &path)?;
    let mut fields = descriptor(&String::from_utf8_lossy(&bytes));
    Ok(fields.remove("loading_screen_texture").filter(|path| !path.is_empty()))
}

/// Resolve user input to an asset path before descriptor parsing. A filename is
/// a selector, not a root-relative resource name; never choose an ambiguous one.
fn resolve_level_path(root: &Path, files: &[VirtualFile], requested: &str) -> Result<String> {
    let text = requested.trim().trim_matches('"').replace('\\', "/");
    ensure!(!text.is_empty(), "Choose a level first");
    let path = Path::new(&text);
    let relative = if path.is_absolute() {
        let root = root.canonicalize().context("Resolve game data folder")?;
        let selected = path
            .canonicalize()
            .with_context(|| format!("Level file does not exist: {text}"))?;
        selected
            .strip_prefix(&root)
            .context("Choose a level inside the selected game data folder")?
            .to_string_lossy()
            .replace('\\', "/")
    } else {
        text
    };
    let relative = super::resource_path::canonical(&relative)?;
    ensure!(
        key(&relative).ends_with(".level"),
        "Choose a .level descriptor; a scene .bin does not include the level's startup parts"
    );
    if root.join(&relative).is_file() {
        return Ok(relative);
    }
    if let Some(file) = lookup(files, &relative) {
        return Ok(file.path.clone());
    }
    if !relative.contains('/') {
        let loose = format!("Levels/{relative}");
        if root.join(&loose).is_file() {
            return Ok(loose);
        }
        let mut matches = files
            .iter()
            .filter(|file| {
                file.path
                    .replace('\\', "/")
                    .rsplit('/')
                    .next()
                    .is_some_and(|name| name.eq_ignore_ascii_case(&relative))
            })
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();
        matches.sort_by_key(|path| key(path));
        matches.dedup_by(|a, b| key(a) == key(b));
        ensure!(
            matches.len() <= 1,
            "Level filename is ambiguous: {}. Select its full asset path.",
            matches.join(", ")
        );
        if let Some(path) = matches.pop() {
            return Ok(path);
        }
    }
    anyhow::bail!("Level descriptor not found: {relative}. Choose a level from the level list or select a .level file in this game's data folder.")
}
#[cfg(test)]
mod level_selection_tests {
    use super::*;
    #[test]
    fn level_filename_resolution_is_unique_and_preserves_asset_identity() -> Result<()> {
        let file = |path: &str| {
            VirtualFile::new(
                epic_mickey_lib_rs::packfile::EndianDependentString::new("TEXT".into()),
                false,
                0,
                path.into(),
                Vec::new(),
            )
        };
        let files = vec![
            file("Levels/HM_Start.Level"),
            file("Levels/Sub/Other.level"),
        ];
        let root = Path::new("target/nonexistent-level-selection-root");
        for input in [
            "HM_Start.level",
            "  \"hm_start.LEVEL\"  ",
            "Levels\\HM_Start.Level",
        ] {
            assert_eq!(
                resolve_level_path(root, &files, input)?,
                "Levels/HM_Start.Level"
            );
        }
        assert_eq!(
            resolve_level_path(root, &files, "other.level")?,
            "Levels/Sub/Other.level"
        );
        let mut ambiguous = files.clone();
        ambiguous.push(file("Levels/Development/Other.level"));
        assert!(resolve_level_path(root, &ambiguous, "Other.level")
            .unwrap_err()
            .to_string()
            .contains("ambiguous"));
        for input in ["../outside.level", "missing.level", "", "Scenes/foo.bin"] {
            assert!(resolve_level_path(root, &files, input).is_err(), "{input}");
        }
        Ok(())
    }
    #[test]
    fn selected_loose_level_stays_inside_game_root() -> Result<()> {
        struct Directory(PathBuf);
        impl Drop for Directory {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(self.0.join("files/Levels/Custom.level"));
                let _ = std::fs::remove_file(self.0.join("Other.level"));
                for path in [
                    self.0.join("files/Levels"),
                    self.0.join("files"),
                    self.0.clone(),
                ] {
                    let _ = std::fs::remove_dir(path);
                }
            }
        }
        let directory = Directory(std::env::temp_dir().join(
            format!("blotworx-level-selection-{}-{}",std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos()),
        ));
        let root = directory.0.join("files");
        std::fs::create_dir_all(root.join("Levels"))?;
        let selected = root.join("Levels/Custom.level");
        std::fs::write(&selected, b"Shared=Custom.gsa")?;
        assert_eq!(
            resolve_level_path(&root, &[], &format!("\"{}\"", selected.display()))?,
            "Levels/Custom.level"
        );
        assert_eq!(
            resolve_level_path(&root, &[], "Custom.level")?,
            "Levels/Custom.level"
        );
        let outside = directory.0.join("Other.level");
        std::fs::write(&outside, b"")?;
        assert!(resolve_level_path(&root, &[], &outside.to_string_lossy()).is_err());
        Ok(())
    }
}
#[derive(Clone)]
pub struct MorphPreview {
    pub model: String,
    pub keyframes: String,
    pub sequence: String,
    pub time: f32,
}
/// Shared jigsaw publication for initial loading and streamed scene batches.
#[allow(clippy::too_many_arguments)]
fn initialize_jigsaw_entities(
    entities: &[RuntimeEntity],
    range: std::ops::Range<usize>,
    jigsaws: &HashMap<String, Vec<super::jigsaw::Asset>>,
    collision: &mut Collision,
    draws: &mut [GpuDrawCommand],
    report: &mut Report,
    sketch_shaders: &super::sketch_render::Shaders,
    files: &[VirtualFile],
    textures: &mut HashMap<String, Option<DecodedImage>>,
    sources: &material_preview::Sources,
    effect_templates: &HashMap<String, Arc<super::jigsaw_effects::Template>>,
    sketch_draws: &mut HashSet<usize>,
    hidden_jigsaw_draws: &mut HashSet<usize>,
    jigsaw_instances: &mut Vec<JigsawInstance>,
    jigsaw_owners: &mut HashMap<usize, JigsawOwner>,
    next_owner: &mut usize,
) -> Result<()> {
    for entity_index in range {
        let entity = &entities[entity_index];
        let Some(asset_key) = &entity.jigsaw_asset else {
            continue;
        };
        let component = entity
            .components
            .iter()
            .find(|c| c.class_name == "PhysicsJigsawComponent")
            .with_context(|| format!("{}: missing jigsaw component", entity.name))?;
        let allow_nonoverlapping_painting = super::jigsaw_overlap::load_key_scope(component)
            .with_context(|| format!("{}: jigsaw restoration settings", entity.name))?;
        for (node_index, asset) in jigsaws[asset_key].iter().enumerate() {
            ensure!(asset.collision_root_identity,"{}: nested/transformed jigsaw collision root needs native transform reconstruction",entity.name);
            let instance = jigsaw_instances.len();
            let binding = super::jigsaw_collision::Binding::install(
                asset,
                &entity.jigsaw_states[node_index],
                &entity.transform,
                collision,
                |piece| {
                    let owner = *next_owner;
                    *next_owner += 1;
                    jigsaw_owners.insert(owner, JigsawOwner { instance, piece });
                    owner
                },
            )
            .with_context(|| format!("Bind jigsaw collision for {}", entity.name))?;
            let mut mesh_draws = HashMap::new();
            for piece in &asset.pieces {
                for &mesh in piece.mesh_links.iter().chain(&piece.secondary_mesh_links) {
                    let draws = entity.mesh_draws.get(&mesh).with_context(|| {
                        format!("{}: jigsaw mesh {mesh} has no rendered draw", entity.name)
                    })?;
                    mesh_draws.insert(mesh, draws.clone());
                }
            }
            let mut topology = super::jigsaw_topology::Topology::new(asset.pieces.len());
            for piece in &asset.pieces {
                for mesh in &piece.secondary_mesh_links {
                    hidden_jigsaw_draws.extend(mesh_draws[mesh].iter().copied());
                }
            }
            for (piece_index, &painted) in
                entity.jigsaw_states[node_index].painted.iter().enumerate()
            {
                if !painted {
                    topology.set_thinned(
                        &asset.pieces,
                        &asset.adjacency,
                        piece_index,
                        true,
                        |mesh, visible| {
                            for &draw in &mesh_draws[&mesh] {
                                if visible {
                                    hidden_jigsaw_draws.remove(&draw);
                                } else {
                                    hidden_jigsaw_draws.insert(draw);
                                }
                            }
                        },
                    )?;
                }
            }
            let mut render_states = HashMap::new();
            let mut transition_states = HashMap::new();
            for (piece_index, piece) in asset.pieces.iter().enumerate() {
                for &mesh in piece.mesh_links.iter().chain(&piece.secondary_mesh_links) {
                    let shader =
                        super::jigsaw_material::sketch_shader(false, asset.mesh_alpha_flags[&mesh]);
                    for &draw_index in &mesh_draws[&mesh] {
                        if render_states.contains_key(&draw_index) {
                            continue;
                        }
                        let original = draws[draw_index].clone();
                        match super::jigsaw_effects::prepare(
                            &original,
                            asset.mesh_alpha_flags[&mesh],
                            effect_templates,
                            files,
                            textures,
                        ) {
                            Ok(states) => {
                                transition_states.insert(draw_index, states);
                            }
                            Err(error) => report.warnings.push(format!(
                                "{} mesh {}: transition preview uses instant endpoint: {error:#}",
                                entity.name, mesh
                            )),
                        }
                        let mut sketch = original.clone();
                        let draw = &mut sketch;
                        let mut material = draw
                            .material
                            .as_ref()
                            .context("Jigsaw draw has no material")?
                            .as_ref()
                            .clone();
                        material.shader = Some(shader.into());
                        let program = if material.textures.iter().any(|t| t.slot == 0) {
                            sketch_shaders.program(shader)?
                        } else {
                            material_preview::prepare(&material, sources, false)
                                .map_err(anyhow::Error::msg)?
                        };
                        draw.material_images = Arc::new(
                            material_preview::images(&program, &material, files, textures)
                                .map_err(anyhow::Error::msg)?,
                        );
                        draw.preview = Some(Ok(program));
                        draw.material = Some(Arc::new(material));
                        if !entity.jigsaw_states[node_index].painted[piece_index] {
                            draws[draw_index] = sketch.clone();
                            sketch_draws.insert(draw_index);
                        }
                        render_states.insert(draw_index, [original, sketch]);
                    }
                }
            }
            report.warnings.push(format!("{}: sketch previews use recovered front-surface TEV/colors; back-surface scheduling, object opacity and native transition stage composition remain unverified",entity.name));
            report.geometry.push(serde_json::json!({"kind":"jigsaw_runtime_binding","entity":entity.name,
                "node":asset.node,"collision_owners":binding.owners.iter().flatten().count(),
                "render_meshes":mesh_draws.len(),"runtime_collision_active":true,"runtime_render_state_active":true,"sketch_front_kernel_active":true}));
            jigsaw_instances.push(JigsawInstance {
                allow_nonoverlapping_painting,
                transition_states,
                render_states,
                topology,
                entity: entity_index,
                node: node_index,
                collision: binding,
                mesh_draws,
            });
        }
    }
    Ok(())
}

pub fn load(root: &Path, level: &str, ctx: &egui::Context) -> Result<World> {
    load_with_morph(root, level, ctx, None)
}
pub fn load_with_morph(
    root: &Path,
    level: &str,
    ctx: &egui::Context,
    diagnostic: Option<&MorphPreview>,
) -> Result<World> {
    load_with_particle_preview(root, level, ctx, diagnostic, None)
}
pub fn load_with_particle_preview(
    root: &Path,
    level: &str,
    ctx: &egui::Context,
    diagnostic: Option<&MorphPreview>,
    guardian: Option<&str>,
) -> Result<World> {
    load_with_random(root, level, ctx, diagnostic, guardian, Default::default())
}
pub fn load_with_random(
    root: &Path,
    level: &str,
    ctx: &egui::Context,
    diagnostic: Option<&MorphPreview>,
    guardian: Option<&str>,
    game_random: super::game_random::State,
) -> Result<World> {
    load_with_render_options(
        root,
        level,
        ctx,
        diagnostic,
        guardian,
        game_random,
        Default::default(),
    )
}
pub fn load_with_render_options(
    root: &Path,
    level: &str,
    ctx: &egui::Context,
    diagnostic: Option<&MorphPreview>,
    guardian: Option<&str>,
    game_random: super::game_random::State,
    render_options: super::render_options::State,
) -> Result<World> {
    load_with_score_state(
        root,
        level,
        ctx,
        diagnostic,
        guardian,
        game_random,
        render_options,
        Default::default(),
    )
}
pub(super) fn load_with_score_state(
    root: &Path,
    level: &str,
    ctx: &egui::Context,
    diagnostic: Option<&MorphPreview>,
    guardian: Option<&str>,
    game_random: super::game_random::State,
    render_options: super::render_options::State,
    scores: super::score_state::InitialState,
) -> Result<World> {
    load_with_bootstrap_mode(
        root,
        level,
        ctx,
        diagnostic,
        guardian,
        game_random,
        render_options,
        scores,
        false,
        false,
    )
}
pub(super) fn load_with_bootstrap_mode(
    root: &Path,
    level: &str,
    ctx: &egui::Context,
    diagnostic: Option<&MorphPreview>,
    guardian: Option<&str>,
    game_random: super::game_random::State,
    render_options: super::render_options::State,
    scores: super::score_state::InitialState,
    demo: bool,
    strict_npc_ai: bool,
) -> Result<World> {
    let root = files_root(root);
    ensure!(
        root.join("packfiles").is_dir(),
        "Choose an extracted game's files directory containing packfiles"
    );
    let mut report = Report {
        engine: "Blotworx Engine — native exploration slice".into(),
        level: level.into(),
        ..Default::default()
    };
    let mut files = vec![];
    let setting = "CharacterControllerImmediateVelocityJumpMultiplier";
    // Retail's fallback is zero; the shipped configuration supplies 0.75.
    report.motion.immediate_jump_velocity_multiplier = 0.;
    let mut runtime_settings = BTreeMap::new();
    for (name, command) in [("ConfigFiles.ini", false), ("cmdline.txt", true)] {
        let path = root.join(name);
        if path.exists() {
            let text = std::fs::read_to_string(&path)?;
            runtime_settings.extend(crate::render_support::game_config::runtime_settings(
                &text, command,
            ));
            if let Some(value) = crate::render_support::game_config::setting_value(
                &text,
                "CharacterMaintainVelocity",
                command,
            ) {
                report.motion.maintain_ground_velocity =
                    match value.trim().to_ascii_lowercase().as_str() {
                        "true" | "1" => true,
                        "false" | "0" => false,
                        _ => anyhow::bail!("Invalid CharacterMaintainVelocity in {name}: {value}"),
                    };
            }
            if let Some(value) = crate::render_support::game_config::setting_value(&text, setting, command)
            {
                let value: f32 = value
                    .parse()
                    .with_context(|| format!("Invalid {setting} in {name}"))?;
                ensure!(value.is_finite(), "Non-finite {setting} in {name}");
                report.motion.immediate_jump_velocity_multiplier = value;
            }
        }
    }
    report.jigsaw_time_scale = super::jigsaw_transition::time_scale(&runtime_settings)?;
    report.counter_rim_colors = super::paint_counter::RimColors::load(&runtime_settings)?;
    report.counter_impact_colors = super::paint_counter::RimColors::load_impact(&runtime_settings)?;
    report.jigsaw_overlap_guard_enabled = super::jigsaw_overlap::enabled(&runtime_settings)?;
    report.brush_settings = Some(super::brush::Settings::load(&runtime_settings)?);
    report.ammo_policy = Some(super::ammo::Policy::load(&runtime_settings, true)?);
    report.spin_settings = Some(super::spin::Settings::load(&runtime_settings)?);
    report.ear_settings = Some(super::ears::Settings::load(&runtime_settings)?);
    report.pickup_bob_globals = Some(super::pickup::BobGlobals::load(&runtime_settings)?);
    pack(&root, "levels.pak", &mut files, &mut report)?;
    let resolved_level = resolve_level_path(&root, &files, level)?;
    let level = resolved_level.as_str();
    report.level = resolved_level.clone();
    pack(&root, "bsq.pak", &mut files, &mut report)?;
    // Shipped always_loaded.txt includes this archive. Animation source
    // markers call its global helpers even without a scene ScriptComponent.
    pack(&root, "globalscripts.pak", &mut files, &mut report)?;
    pack(&root, "collectibles.pak", &mut files, &mut report)?;
    let (collectibles, extras) = super::extras::State::load_database(
        &lookup(&files, "collectibles.clb")
            .context("Missing collectibles database")?
            .data,
    )
    .context("Native extras initialization")?;
    let global_database = lookup(&files, "Apprentice/GlobalState/ApprenticeGlobalState.bsq")
        .context("Missing Apprentice global-state database")?;
    let global_state =
        super::global_state::State::load(&global_database.path, &global_database.data)?;
    let quest_database = lookup(&files, "Apprentice/GlobalState/ApprenticeQuests.bsq")
        .context("Missing Apprentice quest database")?;
    let quest_state = super::quest_state::State::load(&quest_database.path, &quest_database.data)?;
    report
        .geometry
        .push(serde_json::json!({"kind":"quest_state","state":quest_state.summary()}));
    report.unsupported.push("Quest conditions driven by global changes and native completion listeners remain incomplete.".into());
    let bytes = read_descriptor(&root, &files, level)?;
    let mut values = descriptor_bytes(&bytes).with_context(|| format!("Level descriptor {level}"))?;
    let restore_snapshot = scores.checkpoint.latest.as_ref()
        .filter(|_| scores.resume == Some(super::score_state::CampaignResume::Checkpoint))
        .cloned();
    let restore = restore_snapshot.as_ref()
        .map(|snapshot| super::save_game::Archive::parse(&snapshot.bytes))
        .transpose()?;
    if let Some(archive) = &restore {
        for (field, offset) in [("initial", 0x4c), ("interior", 0x88)] {
            let part = archive.header_string(offset, 60)?;
            if part.is_empty() {
                values.remove(field);
            } else {
                values.insert(field.into(), part.into());
            }
        }
    }
    let level_resource = super::checkpoint_metadata::Level::bootstrap(
        level,
        values.get("initial").map(String::as_str),
        values.get("interior").map(String::as_str),
    );
    let (mut scene_paths, part_text, part_groups, missing_parts) =
        startup_scene_paths(&root, &files, &values)?;
    if let Some(archive) = &restore {
        ensure!(missing_parts.is_empty(), "Saved level parts are missing: {}", missing_parts.join(", "));
        let section = archive.section(*b"WRLD")?;
        ensure!(section.version == 1, "Unsupported WRLD save version");
        scene_paths = initial_scenes(None, &super::save_world::decode(&section.payload)?.join("\n"))?;
        ensure!(!scene_paths.is_empty(), "Checkpoint has no active scenes");
    }
    report.warnings.extend(missing_parts);
    let mut scene_resources = super::scene_resources::State::bootstrap(
        values.get("shared").map(String::as_str),
        &part_groups,
    );
    for line in values
        .get("shared")
        .map(String::as_str)
        .into_iter()
        .chain(part_text.lines())
    {
        if let Some((scene, options)) = scene_reference(line)? {
            if !options.is_empty() {
                report.geometry.push(serde_json::json!({"kind":"scene_reference_options","scene":scene,"options":options,
                    "bootstrap_ignores_inactive":true,"native_option_flags_verified":true,"native_loader_timing_verified":false}));
            }
        }
    }
    // Global definitions are authoritative and their assets take precedence over embedded copies.
    pack(&root, "_dynamic.pak", &mut files, &mut report)?;
    let globals = lookup(&files, "Palettes/_Dynamic.bin")
        .map(|f| SceneFile::from_binary(&f.data, EndianType::BIG));
    globals
        .as_ref()
        .context("Missing global scene Palettes/_Dynamic.bin")?;
    let mut guardian_templates = BTreeMap::new();
    // The brush itself lives in Player/MagicBrush.bin; its streams are in
    // _Dynamic.bin. Preserve palette scope when resolving inherited data.
    {
        let palettes: Vec<_> = files
            .iter()
            .filter(|f| key(&f.path).starts_with("palettes/") && key(&f.path).ends_with(".bin"))
            .map(|f| {
                (
                    f.path.clone(),
                    SceneFile::from_binary(&f.data, EndianType::BIG),
                )
            })
            .collect();
        for name in ["paintguardian", "thinnerguardian"] {
            guardian_templates.insert(
                name.to_owned(),
                super::brush_assets::Assets::effect(name, &palettes, |path| {
                    Ok(lookup(&files, path)
                        .with_context(|| format!("Missing guardian asset {path}"))?
                        .data
                        .clone())
                })
                .with_context(|| format!("Guardian prefab {name}"))?,
            );
        }
        report.brush_assets = Some(super::brush_assets::Assets::load(&palettes, |path| {
            Ok(lookup(&files, path)
                .with_context(|| format!("Missing brush asset {path}"))?
                .data
                .clone())
        })?);
        let effect = &report
            .spin_settings
            .as_ref()
            .context("Missing spin settings")?
            .effect_template;
        if !effect.is_empty() {
            report.spin_assets = Some(super::brush_assets::Assets::effect(
                effect,
                &palettes,
                |path| {
                    Ok(lookup(&files, path)
                        .with_context(|| format!("Missing spin effect asset {path}"))?
                        .data
                        .clone())
                },
            )?);
            for (path, bytes) in &report.spin_assets.as_ref().unwrap().models {
                let nif = epic_mickey_lib_rs::nif_file::NifFile::load_bytes(bytes)
                    .map_err(anyhow::Error::msg)?;
                let mut generators = Vec::new();
                for block in &nif.blocks {
                    if let epic_mickey_lib_rs::nif_file::NiObject::Unknown(raw) = block {
                        if raw.type_name == "RibbonQuadGenerator" {
                            generators.push(super::ribbon::Generator::load(
                                &raw.raw_data,
                                nif.source_is_little_endian,
                            )?);
                        }
                    }
                }
                if !generators.is_empty() {
                    report.spin_ribbons.insert(path.clone(), generators);
                }
            }
        }
    }
    if let Some(global) = &globals {
        for name in &report.brush_settings.as_ref().unwrap().projectile_templates {
            let entity = global
                .objects
                .iter()
                .find(|e| e.name.eq_ignore_ascii_case(name))
                .with_context(|| format!("Missing global brush projectile template {name}"))?;
            let components = scene::get_effective_components(entity, &global.objects);
            let body = components
                .iter()
                .find(|c| c.class_name == "PhysicsRigidBodyDynamicComponent")
                .with_context(|| format!("Brush template {name} has no dynamic body component"))?;
            let hull = body
                .properties
                .iter()
                .find(|p| p.name == "RB Hull File Path")
                .and_then(|p| p.value.as_str())
                .context("Missing brush hull path")?;
            if !report.brush_shapes.contains_key(hull) {
                let source = lookup(&files, hull)
                    .with_context(|| format!("Missing brush collision asset {hull}"))?;
                let file = HkxFile::load_bytes(&source.data).map_err(anyhow::Error::msg)?;
                report.brush_shapes.insert(
                    hull.to_owned(),
                    super::projectile_shape::load(&file)
                        .with_context(|| format!("Brush collision asset {hull}"))?,
                );
            }
            report.brush_templates.insert(
                name.clone(),
                serde_json::json!({
                    "entity_id":entity.link_id.to_u32(),
                    "components":components.iter().map(|c|serde_json::json!({
                        "class":c.class_name,"properties":c.properties.iter().map(|p|
                            serde_json::json!({"name":p.name,"value":p.value})).collect::<Vec<_>>()
                    })).collect::<Vec<_>>()
                }),
            );
        }
    }
    for path in &scene_paths {
        let stem = Path::new(path)
            .file_stem()
            .context("Scene name")?
            .to_string_lossy();
        pack(&root, &format!("{stem}.pak"), &mut files, &mut report)?;
    }
    pack(&root, "r3mt.pak", &mut files, &mut report)?;
    // Interior-only launch descriptors normally run inside their parent world.
    // For standalone launch recover an unambiguous authored shared-scene context.
    // Never manufacture a player or choose an arbitrary world/spawn.
    let contains_player=scene_paths.iter().any(|path| {
        lookup(&files,path).and_then(|f|super::scene_loading::parse_scene(path,&f.data).ok())
            .is_some_and(|scene|scene.objects.iter().any(|entity|
                scene.scene.iter().any(|id|*id==entity.link_id) &&
                scene::get_effective_components(entity,&scene.objects).iter().any(|c|c.class_name=="MickeyPlayerControllerComponent")))
    });
    if !contains_player {
        let mut candidates=BTreeMap::new();
        for descriptor in files.iter().filter(|f|key(&f.path).ends_with(".level")) {
            let Ok(parent)=descriptor_bytes(&descriptor.data) else {continue};
            let Some(shared)=parent.get("shared") else {continue};
            let Some((shared_path,_))=scene_reference(shared)? else {continue};
            if scene_paths.iter().any(|p|key(p)==key(&shared_path)) {continue}
            let Ok((parent_scenes,_,_,_))=startup_scene_paths(&root,&files,&parent) else {continue};
            if parent_scenes.iter().any(|p|key(p)!=key(&shared_path) && scene_paths.iter().any(|q|key(p)==key(q))) {
                if let Some(folder)=parent.get("folder") {candidates.insert((key(&shared_path),key(folder)),(shared_path,folder.clone()));}
            }
        }
        if candidates.len()==1 {
            let (shared,parent_folder)=candidates.into_values().next().unwrap();
            let stem=Path::new(&shared).file_stem().context("Shared scene name")?.to_string_lossy();
            pack(&root,&format!("{stem}.pak"),&mut files,&mut report)?;
            let source=lookup(&files,&shared).context("Standalone shared scene absent")?;
            let scene=super::scene_loading::parse_scene(&shared,&source.data)?;
            if scene.objects.iter().any(|entity|scene.scene.iter().any(|id|*id==entity.link_id) &&
                scene::get_effective_components(entity,&scene.objects).iter().any(|c|c.class_name=="MickeyPlayerControllerComponent")) {
                report.warnings.push(format!("Standalone interior launch restored authored shared scene {shared}"));
                report.geometry.push(serde_json::json!({"kind":"standalone_shared_context","scene":shared,"parent_folder":parent_folder}));
                scene_resources.shared=Some(super::scene_resources::scene_name(&shared));
                scene_paths.insert(0,shared);
            }
        }
    }
    trace_load_memory("archive indexing and member import complete");
    let sources = material_preview::native_sources(ctx, &files);
    let mut batch = scene_import::Batch::default();
    for path in scene_paths {
        let file = lookup(&files, &path).with_context(|| format!("Missing scene {path}"))?;
        let source = super::scene_loading::parse_scene(&file.path, &file.data)?;
        batch.append_source(
            path.clone(),
            source,
            &files,
            globals.as_ref(),
            &sources,
            &mut report,
            diagnostic,
        )?;
        scene_resources.published(&path);
    }
    trace_load_memory("scene descriptors, meshes, skins, and actor records imported");
    let mut scene_collision_cache = scene_import::StaticCollisionCache::default();
    scene_collision_cache.capture(&batch);
    let mut scene_sequence_cache = scene_import::SequenceCache::default();
    scene_sequence_cache.capture(&batch);
    let scene_import::Batch {
        diagnostic_draws,
        geometry_cache,
        skins,
        jigsaws,
        mut textures,
        mut draws,
        model_mesh_inverse,
        model_local_draws,
        actor_draw_sources,
        actor_sequences,
        entities,
        player_draws,
        player_meshes,
        triangles,
        triangle_radii,
        convex_parts,
        triangle_owners,
        lights,
        model_bounds,
        found_player,
        player_entity,
        player_behavior,
        ..
    } = batch;
    ensure!(
        found_player,
        "No placed Mickey player; cannot determine the authored spawn"
    );
    ensure!(!draws.is_empty(), "No renderable geometry");
    let player_entity = player_entity.context("Missing player entity identity")?;
    let character = entities[player_entity]
        .components
        .iter()
        .find(|c| c.class_name == "PhysicsRigidBodyCharacterComponent")
        .context("Mickey has no resolved character component")?;
    let radius = character
        .properties
        .iter()
        .find(|p| p.name == "Entity Radius")
        .and_then(|p| p.value.as_i64())
        .context("Mickey has no native Entity Radius profile")?;
    report.motion.ground_probe_start_offset =
        super::motion_ground::radius(radius, &runtime_settings)?;
    report.motion.stairs = super::stairs::Settings::load(&runtime_settings)?;
    let dynamic_bodies = dynamic_collision_assets(&entities, &files);
    let dynamic_body_sources = super::dynamic_body::SourceSummary::new(&dynamic_bodies);
    if !dynamic_bodies.is_empty() {
        let ready = dynamic_bodies
            .iter()
            .filter(|body| body.geometry.is_ok())
            .count();
        report.geometry.push(serde_json::json!({"kind":"dynamic_collision_import",
            "decoded_components":ready,"placed_components":dynamic_bodies.len(),
            "admission":"per-system native construction and shared-world eligibility follow import"}));
        for body in &dynamic_bodies {
            if let Err(error) = &body.selected_system {
                let entity = &entities[body.entity];
                report.unsupported.push(format!(
                    "Dynamic system selection {}:{} component {}: {error}",
                    entity.scene, entity.name, body.component
                ));
            }
            if body.geometry.is_ok() {
                if let Err(error) = &body.systems {
                    let entity = &entities[body.entity];
                    report.unsupported.push(format!(
                        "Dynamic physics-system {}:{} component {}: {error}",
                        entity.scene, entity.name, body.component
                    ));
                }
            }
            if let Err(error) = &body.initial_velocity {
                let entity = &entities[body.entity];
                report.unsupported.push(format!(
                    "Dynamic initial velocity {}:{} component {}: {error}",
                    entity.scene, entity.name, body.component
                ));
            }
            if let Err(error) = &body.geometry {
                let entity = &entities[body.entity];
                report.unsupported.push(format!(
                    "Dynamic collision {}:{} component {} asset {}: {error}",
                    entity.scene, entity.name, body.component, body.asset
                ));
            }
        }
    }
    let mut collision = scene_import::build_static_collision(
        triangles,
        triangle_radii,
        triangle_owners,
        convex_parts,
        &report,
    )?;
    trace_load_memory("Box3D static world admitted");

    if collision.planar_convex_count != 0 {
        report.unsupported.push(format!("{} flat convex cores represented by two-sided rounded triangle surfaces; source vertices and radii retained, Havok solver parity unvalidated.",collision.planar_convex_count));
    }
    let sketch_shaders = super::sketch_render::Shaders::load(&files, &runtime_settings)?;
    let mut effect_templates = if jigsaws.is_empty() {
        HashMap::new()
    } else {
        super::jigsaw_effects::templates(&files)?
    };
    let mut sketch_draws = HashSet::new();
    let mut hidden_jigsaw_draws = HashSet::new();
    let mut jigsaw_instances = Vec::new();
    let mut jigsaw_owners = HashMap::new();
    let mut next_owner = report.collision_parts.len();
    initialize_jigsaw_entities(
        &entities,
        0..entities.len(),
        &jigsaws,
        &mut collision,
        &mut draws,
        &mut report,
        &sketch_shaders,
        &files,
        &mut textures,
        &sources,
        &effect_templates,
        &mut sketch_draws,
        &mut hidden_jigsaw_draws,
        &mut jigsaw_instances,
        &mut jigsaw_owners,
        &mut next_owner,
    )?;
    collision.set_player_hull(
        report
            .player_collision
            .as_ref()
            .context("Missing authored Mickey collision hull")?,
    )?;
    // Projector scenes can use only convex collision. An empty triangle soup
    // is valid; convex shapes and dynamic bodies are installed separately.
    report.collision_triangles = collision.triangle_count();
    report.meshes = draws.len();
    report.textures = textures.values().filter(|v| v.is_some()).count();
    report.warnings.sort();
    report.warnings.dedup();
    report.unsupported.extend(["Mickey has locomotion, jump, landing, spin and dynamic-ear playback; full original graph scheduling, facial animation and controller parity remain incomplete. Other skeletal character behavior controllers remain incomplete.".into(),"Original Lua chunks and supported actor APIs execute natively; full script APIs, BSQ events, Oswald AI, conversation, audio and progression remain incomplete. Paint/thinner has an original-asset dissolve preview; native hit rules, full material composition and gameplay events remain incomplete.".into(),"Character collision uses the authored convex hull with a provisional Box3D static mover; Havok contact response, stairs, moving surfaces and full controller behavior remain unverified.".into(),"Part streaming APIs (StreamPart/StreamZones/StreamInterior), direct StreamInLevel requests, coroutine WaitForLevelLoad, and authored part soundbank registration are connected. DeactivateScene removes visible/static-collision participation and suspends supported owner-scoped scripts, events, timers, animations, effects, spawners and moving audio; native activation events/order, complete component teardown, asset-memory reclamation, full AI/physics suspension, unloading and end-to-end projector transitions remain incomplete.".into()]);
    if diagnostic.is_some() {
        ensure!(
            diagnostic_draws > 0,
            "Diagnostic morph matched no rendered mesh"
        );
        report.warnings.push(format!("Diagnostic fixed morph pose applied to {diagnostic_draws} draws; sequence activation and lighting-bound lifecycle unverified"));
    }
    let inspection_vertex_buffers = super::inspection_vertex_cache::Cache::default();
    let player_animations = super::animation_resources::Resources::load(
        report
            .player_behavior
            .as_ref()
            .context("Missing player animation contract")?,
        |path| {
            let k = key(path);
            // This import catalog is still used by authored actors,
            // attachments and palette initialization below. Transferring
            // its buffers here turned valid shared HKX assets into empty
            // files for those consumers (e.g. Mean Street's second actor).
            // The catalog is transient and is released after initialization;
            // Resources owns the lazy animation payloads for gameplay.
            files
                .iter()
                .find(|f| key(&f.path) == k || key(&f.path) == format!("{k}_wii"))
                .map(|f| f.data.clone())
        },
    )?;
    trace_load_memory("player animation resources loaded");
    let mut brush_stimuli = BTreeMap::new();
    let mut brush_decals = BTreeMap::new();
    for (name, template) in &report.brush_templates {
        let components = template["components"]
            .as_array()
            .context("Missing projectile components")?;
        if let Some(decal) = components.iter().find(|c| c["class"] == "DecalComponent") {
            brush_decals.insert(
                name.clone(),
                super::decal::Component::load(
                    decal["properties"]
                        .as_array()
                        .context("Missing decal properties")?,
                    |path| {
                        Ok(lookup(&files, path)
                            .with_context(|| format!("Missing decal {path}"))?
                            .data
                            .clone())
                    },
                )?,
            );
            brush_decals.get_mut(name).unwrap().prepare_materials()?;
            for (path, material) in &mut brush_decals.get_mut(name).unwrap().templates {
                material.prepare(&sources, &files, &mut textures);
                if let Some(Err(error)) = &material.rendering {
                    report
                        .warnings
                        .push(format!("Decal material {path}: {error}"));
                }
            }
        }
        let component = components
            .iter()
            .find(|c| c["class"] == "ProjectileComponent")
            .context("Missing ProjectileComponent")?;
        let properties = component["properties"]
            .as_array()
            .context("Missing projectile properties")?;
        brush_stimuli.insert(
            name.clone(),
            super::projectile_stimulus::Stimuli::load(properties)
                .with_context(|| format!("Projectile {name}"))?,
        );
    }
    let brush = super::brush_render::Brush::load(
        report
            .brush_assets
            .as_ref()
            .context("Missing brush resources")?,
        &files,
        &root,
        &player_animations.rig.bones,
        ctx,
    )?;
    // These source NIFs have been decoded into the live brush draws and
    // particle controllers. Keep the authored template graph for diagnostics,
    // but do not retain another copy of the source byte buffers for the whole
    // play session.
    if let Some(assets) = &mut report.brush_assets {
        assets.models.clear();
    }
    let mut sketch_templates = super::sketch_assets::Library::default();
    let palettes: Vec<_> = files
        .iter()
        .filter(|f| key(&f.path).starts_with("palettes/") && key(&f.path).ends_with(".bin"))
        .map(|f| {
            (
                f.path.clone(),
                SceneFile::from_binary(&f.data, EndianType::BIG),
            )
        })
        .collect();
    for (setting, name) in &runtime_settings {
        if !setting
            .strip_prefix("MickeyAbilities_Sketches_Sketch_")
            .is_some_and(|suffix| suffix.parse::<u32>().is_ok())
        {
            continue;
        }
        let template = format!("paint{name}");
        if let Err(error) = sketch_templates.prepare(&template, &palettes, &files) {
            report
                .unsupported
                .push(format!("Sketch template {template}: {error:#}"));
        }
    }
    // All sketch names enabled by the runtime settings have now been
    // prepared. Their animation entries own the required clip data, while
    // template instantiation uses parsed models, so the import cache of
    // duplicate source bytes can be released before play starts.
    sketch_templates.release_source_bytes();
    if !sketch_templates.templates.is_empty() {
        for (name, template) in &sketch_templates.templates {
            if let Err(error) = &template.animation {
                report
                    .unsupported
                    .push(format!("Sketch animation {name}: {error}"));
            }
        }
        match sketch_templates.prepare_rendering(&files, &sources, &mut textures) {
            Ok(()) => {
                // Each configured sketch now has parsed mesh, skin and
                // hierarchy data in Library::models. Its template graph only
                // needs declarations at runtime, so release the duplicate NIF
                // byte buffers while retaining the parsed model for spawning.
                for template in sketch_templates.templates.values_mut() {
                    template.graph.models.clear();
                }
            }
            Err(error) => report
                .unsupported
                .push(format!("Sketch rendering resources: {error:#}")),
        }
        report.unsupported.push("Original sketch roots can be created with animation, physics, landing IO, Lua and lifetime cleanup. Original action/inventory placement, child effects, crushing and complete receiver/lifecycle behavior remain incomplete.".into());
    }
    let spin_ribbon = if let Some(assets) = &report.spin_assets {
        let definition = &assets.definitions[&assets.root];
        let model = definition
            .model
            .as_ref()
            .context("Spin effect has no model")?;
        Some(
            super::brush_particles::Ribbon::load(&assets.models[model], &files)
                .context("Spin ribbon")?,
        )
    } else {
        None
    };
    let spin_particles = if let Some(assets) = &report.spin_assets {
        let model = assets.definitions[&assets.root]
            .model
            .as_ref()
            .context("Spin effect has no model")?;
        Some(
            super::brush_particles::Hose::load_attached(&assets.models[model], &files)
                .context("Spin particle layers")?,
        )
    } else {
        None
    };
    // The ribbon and hose own their parsed runtime state; the source NIF bytes
    // are no longer needed after both consumers have initialized.
    if let Some(assets) = &mut report.spin_assets {
        assets.models.clear();
    }
    let particle_asset_preview = guardian
        .map(|name| -> Result<_> {
            let loaded;
            let graph = if let Some(graph) = guardian_templates.get(name) {
                graph
            } else {
                let scene_paths = report
                    .scenes
                    .iter()
                    .map(|path| key(path))
                    .collect::<HashSet<_>>();
                let scenes = files
                    .iter()
                    .filter(|file| {
                        key(&file.path).ends_with(".bin")
                            && (key(&file.path).starts_with("palettes/")
                                || scene_paths.contains(&key(&file.path)))
                    })
                    .map(|file| {
                        Ok((
                            file.path.clone(),
                            super::scene_loading::parse_scene(&file.path, &file.data)?,
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?;
                loaded = super::brush_assets::Assets::effect(name, &scenes, |path| {
                    lookup(&files, path)
                        .map(|file| file.data.clone())
                        .with_context(|| format!("Missing effect particle model {path}"))
                })?;
                &loaded
            };
            let preview = super::particle_preview::Instance::load(name, graph, &files)?;
            report
                .warnings
                .extend(preview.particles.warnings.iter().cloned());
            Ok(preview)
        })
        .transpose()?;
    if spin_ribbon.is_some() {
        report.unsupported.push("Spin ribbon and sparkle/pop layers are connected to attack lifetime and brush pose; source RNG, byte-color interpolation and native transform/clock parity remain incomplete. Facing-particle rotation and drag use Gamebryo source semantics pending EM1 reference validation.".into());
    }
    if let Some(particles) = &spin_particles {
        report.warnings.extend(particles.warnings.iter().cloned());
    }
    report.warnings.extend(brush.warnings.iter().cloned());
    report.warnings.sort();
    report.warnings.dedup();
    report.textures = textures.values().filter(|v| v.is_some()).count();
    report.unsupported.push("Brush follows the animated weapon bone. Q/E play authored hose assets as a provisional visual preview: native event dispatch/startup restoration, brush-tip aim, particle RNG/alignment/forces, indirect shader parity, complete surface material fidelity and action-driven brush visibility remain incomplete. Brush SH lighting uses its animated model bound; raycast visibility remains incomplete.".into());
    let mut scene_actor_binding_cache = scene_import::ActorBindingCache::default();
    let actor_bindings = scene_import::prepare_actor_bindings(
        &entities,
        &actor_sequences,
        &model_bounds,
        &files,
        &mut scene_actor_binding_cache,
        &mut report,
    );
    if !actor_sequences.is_empty() {
        report.unsupported.push("Gamebryo actors execute authored startup IDs and supported KFM crossfades. Other evaluator channels, cumulative motion, sequence events and full original blend arithmetic remain incomplete.".into());
    }
    let mut aliases = Vec::new();
    let mut relative_aliases = 0usize;
    for (index, entity) in entities.iter().enumerate() {
        for component in entity
            .components
            .iter()
            .filter(|component| component.class_name == "InputOutputComponent")
        {
            for property in component
                .properties
                .iter()
                .filter(|property| property.name == "EntityGroups")
            {
                for name in property
                    .value
                    .as_array()
                    .context("EntityGroups must be a string array")?
                {
                    let name = name
                        .as_str()
                        .context("EntityGroups member must be a string")?;
                    if name.starts_with('.') {
                        relative_aliases += 1;
                        continue;
                    }
                    if !name.is_empty() {
                        aliases.push((index, name.to_string()));
                    }
                }
            }
        }
    }
    for alias in report
        .geometry
        .iter()
        .filter(|record| record["kind"] == "canonical_player_alias")
    {
        let canonical = alias["canonical"]
            .as_str()
            .context("Canonical player alias target missing")?;
        let owner = entities
            .iter()
            .position(|entity| entity.name == canonical)
            .context("Canonical player alias owner missing")?;
        aliases.push((
            owner,
            alias["alias"]
                .as_str()
                .context("Canonical player alias name missing")?
                .to_owned(),
        ));
    }
    report.geometry.push(serde_json::json!({"kind":"entity_registry","global_alias_registrations":aliases.len(),"local_prefab_aliases":relative_aliases}));
    let global_audio: Arc<[u8]> = std::fs::read(root.join("Audio/Wii/Global.bnk"))
        .context("Read global audio bank")?
        .into();
    let guardian_audio = super::guardians::Audio::load(&global_audio)?;
    let mut audio_playback = super::audio_playback::Playback::load(global_audio)?;
    audio_playback.set_media_root(root.join("Audio/Wii"));
    for line in part_text.lines() {
        let line = line.trim();
        if line.starts_with(['#', ';']) || line.starts_with("//") {
            continue;
        }
        let Some(path) = line
            .split_whitespace()
            .next()
            .filter(|path| key(path).ends_with(".bnk"))
        else {
            continue;
        };
        let path = super::resource_path::canonical(path)?;
        let path = if key(&path).starts_with("audio/") {
            path
        } else {
            format!("Audio/Wii/{path}")
        };
        let loose = root.join(&path);
        let bytes = if loose.is_file() {
            Some(
                std::fs::read(&loose)
                    .with_context(|| format!("Read authored audio bank {path}"))?,
            )
        } else {
            lookup(&files, &path).map(|file| file.data.clone())
        };
        let Some(bytes) = bytes else {
            report.warnings.push(format!("Authored audio bank {path} is absent from the extracted game data; its events will be silent."));
            continue;
        };
        audio_playback
            .register_bank(bytes.into())
            .with_context(|| format!("Register authored audio bank {path}"))?;
    }
    report.unsupported.push("Audio output preview plays the original dry-brush DSP sound with static parent gain. Wwise bus mixing, positioning, container selection, transitions and device-clock scheduling remain incomplete; other Play/container events reject explicitly; direct object stops support empty targets and immediate active stops, while active fades remain incomplete.".into());
    report.unsupported.push("NPC bicone shadow volumes and the player model projector are connected. Native receiver-query parity, player motion-height adjustment, original EFB precision and full shadow frame parity remain unvalidated.".into());
    report.unsupported.push("Silhouette render gate is decoded; runtime mesh enrollment and the original ShadowVolumeShaderSetShader pass remain unimplemented.".into());
    report.unsupported.push("Authored camera FOV and application clip/aspect arithmetic are connected; default gameplay camera generation, active-camera entity lifecycle, complete property inheritance/update ordering and input handoff ownership remain incomplete or unverified.".into());
    let guardian_costs = super::guardians::Costs::load(&runtime_settings)?;
    let guardian_parameters =
        super::guardian_parameters::Library::load(&guardian_costs, &runtime_settings)?;
    let static_collision_owner_parts = report
        .collision_parts
        .iter()
        .enumerate()
        .map(|(part, source)| (source.owner, part))
        .collect();
    let camera_profiles = super::camera_catalog::Catalog::load(&runtime_settings)
        .context("Native camera profile initialization")?;
    let frame_clock = scores.frame_clock.unwrap_or_default();
    let camera_pipeline = super::camera_pipeline::Runtime::new(
        &camera_profiles,
        &runtime_settings,
        frame_clock.clock.unscaled.ticks,
    )?;
    let gameplay_camera_targets = super::camera_generator_target::Targets::load(&runtime_settings)
        .context("Native camera target/path initialization")?;
    let camera_definition = globals
        .as_ref()
        .context("Missing global camera palette")?
        .objects
        .iter()
        .find(|entity| entity.name.eq_ignore_ascii_case("defaultcamera"))
        .context("Missing defaultcamera prefab")?;
    let camera_components =
        scene::get_effective_components(camera_definition, &globals.as_ref().unwrap().objects);
    let mut camera_lens = super::camera_lens::Application::load(
        camera_components
            .iter()
            .find(|component| component.class_name == "JPSCameraComponent")
            .context("Default camera has no lens component")?,
        &runtime_settings,
    )?;
    // The host viewport exists before level scripts run. Publish its aspect
    // for initial callbacks as well as for the per-frame renderer updates.
    if let Some(viewport) = ctx.input(|input| input.viewport().inner_rect) {
        if viewport.width() > 0. && viewport.height() > 0. {
            camera_lens.set_renderer_aspect(viewport.width() / viewport.height())?;
        }
    }
    report.geometry.push(serde_json::json!({"kind":"application_camera_lens","prefab":camera_definition.name,
        "source":"Palettes/_Dynamic.bin","initial":camera_lens,"native_renderer_lifecycle_validated":false}));
    let named_fx = super::effect_runtime::Library::load(&root, &report, &files, &entities)?;
    let scene_particles = super::scene_particles::Runtime::new(&root, &report.packs)?;
    let story_sequences = super::story_sequences::State::load(&entities, |path| {
        Ok(lookup(&files, path).map(|file| (file.path.clone(), file.data.clone())))
    })?;
    if !story_sequences.unresolved.is_empty() {
        report.unsupported.push(format!("{} declared story sequence references could not resolve; see story_sequences.unresolved_declarations. No substitute asset or sequence registration was created.",story_sequences.unresolved.len()));
    }
    let ai_navigation = super::ai_navigation::Runtime::load(&entities, &files, &root);
    let ai_navigation_databases = ai_navigation.database_paths().count();
    let ai_navigation_sectors = ai_navigation.sector_count();
    report
        .warnings
        .extend(ai_navigation.warnings.iter().cloned());
    report.geometry.push(serde_json::json!({
        "kind":"runtime_ai_navigation",
        "referenced_databases":ai_navigation_databases,
        "loaded_sectors":ai_navigation_sectors,
        "native_runtime_floors":ai_navigation.native_floor_count(),
        "route_cache":"per database/sector/node pair",
        "cross_sector_routes":false,
        "native_kynapse_agent_policy":false
    }));
    let mut world = World {
        death_rewarded: BTreeSet::new(),
        cleanup_queue: Default::default(),
        cleanup_preparing: Default::default(),
        cleanup_completed: 0,
        dynamic_rewards: Default::default(),
        level_transition: Default::default(),
        pending_player_transfer: None,
        robot_arm_idle_requests: BTreeMap::new(),
        robot_arm_controls: BTreeMap::new(),
        demo_story: Default::default(),
        story_sequences,
        movies: Default::default(),
        data_root: root.clone(),
        asset_context: ctx.clone(),
        scene_requests: Default::default(),
        scene_requests_prepared_for_models: false,
        scene_resources,
        level_resource,
        texture_cache: HashMap::new(),
        startup_texture_cache_pruned_bytes: 0,
        startup_entity_payload_compacted_bytes: 0,
        scene_geometry_cache: geometry_cache,
        scene_collision_cache,
        scene_sequence_cache,
        scene_actor_binding_cache,
        static_collision_owner_parts,
        particle_asset_preview,
        named_fx,
        scene_particles,
        shadow_volumes: Default::default(),
        guardian_entities: Default::default(),
        guardian_templates,
        guardian_costs,
        guardian_parameters,
        guardians: Default::default(),
        guardian_audio,
        audio_playback,
        music_state: Default::default(),
        multi_position_audio: BTreeMap::new(),
        model_shadow: Default::default(),
        screen_fade: Default::default(),
        map_info: Default::default(),
        camera_profiles,
        camera_pipeline,
        side_scroller: Default::default(),
        contact_stimuli: Default::default(),
        camera_lens,
        camera_target: Default::default(),
        camera_feedback: Default::default(),
        gameplay_camera_targets,
        mickey_tracker: None,
        entity_event_registry: Default::default(),
        render_options,
        ledge_controls: BTreeMap::new(),
        wall_pushing_disabled: HashSet::new(),
        nif_camera: None,
        nif_camera_transition: None,
        nif_camera_completed_frame: None,
        spawners: BTreeMap::new(),
        spawner_sources: HashSet::new(),
        component_control: Default::default(),
        globals: global_state,
        quests: quest_state,
        game_options: super::save_options::State::from_settings(&runtime_settings)?,
        script_feedback: Default::default(),
        instance_changes: Default::default(),
        animation_variable_changes: Default::default(),
        playstyle_points: scores.playstyle_points,
        playstyle_tracking: BTreeSet::new(),
        playthrough: scores.playthrough,
        playtime: scores.playtime,
        frame_clock,
        checkpoint: scores.checkpoint,
        extras,
        collectibles,
        ui_notifications: Vec::new(),
        player_spawn: Default::default(),
        cutscene_input: Default::default(),
        script_player: physics::Player::new(report.spawn, report.motion.clone()),
        script_player_available: true,
        player_knockback: None,
        havok_actors: BTreeMap::new(),
        sketch_templates,
        component_events: super::component_events::Runtime::default(),
        ai_brains: BTreeMap::new(),
        patrol_predecessors: BTreeMap::new(),
        ai_navigation,
        paint_counters: BTreeMap::new(),
        counter_materials: BTreeMap::new(),
        ai_registry: super::ai_registry::Registry::default(),
        entity_event_queue: Default::default(),
        deferred_event_runtime:Default::default(),
        ai_manager_enabled: None,
        cutscene_actions: Default::default(),
        animation_events: super::animation_events::Catalog::load(&root)
            .map_err(|error| format!("{error:#}")),
        player_script_animation_events: Vec::new(),
        controller_feedback: Default::default(),
        authored_havok: super::authored_havok::Runtime::with_wii_source(&root),
        player_physics: None,
        character_physics: BTreeMap::new(),
        npc_relocations: BTreeMap::new(),
        model_paint: BTreeMap::new(),
        model_paint_owners: BTreeMap::new(),
        pickup_bob_globals: report
            .pickup_bob_globals
            .context("Missing pickup global settings")?,
        player_abilities: super::ability_registry::Registry::base_ownership(),
        player_stay_combat_idle: false,
        player_ability_registry: super::ability_registry::Registry::load(&runtime_settings),
        ticket_hud_gain: None,
        ability_hud_requests: Vec::new(),
        diagnostic_pickup_commands: false,
        authored_health: BTreeMap::new(),
        runtime_health: BTreeMap::new(),
        removed_entities: HashSet::new(),
        removed_draws: HashSet::new(),
        hidden_entities: HashSet::new(),
        stream_hidden_entities: BTreeMap::new(),
        cold_scene_assets: HashMap::new(),
        previous_hidden_entities: HashSet::new(),
        hidden_entity_draws: HashSet::new(),
        authored_inventory: BTreeMap::new(),
        pickup_receivers: BTreeMap::new(),
        pickup_settings: BTreeMap::new(),
        pickup_update_states: BTreeMap::new(),
        pickup_collection_blocked: BTreeSet::new(),
        pickup_lifecycles: BTreeMap::new(),
        pickup_physics_leases: BTreeMap::new(),
        pickup_reference_monitor: std::sync::Mutex::new(()),
        size_phantoms: BTreeMap::new(),
        rigid_trigger_hulls: BTreeMap::new(),
        resource_phantom_shapes: BTreeMap::new(),
        resource_phantom_motion: BTreeMap::new(),
        lod_states: BTreeMap::new(),
        resource_phantom_bounds: BTreeMap::new(),
        size_phantom_filters: BTreeMap::new(),
        physics_objects: Default::default(),
        size_phantom_handles: BTreeMap::new(),
        resource_phantom_handles: BTreeMap::new(),
        phantom_systems: BTreeMap::new(),
        phantom_resources: Default::default(),
        actor_playback: None,
        actor_draw_sources,
        outputs: super::actor_io::Outputs::default(),
        actors: BTreeMap::new(),
        idle_actors: HashSet::new(),
        animation_time: 0.,
        scripts: super::actor_scripts::Scripts::with_aliases(
            &entities
                .iter()
                .enumerate()
                .map(|(i, e)| (i, e.name.clone()))
                .collect::<Vec<_>>(),
            &aliases,
        )?,
        actor_sequences,
        actor_bindings,
        dynamic_bodies,
        dynamic_body_sources,
        dynamic_constructions: BTreeMap::new(),
        dynamic_queries: None,
        dynamic_query_owners: Vec::new(),
        dynamic_runtime_sources: BTreeMap::new(),
        dynamic_ground_contacts: Default::default(),
        pressure_plates: Default::default(),
        runtime_lifetimes: BTreeMap::new(),
        rigid_layer_updates: Default::default(),
        dynamic_runtime_filter: super::physics_filter::GameFilter {
            matrix: super::physics_filter::Matrix::retail(),
            ..Default::default()
        },
        dynamic_runtime_time: 0.,
        game_random,
        rotators: Default::default(),
        transform_hierarchy: Default::default(),
        live_child_slots: Default::default(),
        live_transform_children: Default::default(),
        dynamic_parent_bindings: Default::default(),
        script_transformations: 0,
        script_transform_lod_resets: 0,
        parent_pose_forces: 0,
        parent_pose_error: 0.,
        scene_motion_prepared_at: None,
        runtime_settings,
        countdown: super::countdown::State::default(),
        input_presentation: Default::default(),
        logic_timers: Default::default(),
        material_animation: super::material_animation::Runtime::load(&files),
        model_animation: BTreeMap::new(),
        render_objects: BTreeMap::new(),
        model_mesh_inverse,
        model_local_draws,
        spherical_materials: Default::default(),
        spherical_bounds: Default::default(),
        spherical_properties: Default::default(),
        spherical_bindings: Vec::new(),
        child_lighting_properties: BTreeMap::new(),
        jigsaw_effect_queue: Default::default(),
        jigsaw_endpoint_pending: Default::default(),
        jigsaw_effect_values: HashMap::new(),
        hidden_jigsaw_draws,
        sketch_draws,
        jigsaw_instances,
        jigsaw_owners,
        jigsaws,
        debug_font: super::debug_font::Font::load(&root, ctx).map_err(|e| format!("{e:#}")),
        projectile_preview: Default::default(),
        brush_cursor: Default::default(),
        brush_decals,
        brush_stimuli,
        brush,
        spin_ribbon,
        spin_particles,
        player_attachment_pose: player_animations.rig.bones.clone(),
        player_animations,
        player_graph: {
            let graph = player_behavior
                .as_ref()
                .context("Missing behavior for state initialization")?;
            super::generator_plan::prepare(graph, super::behavior_state::State::new(graph)?, 0)?
        },
        player_behavior: player_behavior.context("Missing Mickey behavior graph")?,
        brush_runtime: Default::default(),
        ammo: super::ammo::State::new(
            report.authored_ammo.clone(),
            *report
                .ammo_settings
                .as_ref()
                .context("Missing ammo settings")?,
            report.ammo_policy.context("Missing ammo policy")?,
        ),
        entities,
        player_entity,
        player_meshes,
        skins,
        inspection_vertex_buffers,
        draws,
        player_draws,
        collision,
        lights,
        enhanced_light_registry: Default::default(),
        model_bounds,
        report,
    };
    world.initialize_countdown()?;
    if let (Some(archive), Some(snapshot)) = (&restore, &restore_snapshot) {
        world.restore_checkpoint_owners(archive, &snapshot.bytes)?;
    } else if scores.resume == Some(super::score_state::CampaignResume::NextPlaythrough) {
        let snapshot = world.checkpoint.latest.as_ref()
            .context("Next playthrough requires a completed slot")?.clone();
        world.seed_completed_playthrough(&snapshot.bytes)?;
    }
    if let Some(session) = scores.session {
        session.install(&mut world)?;
    }
    world.install_mickey_tracker(&files)?;
    world.scripts.set_player(world.player_entity)?;
    if let Ok(catalog) = &world.animation_events {
        world.scripts.bind_native_animation_constants(catalog)?;
    }
    if let Err(error) = &world.animation_events {
        world
            .report
            .unsupported
            .push(format!("Native animation events: {error}"));
    }
    world.initialize_component_range(0..world.entities.len(), &files, &root)?;
    if let Some(health) = world.runtime_settings.get("PlayerHealth").filter(|_| restore.is_none())
        .map(|value| super::native_text_number::atoi(value.as_bytes(), &mut 0))
        .filter(|&value| value > 0)
    {
        let player = world.player_entity;
        if let Some(authored) = world.authored_health.get_mut(&player) {
            authored.health = health;
            authored.maximum = health;
            world.runtime_health.insert(player, super::health::Runtime::new(authored));
            for property in world.entities[player].components.iter_mut()
                .filter(|component| component.class_name == "HealthComponent")
                .flat_map(|component| &mut component.properties)
                .filter(|property| matches!(property.name.as_str(), "Health" | "MaxHealth"))
            {
                property.value = serde_json::json!(health);
            }
        }
    }
    trace_load_memory("component constructors and Lua scripts initialized");
    // Map-load callbacks must see the same gameplay APIs as subsequent ticks.
    // Inspection and strict compatibility fixtures retain their original path.
    world.initialize_level_streams()?;
    if demo {
        world.enable_demo_story()?;
        // Apply before the initial frame and map-load callbacks can select AI.
        world.demo_story.npc_fallback.enabled = !strict_npc_ai;
    }
    if let Some(archive) = &restore {
        world.schedule_checkpoint_callback(archive)?;
    }
    // Static toon bodies and draws are already admitted. Bind their paint state
    // before authored spawn and map-load callbacks can send paint commands.
    super::model_paint::initialize(
        &mut world,
        &files,
        &sketch_shaders,
        &sources,
        &mut textures,
        &mut effect_templates,
        true,
    )?;
    world.advance_models(0.)?;
    for entity in 0..world.entities.len() {
        // Placed objects are constructed entities too. Restricting this to
        // child templates discarded authored initialization (door poses,
        // material colors, motion locks) on ordinary scene objects.
        // REL1ee060..1ee1a4 queues entity event3d after component callbacks.
        if !world.spawner_sources.contains(&entity) {
            world.emit_entity_output(entity, "OnSpawned", entity);
        }
        world.emit_entity_output(entity, "OnMapLoad", entity);
        world.emit_entity_output(entity, "OnMapLoadOrStream", entity);
    }
    world.advance_models(0.)?;
    world.construct_dynamic_sources(0..world.dynamic_bodies.len())?;
    world.report.textures = textures.values().filter(|image| image.is_some()).count();
    world.admit_dynamic_sources(0..world.dynamic_bodies.len(), &mut next_owner)?;
    world.report.unsupported.push("Lua math, dynamic initialization and impact decals share the original C rand generator; full boot-time draw ordering, AI and other C rand consumers remain unverified.".into());
    super::model_paint::initialize(
        &mut world,
        &files,
        &sketch_shaders,
        &sources,
        &mut textures,
        &mut effect_templates,
        false,
    )?;
    world.bind_spherical_properties(0..world.entities.len(), &sources, &files, &mut textures)?;
    let released_archive_bytes = files
        .iter()
        .map(|file| file.data.capacity())
        .sum::<usize>();
    // Bootstrap reads complete pack members into `files`, but runtime systems
    // keep their own decoded/source state and all file-backed systems have now
    // consumed this archive snapshot. Drop it before the final collision
    // contact construction so archive payloads do not overlap that work's
    // temporary allocations.
    drop(files);
    trace_load_memory(&format!(
        "{} MiB of bootstrap archive payload released",
        released_archive_bytes / (1024 * 1024)
    ));
    let contacts = world
        .report
        .collision_parts
        .iter()
        .filter(|part| part.triangle_count > 0)
        .map(|part| -> Result<_> {
            let owner = part.owner;
            let entity = world
                .collision_entity(owner)
                .context("Native static contact owner missing")?;
            Ok(super::character_contacts::Source {
                owner,
                resource: part.native_resource.clone(),
                part: part.native_part,
                welding: part.native_welding.clone(),
                body: part.body_offset,
                triangle_start: part.triangle_start,
                initial: super::native_hk_transform::initial_body(
                    &entity.physics_transform(),
                    &part.body_transform,
                )?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    world.collision.install_native_static_character_contacts(
        world
            .report
            .player_native_support
            .as_ref()
            .context("Native Mickey support missing")?,
        world
            .report
            .player_collision
            .as_ref()
            .context("Mickey hull missing")?
            .radius,
        contacts,
    )?;
    if !world.dynamic_runtime_sources.is_empty() {
        world.report.unsupported.push("Admitted bodies use Box3D compatibility simulation, source mass/materials/gravity and native activation policy. Unconstrained multi-body keyframed models bind to their named animation nodes. Havok solver/child filters/quality agents, character pushing, dynamic paint and full runtime callbacks remain unverified or incomplete.".into());
    }
    world.report.spawn = world.script_player.position;
    world.reset_mickey_tracker()?;
    world.report.textures = textures.values().filter(|image| image.is_some()).count();
    world.texture_cache = textures;
    world.startup_texture_cache_pruned_bytes = world.prune_unused_texture_cache();
    world.texture_cache.shrink_to_fit();
    world.startup_entity_payload_compacted_bytes = world.compact_startup_entity_payloads();
    Ok(world)
}

fn compact_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(value) => value.shrink_to_fit(),
        serde_json::Value::Array(values) => {
            for value in values.iter_mut() {
                compact_json_value(value);
            }
            values.shrink_to_fit();
        }
        serde_json::Value::Object(values) => {
            for value in values.values_mut() {
                compact_json_value(value);
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_) => {}
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn startup_property_compaction_preserves_values_and_drops_spare_capacity() {
        use super::compact_json_value;
        let mut array = Vec::with_capacity(128);
        let mut text = String::with_capacity(96);
        text.push_str("authored value");
        array.push(serde_json::Value::String(text));
        let mut object = serde_json::Map::new();
        object.insert("authored property".into(), serde_json::Value::Array(array));
        let mut value = serde_json::Value::Object(object);
        let before = serde_json::to_vec(&value).unwrap();
        let before_object = value.as_object().unwrap();
        let before_array = before_object.values().next().unwrap().as_array().unwrap();
        let before_array_capacity = before_array.capacity();
        let before_text_capacity = match &before_array[0] {
            serde_json::Value::String(text) => text.capacity(),
            _ => unreachable!(),
        };
        compact_json_value(&mut value);
        let after = serde_json::to_vec(&value).unwrap();
        let after_object = value.as_object().unwrap();
        let after_array = after_object.values().next().unwrap().as_array().unwrap();
        let after_array_capacity = after_array.capacity();
        let after_text_capacity = match &after_array[0] {
            serde_json::Value::String(text) => text.capacity(),
            _ => unreachable!(),
        };
        assert_eq!(before, after);
        assert!(after_array_capacity < before_array_capacity);
        assert!(after_text_capacity < before_text_capacity);
    }

    #[test]
    fn retired_entity_releases_component_payload_but_keeps_stable_identity() {
        use super::RuntimeEntity;
        use epic_mickey_lib_rs::scene_file::{Component, ID, Property};

        let property = Property::new(
            String::new(),
            "Large Authored Value".into(),
            false,
            false,
            false,
            serde_json::json!((0..4096).collect::<Vec<u32>>()),
        );
        let component = Component::new(
            "TestComponent".into(),
            "Test".into(),
            ID::new(0),
            ID::from_u32(1),
            ID::new(0),
            vec![property],
        );
        let mut entity = RuntimeEntity {
            authored_scale: 1.0,
            authored_rotation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            runtime_rotation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            model_asset: Some("models/test.nif".into()),
            mesh_draws: [(7, vec![0; 512])].into_iter().collect(),
            jigsaw_asset: None,
            jigsaw_states: Vec::new(),
            scene: "scene/test.bin".into(),
            id: 12,
            name: "Retired test entity".into(),
            transform: [0.0; 16],
            authored_transform: [0.0; 16],
            components: vec![component],
            draws: 3..4,
            collision_parts: 0..0,
        };
        assert!(entity.components.capacity() > 0);
        assert!(entity.mesh_draws.capacity() > 0);

        entity.release_retired_component_data();

        assert!(entity.components.is_empty());
        assert_eq!(entity.components.capacity(), 0);
        assert!(entity.mesh_draws.is_empty());
        assert_eq!(entity.mesh_draws.capacity(), 0);
        assert_eq!(entity.name, "Retired test entity");
        assert_eq!(entity.scene, "scene/test.bin");
        assert_eq!(entity.id, 12);
        assert_eq!(entity.draws, 3..4);
    }

    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_npc_puddle_eligibility_uses_counter_thresholds_and_lua_reads_live_value(
    ) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let (&owner, brain) = world
            .ai_brains
            .iter()
            .find(|(owner, brain)| {
                !brain.npc_puddle.is_empty() && world.paint_counters.contains_key(owner)
            })
            .context("Expected an original NPC puddle/counter pair")?;
        let behavior = brain.npc_puddle[0];
        let event = super::super::entity_events::CounterStatus {
            source: Some(owner),
            header: None,
            previous: 0,
            current: 2,
            auxiliary: 0,
            reference: None,
        };
        assert!(world.receive_known_ai_counter_status(owner, &event)?);
        assert!(world.ai_brains[&owner].reevaluation_requested);
        world
            .ai_brains
            .get_mut(&owner)
            .unwrap()
            .reevaluation_requested = false;
        let mut other = event.clone();
        other.source = Some(world.player_entity);
        assert!(!world.receive_known_ai_counter_status(owner, &other)?);
        assert!(!world.ai_brains[&owner].reevaluation_requested);
        let original = world.paint_counters[&owner].clone();
        assert_eq!(
            world.npc_puddle_eligible(owner, behavior)?,
            original.status() == 2
        );
        // Controlled changes to the runtime scalar state. This verifies the
        // query/receiver dependency, not paint hits or native counter updates.
        world.paint_counters.get_mut(&owner).unwrap().value = original.thinner_threshold;
        assert!(world.npc_puddle_eligible(owner, behavior)?);
        let name = serde_json::to_string(&world.entities[owner].name)?;
        let code = format!(
            "assert(GetPropertyFloat(GetEntity({name}),'CounterStart')=={})",
            original.thinner_threshold
        );
        world.with_script_world_host(World::script_world_api, |scripts| {
            scripts.ensure_global_module("@npc-counter-live-getter", code.as_bytes())
        })?;
        world.paint_counters.get_mut(&owner).unwrap().value = original.paint_threshold;
        assert!(!world.npc_puddle_eligible(owner, behavior)?);
        *world.paint_counters.get_mut(&owner).unwrap() = original;
        let before_retirement = super::super::debug_view::runtime_entity_heap_bytes(&world);
        let retired_identity = world.entities[owner].name.clone();
        let retired_name = serde_json::to_string(&retired_identity)?;
        world.run_script(&format!("BwxRetiredOwner=GetEntity({retired_name})"))?;
        world.retire_entity(owner)?;
        let after_retirement = super::super::debug_view::runtime_entity_heap_bytes(&world);
        assert!(!world.paint_counters.contains_key(&owner));
        assert_eq!(world.entities[owner].components.capacity(), 0);
        assert_eq!(world.entities[owner].mesh_draws.capacity(), 0);
        assert!(after_retirement.1 < before_retirement.1);
        assert!(after_retirement.2 < before_retirement.2);
        assert_eq!(world.entities[owner].name, retired_identity);
        world.run_script(&format!("assert(BwxRetiredOwner~=nil); assert(GetEntity({retired_name})==nil); assert(select('#',GetPropertyFloat(BwxRetiredOwner,'CounterStart'))==0)"))?;
        assert!(world.npc_puddle_eligible(owner, behavior).is_err());
        println!(
            "Retired entity released {:.1} KiB of component/property heap",
            (before_retirement.1 - after_retirement.1 + before_retirement.2
                - after_retirement.2) as f64
                / 1024.0
        );
        let live_actors = world.authored_havok.entities();
        let player_skin_models = world
            .player_meshes
            .iter()
            .map(|mesh| mesh.asset.as_str())
            .collect::<std::collections::HashSet<_>>();
        let retired_actor = live_actors.iter().find_map(|&entity| {
            let model = world.authored_havok.skin_model_for_entity(entity)?;
            let sole_actor = live_actors
                .iter()
                .filter(|&&other| {
                    world.authored_havok.skin_model_for_entity(other) == Some(model)
                })
                .count()
                == 1;
            (sole_actor
                && !player_skin_models.contains(model)
                && world.skins.contains_key(model)
                && !world
                    .named_fx
                    .live
                    .get(&entity)
                    .is_some_and(|effect| !effect.ready))
            .then(|| (entity, model.to_owned()))
        });
        let (actor, model) = retired_actor.context(
            "Expected an authored actor with a skin model not shared by Mickey or another actor",
        )?;
        let released = world.skins[&model]
            .iter()
            .map(super::super::skin::owned_buffer_bytes)
            .sum::<usize>();
        let released_model_bytes = world.authored_havok.cached_model_payload_bytes(&model);
        assert!(released > 0, "Actor skin payload should have owned buffers");
        assert!(released_model_bytes > 0, "Actor model cache should own mesh buffers");
        world.retire_entity(actor)?;
        assert!(!world.authored_havok.entities().contains(&actor));
        assert!(!world.skins.contains_key(&model));
        assert_eq!(world.authored_havok.cached_model_payload_bytes(&model), 0);
        println!(
            "Retired actor released {:.1} KiB skin and {:.1} KiB cached model payload",
            released as f64 / 1024.0,
            released_model_bytes as f64 / 1024.0
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_lua_math_changes_the_world_gameplay_random_stream() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let before = world.game_random.draws;
        world.with_script_world_host(World::script_world_api, |scripts| {
            scripts.ensure_global_module(
                "@shared-lua-random-world",
                b"assert(select('#',math.randomseed(1))==0); assert(math.random(100)==52)",
            )
        })?;
        assert_eq!(world.game_random.seed, 0x41c67ea6);
        assert_eq!(world.game_random.draws, before + 1);
        assert_eq!(
            world.game_random.draw(),
            5758,
            "Gameplay did not follow the Lua draw"
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_lua_float_properties_commit_runtime_effects_and_keep_native_return_counts(
    ) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let (&(entity, component), _) = world
            .rotators
            .iter()
            .next()
            .context("Original rotator missing")?;
        let name = serde_json::to_string(&world.entities[entity].name)?;
        let code=format!("local target=GetEntity({name}); assert(select('#',GetPropertyFloat(target,'Max Speed'))==1); assert(select('#',SetPropertyFloat(target,'Max Speed','75',0))==0); assert(GetPropertyFloat(target,'Max Speed',0)==75); assert(select('#',GetPropertyFloat(target,'MissingFloat'))==0); assert(select('#',SetPropertyFloat(target,'MissingFloat',1))==0); assert(select('#',GetPropertyFloat(target,'Translation'))==0); assert(select('#',GetPropertyFloat('missing-entity','Max Speed'))==0); assert(not pcall(SetPropertyFloat,target,'Max Speed',25,2))");
        world.with_script_world_host(World::script_world_api, |scripts| {
            scripts.ensure_global_module("@float-property-runtime-regression", code.as_bytes())
        })?;
        let property = world.entities[entity]
            .components
            .iter()
            .find(|c| c.link_id.to_u32() == component)
            .unwrap()
            .properties
            .iter()
            .find(|p| p.name == "Max Speed")
            .unwrap();
        assert_eq!(property.value.as_f64(), Some(75.));
        let before = world.entities[entity].runtime_rotation;
        for tick in 0..20 {
            world.prepare_scene_motion(super::super::STEP)?;
            world.dynamic_runtime_time += super::super::STEP;
            world.scene_motion_prepared_at = None;
        }
        ensure!(
            world.entities[entity].runtime_rotation != before,
            "Lua speed setter did not affect live rotator motion"
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_world_child_lookup_and_visibility_use_live_membership_not_transform_descendants(
    ) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let owner = world
            .entities
            .iter()
            .position(|entity| entity.name.contains("Gremlin_Variant4 01"))
            .context("Original Gremlin missing")?;
        let wrench = world
            .child_entity_by_name(owner, "GremlinWrench")?
            .context("Native wrench child was not instantiated")?;
        assert_eq!(
            world.entities[wrench].transform_parent_id()?,
            Some(world.entities[owner].id)
        );
        let child = world
            .entities
            .iter()
            .enumerate()
            .find(|(index, entity)| {
                *index != owner
                    && entity.scene == world.entities[owner].scene
                    && !entity.draws.is_empty()
                    && entity.transform_parent_id().ok().flatten().is_none()
            })
            .context("Expected independently placed rendered entity")?
            .0;
        // Exercise the real World/Lua integration with a controlled membership
        // edit. This is not evidence that native template instantiation works.
        let id = world.entities[child].id;
        world.entities[owner]
            .components
            .iter_mut()
            .find(|c| c.class_name == "ChildComponent")
            .context("Child component missing")?
            .properties
            .iter_mut()
            .find(|p| p.name == "Child Entities")
            .context("Child list missing")?
            .value = serde_json::json!([0, id]);
        let owner_name = serde_json::to_string(&world.entities[owner].name)?;
        let child_name = serde_json::to_string(&world.entities[child].name)?;
        let code=format!("local parent=GetEntity({owner_name}); local child=GetChildEntityByName(parent,{child_name}); assert(child==GetEntity({child_name})); assert(not IsHidden(child)); Hide(child); assert(IsHidden(child)); assert(not IsHidden(parent)); Unhide(child); assert(not IsHidden(child)); assert(not IsHidden(0)); assert(GetChildEntityByName(0,'missing')==0)");
        world.with_script_world_host(World::script_world_api, |scripts| {
            scripts.ensure_global_module(
                "@native-child-membership",
                &crate::lua_cook::compile(&code)?,
            )
        })?;
        ensure!(
            !world.hidden_entities.contains(&child) && !world.hidden_entities.contains(&owner),
            "Unhide did not restore entity visibility"
        );
        world.removed_entities.insert(child);
        assert_eq!(
            world.child_entity_by_name(owner, &world.entities[child].name)?,
            None
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn native_io_animation_inputs_use_native_controller_bridge() -> Result<()> {
        let root=std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world=load(Path::new(&root),"Levels/GV_ZoneA_Start.level",&egui::Context::default())?;
        let _preview=super::super::authored_preview::Preview::new(&mut world)?;
        let player=world.player_entity;
        let mut connection=super::super::actor_io::Connection {instigator:None,target_guid:None,
            owner:player,output:"OnUser1".into(),target:"_self".into(),group:String::new(),
            input:"AnimEvent".into(),parameters:"EVENT_Special_1".into(),delay:0.,flags:0,
        };
        let before=world.player_graph.machines.iter().map(|(id,m)|(id.clone(),m.current)).collect::<Vec<_>>();
        world.dispatch_actor_input(player,None,&connection,None)?;
        let after=world.player_graph.machines.iter().map(|(id,m)|(id.clone(),m.current)).collect::<Vec<_>>();
        ensure!(before!=after,"IO event did not change Mickey's graph synchronously");
        ensure!(world.player_script_animation_events.len()==1,"IO event bypassed native external-event publication");
        // Direct Lua also accepts the native integer reader's global-name form.
        world.run_callback_fixture("AnimEvent(GetPlayer(),'EVENT_Special_1')")?;
        ensure!(world.player_script_animation_events.len()==2,"Direct string-ID delivery failed");
        let pipe=world.scripts.resolve_name("gv_zonea_airlock.GV_ZoneB_EntryDoorA1 01").context("Original GB actor absent")?;
        connection.input="AnimGBSequence".into();
        for name in ["Open2","Rest"] {
            let id=world.actor_sequences[&pipe].find_name(name);
            ensure!(world.actor_sequences[&pipe].get(id).is_some_and(|(sequence,_)|sequence.name.as_deref()==Some(name)),"Original named sequence absent");
            connection.parameters=format!("{name},false,0,0,false");
            world.dispatch_actor_input(pipe,None,&connection,None)?;
            ensure!(world.actors[&pipe].requested_id()==Some(id),"GB IO failed to parse sequence and modifier arguments");
        }
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_lua_anim_event_dispatches_player_graph_before_return() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let mut preview = super::super::authored_preview::Preview::new(&mut world)?;
        let id = world
            .animation_events
            .as_ref()
            .map_err(|e| anyhow::anyhow!(e.clone()))?
            .id("EVENT_Special_1")
            .context("Missing original Special_1 ordinal")?;
        let before = world
            .player_graph
            .machines
            .iter()
            .map(|(id, machine)| (id.clone(), machine.current))
            .collect::<Vec<_>>();
        world.run_callback_fixture(&format!("assert(EVENT_Special_1=={id}); assert(select('#',AnimEvent(GetPlayer(),EVENT_Special_1))==0)"))?;
        let after = world
            .player_graph
            .machines
            .iter()
            .map(|(id, machine)| (id.clone(), machine.current))
            .collect::<Vec<_>>();
        assert_ne!(
            before, after,
            "AnimEvent did not synchronously change the authored player graph"
        );
        let deliveries = std::mem::take(&mut world.player_script_animation_events);
        assert_eq!(deliveries.len(), 1);
        let event = deliveries[0].0.clone();
        for (event, emitted) in deliveries {
            preview.playback.record_external_delivery(event, emitted);
        }
        assert_eq!(
            preview
                .playback
                .delivered_events
                .last()
                .map(|value| value.id),
            Some(event.id)
        );
        assert_eq!(
            world
                .player_graph
                .machines
                .iter()
                .map(|(id, machine)| (id.clone(), machine.current))
                .collect::<Vec<_>>(),
            after,
            "Playback adoption dispatched the command again"
        );
        preview.playback.advance_character(
            &world.player_behavior,
            &mut world.player_graph,
            &mut world.player_animations,
            1,
        )?;
        assert_eq!(
            preview
                .playback
                .delivered_events
                .iter()
                .filter(|value| value.id == event.id)
                .count(),
            1,
            "Script event was lost or redispatched at the animation tick boundary"
        );
        world.run_callback_fixture("assert(select('#',AnimEvent('BwxMissing',-1))==0)")?;
        assert!(world.player_script_animation_events.is_empty());
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_audio_switch_lua_updates_registered_entity_values_without_global_state_aliasing(
    ) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let other_name = "gremlinvillage_zonea_dynamic.GV_ZoneA_Wheel01 01";
        let other = world
            .entities
            .iter()
            .position(|entity| entity.name == other_name)
            .context("Original wheel audio owner missing")?;
        let before = world.audio_playback.switch_control_requests;
        world.run_callback_fixture(&format!("assert(select('#',AudioSetSwitch(GetPlayer(),'Bwx_Surface','Wood'))==0); assert(select('#',AudioSetSwitch(GetEntity({other_name:?}),'BWX_SURFACE','Metal'))==0); assert(select('#',AudioSetSwitch('missing-audio-owner','Bwx_Surface','Stone'))==0)"))?;
        assert_eq!(world.audio_playback.switch_control_requests, before + 2);
        world.audio_playback.poll()?;
        assert_eq!(
            world
                .audio_playback
                .object_switch_value(world.player_entity, "bwx_surface")?,
            Some(super::super::audio_bank::event_id("wood")?)
        );
        assert_eq!(
            world
                .audio_playback
                .object_switch_value(other, "bwx_surface")?,
            Some(super::super::audio_bank::event_id("metal")?)
        );
        assert!(!world
            .demo_story
            .scene
            .audio_states
            .contains_key("Bwx_Surface"));
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_audio_post_failure_does_not_abort_following_lua_commands() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let before = world.audio_playback.rejected_post_count;
        world.run_callback_fixture("assert(AudioPostEventOn(GetPlayer(),'Set_Volume_Enter_2D')==0); SetGlobal('PlayerFirstTimeInZone',9); assert(select('#',AudioPostEventOn('missing','Set_Volume_Enter_2D'))==0)")?;
        assert_eq!(world.globals.get("PlayerFirstTimeInZone"), Some(9));
        assert_eq!(world.audio_playback.rejected_post_count, before + 1);
        assert!(world
            .report
            .unsupported
            .iter()
            .any(|line| line.starts_with("Audio preview rejected:")
                && line.contains("Set_Volume_Enter_2D")));
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_multi_position_audio_connects_authored_io_lua_and_dynamic_source_updates(
    ) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        ensure!(
            world.multi_position_audio.len() >= 8,
            "Original audio controllers not initialized"
        );
        ensure!(
            !world
                .report
                .unsupported
                .iter()
                .any(|message| message
                    .contains("Missing authored script callback MultiPosSoundStart")),
            "Native audio call still fell through to a missing Lua callback"
        );
        let (&key, _) = world
            .multi_position_audio
            .iter()
            .find(|(_, state)| {
                state.active
                    && state.dynamic
                    && !state.positions.is_empty()
                    && state.error.is_none()
            })
            .context("No original dynamic multi-position source started")?;
        let name = world.entities[key.0].name.clone();
        let updates = world.multi_position_audio[&key].position_updates;
        world.advance_models(world.animation_time + super::super::STEP)?;
        assert!(world.multi_position_audio[&key].position_updates > updates);
        let count = world.audio_playback.spatial_post_rejections;
        world.run_callback_fixture(&format!(
            "assert(select('#',MultiPosSoundStop(GetEntity({name:?})))==0)"
        ))?;
        assert!(!world.multi_position_audio[&key].active);
        let updates = world.multi_position_audio[&key].position_updates;
        world.advance_models(world.animation_time + super::super::STEP)?;
        assert_eq!(world.multi_position_audio[&key].position_updates, updates);
        world.run_callback_fixture(&format!("assert(select('#',MultiPosSoundStart(GetEntity({name:?})))==0); assert(select('#',MultiPosSoundStart('BwxMissingAudioOwner'))==0)"))?;
        assert!(world.multi_position_audio[&key].active);
        assert_eq!(
            world.multi_position_audio[&key].playing_id, 0,
            "Unimplemented spatial playback must not invent a successful voice"
        );
        assert_eq!(world.audio_playback.spatial_post_rejections, count + 1);
        assert!(world
            .audio_playback
            .rejected_posts
            .values()
            .any(|post| post.owner == key.0 && post.reason.contains("Multi-position")));
        let state = &world.multi_position_audio[&key];
        assert!(state
            .positions
            .iter()
            .all(|position| position[3].to_bits() == 0));
        world.retire_entity(key.0)?;
        assert!(!world
            .multi_position_audio
            .keys()
            .any(|(owner, _)| *owner == key.0));
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_multi_position_audio_reads_current_source_paint_flags() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let candidate = world
            .multi_position_audio
            .iter()
            .filter(|(_, state)| state.active && state.error.is_none())
            .find_map(|(&key, state)| {
                let component = world.entities[key.0]
                    .components
                    .iter()
                    .find(|c| c.link_id.to_u32() == key.1)?;
                if component
                    .properties
                    .iter()
                    .find(|p| p.name == "Play Condition")?
                    .value
                    .as_i64()
                    != Some(1)
                {
                    return None;
                }
                state.sources.iter().flatten().find_map(|&source| {
                    world.entities[source]
                        .components
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| c.class_name == "PhysicsRigidBodyStaticComponent")
                        .find_map(|(component, c)| {
                            c.properties
                                .iter()
                                .enumerate()
                                .find(|(_, p)| p.name == "Is Painted" && p.value.is_boolean())
                                .map(|(property, _)| (key, source, component, property))
                        })
                })
            })
            .context("Original GV has no condition1 static audio source")?;
        let (key, source, component, property) = candidate;
        let occurrences = world.multi_position_audio[&key]
            .sources
            .iter()
            .filter(|&&value| value == Some(source))
            .count();
        world.entities[source].components[component].properties[property].value =
            serde_json::json!(false);
        world.control_multi_position_audio(key.0, true)?;
        let unpainted = world.multi_position_audio[&key].positions.len();
        world.entities[source].components[component].properties[property].value =
            serde_json::json!(true);
        world.control_multi_position_audio(key.0, true)?;
        assert_eq!(
            world.multi_position_audio[&key].positions.len(),
            unpainted + occurrences,
            "Source selection cached an old paint flag or discarded duplicate source handles"
        );
        assert_eq!(world.multi_position_audio[&key].playing_id, 0);
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn native_animation_feedback_reaches_footstep_lua_and_fidget_owner() -> Result<()> {
        use super::*;
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let target = world
            .ai_brains
            .iter()
            .find(|(owner, brain)| {
                brain.ai_type.starts_with("Gremlin") && world.authored_havok.contains(**owner)
            })
            .map(|(&owner, _)| owner)
            .context("Missing original Gremlin controller")?;
        let scene = SceneFile::new(
            vec![],
            vec![],
            vec![],
            epic_mickey_lib_rs::scene_file::ID::new(0),
            epic_mickey_lib_rs::scene_file::SceneFileVersion::Version1,
        );
        let mut listener = crate::component_schema::CATALOG.components["GameEventsComponent"]
            .instantiate("GameEventsComponent", &scene)?;
        for property in &mut listener.properties {
            property.value = match property.name.as_str() {
                "GameEventTypes" => serde_json::json!([0, 2]),
                "EventSourceAliases" => serde_json::json!(["", ""]),
                "ScriptFunctionNames" => {
                    serde_json::json!(["BwxFootstepNormal", "BwxFootstepAlternate"])
                }
                "AudioEventNames" | "EffectEventNames" | "ActionEventNames" => {
                    serde_json::json!([])
                }
                "EventDelays" => serde_json::json!([0, 0]),
                _ => property.value.clone(),
            };
        }
        world.component_events.bind(target, &listener)?;
        // Explicit test observers verify delivery; original files are unchanged.
        world.with_script_world_host(World::script_world_api,|scripts|scripts.ensure_global_module(
            "@test/native-footstep-observers",b"BwxStepNormal=0; BwxStepAlternate=0; function BwxFootstepNormal(self) assert(GetGlobal('PlayerFirstTimeInZone')~=nil); BwxStepNormal=BwxStepNormal+1 end; function BwxFootstepAlternate(self) BwxStepAlternate=BwxStepAlternate+1 end"))?;
        let noise = world.controller_feedback.noise_requests;
        world.send_native_actor_animation_event(target, 0x38)?;
        assert_eq!(world.controller_feedback.noise_requests, noise + 1);
        assert_eq!(world.controller_feedback.last_noise_source, Some(target));
        world.controller_feedback.variant = true; // exercise the native alternate byte branch
        world.send_native_actor_animation_event(target, 0x39)?;
        world.send_native_actor_animation_event(target, 0x8d)?;
        assert_eq!(world.controller_feedback.noise_requests, noise + 3);
        world.send_native_actor_animation_event(target, 0x8e)?;
        assert!(world.ai_brains[&target].uninterruptible_fidget);
        world.send_native_actor_animation_event(target, 0x8f)?;
        assert!(!world.ai_brains[&target].uninterruptible_fidget);
        let name = serde_json::to_string(&world.entities[target].name)?;
        world.controller_feedback.variant = false;
        world.with_script_world_host(World::script_world_api, |scripts| {
            scripts.ensure_global_module(
                "@test/native-footstep-nested-api",
                format!("BwxNativeAnimationEvent(GetEntity({name}),56)").as_bytes(),
            )
        })?;
        assert_eq!(world.controller_feedback.noise_requests, noise + 4);
        world.with_script_world_host(World::script_world_api,|scripts|scripts.ensure_global_module(
            "@test/native-footstep-results",b"assert(BwxStepNormal==2 and BwxStepAlternate==1, 'Native footstep responses were not delivered')"))?;
        ensure!(
            !world
                .report
                .unsupported
                .iter()
                .any(|line| line.contains("BwxFootstep")
                    || line.contains("Native footstep responses")),
            "Native animation feedback failed to execute its Lua observers"
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_spatter_paused_behavior_receives_ai_events_without_claiming_tree_selection(
    ) -> Result<()> {
        use super::*;
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let owner = world
            .ai_brains
            .iter()
            .find(|(_, brain)| brain.ai_type == "SpatterMelee" && !brain.paused.is_empty())
            .map(|(&owner, _)| owner)
            .context("Original Spatter Paused behavior not imported")?;
        let brain = &world.ai_brains[&owner];
        assert_eq!(
            brain.selector.ordered,
            (0..brain.behaviors.len()).collect::<Vec<_>>()
        );
        assert_eq!(brain.selector.current, None);
        assert_eq!(
            brain
                .behaviors
                .iter()
                .filter(|name| name.as_str() == "Death")
                .count(),
            2,
            "Authored duplicate behavior slots must survive import"
        );
        assert!(brain.paused.values().all(|paused| !paused.active));
        assert!(!brain.reevaluation_requested);
        let event = |action| super::super::ai_behavior_events::Event {
            kind: 0x0c,
            source: Some(owner),
            action,
        };
        assert!(world.receive_known_ai_behavior_event(owner, event(13))?);
        assert!(world.ai_brains[&owner]
            .paused
            .values()
            .all(|paused| paused.active));
        assert!(world.ai_brains[&owner].reevaluation_requested);
        assert!(world.receive_known_ai_behavior_event(owner, event(14))?);
        assert!(world.ai_brains[&owner]
            .paused
            .values()
            .all(|paused| !paused.active));
        // REL2de788 writes the component's motion byte independently of the
        // source. The Paused/ScriptedIdle subscriptions still reject another
        // entity's identity at the following behavior-delivery stage.
        world
            .ai_brains
            .get_mut(&owner)
            .unwrap()
            .reevaluation_requested = false;
        let foreign = super::super::ai_behavior_events::Event {
            kind: 0x0c,
            source: Some(world.player_entity),
            action: 11,
        };
        assert_ne!(owner, world.player_entity);
        assert!(!world.receive_known_ai_behavior_event(owner, foreign)?);
        assert!(world.ai_brains[&owner].motion_suspended);
        assert!(!world.ai_brains[&owner].reevaluation_requested);
        assert!(world.ai_brains[&owner]
            .paused
            .values()
            .all(|paused| !paused.active));
        assert!(!world.receive_known_ai_behavior_event(
            owner,
            super::super::ai_behavior_events::Event {
                action: 12,
                ..foreign
            }
        )?);
        assert!(!world.ai_brains[&owner].motion_suspended);
        assert!(world
            .report
            .unsupported
            .iter()
            .any(|message| message.contains("AI behavior tree")
                && message.contains("selection and controller output pending")));
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_nested_projector_blends_keep_live_source_clocks_and_promote_children() -> Result<()>
    {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let camera = |suffix: &str| {
            world.entities.iter().position(|entity| {
                entity.name
                    == format!("gremlinvillage_zonea.seq_projector 01.{suffix}ProjectorCamera")
            })
        };
        let a = camera("Exit").context("Exit camera missing")?;
        let b = camera("Enter").context("Enter camera missing")?;
        world.select_nif_camera(Some(a), None, 1, 0.)?;
        let selection_a = world.nif_camera.unwrap();
        let frame_a = world.sample_nif_camera(selection_a)?;
        world.select_nif_camera(Some(b), None, 1, 2.)?;
        let selection_b = world.nif_camera.unwrap();
        let frame_b = world.sample_nif_camera(selection_b)?;
        world.advance_nif_camera_transition(0.5)?;
        let previous = world
            .active_nif_camera_frame()?
            .context("First blend output missing")?;
        world.select_nif_camera(Some(a), None, 1, 3.)?;
        assert_eq!(
            world.active_nif_camera_frame()?.unwrap().eye,
            previous.eye,
            "Interruption froze or jumped the outgoing blend"
        );
        assert_eq!(
            world
                .nif_camera_transition
                .as_ref()
                .unwrap()
                .source_transition
                .as_ref()
                .unwrap()
                .clock
                .elapsed,
            0.5
        );
        world.advance_nif_camera_transition(0.5)?;
        let outer = world.nif_camera_transition.as_ref().unwrap();
        let inner = outer.source_transition.as_ref().unwrap();
        assert_eq!(inner.clock.elapsed, 1.);
        assert_eq!(outer.clock.elapsed, 0.5);
        let expected: [f32; 3] = std::array::from_fn(|i| {
            let outgoing = frame_b.eye[i].mul_add(
                inner.clock.weight,
                frame_a.eye[i] * (1. - inner.clock.weight),
            );
            frame_a.eye[i].mul_add(outer.clock.weight, outgoing * (1. - outer.clock.weight))
        });
        assert_eq!(world.active_nif_camera_frame()?.unwrap().eye, expected);
        world.advance_nif_camera_transition(1.)?;
        let outer = world.nif_camera_transition.as_ref().unwrap();
        assert!(
            outer.source_transition.is_none(),
            "Completed child blend retained its outgoing controller"
        );
        assert!(
            outer.source_output.is_some(),
            "Child output was lost before parent composition"
        );
        // The promoted source remains live after its old blend is released.
        world.entities[b].transform[12] += 4.;
        let moved_b = world.sample_nif_camera(selection_b)?;
        world.advance_nif_camera_transition(0.25)?;
        let outer = world.nif_camera_transition.as_ref().unwrap();
        assert!(outer.source_output.is_none());
        let expected: [f32; 3] = std::array::from_fn(|i| {
            frame_a.eye[i].mul_add(
                outer.clock.weight,
                moved_b.eye[i] * (1. - outer.clock.weight),
            )
        });
        assert_eq!(
            world.active_nif_camera_frame()?.unwrap().eye,
            expected,
            "Promoted camera source stopped following the entity"
        );
        world.advance_nif_camera_transition(1.25)?;
        assert!(world.nif_camera_transition.is_none());
        assert!(world.nif_camera_completed_frame.is_some());
        world.advance_nif_camera_transition(0.)?;
        assert!(world.nif_camera_completed_frame.is_none());
        world.select_nif_camera(Some(b), None, 1, 0.)?;
        assert!(world.nif_camera_transition.is_none());
        assert_eq!(world.active_nif_camera_frame()?.unwrap().eye, moved_b.eye);
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_projector_camera_loads_and_selects_through_native_lua() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let name = "gremlinvillage_zonea.seq_projector 01.ExitProjectorCamera";
        let entity = world
            .entities
            .iter()
            .position(|entity| entity.name == name)
            .context("Original projector camera missing")?;
        let graph = world
            .entity_bound_hierarchy(entity)?
            .context("Original camera hierarchy not parsed")?;
        ensure!(
            graph.cameras.contains_key(&15),
            "Original NiCamera block15 not available"
        );
        let old = graph.cameras[&15].clone();
        ensure!(
            old.frustum.iter().all(|v| v.is_finite()),
            "Original camera frustum invalid"
        );
        world.with_script_world_host(World::script_world_api,|scripts|scripts.ensure_global_module("@projector-camera-cut",
            b"assert(select('#',GrabCameraNif(GetEntity('gremlinvillage_zonea.seq_projector 01.ExitProjectorCamera'),nil,1,0))==0)"))?;
        let selection = world
            .nif_camera
            .context("Native camera grab did not select source")?;
        assert_eq!(selection.entity, entity);
        assert_eq!(selection.node, 15);
        let frame = world.sample_nif_camera(selection)?;
        frame.projection()?;
        ensure!(
            frame
                .eye
                .iter()
                .chain(frame.direction.iter())
                .chain(frame.up.iter())
                .all(|v| v.is_finite()),
            "Invalid native camera transform"
        );
        // The source NIF is not the application's final camera projection.
        // Exact bits below execute the retail source-FOV + application lens
        // instruction paths for this projector and the shipped configuration.
        let lens_reference: serde_json::Value = serde_json::from_str(include_str!(
            "../../tools/blotworx/fixtures/camera_lens.json"
        ))?;
        let lens_case = lens_reference["projections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["label"] == "retail_projector")
            .context("Missing original projector lens reference")?;
        let expected: [u32; 6] =
            std::array::from_fn(|i| lens_case["bits"][i].as_u64().unwrap() as u32);
        assert_eq!(frame.frustum.map(f32::to_bits), expected);
        assert_eq!(
            old.frustum[5], 500.,
            "Source camera data must remain unchanged"
        );
        assert_eq!(
            frame.native_lens.unwrap().field_of_view,
            lens_case["fov"].as_f64().unwrap() as f32
        );
        world.with_script_world_host(World::script_world_api,|scripts|scripts.ensure_global_module("@projector-camera-blend",
            b"assert(select('#',GrabCameraNif(GetEntity('gremlinvillage_zonea.seq_projector 01.EnterProjectorCamera'),nil,3,2))==0)"))?;
        ensure!(
            world.nif_camera_transition.is_some(),
            "Native mode3 did not construct a blend"
        );
        assert_eq!(
            world.nif_camera_transition.as_ref().unwrap().source.entity,
            entity
        );
        world.advance_nif_camera_transition(1.)?;
        assert_eq!(
            world
                .nif_camera_transition
                .as_ref()
                .unwrap()
                .clock
                .weight
                .to_bits(),
            0x3f000001
        );
        let mixed = world
            .active_nif_camera_frame()?
            .context("Blended camera frame missing")?;
        mixed.projection()?;
        world.advance_nif_camera_transition(1.)?;
        ensure!(
            world.nif_camera_transition.is_none(),
            "Completed handoff retained outgoing controller"
        );
        ensure!(
            world.nif_camera.unwrap().entity != entity,
            "Handoff did not retain destination"
        );
        world.select_nif_camera(None, None, 1, 0.)?;
        assert!(world.nif_camera.is_none());
        assert!(world.select_nif_camera(Some(entity), None, 1, 1.).is_err());
        assert!(
            world.nif_camera.is_none(),
            "Rejected blend partially changed the camera"
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_nif_focus_uses_model_bounds_tracks_relocation_and_retains_outgoing_target(
    ) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let source_name = "gremlinvillage_zonea.seq_projector 01.ExitProjectorCamera";
        let source = world
            .entities
            .iter()
            .position(|e| e.name == source_name)
            .context("Missing original camera")?;
        let destination = world
            .entities
            .iter()
            .position(|e| e.name == "gremlinvillage_zonea.seq_projector 01.EnterProjectorCamera")
            .context("Missing destination camera")?;
        let focus = (0..world.entities.len())
            .find(|&id| {
                if id == world.player_entity || id == source || id == destination {
                    return false;
                }
                let entity = &world.entities[id];
                if !entity.collision_parts.is_empty()
                    || entity.jigsaw_asset.is_some()
                    || !matches!(entity.transform_parent_id(), Ok(None))
                    || entity
                        .components
                        .iter()
                        .any(|c| c.class_name.starts_with("Physics"))
                {
                    return false;
                }
                if world.entities.iter().any(|child| {
                    child.scene == entity.scene
                        && child.transform_parent_id().ok().flatten() == Some(entity.id)
                }) {
                    return false;
                }
                let Ok(Some(graph)) = world.entity_bound_hierarchy(id) else {
                    return false;
                };
                if graph.nodes.values().any(|n| n.needs_runtime_bound) {
                    return false;
                }
                if !graph
                    .nodes
                    .values()
                    .any(|n| n.authored_bound.is_some_and(|s| s.radius > 0.))
                {
                    return false;
                }
                world
                    .camera_focus_point(id)
                    .ok()
                    .flatten()
                    .is_some_and(|point| point != entity.transform[12..15])
            })
            .context("No original rigid model with a distinct world-bound center")?;
        let target = world
            .camera_focus_point(focus)?
            .context("Focus target missing")?;
        let focus_name = serde_json::to_string(&world.entities[focus].name)?;
        world.run_callback_fixture(&format!("assert(select('#',GrabCameraNif(GetEntity('{source_name}'),GetEntity({focus_name}),1,0))==0)"))?;
        let selection = world.nif_camera.context("Focused camera not acquired")?;
        assert_eq!(selection.focus, Some(focus));
        let frame = world.sample_nif_camera(selection)?;
        let expected = super::super::camera_focus::orientation(frame.eye, target)?
            .context("Degenerate fixture focus")?;
        assert_eq!(frame.direction, expected.direction);
        assert_eq!(frame.up, expected.up);
        assert_eq!(frame.left, expected.left);
        world.select_nif_camera(Some(source), None, 1, 0.)?;
        let authored = world.active_nif_camera_frame()?.unwrap();
        assert_eq!(authored.eye, frame.eye);
        assert_eq!(authored.frustum, frame.frustum);
        assert_ne!(authored.direction, frame.direction);
        world.select_nif_camera(Some(source), Some(focus), 1, 0.)?;
        let runtime = &world.entities[focus];
        let mut position: [f32; 3] = runtime.transform[12..15].try_into().unwrap();
        position[0] += 4.;
        let rotation = runtime.runtime_rotation;
        world.relocate_entity(focus, position, rotation)?;
        let moved = world.camera_focus_point(focus)?.unwrap();
        assert!((moved[0] - target[0] - 4.).abs() < 1e-4);
        assert!((moved[1] - target[1]).abs() < 1e-4 && (moved[2] - target[2]).abs() < 1e-4);
        let live = world.active_nif_camera_frame()?.unwrap();
        assert_eq!(live.eye, frame.eye);
        assert_ne!(live.direction, frame.direction);
        world.select_nif_camera(Some(source), Some(world.player_entity), 1, 0.)?;
        assert_eq!(
            world.nif_camera.unwrap().focus,
            Some(
                world
                    .mickey_tracker
                    .as_ref()
                    .context("Missing tracker")?
                    .owner
            )
        );
        world.select_nif_camera(Some(source), Some(focus), 1, 0.)?;
        world.select_nif_camera(Some(destination), None, 3, 2.)?;
        assert_eq!(
            world.nif_camera_transition.as_ref().unwrap().source.focus,
            Some(focus)
        );
        world.advance_nif_camera_transition(0.5)?;
        world
            .active_nif_camera_frame()?
            .context("Focused outgoing handoff failed")?;
        world.removed_entities.insert(focus);
        let retired = world.sample_nif_camera(selection)?;
        assert_eq!(retired.direction, authored.direction);
        assert_eq!(retired.up, authored.up);
        world.removed_entities.remove(&focus);
        let marker = (0..world.entities.len())
            .find(|&id| {
                id != world.player_entity
                    && world.entities[id].model_asset.is_none()
                    && !world.removed_entities.contains(&id)
            })
            .context("No original position-only focus target")?;
        assert_eq!(
            world.camera_focus_point(marker)?.unwrap(),
            world.entities[marker].transform[12..15]
        );
        assert!(world.camera_focus_point(usize::MAX).is_err());
        eprintln!("Focus target: {}", world.entities[focus].name);
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_destroy_entity_missing_targets_decline_without_fake_live_teardown() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        ensure!(
            !world
                .report
                .unsupported
                .iter()
                .any(|error| error.contains("attempt to call global 'DestroyEntity'")),
            "Projector setup still lacks its native binding"
        );
        let removed = world.removed_entities.clone();
        let player = world.player_entity;
        world.run_callback_fixture("assert(select('#',DestroyEntity('Bwx_No_Such_Entity'))==0); assert(select('#',DestroyEntity(nil))==0); assert(select('#',DestroyEntity(true))==0); assert(select('#',DestroyEntity({}))==0); assert(select('#',DestroyEntity(987654321))==0); local ok,why=pcall(DestroyEntity,GetPlayer()); assert(not ok and string.find(tostring(why),'deferred component')); assert(GetPlayer()~=nil)")?;
        assert_eq!(world.removed_entities, removed);
        ensure!(
            !world.removed_entities.contains(&player),
            "Unsupported destruction removed the live player"
        );
        ensure!(!world.report.unsupported.iter().any(|error|error.contains("SetupProjector") && error.contains("DestroyEntity")),
            "Original projector destroys a real target whose lifecycle still needs integration: {:?}",world.report.unsupported);
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_ability_restore_uses_give_receiver_without_revoking_or_clearing_disable(
    ) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let brush = world
            .player_abilities
            .get_mut("Brush")
            .context("Missing Brush")?;
        brush.owned = [false, false];
        brush.upgrades = [7, -3];
        brush.disabled = true;
        let mut payload = Vec::new();
        super::super::save_game::append_record(&mut payload, "Brush:1", &2_i32.to_be_bytes(), 4)?;
        world.restore_player_ability_record("Brush", &payload)?;
        let brush = &world.player_abilities["Brush"];
        assert_eq!(brush.owned, [false, true]);
        assert_eq!(brush.upgrades, [7, 7]);
        assert!(brush.disabled);
        assert!(!world.player_ability_enabled("Brush", super::super::brush::Ink::Thinner)?);
        let serialized = world.player_ability_save_record("Brush")?;
        let records = super::super::save_game::records(&serialized)?;
        assert_eq!(records[0].name, "BRUSH:7");
        assert_eq!(records[0].data, &2_i32.to_be_bytes());
        let blast = world
            .player_abilities
            .get_mut("Blast")
            .context("Missing Blast")?;
        blast.owned = [false, false];
        blast.disabled = false;
        blast.upgrades = [1, 4];
        let before = world.ability_hud_requests.len();
        let mut payload = Vec::new();
        super::super::save_game::append_record(&mut payload, "Blast:0", &3_i32.to_be_bytes(), 4)?;
        world.restore_player_ability_record("Blast", &payload)?;
        assert_eq!(world.player_abilities["Blast"].owned, [true, true]);
        assert_eq!(world.player_abilities["Blast"].upgrades, [1, 1]);
        assert_eq!(
            &world.ability_hud_requests[before..],
            &[("Show", "HudElement", "AmmoBars")]
        );
        let mut zero = Vec::new();
        super::super::save_game::append_record(&mut zero, "Blast:0", &0_i32.to_be_bytes(), 4)?;
        world.restore_player_ability_record("Blast", &zero)?;
        assert_eq!(world.player_abilities["Blast"].owned, [true, true]);
        assert_eq!(world.ability_hud_requests.len(), before + 1);
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_player_spawn_bindings_store_override_without_teleporting() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let player = world.player_entity;
        let transform = world.entities[player].transform;
        let rotation = world.entities[player].runtime_rotation;
        let source_scale = world.entities[player].authored_scale;
        let report_spawn = world.report.spawn;
        world.run_callback_fixture(
            "assert(select('#',SetPlayerSpawnTransformEntity(GetPlayer()))==0)",
        )?;
        let saved = world
            .player_spawn
            .override_transform
            .context("Missing spawn override")?;
        assert_eq!(saved.position, transform[12..15]);
        assert_eq!(saved.rotation, rotation);
        assert_eq!(saved.scale.to_bits(), 0x3dcccccd);
        world.player_spawn.clear();
        world.run_callback_fixture(
            "assert(select('#',SetPlayerSpawnTransform(GetName(GetPlayer())))==0)",
        )?;
        assert_eq!(world.player_spawn.override_transform, Some(saved));
        world.run_callback_fixture("SetPlayerSpawnTransform('Bwx_No_Spawn_Entity'); SetPlayerSpawnTransform(nil); SetPlayerSpawnTransformEntity(nil); assert(not pcall(SetPlayerSpawnTransformEntity)); assert(not pcall(SetPlayerSpawnTransform,1,2))")?;
        assert_eq!(world.player_spawn.override_transform, Some(saved));
        assert_eq!(world.entities[player].transform, transform);
        assert_eq!(world.entities[player].runtime_rotation, rotation);
        assert_eq!(world.entities[player].authored_scale, source_scale);
        assert_eq!(world.report.spawn, report_spawn);
        let live = super::super::player_spawn::Transform {
            rotation,
            position: transform[12..15].try_into().unwrap(),
            scale: source_scale,
        };
        let mut bytes = Vec::new();
        super::super::player_spawn::append_records(&mut bytes, world.player_spawn.selected(live))?;
        assert_eq!(
            super::super::player_spawn::saved_transform(&bytes)?,
            Some(saved)
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_camera_resets_reach_live_selection_and_target_subscription() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        world.run_callback_fixture("SetDefaultCameraValues(6.25,18,-10); SetCombatCameraValues(7,20,-11); SetTravelCameraValues(8,22,-12)")?;
        let before = world.camera_profiles.controller.selection;
        world.run_callback_fixture("assert(select('#',ResetCombatCameraValues())==0); assert(select('#',ResetTravelCameraValues())==0)")?;
        assert_eq!(world.camera_profiles.controller.selection, before);
        assert_eq!(
            world.camera_profiles.controller.enabled,
            [true, false, false]
        );
        world.run_callback_fixture("assert(select('#',ResetDefaultCameraValues())==0)")?;
        let c = &world.camera_profiles.controller;
        assert_eq!(c.enabled, [false; 3]);
        assert_eq!(c.selection.duration, 0.5);
        assert_eq!(
            c.selection.current,
            c.library.find("Default").unwrap() as i32
        );
        let custom = c.library.find("CustomDefault").unwrap();
        let id = c.library.profiles[custom].properties[1];
        assert_eq!(c.library.properties[id].value, 6.25);
        assert!(world.camera_target.subscribed);
        assert_eq!(
            world.camera_target.resolved_target,
            Some(world.player_entity)
        );
        for (kind, set, reset, original) in [
            (
                super::super::camera_defaults::Kind::Combat,
                "SetCombatCameraValues",
                "ResetCombatCameraValues",
                "Far",
            ),
            (
                super::super::camera_defaults::Kind::Travel,
                "SetTravelCameraValues",
                "ResetTravelCameraValues",
                "Travel",
            ),
        ] {
            world.camera_profiles.controller.mode = kind.mode();
            world.run_callback_fixture(&format!(
                "{set}(9,25,-15); assert(select('#',{reset}())==0); assert(not pcall({reset},1))"
            ))?;
            let c = &world.camera_profiles.controller;
            assert!(!c.enabled[kind.index()]);
            assert_eq!(
                c.selection.current,
                c.library.find(original).unwrap() as i32
            );
            assert_eq!(c.selection.duration, 0.5);
            assert_eq!(
                world.camera_target.resolved_target,
                Some(world.player_entity)
            );
        }
        // Notification failure must retain the preceding clear/selection writes.
        let original = world
            .camera_profiles
            .controller
            .library
            .find("Travel")
            .unwrap();
        let unknown = world.entities.len() - 2;
        let component = world.entities[world.mickey_tracker.as_ref().unwrap().owner]
            .components
            .iter()
            .find(|component| component.class_name == "EntityTrackerComponent")
            .unwrap()
            .clone();
        world.entities[unknown].components.push(component);
        world.camera_profiles.extras[original].target = Some(unknown);
        world.camera_profiles.controller.enabled[2] = true;
        world.run_callback_fixture("local ok,why=pcall(ResetTravelCameraValues); assert(not ok and string.find(tostring(why),'runtime binding is absent'))")?;
        assert!(!world.camera_profiles.controller.enabled[2]);
        assert_eq!(
            world.camera_profiles.controller.selection.current,
            original as i32
        );
        assert_eq!(
            world.camera_target.resolved_target,
            Some(world.player_entity),
            "Failed resolution removed the old subscription"
        );
        // A missing/retired target, in contrast, legitimately clears the subscription.
        world.camera_profiles.extras[original].target = Some(usize::MAX);
        world.run_callback_fixture("ResetTravelCameraValues()")?;
        assert!(!world.camera_target.subscribed);
        assert_eq!(world.camera_target.resolved_target, None);
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_default_camera_values_update_profiles_and_preserve_unsupported_target_failure(
    ) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let custom = world
            .camera_profiles
            .controller
            .library
            .find("CustomDefault")
            .context("Missing custom profile")?;
        world
            .run_callback_fixture("assert(select('#',SetDefaultCameraValues('6.25',18,-10))==0)")?;
        let controller = &world.camera_profiles.controller;
        assert!(controller.enabled[0]);
        assert_eq!(controller.selection.current, custom as i32);
        assert_eq!(controller.selection.previous, 0);
        assert_eq!(controller.selection.duration, 0.5);
        let properties = controller.library.profiles[custom].properties;
        assert_eq!(controller.library.properties[properties[1]].value, 6.25);
        assert_eq!(controller.library.properties[properties[7]].value, 18.);
        assert_eq!(controller.library.properties[properties[6]].value, -10.);
        assert!(world.camera_target.subscribed);
        assert_eq!(
            world.camera_target.resolved_target,
            Some(world.player_entity)
        );
        assert!(world.entity_event_registry.registered_node_for(9, 1));
        world.run_callback_fixture("assert(select('#',SetTravelCameraValues(8,22,-12))==0); assert(select('#',SetCombatCameraValues(6,17,-8))==0)")?;
        let controller = &world.camera_profiles.controller;
        assert_eq!(controller.enabled, [true, true, true]);
        assert_eq!(
            controller.selection.current, custom as i32,
            "Other modes must not replace the default camera"
        );
        for (name, values) in [
            ("CustomTravel", [8., 22., -12.]),
            ("CustomCombat", [6., 17., -8.]),
        ] {
            let profile = controller
                .library
                .find(name)
                .context("Missing custom profile")?;
            for (lane, value) in [1, 7, 6].into_iter().zip(values) {
                assert_eq!(
                    controller.library.properties
                        [controller.library.profiles[profile].properties[lane]]
                        .value,
                    value
                );
            }
        }
        let unknown = world.entities.len() - 2;
        let component = world.entities[world.mickey_tracker.as_ref().unwrap().owner]
            .components
            .iter()
            .find(|component| component.class_name == "EntityTrackerComponent")
            .unwrap()
            .clone();
        world.entities[unknown].components.push(component);
        world.camera_profiles.extras[custom].target = Some(unknown);
        world.run_callback_fixture("local ok,why=pcall(SetDefaultCameraValues,7,20,-11); assert(not ok and string.find(tostring(why),'runtime binding is absent'))")?;
        assert_eq!(
            world.camera_profiles.controller.library.properties[properties[1]].value,
            7.
        );
        assert_eq!(
            world.camera_profiles.controller.selection.current,
            custom as i32
        );
        assert_eq!(
            world.camera_target.resolved_target,
            Some(world.player_entity),
            "Failed resolution removed the old subscription"
        );
        world.run_callback_fixture("assert(not pcall(SetDefaultCameraValues,1,2))")?;
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_set_map_updates_pause_metadata_without_loading_or_entity_admission() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        assert_eq!(world.map_info.map, "UNKNOWN");
        assert_eq!(world.map_info.map_type, "UNKNOWN");
        let scenes = world.report.scenes.clone();
        let entities = world.entities.len();
        let level = world.report.level.clone();
        world.run_callback_fixture("assert(select('#',SetMap(GetPlayer(),'GV_ZONEA','3D'))==0)")?;
        assert_eq!(world.map_info.map, "GV_ZONEA");
        assert_eq!(world.map_info.map_type, "3D");
        world.run_callback_fixture(
            "SetMap('missing-map-owner','Next\\0ignored','Type\\0ignored')",
        )?;
        assert_eq!(world.map_info.map, "Next");
        assert_eq!(world.map_info.map_type, "Type");
        world.run_callback_fixture("SetMap(nil,12,34)")?;
        assert_eq!(world.map_info.map, "12");
        assert_eq!(world.map_info.map_type, "34");
        world.run_callback_fixture("SetMap({},false,nil)")?;
        assert!(world.map_info.map.is_empty() && world.map_info.map_type.is_empty());
        world.run_callback_fixture(
            "SetMap(nil,'Final','2D'); assert(not pcall(SetMap,nil,'wrong-arity'))",
        )?;
        assert_eq!(world.map_info.map, "Final");
        assert_eq!(world.map_info.map_type, "2D");
        assert_eq!(world.report.scenes, scenes);
        assert_eq!(world.entities.len(), entities);
        assert_eq!(world.report.level, level);
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_projector_enable_disable_changes_native_flags_without_visibility_or_forced_spawn(
    ) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let name = "gremlinvillage_zonea.seq_projector 01.SpawnerPortalWarpFX";
        let owner = world
            .entities
            .iter()
            .position(|entity| entity.name == name)
            .context("Original spawner missing")?;
        let component = world.entities[owner]
            .components
            .iter()
            .position(|component| component.class_name == "TemplateSpawnerComponent")
            .unwrap();
        assert_eq!(
            world.spawners[&owner].disabled,
            Some(false),
            "Startup did not enable its spawner"
        );
        ensure!(
            !world
                .report
                .unsupported
                .iter()
                .any(|error| error.contains("SetupProjector")),
            "{:?}",
            world.report.unsupported
        );
        let hidden = world.hidden_entities.clone();
        let draws = world.draws.len();
        let random = world.game_random;
        world.run_callback_fixture(&format!(
            "assert(select('#',Disable(GetEntity('{name}')))==0)"
        ))?;
        assert_eq!(world.component_control.disabled[&owner][&component], true);
        assert_eq!(world.spawners[&owner].disabled, Some(true));
        world.run_callback_fixture(&format!("assert(select('#',Enable('{name}'))==0); assert(select('#',Enable(nil))==0); assert(select('#',Disable('BwxMissing'))==0)"))?;
        assert_eq!(world.spawners[&owner].disabled, Some(false));
        assert_eq!(world.hidden_entities, hidden);
        assert_eq!(world.draws.len(), draws);
        assert_eq!(world.game_random, random);
        // Deliberate runtime component fixture for the native Shooter guard.
        let mut shooter = world.entities[owner].components[component].clone();
        shooter.class_name = "ShooterComponent".into();
        shooter.properties.clear();
        world.entities[owner].components.push(shooter);
        world.set_entity_disabled(owner, true)?;
        world.set_entity_disabled(owner, false)?;
        assert_eq!(
            world.spawners[&owner].disabled,
            Some(true),
            "Shooter-owned spawner was improperly enabled"
        );
        assert_eq!(world.component_control.disabled[&owner][&component], true);
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_world_ledge_gate_is_per_motion_controller_and_does_not_claim_grabbing() -> Result<()>
    {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let player = world.player_entity;
        ensure!(
            world.ledge_controls.contains_key(&player),
            "Original player motion gate missing"
        );
        let before = world.ledge_controls[&player];
        world.with_script_world_host(World::script_world_api, |scripts| {
            scripts.ensure_global_module(
                "@ledge-disable",
                b"assert(select('#',SetLedgeGrabEnabled(GetPlayer(),false))==0)",
            )
        })?;
        assert_eq!(world.ledge_controls[&player].flags, before.flags & !2);
        world.with_script_world_host(World::script_world_api, |scripts| {
            scripts
                .ensure_global_module("@ledge-enable", b"SetLedgeGrabEnabled(GetPlayer(),'true')")
        })?;
        assert_eq!(world.ledge_controls[&player].flags, before.flags | 2);
        let unchanged = world.ledge_controls.clone();
        let other = world
            .entities
            .iter()
            .position(|entity| {
                !entity.components.iter().any(|component| {
                    component.class_name == "MickeyPlayerMotionControllerComponent"
                })
            })
            .context("Missing non-Mickey target")?;
        let name = serde_json::to_string(&world.entities[other].name)?;
        let source=format!("assert(select('#',SetLedgeGrabEnabled(nil,false))==0); SetLedgeGrabEnabled(GetEntity({name}),false); assert(not pcall(SetLedgeGrabEnabled,GetPlayer()))");
        world.with_script_world_host(World::script_world_api, |scripts| {
            scripts.ensure_global_module("@ledge-non-targets", source.as_bytes())
        })?;
        assert_eq!(
            world.ledge_controls, unchanged,
            "Absent or unrelated targets altered a motion controller"
        );
        assert!(world
            .report
            .unsupported
            .iter()
            .any(|message| message.contains("ledge prerequisite flags")
                && message.contains("remain unimplemented")));
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_world_silhouette_binding_uses_native_boolean_and_zero_results() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load_with_render_options(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
            None,
            None,
            Default::default(),
            super::super::render_options::State {
                draw_silhouettes: false,
            },
        )?;
        for (source, expected) in [
            ("assert(select('#',SetDrawSilhouettes(true))==0)", true),
            ("SetDrawSilhouettes(0)", false),
            ("SetDrawSilhouettes('true')", true),
            ("SetDrawSilhouettes('false')", false),
            ("SetDrawSilhouettes(-1)", true),
            ("SetDrawSilhouettes(nil)", false),
            ("SetDrawSilhouettes('0')", false),
        ] {
            world.with_script_world_host(World::script_world_api, |scripts| {
                scripts.ensure_global_module(&format!("@silhouette-{source}"), source.as_bytes())
            })?;
            assert_eq!(world.render_options.draw_silhouettes, expected);
        }
        world.with_script_world_host(World::script_world_api,|scripts|scripts.ensure_global_module("@silhouette-argument-errors",
            b"assert(not pcall(SetDrawSilhouettes)); assert(not pcall(SetDrawSilhouettes,true,false))"))?;
        assert!(
            !world.render_options.draw_silhouettes,
            "Malformed calls modified the state"
        );
        assert!(
            world
                .report
                .unsupported
                .iter()
                .any(|message| message.contains("Silhouette render gate")
                    && message.contains("remain unimplemented")),
            "Missing silhouette shader must not disappear behind a working flag binding"
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_global_ai_pause_returns_before_deferred_delivery_without_hiding_pending_events(
    ) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let order = world.ai_registry.order()?;
        ensure!(!order.is_empty(), "No original AI registrations");
        let before = world.entity_event_queue.entries().len();
        world.with_script_world_host(World::script_world_api, |scripts| {
            scripts.ensure_global_module(
                "@test/deferred-ai-pause",
                br#"
            assert(select('#', PauseAllAI('ignored', GetPlayer())) == 0)
            AfterPauseReturned = true
        "#,
            )
        })?;
        assert_eq!(world.ai_manager_enabled, Some(false));
        let entries = &world.entity_event_queue.entries()[before..];
        assert_eq!(entries.len(), order.len());
        for (entry, owner) in entries.iter().zip(&order) {
            let action = entry
                .event
                .action()
                .context("AI queued a non-action event")?;
            assert_eq!(action.source, Some(*owner));
            assert_eq!(action.action, 13);
            assert_eq!(action.header, Some(world.player_entity + 1));
            assert_eq!(entry.listener, None);
        }
        assert!(
            world
                .ai_brains
                .values()
                .all(|brain| brain.paused.values().all(|state| !state.active)),
            "Enqueueing must not execute receivers synchronously"
        );
        world.with_script_world_host(World::script_world_api, |scripts| {
            scripts.ensure_global_module(
                "@test/deferred-ai-resume",
                br#"
            assert(AfterPauseReturned == true)
            assert(select('#', UnpauseAllAI(GetPlayer(), true)) == 0)
            AfterResumeReturned = true
        "#,
            )
        })?;
        assert_eq!(world.ai_manager_enabled, Some(true));
        assert_eq!(
            world.entity_event_queue.entries().len(),
            before + 2 * order.len()
        );
        assert!(world.entity_event_queue.entries()[before + order.len()..]
            .iter()
            .all(|entry| entry
                .event
                .action()
                .is_some_and(|action| action.action == 14 && action.header.is_none())));
        assert_eq!(
            world
                .report
                .unsupported
                .iter()
                .filter(|message| message.starts_with("Deferred AI action delivery is pending:"))
                .count(),
            1
        );
        assert!(
            world.entity_event_queue.pending() >= 2 * order.len(),
            "Required deferred events were discarded"
        );
        ensure!(
            !world
                .scripts
                .errors
                .iter()
                .any(|error| error.contains("@test/deferred-ai")),
            "{:?}",
            world.scripts.errors
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_gremlin_scripted_idle_suspension_requests_native_reevaluation() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let owner = world
            .ai_brains
            .iter()
            .find(|(_, brain)| {
                brain.ai_type.starts_with("Gremlin")
                    && !brain.scripted_idle.is_empty()
                    && !brain.paused.is_empty()
            })
            .map(|(&owner, _)| owner)
            .context("Expected original Gremlin core behavior slots")?;
        let event = |action| super::super::ai_behavior_events::Event {
            kind: 0x0c,
            source: Some(owner),
            action,
        };
        assert!(world.receive_known_ai_behavior_event(owner, event(11))?);
        let brain = &world.ai_brains[&owner];
        assert!(brain.motion_suspended && brain.reevaluation_requested);
        for idle in brain.scripted_idle.values() {
            assert!(idle.eligible(brain.motion_suspended));
            assert_eq!(idle.update_result(brain.motion_suspended), 1);
            assert_eq!(idle.entry_animation_event(true), Some(194));
        }
        assert!(world.receive_known_ai_behavior_event(owner, event(13))?);
        // Both receivers can be active. Never force Paused over the scripted
        // receiver merely because a pause request was received.
        let brain = &world.ai_brains[&owner];
        assert!(brain
            .scripted_idle
            .values()
            .all(|idle| idle.priority() > brain.paused.values().next().unwrap().priority()));
        assert_eq!(
            brain.selector.current, None,
            "Partial receiver execution must not pretend full selection ran"
        );
        world
            .ai_brains
            .get_mut(&owner)
            .unwrap()
            .reevaluation_requested = false;
        assert!(!world.receive_known_ai_behavior_event(owner, event(12))?);
        assert!(!world.ai_brains[&owner].motion_suspended);
        assert!(world.receive_known_ai_behavior_event(owner, event(14))?);
        assert!(world.ai_brains[&owner]
            .paused
            .values()
            .all(|paused| !paused.active));
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_world_io_group_dispatch_continues_after_receiver_failure() -> Result<()> {
        use super::*;
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let names = [
            world.entities[0].name.clone(),
            world.entities[1].name.clone(),
        ];
        world.scripts = super::super::actor_scripts::Scripts::with_aliases(
            &[(0, names[0].clone()), (1, names[1].clone())],
            &[(0, "TestReceivers".into()), (1, "TestReceivers".into())],
        )?;
        let chunk=crate::lua_cook::compile(
            "GroupCalls=0; function GroupProbe(entity) GroupCalls=GroupCalls+1; if GroupCalls==1 then error('first receiver failed') end end")?;
        world
            .scripts
            .ensure_global_module("io-group-regression.lua", &chunk)?;
        let connection = super::super::actor_io::Connection {instigator:None,target_guid:None,
            owner: 0,
            input: "GroupProbe".into(),
            output: "OnUser1".into(),
            target: "_target_group".into(),
            parameters: String::new(),
            delay: 0.,
            flags: 0,
            group: "TestReceivers".into(),
        };
        let before = world.report.unsupported.len();
        world.dispatch_actor_deliveries(vec![super::super::actor_io::Delivery {
            connection: connection.clone(),
            activator: Some(1),
            payload: None,
        }]);
        world.scripts.ensure_global_module(
            "io-group-count.lua",
            &crate::lua_cook::compile("assert(GroupCalls==2)")?,
        )?;
        ensure!(
            world.report.unsupported[before..]
                .iter()
                .chain(world.scripts.errors.iter())
                .any(|error| error.contains("first receiver failed")),
            "Failed receiver not reported: {:?}, {:?}",
            &world.report.unsupported[before..],
            world.scripts.errors
        );
        // Ordinary selectors ignore the populated TargetGroup field.
        let mut ordinary = connection;
        ordinary.target = "_activator".into();
        world.dispatch_actor_deliveries(vec![super::super::actor_io::Delivery {
            connection: ordinary,
            activator: Some(1),
            payload: None,
        }]);
        world.scripts.ensure_global_module(
            "io-group-final-assert.lua",
            &crate::lua_cook::compile("assert(GroupCalls==3)")?,
        )?;
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_projector_user_output_reaches_its_authored_connections() -> Result<()> {
        use super::*;
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let owner = world
            .scripts
            .resolve_name("GV_ZoneA_ProjectorStart")
            .context("Projector group missing")?;
        let marker = world
            .relative_prefab_entity(owner, ".ExitProjectorMarker")?
            .context("Exit marker missing")?;
        for suffix in [".ExitProjectorStand", ".SpawnerPortalWarpFX"] {
            world
                .relative_prefab_entity(owner, suffix)?
                .with_context(|| format!("Projector member {suffix} missing"))?;
        }
        // Relative groups remain local to this prefab even when display names
        // change. A placement in another prefab cannot satisfy this query.
        let original_name = world.entities[marker].name.clone();
        world.entities[marker].name = "renamed-projector-marker".into();
        assert_eq!(
            world.relative_prefab_entity(owner, ".ExitProjectorMarker")?,
            Some(marker)
        );
        assert_eq!(
            world.relative_prefab_entity(marker, ".ExitProjectorMarker")?,
            Some(marker)
        );
        assert_eq!(
            world.relative_prefab_entity(owner, ".exitprojectormarker")?,
            None
        );
        world.entities[marker].name = original_name;
        let io = world.entities[marker]
            .components
            .iter()
            .find(|component| component.class_name == "InputOutputComponent")
            .context("Projector exit marker IO missing")?;
        let values = |name: &str| {
            io.properties
                .iter()
                .find(|property| property.name == name)
                .and_then(|property| property.value.as_array())
                .context("Missing projector IO array")
        };
        let row = values("Output")?
            .iter()
            .position(|value| value.as_str() == Some("OnUser1"))
            .context("Inherited projector exit event was erased by empty local arrays")?;
        assert_eq!(
            values("Input")?[row].as_str(),
            Some("Mickey_Projector_Exit")
        );
        world.emit_entity_output(marker, "OnUser1", marker);
        let deliveries = world.outputs.ready_for_owner(world.animation_time, marker);
        assert!(deliveries.iter().any(|delivery| delivery.connection.input
            == "Mickey_Projector_Exit"
            && delivery.connection.output == "OnUser1"
            && delivery.activator == Some(marker)));
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_projector_usable_radius_writes_the_live_reflected_scalar() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let owner = world
            .scripts
            .resolve_name("GV_ZoneA_ProjectorStart")
            .context("Projector missing")?;
        let stand = world
            .relative_prefab_entity(owner, ".ExitProjectorStand")?
            .context("Stand missing")?;
        let name = serde_json::to_string(&world.entities[stand].name)?;
        let code=format!("local e=GetEntity({name}); assert(select('#',SetPropertyFloat(e,'UsableRadius',3.5))==0); assert(GetPropertyFloat(e,'UsableRadius')==3.5); SetPropertyFloat(e,'UsableRadius',0,2); assert(GetPropertyFloat(e,'UsableRadius')==3.5); assert(select('#',GetPropertyFloat(e,'UsableRadius',2))==0); assert(select('#',SetPropertyFloat(0,'UsableRadius',0))==0); SetPropertyFloat(e,'UsableRadius',0); assert(GetPropertyFloat(e,'UsableRadius')==0)");
        world.with_script_world_host(World::script_world_api, |scripts| {
            scripts.ensure_global_module("@projector-live-radius", code.as_bytes())
        })?;
        let component = world.entities[stand]
            .components
            .iter_mut()
            .find(|c| c.class_name == "UsableComponent")
            .context("Stand is not usable")?;
        assert_eq!(
            component
                .properties
                .iter()
                .find(|p| p.name == "UsableRadius")
                .unwrap()
                .value,
            serde_json::json!(0.)
        );
        component.properties.retain(|p| p.name != "UsableRadius");
        // Float descriptor constructor13b0bc initializes the omitted field to
        // zero. The catalog's example value2 is not a native default.
        let code=format!("local e=GetEntity({name}); assert(GetPropertyFloat(e,'UsableRadius')==0); SetPropertyFloat(e,'UsableRadius',3.5); assert(GetPropertyFloat(e,'UsableRadius')==3.5)");
        world.with_script_world_host(World::script_world_api, |scripts| {
            scripts.ensure_global_module("@projector-omitted-radius", code.as_bytes())
        })?;
        assert_eq!(
            world.entities[stand]
                .components
                .iter()
                .find(|c| c.class_name == "UsableComponent")
                .unwrap()
                .properties
                .iter()
                .find(|p| p.name == "UsableRadius")
                .unwrap()
                .value,
            serde_json::json!(3.5)
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_counter_status_publication_shares_the_world_event_queue() -> Result<()> {
        use super::super::{action_event_queue::ActionEvent, entity_events::Event};
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let owner = *world
            .paint_counters
            .keys()
            .next()
            .context("Original counter missing")?;
        let previous = world.paint_counters[&owner].published_status;
        let current = if previous == 1 { 2 } else { 1 };
        let before = world.entity_event_queue.entries().len();
        let action = |id| {
            Event::Action(ActionEvent {
                source: Some(owner),
                header: None,
                action: id,
                vector_bits: [0; 4],
                reference: None,
            })
        };
        world.entity_event_queue.enqueue(&action(11), None)?;
        assert!(world.enqueue_counter_status(
            owner,
            current,
            Some(world.player_entity + 1),
            0,
            None,
            &mut |_, _| anyhow::bail!("Null references must not compare")
        )?);
        world.entity_event_queue.enqueue(&action(12), None)?;
        let entries = &world.entity_event_queue.entries()[before..];
        assert_eq!(entries[0].event.action().unwrap().action, 11);
        let Event::CounterStatus(status) = &entries[1].event else {
            anyhow::bail!("Status lost its native kind")
        };
        assert_eq!(
            (status.previous, status.current),
            (previous as i32, current as i32)
        );
        assert_eq!(status.source, Some(owner));
        assert_eq!(status.header, Some(world.player_entity + 1));
        assert_eq!(entries[2].event.action().unwrap().action, 12);
        assert_eq!(world.paint_counters[&owner].published_status, current);
        assert!(
            !world.enqueue_counter_status(owner, current, None, 0, None, &mut |_, _| Ok(false))?
        );
        assert_eq!(world.entity_event_queue.entries().len(), before + 3);
        // This only tests the real publication stage. It does not inject a
        // brush hit or pretend the missing material/receiver stages executed.
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_render_instance_color_publication_preserves_asset_flags_and_other_instances(
    ) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let (&(owner, node), _) = world
            .render_objects
            .iter()
            .find(|((owner, node), _)| {
                !world
                    .transform_hierarchy
                    .values()
                    .any(|local| local.parent == *owner && local.socket.is_some())
                    && world.entities[*owner]
                        .mesh_draws
                        .get(node)
                        .is_some_and(|indices| !indices.is_empty())
                    && world.entities[*owner]
                        .mesh_draws
                        .values()
                        .flatten()
                        .all(|&index| {
                            world.draws[index].preview.as_ref().is_some_and(|program| {
                                program
                                    .as_ref()
                                    .is_ok_and(|program| program.register_defaults.is_some())
                            })
                        })
            })
            .context("Original render object with uniform material missing")?;
        let asset = world.entities[owner]
            .model_asset
            .clone()
            .context("Render model missing")?;
        let before = world.model_bounds[&asset].nodes[&node].flags;
        let other_colors = world
            .render_objects
            .iter()
            .filter(|((other, _), _)| *other != owner)
            .map(|(key, state)| (*key, state.constant_colors))
            .collect::<BTreeMap<_, _>>();
        let update = super::super::paint_counter::MaterialUpdate {
            filtered: 0.08,
            constant_color: 1,
            rgba: [55, 139, 213, 234],
        };
        // Validate the targeted object primitive independently from incomplete
        // counter hit/child attachment callbacks, using the real world instance.
        assert!(world.publish_model_constant_color(owner, update)? > 0);
        let mut draws = world.draws.clone();
        world.apply_model_constant_colors(&mut draws)?;
        for &index in &world.entities[owner].mesh_draws[&node] {
            assert_eq!(
                draws[index].material_registers.unwrap()[4],
                update.rgba.map(|v| f32::from(v) / 255.)
            );
        }
        assert_eq!(world.model_bounds[&asset].nodes[&node].flags, before);
        assert!(other_colors
            .iter()
            .all(|(key, colors)| world.render_objects[key].constant_colors == *colors));
        let state = world.render_objects.get_mut(&(owner, node)).unwrap();
        state.flags.set_ignore_color_override(true);
        assert!(!state.publish(1, [1, 2, 3, 4])?);
        assert_eq!(state.constant_colors[1], Some(update.rgba));
        world.retire_entity(owner)?;
        assert!(!world
            .render_objects
            .keys()
            .any(|(entity, _)| *entity == owner));
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_counter_materials_initialize_and_update_from_live_values() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        assert!(!world.counter_materials.is_empty());
        assert!(
            world
                .counter_materials
                .values()
                .all(|state| state.initialized && state.error.is_none()),
            "Counter publication must not depend on transform children or shader readiness: {:?}",
            world.counter_materials
        );
        let owner = *world
            .counter_materials
            .iter()
            .find(|(owner, state)| {
                state.initialized
                    && state.publications > 0
                    && world.entities[**owner]
                        .mesh_draws
                        .values()
                        .flatten()
                        .next()
                        .is_some()
                    && world.entities[**owner]
                        .mesh_draws
                        .values()
                        .flatten()
                        .all(|&index| {
                            world.draws[index].preview.as_ref().is_some_and(|program| {
                                program
                                    .as_ref()
                                    .is_ok_and(|program| program.register_defaults.is_some())
                            })
                        })
            })
            .with_context(|| {
                format!(
                    "No admitted original counter model: {:?}",
                    world
                        .counter_materials
                        .iter()
                        .map(|(owner, state)| (&world.entities[*owner].name, state))
                        .collect::<Vec<_>>()
                )
            })?
            .0;
        let before = world.counter_materials[&owner].filter.filtered;
        let count = world.counter_materials[&owner].publications;
        // Controlled live-state change validates the real update bridge. This
        // is not a brush-hit fixture or a complete combat callback sequence.
        let counter = world.paint_counters.get_mut(&owner).unwrap();
        counter.value = counter.thinner_threshold;
        world.advance_counter_materials(owner..owner + 1);
        let material = &world.counter_materials[&owner];
        assert!(material.error.is_none());
        assert!(material.filter.filtered < before);
        assert_eq!(material.publications, count + 1);
        let mut draws = world.draws.clone();
        world.apply_model_constant_colors(&mut draws)?;
        assert!(world.entities[owner]
            .mesh_draws
            .iter()
            .all(|(&node, indices)| indices.iter().all(|&index| draws[index]
                .material_registers
                .is_some_and(|registers| registers[4]
                    == world.render_objects[&(owner, node)].constant_colors[1]
                        .unwrap()
                        .map(|v| f32::from(v) / 255.)))));
        assert!(
            world.paint_counters[&owner].value == world.paint_counters[&owner].thinner_threshold
        );
        world.retire_entity(owner)?;
        assert!(!world.counter_materials.contains_key(&owner));
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_counter_colors_follow_nif_children_not_transform_children() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let (&child, local) = world
            .transform_hierarchy
            .iter()
            .find(|(_, local)| {
                local.socket.is_some()
                    && world.paint_counters.contains_key(&local.parent)
                    && world.entities[local.parent].model_asset.is_some()
            })
            .context("Original counter with bone-parented child missing")?;
        let parent = local.parent;
        let other_colors = world
            .render_objects
            .iter()
            .filter(|((owner, _), _)| *owner != parent)
            .map(|(key, state)| (*key, state.constant_colors))
            .collect::<BTreeMap<_, _>>();
        let update = super::super::paint_counter::MaterialUpdate {
            filtered: 0.08,
            constant_color: 1,
            rgba: [55, 139, 213, 234],
        };
        assert!(world.publish_model_constant_color(parent, update)? > 0);
        assert!(world
            .render_objects
            .iter()
            .filter(|((owner, _), _)| *owner == parent)
            .all(|(_, state)| state.flags.ignore_color_override()
                || state.constant_colors[1] == Some(update.rgba)));
        assert!(other_colors
            .iter()
            .all(|(key, colors)| world.render_objects[key].constant_colors == *colors));
        assert_eq!(world.transform_hierarchy[&child].parent, parent);
        // Publication remains valid when the current material is unavailable;
        // applying a frame preserves the renderer's diagnostic verbatim.
        let index = world.entities[parent]
            .mesh_draws
            .values()
            .flatten()
            .copied()
            .next()
            .context("Counter model draw missing")?;
        let mut draw = world.draws[index].clone();
        draw.preview = Some(Err("fixture: pending lighting map".into()));
        let node = *world.entities[parent]
            .mesh_draws
            .iter()
            .find(|(_, indices)| indices.contains(&index))
            .unwrap()
            .0;
        world.render_objects[&(parent, node)].apply(&mut draw)?;
        assert!(
            matches!(draw.preview,Some(Err(ref error)) if error=="fixture: pending lighting map")
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_counter_scalar_setter_is_synchronous_and_does_not_emit_hit_events() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let owner = *world
            .paint_counters
            .iter()
            .find(|(owner, counter)| {
                counter.thinner_threshold < 0.
                    && world.entities[**owner]
                        .mesh_draws
                        .values()
                        .flatten()
                        .next()
                        .is_some()
                    && world.entities[**owner]
                        .mesh_draws
                        .values()
                        .flatten()
                        .all(|&draw| {
                            world.draws[draw].preview.as_ref().is_some_and(|program| {
                                program
                                    .as_ref()
                                    .is_ok_and(|program| program.register_defaults.is_some())
                            })
                        })
            })
            .context("Original counter with drawable material missing")?
            .0;
        let counter = world.paint_counters[&owner].clone();
        let snapshot = |components: &[epic_mickey_lib_rs::scene_file::Component]| {
            components
                .iter()
                .map(|component| {
                    (
                        component.class_name.clone(),
                        component
                            .properties
                            .iter()
                            .map(|property| {
                                (
                                    property.name.clone(),
                                    property.class_name.clone(),
                                    property.value.clone(),
                                )
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let authored = snapshot(&world.entities[owner].components);
        let queued = world.entity_event_queue.entries().len();
        let publications = world.counter_materials[&owner].publications;
        let name = serde_json::to_string(&world.entities[owner].name)?;
        let code=format!("local e=GetEntity({name}); assert(select('#',SetPropertyFloat(e,'CounterStart','{}'))==0); assert(GetPropertyFloat(e,'CounterStart')=={})",
            counter.thinner_threshold,counter.thinner_threshold);
        world.with_script_world_host(World::script_world_api, |scripts| {
            scripts.ensure_global_module("@counter-scalar-first", code.as_bytes())
        })?;
        assert_eq!(
            world.paint_counters[&owner].value,
            counter.thinner_threshold
        );
        assert_eq!(
            world.counter_materials[&owner].publications,
            publications + 1
        );
        let filtered = world.counter_materials[&owner].filter.filtered;
        let doubled = counter.thinner_threshold * 2.;
        let code=format!("local e=GetEntity({name}); SetPropertyFloat(e,'CounterStart',{doubled}); assert(GetPropertyFloat(e,'CounterStart')=={doubled})");
        world.with_script_world_host(World::script_world_api, |scripts| {
            scripts.ensure_global_module("@counter-scalar-second", code.as_bytes())
        })?;
        assert_eq!(
            world.paint_counters[&owner].value, doubled,
            "Setter does not clamp current scalar"
        );
        assert_eq!(
            world.counter_materials[&owner].publications,
            publications + 1,
            "Equal normalized fraction skips material update"
        );
        assert_eq!(world.counter_materials[&owner].filter.filtered, filtered);
        assert_eq!(world.paint_counters[&owner].anchor, counter.anchor);
        assert_eq!(
            world.paint_counters[&owner].published_status,
            counter.published_status
        );
        assert_eq!(
            world.entity_event_queue.entries().len(),
            queued,
            "Scalar setter is not a hit"
        );
        assert_eq!(snapshot(&world.entities[owner].components), authored);
        let mut draws = world.draws.clone();
        world.apply_model_constant_colors(&mut draws)?;
        assert!(world.entities[owner]
            .mesh_draws
            .iter()
            .all(|(&node, indices)| indices.iter().all(|&draw| draws[draw]
                .material_registers
                .unwrap()[4]
                == world.render_objects[&(owner, node)].constant_colors[1]
                    .unwrap()
                    .map(|v| f32::from(v) / 255.))));
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_counter_lifecycle_restores_regular_color_without_overwriting_rim() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let owner = *world
            .paint_counters
            .iter()
            .find(|(owner, counter)| {
                counter.thinner_threshold < 0.
                    && world.entities[**owner]
                        .mesh_draws
                        .values()
                        .flatten()
                        .next()
                        .is_some()
                    && world.entities[**owner]
                        .mesh_draws
                        .values()
                        .flatten()
                        .all(|&draw| {
                            world.draws[draw].preview.as_ref().is_some_and(|program| {
                                program
                                    .as_ref()
                                    .is_ok_and(|program| program.register_defaults.is_some())
                            })
                        })
            })
            .context("Original counter with drawable material missing")?
            .0;
        world.publish_model_color(owner, 1, [64, 32, 0, 255], false)?;
        world.initialize_counter_transients(owner..owner + 1)?;
        assert_eq!(
            world.counter_materials[&owner].transient.baseline,
            [64u8, 32, 0, 255].map(|v| f32::from(v) / 255.)
        );
        let thinner = world.paint_counters[&owner].thinner_threshold;
        world.set_entity_float_property(owner, "CounterStart", thinner)?;
        let rim = world
            .render_objects
            .iter()
            .filter(|((entity, _), _)| *entity == owner)
            .map(|(key, state)| (*key, state.constant_colors))
            .collect::<BTreeMap<_, _>>();
        let runtime = world.counter_materials.get_mut(&owner).unwrap();
        // Controlled pulse validates the actual update/presentation bridge;
        // no combat event or native impact callback is synthesized here.
        runtime.invulnerable_remaining = 0.025;
        let counter_before = serde_json::to_value(&world.paint_counters[&owner])?;
        let queue_before = world.entity_event_queue.pending();
        assert!(world.activate_counter_impact(owner, -1.)?);
        assert_eq!(
            world.counter_materials[&owner].transient.target,
            world.report.counter_impact_colors.impact_rgba(-1.)?
        );
        assert_eq!(
            serde_json::to_value(&world.paint_counters[&owner])?,
            counter_before
        );
        assert_eq!(
            world.entity_event_queue.pending(),
            queue_before,
            "Feedback must not synthesize status events"
        );
        assert!(!world
            .counter_materials
            .get_mut(&owner)
            .unwrap()
            .transient
            .activate(0.5, [0., 1., 0., 1.])?);
        world.advance_counter_lifecycle(0.025)?;
        assert_eq!(world.counter_materials[&owner].invulnerable_remaining, 0.);
        assert!(!world.counter_materials[&owner].status_not_thinned);
        let mut draws = world.draws.clone();
        world.apply_model_constant_colors(&mut draws)?;
        assert!(world.entities[owner]
            .mesh_draws
            .iter()
            .all(|(&node, indices)| indices.iter().all(|&draw| draws[draw]
                .material_registers
                .unwrap()[1]
                == world.render_objects[&(owner, node)].regular_colors[1]
                    .unwrap()
                    .map(|v| f32::from(v) / 255.))));
        assert!(rim
            .iter()
            .all(|(key, colors)| world.render_objects[key].constant_colors == *colors));
        for _ in 0..60 {
            world.advance_counter_lifecycle(1. / 30.)?;
        }
        assert_eq!(
            world.counter_materials[&owner].transient.target,
            world.counter_materials[&owner].transient.baseline
        );
        world
            .counter_materials
            .get_mut(&owner)
            .unwrap()
            .invulnerable_remaining = -1.;
        world.advance_counter_lifecycle(0.5)?;
        assert_eq!(world.counter_materials[&owner].invulnerable_remaining, -1.);
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn sparse_static_owner_metadata_does_not_alias_report_indices() -> anyhow::Result<()> {
        use super::*;
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/DBC_Start_1st.Level",
            &egui::Context::default(),
        )?;
        let index = world
            .report
            .collision_parts
            .iter()
            .position(|part| {
                part.triangle_count > 0 && part.native_welding.contains_key(&part.body_offset)
            })
            .context("Expected native static contact source")?;
        let old = world.report.collision_parts[index].owner;
        let expected_entity = world
            .collision_entity(old)
            .context("Expected static owner entity")?
            .id;
        let expected_filter = world.initial_static_simulation_filter(old)?;
        let triangle = world.report.collision_parts[index].triangle_start;
        let expected_transform = world.initial_static_contact(old, triangle)?.1;
        let incoming_owner = world.collision.next_owner()?;
        // Exercise metadata resolution with an incoming sparse identity. This
        // does not pretend to admit a new body or remap an installed shape.
        assert!(world.imported_static_part(incoming_owner).is_none());
        world.static_collision_owner_parts.remove(&old);
        world
            .static_collision_owner_parts
            .insert(incoming_owner, index);
        world.report.collision_parts[index].owner = incoming_owner;
        assert!(world.imported_static_part(old).is_none());
        assert!(world.initial_static_contact(old, triangle).is_err());
        assert_eq!(
            world.collision_entity(incoming_owner).unwrap().id,
            expected_entity
        );
        assert_eq!(
            world.initial_static_simulation_filter(incoming_owner)?,
            expected_filter
        );
        assert_eq!(
            world.initial_static_contact(incoming_owner, triangle)?.1,
            expected_transform
        );
        // A stale map entry must not resolve a different record accidentally.
        world.report.collision_parts[index].owner = old;
        assert!(world.imported_static_part(incoming_owner).is_none());
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_selected_system_constructor_runs_ordered_stages() -> anyhow::Result<()> {
        use anyhow::{ensure, Context};
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::load(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        ensure!(
            !world.dynamic_query_owners.is_empty(),
            "World load attached no diagnostic body queries"
        );
        for owner in 0..world.dynamic_query_owners.len() {
            let (entity, body, layout) = world
                .dynamic_query_body(owner)
                .context("Diagnostic query owner not resolved")?;
            ensure!(
                !entity.name.is_empty() && !layout.parts.is_empty(),
                "Incomplete dynamic owner binding"
            );
            for &shape in &body.shapes {
                let source = world.dynamic_query_owners[owner].0;
                let identity =
                    if let Some(&runtime_owner) = world.dynamic_runtime_sources.get(&source) {
                        world.collision.shape_owner(shape) == Some(runtime_owner)
                    } else {
                        world.dynamic_queries.as_ref().unwrap().shape_owner(shape) == Some(owner)
                    };
                ensure!(identity, "Dynamic owner aliases another body");
            }
        }
        let mut count = 0;
        for source in 0..world.dynamic_bodies.len() {
            let entity = world.dynamic_bodies[source].entity;
            let original = world.dynamic_bodies[source].geometry.as_ref().ok().cloned();
            let Some(mut state) = super::super::dynamic_body::Construction::new(
                &world.dynamic_bodies[source],
                &world.entities[entity].initial_physics_transform(),
            )?
            else {
                continue;
            };
            // Explicit finite RNG fixture; not the original game's shared
            // bootstrap state or a retail-reference simulation scenario.
            world.initialize_dynamic_system_records(
                source,
                &mut state,
                Some(entity + 1),
                || 1234,
            )?;
            ensure!(
                state.started && state.completed,
                "Construction stages did not complete"
            );
            assert!(world
                .initialize_dynamic_system_records(source, &mut state, Some(entity + 1), || 1234)
                .is_err());
            for (body, layout) in state.bodies.iter().zip(&state.layouts) {
                ensure!(
                    body.state().transform == layout.initial_transform,
                    "Body received duplicate/wrong placement"
                );
                let data = body
                    .auxiliary
                    .user_data
                    .context("Userdata missing after construction")?;
                ensure!(
                    data.dynamic_binding.is_some() && data.activation_cache.is_some(),
                    "Userdata stage incomplete"
                );
                ensure!(
                    body.shapes.is_empty(),
                    "Construction prematurely installed a query body"
                );
            }
            let parts = world.dynamic_bodies[source]
                .geometry
                .as_ref()
                .unwrap()
                .clone();
            let mut query_world =
                super::physics::Collision::with_triangle_owners(vec![], vec![], vec![])?;
            for (body, layout) in state.bodies.iter_mut().zip(&state.layouts) {
                let record = body.motion.current.clone();
                let data = body.auxiliary.user_data;
                let welding = body.welding.as_ref().map(|welding| welding as *const _);
                layout.attach_constructed_queries(body, &parts, &mut query_world, entity)?;
                ensure!(!body.shapes.is_empty(), "No constructed query shapes");
                assert_eq!(
                    body.motion.current, record,
                    "Query attachment recreated motion"
                );
                assert_eq!(
                    body.auxiliary.user_data, data,
                    "Query attachment reset userdata"
                );
                assert_eq!(
                    body.welding.as_ref().map(|welding| welding as *const _),
                    welding,
                    "Welding was regenerated"
                );
                assert!(layout
                    .attach_constructed_queries(body, &parts, &mut query_world, entity)
                    .is_err());
            }
            if let Some(original) = original {
                for part in original.iter() {
                    assert_eq!(
                        part.motion.current.motion_type(),
                        part.motion_type,
                        "Original source motion changed"
                    );
                }
            }
            count += state.bodies.len();
        }
        ensure!(count > 0, "No original selected bodies constructed");
        println!("Constructed {count} original bodies; {} single-body Box3D admissions; native parity pending",world.dynamic_runtime_sources.len());
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_body_activation_uses_owned_motion_cache_and_scene_mover() -> anyhow::Result<()> {
        use super::super::{collidable_owner::EntityData, dynamic_body::PlacedBody};
        use anyhow::{ensure, Context};
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::load(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let (source, parts, index) = world
            .dynamic_bodies
            .iter()
            .enumerate()
            .find_map(|(source, body)| {
                body.geometry.as_ref().ok().and_then(|parts| {
                    if parts
                        .iter()
                        .map(|part| part.body_offset)
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        != 1
                    {
                        return None;
                    }
                    parts
                        .iter()
                        .position(|part| part.motion.current.motion_type() == 3)
                        .map(|index| (source, parts.clone(), index))
                })
            })
            .context("No original box-inertia body")?;
        let part = &parts[index];
        let entity = world.dynamic_bodies[source].entity;
        let component = world.dynamic_bodies[source].component;
        let mover = |world: &super::World| {
            world.entities[entity]
                .components
                .iter()
                .find(|c| c.link_id.to_u32() == component)
                .unwrap()
                .properties
                .iter()
                .find(|p| p.name == "Is Mover")
                .unwrap()
                .value
                .as_bool()
                .unwrap()
        };
        let original = part.motion.current.clone();
        let mut motion = part.motion.instantiate()?;
        let quality = motion.quality;
        // Explicit kinematic fixture checks bit-preserving cache lanes; not
        // recorded gameplay input or a retail-reference acceptance scenario.
        let linear = [1., -0., 3., 7.];
        let angular = [4., 5., -0., -8.];
        motion.set_linear_velocity(linear)?;
        motion.set_angular_velocity(angular)?;
        let auxiliary = part.instantiate_auxiliary()?;
        let mut bodies = [PlacedBody::new(motion, vec![], auxiliary)?];
        world.initialize_dynamic_body_userdata(
            source,
            &mut bodies[0],
            Some(entity + 1),
            original.swept_state().transform,
        )?;
        assert!(bodies[0]
            .initialize_dynamic_user_data(Some(entity + 1))
            .is_err());
        assert_eq!(
            bodies[0]
                .cached_activation_velocities()?
                .0
                .map(f32::to_bits),
            [0; 4]
        );
        assert_eq!(
            bodies[0]
                .cached_activation_velocities()?
                .1
                .map(f32::to_bits),
            [0; 4]
        );
        let mut expected = EntityData::dynamic(Some(entity + 1));
        expected.dynamic_binding = bodies[0].auxiliary.user_data.unwrap().dynamic_binding;
        assert_eq!(bodies[0].auxiliary.user_data.unwrap(), expected);
        let position = [
            world.entities[entity].transform[12],
            world.entities[entity].transform[13],
            world.entities[entity].transform[14],
        ];
        let preplacement = original.swept_state().transform;
        assert!(bodies[0]
            .bind_dynamic_user_data(position, preplacement)
            .is_err());
        let binding = bodies[0]
            .auxiliary
            .user_data
            .unwrap()
            .dynamic_binding
            .unwrap();
        assert_eq!(
            binding.point,
            [
                position[0].to_bits(),
                position[1].to_bits(),
                position[2].to_bits(),
                0
            ]
        );
        for index in 0..16 {
            assert_eq!(
                binding.local_transform[index],
                if [3, 7, 11].contains(&index) {
                    0
                } else {
                    preplacement[index].to_bits()
                }
            );
        }
        world.activate_dynamic_before_admission(
            source,
            &mut bodies,
            false,
            false,
            true,
            0x3f800000,
        )?;
        ensure!(
            bodies[0].motion.current.motion_type() == 4 && bodies[0].motion.has_stored_dynamic(),
            "Keyframe conversion missing"
        );
        ensure!(
            bodies[0].motion.quality == 9,
            "Native quality update missing"
        );
        assert_eq!(bodies[0].linear_velocity(), [0.; 4]);
        assert_eq!(
            bodies[0]
                .cached_activation_velocities()?
                .0
                .map(f32::to_bits),
            linear.map(f32::to_bits)
        );
        assert_eq!(
            bodies[0]
                .cached_activation_velocities()?
                .1
                .map(f32::to_bits),
            angular.map(f32::to_bits)
        );
        assert!(mover(&world));
        world.activate_dynamic_before_admission(
            source,
            &mut bodies,
            true,
            false,
            true,
            0x3f800000,
        )?;
        ensure!(
            bodies[0].motion.current.motion_type() == 3 && !bodies[0].motion.has_stored_dynamic(),
            "Original inertia model not restored"
        );
        assert_eq!(bodies[0].motion.quality, quality);
        assert_eq!(
            bodies[0].linear_velocity().map(f32::to_bits),
            linear.map(f32::to_bits)
        );
        assert_eq!(
            bodies[0].angular_velocity().map(f32::to_bits),
            angular.map(f32::to_bits)
        );
        assert!(!mover(&world));
        assert_eq!(bodies[0].auxiliary.user_data.unwrap().byte_08, 1);
        assert_eq!(part.motion.current, original, "Original asset mutated");
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_dynamic_scene_properties_initialize_owned_motion() -> anyhow::Result<()> {
        use anyhow::{ensure, Context};
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::load(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let mut initialized = 0;
        for source in 0..world.dynamic_bodies.len() {
            let imported = &world.dynamic_bodies[source];
            let Ok(parts) = &imported.geometry else {
                continue;
            };
            let parts = parts.clone();
            let entity = imported.entity;
            let transform = world.entities[entity].initial_physics_transform();
            let layouts = super::super::dynamic_body::layout(&parts, &transform)?;
            let mut seen = false;
            for layout in layouts {
                let index = layout.parts[0];
                let part = &parts[index];
                let untouched = part.motion.current.clone();
                let mut motion = part.motion.instantiate()?.current;
                let mut material = part.material;
                let mut bounds = part.bounds_graph.as_ref().clone();
                world
                    .initialize_dynamic_body_properties(
                        source,
                        index,
                        &mut motion,
                        &mut material,
                        &mut bounds,
                        &mut seen,
                    )
                    .with_context(|| {
                        format!(
                            "{} body {:#x}",
                            world.entities[entity].name, layout.body_offset
                        )
                    })?;
                ensure!(
                    motion.swept_state().transform == layout.initial_transform,
                    "Body placed incorrectly"
                );
                ensure!(part.motion.current == untouched, "Original motion mutated");
                initialized += 1;
            }
        }
        ensure!(
            initialized > 0,
            "No original dynamic body property stages exercised"
        );
        println!("Initialized {initialized} original owned body property stages; simulation/admission not implied");
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn production_scripts_observe_each_committed_controller_tick() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let player = physics::Player::new(world.report.spawn, world.report.motion.clone());
        let mut controller = super::super::controller::Controller::new(player);
        let mut observed = Vec::new();
        controller.advance_with_context(&mut world,|world|&world.collision,0.1,true,
            super::super::controller::Input{movement:[0.,1.],..Default::default()},
            |player,tick,_,_,world|->Result<()> {
                world.run_script(&format!("local p=GetPosition(GetPlayer()); assert(p.r=={:.17e} and p.y=={:.17e} and p.z=={:.17e}, string.format('pose %.17g %.17g %.17g',p.r,p.y,p.z))",
                    player.position[0] as f64,player.position[1] as f64,player.position[2] as f64))?;
                world.advance_player_models(player,tick as f32*super::super::STEP)?;
                assert_eq!(&world.entities[world.player_entity].transform[12..15],&player.position);
                observed.push(player.position);Ok(())
            }).map_err(|error|anyhow::anyhow!("{error}"))?;
        assert_eq!(observed.len(), 3);
        assert!(
            observed.windows(2).any(|pair| pair[0] != pair[1]),
            "Scenario must actually move the player"
        );
        assert!(
            !world
                .report
                .unsupported
                .iter()
                .any(|error| error.contains("runtime animation console")),
            "{:?}",
            world
                .report
                .unsupported
                .iter()
                .filter(|error| error.contains("runtime animation console"))
                .collect::<Vec<_>>()
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn production_get_position_reads_live_world_across_coroutine_wait() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let player = world.player_entity;
        world.entities[player].transform[12..15].copy_from_slice(&[1.25, 2.5, 3.75]);
        world.run_script("local get=GetPosition; local p=get(GetPlayer()); assert(type(p)=='userdata' and p.r==1.25 and p.y==2.5 and p.z==3.75 and p.w==0); local z=get('missing entity'); assert(z.l==0 and z.w==0); wait(0); p=get(GetPlayer()); assert(p.r==9 and p.y==8 and p.z==7)")?;
        world.advance_models(0.)?;
        world.entities[player].transform[12..15].copy_from_slice(&[9., 8., 7.]);
        world.advance_models(1.)?;
        assert!(
            !world
                .report
                .unsupported
                .iter()
                .any(|e| e.contains("runtime animation console")),
            "{:?}",
            world.report.unsupported
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn projectile_contact_reaches_bound_game_event_lua_callback() -> Result<()> {
        projectile_game_events_scenario(false)
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn retail_paint_area_contact_reaches_game_event_lua_once() -> Result<()> {
        projectile_game_events_scenario(true)
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn spin_overlap_dispatches_once_per_attack_to_scene_script() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let effect = world
            .report
            .spin_assets
            .as_ref()
            .context("Spin effect was not imported")?;
        assert_eq!(effect.root, "mk_paladin_spin_fx");
        let ribbon = world
            .report
            .spin_ribbons
            .values()
            .flatten()
            .next()
            .context("Spin ribbon generator missing")?;
        assert_eq!(ribbon.subdivisions, 6);
        let ribbon = world.spin_ribbon.as_mut().context("Live ribbon missing")?;
        let mut attachment = [
            1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
        ];
        ribbon.advance(8., false, attachment)?;
        for tick in 0..8 {
            attachment[12] = tick as f32 * 0.1;
            ribbon.advance(1. / 30., true, attachment)?;
        }
        let mut trail = Vec::new();
        ribbon.append([1., 0., 0.], [0., 1., 0.], &mut trail)?;
        assert_eq!(trail.len(), 1, "Late spin activation emitted no ribbon");
        assert!(!trail[0].indices.is_empty());
        ribbon.advance(0.2, false, attachment)?;
        trail.clear();
        ribbon.append([1., 0., 0.], [0., 1., 0.], &mut trail)?;
        assert!(trail.is_empty(), "Stopped ribbon failed to expire");

        let inactive=world.entities.iter().position(|e|e.name=="gremlinvillage_ai.spawner_items 01.spawner_items.Ball_Ammo_Thinner 01")
            .context("Original unspawned pickup absent")?;
        let (_, &inactive_owner)=world.dynamic_runtime_sources.iter()
            .find(|(source,_)|world.dynamic_bodies[**source].entity==inactive)
            .context("Original unspawned pickup body absent")?;
        ensure!(!world.collision.owner_in_world(inactive_owner)?,"Negative fixture must remain outside the physics world");
        let inactive_shape=world.collision.owner_shapes(inactive_owner).next().context("Inactive body shape absent")?;
        let mut inactive_hits=HashSet::new();
        world.dispatch_spin(world.collision.simulation_pose(inactive_shape)?.position,&mut inactive_hits)?;
        ensure!(!inactive_hits.contains(&inactive),"Spin hit an unspawned source pickup");

        // This is a controlled health/event fixture on an imported dynamic
        // prop. Template bodies outside the world are not positive targets.
        let mut admitted=None;
        for (&source,&owner) in &world.dynamic_runtime_sources {
            let target=world.dynamic_bodies[source].entity;
            if target!=world.player_entity && world.entity_scene_active(target)
                && !world.hidden_entities.contains(&target)
                && !world.removed_entities.contains(&target)
                && world.collision.owner_in_world(owner)? {
                admitted=Some((source,owner,target));break;
            }
        }
        let (source,owner,target)=admitted.context("Original level has no admitted dynamic prop for the spin fixture")?;
        use epic_mickey_lib_rs::scene_file::{SceneFile, SceneFileVersion, ID};
        let scene = SceneFile::new(
            vec![],
            vec![],
            vec![],
            ID::new(0),
            SceneFileVersion::Version1,
        );
        let mut health = crate::component_schema::CATALOG.components["HealthComponent"]
            .instantiate("HealthComponent", &scene)?;
        for p in &mut health.properties {
            p.value = match p.name.as_str() {
                "Health" | "MaxHealth" => serde_json::json!(100),
                "InfiniteHealth" | "ForceInvulnerable" => serde_json::json!(false),
                "InvulnerableDuration" => serde_json::json!(0.),
                _ => p.value.clone(),
            };
        }
        let authored = super::super::health::Authored::from_component(&health)?;
        world.authored_health.insert(target, authored);
        world.runtime_health.remove(&target);
        world.entities[target].components.retain(|c| {
            c.class_name != "HealthComponent" && c.class_name != "VulnerabilityComponent"
        });
        world.entities[target].components.push(health);
        let mut vulnerability = crate::component_schema::CATALOG.components
            ["VulnerabilityComponent"]
            .instantiate("VulnerabilityComponent", &scene)?;
        for p in &mut vulnerability.properties {
            if p.name.ends_with(" Vulnerability") {
                p.value = serde_json::json!(if p.name == "Spin Vulnerability" {
                    1.
                } else {
                    0.
                });
            }
            if p.name.ends_with(" Threshold") {
                p.value = serde_json::json!(0.);
            }
        }
        world.entities[target].components.push(vulnerability);
        let damage = world
            .report
            .spin_settings
            .as_ref()
            .unwrap()
            .preview_value("SpinStim")
            .ceil() as i32;
        let mut listener = crate::component_schema::CATALOG.components["GameEventsComponent"]
            .instantiate("GameEventsComponent", &scene)?;
        for p in &mut listener.properties {
            p.value = match p.name.as_str() {
                "GameEventTypes" => serde_json::json!([9, 1]),
                "EventSourceAliases" => serde_json::json!(["Spin", ""]),
                "ScriptFunctionNames" => {
                    serde_json::json!(["BwxSpinObserver", "BwxDamageObserver"])
                }
                _ => p.value.clone(),
            };
        }
        world.component_events.bind(target, &listener)?;
        world.run_callback_fixture("BwxSpinCount=0; BwxDamageCount=0; function BwxSpinObserver(e) BwxSpinCount=BwxSpinCount+1 end; function BwxDamageObserver(e) BwxDamageCount=BwxDamageCount+1 end")?;
        world.advance_models(0.)?;
        let shape = world
            .collision
            .owner_shapes(owner)
            .next()
            .context("Missing target body")?;
        let position = world.collision.simulation_pose(shape)?.position;
        let mut hits = HashSet::new();
        world.dispatch_spin(position, &mut hits)?;
        assert!(
            hits.contains(&target),
            "Original body did not receive spin overlap: entity={}, source={}, owner={}, shape={}, in_world={:?}, filter={:?}, pose={:?}, raw_overlaps={:?}",
            world.entities[target].name,source,owner,shape,world.collision.owner_in_world(owner),
            world.collision.simulation_filter(shape),position,
            world.collision.overlap_sphere(position,world.report.spin_settings.as_ref().unwrap().preview_value("Range"))
        );
        assert_eq!(world.runtime_health[&target].health, 100 - damage);
        world.dispatch_spin(position, &mut hits)?;
        world.run_script("assert(BwxSpinCount==1, 'Spin hit repeated within one attack')")?;
        world.advance_models(0.)?;
        assert_eq!(world.runtime_health[&target].health, 100 - damage);
        hits.clear();
        world.dispatch_spin(position, &mut hits)?;
        assert_eq!(world.runtime_health[&target].health, 100 - 2 * damage);
        assert_eq!(world.authored_health[&target].health, 100);
        world.run_script("assert(BwxSpinCount==2, 'New attack did not hit again'); assert(BwxDamageCount==2, 'Health damage did not reach scene listener')")?;
        world.advance_models(0.)?;
        // A lethal spin schedules the authored action on component update,
        // then removes the admitted body as well as its rendering.
        world.entities[target]
            .components
            .iter_mut()
            .find(|c| c.class_name == "HealthComponent")
            .unwrap()
            .properties
            .iter_mut()
            .find(|p| p.name == "NoHealthAction")
            .unwrap()
            .value = serde_json::json!(2);
        world.runtime_health.get_mut(&target).unwrap().health = damage;
        let target_name = world.entities[target].name.clone();
        world.run_callback_fixture(&format!(
            "BwxRetiringTarget=GetEntity({target_name:?}); assert(BwxRetiringTarget~=nil)"
        ))?;
        world.advance_models(0.)?;
        hits.clear();
        world.dispatch_spin(position, &mut hits)?;
        assert_eq!(world.runtime_health[&target].health, 0);
        assert!(!world.removed_entities.contains(&target));
        world.advance_models(0.)?;
        assert!(world.removed_entities.contains(&target));
        assert!(world.entities[target]
            .draws
            .clone()
            .all(|draw| world.removed_draws.contains(&draw)));
        for draw in world.entities[target].draws.clone() {
            assert!(world.draws[draw].v_data.is_empty());
            assert!(world.draws[draw].indices.is_empty());
            assert!(world.draws[draw].material_images.is_empty());
        }
        for part in world.entities[target].collision_parts.clone() {
            assert!(world.report.collision_parts[part]
                .native_resource
                .is_empty());
            assert!(world.report.collision_parts[part].native_welding.is_empty());
        }
        assert!(world
            .collision
            .overlap_sphere(position, 0.1)?
            .iter()
            .all(|hit| hit.owner != owner));
        assert!(!world.dynamic_runtime_sources.contains_key(&source));
        assert!(world.dynamic_bodies[source].geometry.is_err());
        assert!(world.dynamic_bodies[source].systems.is_err());
        let retired_transform = world.entities[target].transform;
        world.advance_dynamic_physics(super::super::STEP)?;
        assert_eq!(world.entities[target].transform, retired_transform);
        assert_ne!(world.scripts.resolve_name(&target_name), Some(target));
        // Evaluate the owned handle checks synchronously. Unrelated startup
        // scripts can have unsupported APIs in this strict (non-demo) World.
        world.run_callback_fixture("assert(GetName(BwxRetiringTarget)==nil,'retired name'); assert(GetPrefabData(BwxRetiringTarget)==nil,'retired prefab'); assert(select('#',AnimGBReset(BwxRetiringTarget))==0,'stale reset must decline without a result')")?;
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_attached_platforms_remain_keyframed_through_motion_and_paint() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let sources = world
            .dynamic_runtime_sources
            .keys()
            .copied()
            .filter(|&source| {
                let entity = world.dynamic_bodies[source].entity;
                world.entities[entity].name.contains("GV_ZoneA_Platform01 ")
                    && world.transform_hierarchy.contains_key(&entity)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            sources.len(),
            4,
            "Original wheel assembly lost its four attached platforms"
        );
        for source in sources {
            let entity = world.dynamic_bodies[source].entity;
            let name = world.entities[entity].name.clone();
            assert!(world.entities[entity].transform_parent_id()?.is_some());
            let mode = |world: &World| {
                world.dynamic_constructions[&source].bodies[0]
                    .motion
                    .current
                    .motion_type()
            };
            assert_eq!(mode(&world), 4, "Admission released attached {name}");
            world.run_callback_fixture(&format!(
                "assert(select('#',EnableMotion(GetEntity({name:?})))==0)"
            ))?;
            assert_eq!(mode(&world), 4, "Lua released attached {name}");
            world.paint_admitted_dynamic_model(entity, false)?;
            world.paint_admitted_dynamic_model(entity, true)?;
            assert_eq!(mode(&world), 4, "Paint released attached {name}");
            world.set_entity_motion_enabled(entity, false)?;
            world.set_entity_motion_enabled(entity, true)?;
            assert_eq!(mode(&world), 4, "Motion IO path released attached {name}");
        }
        // Missing targets and real entities without motion components are
        // native declines, while unsupported character/controller work errors.
        world.run_callback_fixture("assert(select('#',DisableMotion('BwxMissingMotionTarget'))==0); assert(select('#',EnableMotion(nil))==0)")?;
        let inert = world
            .entities
            .iter()
            .position(|e| e.name == "gremlinvillage_zonea_static.GV_ZoneA_Inert01 01")
            .context("No inert fixture")?;
        world.set_entity_motion_enabled(inert, false)?;
        let npc = world
            .entities
            .iter()
            .position(|e| e.name == "gremlinvillage_zonea.SpatterMelee 01")
            .context("No original Spatter")?;
        let error = world
            .set_entity_motion_enabled(npc, false)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("character motion suspension"),
            "Missing native branch was hidden: {error}"
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_anvil_delayed_motion_commands_reach_shared_physics() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let (&source, _) = world
            .dynamic_runtime_sources
            .iter()
            .next()
            .context("Missing admitted body")?;
        let target = world.dynamic_bodies[source].entity;
        world.set_entity_motion_enabled(target, true)?;
        let body = &world.dynamic_constructions[&source].bodies[0];
        let shape = body.shapes[0];
        let initial_type = body.motion.current.motion_type();
        let velocity = body.linear_velocity();
        let template = &world.sketch_templates.templates["paintanvil"].graph;
        let mut io = template.definitions[&template.root]
            .components
            .iter()
            .find(|c| c.class_name == "InputOutputComponent")
            .context("Missing anvil IO")?
            .clone();
        let inputs = io
            .properties
            .iter()
            .find(|p| p.name == "Input")
            .unwrap()
            .value
            .as_array()
            .unwrap();
        assert_eq!(inputs[0], "DisableMotion");
        assert_eq!(inputs[1], "EnableMotion");
        // Exercise the two authored motion connections unchanged on a real
        // admitted scene body; the sketch factory is a separate requirement.
        for property in &mut io.properties {
            if let Some(array) = property.value.as_array_mut() {
                array.truncate(2);
            }
        }
        world.outputs = super::super::actor_io::Outputs::default();
        world
            .outputs
            .bind(target, &world.entities[target].name, &[io])?;
        let errors = world.report.unsupported.len();
        world
            .outputs
            .emit(target, "OnSpawned", world.player_entity, 0.);
        world.dispatch_actor_inputs(0.);
        assert_eq!(
            world.dynamic_constructions[&source].bodies[0]
                .motion
                .current
                .motion_type(),
            4
        );
        let frozen = world.collision.simulation_pose(shape)?.position;
        world.advance_dynamic_physics(super::super::STEP)?;
        assert_eq!(world.collision.simulation_pose(shape)?.position, frozen);
        world.dispatch_actor_inputs(0.52);
        assert_eq!(
            world.dynamic_constructions[&source].bodies[0]
                .motion
                .current
                .motion_type(),
            4
        );
        world.dispatch_actor_inputs(0.54);
        let restored = &world.dynamic_constructions[&source].bodies[0];
        assert_eq!(restored.motion.current.motion_type(), initial_type);
        assert_eq!(restored.linear_velocity(), velocity);
        assert_eq!(
            world.collision.simulation_pose(shape)?.linear,
            velocity[..3]
        );
        assert_eq!(
            world.report.unsupported.len(),
            errors,
            "Motion IO failed: {:?}",
            &world.report.unsupported[errors..]
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_config_and_directional_lua_stimulus_reach_dynamic_motion() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let (&source, &owner) = world
            .dynamic_runtime_sources
            .iter()
            .next()
            .context("Missing dynamic fixture")?;
        let target = world.dynamic_bodies[source].entity;
        world.entities[target]
            .components
            .retain(|component| component.class_name != "VulnerabilityComponent");
        world.component_events.remove(target);
        world.set_entity_motion_enabled(target, true)?;
        let shape = world.collision.owner_shapes(owner).next().unwrap();
        let variable = "MickeyAbilities_Sketch_Anvil_AnvilKnockbackStim";
        let original: f32 = world.runtime_settings[variable].parse()?;
        world.run_callback_fixture(&format!(
            "assert(GetConfigVariableFloat({variable:?})=={original:?})"
        ))?;
        world.runtime_settings.insert(variable.into(), "4.0".into());
        // This isolated API regression samples poses and dispatches
        // synchronously. Startup scripts must not interleave those operations.
        let mass = world.dynamic_constructions[&source].bodies[0]
            .motion
            .current
            .solver_inverse_mass()?
            .1;
        ensure!(mass > 0., "Fixture has no dynamic inverse mass");
        world.collision.set_simulation_velocity(shape, [0.; 3])?;
        let position =
            |entity: usize| std::array::from_fn(|i| world.entities[entity].transform[12 + i]);
        let direction = super::super::script_stimulus::direction(
            position(world.player_entity),
            position(target),
            0.,
        )?
        .unwrap();
        ensure!(
            direction.iter().any(|value| *value != 0.),
            "Fixture coincides with source"
        );
        let target_name = world.entities[target].name.clone();
        world.run_callback_fixture(&format!("assert(GetConfigVariableFloat({variable:?})==4); StimulusEntityWithAliasCalculateDirectionInRadius(GetPlayer(),ST_KNOCKBACK,GetConfigVariableFloat({variable:?}),0,GetEntity({target_name:?}),'Anvil')"))?;
        let velocity = world.collision.simulation_pose(shape)?.linear;
        for axis in 0..3 {
            assert_eq!(
                velocity[axis],
                (direction[axis] * 4.) * mass,
                "Directional stimulus missed the live solver body"
            );
        }
        world.run_callback_fixture("assert(not pcall(GetConfigVariableFloat,'__missing_compiled_default')); assert(not pcall(StimulusEntityWithAliasCalculateDirectionInRadius,GetPlayer(),ST_KNOCKBACK,1,0,GetPlayer(),'Anvil'))")?;
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_anvil_landing_script_damages_and_retires_a_breakable_fixture() -> Result<()> {
        anvil_breaks_imported_fixture(true)
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_anvil_landing_breaks_a_freely_moving_fixture() -> Result<()> {
        anvil_breaks_imported_fixture(false)
    }
    fn anvil_breaks_imported_fixture(held: bool) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let (&source, &owner) = world
            .dynamic_runtime_sources
            .iter()
            .next()
            .context("Missing body for breakable fixture")?;
        let target = world.dynamic_bodies[source].entity;
        use epic_mickey_lib_rs::scene_file::{SceneFile, SceneFileVersion, ID};
        let scene = SceneFile::new(
            vec![],
            vec![],
            vec![],
            ID::new(0),
            SceneFileVersion::Version1,
        );
        let mut health = crate::component_schema::CATALOG.components["HealthComponent"]
            .instantiate("HealthComponent", &scene)?;
        for property in &mut health.properties {
            property.value = match property.name.as_str() {
                "Health" | "MaxHealth" => serde_json::json!(10),
                "InfiniteHealth" | "ForceInvulnerable" => serde_json::json!(false),
                "InvulnerableDuration" => serde_json::json!(0.),
                "NoHealthAction" => serde_json::json!(2),
                _ => property.value.clone(),
            };
        }
        world.authored_health.insert(
            target,
            super::super::health::Authored::from_component(&health)?,
        );
        world.runtime_health.remove(&target);
        world.component_events.remove(target);
        world.entities[target].components.retain(|component| {
            component.class_name != "HealthComponent"
                && component.class_name != "VulnerabilityComponent"
        });
        world.entities[target].components.push(health);
        // The target is a test breakable using an actual imported collider and
        // render binding; no retail asset or reviewed Lua source is modified.
        if held {
            world.set_entity_motion_enabled(target, false)?;
        }
        let shape = world.collision.owner_shapes(owner).next().unwrap();
        if !held {
            // The first ordered source is a ghost-layer pickup outside the
            // playable floor. Make the *test instance* an ordinary physical
            // prop on known supporting geometry. Retain original mass, hull,
            // inertia, damping and limits; do not modify retail assets.
            let word = world
                .collision
                .simulation_filter(shape)
                .context("Fixture has no filter")?;
            world.collision.set_simulation_filter(
                shape,
                super::super::body_filter::change_layer(word, 2, 1, 1),
            )?;
            let pose = world.collision.simulation_pose(shape)?;
            world.collision.set_shape_transform(
                shape,
                [
                    world.report.spawn[0],
                    world.report.spawn[1] + 2.,
                    world.report.spawn[2] - 2.5,
                ],
                pose.orientation,
            )?;
        }
        let record = &world.dynamic_constructions[&source].bodies[0]
            .motion
            .current;
        eprintln!("Anvil impact fixture: held={held} source={source} entity={} mass={:?} filter={:?} limits={:?}",
            world.entities[target].name,if held {None} else {Some(record.solver_inverse_mass()?)},world.collision.simulation_filter(shape),
            [super::super::native_motion_limits::decode(record.0[0xbc]),super::super::native_motion_limits::decode(record.0[0xbd])]);
        let mut resting_y = None;
        if !held {
            for _ in 0..120 {
                world.advance_dynamic_physics(super::super::STEP)?;
            }
            let pose = world.collision.simulation_pose(shape)?;
            eprintln!("Settled free fixture before impact: {pose:?}");
            ensure!(
                pose.position[1] > world.report.spawn[1] - 1.,
                "Fixture already fell through the floor without an anvil"
            );
            ensure!(
                super::super::physics::length(pose.linear) < 0.05,
                "Fixture is not at rest before the impact"
            );
            resting_y = Some(pose.position[1]);
        }
        let position = world
            .collision
            .simulation_pose(world.collision.owner_shapes(owner).next().unwrap())?
            .position;
        world.run_script(&format!(
            "StimulusSphereWithAlias(GetPlayer(),ST_BREAKABLE,10,GetPosition({:?}),0,'Anvil')",
            world.entities[target].name
        ))?;
        world.advance_models(0.)?;
        assert!(
            !world.runtime_health.contains_key(&target),
            "Zero-radius sphere became an overlap attack"
        );
        let anvil = world.spawn_sketch_root(
            "paintanvil",
            [position[0], position[1] + 3.5, position[2]],
            [1., 0., 0., 0., 1., 0., 0., 0., 1.],
        )?;
        let mut player =
            super::super::physics::Player::new(world.report.spawn, world.report.motion.clone());
        let anvil_source = *world
            .dynamic_runtime_sources
            .keys()
            .find(|&&source| world.dynamic_bodies[source].entity == anvil)
            .context("Spawned anvil has no body")?;
        let mut mass_logged = false;
        let mut lowest_y = f32::INFINITY;
        let mut damage = false;
        for tick in 1..=120 {
            world.advance_dynamic_physics(super::super::STEP)?;
            let record = &world.dynamic_constructions[&anvil_source].bodies[0]
                .motion
                .current;
            if !mass_logged && matches!(record.motion_type(), 2 | 3 | 6) {
                eprintln!(
                    "Active anvil inverse mass: {:?}",
                    record.solver_inverse_mass()?
                );
                mass_logged = true;
            }
            if let Some(resting) = resting_y {
                let pose = world.collision.simulation_pose(shape)?;
                lowest_y = lowest_y.min(pose.position[1]);
                // Box3D stability allowance, fixed before this impact run.
                // This does not stand in for an original Havok trajectory.
                ensure!(pose.position[1]>=resting-0.02,
                    "Heavy impact drove the settled body into its floor at tick{tick}: restingY={resting}, {pose:?}");
            }
            world.advance_player_models(&mut player, tick as f32 * super::super::STEP)?;
            damage |= world
                .runtime_health
                .get(&target)
                .is_some_and(|state| state.health == 0);
            if world.removed_entities.contains(&target) {
                break;
            }
        }
        if let Some(resting) = resting_y {
            eprintln!("Physical fixture height during impact: {lowest_y}..{resting}");
        }
        assert!(damage && world.removed_entities.contains(&target),"Authored landing script did not break the fixture: health {:?}, root {:?}, target {:?}, script errors {:?}",
            world.runtime_health.get(&target),&world.entities[anvil].transform[12..15],&world.entities[target].transform[12..15],
            world.report.unsupported.iter().filter(|message|message.contains("Anvil") || message.contains("anvil")).collect::<Vec<_>>());
        assert!(world.entities[target]
            .draws
            .clone()
            .all(|draw| world.removed_draws.contains(&draw)));
        assert!(!world.dynamic_runtime_sources.contains_key(&source));
        assert!(!world.removed_entities.contains(&anvil));
        ensure!(
            !world
                .report
                .unsupported
                .iter()
                .any(|message| message.contains("AnvilImpactBreakables")),
            "Authored breakable script failed: {:?}",
            world
                .report
                .unsupported
                .iter()
                .filter(|message| message.contains("AnvilImpactBreakables"))
                .collect::<Vec<_>>()
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn spawned_anvil_authored_lifetime_exits_and_removes_the_live_root() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let position = [
            world.report.spawn[0] + 2.,
            world.report.spawn[1] + 3.5,
            world.report.spawn[2],
        ];
        let entity = world.spawn_sketch_root(
            "paintanvil",
            position,
            [1., 0., 0., 0., 1., 0., 0., 0., 1.],
        )?;
        assert_eq!(world.runtime_lifetimes[&entity].timer.remaining, 7.);
        let target_name = world.entities[entity].name.clone();
        let (&source, &owner) = world
            .dynamic_runtime_sources
            .iter()
            .find(|(source, _)| world.dynamic_bodies[**source].entity == entity)
            .context("Anvil body missing")?;
        // Add an expiry observer and a setter as a mod would, while retaining
        // every original landing/death/motion connection. The expiry setter
        // must be overwritten by the saved subtraction after callback dispatch.
        let mut io = world.entities[entity]
            .components
            .iter()
            .find(|component| component.class_name == "InputOutputComponent")
            .unwrap()
            .clone();
        for (input, parameters) in [
            ("BwxExpiryObserver", ""),
            ("SetPropertyFloat", "Lifetime, 99"),
        ] {
            for property in &mut io.properties {
                if let Some(values) = property.value.as_array_mut() {
                    let value = match property.name.as_str() {
                        "Output" => serde_json::json!("OnLifetimeExpired"),
                        "Input" => serde_json::json!(input),
                        "Target" => serde_json::json!("_self"),
                        "Parameters" => serde_json::json!(parameters),
                        "DelayWait" => serde_json::json!(0),
                        "IoFlags" => serde_json::json!(0),
                        "TargetGroup" => serde_json::json!(""),
                        _ => continue,
                    };
                    values.push(value);
                }
            }
        }
        world.outputs.remove_owner(entity);
        world.outputs.bind(entity, &target_name, &[io])?;
        world.emit_entity_output(entity, "OnSpawned", entity);
        world.run_script("BwxExpiryCount=0; function BwxExpiryObserver(e) assert(GetName(e)~=nil); BwxExpiryCount=BwxExpiryCount+1 end")?;
        let mut player =
            super::super::physics::Player::new(world.report.spawn, world.report.motion.clone());
        let graph = &world.sketch_templates.templates["paintanvil"]
            .animation
            .as_ref()
            .map_err(|e| anyhow::anyhow!(e.clone()))?
            .as_ref()
            .unwrap()
            .graph;
        let state = graph.single("hkbBehaviorGraph")?["rootGenerator"]["ref"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut reset_on_landing = false;
        let mut exit = false;
        let mut physics_removed = false;
        let mut expired = false;
        for tick in 1..=390 {
            let previous = world
                .runtime_lifetimes
                .get(&entity)
                .map(|state| state.timer.remaining);
            world.advance_dynamic_physics(super::super::STEP)?;
            world.advance_player_models(&mut player, tick as f32 * super::super::STEP)?;
            if world.removed_entities.contains(&entity) {
                assert!(previous.is_some_and(|remaining| remaining > 0.));
                expired = true;
                break;
            }
            reset_on_landing |= world.runtime_lifetimes[&entity].timer.remaining > 9.;
            exit |= world.havok_actors[&entity].1.prepared.machines[&state].current == 4;
            physics_removed |= !world.dynamic_runtime_sources.contains_key(&source);
        }
        assert!(reset_on_landing && exit && physics_removed && expired,
            "Incomplete authored lifetime path: reset={reset_on_landing}, exit={exit}, physics={physics_removed}, expired={expired}");
        world.run_script("assert(BwxExpiryCount==1)")?;
        let remaining = world.entities[entity]
            .components
            .iter()
            .find(|component| component.class_name == "LifetimeComponent")
            .unwrap()
            .properties
            .iter()
            .find(|property| property.name == "Lifetime")
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        assert!(
            remaining < 0.,
            "Expiry callback replaced the final timer subtraction"
        );
        assert!(!world.runtime_lifetimes.contains_key(&entity));
        assert!(!world.havok_actors.contains_key(&entity));
        assert!(world.entities[entity]
            .draws
            .clone()
            .all(|draw| world.removed_draws.contains(&draw)));
        // Retired shapes retain identity/filter metadata deliberately; test
        // actual collision participation rather than the immutable lookup.
        let shape = world.dynamic_constructions[&source].bodies[0].shapes[0];
        let center = world.collision.simulation_pose(shape)?.center;
        assert!(world
            .collision
            .overlap_sphere(center, 10.)?
            .iter()
            .all(|hit| hit.owner != owner));
        assert_eq!(world.scripts.resolve_name(&target_name), None);
        ensure!(
            !world
                .report
                .unsupported
                .iter()
                .any(|message| message.contains("SetPropertyFloat")
                    || message.contains("BwxExpiryObserver")),
            "Lifetime IO dispatch failed"
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn spawned_anvil_collision_drives_authored_landing_once() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let position = [
            world.report.spawn[0] + 2.,
            world.report.spawn[1] + 3.5,
            world.report.spawn[2],
        ];
        let entity = world.spawn_sketch_root(
            "paintanvil",
            position,
            [1., 0., 0., 0., 1., 0., 0., 0., 1.],
        )?;
        let (&source, _) = world
            .dynamic_runtime_sources
            .iter()
            .find(|(source, _)| world.dynamic_bodies[**source].entity == entity)
            .context("Anvil body missing")?;
        let mut player =
            super::super::physics::Player::new(world.report.spawn, world.report.motion.clone());
        let graph = &world.sketch_templates.templates["paintanvil"]
            .animation
            .as_ref()
            .map_err(|e| anyhow::anyhow!(e.clone()))?
            .as_ref()
            .unwrap()
            .graph;
        let state = graph.single("hkbBehaviorGraph")?["rootGenerator"]["ref"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut landings = 0;
        for tick in 1..=120 {
            world.advance_dynamic_physics(super::super::STEP)?;
            // Observe the actual physics-produced authored batch, then deliver
            // it through the normal dispatcher without injecting an event.
            let deliveries = world.outputs.ready_for_owner(world.animation_time, entity);
            for delivery in &deliveries {
                if delivery.connection.input == "AnimEvent"
                    && delivery.connection.parameters == "EVENT_Start_Land"
                {
                    assert_eq!(delivery.connection.output, "OnCollisionGround");
                    assert_ne!(delivery.activator, Some(entity), "Contact lost its other entity");
                    assert!(delivery.activator.is_some_and(|activator|!world.removed_entities.contains(&activator)));
                    landings += 1;
                }
            }
            world.dispatch_actor_deliveries(deliveries);
            world.advance_player_models(&mut player, tick as f32 * super::super::STEP)?;
        }
        assert_eq!(
            landings, 1,
            "Resting contacts repeatedly retriggered the landing sequence"
        );
        assert!(world.dynamic_ground_contacts.contains(&source));
        assert_eq!(
            world.havok_actors[&entity].1.prepared.machines[&state].current, 3,
            "Physics landing did not reach authored IdleGround through Land"
        );
        world.retire_entity(entity)?;
        assert!(
            !world.dynamic_ground_contacts.contains(&source),
            "Destroyed listener retained its contact latch"
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn spawned_anvil_root_joins_world_physics_animation_and_lighting() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let position = [
            world.report.spawn[0] + 2.,
            world.report.spawn[1] + 3.5,
            world.report.spawn[2],
        ];
        let entity = world.spawn_sketch_root(
            "paintanvil",
            position,
            [1., 0., 0., 0., 1., 0., 0., 0., 1.],
        )?;
        assert!(world.havok_actors.contains_key(&entity));
        let (&source, &owner) = world
            .dynamic_runtime_sources
            .iter()
            .find(|(source, _)| world.dynamic_bodies[**source].entity == entity)
            .context("New body was not admitted")?;
        assert_eq!(
            world.dynamic_constructions[&source].bodies[0]
                .motion
                .current
                .motion_type(),
            4,
            "Spawn failed to freeze immediately"
        );
        let shape = world
            .collision
            .owner_shapes(owner)
            .next()
            .context("New body shape missing")?;
        let held = world.collision.simulation_pose(shape)?.position;
        world.advance_dynamic_physics(super::super::STEP)?;
        assert_eq!(world.collision.simulation_pose(shape)?.position, held);
        let mut player =
            super::super::physics::Player::new(world.report.spawn, world.report.motion.clone());
        let graph = &world.sketch_templates.templates["paintanvil"]
            .animation
            .as_ref()
            .map_err(|e| anyhow::anyhow!(e.clone()))?
            .as_ref()
            .unwrap()
            .graph;
        let state = graph.single("hkbBehaviorGraph")?["rootGenerator"]["ref"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut initial = world.draws.clone();
        world.apply_model_poses(&mut initial)?;
        for tick in 1..=30 {
            world.advance_player_models(&mut player, tick as f32 * super::super::STEP)?;
        }
        assert_ne!(
            world.dynamic_constructions[&source].bodies[0]
                .motion
                .current
                .motion_type(),
            4,
            "Delayed motion never resumed"
        );
        assert_eq!(
            world.havok_actors[&entity].1.prepared.machines[&state].current,
            1
        );
        let mut draws = world.draws.clone();
        world.apply_model_poses(&mut draws)?;
        assert!(
            world.entities[entity]
                .draws
                .clone()
                .any(|i| draws[i].v_data != initial[i].v_data),
            "Live pose was not rendered"
        );
        let view = [
            1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
        ];
        world.apply_spherical_lighting(&mut draws, &view);
        for draw in world.entities[entity].draws.clone() {
            assert!(
                draws[draw].preview.as_ref().is_some_and(Result::is_ok),
                "New model lighting failed: {:?}",
                draws[draw].preview
            );
        }
        let second = world.spawn_sketch_root(
            "paintanvil",
            [position[0] + 2., position[1], position[2]],
            [1., 0., 0., 0., 1., 0., 0., 0., 1.],
        )?;
        let second_source = *world
            .dynamic_runtime_sources
            .keys()
            .find(|&&s| world.dynamic_bodies[s].entity == second)
            .unwrap();
        assert!(Arc::ptr_eq(
            world.dynamic_bodies[source].geometry.as_ref().unwrap(),
            world.dynamic_bodies[second_source]
                .geometry
                .as_ref()
                .unwrap()
        ));
        assert_eq!(
            world.havok_actors[&second].1.prepared.machines[&state].current,
            0
        );
        world.retire_entity(entity)?;
        assert!(!world.havok_actors.contains_key(&entity));
        assert!(world.havok_actors.contains_key(&second));
        assert!(!world
            .spherical_bindings
            .iter()
            .any(|binding| binding.entity == entity));
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_anvil_death_output_retires_body_after_authored_delay() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let (&source, &owner) = world
            .dynamic_runtime_sources
            .iter()
            .next()
            .context("Missing admitted body")?;
        let target = world.dynamic_bodies[source].entity;
        let template = &world.sketch_templates.templates["paintanvil"].graph;
        let mut io = template.definitions[&template.root]
            .components
            .iter()
            .find(|c| c.class_name == "InputOutputComponent")
            .context("Missing anvil IO")?
            .clone();
        let index = io
            .properties
            .iter()
            .find(|p| p.name == "Input")
            .unwrap()
            .value
            .as_array()
            .unwrap()
            .iter()
            .position(|v| v == "DestroyEntity")
            .context("Missing original destroy connection")?;
        for property in &mut io.properties {
            if let Some(array) = property.value.as_array_mut() {
                *array = array.get(index).cloned().into_iter().collect();
            }
        }
        world.outputs = super::super::actor_io::Outputs::default();
        world
            .outputs
            .bind(target, &world.entities[target].name, &[io])?;
        let errors = world.report.unsupported.len();
        world.outputs.emit(target, "OnDeath", target, 0.);
        // Duplicate ready deliveries also exercise stale-owner batch handling.
        world.outputs.emit(target, "OnDeath", target, 0.);
        world.dispatch_actor_inputs(0.59);
        assert!(!world.removed_entities.contains(&target));
        world.dispatch_actor_inputs(0.61);
        assert!(world.removed_entities.contains(&target));
        assert!(!world.dynamic_runtime_sources.contains_key(&source));
        let shape = world
            .collision
            .owner_shapes(owner)
            .next()
            .context("Missing stable shape")?;
        let position = world.collision.simulation_pose(shape)?.position;
        assert!(world
            .collision
            .overlap_sphere(position, 0.1)?
            .iter()
            .all(|hit| hit.owner != owner));
        world.advance_dynamic_physics(super::super::STEP)?;
        assert_eq!(
            world.report.unsupported.len(),
            errors,
            "Destroy IO failed: {:?}",
            &world.report.unsupported[errors..]
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn anvil_physics_removal_preserves_entity_until_delayed_death() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let (&source, &owner) = world
            .dynamic_runtime_sources
            .iter()
            .next()
            .context("Missing admitted body")?;
        let target = world.dynamic_bodies[source].entity;
        let shape = world
            .collision
            .owner_shapes(owner)
            .next()
            .context("Missing shape")?;
        let position = world.collision.simulation_pose(shape)?.position;
        let template = &world.sketch_templates.templates["paintanvil"].graph;
        let mut io = template.definitions[&template.root]
            .components
            .iter()
            .find(|c| c.class_name == "InputOutputComponent")
            .context("Missing anvil IO")?
            .clone();
        let values = |name: &str| {
            io.properties
                .iter()
                .find(|p| p.name == name)
                .unwrap()
                .value
                .as_array()
                .unwrap()
        };
        let indices = values("Input")
            .iter()
            .enumerate()
            .filter_map(|(i, v)| {
                (v == "DestroyEntity"
                    || (v == "RemovePhysicsFromWorld"
                        && values("Output")[i] == "OnCollisionGround"))
                    .then_some(i)
            })
            .collect::<Vec<_>>();
        assert_eq!(indices.len(), 2);
        for property in &mut io.properties {
            if let Some(array) = property.value.as_array_mut() {
                *array = indices
                    .iter()
                    .filter_map(|&i| array.get(i).cloned())
                    .collect();
            }
        }
        world.outputs = super::super::actor_io::Outputs::default();
        world
            .outputs
            .bind(target, &world.entities[target].name, &[io])?;
        let errors = world.report.unsupported.len();
        world.outputs.emit(target, "OnCollisionGround", target, 0.);
        world.dispatch_actor_inputs(9.5);
        assert!(world.dynamic_runtime_sources.contains_key(&source));
        world.dispatch_actor_inputs(9.7);
        assert!(!world.dynamic_runtime_sources.contains_key(&source));
        assert!(!world.removed_entities.contains(&target));
        assert!(world.entities[target]
            .draws
            .clone()
            .all(|draw| !world.removed_draws.contains(&draw)));
        assert!(world
            .collision
            .overlap_sphere(position, 0.1)?
            .iter()
            .all(|hit| hit.owner != owner));
        world.outputs.emit(target, "OnDeath", target, 9.7);
        world.dispatch_actor_inputs(10.4);
        assert!(world.removed_entities.contains(&target));
        world.advance_dynamic_physics(super::super::STEP)?;
        assert_eq!(
            world.report.unsupported.len(),
            errors,
            "Physics removal IO failed: {:?}",
            &world.report.unsupported[errors..]
        );
        Ok(())
    }
    fn projectile_game_events_scenario(area: bool) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let (&source, &owner) = world
            .dynamic_runtime_sources
            .iter()
            .next()
            .context("No admitted dynamic target")?;
        let target = world.dynamic_bodies[source].entity;
        use epic_mickey_lib_rs::scene_file::{SceneFile, SceneFileVersion, ID};
        let scene = SceneFile::new(
            vec![],
            vec![],
            vec![],
            ID::new(0),
            SceneFileVersion::Version1,
        );
        let mut listener = crate::component_schema::CATALOG.components["GameEventsComponent"]
            .instantiate("GameEventsComponent", &scene)?;
        for p in &mut listener.properties {
            p.value = match p.name.as_str() {
                "GameEventTypes" => serde_json::json!([10]),
                "EventSourceAliases" => serde_json::json!(["Brush"]),
                "ScriptFunctionNames" => serde_json::json!(["BwxContactObserver"]),
                _ => p.value.clone(),
            };
        }
        world.component_events.bind(target, &listener)?;
        world.run_script("BwxContactCount=0; function BwxContactObserver(e) assert(type(e)=='userdata'); BwxContactCount=BwxContactCount+1 end")?;
        world.advance_models(0.)?;
        // The observer is instrumentation. The area scenario retains the
        // original PaintBall mode/radius; direct-mode instrumentation is separate.
        let name = world
            .report
            .brush_settings
            .as_ref()
            .unwrap()
            .projectile_templates[0]
            .clone();
        if area {
            assert_eq!(world.brush_stimuli[&name].impact_modes, [0]);
            assert_eq!(world.brush_stimuli[&name].radii, [1.]);
        } else {
            world.brush_stimuli.get_mut(&name).unwrap().impact_modes = vec![1];
        }
        let shape = world
            .collision
            .owner_shapes(owner)
            .next()
            .context("Target has no collision shape")?;
        let position = world.collision.simulation_pose(shape)?.position;
        let shot = world.projectile_preview.pool.acquire::<anyhow::Error>(
            super::super::brush::Ink::Paint,
            world.player_entity as u64,
            [0.; 3],
            [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            || Ok(0),
        )?;
        shot.set_launch_velocity([0., 0., 1.], false, 1.);
        let contact = super::super::brush_preview::Contact {
            ink: super::super::brush::Ink::Paint,
            projectile: 0,
            hit: super::super::physics::SweepHit {
                shape,
                source_triangle: None,
                fraction: 1.,
                position,
                normal: [0., 1., 0.],
                owner,
            },
        };
        let before = world.component_events.deliveries;
        world.dispatch_projectile_game_events(&contact)?;
        assert!(world.component_events.deliveries > before);
        world.run_script("assert(BwxContactCount==1)")?;
        world.advance_models(0.)?;
        assert!(
            !world
                .report
                .unsupported
                .iter()
                .any(|error| error.contains("runtime animation console")),
            "{:?}",
            world.report.unsupported
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_whole_toon_pipe_paints_and_thins_without_jigsaw() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let entity = world
            .entities
            .iter()
            .position(|e| e.name.ends_with("GV_SteamPipeB1_Toon 03"))
            .context("Missing original whole-toon pipe")?;
        ensure!(
            world.entities[entity].jigsaw_asset.is_none(),
            "Must exercise the non-jigsaw path"
        );
        let model = &world.model_paint[&entity];
        assert!(!model.painted);
        ensure!(
            !model.owners.is_empty() && !model.states.is_empty(),
            "Body/render binding missing"
        );
        assert!(model.states.keys().all(|i| world.sketch_draws.contains(i)));
        for &index in model.states.keys() {
            let program = world.draws[index]
                .preview
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap();
            let registers = program
                .register_defaults
                .context("Sketch front colors not bound")?;
            assert_eq!(registers[3], [1.; 4]);
            assert_eq!(
                registers[4],
                [6. / 255., 73. / 255., 250. / 255., 32. / 255.]
            );
            assert!(
                program.texture_slots.contains(&0),
                "Sketch must read actual diffuse alpha"
            );
        }
        let owner = model.owners[0];
        let active: Vec<_> = model
            .states
            .keys()
            .map(|&i| {
                (
                    i,
                    world.draws[i]
                        .preview
                        .as_ref()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .clone(),
                )
            })
            .collect();
        let mut presentation = world.draws.clone();
        world.apply_spherical_lighting(
            &mut presentation,
            &[
                1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
            ],
        );
        for (i, program) in active {
            assert!(
                Arc::ptr_eq(
                    &program,
                    presentation[i].preview.as_ref().unwrap().as_ref().unwrap()
                ),
                "Lighting replaced active sketch"
            );
        }
        let part = &world.report.collision_parts[owner];
        let mut actual_hit = None;
        for triangle in &world.collision.triangles
            [part.triangle_start..part.triangle_start + part.triangle_count]
        {
            let [a, b, c] = triangle.p;
            let u = physics::sub(b, a);
            let v = physics::sub(c, a);
            let normal = physics::unit([
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ]);
            let center = std::array::from_fn(|i| (a[i] + b[i] + c[i]) / 3.);
            let origin = physics::add(center, physics::mul(normal, 0.2));
            if let Some(hit) =
                world
                    .collision
                    .sweep_sphere(origin, physics::mul(normal, -0.4), 0.025)?
            {
                if hit.owner == owner {
                    actual_hit = Some(hit);
                    break;
                }
            }
        }
        let hit =
            actual_hit.context("No actual sphere contact reached original whole-toon body")?;
        let native_id = world.entities[entity].id;
        let saved = world.entities[entity]
            .components
            .iter()
            .find(|component| component.class_name == "PhysicsRigidBodyStaticComponent")
            .context("Original static pipe component missing")?
            .properties
            .iter()
            .find(|property| property.name == "ShouldSavePaintState")
            .map(|property| {
                property
                    .value
                    .as_bool()
                    .context("Invalid original save flag")
            })
            .transpose()?
            .unwrap_or(false);
        assert!(world.brush_jigsaw_hit(&hit, true, [10000.; 3])?);
        if saved {
            assert_eq!(
                world
                    .instance_changes
                    .get(native_id)
                    .context("Paint did not reach AISS")?
                    .painted,
                1
            );
        } else {
            assert!(
                world.instance_changes.get(native_id).is_none(),
                "Disabled save flag created a record"
            );
        }
        for _ in 0..60 {
            world.advance_jigsaw_effects(super::super::STEP)?;
        }
        let model = &world.model_paint[&entity];
        assert!(model.painted);
        assert!(model.states.keys().all(|i| !world.sketch_draws.contains(i)));
        assert!(world.brush_jigsaw_hit(&hit, false, [10000.; 3])?);
        if saved {
            assert_eq!(world.instance_changes.get(native_id).unwrap().painted, 0);
        }
        for _ in 0..60 {
            world.advance_jigsaw_effects(super::super::STEP)?;
        }
        let model = &world.model_paint[&entity];
        assert!(!model.painted);
        assert!(model.states.keys().all(|i| world.sketch_draws.contains(i)));
        assert!(!world.brush_model_hit(owner, false, [10000.; 3])?);
        // Diagnostic policy change in the imported in-memory component only:
        // exercise the positive gate even if this original fixture opts out.
        let component = world.entities[entity]
            .components
            .iter_mut()
            .find(|component| component.class_name == "PhysicsRigidBodyStaticComponent")
            .unwrap();
        if let Some(property) = component
            .properties
            .iter_mut()
            .find(|property| property.name == "ShouldSavePaintState")
        {
            property.value = serde_json::json!(true);
        } else {
            let mut property = component
                .properties
                .iter()
                .find(|property| property.name == "Is Painted")
                .unwrap()
                .clone();
            property.name = "ShouldSavePaintState".into();
            property.value = serde_json::json!(true);
            component.properties.push(property);
        }
        world.instance_changes.set_mood(native_id, -2.)?;
        assert!(world.brush_jigsaw_hit(&hit, true, [10000.; 3])?);
        let record = world.instance_changes.get(native_id).unwrap();
        assert_eq!((record.mood_bits, record.painted), ((-2f32).to_bits(), 1));
        let bytes = world.instance_changes.save_payload()?;
        let mut restored = super::super::save_instances::State::default();
        restored.restore_payload(&bytes)?;
        assert_eq!(restored.get(native_id), Some(record));
        // Reject the current render-clone ID limitation before changing paint,
        // rather than silently aliasing two runtime instances in one save slot.
        let other = (0..world.entities.len())
            .find(|&index| index != entity)
            .unwrap();
        let original_id = world.entities[other].id;
        world.entities[other].id = native_id;
        assert!(world.brush_model_hit(owner, false, [10000.; 3]).is_err());
        assert!(world.model_paint[&entity].painted);
        assert_eq!(world.instance_changes.get(native_id).unwrap().painted, 1);
        world.entities[other].id = original_id;
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_dbc_prepared_interior_material_receives_projection_anchor() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/DBC_Start_1st.Level",
            &egui::Context::default(),
        )?;
        let entity = world
            .entities
            .iter()
            .position(|entity| entity.name == "dbc_1st_zonea_static.DBC_ZoneD_Toon_05")
            .context("Original white-surface owner missing")?;
        let indices = world.entities[entity]
            .mesh_draws
            .get(&753)
            .context("Original projected interior mesh missing")?
            .clone();
        let mut draws = world.draws.clone();
        let view = look_at_matrix(
            physics::add(world.report.spawn, [0., 2., 5.]),
            world.report.spawn,
            [0., 1., 0.],
        );
        world.apply_spherical_lighting(&mut draws, &view);
        for index in indices {
            let program = draws[index]
                .preview
                .as_ref()
                .context("Missing interior material")?
                .as_ref()
                .map_err(|e| anyhow::anyhow!(e.clone()))?;
            ensure!(
                program.pinned_projection,
                "Fixture must exercise the original projected shader"
            );
            ensure!(draws[index].projection_bound.is_some(),"Final prepared interior shader has no projection anchor; cached mesh bound: {:?}; diagnostics: {:?}",
                world.spherical_bounds.get(&entity).and_then(|bounds|bounds.get(&753)),
                world.report.unsupported.iter().filter(|message|message.contains("Pinned projection") || message.contains("bound hierarchy")).collect::<Vec<_>>());
            assert_eq!(program.texture_slots, vec![0, 1]);
            assert!(program.texture_slots.iter().all(|slot| draws[index]
                .material_images
                .iter()
                .any(|image| image.slot == *slot)));
        }
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_sketch_pass_preserves_transparency_after_counter_publication() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let entity = world
            .entities
            .iter()
            .position(|entity| entity.name == "gremlinvillage_zonea.GV_ZoneA_SM01_Toon 01")
            .context("Original white-gear model missing")?;
        assert!(!world.model_paint[&entity].painted);
        assert!(world.counter_materials[&entity].initialized);
        let mut draws = world.draws.clone();
        world.apply_model_constant_colors(&mut draws)?;
        for (&node, indices) in &world.entities[entity].mesh_draws {
            let stored = world.render_objects[&(entity, node)].constant_colors[1]
                .context("Counter rim missing")?;
            assert_eq!(stored[3], 255, "Initial counter publishes opaque rim state");
            for &index in indices {
                let draw = &draws[index];
                let program = draw.preview.as_ref().unwrap().as_ref().unwrap();
                let object = draw
                    .material_registers
                    .context("Object colors were not applied")?;
                assert_eq!(object[4], stored.map(|v| f32::from(v) / 255.));
                let presented = program.presentation_registers(Some(&object)).unwrap();
                assert_eq!(
                    presented[4],
                    program.pass_constants[1].context("Sketch callback constant missing")?
                );
                assert_eq!(
                    presented[4],
                    [6. / 255., 73. / 255., 250. / 255., 32. / 255.]
                );
                assert_eq!(presented[3], [1.; 4]);
                assert!(presented[4][3] < 1.);
                assert_eq!(
                    world.render_objects[&(entity, node)].constant_colors[1],
                    Some(stored)
                );
            }
        }
        // The retained painted endpoint uses the ordinary shader and must not
        // carry the front-pass fixed constants into painted presentation.
        let endpoints = world.model_paint[&entity]
            .states
            .iter()
            .map(|(&index, states)| (index, states[0].clone()))
            .collect::<Vec<_>>();
        let mut painted = world.draws.clone();
        for (index, draw) in &endpoints {
            painted[*index] = draw.clone();
            world.sketch_draws.remove(index);
        }
        world.apply_spherical_lighting(
            &mut painted,
            &[
                1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
            ],
        );
        world.apply_model_constant_colors(&mut painted)?;
        for (index, _) in endpoints {
            let program = painted[index]
                .preview
                .as_ref()
                .unwrap()
                .as_ref()
                .map_err(|error| anyhow::anyhow!(error.clone()))?;
            assert!(program.pass_constants.iter().all(Option::is_none));
            assert_eq!(painted[index].material_registers.unwrap()[4][3], 1.);
        }
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_visibility_controlled_spawner_receives_generated_lighting() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let entity = world
            .entities
            .iter()
            .position(|entity| {
                entity.name == "gv_zonea_airlock.spatterpipespawner 01.PipeSpawner_Spatter 01"
            })
            .context("Original visibility-controlled spawner missing")?;
        let hierarchy = world.entity_bound_hierarchy(entity)?.unwrap();
        assert!(hierarchy.nodes.values().any(|node| node
            .controllers
            .iter()
            .any(|controller| controller.class_name == "NiVisController")));
        assert!(world
            .spherical_bindings
            .iter()
            .any(|binding| binding.entity == entity));
        let mut draws = world.draws.clone();
        world.apply_spherical_lighting(
            &mut draws,
            &[
                1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
            ],
        );
        let indices = world.entities[entity]
            .mesh_draws
            .values()
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        assert!(!indices.is_empty());
        for index in indices {
            let program = draws[index]
                .preview
                .as_ref()
                .context("Spawner has no program")?
                .as_ref()
                .map_err(|error| anyhow::anyhow!(error.clone()))?;
            let slot = program
                .first_runtime_sh_slot(draws[index].material.as_ref().unwrap())
                .context("Spawner SH slot missing")?;
            let image = draws[index]
                .material_images
                .iter()
                .find(|image| u32::from(image.slot) == slot)
                .context("Generated spawner lighting map missing")?;
            assert_eq!(
                (image.width, image.height, image.rgba.len()),
                (16, 16, 16 * 16 * 4)
            );
        }
        // Morph modifiers use the exported target envelope, not rest vertices.
        let morph = world
            .entities
            .iter()
            .position(|entity| entity.name == "gremlinvillage_zonea.Pipe_Straight_2m_Spawner_01 01")
            .context("Original morph-controlled spawner missing")?;
        let hierarchy = world.entity_bound_hierarchy(morph)?.unwrap();
        assert!(hierarchy.nodes.values().any(|node| node.morph_bound));
        assert!(
            world.spherical_bounds.contains_key(&morph),
            "Mixed transform/morph model must use its actual frame pose provider"
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_portal_color_controllers_keep_lighting_bounds_without_admitting_unowned_manager_transforms(
    ) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let entity = world
            .entities
            .iter()
            .position(|entity| {
                entity.name
                    == "gremlinvillage_zonea.seq_projector 01.ProjectorPortal_ScreenDistortion"
            })
            .context("Original portal distortion model missing")?;
        let hierarchy = world.entity_bound_hierarchy(entity)?.unwrap();
        assert!(hierarchy.nodes.values().any(|node| node
            .controllers
            .iter()
            .any(|controller| controller.class_name == "NiColorExtraDataController")));
        let mut draws = world.draws.clone();
        world.apply_spherical_lighting(
            &mut draws,
            &[
                1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
            ],
        );
        assert!(world.spherical_bounds.contains_key(&entity));
        let mut generated = 0;
        for &index in world.entities[entity].mesh_draws.values().flatten() {
            let draw = &draws[index];
            let program = draw
                .preview
                .as_ref()
                .context("Portal material missing")?
                .as_ref()
                .map_err(|error| anyhow::anyhow!(error.clone()))?;
            if let Some(slot) = program.first_runtime_sh_slot(draw.material.as_ref().unwrap()) {
                let image = draw
                    .material_images
                    .iter()
                    .find(|image| u32::from(image.slot) == slot)
                    .context("Portal lighting map missing")?;
                assert_eq!(
                    (image.width, image.height, image.rgba.len()),
                    (16, 16, 16 * 16 * 4)
                );
                generated += 1;
            }
            if program.pinned_projection {
                assert!(draw.projection_bound.is_some());
            }
        }
        assert!(generated > 0);
        let valve = world
            .entities
            .iter()
            .position(|entity| entity.name == "gv_zonea_airlock.GV_ZoneF_Valve_Inert01 01")
            .context("Original manager-controlled valve missing")?;
        let hierarchy = world.entity_bound_hierarchy(valve)?.unwrap();
        assert!(hierarchy
            .nodes
            .values()
            .any(|node| node
                .controllers
                .iter()
                .any(
                    |controller| controller.class_name == "NiTransformController"
                        && controller.flags & 0x20 != 0
                )));
        assert!(hierarchy
            .direct_world_bounds(&Default::default(), &BTreeMap::new(), &BTreeMap::new())
            .is_err());
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn authored_mickey_cannot_rearm_jumps_from_vertical_wall() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let mut collision = Collision::with_triangle_owners(
            vec![
                Triangle {
                    p: [[-20., 0., -20.], [20., 0., 20.], [20., 0., -20.]],
                },
                Triangle {
                    p: [[-20., 0., -20.], [-20., 0., 20.], [20., 0., 20.]],
                },
                Triangle {
                    p: [[2., -20., -20.], [2., 20., 20.], [2., 20., -20.]],
                },
                Triangle {
                    p: [[2., -20., -20.], [2., -20., 20.], [2., 20., 20.]],
                },
            ],
            vec![0.; 4],
            vec![0, 0, 1, 1],
        )?;
        collision.set_player_hull(
            world
                .report
                .player_collision
                .as_ref()
                .context("Missing authored Mickey hull")?,
        )?;
        let mut player = physics::Player::new([1.5, 0.1, 0.], world.report.motion.clone());
        for _ in 0..60 {
            player.step(&collision, [0.; 3], false, super::super::STEP);
        }
        ensure!(player.grounded, "Failed to settle original hull");
        collision.set_owner_solid(0, false)?;
        let mut jumps = 0;
        let mut wall_ground_ticks = Vec::new();
        for tick in 0..180 {
            player.step(
                &collision,
                [1., 0., 0.],
                (tick % 12 == 0, tick % 12 < 6),
                super::super::STEP,
            );
            jumps += usize::from(
                player.ground_jump_started
                    || player.air_first_jump_started
                    || player.second_jump_started,
            );
            if player.grounded {
                wall_ground_ticks.push((tick, player.position));
            }
        }
        ensure!(
            wall_ground_ticks.is_empty(),
            "Vertical wall created support: {wall_ground_ticks:?}"
        );
        ensure!(
            jumps <= 2,
            "Vertical wall rearmed {jumps} jumps without ground contact"
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn native_lua_ability_grants_and_revocation_share_world_state() -> Result<()> {
        fn until(world: &mut World, ready: impl Fn(&World) -> bool) -> Result<()> {
            // Native swap-removal can defer the newly appended console task.
            // This is a test timeout, not a gameplay scheduling/order change.
            for _ in 0..90 {
                if ready(world) {
                    return Ok(());
                }
                world.advance_models(world.animation_time + super::super::STEP)?;
            }
            ensure!(
                ready(world),
                "Ability task did not finish: {:?}",
                world.report.unsupported
            );
            Ok(())
        }
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        world.run_script("EnableAbility_Player(GetPlayer(), 'Brush', 0)")?;
        until(&mut world, |world| {
            world.player_abilities["Brush"].owned == [true, true]
        })?;
        assert!(world.player_ability_enabled("Brush", super::super::brush::Ink::Paint)?);
        assert_eq!(world.player_abilities["Brush"].owned, [true, true]);
        assert_eq!(world.player_abilities["Brush"].upgrades, [0, 0]);
        world.player_abilities.get_mut("Brush").unwrap().disabled = true;
        world.run_script("DisableAbility_Player(GetPlayer(), 'Brush')")?;
        until(&mut world, |world| {
            world.player_abilities["Brush"].owned == [false, false]
        })?;
        assert_eq!(world.player_abilities["Brush"].owned, [false, false]);
        assert!(world.player_abilities["Brush"].disabled);
        assert!(!world.player_ability_enabled("Brush", super::super::brush::Ink::Paint)?);
        world.run_script(
            "local enable=EnableAbility_Player; wait(0); enable(GetPlayer(), 'Brush', -2.75)",
        )?;
        until(&mut world, |world| {
            world.player_abilities["Brush"].upgrades == [-2, -2]
        })?;
        assert_eq!(world.player_abilities["Brush"].owned, [true, true]);
        assert_eq!(world.player_abilities["Brush"].upgrades, [-2, -2]);
        assert!(world.player_abilities["Brush"].disabled);
        ensure!(
            !world
                .report
                .unsupported
                .iter()
                .any(|s| s.contains("runtime animation console")),
            "Lua callback failed"
        );
        // A registered common checkpoint field must not bypass an unsupported
        // derived gameplay receiver, including revocation, after registry expansion.
        world.player_abilities.get_mut("Slide").unwrap().owned = [true, true];
        world.run_callback_fixture(
            "local ok=pcall(DisableAbility_Player,GetPlayer(),'Slide'); assert(not ok)",
        )?;
        assert_eq!(world.player_abilities["Slide"].owned, [true, true]);
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn production_world_host_observes_mutation_before_lua_returns() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        world.scripts.bind_world_api("ProbeGrant")?;
        world.scripts.bind_world_api("ProbeOwned")?;
        world.run_script("local grant=ProbeGrant; local owned=ProbeOwned; grant(); assert(owned()); wait(0); assert(owned())")?;
        for time in [0., 1.] {
            world.with_script_world_host(
                |world, _, name, _| {
                    let owned = match name {
                        "ProbeGrant" => world
                            .player_give_command("Give Brush All 1")
                            .map_err(mlua::Error::external)?,
                        "ProbeOwned" => world.player_abilities["Brush"].owned == [true, true],
                        _ => return Err(mlua::Error::external("Unknown diagnostic API")),
                    };
                    Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Boolean(
                        owned,
                    )]))
                },
                |scripts| {
                    scripts.advance(time)?;
                    Ok(())
                },
            )?;
        }
        assert_eq!(world.player_abilities["Brush"].owned, [true, true]);
        assert!(
            !world
                .scripts
                .errors
                .iter()
                .any(|e| e.contains("runtime animation console")),
            "{:?}",
            world.scripts.errors
        );
        // An error boundary must restore the scheduler without rebuilding VM.
        let error: Result<()> = world.with_script_world_host(
            |_, _, _, _| Ok(mlua::MultiValue::new()),
            |_| anyhow::bail!("Diagnostic boundary failure"),
        );
        assert!(error.is_err());
        world.scripts.advance(2.)?;
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_collectible_lua_commands_counts_and_header_restore_use_one_owner() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        ensure!(
            world.collectibles.entries.len() == 153,
            "Original collectible count changed"
        );
        let film = world
            .collectibles
            .entries
            .iter()
            .find(|e| e.kind == "Film_Reel")
            .context("No film reel")?
            .name
            .clone();
        let quest = world
            .collectibles
            .entries
            .iter()
            .find(|e| e.kind == "Quest_Object")
            .context("No quest item")?
            .name
            .clone();
        let before = world.collectibles.notifications.len();
        world.run_callback_fixture(&format!("assert(HasCollectible({film:?})==false); assert(select('#',GiveCollectible({film:?}))==0); GiveCollectible({film:?}); assert(HasCollectible({film:?})==true); IncrementCollectibleCount({film:?}); GiveCollectible({quest:?}); assert(HasCollectible({quest:?})==true); assert(HasCollectible({:?})==false); assert(HasCollectible(nil)==false); GiveCollectible('BwxUnknownCollectible'); IncrementCollectibleCount('BwxUnknownCollectible')",film.to_ascii_lowercase()))?;
        let film_index = world
            .collectibles
            .entries
            .iter()
            .position(|e| e.name == film)
            .unwrap();
        ensure!(
            world.collectibles.entries[film_index].count == 2,
            "Give must not repeatedly increment ownership"
        );
        let requests = &world.collectibles.notifications[before..];
        ensure!(
            requests.len() == 3
                && requests[0].event == "UI_NewFilmReceived"
                && requests[2].event == "UI_NewItemReceived",
            "Wrong native notification requests"
        );
        ensure!(
            requests[0].route == super::super::collectibles::Route::CollectibleQueue
                && world.ui_notifications.last().is_some_and(|n| n.route
                    == super::super::collectibles::Route::UiEventManager
                    && n.event == "UI_NewFilmReceived"),
            "Lost native queue distinction"
        );
        let mut header = vec![0; super::super::save_game::HEADER_SIZE];
        header[..4].copy_from_slice(b"GAME");
        header[4..8].copy_from_slice(&16u32.to_be_bytes());
        world.collectibles.write_save_header(&mut header)?;
        ensure!(header[0x1d8 + film_index] == 2, "Wrong native saved count");
        header[0x1d8 + film_index] = 255;
        world.collectibles.restore_save_header(&header, true)?;
        world.run_callback_fixture(&format!("assert(HasCollectible({film:?})==false)"))?;
        ensure!(
            world.collectibles.entries[film_index].count == -1,
            "Saved count must sign extend"
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_extras_database_and_collectible_setup_use_live_native_flags() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        ensure!(
            world.extras.entries.len() == 91,
            "Original extras count changed"
        );
        // Exercise the original developer callback on the actual placed unlock
        // pickup, not a fabricated prefab table or a replacement Lua function.
        world.run_callback_fixture("local p=GetEntity('gv_zonea_airlock.pickup_collectible 01.Pin'); assert(p~=nil); local d=GetPrefabData(p); assert(d.CollectibleType=='Unlock'); assert(d.EndFunction=='GV_ZoneA_ConceptArtCheckPoint'); local before=GetGlobal(d.CollectibleName); local ok,why=pcall(ObtainCollectible,p); if not ok then assert(string.find(tostring(why),'SaveGameToMemory'), tostring(why)) end; assert(PlayerHasUnlockedExtra(d.CollectibleName)==true); if before~=nil then assert(GetGlobal(d.CollectibleName)==1) end")?;
        let sample = world.extras.entries[0].name.clone();
        ensure!(
            world.extras.entries[0].kind == "Cartoon"
                && sample == "AL_visit1_Cyrano_for_Pirate_Happy",
            "Incorrect CLB field order"
        );
        ensure!(
            !world
                .scripts
                .errors
                .iter()
                .any(|message| message.contains("PlayerHasUnlockedExtra")),
            "{:?}",
            world.scripts.errors
        );
        ensure!(
            !world
                .report
                .unsupported
                .iter()
                .any(|message| message.contains("PlayerHasUnlockedExtra")),
            "Original collectible setup still failed"
        );
        let check = |world: &mut World, source: &str| -> Result<()> {
            let chunk = crate::lua_cook::compile(source)?;
            world.with_script_world_host(World::script_world_api, |scripts| {
                scripts.ensure_global_module(source, &chunk)
            })
        };
        check(&mut world,&format!("assert(PlayerHasUnlockedExtra({sample:?}) == false); assert(select('#',PlayerHasUnlockedExtra('not-present')) == 1); assert(PlayerHasUnlockedExtra('not-present') == false); assert(PlayerHasUnlockedExtra(nil) == false)"))?;
        let notifications = world.ui_notifications.len();
        world.run_callback_fixture(&format!("assert(select('#',SetUnlockedExtra({:?},false))==0); assert(PlayerHasUnlockedExtra({sample:?})==true); assert(PlayerHasUnlockedExtra({:?})==false)",format!(" {sample} "),format!(" {sample} ")))?;
        ensure!(
            world.extras.entries[0].secondary_flag,
            "Native extra grant did not set both flags"
        );
        ensure!(
            world.ui_notifications.len() == notifications,
            "False optional argument must suppress notification, not ownership"
        );
        if let Some(value) = world.globals.get(&sample) {
            ensure!(value == 1, "Extra grant did not update its named global");
        }
        world.run_callback_fixture(&format!(
            "SetUnlockedExtra({sample:?}); SetUnlockedExtra('BwxUnknownExtra')"
        ))?;
        ensure!(
            world.ui_notifications.len() == notifications + 1,
            "Unknown extra fabricated a UI request"
        );
        let request = world.ui_notifications.last().unwrap();
        ensure!(
            request.event == "UI_NewFilmReceived"
                && request.icon == r"HUDElements\Icons\ui_icon_unlockable_film_tex.nif",
            "Wrong native extra notification"
        );
        let mut flags = vec![0; super::super::save_game::HEADER_SIZE];
        flags[..4].copy_from_slice(b"GAME");
        flags[4..8].copy_from_slice(&16u32.to_be_bytes());
        flags[0x158] = 3;
        world.extras.restore_save_header(&flags)?;
        check(&mut world,&format!("assert(PlayerHasUnlockedExtra({sample:?}) == true); assert(PlayerHasUnlockedExtra({:?}) == false)",sample.to_ascii_lowercase()))?;
        flags[0x158] = 0;
        world.extras.restore_save_header(&flags)?;
        ensure!(
            world.extras.has(&sample),
            "Restore revoked a native persistent unlock"
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_world_captures_live_header_owners_without_fabricating_checkpoint() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        // Mutate actual owned state so this cannot pass by copying a native save.
        world.game_options.sfx_volume = 27;
        world.game_options.music_volume = 63;
        world.game_options.left_handed = 1;
        world.playthrough.value = -17;
        world.collectibles.entries[0].count = -3;
        let extra = world.extras.entries[0].name.clone();
        world
            .extras
            .unlock(&extra)
            .context("Missing original extra")?;
        let mut header = vec![0xa5; 0x400];
        super::super::save_game::initialize_header(&mut header)?;
        let before = header.clone();
        world.write_save_header_owners(&mut header)?;
        assert_eq!(
            super::super::save_options::State::from_header(&header)?,
            world.game_options
        );
        assert_eq!(header[0x1d8], 253);
        assert_ne!(header[0x158] & 1, 0);
        for index in 0..header.len() {
            if !matches!(index,0x10..=0xc3|0x100..=0x13b|0x148..=0x153|0x158..=0x2b0|0x2b4|0x2b5|0x2b8..=0x2c3|0x310..=0x31f)
            {
                assert_eq!(header[index], before[index], "Unowned byte {index:#x}");
            }
        }
        assert_eq!(
            super::super::score_state::Playthrough::from_header(&header)?.value,
            -17
        );
        assert!(
            super::super::save_game::Archive::parse(&header).is_err(),
            "Header owner capture must not masquerade as a complete checkpoint"
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn production_level_loader_handles_convex_only_jigsaws_and_stale_player_hull() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let levels = available_levels(Path::new(&root))?;
        for level in [
            "Levels/DBC_Start_1st.Level",
            "Levels/GV_ZoneB_Start.level",
            "Levels/ShadowBlot_Boss.level",
            "Levels/BuildingStandards.level",
            "Levels/TL_Start.Level",
            "Levels/MJM_ZoneA.Level",
            "Levels/2D_HauntedHouse01.level",
            "Levels/HM_Start.Level",
            "Levels/TL_SpaceMountain.Level",
            "Levels/HM_StartI.Level",
            "Levels/ToonTown_V3.level",
            "Levels/TL_SpaceMountain_TronPete.Level",
            "Levels/PotW_Start.Level",
            "Levels/MeanStreet_V1.level",
            "Levels/SW_ClockWorkBoss_static.Level",
            "Levels/HM_StartM.Level",
            "Levels/PotW_Skull_Start.Level",
        ] {
            ensure!(
                levels.iter().any(|path| path.eq_ignore_ascii_case(level)),
                "Picker omitted {level}"
            );
            let mut world = load(Path::new(&root), level, &egui::Context::default())
                .with_context(|| format!("Production level regression {level}"))?;
            world.attach_player_body()?;
            super::super::authored_preview::Preview::new(&mut world)
                .with_context(|| format!("Start gameplay animation {level}"))?;
            ensure!(
                !world.draws.is_empty() && !world.report.collision_parts.is_empty(),
                "Empty production world {level}"
            );
        }
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn preview_recovers_failed_entrance_without_overriding_working_cutscenes() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        assert!(world.screen_fade.out || world.cutscene_input.restricted);
        // Retail entrance coroutines yield before hitting unsupported APIs;
        // project builds may fail during the synchronous map-load stage.
        let mut recovered = world.recover_failed_preview_script(0);
        for tick in 1..=90 {
            if recovered {
                break;
            }
            let since = world.report.unsupported.len();
            world.advance_models(tick as f32 / 30.)?;
            recovered = world.recover_failed_preview_script(since);
        }
        assert!(recovered, "{:?}", world.report.unsupported);
        assert_eq!(world.screen_fade.opacity, 0.);
        assert!(!world.cutscene_input.restricted);
        assert!(!world.script_player.script_input_restricted);
        assert!(!world.hidden_entities.contains(&world.player_entity));
        let other = (0..world.entities.len())
            .find(|&entity| {
                entity != world.player_entity && !world.hidden_entities.contains(&entity)
            })
            .context("No independent visibility target")?;
        world.set_entity_hidden(other, true)?;
        world.set_entity_hidden(world.player_entity, true)?;
        let hidden_since = world.report.unsupported.len();
        assert!(
            !world.recover_failed_preview_script(hidden_since),
            "A valid player hide was overridden"
        );
        world.report.unsupported.push(
            "Authored IO test: Mickey_Projector_Exit: Missing authored script callback Missing".into(),
        );
        assert!(!world.recover_failed_preview_script(hidden_since));
        assert!(world.hidden_entities.contains(&world.player_entity));
        world.report.unsupported.push("Game Lua: Mickey_Projector_Exit: interrupted executing callback".into());
        assert!(world.recover_failed_preview_script(hidden_since));
        assert!(!world.hidden_entities.contains(&world.player_entity));
        assert!(
            world.hidden_entities.contains(&other),
            "Projector recovery unhid an unrelated entity"
        );
        let since = world.report.unsupported.len();
        world.cutscene_input.restrict();
        world.screen_fade.start_out(1.)?;
        assert!(!world.recover_failed_preview_script(since));
        assert!(world.cutscene_input.restricted && world.screen_fade.out);
        world.report.unsupported.push("Authored IO test: OnMapLoad -> entrance: MoveToEntity requires character relocation effects".into());
        assert!(
            world.recover_failed_preview_script(since),
            "External IO errors must release an abandoned entrance fade"
        );
        assert_eq!(world.screen_fade.opacity, 0.);
        assert!(!world.cutscene_input.restricted);
        let since = world.report.unsupported.len();
        world.cutscene_input.restrict();
        world.screen_fade.start_out(1.)?;
        assert!(
            !world.recover_failed_preview_script(since),
            "Old errors must not override a later valid cutscene"
        );
        world
            .report
            .unsupported
            .push("Game Lua: test interrupted sequence".into());
        assert!(world.recover_failed_preview_script(since));
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn retail_catalog_distinguishes_shipped_levels_from_stale_descriptors() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let catalog = level_catalog(Path::new(&root))?;
        for path in [
            "Levels/DBC_Start_1st.Level",
            "Levels/PotW_Start.Level",
            "Levels/MJM_ZoneA.Level",
            "Levels/MeanStreet_V1.level",
            "Levels/HM_Start.level",
            "Levels/ShadowBlot_Boss.level",
        ] {
            let entry = catalog
                .iter()
                .find(|entry| entry.path.eq_ignore_ascii_case(path))
                .context(path.to_owned())?;
            ensure!(
                entry.unavailable.is_none(),
                "{path}: {:?}",
                entry.unavailable
            );
        }
        let stale = catalog
            .iter()
            .find(|entry| {
                entry
                    .path
                    .eq_ignore_ascii_case("Levels/SW_SteamA_Start.level")
            })
            .context("Expected retained development descriptor")?;
        ensure!(
            stale
                .unavailable
                .as_deref()
                .is_some_and(|reason| reason.contains("stream_5.part")),
            "Missing original part must be reported instead of presented as a loadable level: {:?}",
            stale.unavailable
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_visibility_removes_physics_without_erasing_body_state() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        // Initial map-load Hide requests precede dynamic admission.
        for (&source, &owner) in &world.dynamic_runtime_sources {
            let entity = world.dynamic_bodies[source].entity;
            assert_eq!(
                world.collision.owner_in_world(owner)?,
                !world.hidden_entities.contains(&entity)
            );
        }
        let (&source, &owner) = world
            .dynamic_runtime_sources
            .iter()
            .find(|(source, _)| {
                !world
                    .hidden_entities
                    .contains(&world.dynamic_bodies[**source].entity)
            })
            .context("No visible admitted rigid body")?;
        let entity = world.dynamic_bodies[source].entity;
        let shape = world.dynamic_constructions[&source].bodies[0].shapes[0];
        let mode = world.dynamic_constructions[&source].bodies[0]
            .motion
            .current
            .motion_type();
        let filter = world.collision.simulation_filter(shape);
        // Use its actual placed pose: relocating an attached owner would
        // test the still-incomplete child-relocation path instead of hiding.
        let before = world.collision.simulation_pose(shape)?;
        let bounds = (
            before.position.map(|v| v - 50.),
            before.position.map(|v| v + 50.),
        );
        assert!(world
            .collision
            .overlap_candidates(bounds.0, bounds.1)?
            .iter()
            .any(|hit| hit.owner == owner));
        world.run_callback_fixture(&format!(
            "Hide(GetEntity({:?}))",
            world.entities[entity].name
        ))?;
        assert!(world.hidden_entities.contains(&entity));
        assert!(!world.collision.owner_in_world(owner)?);
        {
            let state = world
                .dynamic_constructions
                .get_mut(&source)
                .context("Missing activation fixture")?;
            let component = world.entities[entity]
                .components
                .iter_mut()
                .find(|component| {
                    component.link_id.to_u32() == world.dynamic_bodies[source].component
                })
                .context("Missing activation component")?;
            let records = super::super::body_activation::PreAdmission {
                bodies: &mut state.bodies,
                component,
                gravity_factor_word: 0x3f800000,
            };
            let backend = super::super::body_activation::Admitted {
                records,
                world: &mut world.collision,
                materials: &state.materials,
            };
            assert!(
                !super::super::body_activation::System::in_world(&backend, 0)?,
                "Activation treated a hidden body as admitted"
            );
        }
        assert!(world
            .collision
            .overlap_candidates(bounds.0, bounds.1)?
            .iter()
            .all(|hit| hit.owner != owner));
        for _ in 0..3 {
            world.advance_dynamic_physics(1. / 30.)?;
        }
        assert_eq!(
            world.collision.simulation_pose(shape)?.position,
            before.position
        );
        world.run_callback_fixture(&format!(
            "Unhide(GetEntity({:?}))",
            world.entities[entity].name
        ))?;
        assert!(!world.hidden_entities.contains(&entity));
        assert!(world.collision.owner_in_world(owner)?);
        assert_eq!(world.collision.simulation_filter(shape), filter);
        assert_eq!(
            world.dynamic_constructions[&source].bodies[0]
                .motion
                .current
                .motion_type(),
            mode
        );
        assert!(world
            .collision
            .overlap_candidates(bounds.0, bounds.1)?
            .iter()
            .any(|hit| hit.owner == owner));
        // Also exercise a real static owner, whose render and physical identity
        // must be restored without rebuilding or modifying source geometry.
        let (entity, owner) = world
            .report
            .collision_parts
            .iter()
            .filter_map(|part| {
                let target = world.collision_entity(part.owner)?;
                let entity = world
                    .entities
                    .iter()
                    .position(|candidate| std::ptr::eq(candidate, target))?;
                (target
                    .components
                    .iter()
                    .any(|c| c.class_name == "PhysicsRigidBodyStaticComponent")
                    && !world.hidden_entities.contains(&entity)
                    && world.collision.owner_shapes(part.owner).next().is_some())
                .then_some((entity, part.owner))
            })
            .next()
            .context("No static visibility fixture")?;
        world.set_entity_hidden(entity, true)?;
        assert!(!world.collision.owner_in_world(owner)?);
        world.set_entity_hidden(entity, false)?;
        assert!(world.collision.owner_in_world(owner)?);
        println!(
            "Static visibility fixture: {} at {:?}",
            world.entities[entity].name,
            &world.entities[entity].transform[12..15]
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_visibility_is_reversible_instance_state_and_preserves_thinned_geometry(
    ) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/DBC_Start_1st.Level",
            &egui::Context::default(),
        )?;
        let player = world.player_entity;
        let source_draws = world.draws.len();
        let source_triangles = world.collision.triangles.len();
        let skin = world.player_draws[0];
        world.hidden_jigsaw_draws.insert(skin);
        world.run_script("assert(select('#',Hide(GetPlayer()))==0); Hide('missing-visibility-target'); Wait(0); Unhide(GetPlayer())")?;
        world.advance_models(0.)?;
        assert!(world.hidden_entities.contains(&player));
        assert!(world
            .player_draws
            .iter()
            .all(|draw| world.hidden_entity_draws.contains(draw)));
        assert!(world
            .scripts
            .resolve_name(&world.entities[player].name)
            .is_some());
        world.advance_models(0.)?;
        assert!(!world.hidden_entities.contains(&player));
        assert!(world
            .player_draws
            .iter()
            .all(|draw| !world.hidden_entity_draws.contains(draw)));
        assert!(
            world.hidden_jigsaw_draws.contains(&skin),
            "Unhide restored an independently thinned draw"
        );
        assert_eq!(world.draws.len(), source_draws);
        assert_eq!(world.collision.triangles.len(), source_triangles);
        // Use an actual imported parent/child pair to check recursive state.
        let (&child, local) = world
            .transform_hierarchy
            .iter()
            .next()
            .context("Expected Castle child hierarchy")?;
        let parent = local.parent;
        world.set_entity_hidden(parent, true)?;
        assert!(world.hidden_entities.contains(&child));
        world.set_entity_hidden(parent, false)?;
        assert!(!world.hidden_entities.contains(&child));
        // Visibility is an entity relationship, not a successful pose-cache
        // binding. Bone/rotation bindings can be pending while hiding works.
        world.transform_hierarchy.remove(&child);
        world.set_entity_hidden(parent, true)?;
        assert!(world.hidden_entities.contains(&child));
        world.set_entity_hidden(parent, false)?;
        assert!(!world.hidden_entities.contains(&child));
        assert!(
            !world
                .report
                .unsupported
                .iter()
                .any(|line| line.contains("runtime animation console")),
            "{:?}",
            world.report.unsupported
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_actor_idle_gate_freezes_only_its_clock_and_resumes_without_catchup() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/DBC_Start_1st.Level",
            &egui::Context::default(),
        )?;
        let entity = *world
            .actors
            .iter()
            .find(|(_, actor)| actor.clock.use_delta_time)
            .context("Expected delta-time actor")?
            .0;
        let other = *world
            .actors
            .iter()
            .find(|(index, actor)| **index != entity && actor.clock.use_delta_time)
            .context("Expected second actor")?
            .0;
        let name = serde_json::to_string(&world.entities[entity].name)?;
        let other_name=serde_json::to_string(&world.entities[other].name)?;
        // Both comparison clocks must start awake; authored camera actors may
        // legitimately load idle. The test below isolates a later single-owner
        // pause, rather than assuming the unrelated actor's startup state.
        world.run_script(&format!("assert(select('#',AnimGBSetIdle(GetEntity({name}), false))==0); AnimGBSetIdle(GetEntity({other_name}),false); AnimGBSetIdle('BwxMissingActor', true)"))?;
        world.advance_models(0.1)?;
        let actor=world.actors.get(&entity).with_context(||format!("Clock-test actor {} disappeared (retired={}): {:?}",world.entities[entity].name,world.removed_entities.contains(&entity),world.report.unsupported.iter().filter(|s|s.contains(&world.entities[entity].name)).collect::<Vec<_>>()))?;
        let clock = actor.clock.initial_update_time();
        let active = world.actors[&entity].active_id();
        let other_clock = world.actors[&other].clock.initial_update_time();
        world.run_script(&format!("AnimGBSetIdle(GetEntity({name}), true)"))?;
        for time in [0.2, 0.4] {
            world.advance_models(time)?;
            assert_eq!(world.actors[&entity].clock.initial_update_time(), clock);
        }
        assert!(world.idle_actors.contains(&entity));
        assert_eq!(world.actors[&entity].active_id(), active);
        assert!(
            world.actors[&other].clock.initial_update_time() > other_clock,
            "Idle paused unrelated actor clocks"
        );
        world.run_script(&format!("AnimGBSetIdle(GetEntity({name}), 'false')"))?;
        world.advance_models(0.5)?;
        assert!(!world.idle_actors.contains(&entity));
        let resumed = world.actors[&entity].clock.initial_update_time();
        ensure!(
            (resumed - (clock + 0.1)).abs() < 0.000001,
            "Idle duration incorrectly accumulated: {clock} -> {resumed}"
        );
        assert!(
            !world
                .report
                .unsupported
                .iter()
                .any(|line| line.contains("runtime animation console")),
            "{:?}",
            world.report.unsupported
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn demo_ability_setup_grants_both_channels_without_bypassing_script_disables() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        assert!(!world.player_ability_enabled("Brush", super::super::brush::Ink::Paint)?);
        world.enable_demo_abilities()?;
        for name in ["Brush", "Blast", "Spin"] {
            for ink in [
                super::super::brush::Ink::Paint,
                super::super::brush::Ink::Thinner,
            ] {
                assert!(world.player_ability_enabled(name, ink)?);
            }
        }
        world.player_abilities.get_mut("Brush").unwrap().disabled = true;
        world.enable_demo_abilities()?;
        assert!(!world.player_ability_enabled("Brush", super::super::brush::Ink::Paint)?);
        assert!(!world.player_ability_enabled("Brush", super::super::brush::Ink::Thinner)?);
        assert_eq!(
            world.ability_hud_requests.len(),
            1,
            "Repeated setup must not duplicate HUD requests"
        );
        assert!(
            !world.diagnostic_pickup_commands,
            "Demo setup must not claim pickup collection occurred"
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn production_pickup_commands_update_channel_ownership_and_preserve_ui_requests() -> Result<()>
    {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let owner = world
            .scripts
            .resolve_name("gremlinvillage_zonea_dynamic.Ability_AllTheBasics")
            .context("Startup pickup missing")?;
        let commands = world.pickup_receivers[&owner].commands.clone();
        for command in &commands {
            assert!(world.player_give_command(command)?);
        }
        for name in ["Brush", "Blast", "Spin"] {
            assert_eq!(world.player_abilities[name].owned, [true, true]);
            assert_eq!(world.player_abilities[name].upgrades, [1, 1]);
        }
        assert_eq!(
            world.ability_hud_requests,
            [("Show", "HudElement", "AmmoBars")]
        );
        assert!(world.player_give_command("Give Blast All 1")?);
        assert_eq!(
            world.ability_hud_requests.len(),
            1,
            "Repeated enable must not invent HUD transitions"
        );
        world.player_abilities.get_mut("Blast").unwrap().owned = [false, false];
        assert!(world.player_give_command("Give Blast Paint 1")?);
        assert!(world.player_give_command("Give Blast Thinner 2")?);
        assert_eq!(
            world.ability_hud_requests.len(),
            3,
            "Thinner grant must test prior Thinner ownership"
        );
        let maximum = world.authored_health[&world.player_entity].maximum;
        world
            .runtime_health
            .get_mut(&world.player_entity)
            .unwrap()
            .health = 1;
        assert!(world.player_give_command("Give Health 2")?);
        assert_eq!(
            world.runtime_health[&world.player_entity].health,
            3.min(maximum)
        );
        assert!(world.player_give_command("Give Health 10")?);
        assert_eq!(world.runtime_health[&world.player_entity].health, maximum);
        let property = world.entities[world.player_entity]
            .components
            .iter()
            .find(|c| c.class_name == "HealthComponent")
            .unwrap()
            .properties
            .iter()
            .find(|p| p.name == "Health")
            .unwrap();
        assert_eq!(property.value, serde_json::json!(maximum));
        assert!(world.player_give_command("Give Health -3")?);
        assert_eq!(
            world.runtime_health[&world.player_entity].health, maximum,
            "Negative healing damaged the player"
        );
        world
            .runtime_health
            .get_mut(&world.player_entity)
            .unwrap()
            .health = 0;
        assert!(world.player_give_command("Give Health 10")?);
        assert_eq!(
            world.runtime_health[&world.player_entity].health, 0,
            "Health pickup resurrected a dead recipient"
        );
        assert!(world
            .player_give_command("Give MissingAction All 1")
            .is_err());
        assert!(!world.player_give_command("Add Paint 1")?);
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn production_pickup_receives_inventory_overlap_without_inventing_grants() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let owner = world
            .scripts
            .resolve_name("gremlinvillage_zonea_dynamic.Ability_AllTheBasics")
            .context("Startup pickup missing")?;
        assert_eq!(
            world.pickup_receivers[&owner].commands,
            ["Give Brush All 1", "Give Blast All 1", "Give Spin All 1"]
        );
        let inventory = world.player_entity;
        let other = world
            .entities
            .iter()
            .position(|entity| {
                !entity
                    .components
                    .iter()
                    .any(|c| c.class_name == "InventoryComponent")
            })
            .context("Expected original non-inventory visitor")?;
        world.pickup_overlap(owner, Some(other), true)?;
        assert_eq!(
            world.pickup_receivers[&owner].collector(|index| index < world.entities.len()),
            None
        );
        world.pickup_overlap(owner, Some(inventory), true)?;
        world.pickup_overlap(owner, Some(other), false)?;
        assert_eq!(
            world.pickup_receivers[&owner].collector(|index| index < world.entities.len()),
            Some(inventory)
        );
        world.pickup_overlap(owner, Some(inventory), false)?;
        assert_eq!(
            world.pickup_receivers[&owner].collector(|index| index < world.entities.len()),
            None
        );
        assert!(world
            .pickup_overlap(owner, Some(world.entities.len()), true)
            .is_err());
        ensure!(
            world
                .report
                .unsupported
                .iter()
                .any(|s| s.contains("pickup lifecycle remain unimplemented")),
            "No complete-pickup claim"
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn production_pickup_resolves_physics_ownership_before_entity_dispatch() -> Result<()> {
        use super::super::collidable_owner::{Collidable, Records, UserData};
        struct RecordsFixture;
        impl Records for RecordsFixture {
            fn collidable(&self, address: usize) -> Result<Collidable> {
                ensure!(address == 108, "Unexpected collidable");
                Ok(Collidable {
                    has_shape: true,
                    owner_offset: -8,
                })
            }
            fn user_data(&self, owner: usize) -> Result<UserData> {
                Ok(match owner {
                    200 => UserData::Entity(91),
                    100 => UserData::Entity(92),
                    _ => anyhow::bail!("Missing physics owner"),
                })
            }
        }
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let owner = world
            .scripts
            .resolve_name("gremlinvillage_zonea_dynamic.Ability_AllTheBasics")
            .context("Startup pickup missing")?;
        let inventory = world.player_entity;
        let resolve = |pointer| {
            Ok(match pointer {
                91 => Some(owner),
                92 => Some(inventory),
                _ => None,
            })
        };
        world.pickup_phantom_overlap(200, 108, true, &RecordsFixture, resolve)?;
        assert_eq!(
            world.pickup_receivers[&owner].collector(|i| i < world.entities.len()),
            Some(inventory)
        );
        world.pickup_phantom_overlap(200, 999, true, &RecordsFixture, resolve)?;
        world.pickup_phantom_overlap(200, 108, false, &RecordsFixture, resolve)?;
        assert_eq!(
            world.pickup_receivers[&owner].collector(|i| i < world.entities.len()),
            None
        );
        world.pickup_phantom_overlap(200, 999, false, &RecordsFixture, resolve)?;
        assert!(world
            .pickup_phantom_overlap(200, 108, true, &RecordsFixture, |_| Ok(None))
            .is_err());
        struct Bindings {
            owner: usize,
            inventory: usize,
        }
        impl Records for Bindings {
            fn collidable(&self, a: usize) -> Result<Collidable> {
                RecordsFixture.collidable(a)
            }
            fn user_data(&self, a: usize) -> Result<UserData> {
                RecordsFixture.user_data(a)
            }
        }
        impl super::super::pickup_listener::Bindings for Bindings {
            fn entity_identity(&self, p: usize) -> Result<Option<usize>> {
                Ok(match p {
                    91 => Some(self.owner),
                    92 => Some(self.inventory),
                    _ => None,
                })
            }
        }
        let phantom = world.size_phantom_handles[&owner];
        let listener = world.physics_objects.allocate_pickup_listener()?;
        world
            .physics_objects
            .phantoms
            .get_mut(&phantom)
            .unwrap()
            .overlaps
            .register(Some(listener));
        let bindings = Bindings { owner, inventory };
        assert!(world.dispatch_pickup_phantom_overlap(phantom, 108, true, &bindings)?);
        assert_eq!(
            world.pickup_receivers[&owner].collector(|i| i < world.entities.len()),
            Some(inventory)
        );
        assert!(world.physics_objects.phantoms[&phantom]
            .overlaps
            .contains(108));
        // Ignored visitors still enter the phantom's membership array; pickup
        // callbacks do not reject the event and must not decode visitor999.
        assert!(world.dispatch_pickup_phantom_overlap(phantom, 999, true, &bindings)?);
        assert!(world.dispatch_pickup_phantom_overlap(phantom, 108, false, &bindings)?);
        assert_eq!(
            world.pickup_receivers[&owner].collector(|i| i < world.entities.len()),
            None
        );
        assert!(world.dispatch_pickup_phantom_overlap(phantom, 999, false, &bindings)?);
        assert!(world.physics_objects.phantoms[&phantom]
            .overlaps
            .collidables
            .is_empty());
        assert!(world
            .dispatch_pickup_phantom_overlap(phantom, 999, true, &bindings)
            .is_err());
        assert!(world.physics_objects.phantoms[&phantom]
            .overlaps
            .collidables
            .is_empty());
        // Read the current owned binding on each event, never a cached owner.
        world
            .physics_objects
            .bind_phantom_entity(phantom, world.entities.len() + 1)?;
        assert!(world
            .dispatch_pickup_phantom_overlap(phantom, 108, true, &bindings)
            .is_err());
        assert!(world.physics_objects.phantoms[&phantom]
            .overlaps
            .collidables
            .is_empty());
        world
            .physics_objects
            .bind_phantom_entity(phantom, owner + 1)?;
        assert!(world.dispatch_pickup_phantom_overlap(phantom, 108, true, &bindings)?);
        assert_eq!(
            world.pickup_receivers[&owner].collector(|i| i < world.entities.len()),
            Some(inventory)
        );
        let ticket = world
            .scripts
            .resolve_name("gremlinvillage_zonea.ETicket_C_Nif 01")
            .context("Ticket missing")?;
        let resource = world.resource_phantom_handles[&ticket];
        struct Shapeless;
        impl Records for Shapeless {
            fn collidable(&self, _: usize) -> Result<Collidable> {
                Ok(Collidable {
                    has_shape: false,
                    owner_offset: -8,
                })
            }
            fn user_data(&self, _: usize) -> Result<UserData> {
                anyhow::bail!("Shapeless visitor must not reach pickup callbacks")
            }
        }
        impl super::super::pickup_listener::Bindings for Shapeless {
            fn entity_identity(&self, _: usize) -> Result<Option<usize>> {
                anyhow::bail!("Shapeless visitor must not resolve an entity")
            }
        }
        assert!(!world.dispatch_pickup_phantom_overlap(resource, 999, true, &Shapeless)?);
        assert!(!world.dispatch_pickup_phantom_overlap(resource, 999, false, &Shapeless)?);
        assert!(world.physics_objects.phantoms[&resource]
            .overlaps
            .collidables
            .is_empty());
        assert_eq!(
            world.pickup_receivers[&ticket].collector(|i| i < world.entities.len()),
            None
        );
        assert!(world.dispatch_pickup_phantom_overlap(resource, 108, true, &bindings)?);
        assert_eq!(
            world.pickup_receivers[&ticket].collector(|i| i < world.entities.len()),
            Some(inventory)
        );
        assert!(world.dispatch_pickup_phantom_overlap(resource, 108, false, &bindings)?);
        assert_eq!(
            world.pickup_receivers[&ticket].collector(|i| i < world.entities.len()),
            None
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn production_health_retains_original_fields_for_nonmutating_acceptance() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let state = world
            .authored_health
            .get(&world.player_entity)
            .context("Player authored health not imported")?;
        let component = world.entities[world.player_entity]
            .components
            .iter()
            .find(|c| c.class_name == "HealthComponent")
            .context("Player HealthComponent missing")?;
        let original = super::super::health::Authored::from_component(component)?;
        assert_eq!(state.health, original.health);
        assert_eq!(state.maximum, original.maximum);
        assert_eq!(state.infinite, original.infinite);
        let mut probe = state.clone();
        probe.health = 1;
        probe.maximum = 5;
        probe.infinite = false;
        assert!(super::super::pickup::attraction_commands(
            &["Give Health 2".into()],
            |amount| Ok(probe.accepts_pickup(amount, 0))
        )?);
        assert_eq!(probe.health, 1);
        assert!(!probe.accepts_pickup(2, 1));
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn production_inventory_retains_scalar_and_parallel_array_storage() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let imported = world
            .authored_inventory
            .get(&world.player_entity)
            .with_context(|| {
                format!(
                    "Player inventory not imported: {:?}",
                    world
                        .report
                        .unsupported
                        .iter()
                        .filter(|s| s.starts_with("Inventory state"))
                        .collect::<Vec<_>>()
                )
            })?;
        let component = world.entities[world.player_entity]
            .components
            .iter()
            .find(|c| c.class_name == "InventoryComponent")
            .context("Player inventory missing")?;
        assert_eq!(
            imported,
            &super::super::inventory_state::Authored::from_component(component)?
        );
        imported.powerup_values()?;
        let locale = super::super::native_names::initial_table();
        let before = imported.clone();
        let paint = imported.floats["Paint"];
        let maximum = imported.floats.get("PaintMax").copied().unwrap_or(0.);
        assert_eq!(
            imported.preview_add("Paint", 1., &locale)?,
            super::super::inventory_commands::preview_scalar_balance(paint, 1., maximum) != paint
        );
        assert_eq!(imported, &before);
        for name in ["Sketch", "SketchCount", "SketchMax"] {
            let source = component
                .properties
                .iter()
                .find(|p| p.name == name)
                .unwrap()
                .value
                .as_array()
                .unwrap();
            let actual = match name {
                "Sketch" => imported.sketches.len(),
                "SketchCount" => imported.counts.len(),
                _ => imported.maxima.len(),
            };
            assert_eq!(actual, source.len());
        }
        let mut probe = component.clone();
        probe
            .properties
            .iter_mut()
            .find(|p| p.name == "Sketch")
            .unwrap()
            .value = serde_json::json!(["SketchAnvil", "SketchAnvil"]);
        probe
            .properties
            .iter_mut()
            .find(|p| p.name == "SketchCount")
            .unwrap()
            .value = serde_json::json!([-2]);
        probe
            .properties
            .iter_mut()
            .find(|p| p.name == "SketchMax")
            .unwrap()
            .value = serde_json::json!([3., 4., 5.]);
        let preserved = super::super::inventory_state::Authored::from_component(&probe)?;
        assert_eq!(preserved.sketches, vec!["SketchAnvil", "SketchAnvil"]);
        assert_eq!(preserved.counts, vec![-2]);
        assert_eq!(preserved.maxima, vec![3., 4., 5.]);
        assert!(preserved.preview_add("SketchAnvil", 0., &locale)?);
        assert!(preserved.preview_add("sketchAnvil", 0., &locale)?); // family matches, interned name differs.
        assert!(preserved.preview_add("SketchNew", 0., &locale)?);
        assert!(preserved.preview_add("UnknownScalar", 1., &locale).is_err());
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn production_pickup_settings_preserve_resolved_fields_and_constructor_state() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let index = world
            .scripts
            .resolve_name("gremlinvillage_zonea_dynamic.Ability_AllTheBasics")
            .context("Startup pickup missing")?;
        let settings = world.pickup_settings.get(&index).with_context(|| {
            format!(
                "Settings missing: {:?}",
                world
                    .report
                    .unsupported
                    .iter()
                    .filter(|s| s.starts_with("Pickup settings"))
                    .collect::<Vec<_>>()
            )
        })?;
        let component = world.entities[index]
            .components
            .iter()
            .find(|c| c.class_name == "PlayerPickupComponent")
            .unwrap();
        assert_eq!(
            settings,
            &super::super::pickup::Settings::from_component(component)?
        );
        let state = &world.pickup_update_states[&index];
        assert!(!state.attraction.active);
        assert_eq!(state.attraction.wait, 0.);
        assert!(!state.gate.delayed);
        assert_eq!(state.gate.elapsed, 0.);
        assert_eq!(state.bob.phase, 0.);
        assert_eq!(
            state.bob.base_height,
            world.entities[index].authored_transform[13]
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn production_phantom_uses_inherited_size_and_live_unscaled_pose() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let index = world
            .scripts
            .resolve_name("gremlinvillage_zonea_dynamic.Ability_AllTheBasics")
            .context("Original startup pickup not loaded")?;
        let ticket = world
            .scripts
            .resolve_name("gremlinvillage_zonea.ETicket_C_Nif 01")
            .context("Original ticket missing")?;
        let ticket_shapes = world
            .resource_phantom_shapes
            .get(&ticket)
            .context("Ticket resource geometry not imported")?;
        assert_eq!(ticket_shapes.len(), 1);
        assert!(ticket_shapes[0].convex);
        assert_eq!(ticket_shapes[0].vertices.len(), 8);
        let ticket_motion = &world.resource_phantom_motion[&ticket];
        assert_eq!(
            &ticket_motion.transform[12..15],
            &world.entities[ticket].transform[12..15]
        );
        assert_eq!(ticket_motion.transform[15], 0.);
        assert_eq!(ticket_motion.previous_center, ticket_motion.center);
        assert_eq!(
            ticket_motion.previous_orientation,
            ticket_motion.orientation
        );
        assert_eq!(ticket_motion.local_center, [0.; 4]);
        let query = world.resource_phantom_aabb(ticket, 0.)?;
        assert!(query.iter().flatten().all(|v| v.is_finite()));
        for axis in 0..3 {
            assert!(query[0][axis] < ticket_motion.transform[12 + axis]);
            assert!(query[1][axis] > ticket_motion.transform[12 + axis]);
        }
        assert!(world.resource_phantom_aabb(ticket, -1.).is_err());
        let pending = world.phantom_admission_bounds(ticket)?;
        let native = world
            .resource_phantom_aabb(ticket, world.report.physics_world.shape_phantom_tolerance())?;
        assert_eq!(
            pending.min.map(f32::to_bits),
            native[0][..3]
                .iter()
                .map(|v| v.to_bits())
                .collect::<Vec<_>>()
                .as_slice()
        );
        assert_eq!(
            pending.max.map(f32::to_bits),
            native[1][..3]
                .iter()
                .map(|v| v.to_bits())
                .collect::<Vec<_>>()
                .as_slice()
        );
        assert_eq!(
            world.report.physics_world.collision_tolerance.to_bits(),
            0x3dcccccd
        );
        let encoded = world.phantom_admission_sweep_bounds(ticket)?;
        assert_eq!(
            encoded,
            world
                .report
                .physics_world
                .sweep_from_aabb(pending.min, pending.max)?
        );
        assert!(encoded.minimum.iter().all(|value| value & 1 == 0));
        assert!(encoded.maximum.iter().all(|value| value & 1 == 1));
        let ticket_cache = &world.resource_phantom_bounds[&ticket];
        assert!(
            world
                .resource_phantom_bounds
                .values()
                .filter(|cache| Arc::ptr_eq(cache, ticket_cache))
                .count()
                > 1,
            "Shared resource shape bounds must retain a shared mutable cache"
        );
        // Cache keys are resolved archive identities; authored .hkx names may
        // resolve to .hkx_wii and must not merge distinct exact files.
        let hkx_path = world
            .report
            .geometry
            .iter()
            .find(|entry| {
                entry["kind"].as_str() == Some("resource_phantom_shapes")
                    && entry["entity"].as_str() == Some(world.entities[ticket].name.as_str())
            })
            .and_then(|entry| entry["hkx"].as_str())
            .context("Ticket resolved HKX path missing")?
            .to_owned();
        let resource_count = world.phantom_resources.len();
        let shape_count = world.physics_objects.shapes.len();
        // A later initialization batch acquires the existing immutable asset,
        // even with different path casing/separators; it must not decode again.
        let shared = world.phantom_resources.acquire(
            &hkx_path.to_ascii_uppercase().replace('/', "\\"),
            &[],
            &mut world.physics_objects,
        )?;
        assert!(Arc::ptr_eq(&shared.shapes, ticket_shapes));
        assert!(Arc::ptr_eq(&shared.bounds, ticket_cache));
        assert_ne!(
            shared.reference.offset, ticket_shapes[0].bounds_root,
            "Serialized address confused with graph identity"
        );
        assert_eq!(world.phantom_resources.len(), resource_count);
        assert_eq!(world.physics_objects.shapes.len(), shape_count);
        assert!(world
            .phantom_resources
            .acquire(
                "Missing/InvalidPhantom.hkx_wii",
                &[],
                &mut world.physics_objects
            )
            .is_err());
        assert_eq!(world.phantom_resources.len(), resource_count);
        assert_eq!(world.physics_objects.shapes.len(), shape_count);
        assert!(!world.size_phantoms.contains_key(&ticket));
        assert!(!world.size_phantom_handles.contains_key(&ticket));
        let handle = world.resource_phantom_handles[&ticket];
        let phantom = &world.physics_objects.phantoms[&handle];
        assert_eq!(
            phantom.header,
            super::super::hk_reference::Header {
                memory_size: 0x170,
                references: 3
            }
        );
        assert_eq!(phantom.bounds, None);
        assert_eq!(phantom.world, None);
        assert_eq!(phantom.collidable.unwrap().broadphase_kind, 2);
        assert_eq!(phantom.collidable.unwrap().owner_offset, -16);
        assert_eq!(
            world.physics_objects.systems[&world.phantom_systems[&ticket]].phantoms,
            vec![handle]
        );
        assert_eq!(world.pickup_lifecycles[&ticket].phantom, Some(handle));
        let listener = world.pickup_lifecycles[&ticket].listener.unwrap();
        assert!(phantom.overlaps.listeners.contains(&Some(listener)));
        assert_eq!(
            world.physics_objects.owner_user_data(handle)?,
            super::super::collidable_owner::UserData::Entity(ticket + 1)
        );
        let same_shape = phantom.collidable.unwrap().shape.unwrap();
        assert_eq!(same_shape, shared.shape_identity);
        let retained = world
            .resource_phantom_handles
            .values()
            .filter(|&&other| {
                world.physics_objects.phantoms[&other]
                    .collidable
                    .unwrap()
                    .shape
                    == Some(same_shape)
            })
            .count();
        let mut expected = shared.reference.header;
        if expected.memory_size != 0 {
            expected.references = expected.references.wrapping_add(retained as i16);
        }
        assert_eq!(world.physics_objects.shapes[&same_shape], expected);
        assert!(
            world
                .resource_phantom_handles
                .values()
                .filter(|&&other| world.physics_objects.phantoms[&other]
                    .collidable
                    .unwrap()
                    .shape
                    == Some(same_shape))
                .count()
                > 1
        );

        assert!(
            world
                .resource_phantom_shapes
                .values()
                .filter(|shapes| Arc::ptr_eq(shapes, ticket_shapes))
                .count()
                > 1,
            "Repeated pickup collision resource should share decoded geometry"
        );
        let ticket_before = world.resource_phantom_motion[&ticket].clone();
        let ticket_authored = world.entities[ticket].authored_transform;
        world.relocate_entity(ticket, [5., 7., -3.], [0., 0., 1., 0., 1., 0., -1., 0., 0.])?;
        let relocated = &world.resource_phantom_motion[&ticket];
        assert_eq!(&relocated.transform[..12], &ticket_before.transform[..12]);
        assert_eq!(&relocated.transform[12..], &[5., 7., -3., 0.]);
        assert_eq!(relocated.center, ticket_before.center);
        assert_eq!(relocated.previous_center, ticket_before.previous_center);
        assert_eq!(relocated.orientation, ticket_before.orientation);
        assert_eq!(world.entities[ticket].authored_transform, ticket_authored);
        let moved_bounds = world.resource_phantom_aabb(ticket, 0.)?;
        assert!(moved_bounds[0][1] < 7. && moved_bounds[1][1] > 7.);
        // Exercise the real World callback without advancing an unrelated entrance coroutine.
        world.run_callback_fixture("MoveToEntity(GetEntity('gremlinvillage_zonea.ETicket_C_Nif 01'),GetPlayer(),false,false)")?;
        assert_eq!(
            &world.resource_phantom_motion[&ticket].transform[12..15],
            &world.entities[world.player_entity].transform[12..15],
            "{:?}",
            world
                .report
                .unsupported
                .iter()
                .filter(|message| message.contains("runtime animation console"))
                .collect::<Vec<_>>()
        );
        let lod = &world.lod_states[&ticket];
        assert_eq!(lod.remaining.to_bits(), 0.0f32.to_bits());
        assert_eq!((lod.flags, lod.enabled, lod.visible), (0, 1, 1));
        assert_eq!(
            world
                .size_phantoms
                .get(&index)
                .context("Inherited phantom size not reconstructed")?
                .size,
            [1.; 3]
        );
        let handle = world.size_phantom_handles[&index];
        let owned = &world.physics_objects.phantoms[&handle];
        assert_eq!(
            owned.header,
            super::super::hk_reference::Header {
                memory_size: 224,
                references: 3
            }
        );
        assert_eq!(
            world.physics_objects.owner_user_data(handle)?,
            super::super::collidable_owner::UserData::Entity(index + 1)
        );
        assert_eq!(owned.world, None);
        assert!(owned.overlaps.collidables.is_empty());
        let lifecycle = &world.pickup_lifecycles[&index];
        assert_eq!(lifecycle.phantom, Some(handle));
        assert!(owned.overlaps.listeners.contains(&lifecycle.listener));
        assert!(world
            .physics_objects
            .listeners
            .contains_key(&lifecycle.listener.unwrap()));
        assert_eq!(world.pickup_physics_leases[&(index + 1)].references, 2);
        let entity = &world.entities[index];
        assert_eq!(
            owned.bounds,
            Some(world.size_phantoms[&index].padded_bounds(
                entity.transform[12..15].try_into().unwrap(),
                entity.runtime_rotation
            )?)
        );
        assert_eq!(
            owned.collidable.unwrap().filter_info,
            world.size_phantom_filters[&index]
        );
        let pending = world.phantom_admission_bounds(index)?;
        let stored = owned.bounds.unwrap();
        assert_eq!(
            pending.min.map(f32::to_bits),
            [stored[0], stored[1], stored[2]]
        );
        assert_eq!(
            pending.max.map(f32::to_bits),
            [stored[4], stored[5], stored[6]]
        );
        assert_eq!(
            world.physics_objects.phantoms.len(),
            world.size_phantom_handles.len() + world.resource_phantom_handles.len()
        );
        assert_eq!(
            world.phantom_systems.len(),
            world.physics_objects.phantoms.len()
        );
        assert_eq!(
            world
                .size_phantom_handles
                .values()
                .collect::<HashSet<_>>()
                .len(),
            world.size_phantom_handles.len()
        );
        let component = world.entities[index]
            .components
            .iter()
            .find(|c| c.class_name == "PhysicsPhantomComponent")
            .unwrap();
        assert_eq!(
            world.size_phantom_filters.get(&index).copied(),
            Some(super::super::phantom_bounds::authored_size_filter(
                component
            )?)
        );
        let before = world
            .phantom_bounds(index)?
            .context("Phantom bounds missing")?;
        let pickup_position: V = world.entities[index].transform[12..15].try_into().unwrap();
        let player_position: V = world.entities[world.player_entity].transform[12..15]
            .try_into()
            .unwrap();
        assert_eq!(
            pickup_position, player_position,
            "Authored startup Lua must relocate the ability pickup to Mickey"
        );
        assert_eq!(
            world.entities[index].runtime_rotation,
            world.entities[world.player_entity].runtime_rotation
        );
        for axis in 0..3 {
            assert!(
                before.min[axis] <= pickup_position[axis]
                    && before.max[axis] >= pickup_position[axis]
            );
        }
        let authored = world.entities[index].authored_transform;
        world.relocate_entity(index, [5., 7., -3.], [1., 0., 0., 0., 1., 0., 0., 0., 1.])?;
        let after = world.phantom_bounds(index)?.unwrap();
        assert_eq!(after.min, [4.5, 6.5, -3.5]);
        assert_eq!(after.max, [5.5, 7.5, -2.5]);
        assert_eq!(
            world.physics_objects.phantoms[&handle].bounds,
            Some(
                world.size_phantoms[&index]
                    .padded_bounds([5., 7., -3.], world.entities[index].runtime_rotation)?
            )
        );
        // Exercise the real World callback without advancing an unrelated entrance coroutine.
        world.run_callback_fixture("MoveToEntity(GetEntity('gremlinvillage_zonea_dynamic.Ability_AllTheBasics'), GetPlayer(), false, false)")?;
        assert_eq!(
            world.entities[index].transform[12..15],
            world.entities[world.player_entity].transform[12..15]
        );
        assert_eq!(
            world.physics_objects.phantoms[&handle].bounds,
            Some(world.size_phantoms[&index].padded_bounds(
                world.entities[index].transform[12..15].try_into().unwrap(),
                world.entities[index].runtime_rotation
            )?)
        );
        assert_eq!(world.entities[index].authored_transform, authored);
        ensure!(
            world
                .report
                .unsupported
                .iter()
                .any(|s| s.contains("not active gameplay sensors")),
            "Bounds must not imply functional overlap dispatch"
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn production_relocation_keeps_rendering_collision_and_native_contacts_together() -> Result<()>
    {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let index = world
            .entities
            .iter()
            .enumerate()
            .find(|(i, e)| {
                *i != world.player_entity
                    && !e.draws.is_empty()
                    && !e.collision_parts.is_empty()
                    && e.transform_parent_id().is_ok_and(|parent| parent.is_none())
                    && e.jigsaw_asset.is_none()
                    && !e.components.iter().any(|c| {
                        matches!(
                            c.class_name.as_str(),
                            "PhysicsRigidBodyDynamicComponent"
                                | "PhysicsPhantomComponent"
                                | "PhysicsCharacterControllerComponent"
                        )
                    })
                    && !world.entities.iter().any(|c| {
                        c.scene == e.scene && c.transform_parent_id().is_ok_and(|p| p == Some(e.id))
                    })
            })
            .map(|(i, _)| i)
            .context("No independent static owner for original relocation check")?;
        let entity = &world.entities[index];
        let authored = entity.authored_transform;
        let owner = entity.collision_parts.start;
        let part = &world.report.collision_parts[owner];
        let (triangle, center, normal) = (part.triangle_start
            ..part.triangle_start + part.triangle_count)
            .find_map(|triangle| {
                let [a, b, c] = world.collision.triangles[triangle].p;
                let normal = physics::cross(physics::sub(b, a), physics::sub(c, a));
                (physics::length(normal) > 0.).then(|| {
                    (
                        triangle,
                        physics::mul(physics::add(physics::add(a, b), c), 1. / 3.),
                        physics::unit(normal),
                    )
                })
            })
            .context("No nondegenerate source triangle")?;
        let hits = world.collision.ray_hits(
            physics::add(center, physics::mul(normal, 2.)),
            physics::mul(normal, -4.),
        )?;
        ensure!(
            hits.iter().any(|h| h.owner == owner),
            "Original query did not hit chosen owner"
        );
        let hit = hits.iter().find(|h| h.owner == owner).unwrap();
        let candidate = physics::SphereCandidate {
            shape: hit.shape,
            owner,
            source_triangle: hit.source_triangle,
        };
        let source = world.imported_static_candidate(candidate)?;
        let local = hit.source_triangle.unwrap() - part.triangle_start;
        let mesh = &part.native_resource[part.native_part];
        assert_eq!(source.part, owner);
        assert_eq!(source.body_offset, mesh.body_offset);
        assert_eq!(source.shape_key, mesh.shape_keys.key(Some(local))?);
        assert_eq!(source.serialized_body_filter, mesh.body_filter_info);
        assert_eq!(
            source.serialized_child_filter,
            mesh.child_filters.at(Some(local))?
        );
        assert_eq!(source.serialized_quality, mesh.motion.quality);
        assert!(world
            .imported_static_candidate(physics::SphereCandidate {
                shape: usize::MAX,
                ..candidate
            })
            .is_err());
        assert!(world
            .imported_static_candidate(physics::SphereCandidate {
                source_triangle: Some(part.triangle_start + part.triangle_count),
                ..candidate
            })
            .is_err());
        let old_native = world.initial_static_contact(owner, triangle)?.1;
        let old_pose = entity.initial_physics_transform();
        let position = physics::add(
            entity.transform[12..15].try_into().unwrap(),
            [1000., 500., 200.],
        );
        let r = entity.authored_rotation;
        let rotation = [r[6], r[7], r[8], r[3], r[4], r[5], -r[0], -r[1], -r[2]];
        world.relocate_entity(index, position, rotation)?;
        let entity = &world.entities[index];
        assert_eq!(entity.authored_transform, authored);
        assert!(entity
            .draws
            .clone()
            .all(|i| world.draws[i].trans_mat == entity.transform));
        let identity = super::super::native_hk_transform::from_entity(
            [1., 0., 0., 0., 1., 0., 0., 0., 1.],
            [0.; 3],
        );
        let inverse = super::super::native_hk_transform::relative(&old_pose, &identity);
        let delta =
            super::super::native_hk_transform::compose(&entity.physics_transform(), &inverse);
        let next_center = physics::point(&delta, center);
        let next_normal = physics::sub(
            physics::point(&delta, normal),
            physics::point(&delta, [0.; 3]),
        );
        let hits = world.collision.ray_hits(
            physics::add(next_center, physics::mul(next_normal, 2.)),
            physics::mul(next_normal, -4.),
        )?;
        ensure!(
            hits.iter().any(|h| h.owner == owner),
            "Moved render owner left collision behind"
        );
        let old_hits = world.collision.ray_hits(
            physics::add(center, physics::mul(normal, 2.)),
            physics::mul(normal, -4.),
        )?;
        ensure!(
            !old_hits.iter().any(|h| h.owner == owner),
            "Old collision was duplicated after movement"
        );
        assert_ne!(world.initial_static_contact(owner, triangle)?.1, old_native);
        let once = world.entities[index].transform;
        world.relocate_entity(index, position, rotation)?;
        assert_eq!(
            world.entities[index].transform, once,
            "Absolute relocation accumulated transforms"
        );
        assert!(world
            .relocate_entity(index, [f32::NAN, 0., 0.], rotation)
            .is_err());
        assert_eq!(
            world.entities[index].transform, once,
            "Rejected pose changed the owner"
        );
        std::fs::write(
            "target/owner-relocation-evidence.json",
            serde_json::to_vec_pretty(&serde_json::json!({
            "level":"Levels/GV_ZoneA_Start.level","entity":world.entities[index].name,
            "original_position":authored[12..15],"moved_position":position,
            "checks":["render owner updated","new ray hits owner","old ray no longer hits owner","native contact pose changed","absolute update does not accumulate","invalid update rejected"],
            "original_game_reference_validated":false}))?,
        )?;
        println!("Original static owner relocated across rendering, Box3D queries and native contact transforms");
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn production_scripted_actor_crossfade_reaches_rendered_morphs() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        // This test verifies shared prefab tables and rendered actor blending.
        // MapLoad_SetupPosition is mutable gameplay state, not an invariant of
        // a world that has already dispatched OnMapLoad. Constructor values
        // are covered separately in actor_scripts' prefab-data regression.
        world.run_script("local data = GetPrefabData('GV_ZoneA_ProjectorStart'); assert(type(data) == 'table', 'Missing prefab table'); assert(data.ExitFunction == 'GV_ZoneA_EntryIntoGV', 'ExitFunction='..tostring(data.ExitFunction)); assert(data.ToFlyThrough == 'True', 'ToFlyThrough='..tostring(data.ToFlyThrough)); data.__blotworx_test = 'persisted'; assert(GetPrefabData('GV_ZoneA_ProjectorStart').__blotworx_test == 'persisted', 'Prefab write did not persist'); data.__blotworx_test = nil")?;
        world.advance_models(0.)?;
        ensure!(
            !world
                .report
                .unsupported
                .iter()
                .any(|message| message.starts_with("Game Lua:")),
            "Original projector settings did not bind to its member entity: {:?}",
            world
                .report
                .unsupported
                .iter()
                .filter(|message| message.starts_with("Game Lua:"))
                .collect::<Vec<_>>()
        );
        let name = "gremlinvillage_zonea.Pipe_Straight_2m_Spawner_01 01";
        let entity = world
            .entities
            .iter()
            .position(|e| e.name == name)
            .context("Missing original pipe actor")?;
        let idles = world
            .actors
            .values()
            .filter(|actor| actor.active_id().is_some())
            .count();
        let authored = world
            .report
            .geometry
            .iter()
            .filter(|entry| {
                entry["kind"] == "actor_instance" && !entry["active_sequence"].is_null()
            })
            .count();
        assert_eq!(
            idles, authored,
            "Authored startup was not retained by actor instances"
        );
        assert!(
            world.actor_playback.is_none(),
            "Gameplay test must not use manual playback"
        );
        // Control actor request timing directly through the live Lua host.
        // Entrance task retirement can legitimately skip a swapped-in test
        // coroutine for one update; it must not change this blend scenario.
        world.with_script_world_host(World::script_world_api, |scripts| {
            scripts.ensure_global_module(
                "@test/morph-rest",
                format!("AnimGBReset({name:?}); AnimGBSequence({name:?}, 'Rest')").as_bytes(),
            )
        })?;
        world.advance_models(0.)?;
        assert_eq!(
            world.actors[&entity].active_id(),
            Some(0),
            "{:?}",
            world
                .report
                .unsupported
                .iter()
                .filter(|message| message.starts_with("Game Lua:"))
                .collect::<Vec<_>>()
        );
        let idle = world.actors[&entity].pose.as_ref().unwrap().morphs.clone();
        world.with_script_world_host(World::script_world_api, |scripts| {
            scripts.ensure_global_module(
                "@test/morph-spawn",
                format!("AnimGBSequence({name:?}, 'Spawn')").as_bytes(),
            )
        })?;
        world.advance_models(0.1)?;
        assert_eq!(world.actors[&entity].active_id(), Some(1));
        world.advance_models(0.15)?;
        let actor = &world.actors[&entity];
        let source = world.actor_bindings[&entity].sample(0, 0.)?;
        let destination = world.actor_bindings[&entity].sample(1, 0.05)?;
        let mut changed = false;
        for (key, (_, vertices)) in &actor.pose.as_ref().unwrap().morphs {
            for ((point, a), b) in vertices
                .iter()
                .zip(&source.morphs[key].1)
                .zip(&destination.morphs[key].1)
            {
                for axis in 0..3 {
                    assert!((point[axis] - (a[axis] * 0.5 + b[axis] * 0.5)).abs() < 0.0001);
                }
            }
            changed |= *vertices != idle[key].1;
        }
        assert!(changed, "Lua request never changed original morph geometry");
        let mut draws = world.draws.clone();
        world.apply_model_poses(&mut draws)?;
        assert!(
            world.entities[entity]
                .mesh_draws
                .values()
                .flatten()
                .any(|&i| draws[i].v_data != world.draws[i].v_data),
            "Actor pose did not reach renderer"
        );
        world.advance_models(0.25)?;
        assert_eq!(world.actors[&entity].active_id(), Some(1));
        ensure!(
            !world
                .report
                .unsupported
                .iter()
                .any(|message| message.starts_with("Script animation request:")),
            "Script request failed"
        );
        println!(
            "Authored actors active: {idles}; native Lua Rest -> Spawn crossfade reached renderer"
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_sequence_api_changes_actor_before_return_and_resumes_idle_clock() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let name = "gremlinvillage_zonea.Pipe_Straight_2m_Spawner_01 01";
        let entity = world
            .scripts
            .resolve_name(name)
            .context("Original pipe actor missing")?;
        let id = world.actor_sequences[&entity].find_name("Spawn");
        ensure!(
            world.actor_sequences[&entity].get(id).is_some(),
            "Original Spawn sequence missing"
        );
        let source = format!(
            r#"
            local actor=GetEntity({name:?})
            assert(select('#',AnimGBSequence(actor,'Spawn',false,nil,'nonnumeric',false))==0)
            assert(ObservedSequence=={id}, 'Actor request was deferred')
            assert(select('#',AnimGBSequence('ignored',actor,'Spawn',false,-1,-2,false))==0)
            assert(select('#',AnimGBSequence(actor,'Spawn',true))==0)
            assert(select('#',AnimGBSequence('missing-actor','Spawn',true,5,5,true))==0)
        "#
        );
        world.with_script_world_host(
            |world, lua, api, args| {
                let result = World::script_world_api(world, lua, api, args)?;
                if api == "AnimGBSequence" {
                    lua.globals()
                        .set("ObservedSequence", world.actors[&entity].active_id())?;
                }
                Ok(result)
            },
            |scripts| {
                scripts.ensure_global_module("@test/synchronous-native-sequence", source.as_bytes())
            },
        )?;
        assert_eq!(world.actors[&entity].active_id(), Some(id));
        world.idle_actors.insert(entity);
        world.with_script_world_host(World::script_world_api,|scripts|scripts.ensure_global_module(
            "@test/sequence-resumes-idle",format!("assert(select('#',AnimGBSequence({name:?},'BwxMissingSequence',true,2,3,false))==0)").as_bytes()))?;
        assert!(!world.idle_actors.contains(&entity));
        assert_eq!(
            world.actors[&entity].active_id(),
            Some(id),
            "Unknown sequence changed active playback"
        );
        world.advance_models(0.1)?;
        let pose = world.actors[&entity]
            .pose
            .as_ref()
            .context("Requested actor has no sampled pose")?
            .morphs
            .clone();
        world.advance_models(0.4)?;
        assert_ne!(
            world.actors[&entity].pose.as_ref().unwrap().morphs,
            pose,
            "Actual morph playback did not advance"
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_projector_spawner_scalar_setters_update_live_fields_without_spawning() -> Result<()>
    {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let entity = world
            .entities
            .iter()
            .position(|entity| {
                entity.name.ends_with(".SpawnerPortalWarpFX")
                    && entity
                        .components
                        .iter()
                        .any(|c| c.class_name == "TemplateSpawnerComponent")
            })
            .context("Original projector warp spawner missing")?;
        let name = world.entities[entity].name.clone();
        let count = world.entities.len();
        let source = format!(
            r#"
            local e=GetEntity({name:?})
            assert(select('#',SetPropertyString(e,'Spawn Group','Bwx_Portal_Active\000ignored',0))==0)
            assert(ObservedSpawnGroup=='Bwx_Portal_Active', 'String setter was deferred')
            assert(select('#',SetPropertyInt(e,'Index To Spawn','2.9',0))==0)
            assert(ObservedSpawnIndex==2, 'Integer setter was deferred')
            SetPropertyString(e,'Spawn Group','must-not-write',1)
            SetPropertyInt(e,'Spawn Group',3)
            SetPropertyInt(e,'Index To Spawn',44,-1)
            assert(select('#',SetPropertyString('missing-spawner','Spawn Group','x'))==0)
            assert(select('#',SetPropertyInt(e,'BwxMissingProperty',4))==0)
            assert(not pcall(SetPropertyString,e,'Templates To Spawn','missing-template',0))
        "#
        );
        world.with_script_world_host(
            |world, lua, api, args| {
                let result = World::script_world_api(world, lua, api, args)?;
                let component = world.entities[entity]
                    .components
                    .iter()
                    .find(|c| c.class_name == "TemplateSpawnerComponent")
                    .unwrap();
                let field = |name: &str| {
                    component
                        .properties
                        .iter()
                        .find(|p| p.name == name)
                        .map(|p| &p.value)
                };
                if let Some(value) = field("Spawn Group").and_then(|v| v.as_str()) {
                    lua.globals().set("ObservedSpawnGroup", value)?;
                }
                if let Some(value) = field("Index To Spawn").and_then(|v| v.as_i64()) {
                    lua.globals().set("ObservedSpawnIndex", value)?;
                }
                Ok(result)
            },
            |scripts| scripts.ensure_global_module("@test/live-spawner-scalars", source.as_bytes()),
        )?;
        let component = world.entities[entity]
            .components
            .iter()
            .find(|c| c.class_name == "TemplateSpawnerComponent")
            .unwrap();
        assert_eq!(
            component
                .properties
                .iter()
                .find(|p| p.name == "Spawn Group")
                .unwrap()
                .value,
            serde_json::json!("Bwx_Portal_Active")
        );
        assert_eq!(
            component
                .properties
                .iter()
                .find(|p| p.name == "Index To Spawn")
                .unwrap()
                .value,
            serde_json::json!(2)
        );
        // A reflected field remains writable when an unchanged default was
        // omitted by serialization; no guessed default value is required.
        world.entities[entity]
            .components
            .iter_mut()
            .find(|c| c.class_name == "TemplateSpawnerComponent")
            .unwrap()
            .properties
            .retain(|p| p.name != "Max Live Spawns");
        world.with_script_world_host(World::script_world_api, |scripts| {
            scripts.ensure_global_module(
                "@test/omitted-spawner-field",
                format!("SetPropertyInt(GetEntity({name:?}),'Max Live Spawns',5)").as_bytes(),
            )
        })?;
        let component = world.entities[entity]
            .components
            .iter()
            .find(|c| c.class_name == "TemplateSpawnerComponent")
            .unwrap();
        assert_eq!(
            component
                .properties
                .iter()
                .find(|p| p.name == "Max Live Spawns")
                .unwrap()
                .value,
            serde_json::json!(5)
        );
        assert_eq!(
            world.entities.len(),
            count,
            "Scalar writes must not invent immediate spawning"
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn production_scene_prepares_gamebryo_sequence_resources() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let (index, _) = world
            .entities
            .iter()
            .enumerate()
            .find(|(_, e)| e.name == "gremlinvillage_zonea.Pipe_Straight_2m_Spawner_01 01")
            .context("Missing placed spawner")?;
        let resources = world
            .actor_sequences
            .get(&index)
            .context("Spawner resources not prepared")?;
        assert_eq!(resources.find_name("Rest"), 0);
        assert_eq!(resources.find_name("Spawn"), 1);
        let bindings = world
            .actor_bindings
            .get(&index)
            .context("Spawner transform bindings not prepared")?;
        assert_eq!(bindings.sequences.len(), 2);
        for id in [0, 1] {
            let pose = bindings.sample(id, 0.)?;
            assert_eq!(pose.transforms.len(), 3);
            assert_eq!(pose.morphs.len(), 2);
        }
        assert!(bindings.other_evaluators.is_empty());
        let idle = bindings.sample(0, 0.)?;
        let spawn = bindings.sample(1, 0.25)?;
        assert!(spawn
            .morphs
            .iter()
            .any(|(key, value)| value.1 != idle.morphs[key].1));
        let asset = world.entities[index]
            .model_asset
            .as_ref()
            .context("Missing spawner model identity")?;
        assert_eq!(
            world.model_bounds[asset].nodes[&bindings.actor_root].name,
            resources.manager().model_root
        );
        assert!(world
            .report
            .geometry
            .iter()
            .any(|entry| entry["kind"] == "actor_sequence_resources"
                && entry["playback_active"] == false));
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn production_dynamic_collision_ownership_and_sharing() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let expected = world
            .entities
            .iter()
            .map(|e| {
                e.components
                    .iter()
                    .filter(|c| c.class_name == "PhysicsRigidBodyDynamicComponent")
                    .count()
            })
            .sum::<usize>();
        ensure!(
            expected > 0 && world.dynamic_bodies.len() == expected,
            "Dynamic components lost during loading"
        );
        let mut identities = HashSet::new();
        let mut resources = BTreeMap::new();
        let mut failures = BTreeMap::new();
        let mut failed_components = Vec::new();
        let mut platforms = 0;
        let mut wheels = 0;
        for body in &world.dynamic_bodies {
            let entity = &world.entities[body.entity];
            ensure!(
                body.initial_velocity.is_ok(),
                "Initial velocity {}:{}: {:?}",
                entity.scene,
                entity.name,
                body.initial_velocity
            );
            ensure!(
                identities.insert((body.entity, body.component)),
                "Duplicate dynamic component identity"
            );
            ensure!(
                entity
                    .components
                    .iter()
                    .any(|c| c.class_name == "PhysicsRigidBodyDynamicComponent"
                        && c.link_id.to_u32() == body.component),
                "Dynamic component has the wrong entity owner"
            );
            match &body.geometry {
                Ok(parts) => {
                    let systems = body
                        .systems
                        .as_ref()
                        .map_err(|error| anyhow::anyhow!(error.clone()))?;
                    let selected = body
                        .selected_system
                        .as_ref()
                        .map_err(|error| anyhow::anyhow!(error.clone()))?
                        .context("Native resource selected no system")?;
                    ensure!(
                        systems.iter().any(|system| system.offset == selected),
                        "Selection outside imported systems"
                    );
                    ensure!(
                        !systems.is_empty(),
                        "{} has no explicit physics-system records",
                        body.asset
                    );
                    let ordered = systems
                        .iter()
                        .flat_map(|source| source.system.bodies.iter().flatten())
                        .copied()
                        .collect::<std::collections::BTreeSet<_>>();
                    let decoded = parts
                        .iter()
                        .map(|part| part.body_offset)
                        .collect::<std::collections::BTreeSet<_>>();
                    ensure!(
                        ordered == decoded,
                        "System body references differ from geometry definitions in {}",
                        body.asset
                    );
                    for system in systems.iter() {
                        let ordered_layout =
                            body.system_layout(system.offset, &entity.initial_physics_transform())?;
                        ensure!(
                            ordered_layout
                                .iter()
                                .map(|body| body.as_ref().map(|body| body.body_offset))
                                .collect::<Vec<_>>()
                                == system.system.bodies,
                            "System body order/null slots changed"
                        );
                    }
                    let layout = crate::blotworx::dynamic_body::layout(
                        parts,
                        &entity.initial_physics_transform(),
                    )?;
                    ensure!(
                        layout.iter().map(|body| body.parts.len()).sum::<usize>() == parts.len(),
                        "Rigid-body layout lost geometry parts"
                    );
                    let mut queries = Collision::new(Vec::new())?;
                    // Force body-local triangle zero to differ from world zero.
                    queries.add_static_triangles(
                        &[super::physics::Triangle {
                            p: [[0., -100., 0.], [1., -100., 0.], [0., -100., 1.]],
                        }],
                        0.,
                        usize::MAX,
                    )?;
                    let mut bounds = parts[0].bounds_graph.as_ref().clone();
                    let mut installed = HashSet::new();
                    for group in &layout {
                        let first = &parts[group.parts[0]];
                        for &index in &group.parts {
                            ensure!(
                                Arc::ptr_eq(&first.properties, &parts[index].properties),
                                "Body property inputs duplicated between shape parts"
                            );
                        }
                        let mut auxiliary = first.instantiate_auxiliary()?;
                        ensure!(
                            auxiliary.properties.entries() == first.properties.as_ref(),
                            "Authored body properties lost during instantiation"
                        );
                        let mut probe_key = u32::MAX;
                        while auxiliary.properties.entry(probe_key).is_some() {
                            probe_key =
                                probe_key.checked_sub(1).context("No property probe key")?;
                        }
                        auxiliary.properties.add(probe_key, 0xffffffff80000000)?;
                        let independent = first.instantiate_auxiliary()?;
                        ensure!(
                            independent.properties.entry(probe_key).is_none(),
                            "Body property mutation leaked to source or another instance"
                        );
                        let mut motion = group.instantiate_motion(parts)?;
                        ensure!(
                            motion.current.motion_type() == parts[group.parts[0]].motion_type,
                            "Imported motion type changed during instantiation"
                        );
                        ensure!(
                            motion.current.swept_state().transform
                                == group.initial_motion_state()?.transform,
                            "Imported motion placement differs from query placement"
                        );
                        ensure!(
                            motion.current.linear_velocity()
                                == parts[group.parts[0]].linear_velocity,
                            "Placement changed authored velocity"
                        );
                        for &index in &group.parts {
                            ensure!(
                                Arc::ptr_eq(&parts[0].bounds_graph, &parts[index].bounds_graph),
                                "HKX parts duplicated authored bounds graph"
                            );
                            ensure!(
                                parts[group.parts[0]].bounds_root == parts[index].bounds_root,
                                "Compound body lost its bounds root"
                            );
                            ensure!(
                                Arc::ptr_eq(&parts[group.parts[0]].motion, &parts[index].motion),
                                "Compound body duplicated imported motion storage"
                            );
                            ensure!(
                                match (&parts[group.parts[0]].mopp, &parts[index].mopp) {
                                    (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                                    (None, None) => true,
                                    _ => false,
                                },
                                "Compound body duplicated or lost its MOPP program"
                            );
                        }
                        // Exercise the native body setter with its original shape,
                        // including compound bounds, without reparsing the resource.
                        let identity = [
                            1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
                        ];
                        motion
                            .current
                            .set_center_with_bounds(group.local_center_of_mass, |_| {
                                bounds.aabb(parts[group.parts[0]].bounds_root, &identity, 0.)
                            })?;
                        let mut source_triangle = queries.triangles.len();
                        let placed = group.place_initialized_query_body(
                            parts,
                            motion,
                            auxiliary,
                            &mut queries,
                            body.entity,
                        )?;
                        if !matches!(placed.motion.current.motion_type(), 4 | 5) {
                            placed.admit_native_mass(&mut queries).with_context(|| {
                                format!(
                                    "Native mass {}:{} body {:#x}",
                                    entity.scene, entity.name, group.body_offset
                                )
                            })?;
                        }
                        ensure!(
                            placed
                                .auxiliary
                                .properties
                                .entry(probe_key)
                                .is_some_and(|entry| entry.value == 0xffffffff80000000),
                            "Placed body lost initialized property changes"
                        );
                        for (&index, &shape) in group.parts.iter().zip(&placed.shapes) {
                            let part = &parts[index];
                            if !part.convex {
                                for local in 0..part.indices.len() / 3 {
                                    ensure!(
                                        placed.local_contact_triangle(
                                            shape,
                                            source_triangle + local
                                        )? == Some(local),
                                        "World contact lost original local triangle identity"
                                    );
                                    ensure!(
                                        placed
                                            .contact_welding_word(shape, source_triangle + local)?
                                            == placed.welding_word(shape, local)?,
                                        "World contact selected wrong welding word"
                                    );
                                }
                                ensure!(
                                    placed
                                        .local_contact_triangle(shape, source_triangle - 1)
                                        .is_err(),
                                    "Previous mesh triangle accepted"
                                );
                                source_triangle += part.indices.len() / 3;
                                ensure!(
                                    placed
                                        .local_contact_triangle(shape, source_triangle)
                                        .is_err(),
                                    "Next mesh triangle accepted"
                                );
                            } else {
                                ensure!(
                                    placed.local_contact_triangle(shape, 0)?.is_none(),
                                    "Convex shape acquired triangle identity"
                                );
                            }
                            if let Some(authored) = &part.welding {
                                let crate::blotworx::shape_welding::Prepared::Authored(retained) =
                                    placed
                                        .welding
                                        .as_ref()
                                        .context("Placed body lost welding")?
                                else {
                                    anyhow::bail!("Unexpected authored regeneration");
                                };
                                ensure!(
                                    Arc::ptr_eq(authored, retained),
                                    "Placed body copied authored welding"
                                );
                                for triangle in 0..part.indices.len() / 3 {
                                    ensure!(
                                        placed.welding_word(shape, triangle)?
                                            == Some(authored.words[part.welding_offset + triangle]),
                                        "Placed body welding identity changed"
                                    );
                                }
                                ensure!(
                                    placed.welding_word(shape, part.indices.len() / 3).is_err(),
                                    "Out-of-range welding read accepted"
                                );
                            } else {
                                ensure!(
                                    placed.welding_word(shape, 0)?.is_none(),
                                    "Convex body gained fabricated welding"
                                );
                            }
                            ensure!(
                                installed.insert(shape),
                                "Body assembly reused another body's collision shape"
                            );
                        }
                    }
                    ensure!(
                        installed.len() == parts.len(),
                        "Body assembly did not install every shape"
                    );
                    ensure!(
                        queries.min_y.is_finite(),
                        "Installed body assembly has invalid bounds"
                    );
                    if let Some(previous) = resources.insert(body.asset.clone(), parts) {
                        ensure!(
                            Arc::ptr_eq(previous, parts),
                            "Repeated collision resource allocated duplicate geometry"
                        );
                    }
                    if body
                        .asset
                        .ends_with("gv_zonea_platform01_dynamic_mesh.hkx_wii")
                    {
                        ensure!(layout.len() == 1, "Platform must retain one rigid body");
                        platforms += 1;
                        ensure!(
                            parts.len() == 1
                                && parts[0].vertices.len() == 42
                                && parts[0].indices.len() == 240,
                            "Platform collision geometry changed"
                        );
                        ensure!(
                            parts[0].motion_type == 3,
                            "Serialized platform motion was overwritten"
                        );
                    }
                    if body
                        .asset
                        .ends_with("gv_zonea_wheel01_dynamic_hull.hkx_wii")
                    {
                        ensure!(layout.len() == 10, "Wheel rigid bodies were merged");
                        wheels += 1;
                        ensure!(
                            parts.len() == 10
                                && parts.iter().all(|p| p.convex && p.motion_type == 3),
                            "Wheel body assembly changed"
                        );
                    }
                }
                Err(error) => {
                    failures.insert(body.asset.clone(), error.clone());
                    failed_components.push(serde_json::json!({"scene":entity.scene,"entity":entity.name,
                        "entity_id":entity.id,"component_id":body.component,"asset":body.asset,
                        "error":error,"components":entity.components.iter().map(|c|
                            serde_json::json!({"class":c.class_name,"properties":c.properties.iter().map(|p|
                                serde_json::json!({"name":p.name,"value":p.value})).collect::<Vec<_>>()})).collect::<Vec<_>>()}));
                    ensure!(
                        world
                            .report
                            .unsupported
                            .iter()
                            .any(|s| s.contains(error) && s.contains(&entity.name)),
                        "Dynamic decode failure is not reported"
                    );
                }
            }
        }
        ensure!(
            platforms == 4 && wheels == 1,
            "Expected four platform placements and one wheel, got {platforms}/{wheels}"
        );
        let mut welding_report = Vec::new();
        let identity = crate::blotworx::native_hk_transform::from_entity(
            [1., 0., 0., 0., 1., 0., 0., 0., 1.],
            [0.; 3],
        );
        for (asset, parts) in &resources {
            for group in crate::blotworx::dynamic_body::layout(parts, &identity)? {
                if group.parts.iter().all(|&i| parts[i].convex) {
                    continue;
                }
                let generated = group
                    .generate_local_welding(parts)
                    .with_context(|| format!("Welding {asset}"))?
                    .into_iter()
                    .collect::<BTreeMap<_, _>>();
                let mut compared = 0;
                let mut differences = 0;
                let mut excluded = 0;
                for &index in &group.parts {
                    let part = &parts[index];
                    if let Some(welding) = &part.welding {
                        if welding.mode == 5 {
                            if let Some(words) =
                                welding.subpart(part.welding_offset, part.indices.len() / 3)?
                            {
                                for (i, &word) in words.iter().enumerate() {
                                    let vertices = std::array::from_fn::<_, 3, _>(|j| {
                                        let v = part.vertices[part.indices[i * 3 + j] as usize];
                                        [v[0], v[1], v[2], 0.]
                                    });
                                    let key = part.shape_keys.key(Some(i))?;
                                    if crate::blotworx::shape_welding::degenerate_triangle(
                                        vertices,
                                    )? {
                                        ensure!(
                                            !generated.contains_key(&key),
                                            "Degenerate source generated a write"
                                        );
                                        excluded += 1;
                                        continue;
                                    }
                                    compared += 1;
                                    if *generated
                                        .get(&key)
                                        .context("Missing eligible welding write")?
                                        != word
                                    {
                                        differences += 1;
                                    }
                                }
                            }
                        }
                    }
                }
                welding_report.push(serde_json::json!({"asset":asset,"body":group.body_offset,
                    "generated":generated.len(),"compared":compared,"differences":differences,"excluded":excluded}));
            }
        }
        std::fs::write(
            "target/production-welding-coverage.json",
            serde_json::to_vec_pretty(&welding_report)?,
        )?;
        ensure!(
            welding_report.iter().all(|v| v["differences"] == 0),
            "Production welding differs; see coverage report"
        );
        let summary = serde_json::json!({"components":expected,"unique_decoded_assets":resources.len(),
            "platforms":platforms,"wheels":wheels,"failures":failures});
        std::fs::write(
            "target/production-dynamic-collision-failures.json",
            serde_json::to_vec_pretty(&failed_components)?,
        )?;
        std::fs::write(
            "target/production-dynamic-collision.json",
            serde_json::to_vec_pretty(&summary)?,
        )?;
        println!("{summary}");
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn production_model_pose_drives_draws_and_lighting_bounds() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let entity = world
            .entities
            .iter()
            .position(|e| {
                e.model_asset
                    .as_ref()
                    .is_some_and(|a| a.ends_with("/gv_zonea_platform01.nif_wii"))
            })
            .context("Production platform missing")?;
        ensure!(
            world.model_animation.contains_key(&entity),
            "Platform has no runtime instance"
        );
        let view = [
            1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
        ];
        let mut initial = world.draws.clone();
        world.apply_spherical_lighting(&mut initial, &view);
        world.apply_model_poses(&mut initial)?;
        let first = world
            .spherical_bounds
            .get(&entity)
            .context("Initial platform lighting bounds missing")?
            .clone();
        world.advance_models(1.)?;
        let mut moved = world.draws.clone();
        world.apply_spherical_lighting(&mut moved, &view);
        world.apply_model_poses(&mut moved)?;
        ensure!(
            world.spherical_bounds[&entity] != first,
            "Lighting bounds remained in bind pose"
        );
        let moved_bounds = world.spherical_bounds[&entity].clone();
        // A second rendered view at the same simulation time must start from
        // instance transforms, not feed animated mesh deltas back into the
        // model-owner transform used to evaluate its lighting bounds.
        let mut second_view = world.draws.clone();
        world.apply_spherical_lighting(&mut second_view, &view);
        world.apply_model_poses(&mut second_view)?;
        ensure!(
            world.spherical_bounds[&entity] == moved_bounds,
            "Repeated presentation accumulated motion in lighting bounds"
        );
        for i in world.entities[entity].draws.clone() {
            ensure!(
                second_view[i].trans_mat == moved[i].trans_mat,
                "Repeated presentation changed animated mesh transform"
            );
            ensure!(
                second_view[i].material_images.len() == moved[i].material_images.len(),
                "Repeated presentation changed material image bindings"
            );
            for (before, after) in moved[i]
                .material_images
                .iter()
                .zip(second_view[i].material_images.iter())
            {
                ensure!(
                    before.slot == after.slot && Arc::ptr_eq(&before.rgba, &after.rgba),
                    "Unchanged animated lighting image was replaced"
                );
            }
        }
        let indices = world.entities[entity].draws.clone();
        ensure!(
            indices
                .clone()
                .any(|i| initial[i].trans_mat != moved[i].trans_mat),
            "Platform render matrices did not animate"
        );
        for i in indices {
            ensure!(
                matches!(&moved[i].preview, Some(Ok(_))),
                "Platform still has an unsupported material/bound: {:?}",
                moved[i].preview.as_ref().and_then(|p| p.as_ref().err())
            );
            ensure!(
                Arc::ptr_eq(&initial[i].v_data, &moved[i].v_data),
                "Rigid model animation copied vertex buffers"
            );
        }
        world.advance_models(1.)?;
        let mut repeated = world.draws.clone();
        world.apply_model_poses(&mut repeated)?;
        ensure!(
            world.entities[entity]
                .draws
                .clone()
                .all(|i| repeated[i].trans_mat == moved[i].trans_mat),
            "Repeated frame accumulated model motion"
        );
        println!("{} model instances; platform draw matrices and lighting bounds share animation; vertex buffers reused",world.model_animation.len());
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn production_spherical_lights() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        // Register a model again after teardown without replacing existing
        // property identities or cached images belonging to other entities.
        let target = world
            .spherical_bindings
            .iter()
            .find(|binding| {
                binding.entity != world.player_entity
                    && world
                        .spherical_properties
                        .get(binding.property)
                        .is_ok_and(|p| p.owner.entity == binding.entity as u64)
                    && world
                        .spherical_bindings
                        .iter()
                        .filter(|b| b.property == binding.property)
                        .all(|b| b.entity == binding.entity)
            })
            .context("Missing independently lit model")?
            .entity;
        let identities = world
            .spherical_bindings
            .iter()
            .filter(|b| b.entity != target)
            .map(|b| (b.entity, b.mesh, b.draw, b.property))
            .collect::<Vec<_>>();
        world.remove_entity_lighting(target)?;
        let mut files = Vec::new();
        let mut loading = Report::default();
        for name in &world.report.packs {
            pack(Path::new(&root), name, &mut files, &mut loading)?;
        }
        let sources = material_preview::native_sources(&egui::Context::default(), &files);
        world.bind_spherical_properties(
            target..target + 1,
            &sources,
            &files,
            &mut HashMap::new(),
        )?;
        assert!(world.spherical_bindings.iter().any(|b| b.entity == target));
        assert_eq!(
            world
                .spherical_bindings
                .iter()
                .filter(|b| b.entity != target)
                .map(|b| (b.entity, b.mesh, b.draw, b.property))
                .collect::<Vec<_>>(),
            identities
        );
        assert!(world
            .bind_spherical_properties(target..target + 1, &sources, &files, &mut HashMap::new())
            .is_err());
        let view = [
            1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
        ];
        let mut lit_draws = world.draws.clone();
        world.apply_spherical_lighting(&mut lit_draws, &view);
        let mut generated = Vec::new();
        for &draw in &world.player_draws {
            let command = &lit_draws[draw];
            ensure!(
                matches!(&command.preview, Some(Ok(_))),
                "Mickey lighting material failed: {:?}",
                command.preview.as_ref().and_then(|p| p.as_ref().err())
            );
            let slot = world
                .spherical_bindings
                .iter()
                .find(|b| b.draw == draw)
                .context("Missing Mickey SH binding")?
                .slot;
            let image = command
                .material_images
                .iter()
                .find(|i| u32::from(i.slot) == slot)
                .context("Mickey SH map not attached")?;
            ensure!(
                image.width == 16 && image.height == 16,
                "Wrong native SH dimensions"
            );
            ensure!(
                image.rgba.chunks_exact(4).any(|p| p[..3] != [0, 0, 0]),
                "Production Mickey SH image is black"
            );
            generated.push((draw, slot, image.rgba.clone()));
        }
        world.apply_spherical_lighting(&mut lit_draws, &view);
        for (draw, slot, pixels) in &generated {
            let image = lit_draws[*draw]
                .material_images
                .iter()
                .find(|i| u32::from(i.slot) == *slot)
                .unwrap();
            ensure!(
                Arc::ptr_eq(pixels, &image.rgba),
                "Unchanged SH image was reallocated"
            );
        }
        println!(
            "Attached generated SH lighting to {} Mickey draws; unchanged image storage reused",
            generated.len()
        );
        let lights = std::mem::take(&mut world.report.spherical_lights);
        world.apply_spherical_lighting(&mut lit_draws, &view);
        for (draw, slot, _) in &generated {
            let image = lit_draws[*draw]
                .material_images
                .iter()
                .find(|i| u32::from(i.slot) == *slot)
                .unwrap();
            ensure!(
                image.rgba.chunks_exact(4).all(|p| p == [0, 0, 0, 255]),
                "No-light scene received invented lighting"
            );
        }
        world.report.spherical_lights = lights;
        ensure!(
            !world.model_bounds.is_empty(),
            "No model bound hierarchies retained"
        );
        let mut placed_models = 0;
        for (index, entity) in world.entities.iter().enumerate() {
            if entity.draws.is_empty() {
                continue;
            }
            if let Some(hierarchy) = world.entity_bound_hierarchy(index)? {
                placed_models += 1;
                for (&mesh, draws) in &entity.mesh_draws {
                    ensure!(
                        hierarchy.nodes.contains_key(&mesh),
                        "Draw mesh {mesh} missing from {} hierarchy",
                        entity.name
                    );
                    ensure!(
                        draws.iter().all(|draw| entity.draws.contains(draw)),
                        "Draw outside model instance {}",
                        entity.name
                    );
                }
            } else {
                ensure!(
                    entity.draws.is_empty(),
                    "Draws without model identity on {}",
                    entity.name
                );
            }
        }
        println!("Resolved {placed_models} placed model identities to bound hierarchies");
        ensure!(
            !world.spherical_bindings.is_empty(),
            "No SH mesh bindings initialized"
        );
        let shared_links = world.lighting_owner_links(&world.child_lighting_properties)?;
        let mut mesh_properties = BTreeMap::new();
        let mut property_ids = std::collections::BTreeSet::new();
        for binding in &world.spherical_bindings {
            let property = world.spherical_properties.get(binding.property)?;
            let hierarchy = world
                .entity_bound_hierarchy(property.owner.entity as usize)?
                .context("SH owner model missing")?;
            ensure!(
                hierarchy.nodes.contains_key(&property.owner.node),
                "SH owner root absent"
            );
            if let Some(previous) =
                mesh_properties.insert((binding.entity, binding.mesh), binding.property)
            {
                ensure!(
                    previous == binding.property,
                    "One mesh acquired different SH properties"
                );
            }
            if let Some(inherited) = super::super::spherical_lighting::inherited_property(
                binding.entity as u64,
                None,
                &shared_links,
            )? {
                ensure!(
                    binding.property == inherited,
                    "SH mesh ignored shared ancestor"
                );
            } else {
                ensure!(
                    property.owner.entity == binding.entity as u64,
                    "Unshared SH property crosses model instances"
                );
            }
            property_ids.insert(binding.property);
        }
        ensure!(
            world
                .player_draws
                .iter()
                .all(|draw| world.spherical_bindings.iter().any(|b| b.draw == *draw)),
            "Mickey draw missing generated-lighting ownership"
        );
        println!(
            "Initialized {} SH draw bindings, {} referenced properties, {} child-shared properties",
            world.spherical_bindings.len(),
            property_ids.len(),
            world.child_lighting_properties.len()
        );
        std::fs::write(
            "target/production-sh-bindings.json",
            serde_json::to_vec_pretty(&world.spherical_bindings)?,
        )?;
        for &draw in &world.player_draws {
            let material = world.draws[draw]
                .material
                .as_ref()
                .context("Missing Mickey material")?;
            let flags = material
                .vertex_color_flags
                .clone()
                .transpose()
                .map_err(anyhow::Error::msg)?;
            println!(
                "Mickey shader {:?}, vertex flags {flags:?}, R3MTLightingFlags {:?}",
                material.shader,
                material.integers.get("R3MTLightingFlags")
            );
        }
        let owner_links = world.lighting_owner_links(&Default::default())?;
        let sharing = world
            .entities
            .iter()
            .map(RuntimeEntity::children_share_lighting)
            .collect::<Result<Vec<_>>>()?;
        println!(
            "{} entities explicitly request shared child lighting",
            sharing.iter().filter(|v| **v).count()
        );
        for &id in owner_links.keys() {
            ensure!(
                super::super::spherical_lighting::inherited_property(id, None, &owner_links)?
                    .is_none(),
                "Unexpected SH property before attachment"
            );
        }
        println!(
            "Resolved {} runtime transform links ({} parented)",
            owner_links.len(),
            owner_links.values().filter(|v| v.parent.is_some()).count()
        );
        let summaries: std::collections::BTreeMap<_, _> = world
            .model_bounds
            .iter()
            .map(|(asset, h)| {
                (
                    asset,
                    serde_json::json!({"roots":h.roots,"nodes":h.nodes.len(),
                "authored_spheres":h.nodes.values().filter(|n|n.authored_bound.is_some()).count(),
                "dynamic_nodes":h.nodes.values().filter(|n|n.needs_runtime_bound).count()}),
                )
            })
            .collect();
        std::fs::write(
            "target/production-bound-hierarchies.json",
            serde_json::to_vec_pretty(&summaries)?,
        )?;
        let errors: Vec<_> = world
            .report
            .unsupported
            .iter()
            .filter(|s| s.starts_with("Model bound hierarchy "))
            .collect();
        println!(
            "{} unresolved non-drawable model hierarchies: {errors:?}",
            errors.len()
        );
        let mut static_bounds = std::collections::BTreeMap::new();
        let mut runtime_bounds = std::collections::BTreeMap::new();
        for (asset, hierarchy) in &world.model_bounds {
            let dynamic:Vec<_>=hierarchy.nodes.iter().filter(|(_,n)|n.needs_runtime_bound)
                .map(|(id,n)|serde_json::json!({"node":id,"class":n.class_name,"controllers":n.controllers})).collect();
            if dynamic.is_empty() {
                let bounds = hierarchy
                    .static_world_bounds(None, &Default::default())
                    .with_context(|| format!("Static production bounds: {asset}"))?;
                ensure!(
                    bounds.len() == hierarchy.nodes.len(),
                    "Incomplete bounds for {asset}"
                );
                static_bounds.insert(asset, bounds);
            } else {
                runtime_bounds.insert(asset, dynamic);
            }
        }
        ensure!(
            !static_bounds.is_empty(),
            "No production static bounds evaluated"
        );
        std::fs::write(
            "target/production-world-bounds.json",
            serde_json::to_vec_pretty(
                &serde_json::json!({"space":"model root; entity transform not applied",
                "static":static_bounds,"requires_runtime":runtime_bounds}),
            )?,
        )?;
        println!(
            "Evaluated {} static model graphs; {} require runtime updates",
            static_bounds.len(),
            runtime_bounds.len()
        );
        std::fs::write(
            "target/production-spherical-lights.json",
            serde_json::to_vec_pretty(&world.report.spherical_lights)?,
        )?;
        let errors: Vec<_> = world
            .report
            .unsupported
            .iter()
            .filter(|s| s.starts_with("SH light "))
            .collect();
        ensure!(
            errors.is_empty(),
            "Unresolved production lights: {errors:?}"
        );
        ensure!(
            !world.report.spherical_lights.is_empty(),
            "No production SH lights decoded"
        );
        ensure!(world.report.spherical_lights.iter().all(|l|!l.raycast),
            "This diagnostic requires authored non-raycasting lights; visibility cannot be invented");
        for light in &world.report.spherical_lights {
            ensure!(
                world
                    .entities
                    .iter()
                    .any(|e| e.scene == light.scene && e.id == light.entity_id),
                "Unplaced light registered: {}",
                light.name
            );
        }
        println!(
            "Decoded {} placed runtime SH lights",
            world.report.spherical_lights.len()
        );
        // Actual authored light data, sampled at the spawn and three placed
        // light positions. Fixed diagnostic cameras, not original-game frames.
        let mut atlas = image::RgbaImage::new(64, 32);
        let positions = [
            world.report.spawn,
            world.report.spherical_lights[0].light.position,
            world.report.spherical_lights[1].light.position,
            world.report.spherical_lights[2].light.position,
        ];
        for (row, eye) in [[0., 0., 10.], [10., 0., 0.]].into_iter().enumerate() {
            let view = look_at_matrix(eye, [0.; 3], [0., 1., 0.]);
            for (column, position) in positions.into_iter().enumerate() {
                let bytes = super::super::spherical_lighting::view_texture(
                    world.report.spherical_lights.iter().map(|l| (&l.light, 1.)),
                    position,
                    &view,
                )?;
                for y in 0..16 {
                    for x in 0..16 {
                        let i = (y * 16 + x) * 4;
                        atlas.put_pixel(
                            (column * 16 + x) as u32,
                            (row * 16 + y) as u32,
                            image::Rgba(bytes[i..i + 4].try_into()?),
                        );
                    }
                }
            }
        }
        image::imageops::resize(&atlas, 768, 384, image::imageops::FilterType::Nearest)
            .save("target/production-sh-camera-diagnostic.png")?;
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn production_player_visual_transforms() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let player = &world.entities[world.player_entity];
        for yaw in [0., 1., -2.] {
            let matrix = player.controller_matrix(world.report.spawn, yaw);
            assert!(
                (matrix[8].atan2(matrix[10]) - yaw).abs() < 1e-5,
                "Spawn rotation added to world heading"
            );
        }
        let pose = super::super::skin::model_pose(&world.player_animations.rig.bones, &[])?;
        let actor = player.controller_matrix(world.report.spawn, 0.);
        let mut draws = Vec::new();
        world.brush.append(&pose, actor, &mut draws)?;
        let view = [
            1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
        ];
        world
            .brush
            .apply_lighting(&mut draws, &view, &world.report.spherical_lights)?;
        let mut lit = Vec::new();
        for (index, draw) in draws.iter().enumerate() {
            let program = draw
                .preview
                .as_ref()
                .context("Missing brush program")?
                .as_ref()
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            if let Some(slot) = program
                .first_runtime_sh_slot(draw.material.as_ref().context("Missing brush material")?)
            {
                let image = draw
                    .material_images
                    .iter()
                    .find(|i| u32::from(i.slot) == slot)
                    .context("Brush lighting map not attached")?;
                ensure!(
                    image.rgba.chunks_exact(4).any(|p| p[..3] != [0, 0, 0]),
                    "Lit-level brush SH map is black"
                );
                lit.push((index, slot, image.rgba.clone()));
            }
            let (lo, hi) = transform_aabb(draw.aabb_min, draw.aabb_max, &draw.trans_mat);
            let extent = super::super::physics::length(super::super::physics::sub(hi, lo));
            assert!(
                extent > 0.5 && extent < 1.5,
                "Stock brush shrunk in the attachment: {extent}"
            );
        }
        ensure!(!lit.is_empty(), "No brush SH maps tested");
        world
            .brush
            .apply_lighting(&mut draws, &view, &world.report.spherical_lights)?;
        for (index, slot, pixels) in &lit {
            let image = draws[*index]
                .material_images
                .iter()
                .find(|i| u32::from(i.slot) == *slot)
                .unwrap();
            ensure!(
                Arc::ptr_eq(pixels, &image.rgba),
                "Unchanged brush image was reallocated"
            );
        }
        for draw in &mut draws {
            draw.trans_mat[12] += 1000.;
        }
        world
            .brush
            .apply_lighting(&mut draws, &view, &world.report.spherical_lights)?;
        for (index, slot, pixels) in &lit {
            let image = draws[*index]
                .material_images
                .iter()
                .find(|i| u32::from(i.slot) == *slot)
                .unwrap();
            ensure!(
                image.rgba.as_slice() != pixels.as_slice(),
                "Brush lighting did not follow attachment motion"
            );
        }
        println!(
            "Validated {} attached brush lighting maps, image reuse and moving sample position",
            lit.len()
        );
        let excluded: Vec<_> = world
            .report
            .geometry
            .iter()
            .filter(|e| e["kind"] == "unsupported_particle_draw")
            .collect();
        ensure!(
            !excluded.is_empty(),
            "Production particle buffers were not classified"
        );
        for particle in &excluded {
            assert!(!world
                .report
                .geometry
                .iter()
                .any(|draw| draw["kind"] == "render"
                    && draw["entity"] == particle["entity"]
                    && draw["asset"] == particle["asset"]
                    && draw["nif_mesh_block"] == particle["nif_mesh_block"]));
        }
        println!(
            "Excluded {} serialized particle buffers from static rendering",
            excluded.len()
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn production_projectiles_erase_and_restore_jigsaw() -> Result<()> {
        use super::super::{
            brush::Ink,
            physics::{add, cross, mul, point, sub, unit},
        };
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        // Find an exposed authored face in the fully loaded production world,
        // retaining surrounding collision instead of isolating a synthetic target.
        let mut launch = None;
        'search: for instance in &world.jigsaw_instances {
            let entity = &world.entities[instance.entity];
            let asset = &world.jigsaws[entity.jigsaw_asset.as_ref().unwrap()][instance.node];
            for part in &asset.collision.parts {
                let vertices = &asset.collision.vertices[part.vertex_stream];
                for face in part.indices.chunks_exact(3) {
                    let [a, b, c] = std::array::from_fn(|i| {
                        point(&entity.transform, vertices[face[i] as usize])
                    });
                    let n = unit(cross(sub(b, a), sub(c, a)));
                    let center = mul(add(add(a, b), c), 1. / 3.);
                    let origin = add(center, mul(n, 0.5));
                    let target = sub(center, mul(n, 0.5));
                    if let Some(hit) =
                        world
                            .collision
                            .sweep_sphere(origin, sub(target, origin), 0.025)?
                    {
                        if world.jigsaw_owners.contains_key(&hit.owner) {
                            launch = Some((origin, target, hit.owner));
                            break 'search;
                        }
                    }
                }
            }
        }
        let (origin, target, owner) = launch.context("No exposed production jigsaw face")?;
        world.preview_jigsaw_contact(owner, true)?;
        let source = world.jigsaw_owners[&owner];
        let instance = &world.jigsaw_instances[source.instance];
        let entity = instance.entity;
        let node = instance.node;
        let hidden = world.hidden_jigsaw_draws.clone();
        let sketch = world.sketch_draws.clone();
        for ink in [Ink::Thinner, Ink::Paint] {
            world.projectile_preview.spawn(
                &world.report,
                ink,
                world.player_entity as u64,
                origin,
                target,
            )?;
            let mut dispatched = false;
            for _ in 0..30 {
                world.advance_brush_projectiles(1. / 30.)?;
                let contacts = world.projectile_preview.step_contacts.clone();
                for contact in contacts {
                    assert_eq!(
                        contact.hit.owner, owner,
                        "Projectile hit a different production target"
                    );
                    assert!(world.brush_jigsaw_hit(
                        &contact.hit,
                        contact.ink == Ink::Paint,
                        [10000.; 3]
                    )?);
                    dispatched = true;
                }
                if dispatched {
                    break;
                }
            }
            ensure!(dispatched, "Projectile never reached jigsaw");
            assert!(world.jigsaw_effect_queue.len() > 0);
            let shader_before: Vec<_> = world
                .draws
                .iter()
                .filter_map(|d| d.material_registers)
                .collect();
            assert!(!shader_before.is_empty(), "No animated transition draws");
            for _ in 0..60 {
                world.advance_jigsaw_effects(1. / 30.)?;
            }
            assert_eq!(world.jigsaw_effect_queue.len(), 0);
            assert!(world.jigsaw_effect_values.is_empty());
            assert_eq!(
                world.entities[entity].jigsaw_states[node].painted[source.piece],
                ink == Ink::Paint
            );
        }
        assert_eq!(world.hidden_jigsaw_draws, hidden);
        assert_eq!(world.sketch_draws, sketch);
        assert!(world.brush_jigsaw_contact(owner, false)?);
        for _ in 0..10 {
            world.advance_jigsaw_effects(1. / 30.)?;
        }
        assert!(world.brush_jigsaw_contact(owner, true)?);
        for _ in 0..20 {
            world.advance_jigsaw_effects(1. / 30.)?;
        }
        assert_eq!(world.jigsaw_effect_queue.len(), 0);
        assert_eq!(world.hidden_jigsaw_draws, hidden);
        assert_eq!(world.sketch_draws, sketch);
        println!("Production shot roundtrip: owner={owner}, origin={origin:?}, target={target:?}");
        // Exercise the actual projectile dispatch with Mickey intersecting the
        // original target, and verify rejection leaves render/collision state
        // and the transition queue untouched. This is not Havok parity proof.
        let hit = world
            .collision
            .sweep_sphere(origin, sub(target, origin), 0.025)?
            .context("Lost original target")?;
        assert!(world.brush_jigsaw_hit(&hit, false, hit.position)?);
        for _ in 0..60 {
            world.advance_jigsaw_effects(1. / 30.)?;
        }
        let instance = &world.jigsaw_instances[source.instance];
        let asset = &world.jigsaws[world.entities[entity].jigsaw_asset.as_ref().unwrap()][node];
        let state = &world.entities[entity].jigsaw_states[node];
        let hull = world.report.player_collision.as_ref().unwrap();
        let hull_center = std::array::from_fn(|axis| {
            let lo = hull
                .vertices
                .iter()
                .map(|p| p[axis])
                .fold(f32::INFINITY, f32::min);
            let hi = hull
                .vertices
                .iter()
                .map(|p| p[axis])
                .fold(f32::NEG_INFINITY, f32::max);
            (lo + hi) * 0.5
        });
        let overlapping = sub(add(hit.position, mul(hit.normal, 0.02)), hull_center);
        ensure!(
            instance.collision.contacts_reject_restore(
                asset,
                state,
                &world.collision,
                overlapping,
                u32::MAX
            )?,
            "Original face did not produce a rejecting overlap"
        );
        assert!(!world.brush_jigsaw_hit(&hit, true, overlapping)?);
        assert!(!world.entities[entity].jigsaw_states[node].painted[source.piece]);
        assert_eq!(world.jigsaw_effect_queue.len(), 0);
        world.report.jigsaw_overlap_guard_enabled = false;
        assert!(world.brush_jigsaw_hit(&hit, true, overlapping)?);
        assert!(world.entities[entity].jigsaw_states[node].painted[source.piece]);
        Ok(())
    }

    #[test]
    #[ignore = "requires an imported EM1 DATA/files directory in BLOTWORX_TEST_DATA"]
    fn production_jigsaw_endpoint_roundtrip() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let owners: Vec<_> = world.jigsaw_owners.keys().copied().collect();
        ensure!(!owners.is_empty(), "No production jigsaw owners");
        let hidden = world.hidden_jigsaw_draws.clone();
        let sketch = world.sketch_draws.clone();
        for owner in owners {
            let source = world.jigsaw_owners[&owner];
            let instance = &world.jigsaw_instances[source.instance];
            let initial =
                world.entities[instance.entity].jigsaw_states[instance.node].painted[source.piece];
            assert!(world.preview_jigsaw_contact(owner, !initial)?);
            assert!(!world.preview_jigsaw_contact(owner, !initial)?);
            assert!(world.preview_jigsaw_contact(owner, initial)?);
        }
        assert_eq!(world.hidden_jigsaw_draws, hidden);
        assert_eq!(world.sketch_draws, sketch);
        Ok(())
    }

    #[test]
    fn production_part_scene_options_do_not_discard_geometry() -> anyhow::Result<()> {
        assert_eq!(
            super::initial_scenes(
                Some("World/shared.gsa"),
                "// gsas:\r\nWorld/ZoneA.gsa -i\r\n// audio:\r\nglobal.bnk"
            )?,
            ["World/shared.bin", "World/ZoneA.bin"]
        );
        assert_eq!(
            super::scene_reference("World/ZoneA.gsa -i")?,
            Some(("World/ZoneA.bin".into(), vec!["-i".into()]))
        );
        assert_eq!(
            super::initial_scenes(None, "# ignored.gsa\nWorld/ZoneA.gsa -INACTIVE")?,
            ["World/ZoneA.bin"]
        );
        assert!(super::scene_reference("World/ZoneA.gsa -unknown").is_err());
        Ok(())
    }

    use super::*;
    #[test]
    fn live_entity_pose_preserves_authored_basis_and_does_not_accumulate_yaw() {
        let authored = [
            2., 0., 0., 0., 0., 3., 0., 0., 0., 0., 4., 0., 10., 20., 30., 1.,
        ];
        let mut entity = RuntimeEntity {
            authored_scale: 1.,
            authored_rotation: [1., 0., 0., 0., 1., 0., 0., 0., 1.],
            runtime_rotation: [1., 0., 0., 0., 1., 0., 0., 0., 1.],
            model_asset: None,
            mesh_draws: HashMap::new(),
            jigsaw_asset: None,
            jigsaw_states: Vec::new(),
            scene: "test.bin".into(),
            id: 7,
            name: "Player".into(),
            transform: authored,
            authored_transform: authored,
            components: vec![],
            draws: 0..0,
            collision_parts: 0..0,
        };
        entity.set_controller_pose([1., 2., 3.], std::f32::consts::FRAC_PI_2);
        let once = entity.transform;
        entity.set_controller_pose([1., 2., 3.], std::f32::consts::FRAC_PI_2);
        assert_eq!(entity.transform, once);
        assert_eq!(entity.authored_transform, authored);
        assert!(entity.physics_transform().iter().all(|v| v.is_finite()));
        assert_eq!(&entity.transform[12..15], &[1., 2., 3.]);
        assert!((entity.transform[2] + 2.).abs() < 1e-6);
        entity.set_controller_pose([10., 20., 30.], 0.);
        assert_eq!(entity.transform, authored);
        entity.authored_scale = 0.;
        entity.authored_rotation = [0., 0., 1., 0., 1., 0., -1., 0., 0.];
        entity.authored_transform[..12].fill(0.);
        entity.set_controller_pose([10., 20., 30.], std::f32::consts::FRAC_PI_2);
        assert_eq!(
            entity.runtime_rotation, entity.authored_rotation,
            "Zero render scale lost the unscaled owner orientation"
        );
        assert_eq!(entity.physics_transform()[8], 1.);
        assert!(entity.transform[..12].iter().all(|&v| v == 0.));
    }
    #[test]
    fn shared_scene_loads_with_soundbank_only_part() {
        assert_eq!(
            initial_scenes(
                Some("Environments/_Test/Building_Standards.gsa"),
                "\r\n//Soundbanks list\r\nGlobal.bnk"
            )
            .unwrap(),
            ["Environments/_Test/Building_Standards.bin"]
        );
    }
    #[test]
    fn shared_and_part_scenes_preserve_order_without_duplicate_spawns() {
        assert_eq!(
            initial_scenes(
                Some("World/Shared.gsa"),
                "world/shared.GSA\nWorld/Room.gsa\nWorld/Other.bin\nGlobal.bnk"
            )
            .unwrap(),
            ["World/Shared.bin", "World/Room.bin", "World/Other.bin"]
        );
        assert!(initial_scenes(None, "Global.bnk").is_err());
    }

    #[test]
    #[ignore = "requires an imported EM1 DATA/files directory in BLOTWORX_TEST_DATA"]
    fn recovery_restarts_brush_in_new_authored_graph() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/EB_VillageTest.level",
            &egui::Context::default(),
        )?;
        let mut preview = super::super::authored_preview::Preview::new(&mut world)?;
        preview.diagnostic_brush_direction = true;
        let binding = super::super::brush::AnimationBindings::load(&world.player_behavior)?;
        let mut player =
            super::super::physics::Player::new(world.report.spawn, world.report.motion.clone());
        preview.brush_transition(
            true,
            &world.player_behavior,
            &mut world.player_graph,
            &mut world.player_animations,
        )?;
        assert_eq!(world.player_graph.state.words()[binding.state], 1);
        player.respawn();
        assert!(preview.synchronize_player(
            &player,
            &world.player_behavior,
            &mut world.player_graph,
            &mut world.player_animations
        )?);
        assert!(preview.diagnostic_brush_direction);
        assert_eq!(world.player_graph.state.words()[binding.state], 0);
        preview.brush_transition(
            true,
            &world.player_behavior,
            &mut world.player_graph,
            &mut world.player_animations,
        )?;
        assert_eq!(world.player_graph.state.words()[binding.state], 1);
        assert_eq!(preview.delivered_events.last().unwrap().id, binding.start);
        assert!(!preview.synchronize_player(
            &player,
            &world.player_behavior,
            &mut world.player_graph,
            &mut world.player_animations
        )?);
        assert_eq!(world.player_graph.state.words()[binding.state], 1);
        Ok(())
    }

    #[test]
    #[ignore = "requires an imported EM1 DATA/files directory in BLOTWORX_TEST_DATA"]
    fn brush_strafe_animation_roundtrip() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = load(
            Path::new(&root),
            "Levels/EB_VillageTest.level",
            &egui::Context::default(),
        )?;
        let binding = super::super::brush::AnimationBindings::load(&world.player_behavior)?;
        assert_eq!((binding.start, binding.stop, binding.state), (138, 139, 47));
        let mut preview = super::super::authored_preview::Preview::new(&mut world)?;
        preview.advance(&mut world, 15)?;
        preview.brush_transition(
            true,
            &world.player_behavior,
            &mut world.player_graph,
            &mut world.player_animations,
        )?;
        assert_eq!(world.player_graph.state.words()[binding.state], 1);
        assert_eq!(preview.delivered_events.last().unwrap().id, binding.start);
        let mut shuffle = super::super::brush::AimShuffle::default();
        for (yaw, index) in [(0.5, 0), (0., 2), (-0.5, 1), (0., 2)] {
            preview.brush_shuffle(
                &mut shuffle,
                yaw,
                [0.; 3],
                [1., 4.],
                0.25,
                &world.player_behavior,
                &mut world.player_graph,
                &mut world.player_animations,
            )?;
            assert_eq!(
                preview.delivered_events.last().unwrap().id,
                binding.shuffle[index]
            );
            preview.advance(&mut world, 15)?;
        }

        // Settle down, then cross to the other endpoint. A topology rebuild
        // must preserve this tick's activation flag until the next update.
        preview.brush_aim(&mut world.player_graph.state, -0.4, 0.)?;
        preview.advance(&mut world, 15)?;
        preview.brush_aim(&mut world.player_graph.state, 0.707, 0.)?;
        preview.advance(&mut world, 1)?;
        let blend = &world.player_graph.blenders["2:16720"];
        assert!(blend[1].active && blend[1].newly_active);
        assert!(!blend[0].active);
        let sampled = blend.clone();
        // A later variable write plus an unrelated replan must not resample
        // weights mid-tick or clear the newly-active synchronization marker.
        preview.brush_aim(&mut world.player_graph.state, -0.4, 0.)?;
        assert!(super::super::generator_plan::rebuild_in_place(
            &world.player_behavior,
            &mut world.player_graph
        )?
        .is_empty());
        assert_eq!(world.player_graph.blenders["2:16720"], sampled);
        preview.brush_aim(&mut world.player_graph.state, 0.707, 0.)?;
        preview.advance(&mut world, 1)?;
        assert!(!world.player_graph.blenders["2:16720"][1].newly_active);
        for elevation in [-0.4f32, 0.707, 0.] {
            preview.brush_aim(&mut world.player_graph.state, elevation, 0.)?;
            preview.advance(&mut world, 15)?;
            let expected = if elevation < 0. {
                "mickey_idle_strafe_mid_down.hkx"
            } else {
                "mickey_idle_strafe_mid_up.hkx"
            };
            let clips: Vec<_> = world
                .player_graph
                .plan
                .nodes
                .iter()
                .filter_map(|n| {
                    world
                        .player_graph
                        .state
                        .clips
                        .get(&n.object)
                        .map(|c| c.settings.animation_name.as_str())
                })
                .collect();
            assert!(clips.contains(&expected), "Aim {elevation}: {clips:?}");
            if elevation != 0. {
                assert_eq!(clips.len(), 1, "Endpoint should select one authored pose");
            }
        }
        preview.advance(&mut world, 15)?;
        let clips: Vec<_> = world
            .player_graph
            .plan
            .nodes
            .iter()
            .filter_map(|n| {
                world
                    .player_graph
                    .state
                    .clips
                    .get(&n.object)
                    .map(|c| c.settings.animation_name.clone())
            })
            .collect();
        println!("Brush animation clips: {clips:?}");
        assert!(clips
            .iter()
            .any(|name| name.to_ascii_lowercase().contains("strafe")));
        preview.brush_transition(
            false,
            &world.player_behavior,
            &mut world.player_graph,
            &mut world.player_animations,
        )?;
        assert_eq!(world.player_graph.state.words()[binding.state], 0);
        assert_eq!(preview.delivered_events.last().unwrap().id, binding.stop);
        preview.advance(&mut world, 30)?;
        assert!(preview.pose.iter().all(|b| b
            .local_translation
            .iter()
            .chain(&b.local_rotation)
            .all(|v| v.is_finite())));
        Ok(())
    }
    #[test]
    #[ignore = "requires an imported EM1 DATA/files directory in BLOTWORX_TEST_DATA"]
    fn village_collision_provenance_covers_world() {
        let root = std::env::var_os("BLOTWORX_TEST_DATA")
            .expect("Set BLOTWORX_TEST_DATA to imported DATA/files");
        let world = load(
            Path::new(&root),
            "Levels/EB_VillageTest.level",
            &egui::Context::default(),
        )
        .unwrap();
        let brush = world.report.brush_assets.as_ref().unwrap();
        assert_eq!(
            world.player_animations.rig.bones[world.brush.bone].name,
            "SKEL_weapon"
        );
        assert!(!world.brush.draws.is_empty());
        let mut pose =
            super::super::skin::model_pose(&world.player_animations.rig.bones, &[]).unwrap();
        let actor = [
            0.1, 0., 0., 0., 0., 0.1, 0., 0., 0., 0., 0.1, 0., 7., 8., 9., 1.,
        ];
        let mut attached = Vec::new();
        world.brush.append(&pose, actor, &mut attached).unwrap();
        let before = attached[0].trans_mat;
        pose[world.brush.bone][12] += 10.;
        attached.clear();
        world.brush.append(&pose, actor, &mut attached).unwrap();
        assert!((attached[0].trans_mat[12] - before[12] - 1.).abs() < 0.00001);
        assert_eq!(&attached[0].trans_mat[13..], &before[13..]);
        assert!(world.brush.append(&[], actor, &mut attached).is_err());
        assert_eq!(
            brush.definitions["magicbrush"].children,
            ["thinnerstream", "paintstream", "mk_hose_nozzle"]
        );
        assert!(
            brush.models.is_empty(),
            "Decoded brush NIF payloads should not remain resident"
        );
        for (name, model) in [
            ("paintstream", "Effects/Characters/Mickey/MK_Paint_Hose.nif"),
            (
                "thinnerstream",
                "Effects/Characters/Mickey/MK_Thinner_Hose.nif",
            ),
        ] {
            let definition = &brush.definitions[name];
            assert_eq!(definition.model.as_deref(), Some(model));
            let particles = definition
                .components
                .iter()
                .find(|c| c.class_name == "ParticleSystemControllerComponent")
                .unwrap();
            assert_eq!(
                particles
                    .properties
                    .iter()
                    .find(|p| p.name == "StartEmittersEnabled")
                    .unwrap()
                    .value,
                serde_json::json!(false)
            );
        }
        for name in &world
            .report
            .brush_settings
            .as_ref()
            .unwrap()
            .projectile_templates
        {
            let template = &world.report.brush_templates[name];
            assert!(template["components"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["class"] == "ProjectileComponent"));
            let stimuli = &world.brush_stimuli[name];
            assert_eq!(
                stimuli.types,
                vec![if name.eq_ignore_ascii_case("paintball") {
                    1
                } else {
                    0
                }]
            );
            assert_eq!(stimuli.amounts, vec![0.5]);
            assert_eq!(stimuli.radii, vec![1.]);
            assert_eq!(stimuli.impact_modes, vec![0]);
            let decal = &world.brush_decals[name];
            let request = decal.request(-1, [0.; 3], [0., 1., 0.], || 0).unwrap();
            assert!(!decal.sources[request.material].is_empty());
            let material = &decal.templates[request.material];
            println!(
                "{name} colors {:?}; flags {:?}",
                material.colors, material.integers
            );
            assert!(material.mesh.is_some(), "Missing material source mesh");
            assert!(material.shader().is_some(), "Missing decal shader");
            let transform = [
                1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 2., 3., 4., 1.,
            ];
            let command = material
                .draw(
                    super::super::decal_geometry::Projected {
                        positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
                        indices: vec![0, 1, 2],
                        uv: vec![[0., 0.], [1., 0.], [0., 1.]],
                    },
                    transform,
                )
                .expect("Retail decal draw preparation failed");
            assert_eq!(command.trans_mat, transform);
            assert_eq!(command.v_data.len(), 36);
            assert_eq!(command.aabb_max, [1., 1., 0.]);
            assert_eq!(command.material_images.len(), 1);
            assert!(
                !material.bindings.textures.is_empty(),
                "Missing decal texture bindings"
            );
            match material
                .rendering
                .as_ref()
                .expect("Decal material not prepared")
            {
                Ok(prepared) => {
                    let expected = if name.eq_ignore_ascii_case("paintball") {
                        [55., 139., 213., 255.]
                    } else {
                        [100., 132., 29., 255.]
                    };
                    let registers = prepared
                        .program
                        .register_defaults
                        .as_ref()
                        .expect("Decal colors must use runtime register uniforms");
                    assert_eq!(
                        registers[1],
                        expected.map(|v| v / 255.),
                        "Authored splat color missing from register 2"
                    );
                    println!(
                        "Decal prepared: slots {:?}, {} images",
                        prepared.program.texture_slots,
                        prepared.images.len()
                    );
                }
                Err(error) => panic!("Retail decal material preparation failed: {error}"),
            }
            for texture in &material.bindings.textures {
                println!("Decal texture slot {}: {}", texture.slot, texture.path);
            }
            println!(
                "{name} decal {}: mesh {:?}, shader {:?}",
                request.material,
                material.mesh,
                material.shader()
            );
            assert_eq!(request.radius, 2.);
            assert_eq!(request.lifetime, 5.);
            assert_eq!(request.position, [0., 0.05, 0.]);
        }
        let mut end = 0usize;
        let ammo = world.report.ammo_settings.as_ref().unwrap();
        let inventory = world.report.authored_ammo.as_ref().unwrap();
        assert_eq!(inventory.maxima, [Some(30.), Some(30.)]);
        assert!(inventory.amounts.iter().all(|n| n.is_finite()));
        assert_eq!(ammo.channels[0].recharge_rate, 2.);
        assert_eq!(ammo.channels[1].recharge_max, 10.);
        assert_eq!(world.report.brush_shapes.len(), 1);
        let body = world.report.brush_shapes.values().next().unwrap();
        assert_eq!(body.radius.to_bits(), 0x3cccccd0);
        assert_eq!(body.motion_type, 2);
        assert_eq!(body.linear_velocity, [0.; 3]);
        // The player is imported through a dedicated path and is intentionally
        // excluded from the serialized-placement counter.
        assert_eq!(
            world.entities.len(),
            world.report.placed_entities + 1,
            "Runtime entities include the dedicated player entity in addition to serialized placements"
        );
        let mut draw_end = 0;
        let mut part_end = 0;
        for entity in &world.entities {
            assert_eq!(entity.draws.start, draw_end);
            assert_eq!(entity.collision_parts.start, part_end);
            for part in entity.collision_parts.clone() {
                let owner = world.collision_entity(part).unwrap();
                assert_eq!((&owner.scene, owner.id), (&entity.scene, entity.id));
                assert_eq!(
                    owner.projectile_target_body(),
                    Some(super::super::projectile_stimulus::TargetBody::Static)
                );
                assert!(owner.projectile_real_surface(false).unwrap());
            }
            draw_end = entity.draws.end;
            part_end = entity.collision_parts.end;
        }
        assert_eq!(draw_end, world.draws.len());
        assert_eq!(part_end, world.report.collision_parts.len());
        let mut identities = std::collections::HashSet::new();
        let mut shared_resources = HashMap::new();
        for part in &world.report.collision_parts {
            if let Some(prior) =
                shared_resources.insert(key(&part.asset), part.native_resource.clone())
            {
                assert!(
                    Arc::ptr_eq(&prior, &part.native_resource),
                    "Repeated static resource was decoded twice"
                );
            }
            let native = &part.native_resource[part.native_part];
            assert_eq!(native.body_offset, part.body_offset);
            assert_eq!(native.subpart_index, part.subpart_index);
            assert_eq!(native.indices.len() / 3, part.triangle_count);
            for (local, indices) in native.indices.chunks_exact(3).enumerate() {
                for corner in 0..3 {
                    let p = physics::point(
                        &native.body_transform,
                        native.vertices[indices[corner] as usize],
                    );
                    assert_eq!(
                        physics::point(&part.entity_transform, p),
                        world.collision.triangles[part.triangle_start + local].p[corner]
                    );
                }
            }
            assert_eq!(
                part.triangle_start, end,
                "Collision provenance must partition world triangles"
            );
            end = end.checked_add(part.triangle_count).unwrap();
            assert!(end <= world.collision.triangles.len());
            assert!(
                identities.insert((
                    &part.scene,
                    part.entity_id,
                    &part.asset,
                    part.body_offset,
                    part.subpart_index
                )),
                "Body subpart repeated within the same placed entity"
            );
            assert!(part
                .body_transform
                .iter()
                .chain(&part.entity_transform)
                .all(|x| x.is_finite()));
        }
        assert_eq!(end, world.collision.triangles.len());
        assert!(!identities.is_empty());
        let mut player = physics::Player::new(world.report.spawn, world.report.motion.clone());
        for _ in 0..30 {
            player.step(&world.collision, [0.; 3], false, 1. / 30.);
        }
        assert!(player.grounded);
        println!("Native settled position: {:?}", player.position);
        let probe =
            super::super::debug_view::native_contact_probe(&world, player.position).unwrap();
        assert!(
            probe.iter().any(|line| line.contains(" d=")),
            "Debug probe did not expose a settled contact: {probe:?}"
        );
        let owners = world.collision.contact_owners();
        assert!(!owners.is_empty());
        assert!(
            owners
                .iter()
                .all(|owner| owner.is_some_and(|i| i < world.report.collision_parts.len())),
            "Imported hull contact has missing source identity: {owners:?}"
        );
        let sources = world.collision.contact_sources();
        assert_eq!(sources.len(), owners.len());
        let imported = world.report.player_native_support.as_ref().unwrap();
        let mut support = super::super::native_gsk::ConvexSupport {
            packed_vertices: imported.packed_vertices.clone(),
            vertex_count: imported.vertex_count,
            stored_center: imported.stored_center,
        };
        // Box3D's current mover translates an axis-aligned authored hull;
        // this comparison matches that pose, not an inferred retail rotation.
        let player_transform = super::super::native_hk_transform::from_entity(
            [1., 0., 0., 0., 1., 0., 0., 0., 1.],
            player.position,
        );
        let mut observations = Vec::new();
        for (owner, (_, triangle)) in owners.iter().zip(&sources) {
            let owner = owner.unwrap();
            let triangle = triangle.expect("Village contact lost its original triangle");
            let (mut native, transform) = world.initial_static_contact(owner, triangle).unwrap();
            assert_eq!(native.mode, 5);
            assert!(transform.iter().all(|v| v.is_finite()));
            let part = &world.report.collision_parts[owner];
            let local = triangle - part.triangle_start;
            let mesh = &part.native_resource[part.native_part];
            assert_eq!(
                native.support.vertices,
                mesh.triangle_support(local).unwrap().vertices
            );
            assert_eq!(native.radius, mesh.triangle_radius);
            assert!(world
                .initial_static_contact(owner, part.triangle_start + part.triangle_count)
                .is_err());
            let relative =
                super::super::native_hk_transform::relative(&player_transform, &transform);
            let mut cache = super::super::native_gsk::Cache([0; 12]);
            cache
                .initialize_triangle(&mut support, native.support.vertices, &relative)
                .unwrap();
            let result = super::super::shape_welding::query_cached_triangle_contact(
                &mut cache,
                &mut support,
                &mut native.support,
                &player_transform,
                &transform,
                world.report.player_collision.as_ref().unwrap().radius,
                native.radius,
                f32::MAX,
                true,
                &mut [[0.; 4]; 4],
                None,
                true,
                native.mode,
                native.word,
                &world.collision.contact_welding,
            )
            .unwrap();
            use super::super::shape_welding::ClosestQuery;
            let outcome = match result {
                ClosestQuery::Contact(contact) => {
                    assert!(contact
                        .position
                        .iter()
                        .chain(&contact.separating)
                        .all(|v| v.is_finite()));
                    serde_json::json!({"result":"contact","position":contact.position,"separating":contact.separating})
                }
                ClosestQuery::NoContact => serde_json::json!({"result":"welding_rejected"}),
                ClosestQuery::Unresolved => panic!(
                    "Native settled contact unresolved for owner {owner} triangle {triangle}"
                ),
            };
            observations.push(serde_json::json!({"owner":owner,"triangle":triangle,
                "asset":part.asset,"word":native.word,"cache":cache.0,"outcome":outcome}));
        }
        assert!(
            observations
                .iter()
                .any(|o| o["outcome"]["result"] == "contact"),
            "Native query rejected every settled contact"
        );
        std::fs::write("target/village-settled-native-contacts.json",serde_json::to_vec_pretty(&serde_json::json!({
            "player_position":player.position,"original_execution_validated":false,"contacts":observations})).unwrap()).unwrap();
        let start = player.position;
        for tick in 0..60 {
            player.step_input(&world.collision, [0., -1.], [0., 0., 1.], false, 1. / 30.);
            assert!(
                player.grounded,
                "Lost support on spawn's forward route at {tick}: {:?}",
                player.position
            );
            assert!(player
                .position
                .iter()
                .chain(&player.velocity)
                .all(|v| v.is_finite()));
        }
        assert!(
            player.position[2] > start[2] + 5.,
            "Forward input did not traverse the spawn route"
        );
        println!("Native forward route: {:?} -> {:?}", start, player.position);
        println!(
            "VillageTest: {} source subparts account for all {} collision triangles",
            identities.len(),
            end
        );
    }
    #[test]
    fn parses_crlf_descriptor() {
        let d = descriptor("name = test\r\nfolder = Test\r\ninitial=Test\r\n");
        assert_eq!(d["folder"], "Test");
    }
    #[test]
    fn game_folder_selection_resolves_disc_data_files_and_packfiles() -> Result<()> {
        let base = std::env::temp_dir().join(format!(
            "blotworx-root-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        let expected = base.join("DATA/files");
        std::fs::create_dir_all(expected.join("packfiles"))?;
        let result = (|| -> Result<()> {
            for selected in [
                &base,
                &base.join("DATA"),
                &expected,
                &expected.join("packfiles"),
            ] {
                assert_eq!(files_root(selected), expected, "{}", selected.display());
                let pasted = format!("  \"{}\"  ", selected.display());
                assert_eq!(files_root(Path::new(&pasted)), expected);
            }
            Ok(())
        })();
        std::fs::remove_dir(expected.join("packfiles"))?;
        std::fs::remove_dir(&expected)?;
        std::fs::remove_dir(base.join("DATA"))?;
        std::fs::remove_dir(&base)?;
        result
    }
    #[test]
    fn startup_descriptors_accept_legacy_credits_explicit_extensions_and_interior_only(
    ) -> Result<()> {
        let values =
            descriptor_bytes(b"folder=hm\r\ninitial=stream_18.part\r\nauthor=Drew Marr\xe9\r\n")?;
        assert_eq!(
            startup_part(&values)?,
            Some("Levels/hm/stream_18.part".into())
        );
        let values = descriptor_bytes(
            b"folder=dbc\r\nshared=world.gsa\r\ninterior=stream_sleepingbeauty2d\r\n",
        )?;
        assert_eq!(
            startup_part(&values)?,
            Some("Levels/dbc/stream_sleepingbeauty2d.part".into())
        );
        let values = descriptor_bytes(b"folder=hm\ninitial=stream_4_ext\ninterior=stream_4\n")?;
        assert_eq!(
            startup_parts(&values)?,
            vec!["Levels/hm/stream_4_ext.part", "Levels/hm/stream_4.part"]
        );
        let values = descriptor_bytes(b"folder=hm\ninitial=stream_4\ninterior=STREAM_4.part\n")?;
        assert_eq!(startup_parts(&values)?.len(), 1);
        let values = descriptor_bytes(b"shared=world.gsa\r\n")?;
        assert_eq!(startup_part(&values)?, None);
        assert_eq!(
            initial_scenes(Some(&values["shared"]), "")?,
            vec!["world.bin"]
        );
        assert!(descriptor_bytes(b"folder=bad\xff\ninitial=test").is_err());
        Ok(())
    }
}

#[cfg(debug_assertions)]
pub(super) fn validate_streamed_story()->Result<()> {scene_publish::validate_streamed_story()}

#[cfg(debug_assertions)]
pub(super) fn validate_palette_root()->Result<()> {template_instance::validate_palette_root()}

#[cfg(debug_assertions)]
pub(super) fn validate_owned_cleanup()->Result<()> {entity_retirement::validate_owned_cleanup()}
