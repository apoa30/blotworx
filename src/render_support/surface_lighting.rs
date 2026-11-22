//! Opt-in forward surface lighting. Original TEV output remains the authored
//! indirect/color layer; enhanced profiles add world-space, per-pixel lighting.
use super::{
    material_preview::Program,
    viewport3d::{GpuDrawCommand, GpuLight},
};
use glow::{self, HasContext};
use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    pub mode: u32,
    pub direct: f32,
    pub indirect: f32,
    pub specular: f32,
    pub roughness: f32,
    pub detail: f32,
    pub shadows: f32,
    pub softness: f32,
    pub fill: f32,
    pub occlusion: f32,
    pub contact_radius: f32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: 0,
            direct: 0.,
            indirect: 1.,
            specular: 0.,
            roughness: 0.7,
            detail: 0.,
            shadows: 0.,
            softness: 1.,
            fill: 0.,
            occlusion: 0.,
            contact_radius: 0.35,
        }
    }
}
impl Settings {
    pub fn presets() -> [Self; 3] {
        [
            Self::default(),
            Self {
                mode: 1,
                direct: 0.38,
                indirect: 0.92,
                specular: 0.35,
                roughness: 0.72,
                detail: 0.08,
                shadows: 0.55,
                softness: 1.5,
                fill: 0.03,
                occlusion: 0.2,
                contact_radius: 0.2,
            },
            Self {
                mode: 2,
                direct: 1.1,
                indirect: 0.66,
                specular: 1.0,
                roughness: 0.48,
                detail: 0.22,
                shadows: 0.85,
                softness: 2.5,
                fill: 0.12,
                occlusion: 0.8,
                contact_radius: 0.35,
            },
        ]
    }
    pub fn sanitize(&mut self) {
        self.mode = self.mode.min(2);
        for (x, a, b, d) in [
            (&mut self.direct, 0., 3., 0.5),
            (&mut self.indirect, 0., 2., 1.),
            (&mut self.specular, 0., 3., 0.5),
            (&mut self.roughness, 0.12, 1., 0.7),
            (&mut self.detail, 0., 0.6, 0.),
            (&mut self.shadows, 0., 1., 0.),
            (&mut self.softness, 0.5, 4., 1.),
            (&mut self.fill, 0., 0.4, 0.),
            (&mut self.occlusion, 0., 1., 0.),
            (&mut self.contact_radius, 0.05, 1.5, 0.35),
        ] {
            *x = if x.is_finite() { x.clamp(a, b) } else { d };
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        ui.columns(2, |c| {
            egui::Grid::new("surface_light")
                .num_columns(2)
                .show(&mut c[0], |ui| {
                    for (n, v, r) in [
                        ("Direct light", &mut self.direct, 0.0..=3.0),
                        ("Baked / indirect", &mut self.indirect, 0.0..=2.0),
                        ("Soft fill", &mut self.fill, 0.0..=0.4),
                        ("Contact visibility", &mut self.occlusion, 0.0..=1.0),
                        ("Shadows", &mut self.shadows, 0.0..=1.0),
                    ] {
                        ui.label(n);
                        ui.add(egui::Slider::new(v, r));
                        ui.end_row();
                    }
                });
            egui::Grid::new("surface_material")
                .num_columns(2)
                .show(&mut c[1], |ui| {
                    for (n, v, r) in [
                        ("Specular", &mut self.specular, 0.0..=3.0),
                        ("Roughness", &mut self.roughness, 0.12..=1.0),
                        ("Surface detail", &mut self.detail, 0.0..=0.6),
                        ("Shadow softness", &mut self.softness, 0.5..=4.0),
                        ("Contact radius", &mut self.contact_radius, 0.05..=1.5),
                    ] {
                        ui.label(n);
                        ui.add(egui::Slider::new(v, r));
                        ui.end_row();
                    }
                });
        });
    }
}
pub fn eligible(source: &Program, cmd: &GpuDrawCommand) -> bool {
    if !source.has_linear_clip_position()
        || !source.depth_test
        || !source.depth_write
        || source.blend.is_some()
        || source.pinned_projection
    {
        return false;
    }
    let name = cmd
        .material
        .as_ref()
        .and_then(|m| m.shader.as_deref())
        .unwrap_or("")
        .replace('\\', "/")
        .to_ascii_lowercase();
    let name = name.rsplit('/').next().unwrap_or("");
    ![
        "sky",
        "particle",
        "billboard",
        "glow",
        "flame",
        "screen",
        "decal",
        "water",
        "invisible",
    ]
    .iter()
    .any(|s| name.contains(s))
}
fn group_applies(object: &str, light: &str) -> bool {
    let wildcard = |g: &str| g.is_empty() || g.eq_ignore_ascii_case("(null)");
    object.trim().is_empty()
        || object.split_whitespace().any(wildcard)
        || light.trim().is_empty()
        || light.split_whitespace().any(wildcard)
        || object
            .split_whitespace()
            .any(|a| light.split_whitespace().any(|b| a.eq_ignore_ascii_case(b)))
}
/// Uniform locations are immutable for the lifetime of a linked program.
#[derive(Clone)]
pub(super) struct Uniforms {
    bw_clusters: Option<glow::UniformLocation>,
    bw_group_masks: Option<glow::UniformLocation>,
    bw_lights: Option<glow::UniformLocation>,
    bw_depth: Option<glow::UniformLocation>,
    bw_hdr: Option<glow::UniformLocation>,
    bw_style: Option<glow::UniformLocation>,
    bw_model: Option<glow::UniformLocation>,
    bw_authored_surface: Option<glow::UniformLocation>,
    bw_base_tint: Option<glow::UniformLocation>,
    bw_eye: Option<glow::UniformLocation>,
    bw_light_settings: Option<glow::UniformLocation>,
    bw_surface: Option<glow::UniformLocation>,
    bw_object_group: Option<glow::UniformLocation>,
    bw_group_words: Option<glow::UniformLocation>,
    bw_grid_size: Option<glow::UniformLocation>,
    bw_grid_depth: Option<glow::UniformLocation>,
    bw_viewport_origin: Option<glow::UniformLocation>,
    bw_view: Option<glow::UniformLocation>,
    bw_light_vp: Option<glow::UniformLocation>,
    bw_has_shadow: Option<glow::UniformLocation>,
}

impl Uniforms {
    pub unsafe fn load(gl: &glow::Context, program: glow::Program) -> Self {
        Self {
            bw_clusters: gl.get_uniform_location(program, "bw_clusters"),
            bw_group_masks: gl.get_uniform_location(program, "bw_group_masks"),
            bw_lights: gl.get_uniform_location(program, "bw_lights"),
            bw_depth: gl.get_uniform_location(program, "bw_depth"),
            bw_hdr: gl.get_uniform_location(program, "bw_hdr"),
            bw_style: gl.get_uniform_location(program, "bw_style"),
            bw_model: gl.get_uniform_location(program, "bw_model"),
            bw_authored_surface: gl.get_uniform_location(program, "bw_authored_surface"),
            bw_base_tint: gl.get_uniform_location(program, "bw_base_tint"),
            bw_eye: gl.get_uniform_location(program, "bw_eye"),
            bw_light_settings: gl.get_uniform_location(program, "bw_light_settings"),
            bw_surface: gl.get_uniform_location(program, "bw_surface"),
            bw_object_group: gl.get_uniform_location(program, "bw_object_group"),
            bw_group_words: gl.get_uniform_location(program, "bw_group_words"),
            bw_grid_size: gl.get_uniform_location(program, "bw_grid_size"),
            bw_grid_depth: gl.get_uniform_location(program, "bw_grid_depth"),
            bw_viewport_origin: gl.get_uniform_location(program, "bw_viewport_origin"),
            bw_view: gl.get_uniform_location(program, "bw_view"),
            bw_light_vp: gl.get_uniform_location(program, "bw_light_vp"),
            bw_has_shadow: gl.get_uniform_location(program, "bw_has_shadow"),
        }
    }
}

#[derive(Default)]
pub struct State {
    pub settings: Settings,
    pub eye: [f32; 3],
    pub light_vp: [f32; 16],
    pub shadow: Option<glow::Texture>,
    pub shadow_valid: bool,
    variants: HashMap<
        (usize, usize),
        (
            Weak<Program>,
            Option<Weak<super::nif::MaterialBindings>>,
            Arc<Program>,
        ),
    >,
    object_groups: HashMap<usize, i32>,
    grid: super::clustered_lights::Grid,
    cluster_dirty: bool,
    uploaded_lights: Vec<f32>,
    uploaded_masks: Vec<u32>,
    buffer_capacity: Option<usize>,
    view: [f32; 16],
    cluster_buffer: Option<glow::Buffer>,
    cluster_texture: Option<glow::Texture>,
    group_buffer: Option<glow::Buffer>,
    group_texture: Option<glow::Texture>,
    group_words: i32,
    pub candidate_pairs: usize,
    pub admitted_pairs: usize,
    pub cluster_rebuilds: u64,
    pub buffer_uploads: u64,
    buffer: Option<glow::Buffer>,
    texture: Option<glow::Texture>,
    pub error: Option<String>,
    pub light_records: usize,
    pub shadow_pass: bool,
    target: Option<Target>,
}
impl State {
    pub fn variant(&mut self, source: &Arc<Program>, cmd: &GpuDrawCommand) -> Option<Arc<Program>> {
        if self.settings.mode == 0 || self.error.is_some() {
            return None;
        }
        let key = (
            Arc::as_ptr(source) as usize,
            cmd.material
                .as_ref()
                .map(|m| Arc::as_ptr(m) as usize)
                .unwrap_or(0),
        );
        if !self.variants.contains_key(&key) {
            let mut p = (**source).clone();
            p.fragment = p.fragment.replace(
                "out vec4 frag_color;",
                "layout(location=0) out vec4 frag_color;",
            );
            if eligible(source, cmd) {
                let vertex = p
                    .vertex
                    .replacen("void main(){", "void bw_original_vertex(){", 1);
                let vertex=format!("{vertex}\nuniform mat4 bw_model;out vec3 bw_position;out vec3 bw_normal;\nvoid main(){{bw_original_vertex();bw_position=(bw_model*vec4(a_pos,1.)).xyz;bw_normal=transpose(inverse(mat3(bw_model)))*a_normal;}}\n");
                p.vertex = Arc::from(vertex);
                p.linear_clip_source = Some(Arc::downgrade(&p.vertex));
                p.fragment = p
                    .fragment
                    .replacen("void main(){", "void bw_original_fragment(){", 1);
                let plan = super::surface_material::Plan::native(source, cmd.material.as_deref());
                p.fragment
                    .push_str("\nin vec3 bw_position;in vec3 bw_normal;\n");
                p.fragment.push_str(&plan.glsl());
                p.fragment.push_str(FRAGMENT);
            } else {
                p.fragment = p
                    .fragment
                    .replacen("void main(){", "void bw_original_fragment(){", 1);
                let multiply = source.blend.is_some_and(|(a, b, e)| {
                    (a == glow::DST_COLOR && b == glow::ZERO)
                        || (a == glow::ZERO && b == glow::SRC_COLOR)
                        || e == glow::FUNC_REVERSE_SUBTRACT
                });
                let indirect = if multiply {
                    "frag_color.rgb"
                } else {
                    "vec3(0.)"
                };
                let alpha = if super::surface_material::encoded_emission(source.blend) {
                    "frag_color.a=pow(clamp(frag_color.a,0.,1.),2.2);"
                } else {
                    ""
                };
                p.fragment.push_str(&format!("\nlayout(location=1) out vec4 bw_indirect_color;\nvoid main(){{bw_original_fragment();frag_color.rgb=pow(max(frag_color.rgb,vec3(0.)),vec3(2.2));{alpha}bw_indirect_color=vec4({indirect},frag_color.a);}}\n"));
            }
            self.variants.insert(
                key,
                (
                    Arc::downgrade(source),
                    cmd.material.as_ref().map(Arc::downgrade),
                    Arc::new(p),
                ),
            );
        }
        self.variants.get(&key).map(|(_, _, v)| v.clone())
    }
    pub fn collect(&mut self, invalidated: bool) {
        self.variants.retain(|_, (s, m, _)| {
            !invalidated && s.strong_count() > 0 && m.as_ref().is_none_or(|m| m.strong_count() > 0)
        });
    }
    pub unsafe fn destroy(&mut self, gl: &glow::Context) {
        if let Some(t) = self.target.take() {
            t.destroy(gl);
        }
        if let Some(b) = self.buffer.take() {
            gl.delete_buffer(b)
        }
        for b in [self.cluster_buffer.take(), self.group_buffer.take()]
            .into_iter()
            .flatten()
        {
            gl.delete_buffer(b)
        }
        for t in [self.cluster_texture.take(), self.group_texture.take()]
            .into_iter()
            .flatten()
        {
            gl.delete_texture(t)
        }
        if let Some(t) = self.texture.take() {
            gl.delete_texture(t)
        }
        self.object_groups.clear();
        self.uploaded_lights.clear();
        self.uploaded_masks.clear();
        self.buffer_capacity = None;
    }
    pub unsafe fn begin(
        &mut self,
        gl: &glow::Context,
        settings: Settings,
        draws: &[GpuDrawCommand],
        lights: &[GpuLight],
        eye: [f32; 3],
        light_vp: [f32; 16],
        view: [f32; 16],
        projection: [f32; 16],
        rect: [i32; 4],
    ) {
        self.settings = settings;
        self.eye = eye;
        self.light_vp = light_vp;
        self.view = view;
        if settings.mode != 0 && self.grid.build(lights, view, projection, rect) {
            self.cluster_rebuilds += 1;
            self.cluster_dirty = true;
        }
        self.error = None;
        self.shadow = None;
        self.shadow_valid = false;
        self.shadow_pass = false;
        self.light_records = 0;
        self.candidate_pairs = 0;
        self.admitted_pairs = 0;

        if settings.mode == 0 {
            return;
        }
        if let Err(e) = self.upload(gl, draws, lights) {
            self.error = Some(e);
        }
    }
    unsafe fn upload(
        &mut self,
        gl: &glow::Context,
        draws: &[GpuDrawCommand],
        lights: &[GpuLight],
    ) -> Result<(), String> {
        self.object_groups.clear();
        let mut names = Vec::<String>::new();
        let mut name_ids = HashMap::<String, i32>::new();
        for draw in draws {
            let id = if let Some(id) = name_ids.get(draw.light_group.as_ref()) {
                *id
            } else {
                let id = names.len() as i32;
                names.push(draw.light_group.to_string());
                name_ids.insert(draw.light_group.to_string(), id);
                id
            };
            self.object_groups
                .insert(draw as *const GpuDrawCommand as usize, id);
        }
        self.group_words = ((names.len() + 31) / 32).max(1) as i32;
        let mut masks = vec![0u32; lights.len() * self.group_words as usize];
        let mut packed = Vec::<f32>::with_capacity(lights.len() * 24);
        let primary = super::spatial_lights::key_directional(lights, self.eye);
        for (i, l) in lights.iter().enumerate() {
            for (g, name) in names.iter().enumerate() {
                if group_applies(name, &l.light_group) {
                    masks[i * self.group_words as usize + g / 32] |= 1u32 << (g % 32);
                }
            }
            packed.extend_from_slice(&[
                l.position[0],
                l.position[1],
                l.position[2],
                l.light_type as f32,
                l.direction[0],
                l.direction[1],
                l.direction[2],
                l.dimmer,
                l.diffuse[0],
                l.diffuse[1],
                l.diffuse[2],
                l.range,
                l.ambient[0],
                l.ambient[1],
                l.ambient[2],
                if Some(i) == primary { 1. } else { 0. },
                l.attenuation[0],
                l.attenuation[1],
                l.attenuation[2],
                l.range_start,
                l.spot_inner,
                l.spot_outer,
                l.spot_exponent,
                0.,
            ]);
        }
        self.light_records = lights.len();
        self.candidate_pairs = self.grid.candidates;
        self.admitted_pairs = self.grid.admitted;
        let capacity = *self.buffer_capacity.get_or_insert_with(|| {
            gl.get_parameter_i32(glow::MAX_TEXTURE_BUFFER_SIZE).max(0) as usize
        });
        if packed.len() / 4 > capacity
            || masks.len() > capacity
            || self.grid.indices.len() > capacity
        {
            return Err(
                "Clustered light buffers exceed GPU capacity; no lights were truncated".into(),
            );
        }
        if masks.is_empty() {
            masks.push(0)
        }
        if self.cluster_dirty || self.cluster_texture.is_none() {
            Self::upload_buffer(gl, &mut self.cluster_buffer, &mut self.cluster_texture,
                12, glow::R32I, bytemuck::cast_slice(&self.grid.indices))?;
            self.cluster_dirty = false;
            self.buffer_uploads += 1;
        }
        if self.uploaded_masks != masks {
            Self::upload_buffer(gl, &mut self.group_buffer, &mut self.group_texture,
                13, glow::R32UI, bytemuck::cast_slice(&masks))?;
            self.uploaded_masks = masks;
            self.buffer_uploads += 1;
        }
        if packed.is_empty() {
            packed.resize(24, 0.)
        }
        let packed_bytes: &[u8] = bytemuck::cast_slice(&packed);
        if bytemuck::cast_slice::<f32, u8>(&self.uploaded_lights) != packed_bytes {
            Self::upload_buffer(gl, &mut self.buffer, &mut self.texture,
                11, glow::RGBA32F, packed_bytes)?;
            self.uploaded_lights = packed;
            self.buffer_uploads += 1;
        }
        Ok(())
    }
    unsafe fn upload_buffer(
        gl: &glow::Context,
        buffer: &mut Option<glow::Buffer>,
        texture: &mut Option<glow::Texture>,
        unit: u32,
        format: u32,
        bytes: &[u8],
    ) -> Result<(), String> {
        if buffer.is_none() {
            *buffer = Some(gl.create_buffer()?)
        }
        if texture.is_none() {
            *texture = Some(gl.create_texture()?)
        }
        gl.bind_buffer(glow::TEXTURE_BUFFER, *buffer);
        gl.buffer_data_u8_slice(glow::TEXTURE_BUFFER, bytes, glow::STREAM_DRAW);
        gl.active_texture(glow::TEXTURE0 + unit);
        gl.bind_texture(glow::TEXTURE_BUFFER, *texture);
        gl.tex_buffer(glow::TEXTURE_BUFFER, format, *buffer);
        gl.bind_buffer(glow::TEXTURE_BUFFER, None);
        Ok(())
    }
    pub unsafe fn bind(
        &self,
        gl: &glow::Context,
        uniforms: &Uniforms,
        cmd: &GpuDrawCommand,
        shadow_pass: bool,
    ) {
        gl.uniform_1_i32(uniforms.bw_clusters.as_ref(), 12);
        gl.uniform_1_i32(
            uniforms.bw_group_masks.as_ref(),
            13,
        );
        if self.settings.mode == 0 {
            gl.uniform_1_i32(uniforms.bw_lights.as_ref(), 11);
            gl.uniform_1_i32(uniforms.bw_depth.as_ref(), 10);
            gl.uniform_1_i32(uniforms.bw_hdr.as_ref(), 0);
            gl.uniform_1_i32(uniforms.bw_style.as_ref(), 0);
            return;
        }
        let loc = &uniforms.bw_style;
        if loc.is_none() {
            return;
        }
        let fallback_surface = cmd.preview.is_none()
            && cmd
                .material
                .as_ref()
                .and_then(|m| m.legacy_alpha_flags)
                .is_none_or(|f| f & 1 == 0);
        let enabled = self.settings.mode != 0
            && self.error.is_none()
            && !shadow_pass
            && (cmd.preview.is_some() || fallback_surface);
        gl.uniform_1_i32(
            uniforms.bw_hdr.as_ref(),
            i32::from(!shadow_pass && self.settings.mode != 0 && self.error.is_none()),
        );
        gl.uniform_1_i32(
            loc.as_ref(),
            if enabled {
                self.settings.mode as i32
            } else {
                0
            },
        );
        gl.uniform_matrix_4_f32_slice(
            uniforms.bw_model.as_ref(),
            false,
            &cmd.trans_mat,
        );
        gl.uniform_1_i32(uniforms.bw_lights.as_ref(), 11);
        gl.uniform_1_i32(uniforms.bw_depth.as_ref(), 10);
        gl.active_texture(glow::TEXTURE11);
        gl.bind_sampler(11, None);
        gl.bind_texture(glow::TEXTURE_BUFFER, self.texture);
        gl.active_texture(glow::TEXTURE10);
        gl.bind_sampler(10, None);
        gl.bind_texture(
            glow::TEXTURE_2D,
            if shadow_pass { None } else { self.shadow },
        );
        gl.active_texture(glow::TEXTURE12);
        gl.bind_sampler(12, None);
        gl.bind_texture(glow::TEXTURE_BUFFER, self.cluster_texture);
        gl.active_texture(glow::TEXTURE13);
        gl.bind_sampler(13, None);
        gl.bind_texture(glow::TEXTURE_BUFFER, self.group_texture);
        if !enabled {
            return;
        }
        let preserve = cmd.material_indirect.is_some()
            || cmd
                .preview
                .as_ref()
                .and_then(|p| p.as_ref().ok())
                .is_some_and(|p| {
                    p.pass_constants.iter().any(Option::is_some)
                        || cmd
                            .material_registers
                            .as_ref()
                            .zip(p.register_defaults.as_ref())
                            .is_some_and(|(a, b)| a != b)
                });
        gl.uniform_1_i32(
            uniforms.bw_authored_surface
                .as_ref(),
            i32::from(preserve),
        );
        let s = self.settings;
        let tint = cmd
            .material
            .as_ref()
            .and_then(|m| m.legacy_diffuse)
            .unwrap_or(cmd.color);
        gl.uniform_3_f32(
            uniforms.bw_base_tint.as_ref(),
            tint[0],
            tint[1],
            tint[2],
        );
        gl.uniform_3_f32(
            uniforms.bw_eye.as_ref(),
            self.eye[0],
            self.eye[1],
            self.eye[2],
        );
        gl.uniform_4_f32(
            uniforms.bw_light_settings
                .as_ref(),
            s.direct,
            s.indirect,
            s.specular,
            s.fill,
        );
        let roughness = cmd
            .material
            .as_ref()
            .and_then(|m| m.legacy_colors.as_ref())
            .filter(|m| m.shininess > 0.)
            .map(|m| ((2. / (m.shininess + 2.)).sqrt() * (s.roughness / 0.7)).clamp(0.12, 1.))
            .unwrap_or(s.roughness);
        gl.uniform_4_f32(
            uniforms.bw_surface.as_ref(),
            roughness,
            s.detail,
            s.shadows,
            s.softness,
        );
        gl.uniform_1_i32(
            uniforms.bw_object_group.as_ref(),
            self.object_groups
                .get(&(cmd as *const GpuDrawCommand as usize))
                .copied()
                .unwrap_or(0),
        );
        gl.uniform_1_i32(
            uniforms.bw_group_words.as_ref(),
            self.group_words,
        );
        gl.uniform_3_i32(
            uniforms.bw_grid_size.as_ref(),
            self.grid.dims[0],
            self.grid.dims[1],
            self.grid.dims[2],
        );
        gl.uniform_4_f32(
            uniforms.bw_grid_depth.as_ref(),
            self.grid.near,
            self.grid.far,
            if self.grid.perspective { 1. } else { 0. },
            0.,
        );
        gl.uniform_2_f32(
            uniforms.bw_viewport_origin
                .as_ref(),
            self.grid.rect[0] as f32,
            self.grid.rect[1] as f32,
        );
        gl.uniform_matrix_4_f32_slice(
            uniforms.bw_view.as_ref(),
            false,
            &self.view,
        );
        gl.uniform_matrix_4_f32_slice(
            uniforms.bw_light_vp.as_ref(),
            false,
            &self.light_vp,
        );
        gl.uniform_1_i32(
            uniforms.bw_has_shadow.as_ref(),
            i32::from(self.shadow_valid),
        );
    }
}
pub fn legacy_fragment(original: &str) -> String {
    let original = original
        .replacen("void main() {", "void bw_original_fragment() {", 1)
        .replace(
            "out vec4 FragColor;",
            "layout(location=0) out vec4 FragColor;",
        );
    let channels="vec3 bw_material_albedo(vec3 authored){return pow(max((u_use_tex?texture(u_tex,v_uv).rgb:vec3(1.))*u_color.rgb,vec3(0.)),vec3(2.2));} float bw_material_roughness(float r){return r;}float bw_material_metallic(){return 0.;}vec3 bw_material_normal(vec3 N){return N;}";
    let lighting = format!(
        "#define bw_position v_world_pos\n#define bw_normal v_normal\n{channels}\n{}",
        FRAGMENT
    )
    .replace("frag_color", "FragColor");
    format!("{original}\n{lighting}")
}
struct Target {
    fbo: glow::Framebuffer,
    color: glow::Texture,
    indirect: glow::Texture,
    depth: glow::Texture,
    size: [i32; 2],
}
impl Target {
    unsafe fn new(gl: &glow::Context, size: [i32; 2]) -> Result<Self, String> {
        let fbo = gl.create_framebuffer()?;
        let color = gl.create_texture()?;
        let indirect = gl.create_texture()?;
        let depth = gl.create_texture()?;
        let old =
            std::num::NonZeroU32::new(gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING) as u32)
                .map(glow::NativeFramebuffer);
        gl.active_texture(glow::TEXTURE0);
        for (texture, format, kind, type_) in [
            (color, glow::RGBA16F, glow::RGBA, glow::FLOAT),
            (indirect, glow::RGBA16F, glow::RGBA, glow::FLOAT),
            (
                depth,
                glow::DEPTH24_STENCIL8,
                glow::DEPTH_STENCIL,
                glow::UNSIGNED_INT_24_8,
            ),
        ] {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                format as i32,
                size[0],
                size[1],
                0,
                kind,
                type_,
                glow::PixelUnpackData::Slice(None),
            );
            for p in [glow::TEXTURE_MIN_FILTER, glow::TEXTURE_MAG_FILTER] {
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    p,
                    if texture == depth {
                        glow::NEAREST
                    } else {
                        glow::LINEAR
                    } as i32,
                );
            }
            for p in [glow::TEXTURE_WRAP_S, glow::TEXTURE_WRAP_T] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, p, glow::CLAMP_TO_EDGE as i32);
            }
        }
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        for (attachment, texture) in [
            (glow::COLOR_ATTACHMENT0, color),
            (glow::COLOR_ATTACHMENT1, indirect),
            (glow::DEPTH_STENCIL_ATTACHMENT, depth),
        ] {
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                attachment,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
        }
        gl.draw_buffers(&[glow::COLOR_ATTACHMENT0, glow::COLOR_ATTACHMENT1]);
        let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
        gl.bind_framebuffer(glow::FRAMEBUFFER, old);
        gl.bind_texture(glow::TEXTURE_2D, None);
        if status != glow::FRAMEBUFFER_COMPLETE {
            gl.delete_framebuffer(fbo);
            for t in [color, indirect, depth] {
                gl.delete_texture(t)
            }
            return Err(format!(
                "Floating-point scene framebuffer incomplete: {status:x}"
            ));
        }
        Ok(Self {
            fbo,
            color,
            indirect,
            depth,
            size,
        })
    }
    unsafe fn destroy(self, gl: &glow::Context) {
        gl.delete_framebuffer(self.fbo);
        for t in [self.color, self.indirect, self.depth] {
            gl.delete_texture(t)
        }
    }
}
impl State {
    pub unsafe fn target(
        &mut self,
        gl: &glow::Context,
        size: [i32; 2],
    ) -> Option<(
        glow::Framebuffer,
        glow::Texture,
        glow::Texture,
        glow::Texture,
    )> {
        if self.settings.mode == 0 || self.error.is_some() {
            return None;
        }
        if self.target.as_ref().is_some_and(|t| t.size != size) {
            self.target.take().unwrap().destroy(gl);
        }
        if self.target.is_none() {
            match Target::new(gl, size) {
                Ok(t) => self.target = Some(t),
                Err(e) => {
                    self.error = Some(e);
                    return None;
                }
            }
        }
        self.target
            .as_ref()
            .map(|t| (t.fbo, t.color, t.indirect, t.depth))
    }
}
const FRAGMENT: &str = r#"
layout(location=1) out vec4 bw_indirect_color;vec3 bw_indirect_term;
uniform int bw_hdr;uniform int bw_style;uniform mat4 bw_model;uniform vec3 bw_eye;
uniform vec4 bw_light_settings;uniform vec4 bw_surface;
uniform samplerBuffer bw_lights;uniform isamplerBuffer bw_clusters;uniform usamplerBuffer bw_group_masks;uniform int bw_object_group;uniform int bw_group_words;uniform ivec3 bw_grid_size;uniform vec4 bw_grid_depth;uniform vec2 bw_viewport_origin;uniform mat4 bw_view;
uniform sampler2D bw_depth;uniform mat4 bw_light_vp;uniform bool bw_has_shadow;
float bw_shadow(vec3 N,vec3 L){
 if(!bw_has_shadow||bw_surface.z<=0.)return 1.;vec4 p=bw_light_vp*vec4(bw_position,1.);vec3 uv=p.xyz/p.w*.5+.5;
 if(any(lessThan(uv,vec3(0.)))||any(greaterThan(uv,vec3(1.))))return 1.;
 float bias=max(.00008,.0004*(1.-max(dot(N,L),0.)));vec2 t=1./vec2(textureSize(bw_depth,0));float lit=0.;int r=bw_style==2?2:1;float count=0.;
 for(int y=-r;y<=r;y++)for(int x=-r;x<=r;x++){lit+=step(uv.z-bias,texture(bw_depth,uv.xy+vec2(x,y)*t*bw_surface.w).r);count+=1.;}
 return mix(1.,lit/count,bw_surface.z);
}
vec3 bw_relight(vec3 authored){
 vec3 N=length(bw_normal)>1e-6?normalize(bw_normal):normalize(cross(dFdx(bw_position),dFdy(bw_position)));
 if(!gl_FrontFacing)N=-N;
 vec3 baked=pow(clamp(authored,0.,1.),vec3(2.2));vec3 albedo=bw_style==2?bw_material_albedo(authored):baked;N=bw_material_normal(N);
 float h=dot(albedo,vec3(.2126,.7152,.0722));vec3 dp1=dFdx(bw_position),dp2=dFdy(bw_position);
 vec3 R1=cross(dp2,N),R2=cross(N,dp1);float det=dot(dp1,R1);
 if(abs(det)>1e-7)N=normalize(N-bw_surface.y*sign(det)*(dFdx(h)*R1+dFdy(h)*R2)/max(abs(det),.001));
 vec3 V=normalize(bw_eye-bw_position);float nv=max(dot(N,V),.001);float rough=max(bw_material_roughness(bw_surface.x),.12);float metallic=bw_material_metallic();float a=rough*rough;float a2=a*a;
 vec3 direct=vec3(0.),ambient=vec3(0.);float hemispheric=.3+.7*max(N.y,0.);
 ivec2 tile=clamp(ivec2((gl_FragCoord.xy-bw_viewport_origin)/64.),ivec2(0),bw_grid_size.xy-1);float viewZ=max(-(bw_view*vec4(bw_position,1.)).z,bw_grid_depth.x);float depthT=bw_grid_depth.z!=0.?log(viewZ/bw_grid_depth.x)/log(bw_grid_depth.y/bw_grid_depth.x):(viewZ-bw_grid_depth.x)/(bw_grid_depth.y-bw_grid_depth.x);int layer=clamp(int(floor(depthT*float(bw_grid_size.z))),0,bw_grid_size.z-1);int header=((layer*bw_grid_size.y+tile.y)*bw_grid_size.x+tile.x)*2;int start=texelFetch(bw_clusters,header).r,count=texelFetch(bw_clusters,header+1).r;
 for(int i=0;i<count;i++){
  int source=texelFetch(bw_clusters,start+i).r;uint groupMask=texelFetch(bw_group_masks,source*bw_group_words+bw_object_group/32).r;if((groupMask&(1u<<uint(bw_object_group%32)))==0u)continue;
  int k=source*6;vec4 p=texelFetch(bw_lights,k),d=texelFetch(bw_lights,k+1),c=texelFetch(bw_lights,k+2),amb=texelFetch(bw_lights,k+3),falloff=texelFetch(bw_lights,k+4);int kind=int(p.w+.5);
  vec3 delta=p.xyz-bw_position;float distance=length(delta);float attenuation=1.;
  if(c.w>0.){if(distance>c.w)continue;if(distance>=falloff.w)attenuation=clamp((c.w-distance)/max(c.w-falloff.w,1e-6),0.,1.);}
  else if(kind>=2){float denominator=dot(falloff.xyz,vec3(1.,distance,distance*distance));if(denominator<1e-6)continue;attenuation=clamp(1./denominator,0.,1.);}
  if(kind==0){ambient+=amb.rgb*d.w*attenuation;continue;}
  vec3 L=kind==1?normalize(-d.xyz):delta/max(distance,1e-5);
  if(kind==3){vec3 spot=texelFetch(bw_lights,k+5).xyz;float cone=dot(-L,normalize(d.xyz));attenuation*=pow(clamp((cone-spot.y)/max(spot.x-spot.y,.0001),0.,1.),max(spot.z,.001));}
  float nl=max(dot(N,L),0.);if(nl<=0.)continue;vec3 H=normalize(L+V);float nh=max(dot(N,H),0.),vh=max(dot(V,H),0.);
  float D=a2/max(3.14159265*pow(nh*nh*(a2-1.)+1.,2.),.0001);float kG=pow(rough+1.,2.)/8.;float G=(nv/(nv*(1.-kG)+kG))*(nl/(nl*(1.-kG)+kG));vec3 F0=mix(vec3(.04),albedo,metallic);vec3 F=F0+(vec3(1.)-F0)*pow(1.-vh,5.);
  vec3 specular=D*G*F/max(4.*nv*nl,.001)*bw_light_settings.z;float shade=amb.w>.5?bw_shadow(N,L):1.;
  vec3 radiance=c.rgb*d.w*attenuation;
  direct+=(albedo*(1.-metallic)*(vec3(1.)-F)/3.14159265+specular)*radiance*nl*shade;
  ambient+=amb.rgb*d.w*attenuation;
 }
 // Preserve the authored paint palette and baked light as the indirect layer.
 // Analytic hemisphere fill is an explicit enhancement, not claimed baked GI.
 vec3 indirect=baked*bw_light_settings.y*(.88+.12*hemispheric);
 bw_indirect_term=max(indirect+albedo*(ambient*.2+vec3(bw_light_settings.w*hemispheric)),vec3(0.));
 vec3 result=bw_indirect_term+direct*bw_light_settings.x;
 return max(result,vec3(0.));
}
void main(){bw_original_fragment();bw_indirect_color=vec4(0.,0.,0.,frag_color.a);if(bw_style!=0){frag_color.rgb=bw_relight(frag_color.rgb);bw_indirect_color.rgb=bw_indirect_term;}else if(bw_hdr!=0)frag_color.rgb=pow(max(frag_color.rgb,vec3(0.)),vec3(2.2));}
"#;
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn groups_preserve_wildcards_and_multiple_memberships() {
        assert!(group_applies("Village Interior", "interior"));
        assert!(!group_applies("Village", "Interior"));
        assert!(group_applies("(null)", "Village"));
        assert!(group_applies("Village", ""));
    }
    #[test]
    fn profiles_keep_classic_unchanged_and_bound_roughness() {
        let mut p = Settings::presets();
        assert_eq!(p[0].mode, 0);
        assert!(p[1].direct < p[2].direct);
        p[2].roughness = f32::NAN;
        p[2].sanitize();
        assert!(p[2].roughness.is_finite());
    }
}
