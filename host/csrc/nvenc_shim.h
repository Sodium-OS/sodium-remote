// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// A small C API over CUDA (external memory import) and NVENC for remoted.
// libcuda.so.1 and libnvidia-encode.so.1 are loaded at run time.
// Every function returns 0 on success; on failure it writes a message to err.
#pragma once
#include <stdint.h>

typedef struct RdCuda RdCuda;
typedef struct RdExtMem RdExtMem;
typedef struct RdEnc RdEnc;

// Opens the CUDA device whose UUID matches the Vulkan device's (VkPhysicalDeviceIDProperties).
int rd_cuda_open(const uint8_t uuid[16], RdCuda **out, char *err, int errlen);
void rd_cuda_close(RdCuda *cu);

// Imports Vulkan memory exported as an opaque fd (CUDA takes ownership of fd on success)
// and maps it as one linear buffer.
int rd_cuda_import(RdCuda *cu, int fd, uint64_t size, int dedicated, RdExtMem **out, uint64_t *devptr, char *err,
                   int errlen);
void rd_cuda_release(RdCuda *cu, RdExtMem *m);

enum { RD_CODEC_HEVC = 0, RD_CODEC_AV1 = 1 };
enum { RD_INPUT_NV12 = 0, RD_INPUT_YUV444 = 1, RD_INPUT_ARGB = 2 };

typedef struct {
    int codec;
    int input;      // RD_INPUT_*
    int chroma444;  // encode 4:4:4 (HEVC only; with RD_INPUT_ARGB NVENC converts)
    uint32_t width, height, fps;
    uint32_t kbps;
    uint32_t vbv_pct; // VBV size in percent of one frame's budget (100 = one frame)
    int split;        // NV_ENC_SPLIT_ENCODE_MODE: 0 auto, 2 two-way forced, 15 off
    int m1;           // milestone 1 features: QP delta maps, ME hints, on-demand intra refresh
} RdEncParams;

typedef struct {
    const uint8_t *data; // valid until the next rd_enc_encode
    uint32_t size;
    int pic_type;        // NV_ENC_PIC_TYPE
    uint32_t avg_qp;
    double submit_to_ready_ms;
} RdPacket;

int rd_enc_open(RdCuda *cu, const RdEncParams *p, RdEnc **out, char *err, int errlen);
// Registers a CUDA device buffer (from rd_cuda_import) as an input surface; returns its slot.
int rd_enc_register(RdEnc *e, uint64_t devptr, uint32_t pitch, int *slot, char *err, int errlen);
// Encodes the surface in slot and blocks until the bitstream is ready (Annex-B for HEVC).
int rd_enc_encode(RdEnc *e, int slot, uint64_t pts, int force_idr, RdPacket *out, char *err, int errlen);

// Per-picture extras for rd_enc_encode2 (milestone 1).
typedef struct {
    int force_idr;
    uint32_t force_ir_frames; // on-demand intra refresh over this many frames (0: none)
    // QP delta per CTB (HEVC, 32x32) or SB (AV1, 64x64; qindex units), raster order; NULL: none
    const int8_t *qp_delta;
    uint32_t qp_delta_size;
    // Motion hints, one per block in raster order: 16x16 (HEVC) or 64x64 SB (AV1); NULL: none.
    // Integer pixels, the vector from the block to its match in the previous frame.
    const int16_t *mv_xy; // n_mv pairs (x, y)
    const uint8_t *mv_valid; // n_mv flags; 0: no hint for that block
    uint32_t n_mv;
} RdPicOpts;
int rd_enc_encode2(RdEnc *e, int slot, uint64_t pts, const RdPicOpts *o, RdPacket *out, char *err, int errlen);
// Block sizes the maps above use: QP map cell and motion hint block, in pixels.
void rd_enc_grid(RdEnc *e, uint32_t *qp_cell, uint32_t *mv_block);

// New target bitrate from the next frame on: no reset, no IDR.
int rd_enc_set_bitrate(RdEnc *e, uint32_t kbps, char *err, int errlen);
// Bitrate, VBV (percent of one frame's budget), split mode and the rate controller's QP
// ceiling (before the QP delta map, codec units; 0: none) at once, by reconfigure
// (no reset, no IDR; measured to take effect on the next frame).
int rd_enc_set_rate(RdEnc *e, uint32_t kbps, uint32_t vbv_pct, int split, uint32_t max_qp, char *err, int errlen);
// The frame with this pts was lost: stop predicting from it.
int rd_enc_invalidate(RdEnc *e, uint64_t pts, char *err, int errlen);
void rd_enc_close(RdEnc *e);
