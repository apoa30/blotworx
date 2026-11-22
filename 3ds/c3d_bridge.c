/*
 * Blotworx Engine - Thin C FFI bridge to libctru & citro3d (PICA200 GPU)
 * for the native Rust armv6k-nintendo-3ds (target_os = "horizon") runtime.
 */
#include <3ds.h>
#include <citro3d.h>
#include <stdint.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "vshader_shbin.h"

#define DISPLAY_TRANSFER_FLAGS \
    (GX_TRANSFER_FLIP_VERT(0) | GX_TRANSFER_OUT_TILED(0) | GX_TRANSFER_RAW_COPY(0) | \
     GX_TRANSFER_IN_FORMAT(GX_TRANSFER_FMT_RGBA8) | GX_TRANSFER_OUT_FORMAT(GX_TRANSFER_FMT_RGB8) | \
     GX_TRANSFER_SCALING(GX_TRANSFER_SCALE_NO))

#define MAX_PICA_TEXTURES 512

typedef struct {
    uint32_t k_down;
    uint32_t k_held;
    uint32_t k_up;
    int16_t  cpad_x;
    int16_t  cpad_y;
    int16_t  cstick_x;
    int16_t  cstick_y;
    uint16_t touch_x;
    uint16_t touch_y;
    uint8_t  touching;
    uint8_t  _pad[3];
    float    slider_3d;
} Bw3dsInput;

/* Give Rust main thread a 2 MB stack (default .3dsx stack is only 32 KB) and 16 MB linear heap for PICA200 VBOs & textures */
u32 __stacksize__           = 2 * 1024 * 1024;
u32 __ctru_linear_heap_size = 16 * 1024 * 1024;

static DVLB_s*           g_dvlb = NULL;
static shaderProgram_s   g_program;
static int               g_uLoc_proj = -1;
static int               g_uLoc_modelView = -1;
static int               g_uLoc_lightDir = -1;
static int               g_uLoc_materialTint = -1;
static C3D_RenderTarget* g_targetLeft = NULL;
static C3D_RenderTarget* g_targetRight = NULL;
static C3D_Tex           g_whiteTex;
static C3D_Tex           g_texPool[MAX_PICA_TEXTURES];
static uint8_t           g_texUsed[MAX_PICA_TEXTURES];
static C3D_Mtx           g_curView;
static const void*       g_lastVbo = NULL;
static uint32_t          g_lastTex = 0xFFFFFFFFu;
static bool              g_inited = false;

static inline unsigned morton_8x8(unsigned x, unsigned y) {
    unsigned i = 0;
    for (unsigned bit = 0; bit < 3; ++bit) {
        i |= ((x >> bit) & 1u) << (2u * bit);
        i |= ((y >> bit) & 1u) << (2u * bit + 1u);
    }
    return i;
}

static void colmajor_to_c3d_mtx(C3D_Mtx* out, const float* m) {
    for (int row = 0; row < 4; ++row) {
        out->r[row] = FVec4_New(m[0 * 4 + row], m[1 * 4 + row], m[2 * 4 + row], m[3 * 4 + row]);
    }
}

void bw3ds_init(void) {
    if (g_inited) return;
    g_inited = true;

    osSetSpeedupEnable(true);
    gfxInitDefault();
    gfxSet3D(true);
    romfsInit();
    consoleInit(GFX_BOTTOM, NULL);

    C3D_Init(C3D_DEFAULT_CMDBUF_SIZE * 4);

    g_targetLeft  = C3D_RenderTargetCreate(240, 400, GPU_RB_RGBA8, GPU_RB_DEPTH24_STENCIL8);
    g_targetRight = C3D_RenderTargetCreate(240, 400, GPU_RB_RGBA8, GPU_RB_DEPTH24_STENCIL8);
    C3D_RenderTargetSetOutput(g_targetLeft,  GFX_TOP, GFX_LEFT,  DISPLAY_TRANSFER_FLAGS);
    C3D_RenderTargetSetOutput(g_targetRight, GFX_TOP, GFX_RIGHT, DISPLAY_TRANSFER_FLAGS);

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

    C3D_TexInit(&g_whiteTex, 16, 16, GPU_RGBA5551);
    memset(g_whiteTex.data, 0xFF, 16 * 16 * 2);
    GSPGPU_FlushDataCache(g_whiteTex.data, 16 * 16 * 2);
    memset(g_texUsed, 0, sizeof(g_texUsed));
}

void bw3ds_fini(void) {
    if (!g_inited) return;
    C3D_FrameSync();
    for (int i = 0; i < MAX_PICA_TEXTURES; ++i) {
        if (g_texUsed[i]) {
            C3D_TexDelete(&g_texPool[i]);
            g_texUsed[i] = 0;
        }
    }
    C3D_TexDelete(&g_whiteTex);
    shaderProgramFree(&g_program);
    DVLB_Free(g_dvlb);
    C3D_Fini();
    romfsExit();
    gfxExit();
    g_inited = false;
}

uint8_t bw3ds_apt_main_loop(void) {
    return aptMainLoop() ? 1 : 0;
}

uint64_t bw3ds_system_tick(void) {
    return svcGetSystemTick();
}

void bw3ds_poll_input(Bw3dsInput* out) {
    hidScanInput();
    circlePosition cpad, cstick;
    hidCircleRead(&cpad);
    hidCstickRead(&cstick);
    touchPosition touch;
    u32 held = hidKeysHeld();
    bool touching = (held & KEY_TOUCH) != 0;
    if (touching) hidTouchRead(&touch);
    else { touch.px = 0; touch.py = 0; }

    out->k_down    = hidKeysDown();
    out->k_held    = held;
    out->k_up      = hidKeysUp();
    out->cpad_x    = cpad.dx;
    out->cpad_y    = cpad.dy;
    out->cstick_x  = cstick.dx;
    out->cstick_y  = cstick.dy;
    out->touch_x   = touch.px;
    out->touch_y   = touch.py;
    out->touching  = touching ? 1 : 0;
    out->slider_3d = osGet3DSliderState();
}

void bw3ds_ram_free(uint32_t* app_free_mb, uint32_t* lin_free_mb) {
    if (app_free_mb) {
        s64 f = osGetMemRegionFree(MEMREGION_APPLICATION);
        *app_free_mb = (f > 0 && f < (s64)(256 * 1024 * 1024)) ? (uint32_t)(f / (1024 * 1024)) : 0;
    }
    if (lin_free_mb) *lin_free_mb = linearSpaceFree() / (1024 * 1024);
}

void bw3ds_debug_log(const char* text) {
    if (!text) return;
    svcOutputDebugString(text, (s32)strlen(text));
}

void bw3ds_console_clear(void) {
    consoleClear();
}

void bw3ds_console_line(int row, const char* text) {
    printf("\x1b[%d;1H%-39.39s", row, text ? text : "");
}

void bw3ds_console_present_loading(void) {
    /* Present a blank top frame + updated bottom console text during level load */
    gfxFlushBuffers();
    C3D_FrameBegin(C3D_FRAME_SYNCDRAW);
    C3D_RenderTargetClear(g_targetLeft, C3D_CLEAR_ALL, 0x101828FFu, 0);
    C3D_FrameDrawOn(g_targetLeft);
    C3D_FrameEnd(0);
}

uint32_t bw3ds_tex_create_rgba8(const uint8_t* rgba, uint32_t width, uint32_t height, uint32_t target_dim) {
    if (!rgba || width == 0 || height == 0) return 0;
    if (target_dim != 32 && target_dim != 64 && target_dim != 128) target_dim = 64;

    int slot = -1;
    for (int i = 0; i < MAX_PICA_TEXTURES; ++i) {
        if (!g_texUsed[i]) { slot = i; break; }
    }
    if (slot < 0) return 0;

    if (!C3D_TexInit(&g_texPool[slot], (u16)target_dim, (u16)target_dim, GPU_RGBA5551)) {
        return 0;
    }
    C3D_TexSetFilter(&g_texPool[slot], GPU_LINEAR, GPU_LINEAR);
    C3D_TexSetWrap(&g_texPool[slot], GPU_REPEAT, GPU_REPEAT);

    uint16_t* dst = (uint16_t*)g_texPool[slot].data;
    uint32_t tiles = target_dim / 8;
    for (uint32_t ty = 0; ty < tiles; ++ty) {
        for (uint32_t tx = 0; tx < tiles; ++tx) {
            uint32_t tile_base = (ty * tiles + tx) * 64;
            for (uint32_t py = 0; py < 8; ++py) {
                for (uint32_t px = 0; px < 8; ++px) {
                    uint32_t dx = tx * 8 + px;
                    uint32_t dy = ty * 8 + py;
                    uint32_t sx = (dx * width) / target_dim;
                    if (sx >= width) sx = width - 1;
                    uint32_t sy = ((target_dim - 1 - dy) * height) / target_dim;
                    if (sy >= height) sy = height - 1;
                    uint32_t idx = (sy * width + sx) * 4;
                    uint16_t r = (uint16_t)(rgba[idx + 0] >> 3);
                    uint16_t g = (uint16_t)(rgba[idx + 1] >> 3);
                    uint16_t b = (uint16_t)(rgba[idx + 2] >> 3);
                    uint16_t a = rgba[idx + 3] >= 96 ? 1u : 0u;
                    dst[tile_base + morton_8x8(px, py)] = (uint16_t)((r << 11) | (g << 6) | (b << 1) | a);
                }
            }
        }
    }
    GSPGPU_FlushDataCache(dst, target_dim * target_dim * 2);
    g_texUsed[slot] = 1;
    return (uint32_t)(slot + 1);
}

void bw3ds_tex_free_all(void) {
    C3D_FrameSync();
    for (int i = 0; i < MAX_PICA_TEXTURES; ++i) {
        if (g_texUsed[i]) {
            C3D_TexDelete(&g_texPool[i]);
            g_texUsed[i] = 0;
        }
    }
}

void* bw3ds_linear_alloc(uint32_t bytes) {
    if (bytes == 0) return NULL;
    return linearAlloc(bytes);
}

void bw3ds_linear_free(void* ptr) {
    if (ptr) linearFree(ptr);
}

void bw3ds_flush_cache(const void* ptr, uint32_t bytes) {
    if (ptr && bytes > 0) GSPGPU_FlushDataCache((void*)ptr, bytes);
}

void bw3ds_frame_sync(void) {
    C3D_FrameSync();
}

void bw3ds_frame_begin(void) {
    C3D_FrameBegin(C3D_FRAME_SYNCDRAW);
}

void bw3ds_set_blend_mode(int transparent) {
    if (transparent) {
        C3D_DepthTest(true, GPU_GEQUAL, GPU_WRITE_COLOR);
        C3D_AlphaBlend(GPU_BLEND_ADD, GPU_BLEND_ADD, GPU_SRC_ALPHA, GPU_ONE_MINUS_SRC_ALPHA, GPU_SRC_ALPHA, GPU_ONE_MINUS_SRC_ALPHA);
        C3D_AlphaTest(false, GPU_ALWAYS, 0);
    } else {
        C3D_DepthTest(true, GPU_GEQUAL, GPU_WRITE_ALL);
        C3D_AlphaBlend(GPU_BLEND_ADD, GPU_BLEND_ADD, GPU_ONE, GPU_ZERO, GPU_ONE, GPU_ZERO);
        C3D_AlphaTest(true, GPU_GREATER, 0x20);
    }
}

void bw3ds_begin_eye(int eye_idx, uint32_t clear_rgba, float fov_deg, float near_z, float far_z, float iod, const float* view_col_major_16) {
    C3D_RenderTarget* tgt = (eye_idx == 0) ? g_targetLeft : g_targetRight;
    C3D_RenderTargetClear(tgt, C3D_CLEAR_ALL, clear_rgba, 0);
    C3D_FrameDrawOn(tgt);

    C3D_Mtx proj;
    Mtx_PerspStereoTilt(&proj, C3D_AngleFromDegrees(fov_deg), C3D_AspectRatioTop, near_z, far_z, iod, 3.0f, false);
    colmajor_to_c3d_mtx(&g_curView, view_col_major_16);

    C3D_FVUnifMtx4x4(GPU_VERTEX_SHADER, g_uLoc_proj, &proj);
    C3D_FVUnifSet(GPU_VERTEX_SHADER, g_uLoc_lightDir, 0.35f, 0.82f, 0.45f, 1.0f);

    C3D_TexEnv* env = C3D_GetTexEnv(0);
    C3D_TexEnvInit(env);
    C3D_TexEnvSrc(env, C3D_Both, GPU_TEXTURE0, GPU_PRIMARY_COLOR, 0);
    C3D_TexEnvFunc(env, C3D_Both, GPU_MODULATE);
    bw3ds_set_blend_mode(0);

    g_lastVbo = NULL;
    g_lastTex = 0xFFFFFFFFu;
}

void bw3ds_draw_indexed(
    const void* vbo_linear,
    const uint16_t* ibo_linear,
    uint32_t index_count,
    uint32_t tex_handle,
    const float* model_col_major_16,
    const float* tint_rgba_4
) {
    if (!vbo_linear || !ibo_linear || index_count < 3) return;

    C3D_Mtx model, mv;
    if (model_col_major_16) {
        colmajor_to_c3d_mtx(&model, model_col_major_16);
        Mtx_Multiply(&mv, &g_curView, &model);
        C3D_FVUnifMtx4x4(GPU_VERTEX_SHADER, g_uLoc_modelView, &mv);
    } else {
        C3D_FVUnifMtx4x4(GPU_VERTEX_SHADER, g_uLoc_modelView, &g_curView);
    }

    if (vbo_linear != g_lastVbo) {
        C3D_BufInfo* bufInfo = C3D_GetBufInfo();
        BufInfo_Init(bufInfo);
        BufInfo_Add(bufInfo, vbo_linear, 36, 4, 0x3210);
        g_lastVbo = vbo_linear;
    }

    if (tex_handle != g_lastTex) {
        if (tex_handle >= 1 && tex_handle <= MAX_PICA_TEXTURES && g_texUsed[tex_handle - 1]) {
            C3D_TexBind(0, &g_texPool[tex_handle - 1]);
        } else {
            C3D_TexBind(0, &g_whiteTex);
        }
        g_lastTex = tex_handle;
    }

    if (tint_rgba_4) {
        C3D_FVUnifSet(GPU_VERTEX_SHADER, g_uLoc_materialTint, tint_rgba_4[0], tint_rgba_4[1], tint_rgba_4[2], tint_rgba_4[3]);
    } else {
        C3D_FVUnifSet(GPU_VERTEX_SHADER, g_uLoc_materialTint, 1.0f, 1.0f, 1.0f, 1.0f);
    }

    C3D_DrawElements(GPU_TRIANGLES, (int)index_count, C3D_UNSIGNED_SHORT, ibo_linear);
}

void bw3ds_frame_end(void) {
    C3D_FrameEnd(0);
}

/* Provide POSIX getrandom() for Rust std HashMap/random seeding on Nintendo 3DS */
ssize_t getrandom(void* buf, size_t buflen, unsigned int flags) {
    (void)flags;
    if (!buf || buflen == 0) return 0;
    uint8_t* out = (uint8_t*)buf;
    uint64_t state = svcGetSystemTick() ^ 0x9E3779B97F4A7C15ULL;
    for (size_t i = 0; i < buflen; ++i) {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out[i] = (uint8_t)(state & 0xFFu);
    }
    return (ssize_t)buflen;
}
