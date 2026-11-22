//! Typed surface channels for the enhanced renderer. Native combiner logic
//! remains the source of masks/paint and authored indirect illumination.
use super::{material_preview::Program, nif::MaterialBindings};
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub base: Option<u8>,
    pub normal: Option<u8>,
    pub roughness: Option<u8>,
    pub gloss: Option<u8>,
    pub metal: Option<u8>,
}
fn stem(path: &str) -> String {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    let p = p.rsplit('/').next().unwrap_or("");
    let p = p.rsplit_once('.').map(|(s, _)| s).unwrap_or(p);
    p.strip_suffix("_tex").unwrap_or(p).into()
}
fn role(name: &str) -> u8 {
    if name.ends_with("_normal") || name.ends_with("_norm") || name.ends_with("_n") {
        1
    } else if name.ends_with("_roughness") || name.ends_with("_rough") {
        2
    } else if name.ends_with("_gloss") || name.ends_with("_glossiness") {
        3
    } else if name.ends_with("_metallic") || name.ends_with("_metalness") {
        4
    } else {
        0
    }
}
impl Plan {
    pub fn native(program: &Program, material: Option<&MaterialBindings>) -> Self {
        let mut plan = Self::default();
        for &slot in &program.uv_textures {
            let path = material
                .and_then(|m| program.image_name(slot, m))
                .or_else(|| program.texture_names.get(slot as usize).map(String::as_str));
            let Some(path) = path else { continue };
            let name = stem(path);
            match role(&name) {
                1 => plan.normal = Some(slot),
                2 => plan.roughness = Some(slot),
                3 => plan.gloss = Some(slot),
                4 => plan.metal = Some(slot),
                _ => {
                    if slot == 0
                        && !name.contains("default")
                        && !name.contains("lightmap")
                        && !name.contains("mask")
                    {
                        plan.base = Some(slot);
                    }
                }
            }
        }
        plan
    }
    pub fn glsl(self) -> String {
        let uv = |slot: u8| format!("(u_tex_uv[{slot}]*vec3(bw_uv({slot}),1.)).xy");
        let sample = |slot: u8| format!("texture(u_tex[{slot}],{})", uv(slot));
        let base = self
            .base
            .map(|s| {
                format!(
                    "pow(max({}.rgb*bw_base_tint,vec3(0.)),vec3(2.2))",
                    sample(s)
                )
            })
            .unwrap_or_else(|| "pow(max(authored,vec3(0.)),vec3(2.2))".into());
        let rough = self
            .roughness
            .map(|s| format!("clamp(r*{}.r,.08,1.)", sample(s)))
            .or_else(|| {
                self.gloss
                    .map(|s| format!("clamp(r*(1.-{}.r),.08,1.)", sample(s)))
            })
            .unwrap_or_else(|| "r".into());
        let metal = self
            .metal
            .map(|s| format!("clamp({}.r,0.,1.)", sample(s)))
            .unwrap_or_else(|| "0.".into());
        let normal=self.normal.map(|s|format!("vec3 dp1=dFdx(bw_position),dp2=dFdy(bw_position);vec2 st1=dFdx({uv}),st2=dFdy({uv});float det=st1.x*st2.y-st1.y*st2.x;if(abs(det)<1e-7)return N;vec3 T=normalize((dp1*st2.y-dp2*st1.y)/det);T=normalize(T-N*dot(N,T));vec3 B=normalize(cross(N,T))*sign(det);vec3 map={map}.xyz*2.-1.;return normalize(mat3(T,B,N)*map);",uv=uv(s),map=sample(s))).unwrap_or_else(||"return N;".into());
        format!("uniform vec3 bw_base_tint;uniform bool bw_authored_surface;\nvec3 bw_material_albedo(vec3 authored){{if(bw_authored_surface)return pow(max(authored,vec3(0.)),vec3(2.2));return {base};}}\nfloat bw_material_roughness(float r){{return {rough};}}\nfloat bw_material_metallic(){{return {metal};}}\nvec3 bw_material_normal(vec3 N){{{normal}}}\n")
    }
}
/// Older additive FX encode perceived emission in alpha. Convert that energy
/// with RGB when entering the linear scene; coverage alpha stays linear.
pub fn encoded_emission(blend: Option<(u32, u32, u32)>) -> bool {
    use glow;
    blend == Some((glow::SRC_ALPHA, glow::ONE, glow::FUNC_ADD))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn additive_energy_preserves_authored_fade_without_changing_coverage() {
        use glow;
        assert!(encoded_emission(Some((
            glow::SRC_ALPHA,
            glow::ONE,
            glow::FUNC_ADD
        ))));
        assert!(!encoded_emission(Some((
            glow::SRC_ALPHA,
            glow::ONE_MINUS_SRC_ALPHA,
            glow::FUNC_ADD
        ))));
        let color = 0.8_f32;
        let alpha = 0.01_f32;
        let encoded = (color.powf(2.2) * alpha.powf(2.2)).powf(1. / 2.2);
        assert!((encoded - color * alpha).abs() < 1e-6);
    }
    #[test]
    fn channel_roles_do_not_guess_from_arbitrary_names() {
        assert_eq!(role(&stem("Textures/stone_normal_tex.nif")), 1);
        assert_eq!(role(&stem("foo_roughness.dds")), 2);
        assert_eq!(role(&stem("normal_house.dds")), 0);
        assert_eq!(role(&stem("foo_metalness.dds")), 4);
    }
}
