//! Shared scene resource preparation. Initial loading and incoming scenes use
//! the same decoding path; publication into a live world is a separate stage.
use super::*;

#[derive(Default, Clone)]
pub(super) struct GeometryCache {
    // This is a de-duplication index, not an owner. Strong entries kept every
    // streamed scene's CPU mesh arrays alive after all of its draws were
    // hidden, defeating scene retirement and multiplying memory use across
    // visited areas.
    vertices: HashMap<String, std::sync::Weak<Vec<f32>>>,
    indices: HashMap<String, std::sync::Weak<Vec<u32>>>,
}

#[derive(Default, Clone)]
pub(super) struct StaticCollisionCache {
    meshes: HashMap<String, std::sync::Weak<Vec<super::super::collision::StaticMesh>>>,
    welding:
        HashMap<String, std::sync::Weak<BTreeMap<usize, super::super::shape_welding::Prepared>>>,
}
impl StaticCollisionCache {
    pub(super) fn seed(&self, batch: &mut Batch) {
        for (path, data) in &self.meshes {
            if let Some(data) = data.upgrade() {
                batch.static_collision_resources.insert(path.clone(), data);
            }
        }
        for (path, data) in &self.welding {
            if let Some(data) = data.upgrade() {
                batch.static_welding_resources.insert(path.clone(), data);
            }
        }
    }
    pub(super) fn capture(&mut self, batch: &Batch) {
        self.meshes.extend(
            batch
                .static_collision_resources
                .iter()
                .map(|(path, data)| (path.clone(), Arc::downgrade(data))),
        );
        self.welding.extend(
            batch
                .static_welding_resources
                .iter()
                .map(|(path, data)| (path.clone(), Arc::downgrade(data))),
        );
        self.prune();
    }
    pub(super) fn prune(&mut self) {
        self.meshes.retain(|_, data| data.strong_count() > 0);
        self.welding.retain(|_, data| data.strong_count() > 0);
    }
}

#[derive(Default, Clone)]
pub(super) struct SequenceCache {
    resources: HashMap<String, std::sync::Weak<super::super::nif_sequence::SequenceResources>>,
}
impl SequenceCache {
    pub(super) fn seed(&self, batch: &mut Batch) {
        for (path, resource) in &self.resources {
            if let Some(resource) = resource.upgrade() {
                batch.sequence_resources.insert(path.clone(), Ok(resource));
            }
        }
    }
    pub(super) fn capture(&mut self, batch: &Batch) {
        self.resources.extend(
            batch
                .sequence_resources
                .iter()
                .filter_map(|(path, resource)| {
                    resource
                        .as_ref()
                        .ok()
                        .map(|resource| (path.clone(), Arc::downgrade(resource)))
                }),
        );
        self.prune();
    }
    pub(super) fn prune(&mut self) {
        self.resources
            .retain(|_, resource| resource.strong_count() > 0);
    }
}

#[derive(Default, Clone)]
pub(super) struct ActorBindingCache {
    bindings: HashMap<(String, String), std::sync::Weak<super::super::nif_sequence::ActorBindings>>,
}
impl ActorBindingCache {
    pub(super) fn get(
        &self,
        model: &str,
        manager: &str,
    ) -> Option<Arc<super::super::nif_sequence::ActorBindings>> {
        self.bindings
            .get(&(model.to_owned(), manager.to_owned()))?
            .upgrade()
    }
    pub(super) fn insert(
        &mut self,
        model: String,
        manager: String,
        bindings: &Arc<super::super::nif_sequence::ActorBindings>,
    ) {
        self.bindings
            .insert((model, manager), Arc::downgrade(bindings));
    }
    pub(super) fn prune(&mut self) {
        self.bindings
            .retain(|_, binding| binding.strong_count() > 0);
    }
}

impl GeometryCache {
    fn vertex(&mut self, key: &str, source: &Arc<Vec<f32>>) -> Arc<Vec<f32>> {
        if let Some(shared) = self.vertices.get(key).and_then(std::sync::Weak::upgrade) {
            return shared;
        }
        self.vertices
            .insert(key.to_owned(), Arc::downgrade(source));
        source.clone()
    }
    fn indices(&mut self, key: &str, source: &Arc<Vec<u32>>) -> Arc<Vec<u32>> {
        if let Some(shared) = self.indices.get(key).and_then(std::sync::Weak::upgrade) {
            return shared;
        }
        self.indices
            .insert(key.to_owned(), Arc::downgrade(source));
        source.clone()
    }
    pub(super) fn prune(&mut self) {
        self.vertices.retain(|_, data| data.strong_count() > 0);
        self.indices.retain(|_, data| data.strong_count() > 0);
    }
}

#[cfg(test)]
mod geometry_cache_tests {
    use super::GeometryCache;
    use std::sync::Arc;

    #[test]
    fn geometry_index_does_not_own_retired_arrays() {
        let mut cache = GeometryCache::default();
        let source = Arc::new(vec![0.0_f32, 1.0, 2.0]);
        let active = cache.vertex("scene/model/mesh", &source);
        assert!(Arc::ptr_eq(&source, &active));
        drop(source);
        drop(active);

        cache.prune();
        assert!(cache.vertices.is_empty());
        assert!(cache.indices.is_empty());
    }
}

pub(super) fn build_static_collision(
    triangles: Vec<Triangle>,
    radii: Vec<f32>,
    owners: Vec<usize>,
    convex: Vec<(Vec<V>, f32, usize)>,
    report: &Report,
) -> Result<Collision> {
    let sources = report
        .collision_parts
        .iter()
        .map(|part| (part.owner, part))
        .collect::<HashMap<_, _>>();
    ensure!(
        sources.len() == report.collision_parts.len(),
        "Duplicate static collider owner identity"
    );
    ensure!(
        owners.iter().all(|owner| sources.contains_key(owner)),
        "Triangle collider owner has no source record"
    );
    let mut collision = Collision::with_triangle_owners(triangles, radii, owners)?;
    for (vertices, radius, owner) in convex {
        let source = sources
            .get(&owner)
            .with_context(|| format!("Static convex owner {owner} has no source record"))?;
        collision
            .add_static_convex(&vertices, radius, owner)
            .with_context(|| {
                format!(
                    "Static convex collision {} entity {} body {:#x}, {} authored vertices",
                    source.asset,
                    source.entity_id,
                    source.body_offset,
                    vertices.len()
                )
            })?;
    }
    retain_fixed_ground_velocities(&mut collision, report.collision_parts.iter())?;
    Ok(collision)
}

/// Share source records between initial loading and incoming scene publication.
/// Nonfixed static-component overrides remain unresolved: don't fabricate their
/// runtime velocities from a serialized moving body's startup values.
pub(super) fn retain_fixed_ground_velocities<'a>(
    collision: &mut Collision,
    parts: impl IntoIterator<Item = &'a CollisionPart>,
) -> Result<()> {
    for part in parts {
        let source = part
            .native_resource
            .get(part.native_part)
            .context("Missing static body source")?;
        ensure!(
            source.body_offset == part.body_offset,
            "Mismatched static body source"
        );
        if source.motion_type == 5 {
            collision.retain_fixed_source_velocity(part.owner, source.linear_velocity)?;
        }
    }
    Ok(())
}

#[derive(Default)]
pub(super) struct Batch {
    pub(super) diagnostic_draws: usize,
    pub(super) external_player_name: Option<String>,
    pub(super) skins: HashMap<String, Vec<super::super::skin::Skin>>,
    pub(super) jigsaws: HashMap<String, Vec<super::super::jigsaw::Asset>>,
    pub(super) particle_meshes: HashMap<String, HashSet<usize>>,
    pub(super) model_registers: HashMap<String, BTreeMap<u32, [Option<[f32; 4]>; 7]>>,
    pub(super) textures: HashMap<String, Option<DecodedImage>>,
    pub(super) models: HashMap<String, Vec<scene::CachedNifMesh>>,
    pub(super) draws: Vec<GpuDrawCommand>,
    pub(super) model_mesh_inverse: BTreeMap<usize, [f32; 16]>,
    pub(super) model_local_draws: HashSet<usize>,
    pub(super) actor_draw_sources: BTreeMap<usize, (Vec<u32>, [[f32; 4]; 4])>,
    pub(super) actor_sequences: BTreeMap<usize, Arc<super::super::nif_sequence::SequenceResources>>,
    pub(super) sequence_resources: HashMap<
        String,
        std::result::Result<Arc<super::super::nif_sequence::SequenceResources>, String>,
    >,
    pub(super) geometry_cache: GeometryCache,
    pub(super) entities: Vec<RuntimeEntity>,
    pub(super) player_draws: Vec<usize>,
    pub(super) player_meshes: Vec<PlayerMesh>,
    pub(super) triangles: Vec<Triangle>,
    pub(super) triangle_radii: Vec<f32>,
    pub(super) convex_parts: Vec<(Vec<V>, f32, usize)>,
    pub(super) triangle_owners: Vec<usize>,
    pub(super) static_collision_resources:
        HashMap<String, Arc<Vec<super::super::collision::StaticMesh>>>,
    pub(super) static_welding_resources:
        HashMap<String, Arc<BTreeMap<usize, super::super::shape_welding::Prepared>>>,
    pub(super) lights: Vec<GpuLight>,
    pub(super) model_bounds: HashMap<String, super::super::scene_bounds::Hierarchy>,
    pub(super) found_player: bool,
    pub(super) player_entity: Option<usize>,
    pub(super) player_behavior: Option<super::super::behavior::Document>,
}
impl Batch {
    pub(super) fn append_source(
        &mut self,
        path: String,
        mut scene: SceneFile,
        files: &[VirtualFile],
        globals: Option<&SceneFile>,
        sources: &material_preview::Sources,
        report: &mut Report,
        diagnostic: Option<&MorphPreview>,
    ) -> Result<()> {
        let Self {
            external_player_name,
            diagnostic_draws,
            skins,
            jigsaws,
            particle_meshes,
            model_registers,
            textures,
            models,
            draws,
            model_mesh_inverse,
            model_local_draws,
            actor_draw_sources,
            actor_sequences,
            sequence_resources,
            geometry_cache,
            entities,
            player_draws,
            player_meshes,
            triangles,
            triangle_radii,
            convex_parts,
            triangle_owners,
            static_collision_resources,
            static_welding_resources,
            lights,
            model_bounds,
            found_player,
            player_entity,
            player_behavior,
        } = self;
        geometry_cache.prune();
        report.scenes.push(path.clone());
        let mut placed: HashSet<_> = scene.scene.iter().map(|id| id.to_u32()).collect();
        if let Some(global) = &globals {
            for object in &mut scene.objects {
                if !placed.contains(&object.link_id.to_u32()) {
                    if let Some(g) = global.objects.iter().find(|g| {
                        g.link_id == object.link_id
                            && g.name.eq_ignore_ascii_case(&object.name)
                            && g.class_name == object.class_name
                    }) {
                        *object = g.clone();
                        report.global_overrides += 1;
                    }
                }
            }
        }
        super::super::child_templates::expand(&path, &mut scene, files, globals, report)?;
        placed.extend(scene.scene.iter().map(|id| id.to_u32()));
        // A loaded scene can retain its editor player even when the shared
        // player-start scene already supplies Mickey. It is a reference to
        // the single gameplay player, not a second NPC/body/animation owner.
        let mut canonical = player_entity.map(|index| entities[index].name.clone()).or_else(||external_player_name.clone());
        let mut player_replicas = HashSet::new();
        for entity in scene
            .objects
            .iter()
            .filter(|entity| placed.contains(&entity.link_id.to_u32()))
        {
            let components = scene::get_effective_components(entity, &scene.objects);
            if !components
                .iter()
                .any(|component| component.class_name == "MickeyPlayerControllerComponent")
            {
                continue;
            }
            if let Some(canonical) = &canonical {
                player_replicas.insert(entity.link_id.to_u32());
                report.geometry.push(serde_json::json!({"kind":"canonical_player_alias","scene":path,"alias":entity.name,"canonical":canonical}));
            } else {
                canonical = Some(entity.name.clone());
            }
        }
        // The duplicate's brush/FX children share its ownership. Do not leave
        // detached copies behind after resolving the player to its live owner.
        loop {
            let before = player_replicas.len();
            for entity in scene
                .objects
                .iter()
                .filter(|entity| placed.contains(&entity.link_id.to_u32()))
            {
                let components = scene::get_effective_components(entity, &scene.objects);
                let parent = components
                    .iter()
                    .find(|component| component.class_name == "JPSTransformationComponent")
                    .and_then(|component| {
                        component
                            .properties
                            .iter()
                            .find(|field| field.name == "Parent Entity")
                    })
                    .and_then(|field| field.value.as_u64())
                    .and_then(|id| u32::try_from(id).ok());
                if parent.is_some_and(|id| id != 0 && player_replicas.contains(&id)) {
                    player_replicas.insert(entity.link_id.to_u32());
                }
            }
            if before == player_replicas.len() {
                break;
            }
        }
        let mut baked = BakedPreview::default();
        let lighting_path = path.trim_end_matches(".bin");
        if let Some(f) = files
            .iter()
            .find(|f| key(&f.path) == format!("{}.lit_cooked", key(lighting_path)))
        {
            baked.load(&f.data);
            if let Some(e) = &baked.error {
                report.warnings.push(e.clone());
            }
        }
        lights.extend(scene::runtime_lights(&scene));
        let io_layers = super::super::actor_io::component_index(&scene.objects);
        for entity in &scene.objects {
            if !placed.contains(&entity.link_id.to_u32())
                || player_replicas.contains(&entity.link_id.to_u32())
            {
                continue;
            }
            report.placed_entities += 1;
            let draw_start = draws.len();
            let mut mesh_draws = HashMap::<u32, Vec<usize>>::new();
            let mut model_asset = None;
            let mut jigsaw_asset = None;
            let mut jigsaw_states = Vec::new();
            let collision_start = report.collision_parts.len();
            let mut components = scene::get_effective_components(entity, &scene.objects);
            super::super::actor_io::inherit_connections(&mut components, &io_layers)
                .with_context(|| format!("Scene IO inheritance {}:{}", path, entity.name))?;
            for c in &components {
                *report.components.entry(c.class_name.clone()).or_default() += 1;
            }
            for component in components
                .iter()
                .filter(|c| c.class_name == "AnimationComponentGB")
            {
                let manager = component
                    .properties
                    .iter()
                    .find(|p| p.name.replace(' ', "") == "KFMFilePath")
                    .and_then(|p| p.value.as_str())
                    .filter(|p| !p.is_empty());
                if let Some(manager) = manager {
                    let identity = lookup(&files, manager)
                        .map(|file| key(&file.path))
                        .unwrap_or_else(|| key(manager));
                    let prepared = sequence_resources.entry(identity).or_insert_with(|| {
                        super::super::nif_sequence::SequenceResources::load_from_assets(
                            manager, &files,
                        )
                        .map(Arc::new)
                        .map_err(|error| format!("{error:#}"))
                    });
                    match prepared {
                        Ok(resources) => {
                            ensure!(
                                !actor_sequences.contains_key(&entities.len()),
                                "Multiple Gamebryo animation managers on {}",
                                entity.name
                            );
                            actor_sequences.insert(entities.len(), resources.clone());
                            report.geometry.push(serde_json::json!({"kind":"actor_sequence_resources","entity":entity.name,
                                "scene":path,"manager":manager,"playback_active":false,
                                "sequences":resources.sequences().map(|(id,sequence)|serde_json::json!({"id":id,"name":sequence.name,"duration":sequence.duration})).collect::<Vec<_>>()}));
                        }
                        Err(error) => report.unsupported.push(format!(
                            "Animation resources {}:{} ({manager}): {error}",
                            path, entity.name
                        )),
                    }
                } else {
                    report.unsupported.push(format!(
                        "Animation resources {}:{}: AnimationComponentGB has no KFM File Path",
                        path, entity.name
                    ));
                }
            }
            let property = |name: &str| {
                components
                    .iter()
                    .flat_map(|c| &c.properties)
                    .find(|p| p.name == name)
                    .map(|p| &p.value)
            };
            let draw_light_group = property("Light Group")
                .and_then(|value| value.as_str())
                .map(Arc::<str>::from)
                .unwrap_or_else(|| Arc::from(""));
            let is_player = components
                .iter()
                .any(|c| c.class_name == "MickeyPlayerControllerComponent");
            let matrix = scene::runtime_matrix(entity, &scene.objects);
            if let Some(light) = components
                .iter()
                .find(|c| c.class_name == "JPSLightComponent")
            {
                if let Some(baking) = components
                    .iter()
                    .find(|c| c.class_name == "JPSLightBakingComponent")
                {
                    let transform = scene::get_entity_transform(entity, &scene.objects);
                    match super::super::spherical_lighting::authored(
                        |name| {
                            light
                                .properties
                                .iter()
                                .find(|p| p.name == name)
                                .map(|p| &p.value)
                        },
                        |name| {
                            baking
                                .properties
                                .iter()
                                .find(|p| p.name == name)
                                .map(|p| &p.value)
                        },
                        transform.translation,
                        transform.rotation,
                    ) {
                        Ok(Some((light, raycast))) => report.spherical_lights.push(
                            super::super::spherical_lighting::RegisteredLight {
                                scene: path.clone(),
                                entity_id: entity.link_id.to_u32(),
                                name: entity.name.clone(),
                                raycast,
                                light,
                            },
                        ),
                        Ok(None) => {}
                        Err(error) => report
                            .unsupported
                            .push(format!("SH light {path}/{}: {error:#}", entity.name)),
                    }
                }
            }
            if is_player && !*found_player {
                *found_player = true;
                *player_entity = Some(entities.len());
                report.authored_ammo = Some(super::super::ammo::Inventory::load(
                    components
                        .iter()
                        .find(|c| c.class_name == "InventoryComponent")
                        .context("Mickey has no inventory component")?,
                )?);
                report.ammo_settings = Some(super::super::ammo::Settings::load(
                    components
                        .iter()
                        .find(|c| c.class_name == "AmmoRechargeComponent")
                        .context("Mickey has no ammo recharge component")?,
                )?);
                let motion_component = components
                    .iter()
                    .find(|c| c.class_name == "MickeyPlayerMotionControllerComponent")
                    .context("Mickey has no resolved motion controller component")?;
                let motion_value = |name: &str| {
                    motion_component.properties.iter().find(|p| p.name == name).map(|p| &p.value)
                };
                report.motion.damage_leap_enabled = motion_value("DoesDamagedLeap")
                    .and_then(|v| v.as_bool()).unwrap_or(false);
                for (name, target) in [
                    ("DamagedLeapLaunchSpeed", &mut report.motion.damage_leap_launch_speed),
                    ("DamagedLeapGravity", &mut report.motion.damage_leap_gravity),
                    ("DamagedLeapMaxAirSpeed", &mut report.motion.damage_leap_air_speed),
                    ("DamagedLeapAirAccel", &mut report.motion.damage_leap_acceleration),
                ] {
                    if let Some(value) = motion_value(name) {
                        *target = value.as_f64().with_context(|| format!("Invalid {name}"))? as f32;
                        ensure!(target.is_finite(), "Nonfinite {name}");
                    }
                }
                report.locomotion_action_settings =
                    Some(super::super::locomotion_actions::Settings::load(|name| {
                        motion_component
                            .properties
                            .iter()
                            .find(|p| p.name == name)
                            .map(|p| &p.value)
                    })?);
                let project = property("Behavior File Path")
                    .and_then(|v| v.as_str())
                    .context("Mickey has no behavior project")?;
                let character = property("CharacterName")
                    .and_then(|v| v.as_str())
                    .context("Mickey has no behavior character name")?;
                let (contract, graph) =
                    super::super::behavior::character_assets(project, character, |path| {
                        lookup(&files, path).map(|file| file.data.as_slice())
                    })
                    .context("Load Mickey's authored behavior dependencies")?;
                for path in &contract.unavailable_clips {
                    report.warnings.push(format!("Behavior clip not present in loaded packs: {path}; activating it requires loading its animation list."));
                }
                report.player_behavior = Some(contract);
                *player_behavior = Some(graph);
                let collision_asset = property("RB Hull File Path")
                    .and_then(|v| v.as_str())
                    .context("Mickey has no authored collision hull")?;
                let hull = match lookup(&files, collision_asset) {
                    Some(hull) => hull,
                    None => {
                        // Some retail placements retain an unavailable test
                        // hull reference. Keep the serialized placement intact;
                        // explicitly report use of the global player body.
                        let master = scene::resolve_master_chain(entity, &scene.objects)
                            .last()
                            .copied()
                            .context("Missing player master")?;
                        let global = globals
                            .as_ref()
                            .unwrap()
                            .objects
                            .iter()
                            .find(|object| {
                                object.name == master.name && object.class_name == master.class_name
                            })
                            .context("Missing global player body definition")?;
                        let global_components = scene::get_effective_components(
                            global,
                            &globals.as_ref().unwrap().objects,
                        );
                        let path = global_components
                            .iter()
                            .find(|c| c.class_name == "PhysicsRigidBodyCharacterComponent")
                            .and_then(|c| {
                                c.properties.iter().find(|p| p.name == "RB Hull File Path")
                            })
                            .and_then(|p| p.value.as_str())
                            .context("Missing global player hull")?;
                        let hull=lookup(&files,path).with_context(||format!("Missing Mickey collision hull {collision_asset} and global hull {path}"))?;
                        report.unsupported.push(format!("Player placement references unavailable hull {collision_asset}; using authored global hull {path}. Retail missing-resource resolution remains unvalidated."));
                        hull
                    }
                };
                let hull = HkxFile::load_bytes(&hull.data).map_err(anyhow::Error::msg)?;
                let (collision_hull, native_support) =
                    super::super::character_hull::load_with_support(&hull)
                        .with_context(|| format!("Read Mickey collision hull {collision_asset}"))?;
                report.player_collision = Some(collision_hull);
                report.player_native_support = Some(native_support);
                report.player_components = components
                    .iter()
                    .map(|component| {
                        serde_json::json!({
                            "class": component.class_name,
                            "properties": component.properties.iter().map(|p| serde_json::json!({
                                "name":p.name,"value":p.value
                            })).collect::<Vec<_>>()
                        })
                    })
                    .collect();
                report.spawn = [matrix[12], matrix[13], matrix[14]];
                super::super::authored_motion::apply(&property, &mut report.motion)?;
            }
            if let Some(component) = components
                .iter()
                .find(|c| c.class_name == "PhysicsRigidBodyStaticComponent")
            {
                let hull = component
                    .properties
                    .iter()
                    .find(|p| p.name == "RB Hull File Path")
                    .and_then(|p| p.value.as_str())
                    .context("Static rigid body has no collision asset property")?;
                // An authored empty resource is a body with no imported shape;
                // it must not prevent its model and other components loading.
                // Some cooked optional paths contain only the source folder,
                // with a trailing separator and no filename (e.g. the film
                // animation in HauntedHouse01). They name no shape resource.
                if !hull.is_empty() && !hull.ends_with(['/', '\\']) {
                    let collision_scene_path = path.clone();
                    let path = hull;
                    if let Some(collision) = lookup(&files, path) {
                        if collision.data.starts_with(b"Gamebryo") || collision.data.starts_with(b"NetImmerse") {
                            let nif=epic_mickey_lib_rs::nif_file::NifFile::load_bytes(&collision.data).map_err(anyhow::Error::msg)?;
                            ensure!(!nif.block_types.iter().any(|kind|kind.starts_with("bhk") || kind.contains("Collision")),
                                "NIF collision resource requires embedded physics decoding: {path}");
                            // Retail scenery shams retain visual NIF paths in their
                            // hull fields. They contain no physics representation.
                            // Keep the visual placement; do not invent collision or
                            // feed NIF bytes to the Havok decoder.
                            report.warnings.push(format!("{}: static hull {path} is a visual NIF without collision blocks; no shape admitted",entity.name));
                            report.geometry.push(serde_json::json!({"kind":"nonphysics_hull_resource","entity":entity.name,"asset":path}));
                        } else {
                        let resource_key = key(&collision.path);
                        let meshes = if let Some(cached) =
                            static_collision_resources.get(&resource_key)
                        {
                            cached.clone()
                        } else {
                            let hkx =
                                HkxFile::load_bytes(&collision.data).map_err(anyhow::Error::msg)?;
                            let resource = Arc::new(
                                super::super::collision::body_local_meshes(&hkx)
                                    .with_context(|| format!("Decode static collision {path}"))?,
                            );
                            static_collision_resources
                                .insert(resource_key.clone(), resource.clone());
                            resource
                        };
                        let native_welding = if let Some(cached) =
                            static_welding_resources.get(&resource_key)
                        {
                            cached.clone()
                        } else {
                            let identity = super::super::native_hk_transform::from_entity(
                                [1., 0., 0., 0., 1., 0., 0., 0., 1.],
                                [0.; 3],
                            );
                            let mut prepared = BTreeMap::new();
                            for body in super::super::dynamic_body::layout(&meshes, &identity)? {
                                if let Some(welding) =
                                    body.prepare_welding(&meshes).with_context(|| {
                                        format!(
                                            "Prepare static collision welding {path} body {:#x}",
                                            body.body_offset
                                        )
                                    })?
                                {
                                    prepared.insert(body.body_offset, welding);
                                }
                            }
                            let prepared = Arc::new(prepared);
                            static_welding_resources.insert(resource_key, prepared.clone());
                            prepared
                        };
                        ensure!(
                            !meshes.is_empty(),
                            "No native collision geometry decoded from {path}"
                        );
                        for (native_part, mesh) in meshes.iter().enumerate() {
                            // Preserve the previous body-then-entity arithmetic for
                            // Box3D while retaining untouched native source vertices.
                            let body_vertices = mesh
                                .vertices
                                .iter()
                                .map(|&p| physics::point(&mesh.body_transform, p))
                                .collect::<Vec<_>>();
                            if mesh.motion_type != 5 {
                                report.warnings.push(format!("Static component {} uses {path} body {:#x} with serialized motion type {}; runtime motion override is unverified",entity.name,mesh.body_offset,mesh.motion_type));
                            }
                            let owner = report.collision_parts.len();
                            report.collision_parts.push(CollisionPart {
                                owner,
                                native_resource: meshes.clone(),
                                native_part,
                                native_welding: native_welding.clone(),
                                scene: collision_scene_path.clone(),
                                entity_id: entity.link_id.to_u32(),
                                asset: path.to_owned(),
                                motion_type: mesh.motion_type,
                                linear_velocity: mesh.linear_velocity[..3].try_into().unwrap(),
                                angular_velocity: mesh.angular_velocity[..3].try_into().unwrap(),
                                body_offset: mesh.body_offset,
                                body_filter_info: mesh.body_filter_info,
                                subpart_index: mesh.subpart_index,
                                body_transform: mesh.body_transform,
                                shape_keys: mesh.shape_keys.clone(),
                                child_filters: mesh.child_filters.clone(),
                                entity_transform: matrix,
                                triangle_start: triangles.len(),
                                triangle_count: mesh.indices.len() / 3,
                            });
                            let lo: V = std::array::from_fn(|a| {
                                body_vertices
                                    .iter()
                                    .map(|p| physics::point(&matrix, *p)[a])
                                    .fold(f32::INFINITY, f32::min)
                            });
                            let hi: V = std::array::from_fn(|a| {
                                body_vertices
                                    .iter()
                                    .map(|p| physics::point(&matrix, *p)[a])
                                    .fold(f32::NEG_INFINITY, f32::max)
                            });
                            // Runtime entity-scale handling of Havok's margin needs verification.
                            let scales: [f32; 3] = std::array::from_fn(|a| {
                                physics::length([
                                    matrix[a * 4],
                                    matrix[a * 4 + 1],
                                    matrix[a * 4 + 2],
                                ])
                            });
                            if mesh.triangle_radius != 0.
                                && scales.iter().any(|s| (*s - 1.).abs() >= 1e-5)
                            {
                                report.warnings.push(format!("Static collision {path} has entity scale {scales:?}; its serialized triangle radius is preserved, but retail runtime scaling of that radius remains unverified."));
                            }
                            let triangle_radius = mesh.triangle_radius;
                            report.geometry.push(serde_json::json!({"entity":entity.name,"kind":"collision","min":lo,"max":hi,"matrix":matrix,"vertices":mesh.vertices.len(),"indices":mesh.indices.len(),"triangle_radius":triangle_radius}));
                            if mesh.convex {
                                convex_parts.push((
                                    body_vertices
                                        .iter()
                                        .map(|p| physics::point(&matrix, *p))
                                        .collect::<Vec<_>>(),
                                    triangle_radius,
                                    owner,
                                ));
                            }
                            for face in mesh.indices.chunks_exact(3) {
                                ensure!(
                                    face.iter().all(|&i| (i as usize) < mesh.vertices.len()),
                                    "Invalid collision index in {path}"
                                );
                                triangles.push(Triangle {
                                    p: std::array::from_fn(|i| {
                                        physics::point(&matrix, body_vertices[face[i] as usize])
                                    }),
                                });
                                triangle_radii.push(triangle_radius);
                                triangle_owners.push(owner);
                            }
                        }
                        }
                    } else {
                        // Some retail scene placements point at collision resources
                        // absent from a partial extraction. Keep the visual entity
                        // and the rest of the level available; omit only this collider.
                        report.warnings.push(format!("Missing static collision {path} on {collision_scene_path}:{}; loaded the entity without this collider.",entity.name));
                        report.geometry.push(serde_json::json!({"kind":"missing_collision","scene":collision_scene_path,
                        "entity":entity.name,"asset":path,"fallback":"visual-only"}));
                    }
                }
            }
            if let Some(path) = render_model_path(&components)
                .with_context(|| format!("Model for {}", entity.name))?
            {
                if let Some(file) = crate::wii_mesh::resolve_model(&files, &path) {
                    let identity = key(&file.path);
                    model_asset = Some(identity.clone());
                    if let Some(component) = components
                        .iter()
                        .find(|c| c.class_name == "PhysicsJigsawComponent")
                    {
                        if !jigsaws.contains_key(&identity) {
                            let decoded = super::super::jigsaw::load(&file.data)
                                .with_context(|| format!("Jigsaw asset {}", file.path))?;
                            for asset in &decoded {
                                for (subpart, part) in asset.collision.parts.iter().enumerate() {
                                    if !part.unreachable_triangles.is_empty() {
                                        report.warnings.push(format!("{} node {} subpart {subpart}: retained {} invalid triangle records absent from the MOPP collision tree",file.path,asset.node,part.unreachable_triangles.len()));
                                    }
                                }
                            }
                            report.geometry.push(serde_json::json!({"asset":file.path,"kind":"jigsaw_source",
                                "nodes":decoded.iter().map(|a|serde_json::json!({"node":a.node,"pieces":a.pieces.len(),
                                    "adjacency_entries":a.adjacency.len(),"triangle_parts":a.collision.parts.len(),
                                    "convex_parts":a.collision.convex.len(),"vertex_streams":a.bindings.vertex_streams.len()})).collect::<Vec<_>>(),
                                "runtime_collision_active":false}));
                            jigsaws.insert(identity.clone(), decoded);
                        }
                        for asset in &jigsaws[&identity] {
                            let state = super::super::jigsaw_state::PaintState::load(
                                component,
                                asset.pieces.len(),
                            )
                            .with_context(|| {
                                format!("Initial jigsaw paint state for {}", entity.name)
                            })?;
                            report.geometry.push(serde_json::json!({"entity":entity.name,"kind":"jigsaw_initial_state",
                                "asset":file.path,"node":asset.node,"pieces":state.painted.len(),
                                "painted":state.painted.iter().filter(|v|**v).count(),"runtime_collision_active":false}));
                            jigsaw_states.push(state);
                        }
                        jigsaw_asset = Some(identity.clone());
                        report.warnings.push(format!("{}: jigsaw uses initial paint state and sketch materials; native filter policy and brush dispatch remain unimplemented",entity.name));
                    }
                    if !skins.contains_key(&identity) {
                        let nif = epic_mickey_lib_rs::nif_file::NifFile::load_bytes(&file.data)
                            .map_err(anyhow::Error::msg)?;
                        model_registers.insert(
                            identity.clone(),
                            super::super::material_animation::model_registers(&nif)?,
                        );
                        match super::super::scene_bounds::Hierarchy::load_with_source(&nif,&file.data) {
                            Ok(bounds) => {
                                model_bounds.insert(identity.clone(), bounds);
                            }
                            Err(error) => report
                                .unsupported
                                .push(format!("Model bound hierarchy {}: {error:#}", file.path)),
                        }
                        particle_meshes.insert(
                            identity.clone(),
                            super::super::scene_bounds::particle_geometry_blocks(&nif),
                        );
                        let effects: Vec<_> = nif
                            .block_types
                            .iter()
                            .filter(|kind| {
                                kind.contains("Particle")
                                    || kind.contains("Billboard")
                                    || kind.contains("PSys")
                            })
                            .cloned()
                            .collect();
                        if !effects.is_empty() {
                            report.geometry.push(serde_json::json!({"kind":"unsupported_effect_behavior","asset":file.path,"blocks":effects}));
                            report.warnings.push(format!(
                                "{}: scene effect behavior is not evaluated ({})",
                                file.path,
                                effects.join(", ")
                            ));
                        }
                        // This decoder reads EM1's custom NiMesh tail only.
                        // Legacy static NiTriShape/occluder files have no such
                        // payload and must continue through their existing renderer.
                        let decoded = if nif.block_types.iter().any(|kind|kind=="NiMesh") {
                            super::super::skin::read_nif(&file.data)
                                .with_context(||format!("Cooked skin data in {}",file.path))?
                        } else {Vec::new()};
                        for skin in &decoded {
                            report.cooked_skins.push(serde_json::json!({
                                "asset": file.path, "block": skin.block,
                                "transform_floats": skin.transform_floats.len(),
                                "weight_records": skin.weights.len(),
                                "vertex_mappings": skin.vertex_weights.len(),
                                "morph_indices": skin.morph_indices.len(),
                                "morph_width": skin.morph_width,
                                "partitions": skin.partitions.len(),
                                "playback_verified": false
                            }));
                        }
                        skins.insert(identity, decoded);
                    }
                }
                let cache = scene::ensure_nif_cached(&path, &files, models);
                let meshes = &models[&cache];
                let morph_preview = diagnostic.filter(|request| {
                    model_asset.as_ref().is_some_and(|asset| {
                        asset == &key(&request.model)
                            || asset == &format!("{}_wii", key(&request.model))
                    })
                });
                let morph_samples = if let Some(request) = morph_preview {
                    let model = crate::wii_mesh::resolve_model(&files, &path)
                        .context("Missing diagnostic model")?;
                    let kf = lookup(&files, &request.keyframes)
                        .context("Missing diagnostic keyframes")?;
                    Some(super::super::nif_sequence::preview_positions(
                        &model.data,
                        &kf.data,
                        &request.sequence,
                        request.time,
                    )?)
                } else {
                    None
                };
                let particle_blocks = crate::wii_mesh::resolve_model(&files, &path)
                    .and_then(|file| particle_meshes.get(&key(&file.path)));
                let expects_meshes=model_asset.as_ref().and_then(|asset|model_bounds.get(asset)).is_none_or(|graph| {
                    graph.nodes.values().any(|node|!matches!(node.class_name.as_str(),"NiNode"|"NiBillboardNode"|"NiSortAdjustNode"|"NiCamera"|"NiPSParticleSystem"|"NonUniformParticleSystem"))
                });
                if meshes.is_empty() && expects_meshes {
                    report
                        .warnings
                        .push(format!("{}: model not decoded: {path}", entity.name));
                }
                baked.validate_entity(entity.link_id.to_u32(), meshes);
                for (submesh_index, mesh) in meshes.iter().enumerate() {
                    if particle_blocks.is_some_and(|blocks| blocks.contains(&mesh.mesh_index)) {
                        report
                            .geometry
                            .push(serde_json::json!({"entity":entity.name,"asset":path,
                            "kind":"unsupported_particle_draw","nif_mesh_block":mesh.mesh_index}));
                        continue;
                    }
                    let tex = mesh.texture_name.as_ref().and_then(|name| {
                        textures
                            .entry(name.clone())
                            .or_insert_with(|| {
                                crate::render_support::nif::find_pak_texture(&files, name)
                                    .and_then(|f| crate::render_support::nif::decode_image_bytes(&f.data))
                                    .map(|(rgba, width, height)| DecodedImage {
                                        rgba: Arc::new(rgba),
                                        width,
                                        height,
                                    })
                            })
                            .as_ref()
                            .map(|i| (i.rgba.clone(), i.width, i.height))
                    });
                    let applied = baked.apply(entity.link_id.to_u32(), mesh);
                    let has_baked = matches!(applied, Some(Ok(_)));
                    let baked_error = applied.as_ref().and_then(|r| r.as_ref().err()).cloned();
                    let mut vertices = applied
                        .and_then(Result::ok)
                        .unwrap_or_else(|| mesh.v_data.clone());
                    let mut draw_bounds = (mesh.aabb_min, mesh.aabb_max);
                    let morph_override = morph_samples
                        .as_ref()
                        .and_then(|samples| samples.get(&(mesh.mesh_index as u32)));
                    if let Some((first_source, positions)) = morph_override {
                        let sources = mesh
                            .source_indices
                            .as_ref()
                            .context("Morph draw lacks source vertex mapping")?;
                        let (deformed, lo, hi) = super::super::nif_sequence::render_positions(
                            &vertices,
                            sources,
                            *first_source,
                            positions,
                            mesh.mesh_to_model,
                        )?;
                        vertices = Arc::new(deformed);
                        draw_bounds = (lo, hi);
                        *diagnostic_draws += 1;
                    }
                    // A single NiMesh block may yield multiple region/material
                    // submeshes. Include the cached submesh ordinal to avoid
                    // aliasing their distinct vertex/index streams.
                    let geometry_key =
                        format!("{}#{}:{submesh_index}", key(&path), mesh.mesh_index);
                    if !has_baked && morph_override.is_none() {
                        vertices = geometry_cache.vertex(&geometry_key, &mesh.v_data);
                    }
                    let indices = geometry_cache.indices(&geometry_key, &mesh.indices);
                    let mut images = vec![];
                    let preview = material_preview::prepare(&mesh.material, &sources, has_baked)
                        .and_then(|program| {
                            images = material_preview::images(
                                &program,
                                &mesh.material,
                                &files,
                                textures,
                            )?;
                            Ok(program)
                        });
                    if let Err(e) = &preview {
                        report.warnings.push(e.clone())
                    }
                    if let Some(e) = &baked_error {
                        report.warnings.push(e.clone())
                    }
                    if mesh.texture_name.is_some() && tex.is_none() {
                        report
                            .warnings
                            .push(format!("Missing texture {:?}", mesh.texture_name))
                    }
                    if is_player {
                        player_draws.push(draws.len());
                        let file = crate::wii_mesh::resolve_model(&files, &path)
                            .context("Missing player model")?;
                        let nif = epic_mickey_lib_rs::nif_file::NifFile::load_bytes(&file.data)
                            .map_err(anyhow::Error::msg)?;
                        if let Some(epic_mickey_lib_rs::nif_file::NiObject::NiMesh(raw)) =
                            nif.blocks.get(mesh.mesh_index)
                        {
                            let decoded = crate::wii_mesh::extract(&nif, raw)?;
                            ensure!(
                                mesh.source_indices.as_ref() == Some(&decoded.source_indices),
                                "Player render/source vertex mapping differs"
                            );
                            player_meshes.push(PlayerMesh {
                                draw: draws.len(),
                                asset: key(&file.path),
                                block: mesh.mesh_index,
                                positions: decoded.mesh.positions,
                                normals: decoded
                                    .mesh
                                    .normals
                                    .context("Player mesh lacks authored normals")?,
                                sources: decoded.source_indices,
                                transform: mesh.mesh_to_model,
                            });
                        }
                    }
                    report.render_triangles += mesh.indices.len() / 3;
                    let (lo, hi) = transform_aabb(draw_bounds.0, draw_bounds.1, &matrix);
                    report.geometry.push(
                        serde_json::json!({"entity":entity.name,"asset":path,"kind":"render","min":lo,"max":hi,
                            "nif_mesh_block":mesh.mesh_index,
                            "source_vertex_count":mesh.source_indices.as_ref().map(Vec::len),
                            "shader":mesh.material.shader,
                            "shader_supported":preview.is_ok(),
                            "shader_error":preview.as_ref().err(),
                            "baked_lighting":has_baked,
                            "baked_error":baked_error,
                            "light_group":property("Light Group").and_then(|v|v.as_str()).unwrap_or(""),
                            "mesh_to_model":mesh.mesh_to_model}),
                    );
                    mesh_draws
                        .entry(mesh.mesh_index as u32)
                        .or_default()
                        .push(draws.len());
                    let mut draw_matrix = matrix;
                    let mut morph_bind = mesh.mesh_to_model;
                    if actor_sequences.contains_key(&entities.len())
                        || model_asset
                            .as_ref()
                            .and_then(|asset| model_bounds.get(asset))
                            .is_some_and(|h| {
                                h.nodes.values().any(|n| {
                                    n.controllers
                                        .iter()
                                        .any(|c| c.class_name == "NiTransformController")
                                })
                            })
                    {
                        match super::super::nif_runtime::inverse(mesh.mesh_to_model) {
                            Ok(inverse) => {
                                model_mesh_inverse.insert(draws.len(), inverse);
                            }
                            Err(_) => {
                                ensure!(
                                    mesh.mesh_to_model
                                        .iter()
                                        .flatten()
                                        .all(|value| value.is_finite())
                                        && mesh.mesh_to_model[3] == [0., 0., 0., 1.],
                                    "Invalid animated mesh bind transform"
                                );
                                let file = crate::wii_mesh::resolve_model(&files, &path)
                                    .context("Missing animated mesh source")?;
                                let nif =
                                    epic_mickey_lib_rs::nif_file::NifFile::load_bytes(&file.data)
                                        .map_err(anyhow::Error::msg)?;
                                let node = match nif.blocks.get(mesh.mesh_index) {
                                    Some(epic_mickey_lib_rs::nif_file::NiObject::NiMesh(node)) => {
                                        node
                                    }
                                    _ => {
                                        anyhow::bail!("Singular animated mesh source is not NiMesh")
                                    }
                                };
                                let local = crate::wii_mesh::extract(&nif, node)
                                    .with_context(|| {
                                        format!(
                                            "Original local geometry {path}:{}",
                                            mesh.mesh_index
                                        )
                                    })?
                                    .mesh;
                                ensure!(
                                    local.positions.len() * 12 == vertices.len(),
                                    "Local animated vertex mapping mismatch"
                                );
                                ensure!(
                                    local.normals.as_ref().is_none_or(
                                        |normals| normals.len() == local.positions.len()
                                    ),
                                    "Local animated normal mapping mismatch"
                                );
                                let output = Arc::make_mut(&mut vertices);
                                draw_bounds = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
                                for (vertex, row) in output.chunks_exact_mut(12).enumerate() {
                                    row[..3].copy_from_slice(&local.positions[vertex]);
                                    row[3..6].copy_from_slice(
                                        &local
                                            .normals
                                            .as_ref()
                                            .map(|n| n[vertex])
                                            .unwrap_or([0., 1., 0.]),
                                    );
                                    for axis in 0..3 {
                                        draw_bounds.0[axis] = draw_bounds.0[axis].min(row[axis]);
                                        draw_bounds.1[axis] = draw_bounds.1[axis].max(row[axis]);
                                    }
                                }
                                model_local_draws.insert(draws.len());
                                let bind = std::array::from_fn(|index| {
                                    mesh.mesh_to_model[index % 4][index / 4]
                                });
                                draw_matrix = super::super::skin::matrix_product(&matrix, &bind);
                                morph_bind = [
                                    [1., 0., 0., 0.],
                                    [0., 1., 0., 0.],
                                    [0., 0., 1., 0.],
                                    [0., 0., 0., 1.],
                                ];
                            }
                        }
                    }
                    if actor_sequences.contains_key(&entities.len()) {
                        if let Some(sources) = &mesh.source_indices {
                            actor_draw_sources.insert(draws.len(), (sources.clone(), morph_bind));
                        }
                    }
                    draws.push(GpuDrawCommand {
                        shader_time:None,source_attributes: mesh.source_attributes.clone(),
                        extra_uvs: mesh.extra_uvs.clone(),
                        projection_bound: None,
                        projected_shadow: false,
                        skybox: false,
                        material_indirect: None,
                        material_registers: preview
                            .as_ref()
                            .ok()
                            .and_then(|p| p.register_defaults)
                            .map(|mut registers| {
                                if let Some(colors) = model_asset
                                    .as_ref()
                                    .and_then(|a| model_registers.get(a))
                                    .and_then(|m| m.get(&(mesh.mesh_index as u32)))
                                {
                                    for (index, color) in colors.iter().enumerate() {
                                        if let Some(color) = color {
                                            registers[index] = *color;
                                        }
                                    }
                                }
                                registers
                            }),
                        material_images: Arc::new(images),
                        baked: has_baked,
                        baked_error,
                        preview: Some(preview),
                        uv_transform: mesh.uv_transform,
                        material: Some(mesh.material.clone()),
                        sampler: mesh.sampler,
                        v_data: vertices,
                        indices,
                        trans_mat: draw_matrix,
                        color: [1.; 4],
                        tex_bytes: tex,
                        aabb_min: draw_bounds.0,
                        aabb_max: draw_bounds.1,
                        light_group: draw_light_group.clone(),
                    });
                }
            }
            let authored_pose = scene::get_entity_transform(entity, &scene.objects);
            entities.push(RuntimeEntity {
                model_asset,
                mesh_draws,
                jigsaw_asset,
                jigsaw_states,
                scene: path.clone(),
                id: entity.link_id.to_u32(),
                name: entity.name.clone(),
                authored_scale: authored_pose.scale[0],
                authored_rotation: std::array::from_fn(|i| authored_pose.rotation[i / 3][i % 3]),
                runtime_rotation: std::array::from_fn(|i| authored_pose.rotation[i / 3][i % 3]),
                transform: matrix,
                authored_transform: matrix,
                components,
                draws: draw_start..draws.len(),
                collision_parts: collision_start..report.collision_parts.len(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streamed_imports_share_static_mesh_buffers_without_owning_them_forever() {
        let mut cache = GeometryCache::default();
        let first_vertices = Arc::new(vec![0., 1., 2.]);
        let first_indices = Arc::new(vec![0, 1, 2]);
        let a_vertices = cache.vertex("models/prop.nif#7", &first_vertices);
        let a_indices = cache.indices("models/prop.nif#7", &first_indices);
        let second_vertices = Arc::new(vec![9., 9., 9.]);
        let second_indices = Arc::new(vec![2, 1, 0]);
        let b_vertices = cache.vertex("models/prop.nif#7", &second_vertices);
        let b_indices = cache.indices("models/prop.nif#7", &second_indices);
        assert!(Arc::ptr_eq(&a_vertices, &b_vertices));
        assert!(Arc::ptr_eq(&a_indices, &b_indices));
        assert_eq!(b_vertices.as_slice(), &[0., 1., 2.]);
        let mut animated = b_vertices.clone();
        Arc::make_mut(&mut animated)[0] = 8.;
        assert_eq!(
            b_vertices[0], 0.,
            "copy-on-write changed shared bind geometry"
        );
        drop((
            a_vertices,
            a_indices,
            b_vertices,
            b_indices,
            animated,
            first_vertices,
            first_indices,
        ));
        cache.prune();
        assert!(cache.vertices.is_empty());
        assert!(cache.indices.is_empty());
    }

    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_dbc_rest_geometry_matches_local_meshes_before_playback() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let root = super::files_root(std::path::Path::new(&root));
        let world = super::load(&root, "Levels/DBC_Start_1st.level", &egui::Context::default())?;
        let pak = crate::pack_io::read(std::fs::read(root.join("packfiles/dbc_1st_zonea.pak"))?)?.pak;
        let mut posed = world.draws.clone();
        world.apply_model_poses(&mut posed)?;
        let point = |matrix: &[f32; 16], p: [f32; 3]| -> [f32; 3] {
            std::array::from_fn(|axis| matrix[12+axis] +
                (0..3).map(|i| matrix[i*4+axis]*p[i]).sum::<f32>())
        };
        let mut owners=0;
        let mut vertices=0;
        let mut maximum_error=0f32;
        for (entity, owner) in world.entities.iter().enumerate().filter(|(_,e)|e.name.starts_with("dbc_1st_zonea_dynamic.DBC_ZoneD_Break_")) {
            owners+=1;
            ensure!(world.entity_scene_active(entity), "Destruction floor starts in an inactive scene: {}", owner.name);
            ensure!(!world.hidden_entities.contains(&entity) && !world.removed_entities.contains(&entity),
                "Destruction floor starts hidden or retired: {}", owner.name);
            let visible_triangles: usize = owner.mesh_draws.values().flatten()
                .filter(|&&draw| !world.hidden_entity_draws.contains(&draw))
                .map(|&draw| posed[draw].indices.len()/3).sum();
            ensure!(visible_triangles > 0, "Destruction floor has no visible triangles before playback: {}", owner.name);
            let actor=world.actors.get(&entity).context("Destruction actor absent")?;
            ensure!(actor.active_id().is_none(),"Rest check must precede playback");
            let file=crate::wii_mesh::resolve_model(&pak.files,owner.model_asset.as_ref().context("Model absent")?).context("Original model absent")?;
            let nif=epic_mickey_lib_rs::nif_file::NifFile::load_bytes(&file.data).map_err(anyhow::Error::msg)?;
            for (&mesh, draws) in &owner.mesh_draws {
                let Some(epic_mickey_lib_rs::nif_file::NiObject::NiMesh(node))=nif.blocks.get(mesh as usize) else {continue;};
                let local=crate::wii_mesh::extract(&nif,node)?.mesh;
                let frame=crate::blotworx::nif_runtime::matrix(actor.world.get(&mesh).context("Rest mesh frame absent")?);
                let expected=crate::blotworx::skin::matrix_product(&owner.transform,&frame);
                for &draw in draws {
                    let draw=&posed[draw];
                    ensure!(local.positions.len()*12==draw.v_data.len(),"Original vertex mapping differs");
                    for (i,p) in local.positions.iter().enumerate() {
                        let reference=point(&expected,*p);
                        let actual=point(&draw.trans_mat,draw.v_data[i*12..i*12+3].try_into()?);
                        let error=(0..3).map(|a|(reference[a]-actual[a]).abs()).fold(0f32,f32::max);
                        maximum_error=maximum_error.max(error);
                        ensure!(error<0.001,"{} mesh {mesh} vertex {i}: initial rendering differs from local Rest geometry by {error}m: {actual:?} vs {reference:?}",owner.name);
                        vertices+=1;
                    }
                }
            }
        }
        ensure!(owners==3 && vertices>1000,"Original destruction fixtures missing");
        println!("DBC Rest: {owners} owners, {vertices} vertices, maximum transformed-coordinate error {maximum_error}m");
        Ok(())
    }

    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn incoming_airlock_prepares_rendering_and_physics_without_a_new_player() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let root = Path::new(&root);
        let incoming = super::super::super::scene_loading::Source::read(
            root,
            Arc::from("Environments/thepark/fantasyLand/SmallWorld/GSA/GV_ZoneA_airlock.gsa"),
        )?;
        let mut report = Report::default();
        let mut files = Vec::new();
        // Global assets retain authority over copies embedded in scene packs.
        pack(root, "_dynamic.pak", &mut files, &mut report)?;
        pack(root, "r3mt.pak", &mut files, &mut report)?;
        let mut existing = files
            .iter()
            .map(|file| key(&file.path))
            .collect::<HashSet<_>>();
        files.extend(
            incoming
                .files
                .into_iter()
                .filter(|file| existing.insert(key(&file.path))),
        );
        let global = lookup(&files, "Palettes/_Dynamic.bin").context("Global palette missing")?;
        let global = super::super::super::scene_loading::parse_scene(&global.path, &global.data)?;
        let ctx = egui::Context::default();
        let sources = material_preview::native_sources(&ctx, &files);
        let mut batch = Batch::default();
        batch.append_source(
            incoming.path,
            incoming.scene,
            &files,
            Some(&global),
            &sources,
            &mut report,
            None,
        )?;
        ensure!(
            !batch.found_player && batch.player_entity.is_none() && batch.player_draws.is_empty(),
            "Incoming scene incorrectly initializes another player"
        );
        ensure!(
            !batch.entities.is_empty() && !batch.draws.is_empty(),
            "Incoming airlock has no prepared entities/geometry"
        );
        ensure!(
            !report.collision_parts.is_empty(),
            "Incoming airlock has no prepared static bodies"
        );
        assert_eq!(batch.triangles.len(), batch.triangle_radii.len());
        assert_eq!(batch.triangles.len(), batch.triangle_owners.len());
        assert!(batch
            .entities
            .iter()
            .all(|entity| entity.draws.end <= batch.draws.len()
                && entity.collision_parts.end <= report.collision_parts.len()));
        assert!(batch
            .actor_sequences
            .keys()
            .all(|&entity| entity < batch.entities.len()));
        assert!(batch
            .triangle_owners
            .iter()
            .all(|&owner| owner < report.collision_parts.len()));
        assert!(!batch.lights.is_empty());
        println!("Prepared airlock: {} entities, {} meshes, {} render triangles, {} static bodies, {} triangle contacts, {} convex cores, {} actor resource sets; no player publication",
            batch.entities.len(),batch.draws.len(),report.render_triangles,report.collision_parts.len(),
            batch.triangles.len(),batch.convex_parts.len(),batch.actor_sequences.len());
        // Native backend queries must return collider identities, even when
        // those identities cannot be mistaken for indices into this report.
        let sparse_base = 1000;
        for part in &mut report.collision_parts {
            part.owner += sparse_base;
        }
        for owner in &mut batch.triangle_owners {
            *owner += sparse_base;
        }
        for (_, _, owner) in &mut batch.convex_parts {
            *owner += sparse_base;
        }
        let triangle = batch
            .triangles
            .iter()
            .find(|triangle| {
                physics::length(physics::cross(
                    physics::sub(triangle.p[1], triangle.p[0]),
                    physics::sub(triangle.p[2], triangle.p[0]),
                )) > 0.
            })
            .context("Expected nondegenerate airlock triangle")?
            .p;
        let normal = physics::cross(
            physics::sub(triangle[1], triangle[0]),
            physics::sub(triangle[2], triangle[0]),
        );
        let normal = physics::mul(normal, 1. / physics::length(normal));
        let center = physics::mul(
            physics::add(physics::add(triangle[0], triangle[1]), triangle[2]),
            1. / 3.,
        );
        let collision = build_static_collision(
            batch.triangles,
            batch.triangle_radii,
            batch.triangle_owners,
            batch.convex_parts,
            &report,
        )?;
        let hits = collision.ray_hits(
            physics::add(center, physics::mul(normal, 2.)),
            physics::mul(normal, -4.),
        )?;
        ensure!(
            !hits.is_empty(),
            "Prepared incoming collision is not queryable"
        );
        for hit in &hits {
            let part = report
                .collision_parts
                .iter()
                .find(|part| part.owner == hit.owner)
                .unwrap();
            let source = &part.native_resource[part.native_part];
            if source.motion_type == 5 {
                assert_eq!(
                    collision.ground_body_linear_velocity(hit.shape),
                    Some(source.linear_velocity)
                );
            }
        }
        assert!(hits.iter().all(|hit| hit.owner >= sparse_base
            && report
                .collision_parts
                .iter()
                .any(|part| part.owner == hit.owner)));
        Ok(())
    }
}

// Shared immutable pose bindings; actor activation remains instance state.
pub(super) fn prepare_actor_bindings(
    entities: &[RuntimeEntity],
    actor_sequences: &BTreeMap<usize, Arc<super::super::nif_sequence::SequenceResources>>,
    model_bounds: &HashMap<String, super::super::scene_bounds::Hierarchy>,
    files: &[VirtualFile],
    shared_cache: &mut ActorBindingCache,
    report: &mut Report,
) -> BTreeMap<usize, Arc<super::super::nif_sequence::ActorBindings>> {
    let mut actor_bindings = BTreeMap::new();
    let mut bindings_cache = HashMap::<
        (String, String),
        std::result::Result<Arc<super::super::nif_sequence::ActorBindings>, String>,
    >::new();
    for (&index, resources) in actor_sequences {
        let entity = &entities[index];
        let prepared = (|| -> Result<Arc<super::super::nif_sequence::ActorBindings>> {
            let asset = entity
                .model_asset
                .as_ref()
                .context("Animated entity has no model")?;
            let manager = entity
                .components
                .iter()
                .filter(|component| component.class_name == "AnimationComponentGB")
                .flat_map(|component| &component.properties)
                .find(|property| property.name.replace(' ', "") == "KFMFilePath")
                .and_then(|property| property.value.as_str())
                .filter(|manager| !manager.is_empty())
                .context("Animated entity has no KFM manager path")?;
            let manager = lookup(files, manager)
                .map(|file| key(&file.path))
                .unwrap_or_else(|| key(manager));
            let model_key = key(asset);
            let identity = (model_key.clone(), manager.clone());
            if let Some(prepared) = bindings_cache.get(&identity) {
                return prepared.clone().map_err(anyhow::Error::msg);
            }
            let result = if let Some(bindings) = shared_cache.get(&model_key, &manager) {
                Ok(bindings)
            } else {
                (|| -> Result<Arc<super::super::nif_sequence::ActorBindings>> {
                    let hierarchy = model_bounds
                        .get(asset)
                        .context("Animated model hierarchy unavailable")?;
                    let roots = hierarchy
                        .roots
                        .iter()
                        .flatten()
                        .copied()
                        .collect::<Vec<_>>();
                    ensure!(
                        roots.len() == 1,
                        "Animated model has multiple/no NIF roots; resource root selection requires support"
                    );
                    let file = lookup(files, asset).context("Animated model bytes unavailable")?;
                    let model = epic_mickey_lib_rs::nif_file::NifFile::load_bytes(&file.data)
                        .map_err(anyhow::Error::msg)?;
                    super::super::nif_sequence::ActorBindings::prepare(
                        resources, &model, hierarchy, roots[0],
                    )
                    .map(Arc::new)
                })()
            };
            if let Ok(bindings) = &result {
                shared_cache.insert(model_key, manager, bindings);
            }
            let local_result = match &result {
                Ok(bindings) => Ok(bindings.clone()),
                Err(error) => Err(format!("{error:#}")),
            };
            bindings_cache.insert(identity, local_result);
            result
        })();
        match prepared {
            Ok(bindings) => {
                report.geometry.push(
                    serde_json::json!({"kind":"actor_bindings","entity":entity.name,
                    "actor_root":bindings.actor_root,"sequence_count":bindings.sequences.len(),
                    "other_evaluators":bindings.other_evaluators,"playback_active":false}),
                );
                actor_bindings.insert(index, bindings);
            }
            Err(error) => report
                .unsupported
                .push(format!("Actor pose binding {}: {error:#}", entity.name)),
        }
    }
    actor_bindings
}
