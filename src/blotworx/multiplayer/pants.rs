//! Per-player unique Mickey pants recoloring & remote player 3D draw generation.
//!
//! Epic Mickey's player model (`Characters/Mickey/Mickey.nif`) uses a unified texture where
//! Mickey's shorts are bright saturated red while his fur is black, face is peach, gloves/buttons
//! are white, and shoes are yellow. Recoloring only the red-shorts HSV band produces an authentic
//! unique pants color for each of the 16 players (cached per `(source_ptr, color_slot)` so OpenGL
//! uploads each color once with zero per-frame allocation).

use super::protocol::{pants_color, FLAG_JUMPING, FLAG_SPINNING, FLAG_SPRAY_PAINT, FLAG_SPRAY_THINNER};
use crate::render_support::{material_preview::MaterialImage, viewport3d::GpuDrawCommand};
use std::{collections::HashMap, sync::Arc};

/// Cached recolored RGBA textures keyed by `(original_rgba_ptr, color_slot)`.
#[derive(Default)]
pub struct PantsTextureCache {
    by_rgba: HashMap<(usize, u8), Arc<Vec<u8>>>,
    by_mat_list: HashMap<(usize, u8), Arc<Vec<MaterialImage>>>,
}

impl PantsTextureCache {
    pub fn clear(&mut self) {
        self.by_rgba.clear();
        self.by_mat_list.clear();
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

    fn recolor_material_images(
        &mut self,
        src: &Arc<Vec<MaterialImage>>,
        color_slot: u8,
    ) -> Arc<Vec<MaterialImage>> {
        let key = (Arc::as_ptr(src) as usize, color_slot);
        if let Some(cached) = self.by_mat_list.get(&key) {
            return cached.clone();
        }
        let mut next = Vec::with_capacity(src.len());
        for img in src.iter() {
            // Slot 0 (or diffuse albedo maps with red shorts pixels) gets recolored;
            // non-albedo lookup/specular/normal slots generally have no saturated pure red shorts band
            // or are slot > 0 — we recolor slot 0 directly so normal/specular/rim maps stay untouched.
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
        let arc = Arc::new(next);
        self.by_mat_list.insert(key, arc.clone());
        arc
    }

    /// Apply a player's unique pants color to a cloned Mickey draw command.
    pub fn apply_to_draw(&mut self, draw: &mut GpuDrawCommand, color_slot: u8) {
        if let Some((ref rgba, w, h)) = draw.tex_bytes {
            if !rgba.is_empty() {
                let recolored = self.recolor_rgba(rgba, color_slot);
                draw.tex_bytes = Some((recolored, w, h));
            }
        }
        if !draw.material_images.is_empty() {
            draw.material_images = self.recolor_material_images(&draw.material_images, color_slot);
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

/// Build a remote player's column-major 4x4 transform matrix from interpolated pose + motion flags.
pub fn remote_player_matrix(
    pos: [f32; 3],
    yaw: f32,
    anim_speed: f32,
    flags: u8,
    time_s: f64,
    entity_template: &crate::blotworx::assets::RuntimeEntity,
) -> [f32; 16] {
    let spin_offset = if (flags & FLAG_SPINNING) != 0 {
        (time_s * 18.0) as f32
    } else {
        0.0
    };
    let bob_y = if (flags & FLAG_JUMPING) != 0 {
        0.06
    } else if anim_speed > 0.35 {
        let freq = (anim_speed * 2.4).clamp(6.0, 14.0);
        ((time_s as f32 * freq).sin().abs()) * 0.09
    } else {
        (time_s as f32 * 2.2).sin() * 0.015
    };
    let mut mat = entity_template.controller_matrix(
        [pos[0], pos[1] + bob_y, pos[2]],
        yaw + spin_offset,
    );
    // Subtle paint/thinner action lean when spraying
    if (flags & (FLAG_SPRAY_PAINT | FLAG_SPRAY_THINNER)) != 0 {
        mat[13] += 0.02;
    }
    mat
}

/// Append a remote player's Mickey mesh draws (with their unique pants color) onto `draws`.
pub fn append_remote_player_draws(
    cache: &mut PantsTextureCache,
    base_player_draws: &[GpuDrawCommand],
    remote_matrix: [f32; 16],
    color_slot: u8,
    flags: u8,
    out_draws: &mut Vec<GpuDrawCommand>,
) {
    for base in base_player_draws {
        let mut d = base.clone();
        d.trans_mat = remote_matrix;
        cache.apply_to_draw(&mut d, color_slot);
        // Subtle glow tint if actively spraying Paint or Thinner
        if (flags & FLAG_SPRAY_PAINT) != 0 {
            d.color = [0.92, 0.97, 1.0, 1.0];
        } else if (flags & FLAG_SPRAY_THINNER) != 0 {
            d.color = [0.92, 1.0, 0.92, 1.0];
        }
        out_draws.push(d);
    }
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
