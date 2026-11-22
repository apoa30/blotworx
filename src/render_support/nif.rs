// editors/nif.rs — Custom UI for NetImmerse / Gamebryo NIF model files (.nif, .nif_wii)
#[path = "nif_pixel_data.rs"]
pub(crate) mod pixel_data;

#[cfg(test)]
mod material_binding_audit {
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn production_texture_sources() -> anyhow::Result<()> {
        use epic_mickey_lib_rs::{
            nif_file::{NiObject, NifFile},
            packfile::Packfile,
        };
        let root = std::path::PathBuf::from(std::env::var("BLOTWORX_TEST_DATA")?);
        let pak = Packfile::from_binary(std::fs::read(
            root.join("packfiles/gremlinvillage_zonea.pak"),
        )?);
        let mut rows = Vec::new();
        let mut multi_texture_meshes = 0;
        let mut harmonic_meshes = 0;
        for file in &pak.files {
            if !file.path.to_ascii_lowercase().ends_with(".nif_wii") {
                continue;
            }
            let nif = NifFile::load_bytes(&file.data).map_err(anyhow::Error::msg)?;
            for (index, block) in nif.blocks.iter().enumerate() {
                if !matches!(block, NiObject::NiMesh(_)) {
                    continue;
                }
                let surface = super::find_mesh_surface(&nif, index);
                if surface.material.shader.as_deref() == Some("SH_Diffuse") {
                    anyhow::ensure!(
                        surface.material.integers.get("SphericalHarmonicMap") == Some(&1),
                        "SH texture role lost"
                    );
                    anyhow::ensure!(
                        surface.material.integers.contains_key("R3MTLightingFlags"),
                        "Lighting flags lost"
                    );
                    harmonic_meshes += 1;
                }
                let textures = &surface.material.textures;
                if textures.len() > 1 {
                    let slots = textures
                        .iter()
                        .map(|t| t.slot)
                        .collect::<std::collections::BTreeSet<_>>();
                    anyhow::ensure!(
                        slots.len() == textures.len(),
                        "{} mesh {index}: texture inputs collapsed onto the same slot",
                        file.path
                    );
                    multi_texture_meshes += 1;
                }
            }
            for (index, block) in nif.blocks.iter().enumerate() {
                let NiObject::Unknown(raw) = block else {
                    continue;
                };
                if raw.type_name != "NiTexturingProperty" {
                    continue;
                }
                let maps = crate::nif_textures::bindings(
                    &raw.raw_data,
                    nif.source_is_little_endian,
                    nif.version,
                )?;
                for map in maps {
                    let source = nif.blocks.get(map.source as usize);
                    rows.push(serde_json::json!({"asset":file.path,"property":index,"slot":map.slot,"shader_id":map.shader_id,
                        "source_index":map.source,"source_type":source.map(NiObject::type_name),
                        "texture":source.and_then(|b|if let NiObject::NiSourceTexture(t)=b {Some(t)} else {None})}));
                }
            }
        }
        anyhow::ensure!(!rows.is_empty(), "No texture sources audited");
        anyhow::ensure!(
            multi_texture_meshes > 0,
            "No multi-texture meshes exercised"
        );
        anyhow::ensure!(harmonic_meshes > 0, "No SH material metadata exercised");
        std::fs::write(
            "target/production-texture-sources.json",
            serde_json::to_vec_pretty(&rows)?,
        )?;
        println!("Audited {} original texture bindings; {multi_texture_meshes} multi-texture meshes retain distinct slots; {harmonic_meshes} SH meshes retain named input and lighting metadata",rows.len());
        Ok(())
    }
}

use super::viewport3d::{DecodedImage, Viewport3DState as ViewportState};
use anyhow::{Context, Result};
use egui::{
    Color32, Grid, Pos2, RichText, ScrollArea, Sense, Shape, Stroke, TextEdit, TextureHandle, Ui,
    Vec2,
};
use epic_mickey_lib_rs::file_manipulator::EndianType;
use epic_mickey_lib_rs::nif_file::{
    extract_mesh, NiAVObjectData, NiObject, NiTransform, NiWiiUPersistentSrcTextureRendererData,
    NifFile,
};
use epic_mickey_lib_rs::packfile::VirtualFile;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct MaterialTexture {
    /// None is a standard Gamebryo map; Some retains the shader-map metadata.
    pub shader_id: Option<u32>,
    pub uv_set: u8,
    pub slot: u32,
    pub path: String,
    pub sampler: super::viewport3d::TextureSampler,
    pub transform: Option<crate::nif_textures::UvTransform>,
}
#[derive(Clone, Debug, Default)]
pub struct MaterialBindings {
    /// Descriptor transforms survive independently of texture image paths.
    pub(crate) texture_transforms: std::collections::BTreeMap<
        (bool, u32),
        Result<Option<crate::nif_textures::UvTransform>, String>,
    >,
    /// Exact mesh-local numeric extra data for original effect parameters.
    /// Malformed attributes remain errors rather than disappearing into defaults.
    pub(crate) attributes:
        Option<Result<std::sync::Arc<crate::nif_shader_attributes::Table>, String>>,
    /// Complete inherited material state. Diffuse alone is insufficient for
    /// the default Wii shader's emissive-only and lit channel paths.
    pub legacy_colors: Option<LegacyMaterialColors>,
    pub legacy_diffuse: Option<[f32; 4]>,
    pub legacy_alpha_flags: Option<u16>,
    pub legacy_alpha_threshold: Option<u8>,
    pub(crate) sort_disabled: bool,
    pub legacy_depth_flags: Option<Result<u16, String>>,
    pub legacy_texture_flags: Option<Result<u16, String>>,
    pub legacy_stencil_flags: Option<Result<u16, String>>,
    pub legacy_wireframe_flags: Option<Result<u16, String>>,
    pub legacy_shade_flags: Option<Result<u16, String>>,
    /// Actual decoded COLOR0 presence, distinct from the white placeholder in
    /// the interleaved editor vertex buffer when the source has no colors.
    pub has_vertex_colors: bool,
    /// Effective inherited NiVertexColorProperty, with malformed data retained
    /// as an error rather than conflated with an absent property.
    pub vertex_color_flags: Option<Result<u16, String>>,
    pub shader: Option<String>,
    pub textures: Vec<MaterialTexture>,
    /// Authored material parameters, including runtime-map slots and lighting
    /// overrides. These are distinct from ShaderMap IDs and sampled images.
    pub integers: std::collections::BTreeMap<String, i32>,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct LegacyMaterialColors {
    pub ambient: [f32; 3],
    pub diffuse: [f32; 3],
    pub specular: [f32; 3],
    pub emissive: [f32; 3],
    pub shininess: f32,
    pub alpha: f32,
}
pub struct CachedSubmesh {
    pub(crate) source_attributes:
        Option<Result<std::sync::Arc<crate::nif_vertex_attributes::Source>, String>>,
    pub extra_uvs:
        Option<std::sync::Arc<std::collections::BTreeMap<u32, std::sync::Arc<Vec<[f32; 2]>>>>>,
    /// Original position indices; absent for legacy geometry without provenance.
    pub source_indices: Option<Vec<u32>>,
    /// Row-major authored mesh-to-model transform, applied after deformation.
    pub mesh_to_model: [[f32; 4]; 4],
    pub hierarchy_path: Option<Vec<u32>>,
    pub colour_indices: Option<Vec<u32>>,
    pub colour_count: Option<usize>,
    pub uv_transform: Option<crate::nif_textures::UvTransform>,
    pub material: std::sync::Arc<MaterialBindings>,
    pub sampler: super::viewport3d::TextureSampler,
    pub mesh_index: usize,
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub uvs: Option<Vec<[f32; 2]>>,
    pub indices: std::sync::Arc<Vec<u32>>,
    pub texture_name: Option<String>,
    pub center: [f32; 3],
    pub v_data: std::sync::Arc<Vec<f32>>,
}

pub struct CachedNifScene {
    pub submeshes: Vec<CachedSubmesh>,
    pub min_pos: [f32; 3],
    pub max_pos: [f32; 3],
    pub bbox_radius: f32,
    pub center_pos: [f32; 3],
    pub total_verts: usize,
    pub total_tris: usize,
}

pub(crate) fn build_cached_scene(nif: &NifFile) -> CachedNifScene {
    let world_mats = compute_world_transforms(nif);
    let mut paths = HashMap::new();
    if let Some(Some(root)) = nif.roots.first() {
        let mut queue = vec![(*root, Vec::new())];
        while let Some((index, path)) = queue.pop() {
            if paths.contains_key(&index) {
                continue;
            }
            paths.insert(index, path.clone());
            if let Some(block) = nif.blocks.get(index as usize) {
                for (slot, child) in block.children().iter().enumerate() {
                    if let Some(child) = child {
                        let mut p = path.clone();
                        p.push(slot as u32);
                        queue.push((*child, p));
                    }
                }
            }
        }
    }
    let mut submeshes = Vec::new();
    let mut min_pos = [f32::MAX, f32::MAX, f32::MAX];
    let mut max_pos = [f32::MIN, f32::MIN, f32::MIN];
    let mut total_verts = 0;
    let mut total_tris = 0;

    for (idx, block) in nif.blocks.iter().enumerate() {
        let mesh_data = match block {
            NiObject::NiMesh(mesh_data) => Some(mesh_data),
            NiObject::NiPSParticleSystem(ps_data) => Some(&ps_data.base),
            _ => None,
        };
        if let Some(mesh_data) = mesh_data {
            let (geometry, colour_indices, source_indices, extra_uvs) =
                match crate::wii_mesh::extract(nif, mesh_data) {
                    Ok(decoded) => (
                        Some(decoded.mesh),
                        decoded.colour_indices,
                        Some(decoded.source_indices),
                        (!decoded.extra_uvs.is_empty()).then(|| {
                            std::sync::Arc::new(
                                decoded
                                    .extra_uvs
                                    .into_iter()
                                    .map(|(channel, values)| (channel, std::sync::Arc::new(values)))
                                    .collect(),
                            )
                        }),
                    ),
                    Err(_) => (extract_mesh(nif, mesh_data), None, None, None),
                };
            if let Some(extracted) = geometry {
                let mut surface = find_mesh_surface(nif, idx);
                surface.material.has_vertex_colors =
                    extracted.colors.as_ref().is_some_and(|c| !c.is_empty());
                let mat = world_mats[idx];
                let source_attributes = match crate::nif_vertex_attributes::extract(
                    nif,
                    mesh_data,
                    source_indices.as_deref(),
                    mat.m,
                ) {
                    Ok(Some(source)) => Some(Ok(std::sync::Arc::new(source))),
                    Ok(None) => None,
                    Err(error) => Some(Err(format!("{error:#}"))),
                };
                let mut mesh_min = [f32::MAX, f32::MAX, f32::MAX];
                let mut mesh_max = [f32::MIN, f32::MIN, f32::MIN];

                let transformed: Vec<[f32; 3]> = extracted
                    .positions
                    .iter()
                    .map(|&p| {
                        let pt = mat.transform_point(p);
                        min_pos[0] = min_pos[0].min(pt[0]);
                        min_pos[1] = min_pos[1].min(pt[1]);
                        min_pos[2] = min_pos[2].min(pt[2]);
                        max_pos[0] = max_pos[0].max(pt[0]);
                        max_pos[1] = max_pos[1].max(pt[1]);
                        max_pos[2] = max_pos[2].max(pt[2]);
                        mesh_min[0] = mesh_min[0].min(pt[0]);
                        mesh_min[1] = mesh_min[1].min(pt[1]);
                        mesh_min[2] = mesh_min[2].min(pt[2]);
                        mesh_max[0] = mesh_max[0].max(pt[0]);
                        mesh_max[1] = mesh_max[1].max(pt[1]);
                        mesh_max[2] = mesh_max[2].max(pt[2]);
                        pt
                    })
                    .collect();

                total_verts += transformed.len();
                total_tris += extracted.indices.len() / 3;

                let submesh_center = [
                    (mesh_min[0] + mesh_max[0]) * 0.5,
                    (mesh_min[1] + mesh_max[1]) * 0.5,
                    (mesh_min[2] + mesh_max[2]) * 0.5,
                ];

                let tex_name = surface.texture;
                let name = block.name().to_string();

                let clean_indices = extracted
                    .indices
                    .chunks_exact(3)
                    .filter(|tri| {
                        tri.iter().all(|&i| {
                            transformed
                                .get(i as usize)
                                .is_some_and(|p| p.iter().all(|v| v.is_finite()))
                        })
                    })
                    .flat_map(|tri| tri.iter().copied())
                    .collect::<Vec<_>>();

                let mut computed_normals = vec![[0.0f32; 3]; transformed.len()];
                let authored_normals = extracted
                    .normals
                    .as_ref()
                    .filter(|ns| ns.len() == transformed.len())
                    .and_then(|ns| {
                        ns.iter()
                            .map(|n| mat.transform_normal(*n))
                            .collect::<Option<Vec<_>>>()
                    });
                if let Some(normals) = authored_normals {
                    computed_normals = normals;
                } else {
                    for chunk in clean_indices.chunks_exact(3) {
                        let i0 = chunk[0] as usize;
                        let i1 = chunk[1] as usize;
                        let i2 = chunk[2] as usize;
                        if i0 < transformed.len()
                            && i1 < transformed.len()
                            && i2 < transformed.len()
                        {
                            let p0 = transformed[i0];
                            let p1 = transformed[i1];
                            let p2 = transformed[i2];
                            let v01 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
                            let v02 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
                            let n = [
                                v01[1] * v02[2] - v01[2] * v02[1],
                                v01[2] * v02[0] - v01[0] * v02[2],
                                v01[0] * v02[1] - v01[1] * v02[0],
                            ];
                            for i in [i0, i1, i2] {
                                computed_normals[i][0] += n[0];
                                computed_normals[i][1] += n[1];
                                computed_normals[i][2] += n[2];
                            }
                        }
                    }
                    for n in &mut computed_normals {
                        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                        if len > 0.00001 {
                            n[0] /= len;
                            n[1] /= len;
                            n[2] /= len;
                        }
                    }
                }

                let mut v_data = Vec::with_capacity(transformed.len() * 12);
                let uvs_ref = extracted.uvs.as_ref();
                let colors_ref = extracted.colors.as_ref();
                for i in 0..transformed.len() {
                    let p = transformed[i];
                    let n = computed_normals[i];
                    let u = uvs_ref
                        .and_then(|us| us.get(i))
                        .copied()
                        .unwrap_or([0.0, 0.0]);
                    let c = colors_ref
                        .and_then(|cs| cs.get(i))
                        .copied()
                        .unwrap_or([1.0, 1.0, 1.0, 1.0]);
                    v_data.extend_from_slice(&[
                        p[0], p[1], p[2], n[0], n[1], n[2], u[0], u[1], c[0], c[1], c[2], c[3],
                    ]);
                }

                submeshes.push(CachedSubmesh {
                    source_attributes,
                    extra_uvs,
                    hierarchy_path: paths.get(&(idx as u32)).cloned(),
                    colour_indices,
                    colour_count: mesh_data
                        .stream_refs
                        .iter()
                        .find(|r| {
                            r.semantics
                                .iter()
                                .any(|s| s.name == "COLOR" && s.index == 0)
                        })
                        .and_then(|r| {
                            let NiObject::NiDataStream(ds) =
                                nif.blocks.get(r.stream_link as usize)?
                            else {
                                return None;
                            };
                            ds.regions
                                    .get(r.submesh_region_indices.first().copied().unwrap_or(0)
                                        as usize)
                                    .map(|r| r.count as usize)
                        }),
                    sampler: surface.sampler,
                    material: std::sync::Arc::new(surface.material),
                    uv_transform: surface.transform,
                    mesh_index: idx,
                    source_indices,
                    mesh_to_model: mat.m,
                    name,
                    positions: transformed,
                    uvs: extracted.uvs,
                    indices: std::sync::Arc::new(clean_indices),
                    texture_name: tex_name,
                    center: submesh_center,
                    v_data: std::sync::Arc::new(v_data),
                });
            }
        }
    }

    if total_verts == 0 {
        return CachedNifScene {
            submeshes: Vec::new(),
            min_pos: [0.0; 3],
            max_pos: [0.0; 3],
            bbox_radius: 1.0,
            center_pos: [0.0; 3],
            total_verts: 0,
            total_tris: 0,
        };
    }

    let center_pos = [
        (min_pos[0] + max_pos[0]) * 0.5,
        (min_pos[1] + max_pos[1]) * 0.5,
        (min_pos[2] + max_pos[2]) * 0.5,
    ];

    let dx = max_pos[0] - min_pos[0];
    let dy = max_pos[1] - min_pos[1];
    let dz = max_pos[2] - min_pos[2];
    let bbox_radius = (dx * dx + dy * dy + dz * dz).sqrt() * 0.5;

    CachedNifScene {
        submeshes,
        min_pos,
        max_pos,
        bbox_radius,
        center_pos,
        total_verts,
        total_tris,
    }
}

/// Effective properties, nearest node first. Child properties override the same
/// property type inherited from an ancestor. Cycles and dangling links terminate safely.
fn surface_properties(nif: &NifFile, selected: usize) -> Vec<(usize, usize)> {
    let mut parents = vec![None; nif.blocks.len()];
    for (index, block) in nif.blocks.iter().enumerate() {
        for child in block.children().iter().flatten() {
            if let Some(parent) = parents.get_mut(*child as usize) {
                if parent.is_none() {
                    *parent = Some(index);
                }
            }
        }
    }
    let mut result = Vec::new();
    let mut visited = std::collections::BTreeSet::new();
    let mut types = std::collections::BTreeSet::new();
    let mut current = Some(selected);
    while let Some(index) = current {
        if !visited.insert(index) {
            break;
        }
        let links = match nif.blocks.get(index) {
            Some(NiObject::NiMesh(m)) => &m.base.base.property_links,
            Some(NiObject::NiNode(n)) => &n.base.property_links,
            Some(NiObject::NiSortAdjustNode(n)) => &n.base.base.property_links,
            Some(NiObject::NiPSParticleSystem(p)) => &p.base.base.base.property_links,
            _ => break,
        };
        for link in links.iter().flatten() {
            if let Some(block) = nif.blocks.get(*link as usize) {
                if types.insert(block.type_name().to_string()) {
                    result.push((*link as usize, index));
                }
            }
        }
        current = parents.get(index).copied().flatten();
    }
    result
}
#[derive(Default)]
pub(crate) struct PreviewSurface {
    pub(crate) material: MaterialBindings,
    pub(crate) texture: Option<String>,
    pub(crate) transform: Option<crate::nif_textures::UvTransform>,
    pub(crate) sampler: super::viewport3d::TextureSampler,
}
pub(crate) fn find_mesh_surface(nif: &NifFile, mesh: usize) -> PreviewSurface {
    let render = nif.blocks.get(mesh).and_then(|b| match b {
        NiObject::NiMesh(m) => Some(&m.base),
        NiObject::NiPSParticleSystem(p) => Some(&p.base.base),
        _ => None,
    });
    let shader = render
        .and_then(|base| {
            base.materials
                .get(usize::try_from(base.active_material).ok()?)
        })
        .map(|m| m.name.clone());
    let integers = render
        .into_iter()
        .flat_map(|base| base.base.base.extra_data_links.iter().flatten())
        .filter_map(|index| match nif.blocks.get(*index as usize) {
            Some(NiObject::NiIntegerExtraData(data)) => Some((data.base.name.clone(), data.value)),
            Some(NiObject::NiBooleanExtraData(data)) if data.base.name == "R3MTStateOverride" => {
                Some((data.base.name.clone(), i32::from(data.value != 0)))
            }
            _ => None,
        })
        .collect();
    let attributes = render.and_then(|base| {
        match crate::nif_shader_attributes::extract(nif, &base.base.base.extra_data_links) {
            Ok(table) if table.is_empty() => None,
            Ok(table) => Some(Ok(std::sync::Arc::new(table))),
            Err(error) => Some(Err(format!("{error:#}"))),
        }
    });
    let mut result = PreviewSurface {
        material: MaterialBindings {
            texture_transforms: Default::default(),
            attributes,
            legacy_colors: None,
            legacy_diffuse: None,
            legacy_alpha_flags: None,
            legacy_alpha_threshold: None,
            // NiAVObject::DISABLE_SORTING applies to this render object.
            sort_disabled: render.is_some_and(|base| base.base.flags & 0x0040 != 0),
            legacy_depth_flags: None,
            legacy_texture_flags: None,
            legacy_stencil_flags: None,
            legacy_wireframe_flags: None,
            legacy_shade_flags: None,
            has_vertex_colors: false,
            vertex_color_flags: None,
            shader,
            textures: vec![],
            integers,
        },
        ..Default::default()
    };
    let properties = surface_properties(nif, mesh);
    // Texture decoding below can return early. Property-array order must not
    // determine whether the effective inherited vertex-color state is read.
    for &(index, _) in &properties {
        match &nif.blocks[index] {
            NiObject::NiMaterialProperty(property) => {
                let vector = |v: &epic_mickey_lib_rs::nif_file::Vector3| [v.x, v.y, v.z];
                result.material.legacy_colors = Some(LegacyMaterialColors {
                    ambient: vector(&property.ambient),
                    diffuse: vector(&property.diffuse),
                    specular: vector(&property.specular),
                    emissive: vector(&property.emissive),
                    shininess: property.shininess,
                    alpha: property.alpha,
                });
                result.material.legacy_diffuse = Some([
                    property.diffuse.x,
                    property.diffuse.y,
                    property.diffuse.z,
                    property.alpha,
                ]);
            }
            NiObject::NiAlphaProperty(property) => {
                result.material.legacy_alpha_flags = Some(property.flags);
                result.material.legacy_alpha_threshold = Some(property.threshold);
            }
            _ => {}
        }
        if let NiObject::Unknown(raw) = &nif.blocks[index] {
            if raw.type_name == "NiVertexColorProperty" {
                result.material.vertex_color_flags = Some(vertex_color_flags(
                    &raw.raw_data,
                    nif.source_is_little_endian,
                    nif.version,
                ));
            }
            if raw.type_name == "NiZBufferProperty" {
                result.material.legacy_depth_flags = Some(legacy_property_flags(
                    raw,
                    nif.source_is_little_endian,
                    nif.version,
                    Some(0),
                ));
            }
            let target = match raw.type_name.as_str() {
                "NiTexturingProperty" => Some((&mut result.material.legacy_texture_flags, None)),
                "NiStencilProperty" => Some((&mut result.material.legacy_stencil_flags, Some(8))),
                "NiWireframeProperty" => {
                    Some((&mut result.material.legacy_wireframe_flags, Some(0)))
                }
                "NiShadeProperty" => Some((&mut result.material.legacy_shade_flags, Some(0))),
                _ => None,
            };
            if let Some((target, tail)) = target {
                *target = Some(legacy_property_flags(
                    raw,
                    nif.source_is_little_endian,
                    nif.version,
                    tail,
                ));
            }
        }
    }
    for (index, _) in properties {
        if let NiObject::Unknown(raw) = &nif.blocks[index] {
            if raw.type_name == "NiTexturingProperty" {
                let maps = match crate::nif_textures::bindings(
                    &raw.raw_data,
                    nif.source_is_little_endian,
                    nif.version,
                ) {
                    Ok(maps) => maps,
                    Err(error) => {
                        result.material.legacy_texture_flags =
                            Some(Err(format!("NiTexturingProperty: {error:#}")));
                        return result;
                    }
                };
                for input in &maps {
                    let transform = crate::nif_textures::transform(
                        &raw.raw_data,
                        input,
                        nif.source_is_little_endian,
                        nif.version,
                    )
                    .map_err(|error| format!("{error:#}"));
                    if result
                        .material
                        .texture_transforms
                        .insert(
                            (input.shader_id.is_some(), input.slot as u32),
                            transform.clone(),
                        )
                        .is_some()
                    {
                        result.material.legacy_texture_flags =
                            Some(Err("Duplicate texture transform slot".into()));
                        return result;
                    }
                    if let Some(NiObject::NiSourceTexture(texture)) =
                        nif.blocks.get(input.source as usize)
                    {
                        if let Some(path) = texture.file_name.clone().filter(|s| !s.is_empty()) {
                            let at = input.source_offset + 4;
                            let b = raw.raw_data[at..at + 2].try_into().unwrap();
                            let flags = if nif.source_is_little_endian {
                                u16::from_le_bytes(b)
                            } else {
                                u16::from_be_bytes(b)
                            };
                            let mut sampler = super::viewport3d::TextureSampler::from_nif_flags(flags);
                            // The treehouse roof spans repeated S tiles. The Wii
                            // shader declares repeat, but this cooked roof map
                            // carries a stale ClampS sampler override.
                            if flags == 0x5200 && path.rsplit(['\\', '/']).next().is_some_and(|name| {
                                name.eq_ignore_ascii_case("AL_TreeHouse_RoofB1_Inert_tex.nif")
                            }) {
                                sampler.wrap_u = glow::REPEAT;
                            }
                            result.material.textures.push(MaterialTexture {
                                shader_id: input.shader_id,
                                uv_set: (flags & 255) as u8,
                                // ShaderMap::GetID is metadata, not its array
                                // index. Retail maps commonly all have ID 0.
                                slot: input.slot as u32,
                                path,
                                sampler,
                                transform: transform.ok().flatten(),
                            });
                        }
                    }
                }
                let Some(map) = maps
                    .iter()
                    .find(|m| m.shader_id.is_none() && m.slot == 0)
                    .or_else(|| maps.iter().find(|m| m.shader_id.is_some() && m.slot == 0))
                else {
                    return result;
                };
                let at = map.source_offset + 4;
                let bytes = raw.raw_data[at..at + 2].try_into().unwrap();
                let sampler = super::viewport3d::TextureSampler::from_nif_flags(
                    if nif.source_is_little_endian {
                        u16::from_le_bytes(bytes)
                    } else {
                        u16::from_be_bytes(bytes)
                    },
                );
                let texture = match nif.blocks.get(map.source as usize) {
                    Some(NiObject::NiSourceTexture(t)) => {
                        t.file_name.clone().filter(|s| !s.is_empty())
                    }
                    _ => None,
                };
                let transform = crate::nif_textures::transform(
                    &raw.raw_data,
                    map,
                    nif.source_is_little_endian,
                    nif.version,
                )
                .ok()
                .flatten();
                result.texture = texture;
                result.transform = transform;
                result.sampler = result.material.textures.iter()
                    .find(|binding| binding.slot == map.slot as u32 && binding.shader_id == map.shader_id)
                    .map_or(sampler, |binding| binding.sampler);
                return result;
            }
        }
    }
    result
}

fn vertex_color_flags(data: &[u8], little: bool, version: u32) -> Result<u16, String> {
    legacy_flags(data, little, version, Some(0), "NiVertexColorProperty")
}
fn legacy_property_flags(
    raw: &epic_mickey_lib_rs::nif_file::RawBlockData,
    little: bool,
    version: u32,
    tail: Option<usize>,
) -> Result<u16, String> {
    // Variable texture-map payloads are validated once by the texture loop,
    // after all inherited properties have been collected.
    legacy_flags(&raw.raw_data, little, version, tail, &raw.type_name)
}
fn legacy_flags(
    data: &[u8],
    little: bool,
    version: u32,
    tail: Option<usize>,
    kind: &str,
) -> Result<u16, String> {
    if !matches!(version, 0x14030109 | 0x14060500) {
        return Err(format!("Unsupported {kind} version {version:#x}"));
    }
    let count: [u8; 4] = data
        .get(4..8)
        .ok_or_else(|| format!("Truncated {kind} header"))?
        .try_into()
        .unwrap();
    let count = if little {
        u32::from_le_bytes(count)
    } else {
        u32::from_be_bytes(count)
    } as usize;
    let offset = count
        .checked_mul(4)
        .and_then(|n| n.checked_add(12))
        .ok_or_else(|| format!("{kind} offset overflow"))?;
    let end = offset
        .checked_add(2)
        .ok_or_else(|| format!("{kind} size overflow"))?;
    if data.len() < end || tail.is_some_and(|tail| end.checked_add(tail) != Some(data.len())) {
        return Err(format!("Invalid {kind} size"));
    }
    let bytes = data[offset..end].try_into().unwrap();
    Ok(if little {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    })
}

#[cfg(test)]
mod vertex_property_tests {
    #[test]
    fn inherited_legacy_colors_alpha_and_depth_survive_texture_early_return() {
        use epic_mickey_lib_rs::nif_file::{
            NiAlphaPropertyData, NiMaterialPropertyData, NiNodeData, NiObject, NifFile,
            RawBlockData, Vector3,
        };
        let mut parent = NiNodeData::default();
        parent.children = vec![Some(1)];
        parent.base.property_links = vec![Some(3), Some(4), Some(5)];
        let mut child = NiNodeData::default();
        child.base.property_links = vec![Some(2)];
        let mut depth = vec![0; 12];
        depth.extend(0xdu16.to_be_bytes());
        let mut colors = NiMaterialPropertyData::default();
        colors.ambient = Vector3 {
            x: 0.1,
            y: 0.2,
            z: 0.3,
        };
        colors.diffuse = Vector3 {
            x: 0.4,
            y: 0.5,
            z: 0.6,
        };
        colors.specular = Vector3 {
            x: 0.7,
            y: 0.8,
            z: 0.9,
        };
        colors.emissive = Vector3 {
            x: 1.,
            y: 0.25,
            z: 0.75,
        };
        colors.shininess = 12.;
        colors.alpha = 0.5;
        let mut nif = NifFile {
            version: 0x14060500,
            blocks: vec![
                NiObject::NiNode(parent),
                NiObject::NiNode(child),
                NiObject::Unknown(RawBlockData {
                    type_name: "NiTexturingProperty".into(),
                    raw_data: vec![0; 22],
                }),
                NiObject::NiMaterialProperty(colors),
                NiObject::NiAlphaProperty(NiAlphaPropertyData {
                    flags: 3073,
                    threshold: 37,
                    ..Default::default()
                }),
                NiObject::Unknown(RawBlockData {
                    type_name: "NiZBufferProperty".into(),
                    raw_data: depth,
                }),
            ],
            ..Default::default()
        };
        let state = super::find_mesh_surface(&nif, 1).material;
        let colors = state.legacy_colors.unwrap();
        assert_eq!(colors.ambient, [0.1, 0.2, 0.3]);
        assert_eq!(colors.diffuse, [0.4, 0.5, 0.6]);
        assert_eq!(colors.specular, [0.7, 0.8, 0.9]);
        assert_eq!(colors.emissive, [1., 0.25, 0.75]);
        assert_eq!(colors.shininess, 12.);
        assert_eq!(colors.alpha, 0.5);
        assert_eq!(state.legacy_alpha_flags, Some(3073));
        assert_eq!(state.legacy_alpha_threshold, Some(37));
        assert_eq!(state.legacy_depth_flags, Some(Ok(0xd)));
        // A malformed child override must remain an error, not silently fall
        // back to the parent's valid depth state.
        nif.blocks.push(NiObject::Unknown(RawBlockData {
            type_name: "NiZBufferProperty".into(),
            raw_data: vec![0; 13],
        }));
        if let NiObject::NiNode(child) = &mut nif.blocks[1] {
            child.base.property_links.push(Some(6));
        }
        assert!(super::find_mesh_surface(&nif, 1)
            .material
            .legacy_depth_flags
            .unwrap()
            .is_err());
    }
    #[test]
    fn texture_property_order_does_not_hide_inherited_vertex_state() {
        use epic_mickey_lib_rs::nif_file::{NiNodeData, NiObject, NifFile, RawBlockData};
        let mut color = vec![0; 12];
        color.extend(0x18u16.to_be_bytes());
        let mut parent = NiNodeData::default();
        parent.children = vec![Some(1)];
        parent.base.property_links = vec![Some(3)];
        let mut node = NiNodeData::default();
        node.base.property_links = vec![Some(2)];
        let mut nif = NifFile {
            version: 0x14060500,
            blocks: vec![
                NiObject::NiNode(parent),
                NiObject::NiNode(node),
                NiObject::Unknown(RawBlockData {
                    type_name: "NiTexturingProperty".into(),
                    raw_data: vec![0; 22],
                }),
                NiObject::Unknown(RawBlockData {
                    type_name: "NiVertexColorProperty".into(),
                    raw_data: color,
                }),
            ],
            ..Default::default()
        };
        // A valid empty texture property returns before the old loop reaches
        // the inherited color property. Both properties must still resolve.
        assert_eq!(
            super::find_mesh_surface(&nif, 1)
                .material
                .vertex_color_flags,
            Some(Ok(0x18))
        );
        if let NiObject::NiNode(node) = &mut nif.blocks[1] {
            node.base.property_links = vec![Some(3), Some(2)];
        }
        assert_eq!(
            super::find_mesh_surface(&nif, 1)
                .material
                .vertex_color_flags,
            Some(Ok(0x18))
        );
        if let NiObject::NiNode(node) = &mut nif.blocks[1] {
            node.base.property_links.reverse();
        }
        assert_eq!(
            super::find_mesh_surface(&nif, 1)
                .material
                .vertex_color_flags,
            Some(Ok(0x18))
        );
        if let NiObject::Unknown(raw) = &mut nif.blocks[2] {
            raw.raw_data.push(0);
        }
        let material = super::find_mesh_surface(&nif, 1).material;
        assert_eq!(material.vertex_color_flags, Some(Ok(0x18)));
        assert!(material.legacy_texture_flags.unwrap().is_err());
    }
    #[test]
    fn decodes_endian_and_extra_links_without_guessing_offsets() {
        for little in [false, true] {
            let mut data = vec![0; 4];
            data.extend(if little {
                2u32.to_le_bytes()
            } else {
                2u32.to_be_bytes()
            });
            data.extend([0; 12]);
            data.extend(if little {
                0x1234u16.to_le_bytes()
            } else {
                0x1234u16.to_be_bytes()
            });
            assert_eq!(
                super::vertex_color_flags(&data, little, 0x14060500).unwrap(),
                0x1234
            );
            data.pop();
            assert!(super::vertex_color_flags(&data, little, 0x14060500).is_err());
        }
    }
}

fn vec3_sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn vec3_cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn vec3_dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn vec3_normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 0.00001 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, 0.0, 1.0]
    }
}

#[derive(Clone, Copy)]
struct Mat4 {
    m: [[f32; 4]; 4],
}

impl Mat4 {
    fn identity() -> Self {
        Self {
            m: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }

    fn from_transform(trans: &NiTransform) -> Self {
        let r = &trans.rotation.rows;
        let s = trans.scale;
        let t = &trans.translation;
        Self {
            m: [
                [r[0][0] * s, r[0][1] * s, r[0][2] * s, t.x],
                [r[1][0] * s, r[1][1] * s, r[1][2] * s, t.y],
                [r[2][0] * s, r[2][1] * s, r[2][2] * s, t.z],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }

    fn mul(&self, other: &Mat4) -> Mat4 {
        let mut res = [[0.0f32; 4]; 4];
        for r in 0..4 {
            for c in 0..4 {
                res[r][c] = self.m[r][0] * other.m[0][c]
                    + self.m[r][1] * other.m[1][c]
                    + self.m[r][2] * other.m[2][c]
                    + self.m[r][3] * other.m[3][c];
            }
        }
        Mat4 { m: res }
    }

    fn transform_normal(&self, normal: [f32; 3]) -> Option<[f32; 3]> {
        // Inverse transpose of the affine linear part. Translation never affects normals.
        let m = self.m.map(|row| row.map(f64::from));
        let cofactor = [
            [
                m[1][1] * m[2][2] - m[1][2] * m[2][1],
                m[1][2] * m[2][0] - m[1][0] * m[2][2],
                m[1][0] * m[2][1] - m[1][1] * m[2][0],
            ],
            [
                m[0][2] * m[2][1] - m[0][1] * m[2][2],
                m[0][0] * m[2][2] - m[0][2] * m[2][0],
                m[0][1] * m[2][0] - m[0][0] * m[2][1],
            ],
            [
                m[0][1] * m[1][2] - m[0][2] * m[1][1],
                m[0][2] * m[1][0] - m[0][0] * m[1][2],
                m[0][0] * m[1][1] - m[0][1] * m[1][0],
            ],
        ];
        let determinant = (0..3).map(|i| m[0][i] * cofactor[0][i]).sum::<f64>();
        if !determinant.is_finite() || determinant == 0. {
            return None;
        }
        let transformed = cofactor
            .map(|row| (0..3).map(|i| row[i] * f64::from(normal[i])).sum::<f64>() / determinant);
        let length = transformed.iter().map(|v| v * v).sum::<f64>().sqrt();
        if !length.is_finite() || length == 0. {
            return None;
        }
        Some(transformed.map(|v| (v / length) as f32))
    }

    fn transform_point(&self, p: [f32; 3]) -> [f32; 3] {
        let x = self.m[0][0] * p[0] + self.m[0][1] * p[1] + self.m[0][2] * p[2] + self.m[0][3];
        let y = self.m[1][0] * p[0] + self.m[1][1] * p[1] + self.m[1][2] * p[2] + self.m[1][3];
        let z = self.m[2][0] * p[0] + self.m[2][1] * p[1] + self.m[2][2] * p[2] + self.m[2][3];
        [x, y, z]
    }
}

fn compute_world_transforms(nif: &NifFile) -> Vec<Mat4> {
    let mut parents: Vec<Option<usize>> = vec![None; nif.blocks.len()];
    for (i, block) in nif.blocks.iter().enumerate() {
        for &child_opt in block.children() {
            if let Some(child_idx) = child_opt {
                if (child_idx as usize) < parents.len() {
                    parents[child_idx as usize] = Some(i);
                }
            }
        }
    }

    let mut world_transforms = vec![Mat4::identity(); nif.blocks.len()];
    for i in 0..nif.blocks.len() {
        let mut chain = Vec::new();
        let mut cur = i;
        let mut visited = std::collections::HashSet::from([i]);
        while let Some(p) = parents[cur] {
            if !visited.insert(p) {
                break;
            }
            chain.push(p);
            cur = p;
        }
        chain.reverse();

        let mut mat = Mat4::identity();
        for &idx in &chain {
            if let Some(av) = nif.blocks[idx].as_av_object() {
                mat = mat.mul(&Mat4::from_transform(&av.transform));
            }
        }
        if let Some(av) = nif.blocks[i].as_av_object() {
            mat = mat.mul(&Mat4::from_transform(&av.transform));
        }
        world_transforms[i] = mat;
    }
    world_transforms
}

// ─────────────────────────────────────────────────────────────────────────────
// PAK Texture Search & GX2 / BCn Texture Decoder
// ─────────────────────────────────────────────────────────────────────────────

pub(crate) fn find_pak_texture<'a>(
    pak_files: &'a [VirtualFile],
    file_name: &str,
) -> Option<&'a VirtualFile> {
    let clean = file_name.replace('\\', "/");
    let path1 = clean.trim_start_matches('/');
    let path2 = format!("{path1}_wii");
    let path_eq = |raw: &str, target: &str| -> bool {
        let a = raw.as_bytes();
        let b = target.as_bytes();
        if a.len() != b.len() {
            return false;
        }
        a.iter().zip(b.iter()).all(|(&ca, &cb)| {
            let na = if ca == b'\\' { b'/' } else { ca };
            na.eq_ignore_ascii_case(&cb)
        })
    };

    // 1. Full path match
    for vf in pak_files {
        if path_eq(&vf.path, path1) || path_eq(&vf.path, &path2) {
            return Some(vf);
        }
    }

    // 2. Basename / filename stem match
    let target_name = path1.rsplit('/').next().unwrap_or(path1);
    let target_name_wii = format!("{target_name}_wii");
    let target_stem = target_name
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(target_name);
    let target_stem_wii = format!("{target_stem}_wii");

    for vf in pak_files {
        let vf_name = vf
            .path
            .rsplit(|c| c == '/' || c == '\\')
            .next()
            .unwrap_or(&vf.path);
        let vf_stem = vf_name.rsplit_once('.').map(|(s, _)| s).unwrap_or(vf_name);

        if vf_name.eq_ignore_ascii_case(target_name)
            || vf_name.eq_ignore_ascii_case(&target_name_wii)
            || vf_stem.eq_ignore_ascii_case(target_stem)
            || vf_stem.eq_ignore_ascii_case(&target_stem_wii)
        {
            return Some(vf);
        }
    }

    None
}

// ─────────────────────────────────────────────────────────────────────────────
// Wii Morton Z-curve deswizzle (ported from D:\Repos\cheese\src\nif\textures.rs)
// ─────────────────────────────────────────────────────────────────────────────

/// Interleave bits of x and y (Morton / Z-curve) — exact port of
/// `CoordinateToMortonAddress(x, y, log_w, log_h)` from the EM2 Wii U decompilation.
#[inline]
fn coordinate_to_morton(mut x: u32, mut y: u32, mut log_w: u32, mut log_h: u32) -> u32 {
    let mut addr = 0u32;
    let mut bit = 0u32;
    while log_w + log_h > 0 {
        if log_w > 0 {
            log_w -= 1;
            addr |= (x & 1) << bit;
            bit += 1;
            x >>= 1;
        }
        if log_h > 0 {
            log_h -= 1;
            addr |= (y & 1) << bit;
            bit += 1;
            y >>= 1;
        }
    }
    addr
}

#[inline]
fn integer_log2(mut n: u32) -> u32 {
    let mut log = 0u32;
    while n > 1 {
        n >>= 1;
        log += 1;
    }
    log
}

/// Deswizzle a Wii Morton-order texture to linear (row-major).
/// `bpp` = bytes-per-pixel (1,2,4) or bytes-per-4×4-block (8,16) for BCn.
/// For BCn pass block counts as width/height.
fn wii_deswizzle(swizzled: &[u8], width: u32, height: u32, bpp: u32) -> Vec<u8> {
    let total = (width * height) as usize * bpp as usize;
    let mut linear = vec![0u8; total];
    let _log_w = integer_log2(width);
    let log_h = integer_log2(height);
    let (eff_w, addr_shift, elem_bytes): (u32, u32, u32) = match bpp {
        1 => (width, 0, 1),
        2 => (width, 1, 2),
        4 => (width, 2, 4),
        8 => (width * 2, 2, 4),  // 8-byte blocks (DXT1)
        16 => (width * 4, 2, 4), // 16-byte blocks (DXT3/5)
        _ => (width, 2, 4),
    };
    let eff_log_w = integer_log2(eff_w);
    for y in 0..height {
        for x in 0..eff_w {
            let morton = coordinate_to_morton(x, y, eff_log_w, log_h);
            let src_off = (morton << addr_shift) as usize;
            let dst_off = (y * eff_w + x) as usize * elem_bytes as usize;
            let n = elem_bytes as usize;
            if src_off + n <= swizzled.len() && dst_off + n <= linear.len() {
                linear[dst_off..dst_off + n].copy_from_slice(&swizzled[src_off..src_off + n]);
            }
        }
    }
    linear
}

fn decode_565(val: u16) -> [u8; 4] {
    let mut r = (((val >> 11) & 0x1F) << 3) as u8;
    let mut g = (((val >> 5) & 0x3F) << 2) as u8;
    let mut b = ((val & 0x1F) << 3) as u8;
    r |= r >> 5;
    g |= g >> 6;
    b |= b >> 5;
    [r, g, b, 255]
}

/// CMPR stores four 4×4 blocks in each 8×8 tile, in reading order.
fn decompress_wii_cmpr(data: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let (width, height) = (width as usize, height as usize);
    let columns = width.div_ceil(8);
    let rows = height.div_ceil(8);
    let data = data.get(..columns.checked_mul(rows)?.checked_mul(32)?)?;
    let mut out = vec![0; width.checked_mul(height)?.checked_mul(4)?];
    for (tile, bytes) in data.chunks_exact(32).enumerate() {
        let tile_x = (tile % columns) * 8;
        let tile_y = (tile / columns) * 8;
        for (cell, block) in bytes.chunks_exact(8).enumerate() {
            let c0 = u16::from_be_bytes([block[0], block[1]]);
            let c1 = u16::from_be_bytes([block[2], block[3]]);
            let a = decode_565(c0);
            let b = decode_565(c1);
            let blend = |weight_a, weight_b, divisor| {
                let mut color = [255; 4];
                for i in 0..3 {
                    color[i] = ((u16::from(a[i]) * weight_a + u16::from(b[i]) * weight_b) / divisor) as u8;
                }
                color
            };
            let (c, d) = if c0 > c1 {
                (blend(5u16, 3u16, 8u16), blend(3u16, 5u16, 8u16))
            } else {
                let average = blend(1u16, 1u16, 2u16);
                let mut transparent = average;
                transparent[3] = 0;
                (average, transparent)
            };
            let palette = [a, b, c, d];
            let bx = tile_x + (cell % 2) * 4;
            let by = tile_y + (cell / 2) * 4;
            for (py, &selectors) in block[4..].iter().enumerate().take(height.saturating_sub(by)) {
                for px in 0..4.min(width.saturating_sub(bx)) {
                    let code = (selectors >> (6 - px * 2)) & 3;
                    let at = ((by + py) * width + bx + px) * 4;
                    out[at..at + 4].copy_from_slice(&palette[code as usize]);
                }
            }
        }
    }
    Some(out)
}

/// Decode a Wii (Wii, not Wii U) texture block.
pub(crate) fn decode_wii_texture(
    tex: &NiWiiUPersistentSrcTextureRendererData,
) -> Option<(Vec<u8>, u32, u32)> {
    let (width, height) = if let Some(m) = tex.mip_levels.first() {
        (m.width, m.height)
    } else if tex.width > 0 && tex.height > 0 {
        (tex.width, tex.height)
    } else {
        (0, 0)
    };
    if width == 0 || height == 0 {
        return None;
    }

    let ni_fmt = tex.pixel_format.format;
    // Indexed data requires the owning NIF's linked NiPalette. Treating its
    // indices as intensity silently destroys authored colors/transparency.
    if matches!(ni_fmt, 2 | 3) {
        return None;
    }
    let gx2 = tex.gx2_format;
    let data = &tex.raw_pixels;

    let bpp = if tex.pixel_format.bits_per_pixel > 0 {
        tex.pixel_format.bits_per_pixel as u32 / 8
    } else {
        4
    };

    // Full GX tile format decoders
    let rgba = if ni_fmt == 12 && matches!(tex.pixel_format.bits_per_pixel, 8 | 16) {
        // NiPixelFormat::FORMAT_TWO_CHANNEL + Wii tiling represents IA44
        // or IA88 here, not RGB565. Retail hoseDrops is IA88 (format 12,
        // 16 bpp); decoding it as RGB produced opaque, colored rectangles.
        let (tw, th, bytes) = if tex.pixel_format.bits_per_pixel == 16 {
            (4u32, 4u32, 32u64)
        } else {
            (8, 4, 32)
        };
        let required = u64::from(width.div_ceil(tw)) * u64::from(height.div_ceil(th)) * bytes;
        if required > data.len() as u64 {
            return None;
        }
        if tex.pixel_format.bits_per_pixel == 16 {
            gx_decode_ia8(data, width, height)
        } else {
            gx_decode_ia4(data, width, height)
        }
    } else if gx2 == 0x31 || ni_fmt == 14 || ni_fmt == 4 || ni_fmt == 5 {
        decompress_wii_cmpr(data, width, height)?
    } else if bpp == 2 {
        if ni_fmt == 1 || ni_fmt == 5 || ni_fmt == 6 {
            gx_decode_rgb5a3(data, width, height)
        } else {
            gx_decode_rgb565(data, width, height)
        }
    } else if bpp == 1 {
        if ni_fmt == 2 {
            gx_decode_ia4(data, width, height)
        } else {
            gx_decode_i8(data, width, height)
        }
    } else if bpp == 4 {
        gx_decode_rgba8(data, width, height)
    } else {
        // Fallback for bpp == 0 or others (like 4-bit)
        gx_decode_i4(data, width, height)
    };

    #[cfg(target_os = "horizon")]
    {
        const MAX_3DS_DIM: u32 = 64;
        if width > MAX_3DS_DIM || height > MAX_3DS_DIM {
            let nw = width.min(MAX_3DS_DIM).max(1);
            let nh = height.min(MAX_3DS_DIM).max(1);
            let mut small = vec![0u8; (nw * nh * 4) as usize];
            for dy in 0..nh {
                let sy = ((dy as u64 * height as u64) / nh as u64).min(height as u64 - 1) as usize;
                for dx in 0..nw {
                    let sx = ((dx as u64 * width as u64) / nw as u64).min(width as u64 - 1) as usize;
                    let src = (sy * width as usize + sx) * 4;
                    let dst = (dy as usize * nw as usize + dx as usize) * 4;
                    if src + 4 <= rgba.len() {
                        small[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
                    }
                }
            }
            return Some((small, nw, nh));
        }
    }

    Some((rgba, width, height))
}

// ── GX tile format decoders ──────────────────────────────────────────────────
fn decode_wii_texture_in_nif(
    tex: &NiWiiUPersistentSrcTextureRendererData,
    nif: &NifFile,
) -> Option<(Vec<u8>, u32, u32)> {
    if !matches!(tex.pixel_format.format, 2 | 3) {
        return decode_wii_texture(tex);
    }
    let NiObject::Unknown(palette) = nif.blocks.get(tex.palette_link? as usize)? else {
        return None;
    };
    if palette.type_name != "NiPalette" {
        return None;
    }
    decode_wii_paletted(tex, &palette.raw_data, nif.source_is_little_endian)
}
fn decode_wii_paletted(
    tex: &NiWiiUPersistentSrcTextureRendererData,
    raw: &[u8],
    little: bool,
) -> Option<(Vec<u8>, u32, u32)> {
    let count_bytes: [u8; 4] = raw.get(1..5)?.try_into().ok()?;
    let count = if little {
        u32::from_le_bytes(count_bytes)
    } else {
        u32::from_be_bytes(count_bytes)
    } as usize;
    if count == 0 || count > 256 || raw.len() != 5 + count * 4 {
        return None;
    }
    let mip = tex.mip_levels.first()?;
    let (w, h) = (mip.width, mip.height);
    if w == 0 || h == 0 {
        return None;
    }
    let bits = tex.pixel_format.bits_per_pixel;
    let (tw, th) = match bits {
        4 => (8u32, 8u32),
        8 => (8, 4),
        _ => return None,
    };
    let length = (w.div_ceil(tw) as usize)
        .checked_mul(h.div_ceil(th) as usize)?
        .checked_mul(32)?;
    let start = mip.offset_in_bytes as usize;
    let data = tex.raw_pixels.get(start..start.checked_add(length)?)?;
    let mut rgba = vec![0; (w as usize).checked_mul(h as usize)?.checked_mul(4)?];
    let mut tile = 0usize;
    for y in (0..h).step_by(th as usize) {
        for x in (0..w).step_by(tw as usize) {
            for py in 0..th {
                for px in 0..tw {
                    let pixel = (py * tw + px) as usize;
                    let index = if bits == 4 {
                        let byte = data[tile + pixel / 2];
                        if pixel % 2 == 0 {
                            byte >> 4
                        } else {
                            byte & 15
                        }
                    } else {
                        data[tile + pixel]
                    } as usize;
                    if index >= count {
                        return None;
                    }
                    if x + px < w && y + py < h {
                        let out = (((y + py) * w + x + px) * 4) as usize;
                        rgba[out..out + 4].copy_from_slice(&raw[5 + index * 4..9 + index * 4]);
                    }
                }
            }
            tile += 32;
        }
    }
    Some((rgba, w, h))
}

fn gx_decode_i4(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (tw, th) = (8u32, 8u32);
    let mut out = vec![0u8; (width * height * 4) as usize];
    let mut src = 0usize;
    for ty in 0..(height + th - 1) / th {
        for tx in 0..(width + tw - 1) / tw {
            for py in 0..th {
                for px in (0..tw).step_by(2) {
                    if src >= data.len() {
                        break;
                    }
                    let byte = data[src];
                    src += 1;
                    for (half, dx) in [(byte >> 4, 0u32), (byte & 0xF, 1u32)] {
                        let i = half * 17;
                        let (fx, fy) = (tx * tw + px + dx, ty * th + py);
                        if fx < width && fy < height {
                            let o = ((fy * width + fx) * 4) as usize;
                            out[o..o + 4].copy_from_slice(&[i, i, i, i]);
                        }
                    }
                }
            }
        }
    }
    out
}

fn gx_decode_i8(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (tw, th) = (8u32, 4u32);
    let mut out = vec![0u8; (width * height * 4) as usize];
    let mut src = 0usize;
    for ty in 0..(height + th - 1) / th {
        for tx in 0..(width + tw - 1) / tw {
            for py in 0..th {
                for px in 0..tw {
                    if src >= data.len() {
                        break;
                    }
                    let i = data[src];
                    src += 1;
                    let (fx, fy) = (tx * tw + px, ty * th + py);
                    if fx < width && fy < height {
                        let o = ((fy * width + fx) * 4) as usize;
                        out[o..o + 4].copy_from_slice(&[i, i, i, i]);
                    }
                }
            }
        }
    }
    out
}

fn gx_decode_ia4(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (tw, th) = (8u32, 4u32);
    let mut out = vec![0u8; (width * height * 4) as usize];
    let mut src = 0usize;
    for ty in 0..(height + th - 1) / th {
        for tx in 0..(width + tw - 1) / tw {
            for py in 0..th {
                for px in 0..tw {
                    if src >= data.len() {
                        break;
                    }
                    let byte = data[src];
                    src += 1;
                    let a = (byte >> 4) * 17;
                    let i = (byte & 0xF) * 17;
                    let (fx, fy) = (tx * tw + px, ty * th + py);
                    if fx < width && fy < height {
                        let o = ((fy * width + fx) * 4) as usize;
                        out[o..o + 4].copy_from_slice(&[i, i, i, a]);
                    }
                }
            }
        }
    }
    out
}

fn gx_decode_ia8(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (tw, th) = (4u32, 4u32);
    let mut out = vec![0u8; (width * height * 4) as usize];
    let mut src = 0usize;
    for ty in 0..(height + th - 1) / th {
        for tx in 0..(width + tw - 1) / tw {
            for py in 0..th {
                for px in 0..tw {
                    if src + 1 >= data.len() {
                        src += 2;
                        continue;
                    }
                    let (a, i) = (data[src], data[src + 1]);
                    src += 2;
                    let (fx, fy) = (tx * tw + px, ty * th + py);
                    if fx < width && fy < height {
                        let o = ((fy * width + fx) * 4) as usize;
                        out[o..o + 4].copy_from_slice(&[i, i, i, a]);
                    }
                }
            }
        }
    }
    out
}

fn gx_decode_rgb565(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (tw, th) = (4u32, 4u32);
    let mut out = vec![0u8; (width * height * 4) as usize];
    let mut src = 0usize;
    for ty in 0..(height + th - 1) / th {
        for tx in 0..(width + tw - 1) / tw {
            for py in 0..th {
                for px in 0..tw {
                    if src + 1 > data.len() {
                        src += 2;
                        continue;
                    }
                    let val = u16::from_be_bytes([data[src], data[src + 1]]);
                    src += 2;
                    let [r, g, b, _] = decode_565(val);
                    let (fx, fy) = (tx * tw + px, ty * th + py);
                    if fx < width && fy < height {
                        let o = ((fy * width + fx) * 4) as usize;
                        out[o..o + 4].copy_from_slice(&[r, g, b, 255]);
                    }
                }
            }
        }
    }
    out
}

/// GX RGB5A3 — high bit=1 → RGB555 opaque, high bit=0 → RGB4A3
fn gx_decode_rgb5a3(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (tw, th) = (4u32, 4u32);
    let mut out = vec![0u8; (width * height * 4) as usize];
    let mut src = 0usize;
    for ty in 0..(height + th - 1) / th {
        for tx in 0..(width + tw - 1) / tw {
            for py in 0..th {
                for px in 0..tw {
                    if src + 1 > data.len() {
                        src += 2;
                        continue;
                    }
                    let val = u16::from_be_bytes([data[src], data[src + 1]]);
                    src += 2;
                    let (r, g, b, a) = if val & 0x8000 != 0 {
                        let r5 = ((val >> 10) & 0x1F) as u8;
                        let g5 = ((val >> 5) & 0x1F) as u8;
                        let b5 = (val & 0x1F) as u8;
                        (r5 * 8 + r5 / 4, g5 * 8 + g5 / 4, b5 * 8 + b5 / 4, 255u8)
                    } else {
                        let a3 = ((val >> 12) & 0x7) as u8;
                        let r4 = ((val >> 8) & 0xF) as u8;
                        let g4 = ((val >> 4) & 0xF) as u8;
                        let b4 = (val & 0xF) as u8;
                        (r4 * 17, g4 * 17, b4 * 17, a3 * 36 + a3 / 2)
                    };
                    let (fx, fy) = (tx * tw + px, ty * th + py);
                    if fx < width && fy < height {
                        let o = ((fy * width + fx) * 4) as usize;
                        out[o..o + 4].copy_from_slice(&[r, g, b, a]);
                    }
                }
            }
        }
    }
    out
}

/// GX RGBA8 — 4x4 tiles, 64 bytes each: 32 bytes AR pairs then 32 bytes GB pairs
fn gx_decode_rgba8(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (tw, th) = (4u32, 4u32);
    let mut out = vec![0u8; (width * height * 4) as usize];
    let mut src = 0usize;
    for ty in 0..(height + th - 1) / th {
        for tx in 0..(width + tw - 1) / tw {
            if src + 64 > data.len() {
                src += 64;
                continue;
            }
            let tile = &data[src..src + 64];
            src += 64;
            for py in 0..th {
                for px in 0..tw {
                    let pi = (py * tw + px) as usize;
                    let (a, r) = (tile[pi * 2], tile[pi * 2 + 1]);
                    let (g, b) = (tile[32 + pi * 2], tile[32 + pi * 2 + 1]);
                    let (fx, fy) = (tx * tw + px, ty * th + py);
                    if fx < width && fy < height {
                        let o = ((fy * width + fx) * 4) as usize;
                        out[o..o + 4].copy_from_slice(&[r, g, b, a]);
                    }
                }
            }
        }
    }
    out
}

pub(crate) fn decode_wiiu_texture(
    tex: &NiWiiUPersistentSrcTextureRendererData,
) -> Option<(Vec<u8>, u32, u32)> {
    let (width, height) = if let Some(m) = tex.mip_levels.first() {
        (m.width, m.height)
    } else if tex.width > 0 && tex.height > 0 {
        (tex.width, tex.height)
    } else {
        (0, 0)
    };
    if width == 0 || height == 0 {
        return None;
    }

    let gx2_format = tex.gx2_format;

    let format = wiiu_swizzle::SurfaceFormat::from_repr(gx2_format)
        .unwrap_or(wiiu_swizzle::SurfaceFormat::Invalid);
    if format == wiiu_swizzle::SurfaceFormat::Invalid {
        return None;
    }

    let tile_mode = wiiu_swizzle::TileMode::from_repr(tex.pixel_format.tiling)
        .unwrap_or(wiiu_swizzle::TileMode::D2TiledThin1);
    let swizzle = tex.pixel_format.extra_data & 0xFFFFFF;

    let (block_w, _) = format.block_dim();
    let pitch_in_blocks = (width + block_w - 1) / block_w;

    let surface = wiiu_swizzle::Gx2Surface {
        dim: wiiu_swizzle::SurfaceDim::D2,
        width,
        height,
        depth_or_array_layers: 1,
        mipmap_count: 1,
        format,
        aa: wiiu_swizzle::AaMode::X1,
        usage: 1,
        image_data: &tex.raw_pixels,
        mipmap_data: &[],
        tile_mode,
        swizzle,
        alignment: 4096,
        pitch: pitch_in_blocks,
        mipmap_offsets: [0; 13],
    };

    let linear = surface.deswizzle().ok()?;

    if matches!(gx2_format, 0x31..=0x35) {
        if let Some(rgba) = bcn_to_rgba8(&linear, width, height, gx2_format, false) {
            return Some((rgba, width, height));
        }
    } else if gx2_format == 0x1a {
        return Some((linear, width, height));
    }

    None
}

fn bcn_to_rgba8(
    bcn: &[u8],
    width: u32,
    height: u32,
    gx2_format: u32,
    is_be: bool,
) -> Option<Vec<u8>> {
    let blk_w = (width.max(1) + 3) / 4;
    let blk_h = (height.max(1) + 3) / 4;
    let mut rgba = vec![0u8; (width * height * 4) as usize];

    match gx2_format {
        0x31 => {
            // BC1 / DXT1
            for by in 0..blk_h {
                for bx in 0..blk_w {
                    let src = ((by * blk_w + bx) * 8) as usize;
                    if src + 8 > bcn.len() {
                        continue;
                    }
                    decode_bc1_block(&bcn[src..src + 8], bx, by, width, height, &mut rgba, is_be);
                }
            }
        }
        0x33 => {
            // BC3 / DXT5
            for by in 0..blk_h {
                for bx in 0..blk_w {
                    let src = ((by * blk_w + bx) * 16) as usize;
                    if src + 16 > bcn.len() {
                        continue;
                    }
                    decode_bc3_block(&bcn[src..src + 16], bx, by, width, height, &mut rgba, is_be);
                }
            }
        }
        0x35 => {
            // BC5
            for by in 0..blk_h {
                for bx in 0..blk_w {
                    let src = ((by * blk_w + bx) * 16) as usize;
                    if src + 16 > bcn.len() {
                        continue;
                    }
                    decode_bc5_block(&bcn[src..src + 16], bx, by, width, height, &mut rgba, is_be);
                }
            }
        }
        _ => return None,
    }
    Some(rgba)
}

fn decode_bc1_block(
    block: &[u8],
    bx: u32,
    by: u32,
    w: u32,
    h: u32,
    out: &mut Vec<u8>,
    is_be: bool,
) {
    let c0 = if is_be {
        u16::from_be_bytes([block[0], block[1]])
    } else {
        u16::from_le_bytes([block[0], block[1]])
    };
    let c1 = if is_be {
        u16::from_be_bytes([block[2], block[3]])
    } else {
        u16::from_le_bytes([block[2], block[3]])
    };
    let lut = if is_be {
        u32::from_be_bytes([block[4], block[5], block[6], block[7]])
    } else {
        u32::from_le_bytes([block[4], block[5], block[6], block[7]])
    };
    let colors = bc1_palette(c0, c1);
    for py in 0..4u32 {
        for px in 0..4u32 {
            let tx = bx * 4 + px;
            let ty = by * 4 + py;
            if tx >= w || ty >= h {
                continue;
            }
            let code = (lut >> ((py * 4 + px) * 2)) & 3;
            let [r, g, b, a] = colors[code as usize];
            let idx = ((ty * w + tx) * 4) as usize;
            if idx + 3 < out.len() {
                out[idx] = r;
                out[idx + 1] = g;
                out[idx + 2] = b;
                out[idx + 3] = a;
            }
        }
    }
}

fn decode_bc3_block(
    block: &[u8],
    bx: u32,
    by: u32,
    w: u32,
    h: u32,
    out: &mut Vec<u8>,
    is_be: bool,
) {
    let a_palette = bc4_palette(block[0], block[1]);
    let a_bits: u64 = if is_be {
        u64::from_be_bytes([
            0, 0, block[2], block[3], block[4], block[5], block[6], block[7],
        ])
    } else {
        (block[2] as u64)
            | ((block[3] as u64) << 8)
            | ((block[4] as u64) << 16)
            | ((block[5] as u64) << 24)
            | ((block[6] as u64) << 32)
            | ((block[7] as u64) << 40)
    };
    let c0 = if is_be {
        u16::from_be_bytes([block[8], block[9]])
    } else {
        u16::from_le_bytes([block[8], block[9]])
    };
    let c1 = if is_be {
        u16::from_be_bytes([block[10], block[11]])
    } else {
        u16::from_le_bytes([block[10], block[11]])
    };
    let lut = if is_be {
        u32::from_be_bytes([block[12], block[13], block[14], block[15]])
    } else {
        u32::from_le_bytes([block[12], block[13], block[14], block[15]])
    };
    let colors = bc1_palette(c0, c1);
    for py in 0..4u32 {
        for px in 0..4u32 {
            let tx = bx * 4 + px;
            let ty = by * 4 + py;
            if tx >= w || ty >= h {
                continue;
            }
            let i = (py * 4 + px) as u64;
            let a_code = (a_bits >> (i * 3)) & 7;
            let c_code = (lut >> (i * 2)) & 3;
            let [r, g, b, _] = colors[c_code as usize];
            let a = a_palette[a_code as usize];
            let idx = ((ty * w + tx) * 4) as usize;
            if idx + 3 < out.len() {
                out[idx] = r;
                out[idx + 1] = g;
                out[idx + 2] = b;
                out[idx + 3] = a;
            }
        }
    }
}

fn decode_bc5_block(
    block: &[u8],
    bx: u32,
    by: u32,
    w: u32,
    h: u32,
    out: &mut Vec<u8>,
    is_be: bool,
) {
    let r_palette = bc4_palette(block[0], block[1]);
    let r_bits: u64 = if is_be {
        u64::from_be_bytes([
            0, 0, block[2], block[3], block[4], block[5], block[6], block[7],
        ])
    } else {
        (block[2] as u64)
            | ((block[3] as u64) << 8)
            | ((block[4] as u64) << 16)
            | ((block[5] as u64) << 24)
            | ((block[6] as u64) << 32)
            | ((block[7] as u64) << 40)
    };
    let g_palette = bc4_palette(block[8], block[9]);
    let g_bits: u64 = if is_be {
        u64::from_be_bytes([
            0, 0, block[10], block[11], block[12], block[13], block[14], block[15],
        ])
    } else {
        (block[10] as u64)
            | ((block[11] as u64) << 8)
            | ((block[12] as u64) << 16)
            | ((block[13] as u64) << 24)
            | ((block[14] as u64) << 32)
            | ((block[15] as u64) << 40)
    };
    for py in 0..4u32 {
        for px in 0..4u32 {
            let tx = bx * 4 + px;
            let ty = by * 4 + py;
            if tx >= w || ty >= h {
                continue;
            }
            let i = (py * 4 + px) as u64;
            let r = r_palette[((r_bits >> (i * 3)) & 7) as usize];
            let g = g_palette[((g_bits >> (i * 3)) & 7) as usize];
            let idx = ((ty * w + tx) * 4) as usize;
            if idx + 3 < out.len() {
                out[idx] = r;
                out[idx + 1] = g;
                out[idx + 2] = 0;
                out[idx + 3] = 255;
            }
        }
    }
}

fn bc1_palette(c0: u16, c1: u16) -> [[u8; 4]; 4] {
    let expand5 = |v: u16| -> u8 { ((v * 527 + 23) >> 6) as u8 };
    let expand6 = |v: u16| -> u8 { ((v * 259 + 33) >> 6) as u8 };
    let r0 = expand5((c0 >> 11) & 0x1F);
    let g0 = expand6((c0 >> 5) & 0x3F);
    let b0 = expand5(c0 & 0x1F);
    let r1 = expand5((c1 >> 11) & 0x1F);
    let g1 = expand6((c1 >> 5) & 0x3F);
    let b1 = expand5(c1 & 0x1F);
    if c0 > c1 {
        [
            [r0, g0, b0, 255],
            [r1, g1, b1, 255],
            [
                lerp_u8(r0, r1, 1, 3),
                lerp_u8(g0, g1, 1, 3),
                lerp_u8(b0, b1, 1, 3),
                255,
            ],
            [
                lerp_u8(r0, r1, 2, 3),
                lerp_u8(g0, g1, 2, 3),
                lerp_u8(b0, b1, 2, 3),
                255,
            ],
        ]
    } else {
        [
            [r0, g0, b0, 255],
            [r1, g1, b1, 255],
            [
                lerp_u8(r0, r1, 1, 2),
                lerp_u8(g0, g1, 1, 2),
                lerp_u8(b0, b1, 1, 2),
                255,
            ],
            [0, 0, 0, 0],
        ]
    }
}

fn bc4_palette(e0: u8, e1: u8) -> [u8; 8] {
    if e0 > e1 {
        [
            e0,
            e1,
            lerp_u8(e0, e1, 1, 7),
            lerp_u8(e0, e1, 2, 7),
            lerp_u8(e0, e1, 3, 7),
            lerp_u8(e0, e1, 4, 7),
            lerp_u8(e0, e1, 5, 7),
            lerp_u8(e0, e1, 6, 7),
        ]
    } else {
        [
            e0,
            e1,
            lerp_u8(e0, e1, 1, 5),
            lerp_u8(e0, e1, 2, 5),
            lerp_u8(e0, e1, 3, 5),
            lerp_u8(e0, e1, 4, 5),
            0,
            255,
        ]
    }
}

fn lerp_u8(a: u8, b: u8, n: u32, d: u32) -> u8 {
    ((a as u32 * (d - n) + b as u32 * n + d / 2) / d) as u8
}

pub(crate) fn decode_image_bytes(data: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    if let Ok(nif) = NifFile::load_bytes(data) {
        for (block_index, block) in nif.blocks.iter().enumerate() {
            if let NiObject::Unknown(raw) = block {
                if raw.type_name == "NiPixelData" {
                    if let Ok(image) =
                        pixel_data::decode(&raw.raw_data, nif.source_is_little_endian, nif.version)
                    {
                        return Some(image);
                    }
                }
            }
            if let NiObject::NiWiiUTextureData(tex) = block {
                let type_name = nif
                    .block_type_indices
                    .get(block_index)
                    .and_then(|&i| nif.block_types.get(i as usize))
                    .map(String::as_str);
                if type_name == Some("NiPersistentSrcTextureRendererData")
                    && nif.source_is_little_endian
                    && matches!(tex.target_platform, 4 | 8)
                {
                    // Modern PC platform 4 is DX9; legacy Wii also used 4.
                    // Endianness and block class distinguish these serialized layouts.
                    // Unsupported PC formats must never fall through to GX2.
                    let ranges = crate::nif_save::block_ranges(data).ok()?;
                    let raw = data.get(ranges.get(block_index)?.clone())?;
                    return pixel_data::decode_persistent(raw, true, nif.version).ok();
                }
                let is_wii = tex.target_platform == 4;
                if is_wii {
                    if let Some(res) = decode_wii_texture_in_nif(tex, &nif) {
                        return Some(res);
                    }
                } else {
                    if let Some(res) = decode_wiiu_texture(tex) {
                        return Some(res);
                    }
                }
            }
        }
    }
    let decoded = image::load_from_memory(data)
        .or_else(|_| image::load_from_memory_with_format(data, image::ImageFormat::Tga));
    if let Ok(img) = decoded {
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        return Some((rgba.into_raw(), w, h));
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// 3D Viewport Renderer with Back-Face Culling & Directional Shading
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod wii_intensity_alpha_tests {
    use super::*;
    #[test]
    fn intensity_formats_replicate_intensity_into_alpha() {
        let mut i8 = vec![0; 32];
        i8[..3].copy_from_slice(&[0, 127, 255]);
        assert_eq!(
            gx_decode_i8(&i8, 3, 1),
            [0, 0, 0, 0, 127, 127, 127, 127, 255, 255, 255, 255]
        );
        let mut i4 = vec![0; 32];
        i4[..2].copy_from_slice(&[0x07, 0xf0]);
        assert_eq!(
            gx_decode_i4(&i4, 3, 1),
            [0, 0, 0, 0, 119, 119, 119, 119, 255, 255, 255, 255]
        );
    }
    #[test]
    fn indexed_wii_tiles_use_linked_rgba_palette_and_reject_bad_payloads() {
        let mut palette = vec![1];
        palette.extend_from_slice(&16u32.to_be_bytes());
        for index in 0..16 {
            palette.extend_from_slice(&[index, 40 + index, 200 - index, index * 7]);
        }
        let mut tex = NiWiiUPersistentSrcTextureRendererData::default();
        tex.pixel_format.format = 2;
        tex.pixel_format.bits_per_pixel = 4;
        tex.mip_levels.push(epic_mickey_lib_rs::nif_file::MipLevel {
            width: 9,
            height: 1,
            offset_in_bytes: 0,
        });
        tex.raw_pixels = vec![0; 64];
        tex.raw_pixels[0] = 0x12;
        tex.raw_pixels[32] = 0x30;
        let (rgba, _, _) = decode_wii_paletted(&tex, &palette, false).unwrap();
        assert_eq!(&rgba[..8], &[1, 41, 199, 7, 2, 42, 198, 14]);
        assert_eq!(&rgba[32..36], &[3, 43, 197, 21]);
        tex.pixel_format.bits_per_pixel = 8;
        tex.raw_pixels[0] = 1;
        tex.raw_pixels[1] = 2;
        tex.raw_pixels[32] = 3;
        assert_eq!(decode_wii_paletted(&tex, &palette, false).unwrap().0, rgba);
        tex.raw_pixels[0] = 255;
        assert!(decode_wii_paletted(&tex, &palette, false).is_none());
        tex.raw_pixels[0] = 1;
        tex.raw_pixels.pop();
        assert!(decode_wii_paletted(&tex, &palette, false).is_none());
        assert!(
            decode_wii_texture(&tex).is_none(),
            "Palette indices must not fall back to intensity"
        );
    }
    #[test]
    fn two_channel_wii_texture_preserves_intensity_and_alpha_across_tiles() {
        let mut tex = NiWiiUPersistentSrcTextureRendererData::default();
        tex.target_platform = 4;
        tex.width = 5;
        tex.height = 2;
        tex.pixel_format.format = 12;
        tex.pixel_format.bits_per_pixel = 16;
        tex.raw_pixels = [vec![0u8, 200].repeat(16), vec![127u8, 80].repeat(16)].concat();
        let (rgba, w, h) = decode_wii_texture(&tex).unwrap();
        assert_eq!((w, h), (5, 2));
        assert_eq!(&rgba[0..4], &[200, 200, 200, 0]);
        assert_eq!(&rgba[16..20], &[80, 80, 80, 127]);
        assert_eq!(&rgba[36..40], &[80, 80, 80, 127]);
        tex.raw_pixels.pop();
        assert!(decode_wii_texture(&tex).is_none());
    }
    #[test]
    fn two_channel_four_bit_channels_and_truncated_pair() {
        let mut tex = NiWiiUPersistentSrcTextureRendererData::default();
        tex.target_platform = 4;
        tex.width = 1;
        tex.height = 1;
        tex.pixel_format.format = 12;
        tex.pixel_format.bits_per_pixel = 8;
        tex.raw_pixels = vec![0xa3; 32];
        assert_eq!(decode_wii_texture(&tex).unwrap().0, vec![51, 51, 51, 170]);
        assert_eq!(gx_decode_ia8(&[1], 1, 1), vec![0, 0, 0, 0]);
    }
}

#[cfg(test)]
mod normal_transform_tests {
    use super::*;
    #[test]
    fn normal_transform_respects_rotation_nonuniform_scale_and_reflection() {
        let matrix = Mat4 {
            m: [
                [0., -3., 0., 100.],
                [2., 0., 0., 200.],
                [0., 0., 4., 300.],
                [0., 0., 0., 1.],
            ],
        };
        let n = matrix.transform_normal([1., 1., 0.]).unwrap();
        let tangent = [-3., -2., 0.];
        assert!((n[0] * tangent[0] + n[1] * tangent[1]).abs() < 1e-6);
        assert!((n.iter().map(|v| v * v).sum::<f32>() - 1.).abs() < 1e-6);
        let reflected = Mat4 {
            m: [
                [-2., 0., 0., 0.],
                [0., 2., 0., 0.],
                [0., 0., 2., 0.],
                [0., 0., 0., 1.],
            ],
        };
        assert_eq!(
            reflected.transform_normal([1., 0., 0.]).unwrap(),
            [-1., 0., 0.]
        );
        let collapsed = Mat4 { m: [[0.; 4]; 4] };
        assert!(collapsed.transform_normal([1., 0., 0.]).is_none());
        assert!(matrix.transform_normal([f32::NAN, 0., 0.]).is_none());
    }
}
