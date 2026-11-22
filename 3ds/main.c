#include <3ds.h>
#include <citro3d.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <dirent.h>

#include "bw3d_format.h"
#include "vshader_shbin.h"

#define CLEAR_COLOR_DEFAULT 0x182238FFu
#define DISPLAY_TRANSFER_FLAGS \
    (GX_TRANSFER_FLIP_VERT(0) | GX_TRANSFER_OUT_TILED(0) | GX_TRANSFER_RAW_COPY(0) | \
     GX_TRANSFER_IN_FORMAT(GX_TRANSFER_FMT_RGBA8) | GX_TRANSFER_OUT_FORMAT(GX_TRANSFER_FMT_RGB8) | \
     GX_TRANSFER_SCALING(GX_TRANSFER_SCALE_NO))

#define MAX_LEVEL_PATHS 32
#define MAX_DYN_VERTS   6144
#define MAX_PROJECTILES 32
#define MAX_PARTICLES   96

/* Cap heap and linear allocations to 24 MB each so Old 3DS (64 MB app region / 128 MB total) has 16+ MB free headroom */
u32 __ctru_heap_size        = 24 * 1024 * 1024;
u32 __ctru_linear_heap_size = 24 * 1024 * 1024;

typedef struct {
    Bw3dActor base;
    float     spawn_pos[3];
    float     vel[3];
    float     state_timer;
    float     cooldown;
    float     stun_timer;
    uint8_t   ai_state;   /* 0=idle/patrol, 1=chase, 2=windup, 3=attack/charge, 4=recover, 5=dead */
    uint8_t   triggered;
    uint8_t   armor_intact;
    uint8_t   patrol_dir;
} LiveActor;

typedef struct {
    uint8_t active;
    uint8_t from_player;
    uint8_t is_paint;
    float   pos[3];
    float   vel[3];
    float   ttl;
    float   radius;
} Projectile;

typedef struct {
    uint8_t active;
    uint8_t rgba[4];
    float   pos[3];
    float   vel[3];
    float   ttl;
    float   size;
} Particle;

typedef struct {
    float   pos[3];
    float   vel[3];
    float   spawn_pos[3];
    float   yaw;
    float   cam_yaw;
    float   cam_pitch;
    float   aim_yaw_off;
    float   aim_pitch_off;
    float   aim_world[3];
    int     hp;
    int     max_hp;
    float   paint;
    float   max_paint;
    float   thinner;
    float   max_thinner;
    int     etickets;
    uint8_t grounded;
    uint8_t jump_count;
    uint8_t air_ink_avail;
    uint8_t ledge_grabbed;
    float   ledge_pos[3];
    float   spin_timer;
    float   blast_timer;
    uint8_t blast_is_paint;
    float   iframe_timer;
    uint8_t sliding;
    uint8_t input_restricted;
    int     active_dialogue;
} PlayerState;

/* GPU state */
static DVLB_s*         g_dvlb = NULL;
static shaderProgram_s g_program;
static int             g_uLoc_proj = -1;
static int             g_uLoc_modelView = -1;
static int             g_uLoc_lightDir = -1;
static int             g_uLoc_materialTint = -1;
static C3D_RenderTarget* g_targetLeft = NULL;
static C3D_RenderTarget* g_targetRight = NULL;
static C3D_Tex         g_whiteTex;

/* Active Level Resources (strictly freed before loading next level for 128 MB Old3DS budget) */
static Bw3dHeader      g_hdr;
static C3D_Tex*        g_textures = NULL;
static uint8_t*        g_tex_valid = NULL;
static Bw3dSubmesh*    g_submeshes = NULL;
static Bw3dVertex*     g_level_vbo = NULL;
static uint16_t*       g_level_ibo = NULL;
static Bw3dCollTri*    g_coll_tris = NULL;
static LiveActor*      g_actors = NULL;
static Bw3dDialogue*   g_dialogues = NULL;

/* Dynamic frame VBO in linear RAM for Mickey, enemies, projectiles, particles, reticle */
static Bw3dVertex*     g_dyn_vbo = NULL;
static uint32_t        g_dyn_vbo_count = 0;

static PlayerState     g_player;
static Projectile      g_projectiles[MAX_PROJECTILES];
static Particle        g_particles[MAX_PARTICLES];

static char            g_level_paths[MAX_LEVEL_PATHS][256];
static char            g_level_names[MAX_LEVEL_PATHS][48];
static int             g_num_levels = 0;
static int             g_cur_level = 0;
static int             g_menu_cursor = 0;
static uint8_t         g_show_level_menu = 0;
static float           g_time_sec = 0.0f;
static char            g_status_msg[64] = "Welcome to Blotworx 3DS!";
static float           g_status_timer = 4.0f;

static float clampf(float v, float lo, float hi) {
    return v < lo ? lo : (v > hi ? hi : v);
}

static float dist3_sq(const float a[3], const float b[3]) {
    float dx = a[0] - b[0], dy = a[1] - b[1], dz = a[2] - b[2];
    return dx * dx + dy * dy + dz * dz;
}

static void set_status(const char* msg, float duration) {
    strncpy(g_status_msg, msg, sizeof(g_status_msg) - 1);
    g_status_msg[sizeof(g_status_msg) - 1] = '\0';
    g_status_timer = duration;
}

static void spawn_particle(const float pos[3], const float vel[3], uint8_t r, uint8_t g, uint8_t b, float size, float ttl) {
    for (int i = 0; i < MAX_PARTICLES; ++i) {
        if (!g_particles[i].active) {
            g_particles[i].active = 1;
            g_particles[i].pos[0] = pos[0];
            g_particles[i].pos[1] = pos[1];
            g_particles[i].pos[2] = pos[2];
            g_particles[i].vel[0] = vel[0];
            g_particles[i].vel[1] = vel[1];
            g_particles[i].vel[2] = vel[2];
            g_particles[i].rgba[0] = r;
            g_particles[i].rgba[1] = g;
            g_particles[i].rgba[2] = b;
            g_particles[i].rgba[3] = 255;
            g_particles[i].size = size;
            g_particles[i].ttl = ttl;
            return;
        }
    }
}

static void spawn_projectile(const float pos[3], const float vel[3], uint8_t from_player, uint8_t is_paint, float radius, float ttl) {
    for (int i = 0; i < MAX_PROJECTILES; ++i) {
        if (!g_projectiles[i].active) {
            g_projectiles[i].active = 1;
            g_projectiles[i].from_player = from_player;
            g_projectiles[i].is_paint = is_paint;
            g_projectiles[i].pos[0] = pos[0];
            g_projectiles[i].pos[1] = pos[1];
            g_projectiles[i].pos[2] = pos[2];
            g_projectiles[i].vel[0] = vel[0];
            g_projectiles[i].vel[1] = vel[1];
            g_projectiles[i].vel[2] = vel[2];
            g_projectiles[i].radius = radius;
            g_projectiles[i].ttl = ttl;
            return;
        }
    }
}

static void unload_current_level(void) {
    C3D_FrameSync();
    if (g_textures) {
        for (uint32_t i = 0; i < g_hdr.num_textures; ++i) {
            if (g_tex_valid && g_tex_valid[i]) {
                C3D_TexDelete(&g_textures[i]);
            }
        }
        free(g_textures);
        g_textures = NULL;
    }
    if (g_tex_valid) { free(g_tex_valid); g_tex_valid = NULL; }
    if (g_submeshes) { free(g_submeshes); g_submeshes = NULL; }
    if (g_level_vbo) { linearFree(g_level_vbo); g_level_vbo = NULL; }
    if (g_level_ibo) { linearFree(g_level_ibo); g_level_ibo = NULL; }
    if (g_coll_tris) { free(g_coll_tris); g_coll_tris = NULL; }
    if (g_actors)    { free(g_actors); g_actors = NULL; }
    if (g_dialogues) { free(g_dialogues); g_dialogues = NULL; }
    memset(&g_hdr, 0, sizeof(g_hdr));
    memset(g_projectiles, 0, sizeof(g_projectiles));
    memset(g_particles, 0, sizeof(g_particles));
}

static void append_box_to_vbo(Bw3dVertex* vbo, uint32_t* vcount, uint16_t* ibo, uint32_t* icount,
                              float cx, float cy, float cz, float hx, float hy, float hz,
                              uint8_t r, uint8_t g, uint8_t b, uint8_t a) {
    static const float faces[6][4][3] = {
        {{-1,-1, 1}, { 1,-1, 1}, { 1, 1, 1}, {-1, 1, 1}}, /* +Z */
        {{ 1,-1,-1}, {-1,-1,-1}, {-1, 1,-1}, { 1, 1,-1}}, /* -Z */
        {{-1, 1, 1}, { 1, 1, 1}, { 1, 1,-1}, {-1, 1,-1}}, /* +Y */
        {{-1,-1,-1}, { 1,-1,-1}, { 1,-1, 1}, {-1,-1, 1}}, /* -Y */
        {{ 1,-1, 1}, { 1,-1,-1}, { 1, 1,-1}, { 1, 1, 1}}, /* +X */
        {{-1,-1,-1}, {-1,-1, 1}, {-1, 1, 1}, {-1, 1,-1}}, /* -X */
    };
    static const float norms[6][3] = {
        {0,0,1}, {0,0,-1}, {0,1,0}, {0,-1,0}, {1,0,0}, {-1,0,0}
    };
    static const float uvs[4][2] = {{0,0}, {1,0}, {1,1}, {0,1}};
    for (int f = 0; f < 6; ++f) {
        uint16_t base = (uint16_t)(*vcount);
        for (int v = 0; v < 4; ++v) {
            Bw3dVertex* vert = &vbo[(*vcount)++];
            vert->pos[0] = cx + faces[f][v][0] * hx;
            vert->pos[1] = cy + faces[f][v][1] * hy;
            vert->pos[2] = cz + faces[f][v][2] * hz;
            vert->uv[0]  = uvs[v][0];
            vert->uv[1]  = uvs[v][1];
            vert->rgba[0] = r; vert->rgba[1] = g; vert->rgba[2] = b; vert->rgba[3] = a;
            vert->nrm[0] = norms[f][0]; vert->nrm[1] = norms[f][1]; vert->nrm[2] = norms[f][2];
        }
        if (ibo && icount) {
            ibo[(*icount)++] = base + 0;
            ibo[(*icount)++] = base + 1;
            ibo[(*icount)++] = base + 2;
            ibo[(*icount)++] = base + 0;
            ibo[(*icount)++] = base + 2;
            ibo[(*icount)++] = base + 3;
        }
    }
}

static void add_floor_coll_box(Bw3dCollTri* tris, uint32_t* tcount, float cx, float top_y, float cz, float hx, float hz, uint16_t flags, int16_t submesh_id) {
    Bw3dCollTri* t0 = &tris[(*tcount)++];
    t0->v0[0] = cx - hx; t0->v0[1] = top_y; t0->v0[2] = cz - hz;
    t0->v1[0] = cx - hx; t0->v1[1] = top_y; t0->v1[2] = cz + hz;
    t0->v2[0] = cx + hx; t0->v2[1] = top_y; t0->v2[2] = cz + hz;
    t0->normal[0] = 0.0f; t0->normal[1] = 1.0f; t0->normal[2] = 0.0f;
    t0->flags = flags; t0->submesh_id = submesh_id;

    Bw3dCollTri* t1 = &tris[(*tcount)++];
    t1->v0[0] = cx - hx; t1->v0[1] = top_y; t1->v0[2] = cz - hz;
    t1->v1[0] = cx + hx; t1->v1[1] = top_y; t1->v1[2] = cz + hz;
    t1->v2[0] = cx + hx; t1->v2[1] = top_y; t1->v2[2] = cz - hz;
    t1->normal[0] = 0.0f; t1->normal[1] = 1.0f; t1->normal[2] = 0.0f;
    t1->flags = flags; t1->submesh_id = submesh_id;
}

static void reset_player_to_spawn(void) {
    memset(&g_player, 0, sizeof(g_player));
    g_player.pos[0] = g_hdr.spawn_pos[0];
    g_player.pos[1] = g_hdr.spawn_pos[1];
    g_player.pos[2] = g_hdr.spawn_pos[2];
    g_player.spawn_pos[0] = g_hdr.spawn_pos[0];
    g_player.spawn_pos[1] = g_hdr.spawn_pos[1];
    g_player.spawn_pos[2] = g_hdr.spawn_pos[2];
    g_player.yaw = g_hdr.spawn_yaw;
    g_player.cam_yaw = g_hdr.spawn_yaw;
    g_player.cam_pitch = 0.30f;
    g_player.hp = 5;
    g_player.max_hp = 5;
    g_player.paint = 100.0f;
    g_player.max_paint = 100.0f;
    g_player.thinner = 100.0f;
    g_player.max_thinner = 100.0f;
    g_player.etickets = 0;
    g_player.active_dialogue = -1;
    g_player.air_ink_avail = 1;
}

/* Built-in fallback Gremlin Village showcase arena if no .bw3d bundle is found */
static void build_builtin_fallback_level(void) {
    unload_current_level();
    g_hdr.magic = BW3D_MAGIC;
    g_hdr.version = BW3D_VERSION;
    strncpy(g_hdr.title, "GV_ZoneA_Start (Built-in 3DS Showcase)", sizeof(g_hdr.title) - 1);
    g_hdr.spawn_pos[0] = 0.0f; g_hdr.spawn_pos[1] = 0.5f; g_hdr.spawn_pos[2] = 14.0f;
    g_hdr.spawn_yaw = 0.0f;
    g_hdr.sky_rgba[0] = 24; g_hdr.sky_rgba[1] = 34; g_hdr.sky_rgba[2] = 58; g_hdr.sky_rgba[3] = 255;

    g_hdr.num_submeshes = 7;
    g_hdr.num_vertices  = 7 * 24;
    g_hdr.num_indices   = 7 * 36;
    g_hdr.num_coll_tris = 14;
    g_hdr.num_actors    = 9;
    g_hdr.num_dialogues = 3;

    g_submeshes = (Bw3dSubmesh*)calloc(g_hdr.num_submeshes, sizeof(Bw3dSubmesh));
    g_level_vbo = (Bw3dVertex*)linearAlloc(g_hdr.num_vertices * sizeof(Bw3dVertex));
    g_level_ibo = (uint16_t*)linearAlloc(g_hdr.num_indices * sizeof(uint16_t));
    g_coll_tris = (Bw3dCollTri*)calloc(g_hdr.num_coll_tris, sizeof(Bw3dCollTri));
    g_actors    = (LiveActor*)calloc(g_hdr.num_actors, sizeof(LiveActor));
    g_dialogues = (Bw3dDialogue*)calloc(g_hdr.num_dialogues, sizeof(Bw3dDialogue));

    uint32_t vc = 0, ic = 0, tc = 0;
    struct { float cx, cy, cz, hx, hy, hz; uint8_t r, g, b; uint16_t flags; } blocks[7] = {
        /* 0: Start island */
        {  0.0f, -0.5f,  12.0f,  7.0f, 0.5f,  6.0f,  95, 115, 145, SUBMESH_PAINTED },
        /* 1: Thinner River hazard */
        {  0.0f, -2.2f,   0.0f, 26.0f, 0.3f, 26.0f,  35, 220,  65, SUBMESH_HAZARD | SUBMESH_PAINTED },
        /* 2: Paintable Jigsaw Bridge 1 (starts thinned!) */
        {  0.0f, -0.4f,   3.5f,  2.4f, 0.4f,  2.5f,  65, 175, 250, SUBMESH_PAINTABLE },
        /* 3: Paintable Jigsaw Bridge 2 (starts painted!) */
        {  0.0f, -0.4f,  -1.5f,  2.4f, 0.4f,  2.5f, 235, 185,  70, SUBMESH_PAINTABLE | SUBMESH_PAINTED },
        /* 4: Main Courtyard Platform */
        {  0.0f, -0.5f, -11.0f, 11.0f, 0.5f,  7.0f, 110, 100, 140, SUBMESH_PAINTED },
        /* 5: Elevated Boss/Gremlin Tower Platform (Ledge-grabbable!) */
        {  0.0f,  1.2f, -20.0f,  6.0f, 1.7f,  4.0f, 130, 120, 165, SUBMESH_PAINTED },
        /* 6: Thinnable Wall blocking secret E-Ticket alcove */
        {  6.5f,  1.2f, -11.0f,  1.5f, 1.2f,  1.5f, 240,  95, 165, SUBMESH_PAINTABLE | SUBMESH_PAINTED },
    };

    for (int i = 0; i < 7; ++i) {
        g_submeshes[i].first_index = ic;
        g_submeshes[i].index_count = 36;
        g_submeshes[i].tex_id = -1;
        g_submeshes[i].flags = blocks[i].flags;
        g_submeshes[i].aabb_min[0] = blocks[i].cx - blocks[i].hx;
        g_submeshes[i].aabb_min[1] = blocks[i].cy - blocks[i].hy;
        g_submeshes[i].aabb_min[2] = blocks[i].cz - blocks[i].hz;
        g_submeshes[i].aabb_max[0] = blocks[i].cx + blocks[i].hx;
        g_submeshes[i].aabb_max[1] = blocks[i].cy + blocks[i].hy;
        g_submeshes[i].aabb_max[2] = blocks[i].cz + blocks[i].hz;
        append_box_to_vbo(g_level_vbo, &vc, g_level_ibo, &ic,
                          blocks[i].cx, blocks[i].cy, blocks[i].cz,
                          blocks[i].hx, blocks[i].hy, blocks[i].hz,
                          blocks[i].r, blocks[i].g, blocks[i].b, 255);
        add_floor_coll_box(g_coll_tris, &tc,
                           blocks[i].cx, blocks[i].cy + blocks[i].hy, blocks[i].cz,
                           blocks[i].hx, blocks[i].hz,
                           (blocks[i].flags & SUBMESH_HAZARD) ? 2 : 1, (int16_t)i);
    }

    GSPGPU_FlushDataCache(g_level_vbo, g_hdr.num_vertices * sizeof(Bw3dVertex));
    GSPGPU_FlushDataCache(g_level_ibo, g_hdr.num_indices * sizeof(uint16_t));

    /* Dialogues */
    strncpy(g_dialogues[0].speaker, "Gremlin Gus", 31);
    strncpy(g_dialogues[0].text, "Mickey! Aim with the touch screen and press R to Paint the thinned bridge across the Thinner River!", 127);
    strncpy(g_dialogues[1].speaker, "Gremlin Tim", 31);
    strncpy(g_dialogues[1].text, "Thanks for reaching the courtyard platform! Watch out for Spatters, Sweepers, Spladooshes, and Beetleworx!", 127);
    strncpy(g_dialogues[2].speaker, "Petetronic", 31);
    strncpy(g_dialogues[2].text, "END OF LINE, MOUSE! Thin my armor or stun me with your Spin Attack (Y) if you dare!", 127);

    /* Actors */
    /* 0: Gremlin Gus on start island */
    g_actors[0].base.kind = ACTOR_NPC; g_actors[0].base.team = TEAM_NEUTRAL;
    g_actors[0].base.hp = 10; g_actors[0].base.max_hp = 10; g_actors[0].base.dialogue_id = 0;
    g_actors[0].base.pos[0] = -2.2f; g_actors[0].base.pos[1] = 0.0f; g_actors[0].base.pos[2] = 10.5f;
    strncpy(g_actors[0].base.name, "Gremlin Gus", 31);

    /* 1: Platform TriggerVolume on Main Courtyard (only fires when grounded!) */
    g_actors[1].base.kind = ACTOR_TRIGGER_VOL; g_actors[1].base.team = TEAM_NEUTRAL;
    g_actors[1].base.dialogue_id = 1;
    g_actors[1].base.pos[0] = 0.0f; g_actors[1].base.pos[1] = 1.0f; g_actors[1].base.pos[2] = -5.5f;
    g_actors[1].base.half_extents[0] = 3.0f; g_actors[1].base.half_extents[1] = 2.0f; g_actors[1].base.half_extents[2] = 2.0f;
    strncpy(g_actors[1].base.name, "CourtyardArrivalTrigger", 31);

    /* 2: SpatterMelee */
    g_actors[2].base.kind = ACTOR_SPATTER; g_actors[2].base.team = TEAM_HOSTILE;
    g_actors[2].base.hp = 3; g_actors[2].base.max_hp = 3;
    g_actors[2].base.pos[0] = -4.0f; g_actors[2].base.pos[1] = 0.0f; g_actors[2].base.pos[2] = -9.5f;
    g_actors[2].base.patrol_pos[0] = -1.0f; g_actors[2].base.patrol_pos[1] = 0.0f; g_actors[2].base.patrol_pos[2] = -9.5f;
    g_actors[2].base.paint_thresh = 3.0f; g_actors[2].base.thin_thresh = 3.0f;
    strncpy(g_actors[2].base.name, "SpatterMelee 01", 31);

    /* 3: Sweeper (Ranged Spatter) */
    g_actors[3].base.kind = ACTOR_SWEEPER; g_actors[3].base.team = TEAM_HOSTILE;
    g_actors[3].base.hp = 3; g_actors[3].base.max_hp = 3;
    g_actors[3].base.pos[0] = 4.5f; g_actors[3].base.pos[1] = 0.0f; g_actors[3].base.pos[2] = -10.5f;
    g_actors[3].base.patrol_pos[0] = 2.0f; g_actors[3].base.patrol_pos[1] = 0.0f; g_actors[3].base.patrol_pos[2] = -12.5f;
    g_actors[3].base.paint_thresh = 3.0f; g_actors[3].base.thin_thresh = 3.0f;
    strncpy(g_actors[3].base.name, "Sweeper 01", 31);

    /* 4: Spladoosh (Kamikaze) */
    g_actors[4].base.kind = ACTOR_SPLADOOSH; g_actors[4].base.team = TEAM_HOSTILE;
    g_actors[4].base.hp = 3; g_actors[4].base.max_hp = 3;
    g_actors[4].base.pos[0] = -5.5f; g_actors[4].base.pos[1] = 0.0f; g_actors[4].base.pos[2] = -14.0f;
    g_actors[4].base.paint_thresh = 3.0f; g_actors[4].base.thin_thresh = 2.0f;
    strncpy(g_actors[4].base.name, "Spladoosh 01", 31);

    /* 5: Seer (Rolling Charge) */
    g_actors[5].base.kind = ACTOR_SEER; g_actors[5].base.team = TEAM_HOSTILE;
    g_actors[5].base.hp = 4; g_actors[5].base.max_hp = 4;
    g_actors[5].base.pos[0] = 2.5f; g_actors[5].base.pos[1] = 0.0f; g_actors[5].base.pos[2] = -14.5f;
    g_actors[5].base.paint_thresh = 3.0f; g_actors[5].base.thin_thresh = 3.0f;
    strncpy(g_actors[5].base.name, "Seer 01", 31);

    /* 6: Beetleworx Melee (Armor shell -> weak point) */
    g_actors[6].base.kind = ACTOR_BEETLEWORX; g_actors[6].base.team = TEAM_HOSTILE;
    g_actors[6].base.hp = 4; g_actors[6].base.max_hp = 4;
    g_actors[6].base.pos[0] = 0.0f; g_actors[6].base.pos[1] = 0.0f; g_actors[6].base.pos[2] = -13.5f;
    g_actors[6].base.paint_thresh = 4.0f; g_actors[6].base.thin_thresh = 3.0f;
    g_actors[6].armor_intact = 1;
    strncpy(g_actors[6].base.name, "BeetleworxBasher 01", 31);

    /* 7: Boss on elevated platform */
    g_actors[7].base.kind = ACTOR_BOSS; g_actors[7].base.team = TEAM_HOSTILE;
    g_actors[7].base.hp = 12; g_actors[7].base.max_hp = 12; g_actors[7].base.dialogue_id = 2;
    g_actors[7].base.pos[0] = 0.0f; g_actors[7].base.pos[1] = 2.9f; g_actors[7].base.pos[2] = -20.0f;
    g_actors[7].base.paint_thresh = 10.0f; g_actors[7].base.thin_thresh = 10.0f;
    g_actors[7].armor_intact = 1;
    strncpy(g_actors[7].base.name, "TronPete Boss", 31);

    /* 8: Golden E-Ticket Pickup */
    g_actors[8].base.kind = ACTOR_PICKUP; g_actors[8].base.team = TEAM_NEUTRAL;
    g_actors[8].base.hp = 1; g_actors[8].base.max_hp = 1;
    g_actors[8].base.pos[0] = 6.5f; g_actors[8].base.pos[1] = 0.5f; g_actors[8].base.pos[2] = -11.0f;
    strncpy(g_actors[8].base.name, "Gold_ETicket", 31);

    for (uint32_t i = 0; i < g_hdr.num_actors; ++i) {
        memcpy(g_actors[i].spawn_pos, g_actors[i].base.pos, sizeof(float) * 3);
    }
    reset_player_to_spawn();
}

static bool load_bw3d_level(const char* path) {
    FILE* f = fopen(path, "rb");
    if (!f) return false;

    Bw3dHeader hdr;
    if (fread(&hdr, sizeof(Bw3dHeader), 1, f) != 1 || hdr.magic != BW3D_MAGIC || hdr.version != BW3D_VERSION) {
        fclose(f);
        return false;
    }

    /* Strictly free previous level first to honor Old 3DS 128 MB RAM cap */
    unload_current_level();
    g_hdr = hdr;

    if (g_hdr.num_textures > 0) {
        g_textures  = (C3D_Tex*)calloc(g_hdr.num_textures, sizeof(C3D_Tex));
        g_tex_valid = (uint8_t*)calloc(g_hdr.num_textures, 1);
        Bw3dTexture* tmp = (Bw3dTexture*)malloc(sizeof(Bw3dTexture));
        for (uint32_t i = 0; i < g_hdr.num_textures; ++i) {
            if (fread(tmp, sizeof(Bw3dTexture), 1, f) == 1) {
                if (C3D_TexInit(&g_textures[i], 64, 64, GPU_RGBA5551)) {
                    C3D_TexSetFilter(&g_textures[i], GPU_LINEAR, GPU_LINEAR);
                    C3D_TexSetWrap(&g_textures[i], GPU_REPEAT, GPU_REPEAT);
                    memcpy(g_textures[i].data, tmp->pixels, sizeof(tmp->pixels));
                    GSPGPU_FlushDataCache(g_textures[i].data, sizeof(tmp->pixels));
                    g_tex_valid[i] = 1;
                }
            }
        }
        free(tmp);
    }

    if (g_hdr.num_submeshes > 0) {
        g_submeshes = (Bw3dSubmesh*)calloc(g_hdr.num_submeshes, sizeof(Bw3dSubmesh));
        fread(g_submeshes, sizeof(Bw3dSubmesh), g_hdr.num_submeshes, f);
    }
    if (g_hdr.num_vertices > 0) {
        g_level_vbo = (Bw3dVertex*)linearAlloc(g_hdr.num_vertices * sizeof(Bw3dVertex));
        fread(g_level_vbo, sizeof(Bw3dVertex), g_hdr.num_vertices, f);
        GSPGPU_FlushDataCache(g_level_vbo, g_hdr.num_vertices * sizeof(Bw3dVertex));
    }
    if (g_hdr.num_indices > 0) {
        g_level_ibo = (uint16_t*)linearAlloc(g_hdr.num_indices * sizeof(uint16_t));
        fread(g_level_ibo, sizeof(uint16_t), g_hdr.num_indices, f);
        GSPGPU_FlushDataCache(g_level_ibo, g_hdr.num_indices * sizeof(uint16_t));
    }
    if (g_hdr.num_coll_tris > 0) {
        g_coll_tris = (Bw3dCollTri*)calloc(g_hdr.num_coll_tris, sizeof(Bw3dCollTri));
        fread(g_coll_tris, sizeof(Bw3dCollTri), g_hdr.num_coll_tris, f);
    }
    if (g_hdr.num_actors > 0) {
        g_actors = (LiveActor*)calloc(g_hdr.num_actors, sizeof(LiveActor));
        for (uint32_t i = 0; i < g_hdr.num_actors; ++i) {
            fread(&g_actors[i].base, sizeof(Bw3dActor), 1, f);
            memcpy(g_actors[i].spawn_pos, g_actors[i].base.pos, sizeof(float) * 3);
            g_actors[i].armor_intact = (g_actors[i].base.kind == ACTOR_BEETLEWORX || g_actors[i].base.kind == ACTOR_BOSS) ? 1 : 0;
        }
    }
    if (g_hdr.num_dialogues > 0) {
        g_dialogues = (Bw3dDialogue*)calloc(g_hdr.num_dialogues, sizeof(Bw3dDialogue));
        fread(g_dialogues, sizeof(Bw3dDialogue), g_hdr.num_dialogues, f);
    }

    fclose(f);
    reset_player_to_spawn();
    char msg[64];
    snprintf(msg, sizeof(msg), "Loaded %.48s", g_hdr.title);
    set_status(msg, 3.5f);
    return true;
}

static void scan_level_directory(const char* dir_path) {
    DIR* d = opendir(dir_path);
    if (!d) return;
    struct dirent* ent;
    while ((ent = readdir(d)) != NULL && g_num_levels < MAX_LEVEL_PATHS) {
        const char* name = ent->d_name;
        size_t len = strlen(name);
        if (len > 5 && strcasecmp(name + len - 5, ".bw3d") == 0) {
            /* Avoid duplicate basenames if both sdmc and romfs have it */
            bool dup = false;
            for (int i = 0; i < g_num_levels; ++i) {
                if (strncasecmp(g_level_names[i], name, len - 5) == 0 && g_level_names[i][len - 5] == '\0') {
                    dup = true;
                    break;
                }
            }
            if (dup) continue;
            snprintf(g_level_paths[g_num_levels], sizeof(g_level_paths[0]), "%s/%s", dir_path, name);
            size_t copy_len = len - 5 < sizeof(g_level_names[0]) - 1 ? len - 5 : sizeof(g_level_names[0]) - 1;
            memcpy(g_level_names[g_num_levels], name, copy_len);
            g_level_names[g_num_levels][copy_len] = '\0';
            g_num_levels++;
        }
    }
    closedir(d);
}

static void discover_levels(void) {
    g_num_levels = 0;
    scan_level_directory("sdmc:/blotworx/levels");
    scan_level_directory("sdmc:/3ds/blotworx/levels");
    scan_level_directory("romfs:/levels");
}

static bool point_in_tri_xz(float px, float pz, const float v0[3], const float v1[3], const float v2[3]) {
    float dX = px - v2[0], dZ = pz - v2[2];
    float dX21 = v2[0] - v1[0], dZ12 = v1[2] - v2[2];
    float D = dZ12 * (v0[0] - v2[0]) + dX21 * (v0[2] - v2[2]);
    if (fabsf(D) < 1e-5f) return false;
    float s = dZ12 * dX + dX21 * dZ;
    float t = (v2[2] - v0[2]) * dX + (v0[0] - v2[0]) * dZ;
    if (D < 0.0f) return s <= 0.0f && t <= 0.0f && s + t >= D;
    return s >= 0.0f && t >= 0.0f && s + t <= D;
}

static bool query_ground_at(float x, float y, float z, float* out_y, uint16_t* out_flags, float out_normal[3]) {
    bool found = false;
    float best_y = -1e9f;
    uint16_t best_flags = 0;
    float best_n[3] = {0.0f, 1.0f, 0.0f};

    for (uint32_t i = 0; i < g_hdr.num_coll_tris; ++i) {
        const Bw3dCollTri* t = &g_coll_tris[i];
        if (t->submesh_id >= 0 && (uint32_t)t->submesh_id < g_hdr.num_submeshes) {
            if ((g_submeshes[t->submesh_id].flags & SUBMESH_PAINTABLE) &&
                !(g_submeshes[t->submesh_id].flags & SUBMESH_PAINTED)) {
                continue; /* Thinned toon platform has no collision! */
            }
        }
        if (t->normal[1] < 0.25f) continue;
        if (!point_in_tri_xz(x, z, t->v0, t->v1, t->v2)) continue;
        /* Interpolate plane Y at (x, z) */
        float ny = t->normal[1];
        float py = t->v0[1] - ((x - t->v0[0]) * t->normal[0] + (z - t->v0[2]) * t->normal[2]) / ny;
        if (py <= y + 0.85f && py > best_y) {
            best_y = py;
            best_flags = t->flags;
            best_n[0] = t->normal[0]; best_n[1] = t->normal[1]; best_n[2] = t->normal[2];
            found = true;
        }
    }
    if (found) {
        *out_y = best_y;
        *out_flags = best_flags;
        if (out_normal) {
            out_normal[0] = best_n[0]; out_normal[1] = best_n[1]; out_normal[2] = best_n[2];
        }
    }
    return found;
}

static void apply_ink_in_radius(const float center[3], float radius, bool is_paint, float amount) {
    float r2 = radius * radius;
    /* 1. Check paintable/thinnable submeshes */
    for (uint32_t i = 0; i < g_hdr.num_submeshes; ++i) {
        Bw3dSubmesh* sm = &g_submeshes[i];
        if (!(sm->flags & SUBMESH_PAINTABLE)) continue;
        float cx = clampf(center[0], sm->aabb_min[0], sm->aabb_max[0]);
        float cy = clampf(center[1], sm->aabb_min[1], sm->aabb_max[1]);
        float cz = clampf(center[2], sm->aabb_min[2], sm->aabb_max[2]);
        float p[3] = {cx, cy, cz};
        if (dist3_sq(center, p) <= r2) {
            bool was_painted = (sm->flags & SUBMESH_PAINTED) != 0;
            if (is_paint && !was_painted) {
                sm->flags |= SUBMESH_PAINTED;
                set_status("Painted Toon Geometry!", 1.8f);
            } else if (!is_paint && was_painted) {
                sm->flags &= ~SUBMESH_PAINTED;
                set_status("Thinned Toon Geometry!", 1.8f);
            }
        }
    }

    /* 2. Check actors & enemies */
    for (uint32_t i = 0; i < g_hdr.num_actors; ++i) {
        LiveActor* a = &g_actors[i];
        if (a->ai_state == 5) continue;
        if (a->base.kind < ACTOR_SPATTER || a->base.kind > ACTOR_BOSS) continue;
        float ap[3] = {a->base.pos[0], a->base.pos[1] + 0.7f, a->base.pos[2]};
        if (dist3_sq(center, ap) <= (radius + 0.9f) * (radius + 0.9f)) {
            if (is_paint) {
                a->base.paint_val += amount;
                if (a->base.kind == ACTOR_BEETLEWORX) {
                    /* Paint repairs Beetleworx shell */
                    if (a->base.paint_val >= a->base.paint_thresh) {
                        a->armor_intact = 1;
                        a->base.paint_val = a->base.paint_thresh;
                    }
                } else if (a->base.kind == ACTOR_BOSS) {
                    if (a->base.paint_val >= a->base.paint_thresh) {
                        a->base.team = TEAM_FRIENDLY;
                        a->ai_state = 0;
                        set_status("Boss Redeemed with Paint!", 3.5f);
                    }
                } else if (a->base.paint_thresh > 0.0f && a->base.paint_val >= a->base.paint_thresh) {
                    a->base.team = TEAM_FRIENDLY;
                    a->base.paint_val = a->base.paint_thresh;
                    set_status("Befriended Enemy with Paint!", 2.5f);
                }
            } else {
                a->base.paint_val -= amount;
                if (a->base.kind == ACTOR_BEETLEWORX) {
                    if (a->base.paint_val <= -a->base.thin_thresh) {
                        if (a->armor_intact) {
                            a->armor_intact = 0;
                            a->stun_timer = 4.0f;
                            set_status("Beetleworx Armor Thinned! Spin Attack (Y)!", 2.8f);
                        }
                    }
                } else if (a->base.kind == ACTOR_BOSS) {
                    if (a->base.paint_val <= -2.5f) {
                        a->armor_intact = 0;
                        a->base.paint_val = 0.0f;
                        a->base.hp -= 1;
                        set_status("Boss Armor Thinned!", 2.0f);
                        if (a->base.hp <= 0) {
                            a->ai_state = 5;
                            g_player.etickets += 25;
                            set_status("Boss Defeated with Thinner!", 4.0f);
                        }
                    }
                } else if (a->base.thin_thresh > 0.0f && a->base.paint_val <= -a->base.thin_thresh) {
                    a->ai_state = 5;
                    g_player.etickets += 5;
                    set_status("Dissolved Enemy with Thinner!", 2.2f);
                }
            }
        }
    }
}

static void damage_player(int dmg, const float knock_from[3], bool is_thinner_leap) {
    if (g_player.input_restricted || g_player.active_dialogue >= 0) return; /* Cutscene invulnerability (REL 0x324b98) */
    if (g_player.iframe_timer <= 0.0f) {
        g_player.hp -= dmg;
        g_player.iframe_timer = 1.4f;
        if (g_player.hp <= 0) {
            reset_player_to_spawn();
            set_status("Mickey Respawned at Checkpoint!", 2.5f);
            return;
        }
    }
    if (is_thinner_leap) {
        g_player.vel[1] = 8.8f;
        g_player.grounded = 0;
        set_status("Thinner Damage Leap!", 1.2f);
    } else if (knock_from) {
        float dx = g_player.pos[0] - knock_from[0];
        float dz = g_player.pos[2] - knock_from[2];
        float len = sqrtf(dx * dx + dz * dz);
        if (len > 0.01f) {
            g_player.vel[0] = (dx / len) * 6.0f;
            g_player.vel[2] = (dz / len) * 6.0f;
        }
        g_player.vel[1] = 5.5f;
        g_player.grounded = 0;
    }
}

static void update_simulation(float dt, u32 kDown, u32 kHeld, const circlePosition* cpad, const circlePosition* cstick, const touchPosition* touch, bool touching) {
    g_time_sec += dt;
    if (g_status_timer > 0.0f) g_status_timer -= dt;
    if (g_player.iframe_timer > 0.0f) g_player.iframe_timer -= dt;
    if (g_player.spin_timer > 0.0f) g_player.spin_timer -= dt;
    if (g_player.blast_timer > 0.0f) g_player.blast_timer -= dt;

    /* Regenerate minimal Paint/Thinner pool passively to match retail restock floor */
    if (g_player.paint < 25.0f) g_player.paint = clampf(g_player.paint + 4.0f * dt, 0.0f, g_player.max_paint);
    if (g_player.thinner < 25.0f) g_player.thinner = clampf(g_player.thinner + 4.0f * dt, 0.0f, g_player.max_thinner);

    /* Level selection menu on bottom screen */
    if (kDown & KEY_SELECT) {
        g_show_level_menu = !g_show_level_menu;
    }
    if (g_show_level_menu) {
        if ((kDown & KEY_DUP) && g_menu_cursor > 0) g_menu_cursor--;
        if ((kDown & KEY_DDOWN) && g_menu_cursor + 1 < (g_num_levels > 0 ? g_num_levels : 1)) g_menu_cursor++;
        if (kDown & KEY_A) {
            g_show_level_menu = 0;
            if (g_num_levels > 0 && g_menu_cursor < g_num_levels) {
                g_cur_level = g_menu_cursor;
                if (!load_bw3d_level(g_level_paths[g_cur_level])) {
                    build_builtin_fallback_level();
                }
            } else {
                build_builtin_fallback_level();
            }
        }
        return;
    }

    /* Dialogue box handling */
    if (g_player.active_dialogue >= 0) {
        g_player.input_restricted = 1;
        if (kDown & (KEY_A | KEY_B)) {
            g_player.active_dialogue = -1;
            g_player.input_restricted = 0;
        }
        return;
    }
    g_player.input_restricted = 0;

    /* Touch screen aiming (320x240 bottom screen -> 3D reticle aim offset & bottom toolbar) */
    if (touching && touch) {
        if (touch->py >= 205) {
            /* Bottom touch toolbar buttons */
            if (touch->px < 80 && g_player.blast_timer <= 0.0f && g_player.paint >= 15.0f) {
                g_player.paint -= 15.0f;
                g_player.blast_timer = 0.45f;
                g_player.blast_is_paint = 1;
                apply_ink_in_radius(g_player.pos, 4.5f, true, 3.0f);
                set_status("Paint Blast Cone!", 1.5f);
            } else if (touch->px >= 80 && touch->px < 160 && g_player.blast_timer <= 0.0f && g_player.thinner >= 15.0f) {
                g_player.thinner -= 15.0f;
                g_player.blast_timer = 0.45f;
                g_player.blast_is_paint = 0;
                apply_ink_in_radius(g_player.pos, 4.5f, false, 3.0f);
                set_status("Thinner Blast Cone!", 1.5f);
            } else if (touch->px >= 160 && touch->px < 240) {
                reset_player_to_spawn();
                set_status("Respawned at Checkpoint", 1.5f);
            } else if (touch->px >= 240) {
                g_show_level_menu = 1;
            }
        } else {
            float nx = ((float)touch->px / 160.0f) - 1.0f;
            float ny = 1.0f - ((float)touch->py / 102.0f);
            g_player.aim_yaw_off   = clampf(-nx * 0.85f, -0.95f, 0.95f);
            g_player.aim_pitch_off = clampf(ny * 0.55f, -0.55f, 0.65f);
        }
    } else {
        g_player.aim_yaw_off   *= 0.92f;
        g_player.aim_pitch_off *= 0.92f;
    }

    /* Camera orbit via C-Stick or D-Pad Left/Right */
    if (abs(cstick->dx) > 15) g_player.cam_yaw -= ((float)cstick->dx / 150.0f) * 2.4f * dt;
    if (abs(cstick->dy) > 15) g_player.cam_pitch = clampf(g_player.cam_pitch - ((float)cstick->dy / 150.0f) * 1.6f * dt, -0.15f, 0.95f);
    if (kHeld & KEY_DLEFT)  g_player.cam_yaw += 2.0f * dt;
    if (kHeld & KEY_DRIGHT) g_player.cam_yaw -= 2.0f * dt;
    if (kHeld & KEY_DUP)    g_player.cam_pitch = clampf(g_player.cam_pitch + 1.4f * dt, -0.15f, 0.95f);
    if (kHeld & KEY_DDOWN)  g_player.cam_pitch = clampf(g_player.cam_pitch - 1.4f * dt, -0.15f, 0.95f);

    /* Ledge Grab State (State 8) */
    if (g_player.ledge_grabbed) {
        g_player.vel[0] = g_player.vel[1] = g_player.vel[2] = 0.0f;
        if ((kDown & KEY_B) || cpad->dy > 60) {
            g_player.ledge_grabbed = 0;
            g_player.pos[0] = g_player.ledge_pos[0];
            g_player.pos[1] = g_player.ledge_pos[1] + 0.1f;
            g_player.pos[2] = g_player.ledge_pos[2];
            g_player.vel[1] = 4.5f;
            g_player.grounded = 1;
            g_player.jump_count = 0;
            set_status("Ledge Climb Up!", 1.2f);
        } else if (cpad->dy < -60) {
            g_player.ledge_grabbed = 0;
        }
    } else {
        /* Circle Pad movement relative to camera yaw */
        float sx = (abs(cpad->dx) > 18) ? ((float)cpad->dx / 154.0f) : 0.0f;
        float sy = (abs(cpad->dy) > 18) ? ((float)cpad->dy / 154.0f) : 0.0f;
        float mag = sqrtf(sx * sx + sy * sy);
        if (mag > 1.0f) { sx /= mag; sy /= mag; mag = 1.0f; }

        float fwd_x = -sinf(g_player.cam_yaw);
        float fwd_z = -cosf(g_player.cam_yaw);
        float right_x = cosf(g_player.cam_yaw);
        float right_z = -sinf(g_player.cam_yaw);

        float move_x = fwd_x * sy + right_x * sx;
        float move_z = fwd_z * sy + right_z * sx;
        float speed = 6.8f;

        if (mag > 0.05f) {
            g_player.vel[0] = move_x * speed;
            g_player.vel[2] = move_z * speed;
            g_player.yaw = atan2f(-move_x, -move_z);
            float cam_diff = g_player.yaw - g_player.cam_yaw;
            while (cam_diff > 3.14159265f) cam_diff -= 6.2831853f;
            while (cam_diff < -3.14159265f) cam_diff += 6.2831853f;
            g_player.cam_yaw += cam_diff * 1.4f * dt;
        } else if (g_player.grounded) {
            g_player.vel[0] *= 0.75f;
            g_player.vel[2] *= 0.75f;
        } else {
            g_player.vel[0] *= 0.96f;
            g_player.vel[2] *= 0.96f;
        }

        /* Jump / Double Jump (B) */
        if (kDown & KEY_B) {
            if (g_player.grounded) {
                g_player.vel[1] = 8.2f;
                g_player.grounded = 0;
                g_player.jump_count = 1;
            } else if (g_player.jump_count < 2) {
                g_player.vel[1] = 7.6f;
                g_player.jump_count = 2;
            }
        }

        /* Mid-Air Ink Jump + Grounded/Air Blast Cone (X) */
        if ((kDown & KEY_X) && g_player.blast_timer <= 0.0f) {
            bool use_paint = (g_player.paint >= g_player.thinner);
            if (!g_player.grounded && g_player.air_ink_avail) {
                g_player.air_ink_avail = 0;
                g_player.vel[1] = 7.2f;
                set_status(use_paint ? "Mid-Air Paint Jump!" : "Mid-Air Thinner Jump!", 1.4f);
            } else {
                set_status(use_paint ? "Paint Blast Cone!" : "Thinner Blast Cone!", 1.4f);
            }
            g_player.blast_timer = 0.45f;
            g_player.blast_is_paint = use_paint ? 1 : 0;
            if (use_paint && g_player.paint >= 10.0f) g_player.paint -= 10.0f;
            else if (!use_paint && g_player.thinner >= 10.0f) g_player.thinner -= 10.0f;
            apply_ink_in_radius(g_player.pos, 4.5f, use_paint, 3.0f);
        }

        /* Gravity & integration */
        g_player.vel[1] -= 21.0f * dt;
        g_player.pos[0] += g_player.vel[0] * dt;
        g_player.pos[1] += g_player.vel[1] * dt;
        g_player.pos[2] += g_player.vel[2] * dt;

        /* Ground support, steep-slope sliding, thinner hazards & ledge grab detection */
        float ground_y = -100.0f;
        uint16_t ground_flags = 0;
        float ground_n[3] = {0.0f, 1.0f, 0.0f};
        if (query_ground_at(g_player.pos[0], g_player.pos[1], g_player.pos[2], &ground_y, &ground_flags, ground_n)) {
            if (g_player.pos[1] <= ground_y + 0.08f && g_player.vel[1] <= 0.0f) {
                g_player.pos[1] = ground_y;
                g_player.vel[1] = 0.0f;
                g_player.grounded = 1;
                g_player.jump_count = 0;
                g_player.air_ink_avail = 1;
                if (ground_n[1] < 0.72f) {
                    /* Steep-slope sliding (State 3) */
                    g_player.sliding = 1;
                    g_player.pos[0] += ground_n[0] * 5.0f * dt;
                    g_player.pos[2] += ground_n[2] * 5.0f * dt;
                } else {
                    g_player.sliding = 0;
                }
                if (ground_flags & 2) {
                    /* Thinner river hazard -> DamageLeap! */
                    damage_player(1, NULL, true);
                }
            } else {
                g_player.grounded = 0;
            }
        } else {
            g_player.grounded = 0;
            /* Check forward ledge lip when falling near a platform edge */
            if (g_player.vel[1] < 0.0f && mag > 0.2f) {
                float probe_x = g_player.pos[0] - sinf(g_player.yaw) * 0.75f;
                float probe_z = g_player.pos[2] - cosf(g_player.yaw) * 0.75f;
                float ledge_y = -100.0f;
                uint16_t lflags = 0;
                if (query_ground_at(probe_x, g_player.pos[1] + 1.35f, probe_z, &ledge_y, &lflags, NULL)) {
                    float dy = ledge_y - g_player.pos[1];
                    if (!(lflags & 2) && dy >= 0.45f && dy <= 1.45f) {
                        g_player.ledge_grabbed = 1;
                        g_player.ledge_pos[0] = probe_x;
                        g_player.ledge_pos[1] = ledge_y;
                        g_player.ledge_pos[2] = probe_z;
                        set_status("Ledge Grab! Press B or Up to Climb", 1.5f);
                    }
                }
            }
        }

        /* Death fall rescue */
        if (g_player.pos[1] < -18.0f) {
            g_player.pos[0] = g_player.spawn_pos[0];
            g_player.pos[1] = g_player.spawn_pos[1] + 0.5f;
            g_player.pos[2] = g_player.spawn_pos[2];
            g_player.vel[0] = g_player.vel[1] = g_player.vel[2] = 0.0f;
            damage_player(1, NULL, false);
        }
    }

    /* Compute 3D World Aim Point from camera + touch offset */
    float aim_yaw   = g_player.cam_yaw + g_player.aim_yaw_off;
    float aim_pitch = -0.10f + g_player.aim_pitch_off;
    float dir_x = -sinf(aim_yaw) * cosf(aim_pitch);
    float dir_y = sinf(aim_pitch);
    float dir_z = -cosf(aim_yaw) * cosf(aim_pitch);
    g_player.aim_world[0] = g_player.pos[0] + dir_x * 9.0f;
    g_player.aim_world[1] = g_player.pos[1] + 1.0f + dir_y * 9.0f;
    g_player.aim_world[2] = g_player.pos[2] + dir_z * 9.0f;

    /* Spin Attack (Y) */
    if ((kDown & KEY_Y) && g_player.spin_timer <= 0.0f) {
        g_player.spin_timer = 0.38f;
        for (uint32_t i = 0; i < g_hdr.num_actors; ++i) {
            LiveActor* a = &g_actors[i];
            if (a->ai_state == 5) continue;
            if (dist3_sq(g_player.pos, a->base.pos) <= 2.6f * 2.6f) {
                if (a->base.kind == ACTOR_BEETLEWORX && a->armor_intact) {
                    set_status("Armor Deflected Spin! Thin Shell First (L)!", 2.2f);
                } else if (a->base.kind >= ACTOR_SPATTER && a->base.kind <= ACTOR_BOSS && a->base.team == TEAM_HOSTILE) {
                    a->base.hp -= 2;
                    a->stun_timer = 2.0f;
                    if (a->base.hp <= 0) {
                        a->ai_state = 5;
                        g_player.etickets += (a->base.kind == ACTOR_BOSS) ? 25 : 5;
                        set_status("Defeated Enemy with Spin Attack!", 2.2f);
                    } else {
                        set_status("Spin Attack Hit + Stun!", 1.5f);
                    }
                }
            }
        }
    }

    /* Paint Stream (R) & Thinner Stream (L) */
    bool spray_paint   = (kHeld & (KEY_R | KEY_ZR)) != 0;
    bool spray_thinner = (kHeld & (KEY_L | KEY_ZL)) != 0;
    if (spray_paint && g_player.paint > 0.5f) {
        g_player.paint = clampf(g_player.paint - 12.0f * dt, 0.0f, g_player.max_paint);
        float muzzle[3] = {g_player.pos[0], g_player.pos[1] + 1.05f, g_player.pos[2]};
        float pvel[3]   = {dir_x * 18.0f, dir_y * 18.0f + 1.5f, dir_z * 18.0f};
        spawn_projectile(muzzle, pvel, 1, 1, 1.35f, 0.85f);
        spawn_particle(muzzle, pvel, 55, 165, 255, 0.22f, 0.45f);
    } else if (spray_thinner && g_player.thinner > 0.5f) {
        g_player.thinner = clampf(g_player.thinner - 12.0f * dt, 0.0f, g_player.max_thinner);
        float muzzle[3] = {g_player.pos[0], g_player.pos[1] + 1.05f, g_player.pos[2]};
        float pvel[3]   = {dir_x * 18.0f, dir_y * 18.0f + 1.5f, dir_z * 18.0f};
        spawn_projectile(muzzle, pvel, 1, 0, 1.35f, 0.85f);
        spawn_particle(muzzle, pvel, 55, 245, 85, 0.22f, 0.45f);
    }

    /* Update Projectiles */
    for (int i = 0; i < MAX_PROJECTILES; ++i) {
        Projectile* pr = &g_projectiles[i];
        if (!pr->active) continue;
        pr->ttl -= dt;
        if (pr->ttl <= 0.0f) { pr->active = 0; continue; }
        if (!pr->from_player) pr->vel[1] -= 9.8f * dt; /* Sweeper gravity-arc delivery (Mode 4) */
        pr->pos[0] += pr->vel[0] * dt;
        pr->pos[1] += pr->vel[1] * dt;
        pr->pos[2] += pr->vel[2] * dt;

        if (pr->from_player) {
            apply_ink_in_radius(pr->pos, pr->radius, pr->is_paint != 0, 18.0f * dt);
        } else {
            if (dist3_sq(pr->pos, g_player.pos) <= 1.2f * 1.2f) {
                damage_player(1, pr->pos, false);
                pr->active = 0;
            }
        }
    }

    /* Update Particles */
    for (int i = 0; i < MAX_PARTICLES; ++i) {
        Particle* pt = &g_particles[i];
        if (!pt->active) continue;
        pt->ttl -= dt;
        if (pt->ttl <= 0.0f) { pt->active = 0; continue; }
        pt->pos[0] += pt->vel[0] * dt;
        pt->pos[1] += pt->vel[1] * dt;
        pt->pos[2] += pt->vel[2] * dt;
    }

    /* Update Actors, Triggers, NPCs, Enemies & Bosses */
    for (uint32_t i = 0; i < g_hdr.num_actors; ++i) {
        LiveActor* a = &g_actors[i];
        if (a->ai_state == 5) continue;
        if (a->stun_timer > 0.0f) a->stun_timer -= dt;
        if (a->cooldown > 0.0f)   a->cooldown -= dt;

        float dx = g_player.pos[0] - a->base.pos[0];
        float dy = g_player.pos[1] - a->base.pos[1];
        float dz = g_player.pos[2] - a->base.pos[2];
        float d2 = dx * dx + dy * dy + dz * dz;

        /* Grounded-Only Platform TriggerVolume Activation (never cuts off mid-air jumps!) */
        if (a->base.kind == ACTOR_TRIGGER_VOL) {
            if (!a->triggered && g_player.grounded) {
                float hx = a->base.half_extents[0] > 0.1f ? a->base.half_extents[0] : 2.5f;
                float hy = a->base.half_extents[1] > 0.1f ? a->base.half_extents[1] : 2.0f;
                float hz = a->base.half_extents[2] > 0.1f ? a->base.half_extents[2] : 2.5f;
                if (fabsf(dx) <= hx && fabsf(dy) <= hy && fabsf(dz) <= hz) {
                    a->triggered = 1;
                    g_player.spawn_pos[0] = g_player.pos[0];
                    g_player.spawn_pos[1] = g_player.pos[1];
                    g_player.spawn_pos[2] = g_player.pos[2];
                    if (a->base.dialogue_id >= 0 && (uint32_t)a->base.dialogue_id < g_hdr.num_dialogues) {
                        g_player.active_dialogue = a->base.dialogue_id;
                    } else {
                        set_status("Checkpoint Triggered on Ground!", 2.0f);
                    }
                }
            }
            continue;
        }

        if (a->base.kind == ACTOR_TRIGGER_DMG) {
            float hx = a->base.half_extents[0] > 0.1f ? a->base.half_extents[0] : 2.0f;
            float hy = a->base.half_extents[1] > 0.1f ? a->base.half_extents[1] : 1.5f;
            float hz = a->base.half_extents[2] > 0.1f ? a->base.half_extents[2] : 2.0f;
            if (fabsf(dx) <= hx && fabsf(dy) <= hy && fabsf(dz) <= hz) {
                damage_player(1, a->base.pos, true);
            }
            continue;
        }

        if (a->base.kind == ACTOR_PICKUP) {
            a->base.yaw += 2.5f * dt;
            if (d2 <= 1.5f * 1.5f) {
                a->ai_state = 5;
                g_player.etickets += 10;
                g_player.hp = g_player.max_hp;
                g_player.paint = g_player.max_paint;
                g_player.thinner = g_player.max_thinner;
                set_status("Collected Golden E-Ticket & Full Ink!", 2.5f);
            }
            continue;
        }

        /* NPC & Enemy Head Bounce + Stomp-Stun (Spatter_StompStunDuration = 2.0s) */
        if (!g_player.grounded && g_player.vel[1] < 0.0f &&
            (dx * dx + dz * dz) <= 1.15f * 1.15f && dy >= 0.65f && dy <= 1.85f) {
            bool stunned = a->stun_timer > 0.0f;
            bool friendly = (a->base.team != TEAM_HOSTILE);
            float mult = stunned ? 1.65f : (friendly ? 1.35f : 1.05f);
            g_player.vel[1] = 8.0f * mult;
            g_player.jump_count = 1;
            if (a->base.team == TEAM_HOSTILE) {
                a->stun_timer = 2.0f; /* Spatter_StompStunDuration */
                set_status("Head Bounce + Stomp Stun (2.0s)!", 1.5f);
            } else {
                set_status("Friendly Head Bounce!", 1.2f);
            }
        }

        if (a->base.kind == ACTOR_NPC) {
            /* Turn to face Mickey and allow A-button conversation */
            if (d2 <= 6.0f * 6.0f) {
                a->base.yaw = atan2f(-dx, -dz);
                if ((kDown & KEY_A) && d2 <= 3.0f * 3.0f && a->base.dialogue_id >= 0 && (uint32_t)a->base.dialogue_id < g_hdr.num_dialogues) {
                    g_player.active_dialogue = a->base.dialogue_id;
                }
            }
            continue;
        }

        if (a->stun_timer > 0.0f) continue;

        /* Friendly Converted Ally AI (Team 3): attacks nearby hostiles or follows Mickey */
        if (a->base.team == TEAM_FRIENDLY) {
            LiveActor* target = NULL;
            float best_d2 = 12.0f * 12.0f;
            for (uint32_t j = 0; j < g_hdr.num_actors; ++j) {
                if (i == j || g_actors[j].ai_state == 5) continue;
                if (g_actors[j].base.team == TEAM_HOSTILE && g_actors[j].base.kind >= ACTOR_SPATTER && g_actors[j].base.kind <= ACTOR_BOSS) {
                    float hd2 = dist3_sq(a->base.pos, g_actors[j].base.pos);
                    if (hd2 < best_d2) { best_d2 = hd2; target = &g_actors[j]; }
                }
            }
            if (target) {
                float tx = target->base.pos[0] - a->base.pos[0];
                float tz = target->base.pos[2] - a->base.pos[2];
                float tl = sqrtf(tx * tx + tz * tz);
                if (tl > 1.2f) {
                    a->base.pos[0] += (tx / tl) * 3.8f * dt;
                    a->base.pos[2] += (tz / tl) * 3.8f * dt;
                    a->base.yaw = atan2f(-tx, -tz);
                } else if (a->cooldown <= 0.0f) {
                    target->base.hp -= 1;
                    a->cooldown = 1.2f;
                    if (target->base.hp <= 0) target->ai_state = 5;
                }
            } else if (d2 > 3.2f * 3.2f && d2 < 16.0f * 16.0f) {
                float tl = sqrtf(dx * dx + dz * dz);
                if (tl > 0.1f) {
                    a->base.pos[0] += (dx / tl) * 3.5f * dt;
                    a->base.pos[2] += (dz / tl) * 3.5f * dt;
                    a->base.yaw = atan2f(-dx, -dz);
                }
            }
            continue;
        }

        /* Hostile Enemy & Boss Families (Team 2) */
        float dist_xz = sqrtf(dx * dx + dz * dz);
        float aggro_range = (a->base.kind == ACTOR_BOSS) ? 18.0f : 11.5f;

        if (dist_xz > aggro_range) {
            /* Patrol between spawn_pos and patrol_pos */
            const float* goal = a->patrol_dir ? a->spawn_pos : a->base.patrol_pos;
            if (fabsf(goal[0]) > 0.01f || fabsf(goal[2]) > 0.01f) {
                float px = goal[0] - a->base.pos[0];
                float pz = goal[2] - a->base.pos[2];
                float pl = sqrtf(px * px + pz * pz);
                if (pl < 0.4f) {
                    a->patrol_dir = !a->patrol_dir;
                } else {
                    a->base.pos[0] += (px / pl) * 2.0f * dt;
                    a->base.pos[2] += (pz / pl) * 2.0f * dt;
                    a->base.yaw = atan2f(-px, -pz);
                }
            }
            continue;
        }

        a->base.yaw = atan2f(-dx, -dz);

        if (a->base.kind == ACTOR_SPLADOOSH) {
            /* Spladoosh self-destruct telegraph + thinner CSG explosion */
            if (a->ai_state == 2) {
                a->state_timer -= dt;
                if (a->state_timer <= 0.0f) {
                    /* Detonate: thin nearby paintable geometry & damage Mickey if within blast radius */
                    apply_ink_in_radius(a->base.pos, 3.8f, false, 10.0f);
                    if (dist_xz <= 3.6f) damage_player(2, a->base.pos, false);
                    a->ai_state = 5;
                    set_status("Spladoosh Detonated!", 2.0f);
                }
            } else if (dist_xz <= 2.6f) {
                a->ai_state = 2;
                a->state_timer = 1.6f;
                set_status("Spladoosh Telegraphing Explosion! Flee!", 1.6f);
            } else if (dist_xz > 0.1f) {
                a->base.pos[0] += (dx / dist_xz) * 3.4f * dt;
                a->base.pos[2] += (dz / dist_xz) * 3.4f * dt;
            }
        } else if (a->base.kind == ACTOR_SWEEPER) {
            /* Sweeper ranged gravity-arc projectile attack */
            if (dist_xz > 6.5f) {
                a->base.pos[0] += (dx / dist_xz) * 2.8f * dt;
                a->base.pos[2] += (dz / dist_xz) * 2.8f * dt;
            } else if (a->cooldown <= 0.0f) {
                float muzzle[3] = {a->base.pos[0], a->base.pos[1] + 1.1f, a->base.pos[2]};
                float t_flight = clampf(dist_xz / 8.5f, 0.35f, 1.4f);
                float pvel[3] = {
                    dx / t_flight,
                    (dy / t_flight) + 0.5f * 9.8f * t_flight,
                    dz / t_flight
                };
                spawn_projectile(muzzle, pvel, 0, 0, 0.6f, 2.2f);
                a->cooldown = 2.4f;
            }
        } else if (a->base.kind == ACTOR_SEER) {
            /* Seer windup -> high-speed rolling charge -> recovery */
            if (a->ai_state == 3) {
                a->state_timer -= dt;
                a->base.pos[0] += a->vel[0] * dt;
                a->base.pos[2] += a->vel[2] * dt;
                if (dist_xz <= 1.3f) {
                    damage_player(1, a->base.pos, false);
                    a->state_timer = 0.0f;
                }
                if (a->state_timer <= 0.0f) {
                    a->ai_state = 0;
                    a->cooldown = 2.0f;
                }
            } else if (a->cooldown <= 0.0f && dist_xz <= 8.0f && dist_xz > 0.1f) {
                a->ai_state = 3;
                a->state_timer = 1.1f;
                a->vel[0] = (dx / dist_xz) * 9.0f;
                a->vel[2] = (dz / dist_xz) * 9.0f;
            } else if (dist_xz > 1.5f) {
                a->base.pos[0] += (dx / dist_xz) * 2.6f * dt;
                a->base.pos[2] += (dz / dist_xz) * 2.6f * dt;
            }
        } else {
            /* SpatterMelee, Beetleworx, and Bosses (Petetronic / Captain Hook / Shadow Blot) */
            float chase_spd = (a->base.kind == ACTOR_BOSS) ? 4.2f : 3.4f;
            if (dist_xz > 1.45f) {
                a->base.pos[0] += (dx / dist_xz) * chase_spd * dt;
                a->base.pos[2] += (dz / dist_xz) * chase_spd * dt;
                if (a->base.kind == ACTOR_BOSS && a->cooldown <= 0.0f && dist_xz > 4.5f) {
                    float muzzle[3] = {a->base.pos[0], a->base.pos[1] + 1.5f, a->base.pos[2]};
                    float pvel[3] = {(dx / dist_xz) * 10.0f, 3.2f, (dz / dist_xz) * 10.0f};
                    spawn_projectile(muzzle, pvel, 0, 0, 0.8f, 2.0f);
                    a->cooldown = 2.2f;
                }
            } else if (a->cooldown <= 0.0f) {
                damage_player((a->base.kind == ACTOR_BOSS) ? 2 : 1, a->base.pos, false);
                a->cooldown = 1.4f;
            }
        }
    }
}

static void render_scene_for_eye(float iod) {
    C3D_Mtx proj, modelView;
    Mtx_PerspStereoTilt(&proj, C3D_AngleFromDegrees(55.0f), C3D_AspectRatioTop, 0.25f, 260.0f, iod, 3.0f, false);

    float cam_dist = 5.8f;
    float eye_x = g_player.pos[0] + sinf(g_player.cam_yaw) * cosf(g_player.cam_pitch) * cam_dist;
    float eye_y = g_player.pos[1] + 1.75f + sinf(g_player.cam_pitch) * cam_dist;
    float eye_z = g_player.pos[2] + cosf(g_player.cam_yaw) * cosf(g_player.cam_pitch) * cam_dist;

    float look_x = g_player.pos[0] - sinf(g_player.cam_yaw) * 3.5f;
    float look_y = g_player.pos[1] + 1.10f;
    float look_z = g_player.pos[2] - cosf(g_player.cam_yaw) * 3.5f;

    C3D_FVec camPos = FVec3_New(eye_x, eye_y, eye_z);
    C3D_FVec camTgt = FVec3_New(look_x, look_y, look_z);
    C3D_FVec camUp  = FVec3_New(0.0f, 1.0f, 0.0f);
    Mtx_LookAt(&modelView, camPos, camTgt, camUp, false);

    C3D_FVUnifMtx4x4(GPU_VERTEX_SHADER, g_uLoc_proj, &proj);
    C3D_FVUnifMtx4x4(GPU_VERTEX_SHADER, g_uLoc_modelView, &modelView);
    C3D_FVUnifSet(GPU_VERTEX_SHADER, g_uLoc_lightDir, 0.35f, 0.82f, 0.45f, 1.0f);

    /* Configure TEV stage 0: Texture0 * VertexFragmentColor */
    C3D_TexEnv* env = C3D_GetTexEnv(0);
    C3D_TexEnvInit(env);
    C3D_TexEnvSrc(env, C3D_Both, GPU_TEXTURE0, GPU_PRIMARY_COLOR, 0);
    C3D_TexEnvFunc(env, C3D_Both, GPU_MODULATE);

    /* 1. Draw Level Submeshes from g_level_vbo / g_level_ibo */
    if (g_level_vbo && g_level_ibo && g_hdr.num_submeshes > 0) {
        C3D_BufInfo* bufInfo = C3D_GetBufInfo();
        BufInfo_Init(bufInfo);
        BufInfo_Add(bufInfo, g_level_vbo, sizeof(Bw3dVertex), 4, 0x3210);

        for (uint32_t i = 0; i < g_hdr.num_submeshes; ++i) {
            const Bw3dSubmesh* sm = &g_submeshes[i];
            if (sm->index_count == 0) continue;

            if (sm->tex_id >= 0 && (uint32_t)sm->tex_id < g_hdr.num_textures && g_tex_valid && g_tex_valid[sm->tex_id]) {
                C3D_TexBind(0, &g_textures[sm->tex_id]);
            } else {
                C3D_TexBind(0, &g_whiteTex);
            }

            if ((sm->flags & SUBMESH_PAINTABLE) && !(sm->flags & SUBMESH_PAINTED)) {
                /* Thinned Toon Ghost Shimmer (cyan-green pulse so player sees paintable silhouettes!) */
                float pulse = 0.45f + 0.25f * sinf(g_time_sec * 5.0f);
                C3D_FVUnifSet(GPU_VERTEX_SHADER, g_uLoc_materialTint, 0.25f, pulse + 0.25f, 0.95f, 0.50f);
            } else {
                C3D_FVUnifSet(GPU_VERTEX_SHADER, g_uLoc_materialTint, 1.0f, 1.0f, 1.0f, 1.0f);
            }

            C3D_DrawElements(GPU_TRIANGLES, sm->index_count, C3D_UNSIGNED_SHORT, g_level_ibo + sm->first_index);
        }
    }

    /* 2. Draw Dynamic Gameplay Objects (Mickey, Paintbrush, Blast Cone, Enemies, Bosses, Reticle) */
    if (g_dyn_vbo_count > 0) {
        C3D_TexBind(0, &g_whiteTex);
        C3D_FVUnifSet(GPU_VERTEX_SHADER, g_uLoc_materialTint, 1.0f, 1.0f, 1.0f, 1.0f);
        C3D_BufInfo* bufInfo = C3D_GetBufInfo();
        BufInfo_Init(bufInfo);
        BufInfo_Add(bufInfo, g_dyn_vbo, sizeof(Bw3dVertex), 4, 0x3210);
        C3D_DrawArrays(GPU_TRIANGLES, 0, g_dyn_vbo_count);
    }
}

static void emit_dyn_box(float cx, float cy, float cz, float hx, float hy, float hz, uint8_t r, uint8_t g, uint8_t b, uint8_t a) {
    static const float tris[36][3] = {
        {-1,-1, 1},{ 1,-1, 1},{ 1, 1, 1}, {-1,-1, 1},{ 1, 1, 1},{-1, 1, 1},
        { 1,-1,-1},{-1,-1,-1},{-1, 1,-1}, { 1,-1,-1},{-1, 1,-1},{ 1, 1,-1},
        {-1, 1, 1},{ 1, 1, 1},{ 1, 1,-1}, {-1, 1, 1},{ 1, 1,-1},{-1, 1,-1},
        {-1,-1,-1},{ 1,-1,-1},{ 1,-1, 1}, {-1,-1,-1},{ 1,-1, 1},{-1,-1, 1},
        { 1,-1, 1},{ 1,-1,-1},{ 1, 1,-1}, { 1,-1, 1},{ 1, 1,-1},{ 1, 1, 1},
        {-1,-1,-1},{-1,-1, 1},{-1, 1, 1}, {-1,-1,-1},{-1, 1, 1},{-1, 1,-1}
    };
    static const float norms[6][3] = {
        {0,0,1},{0,0,-1},{0,1,0},{0,-1,0},{1,0,0},{-1,0,0}
    };
    if (g_dyn_vbo_count + 36 > MAX_DYN_VERTS) return;
    for (int i = 0; i < 36; ++i) {
        Bw3dVertex* v = &g_dyn_vbo[g_dyn_vbo_count++];
        v->pos[0] = cx + tris[i][0] * hx;
        v->pos[1] = cy + tris[i][1] * hy;
        v->pos[2] = cz + tris[i][2] * hz;
        v->uv[0]  = 0.5f;
        v->uv[1]  = 0.5f;
        v->rgba[0] = r; v->rgba[1] = g; v->rgba[2] = b; v->rgba[3] = a;
        v->nrm[0] = norms[i / 6][0];
        v->nrm[1] = norms[i / 6][1];
        v->nrm[2] = norms[i / 6][2];
    }
}

static void build_dynamic_frame_vbo(void) {
    g_dyn_vbo_count = 0;

    /* 1. Mickey Character Model */
    bool blink = (g_player.iframe_timer > 0.0f) && (((int)(g_time_sec * 16.0f)) & 1);
    if (!blink) {
        float px = g_player.pos[0], py = g_player.pos[1], pz = g_player.pos[2];
        float s = sinf(g_player.yaw), c = cosf(g_player.yaw);
        /* Shoes (yellow) */
        emit_dyn_box(px - 0.16f * c, py + 0.12f, pz + 0.16f * s, 0.13f, 0.11f, 0.18f, 245, 205, 45, 255);
        emit_dyn_box(px + 0.16f * c, py + 0.12f, pz - 0.16f * s, 0.13f, 0.11f, 0.18f, 245, 205, 45, 255);
        /* Shorts (red) */
        emit_dyn_box(px, py + 0.42f, pz, 0.24f, 0.18f, 0.22f, 225, 40, 45, 255);
        /* Torso (black) */
        emit_dyn_box(px, py + 0.75f, pz, 0.21f, 0.20f, 0.19f, 28, 28, 35, 255);
        /* Head & Iconic Mickey Ears */
        emit_dyn_box(px, py + 1.15f, pz, 0.24f, 0.24f, 0.24f, 32, 32, 40, 255);
        emit_dyn_box(px - 0.26f * c, py + 1.44f, pz + 0.26f * s, 0.15f, 0.15f, 0.08f, 22, 22, 28, 255);
        emit_dyn_box(px + 0.26f * c, py + 1.44f, pz - 0.26f * s, 0.15f, 0.15f, 0.08f, 22, 22, 28, 255);
        /* Magic Paintbrush in hand */
        float bx = px - s * 0.42f + c * 0.25f;
        float bz = pz - c * 0.42f - s * 0.25f;
        emit_dyn_box(bx, py + 0.85f, bz, 0.07f, 0.32f, 0.07f, 150, 105, 55, 255);
        emit_dyn_box(bx, py + 1.22f, bz, 0.11f, 0.12f, 0.11f, 45, 165, 255, 255);
    }

    /* Spin Attack ring & Blast Cone wave */
    if (g_player.spin_timer > 0.0f) {
        emit_dyn_box(g_player.pos[0], g_player.pos[1] + 0.65f, g_player.pos[2], 1.45f, 0.08f, 1.45f, 230, 240, 255, 180);
    }
    if (g_player.blast_timer > 0.0f) {
        float r = (0.5f - g_player.blast_timer) * 9.0f;
        uint8_t br = g_player.blast_is_paint ? 45  : 55;
        uint8_t bg = g_player.blast_is_paint ? 165 : 245;
        uint8_t bb = g_player.blast_is_paint ? 255 : 85;
        emit_dyn_box(g_player.pos[0], g_player.pos[1] + 0.35f, g_player.pos[2], r, 0.15f, r, br, bg, bb, 170);
    }

    /* 2. Actors, Enemies, Bosses, NPCs, Pickups */
    for (uint32_t i = 0; i < g_hdr.num_actors; ++i) {
        const LiveActor* a = &g_actors[i];
        if (a->ai_state == 5 || a->base.kind == ACTOR_TRIGGER_VOL || a->base.kind == ACTOR_TRIGGER_DMG) continue;
        float ax = a->base.pos[0], ay = a->base.pos[1], az = a->base.pos[2];

        if (a->base.kind == ACTOR_PICKUP) {
            float bob = 0.15f * sinf(g_time_sec * 4.0f + (float)i);
            emit_dyn_box(ax, ay + 0.45f + bob, az, 0.28f, 0.28f, 0.10f, 255, 215, 45, 255);
            continue;
        }
        if (a->base.kind == ACTOR_NPC) {
            /* Gremlin NPC (purple-blue flight suit + goggles) */
            emit_dyn_box(ax, ay + 0.50f, az, 0.25f, 0.50f, 0.25f, 80, 145, 220, 255);
            emit_dyn_box(ax, ay + 1.15f, az, 0.24f, 0.22f, 0.24f, 195, 165, 120, 255);
            continue;
        }

        uint8_t cr = 135, cg = 50, cb = 185;
        float scale = 1.0f;
        if (a->base.team == TEAM_FRIENDLY) {
            cr = 45; cg = 195; cb = 250; /* Friendly Paint-converted ally! */
        } else if (a->stun_timer > 0.0f) {
            cr = 250; cg = 235; cb = 65; /* Stunned yellow flash */
        } else if (a->base.kind == ACTOR_SWEEPER) {
            cr = 60; cg = 195; cb = 110;
        } else if (a->base.kind == ACTOR_SPLADOOSH) {
            bool flash = (a->ai_state == 2) && (((int)(g_time_sec * 12.0f)) & 1);
            cr = flash ? 255 : 235; cg = flash ? 60 : 140; cb = 40;
            scale = (a->ai_state == 2) ? 1.35f : 1.1f;
        } else if (a->base.kind == ACTOR_SEER) {
            cr = 210; cg = 75; cb = 140;
        } else if (a->base.kind == ACTOR_BEETLEWORX) {
            cr = a->armor_intact ? 155 : 255;
            cg = a->armor_intact ? 165 : 55;
            cb = a->armor_intact ? 175 : 55;
            scale = 1.25f;
        } else if (a->base.kind == ACTOR_BOSS) {
            cr = a->armor_intact ? 35 : 255;
            cg = a->armor_intact ? 215 : 85;
            cb = 255;
            scale = 2.0f;
        }

        emit_dyn_box(ax, ay + 0.55f * scale, az, 0.38f * scale, 0.55f * scale, 0.38f * scale, cr, cg, cb, 255);
        emit_dyn_box(ax, ay + 1.25f * scale, az, 0.30f * scale, 0.25f * scale, 0.30f * scale,
                     (uint8_t)clampf(cr + 30, 0, 255), (uint8_t)clampf(cg + 30, 0, 255), (uint8_t)clampf(cb + 30, 0, 255), 255);
    }

    /* 3. Projectiles & Particles */
    for (int i = 0; i < MAX_PROJECTILES; ++i) {
        if (!g_projectiles[i].active) continue;
        uint8_t r = g_projectiles[i].is_paint ? 55 : 55;
        uint8_t g = g_projectiles[i].is_paint ? 165 : 245;
        uint8_t b = g_projectiles[i].is_paint ? 255 : 85;
        emit_dyn_box(g_projectiles[i].pos[0], g_projectiles[i].pos[1], g_projectiles[i].pos[2], 0.18f, 0.18f, 0.18f, r, g, b, 255);
    }
    for (int i = 0; i < MAX_PARTICLES; ++i) {
        if (!g_particles[i].active) continue;
        float s = g_particles[i].size;
        emit_dyn_box(g_particles[i].pos[0], g_particles[i].pos[1], g_particles[i].pos[2],
                     s, s, s, g_particles[i].rgba[0], g_particles[i].rgba[1], g_particles[i].rgba[2], 220);
    }

    /* 4. 3D World Reticle Crosshair */
    emit_dyn_box(g_player.aim_world[0], g_player.aim_world[1], g_player.aim_world[2], 0.22f, 0.05f, 0.05f, 95, 225, 255, 255);
    emit_dyn_box(g_player.aim_world[0], g_player.aim_world[1], g_player.aim_world[2], 0.05f, 0.22f, 0.05f, 95, 225, 255, 255);

    GSPGPU_FlushDataCache(g_dyn_vbo, g_dyn_vbo_count * sizeof(Bw3dVertex));
}

static void draw_bottom_hud(void) {
    printf("\x1b[1;1H\x1b[36;1m== BLOTWORX ENGINE 3DS v0.1.0-demo.8 ==\x1b[0m");
    printf("\x1b[2;1HLvl: \x1b[33m%-34.34s\x1b[0m", g_hdr.title);

    /* Health, Paint, Thinner, E-Tickets */
    char hp_bar[16] = {0};
    for (int i = 0; i < g_player.max_hp && i < 10; ++i) hp_bar[i] = (i < g_player.hp) ? '*' : '.';
    printf("\x1b[4;1HHP: \x1b[31;1m[%s]\x1b[0m   E-Tickets: \x1b[33;1m%03d\x1b[0m       ", hp_bar, g_player.etickets);
    printf("\x1b[5;1H\x1b[34;1mPaint: %5.1f%%\x1b[0m  \x1b[32;1mThinner: %5.1f%%\x1b[0m    ", g_player.paint, g_player.thinner);

    /* Count painted toon pieces & active hostiles/friendlies */
    int painted_toons = 0, total_toons = 0, hostiles = 0, allies = 0;
    const LiveActor* boss = NULL;
    for (uint32_t i = 0; i < g_hdr.num_submeshes; ++i) {
        if (g_submeshes[i].flags & SUBMESH_PAINTABLE) {
            total_toons++;
            if (g_submeshes[i].flags & SUBMESH_PAINTED) painted_toons++;
        }
    }
    for (uint32_t i = 0; i < g_hdr.num_actors; ++i) {
        if (g_actors[i].ai_state == 5) continue;
        if (g_actors[i].base.kind >= ACTOR_SPATTER && g_actors[i].base.kind <= ACTOR_BOSS) {
            if (g_actors[i].base.team == TEAM_HOSTILE) hostiles++;
            else if (g_actors[i].base.team == TEAM_FRIENDLY) allies++;
            if (g_actors[i].base.kind == ACTOR_BOSS) boss = &g_actors[i];
        }
    }
    printf("\x1b[6;1HToons:%d/%d  Enemies:%d  Allies:%d      ", painted_toons, total_toons, hostiles, allies);
    if (boss) {
        printf("\x1b[7;1H\x1b[35;1mBOSS %-14.14s HP:%2d/%2d %s\x1b[0m",
               boss->base.name, boss->base.hp, boss->base.max_hp, boss->armor_intact ? "[ARMOR]" : "[WEAK!]");
    } else {
        printf("\x1b[7;1HPos:(%5.1f,%4.1f,%5.1f) %-8.8s    ",
               g_player.pos[0], g_player.pos[1], g_player.pos[2],
               g_player.ledge_grabbed ? "[LEDGE]" : (g_player.grounded ? "[GROUND]" : "[IN AIR]"));
    }

    /* Old3DS / New3DS Memory Budget Telemetry */
    u32 app_free = osGetMemRegionFree(MEMREGION_APPLICATION) / (1024 * 1024);
    u32 lin_free = linearSpaceFree() / (1024 * 1024);
    printf("\x1b[8;1HRAM Free: App %2luMB | Lin %2luMB (128M OK)", (unsigned long)app_free, (unsigned long)lin_free);

    if (g_show_level_menu) {
        printf("\x1b[10;1H\x1b[33;1m-- SELECT LEVEL (DPad+A, SELECT exit) -\x1b[0m");
        int total = g_num_levels > 0 ? g_num_levels : 1;
        for (int i = 0; i < 8; ++i) {
            int idx = i;
            if (idx < total) {
                const char* nm = (g_num_levels > 0) ? g_level_names[idx] : "GV_ZoneA_Start (Built-in)";
                printf("\x1b[%d;1H %c %-35.35s", 11 + i, (idx == g_menu_cursor) ? '>' : ' ', nm);
            } else {
                printf("\x1b[%d;1H                                       ", 11 + i);
            }
        }
    } else if (g_player.active_dialogue >= 0 && (uint32_t)g_player.active_dialogue < g_hdr.num_dialogues) {
        const Bw3dDialogue* dlg = &g_dialogues[g_player.active_dialogue];
        printf("\x1b[10;1H\x1b[32;1m+-- %-29.29s --+\x1b[0m", dlg->speaker);
        printf("\x1b[11;1H| %-35.35s |", dlg->text);
        size_t len = strlen(dlg->text);
        printf("\x1b[12;1H| %-35.35s |", len > 35 ? dlg->text + 35 : "");
        printf("\x1b[13;1H| %-35.35s |", len > 70 ? dlg->text + 70 : "");
        printf("\x1b[14;1H\x1b[32;1m+-- [Press A to Continue] -----------+\x1b[0m");
        printf("\x1b[15;1H                                       ");
        printf("\x1b[16;1H                                       ");
        printf("\x1b[17;1H                                       ");
        printf("\x1b[18;1H                                       ");
    } else {
        printf("\x1b[10;1H\x1b[36m[Touch Screen: Aim Paint/Thinner]      \x1b[0m");
        printf("\x1b[11;1HCPad:Move  B:Jump/2x  Y:Spin  X:Blast  ");
        printf("\x1b[12;1HR:Paint    L:Thinner  A:Talk  DPad:Cam ");
        printf("\x1b[13;1HSELECT:Level Menu     START:Exit       ");
        printf("\x1b[14;1HPC Export: blotworx.exe export-3ds     ");
        printf("\x1b[15;1HSD Path  : sdmc:/blotworx/levels/*.bw3d");
        printf("\x1b[16;1H                                       ");
        printf("\x1b[17;1H                                       ");
        printf("\x1b[18;1H                                       ");
    }

    printf("\x1b[20;1H\x1b[33m%-39.39s\x1b[0m", g_status_timer > 0.0f ? g_status_msg : "Status: Ready");
    printf("\x1b[27;1H\x1b[44;37;1m[P-BLAST]\x1b[42;30;1m[T-BLAST]\x1b[43;30;1m[RESPAWN]\x1b[45;37;1m[ LEVELS ]\x1b[0m");
}

int main(int argc, char** argv) {
    (void)argc; (void)argv;
    osSetSpeedupEnable(true); /* Enable 804 MHz & L2 cache on New 3DS while remaining 100% Old 3DS compatible */
    gfxInitDefault();
    gfxSet3D(true);
    romfsInit();
    consoleInit(GFX_BOTTOM, NULL);

    C3D_Init(C3D_DEFAULT_CMDBUF_SIZE);

    g_targetLeft  = C3D_RenderTargetCreate(240, 400, GPU_RB_RGBA8, GPU_RB_DEPTH24_STENCIL8);
    g_targetRight = C3D_RenderTargetCreate(240, 400, GPU_RB_RGBA8, GPU_RB_DEPTH24_STENCIL8);
    C3D_RenderTargetSetOutput(g_targetLeft,  GFX_TOP, GFX_LEFT,  DISPLAY_TRANSFER_FLAGS);
    C3D_RenderTargetSetOutput(g_targetRight, GFX_TOP, GFX_RIGHT, DISPLAY_TRANSFER_FLAGS);

    /* Load PICA200 Vertex Shader */
    g_dvlb = DVLB_ParseFile((u32*)vshader_shbin, vshader_shbin_size);
    shaderProgramInit(&g_program);
    shaderProgramSetVsh(&g_program, &g_dvlb->DVLE[0]);
    C3D_BindProgram(&g_program);
    C3D_DepthTest(true, GPU_GEQUAL, GPU_WRITE_ALL);
    C3D_CullFace(GPU_CULL_NONE);
    C3D_AlphaBlend(GPU_BLEND_ADD, GPU_BLEND_ADD, GPU_SRC_ALPHA, GPU_ONE_MINUS_SRC_ALPHA, GPU_SRC_ALPHA, GPU_ONE_MINUS_SRC_ALPHA);

    g_uLoc_proj         = shaderInstanceGetUniformLocation(g_program.vertexShader, "projection");
    g_uLoc_modelView    = shaderInstanceGetUniformLocation(g_program.vertexShader, "modelView");
    g_uLoc_lightDir     = shaderInstanceGetUniformLocation(g_program.vertexShader, "lightDir");
    g_uLoc_materialTint = shaderInstanceGetUniformLocation(g_program.vertexShader, "materialTint");

    C3D_AttrInfo* attrInfo = C3D_GetAttrInfo();
    AttrInfo_Init(attrInfo);
    AttrInfo_AddLoader(attrInfo, 0, GPU_FLOAT, 3);         /* v0: pos */
    AttrInfo_AddLoader(attrInfo, 1, GPU_FLOAT, 2);         /* v1: uv */
    AttrInfo_AddLoader(attrInfo, 2, GPU_UNSIGNED_BYTE, 4); /* v2: rgba */
    AttrInfo_AddLoader(attrInfo, 3, GPU_FLOAT, 3);         /* v3: nrm */

    /* Initialize 16x16 white fallback texture */
    C3D_TexInit(&g_whiteTex, 16, 16, GPU_RGBA5551);
    memset(g_whiteTex.data, 0xFF, 16 * 16 * 2);
    GSPGPU_FlushDataCache(g_whiteTex.data, 16 * 16 * 2);

    g_dyn_vbo = (Bw3dVertex*)linearAlloc(MAX_DYN_VERTS * sizeof(Bw3dVertex));

    /* Discover levels in sdmc:/blotworx/levels and romfs:/levels */
    discover_levels();
    if (g_num_levels > 0 && load_bw3d_level(g_level_paths[0])) {
        g_cur_level = 0;
    } else {
        build_builtin_fallback_level();
    }

    u64 last_tick = svcGetSystemTick();

    while (aptMainLoop()) {
        hidScanInput();
        u32 kDown = hidKeysDown();
        u32 kHeld = hidKeysHeld();
        if (kDown & KEY_START) break;

        circlePosition cpad, cstick;
        hidCircleRead(&cpad);
        hidCstickRead(&cstick);
        touchPosition touch;
        bool touching = (kHeld & KEY_TOUCH) != 0;
        if (touching) hidTouchRead(&touch);

        u64 now_tick = svcGetSystemTick();
        float dt = (float)(now_tick - last_tick) / (float)SYSCLOCK_ARM11;
        last_tick = now_tick;
        if (dt <= 0.0f || dt > 0.1f) dt = 1.0f / 60.0f;

        update_simulation(dt, kDown, kHeld, &cpad, &cstick, &touch, touching);
        build_dynamic_frame_vbo();
        draw_bottom_hud();

        float slider = osGet3DSliderState();
        float iod = slider * 0.18f;
        u32 clear_clr = ((u32)g_hdr.sky_rgba[0] << 24) | ((u32)g_hdr.sky_rgba[1] << 16) |
                        ((u32)g_hdr.sky_rgba[2] << 8) | 0xFFu;
        if (clear_clr == 0xFFu) clear_clr = CLEAR_COLOR_DEFAULT;

        C3D_FrameBegin(C3D_FRAME_SYNCDRAW);
        {
            C3D_RenderTargetClear(g_targetLeft, C3D_CLEAR_ALL, clear_clr, 0);
            C3D_FrameDrawOn(g_targetLeft);
            render_scene_for_eye(-iod);

            if (iod > 0.001f) {
                C3D_RenderTargetClear(g_targetRight, C3D_CLEAR_ALL, clear_clr, 0);
                C3D_FrameDrawOn(g_targetRight);
                render_scene_for_eye(iod);
            }
        }
        C3D_FrameEnd(0);
    }

    unload_current_level();
    if (g_dyn_vbo) linearFree(g_dyn_vbo);
    C3D_TexDelete(&g_whiteTex);
    shaderProgramFree(&g_program);
    DVLB_Free(g_dvlb);
    C3D_Fini();
    romfsExit();
    gfxExit();
    return 0;
}
