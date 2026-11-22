#pragma once
#include <cmath>
#include <cstdint>
#include <cstring>

struct alignas(16) __m128 {
    float f[4];
};

struct alignas(16) __m128i {
    int32_t i[4];
};

#ifndef _MM_SHUFFLE
#define _MM_SHUFFLE(fp3, fp2, fp1, fp0) (((fp3) << 6) | ((fp2) << 4) | ((fp1) << 2) | ((fp0)))
#endif

static inline uint32_t _sse_f2u(float v) { uint32_t u; std::memcpy(&u, &v, 4); return u; }
static inline float _sse_u2f(uint32_t u) { float v; std::memcpy(&v, &u, 4); return v; }

static inline __m128 _mm_setzero_ps() { return {{0.0f, 0.0f, 0.0f, 0.0f}}; }
static inline __m128i _mm_setzero_si128() { return {{0, 0, 0, 0}}; }
static inline __m128 _mm_set1_ps(float v) { return {{v, v, v, v}}; }
static inline __m128 _mm_set_ps(float z, float y, float x, float w) { return {{w, x, y, z}}; }
static inline __m128i _mm_set1_epi32(int32_t v) { return {{v, v, v, v}}; }
static inline __m128i _mm_set_epi32(int32_t e3, int32_t e2, int32_t e1, int32_t e0) { return {{e0, e1, e2, e3}}; }

static inline __m128 _mm_add_ps(__m128 a, __m128 b) {
    return {{a.f[0] + b.f[0], a.f[1] + b.f[1], a.f[2] + b.f[2], a.f[3] + b.f[3]}};
}
static inline __m128 _mm_sub_ps(__m128 a, __m128 b) {
    return {{a.f[0] - b.f[0], a.f[1] - b.f[1], a.f[2] - b.f[2], a.f[3] - b.f[3]}};
}
static inline __m128 _mm_mul_ps(__m128 a, __m128 b) {
    return {{a.f[0] * b.f[0], a.f[1] * b.f[1], a.f[2] * b.f[2], a.f[3] * b.f[3]}};
}
static inline __m128 _mm_div_ps(__m128 a, __m128 b) {
    return {{a.f[0] / b.f[0], a.f[1] / b.f[1], a.f[2] / b.f[2], a.f[3] / b.f[3]}};
}
static inline __m128 _mm_hsub_ps(__m128 a, __m128 b) {
    return {{a.f[0] - a.f[1], a.f[2] - a.f[3], b.f[0] - b.f[1], b.f[2] - b.f[3]}};
}
static inline __m128 _mm_sqrt_ss(__m128 a) {
    return {{std::sqrt(a.f[0]), a.f[1], a.f[2], a.f[3]}};
}
static inline float _mm_cvtss_f32(__m128 a) { return a.f[0]; }

static inline __m128 _mm_and_ps(__m128 a, __m128 b) {
    return {{_sse_u2f(_sse_f2u(a.f[0]) & _sse_f2u(b.f[0])),
             _sse_u2f(_sse_f2u(a.f[1]) & _sse_f2u(b.f[1])),
             _sse_u2f(_sse_f2u(a.f[2]) & _sse_f2u(b.f[2])),
             _sse_u2f(_sse_f2u(a.f[3]) & _sse_f2u(b.f[3]))}};
}
static inline __m128 _mm_or_ps(__m128 a, __m128 b) {
    return {{_sse_u2f(_sse_f2u(a.f[0]) | _sse_f2u(b.f[0])),
             _sse_u2f(_sse_f2u(a.f[1]) | _sse_f2u(b.f[1])),
             _sse_u2f(_sse_f2u(a.f[2]) | _sse_f2u(b.f[2])),
             _sse_u2f(_sse_f2u(a.f[3]) | _sse_f2u(b.f[3]))}};
}
static inline __m128 _mm_xor_ps(__m128 a, __m128 b) {
    return {{_sse_u2f(_sse_f2u(a.f[0]) ^ _sse_f2u(b.f[0])),
             _sse_u2f(_sse_f2u(a.f[1]) ^ _sse_f2u(b.f[1])),
             _sse_u2f(_sse_f2u(a.f[2]) ^ _sse_f2u(b.f[2])),
             _sse_u2f(_sse_f2u(a.f[3]) ^ _sse_f2u(b.f[3]))}};
}

static inline __m128 _mm_cmplt_ps(__m128 a, __m128 b) {
    return {{_sse_u2f(a.f[0] < b.f[0] ? 0xFFFFFFFFu : 0u),
             _sse_u2f(a.f[1] < b.f[1] ? 0xFFFFFFFFu : 0u),
             _sse_u2f(a.f[2] < b.f[2] ? 0xFFFFFFFFu : 0u),
             _sse_u2f(a.f[3] < b.f[3] ? 0xFFFFFFFFu : 0u)}};
}
static inline __m128 _mm_cmple_ps(__m128 a, __m128 b) {
    return {{_sse_u2f(a.f[0] <= b.f[0] ? 0xFFFFFFFFu : 0u),
             _sse_u2f(a.f[1] <= b.f[1] ? 0xFFFFFFFFu : 0u),
             _sse_u2f(a.f[2] <= b.f[2] ? 0xFFFFFFFFu : 0u),
             _sse_u2f(a.f[3] <= b.f[3] ? 0xFFFFFFFFu : 0u)}};
}
static inline __m128 _mm_cmpgt_ps(__m128 a, __m128 b) {
    return {{_sse_u2f(a.f[0] > b.f[0] ? 0xFFFFFFFFu : 0u),
             _sse_u2f(a.f[1] > b.f[1] ? 0xFFFFFFFFu : 0u),
             _sse_u2f(a.f[2] > b.f[2] ? 0xFFFFFFFFu : 0u),
             _sse_u2f(a.f[3] > b.f[3] ? 0xFFFFFFFFu : 0u)}};
}
static inline __m128 _mm_cmpge_ps(__m128 a, __m128 b) {
    return {{_sse_u2f(a.f[0] >= b.f[0] ? 0xFFFFFFFFu : 0u),
             _sse_u2f(a.f[1] >= b.f[1] ? 0xFFFFFFFFu : 0u),
             _sse_u2f(a.f[2] >= b.f[2] ? 0xFFFFFFFFu : 0u),
             _sse_u2f(a.f[3] >= b.f[3] ? 0xFFFFFFFFu : 0u)}};
}
static inline int _mm_movemask_ps(__m128 a) {
    return ((_sse_f2u(a.f[0]) >> 31) << 0) |
           ((_sse_f2u(a.f[1]) >> 31) << 1) |
           ((_sse_f2u(a.f[2]) >> 31) << 2) |
           ((_sse_f2u(a.f[3]) >> 31) << 3);
}
static inline __m128 _mm_blendv_ps(__m128 a, __m128 b, __m128 mask) {
    return {{(_sse_f2u(mask.f[0]) & 0x80000000u) ? b.f[0] : a.f[0],
             (_sse_f2u(mask.f[1]) & 0x80000000u) ? b.f[1] : a.f[1],
             (_sse_f2u(mask.f[2]) & 0x80000000u) ? b.f[2] : a.f[2],
             (_sse_f2u(mask.f[3]) & 0x80000000u) ? b.f[3] : a.f[3]}};
}
static inline __m128 _mm_shuffle_ps(__m128 a, __m128 b, int imm8) {
    return {{a.f[(imm8 >> 0) & 3], a.f[(imm8 >> 2) & 3], b.f[(imm8 >> 4) & 3], b.f[(imm8 >> 6) & 3]}};
}
static inline __m128 _mm_dp_ps(__m128 a, __m128 b, int imm8) {
    float sum = 0.0f;
    for (int k = 0; k < 4; ++k) {
        if (imm8 & (1 << (4 + k))) sum += a.f[k] * b.f[k];
    }
    return {{(imm8 & 1) ? sum : 0.0f, (imm8 & 2) ? sum : 0.0f,
             (imm8 & 4) ? sum : 0.0f, (imm8 & 8) ? sum : 0.0f}};
}
static inline __m128 _mm_insert_ps(__m128 a, __m128 b, int imm8) {
    float tmp = b.f[(imm8 >> 6) & 3];
    __m128 res = a;
    res.f[(imm8 >> 4) & 3] = tmp;
    for (int k = 0; k < 4; ++k) {
        if (imm8 & (1 << k)) res.f[k] = 0.0f;
    }
    return res;
}

static inline __m128i _mm_add_epi32(__m128i a, __m128i b) {
    return {{a.i[0] + b.i[0], a.i[1] + b.i[1], a.i[2] + b.i[2], a.i[3] + b.i[3]}};
}
static inline __m128i _mm_sub_epi32(__m128i a, __m128i b) {
    return {{a.i[0] - b.i[0], a.i[1] - b.i[1], a.i[2] - b.i[2], a.i[3] - b.i[3]}};
}
static inline __m128i _mm_mullo_epi32(__m128i a, __m128i b) {
    return {{a.i[0] * b.i[0], a.i[1] * b.i[1], a.i[2] * b.i[2], a.i[3] * b.i[3]}};
}
static inline __m128i _mm_and_si128(__m128i a, __m128i b) {
    return {{a.i[0] & b.i[0], a.i[1] & b.i[1], a.i[2] & b.i[2], a.i[3] & b.i[3]}};
}
static inline __m128i _mm_or_si128(__m128i a, __m128i b) {
    return {{a.i[0] | b.i[0], a.i[1] | b.i[1], a.i[2] | b.i[2], a.i[3] | b.i[3]}};
}
static inline __m128i _mm_xor_si128(__m128i a, __m128i b) {
    return {{a.i[0] ^ b.i[0], a.i[1] ^ b.i[1], a.i[2] ^ b.i[2], a.i[3] ^ b.i[3]}};
}
static inline __m128i _mm_cmpeq_epi32(__m128i a, __m128i b) {
    return {{a.i[0] == b.i[0] ? -1 : 0, a.i[1] == b.i[1] ? -1 : 0,
             a.i[2] == b.i[2] ? -1 : 0, a.i[3] == b.i[3] ? -1 : 0}};
}
static inline __m128i _mm_slli_epi32(__m128i a, int imm8) {
    uint32_t s = (uint32_t)imm8;
    if (s >= 32) return {{0, 0, 0, 0}};
    return {{(int32_t)((uint32_t)a.i[0] << s), (int32_t)((uint32_t)a.i[1] << s),
             (int32_t)((uint32_t)a.i[2] << s), (int32_t)((uint32_t)a.i[3] << s)}};
}
static inline __m128i _mm_srli_epi32(__m128i a, int imm8) {
    uint32_t s = (uint32_t)imm8;
    if (s >= 32) return {{0, 0, 0, 0}};
    return {{(int32_t)((uint32_t)a.i[0] >> s), (int32_t)((uint32_t)a.i[1] >> s),
             (int32_t)((uint32_t)a.i[2] >> s), (int32_t)((uint32_t)a.i[3] >> s)}};
}
static inline __m128i _mm_srai_epi32(__m128i a, int imm8) {
    int s = imm8 > 31 ? 31 : (imm8 < 0 ? 0 : imm8);
    return {{a.i[0] >> s, a.i[1] >> s, a.i[2] >> s, a.i[3] >> s}};
}
static inline __m128 _mm_castsi128_ps(__m128i a) {
    __m128 r; std::memcpy(&r, &a, 16); return r;
}
static inline __m128 _mm_cvtepi32_ps(__m128i a) {
    return {{(float)a.i[0], (float)a.i[1], (float)a.i[2], (float)a.i[3]}};
}
static inline __m128i _mm_cvtps_epi32(__m128 a) {
    return {{(int32_t)std::nearbyint(a.f[0]), (int32_t)std::nearbyint(a.f[1]),
             (int32_t)std::nearbyint(a.f[2]), (int32_t)std::nearbyint(a.f[3])}};
}
static inline __m128i _mm_hadd_epi32(__m128i a, __m128i b) {
    return {{a.i[0] + a.i[1], a.i[2] + a.i[3], b.i[0] + b.i[1], b.i[2] + b.i[3]}};
}
static inline int32_t _mm_extract_epi32(__m128i a, int imm8) {
    return a.i[imm8 & 3];
}

#ifndef _MM_TRANSPOSE4_PS
#define _MM_TRANSPOSE4_PS(row0, row1, row2, row3) do { \
    __m128 _t0 = {{(row0).f[0], (row1).f[0], (row2).f[0], (row3).f[0]}}; \
    __m128 _t1 = {{(row0).f[1], (row1).f[1], (row2).f[1], (row3).f[1]}}; \
    __m128 _t2 = {{(row0).f[2], (row1).f[2], (row2).f[2], (row3).f[2]}}; \
    __m128 _t3 = {{(row0).f[3], (row1).f[3], (row2).f[3], (row3).f[3]}}; \
    (row0) = _t0; (row1) = _t1; (row2) = _t2; (row3) = _t3; \
} while (0)
#endif
