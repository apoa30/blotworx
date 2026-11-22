//! Static Wii display lists with independent attribute indices. In particular,
//! colour seams must survive vertex welding for BKLT colour-stream replacement.
use anyhow::{ensure, Context, Result};
use epic_mickey_lib_rs::nif_file::*;

/// Scene NIF-path properties can point at an animation manager. Follow its
/// model reference without attempting to parse the KFM bytes as a NIF.
pub fn resolve_model<'a>(
    files: &'a [epic_mickey_lib_rs::packfile::VirtualFile],
    path: &str,
) -> Option<&'a epic_mickey_lib_rs::packfile::VirtualFile> {
    let clean = |p: &str| {
        let p = p
            .replace('\\', "/")
            .trim_matches(&['/', '"', ' '][..])
            .to_ascii_lowercase();
        let mut parts = Vec::new();
        for part in p.split('/') {
            match part {
                "" | "." => {}
                ".." if parts.last().is_some_and(|p| *p != "..") => {
                    parts.pop();
                }
                _ => parts.push(part),
            }
        }
        parts.join("/")
    };
    let matches_clean = |raw: &str, target: &str| -> bool {
        let trimmed = raw.trim_matches(&['/', '\\', '"', ' '][..]);
        if trimmed.contains("..") || trimmed.contains("//") || trimmed.contains("\\\\") || trimmed.contains("/.") || trimmed.contains("\\.") {
            return clean(raw) == target;
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
    };
    let mut current = clean(path);
    let mut visited = std::collections::HashSet::new();
    for _ in 0..8 {
        if !visited.insert(current.clone()) {
            return None;
        }
        let current_wii = format!("{current}_wii");
        let file = files.iter().find(|f| {
            matches_clean(&f.path, &current) || matches_clean(&f.path, &current_wii)
        })?;
        if !matches!(
            file.path.to_ascii_lowercase().rsplit('.').next(),
            Some("kfm" | "kfm_wii")
        ) {
            return Some(file);
        }
        let manager = epic_mickey_lib_rs::kfm_file::KfmFile::load_bytes(&file.data).ok()?;
        let model = clean(&manager.model_path);
        if model.is_empty() {
            return None;
        }
        let model_wii = format!("{model}_wii");
        if files.iter().any(|f| {
            matches_clean(&f.path, &model) || matches_clean(&f.path, &model_wii)
        }) {
            current = model;
        } else {
            let owner = clean(&file.path);
            current = clean(&format!("{}/{}", owner.rsplit_once('/')?.0, model));
        }
    }
    None
}
pub struct Decoded {
    pub extra_uvs: std::collections::BTreeMap<u32, Vec<[f32; 2]>>,
    pub mesh: ExtractedMesh,
    pub source_indices: Vec<u32>,
    pub colour_indices: Option<Vec<u32>>,
}
struct Attribute {
    kind: u8,
    index: u32,
    values: Vec<[f32; 4]>,
}
fn primitives(bytes: &[u8], counts: &[usize], width: usize) -> Result<Vec<(u8, Vec<Vec<usize>>)>> {
    let mut p = 0;
    let mut result = Vec::new();
    while p < bytes.len() {
        let command = bytes[p];
        p += 1;
        if command == 0 {
            continue;
        }
        let primitive = command & 0xf8;
        ensure!(
            matches!(primitive, 0x80 | 0x90 | 0x98 | 0xa0),
            "Unsupported display-list command {command:#x}"
        );
        let n = u16::from_be_bytes(
            bytes
                .get(p..p + 2)
                .context("Truncated primitive")?
                .try_into()?,
        ) as usize;
        p += 2;
        ensure!(
            n > 0 && n <= bytes.len().saturating_sub(p) / (counts.len() * width),
            "Invalid primitive size"
        );
        let mut vertices = Vec::with_capacity(n);
        for _ in 0..n {
            let mut tuple = Vec::with_capacity(counts.len());
            for &count in counts {
                let index = if width == 1 {
                    bytes[p] as usize
                } else {
                    u16::from_be_bytes(bytes[p..p + 2].try_into()?) as usize
                };
                p += width;
                ensure!(index < count, "Attribute index out of range");
                tuple.push(index);
            }
            vertices.push(tuple);
        }
        ensure!(
            (primitive != 0x80 || n % 4 == 0) && (primitive != 0x90 || n % 3 == 0),
            "Incomplete primitive"
        );
        result.push((primitive, vertices));
    }
    ensure!(!result.is_empty(), "Empty display list");
    Ok(result)
}
pub fn extract(nif: &NifFile, mesh: &NiMeshData) -> Result<Decoded> {
    if !mesh
        .stream_refs
        .iter()
        .any(|r| r.semantics.iter().any(|s| s.name == "DISPLAYLIST"))
    {
        let (output, source_indices) = extract_mesh_with_source_indices(nif, mesh)
            .context("Missing source-index-preserving mesh geometry")?;
        let colour_indices = mesh
            .stream_refs
            .iter()
            .find(|r| {
                r.semantics
                    .iter()
                    .any(|s| s.name == "COLOR" && s.index == 0)
            })
            .and_then(|r| {
                if r.submesh_region_indices.len() > 1 {
                    return None;
                }
                let NiObject::NiDataStream(ds) = nif.blocks.get(r.stream_link as usize)? else {
                    return None;
                };
                let region = ds
                    .regions
                    .get(r.submesh_region_indices.first().copied().unwrap_or(0) as usize)?;
                (region.count as usize == output.vertex_count).then(|| (0..region.count).collect())
            });
        return Ok(Decoded {
            extra_uvs: secondary_linear_uvs(nif, mesh, &source_indices)?,
            mesh: output,
            source_indices,
            colour_indices,
        });
    }

    ensure!(
        mesh.hw_skin_flag == 0 && mesh.modifiers.iter().flatten().all(|id|
            nif.blocks.get(*id as usize).is_some_and(|block|block.type_name()=="NiMorphMeshModifier")),
        "Display-list modifier requires an unsupported matrix/index layout"
    );
    let mut attributes = Vec::new();
    let mut display = None;
    for r in &mesh.stream_refs {
        ensure!(
            r.submesh_region_indices.len() <= 1,
            "Multiple display-list regions are not yet supported"
        );
        let NiObject::NiDataStream(stream) = nif
            .blocks
            .get(r.stream_link as usize)
            .context("Missing stream")?
        else {
            anyhow::bail!("Not a data stream");
        };
        let region = stream
            .regions
            .get(r.submesh_region_indices.first().copied().unwrap_or(0) as usize)
            .context("Missing stream region")?;
        let start = region.start_index as usize;
        let count = region.count as usize;
        for (i, semantic) in r.semantics.iter().enumerate() {
            let kind = match semantic.name.as_str() {
                "POSITION" => 0,
                "NORMAL" => 1,
                "COLOR" => 2,
                "TEXCOORD" => 3,
                "DISPLAYLIST" => {
                    display = Some(
                        stream
                            .buffer
                            .get(start..start + count)
                            .context("Truncated display list")?,
                    );
                    continue;
                }
                // These streams are consumed by the morph modifier, not
                // indexed attributes in GX vertex records. Keep POSITION's
                // source mapping for the live deformation runtime.
                "INDEX" | "MORPH_POSITION" | "MORPHWEIGHTS" => continue,
                _ => anyhow::bail!("Unsupported vertex semantic"),
            };
            ensure!(
                i < stream.component_formats.len(),
                "Missing component format"
            );
            let stride = vertex_stride(stream);
            ensure!(
                stride > 0
                    && start
                        .checked_add(count)
                        .is_some_and(|end| end <= stream.buffer.len() / stride),
                "Truncated attribute stream"
            );
            let values = match kind {
                0 | 1 => extract_f32x3_stream(nif.source_is_little_endian, stream, i, start, count)
                    .into_iter()
                    .map(|v| [v[0], v[1], v[2], 1.])
                    .collect(),
                2 => extract_f32x4_stream(nif.source_is_little_endian, stream, i, start, count),
                _ => extract_f32x2_stream(nif.source_is_little_endian, stream, i, start, count)
                    .into_iter()
                    .map(|v| [v[0], v[1], 0., 1.])
                    .collect(),
            };
            attributes.push(Attribute {
                kind,
                index: semantic.index,
                values,
            });
        }
    }
    attributes.sort_by_key(|a| (a.kind, a.index));
    ensure!(
        attributes.first().is_some_and(|a| a.kind == 0),
        "Missing position input"
    );
    let display = display.context("Not a display-list mesh")?;
    let counts = attributes
        .iter()
        .map(|a| a.values.len())
        .collect::<Vec<_>>();
    let parsed = match (
        primitives(display, &counts, 1),
        primitives(display, &counts, 2),
    ) {
        (Ok(a), Err(_)) | (Err(_), Ok(a)) => a,
        (Ok(_), Ok(_)) => anyhow::bail!("Ambiguous display-list index width"),
        (Err(_), Err(e)) => return Err(e),
    };
    let normal = attributes.iter().position(|a| a.kind == 1);
    let colour = attributes.iter().position(|a| a.kind == 2 && a.index == 0);
    let uv = attributes.iter().position(|a| a.kind == 3 && a.index == 0);
    let mut output = ExtractedMesh {
        positions: vec![],
        normals: normal.map(|_| vec![]),
        uvs: uv.map(|_| vec![]),
        colors: colour.map(|_| vec![]),
        indices: vec![],
        vertex_count: 0,
    };
    let mut colour_indices = colour.map(|_| Vec::new());
    let mut source_indices = Vec::new();
    let mut extra_uvs: std::collections::BTreeMap<u32, Vec<[f32; 2]>> = attributes
        .iter()
        .filter(|a| a.kind == 3 && a.index != 0)
        .map(|a| (a.index, Vec::new()))
        .collect();
    ensure!(
        extra_uvs.keys().all(|&channel| channel < 8),
        "UV semantic exceeds GX texture-coordinate inputs"
    );
    let mut unique = std::collections::HashMap::new();
    for (primitive, tuples) in parsed {
        let mut ids = Vec::with_capacity(tuples.len());
        for tuple in tuples {
            let next = output.positions.len() as u32;
            let index = *unique.entry(tuple.clone()).or_insert_with(|| {
                let p = attributes[0].values[tuple[0]];
                output.positions.push([p[0], p[1], p[2]]);
                source_indices.push(tuple[0] as u32);
                for (i, attribute) in attributes
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| a.kind == 3 && a.index != 0)
                {
                    let value = attribute.values[tuple[i]];
                    extra_uvs
                        .get_mut(&attribute.index)
                        .unwrap()
                        .push([value[0], value[1]]);
                }
                if let Some(i) = normal {
                    let v = attributes[i].values[tuple[i]];
                    output.normals.as_mut().unwrap().push([v[0], v[1], v[2]]);
                }
                if let Some(i) = uv {
                    let v = attributes[i].values[tuple[i]];
                    output.uvs.as_mut().unwrap().push([v[0], v[1]]);
                }
                if let Some(i) = colour {
                    output
                        .colors
                        .as_mut()
                        .unwrap()
                        .push(attributes[i].values[tuple[i]]);
                    colour_indices.as_mut().unwrap().push(tuple[i] as u32);
                }
                next
            });
            ids.push(index);
        }
        let mut triangle = |a, b, c| {
            if a != b && b != c && a != c {
                output.indices.extend([a, b, c]);
            }
        };
        match primitive {
            0x80 => {
                for q in ids.chunks_exact(4) {
                    triangle(q[0], q[1], q[2]);
                    triangle(q[0], q[2], q[3]);
                }
            }
            0x90 => {
                for t in ids.chunks_exact(3) {
                    triangle(t[0], t[1], t[2]);
                }
            }
            0x98 => {
                for i in 2..ids.len() {
                    if i % 2 == 0 {
                        triangle(ids[i - 2], ids[i - 1], ids[i]);
                    } else {
                        triangle(ids[i - 1], ids[i - 2], ids[i]);
                    }
                }
            }
            _ => {
                for i in 2..ids.len() {
                    triangle(ids[0], ids[i - 1], ids[i]);
                }
            }
        }
    }
    output.vertex_count = output.positions.len();
    Ok(Decoded {
        extra_uvs,
        mesh: output,
        source_indices,
        colour_indices,
    })
}
fn secondary_linear_uvs(
    nif: &NifFile,
    mesh: &NiMeshData,
    sources: &[u32],
) -> Result<std::collections::BTreeMap<u32, Vec<[f32; 2]>>> {
    let mut output = std::collections::BTreeMap::new();
    for reference in &mesh.stream_refs {
        for (component, semantic) in reference
            .semantics
            .iter()
            .enumerate()
            .filter(|(_, s)| s.name == "TEXCOORD" && s.index != 0)
        {
            ensure!(
                semantic.index < 8 && reference.submesh_region_indices.len() <= 1,
                "Secondary UV regions need unambiguous source mapping"
            );
            let Some(NiObject::NiDataStream(stream)) =
                nif.blocks.get(reference.stream_link as usize)
            else {
                anyhow::bail!("Missing secondary UV stream")
            };
            let region = stream
                .regions
                .get(
                    reference
                        .submesh_region_indices
                        .first()
                        .copied()
                        .unwrap_or(0) as usize,
                )
                .context("Missing secondary UV region")?;
            let values = extract_f32x2_stream(
                nif.source_is_little_endian,
                stream,
                component,
                region.start_index as usize,
                region.count as usize,
            );
            let mapped = sources
                .iter()
                .map(|&source| {
                    values
                        .get(source as usize)
                        .copied()
                        .context("Secondary UV source index out of range")
                })
                .collect::<Result<Vec<_>>>()?;
            ensure!(
                output.insert(semantic.index, mapped).is_none(),
                "Duplicate secondary UV semantic"
            );
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn retail_secondary_uv_indices_survive_display_list_vertex_mapping() -> anyhow::Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let pak = crate::pack_io::read_filtered(
            std::fs::read(std::path::Path::new(&root).join("packfiles/dbc_1st_zonea.pak"))?,
            |_, path| {
                path.to_ascii_lowercase()
                    .ends_with("/dbc_zonea_inert.nif_wii")
            },
        )?;
        let file = pak
            .pak
            .files
            .iter()
            .find(|f| !f.data.is_empty())
            .context("Missing DBC inert model")?;
        let mut nif = NifFile::load_bytes(&file.data).map_err(anyhow::Error::msg)?;
        for block in [1022, 1212, 1245, 1260] {
            let NiObject::NiMesh(mesh) = &nif.blocks[block] else {
                anyhow::bail!("Not a mesh")
            };
            let mesh = mesh.clone();
            let decoded = extract(&nif, &mesh)?;
            let uv = decoded
                .extra_uvs
                .get(&1)
                .context("Secondary UV stream discarded")?;
            assert_eq!(uv.len(), decoded.mesh.vertex_count);
            assert!(uv.iter().flatten().all(|v| v.is_finite()));
            assert!(
                uv.iter().any(|value| value != &uv[0]),
                "All secondary UVs collapsed"
            );
            let stream = mesh
                .stream_refs
                .iter()
                .find(|r| {
                    r.semantics
                        .iter()
                        .any(|s| s.name == "TEXCOORD" && s.index == 1)
                })
                .unwrap()
                .stream_link as usize;
            let NiObject::NiDataStream(data) = &mut nif.blocks[stream] else {
                anyhow::bail!("Missing UV stream")
            };
            assert_eq!(data.component_formats, [0x00020240]);
            let before = data.buffer.clone();
            for row in data.buffer.chunks_exact_mut(4) {
                row.copy_from_slice(&[0x03, 0xff, 0x02, 0x00]);
            }
            let changed = extract(&nif, &mesh)?;
            assert_eq!(
                changed.mesh.uvs, decoded.mesh.uvs,
                "Editing UV1 changed UV0"
            );
            assert!(
                changed.extra_uvs[&1]
                    .iter()
                    .all(|value| *value == [1., 512. / 1023.]),
                "UV1 mutation was lost or read from the primary stream"
            );
            let NiObject::NiDataStream(data) = &mut nif.blocks[stream] else {
                unreachable!()
            };
            data.buffer = before;
        }
        Ok(())
    }
    use super::*;
    #[test]
    fn animation_manager_resolves_relative_model_and_rejects_cycles() {
        use epic_mickey_lib_rs::packfile::{EndianDependentString, VirtualFile};
        let file = |path: &str, data: &[u8]| {
            VirtualFile::new(
                EndianDependentString::new("TEST".into()),
                false,
                0,
                path.into(),
                data.to_vec(),
            )
        };
        let mut files = vec![
            file(
                "Models/Managers/actor.kfm_wii",
                b";Gamebryo KFM File Version 1.2.2.0\n..\\actor.nif_wii Root\n",
            ),
            file("Models/actor.nif_wii", b"model"),
        ];
        assert_eq!(
            resolve_model(&files, "Models/Managers/actor.kfm")
                .unwrap()
                .data,
            b"model"
        );
        files[0].data = b";Gamebryo KFM File Version 1.2.2.0\n.\\actor.kfm_wii Root\n".to_vec();
        assert!(resolve_model(&files, "Models/Managers/actor.kfm").is_none());
    }
    #[test]
    fn independent_colour_indices_survive_display_list_parsing() {
        let bytes = [0x90, 0, 3, 0, 0, 0, 1, 1, 2];
        let parsed = primitives(&bytes, &[2, 3], 1).unwrap();
        assert_eq!(parsed[0].1, vec![vec![0, 0], vec![0, 1], vec![1, 2]]);
        assert!(primitives(&bytes, &[2, 2], 1).is_err());
        for end in 1..bytes.len() {
            assert!(primitives(&bytes[..end], &[2, 3], 1).is_err());
        }
        assert!(primitives(&[0x80, 0, 3, 0, 0, 0], &[1], 1).is_err());
        assert!(primitives(&[0x08, 0, 0, 0, 0], &[1], 1).is_err());
    }
}
