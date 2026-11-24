// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// A small C shim over libavcodec (FFmpeg 9) for the reference client: open a
// low-latency HEVC or AV1 decoder (NVDEC through CUDA, VA-API, or software),
// decode one access unit, and hand back the picture in system memory.
// FFmpeg's structs change between majors, so C lays them out, not Rust FFI.
#pragma once
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum { RC_CODEC_HEVC = 0, RC_CODEC_AV1 = 1, RC_CODEC_H264 = 2 };
enum { RC_HW_AUTO = 0, RC_HW_CUDA = 1, RC_HW_VAAPI = 2, RC_HW_NONE = 3, RC_HW_D3D11 = 4 };
enum { RC_FMT_NV12 = 1, RC_FMT_YUV420P = 2, RC_FMT_YUV444P = 3 };

typedef struct rc_dec rc_dec;

typedef struct {
    int width, height;
    int format;          // RC_FMT_*
    int full_range;      // 1: full (JPEG) range, 0: limited
    int matrix;          // 1: BT.709, 6: BT.601, as AVColorSpace
    int hw;              // 1 when the picture came from a hardware decoder
    const uint8_t *data[3];
    int linesize[3];
    double transfer_ms;  // hardware picture -> system memory
    void *ref;           // an AVFrame reference keeping the planes alive: rc_frame_free it
    int cuda;            // 1: data[] are CUDA device pointers (rc_dec_keep_cuda)
    void *cuctx;         // their CUcontext
    int d3d11;           // 1: the picture is a slice of a D3D11 decoder texture (Windows zero copy)
    void *texture;       // its ID3D11Texture2D (an NV12 texture array on the shared device)
    int slice;           // its array slice
    int tex_width, tex_height; // the texture's size (the decoder pads it beyond width x height)
} rc_frame;

// Opens a decoder. `backend` receives a short description ("hevc+cuda", "libdav1d").
int rc_dec_open(int codec, int hw, rc_dec **out, char *backend, int backend_len, char *err, int errlen);
// Sends one access unit. *got is set to 1 when a picture came out.
int rc_dec_decode(rc_dec *d, const uint8_t *data, int size, int *got, char *err, int errlen);
// The last picture, in system memory. The planes stay valid until rc_frame_free(out->ref),
// on any thread; FFmpeg's buffer pools recycle them.
int rc_dec_frame(rc_dec *d, rc_frame *out, char *err, int errlen);
void rc_frame_free(void *ref);
// NVDEC pictures stay on the GPU (rc_frame.cuda) instead of being copied down; on Windows,
// D3D11 pictures decoded on the shared device stay there too (rc_frame.d3d11).
void rc_dec_keep_cuda(rc_dec *d, int on);
// Mean of the luma plane over a rectangle (copies just that rectangle down for CUDA and D3D11).
int rc_frame_luma(const rc_frame *f, int x, int y, int w, int h, double *mean, char *err, int errlen);

// CUDA → GL: copy a CUDA picture's planes straight into GL textures (R8, RG8 for
// NV12's CbCr), on the thread whose GL context is current.
typedef struct rc_gl rc_gl;
int rc_gl_open(void *cuctx, rc_gl **out, char *err, int errlen);
// Register (again, after reallocating) the texture for one plane.
int rc_gl_register(rc_gl *g, int plane, unsigned tex, char *err, int errlen);
int rc_gl_copy(rc_gl *g, const rc_frame *f, char *err, int errlen);
void rc_gl_close(rc_gl *g);
void rc_dec_close(rc_dec *d);

#ifdef _WIN32
// Windows zero copy: the presenter's ID3D11Device (created with video support). D3D11VA decoders
// opened after this decode on it, into textures the presenter can sample. NULL stops sharing.
// The device is made multithread-protected; rc_d3d11_lock/unlock is the lock FFmpeg takes around
// its own use of the immediate context, for the presenter to take around its draws.
int rc_d3d11_set_device(void *device);
void rc_d3d11_lock(void);
void rc_d3d11_unlock(void);
// 0: hand every picture back in system memory again (the presenter could not use the textures).
void rc_d3d11_zero_copy(int on);
#endif

#ifdef __cplusplus
}
#endif
