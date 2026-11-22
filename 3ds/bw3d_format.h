#ifndef BW3D_FORMAT_H
#define BW3D_FORMAT_H

#include <stdint.h>

#define BW3D_MAGIC 0x44335742u /* "BW3D" little-endian */
#define BW3D_VERSION 1u

#define SUBMESH_PAINTABLE (1u << 0)
#define SUBMESH_PAINTED   (1u << 1)
#define SUBMESH_HAZARD    (1u << 2)
#define SUBMESH_DYNAMIC   (1u << 3)
#define SUBMESH_INERT     (1u << 4)

#define ACTOR_PROP         0u
#define ACTOR_SPATTER      1u
#define ACTOR_SWEEPER      2u
#define ACTOR_SPLADOOSH    3u
#define ACTOR_SEER         4u
#define ACTOR_BEETLEWORX   5u
#define ACTOR_BOSS         6u
#define ACTOR_NPC          7u
#define ACTOR_TRIGGER_VOL  8u
#define ACTOR_TRIGGER_DMG  9u
#define ACTOR_PICKUP       10u

#define TEAM_PLAYER   1u
#define TEAM_HOSTILE  2u
#define TEAM_FRIENDLY 3u
#define TEAM_NEUTRAL  4u

#pragma pack(push, 1)

typedef struct {
    uint32_t magic;
    uint32_t version;
    char     title[64];
    float    spawn_pos[3];
    float    spawn_yaw;
    uint8_t  sky_rgba[4];
    uint32_t num_textures;
    uint32_t num_submeshes;
    uint32_t num_vertices;
    uint32_t num_indices;
    uint32_t num_coll_tris;
    uint32_t num_actors;
    uint32_t num_dialogues;
    uint32_t reserved[5];
} Bw3dHeader;

typedef struct {
    uint16_t width;
    uint16_t height;
    uint16_t pixels[64 * 64]; /* Pre-swizzled PICA200 Morton 8x8 RGBA5551 */
} Bw3dTexture;

typedef struct {
    uint32_t first_index;
    uint32_t index_count;
    int16_t  tex_id;
    uint16_t flags;
    uint16_t actor_id;
    uint16_t jigsaw_piece;
    float    aabb_min[3];
    float    aabb_max[3];
} Bw3dSubmesh;

typedef struct {
    float   pos[3];
    float   uv[2];
    uint8_t rgba[4];
    float   nrm[3];
} Bw3dVertex; /* 36 bytes */

typedef struct {
    float   v0[3];
    float   v1[3];
    float   v2[3];
    float   normal[3];
    uint16_t flags;
    int16_t  submesh_id;
} Bw3dCollTri; /* 52 bytes */

typedef struct {
    uint8_t  kind;
    uint8_t  team;
    int16_t  hp;
    int16_t  max_hp;
    int16_t  dialogue_id;
    float    pos[3];
    float    yaw;
    float    half_extents[3];
    float    patrol_pos[3];
    float    paint_val;
    float    paint_thresh;
    float    thin_thresh;
    char     name[32];
} Bw3dActor; /* 96 bytes */

typedef struct {
    char speaker[32];
    char text[128];
} Bw3dDialogue; /* 160 bytes */

#pragma pack(pop)

#endif /* BW3D_FORMAT_H */
