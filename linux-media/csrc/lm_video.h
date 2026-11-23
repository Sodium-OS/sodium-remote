// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
/* A portal ScreenCast stream over PipeWire: the
 * generic Linux host's picture, cursor and damage. Part of remote-linux-media.
 *
 * The stream is opened on the PipeWire remote the portal handed out
 * (OpenPipeWireRemote's fd) for one node. Formats: 32-bit RGB in any byte order,
 * delivered as BGRx rows; with `dmabuf`, DMA-BUF in BGRx/BGRA/RGBx/RGBA with the
 * LINEAR or implicit modifier is offered first and SHM (MemFd/MemPtr) stays the
 * fallback. Metadata: SPA_META_Header, SPA_META_Cursor (position, hotspot, bitmap),
 * SPA_META_VideoDamage.
 *
 * The newest buffer is held until the reader takes it; an older one that nobody took
 * goes back to the producer and its damage is merged into the next. Every function
 * locks the stream's thread loop itself. Returns: 0 or a count on success, a negative
 * errno on failure. */
#pragma once
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct lm_video lm_video;

typedef struct {
    int32_t x, y, w, h;
} lm_rect;

#define LM_MAX_DAMAGE 32
#define LM_MAX_PLANES 4
#define LM_CURSOR_MAX 256

typedef struct {
    /* 1: a new picture. SHM: copied into the caller's `dst` as BGRx rows of width*4
     * bytes. DMA-BUF: `dmabuf` is 1 and the planes below stay valid until
     * lm_video_release(buffer). */
    int frame;
    int width, height;
    /* What changed, stream pixels; 0 rects means the whole picture. */
    int damage_n;
    lm_rect damage[LM_MAX_DAMAGE];
    int dmabuf;
    uint32_t drm_format; /* DRM fourcc */
    uint64_t modifier;
    int planes;
    int fd[LM_MAX_PLANES];
    uint32_t offset[LM_MAX_PLANES];
    uint32_t stride[LM_MAX_PLANES];
    void *buffer;
    /* 1: the pointer's position or visibility below is new. */
    int cursor;
    int cursor_visible;
    int32_t cursor_x, cursor_y; /* the hotspot, stream pixels */
    /* 1: a new shape, premultiplied BGRA in the caller's `shape` buffer. */
    int shape;
    int shape_w, shape_h, hot_x, hot_y;
} lm_frame;

/* pw_fd is taken over (closed by lm_video_close). fps caps the producer's rate. */
lm_video *lm_video_open(int pw_fd, uint32_t node, int fps, int dmabuf, int *err);
/* The negotiated size: 1 once known, 0 before, -EPIPE when the stream ended. */
int lm_video_size(lm_video *v, int *w, int *h, int *dmabuf);
/* Waits up to timeout_ms for news. 1 with *out filled, 0 on a timeout, -EPIPE once
 * the stream ended (the node went away: sharing stopped), -ENOSPC when `dst` is
 * smaller than width*height*4 (the size changed: ask lm_video_size and come back). */
int lm_video_next(lm_video *v, int timeout_ms, uint8_t *dst, int dst_cap, uint8_t *shape, int shape_cap,
                  lm_frame *out);
/* Gives a DMA-BUF buffer back to the producer. */
void lm_video_release(lm_video *v, void *buffer);
void lm_video_close(lm_video *v);

#ifdef __cplusplus
}
#endif
