// editors/viewport3d.rs — Centralized 3D Viewport, Camera, Shaders & Hardware GL Renderer

use egui::{Color32, Rect, Response, Sense, Ui};
use egui_glow::glow::{self, HasContext};
use std::sync::{Arc, OnceLock};

// ─────────────────────────────────────────────────────────────────────────────
// Shared Types & Data
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct DecodedImage {
    pub rgba: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RenderMode {
    Solid,
    Wireframe,
    Lit,
    Unlit,
}

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum LightingMode {
    #[default]
    Headlight,
    SceneLights,
    Unlit,
}

#[derive(Default, PartialEq, Clone, Copy, Debug)]
pub enum GizmoMode {
    #[default]
    Select,
    Translate,
    Rotate,
    Scale,
}

#[derive(Default, PartialEq, Clone, Copy, Debug)]
pub enum ActiveAxis {
    #[default]
    None,
    X,
    Y,
    Z,
    Center,
}

/// Explicit authored camera submission; orbit editing remains the default.
#[derive(Clone, Copy, Debug)]
pub struct CameraOverride {
    pub eye: [f32; 3],
    pub view: [f32; 16],
    pub projection: [f32; 16],
}

#[derive(Clone, Debug)]
pub struct Viewport3DState {
    pub smooth_textures: bool,
    pub(crate) native_material:Option<super::native_material::Shared>,
    pub view_culling: bool,
    pub limitations_overlay: bool,
    pub show_limitations: bool,
    pub(crate) pixel_probe:Option<[f32;2]>,
    pub(crate) finish: super::scene_finish::Settings,
    pub(crate) surfaces: super::surface_lighting::Settings,
    pub render_work: Arc<Mutex<RenderWork>>,
    pub volume_shadow_mesh: Option<Arc<super::volume_shadow::Mesh>>,
    pub volume_shadows: Vec<[f32; 16]>,
    pub volume_shadow_report: Arc<Mutex<super::volume_shadow::Report>>,
    pub(crate) model_shadow: Option<Arc<super::model_shadow::Frame>>,
    pub(crate) model_shadow_report: Arc<Mutex<super::model_shadow::Report>>,
    pub camera_override: Option<CameraOverride>,
    pub pitch: f32,
    pub yaw: f32,
    pub distance: f32,
    pub camera_target: [f32; 3],
    pub fov_degrees: f32,
    pub near_clip: f32,
    pub far_clip: f32,
    pub grid_size: f32,
    pub show_grid: bool,
    pub show_models: bool,
    pub show_textures: bool,
    pub show_triggers: bool,
    pub show_splines: bool,
    pub show_lights: bool,
    pub show_cameras: bool,
    pub show_markers: bool,
    pub show_bones: bool,
    pub show_collisions: bool,
    pub show_labels: bool,
    pub show_navigation: bool,
    pub render_mode: RenderMode,
    pub lighting_mode: LightingMode,
    pub backface_culling: bool,
    pub flip_winding: bool,
    pub gizmo_mode: GizmoMode,
    pub gizmo_local_space: bool,
    pub active_axis: ActiveAxis,
    pub is_dragging_gizmo: bool,
    pub gizmo_drag_start_trans: [f32; 3],
    pub gizmo_size_screen: f32,
    pub primary_drag_orbits: bool,
}

impl Default for Viewport3DState {
    fn default() -> Self {
        Self {
            native_material:None,
            view_culling: true,
            limitations_overlay: false,
            show_limitations: true,
            pixel_probe:None,
            finish: Default::default(),
            surfaces: Default::default(),
            smooth_textures: false,
            render_work: Default::default(),
            volume_shadow_mesh: None,
            volume_shadows: Vec::new(),
            volume_shadow_report: Default::default(),
            model_shadow: None,
            model_shadow_report: Default::default(),
            camera_override: None,
            pitch: 0.4,
            yaw: 0.7,
            distance: 40.0,
            camera_target: [0.0; 3],
            fov_degrees: 60.0,
            near_clip: 0.1,
            far_clip: 50000.0,
            grid_size: 5.0,
            show_grid: true,
            show_models: true,
            show_textures: true,
            show_triggers: true,
            show_splines: true,
            show_lights: true,
            show_cameras: true,
            show_markers: true,
            show_bones: true,
            show_collisions: true,
            show_labels: false,
            show_navigation: false,
            render_mode: RenderMode::Lit,
            lighting_mode: LightingMode::SceneLights,
            backface_culling: true,
            flip_winding: false,
            gizmo_mode: GizmoMode::Select,
            gizmo_local_space: false,
            active_axis: ActiveAxis::None,
            is_dragging_gizmo: false,
            gizmo_drag_start_trans: [0.0; 3],
            gizmo_size_screen: 80.0,
            primary_drag_orbits: true,
        }
    }
}

/// Per-surface sampler state; textures can be shared by differently tiled materials.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureSampler {
    pub wrap_u: u32,
    pub wrap_v: u32,
    pub min_filter: u32,
    pub mag_filter: u32,
}
impl Default for TextureSampler {
    fn default() -> Self {
        Self::from_nif_flags(0x3200)
    }
}
impl TextureSampler {
    pub fn from_nif_flags(flags: u16) -> Self {
        let filter = (flags >> 8) & 15;
        Self {
            wrap_u: if flags & 0x2000 != 0 {
                glow::REPEAT
            } else {
                glow::CLAMP_TO_EDGE
            },
            wrap_v: if flags & 0x1000 != 0 {
                glow::REPEAT
            } else {
                glow::CLAMP_TO_EDGE
            },
            min_filter: match filter {
                0 => glow::NEAREST,
                1 => glow::LINEAR,
                3 => glow::NEAREST_MIPMAP_NEAREST,
                4 => glow::NEAREST_MIPMAP_LINEAR,
                5 => glow::LINEAR_MIPMAP_NEAREST,
                _ => glow::LINEAR_MIPMAP_LINEAR,
            },
            mag_filter: if matches!(filter, 0 | 3 | 4) {
                glow::NEAREST
            } else {
                glow::LINEAR
            },
        }
    }
    fn smoothed(self, enabled: bool) -> Self {
        if enabled {
            Self { min_filter: glow::LINEAR_MIPMAP_LINEAR, mag_filter: glow::LINEAR, ..self }
        } else {
            self
        }
    }
    unsafe fn apply(self, gl: &glow::Context) {
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, self.wrap_u as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, self.wrap_v as i32);
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            self.min_filter as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            self.mag_filter as i32,
        );
    }
}
#[derive(Clone, Debug)]
pub struct GpuDrawCommand {
    /// Per-mesh runtime controller value; material assets remain immutable.
    pub(crate) shader_time:Option<crate::nif_shader_time::Clock>,
    pub(crate) source_attributes: Option<Result<std::sync::Arc<crate::nif_vertex_attributes::Source>,String>>,
    pub(crate) projected_shadow: bool,
    pub(crate) skybox: bool,
    pub extra_uvs: Option<Arc<std::collections::BTreeMap<u32, Arc<Vec<[f32; 2]>>>>>,
    /// Original mesh NiAVObject world-bound center for pinned projection.
    /// Missing bounds must not be replaced with the entity origin or an AABB.
    pub projection_bound: Option<[f32; 3]>,
    pub material_registers: Option<[[f32; 4]; 7]>,
    pub material_indirect: Option<[[i32; 3]; 6]>,
    /// Immutable after publication in the common render path. Sharing this
    /// list keeps per-frame draw-command snapshots from allocating one small
    /// Vec per mesh every frame.
    pub material_images: Arc<Vec<super::material_preview::MaterialImage>>,
    pub baked: bool,
    pub baked_error: Option<String>,
    pub preview: Option<Result<Arc<super::material_preview::Program>, String>>,
    pub uv_transform: Option<crate::nif_textures::UvTransform>,
    pub material: Option<Arc<super::nif::MaterialBindings>>,
    pub sampler: TextureSampler,
    pub v_data: Arc<Vec<f32>>,
    pub indices: Arc<Vec<u32>>,
    pub trans_mat: [f32; 16],
    pub color: [f32; 4],
    pub tex_bytes: Option<(Arc<Vec<u8>>, u32, u32)>,
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
    /// CPU-only: trimmed Light Group name from the entity's component (e.g. "Global", "(null)", "")
    pub light_group: Arc<str>,
}
impl GpuDrawCommand {
    fn needs_alpha_sort(&self, prepared: bool, platform_native: bool) -> bool {
        let alpha = self.material.as_ref().and_then(|m| m.legacy_alpha_flags).unwrap_or(0);
        if alpha & 0x2000 != 0 || self.material.as_ref().is_some_and(|m| m.sort_disabled) {
            return false;
        }
        if platform_native {
            return alpha & 1 != 0;
        }
        if prepared {
            if let Some(Ok(program)) = &self.preview {
                return program.blend.is_some_and(|blend| {
                    blend != (glow::ONE, glow::ZERO, glow::FUNC_ADD)
                });
            }
        }
        self.material.as_ref().is_some_and(|m| m.shader.is_none()) && alpha & 1 != 0
    }

    fn alpha_depth(&self, view: &[f32; 16]) -> f32 {
        let center: [f32; 3] = std::array::from_fn(|axis| {
            (self.aabb_min[axis] + self.aabb_max[axis]) * 0.5
        });
        let world: [f32; 3] = std::array::from_fn(|axis| {
            self.trans_mat[12 + axis]
                + (0..3).map(|column| self.trans_mat[column * 4 + axis] * center[column]).sum::<f32>()
        });
        -(view[2] * world[0] + view[6] * world[1] + view[10] * world[2])
    }

    /// Release per-draw asset references after its owning entity is retired.
    /// Keep the stable draw slot and lightweight bounds/transform metadata so
    /// indices held by reports and runtime handles never change.
    pub(crate) fn release_retired_assets(&mut self) {
        static EMPTY_FLOATS: OnceLock<Arc<Vec<f32>>> = OnceLock::new();
        static EMPTY_INDICES: OnceLock<Arc<Vec<u32>>> = OnceLock::new();
        static EMPTY_IMAGES: OnceLock<Arc<Vec<super::material_preview::MaterialImage>>> =
            OnceLock::new();
        self.v_data = EMPTY_FLOATS.get_or_init(|| Arc::new(Vec::new())).clone();
        self.indices = EMPTY_INDICES.get_or_init(|| Arc::new(Vec::new())).clone();
        self.material_images = EMPTY_IMAGES.get_or_init(|| Arc::new(Vec::new())).clone();
        self.extra_uvs = None;
        self.source_attributes = None;
        self.shader_time = None;
        self.projection_bound = None;
        self.material_registers = None;
        self.material_indirect = None;
        self.preview = None;
        self.uv_transform = None;
        self.material = None;
        self.tex_bytes = None;
        self.baked = false;
        self.baked_error = None;
        self.light_group = Arc::from("");
    }
}

#[derive(Clone, Debug)]
pub struct GpuLineSegment {
    pub start: [f32; 3],
    pub end: [f32; 3],
    pub color: [f32; 4],
}

// ─────────────────────────────────────────────────────────────────────────────
// Matrix & Vector Math
// ─────────────────────────────────────────────────────────────────────────────

pub fn perspective_matrix(fovy: f32, aspect: f32, near: f32, far: f32) -> [f32; 16] {
    let f = 1.0 / (fovy * 0.5).tan();
    let nf = 1.0 / (near - far);
    [
        f / aspect,
        0.0,
        0.0,
        0.0,
        0.0,
        f,
        0.0,
        0.0,
        0.0,
        0.0,
        (far + near) * nf,
        -1.0,
        0.0,
        0.0,
        (2.0 * far * near) * nf,
        0.0,
    ]
}

pub fn ortho_matrix(
    left: f32,
    right: f32,
    bottom: f32,
    top: f32,
    near: f32,
    far: f32,
) -> [f32; 16] {
    let r_l = 1.0 / (right - left);
    let t_b = 1.0 / (top - bottom);
    let f_n = 1.0 / (far - near);
    [
        2.0 * r_l,
        0.0,
        0.0,
        0.0,
        0.0,
        2.0 * t_b,
        0.0,
        0.0,
        0.0,
        0.0,
        -2.0 * f_n,
        0.0,
        -(right + left) * r_l,
        -(top + bottom) * t_b,
        -(far + near) * f_n,
        1.0,
    ]
}

pub fn look_at_matrix(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> [f32; 16] {
    let fx = target[0] - eye[0];
    let fy = target[1] - eye[1];
    let fz = target[2] - eye[2];
    let flen = (fx * fx + fy * fy + fz * fz).sqrt().max(1e-6);
    let (fx, fy, fz) = (fx / flen, fy / flen, fz / flen);

    let rx = fy * up[2] - fz * up[1];
    let ry = fz * up[0] - fx * up[2];
    let rz = fx * up[1] - fy * up[0];
    let rlen = (rx * rx + ry * ry + rz * rz).sqrt().max(1e-6);
    let (rx, ry, rz) = (rx / rlen, ry / rlen, rz / rlen);

    let ux = ry * fz - rz * fy;
    let uy = rz * fx - rx * fz;
    let uz = rx * fy - ry * fx;

    [
        rx,
        ux,
        -fx,
        0.0,
        ry,
        uy,
        -fy,
        0.0,
        rz,
        uz,
        -fz,
        0.0,
        -(rx * eye[0] + ry * eye[1] + rz * eye[2]),
        -(ux * eye[0] + uy * eye[1] + uz * eye[2]),
        (fx * eye[0] + fy * eye[1] + fz * eye[2]),
        1.0,
    ]
}

pub fn mat4_mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut out = [0.0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = a[0 * 4 + row] * b[col * 4 + 0]
                + a[1 * 4 + row] * b[col * 4 + 1]
                + a[2 * 4 + row] * b[col * 4 + 2]
                + a[3 * 4 + row] * b[col * 4 + 3];
        }
    }
    out
}

pub fn transform_aabb(
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
    trans_mat: &[f32; 16],
) -> ([f32; 3], [f32; 3]) {
    let corners = [
        [aabb_min[0], aabb_min[1], aabb_min[2]],
        [aabb_max[0], aabb_min[1], aabb_min[2]],
        [aabb_min[0], aabb_max[1], aabb_min[2]],
        [aabb_max[0], aabb_max[1], aabb_min[2]],
        [aabb_min[0], aabb_min[1], aabb_max[2]],
        [aabb_max[0], aabb_min[1], aabb_max[2]],
        [aabb_min[0], aabb_max[1], aabb_max[2]],
        [aabb_max[0], aabb_max[1], aabb_max[2]],
    ];
    let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
    for c in corners {
        let x = c[0] * trans_mat[0] + c[1] * trans_mat[4] + c[2] * trans_mat[8] + trans_mat[12];
        let y = c[0] * trans_mat[1] + c[1] * trans_mat[5] + c[2] * trans_mat[9] + trans_mat[13];
        let z = c[0] * trans_mat[2] + c[1] * trans_mat[6] + c[2] * trans_mat[10] + trans_mat[14];
        for i in 0..3 {
            mn[i] = mn[i].min([x, y, z][i]);
            mx[i] = mx[i].max([x, y, z][i]);
        }
    }
    (mn, mx)
}

pub fn is_aabb_visible(mn: [f32; 3], mx: [f32; 3], vp_mat: &[f32; 16]) -> bool {
    let planes = [
        [
            vp_mat[3] + vp_mat[0],
            vp_mat[7] + vp_mat[4],
            vp_mat[11] + vp_mat[8],
            vp_mat[15] + vp_mat[12],
        ],
        [
            vp_mat[3] - vp_mat[0],
            vp_mat[7] - vp_mat[4],
            vp_mat[11] - vp_mat[8],
            vp_mat[15] - vp_mat[12],
        ],
        [
            vp_mat[3] + vp_mat[1],
            vp_mat[7] + vp_mat[5],
            vp_mat[11] + vp_mat[9],
            vp_mat[15] + vp_mat[13],
        ],
        [
            vp_mat[3] - vp_mat[1],
            vp_mat[7] - vp_mat[5],
            vp_mat[11] - vp_mat[9],
            vp_mat[15] - vp_mat[13],
        ],
        [
            vp_mat[3] + vp_mat[2],
            vp_mat[7] + vp_mat[6],
            vp_mat[11] + vp_mat[10],
            vp_mat[15] + vp_mat[14],
        ],
        [
            vp_mat[3] - vp_mat[2],
            vp_mat[7] - vp_mat[6],
            vp_mat[11] - vp_mat[10],
            vp_mat[15] - vp_mat[14],
        ],
    ];
    for p in planes {
        let px = if p[0] > 0.0 { mx[0] } else { mn[0] };
        let py = if p[1] > 0.0 { mx[1] } else { mn[1] };
        let pz = if p[2] > 0.0 { mx[2] } else { mn[2] };
        if p[0] * px + p[1] * py + p[2] * pz + p[3] < 0.0 {
            return false;
        }
    }
    true
}

// ─────────────────────────────────────────────────────────────────────────────
// Hardware GL Renderers & Texture Upload
// ─────────────────────────────────────────────────────────────────────────────

pub unsafe fn upload_gpu_texture(
    gl: &glow::Context,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> Option<glow::NativeTexture> {
    let tex = gl.create_texture().ok()?;
    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
    gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);

    let Some(required_len) = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
    else {
        gl.delete_texture(tex);
        return None;
    };
    if rgba.len() < required_len {
        gl.delete_texture(tex);
        return None;
    }
    // Every RGBA8 source byte is preserved at upload. If alpha is uniformly
    // opaque, RGB8 storage is exactly equivalent: sampling an absent alpha
    // channel returns 1.0, identical to the authored 255/255 value.
    let (opaque, _, _) = texture_storage_estimate(rgba, width, height);
    let storage_format = if opaque { glow::RGB8 } else { glow::RGBA8 };

    gl.tex_image_2d(
        glow::TEXTURE_2D,
        0,
        storage_format as i32,
        width as i32,
        height as i32,
        0,
        glow::RGBA,
        glow::UNSIGNED_BYTE,
        glow::PixelUnpackData::Slice(Some(&rgba[..required_len])),
    );
    gl.generate_mipmap(glow::TEXTURE_2D);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::REPEAT as i32);
    gl.tex_parameter_i32(
        glow::TEXTURE_2D,
        glow::TEXTURE_MIN_FILTER,
        glow::LINEAR_MIPMAP_LINEAR as i32,
    );
    gl.tex_parameter_i32(
        glow::TEXTURE_2D,
        glow::TEXTURE_MAG_FILTER,
        glow::LINEAR as i32,
    );
    Some(tex)
}

fn mip_chain_pixels(width: u32, height: u32) -> usize {
    if width == 0 || height == 0 {
        return 0;
    }
    let mut width = width as usize;
    let mut height = height as usize;
    let mut total = 0usize;
    loop {
        total = total.saturating_add(width.saturating_mul(height));
        if width == 1 && height == 1 {
            break;
        }
        width = (width / 2).max(1);
        height = (height / 2).max(1);
    }
    total
}

/// `(opaque, estimated GPU mip-chain bytes, bytes saved versus RGBA8)`.
/// Driver padding/alignment is implementation-specific; this uses the exact
/// requested storage formats and OpenGL's floor-halved mip dimensions.
fn texture_storage_estimate(rgba: &[u8], width: u32, height: u32) -> (bool, usize, usize) {
    let base_pixels = (width as usize).checked_mul(height as usize);
    let Some(base_bytes) = base_pixels.and_then(|pixels| pixels.checked_mul(4)) else {
        return (false, 0, 0);
    };
    if rgba.len() < base_bytes {
        return (false, 0, 0);
    }
    let opaque = rgba[..base_bytes]
        .chunks_exact(4)
        .all(|pixel| pixel[3] == u8::MAX);
    let mip_pixels = mip_chain_pixels(width, height);
    let full_rgba = mip_pixels.saturating_mul(4);
    let channels = if opaque { 3 } else { 4 };
    (opaque, mip_pixels.saturating_mul(channels), full_rgba.saturating_sub(mip_pixels.saturating_mul(channels)))
}

use std::collections::HashMap;
use std::sync::Mutex;

static GLOBAL_GL_CACHE: Mutex<Option<GpuViewportCache>> = Mutex::new(None);

/// Estimate currently live scene-asset storage represented by the shared GL
/// cache, including texture mip chains and RGB8 opaque textures. It excludes
/// driver metadata and render targets.
pub fn cached_scene_asset_payload_bytes() -> usize {
    let Ok(guard) = GLOBAL_GL_CACHE.lock() else {
        return 0;
    };
    let Some(cache) = guard.as_ref() else {
        return 0;
    };
    let mesh_bytes = cache
        .meshes
        .values()
        .map(|(vertices, indices, _)| {
            vertices
                .upgrade()
                .map_or(0, |values| {
                    values.len().saturating_mul(std::mem::size_of::<f32>())
                })
                .saturating_add(indices.upgrade().map_or(0, |values| {
                    values.len().saturating_mul(std::mem::size_of::<u32>())
                }))
        })
        .sum::<usize>();
    let uv_bytes = cache
        .uv_buffers
        .values()
        .map(|(values, _)| {
            values.upgrade().map_or(0, |values| {
                values.len().saturating_mul(2 * std::mem::size_of::<f32>())
            })
        })
        .sum::<usize>();
    let texture_bytes = cache
        .textures
        .iter()
        .map(|(_, (_, _, storage_bytes, _))| *storage_bytes)
        .sum::<usize>();
    mesh_bytes
        .saturating_add(uv_bytes)
        .saturating_add(texture_bytes)
}

/// Estimated GPU bytes saved by storing fully opaque images as RGB8 rather
/// than RGBA8, including their generated mip levels.
pub fn cached_opaque_texture_savings_bytes() -> usize {
    GLOBAL_GL_CACHE
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|cache| {
            cache.textures.values().map(|(_, _, _, savings)| *savings).sum()
        }))
        .unwrap_or(0)
}

/// Whether the large legacy scene-light shadow target has been allocated.
pub fn legacy_shadow_target_allocated() -> bool {
    GLOBAL_GL_CACHE
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|cache| cache.shadow_tex.is_some()))
        .unwrap_or(false)
}

pub fn clear_gpu_cache() {
    if let Ok(mut cache_guard) = GLOBAL_GL_CACHE.lock() {
        if let Some(ref mut cache) = *cache_guard {
            cache.invalidated = true;
        }
    }
}

#[derive(Clone, Copy)]
pub struct GpuMeshHandle {
    pub vao: glow::NativeVertexArray,
    pub vbo: glow::NativeBuffer,
    pub ebo: glow::NativeBuffer,
    pub index_count: i32,
    bounds: Option<super::clip_visibility::Bounds>,
}

#[derive(Clone, Debug)]
pub struct GpuLight {
    pub light_type: i32, // 0 = Ambient, 1 = Directional, 2 = Point, 3 = Spot
    pub position: [f32; 3],
    pub direction: [f32; 3],
    pub dimmer: f32,
    pub ambient: [f32; 3],
    pub diffuse: [f32; 3],
    pub specular: [f32; 3],
    pub attenuation: [f32; 3], // constant, linear, quadratic
    pub range: f32,
    pub range_start: f32,
    pub spot_inner: f32, // cos of inner angle
    pub spot_outer: f32, // cos of outer angle
    pub spot_exponent: f32,
    /// CPU-only: trimmed Light Group name from JPSLightComponent (e.g. "Global", "(null)", "")
    pub light_group: String,
}

impl Default for GpuLight {
    fn default() -> Self {
        Self {
            light_type: 2,
            position: [0.0; 3],
            direction: [0.0, -1.0, 0.0],
            dimmer: 1.0,
            ambient: [0.0; 3],
            diffuse: [0.0; 3],
            specular: [0.0; 3],
            attenuation: [1.0, 0.05, 0.005],
            range: 50.0,
            range_start: 0.,
            spot_inner: 0.95,
            spot_outer: 0.8,
            spot_exponent: 1.0,
            light_group: String::new(),
        }
    }
}

#[derive(Clone)]
struct MaterialGpuProgram {
    surface_uniforms: Arc<super::surface_lighting::Uniforms>,
    shadow_matrix: Option<glow::UniformLocation>,
    clip_matrix: Option<glow::UniformLocation>,
    shadow_image: Option<glow::UniformLocation>,
    clip_image: Option<glow::UniformLocation>,
    uv_sets: Option<glow::UniformLocation>,
    projection_bound: Option<glow::UniformLocation>,
    scene_rect: Option<glow::UniformLocation>,
    indirect: Option<glow::UniformLocation>,
    indirect_override: Option<glow::UniformLocation>,
    colors: Option<glow::UniformLocation>,
    constants: Option<glow::UniformLocation>,
    program: glow::Program,
    model_view: Option<glow::UniformLocation>,
    mvp: Option<glow::UniformLocation>,
    uv: Option<glow::UniformLocation>,
    texture: Option<glow::UniformLocation>,
}
#[derive(Clone)]
struct PreparedMaterial {
    projector: Option<([[f32; 16]; 2], [glow::Texture; 2])>,
    program: MaterialGpuProgram,
    mvp: [f32; 16],
    textures: Vec<(u8, glow::Texture, glow::Sampler, [f32; 9])>,
    uv_sets: [i32; 8],
    projection_bound: Option<[f32; 3]>,
    mesh: GpuMeshHandle,
    extra_uv_buffers: [Option<glow::Buffer>; 7],
}
#[derive(Default, Clone, Debug, serde::Serialize)]
pub struct RenderWork {
    pub platform_native_draws:usize,
    pub platform_native_errors:std::collections::BTreeMap<String,usize>,
    pub pixel_history:Vec<serde_json::Value>,
    pub surface_style: u32,
    pub hdr_scene: bool,
    pub occlusion_frames:u64,
    pub occlusion_error:Option<String>,
    pub surface_draws: usize,
    pub surface_light_records: usize,
    pub surface_candidate_pairs: usize,
    pub surface_admitted_pairs: usize,
    pub surface_cluster_rebuilds: u64,
    pub surface_buffer_uploads: u64,
    pub vertex_buffer_uploads: u64,
    pub index_buffer_uploads: u64,
    pub surface_error: Option<String>,
    pub scene_light_counts: [usize;4],
    pub finishing_frames: u64,
    pub finish_error: Option<String>,
    pub gpu_samples: u64,
    pub gpu_total_ms: f64,
    pub gpu_max_ms: f64,
    pub gpu_skipped_samples: u64,
    pub gpu_timing_error: Option<String>,
    pub frames: u64,
    pub native_draws: usize,
    pub fallback_draws: usize,
    pub shadow_pass_frames: u64,
    pub skipped_shadow_pass_frames: u64,
    pub shadow_caster_draws: u64,
    pub rejected_shadow_casters: u64,
    pub submission_total_ms: f64,
    pub submission_max_ms: f64,
    pub fallback_materials: std::collections::BTreeMap<String, usize>,
    pub rejected_main_draws: usize,
    pub rejected_main_draws_total: u64,
    pub early_rejected_draws: usize,
    pub early_rejected_draws_total: u64,
    pub submitted_native_draws: usize,
    pub submitted_fallback_draws: usize,
}
unsafe fn read_probe_pixel(gl:&glow::Context,p:[i32;2])->[f32;4]{
 let mut rgba=[0f32;4];let bytes=std::slice::from_raw_parts_mut(rgba.as_mut_ptr()as *mut u8,16);gl.read_pixels(p[0],p[1],1,1,glow::RGBA,glow::FLOAT,glow::PixelPackData::Slice(Some(bytes)));rgba
}
unsafe fn read_probe_depth(gl:&glow::Context,p:[i32;2])->f32{
 let mut depth=1f32;let bytes=std::slice::from_raw_parts_mut((&mut depth)as *mut f32 as *mut u8,4);gl.read_pixels(p[0],p[1],1,1,glow::DEPTH_COMPONENT,glow::FLOAT,glow::PixelPackData::Slice(Some(bytes)));depth
}
struct IndexBuffer {
    owner: std::sync::Weak<Vec<u32>>,
    buffer: glow::NativeBuffer,
    max_index: Option<u32>,
}

pub struct GpuViewportCache {
    smooth_textures: bool,
    surfaces: super::surface_lighting::State,
    contact_pass:Option<Result<super::contact_occlusion::Pass,String>>,
    finish_pass: Option<Result<super::scene_finish::Pass,String>>,
    em2_post: Option<Result<super::em2_post::Pass,String>>,
    scene_color: Option<(glow::Texture, [i32; 2])>,
    gpu_timing: super::gpu_timing::Pool,
    model_shadow_pass: Option<Result<super::model_shadow::Pass, String>>,
    volume_shadow_pass: Option<Result<super::volume_shadow::Pass, String>>,
    uv_buffers: HashMap<usize, (std::sync::Weak<Vec<[f32; 2]>>, glow::NativeBuffer)>,
    index_buffers: HashMap<usize, IndexBuffer>,
    vertex_buffer_uploads: u64,
    index_buffer_uploads: u64,
    pub mesh_program: glow::Program,
    mesh_surface_uniforms: super::surface_lighting::Uniforms,
    pub u_mvp: glow::UniformLocation,
    pub u_model: glow::UniformLocation,
    pub u_color: glow::UniformLocation,
    pub u_use_tex: glow::UniformLocation,
    pub u_alpha_function: glow::UniformLocation,
    pub u_alpha_reference: glow::UniformLocation,
    pub u_modulate_vertex_color: glow::UniformLocation,
    pub u_baked: glow::UniformLocation,
    pub u_uv_transform: glow::UniformLocation,
    pub u_view_pos: glow::UniformLocation,
    pub u_num_lights: glow::UniformLocation,
    pub u_lights_loc: glow::UniformLocation,
    pub u_lighting_mode: glow::UniformLocation,
    pub u_mesh_light_vp: glow::UniformLocation,
    pub u_shadow_map_loc: glow::UniformLocation,

    pub shadow_fbo: Option<glow::NativeFramebuffer>,
    /// Frame counter and the light matrix the legacy shadow map was last drawn with.
    pub shadow_age: u64,
    pub shadow_vp: Option<[f32; 16]>,
    pub shadow_tex: Option<glow::NativeTexture>,
    pub shadow_program: glow::Program,
    pub u_shadow_light_vp: glow::UniformLocation,
    pub u_shadow_model: glow::UniformLocation,

    pub line_program: glow::Program,
    pub line_u_vp: glow::UniformLocation,
    pub line_vao: glow::NativeVertexArray,
    pub line_vbo: glow::NativeBuffer,

    pub grid_program: glow::Program,
    pub grid_u_vp: glow::UniformLocation,
    pub grid_vao: glow::NativeVertexArray,
    pub grid_vbo: glow::NativeBuffer,
    pub grid_vert_count: i32,

    meshes: HashMap<
        (usize, usize),
        (
            std::sync::Weak<Vec<f32>>,
            std::sync::Weak<Vec<u32>>,
            GpuMeshHandle,
        ),
    >,
    textures: HashMap<(usize, u32, u32), (std::sync::Weak<Vec<u8>>, glow::NativeTexture, usize, usize)>,
    material_samplers: HashMap<TextureSampler, glow::NativeSampler>,
    material_programs: HashMap<
        usize,
        (
            std::sync::Weak<super::material_preview::Program>,
            Option<MaterialGpuProgram>,
        ),
    >,
    invalidated: bool,
}

/// Developer smoke test using the same program cache and draw path as both editors.
#[cfg(debug_assertions)]
pub unsafe fn validate_material_gpu(gl: &glow::Context) -> anyhow::Result<()> {
    use anyhow::{ensure, Context};
    let mut cache = GpuViewportCache::init(gl).context("Viewport GPU initialization failed")?;
    let texture = gl.create_texture().map_err(anyhow::Error::msg)?;
    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
    gl.tex_image_2d(
        glow::TEXTURE_2D,
        0,
        glow::RGBA8 as i32,
        4,
        4,
        0,
        glow::RGBA,
        glow::UNSIGNED_BYTE,
        glow::PixelUnpackData::Slice(None),
    );
    let fbo = gl.create_framebuffer().map_err(anyhow::Error::msg)?;
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
    gl.framebuffer_texture_2d(
        glow::FRAMEBUFFER,
        glow::COLOR_ATTACHMENT0,
        glow::TEXTURE_2D,
        Some(texture),
        0,
    );
    ensure!(
        gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE,
        "Incomplete test framebuffer"
    );
    gl.viewport(0, 0, 4, 4);
    gl.disable(glow::SCISSOR_TEST);
    gl.disable(glow::FRAMEBUFFER_SRGB);
    gl.front_face(glow::CCW);
    gl.color_mask(true, true, true, true);
    gl.clear_color(0., 0., 0., 0.);
    gl.clear(glow::COLOR_BUFFER_BIT);
    let identity = [
        1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
    ];
    let mut vertices = Vec::new();
    for position in [[-1., -1., 0.], [3., -1., 0.], [-1., 3., 0.]] {
        vertices.extend(position);
        vertices.extend([0., 0., 1., 0., 0., 0.2, 0.4, 0.6, 0.8]);
    }
    let source = Arc::new(super::material_preview::compile(
        &super::shader::new_shader(),
        false,
    )?);
    let command = GpuDrawCommand {
        shader_time:None,source_attributes: None,
        extra_uvs: None,
        projection_bound: None,
        projected_shadow: false,
        skybox: false,
        material_registers: None,
        material_indirect: None,
        material_images: Arc::new(vec![]),
        baked: false,
        baked_error: None,
        preview: Some(Ok(source.clone())),
        uv_transform: None,
        material: None,
        sampler: Default::default(),
        v_data: Arc::new(vertices),
        indices: Arc::new(vec![0, 1, 2]),
        trans_mat: identity,
        color: [1.; 4],
        tex_bytes: None,
        aabb_min: [-1.; 3],
        aabb_max: [3.; 3],
        light_group: Arc::from(""),
    };
    ensure!(
        cache.draw_material(gl, &source, &command, &identity, &identity),
        "Material GPU draw failed"
    );
    let mut pixel = [0u8; 4];
    gl.read_pixels(
        1,
        1,
        1,
        1,
        glow::RGBA,
        glow::UNSIGNED_BYTE,
        glow::PixelPackData::Slice(Some(&mut pixel)),
    );
    let mut lightmap_pixels = Vec::new();
    if let Ok(text) =
        std::fs::read_to_string("target/shader-audit/GEN_Diffuse_VertexColor_Lightmap.r3mt")
    {
        let lightmap = Arc::new(super::material_preview::compile(&text, true)?);
        ensure!(
            lightmap.texture_slots == vec![0, 1],
            "Lightmap texture inputs were lost"
        );
        let mut command = command.clone();
        for (map, expected) in [
            ([255u8, 255, 255, 255], [51u8, 51, 38, 255]),
            ([128u8, 128, 128, 128], [10u8, 20, 38, 255]),
        ] {
            command.material_images = Arc::new(
                vec![[128u8, 64, 32, 255], map]
                    .into_iter()
                    .enumerate()
                    .map(|(slot, rgba)| super::material_preview::MaterialImage {
                        slot: slot as u8,
                        rgba: Arc::new(rgba.to_vec()),
                        width: 1,
                        height: 1,
                        sampler: Default::default(),
                        matrix: [1., 0., 0., 0., 1., 0., 0., 0., 1.],
                    })
                    .collect(),
            );
            ensure!(
                cache.draw_material(gl, &lightmap, &command, &identity, &identity),
                "Lightmap GPU draw failed"
            );
            let mut actual = [0u8; 4];
            gl.read_pixels(
                1,
                1,
                1,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut actual)),
            );
            ensure!(
                actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1),
                "Incorrect lightmap pixel: {actual:?}, expected {expected:?}"
            );
            lightmap_pixels.push(actual);
        }
        // One GPU image, two independently tiled inputs: sampler state must not leak.
        let pixels = Arc::new(vec![255u8, 255, 255, 255, 128, 64, 32, 255]);
        let mut vertices = command.v_data.as_ref().clone();
        for vertex in vertices.chunks_exact_mut(12) {
            vertex[6] = 1.25;
        }
        command.v_data = Arc::new(vertices);
        command.material_images = Arc::new(
            (0..2)
                .map(|slot| super::material_preview::MaterialImage {
                    slot,
                    rgba: pixels.clone(),
                    width: 2,
                    height: 1,
                    sampler: TextureSampler {
                        wrap_u: if slot == 0 {
                            glow::CLAMP_TO_EDGE
                        } else {
                            glow::REPEAT
                        },
                        wrap_v: glow::CLAMP_TO_EDGE,
                        min_filter: glow::NEAREST,
                        mag_filter: glow::NEAREST,
                    },
                    matrix: [1., 0., 0., 0., 1., 0., 0., 0., 1.],
                })
                .collect::<Vec<_>>(),
        );
        ensure!(
            cache.draw_material(gl, &lightmap, &command, &identity, &identity),
            "Shared-texture sampler draw failed"
        );
        let mut actual = [0u8; 4];
        gl.read_pixels(
            1,
            1,
            1,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut actual)),
        );
        ensure!(
            actual
                .iter()
                .zip([51u8, 51, 38, 255])
                .all(|(a, b)| a.abs_diff(b) <= 1),
            "Sampler state leaked between inputs: {actual:?}"
        );
        lightmap_pixels.push(actual);
    }
    let mut generated_pixels = Vec::new();
    for (method, view, expected) in [
        ("Environ", identity, [0u8, 255, 0, 204]),
        (
            "Environ",
            [
                -1., 0., 0., 0., 0., -1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
            ],
            [0u8, 0, 255, 204],
        ),
        ("Projection", identity, [255u8, 255, 255, 204]),
    ] {
        let map=format!("<TextureMaps><TextureMap ImageName=\"ValidationTexture\"><TexCoordGen Method=\"{method}\" CameraIndex=\"-1\" UVNumber=\"0\" ScaleS=\"1\" ScaleT=\"1\" Rotate=\"0\" TransS=\"0\" TransT=\"0\"/></TextureMap></TextureMaps>");
        let text = super::shader::new_shader()
            .replace("<TextureMaps Applyable=\"true\"/>", &map)
            .replace(
                "TexMapOrder=\"-1\" TexCoordOrder=\"-1\"",
                "TexMapOrder=\"0\" TexCoordOrder=\"0\"",
            )
            .replace("A=\"RasC\"", "A=\"TexC\"");
        let generated = Arc::new(super::material_preview::compile(&text, false)?);
        let mut cmd = command.clone();
        let mut vertices = cmd.v_data.as_ref().clone();
        for v in vertices.chunks_exact_mut(12) {
            v[3] = 0.5;
            v[4] = 0.5;
            v[5] = 0.5f32.sqrt();
        }
        let mut projection = identity;
        if method == "Projection" {
            for (v, p) in vertices.chunks_exact_mut(12).zip([
                [-1., -1., -1.],
                [6., -2., -2.],
                [-4., 12., -4.],
            ]) {
                v[..3].copy_from_slice(&p);
            }
            projection = [
                1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 0., -1., 0., 0., 0., 0.,
            ];
        }
        cmd.v_data = Arc::new(vertices);
        cmd.material_images = Arc::new(vec![super::material_preview::MaterialImage {
            slot: 0,
            rgba: Arc::new(vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ]),
            width: 2,
            height: 2,
            sampler: TextureSampler {
                wrap_u: glow::CLAMP_TO_EDGE,
                wrap_v: glow::CLAMP_TO_EDGE,
                min_filter: glow::NEAREST,
                mag_filter: glow::NEAREST,
            },
            matrix: [1., 0., 0., 0., 1., 0., 0., 0., 1.],
        }]);
        ensure!(
            cache.draw_material(gl, &generated, &cmd, &projection, &view),
            "Generated-coordinate draw failed"
        );
        let mut actual = [0u8; 4];
        gl.read_pixels(
            if method == "Projection" { 2 } else { 1 },
            1,
            1,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut actual)),
        );
        ensure!(
            actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1),
            "Incorrect {method} pixel {actual:?}; expected {expected:?}"
        );
        if method == "Projection" {
            // This pixel distinguishes homogeneous division from both omitted
            // division and prematurely divided, perspective-interpolated UVs.
            let mut check = [0u8; 4];
            gl.read_pixels(
                2,
                2,
                1,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut check)),
            );
            ensure!(
                check
                    .iter()
                    .zip([0u8, 255, 0, 204])
                    .all(|(a, b)| a.abs_diff(b) <= 1),
                "Projection lost its homogeneous coordinate: {check:?}"
            );
        }
        generated_pixels.push(actual);
    }
    let mut clip_visibility = Vec::new();
    for (name, points, negative_w, expected_rejected, expected_visible) in [
        (
            "inside",
            [[-0.8, -0.8, 0.], [0.8, -0.8, 0.], [0., 0.8, 0.]],
            false,
            false,
            true,
        ),
        (
            "left",
            [[-3., -0.8, 0.], [-2., -0.8, 0.], [-2., 0.8, 0.]],
            false,
            true,
            false,
        ),
        (
            "right",
            [[2., -0.8, 0.], [3., -0.8, 0.], [2., 0.8, 0.]],
            false,
            true,
            false,
        ),
        (
            "below",
            [[-0.8, -3., 0.], [0.8, -3., 0.], [0., -2., 0.]],
            false,
            true,
            false,
        ),
        (
            "above",
            [[-0.8, 2., 0.], [0.8, 2., 0.], [0., 3., 0.]],
            false,
            true,
            false,
        ),
        (
            "crossing",
            [[-4., -4., 0.], [4., -4., 0.], [0., 4., 0.]],
            false,
            false,
            true,
        ),
        (
            "boundary",
            [[1., -0.8, 0.], [1., 0.8, 0.], [1., 0., 0.]],
            false,
            false,
            false,
        ),
        (
            "negative_w",
            [[-0.8, -0.8, 0.], [0.8, -0.8, 0.], [0., 0.8, 0.]],
            true,
            true,
            false,
        ),
        (
            "depth_only",
            [[-0.8, -0.8, 100.], [0.8, -0.8, 100.], [0., 0.8, 100.]],
            false,
            false,
            false,
        ),
    ] {
        let mut test = command.clone();
        let mut vertices = Vec::new();
        for position in points {
            vertices.extend(position);
            vertices.extend([0., 0., 1., 0., 0., 0.2, 0.4, 0.6, 0.8]);
        }
        test.v_data = Arc::new(vertices);
        let mut mvp = identity;
        if negative_w {
            mvp[15] = -1.;
        }
        let rejected = super::clip_visibility::Bounds::from_interleaved(&test.v_data)
            .unwrap()
            .outside(&mvp);
        ensure!(
            rejected == expected_rejected,
            "Unexpected clip classification for {name}"
        );
        gl.clear(glow::COLOR_BUFFER_BIT);
        ensure!(
            cache.draw_material(gl, &source, &test, &mvp, &identity),
            "Clip GPU draw failed: {name}"
        );
        let mut pixels = [0u8; 64];
        gl.read_pixels(
            0,
            0,
            4,
            4,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut pixels)),
        );
        let visible = pixels
            .chunks_exact(4)
            .any(|pixel| pixel.iter().any(|v| *v != 0));
        ensure!(
            !rejected || !visible,
            "Rejected geometry rendered pixels: {name}"
        );
        ensure!(
            !expected_visible || visible,
            "Control geometry rendered no pixels: {name}"
        );
        clip_visibility
            .push(serde_json::json!({"case":name,"rejected":rejected,"rendered_pixels":visible}));
    }
    let error = gl.get_error();
    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
    ensure!(error == glow::NO_ERROR, "OpenGL error {error:#x}");
    ensure!(
        pixel
            .iter()
            .zip([51u8, 102, 153, 204])
            .all(|(a, b)| a.abs_diff(b) <= 1),
        "Incorrect material pixel: {pixel:?}"
    );
    // Exercise the real retained target and receiver shader. These expected
    // RGBA8 values follow the recorded I4 level119 and native stage operands.
    let shadow_pass = super::model_shadow::Pass::new(gl).map_err(anyhow::Error::msg)?;
    let mut shadow_frame = super::model_shadow::Frame {
        view_projection: identity,
        projection: [0., 0., 0., 0.5, 0., 0., 0., 0.5, 0., 0., 0., 0.],
        clip: [0., 0., 0., 0.5, 0., 0., 0., 0.5, 0., 0., 0., 0.],
        casters: vec![],
    };
    shadow_pass.begin(gl, &shadow_frame);
    shadow_pass.model(gl, &identity);
    let mesh = cache
        .get_or_upload_mesh(gl, &command.v_data, &command.indices)
        .context("Projector fixture mesh")?;
    gl.bind_vertex_array(Some(mesh.vao));
    gl.draw_elements(glow::TRIANGLES, mesh.index_count, glow::UNSIGNED_INT, 0);
    let mut mask_pixel = [0u8; 4];
    gl.read_pixels(
        128,
        128,
        1,
        1,
        glow::RED,
        glow::UNSIGNED_BYTE,
        glow::PixelPackData::Slice(Some(&mut mask_pixel)),
    );
    ensure!(
        mask_pixel[0] == 119,
        "I4 projector level differs: {mask_pixel:?}"
    );
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
    gl.viewport(0, 0, 4, 4);
    gl.disable(glow::SCISSOR_TEST);
    let mut model_shadow_pixels = Vec::new();
    for (index, expected) in [[24u8, 47, 72, 204], [51, 102, 153, 204], [0, 0, 0, 0]]
        .into_iter()
        .enumerate()
    {
        if index == 1 {
            shadow_frame.clip[3] = 0.;
        }
        let base = if index == 2 {
            Arc::new(super::material_preview::compile(
                &super::shader::new_shader().replace("OutReg=\"Prev\"", "OutReg=\"Reg2\""),
                false,
            )?)
        } else {
            source.clone()
        };
        let projected = Arc::new(base.projected_shadow()?);
        let mut cmd = command.clone();
        cmd.projected_shadow = true;
        let prepared = cache
            .prepare_material_projected(
                gl,
                &projected,
                &cmd,
                &identity,
                &identity,
                Some((&shadow_frame, [shadow_pass.image, shadow_pass.clip_image])),
            )
            .context("Projected receiver GPU resources missing")?;
        cache.draw_prepared_material(gl, &projected, &cmd, &identity, &identity, &prepared);
        let mut rgba = [0u8; 4];
        gl.read_pixels(
            1,
            1,
            1,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut rgba)),
        );
        ensure!(
            rgba == expected,
            "Projected receiver case{index}: {rgba:?}, expected{expected:?}"
        );
        model_shadow_pixels.push(rgba);
    }
    shadow_pass.destroy(gl);
    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
    gl.delete_framebuffer(fbo);
    gl.delete_texture(texture);
    ensure!(
        gl.get_error() == glow::NO_ERROR,
        "Projector GPU validation produced an error"
    );
    let gpu_interval = super::gpu_timing::validate(gl)?;
    std::fs::write(
        "target/material-gpu-validation.json",
        serde_json::to_vec_pretty(
            &serde_json::json!({"renderer":gl.get_parameter_string(glow::RENDERER),"pixel":pixel,"expected":[51,102,153,204],"lightmap_pixels":lightmap_pixels,"generated_pixels":generated_pixels,"clip_visibility":clip_visibility,"projector_i4_level":mask_pixel[0],"projector_pixels":model_shadow_pixels,"projector_original_frame_parity":false,"gpu_interval":gpu_interval}),
        )?,
    )?;
    Ok(())
}

impl GpuViewportCache {
    unsafe fn draw_material(
        &mut self,
        gl: &glow::Context,
        source: &Arc<super::material_preview::Program>,
        cmd: &GpuDrawCommand,
        vp: &[f32; 16],
        view: &[f32; 16],
    ) -> bool {
        let Some(prepared) = self.prepare_material(gl, source, cmd, vp, view) else {
            return false;
        };
        self.draw_prepared_material(gl, source, cmd, vp, view, &prepared);
        true
    }
    unsafe fn prepare_material(
        &mut self,
        gl: &glow::Context,
        source: &Arc<super::material_preview::Program>,
        cmd: &GpuDrawCommand,
        vp: &[f32; 16],
        view: &[f32; 16],
    ) -> Option<PreparedMaterial> {
        self.prepare_material_projected(gl, source, cmd, vp, view, None)
    }
    unsafe fn prepare_material_projected(
        &mut self,
        gl: &glow::Context,
        source: &Arc<super::material_preview::Program>,
        cmd: &GpuDrawCommand,
        vp: &[f32; 16],
        view: &[f32; 16],
        projection: Option<(&super::model_shadow::Frame, [glow::Texture; 2])>,
    ) -> Option<PreparedMaterial> {
        let enhanced = self.surfaces.variant(source,cmd);
        let source = enhanced.as_ref().unwrap_or(source);
        let key = Arc::as_ptr(source) as usize;
        if !self.material_programs.contains_key(&key) {
            let compile = || -> Result<glow::Program, String> {
                let program = gl.create_program()?;
                for (kind, text) in [
                    (glow::VERTEX_SHADER, source.vertex.as_ref()),
                    (glow::FRAGMENT_SHADER, source.fragment.as_str()),
                ] {
                    let shader = match gl.create_shader(kind) {
                        Ok(s) => s,
                        Err(e) => {
                            gl.delete_program(program);
                            return Err(e);
                        }
                    };
                    gl.shader_source(shader, &crate::render_support::shader_source(text));
                    gl.compile_shader(shader);
                    if !gl.get_shader_compile_status(shader) {
                        let error = gl.get_shader_info_log(shader);
                        gl.delete_shader(shader);
                        gl.delete_program(program);
                        return Err(error);
                    }
                    gl.attach_shader(program, shader);
                    gl.delete_shader(shader);
                }
                gl.link_program(program);
                if !gl.get_program_link_status(program) {
                    let error = gl.get_program_info_log(program);
                    gl.delete_program(program);
                    return Err(error);
                }
                Ok(program)
            };
            let compiled=compile();
            if self.surfaces.settings.mode!=0 {if let Err(error)=&compiled {self.surfaces.error=Some(format!("Enhanced material shader: {error}"));}}
            let program = compiled
                .map(|program| MaterialGpuProgram {
                    surface_uniforms: Arc::new(super::surface_lighting::Uniforms::load(gl, program)),
                    shadow_matrix: gl.get_uniform_location(program, "bw_shadow_matrix"),
                    clip_matrix: gl.get_uniform_location(program, "bw_clip_matrix"),
                    shadow_image: gl.get_uniform_location(program, "bw_shadow_image"),
                    clip_image: gl.get_uniform_location(program, "bw_clip_image"),
                    uv_sets: gl.get_uniform_location(program, "u_tex_uv_set[0]"),
                    projection_bound: gl.get_uniform_location(program, "bw_projection_bound"),
                    indirect: gl.get_uniform_location(program, "bw_indirect[0]"),
                    indirect_override: gl.get_uniform_location(program, "bw_indirect_override"),
                    scene_rect: gl.get_uniform_location(program, "bw_scene_rect"),
                    colors: gl.get_uniform_location(program, "bt_colors[0]"),
                    constants: gl.get_uniform_location(program, "bt_constants[0]"),
                    program,
                    model_view: gl.get_uniform_location(program, "u_model_view"),
                    mvp: gl.get_uniform_location(program, "u_mvp"),
                    uv: gl.get_uniform_location(program, "u_tex_uv[0]"),
                    texture: gl.get_uniform_location(program, "u_tex[0]"),
                })
                .map_err(|e| {
                    log::error!("Material preview compilation failed: {e}");
                    eprintln!(
                        "Material preview compilation failed for {:?}: {e}",
                        cmd.material.as_ref().and_then(|m| m.shader.as_deref())
                    );
                })
                .ok();
            self.material_programs
                .insert(key, (Arc::downgrade(source), program));
        }
        let Some(program) = self
            .material_programs
            .get(&key)
            .and_then(|(_, p)| p.clone())
        else {
            return None;
        };
        let mut textures = Vec::new();
        let mut uv_sets = source.uv_sets.map(i32::from);
        if let Some(bindings) = &cmd.material {
            for &slot in &source.uv_textures {
                if let Some(binding) = bindings
                    .textures
                    .iter()
                    .rev()
                    .find(|binding| binding.slot == u32::from(slot))
                {
                    uv_sets[slot as usize] = binding.uv_set as i32;
                }
            }
        }
        for &slot in &source.uv_textures {
            let channel = uv_sets[slot as usize];
            if !(0..8).contains(&channel) {
                return None;
            }
            if channel != 0
                && !cmd
                    .extra_uvs
                    .as_ref()
                    .and_then(|channels| channels.get(&(channel as u32)))
                    .is_some_and(|values| values.len() * 12 == cmd.v_data.len())
            {
                return None;
            }
        }
        let projection_bound = if source.pinned_projection {
            let Some(center) = cmd.projection_bound else {
                return None;
            };
            let Ok(parameters) =
                super::material_preview::projection::bound_parameters(vp, view, center)
            else {
                return None;
            };
            Some(parameters)
        } else {
            None
        };
        for &slot in &source.texture_slots {
            let Some(image) = cmd.material_images.iter().find(|i| i.slot == slot) else {
                return None;
            };
            let Some(texture) =
                self.get_or_upload_texture(gl, &image.rgba, image.width, image.height)
            else {
                return None;
            };
            let sampling = image.sampler.smoothed(self.smooth_textures && source.uv_textures.contains(&slot));
            if !self.material_samplers.contains_key(&sampling) {
                let Ok(sampler) = gl.create_sampler() else {
                    return None;
                };
                for (name, value) in [
                    (glow::TEXTURE_WRAP_S, sampling.wrap_u),
                    (glow::TEXTURE_WRAP_T, sampling.wrap_v),
                    (glow::TEXTURE_MIN_FILTER, sampling.min_filter),
                    (glow::TEXTURE_MAG_FILTER, sampling.mag_filter),
                ] {
                    gl.sampler_parameter_i32(sampler, name, value as i32);
                }
                self.material_samplers.insert(sampling, sampler);
            }
            textures.push((
                slot,
                texture,
                self.material_samplers[&sampling],
                image.matrix,
            ));
        }
        let Some(mesh) = self.get_or_upload_mesh(gl, &cmd.v_data, &cmd.indices) else {
            return None;
        };
        let mut extra_uv_buffers = [None; 7];
        for channel in 1..8 {
            if let Some(values) = cmd
                .extra_uvs
                .as_ref()
                .and_then(|channels| channels.get(&channel))
            {
                let key = Arc::as_ptr(values) as usize;
                if !self.uv_buffers.contains_key(&key) {
                    let buffer = gl.create_buffer().ok()?;
                    gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
                    gl.buffer_data_u8_slice(
                        glow::ARRAY_BUFFER,
                        std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), values.len() * 8),
                        glow::STATIC_DRAW,
                    );
                    self.uv_buffers
                        .insert(key, (Arc::downgrade(values), buffer));
                }
                extra_uv_buffers[channel as usize - 1] = Some(self.uv_buffers[&key].1);
            }
        }
        let projector = if cmd.projected_shadow {
            let (frame, images) = projection?;
            Some((frame.receiver_matrices(&cmd.trans_mat).ok()?, images))
        } else {
            None
        };
        Some(PreparedMaterial {
            projector,
            program,
            mvp: mat4_mul(vp, &cmd.trans_mat),
            textures,
            uv_sets,
            projection_bound,
            mesh,
            extra_uv_buffers,
        })
    }
    unsafe fn draw_prepared_material(
        &mut self,
        gl: &glow::Context,
        source: &super::material_preview::Program,
        cmd: &GpuDrawCommand,
        _vp: &[f32; 16],
        view: &[f32; 16],
        prepared: &PreparedMaterial,
    ) {
        let PreparedMaterial {
            projector,
            program,
            mvp,
            textures,
            uv_sets,
            projection_bound,
            mesh,
            extra_uv_buffers,
        } = prepared;
        gl.use_program(Some(program.program));
        let mut count = 0;
        if let Some(slot) = source.scene_color_slot {
            let unit = u32::from(slot);
            let mut rect = [0; 4];
            gl.get_parameter_i32_slice(glow::VIEWPORT, &mut rect);
            let [x, y, w, h] = rect;
            if w <= 0 || h <= 0 {
                return;
            }
            gl.active_texture(glow::TEXTURE0 + unit);
            gl.bind_sampler(unit, None);
            if self.scene_color.is_none() {
                let Ok(texture) = gl.create_texture() else { return };
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                for (parameter, value) in [
                    (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                    (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                    (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                    (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                ] {
                    gl.tex_parameter_i32(glow::TEXTURE_2D, parameter, value as i32);
                }
                self.scene_color = Some((texture, [0, 0]));
            }
            let (texture, size) = self.scene_color.as_mut().unwrap();
            gl.bind_texture(glow::TEXTURE_2D, Some(*texture));
            if *size == [w, h] {
                gl.copy_tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, x, y, w, h);
            } else {
                gl.copy_tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA, x, y, w, h, 0);
                *size = [w, h];
            }
            gl.uniform_4_f32(program.scene_rect.as_ref(), x as f32, y as f32, w as f32, h as f32);
            count = usize::from(slot) + 1;
        }
        if let Some((matrices, images)) = projector {
            gl.uniform_matrix_4_f32_slice(program.shadow_matrix.as_ref(), false, &matrices[0]);
            gl.uniform_matrix_4_f32_slice(program.clip_matrix.as_ref(), false, &matrices[1]);
            for (unit, image, location) in [
                (8, images[0], program.shadow_image.as_ref()),
                (9, images[1], program.clip_image.as_ref()),
            ] {
                gl.active_texture(glow::TEXTURE0 + unit);
                gl.bind_sampler(unit, None);
                gl.bind_texture(glow::TEXTURE_2D, Some(image));
                gl.uniform_1_i32(location, unit as i32);
            }
        }
        gl.uniform_1_i32(
            program.indirect_override.as_ref(),
            i32::from(cmd.material_indirect.is_some()),
        );
        if let Some(matrices) = cmd.material_indirect.as_ref() {
            gl.uniform_3_i32_slice(program.indirect.as_ref(), matrices.as_flattened());
        }
        if let Some(registers) = source.presentation_registers(cmd.material_registers.as_ref()) {
            gl.uniform_4_f32_slice(program.colors.as_ref(), registers[..3].as_flattened());
            gl.uniform_4_f32_slice(program.constants.as_ref(), registers[3..].as_flattened());
        }
        gl.uniform_matrix_4_f32_slice(program.mvp.as_ref(), false, mvp);
        gl.uniform_1_i32_slice(program.uv_sets.as_ref(), uv_sets);
        if let Some([s, t, depth]) = *projection_bound {
            gl.uniform_3_f32(program.projection_bound.as_ref(), s, t, depth);
        }
        gl.uniform_matrix_4_f32_slice(
            program.model_view.as_ref(),
            false,
            &mat4_mul(view, &cmd.trans_mat),
        );
        let mut matrices = [0f32; 72];
        for &(slot, texture, sampler, matrix) in textures {
            gl.active_texture(glow::TEXTURE0 + slot as u32);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.bind_sampler(slot as u32, Some(sampler));
            let start = slot as usize * 9;
            matrices[start..start + 9].copy_from_slice(&matrix);
            count = count.max(slot as usize + 1);
        }
        if count > 0 {
            gl.uniform_matrix_3_f32_slice(program.uv.as_ref(), false, &matrices[..count * 9]);
            gl.uniform_1_i32_slice(program.texture.as_ref(), &[0, 1, 2, 3, 4, 5, 6, 7][..count]);
        }
        if source.depth_test {
            gl.enable(glow::DEPTH_TEST);
        } else {
            gl.disable(glow::DEPTH_TEST);
        }
        gl.depth_func(source.depth_func);
        gl.depth_mask(source.depth_write);
        if let Some((src, dst, equation)) = source.blend {
            gl.enable(glow::BLEND);
            gl.blend_func(src, dst);
            gl.blend_equation(equation);
        } else {
            gl.disable(glow::BLEND);
        }
        if let Some(face) = source.cull {
            gl.enable(glow::CULL_FACE);
            gl.cull_face(face);
        } else {
            gl.disable(glow::CULL_FACE);
        }
        if self.surfaces.settings.mode!=0 {self.surfaces.bind(gl,&program.surface_uniforms,cmd,self.surfaces.shadow_pass);}
        if self.surfaces.shadow_pass {gl.enable(glow::DEPTH_TEST);gl.depth_mask(true);gl.depth_func(glow::LESS);}
        gl.bind_vertex_array(Some(mesh.vao));
        for (i, buffer) in extra_uv_buffers.iter().enumerate() {
            let attribute = 4 + i as u32;
            if let Some(buffer) = buffer {
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(*buffer));
                gl.enable_vertex_attrib_array(attribute);
                gl.vertex_attrib_pointer_f32(attribute, 2, glow::FLOAT, false, 8, 0);
            } else {
                gl.disable_vertex_attrib_array(attribute);
                gl.vertex_attrib_2_f32(attribute, 0., 0.);
            }
        }
        gl.draw_elements(glow::TRIANGLES, mesh.index_count, glow::UNSIGNED_INT, 0);
        for (slot, _, _, _) in textures {
            gl.bind_sampler(*slot as u32, None);
        }
        gl.active_texture(glow::TEXTURE0);
    }
    pub unsafe fn init(gl: &glow::Context) -> Option<Self> {
        // ── Shadow Map Pass Shaders ──────────────────────────────────────────
        let vs_shadow_src = r#"
            #version 330 core
            layout(location = 0) in vec3 a_pos;
            uniform mat4 u_light_vp;
            uniform mat4 u_model;
            uniform mat3 u_uv_transform;

            void main() {
                gl_Position = u_light_vp * u_model * vec4(a_pos, 1.0);
            }
        "#;
        let fs_shadow_src = r#"
            #version 330 core
            void main() {
                // Depth automatically written
            }
        "#;

        let vs_shadow = gl.create_shader(glow::VERTEX_SHADER).ok()?;
        gl.shader_source(vs_shadow, &crate::render_support::shader_source(vs_shadow_src));
        gl.compile_shader(vs_shadow);

        let fs_shadow = gl.create_shader(glow::FRAGMENT_SHADER).ok()?;
        gl.shader_source(fs_shadow, &crate::render_support::shader_source(fs_shadow_src));
        gl.compile_shader(fs_shadow);

        let shadow_program = gl.create_program().ok()?;
        gl.attach_shader(shadow_program, vs_shadow);
        gl.attach_shader(shadow_program, fs_shadow);
        gl.link_program(shadow_program);
        gl.delete_shader(vs_shadow);
        gl.delete_shader(fs_shadow);

        let u_shadow_light_vp = gl.get_uniform_location(shadow_program, "u_light_vp")?;
        let u_shadow_model = gl.get_uniform_location(shadow_program, "u_model")?;

        // The 2048² legacy shadow target costs roughly 16 MiB of depth storage.
        // Most game scenes use native Wii material programs and never sample it,
        // so allocate it only when a frame actually needs the legacy fallback.
        let (shadow_fbo, shadow_tex) = (None, None);

        // ── Main Mesh Shaders ────────────────────────────────────────────────
        let vs_mesh_src = r#"
            #version 330 core
            layout(location = 0) in vec3 a_pos;
            layout(location = 1) in vec3 a_normal;
            layout(location = 2) in vec2 a_uv;
            layout(location = 3) in vec4 a_color;

            uniform mat4 u_mvp;
            uniform mat4 u_model;
            uniform mat3 u_uv_transform;
            uniform mat4 u_light_vp;

            out vec3 v_world_pos;
            out vec3 v_normal;
            out vec2 v_uv;
            out vec4 v_color;
            out vec4 v_shadow_pos;

            void main() {
                vec4 world_pos = u_model * vec4(a_pos, 1.0);
                v_world_pos = world_pos.xyz;
                mat3 normal_matrix = transpose(inverse(mat3(u_model)));
                v_normal = normal_matrix * a_normal;
                v_uv = (u_uv_transform * vec3(a_uv,1.0)).xy;
                v_color = a_color;
                v_shadow_pos = u_light_vp * world_pos;
                gl_Position = u_mvp * vec4(a_pos, 1.0);
            }
        "#;
        let fs_mesh_src = r#"
            #version 330 core
            in vec3 v_world_pos;
            in vec3 v_normal;
            in vec2 v_uv;
            in vec4 v_color;
            in vec4 v_shadow_pos;

            const int MAX_LIGHTS = 128;
            uniform int u_num_lights;
            uniform vec4 u_lights[MAX_LIGHTS * 6];
            uniform vec3 u_view_pos;
            uniform int u_lighting_mode;

            uniform vec4 u_color;
            uniform sampler2D u_tex;
            uniform bool u_use_tex;
            uniform int u_alpha_function;
            uniform float u_alpha_reference;
            uniform bool u_modulate_vertex_color;
            uniform bool u_baked;
            uniform sampler2D u_shadow_map;

            out vec4 FragColor;

            float calc_shadow(vec4 shadow_pos, vec3 N, vec3 L) {
                vec3 proj = shadow_pos.xyz / shadow_pos.w;
                proj = proj * 0.5 + 0.5;
                if (proj.z > 1.0 || proj.x < 0.0 || proj.x > 1.0 || proj.y < 0.0 || proj.y > 1.0) {
                    return 1.0;
                }
                float current_depth = proj.z;
                float bias = max(0.004 * (1.0 - dot(N, L)), 0.0015);
                float shadow = 0.0;
                vec2 texel_size = vec2(1.0 / 2048.0);
                for (int x = -1; x <= 1; ++x) {
                    for (int y = -1; y <= 1; ++y) {
                        float pcf_depth = texture(u_shadow_map, proj.xy + vec2(x, y) * texel_size).r;
                        shadow += (current_depth - bias) > pcf_depth ? 0.08 : 1.0;
                    }
                }
                return shadow / 9.0;
            }

            void main() {
                vec4 base = u_use_tex ? texture(u_tex, v_uv) : u_color;
                // Preserve authored vertex colour; baked buffers contain instance lighting.

                if (u_use_tex) base.a *= u_color.a;
                if (u_modulate_vertex_color) base *= v_color;
                // NiAlphaProperty controls rejection. Diffuse alpha may carry
                // material data on an opaque surface; it is not an implicit cutout.
                bool alpha_pass = u_alpha_function == 0
                    || (u_alpha_function == 1 && base.a < u_alpha_reference)
                    || (u_alpha_function == 2 && base.a == u_alpha_reference)
                    || (u_alpha_function == 3 && base.a <= u_alpha_reference)
                    || (u_alpha_function == 4 && base.a > u_alpha_reference)
                    || (u_alpha_function == 5 && base.a != u_alpha_reference)
                    || (u_alpha_function == 6 && base.a >= u_alpha_reference);
                if (!alpha_pass) discard;
                if (u_baked || u_lighting_mode == 2) {
                    FragColor = base;
                    return;
                }

                vec3 N = length(v_normal) > 0.001 ? normalize(v_normal) : vec3(0.0, 1.0, 0.0);
                vec3 V = normalize(u_view_pos - v_world_pos);
                vec3 frag_pos = v_world_pos;

                if (u_lighting_mode == 0) {
                    vec3 L = normalize(u_view_pos - v_world_pos);
                    float diff = max(dot(N, L), 0.0);
                    float back_diff = max(dot(-N, L), 0.0) * 0.25;
                    float lighting = 0.35 + 0.65 * (diff + back_diff);
                    FragColor = vec4(clamp(base.rgb * lighting, 0.0, 1.0), base.a);
                    return;
                }

                vec3 total_ambient = vec3(0.0);
                vec3 total_diffuse = vec3(0.0);

                for (int i = 0; i < u_num_lights && i < MAX_LIGHTS; ++i) {
                    vec4 v0 = u_lights[i * 6 + 0];
                    vec4 v1 = u_lights[i * 6 + 1];
                    vec4 v2 = u_lights[i * 6 + 2];
                    vec4 v3 = u_lights[i * 6 + 3];

                    vec3 light_pos = v0.xyz;
                    int light_type = int(v0.w + 0.5);

                    vec3 light_dir = v1.xyz;
                    float dimmer = v1.w;

                    vec3 diff_color = v2.rgb;
                    float range = v2.w;

                    vec3 amb_color = v3.rgb;

                    vec4 falloff=u_lights[i*6+4];
                    vec4 spot=u_lights[i*6+5];
                    float atten = 1.0;
                    vec3 L = vec3(0.0);

                    if (light_type == 0) { // Ambient
                        total_ambient += dimmer * amb_color;
                        continue;
                    } else if (light_type == 1) { // Directional
                        L = normalize(-light_dir);
                        total_ambient += dimmer * amb_color;
                    } else if (light_type == 2 || light_type == 3) { // Point or Spot
                        vec3 dir = light_pos - frag_pos;
                        float dist = length(dir);

                        float denominator=falloff.x+falloff.y*dist+falloff.z*dist*dist;
                        atten=denominator>0.000001 ? 1.0/denominator : 1.0;
                        if(range>0.0 && dist>range)atten=0.0;

                        L = dir / max(0.0001, dist);

                        if (light_type == 3) { // Spot
                            float cos_alpha = dot(-L, normalize(light_dir));
                            cos_alpha = clamp((cos_alpha - spot.y) / max(0.000001, spot.x - spot.y), 0.0, 1.0);
                            atten *= pow(cos_alpha, max(0.000001,spot.z));
                        }

                        total_ambient += (dimmer * amb_color) * atten;
                    }

                    // Direct Lambertian NdotL for sharp 3D shadow falloff
                    float NdotL = max(0.0, dot(L, N));

                    float shadow_factor = 1.0;
                    if (i == 0 || light_type == 1) {
                        shadow_factor = calc_shadow(v_shadow_pos, N, L);
                    }

                    float light_weight = 1.0;
                    vec3 diff_contrib = (dimmer * light_weight * diff_color) * NdotL * atten * shadow_factor;
                    total_diffuse += diff_contrib;
                }

                // Preserve coloured vertex lighting. No invented AO, ambient floor,
                // or tone curve: these belong to the material, not the viewport.
                FragColor = vec4(clamp(base.rgb*(total_ambient+total_diffuse),0.0,1.0),base.a);
            }
        "#;

        let vs_mesh = gl.create_shader(glow::VERTEX_SHADER).ok()?;
        gl.shader_source(vs_mesh, &crate::render_support::shader_source(vs_mesh_src));
        gl.compile_shader(vs_mesh);
        if !gl.get_shader_compile_status(vs_mesh) {
            log::error!("Mesh VS compile error: {}", gl.get_shader_info_log(vs_mesh));
        }

        let fs_mesh = gl.create_shader(glow::FRAGMENT_SHADER).ok()?;
        let fs_mesh_src=super::surface_lighting::legacy_fragment(fs_mesh_src);
        gl.shader_source(fs_mesh, &crate::render_support::shader_source(&fs_mesh_src));
        gl.compile_shader(fs_mesh);
        if !gl.get_shader_compile_status(fs_mesh) {
            log::error!("Mesh FS compile error: {}", gl.get_shader_info_log(fs_mesh));
        }

        let mesh_program = gl.create_program().ok()?;
        gl.attach_shader(mesh_program, vs_mesh);
        gl.attach_shader(mesh_program, fs_mesh);
        gl.link_program(mesh_program);
        if !gl.get_program_link_status(mesh_program) {
            log::error!(
                "Mesh Program link error: {}",
                gl.get_program_info_log(mesh_program)
            );
        }
        gl.delete_shader(vs_mesh);
        gl.delete_shader(fs_mesh);

        let u_mvp = gl.get_uniform_location(mesh_program, "u_mvp")?;
        let u_model = gl.get_uniform_location(mesh_program, "u_model")?;
        let u_color = gl.get_uniform_location(mesh_program, "u_color")?;
        let u_use_tex = gl.get_uniform_location(mesh_program, "u_use_tex")?;
        let u_alpha_function = gl.get_uniform_location(mesh_program, "u_alpha_function")?;
        let u_alpha_reference = gl.get_uniform_location(mesh_program, "u_alpha_reference")?;
        let u_modulate_vertex_color = gl.get_uniform_location(mesh_program, "u_modulate_vertex_color")?;
        let u_baked = gl.get_uniform_location(mesh_program, "u_baked")?;
        let u_uv_transform = gl.get_uniform_location(mesh_program, "u_uv_transform")?;
        let u_view_pos = gl.get_uniform_location(mesh_program, "u_view_pos")?;
        let u_num_lights = gl.get_uniform_location(mesh_program, "u_num_lights")?;
        let u_lights_loc = gl.get_uniform_location(mesh_program, "u_lights[0]")?;
        let u_lighting_mode = gl.get_uniform_location(mesh_program, "u_lighting_mode")?;
        let u_mesh_light_vp = gl.get_uniform_location(mesh_program, "u_light_vp")?;
        let u_shadow_map_loc = gl.get_uniform_location(mesh_program, "u_shadow_map")?;

        let vs_line_src = r#"
            #version 330 core
            layout(location = 0) in vec3 a_pos;
            layout(location = 1) in vec4 a_color;

            uniform mat4 u_vp;
            out vec4 v_color;

            void main() {
                v_color = a_color;
                gl_Position = u_vp * vec4(a_pos, 1.0);
            }
        "#;
        let fs_line_src = r#"
            #version 330 core
            in vec4 v_color;
            out vec4 FragColor;

            void main() {
                FragColor = v_color;
            }
        "#;

        let vs_line = gl.create_shader(glow::VERTEX_SHADER).ok()?;
        gl.shader_source(vs_line, &crate::render_support::shader_source(vs_line_src));
        gl.compile_shader(vs_line);

        let fs_line = gl.create_shader(glow::FRAGMENT_SHADER).ok()?;
        gl.shader_source(fs_line, &crate::render_support::shader_source(fs_line_src));
        gl.compile_shader(fs_line);

        let line_program = gl.create_program().ok()?;
        gl.attach_shader(line_program, vs_line);
        gl.attach_shader(line_program, fs_line);
        gl.link_program(line_program);
        gl.delete_shader(vs_line);
        gl.delete_shader(fs_line);

        let line_u_vp = gl.get_uniform_location(line_program, "u_vp")?;

        let line_vao = gl.create_vertex_array().ok()?;
        let line_vbo = gl.create_buffer().ok()?;
        gl.bind_vertex_array(Some(line_vao));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(line_vbo));
        let line_stride = (7 * std::mem::size_of::<f32>()) as i32;
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, line_stride, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(1, 4, glow::FLOAT, false, line_stride, 3 * 4);
        gl.bind_vertex_array(None);

        let grid_program = line_program;
        let grid_u_vp = line_u_vp;

        let grid_size = 5.0f32;
        let range = 40;
        let step = grid_size.max(0.5);
        let extent = range as f32 * step;
        let mut grid_verts: Vec<f32> = Vec::new();

        for i in -range..=range {
            let fi = i as f32 * step;
            let (r, g, b, a) = if i == 0 {
                (0.2, 0.2, 0.9, 0.8)
            } else if i % 5 == 0 {
                (0.6, 0.6, 0.6, 0.35)
            } else {
                (0.4, 0.4, 0.4, 0.15)
            };

            grid_verts.extend_from_slice(&[fi, 0.0, -extent, r, g, b, a]);
            grid_verts.extend_from_slice(&[fi, 0.0, extent, r, g, b, a]);

            let (r, g, b, a) = if i == 0 {
                (0.9, 0.2, 0.2, 0.8)
            } else if i % 5 == 0 {
                (0.6, 0.6, 0.6, 0.35)
            } else {
                (0.4, 0.4, 0.4, 0.15)
            };

            grid_verts.extend_from_slice(&[-extent, 0.0, fi, r, g, b, a]);
            grid_verts.extend_from_slice(&[extent, 0.0, fi, r, g, b, a]);
        }
        grid_verts.extend_from_slice(&[0.0, -extent, 0.0, 0.2, 0.9, 0.2, 0.8]);
        grid_verts.extend_from_slice(&[0.0, extent, 0.0, 0.2, 0.9, 0.2, 0.8]);
        let grid_vert_count = (grid_verts.len() / 7) as i32;

        let grid_vao = gl.create_vertex_array().ok()?;
        let grid_vbo = gl.create_buffer().ok()?;
        gl.bind_vertex_array(Some(grid_vao));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(grid_vbo));
        let grid_byte_slice =
            std::slice::from_raw_parts(grid_verts.as_ptr() as *const u8, grid_verts.len() * 4);
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, grid_byte_slice, glow::STATIC_DRAW);
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, line_stride, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(1, 4, glow::FLOAT, false, line_stride, 3 * 4);
        gl.bind_vertex_array(None);

        Some(Self {
            smooth_textures: false,
            surfaces: Default::default(),
            contact_pass:None,
            finish_pass: None,
            em2_post: None,
            scene_color: None,
            gpu_timing: Default::default(),
            model_shadow_pass: None,
            volume_shadow_pass: None,
            mesh_program,
            mesh_surface_uniforms: super::surface_lighting::Uniforms::load(gl, mesh_program),
            u_mvp,
            u_model,
            u_color,
            u_use_tex,
            u_alpha_function,
            u_alpha_reference,
            u_modulate_vertex_color,
            u_baked,
            u_uv_transform,
            u_view_pos,
            u_num_lights,
            u_lights_loc,
            u_lighting_mode,
            u_mesh_light_vp,
            u_shadow_map_loc,

            shadow_fbo,
            shadow_age: 0,
            shadow_vp: None,
            shadow_tex,
            shadow_program,
            u_shadow_light_vp,
            u_shadow_model,

            line_program,
            line_u_vp,
            line_vao,
            line_vbo,

            grid_program,
            grid_u_vp,
            grid_vao,
            grid_vbo,
            grid_vert_count,

            material_samplers: HashMap::new(),
            material_programs: HashMap::new(),
            uv_buffers: HashMap::new(),
            invalidated: false,
            meshes: HashMap::new(),
            index_buffers: HashMap::new(),
            vertex_buffer_uploads: 0,
            index_buffer_uploads: 0,
            textures: HashMap::new(),
        })
    }

    /// Run only while the owning GL context is current. Weak owners prevent
    /// allocator-address reuse without retaining discarded CPU asset buffers.
    unsafe fn collect_unused(&mut self, gl: &glow::Context) {
        let invalidated = self.invalidated;
        self.uv_buffers.retain(|_, (owner, buffer)| {
            if !invalidated && owner.strong_count() > 0 {
                true
            } else {
                gl.delete_buffer(*buffer);
                false
            }
        });
        self.meshes.retain(|_, (vertices, indices, handle)| {
            if !invalidated && vertices.strong_count() > 0 && indices.strong_count() > 0 {
                return true;
            }
            gl.delete_vertex_array(handle.vao);
            gl.delete_buffer(handle.vbo);
            false
        });
        self.index_buffers.retain(|_, indices| {
            if !invalidated && indices.owner.strong_count() > 0 {
                return true;
            }
            gl.delete_buffer(indices.buffer);
            false
        });
        self.textures.retain(|_, (pixels, texture, _, _)| {
            if !invalidated && pixels.strong_count() > 0 {
                return true;
            }
            gl.delete_texture(*texture);
            false
        });
        self.surfaces.collect(invalidated);
        self.material_programs.retain(|_, (source, program)| {
            if !invalidated && source.strong_count() > 0 {
                return true;
            }
            if let Some(program) = program {
                gl.delete_program(program.program);
            }
            false
        });
        if invalidated {
            self.surfaces.destroy(gl);
            if let Some(Ok(pass))=self.contact_pass.take(){pass.destroy(gl);}
            self.gpu_timing.destroy(gl);
            if let Some(Ok(pass)) = self.model_shadow_pass.take() {
                pass.destroy(gl);
            }
            for (_, sampler) in self.material_samplers.drain() {
                gl.delete_sampler(sampler);
            }
        }
        self.invalidated = false;
    }

    unsafe fn ensure_legacy_shadow_target(&mut self, gl: &glow::Context) {
        if self.shadow_fbo.is_some() && self.shadow_tex.is_some() {
            return;
        }
        let Ok(texture) = gl.create_texture() else {
            return;
        };
        let Ok(framebuffer) = gl.create_framebuffer() else {
            gl.delete_texture(texture);
            return;
        };
        let previous_framebuffer =
            std::num::NonZeroU32::new(gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING) as u32)
                .map(glow::NativeFramebuffer);
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::DEPTH_COMPONENT24 as i32,
            2048,
            2048,
            0,
            glow::DEPTH_COMPONENT,
            glow::FLOAT,
            glow::PixelUnpackData::Slice(None),
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_BORDER as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_BORDER as i32,
        );
        gl.tex_parameter_f32_slice(
            glow::TEXTURE_2D,
            glow::TEXTURE_BORDER_COLOR,
            &[1.0, 1.0, 1.0, 1.0],
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::DEPTH_ATTACHMENT,
            glow::TEXTURE_2D,
            Some(texture),
            0,
        );
        gl.draw_buffer(glow::NONE);
        gl.read_buffer(glow::NONE);
        let complete = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
        gl.bind_framebuffer(glow::FRAMEBUFFER, previous_framebuffer);
        if complete {
            self.shadow_fbo = Some(framebuffer);
            self.shadow_tex = Some(texture);
        } else {
            gl.delete_framebuffer(framebuffer);
            gl.delete_texture(texture);
        }
    }

    pub unsafe fn get_or_upload_mesh(
        &mut self,
        gl: &glow::Context,
        v_data: &Arc<Vec<f32>>,
        indices: &Arc<Vec<u32>>,
    ) -> Option<GpuMeshHandle> {
        let key = (Arc::as_ptr(v_data) as usize, Arc::as_ptr(indices) as usize);
        if let Some((_, _, handle)) = self.meshes.get(&key) {
            return Some(*handle);
        }

        let vao = gl.create_vertex_array().ok()?;
        let vbo = gl.create_buffer().ok()?;
        gl.bind_vertex_array(Some(vao));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        let v_bytes = std::slice::from_raw_parts(v_data.as_ptr() as *const u8, v_data.len() * 4);
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, v_bytes, glow::STATIC_DRAW);
        self.vertex_buffer_uploads += 1;

        let mesh_stride = (12 * std::mem::size_of::<f32>()) as i32;
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, mesh_stride, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, mesh_stride, 3 * 4);
        gl.enable_vertex_attrib_array(2);
        gl.vertex_attrib_pointer_f32(2, 2, glow::FLOAT, false, mesh_stride, 6 * 4);
        gl.enable_vertex_attrib_array(3);
        gl.vertex_attrib_pointer_f32(3, 4, glow::FLOAT, false, mesh_stride, 8 * 4);

        let (ebo, max_index) = if let Some(cached) = self.index_buffers.get(&key.1) {
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(cached.buffer));
            (cached.buffer, cached.max_index)
        } else {
            let Ok(buffer) = gl.create_buffer() else {
                gl.bind_vertex_array(None);
                gl.delete_vertex_array(vao);
                gl.delete_buffer(vbo);
                return None;
            };
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(buffer));
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, bytemuck::cast_slice(indices), glow::STATIC_DRAW);
            let max_index = indices.iter().copied().max();
            self.index_buffers.insert(key.1, IndexBuffer {
                owner: Arc::downgrade(indices), buffer, max_index,
            });
            self.index_buffer_uploads += 1;
            (buffer, max_index)
        };
        gl.bind_vertex_array(None);

        let handle = GpuMeshHandle {
            vao,
            vbo,
            ebo,
            index_count: indices.len() as i32,
            bounds: if indices.len() <= i32::MAX as usize
                && max_index.is_none_or(|index| (index as usize) < v_data.len() / 12)
            {
                super::clip_visibility::Bounds::from_interleaved(v_data)
            } else {
                None
            },
        };
        self.meshes.insert(
            key,
            (Arc::downgrade(v_data), Arc::downgrade(indices), handle),
        );
        Some(handle)
    }

    pub unsafe fn get_or_upload_texture(
        &mut self,
        gl: &glow::Context,
        rgba: &Arc<Vec<u8>>,
        w: u32,
        h: u32,
    ) -> Option<glow::NativeTexture> {
        let key = (Arc::as_ptr(rgba) as usize, w, h);
        if let Some((_, tex, _, _)) = self.textures.get(&key) {
            return Some(*tex);
        }
        let tex = upload_gpu_texture(gl, rgba, w, h)?;
        let (_, storage_bytes, savings_bytes) = texture_storage_estimate(rgba, w, h);
        self.textures.insert(key, (Arc::downgrade(rgba), tex, storage_bytes, savings_bytes));
        Some(tex)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Unified 3D Viewport Navigation & Hardware Render Execution
// ─────────────────────────────────────────────────────────────────────────────

pub fn update_and_render_viewport(
    ui: &mut Ui,
    vp: &mut Viewport3DState,
    draw_commands: Vec<GpuDrawCommand>,
    line_commands: Vec<GpuLineSegment>,
    lights: Vec<GpuLight>,
) -> (Rect, Response, [f32; 16], [f32; 3]) {
    render_viewport(ui, vp, draw_commands, line_commands, lights, true)
}

pub(crate) fn render_viewport(
    ui: &mut Ui,
    vp: &mut Viewport3DState,
    draw_commands: Vec<GpuDrawCommand>,
    line_commands: Vec<GpuLineSegment>,
    lights: Vec<GpuLight>,
    editor_navigation: bool,
) -> (Rect, Response, [f32; 16], [f32; 3]) {
    let mut limitations = std::collections::BTreeSet::new();
    for cmd in &draw_commands {
        if let Some(Err(reason)) = &cmd.preview {
            limitations.insert(reason.as_str());
        }
        if let Some(reason) = &cmd.baked_error {
            limitations.insert(reason.as_str());
        }
    }
    let overlay_warning = if vp.show_limitations && vp.limitations_overlay && !limitations.is_empty() {
        Some((
            format!("{} preview limitations", limitations.len()),
            limitations.iter().copied().collect::<Vec<_>>().join("\n"),
        ))
    } else {
        None
    };
    if vp.show_limitations && !vp.limitations_overlay && !limitations.is_empty() {
        ui.label(
            egui::RichText::new(format!("{} preview limitations", limitations.len()))
                .small()
                .color(egui::Color32::GOLD),
        )
        .on_hover_text(limitations.into_iter().collect::<Vec<_>>().join("\n"));
    }
    ui.ctx().request_repaint();
    let available = ui.available_size();
    let (rect, response) = ui.allocate_at_least(available, Sense::click_and_drag());

    // ── Camera Vectors ────────────────────────────────────────────────────────
    let cos_p = vp.pitch.cos();
    let sin_p = vp.pitch.sin();
    let cos_y = vp.yaw.cos();
    let sin_y = vp.yaw.sin();

    // Camera direction vectors in world space
    let forward = [-cos_y * cos_p, -sin_p, -sin_y * cos_p];
    let right = [sin_y, 0.0, -cos_y];
    let up = [-cos_y * sin_p, cos_p, -sin_y * sin_p];

    // ── Mouse Navigation Controls ─────────────────────────────────────────────
    if editor_navigation && (response.hovered() || response.dragged()) {
        // Orbit (Right-click or Left-click drag without Shift)
        if response.dragged_by(egui::PointerButton::Secondary)
            || (vp.primary_drag_orbits && response.dragged_by(egui::PointerButton::Primary))
        {
            let delta = response.drag_delta();
            if ui.input(|i| i.modifiers.shift) {
                // Pan: Shift + Drag along screen Right/Up vectors
                let speed = vp.distance * 0.0015;
                vp.camera_target[0] -= (right[0] * delta.x - up[0] * delta.y) * speed;
                vp.camera_target[1] -= (right[1] * delta.x - up[1] * delta.y) * speed;
                vp.camera_target[2] -= (right[2] * delta.x - up[2] * delta.y) * speed;
            } else {
                vp.yaw += delta.x * 0.005;
                vp.pitch = (vp.pitch + delta.y * 0.005).clamp(-1.55, 1.55);
            }
            ui.ctx().request_repaint();
        }

        // Pan: Middle-click drag along screen Right/Up vectors
        if response.dragged_by(egui::PointerButton::Middle) {
            let delta = response.drag_delta();
            let speed = vp.distance * 0.0015;
            vp.camera_target[0] -= (right[0] * delta.x - up[0] * delta.y) * speed;
            vp.camera_target[1] -= (right[1] * delta.x - up[1] * delta.y) * speed;
            vp.camera_target[2] -= (right[2] * delta.x - up[2] * delta.y) * speed;
            ui.ctx().request_repaint();
        }

        // Zoom: Scroll wheel
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            vp.distance = (vp.distance * (1.0 - scroll * 0.002)).clamp(0.1, 50000.0);
            ui.ctx().request_repaint();
        }

        // WASD Fly Mode (when right-click held without Shift)
        if response.dragged_by(egui::PointerButton::Secondary) && !ui.input(|i| i.modifiers.shift) {
            let speed = vp.distance
                * 0.02
                * if ui.input(|i| i.modifiers.alt) {
                    3.0
                } else {
                    1.0
                };
            let mut moved = false;
            if ui.input(|i| i.key_down(egui::Key::W)) {
                vp.camera_target[0] += forward[0] * speed;
                vp.camera_target[1] += forward[1] * speed;
                vp.camera_target[2] += forward[2] * speed;
                moved = true;
            }
            if ui.input(|i| i.key_down(egui::Key::S)) {
                vp.camera_target[0] -= forward[0] * speed;
                vp.camera_target[1] -= forward[1] * speed;
                vp.camera_target[2] -= forward[2] * speed;
                moved = true;
            }
            if ui.input(|i| i.key_down(egui::Key::A)) {
                vp.camera_target[0] -= right[0] * speed;
                vp.camera_target[1] -= right[1] * speed;
                vp.camera_target[2] -= right[2] * speed;
                moved = true;
            }
            if ui.input(|i| i.key_down(egui::Key::D)) {
                vp.camera_target[0] += right[0] * speed;
                vp.camera_target[1] += right[1] * speed;
                vp.camera_target[2] += right[2] * speed;
                moved = true;
            }
            if moved {
                ui.ctx().request_repaint();
            }
        }

        if response.dragged() || vp.is_dragging_gizmo {
            ui.ctx().request_repaint();
        }
    }

    // ── Eye Position & Light Space Shadow Matrix ─────────────────────────────
    let eye = [
        vp.camera_target[0] + vp.distance * cos_y * cos_p,
        vp.camera_target[1] + vp.distance * sin_p,
        vp.camera_target[2] + vp.distance * sin_y * cos_p,
    ];

    let aspect = (rect.width() / rect.height().max(1.0)).max(0.1);
    let proj_mat = perspective_matrix(
        vp.fov_degrees.to_radians(),
        aspect,
        vp.near_clip,
        vp.far_clip,
    );
    let (eye, proj_mat, view_mat) = if let Some(camera) = vp.camera_override {
        (camera.eye, camera.projection, camera.view)
    } else {
        (
            eye,
            proj_mat,
            look_at_matrix(eye, vp.camera_target, [0., 1., 0.]),
        )
    };
    let vp_mat = mat4_mul(&proj_mat, &view_mat);
    // Sky geometry sits beyond the camera's far plane (EM2: 90 units); it is drawn first in its own
    // depth pass, so it gets the same projection with a far plane 100,000 units out.
    let sky_proj_mat = {
        let mut m = proj_mat;
        if (m[10] - 1.0).abs() > 1e-6 && m[11] < 0.0 {
            // m10 = (f+n)/(n-f), m14 = 2fn/(n-f)  =>  near = m14 / (m10 - 1)
            let near = m[14] / (m[10] - 1.0);
            let far = 100_000.0f32;
            m[10] = (far + near) / (near - far); m[14] = 2.0 * far * near / (near - far);
        }
        m
    };

    // Primary directional light vector for directional shadow map matrix
    let primary=if vp.surfaces.mode!=0 {super::spatial_lights::key_directional(&lights,eye).map(|i|lights[i].clone())}else{lights.iter().find(|l|l.light_type==1).cloned()};
    let primary_dir=primary.as_ref().map(|l|l.direction).unwrap_or([-0.577,-0.707,-0.408]);
    let l_dir_len = (primary_dir[0] * primary_dir[0]
        + primary_dir[1] * primary_dir[1]
        + primary_dir[2] * primary_dir[2])
        .sqrt()
        .max(0.0001);
    let l_dir = [
        primary_dir[0] / l_dir_len,
        primary_dir[1] / l_dir_len,
        primary_dir[2] / l_dir_len,
    ];

    let l_pos = [
        vp.camera_target[0] - l_dir[0] * 150.0,
        vp.camera_target[1] - l_dir[1] * 150.0,
        vp.camera_target[2] - l_dir[2] * 150.0,
    ];
    let shadow_up=if vp.surfaces.mode!=0&&l_dir[1].abs()>0.98{[0.,0.,1.]}else{[0.,1.,0.]};
    let l_view = look_at_matrix(l_pos, vp.camera_target, shadow_up);
    let shadow_sz = (vp.distance * 1.5).clamp(40.0, 600.0);
    let l_proj = ortho_matrix(-shadow_sz, shadow_sz, -shadow_sz, shadow_sz, 1.0, 800.0);
    let mut light_vp = mat4_mul(&l_proj, &l_view);
    if vp.surfaces.mode!=0 {for i in [12,13]{let pixel=light_vp[i]*1024.;light_vp[i]+=(pixel.round()-pixel)/1024.;}}

    // ── Execute Hardware GL PaintCallback ─────────────────────────────────────
    let pixel_probe=vp.pixel_probe;
    let native_material=vp.native_material.clone();
    let surfaces = vp.surfaces;
    let smooth_textures = vp.smooth_textures;
    let finish = vp.finish;
    let view_culling = vp.view_culling;
    let render_work = vp.render_work.clone();
    let volume_shadow_mesh = vp.volume_shadow_mesh.clone();
    let volume_shadows = vp.volume_shadows.clone();
    let volume_shadow_report = vp.volume_shadow_report.clone();
    let model_shadow = vp.model_shadow.clone();
    let model_shadow_report = vp.model_shadow_report.clone();
    let show_grid = vp.show_grid;
    let grid_size = vp.grid_size;
    let backface_culling = vp.backface_culling;
    let flip_winding = vp.flip_winding;
    let render_mode = vp.render_mode;
    let lighting_mode = vp.lighting_mode;

    let callback = egui::PaintCallback {
        rect,
        callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                log::debug!(target:"blotworx_phase", "GPU callback begin");
                let gl = painter.gl().as_ref();
                unsafe {
                    let mut cache_guard = GLOBAL_GL_CACHE.lock().unwrap();
                    let cache = match cache_guard.as_mut() {
                        Some(c) => c,
                        None => {
                            if let Some(c) = GpuViewportCache::init(gl) {
                                *cache_guard = Some(c);
                                cache_guard.as_mut().unwrap()
                            } else {
                                return;
                            }
                        }
                    };

                    log::debug!(target:"blotworx_phase", "GPU cache ready");
                    cache.collect_unused(gl);
                    if let Some(backend)=&native_material {backend.lock().unwrap().begin_frame(gl);}
                    let native_managed=native_material.as_ref().map(|backend| {
                        let backend=backend.lock().unwrap();draw_commands.iter().map(|draw|(backend.handles(draw),backend.allows_linear_culling(draw))).collect::<Vec<_>>()
                    });

                    let destination_framebuffer=std::num::NonZeroU32::new(gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING)as u32).map(glow::NativeFramebuffer);
                    let scene_size=[info.screen_size_px[0]as i32,info.screen_size_px[1]as i32];

                    log::debug!(target:"blotworx_phase", "GPU timer begin");
                    let _gpu_interval = cache.gpu_timing.begin(gl, render_work.clone());
                    log::debug!(target:"blotworx_phase", "GPU timer ready");
                    let ppp = info.pixels_per_point;
                    let left = ((rect.min.x * ppp) as i32).max(0);
                    let bottom =
                        ((info.screen_size_px[1] as i32) - (rect.max.y * ppp) as i32).max(0);
                    let width = ((rect.width() * ppp) as i32).max(1);
                    let height = ((rect.height() * ppp) as i32).max(1);
                    cache.smooth_textures = smooth_textures;
                    cache.surfaces.begin(gl,surfaces,&draw_commands,&lights,eye,light_vp,view_mat,proj_mat,[left,bottom,width,height]);
                    let hdr_target=cache.surfaces.target(gl,scene_size);
                    let mut projector_images = None;
                    if let Some(frame) = &model_shadow {
                        let mut report = model_shadow_report.lock().unwrap();
                        report.caster_draws = frame.casters.len();
                        report.receiver_draws =
                            draw_commands.iter().filter(|d| d.projected_shadow).count();
                        let old = std::num::NonZeroU32::new(
                            gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING) as u32,
                        )
                        .map(glow::NativeFramebuffer);
                        let mut clear = [0.; 4];
                        gl.get_parameter_f32_slice(glow::COLOR_CLEAR_VALUE, &mut clear);
                        if cache.model_shadow_pass.is_none() {
                            cache.model_shadow_pass = Some(super::model_shadow::Pass::new(gl));
                            if cache.model_shadow_pass.as_ref().unwrap().is_ok() {
                                report.target_allocations += 1;
                            }
                        }
                        let pass = cache.model_shadow_pass.take().unwrap();
                        let result = (|| -> Result<(), String> {
                            let pass = pass.as_ref().map_err(Clone::clone)?;
                            pass.begin(gl, frame);
                            for caster in &frame.casters {
                                let mesh = cache
                                    .get_or_upload_mesh(gl, &caster.vertices, &caster.indices)
                                    .ok_or("Projector caster GPU mesh unavailable")?;
                                pass.model(gl, &caster.model);
                                gl.bind_vertex_array(Some(mesh.vao));
                                gl.draw_elements(
                                    glow::TRIANGLES,
                                    mesh.index_count,
                                    glow::UNSIGNED_INT,
                                    0,
                                );
                            }
                            let error = gl.get_error();
                            if error != glow::NO_ERROR {
                                return Err(format!("Projector draw GL error {error:#x}"));
                            }
                            projector_images = Some([pass.image, pass.clip_image]);
                            report.frames += 1;
                            Ok(())
                        })();
                        report.error = result.err();
                        cache.model_shadow_pass = Some(pass);
                        gl.bind_framebuffer(glow::FRAMEBUFFER, old);
                        gl.viewport(left, bottom, width, height);
                        gl.enable(glow::SCISSOR_TEST);
                        gl.scissor(left, bottom, width, height);
                        gl.clear_color(clear[0], clear[1], clear[2], clear[3]);
                        gl.bind_vertex_array(None);
                        gl.use_program(None);
                        gl.disable(glow::CULL_FACE);
                    }
                    let submission_started = std::time::Instant::now();
                    // Reject proven linear clip positions before shader, sampler,
                    // texture and extra-UV preparation. Nonlinear vertex programs
                    // retain their original path; these bounds cannot prove them
                    // invisible. Legacy shadow casters are still submitted below.
                    let early_rejected: Vec<_> = draw_commands
                        .iter().enumerate()
                        .map(|(index,cmd)| {
                            if native_managed.as_ref().is_some_and(|managed|managed[index].0&&!managed[index].1) {return false;}
                            if !view_culling || cmd.indices.is_empty() || cmd.v_data.is_empty() {
                                return false;
                            }
                            if cmd
                                .preview
                                .as_ref()
                                .and_then(|p| p.as_ref().ok())
                                .is_some_and(|program| !program.has_linear_clip_position())
                            {
                                return false;
                            }
                            cache
                                .get_or_upload_mesh(gl, &cmd.v_data, &cmd.indices)
                                .and_then(|mesh| mesh.bounds)
                                .is_some_and(|bounds| {
                                    bounds.outside(&mat4_mul(&vp_mat, &cmd.trans_mat))
                                })
                        })
                        .collect();
                    log::debug!(target:"blotworx_phase", "GPU material preparation begin");
                    let prepared_materials: Vec<_> = draw_commands
                        .iter()
                        .zip(&early_rejected)
                        .map(|(cmd, rejected)| {
                            if *rejected {
                                return None;
                            }
                            if cmd.indices.is_empty() || cmd.v_data.is_empty() {
                                return None;
                            }
                            cmd.preview
                                .as_ref()
                                .and_then(|program| program.as_ref().ok())
                                .and_then(|source| {
                                    cache.prepare_material_projected(
                                        gl,
                                        source,
                                        cmd,
                                        &vp_mat,
                                        &view_mat,
                                        model_shadow.as_deref().zip(projector_images),
                                    )
                                })
                        })
                        .collect();
                    log::debug!(target:"blotworx_phase", "GPU material preparation end");
                    let rejected: Vec<_> = draw_commands
                        .iter()
                        .zip(&prepared_materials)
                        .zip(&early_rejected).enumerate()
                        .map(|(index,((cmd, prepared), early))| {
                            if native_managed.as_ref().is_some_and(|managed|managed[index].0&&!managed[index].1) {return false;}
                            if cmd.skybox {return false;}
                            if *early {
                                return true;
                            }
                            if !view_culling || cmd.indices.is_empty() || cmd.v_data.is_empty() {
                                return false;
                            }
                            if let Some(prepared) = prepared {
                                if !cmd
                                    .preview
                                    .as_ref()
                                    .and_then(|p| p.as_ref().ok())
                                    .is_some_and(|p| p.has_linear_clip_position())
                                {
                                    return false;
                                }
                                prepared
                                    .mesh
                                    .bounds
                                    .is_some_and(|bounds| bounds.outside(&prepared.mvp))
                            } else {
                                cache
                                    .get_or_upload_mesh(gl, &cmd.v_data, &cmd.indices)
                                    .and_then(|mesh| mesh.bounds)
                                    .is_some_and(|bounds| {
                                        bounds.outside(&mat4_mul(&vp_mat, &cmd.trans_mat))
                                    })
                            }
                        })
                        .collect();
                    // Only the legacy scene-light shader samples this map. Native
                    // TEV programs and baked/unlit/headlight paths never read it.
                    // Prepare resources first so a failed native draw still gets
                    // exactly the same legacy shadow fallback as before.
                    let enhanced_shadow = surfaces.mode!=0 && surfaces.shadows>0. && primary.is_some();
                    let needs_legacy_shadow = enhanced_shadow || matches!(lighting_mode, LightingMode::SceneLights)
                        && !lights.is_empty()
                        && draw_commands
                            .iter()
                            .zip(&prepared_materials)
                            .zip(&rejected)
                            .any(|((cmd, prepared), rejected)| {
                                !rejected
                                    && !cmd.indices.is_empty()
                                    && !cmd.v_data.is_empty()
                                    && !cmd.baked
                                    && prepared.is_none()
                            });
                    if needs_legacy_shadow {
                        cache.ensure_legacy_shadow_target(gl);
                    }
                    let mut work = render_work.lock().unwrap();
                    work.frames += 1;
                    work.platform_native_draws=0;work.platform_native_errors.clear();
                    work.surface_style=surfaces.mode;
                    work.hdr_scene=hdr_target.is_some();
                    work.surface_draws=0;
                    work.surface_light_records=cache.surfaces.light_records;
                    work.surface_candidate_pairs=cache.surfaces.candidate_pairs;
                    work.surface_admitted_pairs=cache.surfaces.admitted_pairs;
                    work.surface_cluster_rebuilds=cache.surfaces.cluster_rebuilds;
                    work.surface_buffer_uploads=cache.surfaces.buffer_uploads;
                    work.vertex_buffer_uploads=cache.vertex_buffer_uploads;
                    work.index_buffer_uploads=cache.index_buffer_uploads;
                    work.surface_error=cache.surfaces.error.clone();
                    work.scene_light_counts=std::array::from_fn(|i|lights.iter().filter(|l|l.light_type==i as i32).count());
                    work.early_rejected_draws =
                        early_rejected.iter().filter(|&&rejected| rejected).count();
                    work.early_rejected_draws_total += work.early_rejected_draws as u64;
                    work.rejected_main_draws =
                        rejected.iter().filter(|&&rejected| rejected).count();
                    work.rejected_main_draws_total += work.rejected_main_draws as u64;
                    work.submitted_native_draws = 0;
                    work.submitted_fallback_draws = 0;
                    work.native_draws = prepared_materials
                        .iter()
                        .filter(|prepared| prepared.is_some())
                        .count();
                    work.fallback_draws = draw_commands
                        .iter()
                        .zip(&prepared_materials)
                        .zip(&early_rejected)
                        .filter(|((cmd, p), early)| {
                            !**early
                                && !cmd.indices.is_empty()
                                && !cmd.v_data.is_empty()
                                && p.is_none()
                        })
                        .count();
                    // per-material fallback reasons are diagnostics: rebuild them every 60 frames, not every frame
                    let diag_frame = pixel_probe.is_some() || work.frames % 60 == 1;
                    if diag_frame {work.fallback_materials.clear();}
                    for ((cmd, prepared), early) in draw_commands.iter().take(if diag_frame {usize::MAX} else {0})
                        .zip(&prepared_materials)
                        .zip(&early_rejected)
                    {
                        if *early
                            || prepared.is_some()
                            || cmd.indices.is_empty()
                            || cmd.v_data.is_empty()
                        {
                            continue;
                        }
                        let shader = cmd
                            .material
                            .as_ref()
                            .and_then(|material| material.shader.as_deref())
                            .unwrap_or("<shaderless>");
                        let reason = match &cmd.preview {
                            Some(Err(error)) => error.as_str(),
                            Some(Ok(_)) => "GPU resources unavailable",
                            None => "No native program",
                        };
                        let name = format!("{shader} (baked={}): {reason}", cmd.baked);
                        *work.fallback_materials.entry(name).or_default() += 1;
                    }
                    if needs_legacy_shadow {
                        work.shadow_pass_frames += 1;
                    } else {
                        work.skipped_shadow_pass_frames += 1;
                    }

                    gl.enable(glow::DEPTH_TEST);
                    gl.depth_func(glow::LEQUAL);
                    gl.depth_mask(true);
                    gl.color_mask(true, true, true, true);
                    gl.clear_depth(1.0);

                    // ── Pass 1: Render Cast Shadow Map to Framebuffer ────────
                    // The legacy shadow map only needs redrawing every other frame (the scene moves at
                    // 30 Hz); in between, sample it with the matrix it was drawn with.
                    cache.shadow_age = cache.shadow_age.wrapping_add(1);
                    let refresh_shadow = enhanced_shadow || pixel_probe.is_some() || cache.shadow_vp.is_none() || cache.shadow_age % 2 == 0;
                    let light_vp = if needs_legacy_shadow && !refresh_shadow {cache.shadow_vp.unwrap_or(light_vp)} else {light_vp};
                    if needs_legacy_shadow && refresh_shadow {cache.shadow_vp = Some(light_vp);}
                    if let (true, Some(sfbo), Some(_)) =
                        (needs_legacy_shadow && refresh_shadow, cache.shadow_fbo, cache.shadow_tex)
                    {
                        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(sfbo));
                        gl.viewport(0, 0, 2048, 2048);
                        gl.clear(glow::DEPTH_BUFFER_BIT);
                        gl.use_program(Some(cache.shadow_program));
                        gl.uniform_matrix_4_f32_slice(
                            Some(&cache.u_shadow_light_vp),
                            false,
                            &light_vp,
                        );

                        cache.surfaces.shadow_pass=enhanced_shadow;
                        gl.color_mask(false,false,false,false);
                        let mut using_shadow_program = true;
                        for cmd in &draw_commands {
                            if cmd.skybox || cmd.indices.is_empty() || cmd.v_data.is_empty() {
                                continue;
                            }
                            if cmd.preview.as_ref().and_then(|p| p.as_ref().ok())
                                .is_some_and(|program| program.scene_color_slot.is_some())
                            {
                                continue;
                            }
                            if enhanced_shadow {
                                if let Some(Ok(source))=&cmd.preview {
                                    if !super::surface_lighting::eligible(source,cmd){continue;}
                                    let mvp = mat4_mul(&light_vp, &cmd.trans_mat);
                                    if view_culling && cache.get_or_upload_mesh(gl, &cmd.v_data, &cmd.indices)
                                        .and_then(|mesh| mesh.bounds).is_some_and(|bounds| bounds.outside(&mvp))
                                    {
                                        work.rejected_shadow_casters += 1;
                                        continue;
                                    }
                                    // Main-view rejection does not reject shadow casters.
                                    if let Some(mut prepared)=cache.prepare_material(gl,source,cmd,&light_vp,&l_view){
                                        prepared.mvp=mvp;
                                        cache.draw_prepared_material(gl,source,cmd,&light_vp,&l_view,&prepared);
                                        using_shadow_program = false;
                                        work.shadow_caster_draws+=1;
                                    }
                                    continue;
                                }
                                // Unknown/custom PE shaders cannot be treated as solid cards.
                                if cmd.preview.is_some() || cmd.material.as_ref().and_then(|m|m.legacy_alpha_flags).is_some_and(|flags|flags&1!=0){continue;}
                            }
                            if let Some(mesh_handle) =
                                cache.get_or_upload_mesh(gl, &cmd.v_data, &cmd.indices)
                            {
                                // This pass uses the linear shadow shader for every
                                // caster. Reject only proven lateral separation in
                                // its own light clip space, never main-camera culling.
                                if view_culling
                                    && mesh_handle.bounds.is_some_and(|bounds| {
                                        bounds.outside(&mat4_mul(&light_vp, &cmd.trans_mat))
                                    })
                                {
                                    work.rejected_shadow_casters += 1;
                                    continue;
                                }
                                if !using_shadow_program {
                                    gl.use_program(Some(cache.shadow_program));
                                    using_shadow_program = true;
                                }
                                work.shadow_caster_draws += 1;
                                gl.uniform_matrix_4_f32_slice(
                                    Some(&cache.u_shadow_model),
                                    false,
                                    &cmd.trans_mat,
                                );
                                gl.bind_vertex_array(Some(mesh_handle.vao));
                                gl.draw_elements(
                                    glow::TRIANGLES,
                                    mesh_handle.index_count,
                                    glow::UNSIGNED_INT,
                                    0,
                                );
                            }
                        }
                        cache.surfaces.shadow_pass=false;
                        cache.surfaces.shadow=cache.shadow_tex;
                        cache.surfaces.shadow_valid=enhanced_shadow;
                        gl.color_mask(true,true,true,true);
                        gl.bind_vertex_array(None);
                        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                    }

                    if hdr_target.is_some(){for i in 0..4{gl.active_texture(glow::TEXTURE0+i);gl.bind_texture(glow::TEXTURE_2D,None);}}
                    gl.active_texture(glow::TEXTURE0);
                    gl.bind_framebuffer(glow::FRAMEBUFFER,hdr_target.map(|t|t.0).or(destination_framebuffer));
                    if hdr_target.is_some(){gl.draw_buffers(&[glow::COLOR_ATTACHMENT0,glow::COLOR_ATTACHMENT1]);}
                    gl.front_face(if flip_winding { glow::CW } else { glow::CCW });
                    if backface_culling {
                        gl.enable(glow::CULL_FACE);
                        gl.cull_face(glow::BACK);
                        if flip_winding {
                            gl.front_face(glow::CW);
                        } else {
                            gl.front_face(glow::CCW);
                        }
                    } else {
                        gl.disable(glow::CULL_FACE);
                    }

                    let (bg_r, bg_g, bg_b) = match render_mode {
                        RenderMode::Wireframe => (0.04, 0.04, 0.055),
                        _ => (0.07, 0.086, 0.118),
                    };
                    let (bg_r,bg_g,bg_b)=if hdr_target.is_some(){(f32::powf(bg_r,2.2),f32::powf(bg_g,2.2),f32::powf(bg_b,2.2))}else{(bg_r,bg_g,bg_b)};
                    gl.clear_color(bg_r, bg_g, bg_b, 1.0);
                    gl.enable(glow::SCISSOR_TEST);
                    gl.scissor(left, bottom, width, height);
                    gl.viewport(left, bottom, width, height);
                    gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);

                    if show_grid {
                        gl.enable(glow::BLEND);
                        gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
                        gl.use_program(Some(cache.grid_program));
                        gl.uniform_matrix_4_f32_slice(Some(&cache.grid_u_vp), false, &vp_mat);
                        gl.bind_vertex_array(Some(cache.grid_vao));
                        gl.draw_arrays(glow::LINES, 0, cache.grid_vert_count);
                        gl.bind_vertex_array(None);
                        gl.use_program(None);
                        gl.disable(glow::BLEND);
                    }

                    gl.use_program(Some(cache.mesh_program));
                    let mode_int = match lighting_mode {
                        LightingMode::Headlight => 0,
                        LightingMode::SceneLights => 1,
                        LightingMode::Unlit => 2,
                    };
                    gl.uniform_1_i32(Some(&cache.u_lighting_mode), mode_int);
                    gl.uniform_3_f32(Some(&cache.u_view_pos), eye[0], eye[1], eye[2]);
                    gl.uniform_matrix_4_f32_slice(Some(&cache.u_mesh_light_vp), false, &light_vp);

                    if let Some(stex) = cache.shadow_tex {
                        gl.active_texture(glow::TEXTURE1);
                        gl.bind_texture(glow::TEXTURE_2D, Some(stex));
                        gl.uniform_1_i32(Some(&cache.u_shadow_map_loc), 1);
                    }

                    // Light group matching rules:
                    //   Entity group "" or "(null)" → dynamic: receives all lights.
                    //   Entity group is space-separated: "Global ToonLight" means both groups.
                    //   A light matches if any of its own groups intersects with any of the entity's groups.
                    let is_dynamic_group =
                        |g: &str| g.is_empty() || g.eq_ignore_ascii_case("(null)");

                    let probe_pixel=pixel_probe.map(|p|[left+((p[0]*width as f32)as i32).clamp(0,width-1),bottom+(((1.-p[1])*height as f32)as i32).clamp(0,height-1)]);
                    let mut probe_previous=probe_pixel.map(|p|read_probe_pixel(gl,p));
                    let mut probe_previous_depth=probe_pixel.map(|p|read_probe_depth(gl,p));
                    if probe_pixel.is_some(){work.pixel_history.clear();work.pixel_history.push(serde_json::json!({"stage":"clear","rgba":probe_previous}));}
                    // Defer sortable blended geometry until opaque/alpha-tested
                    // draws have populated depth, as NiAlphaAccumulator does.
                    let mut sky_opaque = Vec::new();
                    let mut sky_blended = Vec::new();
                    let mut immediate = Vec::with_capacity(draw_commands.len());
                    let mut blended = Vec::new();
                    for (index, cmd) in draw_commands.iter().enumerate() {
                        if rejected[index] || cmd.indices.is_empty() || cmd.v_data.is_empty() {
                            continue;
                        }
                        let managed = native_managed.as_ref().is_some_and(|values| values[index].0);
                        let samples_scene_color = prepared_materials[index].is_some()
                            && cmd.preview.as_ref().and_then(|p| p.as_ref().ok())
                            .is_some_and(|program| program.scene_color_slot.is_some());
                        if samples_scene_color || cmd.needs_alpha_sort(prepared_materials[index].is_some(), managed) {
                            let pass = if cmd.skybox { &mut sky_blended } else { &mut blended };
                            pass.push((index, cmd.alpha_depth(&view_mat), samples_scene_color));
                        } else {
                            let pass = if cmd.skybox { &mut sky_opaque } else { &mut immediate };
                            pass.push(index);
                        }
                    }
                    // A scene-color material captures the layers behind it, including
                    // translucent draws. Coplanar regular draws enter that capture first.
                    for pass in [&mut sky_blended, &mut blended] {
                        pass.sort_unstable_by(|&(left_index, left_depth, left_capture), &(right_index, right_depth, right_capture)| {
                            right_depth.total_cmp(&left_depth)
                                .then(left_capture.cmp(&right_capture))
                                .then(left_index.cmp(&right_index))
                        });
                    }
                    let has_sky = !sky_opaque.is_empty() || !sky_blended.is_empty();
                    let ordered_draws = sky_opaque.into_iter()
                        .chain(sky_blended.into_iter().map(|(index, _, _)| index))
                        .map(Some)
                        .chain(has_sky.then_some(None))
                        .chain(immediate.into_iter().map(Some))
                        .chain(blended.into_iter().map(|(index, _, _)| Some(index)));
                    for draw in ordered_draws {
                        let Some(draw_index) = draw else {
                            // Keep depth within the sky pass. Level geometry
                            // must start with a clear depth buffer.
                            gl.depth_mask(true);
                            gl.clear(glow::DEPTH_BUFFER_BIT);
                            if let Some(pixel) = probe_pixel {
                                let depth = read_probe_depth(gl, pixel);
                                work.pixel_history.push(serde_json::json!({"stage":"skybox_depth_reset","depth":depth}));
                                probe_previous_depth = Some(depth);
                            }
                            continue;
                        };
                        let cmd = &draw_commands[draw_index];
                        let prepared = &prepared_materials[draw_index];

                        if native_managed.as_ref().is_some_and(|managed|managed[draw_index].0) {
                            let result=cache.get_or_upload_mesh(gl,&cmd.v_data,&cmd.indices)
                                .ok_or_else(||"Native material geometry upload failed".to_owned())
                                .and_then(|mesh|unsafe {native_material.as_ref().unwrap().lock().unwrap().draw(gl,cmd,mesh,&super::native_material::Frame {view:&view_mat,projection:if cmd.skybox {&sky_proj_mat} else {&proj_mat},eye,viewport:[left,bottom,width,height],lights:&lights})});
                            match result {
                                Ok(true)=>{
                                    work.platform_native_draws+=1;work.submitted_native_draws+=1;
                                    if prepared.is_none() {
                                        work.native_draws+=1;work.fallback_draws=work.fallback_draws.saturating_sub(1);
                                    }
                                    if prepared.is_none() && diag_frame {
                                        let shader=cmd.material.as_ref().and_then(|m|m.shader.as_deref()).unwrap_or("<shaderless>");
                                        let reason=match &cmd.preview {Some(Err(e))=>e.as_str(),Some(Ok(_))=>"GPU resources unavailable",None=>"No native program"};
                                        let key=format!("{shader} (baked={}): {reason}",cmd.baked);
                                        if let Some(count)=work.fallback_materials.get_mut(&key){*count=count.saturating_sub(1);if *count==0 {work.fallback_materials.remove(&key);}}
                                    }
                                    if let Some(pixel)=probe_pixel {let rgba=read_probe_pixel(gl,pixel);let depth=read_probe_depth(gl,pixel);work.pixel_history.push(serde_json::json!({"draw":draw_index,"skybox":cmd.skybox,"path":"platform_native","rgba":rgba,"depth":depth}));probe_previous=Some(rgba);probe_previous_depth=Some(depth);}
                                    continue;
                                },
                                Ok(false)=>{},
                                Err(error)=>{*work.platform_native_errors.entry(error).or_default()+=1;},
                            }
                        }

                        if let (Some(Ok(source)), Some(prepared)) = (&cmd.preview, prepared) {
                            cache.draw_prepared_material(
                                gl, source, cmd, &vp_mat, &view_mat, prepared,
                            );
                            if surfaces.mode!=0 && super::surface_lighting::eligible(source,cmd){work.surface_draws+=1;}
                            if let Some(pixel)=probe_pixel {let rgba=read_probe_pixel(gl,pixel);if probe_previous.is_none_or(|before|before.iter().zip(&rgba).any(|(a,b)|(a-b).abs()>1e-7)){
                              work.pixel_history.push(serde_json::json!({"draw":draw_index,"skybox":cmd.skybox,"shader":cmd.material.as_ref().and_then(|m|m.shader.as_deref()),"rgba":rgba,"blend":source.blend,"depth_write":source.depth_write,"textures":cmd.material.as_ref().map(|m|m.textures.iter().map(|t|t.path.as_str()).collect::<Vec<_>>()),"vertex_alpha":cmd.v_data.chunks_exact(12).map(|v|v[11]).fold([f32::INFINITY,f32::NEG_INFINITY],|a,v|[a[0].min(v),a[1].max(v)])}));probe_previous=Some(rgba);
                            }}
                            if let Some(pixel)=probe_pixel {probe_previous_depth=Some(read_probe_depth(gl,pixel));}
                            work.submitted_native_draws += 1;
                            continue;
                        }
                        if let Some(texture) = cache.shadow_tex {
                            gl.active_texture(glow::TEXTURE1);
                            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                        }
                        // A material draw may change program and PE state. Restore every
                        // fallback uniform and state rather than depending on draw order.
                        gl.enable(glow::DEPTH_TEST);
                        gl.depth_mask(true);
                        gl.depth_func(glow::LEQUAL);
                        gl.disable(glow::BLEND);
                        gl.blend_equation(glow::FUNC_ADD);
                        // Shaderless Gamebryo surfaces still carry inherited
                        // material/alpha properties. Keep custom shader state
                        // separate from this legacy fallback.
                        // EM2 Studio: effect surfaces (glows, light rays, smoke, the brush's paint card)
                        // use custom shaders that fall back here; they blend by their NiAlphaProperty
                        // too, and named additive effects with no alpha property add (black = clear).
                        if let Some(material) = cmd.material.as_ref()
                        {
                            let name = format!("{} {}", material.shader.as_deref().unwrap_or(""), material.textures.first().map(|t| t.path.as_str()).unwrap_or(""));
                            let lower = name.to_ascii_lowercase();
                            let additive_name = ["additive", "_add_", "_add.", "halo", "lightray", "glow", "corona", "flare"].iter().any(|k| lower.contains(k));
                            let flags = material.legacy_alpha_flags.filter(|flags| flags & 1 != 0)
                                .or_else(|| additive_name.then_some(1 | (0 << 1) | (0 << 5)));
                            if let Some(flags) = flags
                            {
                                if material.shader.is_some() { gl.depth_mask(false); }
                                let factor = |value: u16| match value {
                                    0 => Some(glow::ONE),
                                    1 => Some(glow::ZERO),
                                    2 => Some(glow::SRC_COLOR),
                                    3 => Some(glow::ONE_MINUS_SRC_COLOR),
                                    4 => Some(glow::DST_COLOR),
                                    5 => Some(glow::ONE_MINUS_DST_COLOR),
                                    6 => Some(glow::SRC_ALPHA),
                                    7 => Some(glow::ONE_MINUS_SRC_ALPHA),
                                    8 => Some(glow::DST_ALPHA),
                                    9 => Some(glow::ONE_MINUS_DST_ALPHA),
                                    10 => Some(glow::SRC_ALPHA_SATURATE),
                                    _ => None,
                                };
                                if let (Some(src), Some(dst)) =
                                    (factor((flags >> 1) & 15), factor((flags >> 5) & 15))
                                {
                                    gl.enable(glow::BLEND);
                                    gl.blend_func(src, dst);
                                    gl.blend_equation(glow::FUNC_ADD);
                                }
                            }
                        }
                        if backface_culling {
                            gl.enable(glow::CULL_FACE);
                            gl.cull_face(glow::BACK);
                        } else {
                            gl.disable(glow::CULL_FACE);
                        }
                        gl.use_program(Some(cache.mesh_program));
                        gl.uniform_1_i32(Some(&cache.u_lighting_mode), if surfaces.mode!=0{2}else{mode_int});
                        gl.uniform_1_i32(Some(&cache.u_baked), cmd.baked as i32);
                        gl.uniform_3_f32(Some(&cache.u_view_pos), eye[0], eye[1], eye[2]);
                        gl.uniform_matrix_4_f32_slice(
                            Some(&cache.u_mesh_light_vp),
                            false,
                            &light_vp,
                        );
                        // The shader ignores lights for baked surfaces and in modes 0/2: skip the
                        // per-draw light filtering and upload entirely for those.
                        let lights_used = !cmd.baked && surfaces.mode == 0 && mode_int != 0 && mode_int != 2;
                        // Split entity groups by whitespace
                        let entity_groups: Vec<&str> = if lights_used {cmd.light_group.split_whitespace().collect()} else {Vec::new()};
                        let entity_is_dynamic = entity_groups.is_empty()
                            || entity_groups.iter().any(|g| is_dynamic_group(g));
                        // world-space bounding sphere of this draw, to drop point/spot lights out of range
                        let (center, radius) = {
                            let c = [(cmd.aabb_min[0]+cmd.aabb_max[0])*0.5,(cmd.aabb_min[1]+cmd.aabb_max[1])*0.5,(cmd.aabb_min[2]+cmd.aabb_max[2])*0.5];
                            let h = [(cmd.aabb_max[0]-cmd.aabb_min[0])*0.5,(cmd.aabb_max[1]-cmd.aabb_min[1])*0.5,(cmd.aabb_max[2]-cmd.aabb_min[2])*0.5];
                            let m = &cmd.trans_mat;
                            let w = [m[0]*c[0]+m[4]*c[1]+m[8]*c[2]+m[12], m[1]*c[0]+m[5]*c[1]+m[9]*c[2]+m[13], m[2]*c[0]+m[6]*c[1]+m[10]*c[2]+m[14]];
                            let scale = (0..3).map(|k|(m[k*4]*m[k*4]+m[k*4+1]*m[k*4+1]+m[k*4+2]*m[k*4+2]).sqrt()).fold(0f32,f32::max);
                            (w, (h[0]*h[0]+h[1]*h[1]+h[2]*h[2]).sqrt()*scale)
                        };
                        let bounded = center.iter().all(|v|v.is_finite()) && radius.is_finite();

                        let mut filtered: Vec<&GpuLight> = if !lights_used {Vec::new()} else {lights
                            .iter()
                            .filter(|l| {
                                if entity_is_dynamic {
                                    return true; // dynamic entity: all lights apply
                                }
                                // Light also supports multi-group (space-separated)
                                let light_groups: Vec<&str> =
                                    l.light_group.split_whitespace().collect();
                                if light_groups.is_empty()
                                    || light_groups.iter().any(|g| is_dynamic_group(g))
                                {
                                    return true; // dynamic light: affects all entities
                                }
                                // Check any intersection between entity groups and light groups
                                entity_groups.iter().any(|eg| {
                                    light_groups.iter().any(|lg| lg.eq_ignore_ascii_case(eg))
                                })
                            })
                            .collect()};
                        // keep the first light (the shader shadows it) and any light that can reach this draw
                        if bounded {
                            let mut index = 0;
                            filtered.retain(|l| {
                                index += 1;
                                index == 1 || !matches!(l.light_type, 2 | 3) || !(l.range > 0.0) || {
                                    let d = [l.position[0]-center[0], l.position[1]-center[1], l.position[2]-center[2]];
                                    (d[0]*d[0]+d[1]*d[1]+d[2]*d[2]).sqrt() <= l.range + radius
                                }
                            });
                        }

                        let num_lights = filtered.len().min(128) as i32;
                        gl.uniform_1_i32(Some(&cache.u_num_lights), num_lights);

                        if num_lights > 0 {
                            let mut packed = Vec::with_capacity(num_lights as usize * 24);
                            for light in filtered.iter().take(128) {
                                packed.extend_from_slice(&[
                                    light.position[0],
                                    light.position[1],
                                    light.position[2],
                                    light.light_type as f32,
                                ]);
                                packed.extend_from_slice(&[
                                    light.direction[0],
                                    light.direction[1],
                                    light.direction[2],
                                    light.dimmer,
                                ]);
                                packed.extend_from_slice(&[
                                    light.diffuse[0],
                                    light.diffuse[1],
                                    light.diffuse[2],
                                    light.range,
                                ]);
                                packed.extend_from_slice(&[
                                    light.ambient[0],
                                    light.ambient[1],
                                    light.ambient[2],
                                    0.0,
                                ]);
                                packed.extend_from_slice(&[
                                    light.attenuation[0],
                                    light.attenuation[1],
                                    light.attenuation[2],
                                    0.,
                                ]);
                                packed.extend_from_slice(&[
                                    light.spot_inner,
                                    light.spot_outer,
                                    light.spot_exponent,
                                    0.,
                                ]);
                            }
                            gl.uniform_4_f32_slice(Some(&cache.u_lights_loc), &packed);
                        }

                        let mvp = mat4_mul(&vp_mat, &cmd.trans_mat);
                        gl.uniform_matrix_4_f32_slice(Some(&cache.u_mvp), false, &mvp);
                        gl.uniform_matrix_4_f32_slice(Some(&cache.u_model), false, &cmd.trans_mat);
                        let color = cmd
                            .material
                            .as_ref()
                            .filter(|m| m.shader.is_none())
                            .and_then(|m| m.legacy_diffuse)
                            .map(|color| {
                                std::array::from_fn::<_, 4, _>(|i| color[i] * cmd.color[i])
                            })
                            .unwrap_or(cmd.color);
                        gl.uniform_4_f32_slice(Some(&cache.u_color), &color);
                        let vertex_color_modulation=super::material_vertex_color::modulates_diffuse(
                            cmd.material.as_ref().and_then(|material|material.shader.as_deref()));
                        gl.uniform_1_i32(Some(&cache.u_modulate_vertex_color),vertex_color_modulation as i32);
                        let alpha_flags=cmd.material.as_ref().and_then(|m|m.legacy_alpha_flags).unwrap_or(0);
                        let alpha_function=if alpha_flags & 0x200 != 0 {(alpha_flags>>10)&7} else {0};
                        let alpha_reference=cmd.material.as_ref().and_then(|m|m.legacy_alpha_threshold).unwrap_or(0) as f32/255.;
                        gl.uniform_1_i32(Some(&cache.u_alpha_function),alpha_function as i32);
                        gl.uniform_1_f32(Some(&cache.u_alpha_reference),alpha_reference);
                        let uv_matrix = cmd
                            .uv_transform
                            .and_then(|t| t.matrix())
                            .unwrap_or([1., 0., 0., 0., 1., 0., 0., 0., 1.]);
                        gl.uniform_matrix_3_f32_slice(
                            Some(&cache.u_uv_transform),
                            false,
                            &uv_matrix,
                        );

                        let gpu_tex = if let Some((ref rgba, w, h)) = cmd.tex_bytes {
                            cache.get_or_upload_texture(gl, rgba, w, h)
                        } else {
                            None
                        };

                        if let Some(tex) = gpu_tex {
                            gl.active_texture(glow::TEXTURE0);
                            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                            cmd.sampler.smoothed(smooth_textures).apply(gl);
                            gl.uniform_1_i32(Some(&cache.u_use_tex), 1);
                        } else {
                            gl.uniform_1_i32(Some(&cache.u_use_tex), 0);
                        }

                        cache.surfaces.bind(gl,&cache.mesh_surface_uniforms,cmd,false);
                        if let Some(mesh_handle) =
                            cache.get_or_upload_mesh(gl, &cmd.v_data, &cmd.indices)
                        {
                            gl.bind_vertex_array(Some(mesh_handle.vao));
                            gl.draw_elements(
                                glow::TRIANGLES,
                                mesh_handle.index_count,
                                glow::UNSIGNED_INT,
                                0,
                            );
                            work.submitted_fallback_draws += 1;
                        }
                        if let Some(pixel)=probe_pixel {
                            let rgba=read_probe_pixel(gl,pixel);let depth=read_probe_depth(gl,pixel);
                            if probe_previous.is_none_or(|before|before.iter().zip(&rgba).any(|(a,b)|(a-b).abs()>1e-7)) || probe_previous_depth!=Some(depth) {
                                work.pixel_history.push(serde_json::json!({"draw":draw_index,"path":"fallback","rgba":rgba,"depth":depth,"previous_depth":probe_previous_depth,
                                    "depth_test":true,"depth_write":true,"depth_func":glow::LEQUAL,"cull":backface_culling,"color":color,
                                    "shader":cmd.material.as_ref().and_then(|m|m.shader.as_deref()),"alpha_flags":alpha_flags,"alpha_reference":alpha_reference,
                                    "textures":cmd.material.as_ref().map(|m|m.textures.iter().map(|t|t.path.as_str()).collect::<Vec<_>>())}));
                            }
                            probe_previous=Some(rgba);probe_previous_depth=Some(depth);
                        }
                    }
                    gl.bind_vertex_array(None);
                    gl.use_program(None);

                    {
                        if hdr_target.is_some(){gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);}
                        let mut report = volume_shadow_report.lock().unwrap();
                        report.submitted = volume_shadows.len();
                        if !volume_shadows.is_empty() {
                            let result = (|| -> Result<(), String> {
                                let mesh = volume_shadow_mesh
                                    .as_ref()
                                    .ok_or("Shadow geometry missing")?;
                                if cache.volume_shadow_pass.is_none() {
                                    cache.volume_shadow_pass =
                                        Some(super::volume_shadow::Pass::new(gl));
                                }
                                cache
                                    .volume_shadow_pass
                                    .as_mut()
                                    .unwrap()
                                    .as_mut()
                                    .map_err(|error| error.clone())?
                                    .draw(
                                        gl,
                                        mesh,
                                        &volume_shadows,
                                        &vp_mat,
                                        [left, bottom, width, height],
                                        &mut report,
                                    )
                            })();
                            report.error = result.err();
                        } else {
                            report.error = None;
                        }
                    }
                    work.occlusion_error=None;
                    if let Some((_,texture,indirect,depth))=hdr_target {
                        let ao=if surfaces.occlusion>0. {
                            let pass=cache.contact_pass.get_or_insert_with(||super::contact_occlusion::Pass::new(gl));
                            let result=match pass{Ok(pass)=>pass.draw(gl,depth,scene_size,[left,bottom,width,height],proj_mat,surfaces.occlusion,surfaces.contact_radius),Err(e)=>Err(e.clone())};
                            match result{Ok(t)=>{work.occlusion_frames+=1;Some(t)},Err(e)=>{work.occlusion_error=Some(e);None}}
                        }else{None};
                        gl.bind_framebuffer(glow::FRAMEBUFFER,destination_framebuffer);
                        let pass=cache.finish_pass.get_or_insert_with(||super::scene_finish::Pass::new(gl));
                        match pass{Ok(pass)=>{pass.draw_hdr(gl,[left,bottom,width,height],finish,surfaces.mode,texture,scene_size,indirect,depth,ao,proj_mat);work.finishing_frames+=1;work.finish_error=None;},Err(e)=>work.finish_error=Some(e.clone())}
                    }else if finish.active(){
                        let pass=cache.finish_pass.get_or_insert_with(||super::scene_finish::Pass::new(gl));
                        match pass{Ok(pass)=>{pass.draw(gl,[left,bottom,width,height],finish);work.finishing_frames+=1;work.finish_error=None;},Err(e)=>work.finish_error=Some(e.clone())}
                    }
                    // EM2's own bloom + colour correction (pp_bloom / pp_colourcorrection), from the active render profile
                    if let Some(params)=super::em2_post::get() {
                        gl.bind_framebuffer(glow::FRAMEBUFFER,destination_framebuffer);
                        match cache.em2_post.get_or_insert_with(||super::em2_post::Pass::new(gl)) {
                            Ok(pass)=>pass.draw(gl,[left,bottom,width,height],params),
                            Err(e)=>{work.finish_error=Some(format!("EM2 post: {e}"));}
                        }
                    }
                    if let Some(pixel)=probe_pixel {work.pixel_history.push(serde_json::json!({"stage":"resolve","rgba":read_probe_pixel(gl,pixel)}));}
                    let submission_ms = submission_started.elapsed().as_secs_f64() * 1000.;
                    work.submission_total_ms += submission_ms;
                    work.submission_max_ms = work.submission_max_ms.max(submission_ms);
                    drop(work);
                    log::debug!(target:"blotworx_phase", "GPU scene draws end");
                    gl.enable(glow::DEPTH_TEST);
                    gl.depth_mask(true);
                    gl.depth_func(glow::LEQUAL);
                    gl.disable(glow::BLEND);
                    gl.blend_equation(glow::FUNC_ADD);
                    gl.disable(glow::CULL_FACE);
                    if !line_commands.is_empty() {
                        let mut line_verts: Vec<f32> = Vec::with_capacity(line_commands.len() * 14);
                        for line in &line_commands {
                            let [r, g, b, a] = line.color;
                            line_verts.extend_from_slice(&[
                                line.start[0],
                                line.start[1],
                                line.start[2],
                                r,
                                g,
                                b,
                                a,
                            ]);
                            line_verts.extend_from_slice(&[
                                line.end[0],
                                line.end[1],
                                line.end[2],
                                r,
                                g,
                                b,
                                a,
                            ]);
                        }

                        gl.enable(glow::BLEND);
                        gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
                        gl.use_program(Some(cache.line_program));
                        gl.uniform_matrix_4_f32_slice(Some(&cache.line_u_vp), false, &vp_mat);
                        gl.bind_vertex_array(Some(cache.line_vao));

                        gl.bind_buffer(glow::ARRAY_BUFFER, Some(cache.line_vbo));
                        let l_bytes = std::slice::from_raw_parts(
                            line_verts.as_ptr() as *const u8,
                            line_verts.len() * 4,
                        );
                        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, l_bytes, glow::STREAM_DRAW);

                        gl.draw_arrays(glow::LINES, 0, (line_verts.len() / 7) as i32);

                        gl.bind_vertex_array(None);
                        gl.use_program(None);
                        gl.disable(glow::BLEND);
                    }

                    gl.bind_texture(glow::TEXTURE_2D, None);
                    gl.bind_buffer(glow::ARRAY_BUFFER, None);
                    gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, None);
                    gl.disable(glow::SCISSOR_TEST);
                    gl.disable(glow::DEPTH_TEST);
                    gl.disable(glow::CULL_FACE);
                    drop(_gpu_interval);
                    log::debug!(target:"blotworx_phase", "GPU callback end");
                }
            }));
            log::debug!(target:"blotworx_phase", "GPU callback returned");
        })),
    };
    ui.painter().add(egui::Shape::Callback(callback));

    if let Some((title, details)) = overlay_warning {
        let warning_rect = ui.painter().with_clip_rect(rect).text(
            rect.min + egui::vec2(8., 4.),
            egui::Align2::LEFT_TOP,
            title,
            egui::TextStyle::Small.resolve(ui.style()),
            egui::Color32::GOLD,
        );
        ui.interact(
            warning_rect,
            ui.id().with("viewport-limitations"),
            Sense::hover(),
        )
        .on_hover_text(details);
    }
    (rect, response, vp_mat, eye)
}

#[cfg(test)]
mod resource_identity_tests {
    use super::*;
    #[test]
    fn weak_cache_owner_prevents_recycled_identity_and_detects_replacement() {
        let mut vertices = Arc::new(vec![1.0f32, 2.0, 3.0]);
        let old = Arc::downgrade(&vertices);
        let old_key = old.as_ptr() as usize;
        // make_mut must allocate a new identity while cached weak ownership exists.
        Arc::make_mut(&mut vertices)[0] = 9.;
        assert_ne!(Arc::as_ptr(&vertices) as usize, old_key);
        assert!(old.upgrade().is_none());
        let live = Arc::downgrade(&vertices);
        assert_eq!(live.strong_count(), 1);
        drop(vertices);
        assert_eq!(live.strong_count(), 0);
    }
}

#[cfg(test)]
mod sampler_tests {
    use super::*;
    #[test]
    fn wrap_axes_and_mipmap_modes_follow_material_flags() {
        let a = TextureSampler::from_nif_flags(0x2100);
        assert_eq!(a.wrap_u, glow::REPEAT);
        assert_eq!(a.wrap_v, glow::CLAMP_TO_EDGE);
        assert_eq!(a.min_filter, glow::LINEAR);
        let b = TextureSampler::from_nif_flags(0x1400);
        assert_eq!(b.wrap_u, glow::CLAMP_TO_EDGE);
        assert_eq!(b.wrap_v, glow::REPEAT);
        assert_eq!(b.min_filter, glow::NEAREST_MIPMAP_LINEAR);
        assert_eq!(b.mag_filter, glow::NEAREST);
        assert_eq!(
            TextureSampler::default().min_filter,
            glow::LINEAR_MIPMAP_LINEAR
        );
    }
}

#[cfg(test)]
mod texture_storage_tests {
    use super::*;

    #[test]
    fn opaque_images_use_rgb_storage_and_save_alpha_bytes_across_mips() {
        let image = vec![20, 30, 40, 255].repeat(16);
        let (opaque, stored, saved) = texture_storage_estimate(&image, 4, 4);
        assert!(opaque);
        assert_eq!(mip_chain_pixels(4, 4), 16 + 4 + 1);
        assert_eq!(stored, 21 * 3);
        assert_eq!(saved, 21);
    }

    #[test]
    fn any_nonopaque_texel_preserves_rgba_storage() {
        let mut image = vec![20, 30, 40, 255].repeat(16);
        image[3] = 254;
        let (opaque, stored, saved) = texture_storage_estimate(&image, 4, 4);
        assert!(!opaque);
        assert_eq!(stored, 21 * 4);
        assert_eq!(saved, 0);
    }

    #[test]
    fn mip_estimate_matches_odd_texture_dimensions() {
        assert_eq!(mip_chain_pixels(3, 5), 15 + 2 + 1);
    }
}
