//! Per-player unique Mickey pants recoloring & remote player 3D draw generation.
//!
//! Epic Mickey's player model (`Characters/Mickey/Mickey.nif`) uses a unified texture where
//! Mickey's shorts are bright saturated red while his fur is black, face is peach, gloves/buttons
//! are white, and shoes are yellow. Recoloring only the red-shorts HSV band produces an authentic
//! unique pants color for each of the 16 players (cached per `(source_ptr, color_slot)` so OpenGL
//! uploads each color once with zero per-frame allocation).

use super::protocol::{pants_color, FLAG_SPRAY_PAINT, FLAG_SPRAY_THINNER};
use crate::blotworx::{assets::World, ears, skin};
use crate::render_support::{material_preview::MaterialImage, viewport3d::GpuDrawCommand};
use std::{collections::HashMap, sync::Arc};

/// Cached recolored diffuse RGBA textures keyed by `(original_rgba_ptr, color_slot)`.
/// NOTE: Only immutable diffuse `Arc<Vec<u8>>` textures are cached — never the outer
/// `Vec<MaterialImage>` which `apply_spherical_lighting` rebuilds every frame with live SH lighting.
#[derive(Default)]
pub struct PantsTextureCache {
    by_rgba: HashMap<(usize, u8), Arc<Vec<u8>>>,
}

impl PantsTextureCache {
    pub fn clear(&mut self) {
        self.by_rgba.clear();
    }

    fn recolor_rgba(&mut self, src: &Arc<Vec<u8>>, color_slot: u8) -> Arc<Vec<u8>> {
        let key = (Arc::as_ptr(src) as usize, color_slot);
        if let Some(cached) = self.by_rgba.get(&key) {
            return cached.clone();
        }
        let target = pants_color(color_slot).rgb;
        let out = Arc::new(recolor_mickey_shorts_rgba(src, target));
        self.by_rgba.insert(key, out.clone());
        out
    }

    /// Apply a player's unique pants color to a Mickey draw command while preserving
    /// all live spherical-lighting / normal / specular texture slots intact.
    pub fn apply_to_draw(&mut self, draw: &mut GpuDrawCommand, color_slot: u8) {
        if let Some((ref rgba, w, h)) = draw.tex_bytes {
            if !rgba.is_empty() {
                let recolored = self.recolor_rgba(rgba, color_slot);
                draw.tex_bytes = Some((recolored, w, h));
            }
        }
        if draw.material_images.iter().any(|img| img.slot == 0 && !img.rgba.is_empty()) {
            let mut next = Vec::with_capacity(draw.material_images.len());
            for img in draw.material_images.iter() {
                if img.slot == 0 && !img.rgba.is_empty() {
                    let recolored = self.recolor_rgba(&img.rgba, color_slot);
                    next.push(MaterialImage {
                        rgba: recolored,
                        ..img.clone()
                    });
                } else {
                    next.push(img.clone());
                }
            }
            draw.material_images = Arc::new(next);
        }
    }
}

/// Recolor only Mickey's red shorts pixels into `target_rgb`, preserving cloth shading & highlights,
/// while leaving black fur, peach skin, white gloves/buttons, and yellow shoes untouched.
pub fn recolor_mickey_shorts_rgba(src: &[u8], target_rgb: [u8; 3]) -> Vec<u8> {
    let mut dst = src.to_vec();
    let tr = target_rgb[0] as f32 / 255.0;
    let tg = target_rgb[1] as f32 / 255.0;
    let tb = target_rgb[2] as f32 / 255.0;
    let t_max = tr.max(tg).max(tb).max(0.001);
    let nr = tr / t_max;
    let ng = tg / t_max;
    let nb = tb / t_max;

    for px in dst.chunks_exact_mut(4) {
        let r = px[0] as f32 / 255.0;
        let g = px[1] as f32 / 255.0;
        let b = px[2] as f32 / 255.0;

        let max_c = r.max(g).max(b);
        let min_c = r.min(g).min(b);
        let delta = max_c - min_c;
        if max_c < 0.16 || delta < 0.12 {
            continue;
        }
        let sat = delta / max_c;
        // Identify saturated red pixels (shorts): red must dominate both green and blue,
        // and reject warm peach skin (where g & b are both fairly close to r) and yellow shoes (where g is high).
        if r < g + 0.14 || r < b + 0.14 || sat < 0.42 {
            continue;
        }
        // Compute hue in degrees [0, 360)
        let mut hue = if (max_c - r).abs() < 1e-6 {
            60.0 * (((g - b) / delta) % 6.0)
        } else if (max_c - g).abs() < 1e-6 {
            60.0 * ((b - r) / delta + 2.0)
        } else {
            60.0 * ((r - g) / delta + 4.0)
        };
        if hue < 0.0 {
            hue += 360.0;
        }
        // Shorts red band: ~335°..360° and 0°..18° (excludes orange/peach skin >= 20° and yellow shoes ~55°)
        let is_red_hue = hue <= 18.0 || hue >= 335.0;
        if !is_red_hue {
            continue;
        }

        // Smooth edge blend on borderline red pixels so seams stay anti-aliased
        let hue_weight = if hue <= 12.0 || hue >= 342.0 {
            1.0
        } else if hue < 18.0 {
            (18.0 - hue) / 6.0
        } else {
            (hue - 335.0) / 7.0
        };
        let sat_weight = ((sat - 0.42) / 0.18).clamp(0.0, 1.0);
        let w = (hue_weight * sat_weight).clamp(0.0, 1.0);

        // Preserve original fabric shading intensity (max_c) and subtle saturation variation
        let shaded_r = max_c * (nr * sat + (1.0 - sat));
        let shaded_g = max_c * (ng * sat + (1.0 - sat));
        let shaded_b = max_c * (nb * sat + (1.0 - sat));

        px[0] = ((r * (1.0 - w) + shaded_r * w).clamp(0.0, 1.0) * 255.0).round() as u8;
        px[1] = ((g * (1.0 - w) + shaded_g * w).clamp(0.0, 1.0) * 255.0).round() as u8;
        px[2] = ((b * (1.0 - w) + shaded_b * w).clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    dst
}

/// Append a remote player's independently animated, rotated, camera-ear-billboarded Mickey mesh
/// (with their unique pants color and paintbrush) onto `out_draws`.
#[allow(clippy::too_many_arguments)]
pub fn append_remote_player_draws(
    cache: &mut PantsTextureCache,
    world: &mut World,
    lit_player_templates: &[(usize, GpuDrawCommand)],
    ear_pose: &mut Option<ears::Pose>,
    ear_camera: ears::EarTransform,
    view: &[f32; 16],
    pos: [f32; 3],
    yaw: f32,
    anim_clip_hash: u32,
    anim_time: f32,
    color_slot: u8,
    flags: u8,
    out_draws: &mut Vec<GpuDrawCommand>,
) {
    let actor_matrix = world.entities[world.player_entity].controller_matrix(pos, yaw);

    // 1. Sample THIS remote player's own Havok animation clip independently of the local player
    let remote_bones = world
        .player_animations
        .sample_bones_by_hash(anim_clip_hash, anim_time)
        .unwrap_or_else(|| world.player_animations.rig.bones.clone());

    // 2. Update THIS remote player's Mickey ears so they authentically face the local viewer's camera
    if ear_pose.is_none() {
        *ear_pose = ears::Pose::new(&world.player_animations.rig.bones).ok();
    }
    if let (Some(ears), Some(ear_settings)) =
        (ear_pose.as_mut(), world.report.ear_settings.as_ref())
    {
        let _ = ears.update(ear_settings, &remote_bones, actor_matrix, ear_camera);
    }
    let ear_overrides = ear_pose.as_ref().map_or(&[][..], |e| e.overrides.as_slice());
    let model_pose = skin::model_pose(&remote_bones, ear_overrides).ok();

    // 3. Clone SH-lit material templates, then skin fresh from UNMODIFIED rest-pose `world.draws`
    let start_idx = out_draws.len();
    let mut draw_slot_by_orig: HashMap<usize, usize> = HashMap::with_capacity(lit_player_templates.len());
    for (orig_idx, base) in lit_player_templates {
        let mut d = base.clone();
        d.trans_mat = actor_matrix;
        cache.apply_to_draw(&mut d, color_slot);
        if (flags & FLAG_SPRAY_PAINT) != 0 {
            d.color = [0.92, 0.97, 1.0, 1.0];
        } else if (flags & FLAG_SPRAY_THINNER) != 0 {
            d.color = [0.92, 1.0, 0.92, 1.0];
        }
        let dst_idx = out_draws.len();
        draw_slot_by_orig.insert(*orig_idx, dst_idx);
        out_draws.push(d);
    }

    if let Some(ref pose) = model_pose {
        for mesh in &world.player_meshes {
            let Some(&dst_idx) = draw_slot_by_orig.get(&mesh.draw) else {
                continue;
            };
            let Some(rest_draw) = world.draws.get(mesh.draw) else {
                continue;
            };
            let Some(skin_data) = world
                .skins
                .get(&mesh.asset)
                .and_then(|s| s.iter().find(|s| s.block == mesh.block))
            else {
                continue;
            };
            if let Ok(palette) = skin::palette_from_model_pose(skin_data, pose) {
                if let Ok((verts, aabb_min, aabb_max)) = skin::render_skinned_pose(
                    skin_data,
                    &palette,
                    &rest_draw.v_data,
                    &mesh.positions,
                    &mesh.normals,
                    &mesh.sources,
                    mesh.transform,
                ) {
                    let dst = &mut out_draws[dst_idx];
                    dst.v_data = Arc::new(verts);
                    dst.aabb_min = aabb_min;
                    dst.aabb_max = aabb_max;
                }
            }
        }

        // 4. Append remote player's paintbrush locked to their animated hand bone
        let brush_start = out_draws.len();
        if world.brush.append(pose, actor_matrix, out_draws).is_ok() {
            let _ = world.brush.apply_lighting(
                &mut out_draws[brush_start..],
                view,
                &world.report.spherical_lights,
            );
        }
    }
    let _ = start_idx;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recolors_only_red_shorts_preserves_fur_skin_gloves_shoes() {
        // Pixels: [red shorts, black fur, peach face, white glove, yellow shoe]
        let src: Vec<u8> = vec![
            220, 24, 28, 255,   // red shorts -> should become blue
            18, 18, 20, 255,    // black fur -> unchanged
            240, 198, 168, 255, // peach skin -> unchanged
            245, 245, 245, 255, // white glove -> unchanged
            245, 210, 30, 255,  // yellow shoe -> unchanged
        ];
        let out = recolor_mickey_shorts_rgba(&src, [36, 114, 240]);
        // Red shorts became strongly blue
        assert!(out[2] > 180 && out[0] < 90, "shorts pixel={:?}", &out[0..4]);
        // Black fur, peach face, white glove, yellow shoe untouched
        assert_eq!(&out[4..8], &[18, 18, 20, 255]);
        assert_eq!(&out[8..12], &[240, 198, 168, 255]);
        assert_eq!(&out[12..16], &[245, 245, 245, 255]);
        assert_eq!(&out[16..20], &[245, 210, 30, 255]);
    }
}
