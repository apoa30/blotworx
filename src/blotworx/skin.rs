//! EM1 NiMesh trailing data. Keeps cooked values intact; matrix semantics are not inferred.
use anyhow::{ensure, Context, Result};
#[derive(Debug)]
pub struct Weight {
    pub bones: [i32; 3],
    pub values: [f32; 3],
}
#[derive(Debug)]
pub struct Partition {
    pub start: i32,
    pub end: i32,
    pub weights: [u16; 10],
}
#[derive(Debug)]
pub struct Skin {
    pub block: usize,
    pub has_weights: u32,
    pub transform_floats: Vec<f32>,
    pub weights: Vec<Weight>,
    pub vertex_weights: Vec<u32>,
    pub morph_width: usize,
    pub morph_deltas: Vec<f32>,
    pub morph_indices: Vec<u16>,
    pub mapped_primitives: Vec<u8>,
    pub partitions: Vec<Partition>,
    pub max_partition_index: u32,
}

/// Heap payload held by the decoded source arrays for memory diagnostics.
pub fn owned_buffer_bytes(skin: &Skin) -> usize {
    skin.transform_floats.capacity() * std::mem::size_of::<f32>()
        + skin.weights.capacity() * std::mem::size_of::<Weight>()
        + skin.vertex_weights.capacity() * std::mem::size_of::<u32>()
        + skin.morph_deltas.capacity() * std::mem::size_of::<f32>()
        + skin.morph_indices.capacity() * std::mem::size_of::<u16>()
        + skin.mapped_primitives.capacity() * std::mem::size_of::<u8>()
        + skin.partitions.capacity() * std::mem::size_of::<Partition>()
}

struct Reader<'a> {
    data: &'a [u8],
    little: bool,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(n <= self.data.len(), "Truncated cooked skin payload");
        let (a, b) = self.data.split_at(n);
        self.data = b;
        Ok(a)
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?.try_into()?;
        Ok(if self.little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }
    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?.try_into()?;
        Ok(if self.little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }
    fn f32(&mut self) -> Result<f32> {
        let v = f32::from_bits(self.u32()?);
        ensure!(v.is_finite(), "Non-finite skin value");
        Ok(v)
    }
    fn count(&mut self, stride: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        ensure!(
            n <= self.data.len() / stride,
            "Skin array count exceeds its block"
        );
        Ok(n)
    }
    fn skip_array(&mut self, stride: usize) -> Result<()> {
        let n = self.count(stride)?;
        self.take(n * stride)?;
        Ok(())
    }
    fn string(&mut self) -> Result<&'a [u8]> {
        let n = self.count(1)?;
        self.take(n)
    }
}
fn payload(r: &mut Reader<'_>, user: u32, block: usize) -> Result<Skin> {
    let has_weights = r.u32()?;
    let n = r.count(4)?;
    let transform_floats = (0..n).map(|_| r.f32()).collect::<Result<Vec<_>>>()?;
    let n = r.count(24)?;
    let mut weights = Vec::with_capacity(n);
    for _ in 0..n {
        weights.push(Weight {
            bones: [r.u32()? as i32, r.u32()? as i32, r.u32()? as i32],
            values: [r.f32()?, r.f32()?, r.f32()?],
        });
    }
    let n = r.count(4)?;
    let vertex_weights = (0..n).map(|_| r.u32()).collect::<Result<Vec<_>>>()?;
    let morph_count = r.u32()? as usize;
    let morph_width = r.u32()? as usize;
    let floats = morph_count
        .checked_mul(morph_width)
        .and_then(|n| n.checked_mul(3))
        .context("Morph dimensions overflow")?;
    ensure!(floats <= r.data.len() / 4, "Truncated morph deltas");
    let morph_deltas = (0..floats).map(|_| r.f32()).collect::<Result<Vec<_>>>()?;
    ensure!(morph_count <= r.data.len() / 2, "Truncated morph indices");
    let morph_indices = (0..morph_count)
        .map(|_| r.u16())
        .collect::<Result<Vec<_>>>()?;
    let n = if user < 17 {
        r.u16()? as usize
    } else {
        r.u32()? as usize
    };
    let mapped_primitives = r.take(n)?.to_vec();
    let n = r.count(28)?;
    let mut partitions = Vec::with_capacity(n);
    for _ in 0..n {
        let start = r.u32()? as i32;
        let end = r.u32()? as i32;
        let mut weights = [0; 10];
        for w in &mut weights {
            *w = r.u16()?;
        }
        partitions.push(Partition {
            start,
            end,
            weights,
        });
    }
    let max_partition_index = r.u32()?;
    ensure!(
        r.data.is_empty(),
        "Unparsed cooked skin bytes: {}",
        r.data.len()
    );
    if has_weights != 0 {
        ensure!(
            vertex_weights.iter().all(|&w| (w as usize) < weights.len()),
            "Vertex references absent skin weight"
        );
    }
    Ok(Skin {
        block,
        has_weights,
        transform_floats,
        weights,
        vertex_weights,
        morph_width,
        morph_deltas,
        morph_indices,
        mapped_primitives,
        partitions,
        max_partition_index,
    })
}
/// Extracts custom mesh payloads alongside the shared reader, without rewriting the asset.
pub fn read_nif(data: &[u8]) -> Result<Vec<Skin>> {
    let newline = data
        .iter()
        .position(|&b| b == b'\n')
        .context("Missing NIF header")?;
    let mut r = Reader {
        data: &data[newline + 1..],
        little: true,
    };
    ensure!(
        r.u32()? == 0x14060500,
        "Skin decoder requires EM1 NIF 20.6.5.0"
    );
    let little = r.byte()? != 0;
    let user = r.u32()?;
    let blocks = r.u32()? as usize;
    r.little = little;
    let n = r.u16()? as usize;
    let mut types = Vec::with_capacity(n);
    for _ in 0..n {
        types.push(r.string()?);
    }
    ensure!(blocks <= r.data.len() / 6, "Invalid NIF block count");
    let indices = (0..blocks).map(|_| r.u16()).collect::<Result<Vec<_>>>()?;
    let sizes = (0..blocks).map(|_| r.u32()).collect::<Result<Vec<_>>>()?;
    let strings = r.u32()? as usize;
    r.u32()?;
    ensure!(strings <= r.data.len() / 4, "Invalid NIF string count");
    for _ in 0..strings {
        r.string()?;
    }
    let groups = r.u32()? as usize;
    for _ in 1..groups {
        r.u32()?;
    }
    let mut skins = Vec::new();
    for (block, (&kind, &size)) in indices.iter().zip(&sizes).enumerate() {
        let bytes = r.take(size as usize)?;
        let kind = types.get(kind as usize).context("Invalid NIF block type")?;
        if *kind != b"NiMesh" {
            continue;
        }
        let mut m = Reader {
            data: bytes,
            little,
        };
        m.u32()?;
        m.skip_array(4)?;
        m.u32()?; // object name, extras, controller
        m.take(54)?;
        m.skip_array(4)?;
        m.u32()?; // AV flags, transform, properties, collision
        m.skip_array(8)?;
        m.take(5)?; // materials and active material
        m.take(12)?;
        if user > 14 {
            m.take(4)?;
        }
        if user > 3 {
            m.take(12)?;
        }
        m.take(19)?; // submeshes, instancing and bound
        let n = m.count(11)?;
        for _ in 0..n {
            m.take(5)?;
            let regions = m.u16()? as usize;
            m.take(regions * 2)?;
            m.skip_array(8)?;
        }
        m.skip_array(4)?; // modifiers
        if user >= 10 && m.byte()? != 0 {
            skins.push(
                payload(&mut m, user, block).with_context(|| format!("NiMesh block {block}"))?,
            );
        } else {
            ensure!(m.data.is_empty(), "Unparsed unskinned NiMesh block {block}");
        }
    }
    Ok(skins)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_truncated_and_overflowing_counts() {
        let mut r = Reader {
            data: &[255; 4],
            little: false,
        };
        assert!(r.count(24).is_err());
        assert!(read_nif(b"Gamebryo\n").is_err());
    }
    #[test]
    fn empty_payload_preserves_fields() {
        let bytes = [0u8; 34];
        let mut r = Reader {
            data: &bytes,
            little: false,
        };
        let s = payload(&mut r, 15, 7).unwrap();
        assert_eq!(s.block, 7);
        assert!(s.weights.is_empty());
        assert!(s.morph_deltas.is_empty());
    }
}
/// Column-major skin matrices, validated against the original Mickey reference rig.
/// Keeps hierarchy transforms in game coordinates; no display-axis conversion.
pub fn bone_palette(
    skin: &Skin,
    bones: &[epic_mickey_lib_rs::hkx_file::HkxBone],
) -> Result<Vec<[f32; 16]>> {
    palette_from_model_pose(skin, &model_pose(bones, &[])?)
}

/// Animation-local pose resolved into skeleton/model space. Explicit model
/// overrides preserve descendants' authored model poses. Retail 0x2e98a8
/// synchronizes direct children before replacing their parent's model transform
/// at 0x2e94dc; those children remain model-valid/local-dirty afterwards.
pub fn model_pose(
    bones: &[epic_mickey_lib_rs::hkx_file::HkxBone],
    overrides: &[(usize, [f32; 16])],
) -> Result<Vec<[f32; 16]>> {
    for (entry, (index, matrix)) in overrides.iter().enumerate() {
        ensure!(
            *index < bones.len(),
            "Pose override references absent bone {index}"
        );
        ensure!(
            matrix.iter().all(|v| v.is_finite()),
            "Non-finite override at bone {index}"
        );
        ensure!(
            matrix[3] == 0. && matrix[7] == 0. && matrix[11] == 0. && matrix[15] == 1.,
            "Non-affine override at bone {index}"
        );
        ensure!(
            !overrides[..entry].iter().any(|(other, _)| other == index),
            "Duplicate override at bone {index}"
        );
    }
    let mut worlds: Vec<[f32; 16]> = Vec::with_capacity(bones.len());
    let mut model_qs: Vec<super::actor_bone_query::Pose> = Vec::with_capacity(bones.len());
    for (index, bone) in bones.iter().enumerate() {
        ensure!(
            bone.parent_index == -1
                || (bone.parent_index >= 0 && (bone.parent_index as usize) < index),
            "Invalid/unordered bone hierarchy at {index}"
        );
        ensure!(
            bone.local_translation
                .iter()
                .chain(&bone.local_rotation)
                .chain(&bone.local_scale)
                .all(|v| v.is_finite()),
            "Non-finite pose at bone {index}"
        );
        let mut local = [0.; 12];
        local[..3].copy_from_slice(&bone.local_translation);
        local[4..8].copy_from_slice(&bone.local_rotation);
        local[8..11].copy_from_slice(&bone.local_scale);
        local[11] = 1.;
        // hkaPose resolves model-space Qs transforms through801e6bc4.
        // Matrix multiplication adds parent-scale translation and shear which
        // this native composition explicitly excludes.
        let model = if bone.parent_index == -1 {
            local
        } else {
            super::actor_pose_compose::compose(model_qs[bone.parent_index as usize], local)
        };
        let mut resolved = bone.clone();
        resolved.local_translation.copy_from_slice(&model[..3]);
        resolved.local_rotation.copy_from_slice(&model[4..8]);
        resolved.local_scale.copy_from_slice(&model[8..11]);
        worlds.push(local_matrix(&resolved));
        model_qs.push(model);
    }
    for (index, matrix) in overrides {
        worlds[*index] = *matrix;
    }
    Ok(worlds)
}

// Keep the palette loop separate from actor admission. Rust 1.98's MSVC
// codegen emitted an invalid stack offset when it inlined this routine there.
#[inline(never)]
pub fn palette_from_model_pose(skin: &Skin, worlds: &[[f32; 16]]) -> Result<Vec<[f32; 16]>> {
    ensure!(
        skin.transform_floats.len()
            == worlds
                .len()
                .checked_mul(16)
                .context("Bone count overflow")?,
        "Skin and skeleton matrix counts differ"
    );
    worlds
        .iter()
        .enumerate()
        .map(|(index, world)| {
            let inverse: [f32; 16] =
                skin.transform_floats[index * 16..index * 16 + 16].try_into()?;
            ensure!(
                world.iter().chain(&inverse).all(|v| v.is_finite()),
                "Non-finite skin matrix at bone {index}"
            );
            Ok(matrix_product(world, &inverse))
        })
        .collect()
}

fn local_matrix(bone: &epic_mickey_lib_rs::hkx_file::HkxBone) -> [f32; 16] {
    let [x, y, z, w] = bone.local_rotation;
    let [sx, sy, sz] = bone.local_scale;
    let [tx, ty, tz] = bone.local_translation;
    [
        (1. - 2. * (y * y + z * z)) * sx,
        2. * (x * y + z * w) * sx,
        2. * (x * z - y * w) * sx,
        0.,
        2. * (x * y - z * w) * sy,
        (1. - 2. * (x * x + z * z)) * sy,
        2. * (y * z + x * w) * sy,
        0.,
        2. * (x * z + y * w) * sz,
        2. * (y * z - x * w) * sz,
        (1. - 2. * (x * x + y * y)) * sz,
        0.,
        tx,
        ty,
        tz,
        1.,
    ]
}
pub(super) fn matrix_product(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    std::array::from_fn(|i| (0..4).map(|k| a[k * 4 + i % 4] * b[i / 4 * 4 + k]).sum())
}
#[cfg(test)]
mod palette_tests {
    use super::*;
    use epic_mickey_lib_rs::hkx_file::HkxBone;
    #[test]
    fn model_pose_uses_native_qs_composition_with_nonuniform_parent_scale() {
        let root = HkxBone {
            local_translation: [10., 20., 30.],
            local_scale: [2., 3., 4.],
            ..Default::default()
        };
        let child = HkxBone {
            parent_index: 0,
            local_translation: [1., 2., 3.],
            local_rotation: [0., 0., 1., 0.],
            ..Default::default()
        };
        let pose = model_pose(&[root, child], &[]).unwrap();
        assert_eq!(&pose[1][12..15], &[11., 22., 33.]);
        assert_eq!(pose[1][0], -2.);
        assert_eq!(pose[1][5], -3.);
        assert_eq!(pose[1][10], 4.);
        assert_eq!(pose[1][1], 0.);
        assert_eq!(pose[1][4], 0.);
    }
    #[test]
    fn model_override_preserves_descendant_model_poses() {
        let bones = [
            HkxBone {
                local_translation: [10., 0., 0.],
                ..Default::default()
            },
            HkxBone {
                parent_index: 0,
                local_translation: [2., 0., 0.],
                ..Default::default()
            },
            HkxBone {
                parent_index: 1,
                local_translation: [0., 3., 0.],
                ..Default::default()
            },
            HkxBone {
                parent_index: 0,
                local_translation: [-2., 0., 0.],
                ..Default::default()
            },
        ];
        let ordinary = model_pose(&bones, &[]).unwrap();
        let replacement = local_matrix(&HkxBone {
            local_translation: [4., 5., 6.],
            local_scale: [2., 3., 4.],
            ..Default::default()
        });
        let result = model_pose(&bones, &[(1, replacement)]).unwrap();
        assert_eq!(result[0], ordinary[0]);
        assert_eq!(result[3], ordinary[3]);
        assert_eq!(&result[1][12..15], &[4., 5., 6.]);
        assert_eq!(result[2], ordinary[2]);
        assert!(model_pose(&bones, &[(4, replacement)]).is_err());
        assert!(model_pose(&bones, &[(1, replacement), (1, replacement)]).is_err());
        let mut invalid = replacement;
        invalid[12] = f32::NAN;
        assert!(model_pose(&bones, &[(1, invalid)]).is_err());
    }
    fn skin() -> Skin {
        let bytes = [0u8; 34];
        payload(
            &mut Reader {
                data: &bytes,
                little: false,
            },
            15,
            0,
        )
        .unwrap()
    }
    #[test]
    fn inverse_bind_cancels_rotated_parent_and_child() {
        let q = std::f32::consts::FRAC_1_SQRT_2;
        let root = HkxBone {
            local_translation: [3., 0., 0.],
            local_rotation: [0., 0., q, q],
            ..Default::default()
        };
        let child = HkxBone {
            parent_index: 0,
            local_translation: [2., 0., 0.],
            ..Default::default()
        };
        let mut s = skin();
        s.transform_floats = vec![
            0., -1., 0., 0., 1., 0., 0., 0., 0., 0., 1., 0., 0., 3., 0., 1., 0., -1., 0., 0., 1.,
            0., 0., 0., 0., 0., 1., 0., -2., 3., 0., 1.,
        ];
        for matrix in bone_palette(&s, &[root, child]).unwrap() {
            for (i, v) in matrix.into_iter().enumerate() {
                assert!((v - if i % 5 == 0 { 1. } else { 0. }).abs() < 1e-6);
            }
        }
    }
    #[test]
    fn rejects_missing_inverse_bind_and_cyclic_parent() {
        let mut s = skin();
        assert!(bone_palette(&s, &[HkxBone::default()]).is_err());
        s.transform_floats = vec![0.; 16];
        assert!(bone_palette(
            &s,
            &[HkxBone {
                parent_index: 0,
                ..Default::default()
            }]
        )
        .is_err());
    }
}
/// Deforms rendered positions using their original POSITION-stream identities.
/// Does not merge coincident vertices or renormalize authored weights.
pub fn deform_positions(
    skin: &Skin,
    palette: &[[f32; 16]],
    positions: &[[f32; 3]],
    sources: &[u32],
) -> Result<Vec<[f32; 3]>> {
    deform_vectors(skin, palette, positions, sources, true)
}
/// Weighted linear normal transform, without positional translation or renormalization.
/// Matches the referenced Gamebryo skinning operation; retail EM1 parity remains to verify.
pub fn deform_normals(
    skin: &Skin,
    palette: &[[f32; 16]],
    normals: &[[f32; 3]],
    sources: &[u32],
) -> Result<Vec<[f32; 3]>> {
    deform_vectors(skin, palette, normals, sources, false)
}
fn deform_vectors(
    skin: &Skin,
    palette: &[[f32; 16]],
    positions: &[[f32; 3]],
    sources: &[u32],
    translate: bool,
) -> Result<Vec<[f32; 3]>> {
    ensure!(
        positions.len() == sources.len(),
        "Rendered vector/source mapping count differs"
    );
    ensure!(skin.has_weights != 0, "Mesh does not declare skin weights");
    positions.iter().zip(sources).map(|(p,&source)|weighted_vector(skin,palette,*p,source,translate)).collect()
}
fn weighted_vector(skin:&Skin,palette:&[[f32;16]],p:[f32;3],source:u32,translate:bool)->Result<[f32;3]> {
    ensure!(
        p.iter().all(|x| x.is_finite()),
        "Non-finite source position"
    );
    let weight_index = *skin
        .vertex_weights
        .get(source as usize)
        .context("Missing source vertex weight mapping")?;
    let weight = skin
        .weights
        .get(weight_index as usize)
        .context("Missing skin weight record")?;
    let mut result = [0.; 3];
    let mut contributed = false;
    for (&bone, &value) in weight.bones.iter().zip(&weight.values) {
        ensure!(value.is_finite() && value >= 0., "Invalid skin weight");
        if value == 0. {
            continue;
        }
        ensure!(bone >= 0, "Active weight references a negative bone");
        let m = palette
            .get(bone as usize)
            .context("Active weight references an absent bone")?;
        for i in 0..3 {
            result[i] += value
                * (m[i] * p[0]
                    + m[4 + i] * p[1]
                    + m[8 + i] * p[2]
                    + if translate { m[12 + i] } else { 0. });
        }
        contributed = true;
    }
    ensure!(
        contributed && result.iter().all(|v| v.is_finite()),
        "Invalid deformed position"
    );
    Ok(result)
}



/// Shares the same immutable skin and palette as position/normal playback.
/// Numerical retail parity is not inferred from the supporting SDK kernel.

pub(super) struct DirectionSnapshot {
    pub skin:std::sync::Arc<Skin>,pub palette:std::sync::Arc<Vec<[f32;16]>>,
    pub sources:std::sync::Arc<Vec<u32>>,
}
impl std::fmt::Debug for DirectionSnapshot {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {f.debug_struct("DirectionSnapshot").field("vertices",&self.sources.len()).field("bones",&self.palette.len()).finish()}
}
impl crate::nif_vertex_attributes::DirectionPose for DirectionSnapshot {
    fn deform_positions(&self,vectors:&mut [[f32;4]])->Result<()> {
        ensure!(vectors.len()==self.sources.len()&&self.skin.has_weights!=0,"Native position skin/source mapping differs");
        for (vector,&source) in vectors.iter_mut().zip(self.sources.iter()) {
            ensure!(vector[3]==1.,"Skinned homogeneous position requires its W adapter");
            let position=weighted_vector(&self.skin,&self.palette,[vector[0],vector[1],vector[2]],source,true)?;
            vector[..3].copy_from_slice(&position);
        }
        Ok(())
    }
    fn deform(&self,vectors:&mut [[f32;4]])->Result<()> {
        ensure!(vectors.len()==self.sources.len()&&self.skin.has_weights!=0,"Auxiliary skin/source mapping differs");
        for (vector,&source) in vectors.iter_mut().zip(self.sources.iter()) {
            let v=weighted_vector(&self.skin,&self.palette,[vector[0],vector[1],vector[2]],source,false)?;
            let length=(v[0]*v[0]+v[1]*v[1]+v[2]*v[2]).sqrt();
            ensure!(length.is_finite(),"Invalid skinned auxiliary direction length");
            let xyz=if length>1e-6 {let reciprocal=1./length;v.map(|x|x*reciprocal)}else{[0.;3]};
            vector[..3].copy_from_slice(&xyz);
        }
        Ok(())
    }
}

/// Converts a deformed mesh-space pose to the renderer's 12-float vertex layout.
/// UVs and colors come from the immutable template; node transforms apply once.
/// Returns model-space bounds for visibility testing of the animated pose.
/// Fuses the two weighted-vector passes with render publication. The arithmetic
/// within each position/normal channel is unchanged; only mapping/weight reads
/// are shared. No temporary deformed position or normal arrays are retained.
pub fn render_skinned_pose(
    skin: &Skin,
    palette: &[[f32;16]],
    template: &[f32],
    positions: &[[f32;3]],
    normals: &[[f32;3]],
    sources: &[u32],
    mesh_to_model: [[f32;4];4],
) -> Result<(Vec<f32>,[f32;3],[f32;3])> {
    ensure!(positions.len()==normals.len() && positions.len()==sources.len(),"Rendered vector/source mapping count differs");
    ensure!(skin.has_weights!=0,"Mesh does not declare skin weights");
    render_vertices(template,positions.len(),mesh_to_model,|index| {
        let p=positions[index];let n=normals[index];
        ensure!(p.iter().chain(&n).all(|v|v.is_finite()),"Non-finite source position or normal");
        let mapping=*skin.vertex_weights.get(sources[index] as usize).context("Missing source vertex weight mapping")?;
        let weight=skin.weights.get(mapping as usize).context("Missing skin weight record")?;
        let mut position=[0.;3];let mut normal=[0.;3];let mut contributed=false;
        for (&bone,&value) in weight.bones.iter().zip(&weight.values) {
            ensure!(value.is_finite() && value>=0.,"Invalid skin weight");
            if value==0. {continue;}
            ensure!(bone>=0,"Active weight references a negative bone");
            let m=palette.get(bone as usize).context("Active weight references an absent bone")?;
            for i in 0..3 {
                position[i]+=value*(m[i]*p[0]+m[4+i]*p[1]+m[8+i]*p[2]+m[12+i]);
                normal[i]+=value*(m[i]*n[0]+m[4+i]*n[1]+m[8+i]*n[2]+0.);
            }
            contributed=true;
        }
        ensure!(contributed && position.iter().chain(&normal).all(|v|v.is_finite()),"Invalid deformed position or normal");
        Ok((position,normal))
    })
}

pub fn render_pose(
    template: &[f32],
    positions: &[[f32;3]],
    normals: &[[f32;3]],
    mesh_to_model: [[f32;4];4],
) -> Result<(Vec<f32>,[f32;3],[f32;3])> {
    ensure!(normals.len()==positions.len(),"Animated render vertex count mismatch");
    render_vertices(template,positions.len(),mesh_to_model,|i|Ok((positions[i],normals[i])))
}

fn render_vertices(
    template: &[f32],
    count: usize,
    mesh_to_model: [[f32;4];4],
    mut sample: impl FnMut(usize)->Result<([f32;3],[f32;3])>,
) -> Result<(Vec<f32>,[f32;3],[f32;3])> {
    ensure!(count!=0,"Empty animated mesh");
    ensure!(template.len().checked_div(12)==Some(count) && template.len()%12==0,"Animated render vertex count mismatch");
    ensure!(
        mesh_to_model.iter().flatten().all(|v| v.is_finite())
            && mesh_to_model[3] == [0., 0., 0., 1.],
        "Invalid affine mesh transform"
    );
    let m = mesh_to_model.map(|row| row.map(f64::from));
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let rows: [[f64; 3]; 3] = std::array::from_fn(|i| [m[i][0], m[i][1], m[i][2]]);
    let cofactors = [
        cross(rows[1], rows[2]),
        cross(rows[2], rows[0]),
        cross(rows[0], rows[1]),
    ];
    let det = (0..3).map(|i| rows[0][i] * cofactors[0][i]).sum::<f64>();
    ensure!(
        det.is_finite() && det != 0.,
        "Singular animated mesh transform"
    );
    let mut output = template.to_vec();
    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    for (index,vertex) in output.chunks_exact_mut(12).enumerate() {
        let (p,n)=sample(index)?;
        ensure!(
            p.iter().chain(&n).all(|v| v.is_finite()),
            "Non-finite animated vertex"
        );
        let normal: [f64; 3] = std::array::from_fn(|i| {
            (0..3)
                .map(|j| cofactors[i][j] * f64::from(n[j]))
                .sum::<f64>()
                / det
        });
        let length = normal.iter().map(|v| v * v).sum::<f64>().sqrt();
        let unit_normal = if length.is_finite() && length > 1e-8 {
            [
                (normal[0] / length) as f32,
                (normal[1] / length) as f32,
                (normal[2] / length) as f32,
            ]
        } else {
            [0., 1., 0.]
        };
        for i in 0..3 {
            vertex[i] = ((0..3).map(|j| m[i][j] * f64::from(p[j])).sum::<f64>() + m[i][3]) as f32;
            ensure!(vertex[i].is_finite(), "Animated vertex overflow");
            vertex[i + 3] = unit_normal[i];
            minimum[i] = minimum[i].min(vertex[i]);
            maximum[i] = maximum[i].max(vertex[i]);
        }
    }
    Ok((output, minimum, maximum))
}

#[cfg(test)]
mod render_tests {
    use super::*;
    #[test]
    fn pose_updates_bounds_and_normals_without_changing_material_attributes() {
        let transform = [
            [2., 0., 0., 10.],
            [0., 1., 0., 20.],
            [0., 0., 1., 30.],
            [0., 0., 0., 1.],
        ];
        let template = [99., 99., 99., 0., 0., 1., 0.2, 0.3, 0.4, 0.5, 0.6, 0.7];
        let (v, lo, hi) =
            render_pose(&template, &[[1., 2., 3.]], &[[1., 1., 0.]], transform).unwrap();
        assert_eq!(&v[..3], &[12., 22., 33.]);
        assert_eq!(lo, [12., 22., 33.]);
        assert_eq!(lo, hi);
        assert!((v[3] - 0.4472136).abs() < 1e-6);
        assert!((v[4] - 0.8944272).abs() < 1e-6);
        assert_eq!(&v[6..], &template[6..]);
        assert!(render_pose(&template, &[[0.; 3]], &[[0.; 3]], transform).is_err());
        assert!(render_pose(&template, &[[0.; 3]], &[[1.; 3]], [[0.; 4]; 4]).is_err());
        assert!(render_pose(&template[..11], &[[0.; 3]], &[[1.; 3]], transform).is_err());
    }
}
#[cfg(test)]
mod deformation_tests {
    use super::*;
    #[test]
    fn duplicated_render_vertices_keep_source_weights() {
        let bytes = [0u8; 34];
        let mut skin = payload(
            &mut Reader {
                data: &bytes,
                little: false,
            },
            15,
            0,
        )
        .unwrap();
        skin.has_weights = 1;
        skin.weights = vec![
            Weight {
                bones: [0, -1, -1],
                values: [1., 0., 0.],
            },
            Weight {
                bones: [1, -1, -1],
                values: [1., 0., 0.],
            },
        ];
        skin.vertex_weights = vec![0, 1];
        let identity = [
            1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
        ];
        let mut translated = identity;
        translated[12] = 10.;
        let result =
            deform_positions(&skin, &[identity, translated], &[[0.; 3]; 3], &[1, 0, 1]).unwrap();
        assert_eq!(result, vec![[10., 0., 0.], [0., 0., 0.], [10., 0., 0.]]);
        assert!(deform_positions(&skin, &[identity], &[[0.; 3]], &[1]).is_err());
        assert!(deform_positions(&skin, &[identity], &[[0.; 3]], &[2]).is_err());
        skin.weights[0].bones[0] = -1;
        assert!(deform_positions(&skin, &[identity], &[[0.; 3]], &[0]).is_err());
    }
}
/// Applies the mesh's sparse vertex-major morph deltas using all authored slots.
/// Ordering relative to skinning must be verified by the playback integration.
pub fn morph_positions(
    skin: &Skin,
    positions: &[[f32; 3]],
    sources: &[u32],
    slots: &[f32],
) -> Result<Vec<[f32; 3]>> {
    ensure!(
        positions.len() == sources.len(),
        "Morph source count mismatch"
    );
    ensure!(
        slots.len() == skin.morph_width && slots.iter().all(|v| v.is_finite()),
        "Missing/non-finite morph slot values"
    );
    let expected = skin
        .morph_indices
        .len()
        .checked_mul(skin.morph_width)
        .and_then(|v| v.checked_mul(3))
        .context("Morph shape overflow")?;
    ensure!(
        expected == skin.morph_deltas.len(),
        "Morph delta dimensions mismatch"
    );
    let mut deltas = std::collections::HashMap::<u32, [f32; 3]>::new();
    for (row, &source) in skin.morph_indices.iter().enumerate() {
        let delta = deltas.entry(source as u32).or_insert([0.; 3]);
        for (slot, &value) in slots.iter().enumerate() {
            let offset = (row * skin.morph_width + slot) * 3;
            for i in 0..3 {
                delta[i] += skin.morph_deltas[offset + i] * value;
            }
        }
    }
    positions
        .iter()
        .zip(sources)
        .map(|(p, source)| {
            let delta = deltas.get(source).copied().unwrap_or([0.; 3]);
            let result = std::array::from_fn(|i| p[i] + delta[i]);
            ensure!(
                result.iter().all(|v| v.is_finite()),
                "Non-finite morphed position"
            );
            Ok(result)
        })
        .collect()
}
#[cfg(test)]
mod morph_tests {
    use super::*;
    #[test]
    fn sparse_morphs_follow_source_vertices_and_slots() {
        let bytes = [0u8; 34];
        let mut skin = payload(
            &mut Reader {
                data: &bytes,
                little: false,
            },
            15,
            0,
        )
        .unwrap();
        skin.morph_width = 2;
        skin.morph_indices = vec![7];
        skin.morph_deltas = vec![2., 0., 0., 0., 4., 0.];
        let result = morph_positions(&skin, &[[1.; 3]; 3], &[7, 2, 7], &[0.5, 0.25]).unwrap();
        assert_eq!(result, vec![[2., 2., 1.], [1., 1., 1.], [2., 2., 1.]]);
        assert!(morph_positions(&skin, &[[1.; 3]], &[7], &[1.]).is_err());
        assert!(morph_positions(&skin, &[[1.; 3]], &[7], &[f32::NAN, 0.]).is_err());
        skin.morph_deltas.pop();
        assert!(morph_positions(&skin, &[[1.; 3]], &[7], &[0., 0.]).is_err());
    }
}

/// Retail EM1 selection at REL offsets 0x2f2644..0x2f2684.
/// This is the original runtime's restriction, not an engine capacity cap.
pub fn retail_morph_slots(slots: &[f32]) -> Result<Vec<f32>> {
    ensure!(
        slots.iter().all(|v| v.is_finite()),
        "Non-finite morph weight"
    );
    let mut selected = 0;
    Ok(slots
        .iter()
        .map(|&value| {
            if value > 0. && selected < 4 {
                selected += 1;
                value
            } else {
                0.
            }
        })
        .collect())
}

/// Base-first, slot-ordered accumulation seen in EM1's paired-single morph loop.
/// Host f32 FMA matches the operation shape; Wii exceptional rounding remains unverified.
pub fn retail_morph_positions(
    skin: &Skin,
    positions: &[[f32; 3]],
    sources: &[u32],
    slots: &[f32],
) -> Result<Vec<[f32; 3]>> {
    ensure!(
        positions.len() == sources.len() && slots.len() == skin.morph_width,
        "Retail morph dimensions mismatch"
    );
    let expected = skin
        .morph_indices
        .len()
        .checked_mul(skin.morph_width)
        .and_then(|v| v.checked_mul(3))
        .context("Morph dimensions overflow")?;
    ensure!(
        expected == skin.morph_deltas.len() && skin.morph_deltas.iter().all(|v| v.is_finite()),
        "Invalid retail morph deltas"
    );
    let selected = retail_morph_slots(slots)?;
    // Retail reloads the original base for each sparse row, so a repeated source
    // is overwritten by its last row rather than adding two rows together.
    let rows: std::collections::HashMap<u32, usize> = skin
        .morph_indices
        .iter()
        .enumerate()
        .map(|(row, &index)| (index as u32, row))
        .collect();
    positions
        .iter()
        .zip(sources)
        .map(|(&position, source)| {
            ensure!(position.iter().all(|v| v.is_finite()), "Invalid morph base");
            let mut result = position;
            if let Some(&row) = rows.get(source) {
                for (slot, &weight) in selected.iter().enumerate().filter(|(_, w)| **w > 0.) {
                    let offset = (row * skin.morph_width + slot) * 3;
                    for axis in 0..3 {
                        result[axis] =
                            skin.morph_deltas[offset + axis].mul_add(weight, result[axis]);
                    }
                }
            }
            ensure!(
                result.iter().all(|v| v.is_finite()),
                "Retail morph overflow"
            );
            Ok(result)
        })
        .collect()
}

#[cfg(test)]
mod retail_morph_tests {
    use super::*;
    #[test]
    fn keeps_first_four_positive_slots_without_sorting_or_normalizing() {
        assert_eq!(
            retail_morph_slots(&[-0.2, 0., 0.1, 2., -1., 0.3, 0.4, 10.]).unwrap(),
            vec![0., 0., 0.1, 2., 0., 0.3, 0.4, 0.]
        );
        assert_eq!(
            retail_morph_slots(&[-1., -0., 0.]).unwrap(),
            vec![0., 0., 0.]
        );
        assert!(retail_morph_slots(&[f32::NAN]).is_err());
        assert!(retail_morph_slots(&[f32::INFINITY]).is_err());
    }
    #[test]
    fn accumulation_starts_at_base_and_repeated_rows_overwrite() {
        let bytes = [0u8; 34];
        let mut skin = payload(
            &mut Reader {
                data: &bytes,
                little: false,
            },
            15,
            0,
        )
        .unwrap();
        skin.morph_width = 2;
        skin.morph_indices = vec![7];
        skin.morph_deltas = vec![-1e8, 0., 0., 1., 0., 0.];
        let positions = [[1e8, 0., 0.]];
        assert_eq!(
            retail_morph_positions(&skin, &positions, &[7], &[1., 1.]).unwrap(),
            vec![[1., 0., 0.]]
        );
        assert_eq!(
            morph_positions(&skin, &positions, &[7], &[1., 1.]).unwrap(),
            vec![[0., 0., 0.]]
        );
        skin.morph_indices.push(7);
        skin.morph_deltas.extend([2., 0., 0., 3., 0., 0.]);
        assert_eq!(
            retail_morph_positions(&skin, &[[0.; 3]; 2], &[7, 1], &[1., 1.]).unwrap(),
            vec![[5., 0., 0.], [0.; 3]]
        );
    }
}

#[cfg(test)]
mod normal_tests {
    use super::*;
    #[test]
    fn normals_rotate_without_translation_and_keep_magnitude() {
        let bytes = [0u8; 34];
        let mut skin = payload(
            &mut Reader {
                data: &bytes,
                little: false,
            },
            15,
            0,
        )
        .unwrap();
        skin.has_weights = 1;
        skin.vertex_weights = vec![0];
        skin.weights = vec![Weight {
            bones: [0, -1, -1],
            values: [1., 0., 0.],
        }];
        let matrix = [
            0., 1., 0., 0., -1., 0., 0., 0., 0., 0., 1., 0., 100., 200., 300., 1.,
        ];
        assert_eq!(
            deform_normals(&skin, &[matrix], &[[2., 0., 0.]], &[0]).unwrap(),
            vec![[0., 2., 0.]]
        );
        assert_eq!(
            deform_positions(&skin, &[matrix], &[[2., 0., 0.]], &[0]).unwrap(),
            vec![[100., 202., 300.]]
        );
    }
}
