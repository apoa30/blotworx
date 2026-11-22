//! PC-side Epic Mickey to Nintendo 3DS (.bw3d) level bundle converter.
//! Repackages real NIF environment geometry, skinned Mickey & actor NIF models,
//! 128x128 PICA200 Morton-swizzled RGBA5551 textures, collision triangles,
//! paintable/thinnable toon flags, actors, triggers, and dialogues into compact
//! streaming bundles constrained for the Old 3DS 128 MB RAM budget.

use super::assets::{RuntimeEntity, World};
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

const BW3D_MAGIC: u32 = 0x4433_5742; // "BW3D"
const BW3D_VERSION: u32 = 2;
const TEX_DIM: usize = 128;

const SUBMESH_PAINTABLE: u16 = 1 << 0;
const SUBMESH_PAINTED: u16 = 1 << 1;
const SUBMESH_HAZARD: u16 = 1 << 2;
const SUBMESH_DYNAMIC: u16 = 1 << 3;
const SUBMESH_PLAYER_MODEL: u16 = 1 << 5;
const SUBMESH_SKYBOX: u16 = 1 << 6;

const ACTOR_SPATTER: u8 = 1;
const ACTOR_SWEEPER: u8 = 2;
const ACTOR_SPLADOOSH: u8 = 3;
const ACTOR_SEER: u8 = 4;
const ACTOR_BEETLEWORX: u8 = 5;
const ACTOR_BOSS: u8 = 6;
const ACTOR_NPC: u8 = 7;
const ACTOR_TRIGGER_VOL: u8 = 8;
const ACTOR_TRIGGER_DMG: u8 = 9;
const ACTOR_PICKUP: u8 = 10;

const TEAM_HOSTILE: u8 = 2;
const TEAM_NEUTRAL: u8 = 4;

const MAX_3DS_TEXTURES: usize = 112;
const MAX_3DS_VERTICES: usize = 150_000;
const MAX_3DS_INDICES: usize = 360_000;
const MAX_3DS_COLL_TRIS: usize = 10_000;
const MAX_3DS_ACTORS: usize = 128;
const MAX_3DS_DIALOGUES: usize = 48;

pub const DEFAULT_3DS_LEVELS: &[&str] = &[
    "Levels/GV_ZoneA_Start.level",
    "Levels/GV_ZoneB_Start.level",
    "Levels/GV_ZoneC_Start.level",
    "Levels/MeanStreet_V3.Level",
    "Levels/TL_SpaceMountain_TronPete.Level",
    "Levels/PotW_JollyRoger_Start.Level",
    "Levels/HM_StartM.Level",
    "Levels/ShadowBlot_Boss.level",
];

fn morton_8x8(x: usize, y: usize) -> usize {
    let mut i = 0usize;
    for bit in 0..3 {
        i |= ((x >> bit) & 1) << (2 * bit);
        i |= ((y >> bit) & 1) << (2 * bit + 1);
    }
    i
}

/// Downscale RGBA8 image to 128x128 and swizzle into PICA200 8x8 Morton tiles (GPU_RGBA5551).
fn swizzle_rgba_to_pica_128(rgba: &[u8], width: u32, height: u32) -> Vec<u16> {
    let mut out = vec![0xFFFFu16; TEX_DIM * TEX_DIM];
    if width == 0 || height == 0 || rgba.len() < (width as usize * height as usize * 4) {
        return out;
    }
    let w = width as usize;
    let h = height as usize;
    let tiles = TEX_DIM / 8;
    for ty in 0..tiles {
        for tx in 0..tiles {
            let tile_base = (ty * tiles + tx) * 64;
            for py in 0..8 {
                for px in 0..8 {
                    let dst_x = tx * 8 + px;
                    let dst_y = ty * 8 + py;
                    let src_x = (dst_x * w / TEX_DIM).min(w - 1);
                    let src_y = (((TEX_DIM - 1 - dst_y) * h) / TEX_DIM).min(h - 1);
                    let idx = (src_y * w + src_x) * 4;
                    let r = (rgba[idx] >> 3) as u16;
                    let g = (rgba[idx + 1] >> 3) as u16;
                    let b = (rgba[idx + 2] >> 3) as u16;
                    let a = if rgba[idx + 3] >= 96 { 1u16 } else { 0u16 };
                    let pixel = (r << 11) | (g << 6) | (b << 1) | a;
                    out[tile_base + morton_8x8(px, py)] = pixel;
                }
            }
        }
    }
    out
}

fn transform_point(m: &[f32; 16], p: [f32; 3]) -> [f32; 3] {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

fn transform_normal(m: &[f32; 16], n: [f32; 3]) -> [f32; 3] {
    let rx = m[0] * n[0] + m[4] * n[1] + m[8] * n[2];
    let ry = m[1] * n[0] + m[5] * n[1] + m[9] * n[2];
    let rz = m[2] * n[0] + m[6] * n[1] + m[10] * n[2];
    let len = (rx * rx + ry * ry + rz * rz).sqrt();
    if len > 1e-5 {
        [rx / len, ry / len, rz / len]
    } else {
        [0.0, 1.0, 0.0]
    }
}

fn fixed_bytes<const N: usize>(s: &str) -> [u8; N] {
    let mut buf = [0u8; N];
    let bytes = s.as_bytes();
    let n = bytes.len().min(N.saturating_sub(1));
    buf[..n].copy_from_slice(&bytes[..n]);
    buf
}

fn classify_entity_actor(entity: &RuntimeEntity) -> Option<(u8, u8)> {
    let lower = entity.name.to_ascii_lowercase();
    let has_comp = |name: &str| entity.components.iter().any(|c| c.class_name == name);
    if has_comp("TriggerDamageComponent") {
        return Some((ACTOR_TRIGGER_DMG, TEAM_NEUTRAL));
    }
    if has_comp("TriggerVolumeComponent") || has_comp("TriggerPlaneComponent") {
        return Some((ACTOR_TRIGGER_VOL, TEAM_NEUTRAL));
    }
    if has_comp("PickupComponent") || lower.contains("eticket") || lower.contains("pickup") {
        return Some((ACTOR_PICKUP, TEAM_NEUTRAL));
    }
    if lower.contains("tronpete")
        || lower.contains("animatronichook")
        || lower.contains("shadowblot")
        || lower.contains("clockworkboss")
    {
        return Some((ACTOR_BOSS, TEAM_HOSTILE));
    }
    if lower.contains("beetleworx") || lower.contains("slobber") || lower.contains("theblob") {
        return Some((ACTOR_BEETLEWORX, TEAM_HOSTILE));
    }
    if lower.contains("spladoosh") || lower.contains("kamikaze") {
        return Some((ACTOR_SPLADOOSH, TEAM_HOSTILE));
    }
    if lower.contains("sweeper") || lower.contains("spatterranged") {
        return Some((ACTOR_SWEEPER, TEAM_HOSTILE));
    }
    if lower.contains("seer") || lower.contains("fodderblotling") {
        return Some((ACTOR_SEER, TEAM_HOSTILE));
    }
    if lower.contains("spatter") {
        return Some((ACTOR_SPATTER, TEAM_HOSTILE));
    }
    if lower.contains("gremlin")
        || has_comp("ConversationComponent")
        || has_comp("UsableComponent")
    {
        return Some((ACTOR_NPC, TEAM_NEUTRAL));
    }
    if has_comp("AIBrainComponent") {
        return Some((ACTOR_SPATTER, TEAM_HOSTILE));
    }
    None
}

pub fn export_world_to_bw3d(world: &World, output_path: &Path) -> Result<()> {
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut draws = world.draws.clone();
    let _ = world.apply_model_geometry(&mut draws);
    let _ = world.apply_model_materials(&mut draws);

    let player_ent = world.player_entity;
    let player_pos = [
        world.entities[player_ent].transform[12],
        world.entities[player_ent].transform[13],
        world.entities[player_ent].transform[14],
    ];
    let spawn = if player_pos.iter().all(|v| v.is_finite()) && player_pos != [0.0; 3] {
        player_pos
    } else {
        world.report.spawn
    };
    let spawn_yaw = 0.0f32;

    // 1. Build Dialogues & Actors first so we know which entities are dynamic actors
    #[repr(C, packed)]
    struct RawDialogue {
        speaker: [u8; 32],
        text: [u8; 128],
    }

    #[repr(C, packed)]
    struct RawActor {
        kind: u8,
        team: u8,
        hp: i16,
        max_hp: i16,
        dialogue_id: i16,
        pos: [f32; 3],
        yaw: f32,
        half_extents: [f32; 3],
        patrol_pos: [f32; 3],
        paint_val: f32,
        paint_thresh: f32,
        thin_thresh: f32,
        name: [u8; 32],
    }

    let mut dialogues: Vec<RawDialogue> = Vec::new();
    dialogues.push(RawDialogue {
        speaker: fixed_bytes("Gremlin Gus"),
        text: fixed_bytes(&format!(
            "Welcome to {}! Use R for Paint, L for Thinner, Y to Spin, X for Blast, and touch to aim!",
            world.report.level
        )),
    });

    let mut actors: Vec<RawActor> = Vec::new();
    let mut ent_to_actor: HashMap<usize, (u16, [f32; 3])> = HashMap::new();

    for (ent_idx, ent) in world.entities.iter().enumerate() {
        if ent_idx == player_ent || actors.len() >= MAX_3DS_ACTORS || world.removed_entities.contains(&ent_idx) {
            continue;
        }
        let Some((kind, team)) = classify_entity_actor(ent) else {
            continue;
        };
        let pos = [ent.transform[12], ent.transform[13], ent.transform[14]];
        if pos.iter().any(|v| !v.is_finite()) {
            continue;
        }
        let hp = world
            .runtime_health
            .get(&ent_idx)
            .map(|h| h.health.clamp(1, 100) as i16)
            .unwrap_or(if kind == ACTOR_BOSS { 12 } else { 3 });
        let dlg_id = if matches!(kind, ACTOR_NPC | ACTOR_TRIGGER_VOL) {
            if dialogues.len() < MAX_3DS_DIALOGUES {
                let id = dialogues.len() as i16;
                dialogues.push(RawDialogue {
                    speaker: fixed_bytes(&ent.name),
                    text: fixed_bytes(&format!(
                        "{}: Explore {} with Paint (R) and Thinner (L)!",
                        ent.name, world.report.level
                    )),
                });
                id
            } else {
                0
            }
        } else {
            -1
        };
        let actor_id = actors.len() as u16;
        actors.push(RawActor {
            kind,
            team,
            hp,
            max_hp: hp,
            dialogue_id: dlg_id,
            pos,
            yaw: 0.0,
            half_extents: [2.2, 2.0, 2.2],
            patrol_pos: [pos[0] + 2.5, pos[1], pos[2]],
            paint_val: 0.0,
            paint_thresh: 3.0,
            thin_thresh: 3.0,
            name: fixed_bytes(&ent.name),
        });
        ent_to_actor.insert(ent_idx, (actor_id, pos));
    }

    // Also associate child entities (like mk_hose_nozzle or Beetleworx shells / Gremlin wrenches) with their root parent
    let mut player_entities: HashSet<usize> = HashSet::from([player_ent]);
    for (&child, local) in &world.transform_hierarchy {
        let mut p = local.parent;
        for _ in 0..8 {
            if p == player_ent {
                player_entities.insert(child);
                break;
            }
            if let Some(&(act_id, act_pos)) = ent_to_actor.get(&p) {
                ent_to_actor.insert(child, (act_id, act_pos));
                break;
            }
            if let Some(next) = world.transform_hierarchy.get(&p) {
                p = next.parent;
            } else {
                break;
            }
        }
    }

    // Map draw index -> entity owner
    let mut draw_owner: HashMap<usize, usize> = HashMap::new();
    for (ent_idx, ent) in world.entities.iter().enumerate() {
        if world.removed_entities.contains(&ent_idx) {
            continue;
        }
        // Always export Mickey's draws and active scene entities
        if !player_entities.contains(&ent_idx)
            && (world.hidden_entities.contains(&ent_idx) || !world.entity_scene_active(ent_idx))
        {
            continue;
        }
        for d in ent.draws.clone() {
            draw_owner.insert(d, ent_idx);
        }
    }

    // Collect candidate draws
    let mut candidate_draws: Vec<(usize, f32, u16, u16, [f32; 3])> = Vec::new();
    for (d_idx, draw) in draws.iter().enumerate() {
        if draw.projected_shadow || world.hidden_entity_draws.contains(&d_idx) {
            continue;
        }
        let v_count = draw.v_data.len() / 12;
        if v_count == 0 || draw.indices.len() < 3 || v_count > 30_000 {
            continue;
        }
        let Some(&ent_idx) = draw_owner.get(&d_idx) else {
            continue;
        };
        let ent = &world.entities[ent_idx];
        let lower = ent.name.to_ascii_lowercase();

        let mut flags = SUBMESH_PAINTED;
        let mut actor_id = 0u16;
        let mut origin_offset = [0.0f32; 3];

        if player_entities.contains(&ent_idx) {
            flags |= SUBMESH_PLAYER_MODEL;
            origin_offset = player_pos;
        } else if let Some(&(act_id, act_pos)) = ent_to_actor.get(&ent_idx) {
            flags |= SUBMESH_DYNAMIC;
            actor_id = act_id;
            origin_offset = act_pos;
        } else if draw.skybox {
            flags |= SUBMESH_SKYBOX;
        } else {
            let is_paintable = ent.components.iter().any(|c| {
                matches!(
                    c.class_name.as_str(),
                    "JigsawPaintComponent" | "WholeModelPaintComponent"
                )
            }) || lower.contains("toon");
            if is_paintable {
                flags |= SUBMESH_PAINTABLE;
                let painted = ent
                    .components
                    .iter()
                    .find(|c| c.class_name == "WholeModelPaintComponent")
                    .and_then(|c| c.properties.iter().find(|p| p.name == "IsPainted"))
                    .and_then(|p| p.value.as_bool())
                    .unwrap_or(true);
                if !painted {
                    flags &= !SUBMESH_PAINTED;
                }
            }
            if lower.contains("thinner") || lower.contains("acid") || lower.contains("river") {
                flags |= SUBMESH_HAZARD;
            }
        }

        let local_center = [
            (draw.aabb_min[0] + draw.aabb_max[0]) * 0.5,
            (draw.aabb_min[1] + draw.aabb_max[1]) * 0.5,
            (draw.aabb_min[2] + draw.aabb_max[2]) * 0.5,
        ];
        let center = transform_point(&draw.trans_mat, local_center);
        let dx = center[0] - spawn[0];
        let dy = center[1] - spawn[1];
        let dz = center[2] - spawn[2];
        let mut score = dx * dx + dy * dy + dz * dz;
        if flags & (SUBMESH_PLAYER_MODEL | SUBMESH_DYNAMIC | SUBMESH_SKYBOX) != 0 {
            score = -1000.0; // Always include Mickey, actors, and skybox first!
        } else if flags & SUBMESH_PAINTABLE != 0 {
            score *= 0.25;
        }
        candidate_draws.push((d_idx, score, flags, actor_id, origin_offset));
    }
    candidate_draws.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut textures: Vec<Vec<u16>> = Vec::new();
    let mut tex_map: HashMap<usize, i16> = HashMap::new();

    #[repr(C, packed)]
    struct RawSubmesh {
        base_vertex: u32,
        first_index: u32,
        index_count: u32,
        tex_id: i16,
        flags: u16,
        actor_id: u16,
        jigsaw_piece: u16,
        aabb_min: [f32; 3],
        aabb_max: [f32; 3],
    }

    #[repr(C, packed)]
    struct RawVertex {
        pos: [f32; 3],
        uv: [f32; 2],
        rgba: [u8; 4],
        nrm: [f32; 3],
    }

    #[repr(C, packed)]
    struct RawCollTri {
        v0: [f32; 3],
        v1: [f32; 3],
        v2: [f32; 3],
        normal: [f32; 3],
        flags: u16,
        submesh_id: i16,
    }

    let mut submeshes: Vec<RawSubmesh> = Vec::new();
    let mut vertices: Vec<RawVertex> = Vec::new();
    let mut indices: Vec<u16> = Vec::new();
    let mut coll_tris: Vec<RawCollTri> = Vec::new();

    for (d_idx, _, flags, actor_id, origin_offset) in candidate_draws {
        let draw = &draws[d_idx];
        let v_count = draw.v_data.len() / 12;
        if v_count > 65000
            || vertices.len() + v_count > MAX_3DS_VERTICES
            || indices.len() + draw.indices.len() > MAX_3DS_INDICES
        {
            continue;
        }

        // Select the primary diffuse texture (prefer draw.tex_bytes, then largest non-empty MaterialImage >= 32px)
        let best_img = draw
            .tex_bytes
            .as_ref()
            .filter(|(rgba, w, h)| !rgba.is_empty() && *w > 0 && *h > 0)
            .map(|(rgba, w, h)| (rgba, *w, *h))
            .or_else(|| {
                draw.material_images
                    .iter()
                    .filter(|img| !img.rgba.is_empty() && img.width >= 32 && img.height >= 32)
                    .max_by_key(|img| img.width * img.height)
                    .or_else(|| draw.material_images.iter().find(|img| !img.rgba.is_empty()))
                    .map(|img| (&img.rgba, img.width, img.height))
            });

        let tex_id = if let Some((rgba, w, h)) = best_img {
            let key = std::sync::Arc::as_ptr(rgba) as usize;
            if let Some(&existing) = tex_map.get(&key) {
                existing
            } else if textures.len() < MAX_3DS_TEXTURES {
                let id = textures.len() as i16;
                textures.push(swizzle_rgba_to_pica_128(rgba, w, h));
                tex_map.insert(key, id);
                id
            } else {
                -1
            }
        } else {
            -1
        };

        let base_vertex = vertices.len() as u32;
        let first_index = indices.len() as u32;
        let mut aabb_min = [f32::INFINITY; 3];
        let mut aabb_max = [f32::NEG_INFINITY; 3];

        for chunk in draw.v_data.chunks_exact(12) {
            let wp = transform_point(&draw.trans_mat, [chunk[0], chunk[1], chunk[2]]);
            let wn = transform_normal(&draw.trans_mat, [chunk[3], chunk[4], chunk[5]]);
            for axis in 0..3 {
                aabb_min[axis] = aabb_min[axis].min(wp[axis]);
                aabb_max[axis] = aabb_max[axis].max(wp[axis]);
            }
            let pos = [
                wp[0] - origin_offset[0],
                wp[1] - origin_offset[1],
                wp[2] - origin_offset[2],
            ];
            let r = (chunk[8].clamp(0.0, 1.0) * draw.color[0].clamp(0.0, 1.0) * 255.0) as u8;
            let g = (chunk[9].clamp(0.0, 1.0) * draw.color[1].clamp(0.0, 1.0) * 255.0) as u8;
            let b = (chunk[10].clamp(0.0, 1.0) * draw.color[2].clamp(0.0, 1.0) * 255.0) as u8;
            let a = (chunk[11].clamp(0.0, 1.0) * draw.color[3].clamp(0.0, 1.0) * 255.0) as u8;
            vertices.push(RawVertex {
                pos,
                uv: [chunk[6], chunk[7]],
                rgba: [r.max(48), g.max(48), b.max(48), a.max(180)],
                nrm: wn,
            });
        }

        let submesh_id = submeshes.len() as i16;
        let is_env = flags & (SUBMESH_PLAYER_MODEL | SUBMESH_DYNAMIC | SUBMESH_SKYBOX) == 0;
        for tri in draw.indices.chunks_exact(3) {
            let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            if i0 >= v_count || i1 >= v_count || i2 >= v_count {
                continue;
            }
            indices.push(i0 as u16);
            indices.push(i1 as u16);
            indices.push(i2 as u16);

            if is_env && coll_tris.len() < MAX_3DS_COLL_TRIS {
                let p0 = vertices[base_vertex as usize + i0].pos;
                let p1 = vertices[base_vertex as usize + i1].pos;
                let p2 = vertices[base_vertex as usize + i2].pos;
                let e1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
                let e2 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
                let nx = e1[1] * e2[2] - e1[2] * e2[1];
                let ny = e1[2] * e2[0] - e1[0] * e2[2];
                let nz = e1[0] * e2[1] - e1[1] * e2[0];
                let len = (nx * nx + ny * ny + nz * nz).sqrt();
                if len > 0.05 {
                    let mut normal = [nx / len, ny / len, nz / len];
                    if normal[1] < -0.35 {
                        normal = [-normal[0], -normal[1], -normal[2]];
                    }
                    if normal[1] >= 0.30 {
                        coll_tris.push(RawCollTri {
                            v0: p0,
                            v1: p1,
                            v2: p2,
                            normal,
                            flags: if flags & SUBMESH_HAZARD != 0 { 2 } else { 1 },
                            submesh_id,
                        });
                    }
                }
            }
        }

        let index_count = (indices.len() as u32) - first_index;
        if index_count > 0 {
            submeshes.push(RawSubmesh {
                base_vertex,
                first_index,
                index_count,
                tex_id,
                flags,
                actor_id,
                jigsaw_piece: 0,
                aabb_min,
                aabb_max,
            });
        }
    }

    // Always ensure a walkable spawn platform pad if level collision was sparse
    if coll_tris.len() < MAX_3DS_COLL_TRIS.saturating_sub(2) {
        let sy = spawn[1];
        coll_tris.push(RawCollTri {
            v0: [spawn[0] - 4.0, sy, spawn[2] - 4.0],
            v1: [spawn[0] - 4.0, sy, spawn[2] + 4.0],
            v2: [spawn[0] + 4.0, sy, spawn[2] + 4.0],
            normal: [0.0, 1.0, 0.0],
            flags: 1,
            submesh_id: -1,
        });
        coll_tris.push(RawCollTri {
            v0: [spawn[0] - 4.0, sy, spawn[2] - 4.0],
            v1: [spawn[0] + 4.0, sy, spawn[2] + 4.0],
            v2: [spawn[0] + 4.0, sy, spawn[2] - 4.0],
            normal: [0.0, 1.0, 0.0],
            flags: 1,
            submesh_id: -1,
        });
    }

    // Write binary .bw3d file
    let mut file = std::fs::File::create(output_path)
        .with_context(|| format!("Failed to create 3DS bundle {}", output_path.display()))?;

    let mut header = Vec::with_capacity(128);
    header.extend_from_slice(&BW3D_MAGIC.to_le_bytes());
    header.extend_from_slice(&BW3D_VERSION.to_le_bytes());
    header.extend_from_slice(&fixed_bytes::<64>(&world.report.level));
    for f in spawn {
        header.extend_from_slice(&f.to_le_bytes());
    }
    header.extend_from_slice(&spawn_yaw.to_le_bytes());
    header.extend_from_slice(&[24u8, 34, 58, 255]);
    header.extend_from_slice(&(textures.len() as u32).to_le_bytes());
    header.extend_from_slice(&(submeshes.len() as u32).to_le_bytes());
    header.extend_from_slice(&(vertices.len() as u32).to_le_bytes());
    header.extend_from_slice(&(indices.len() as u32).to_le_bytes());
    header.extend_from_slice(&(coll_tris.len() as u32).to_le_bytes());
    header.extend_from_slice(&(actors.len() as u32).to_le_bytes());
    header.extend_from_slice(&(dialogues.len() as u32).to_le_bytes());
    header.extend_from_slice(&[0u8; 8]); // reserved[2]
    debug_assert_eq!(header.len(), 128);
    file.write_all(&header)?;

    let dim_u16 = TEX_DIM as u16;
    for tex in &textures {
        file.write_all(&dim_u16.to_le_bytes())?;
        file.write_all(&dim_u16.to_le_bytes())?;
        for &px in tex {
            file.write_all(&px.to_le_bytes())?;
        }
    }
    for sm in &submeshes {
        let bytes = unsafe {
            std::slice::from_raw_parts((sm as *const RawSubmesh).cast::<u8>(), std::mem::size_of::<RawSubmesh>())
        };
        file.write_all(bytes)?;
    }
    for v in &vertices {
        let bytes = unsafe {
            std::slice::from_raw_parts((v as *const RawVertex).cast::<u8>(), std::mem::size_of::<RawVertex>())
        };
        file.write_all(bytes)?;
    }
    for &idx in &indices {
        file.write_all(&idx.to_le_bytes())?;
    }
    for ct in &coll_tris {
        let bytes = unsafe {
            std::slice::from_raw_parts((ct as *const RawCollTri).cast::<u8>(), std::mem::size_of::<RawCollTri>())
        };
        file.write_all(bytes)?;
    }
    for a in &actors {
        let bytes = unsafe {
            std::slice::from_raw_parts((a as *const RawActor).cast::<u8>(), std::mem::size_of::<RawActor>())
        };
        file.write_all(bytes)?;
    }
    for d in &dialogues {
        let bytes = unsafe {
            std::slice::from_raw_parts((d as *const RawDialogue).cast::<u8>(), std::mem::size_of::<RawDialogue>())
        };
        file.write_all(bytes)?;
    }
    file.flush()?;
    Ok(())
}

pub fn level_bundle_filename(level: &str) -> String {
    let stem = Path::new(level)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("level");
    format!("{stem}.bw3d")
}

pub fn export_bundle_cli(data_root: &Path, output_dir: &Path, levels: &[String]) -> Result<Vec<PathBuf>> {
    let levels_dir = if output_dir
        .file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("levels"))
    {
        output_dir.to_path_buf()
    } else {
        output_dir.join("levels")
    };
    std::fs::create_dir_all(&levels_dir)?;
    let list: Vec<String> = if levels.is_empty() {
        DEFAULT_3DS_LEVELS.iter().map(|s| (*s).to_owned()).collect()
    } else {
        levels.to_vec()
    };

    let ctx = egui::Context::default();
    let mut exported = Vec::new();
    for level in &list {
        let mut world = super::assets::load(data_root, level, &ctx)
            .with_context(|| format!("Failed to load {level} for 3DS export"))?;
        let _ = world.enable_demo_story();
        let _ = world.prepare_gameplay_scene_requests();
        let mut player = world.take_startup_player();
        let _ = world.attach_player_body();
        for tick in 1..=15 {
            let _ = world.advance_player_models(&mut player, tick as f32 * super::STEP);
        }
        let out_file = levels_dir.join(level_bundle_filename(level));
        export_world_to_bw3d(&world, &out_file)?;
        println!(
            "[export-3ds] Exported {} -> {} ({} bytes)",
            level,
            out_file.display(),
            std::fs::metadata(&out_file).map(|m| m.len()).unwrap_or(0)
        );
        exported.push(out_file);
    }
    Ok(exported)
}
