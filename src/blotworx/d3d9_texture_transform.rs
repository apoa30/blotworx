//! PC shader texture-transform packing, distinct from generic preview UVs.
use anyhow::{ensure,Context,Result};
use crate::render_support::nif::MaterialBindings;
use super::{d3d9_predefined::{Value,transpose},pc_float::Precision};
pub(super) fn pack(source:Option<[f32;9]>,transposed:bool)->[f32;16] {
    let matrix=if let Some(m)=source {
        // PC d441f0 uses this packing for both 2- and 4-component inputs.
        // In particular a present transform's third/fourth output columns
        // remain zero; a missing transform returns a full identity instead.
        [m[0],m[3],0.,0.,m[1],m[4],0.,0.,m[2],m[5],0.,0.,0.,0.,0.,0.]
    }else{std::array::from_fn(|i|if i%5==0 {1.}else{0.})};
    if transposed {transpose(&matrix)}else{matrix}
}
pub(super) fn resolve(material:&MaterialBindings,mapping:&str,index:Option<i32>,precision:Option<Precision>)->Result<Option<Value>> {
    resolve_animated(material,mapping,index,precision,|_|None)
}
/// `animated` supplies a live (controller-driven) transform for a shader map slot, replacing the authored one.
pub(super) fn resolve_animated(material:&MaterialBindings,mapping:&str,index:Option<i32>,precision:Option<Precision>,animated:impl Fn(u32)->Option<crate::nif_textures::UvTransform>)->Result<Option<Value>> {
    let names=["TexTransformShader","InvTexTransformShader","TexTransformShaderTranspose","InvTexTransformShaderTranspose"];
    let Some(kind)=names.iter().position(|name|mapping.eq_ignore_ascii_case(name)) else{return Ok(None)};
    if let Some(Err(error))=&material.legacy_texture_flags {anyhow::bail!("Texture transform property: {error}")}
    let index=index.context("Shader texture transform requires its authored NTMIndex")? as u32;
    let transform=match animated(index) {Some(t)=>Some(t),None=>material.texture_transforms.get(&(true,index)).map(|value|value.as_ref().map_err(|error|anyhow::anyhow!("{error}"))).transpose()?.copied().flatten()};
    let matrix=if let Some(t)=transform {
        ensure!(kind!=1&&kind!=3,"Inverse shader texture transform needs its native singular-matrix adapter");
        Some(super::pc_float::texture_matrix([t.translation[0],t.translation[1],t.rotation,t.scale[0],t.scale[1],t.center[0],t.center[1]],t.method,precision.context("PC texture transform precision owner is absent")?)?)
    }else{None};
    Ok(Some(Value::Matrix(pack(matrix,kind>=2))))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_texture_matrices_and_packing_match_original_pc()->Result<()> {
        let reference:serde_json::Value=serde_json::from_str(include_str!("../../tools/blotworx/fixtures/pc_texture_transform.json"))?;
        for case in reference["cases"].as_array().unwrap() {
            let values=std::array::from_fn(|i|case["values"][i].as_f64().unwrap() as f32);
            let precision=match case["precision"].as_u64().unwrap(){0x7f=>Precision::Single,0x27f=>Precision::Double,_=>Precision::Extended};
            let matrix=super::super::pc_float::texture_matrix(values,case["method"].as_u64().unwrap() as u32,precision)?;
            for (lane,value) in matrix.iter().enumerate(){assert_eq!(value.to_bits(),case["matrix_words"][lane].as_u64().unwrap() as u32,"{case}, matrix lane {lane}");}
            for (lane,value) in pack(Some(matrix),false).iter().enumerate(){assert_eq!(value.to_bits(),case["packed_words"][lane].as_u64().unwrap() as u32,"{case}, packing lane {lane}");}
        }Ok(())
    }
    #[test]
    fn absent_transform_differs_from_present_identity_and_bad_data_rejects()->Result<()> {
        let mut material=MaterialBindings::default();
        assert_eq!(resolve(&material,"textransformshader",Some(3),None)?,Some(Value::Matrix(pack(None,false))));
        material.texture_transforms.insert((true,3),Ok(Some(crate::nif_textures::UvTransform::default())));
        assert!(resolve(&material,"TexTransformShader",Some(3),None).is_err());
        let Some(Value::Matrix(matrix))=resolve(&material,"TexTransformShader",Some(3),Some(Precision::Extended))? else{panic!("No texture matrix")};
        assert_eq!(matrix[10],0.);assert_eq!(matrix[15],0.);
        assert!(resolve(&material,"InvTexTransformShader",Some(3),Some(Precision::Extended)).is_err());
        material.texture_transforms.insert((true,3),Err("Malformed transform".into()));
        assert!(resolve(&material,"TexTransformShader",Some(3),Some(Precision::Extended)).is_err());
        Ok(())
    }
}
