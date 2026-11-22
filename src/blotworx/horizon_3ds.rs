//! Native Nintendo 3DS (`target_os = "horizon"`) platform driver for the real Blotworx Engine.
//! Runs the full Rust `World`, Lua 5.1 scripting, Havok/Box3D physics, skeletal animation,
//! Paint/Thinner simulation, quests, dialogues, and level transitions directly on 3DS,
//! rendering `World::draws` on the PICA200 GPU via `citro3d` (`3ds/c3d_bridge.c`).

use super::{
    ability_value, assets, authored_preview, brush, brush_preview, brush_runtime, checkpoint,
    checkpoint_time, clip_preview, controller, cursor, ears, frame_clock, health, level_session,
    native_angle, physics, render_options, save_service, score_state, skin, STEP,
};
use crate::render_support::viewport3d::{self, GpuDrawCommand};
use anyhow::{Context, Result};
use std::{
    collections::{HashMap, HashSet},
    ffi::{c_char, c_void, CString},
    path::{Path, PathBuf},
    sync::Arc,
};

// FileDialog stub (matches src/android_app.rs)
pub(crate) struct FileDialog;
impl FileDialog {
    pub fn new() -> Self {
        Self
    }
    pub fn add_filter(self, _: &str, _: &[&str]) -> Self {
        self
    }
    pub fn set_directory(self, _: impl AsRef<Path>) -> Self {
        self
    }
    pub fn set_file_name(self, _: &str) -> Self {
        self
    }
    pub fn pick_file(self) -> Option<PathBuf> {
        None
    }
    pub fn pick_folder(self) -> Option<PathBuf> {
        None
    }
    pub fn save_file(self) -> Option<PathBuf> {
        None
    }
}

// libctru key bitmasks (3ds/services/hid.h)
const KEY_A: u32 = 1 << 0;
const KEY_B: u32 = 1 << 1;
const KEY_SELECT: u32 = 1 << 2;
const KEY_START: u32 = 1 << 3;
const KEY_DRIGHT: u32 = 1 << 4;
const KEY_DLEFT: u32 = 1 << 5;
const KEY_DUP: u32 = 1 << 6;
const KEY_DDOWN: u32 = 1 << 7;
const KEY_R: u32 = 1 << 8;
const KEY_L: u32 = 1 << 9;
const KEY_X: u32 = 1 << 10;
const KEY_Y: u32 = 1 << 11;
const KEY_ZL: u32 = 1 << 14;
const KEY_ZR: u32 = 1 << 15;

const SYSCLOCK_ARM11: f64 = 268_111_856.0;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Bw3dsInput {
    k_down: u32,
    k_held: u32,
    k_up: u32,
    cpad_x: i16,
    cpad_y: i16,
    cstick_x: i16,
    cstick_y: i16,
    touch_x: u16,
    touch_y: u16,
    touching: u8,
    _pad: [u8; 3],
    slider_3d: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Bw3dsVertex {
    pos: [f32; 3],
    uv: [f32; 2],
    rgba: [u8; 4],
    nrm: [f32; 3],
}

extern "C" {
    fn bw3ds_init();
    fn bw3ds_fini();
    fn bw3ds_apt_main_loop() -> u8;
    fn bw3ds_system_tick() -> u64;
    fn bw3ds_poll_input(out: *mut Bw3dsInput);
    fn bw3ds_ram_free(app_free_mb: *mut u32, lin_free_mb: *mut u32);
    fn bw3ds_debug_log(text: *const c_char);
    fn bw3ds_console_clear();
    fn bw3ds_console_line(row: i32, text: *const c_char);
    fn bw3ds_console_present_loading();
    fn bw3ds_tex_create_rgba8(rgba: *const u8, width: u32, height: u32, target_dim: u32) -> u32;
    fn bw3ds_tex_free_all();
    fn bw3ds_linear_alloc(bytes: u32) -> *mut c_void;
    fn bw3ds_linear_free(ptr: *mut c_void);
    fn bw3ds_flush_cache(ptr: *const c_void, bytes: u32);
    fn bw3ds_frame_sync();
    fn bw3ds_frame_begin();
    fn bw3ds_set_blend_mode(transparent: i32);
    fn bw3ds_begin_eye(
        eye_idx: i32,
        clear_rgba: u32,
        fov_deg: f32,
        near_z: f32,
        far_z: f32,
        iod: f32,
        view_col_major_16: *const f32,
    );
    fn bw3ds_draw_indexed(
        vbo_linear: *const c_void,
        ibo_linear: *const u16,
        index_count: u32,
        tex_handle: u32,
        model_col_major_16: *const f32,
        tint_rgba_4: *const f32,
    );
    fn bw3ds_frame_end();
}

fn console_line(row: i32, text: &str) {
    let clean: String = text
        .chars()
        .map(|c| if c.is_ascii_graphic() || c == ' ' { c } else { ' ' })
        .take(39)
        .collect();
    if let Ok(c) = CString::new(clean) {
        unsafe { bw3ds_console_line(row, c.as_ptr()) };
    }
}

struct CachedMesh {
    vbo: *mut Bw3dsVertex,
    ibo: *mut u16,
    v_ptr: usize,
    v_len: usize,
    i_ptr: usize,
    index_count: u32,
    center: [f32; 3],
    radius: f32,
}

impl Drop for CachedMesh {
    fn drop(&mut self) {
        unsafe {
            if !self.vbo.is_null() {
                bw3ds_linear_free(self.vbo.cast());
            }
            if !self.ibo.is_null() {
                bw3ds_linear_free(self.ibo.cast());
            }
        }
    }
}

struct PicaCache {
    textures: HashMap<usize, u32>,
    meshes: HashMap<usize, CachedMesh>,
    dyn_vbo: *mut Bw3dsVertex,
    dyn_vbo_cap: usize,
    dyn_ibo: *mut u16,
    dyn_ibo_cap: usize,
}

impl PicaCache {
    fn new() -> Self {
        let dyn_vbo_cap = 16_384;
        let dyn_ibo_cap = 32_768;
        let dyn_vbo = unsafe {
            bw3ds_linear_alloc((dyn_vbo_cap * std::mem::size_of::<Bw3dsVertex>()) as u32)
                .cast::<Bw3dsVertex>()
        };
        let dyn_ibo = unsafe {
            bw3ds_linear_alloc((dyn_ibo_cap * std::mem::size_of::<u16>()) as u32).cast::<u16>()
        };
        Self {
            textures: HashMap::new(),
            meshes: HashMap::new(),
            dyn_vbo,
            dyn_vbo_cap,
            dyn_ibo,
            dyn_ibo_cap,
        }
    }

    fn clear_level(&mut self) {
        unsafe { bw3ds_frame_sync() };
        self.meshes.clear();
        self.textures.clear();
        unsafe { bw3ds_tex_free_all() };
    }

    fn texture_for_draw(&mut self, draw: &GpuDrawCommand, is_character: bool) -> u32 {
        let best = draw
            .tex_bytes
            .as_ref()
            .filter(|(rgba, w, h)| !rgba.is_empty() && *w > 0 && *h > 0)
            .map(|(rgba, w, h)| (rgba, *w, *h))
            .or_else(|| {
                draw.material_images
                    .iter()
                    .find(|img| !img.rgba.is_empty() && img.width > 0 && img.height > 0)
                    .map(|img| (&img.rgba, img.width, img.height))
            });
        let Some((rgba, w, h)) = best else {
            return 0;
        };
        let key = Arc::as_ptr(rgba) as usize;
        if let Some(&hnd) = self.textures.get(&key) {
            return hnd;
        }
        let target_dim = if is_character { 64 } else { 64 };
        let hnd = unsafe { bw3ds_tex_create_rgba8(rgba.as_ptr(), w, h, target_dim) };
        self.textures.insert(key, hnd);
        hnd
    }

    fn pack_vertices(v_data: &[f32], dst: *mut Bw3dsVertex, v_count: usize) -> ([f32; 3], f32) {
        let mut min_p = [f32::INFINITY; 3];
        let mut max_p = [f32::NEG_INFINITY; 3];
        for i in 0..v_count {
            let b = i * 12;
            let px = v_data[b];
            let py = v_data[b + 1];
            let pz = v_data[b + 2];
            min_p[0] = min_p[0].min(px);
            min_p[1] = min_p[1].min(py);
            min_p[2] = min_p[2].min(pz);
            max_p[0] = max_p[0].max(px);
            max_p[1] = max_p[1].max(py);
            max_p[2] = max_p[2].max(pz);
            let r = (v_data[b + 6].clamp(0.0, 1.0) * 255.0) as u8;
            let g = (v_data[b + 7].clamp(0.0, 1.0) * 255.0) as u8;
            let bl = (v_data[b + 8].clamp(0.0, 1.0) * 255.0) as u8;
            // Wii TEV vertex alpha channels often store blend/AO weights (0.0..0.5),
            // not screen opacity; keep vertex alpha opaque for PICA200 fixed-function blending.
            let a = 255u8;
            let vert = Bw3dsVertex {
                pos: [px, py, pz],
                uv: [v_data[b + 10], 1.0 - v_data[b + 11]],
                rgba: [r, g, bl, a],
                nrm: [v_data[b + 3], v_data[b + 4], v_data[b + 5]],
            };
            unsafe { dst.add(i).write(vert) };
        }
        let center = [
            0.5 * (min_p[0] + max_p[0]),
            0.5 * (min_p[1] + max_p[1]),
            0.5 * (min_p[2] + max_p[2]),
        ];
        let dx = max_p[0] - center[0];
        let dy = max_p[1] - center[1];
        let dz = max_p[2] - center[2];
        (center, (dx * dx + dy * dy + dz * dz).sqrt())
    }

    fn draw_command(
        &mut self,
        draw_idx: usize,
        draw: &GpuDrawCommand,
        is_dynamic: bool,
        is_character: bool,
        eye: [f32; 3],
        tint: [f32; 4],
    ) {
        let v_count = draw.v_data.len() / 12;
        if v_count == 0 || v_count > 65_000 || draw.indices.len() < 3 {
            return;
        }
        let tex = self.texture_for_draw(draw, is_character);

        if is_dynamic {
            if self.dyn_vbo.is_null()
                || self.dyn_ibo.is_null()
                || v_count > self.dyn_vbo_cap
                || draw.indices.len() > self.dyn_ibo_cap
            {
                return;
            }
            Self::pack_vertices(&draw.v_data, self.dyn_vbo, v_count);
            let mut valid_idx = 0usize;
            for tri in draw.indices.chunks_exact(3) {
                if (tri[0] as usize) < v_count
                    && (tri[1] as usize) < v_count
                    && (tri[2] as usize) < v_count
                {
                    unsafe {
                        self.dyn_ibo.add(valid_idx).write(tri[0] as u16);
                        self.dyn_ibo.add(valid_idx + 1).write(tri[1] as u16);
                        self.dyn_ibo.add(valid_idx + 2).write(tri[2] as u16);
                    }
                    valid_idx += 3;
                }
            }
            if valid_idx < 3 {
                return;
            }
            unsafe {
                bw3ds_flush_cache(
                    self.dyn_vbo.cast(),
                    (v_count * std::mem::size_of::<Bw3dsVertex>()) as u32,
                );
                bw3ds_flush_cache(
                    self.dyn_ibo.cast(),
                    (valid_idx * std::mem::size_of::<u16>()) as u32,
                );
                bw3ds_draw_indexed(
                    self.dyn_vbo.cast(),
                    self.dyn_ibo,
                    valid_idx as u32,
                    tex,
                    draw.trans_mat.as_ptr(),
                    tint.as_ptr(),
                );
            }
            return;
        }

        let v_ptr = Arc::as_ptr(&draw.v_data) as usize;
        let i_ptr = Arc::as_ptr(&draw.indices) as usize;
        let needs_upload = match self.meshes.get(&draw_idx) {
            Some(m) => m.v_ptr != v_ptr || m.v_len != draw.v_data.len() || m.i_ptr != i_ptr,
            None => true,
        };
        if needs_upload {
            self.meshes.remove(&draw_idx);
            let vbo_bytes = (v_count * std::mem::size_of::<Bw3dsVertex>()) as u32;
            let ibo_bytes = (draw.indices.len() * std::mem::size_of::<u16>()) as u32;
            let vbo = unsafe { bw3ds_linear_alloc(vbo_bytes).cast::<Bw3dsVertex>() };
            let ibo = unsafe { bw3ds_linear_alloc(ibo_bytes).cast::<u16>() };
            if vbo.is_null() || ibo.is_null() {
                unsafe {
                    if !vbo.is_null() {
                        bw3ds_linear_free(vbo.cast());
                    }
                    if !ibo.is_null() {
                        bw3ds_linear_free(ibo.cast());
                    }
                }
                return;
            }
            let (center, radius) = Self::pack_vertices(&draw.v_data, vbo, v_count);
            let mut valid_idx = 0usize;
            for tri in draw.indices.chunks_exact(3) {
                if (tri[0] as usize) < v_count
                    && (tri[1] as usize) < v_count
                    && (tri[2] as usize) < v_count
                {
                    unsafe {
                        ibo.add(valid_idx).write(tri[0] as u16);
                        ibo.add(valid_idx + 1).write(tri[1] as u16);
                        ibo.add(valid_idx + 2).write(tri[2] as u16);
                    }
                    valid_idx += 3;
                }
            }
            unsafe {
                bw3ds_flush_cache(vbo.cast(), vbo_bytes);
                bw3ds_flush_cache(ibo.cast(), (valid_idx * std::mem::size_of::<u16>()) as u32);
            }
            self.meshes.insert(
                draw_idx,
                CachedMesh {
                    vbo,
                    ibo,
                    v_ptr,
                    v_len: draw.v_data.len(),
                    i_ptr,
                    index_count: valid_idx as u32,
                    center,
                    radius,
                },
            );
        }

        if let Some(m) = self.meshes.get(&draw_idx) {
            if !draw.skybox {
                let wx = draw.trans_mat[0] * m.center[0]
                    + draw.trans_mat[4] * m.center[1]
                    + draw.trans_mat[8] * m.center[2]
                    + draw.trans_mat[12];
                let wy = draw.trans_mat[1] * m.center[0]
                    + draw.trans_mat[5] * m.center[1]
                    + draw.trans_mat[9] * m.center[2]
                    + draw.trans_mat[13];
                let wz = draw.trans_mat[2] * m.center[0]
                    + draw.trans_mat[6] * m.center[1]
                    + draw.trans_mat[10] * m.center[2]
                    + draw.trans_mat[14];
                let dx = wx - eye[0];
                let dy = wy - eye[1];
                let dz = wz - eye[2];
                let dist_sq = dx * dx + dy * dy + dz * dz;
                let max_d = m.radius + 115.0;
                if dist_sq > max_d * max_d {
                    return;
                }
            }
            unsafe {
                bw3ds_draw_indexed(
                    m.vbo.cast(),
                    m.ibo,
                    m.index_count,
                    tex,
                    draw.trans_mat.as_ptr(),
                    tint.as_ptr(),
                );
            }
        }
    }
}

impl Drop for PicaCache {
    fn drop(&mut self) {
        self.clear_level();
        unsafe {
            if !self.dyn_vbo.is_null() {
                bw3ds_linear_free(self.dyn_vbo.cast());
            }
            if !self.dyn_ibo.is_null() {
                bw3ds_linear_free(self.dyn_ibo.cast());
            }
        }
    }
}

struct LiveSession {
    world: assets::World,
    controller: controller::Controller,
    authored_preview: authored_preview::Preview,
    ear_pose: ears::Pose,
    player_skin_frame: clip_preview::SkinFrame,
    hose_active: bool,
    hose_phase: Option<brush::Firing>,
    hose_clock: brush::EmissionClock,
    hose_emitters: [bool; 2],
    brush_action_respawns: u32,
    ink_hold: Option<(usize, f32)>,
    blast_cooldown: [f32; 2],
    spin_pending: bool,
    camera_yaw: f32,
    camera_pitch: f32,
    zoom: f32,
    aim_norm: [f32; 2],
}

fn load_level_session(
    data_root: &Path,
    level: &str,
    egui_ctx: &egui::Context,
    prev_scores: Option<score_state::InitialState>,
) -> Result<LiveSession> {
    unsafe { bw3ds_console_clear() };
    console_line(1, "== BLOTWORX ENGINE 3DS (NATIVE RUST) ==");
    console_line(3, &format!("Loading: {level}"));
    console_line(5, "Reading Wii packfiles, NIF, HKX, Lua...");
    let mut app_mb = 0u32;
    let mut lin_mb = 0u32;
    unsafe { bw3ds_ram_free(&mut app_mb, &mut lin_mb) };
    console_line(7, &format!("RAM Free: App {app_mb}MB | Lin {lin_mb}MB"));
    unsafe { bw3ds_console_present_loading() };

    let scores = prev_scores.unwrap_or_else(|| {
        let frame_clock = frame_clock::Context::boot(None);
        let mut playtime = checkpoint_time::Playtime::default();
        playtime.set(0, &frame_clock.clock);
        let service = save_service::default_directory()
            .and_then(|dir| save_service::Service::open(dir, 0, frame_clock.hardware_sample()))
            .ok();
        score_state::InitialState {
            playtime,
            frame_clock: Some(frame_clock),
            checkpoint: checkpoint::State {
                service,
                ..Default::default()
            },
            ..Default::default()
        }
    });

    let mut world = assets::load_with_bootstrap_mode(
        data_root,
        level,
        egui_ctx,
        None,
        None,
        Default::default(),
        render_options::State::default(),
        scores,
        true,
        false,
    )?;
    if !world.demo_story.enabled {
        let _ = world.enable_demo_story();
    }
    world.recover_failed_preview_script(0);

    let ear_pose = ears::Pose::new(&world.player_animations.rig.bones)?;
    let authored_preview = authored_preview::Preview::new(&mut world)?;
    world.attach_player_body()?;
    let controller = controller::Controller::new(world.take_startup_player());

    Ok(LiveSession {
        world,
        controller,
        authored_preview,
        ear_pose,
        player_skin_frame: Default::default(),
        hose_active: false,
        hose_phase: None,
        hose_clock: Default::default(),
        hose_emitters: [false; 2],
        brush_action_respawns: 0,
        ink_hold: None,
        blast_cooldown: [0.0; 2],
        spin_pending: false,
        camera_yaw: 1.6,
        camera_pitch: 0.28,
        zoom: 5.0,
        aim_norm: [0.5, 0.5],
    })
}

const QUICK_LEVELS: &[&str] = &[
    "Levels/GV_ZoneA_Start.level",
    "Levels/GV_ZoneB_Start.level",
    "Levels/GV_ZoneC_Start.level",
    "Levels/MeanStreet_V3.Level",
    "Levels/TL_SpaceMountain_TronPete.Level",
    "Levels/PotW_JollyRoger_Start.Level",
    "Levels/HM_StartM.Level",
    "Levels/ShadowBlot_Boss.level",
    "Levels/DBC_Start.Level",
];

fn debug_log(msg: &str) {
    if let Ok(c) = CString::new(msg) {
        unsafe { bw3ds_debug_log(c.as_ptr()) };
    }
}

pub(crate) fn debug_stage(stage: &str) {
    let mut app_mb = 0u32;
    let mut lin_mb = 0u32;
    unsafe { bw3ds_ram_free(&mut app_mb, &mut lin_mb) };
    let msg = format!("[3DS] {stage} (app={app_mb}MB lin={lin_mb}MB)");
    debug_log(&msg);
    console_line(9, stage);
    console_line(11, &format!("RAM Free: App {app_mb}MB | Lin {lin_mb}MB"));
    unsafe { bw3ds_console_present_loading() };
}

pub fn run() -> Result<()> {
    unsafe { bw3ds_init() };
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("PANIC: {info}");
        debug_log(&msg);
        unsafe { bw3ds_console_clear() };
        console_line(1, "== BLOTWORX ENGINE 3DS PANIC ==");
        for (i, chunk) in msg.as_bytes().chunks(38).take(18).enumerate() {
            console_line(3 + i as i32, &String::from_utf8_lossy(chunk));
        }
        unsafe { bw3ds_console_present_loading() };
        loop {
            unsafe { bw3ds_console_present_loading() };
        }
    }));
    unsafe { bw3ds_console_clear() };
    console_line(1, "== BLOTWORX ENGINE 3DS (NATIVE RUST) ==");
    console_line(3, "Booting Blotworx Engine...");
    debug_log("[3DS] Booting Blotworx Engine...");
    unsafe { bw3ds_console_present_loading() };

    let result = run_inner();
    if let Err(err) = &result {
        unsafe { bw3ds_console_clear() };
        console_line(1, "== BLOTWORX ENGINE 3DS ERROR ==");
        let msg = format!("{err:#}");
        for (i, chunk) in msg.as_bytes().chunks(38).take(16).enumerate() {
            console_line(3 + i as i32, &String::from_utf8_lossy(chunk));
        }
        console_line(22, "Press START to exit.");
        while unsafe { bw3ds_apt_main_loop() } != 0 {
            let mut inp = Bw3dsInput::default();
            unsafe {
                bw3ds_poll_input(&mut inp);
                bw3ds_console_present_loading();
            }
            if (inp.k_down & KEY_START) != 0 {
                break;
            }
        }
    }

    unsafe { bw3ds_fini() };
    result
}

fn run_inner() -> Result<()> {
    let egui_ctx = egui::Context::default();
    let data_root = assets::default_root().context(
        "No Epic Mickey DATA/files/packfiles folder found on SD card! Copy your extracted Wii DATA/ folder to sdmc:/blotworx/DATA/",
    )?;

    let mut pica = PicaCache::new();
    let mut session = load_level_session(&data_root, QUICK_LEVELS[0], &egui_ctx, None)?;
    debug_stage(&format!(
        "LIVE! Draws={} Entities={}",
        session.world.draws.len(),
        session.world.entities.len()
    ));
    unsafe { bw3ds_console_clear() };
    let mut show_level_menu = false;
    let mut menu_cursor = 0usize;
    let mut last_tick = unsafe { bw3ds_system_tick() };
    let mut hud_counter = 0u32;

    while unsafe { bw3ds_apt_main_loop() } != 0 {
        let mut inp = Bw3dsInput::default();
        unsafe { bw3ds_poll_input(&mut inp) };
        if (inp.k_down & KEY_START) != 0 {
            break;
        }

        let now_tick = unsafe { bw3ds_system_tick() };
        let mut elapsed = (now_tick.wrapping_sub(last_tick) as f64) / SYSCLOCK_ARM11;
        last_tick = now_tick;
        if !(elapsed.is_finite() && elapsed > 0.0 && elapsed <= 0.15) {
            elapsed = f64::from(STEP);
        }

        if (inp.k_down & KEY_SELECT) != 0 {
            show_level_menu = !show_level_menu;
        }

        if show_level_menu {
            if (inp.k_down & KEY_DUP) != 0 && menu_cursor > 0 {
                menu_cursor -= 1;
            }
            if (inp.k_down & KEY_DDOWN) != 0 && menu_cursor + 1 < QUICK_LEVELS.len() {
                menu_cursor += 1;
            }
            if (inp.k_down & KEY_A) != 0 {
                show_level_menu = false;
                let target_level = QUICK_LEVELS[menu_cursor].to_string();
                let scores = score_state::InitialState {
                    playtime: session.world.playtime,
                    frame_clock: Some(session.world.frame_clock),
                    checkpoint: session.world.checkpoint.clone(),
                    ..Default::default()
                };
                drop(session);
                pica.clear_level();
                session = load_level_session(&data_root, &target_level, &egui_ctx, Some(scores))?;
                last_tick = unsafe { bw3ds_system_tick() };
                continue;
            }
        }

        // Check if Lua scripts / projector triggers requested a real level transition!
        if let Some((next_level, _)) = session.world.level_transition.take() {
            let transferred = level_session::Session::capture(&mut session.world).ok();
            let scores = score_state::InitialState {
                session: transferred,
                resume: None,
                playstyle_points: session.world.playstyle_points,
                playthrough: session.world.playthrough,
                playtime: session.world.playtime,
                checkpoint: session.world.checkpoint.clone(),
                frame_clock: Some(session.world.frame_clock),
            };
            drop(session);
            pica.clear_level();
            session = load_level_session(&data_root, &next_level, &egui_ctx, Some(scores))?;
            last_tick = unsafe { bw3ds_system_tick() };
            continue;
        }

        // Map 3DS Circle Pad, C-Stick, D-Pad, Buttons & Touch Screen -> Real Engine Input
        let deadzone = |v: i16| -> f32 {
            let f = (v as f32 / 156.0).clamp(-1.0, 1.0);
            if f.abs() < 0.14 { 0.0 } else { f }
        };
        let move_x = if show_level_menu { 0.0 } else { deadzone(inp.cpad_x) };
        let move_y = if show_level_menu { 0.0 } else { deadzone(inp.cpad_y) };
        let mut cam_x = deadzone(inp.cstick_x);
        let mut cam_y = deadzone(inp.cstick_y);
        if !show_level_menu {
            if (inp.k_held & KEY_DLEFT) != 0 { cam_x -= 0.85; }
            if (inp.k_held & KEY_DRIGHT) != 0 { cam_x += 0.85; }
            if (inp.k_held & KEY_DUP) != 0 { cam_y += 0.65; }
            if (inp.k_held & KEY_DDOWN) != 0 { cam_y -= 0.65; }
        }
        if inp.touching != 0 && !show_level_menu {
            session.aim_norm = [
                (inp.touch_x as f32 / 320.0).clamp(0.05, 0.95),
                (inp.touch_y as f32 / 240.0).clamp(0.05, 0.95),
            ];
        }
        if (inp.k_down & KEY_Y) != 0 && !show_level_menu {
            session.spin_pending = true;
        }

        let raw_requested_hose = if show_level_menu {
            None
        } else if (inp.k_held & (KEY_R | KEY_ZR)) != 0 {
            Some(0usize)
        } else if (inp.k_held & (KEY_L | KEY_ZL)) != 0 {
            Some(1usize)
        } else {
            None
        };

        let ctrl_input = controller::Input {
            movement: [move_x, move_y],
            camera_stick: [cam_x, cam_y],
            camera_delta: [0.0, 0.0],
            jump_pressed: !show_level_menu && (inp.k_down & KEY_B) != 0,
            jump_held: !show_level_menu && (inp.k_held & KEY_B) != 0,
        };

        let LiveSession {
            world,
            controller,
            authored_preview,
            ear_pose,
            player_skin_frame,
            hose_active,
            hose_phase,
            hose_clock,
            hose_emitters,
            brush_action_respawns,
            ink_hold,
            blast_cooldown,
            spin_pending,
            camera_yaw,
            camera_pitch,
            zoom,
            aim_norm,
        } = &mut session;

        let active = !show_level_menu;
        let aim_size = [400u32, 240u32];
        let aim_pixel = [(aim_norm[0] * 400.0) as i32, (aim_norm[1] * 240.0) as i32];
        let mut requested_hose: Option<usize> = None;

        let _ = controller.advance_with_tick_phases_and_camera(
            world,
            |w| &w.collision,
            elapsed,
            active,
            ctrl_input,
            |w| w.published_camera_frame().map(|f| f.map(|f| f.direction)),
            |phase, player, tick, c_yaw, c_pitch, world| -> Result<()> {
                if phase == controller::TickPhase::SimulatePhysics {
                    world.advance_player_physics(player, STEP)?;
                    return Ok(());
                }
                if phase == controller::TickPhase::BeforeMovement {
                    world.with_live_player(player, |w| w.prepare_gameplay_scene_requests())?;
                    world.advance_demo_motion(STEP)?;
                    world.with_live_player(player, |w| w.advance_demo_ai(STEP))?;
                    let trig_pos = player.position;
                    world.with_live_player(player, |w| w.advance_demo_triggers(trig_pos, STEP))?;
                    world.advance_demo_counter_states()?;
                    if let Some(spawn) = world.player_spawn.override_transform {
                        player.spawn = spawn.position;
                    }
                    let dead = world
                        .runtime_health
                        .get(&world.player_entity)
                        .is_some_and(|s| s.health <= 0);
                    let authored_death = dead && world.player_checkpoint_death_available();
                    if dead && !authored_death {
                        player.respawn();
                        if let Some(authored) = world.authored_health.get(&world.player_entity) {
                            let restored = health::Runtime::new(authored);
                            world.runtime_health.insert(world.player_entity, restored);
                        }
                    }
                    world.prepare_side_scroller(player);
                    player.script_input_restricted = authored_death
                        || world.cutscene_input.restricted
                        || world
                            .player_physics
                            .as_ref()
                            .is_some_and(|b| b.suspended_motion.is_some());
                    if player.script_input_restricted {
                        *spin_pending = false;
                    }
                    world.advance_pressure_plates(player, STEP)?;
                    world.prepare_scene_motion(STEP)?;

                    for cd in blast_cooldown.iter_mut() {
                        if *cd > 0.0 {
                            *cd = (*cd - STEP).max(0.0);
                        }
                    }
                    let allowed_ink = raw_requested_hose.filter(|_| {
                        !player.script_input_restricted
                            && !world.side_scroll_disables("SideScrollDisablePaint")
                    });
                    let mut tap_ink: Option<usize> = None;
                    requested_hose = match allowed_ink {
                        Some(ink) => {
                            let channel = if ink == 0 { brush::Ink::Paint } else { brush::Ink::Thinner };
                            let held_t = match *ink_hold {
                                Some((prev, t)) if prev == ink => {
                                    let next = t + STEP;
                                    *ink_hold = Some((ink, next));
                                    next
                                }
                                _ => {
                                    *ink_hold = Some((ink, 0.0));
                                    0.0
                                }
                            };
                            if held_t >= 0.16 && world.player_ability_enabled("Brush", channel)? {
                                Some(ink)
                            } else {
                                None
                            }
                        }
                        None => {
                            if let Some((ink, held_t)) = ink_hold.take() {
                                if held_t <= 0.25 && !*hose_active {
                                    tap_ink = Some(ink);
                                }
                            }
                            None
                        }
                    };
                    if let Some(ink) = tap_ink {
                        let channel = if ink == 0 { brush::Ink::Paint } else { brush::Ink::Thinner };
                        if blast_cooldown[ink] <= 0.0
                            && world.player_ability_enabled("Brush", channel).unwrap_or(false)
                        {
                            let cost = world
                                .report
                                .brush_settings
                                .as_ref()
                                .and_then(|s| {
                                    s.value(channel, "BlastCost", 0, ability_value::NEUTRAL_PLAYSTYLE).ok()
                                })
                                .unwrap_or(1.5);
                            if world.ammo.available(ink) >= cost {
                                let _ = world.ammo.change(ink, -cost, &mut |_, _, _| Ok(()));
                                blast_cooldown[ink] = 0.55;
                                let (sy, cy) = player.yaw.sin_cos();
                                let fwd = [sy, 0.0, cy];
                                let origin = [
                                    player.position[0] + fwd[0] * 0.65,
                                    player.position[1] + 0.85,
                                    player.position[2] + fwd[2] * 0.65,
                                ];
                                let _ = world.dispatch_blast_cone(origin, fwd, channel);
                                let _ = authored_preview
                                    .queue_named_event(&world.player_behavior, "EVENT_Start_Blast");
                            }
                        }
                    }

                    if *brush_action_respawns != player.respawns {
                        authored_preview.synchronize_player(
                            player,
                            &world.player_behavior,
                            &mut world.player_graph,
                            &mut world.player_animations,
                        )?;
                        player.end_brush_motion();
                        if let Some(previous) = hose_phase.take() {
                            *hose_clock = previous.stop_effects(true, |ink| {
                                hose_emitters[usize::from(ink == brush::Ink::Thinner)] = false;
                                Ok(())
                            })?;
                        }
                        *hose_emitters = [false; 2];
                        *hose_active = false;
                        *brush_action_respawns = player.respawns;
                    }
                    if requested_hose.is_some() != *hose_active {
                        authored_preview.brush_transition(
                            requested_hose.is_some(),
                            &world.player_behavior,
                            &mut world.player_graph,
                            &mut world.player_animations,
                        )?;
                        if requested_hose.is_some() {
                            if let Some(settings) = &world.report.brush_settings {
                                player.begin_brush_motion(settings.run_speed, settings.turn_rate)?;
                            }
                        } else {
                            player.end_brush_motion();
                        }
                        if let Some(previous) = hose_phase.take() {
                            *hose_clock = previous.stop_effects(true, |ink| {
                                hose_emitters[usize::from(ink == brush::Ink::Thinner)] = false;
                                Ok(())
                            })?;
                        }
                        *hose_phase = requested_hose.map(|ink| {
                            brush::Firing::begin(
                                if ink == 0 { brush::Ink::Paint } else { brush::Ink::Thinner },
                                std::mem::take(hose_clock),
                            )
                        });
                        *hose_emitters = [false; 2];
                        *hose_active = requested_hose.is_some();
                    }
                    world.publish_player_ground_record(player)?;
                    world.advance_player_knockback(player, STEP)?;
                    return Ok(());
                }

                // AfterMovement phase
                world.refresh_player_stairs_after_physics(player)?;
                let script_report_start = world.report.unsupported.len();
                world.with_live_player(player, |w| w.advance_contact_stimuli(STEP))?;
                world.with_live_player(player, |w| w.advance_ai_projectile_impacts())?;
                world.advance_player_models(player, tick as f32 * STEP)?;
                if world.recover_failed_preview_script(script_report_start) {
                    player.script_input_restricted = false;
                }
                if player.script_input_restricted {
                    requested_hose = None;
                    *spin_pending = false;
                }
                world.brush_runtime.recharge(&mut world.ammo)?;

                if std::mem::take(spin_pending) {
                    if world.player_ability_enabled("Spin", brush::Ink::Paint)? {
                        authored_preview.request_spin();
                    }
                }
                let downward_spray = requested_hose.is_some() && player.can_spray_below();
                authored_preview.brush_jump_transition(downward_spray, &world.player_behavior)?;
                let mut aim_target = None;
                if downward_spray {
                    authored_preview.brush_aim(&mut world.player_graph.state, -1.0, 0.0)?;
                } else if let Some(ink) = requested_hose {
                    let cam = if let Some(frame) = world.published_camera_frame()? {
                        frame.cursor_camera()
                    } else {
                        let (sy, cy) = c_yaw.sin_cos();
                        let (sp, cp) = c_pitch.sin_cos();
                        let dir = [cy * cp, sp, sy * cp];
                        let center = physics::add(player.position, [0.0, 1.0, 0.0]);
                        let dist = world.camera_preview_distance(center, dir, *zoom)?;
                        let extent = (55.0f32.to_radians() * 0.5).tan();
                        let aspect = 400.0 / 240.0;
                        cursor::Camera {
                            position: std::array::from_fn(|i| center[i] + dir[i] * dist),
                            left: [-sy, 0.0, cy],
                            up: [-cy * sp, cp, -sy * sp],
                            forward: [-cy * cp, -sp, -sy * cp],
                            viewport: cursor::Rect { left: 0.0, right: 1.0, top: 1.0, bottom: 0.0 },
                            frustum: cursor::Rect {
                                left: -extent * aspect,
                                right: extent * aspect,
                                top: extent,
                                bottom: -extent,
                            },
                            orthographic: false,
                        }
                    };
                    if let Some(ray) = cam.window_ray(aim_pixel, aim_size)? {
                        let range = world
                            .report
                            .brush_settings
                            .as_ref()
                            .and_then(|s| {
                                s.value(
                                    if ink == 0 { brush::Ink::Paint } else { brush::Ink::Thinner },
                                    "Range",
                                    0,
                                    ability_value::NEUTRAL_PLAYSTYLE,
                                )
                                .ok()
                            })
                            .unwrap_or(14.0);
                        let target = brush_preview::target(&world.collision, ray, player.position, range)?;
                        let aim = brush::target_direction(player.position, target);
                        let yaw = native_angle::atan2(aim.direction[0] as f64, aim.direction[2] as f64) as f32;
                        player.set_aim_facing_target(yaw);
                        authored_preview.brush_aim(
                            &mut world.player_graph.state,
                            aim.elevation_dot,
                            brush::aim_yaw_delta(player.yaw, yaw),
                        )?;
                        if let Some(settings) = &world.report.brush_settings {
                            aim_target = Some(settings.emission_target(target));
                        }
                    }
                }
                world.player_controller_tick(authored_preview, player)?;
                world.export_player_animation_frames(std::mem::take(
                    &mut authored_preview.playback.emitted_frames,
                ))?;
                world.synchronize_player_knockback(player);
                world.entities[world.player_entity].set_controller_pose(player.position, player.yaw);
                world.update_skeletal_children(Some(&authored_preview.playback.pose))?;
                for _ in 0..std::mem::take(&mut authored_preview.pending_combat_queries) {
                    world.dispatch_live_spin(&mut authored_preview.spin_hits)?;
                }
                let pose = skin::model_pose(&authored_preview.pose, &ear_pose.overrides)?;
                let actor = world.entities[world.player_entity]
                    .controller_matrix(player.position, player.yaw);
                let origin = world.brush.emitter_position(&pose, actor, player.position)?;
                if let Some(ink) = requested_hose.filter(|_| downward_spray) {
                    let channel = if ink == 0 { brush::Ink::Paint } else { brush::Ink::Thinner };
                    let range = world
                        .report
                        .brush_settings
                        .as_ref()
                        .and_then(|s| s.value(channel, "Range", 0, ability_value::NEUTRAL_PLAYSTYLE).ok())
                        .unwrap_or(14.0);
                    aim_target = Some([origin[0], origin[1] - range, origin[2]]);
                }
                let direction = aim_target
                    .map(|p| brush::target_direction(origin, p).direction)
                    .unwrap_or([player.yaw.sin(), 0.0, player.yaw.cos()]);
                let mut funded_hose = None;
                if let (Some(phase), Some(ink)) = (hose_phase.as_mut(), requested_hose) {
                    if let Some(target) = aim_target {
                        if let Some(settings) = &world.report.brush_settings {
                            let channel = if ink == 0 { brush::Ink::Paint } else { brush::Ink::Thinner };
                            let cost = settings
                                .value(channel, "Cost", 0, ability_value::NEUTRAL_PLAYSTYLE)
                                .unwrap_or(1.0);
                            let mut effects = brush_runtime::Effects {
                                runtime: &mut world.brush_runtime,
                                emitters: hose_emitters,
                                projectiles: &mut world.projectile_preview,
                                report: &world.report,
                                owner: world.player_entity as u64,
                                origin,
                                target,
                            };
                            if let Ok(firing) = phase.update(
                                channel,
                                true,
                                STEP,
                                settings.projectile_interval,
                                settings.burst,
                                cost,
                                &mut brush::AmmoFiringHost {
                                    ammo: &mut world.ammo,
                                    effects: &mut effects,
                                },
                            ) {
                                if firing.debit.is_some() {
                                    funded_hose = Some(usize::from(channel == brush::Ink::Thinner));
                                }
                            }
                        }
                    }
                }
                let _ = world.advance_brush_projectiles(STEP);
                for index in 0..world.projectile_preview.step_contacts.len() {
                    let contact = world.projectile_preview.step_contacts[index].clone();
                    let _ = world.dispatch_brush_contact(&contact, player.position);
                }
                for (index, hose) in world.brush.hoses.iter_mut().enumerate() {
                    let _ = hose.advance(
                        STEP,
                        hose_emitters[index] && funded_hose == Some(index),
                        origin,
                        direction,
                    );
                }
                Ok(())
            },
        );

        world.entities[world.player_entity]
            .set_controller_pose(controller.player.position, controller.player.yaw);
        let _ = world.advance_screen_fade_frame();

        *camera_yaw = controller.camera_yaw;
        *camera_pitch = controller.camera_pitch;
        let shake = world.script_feedback.angles();
        let cam_yaw = *camera_yaw + shake[0];
        let cam_pitch = *camera_pitch + shake[1];
        let (sy, cy) = cam_yaw.sin_cos();
        let (sp, cp) = cam_pitch.sin_cos();
        let cam_dir = [cy * cp, sp, sy * cp];
        let cam_target = physics::add(controller.player.position, [0.0, 1.0, 0.0]);

        let (eye, view, fov_deg) = if let Ok(Some(frame)) = world.published_camera_frame() {
            (frame.eye, frame.view(), 55.0f32)
        } else {
            let dist = world
                .camera_preview_distance(cam_target, cam_dir, *zoom)
                .unwrap_or(*zoom);
            let eye = std::array::from_fn(|i| cam_target[i] + cam_dir[i] * dist);
            let view = viewport3d::look_at_matrix(eye, cam_target, [0.0, 1.0, 0.0]);
            (eye, view, 55.0f32)
        };

        // Build presented frame draws using the real engine animation & material pipeline!
        let mut draws = world.draws.clone();
        let player_matrix = world.entities[world.player_entity]
            .controller_matrix(controller.player.position, controller.player.yaw);
        for &i in &world.player_draws {
            if let Some(d) = draws.get_mut(i) {
                d.trans_mat = player_matrix;
            }
        }
        let billboard_right = [sy, 0.0, -cy];
        let billboard_up = [-cy * sp, cp, -sy * sp];
        if let Ok(pose) = skin::model_pose(&authored_preview.pose, &ear_pose.overrides) {
            let _ = player_skin_frame.apply(world, &pose, &mut draws);
            if !world.hidden_entities.contains(&world.player_entity) {
                if let Some(&first_p) = world.player_draws.first() {
                    let actor = draws[first_p].trans_mat;
                    let canonical = matches!(
                        world.child_entity_by_name(world.player_entity, "magicbrush"),
                        Ok(Some(child)) if !world.entities[child].draws.is_empty()
                    );
                    if !canonical {
                        let _ = world.brush.append(&pose, actor, &mut draws);
                    }
                    if let Some(ribbon) = &world.spin_ribbon {
                        let _ = ribbon.append(billboard_right, billboard_up, &mut draws);
                    }
                    for hose in &world.brush.hoses {
                        let _ = hose.append(billboard_right, billboard_up, &mut draws);
                    }
                }
            }
        }
        let _ = world.apply_model_geometry(&mut draws);
        let _ = world.apply_model_camera_transforms(&mut draws, eye, billboard_right, billboard_up);
        let _ = world.apply_model_materials(&mut draws);
        let _ = world.apply_model_constant_colors(&mut draws);
        let _ = world.material_animation.apply(world.animation_time, &mut draws);

        let player_draw_set: HashSet<usize> = world.player_draws.iter().copied().collect();
        let orig_draw_count = world.draws.len();

        // Render 3D scene on PICA200 Top Screen (Stereoscopic 3D when slider > 0!)
        let iod = inp.slider_3d * 0.18;
        let clear_rgba = 0x182238FFu32;
        unsafe { bw3ds_frame_begin() };
        let eyes = if iod > 0.001 { 2 } else { 1 };
        for eye_idx in 0..eyes {
            let eye_iod = if eye_idx == 0 { -iod } else { iod };
            unsafe {
                bw3ds_begin_eye(
                    eye_idx,
                    clear_rgba,
                    fov_deg,
                    0.25,
                    450.0,
                    eye_iod,
                    view.as_ptr(),
                );
            }
            let draw_visible = |idx: usize, draw: &GpuDrawCommand| -> bool {
                !draw.projected_shadow
                    && !world.hidden_jigsaw_draws.contains(&idx)
                    && !world.hidden_entity_draws.contains(&idx)
                    && !world.removed_draws.contains(&idx)
                    && !world.demo_story.scene.culled_draws.contains(&idx)
                    && !draw
                        .material
                        .as_ref()
                        .and_then(|m| m.shader.as_deref())
                        .is_some_and(|s| s.eq_ignore_ascii_case("SPL_hiddenSurface"))
            };
            let is_transparent_draw = |idx: usize, draw: &GpuDrawCommand| -> bool {
                world.sketch_draws.contains(&idx)
                    || draw.color[3] < 0.99
                    || draw
                        .material
                        .as_ref()
                        .and_then(|m| m.legacy_alpha_flags)
                        .is_some_and(|flags| (flags & 1) != 0)
            };

            // Pass 0: Opaque geometry (depth write ON, alpha blend OFF, 1-bit alpha cutout ON)
            unsafe { bw3ds_set_blend_mode(0) };
            for (idx, draw) in draws.iter().enumerate() {
                if !draw_visible(idx, draw) || is_transparent_draw(idx, draw) {
                    continue;
                }
                let is_player = player_draw_set.contains(&idx) || idx >= orig_draw_count;
                let tint = [draw.color[0], draw.color[1], draw.color[2], 1.0];
                pica.draw_command(idx, draw, is_player, is_player, eye, tint);
            }

            // Pass 1: Transparent / sketch / brush FX geometry (depth write OFF, alpha blend ON)
            unsafe { bw3ds_set_blend_mode(1) };
            for (idx, draw) in draws.iter().enumerate() {
                if !draw_visible(idx, draw) || !is_transparent_draw(idx, draw) {
                    continue;
                }
                let is_player = player_draw_set.contains(&idx) || idx >= orig_draw_count;
                let tint = if world.sketch_draws.contains(&idx) {
                    [0.35, 0.85, 1.0, 0.50]
                } else {
                    draw.color
                };
                pica.draw_command(idx, draw, is_player, is_player, eye, tint);
            }
        }
        unsafe { bw3ds_frame_end() };

        // Refresh Bottom Screen Real Engine HUD (every 4 frames to save ARM11 cycles)
        hud_counter = hud_counter.wrapping_add(1);
        if hud_counter % 4 == 1 {
            let mut app_mb = 0u32;
            let mut lin_mb = 0u32;
            unsafe { bw3ds_ram_free(&mut app_mb, &mut lin_mb) };
            let hp = world
                .runtime_health
                .get(&world.player_entity)
                .map(|h| h.health)
                .unwrap_or(5);
            let paint = world.ammo.available(0);
            let thinner = world.ammo.available(1);
            console_line(1, "== BLOTWORX ENGINE 3DS (REAL RUST) ==");
            console_line(2, &format!("Lvl: {}", world.report.level));
            console_line(
                4,
                &format!("HP:{hp}  Paint:{paint:.1}  Thinner:{thinner:.1}"),
            );
            let p = controller.player.position;
            console_line(
                5,
                &format!(
                    "Pos:({:.1},{:.1},{:.1}) {}",
                    p[0],
                    p[1],
                    p[2],
                    if controller.player.grounded { "[GND]" } else { "[AIR]" }
                ),
            );
            console_line(
                6,
                &format!(
                    "Draws:{}  Tick:{}  RAM:{app_mb}M/{lin_mb}M",
                    draws.len(),
                    controller.ticks
                ),
            );
            if show_level_menu {
                console_line(8, "-- SELECT LEVEL (DPad+A, SELECT back) -");
                for (i, &lvl) in QUICK_LEVELS.iter().enumerate() {
                    let mark = if i == menu_cursor { '>' } else { ' ' };
                    console_line(9 + i as i32, &format!("{mark} {lvl}"));
                }
            } else {
                console_line(8, "CPad:Move   B:Jump/2x   Y:Spin");
                console_line(9, "R:Paint     L:Thinner   Touch:Aim");
                console_line(10, "CStick/DPad:Camera  SELECT:Levels");
                console_line(11, "START:Exit   (Full Lua+Havok+NIF)");
                for row in 12..19 {
                    console_line(row, "");
                }
            }
        }
    }

    Ok(())
}
