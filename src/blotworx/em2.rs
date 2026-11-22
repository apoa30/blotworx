//! PC EM2 startup bootstrap. Does not substitute an EM1 world or scripts.
use anyhow::{Context, Result, ensure};
use std::{collections::{HashMap, HashSet}, path::Path, sync::{Arc, mpsc}};
use crate::render_support::{scene, viewport3d::{self, GpuDrawCommand, Viewport3DState}};
use epic_mickey_lib_rs::packfile::VirtualFile;

fn archive_path(root: &Path, stem: &str) -> Result<std::path::PathBuf> {
    let name = format!("{stem}.pak");
    std::fs::read_dir(root.join("packfiles"))?.filter_map(Result::ok)
        .find(|e|e.file_name().to_string_lossy().eq_ignore_ascii_case(&name))
        .map(|e|e.path()).with_context(||format!("Missing EM2 archive {name}"))
}
fn safe_stem(path: &str) -> Result<&str> {
    ensure!(!path.contains(':') && !path.split(['/', '\\']).any(|s|s==".."), "Unsafe EM2 resource path");
    Path::new(path).file_stem().and_then(|s|s.to_str()).context("Missing EM2 resource name")
}
/// The level's baked vertex lighting (its `.lit` meshes), supplied by EM2 Studio playtests through
/// BLOTWORX_EM2_LIT: "EM2LIT01", u32 count, then per mesh: u16 name count, names (u16 length +
/// UTF-8), u32 vertex count, RGBA8 colours. A mesh takes the next unused entry with its name and
/// original vertex count; its interleaved COLOR0 (floats 8..12 of each 12-float vertex) is replaced
/// through the vertex's original index.
#[derive(Default)]
struct BakedLighting { by_name: HashMap<String, Vec<(usize, Arc<Vec<u8>>)>>, used: HashSet<(String, usize)>, applied: usize, scale: f32, sat: f32,
    /// .lit meshes whose colour stream has an alpha channel. On Gen3 blend shaders the vertex alpha
    /// is the inert/toon blend, and the .lit stream replaces the mesh's own colours in the game.
    with_alpha: HashSet<usize> }
impl BakedLighting {
    fn load() -> Self {
        let base = Self { scale: std::env::var("BLOTWORX_EM2_LIT_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(2.0), sat: 1.0, ..Self::default() };
        let Some(path) = std::env::var_os("BLOTWORX_EM2_LIT") else { return base };
        let Ok(b) = std::fs::read(path) else { return base };
        let mut out = Self { scale: std::env::var("BLOTWORX_EM2_LIT_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(2.0), sat: std::env::var("BLOTWORX_EM2_LIT_SAT").ok().and_then(|s| s.parse().ok()).unwrap_or(1.0), ..Self::default() };
        if b.len() < 12 || &b[..8] != b"EM2LIT01" { return out; }
        let u16_at = |o: usize| b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as usize);
        let u32_at = |o: usize| b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()) as usize);
        let Some(count) = u32_at(8) else { return out };
        let mut o = 12;
        for index in 0..count {
            let Some(names) = u16_at(o) else { break }; o += 2;
            let mut list = Vec::new();
            for _ in 0..names { let Some(n) = u16_at(o) else { return out }; o += 2; let Some(s) = b.get(o..o + n) else { return out }; list.push(String::from_utf8_lossy(s).to_ascii_lowercase()); o += n; }
            let Some(nv) = u32_at(o) else { break }; o += 4;
            let Some(cols) = b.get(o..o + nv * 4) else { break }; o += nv * 4;
            let cols = Arc::new(cols.to_vec());
            // Each entry lists its .lit node name ("0", "1"...), the "SourceName" label and the source mesh
            // name (e.g. "roofShape:1"). Only the source name identifies a mesh: the generic ones collide
            // across models and handed meshes another model's (often dark) lighting.
            for n in list.into_iter().filter(|n| !(n.chars().all(|c| c.is_ascii_digit()) || n == "sourcename" || n == "mayatoenginescalenode")) {
                out.by_name.entry(n).or_default().push((index, cols.clone()));
            }
        }
        out
    }
    /// The level's own baked lighting: every .lit in the loaded archives (NIFs whose NiMeshes carry
    /// per-vertex light colours, named by their source mesh through NiStringExtraData). This replaces
    /// the editor-extracted file, which only ever held the level last launched from EM2 Studio.
    fn load_level(&mut self, files: &[VirtualFile]) -> (usize, usize) {
        use epic_mickey_lib_rs::nif_file::{NifFile, NiObject};
        let (mut lit_files, mut meshes) = (0, 0);
        let mut found: HashMap<String, Vec<(usize, Arc<Vec<u8>>)>> = HashMap::new();
        let mut index = 0;
        let mut alpha_stats = (0usize, 255u8, 0u8);
        self.with_alpha.clear();
        for file in files.iter().filter(|f| f.path.to_ascii_lowercase().ends_with(".lit")) {
            let Ok(nif) = NifFile::load_bytes(&file.data) else { continue };
            lit_files += 1;
            for block in &nif.blocks {
                let NiObject::NiMesh(m) = block else { continue };
                // COLOR,0 stream: component format = (count << 16) | (bytes per component << 8) | type
                let Some(stream) = m.stream_refs.iter().find(|r| r.semantics.iter().any(|s| s.name == "COLOR" && s.index == 0)) else { continue };
                let Some(NiObject::NiDataStream(ds)) = nif.blocks.get(stream.stream_link as usize) else { continue };
                let formats = &ds.component_formats;
                let bytes = |f: u32| (((f >> 16) & 0xff) * ((f >> 8) & 0xff)) as usize;
                let stride: usize = formats.iter().map(|&f| bytes(f)).sum();
                let Some(&format) = formats.first() else { continue };
                let (channels, size) = (((format >> 16) & 0xff) as usize, ((format >> 8) & 0xff) as usize);
                if stride == 0 || channels == 0 { continue; }
                let count = ds.buffer.len() / stride;
                let mut cols = vec![255u8; count * 4];
                for v in 0..count {
                    for c in 0..channels.min(4) {
                        let o = v * stride + c * size;
                        let x = match size {
                            1 => ds.buffer[o] as f32 / 255.0,
                            2 => {
                                let h = u16::from_le_bytes([ds.buffer[o], ds.buffer[o + 1]]) as u32;
                                let (sign, exp, man) = (h >> 15, (h >> 10) & 0x1f, h & 0x3ff);
                                let mag = if exp == 0 { man as f32 / 1024.0 * 2f32.powi(-14) } else { (1.0 + man as f32 / 1024.0) * 2f32.powi(exp as i32 - 15) };
                                if sign == 1 { -mag } else { mag }
                            },
                            4 => f32::from_le_bytes(ds.buffer[o..o + 4].try_into().unwrap()),
                            _ => 1.0,
                        };
                        cols[v * 4 + c] = (x * 255.0).round().clamp(0.0, 255.0) as u8;
                    }
                }
                if channels >= 4 { self.with_alpha.insert(index); alpha_stats.0 += 1; for v in 0..count { let a = cols[v * 4 + 3]; alpha_stats.1 = alpha_stats.1.min(a); alpha_stats.2 = alpha_stats.2.max(a); } }
                let cols = Arc::new(cols);
                let net = &m.base.base.base;
                let mut names = vec![net.name.to_ascii_lowercase()];
                for link in net.extra_data_links.iter().flatten() {
                    if let Some(NiObject::NiStringExtraData(s)) = nif.blocks.get(*link as usize) { names.push(s.string_data.to_ascii_lowercase()); }
                }
                for n in names.into_iter().filter(|n| !(n.is_empty() || n.chars().all(|c| c.is_ascii_digit()) || n == "sourcename" || n == "mayatoenginescalenode")) {
                    found.entry(n).or_default().push((index, cols.clone()));
                }
                index += 1; meshes += 1;
            }
        }
        // the level's own data wins; the extracted file is only a fallback for levels without .lit
        if meshes > 0 { self.by_name = found; }
        eprintln!("[lit] {} of {meshes} baked meshes carry vertex alpha (range {}..{})", alpha_stats.0, alpha_stats.1, alpha_stats.2);
        (lit_files, meshes)
    }
    fn apply(&mut self, mesh: &crate::render_support::scene::CachedNifMesh, name: Option<&String>) -> Option<Arc<Vec<f32>>> {
        if self.by_name.is_empty() { return None; }
        let source = mesh.source_indices.as_ref()?;
        let original = source.iter().copied().max()? as usize + 1;
        let name = name?.to_ascii_lowercase();
        let list = self.by_name.get(&name)?;
        let (index, cols) = list.iter().find(|(i, c)| c.len() / 4 == original && !self.used.contains(&(name.clone(), *i)))
            .or_else(|| list.iter().find(|(_, c)| c.len() / 4 == original))?.clone();
        self.used.insert((name, index));
        let mut v = (*mesh.v_data).clone();
        for (k, &si) in source.iter().enumerate() {
            let (Some(c), Some(dst)) = (cols.get(si as usize * 4..si as usize * 4 + 4), v.get_mut(k * 12 + 8..k * 12 + 11)) else { continue };
            // Baked light (RGB). The game's colour grading keeps it fairly neutral, so part of its tint
            // is taken out; tunable with BLOTWORX_EM2_LIT_SCALE / _SAT for comparisons.
            let (scale, sat) = (self.scale, self.sat);
            // x2.0 = the Gen3 shaders' own 2*vertexColour; EM2's post pass (render_support/em2_post.rs)
            // supplies the rest of the brightness, as in the game.
            let rgb = [c[0] as f32, c[1] as f32, c[2] as f32].map(|x| x / 255.0);
            let luma = rgb[0] * 0.3 + rgb[1] * 0.59 + rgb[2] * 0.11;
            for j in 0..3 { dst[j] = (luma + (rgb[j] - luma) * sat) * scale; }
            if self.with_alpha.contains(&index) && lit_alpha() { if let Some(a) = v.get_mut(k * 12 + 11) { *a = c[3] as f32 / 255.0; } }
        }
        self.applied += 1;
        Some(Arc::new(v))
    }
}

pub(super) fn is_install(root: &Path) -> bool { root.join("DEM2.exe").is_file() && root.join("packfiles").is_dir() }
pub(super) fn run(options: super::Options, root: &Path) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    if options.inspect.is_some() {
        build(options, root, tx)?;
        return Ok(());
    }
    let wants_capture = options.capture.is_some();
    let root = root.to_owned();
    eframe::run_native(
        "Blotworx Engine - EM2 (F6: shared-controller playtest)",
        eframe::NativeOptions {
            depth_buffer: 24,
            viewport: egui::ViewportBuilder::default().with_inner_size([1280., 800.]),
            // BLOTWORX_VSYNC=0 draws as fast as the GPU allows instead of at the monitor's refresh rate
            glow_options: egui_glow::GlowConfiguration {vsync: std::env::var("BLOTWORX_VSYNC").map_or(true,|v|v!="0"),..Default::default()},
            ..Default::default()
        },
        Box::new(move |cc| {
            let screen = super::loading_screen::Screen::em2(&cc.egui_ctx, &root);
            let (send, pending) = mpsc::channel();
            let ctx = cc.egui_ctx.clone();
            let startup_errors = tx.clone();
            std::thread::spawn(move || {
                let result = build(options, &root, tx).and_then(|scene| scene.context("EM2 startup produced no scene"));
                let _ = send.send(result);
                ctx.request_repaint();
            });
            Ok(Box::new(Startup { pending: Some((screen, pending)), scene: None, movie: None, error: None, wants_capture, startup_errors }))
        }),
    ).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    if wants_capture { rx.try_recv().context("EM2 capture interrupted")??; }
    Ok(())
}

struct Startup {
    pending: Option<(super::loading_screen::Screen, mpsc::Receiver<Result<Bootstrap>>)>,
    scene: Option<Bootstrap>,
    movie: Option<super::movie::Player>,
    error: Option<String>,
    wants_capture: bool,
    startup_errors: mpsc::Sender<Result<()>>,
}

impl eframe::App for Startup {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        if let Some((screen, receiver)) = &mut self.pending {
            if !screen.ready_to_dismiss() {
                screen.paint(ui);
                return;
            }
            let result = match receiver.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => {
                    screen.paint(ui);
                    if !self.wants_capture {
                        super::loading_screen::report_button(ui.ctx(), None, None, Some(screen.elapsed()));
                    }
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => Err(anyhow::anyhow!("EM2 startup worker stopped")),
            };
            self.pending = None;
            match result {
                Ok(mut scene) => {
                    scene.last_frame = std::time::Instant::now();
                    self.scene = Some(scene);
                }
                Err(error) if self.wants_capture => {
                    let _ = self.startup_errors.send(Err(error));
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Err(error) => self.error = Some(format!("{error:#}")),
            }
        }
        if let Some(scene) = &mut self.scene {
            scene.update(ui, &mut self.movie);
        } else if let Some(error) = &self.error {
            ui.heading("Couldn’t load Epic Mickey 2");
            ui.label(error);
            super::loading_screen::report_button(ui.ctx(), None, Some(error), None);
        }
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        if let Some(scene) = &mut self.scene { scene.on_exit(gl); }
    }
}

fn build(options: super::Options, root: &Path, tx: mpsc::Sender<Result<()>>) -> Result<Option<Bootstrap>> {
    let mut settings=std::collections::BTreeMap::new();let mut audio_language=None;
    for (name,command) in [("ConfigFiles.ini",false),("cmdline.txt",true)] {
        let path=root.join(name);if !path.is_file(){continue;}
        let text=std::fs::read_to_string(path)?;
        settings.extend(crate::render_support::game_config::runtime_settings(&text,command));
        if let Some(language)=crate::render_support::game_config::setting_value(&text,"DefaultAudioLanguage",command){audio_language=Some(language);}
    }
    let movie_language=audio_language.context("EM2 ConfigFiles.ini must supply DefaultAudioLanguage")?;
    let movie_skip_seconds=settings.get("UI_SkipMovie_SkipButtonHoldTime")
        .map(|value|super::config_value::decimal_float(value).map_err(anyhow::Error::msg)).transpose()?;
    let movie_root=root.to_owned();let cursor_settings=super::em2_cursor_camera::Settings::load(root);let capture_movie_frame=options.capture_movie_frame;
    let levels=archive_path(root,"levels")?;
    // `--level Level.state` (e.g. MeanStreet_South.visit2) picks the EM2 level and state to start in
    // (EM2 Studio playtests); otherwise the authored new-game start, Yen Sid's Lab HouseStart.
    let requested=options.level.trim();
    let (level_name,state_name)=match requested.split_once('.') {
        Some((l,s)) if !l.contains(['/','\\']) && !s.eq_ignore_ascii_case("level") => (l.to_string(),Some(s.to_string())),
        _ if !requested.is_empty() && !requested.contains(['/','\\','.']) => (requested.to_string(),None),
        _ => ("YensidsLab".to_string(),Some("HouseStart".to_string())),
    };
    let member=crate::pack_io::index(&levels)?.into_iter().find(|m|m.path.eq_ignore_ascii_case(&format!("levels/{level_name}.lvlc"))).with_context(||format!("Level descriptor levels/{level_name}.lvlc is absent"))?;
    let level=Arc::new(super::compiled_level::Level::parse(&member.read(&levels)?)?);
    let state_index=match &state_name {
        Some(s)=>level.states.iter().position(|x|x.name.eq_ignore_ascii_case(&format!("{level_name}.{s}"))||x.name.eq_ignore_ascii_case(s)||x.name.rsplit('.').next().is_some_and(|n|n.eq_ignore_ascii_case(s))).with_context(||format!("{level_name} has no state {s}"))?,
        None=>level.states.iter().position(|x|x.links.iter().any(|l|l.kind==1)).unwrap_or(0),
    };
    let state=&level.states[state_index];
    let startup_links=state.links.iter().any(|l|l.kind==1);
    let mut scene_resources=Vec::new(); let mut asset_resources=Vec::new(); let mut bank_resources=Vec::new();
    for link in &state.links {
        // HouseStart contains explicit startup load links (kind 1); states without any load every zone they list.
        if startup_links && link.kind != 1 { continue; }
        let zone=level.zones.iter().find(|z|z.name.eq_ignore_ascii_case(&link.zone)).with_context(||format!("Unknown startup zone {}",link.zone))?;
        for file in &zone.files {
            if file.path.to_ascii_lowercase().ends_with(".gsa") { scene_resources.push(file.path.clone()); }
            if file.path.to_ascii_lowercase().ends_with(".asl") { asset_resources.push(file.path.clone()); }
            if file.path.to_ascii_lowercase().ends_with(".bnk") { bank_resources.push(file.path.clone()); }
        }
    }
    let mut files: Vec<VirtualFile>=Vec::new(); let mut seen=HashSet::new(); let mut scenes=Vec::new();
    // ASL archives carry shared models/textures omitted from individual scene
    // packs. Publish this catalog before resolving any scene references.
    let mut split_catalog=super::streamed_animation_resources::Catalog::default();
    let mut loaded_archives=HashSet::new(); let mut scene_names=Vec::new();
    for resource in asset_resources.iter().chain(scene_resources.iter()) {
        let archive=archive_path(root,safe_stem(resource)?)?;
        if !loaded_archives.insert(archive.clone()) { continue; }
        split_catalog.mount(&archive)?;
        let pack=crate::pack_io::read(std::fs::read(&archive)?)?;
        if resource.to_ascii_lowercase().ends_with(".gsa") {
            let wanted=format!("{}.bin",&resource[..resource.len()-4]);
            let source=pack.pak.files.iter().find(|f|f.path.eq_ignore_ascii_case(&wanted)).with_context(||format!("Scene {wanted} missing from {}",archive.display()))?;
            scenes.push(super::scene_loading::parse_scene(&wanted,&source.data)?);scene_names.push(wanted);
        }
        for file in pack.pak.files { if seen.insert(file.path.to_ascii_lowercase()) { files.push(file); } }
    }
    // Native PlayerManager creates the archetype from the dynamic palette;
    // CostumeManager mounts its selected model separately from scene packs.
    for stem in ["globalscripts","_Dynamic","default_mickey_costume","default_oswald_costume"] {
        let archive=archive_path(root,stem)?;
        split_catalog.mount(&archive)?;
        let pack=crate::pack_io::read(std::fs::read(&archive)?)?;
        for file in pack.pak.files {if seen.insert(file.path.to_ascii_lowercase()){files.push(file);}}
    }
    let chunk_archive=root.join("animations.chf");
    if chunk_archive.is_file(){split_catalog.mount_chunks(&chunk_archive)?;}
    let split_catalog=Arc::new(split_catalog);
    let globals_archive=archive_path(root,"ApprenticeGlobal")?;
    let global_member=crate::pack_io::index(&globals_archive)?.into_iter()
        .find(|m|m.path.eq_ignore_ascii_case("apprentice/globalstate/ApprenticeGlobalState.bsq"))
        .context("Missing EM2 global state database")?;
    let mut global_state=super::em2_global_state::State::load(&global_member.read(&globals_archive)?)?;
    let dynamic=files.iter().find(|f|f.path.eq_ignore_ascii_case("palettes/_dynamic.bin")).context("Missing dynamic player palette")?;
    let palette=super::scene_loading::parse_scene(&dynamic.path,&dynamic.data)?;
    let player=super::player_source::prepare(&scenes,&palette,super::player_source::pc_archetype(0,0).context("Invalid primary player control type")?,0)?;
    let mut player_report=player.summary();
    let player_spawn=scene::get_entity_transform(player.entity(),&player.scene.objects).translation;
    let mut player_hull=None;
    match player.collision_hull(&files) {
        Ok((path,hull,support))=>{
            let min: [f32;3]=std::array::from_fn(|axis|hull.vertices.iter().map(|p|p[axis]).fold(f32::INFINITY,f32::min));
            let max: [f32;3]=std::array::from_fn(|axis|hull.vertices.iter().map(|p|p[axis]).fold(f32::NEG_INFINITY,f32::max));
            player_report["collision_hull"]=serde_json::json!({"resource":path,"vertices":hull.vertices.len(),"packed_vertices":support.packed_vertices.len(),
                "radius":hull.radius,"bounds_min":min,"bounds_max":max,"stored_center":support.stored_center,"simulation_active":false});
            player_hull=Some(hull);
        },
        Err(error)=>{player_report["collision_error"]=serde_json::json!(format!("{error:#}"));},
    }

    let player_name=player.entity().name.clone();
    // EM2's own movement tuning, from the player's authored MickeyPlayerMotionControllerComponent
    // (EM1's values are only the starting point for anything the template doesn't set).
    let mut player_motion=super::physics::Motion::default();
    {
        let components=scene::get_effective_components(player.entity(),&player.scene.objects);
        let property=|name:&str|components.iter().flat_map(|c|&c.properties).find(|p|p.name==name).map(|p|&p.value);
        match super::authored_motion::apply(property,&mut player_motion) {
            Ok(())=>player_report["motion"]=serde_json::to_value(&player_motion).unwrap_or_default(),
            Err(error)=>{player_report["motion_error"]=serde_json::json!(format!("{error:#}"));player_motion=super::physics::Motion::default();},
        }
        // EM2 skid / 180 turn settings (MickeyPlayerMotionControllerComponent)
        // The entity only stores values that differ from the component defaults. Defaults: the
        // shared EM1/EM2 MickeyPlayerMotionController template (identical in all 192 EM1 examples,
        // and every value EM2 does store matches it).
        let defaults:Vec<(&str,serde_json::Value)>=[("SkidMaxInput",0.1),("SkidMinSpeed",0.4),("SkidTime",0.2),("SkidTimeToCheck",0.09999),
            ("The180MinInput",0.225),("The180MinSpeed",0.2),("The180TimeToCheck",0.16666),("The180Cosine",-0.6),("The180MinTimeBetween180s",0.09999),
            ("The180OrSkidMinTimeOnGround",0.09999),("The180Time",0.39999),("The180DecelRate",12.0)].into_iter().map(|(k,v)|(k,serde_json::json!(v))).collect();
        let with_defaults=|name:&str|property(name).or_else(||defaults.iter().find(|(k,_)|*k==name).map(|(_,v)|v));
        match super::locomotion_actions::Settings::load(with_defaults) {
            Ok(settings)=>{player_motion.em2_moves=Some(settings);},
            Err(error)=>{player_report["em2_moves_error"]=serde_json::json!(format!("{error:#}"));},
        }
        if let Some(v)=property("The180MinAllowedSpeed").and_then(|v|v.as_f64()) {player_motion.turn_min_allowed_speed=v as f32;}
        player_report["em2_moves"]=serde_json::to_value(&player_motion.em2_moves).unwrap_or_default();
        player_report["motion_properties"]=serde_json::json!(components.iter().filter(|c|c.class_name.contains("Motion")||c.class_name.contains("PlayerController"))
            .map(|c|serde_json::json!({"class":c.class_name,"properties":c.properties.iter().map(|p|(p.name.clone(),p.value.clone())).collect::<serde_json::Map<_,_>>()})).collect::<Vec<_>>());
    }
    // PC CreatePlayers (979fa0..97a197) creates the AI companion in
    // single-player mode before tutorial scripts can disable/recreate it.
    // Allocate from the growing palette so opaque entity/component IDs cannot
    // collide between the two independently authored archetypes.
    let companion=super::player_source::prepare(&scenes,&player.scene,super::player_source::pc_archetype(1,1).context("Invalid companion control type")?,1)?;
    let mut companion_report=companion.summary();
    let companion_name=companion.entity().name.clone();
    companion_report["control_type"]=serde_json::json!(1);
    companion_report["role"]=serde_json::json!("AI companion; behavior and full lifecycle are not active");
    let mut actors_source=companion.scene;
    actors_source.scene.splice(0..0,player.scene.scene.iter().cloned());
    scenes.push(actors_source);scene_names.push(dynamic.path.clone());
    drop(player);
    let child_graph=super::em2_children::expand(&mut scenes,&mut scene_names,&files)?;
    let camera_trackers=super::em2_camera_tracking::prepare(&mut scenes,&mut scene_names,&child_graph.templates,[&player_name,&companion_name])?;
    let default_cameras=super::em2_camera_manager::prepare(&mut scenes,&mut scene_names,&child_graph.templates)?;
    let mut child_poses=super::em2_child_pose::State::load(&child_graph,&scenes,&files,&settings);
    let mut actors=Vec::new();let mut actor_indices=HashMap::new();
    for (scene_index,source) in scenes.iter().enumerate() {
        for entity in source.objects.iter().filter(|e|source.scene.contains(&e.link_id)) {
            let components=scene::get_effective_components(entity,&source.objects);
            if !components.iter().any(|c|c.class_name=="AnimationComponent"){continue;}
            let mut actor=AnimatedActor::load(scene_index,entity.link_id.to_u32(),&entity.name,&components,&files);
            if let Some(character)=actor.playback.as_mut(){character.set_split_catalog(split_catalog.clone());}
            actor_indices.insert((scene_index,entity.link_id.to_u32()),actors.len());actors.push(actor);
        }
    }
    let (static_collision,static_collision_error)=match player_hull.as_ref() {
        Some(hull)=>match super::scene_static_collision::load(&scenes,&files,hull,player_spawn) {
            Ok(loaded)=>(Some(loaded),None),Err(error)=>(None,Some(format!("{error:#}"))),
        },
        None=>(None,Some("Player collision hull was not admitted".to_owned())),
    };
    let collision_report=serde_json::json!({"world":static_collision.as_ref().map(|loaded|&loaded.report),"error":static_collision_error});
    let registry=scenes.iter().flat_map(|scene|scene.objects.iter().filter(|entity|scene.scene.contains(&entity.link_id)))
        .enumerate().map(|(i,entity)|(i,entity.name.clone())).collect::<Vec<_>>();
    let mut scripts=super::actor_scripts::Scripts::new(&registry)?;
    let player_owner=registry.iter().find(|(_,name)|*name==player_name).context("Admitted player is absent from script registry")?.0;
    scripts.set_player(player_owner)?;
    let companion_owner=registry.iter().find(|(_,name)|*name==companion_name).context("Admitted companion is absent from script registry")?.0;
    scripts.set_em2_companion(companion_owner,true)?;
    scripts.configure_em2_interface()?;
    for file in &files {if file.path.to_ascii_lowercase().ends_with(".lua") {scripts.import_source(&file.path,&file.data)?;}}
    // ScriptManager::LoadGlobalScripts mounts the archive and executes this
    // module first; its Openfile calls retain their original execution order.
    let global_script="environments/_shared/scripts/globalscripts.lua".to_owned();
    let mut requested_scripts=vec![global_script.clone()];
    let mut known_scripts=HashSet::from([global_script.to_ascii_lowercase()]);
    for source in &scenes {for entity in &source.objects {
        if !source.scene.contains(&entity.link_id){continue;}
        for component in scene::get_effective_components(entity,&source.objects) {
            if component.class_name!="ScriptComponent"{continue;}
            for property in component.properties.iter().filter(|p|p.name=="Lua File Path") {
                for path in super::actor_scripts::authored_paths(&property.value)? {
                    if known_scripts.insert(path.to_ascii_lowercase()){requested_scripts.push(path);}
                }
            }
        }
    }}
    let mut script_module_errors=Vec::new();
    for path in &requested_scripts {
        let result=scripts.with_world_host(|lua,name,args|global_state.call(lua,name,args),
            |scripts|scripts.ensure_resource_module(path));
        if let Err(error)=result {
            script_module_errors.push(serde_json::json!({"path":path,"error":format!("{error:#}")}));
            break;
        }
    }
    if script_module_errors.is_empty() {
        if let Some(path)=options.runtime_script.as_ref() {scripts.load(&path.to_string_lossy(),&std::fs::read(path)?)?;}
    }
    let (mut scene_events,scene_events_error)=if !script_module_errors.is_empty() {
        (None,Some("Required script startup failed; scene callbacks were not dispatched".to_owned()))
    } else {match player_hull.as_ref() {
        Some(hull)=>match super::scene_event_runtime::Runtime::load_pc(&scenes,&scene_names,hull,player_owner,&mut scripts,&settings,super::em2_audio::Catalog::load(root,&movie_language,&bank_resources),super::em2_level_manager::Manager::new(level.clone(),state_index)?,&files) {
            Ok(mut events)=>match events.install_children(&child_graph).and_then(|()|events.install_camera_trackers(&camera_trackers)).and_then(|()|events.install_default_cameras(&default_cameras,&settings)){Ok(())=>(Some(events),None),Err(error)=>(None,Some(format!("{error:#}")))},Err(error)=>(None,Some(format!("{error:#}"))),
        },None=>(None,Some("Scene events require the source player hull".to_owned())),
    }};
    let event_report=serde_json::json!({"world":scene_events.as_ref().map(|events|events.summary()),"error":scene_events_error});
    let script_report=serde_json::json!({"requested_modules":requested_scripts,"module_errors":script_module_errors,
        "memory":scripts.memory_summary(),"scheduler":scripts.task_snapshot(),"queued_world_commands":scripts.queued_world_commands()?,
        "scene_initialization_dispatched":false,"engine":"shared native Lua VM with HKS instruction adapter"});
    super::em2_tex_anim::reset();
    let mut draws=Vec::new();let mut projection_centers=Vec::new();let mut projection_sources=HashMap::<String,Result<std::collections::BTreeMap<u32,Result<super::scene_bounds::Sphere,String>>,String>>::new(); let mut draw_sources=Vec::new(); let mut hidden_draws=HashSet::new(); let mut startup_hidden=Vec::new(); let mut cache=HashMap::new(); let mut missing=Vec::new(); let mut entity_count=0;
    let mut particle_blocks=HashMap::<String,HashSet<usize>>::new();let mut particle_system_draws=Vec::new();
    let mut target=None; let mut lights=Vec::new();
    let mut player_entities=Vec::new(); let mut authored_camera=None;
    let mut baked_lighting=BakedLighting::load();
    let (lit_files,lit_meshes)=baked_lighting.load_level(&files);
    eprintln!("[lit] level baked lighting: {lit_meshes} meshes from {lit_files} .lit files");
    let mut mesh_names=HashMap::<String,HashMap<usize,String>>::new();
    let mut textures=HashMap::new(); let mut failed_textures=HashSet::new();
    let mut shader_catalog=super::d3d9_shader::Catalog::open(&archive_path(root,"shaders")?)?;
    // BLOTWORX_DUMP_FX=dir[:name,name...]: write the GLSL and parameters of whole effects (post-process study)
    if let Ok(spec)=std::env::var("BLOTWORX_DUMP_FX") {
        let (dir,names)=spec.split_once('|').unwrap_or((spec.as_str(),"screenspace_depthfog,pp_bloom,pp_colourcorrection,pp_fxaa,gen3_fx_fog_offscreen,pp_copybackbuffer"));
        for name in names.split(',') {
            let mut out=String::new();
            match shader_catalog.get(name) {
                Err(e)=>out+=&format!("error {e}\n"),
                Ok(effect)=>match effect.layout.as_ref() {
                    Err(e)=>out+=&format!("layout error {e}\n"),
                    Ok(layout)=>{
                        out+="==== PARAMETERS\n";
                        for (i,p) in layout.parameters.iter().enumerate() {out+=&format!("[{i}] {:?}\n    annotations {:?}\n",p.value,effect.binding_annotations.get(i));}
                        for t in &layout.techniques {for p in &t.passes {
                            let (tn,pn)=(t.name.clone().unwrap_or_default(),p.name.clone().unwrap_or_default());
                            out+=&format!("==== TECHNIQUE {tn} PASS {pn}\n");
                            match effect.translate_pass(&tn,&pn) {
                                Err(e)=>out+=&format!("translate error {e:#}\n"),
                                Ok(pass)=>{
                                    if let Ok(states)=effect.render_states(&pass) {out+=&format!("states {:?}\n",states.iter().map(|s|(s.state,s.value)).collect::<Vec<_>>());}
                                    for (stage,t,program) in [("VERTEX",&pass.vertex,pass.vertex_program),("PIXEL",&pass.pixel,pass.pixel_program)] {
                                        out+=&format!("---- {stage}\n");
                                        if let Some(table)=effect.programs.get(program).and_then(|p|p.constants.as_ref()) {out+=&format!("CTAB {:?}\n",table.constants);}
                                        for u in &t.uniforms {out+=&format!("uniform kind={} c{} count={} constant={} name={:?}\n",u.kind,u.index,u.count,u.constant,u.name);}
                                        for s in &t.samplers {out+=&format!("sampler kind={} s{} name={:?}\n",s.kind,s.index,s.name);}
                                        out+=&t.source;out+="\n";
                                    }
                                },
                            }
                        }}
                    },
                },
            }
            let _=std::fs::create_dir_all(dir);let _=std::fs::write(std::path::Path::new(dir).join(format!("{name}.txt")),out);
        }
    }
    let mut material_errors=std::collections::BTreeSet::new();
    let mut resource_catalog=super::d3d9_material::Catalog::new(&files)?;
    let mut native_materials:NativeMaterials=HashMap::new(); let mut r3mt_entities=0usize;
    for (scene_index,source) in scenes.iter().enumerate() {
        lights.extend(scene::runtime_lights_filtered(source,|entity|!super::em2_entity_startup::State::decode(entity.unknown,entity.unknown_em2).start_hidden));
        for entity in &source.objects {
            if !source.scene.contains(&entity.link_id) { continue; }
            entity_count+=1;
            let startup=super::em2_entity_startup::State::decode(entity.unknown,entity.unknown_em2);
            if startup.start_hidden {startup_hidden.push(serde_json::json!({"scene":scene_index,"entity":entity.name,"state":startup}));}
            let matrix=scene::runtime_matrix(entity,&source.objects);
            // Preserve the same inherited light assignment as the shared EM1
            // importer. An empty group means dynamic/all lights in the renderer.
            let components=scene::get_effective_components(entity,&source.objects);
            let light_group:Arc<str>=components.iter().flat_map(|component|&component.properties)
                .find(|property|property.name=="Light Group")
                .and_then(|property|property.value.as_str()).map(Arc::from)
                .unwrap_or_else(||Arc::from(""));
            // R3MTEffectsComponent colour overrides (R3MTConstantColor_0..3, R3MTColorRegister_0..2),
            // used by the game's effect shaders as the "MeshData" ConstantColor/ColorRegister inputs.
            // Slots 0-3 = constant colours, 4-6 = colour registers; disabled slots hold -1.
            let r3mt:Option<[[f32;4];7]>=components.iter().find(|c|c.class_name=="R3MTEffectsComponent").and_then(|effects|{
                let mut regs=[[-1f32;4];7];let mut any=false;
                for (slot,key) in (0..4).map(|i|(i,format!("R3MTConstantColor_{i}"))).chain((0..3).map(|i|(4+i,format!("R3MTColorRegister_{i}")))) {
                    if !effects.properties.iter().any(|p|p.name==format!("{key}_Enable")&&p.value.as_bool()==Some(true)) {continue;}
                    let Some(v)=effects.properties.iter().find(|p|p.name==key).map(|p|&p.value) else {continue};
                    let ch=|n:&str|v.get(n).and_then(|x|x.as_f64()).unwrap_or(1.) as f32;
                    regs[slot]=[ch("r"),ch("g"),ch("b"),ch("a")];any=true;
                }
                any.then_some(regs)
            });
            if r3mt.is_some() {r3mt_entities+=1;}
            // Until the startup script selects its camera, start inspection at
            // the authored house camera, never at an unrelated Player_Marker.
            if entity.name.eq_ignore_ascii_case("ysl_yensidslab_mickeyhouse.MickeyTVIGCCam") {
                authored_camera=Some(matrix);
            }
            let is_player=entity.name==player_name;
            let actor_index=actor_indices.get(&(scene_index,entity.link_id.to_u32())).copied();
            if is_player {
                target=Some([matrix[12],matrix[13],matrix[14]]); player_entities.push(entity.name.clone());
            }
            let Some(path)=scene::find_entity_nif_path_with_master(entity,&source.objects) else {continue};
            let key=scene::ensure_nif_cached(&path,&files,&mut cache);
            let meshes=&cache[&key];
            // the level's sky (beyond the far plane): drawn in the renderer's sky pass
            let is_sky={let p=path.to_ascii_lowercase();p.contains("skybox")||p.contains("skydome")};
            if meshes.is_empty() {
                if !scene_events.as_ref().is_some_and(|events|events.camera_source(scene_index,entity.link_id.to_u32())){missing.push(path);}
                continue;
            }
            if !particle_blocks.contains_key(&key){
                let file=crate::wii_mesh::resolve_model(&files,&path).context("Cached model source is unavailable for particle classification")?;
                let nif=epic_mickey_lib_rs::nif_file::NifFile::load_bytes(&file.data).map_err(anyhow::Error::msg)?;
                particle_blocks.insert(key.clone(),super::scene_bounds::particle_geometry_blocks(&nif));
                mesh_names.insert(key.clone(),nif.blocks.iter().enumerate().filter_map(|(i,block)|match block {epic_mickey_lib_rs::nif_file::NiObject::NiMesh(m)=>Some((i,m.base.base.base.name.to_string())),_=>None}).collect());
                let bounds=(||->Result<_>{
                    let graph=super::scene_bounds::Hierarchy::load_with_source(&nif,&file.data)?;
                    super::em2_tex_anim::register_model(&key,&graph);
                    let parent=epic_mickey_lib_rs::nif_file::NiTransform {scale:1.,..Default::default()};
                    let mut bounds=graph.mesh_world_bounds(&parent,&Default::default(),false,false)?;
                    for (id,bound) in &mut bounds {
                        if graph.nodes[id].needs_runtime_bound {*bound=Err(format!("Mesh {id} requires its runtime bound producer"));}
                    }Ok(bounds)
                })().map_err(|error|format!("{error:#}"));
                projection_sources.insert(key.clone(),bounds);
            }
            for mesh in meshes {
                if particle_blocks[&key].contains(&mesh.mesh_index){
                    particle_system_draws.push(serde_json::json!({"entity":entity.name,"asset":path,"block":mesh.mesh_index,"scope":"Particle-system preview geometry is not a static game draw; live particle playback pending"}));continue;
                }
                if let Some(shader)=mesh.material.shader.as_deref() {
                    let identity=Arc::as_ptr(&mesh.material) as usize;
                    if !native_materials.contains_key(&identity) {
                        let result=shader_catalog.get_material(shader,None).map_err(anyhow::Error::msg).and_then(|effect| {
                            let maps=mesh.material.textures.iter().map(|map|super::d3d9_material::Map {shader:map.shader_id.is_some(),slot:map.slot,path:map.path.clone()}).collect::<Vec<_>>();
                            if std::env::var_os("BLOTWORX_DEBUG_MAPS").is_some() {eprintln!("[maps] {shader}: {:?}",mesh.material.textures.iter().map(|m|(m.shader_id,m.slot,m.uv_set,m.path.rsplit(['/','\\']).next().unwrap_or(""))).collect::<Vec<_>>());}
                            resource_catalog.prepare(effect,&maps).map(Arc::new)
                        }).map_err(|e|format!("{shader}: {e:#}"));
                        if let Err(error)=&result {material_errors.insert(error.clone());}
                        native_materials.insert(identity,(Arc::downgrade(&mesh.material),result));
                    }
                }
                let shader_time=if native_materials.get(&(Arc::as_ptr(&mesh.material) as usize)).and_then(|(_,result)|result.as_ref().ok()).is_some_and(|resources|resources.mesh_time) {
                    let existing=match mesh.material.attributes.as_ref() {
                        Some(Ok(table))=>match table.get("NDL_UpdateTime") {Some(crate::nif_shader_attributes::Attribute::Float(word))=>Some(f32::from_bits(*word)),_=>None},
                        Some(Err(error))=>anyhow::bail!("Mesh time attribute source: {error}"),None=>None,
                    };
                    Some(crate::nif_shader_time::Clock::new(existing)?)
                }else{None};
                let tex_bytes=mesh.texture_name.as_deref().and_then(|name| {
                    let key=name.replace('\\',"/").trim_start_matches('/').to_ascii_lowercase();
                    textures.entry(key.clone()).or_insert_with(|| {
                        let decoded=crate::render_support::nif::find_pak_texture(&files,name)
                            .and_then(|f|crate::render_support::nif::decode_image_bytes(&f.data))
                            .map(|(pixels,w,h)|(Arc::new(pixels),w,h));
                        if decoded.is_none() {failed_textures.insert(key);}
                        decoded
                    }).clone()
                });
                if let Some(index)=actor_index {actors[index].bind(draws.len(),mesh);}
                if let Some(events)=scene_events.as_mut(){events.bind_gamebryo_draw(scene_index,entity.link_id.to_u32(),&path,draws.len(),mesh)?;}
                if startup.start_hidden {hidden_draws.insert(draws.len());}
                draw_sources.push(super::model_body_binding::DrawSource {scene:scene_index,entity:entity.link_id.to_u32(),path:mesh.hierarchy_path.clone(),original_transform:matrix});
                projection_centers.push(projection_sources.get(&key).and_then(|result|result.as_ref().ok()).and_then(|bounds|bounds.get(&(mesh.mesh_index as u32))).and_then(|result|result.as_ref().ok()).map(|bound|bound.center));
                // the sky keeps its authored vertex colours (the haze layer's tint and fade live there)
                // 2 x vertex colour is the game shader's baked term; the default 2.6 also stands in for the
                // sunlight/specular/grading the fallback doesn't reproduce (matched to retail screenshots)
                let lit_scale=baked_lighting.scale;
                let baked_colors=if is_sky {None} else {baked_lighting.apply(mesh,mesh_names.get(&key).and_then(|n|n.get(&mesh.mesh_index)))};
                if let Some(path)=std::env::var_os("BLOTWORX_LIT_REPORT") {
                    use std::io::Write;
                    let name=mesh_names.get(&key).and_then(|n|n.get(&mesh.mesh_index)).cloned().unwrap_or_default();
                    let vertices=mesh.source_indices.as_ref().and_then(|s|s.iter().copied().max()).map_or(0,|m|m as usize+1);
                    let known=baked_lighting.by_name.get(&name.to_ascii_lowercase()).map(|l|l.iter().map(|(_,c)|c.len()/4).collect::<Vec<_>>());
                    if let Ok(mut f)=std::fs::OpenOptions::new().create(true).append(true).open(path) {
                        let _=writeln!(f,"{}\t{}\t{}\t{}\t{:?}\t{}\t{}",if baked_colors.is_some(){"LIT"}else{"MISS"},key,name,vertices,known,mesh.texture_name.as_deref().unwrap_or(""),mesh.material.shader.as_deref().unwrap_or("-"));
                        let _=writeln!(f,"POS\t{}\t{:?}\t{:?}\t{:?}",key,[matrix[12],matrix[13],matrix[14]],mesh.aabb_min,mesh.aabb_max);
                    }
                }
                // surfaces with baked lighting are already lit: don't add every scene light on top
                // (that double-lighting blew big facades out to white). BLOTWORX_EM2_BAKED_ONLY=0 restores it.
                // meshes without level lighting use their model's own vertex colours, x2 like the game's
                // shader (Gen3_StaticEnv: lighting = 2 x vertex colour); BLOTWORX_EM2_NIF_BAKED=0 turns it off
                let baked_colors=baked_colors.or_else(||(!is_sky&&std::env::var("BLOTWORX_EM2_NIF_BAKED").map_or(true,|v|v!="0")).then(||{
                    let mut v=(*mesh.v_data).clone();for c in v.chunks_exact_mut(12){for j in 8..11 {c[j]*=lit_scale;}}Arc::new(v)
                }));
                let baked=baked_colors.is_some()&&std::env::var("BLOTWORX_EM2_BAKED_ONLY").map_or(true,|v|v!="0");
                draws.push(GpuDrawCommand { shader_time,source_attributes:mesh.source_attributes.clone(),projected_shadow:false,skybox:is_sky,extra_uvs:mesh.extra_uvs.clone(),projection_bound:None,
                    material_registers:r3mt,material_indirect:None,material_images:Arc::new(Vec::new()),baked,baked_error:None,
                    preview:None,uv_transform:mesh.uv_transform.clone(),material:Some(mesh.material.clone()),sampler:mesh.sampler,
                    v_data:baked_colors.unwrap_or_else(||mesh.v_data.clone()),indices:mesh.indices.clone(),trans_mat:matrix,color:[1.;4],tex_bytes,
                    aabb_min:mesh.aabb_min,aabb_max:mesh.aabb_max,light_group:light_group.clone() });
                super::em2_tex_anim::register_draw(&key,mesh.mesh_index as u32,draws.last().unwrap());
            }
        }
    }
    eprintln!("[texanim] {} draws with animated texture transforms",super::em2_tex_anim::count());
    child_poses.bind_draws(&draw_sources);
    let model_bodies=static_collision.as_ref().map(|world|super::model_body_binding::State::prepare(&world.dynamic_sources,&scenes,&files,&draw_sources));
    let model_body_report=model_bodies.as_ref().map(|state|state.report.clone());
    for actor in &mut actors {actor.apply(&mut draws);}
    let actor_report=actors.iter().map(AnimatedActor::summary).collect::<Vec<_>>();
    for (name,report) in [(&player_name,&mut player_report),(&companion_name,&mut companion_report)] {
        if let Some(actor)=actors.iter().find(|actor|&actor.name==name) {
            report["animation"]=actor.playback.as_ref().map(|c|c.summary()).unwrap_or(serde_json::Value::Null);
            report["animation_error"]=serde_json::json!(actor.error);
            report["draws"]=serde_json::json!(actor.draws.len());
        }
    }
    let native_material_report=native_materials.values().map(|(source,result)|serde_json::json!({"shader":source.upgrade().and_then(|m|m.shader.clone()),"resources":result.as_ref().ok().map(|r|r.summary()),"error":result.as_ref().err()})).collect::<Vec<_>>();
    let projection_report=projection_sources.iter().map(|(path,bounds)|serde_json::json!({"asset":path,"ready":bounds.as_ref().map(|bounds|bounds.values().filter(|value|value.is_ok()).count()).unwrap_or(0),"error":bounds.as_ref().err(),"runtime_bounds_required":bounds.as_ref().map(|bounds|bounds.values().filter(|value|value.is_err()).count()).unwrap_or(0)})).collect::<Vec<_>>();
    let report=serde_json::json!({"r3mt_override_entities":r3mt_entities,"projection_bound_sources":projection_report,"original_material_resources":native_material_report,"original_image_sources":resource_catalog.summary(),"original_shaders":shader_catalog.summary(),"material_errors":material_errors,"game":"EM2 PC","startup_state":state.name,"diagnostic_script":options.runtime_script,"scene_resources":scene_resources,
        "particle_system_draws":particle_system_draws,"asset_resources":asset_resources,"camera_target":target,"authored_camera":authored_camera,"player_entities":player_entities,"lights":lights.len(),
        "scenes":scenes.len(),"placed_entities":entity_count,"draws":draws.len(),"failed_models":missing,
        "failed_textures":failed_textures,"decoded_textures":textures.len(),"mesh_time_controllers":draws.iter().filter(|draw|draw.shader_time.is_some()).count(),
        "child_entities":child_graph.summary(),"child_poses":child_poses.summary(),"camera_tracker_sources":camera_trackers,"actors":actor_report,"companion":companion_report,"global_state":global_state.summary(),"player":player_report,"scripts":script_report,"scene_events":event_report,"collision":collision_report,"body_bindings":model_body_report,
        "startup_hidden_entities":startup_hidden,"startup_hidden_draws":hidden_draws.len(),"gameplay_implemented":false,"material_mode":"original PC effects with live viewport bridge; unresolved inputs use diffuse fallback","compiled_level":&*level});
    if let Some(path)=options.inspect.as_ref() { std::fs::write(path,serde_json::to_vec_pretty(&report)?)?; return Ok(None); }
    if scene_events.is_none() {
        if let Some(path)=options.capture.as_ref() {std::fs::write(path.with_extension("runtime.json"),serde_json::to_vec_pretty(&report)?)?;}
        anyhow::bail!("EM2 scene initialization failed: {}",scene_events_error.as_deref().unwrap_or("No scene runtime"));
    }
    ensure!(!draws.is_empty(),"EM2 startup loaded no drawable meshes: {report}");
    let mut vp=Viewport3DState::default();vp.show_grid=false;vp.show_limitations=false;
    // EM2 scene finishing (tone, saturation, bloom), calibrated against the PC game. BLOTWORX_EM2_GRADE
    // overrides it: "exposure,contrast,saturation,gamma,warmth,bloom,threshold".
    // Off unless asked for: the finishing pass costs a full-screen pass every frame.
    if let Ok(s)=std::env::var("BLOTWORX_EM2_GRADE"){let mut g=crate::render_support::scene_finish::Settings::default();{let v:Vec<f32>=s.split(',').filter_map(|x|x.trim().parse().ok()).collect();
        for (i,x) in v.iter().enumerate(){match i{0=>g.exposure=*x,1=>g.contrast=*x,2=>g.saturation=*x,3=>g.gamma=*x,4=>g.warmth=*x,5=>g.bloom=*x,6=>g.threshold=*x,_=>{}}}}
     vp.finish=g;}
    if let Some(matrix)=authored_camera {
        let forward=[matrix[8],matrix[9],matrix[10]];
        vp.distance=5.;
        vp.camera_target=std::array::from_fn(|i|matrix[12+i]+forward[i]*vp.distance);
        vp.yaw=(-forward[2]).atan2(-forward[0]);
        vp.pitch=(-forward[1]).clamp(-1.,1.).asin();
    } else if let Some(target)=target { vp.camera_target=target; vp.distance=30.; }
    if let Some(focus)=options.actor_focus.as_deref(){
        let name=if focus.eq_ignore_ascii_case("player"){&player_name}
            else if focus.eq_ignore_ascii_case("companion") || focus.eq_ignore_ascii_case("oswald"){&companion_name}else{focus};
        let actor=actors.iter().find(|actor|actor.name.eq_ignore_ascii_case(name)).context("Unknown startup actor focus")?;
        let mut min=[f32::INFINITY;3];let mut max=[f32::NEG_INFINITY;3];
        for &index in &actor.draws {let draw=&draws[index];for vertex in draw.v_data.chunks_exact(12){
            for axis in 0..3 {let v=draw.trans_mat[axis]*vertex[0]+draw.trans_mat[4+axis]*vertex[1]+draw.trans_mat[8+axis]*vertex[2]+draw.trans_mat[12+axis];min[axis]=min[axis].min(v);max[axis]=max[axis].max(v);}
        }}
        ensure!(min.iter().chain(&max).all(|v|v.is_finite()),"Actor focus has no rendered bounds");
        vp.camera_target=std::array::from_fn(|i|(min[i]+max[i])*0.5);
        let radius=(0..3).map(|i|(max[i]-min[i]).powi(2)).sum::<f32>().sqrt()*0.5;
        vp.distance=radius/(vp.fov_degrees.to_radians()*0.5).sin()*1.1;
    }
    if let Some(target)=options.capture_camera_target { vp.camera_target=target; }
    // BLOTWORX_CAPTURE_VIEW=yaw,pitch,distance (radians, units): orbit angle for --capture-camera-target shots
    if let Ok(view)=std::env::var("BLOTWORX_CAPTURE_VIEW") {
        let v:Vec<f32>=view.split(',').filter_map(|x|x.trim().parse().ok()).collect();
        if let [yaw,pitch,distance]=v[..] {vp.yaw=yaw;vp.pitch=pitch;vp.distance=distance;}
    }
    // Draws own shared vertex/index/pixel allocations. Release packed sources,
    // scene graphs and import caches before entering the presentation loop.
    drop(resource_catalog);drop(shader_catalog);drop(textures);drop(cache);drop(files);drop(scenes);
    let collision_debug=options.debug;
    let collision_lines=if collision_debug {collision_overlay(static_collision.as_ref(),model_bodies.as_ref())}else{Vec::new()};
    // EM2 sound effects, ambience and music (not in screenshot captures)
    let audio_player=if options.capture.is_none() {scene_events.as_ref().and_then(|events|{
        match super::em2_audio_player::Player::load(&events.audio_bank_files(),&root.join("Audio"),&movie_language) {
            Ok(mut player)=>{if !player.errors.is_empty(){log::warn!("EM2 audio: {} bank records skipped: {}",player.errors.len(),player.errors[0]);}player.start_level_music();Some(player)},
            Err(error)=>{log::error!("EM2 audio unavailable: {error:#}");None},
        }
    })} else {None};
    let playtest=if options.shared_playtest||options.input_playback.is_some(){
        let mut controller=super::controller::Controller::new(super::physics::Player::new(player_spawn,player_motion.clone()));
        if let Some(events)=scene_events.as_ref(){let pose=events.playtest_player_matrix();controller.player.yaw=pose[8].atan2(pose[10]);}
        controller.pitch_limits=cursor_settings.pitch_limits();controller.camera_yaw=vp.yaw;controller.camera_pitch=vp.pitch;Some(controller)
    }else{None};
    if playtest.is_some(){if let Some(events)=scene_events.as_mut(){events.enable_playtest_dialogue(&movie_root,&movie_language)?;}}
    let playback=options.input_playback;
    if let Some(recording)=playback.as_ref(){
        ensure!(recording.frames.iter().all(|f|!f.spin_pressed&&!f.anvil_root_pressed&&!f.respawn_pressed&&!f.paint&&!f.thinner&&!f.movie_skip_held&&f.aim.is_none()),"EM2 shared playtest accepts movement, jump/contextual marker use, camera and dialog acknowledgements; other ability/cutscene actions remain unconnected");
    }
    vp.native_material=Some(Arc::new(std::sync::Mutex::new(super::d3d9_draw::Renderer::new(native_materials.into_iter().collect(),Box::new(super::d3d9_draw::ViewportScene {profiles:scene_events.as_ref().map(|events|events.pc_render_profiles())})))));
    vp.pixel_probe=options.render_probe;
    let capture=options.capture;
    let capture_tick=options.capture_tick.unwrap_or(3);
    Ok(Some(Bootstrap {preferences:super::options_menu::Preferences::load(),options_open:std::env::var_os("BLOTWORX_OPTIONS_OPEN").is_some(),options_tab:0,preferences_dirty:false,options_f6:false,follow_camera:Default::default(),smooth:None,perf:[0.;3],player_motion:player_motion.clone(),jump_latch:false,look_accum:[0.;2],cursor_cam:super::em2_cursor_camera::CursorCamera::new(cursor_settings.clone()),mouse_captured:true,mouse_applied:None,audio_player,projection_centers,dialogue_pad:0,playtest_origin:None,playtest_draws:HashMap::new(),playback,playtest,playtest_pad:0,child_poses,draw_sources,movie_root,movie_language,movie_skip_seconds,capture_movie_frame,global_state,scene_events,startup_error_reported:false,static_collision,model_bodies,hidden_draws,collision_debug,collision_lines,vp,draws,lights,scripts,actors,capture,capture_tick,frame:0,ticks:0,capture_requested:false,last_frame:std::time::Instant::now(),accumulator:0.,report,tx}))
}
fn collision_overlay(world:Option<&super::scene_static_collision::Loaded>, bindings:Option<&super::model_body_binding::State>)->Vec<viewport3d::GpuLineSegment> {
    let Some(world)=world else{return Vec::new();};
    let mut lines=world.debug_lines();
    if let Some(bindings)=bindings {lines.extend(bindings.debug_lines(&world.dynamic_sources));}
    lines
}
/// Each authored animation component owns an independent graph/skin clock.
/// The common Character implementation is shared with EM1; this collection
/// does not invent AI decisions or gameplay animation inputs.
struct AnimatedActor {
    scene:usize,entity:u32,name:String,playback:Option<super::character_runtime::Character>,
    error:Option<String>,draws:Vec<usize>,retired:bool,
}
impl AnimatedActor {
    fn load(scene:usize,entity:u32,name:&str,components:&[epic_mickey_lib_rs::scene_file::Component],files:&[VirtualFile])->Self {
        let (playback,error)=match super::character_runtime::Character::load(components,files) {
            Ok(playback)=>(Some(playback),None),Err(error)=>(None,Some(format!("{error:#}"))),
        };
        Self {scene,entity,name:name.into(),playback,error,draws:Vec::new(),retired:false}
    }
    fn retire(&mut self,draws:&mut [GpuDrawCommand],hidden:&mut HashSet<usize>) {
        if self.retired{return;}
        self.retired=true;self.playback=None;
        for &index in &self.draws {hidden.insert(index);draws[index].release_retired_assets();}
    }
    fn failed(&mut self,error:anyhow::Error) {self.error=Some(format!("{error:#}"));self.playback=None;}
    fn bind(&mut self,index:usize,mesh:&scene::CachedNifMesh) {
        self.draws.push(index);
        if let Some(playback)=self.playback.as_mut(){if let Err(error)=playback.bind(index,mesh){self.failed(error);}}
    }
    fn apply(&mut self,draws:&mut [GpuDrawCommand]) {
        if let Some(playback)=self.playback.as_ref(){if let Err(error)=playback.apply(draws){self.failed(error);}}
    }
    fn advance(&mut self,draws:&mut [GpuDrawCommand]) {
        if let Some(playback)=self.playback.as_mut(){if let Err(error)=playback.advance(1,draws){self.failed(error);}}
    }
    fn summary(&self)->serde_json::Value {
        serde_json::json!({"scene":self.scene,"entity":self.entity,"name":self.name,"draws":self.draws.len(),
            "animation":self.playback.as_ref().map(|p|p.summary()),"error":self.error,"retired":self.retired,
            "scope":"authored graph playback; gameplay inputs and full actor lifecycle remain unconnected"})
    }
}
type NativeMaterials=HashMap<usize,(std::sync::Weak<crate::render_support::nif::MaterialBindings>,Result<Arc<super::d3d9_material::Resources>,String>)>;
struct Bootstrap {preferences:super::options_menu::Preferences,options_open:bool,options_tab:usize,preferences_dirty:bool,options_f6:bool,follow_camera:super::em2_follow_camera::FollowCamera,smooth:Option<Smooth>,perf:[f64;3],player_motion:super::physics::Motion,jump_latch:bool,look_accum:[f32;2],cursor_cam:super::em2_cursor_camera::CursorCamera,mouse_captured:bool,mouse_applied:Option<bool>,audio_player:Option<super::em2_audio_player::Player>,projection_centers:Vec<Option<[f32;3]>>,dialogue_pad:u16,playtest_origin:Option<[f32;16]>,playtest_draws:HashMap<usize,[f32;16]>,playback:Option<super::input_playback::Playback>,playtest:Option<super::controller::Controller>,playtest_pad:u16,child_poses:super::em2_child_pose::State,draw_sources:Vec<super::model_body_binding::DrawSource>,movie_root:std::path::PathBuf,movie_language:String,movie_skip_seconds:Option<f32>,capture_movie_frame:Option<u32>, global_state:super::em2_global_state::State,scene_events:Option<super::scene_event_runtime::Runtime>,startup_error_reported:bool,model_bodies:Option<super::model_body_binding::State>, hidden_draws:HashSet<usize>, collision_debug:bool,collision_lines:Vec<viewport3d::GpuLineSegment>, static_collision:Option<super::scene_static_collision::Loaded>, actors:Vec<AnimatedActor>,ticks:u64,capture_requested:bool,last_frame:std::time::Instant,accumulator:f32, scripts:super::actor_scripts::Scripts, vp:Viewport3DState, draws:Vec<GpuDrawCommand>,lights:Vec<viewport3d::GpuLight>,capture:Option<std::path::PathBuf>,capture_tick:u64,
    frame:u64, report:serde_json::Value,tx:std::sync::mpsc::Sender<Result<()>> }
impl Bootstrap {
    fn advance_playtest(&mut self,ctx:&egui::Context,first:bool)->Result<()> {
        let Some(controller)=self.playtest.as_mut() else{return Ok(())};
        let world=self.static_collision.as_ref().context("Shared playtest collision unavailable")?;
        let events=self.scene_events.as_mut().context("Shared playtest scene unavailable")?;
        let pad=super::input::poll();
        // jump presses and right-drag camera movement are collected every drawn frame (see
        // update) and handed to the first tick that follows, so none are lost between ticks
        let jump_pending=(pad.buttons&0x1000!=0&&self.playtest_pad&0x1000==0)||self.jump_latch;
        let (sensitivity,invert_y)=(self.preferences.sensitivity,self.preferences.invert_y);
        let mut input=ctx.input(|i|super::input::movement_controls(i,pad,jump_pending,sensitivity,invert_y,false));
        input.jump_pressed &= first;
        if first {input.camera_delta=std::mem::take(&mut self.look_accum);self.jump_latch=false;}
        // gamepad right stick: left/right inverted on request (matches the mouse)
        input.camera_stick[0]=-input.camera_stick[0];
        let mut explicit_use=false;
        if let Some(playback)=self.playback.as_ref(){
            let frame=playback.frames.get(controller.ticks as usize).context("EM2 playtest input recording exhausted")?;input=frame.input;explicit_use=frame.interact_pressed;
            if let Some(answer)=frame.dialogue_choice {events.answer_playtest_dialogue(answer,&mut self.global_state,&mut self.scripts)?;}
        }
        if events.playtest_dialogue_blocks_input(){input=Default::default();explicit_use=false;}
        controller.player.script_input_restricted=events.playtest_input_restricted();
        if controller.player.script_input_restricted {explicit_use=false;input.jump_pressed=false;}

        // Use precedes Jump in the authored basic ability registration. For
        // immediate markers consume the edge only after OnUse admission.
        if (input.jump_pressed||explicit_use)&&controller.player.grounded
            && events.try_playtest_use(controller.player.position,(controller.ticks+1) as f32*super::STEP)? {
            input.jump_pressed=false;input.jump_held=false;
        }
        self.playtest_pad=pad.buttons;
        let origin=*self.playtest_origin.get_or_insert_with(||events.playtest_player_matrix());
        controller.advance_with(&world.collision,super::STEP as f64,(self.capture.is_some()||ctx.input(|i|i.focused)),input,|_,_|Ok::<_,anyhow::Error>(())).map_err(|e|anyhow::anyhow!("{e}"))?;
        let position=controller.player.position;
        if controller.motion_enabled {events.publish_playtest_player_pose(position,controller.player.yaw)?;}
        let source=events.playtest_player_source();
        let actor=self.actors.iter_mut().find(|a|(a.scene,a.entity)==source).context("Shared playtest player animation actor unavailable")?;
        if controller.motion_enabled {actor.playback.as_mut().context("Shared playtest player animation failed")?.apply_playtest_motion(&controller.player)?;}
        update_playtest_draw_pose(&mut self.draws,&self.draw_sources,&mut self.playtest_draws,&origin,source,&events.playtest_player_matrix())?;
        // EM2 follow camera (framed on the ground he jumped from; pulled in by walls)
        {
            let collision=&world.collision;
            let (pitch,yaw)=(controller.camera_pitch,controller.camera_yaw);
            // same orbit as the viewport: eye = target + distance * (cos y cos p, sin p, sin y cos p)
            let dir=[yaw.cos()*pitch.cos(),pitch.sin(),yaw.sin()*pitch.cos()];
            let sweep=|start:[f32;3],delta:[f32;3]|->Option<f32> {
                let hits=collision.with_player_queries(||collision.sphere_hits(start,delta,0.1)).ok()?;
                hits.iter().map(|h|h.fraction).filter(|f|f.is_finite()&&*f>=0.).min_by(|a,b|a.total_cmp(b))
            };
            let floor=|p:[f32;3],down:f32|sweep(p,[0.,-down,0.]).map(|f|p[1]-down*f);
            let blocked=|from:[f32;3],dist:f32|sweep(from,[dir[0]*dist,dir[1]*dist,dir[2]*dist]).map(|f|f*dist);
            let half_fov=(self.vp.fov_degrees.to_radians()*0.5).max(0.1);
            let (look,dist)=self.follow_camera.update(position,controller.player.grounded,dir[1],half_fov,super::STEP,floor,blocked);
            self.vp.camera_target=look;self.vp.distance=dist;self.vp.yaw=yaw;self.vp.pitch=pitch;
        }
        self.report["shared_controller_playtest"]=serde_json::json!({"enabled":true,"position":position,"ticks":controller.ticks,"yaw":controller.player.yaw,"motion_enabled":controller.motion_enabled,"shared_animation":true,"scope":"Shared EM1 controller and Box3D; provisional motion tuning/camera and relocation reset; native EM2 proxy/contact lifecycle, abilities and full GameState lifecycle pending"});
        Ok(())
    }
    fn apply_igc_controls(&mut self)->Result<()> {
        let Some(events)=self.scene_events.as_mut() else{return Ok(())};
        let source=events.playtest_player_source();let mut controller=self.playtest.as_mut();let actors=&mut self.actors;let collision=self.static_collision.as_ref();
        let count=events.apply_playtest_igc_controls(|write|{
            let controller=controller.as_deref_mut().context("IGC control requires the admitted shared-playtest controller")?;
            match write {
                super::em2_igc_control::PlayerWrite::Motion(enabled)=>controller.motion_enabled=enabled,
                super::em2_igc_control::PlayerWrite::Pose {position,rotation,ground_snap}=>{
                    let position=if ground_snap {
                        let world=&collision.context("IGC grounded teleport requires the admitted collision world")?.collision;
                        let (start,end)=super::em2_igc_control::ground_sweep(position)?;
                        let delta=std::array::from_fn(|i|end[i]-start[i]);
                        let mut hits=world.with_player_queries(||world.sphere_hits(start,delta,0.01))?;
                        hits.sort_by(|a,b|a.fraction.total_cmp(&b.fraction));
                        let hits=hits.into_iter().map(|hit|Ok((hit.position[1],hit.normal[1],world.simulation_filter_word(hit.shape)?))).collect::<Result<Vec<_>>>()?;
                        super::em2_igc_control::grounded_position(start,hits)?
                    }else{position};
                    // Shared reset adapter, not a claim of native EM2 proxy/
                    // contact-notification parity. Position is preserved and
                    // orientation uses the reference-validated conversion.
                    controller.player.accept_script_relocation(position,rotation[2].atan2(rotation[8]));
                    return Ok(Some(position));
                },
                super::em2_igc_control::PlayerWrite::AnimationClip{resource,node}=>{
                    let actor=actors.iter_mut().find(|a|(a.scene,a.entity)==source).context("IGC clip owner has no animation actor")?;
                    actor.playback.as_mut().context("IGC clip owner has no initialized graph")?.link_pc_clip(&resource,&node)?;
                },
                super::em2_igc_control::PlayerWrite::AnimationEvent(name)=>{
                    let actor=actors.iter_mut().find(|a|(a.scene,a.entity)==source).context("IGC event owner has no animation actor")?;
                    actor.playback.as_mut().context("IGC event owner has no initialized graph")?.queue_pc_event(&name)?;
                },
                super::em2_igc_control::PlayerWrite::AnimationCulling(enabled)=>{
                    let actor=actors.iter_mut().find(|a|(a.scene,a.entity)==source).context("IGC culling owner has no animation actor")?;
                    actor.playback.as_mut().context("IGC culling owner has no initialized animation graph")?.set_animation_culling(enabled)?;
                },
            }Ok(None)
        })?;
        if count>0 {
            let origin=self.playtest_origin.as_ref().context("IGC control has no original player draw pose")?;
            update_playtest_draw_pose(&mut self.draws,&self.draw_sources,&mut self.playtest_draws,origin,source,&events.playtest_player_matrix())?;
        }Ok(())
    }
}
#[derive(Clone,Copy)]
struct Snap {pose:[f32;16],target:[f32;3],distance:f32,yaw:f32,pitch:f32}
struct Smooth {prev:Snap,cur:Snap}
fn update_playtest_draw_pose(draws:&mut [GpuDrawCommand],sources:&[super::model_body_binding::DrawSource],originals:&mut HashMap<usize,[f32;16]>,origin:&[f32;16],source:(usize,u32),pose:&[f32;16])->Result<()> {
    let delta=super::model_body_binding::delta(origin,pose)?;
    for (index,(draw,identity)) in draws.iter_mut().zip(sources).enumerate() {
        if (identity.scene,identity.entity)==source {
            let original=originals.entry(index).or_insert(draw.trans_mat);
            draw.trans_mat=super::havok_2012_geometry::compose(&delta,original);
        }
    }Ok(())
}
impl Bootstrap {
    fn on_exit(&mut self,gl:Option<&eframe::glow::Context>) {
        if let (Some(gl),Some(backend))=(gl,&self.vp.native_material) {unsafe {backend.lock().unwrap().destroy(gl);}}
    }
    fn update(&mut self, ui: &mut egui::Ui, movie_player: &mut Option<super::movie::Player>) {

        let ctx=ui.ctx().clone();
        let size=ui.available_size();
        if size.x>0.&&size.y>0. {
            if let Some(events)=self.scene_events.as_mut() {
                if let Err(error)=events.publish_camera_renderer(size.x/size.y){self.report["default_camera_error"]=serde_json::json!(format!("{error:#}"));}
            }
        }
        if ctx.input(|input|input.key_pressed(egui::Key::F3)) {
            self.collision_debug=!self.collision_debug;
            self.collision_lines=if self.collision_debug {collision_overlay(self.static_collision.as_ref(),self.model_bodies.as_ref())}else{Vec::new()};
        }
        // Blotworx F10 options menu (the same panel and preferences as the EM1 front end)
        {
            let before=serde_json::to_vec(&self.preferences).unwrap_or_default();
            let (mut open,mut tab)=(self.options_open,self.options_tab);
            let mut playtest_toggle=false;let mut respawn=false;
            let mut collision=self.collision_debug;let mut post=crate::render_support::em2_post::enabled();let mut exact=super::d3d9_draw::exact_sh();
            let playing=self.playtest.is_some();
            super::options_menu::panel(&ctx,&mut open,&mut tab,|ui,selected|match selected {
                0=>self.preferences.graphics(ui),
                1=>{let _=self.preferences.display_controls(ui,&ctx);},
                2=>{
                    if ui.button(if playing {"Stop playtest · F6"} else {"Start playtest · F6"}).clicked(){playtest_toggle=true;}
                    if ui.add_enabled(playing,egui::Button::new("Respawn")).clicked(){respawn=true;}
                },
                _=>{
                    ui.checkbox(&mut collision,"Collision overlay · F3");
                    ui.checkbox(&mut post,"EM2 bloom and colour correction");
                    ui.checkbox(&mut exact,"Game character lighting (runtime light list)");
                },
            });
            if open&&!self.options_open {self.mouse_captured=false;}
            self.options_open=open;self.options_tab=tab;
            crate::render_support::em2_post::set_enabled(post);super::d3d9_draw::set_exact_sh(exact);
            if collision!=self.collision_debug {
                self.collision_debug=collision;
                self.collision_lines=if collision {collision_overlay(self.static_collision.as_ref(),self.model_bodies.as_ref())}else{Vec::new()};
            }
            if respawn {if let Some(c)=self.playtest.as_mut(){c.respawn();}}
            self.options_f6=playtest_toggle;
            if serde_json::to_vec(&self.preferences).unwrap_or_default()!=before {self.preferences_dirty=true;}
            if self.preferences_dirty && !ctx.input(|i|i.pointer.any_down()) {
                match self.preferences.save(){Ok(())=>self.preferences_dirty=false,Err(error)=>self.report["settings_error"]=serde_json::json!(format!("{error:#}"))}
            }
            // apply the preferences the same way the EM1 front end does
            self.vp.finish=self.preferences.grade();
            self.vp.smooth_textures=self.preferences.smooth_textures;
            self.vp.surfaces=self.preferences.surface();
            self.vp.fov_degrees=self.preferences.fov;
        }
        self.report["collision"]["overlay"]=serde_json::json!({"enabled":self.collision_debug,"segments":self.collision_lines.len(),"mode":"static geometry (green), model-bound dynamic sources (magenta); excludes radius expansion"});
        if movie_player.is_none() {
            if let Some(request)=self.scene_events.as_mut().and_then(|events|events.movies.pending.take()) {
                *movie_player=Some(super::movie::Player::with_audio_selection(self.movie_root.clone(),request,self.movie_skip_seconds,
                    super::movie_tracks::Selection::PcLanguage(self.movie_language.clone())));
                self.accumulator=0.;self.last_frame=std::time::Instant::now();
                if let Some(events)=self.scene_events.as_ref(){self.report["scene_events"]["world"]=events.summary();}
            }
        }
        if let Some(movie)=movie_player.as_mut() {
            let held=ctx.input(|input|input.key_down(egui::Key::Space)) || super::input::poll().buttons&0x1000!=0;
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui,|ui| {
                if self.capture_requested {movie.show_frozen(ui);}else{movie.show(ui,held);}
            });
            let completed=movie.completed;
            let ready=movie.error.is_some() || self.capture_movie_frame.is_some_and(|frame|movie.frame.is_some_and(|current|current>=frame))
                || (completed && self.capture_movie_frame.is_some());
            self.report["movie"]=movie.summary();
            if let Some(error)=&movie.error {ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("Blotworx Engine - movie blocked: {error}")));}
            if completed && !self.capture_requested {
                if let Some(events)=self.scene_events.as_mut() {
                    events.movies.completed+=1;events.movies.last_completed=Some(movie.request.filename.clone());
                    self.report["scene_events"]["world"]=events.summary();
                }
                *movie_player=None;self.accumulator=0.;
            }
            self.last_frame=std::time::Instant::now();
            self.finish_capture(&ctx,ready,movie_player.as_ref().and_then(|movie| movie.frame));ctx.request_repaint();return;
        }
        if ctx.input(|i|i.key_pressed(egui::Key::F6))||std::mem::take(&mut self.options_f6) {
            if self.playtest.is_some(){self.playtest=None;}
            else if self.static_collision.is_some() {
                if let Some(events)=self.scene_events.as_ref(){
                    let mut c=super::controller::Controller::new(super::physics::Player::new(events.player_position(),self.player_motion.clone()));
                    let pose=events.playtest_player_matrix();c.player.yaw=pose[8].atan2(pose[10]);c.pitch_limits=self.cursor_cam.settings.pitch_limits();c.camera_yaw=self.vp.yaw;c.camera_pitch=self.vp.pitch;self.playtest=Some(c);
                }
            }
            if self.playtest.is_some(){if let Some(events)=self.scene_events.as_mut(){if let Err(error)=events.enable_playtest_dialogue(&self.movie_root,&self.movie_language){self.report["dialogue_error"]=serde_json::json!(format!("{error:#}"));self.playtest=None;}}}
            self.report["shared_controller_playtest"]=serde_json::json!({"enabled":self.playtest.is_some(),"scope":"Shared EM1 controller and Box3D; temporary EM1 motion tuning, EM2 animation/abilities/script input gates and native camera remain pending"});
        }
        if self.playtest.is_some()&&self.capture.is_none() {
            // EM2 PC mouse camera: the mouse is captured and moves a cursor inside a box in the
            // middle of the screen; movement past the box's edge turns the camera (see
            // em2_cursor_camera). While captured the OS cursor is hidden and held at the centre of
            // the window; Esc releases it, clicking the window captures it again.
            let (space,escape,click,motion,dt,half,center)=ctx.input(|i|(i.key_pressed(egui::Key::Space),i.key_pressed(egui::Key::Escape),
                i.pointer.primary_pressed(),i.pointer.motion().unwrap_or_default(),i.stable_dt,i.viewport_rect().size()*0.5,i.viewport_rect().center()));
            if space {self.jump_latch=true;}
            if escape {self.mouse_captured=false;} else if click&&!self.mouse_captured&&!self.options_open {self.mouse_captured=true;}
            if self.mouse_captured&&ctx.input(|i|i.focused) {ctx.send_viewport_cmd(egui::ViewportCommand::CursorPosition(center));}
            if self.mouse_applied!=Some(self.mouse_captured) {
                self.mouse_applied=Some(self.mouse_captured);
                ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(if self.mouse_captured {egui::CursorGrab::Confined} else {egui::CursorGrab::None}));
                ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(!self.mouse_captured));
            }
            if self.mouse_captured {
                let turn=self.cursor_cam.update([motion.x,motion.y],[half.x,half.y],dt);
                // left/right turning inverted on request
                let sens=self.preferences.sensitivity;let vertical=if self.preferences.invert_y {-1.} else {1.};
                self.look_accum[0]-=turn[0]*sens;self.look_accum[1]+=turn[1]*sens*vertical;
            }
        }
        let elapsed=self.last_frame.elapsed().as_secs_f32();self.last_frame=std::time::Instant::now();
        let ticks=if self.capture.is_some(){u64::from(!self.capture_requested)}else{self.accumulator+=elapsed;let ticks=(self.accumulator/super::STEP).floor() as u64;self.accumulator-=ticks as f32*super::STEP;ticks};
        let sim_started=std::time::Instant::now();
        self.perf[1]+=1.;self.perf[2]+=elapsed as f64*1000.;
        if ticks>0 {
            for simulation_tick in 0..ticks {
                if let Some(events)=self.scene_events.as_ref() {
                    let profiles=events.pc_render_profiles();
                    let result=profiles.lock().map_err(|_|anyhow::anyhow!("Render-profile state lock poisoned")).and_then(|mut state|state.advance_numeric());
                    if let Err(error)=result {self.report["render_profile_transition_error"]=serde_json::json!(format!("{error:#}"));}
                }
                if let Err(error)=self.advance_playtest(&ctx,simulation_tick==0){
                    log_script_error("Playtest controller error",&format!("{error:#}"));
                    self.report["shared_controller_playtest"]["error"]=serde_json::json!(format!("{error:#}"));self.playtest=None;
                }
                if let Some(events)=self.scene_events.as_mut() {
                    let position=events.player_position();
                    if let Err(error)=events.step((self.ticks+1) as f32*super::STEP,super::STEP,position,&mut self.scripts,&mut self.global_state) {log_script_error("Scene event error",&format!("{error:#}"));self.report["scene_events"]["error"]=serde_json::json!(format!("{error:#}"));}
                    let retired=events.flush_collision_retirements(|scene,entity| {
                        self.static_collision.as_mut().context("Collision retirement requires the admitted collision world")?.retire_source(scene,entity)?;Ok(())
                    });
                    match retired {
                        Ok(count) if count>0=>{
                            for (index,(draw,source)) in self.draws.iter_mut().zip(&self.draw_sources).enumerate() {
                                if events.is_retired_source(source.scene,source.entity){self.hidden_draws.insert(index);draw.release_retired_assets();}
                            }
                            if let Some(collision)=self.static_collision.as_ref(){self.report["collision"]["world"]=collision.report.clone();}
                            if self.collision_debug{self.collision_lines=collision_overlay(self.static_collision.as_ref(),self.model_bodies.as_ref());}
                        }
                        Err(error)=>{log_script_error("Entity retirement error",&format!("{error:#}"));self.report["scene_events"]["retirement_error"]=serde_json::json!(format!("{error:#}"))},
                        _=>{}
                    }
                    if let Some(error)=events.halted(){log_script_error("Script halted",error);}
                    if !self.startup_error_reported {if let Some(error)=events.halted(){ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("Blotworx Engine - startup blocked: {error}")));self.startup_error_reported=true;}}
                    self.report["scene_events"]["world"]=events.summary();
                    if let Some(list)=self.report["scene_events"]["world"]["source_errors"].as_array() {for e in list {log_script_error("Script source error",&e.to_string());}}
                }
                if let Err(error)=self.apply_igc_controls() {log_script_error("IGC control error",&format!("{error:#}"));self.report["scene_events"]["igc_control_error"]=serde_json::json!(format!("{error:#}"));}
                if let Some(events)=self.scene_events.as_mut() {
                    events.apply_animation_writes(|scene,entity,id,value| {
                        let actor=self.actors.iter_mut().find(|a|a.scene==scene&&a.entity==entity).context("Player tool owner has no animation actor")?;
                        actor.playback.as_mut().context("Player tool owner has no initialized behavior graph")?.set_pc_bool(id,value)
                    });
                    self.report["scene_events"]["world"]=events.summary();
                }
                let mut sounds=Vec::new();
                for actor in &mut self.actors {
                    if self.scene_events.as_ref().is_some_and(|events|events.is_retired_source(actor.scene,actor.entity)) {
                        actor.retire(&mut self.draws,&mut self.hidden_draws);
                    } else {
                        actor.advance(&mut self.draws);
                        // sounds attached to the animation clips (footsteps, jump, land...)
                        if let Some(playback)=actor.playback.as_ref() {
                            for name in playback.sound_triggers() {sounds.push((actor.scene,actor.entity,name));}
                            if let Some(path)=std::env::var_os("BLOTWORX_SOUND_LOG") {
                                if actor.name==self.report["player"]["entity"].as_str().unwrap_or("") {
                                    let events=playback.delivered_event_names();let triggers=playback.sound_triggers();
                                    if !events.is_empty()||!triggers.is_empty() {
                                        use std::io::Write;
                                        if let Ok(mut f)=std::fs::OpenOptions::new().create(true).append(true).open(path) {let _=writeln!(f,"{} events={:?} sounds={:?}",self.ticks,events,triggers);}
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(events)=self.scene_events.as_mut() {
                    for (scene,entity,name) in sounds {let _=events.post_actor_sound(scene,entity,&name);}
                }
                if let Some(events)=self.scene_events.as_mut() {
                    if let Err(error)=events.apply_gamebryo_models(&mut self.draws){self.report["gamebryo_model_error"]=serde_json::json!(format!("{error:#}"));}
                    let actors=&self.actors;
                    let writes=self.child_poses.update(|s,id|events.source_live(s,id),|s,id|events.source_pose(s,id),|s,id,name| {
                        actors.iter().find(|a|a.scene==s&&a.entity==id).map(|a|a.playback.as_ref().context("Attachment actor playback unavailable").and_then(|p|p.bone_pose(name))).transpose()
                    },&mut self.draws);
                    for write in &writes {events.publish_child_pose(write);}
                    self.report["child_poses"]=self.child_poses.summary();
                }
                // NiTimeSyncController receives the same raw model-update
                // time as this simulation pass, independently of render frames.
                let model_time=(self.ticks+1) as f32*super::STEP;
                for (draw,source) in self.draws.iter_mut().zip(&self.draw_sources) {
                    if let Some(clock)=draw.shader_time.as_mut() {
                        let live=self.scene_events.as_ref().is_none_or(|events|events.source_live(source.scene,source.entity));
                        if let Err(error)=clock.update(model_time,live) {self.report["shader_time_error"]=serde_json::json!(format!("{error:#}"));}
                    }
                }
                // scrolling clouds/water etc. on the game's shaders (NiTextureTransformController)
                super::em2_tex_anim::advance(model_time);
                self.report["actors"]=serde_json::json!(self.actors.iter().map(AnimatedActor::summary).collect::<Vec<_>>());
                for (name,field) in [(self.report["player"]["entity"].as_str().unwrap_or("").to_owned(),"player"),
                                     (self.report["companion"]["entity"].as_str().unwrap_or("").to_owned(),"companion")] {
                    if let Some(actor)=self.actors.iter().find(|actor|actor.name==name) {
                        self.report[field]["animation"]=actor.playback.as_ref().map(|c|c.summary()).unwrap_or(serde_json::Value::Null);
                        self.report[field]["animation_error"]=serde_json::json!(actor.error);
                    }
                }
                self.ticks+=1;
                // remember this tick's player pose and camera so drawn frames can blend between ticks
                if self.playtest.is_some() {
                    if let Some(events)=self.scene_events.as_ref() {
                        let now=Snap {pose:events.playtest_player_matrix(),target:self.vp.camera_target,distance:self.vp.distance,yaw:self.vp.yaw,pitch:self.vp.pitch};
                        self.smooth=Some(match self.smooth.take() {Some(s)=>Smooth {prev:s.cur,cur:now},None=>Smooth {prev:now,cur:now}});
                    }
                }
                if self.scene_events.as_ref().is_some_and(|events|events.movies.pending.is_some()){break;}
            }
            self.report["global_state"]=self.global_state.summary();
            self.report["scripts"]["scheduler"]=self.scripts.task_snapshot();
            self.report["scripts"]["memory"]=serde_json::json!(self.scripts.memory_summary());
            match self.scripts.queued_world_commands() {
                Ok(count)=>self.report["scripts"]["queued_world_commands"]=serde_json::json!(count),
                Err(error)=>self.report["scripts"]["queue_error"]=serde_json::json!(format!("{error:#}")),
            }
        }
        self.perf[0]+=sim_started.elapsed().as_secs_f64()*1000.;
        // Runtime SH lights for character shading (the 360's per-object light list)
        if let Some(events)=self.scene_events.as_mut() {
            let (records,error)=events.sh_light_records();
            static REPORTED:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
            if !REPORTED.swap(true,std::sync::atomic::Ordering::Relaxed) {
                eprintln!("[sh] {} runtime SH lights{}",records.len(),error.map(|e|format!("; error: {e}")).unwrap_or_default());
                if let Some(path)=std::env::var_os("BLOTWORX_LIGHT_DUMP") {events.dump_lights(std::path::Path::new(&path));}
            }
            super::d3d9_draw::set_scene_sh_lights(records);
            // Active render profile -> EM2 post-processing values (see render_support/em2_post.rs)
            let params=events.pc_render_profiles().lock().ok().and_then(|state|state.current().map(|s|{
                let f=|name:&str,default:f32|match s.get(name){Some(super::render_profile_settings::Value::Float(v))=>*v,_=>default};
                crate::render_support::em2_post::Params {
                    bloom_threshold:f("Gen3BloomThreshold",0.5),bloom_intensity:f("Gen3BloomIntensity",0.5),
                    blur_width:f("Gen3BloomBlur",2.).abs().min(1.5),
                    brightness:f("Gen3ToneBrightness",20.)/100.,contrast:1.+f("Gen3ToneContrast",15.)/100.,saturation:1.+f("Gen3ToneSaturation",5.)/100.,
                }
            }));
            crate::render_support::em2_post::set(params);
        }
        // EM2 audio: queued events/states/switches, listener at the camera, emitters at their entities
        if let Some(events)=self.scene_events.as_mut() {
            let pending=events.take_audio();
            if let Some(player)=self.audio_player.as_mut() {
                let (t,d,y,p)=(self.vp.camera_target,self.vp.distance,self.vp.yaw,self.vp.pitch);
                let eye=[t[0]+d*y.cos()*p.cos(),t[1]+d*p.sin(),t[2]+d*y.sin()*p.cos()];
                let f=[t[0]-eye[0],t[1]-eye[1],t[2]-eye[2]];
                let len=(f[0]*f[0]+f[2]*f[2]).sqrt().max(1e-6);
                // ConfigFiles.ini [Audio] Microphone_Position_Game=0.7: the listener sits 70% of the way
                // from the camera to the player
                let mickey=events.player_position();
                let ear=std::array::from_fn(|i|eye[i]+(mickey[i]-eye[i])*0.7);
                let listener=super::em2_audio_player::Listener {position:ear,right:[-f[2]/len,0.,f[0]/len]};
                let events=&*events;
                player.update(pending,&listener,|owner|events.entity_position(owner));
                if let Some(path)=std::env::var_os("BLOTWORX_AUDIO_REPORT") {if self.frame%120==0 {let _=std::fs::write(path,serde_json::to_string_pretty(&player.summary()).unwrap_or_default());}}
            }
        }
        self.vp.camera_override=None;
        // NIF hierarchy centers are in model space. Keep them tied to the
        // current rendered placement; never substitute an AABB/entity origin.
        for (draw,center) in self.draws.iter_mut().zip(&self.projection_centers) {
            draw.projection_bound=center.and_then(|center|{
                let point=super::d3d9_predefined::transform(&[center[0],center[1],center[2],1.],&draw.trans_mat);
                point.iter().all(|v|v.is_finite()).then_some([point[0],point[1],point[2]])
            });
        }
        if let Some(frame)=self.scene_events.as_ref().and_then(|events|events.playtest_camera_frame()) {
            match frame.projection() {
                Ok(projection)=>self.vp.camera_override=Some(viewport3d::CameraOverride{eye:frame.eye,view:frame.view(),projection}),
                Err(error)=>self.report["camera_error"]=serde_json::json!(format!("{error:#}")),
            }
        }
        // Uncapped presentation: the simulation stays on the game's 30 Hz step; each drawn frame
        // blends the camera and the player (with attachments) between the last two ticks.
        let mut blend:Option<([f32;16],std::collections::HashSet<usize>)>=None;
        if let (Some(s),None,Some(events))=(self.smooth.as_ref(),self.capture.as_ref(),self.scene_events.as_ref()) {
            if self.playtest.is_some()&&self.vp.camera_override.is_none() {
                let t=(self.accumulator/super::STEP).clamp(0.,1.);
                let mix=|a:f32,b:f32|a+(b-a)*t;
                let mut dyaw=s.cur.yaw-s.prev.yaw;
                while dyaw>std::f32::consts::PI {dyaw-=std::f32::consts::TAU;}
                while dyaw< -std::f32::consts::PI {dyaw+=std::f32::consts::TAU;}
                self.vp.camera_target=std::array::from_fn(|i|mix(s.prev.target[i],s.cur.target[i]));
                self.vp.distance=mix(s.prev.distance,s.cur.distance);
                self.vp.yaw=s.prev.yaw+dyaw*t;self.vp.pitch=mix(s.prev.pitch,s.cur.pitch);
                let pose:[f32;16]=std::array::from_fn(|i|mix(s.prev.pose[i],s.cur.pose[i]));
                if let Ok(d)=super::model_body_binding::delta(&s.cur.pose,&pose) {
                    let source=events.playtest_player_source();
                    let mut set:std::collections::HashSet<usize>=self.draw_sources.iter().enumerate().filter(|(_,d)|(d.scene,d.entity)==source).map(|(i,_)|i).collect();
                    set.extend(self.child_poses.attached_draws(source));
                    blend=Some((d,set));
                }
            }
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui,|ui| {
            let visible=self.draws.iter().enumerate().filter(|(index,_)|match self.scene_events.as_ref(){Some(e)=>{let s=&self.draw_sources[*index];!e.hidden_source(s.scene,s.entity)},None=>!self.hidden_draws.contains(index)}).map(|(index,draw)|{
                let mut draw=draw.clone();
                if let Some((d,set))=&blend {if set.contains(&index) {draw.trans_mat=super::havok_2012_geometry::compose(d,&draw.trans_mat);}}
                draw
            }).collect();
            viewport3d::update_and_render_viewport(ui,&mut self.vp,visible,self.collision_lines.clone(),self.lights.clone());
            // the mouse cursor (EM2 draws a 3D cursor model here; a ring stands in for it)
            if self.playtest.is_some()&&self.mouse_captured&&self.capture.is_none() {
                let r=ui.max_rect();let c=r.center();let k=self.cursor_cam.cursor;
                let p=egui::pos2(c.x+k[0]*r.width()*0.5,c.y-k[1]*r.height()*0.5);
                ui.painter().circle_stroke(p,9.,egui::Stroke::new(3.,egui::Color32::from_black_alpha(160)));
                ui.painter().circle_stroke(p,9.,egui::Stroke::new(1.5,egui::Color32::WHITE));
            }
            if let Some(events)=self.scene_events.as_mut() {
                if !self.capture_requested {
                    let delta=if self.capture.is_some(){super::STEP}else{elapsed};
                    events.advance_presentation(delta,delta);
                }
                events.hud.draw(ui);
                let buttons=super::input::poll().buttons;let edges=buttons&!self.dialogue_pad;self.dialogue_pad=buttons;
                events.draw_playtest_dialogue(ui,edges,&mut self.global_state,&mut self.scripts);
                self.report["scene_events"]["world"]=events.summary();
            }
        });
        // Authored modules and their queued commands remain owned by the
        // common script runtime until world initialization can dispatch them.
        // Scene events retain unsupported commands and stop at their first missing service.
        let _=(&self.scripts,&self.static_collision);
        // A valid queued movie owns the next presentation even when another
        // scene callback is unsupported. Do not capture/freeze before its first frame.
        let waiting_movie=self.scene_events.as_ref().is_some_and(|events|events.movies.pending.is_some());
        let ready=!waiting_movie && ((self.capture_movie_frame.is_none() && self.ticks>=self.capture_tick.max(1))
            || (self.playtest.is_none() && self.scene_events.as_ref().is_some_and(|events|events.halted().is_some())));
        self.finish_capture(&ctx,ready,movie_player.as_ref().and_then(|movie| movie.frame));ctx.request_repaint();
    }
}
impl Bootstrap {
    fn finish_capture(&mut self,ctx:&egui::Context,ready:bool,movie_frame:Option<u32>) {
        self.frame+=1;
        if self.capture.is_some() && !self.capture_requested && ready {
            if self.playtest.is_some(){
                let mut bounds=Vec::new();
                for (index,(draw,source)) in self.draws.iter().zip(&self.draw_sources).enumerate(){
                    let mut lo=[f32::INFINITY;3];let mut hi=[f32::NEG_INFINITY;3];
                    for mask in 0..8 {let p=std::array::from_fn::<_,3,_>(|i|if mask&(1<<i)==0{draw.aabb_min[i]}else{draw.aabb_max[i]});
                        for axis in 0..3 {let v=draw.trans_mat[axis]*p[0]+draw.trans_mat[4+axis]*p[1]+draw.trans_mat[8+axis]*p[2]+draw.trans_mat[12+axis];lo[axis]=lo[axis].min(v);hi[axis]=hi[axis].max(v);}
                    }
                    let span=(0..3).map(|i|(hi[i]-lo[i]).powi(2)).sum::<f32>().sqrt();
                    let distance=(0..3).map(|i|((hi[i]+lo[i])*0.5-self.vp.camera_target[i]).powi(2)).sum::<f32>().sqrt();
                    bounds.push((span/(distance+0.1),serde_json::json!({"draw":index,"scene":source.scene,"entity":source.entity,"node":source.path,"source":self.scene_events.as_ref().map(|e|e.source_description(source.scene,source.entity)),"local_min":draw.aabb_min,"local_max":draw.aabb_max,"world_min":lo,"world_max":hi,"matrix":draw.trans_mat})));
                }
                bounds.sort_by(|a,b|b.0.total_cmp(&a.0));
                self.report["capture_draw_bounds"]=serde_json::json!(bounds.into_iter().take(24).map(|(_,v)|v).collect::<Vec<_>>());
            }
            self.report["capture"]=serde_json::json!({"animation_tick":self.ticks,"fixed_delta":super::STEP,
                "movie_frame":movie_frame,"requested_movie_frame":self.capture_movie_frame,
                "actors_frozen_during_readback":true});
            self.capture_requested=true;ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        for event in ctx.input(|i|i.events.clone()) {
            if let egui::Event::Screenshot {image,..}=event {
                if let Some(path)=self.capture.take() {
                    {
                        let w=self.vp.render_work.lock().unwrap();let n=self.perf[1].max(1.);let rf=(w.frames as f64).max(1.);
                        self.report["perf"]=serde_json::json!({"frames":self.perf[1],"frame_ms_avg":self.perf[2]/n,"sim_ms_avg":self.perf[0]/n,
                            "submission_ms_avg":w.submission_total_ms/rf,"submission_ms_max":w.submission_max_ms,"gpu_ms_avg":if w.gpu_samples>0 {w.gpu_total_ms/w.gpu_samples as f64}else{-1.},
                            "native_draws":w.platform_native_draws,"fallback_draws":w.submitted_fallback_draws,"native_errors":w.platform_native_errors.values().sum::<usize>(),
                            "shadow_pass_frames":w.shadow_pass_frames,"shadow_casters_per_frame":w.shadow_caster_draws as f64/rf,"rejected_main_draws":w.rejected_main_draws});
                    }
                    if self.vp.pixel_probe.is_some() {
                        let mut work=serde_json::to_value(&*self.vp.render_work.lock().unwrap()).unwrap_or_default();
                        let visible:Vec<_>=self.draw_sources.iter().enumerate().filter(|(index,s)|match self.scene_events.as_ref(){Some(e)=>!e.hidden_source(s.scene,s.entity),None=>!self.hidden_draws.contains(index)}).collect();
                        if let Some(history)=work["pixel_history"].as_array_mut() {for entry in history {
                            if let Some((index,source))=entry["draw"].as_u64().and_then(|i|visible.get(i as usize)) {
                                entry["source_draw"]=serde_json::json!(index);
                                entry["source"]=serde_json::json!({"scene":source.scene,"entity":source.entity,"node":source.path});
                            }
                        }}
                        self.report["scene_render_work"]=work;
                    }
                    let result=(||->Result<()> {
                        // The game window is opaque. Framebuffer alpha carries
                        // material data, not screenshot coverage; exporting it
                        // makes solid meshes transparent in image viewers.
                        let rgb:Vec<u8>=image.pixels.iter().flat_map(|p|[p.r(),p.g(),p.b()]).collect();
                        image::save_buffer(&path,&rgb,image.size[0] as u32,image.size[1] as u32,image::ColorType::Rgb8)?;
                        std::fs::write(path.with_extension("runtime.json"),serde_json::to_vec_pretty(&self.report)?)?;Ok(())
                    })();
                    let _=self.tx.send(result);ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        ctx.request_repaint();
    }
}

/// Script/scene errors go to the log (file + console), each distinct message once.
fn log_script_error(kind:&str,message:&str) {
    static SEEN:std::sync::Mutex<Option<HashSet<String>>>=std::sync::Mutex::new(None);
    let key=format!("{kind}: {message}");
    if let Ok(mut seen)=SEEN.lock() {if seen.get_or_insert_with(Default::default).insert(key.clone()) {log::error!("{key}");}}
}

/// BLOTWORX_EM2_LIT_ALPHA=1: the .lit stream's alpha replaces the mesh's vertex alpha (experiment for the
/// Gen3 inert/toon blend shaders; off by default, no visible effect seen in Mean Street South).
fn lit_alpha() -> bool { static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new(); *ON.get_or_init(|| std::env::var("BLOTWORX_EM2_LIT_ALPHA").is_ok_and(|v| v == "1")) }
