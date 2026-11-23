// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// host-desktop's C shim: video encoders (FFmpeg's, or OpenH264 used directly),
// BGRA -> NV12/I420 conversion and scaling (swscale), and Opus. C, so FFmpeg's and
// OpenH264's struct layouts come from their own headers (as in client-linux).
// The same file builds for Linux (the system's FFmpeg) and for x86_64-pc-windows-gnu
// (remote/packaging/windows/build-deps.sh).
#pragma once
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum { HD_CODEC_HEVC = 0, HD_CODEC_H264 = 1 };

typedef struct hd_enc hd_enc;

typedef struct {
    // An FFmpeg encoder ("hevc_nvenc", "h264_mf", ...) or "openh264" (the library itself).
    const char *name;
    int width, height;  // the encoded size (even)
    int fps;            // the stream's frame rate: one frame's budget is bitrate / fps
    int64_t bitrate;    // bits per second
    int threads;        // software encoders (0: their default)
} hd_enc_config;

// OpenH264 is loaded by path at run time, never linked. `path` is UTF-8 and absolute on Windows
// (Cisco's DLL in the codecs folder); NULL elsewhere tries the system's libopenh264. 0 on success.
int hd_openh264_load(const char *path, char *err, int errlen);
int hd_openh264_loaded(void);

// FFmpeg's own log: errors only, or nothing (while probing encoders that may be absent).
void hd_log_quiet(int quiet);
int hd_enc_open(const hd_enc_config *cfg, hd_enc **out, char *err, int errlen);
int hd_enc_codec(const hd_enc *e);
// 1 when hd_enc_encode's bitrate takes effect on that very frame, at no cost (OpenH264).
// 0 when the rate is fixed at open (a new rate means a new encoder, so a keyframe).
int hd_enc_dynamic_rate(const hd_enc *e);
// Converts the BGRA picture (w x h, `stride` bytes per row; scaled when it isn't the
// encoder's size) and encodes it. *out stays valid until the next call. *out_len may be 0
// when the encoder held the frame back (it shouldn't, with these settings).
int hd_enc_encode(hd_enc *e, const uint8_t *bgra, int w, int h, int stride, int force_key, int64_t bitrate,
                  const uint8_t **out, int *out_len, int *is_key, double *convert_ms, double *encode_ms, char *err,
                  int errlen);
void hd_enc_close(hd_enc *e);
// 1 when the encoder takes DMA-BUF pictures (a *_vaapi encoder on Linux).
int hd_enc_takes_drm(const hd_enc *e);
// Encodes a DMA-BUF picture (DRM fourcc, modifier, planes), converted and scaled to the
// encoder's size on the GPU. Linux, *_vaapi only.
int hd_enc_encode_drm(hd_enc *e, uint32_t fourcc, uint64_t modifier, int nplanes, const int *fds,
                      const uint32_t *offsets, const uint32_t *pitches, int w, int h, int force_key,
                      const uint8_t **out, int *out_len, int *is_key, double *convert_ms, double *encode_ms, char *err,
                      int errlen);

// Opus at 48 kHz stereo, 10 ms frames (480 samples per channel), as remoted sends.
typedef struct hd_opus_enc hd_opus_enc;
typedef struct hd_opus_dec hd_opus_dec;
hd_opus_enc *hd_opus_enc_new(int bitrate);
// pcm: 480 interleaved stereo float frames. Returns the packet's length or < 0.
int hd_opus_encode(hd_opus_enc *o, const float *pcm, uint8_t *out, int cap);
void hd_opus_enc_free(hd_opus_enc *o);
hd_opus_dec *hd_opus_dec_new(void);
// Returns samples per channel decoded into pcm (interleaved stereo), or < 0.
int hd_opus_decode(hd_opus_dec *o, const uint8_t *data, int len, float *pcm, int max_samples);
void hd_opus_dec_free(hd_opus_dec *o);

#ifdef __cplusplus
}
#endif
