// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
/* See lm_video.h. */
#include "lm_video.h"

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include <pipewire/pipewire.h>
#include <spa/buffer/meta.h>
#include <spa/param/video/format-utils.h>
#include <spa/pod/builder.h>
#include <spa/utils/result.h>

#define FOURCC(a, b, c, d) ((uint32_t)(a) | ((uint32_t)(b) << 8) | ((uint32_t)(c) << 16) | ((uint32_t)(d) << 24))
#define MOD_LINEAR 0ULL
#define MOD_INVALID 0x00ffffffffffffffULL
#define CURSOR_META_SIZE(w, h) (sizeof(struct spa_meta_cursor) + sizeof(struct spa_meta_bitmap) + (w) * (h) * 4)

struct lm_video {
    struct pw_thread_loop *loop;
    struct pw_context *ctx;
    struct pw_core *core;
    struct spa_hook core_listener;
    struct pw_stream *stream;
    struct spa_hook stream_listener;
    int fps;
    int want_dmabuf;

    /* the negotiated format */
    int w, h;
    int known;
    uint32_t spa_format;
    int is_dmabuf;
    uint64_t modifier;
    int ended;
    int streaming;
    int debug, debug_buffers;

    /* the newest buffer with a picture, until the reader takes it */
    struct pw_buffer *pending;
    /* damage since the reader's last picture */
    int damage_full;
    int damage_n;
    lm_rect damage[LM_MAX_DAMAGE];

    /* the pointer */
    int cursor_new, cursor_visible, cursor_valid, shape_empty;
    int32_t cx, cy;
    int shape_new, shape_w, shape_h, hot_x, hot_y;
    uint8_t shape[LM_CURSOR_MAX * LM_CURSOR_MAX * 4];
};

static uint32_t drm_fourcc(uint32_t spa) {
    switch (spa) {
    case SPA_VIDEO_FORMAT_BGRx: return FOURCC('X', 'R', '2', '4');
    case SPA_VIDEO_FORMAT_BGRA: return FOURCC('A', 'R', '2', '4');
    case SPA_VIDEO_FORMAT_RGBx: return FOURCC('X', 'B', '2', '4');
    case SPA_VIDEO_FORMAT_RGBA: return FOURCC('A', 'B', '2', '4');
    default: return 0;
    }
}

/* One row of 32-bit RGB in `fmt`'s byte order to B, G, R, X. */
static void row_to_bgrx(uint32_t fmt, const uint8_t *s, uint8_t *d, int w) {
    switch (fmt) {
    case SPA_VIDEO_FORMAT_BGRx:
    case SPA_VIDEO_FORMAT_BGRA: memcpy(d, s, (size_t)w * 4); return;
    case SPA_VIDEO_FORMAT_RGBx:
    case SPA_VIDEO_FORMAT_RGBA:
        for (int i = 0; i < w; i++, s += 4, d += 4) d[0] = s[2], d[1] = s[1], d[2] = s[0], d[3] = 255;
        return;
    case SPA_VIDEO_FORMAT_xRGB:
    case SPA_VIDEO_FORMAT_ARGB:
        for (int i = 0; i < w; i++, s += 4, d += 4) d[0] = s[3], d[1] = s[2], d[2] = s[1], d[3] = 255;
        return;
    case SPA_VIDEO_FORMAT_xBGR:
    case SPA_VIDEO_FORMAT_ABGR:
        for (int i = 0; i < w; i++, s += 4, d += 4) d[0] = s[1], d[1] = s[2], d[2] = s[3], d[3] = 255;
        return;
    default: memset(d, 0, (size_t)w * 4);
    }
}

/* ---- format and parameters ---- */

static const struct spa_pod *enum_format(struct spa_pod_builder *b, int fps, const uint32_t *fmts, int nfmts,
                                         int with_mods) {
    struct spa_pod_frame f[2];
    struct spa_rectangle def = SPA_RECTANGLE(1920, 1080), min = SPA_RECTANGLE(1, 1), max = SPA_RECTANGLE(16384, 16384);
    struct spa_fraction fr = SPA_FRACTION((uint32_t)fps, 1), frmin = SPA_FRACTION(0, 1), frmax = SPA_FRACTION(360, 1);
    spa_pod_builder_push_object(b, &f[0], SPA_TYPE_OBJECT_Format, SPA_PARAM_EnumFormat);
    spa_pod_builder_add(b, SPA_FORMAT_mediaType, SPA_POD_Id(SPA_MEDIA_TYPE_video), 0);
    spa_pod_builder_add(b, SPA_FORMAT_mediaSubtype, SPA_POD_Id(SPA_MEDIA_SUBTYPE_raw), 0);
    spa_pod_builder_prop(b, SPA_FORMAT_VIDEO_format, 0);
    if (nfmts == 1) {
        spa_pod_builder_id(b, fmts[0]);
    } else {
        spa_pod_builder_push_choice(b, &f[1], SPA_CHOICE_Enum, 0);
        spa_pod_builder_id(b, fmts[0]);
        for (int i = 0; i < nfmts; i++) spa_pod_builder_id(b, fmts[i]);
        spa_pod_builder_pop(b, &f[1]);
    }
    if (with_mods) {
        spa_pod_builder_prop(b, SPA_FORMAT_VIDEO_modifier, SPA_POD_PROP_FLAG_MANDATORY);
        spa_pod_builder_push_choice(b, &f[1], SPA_CHOICE_Enum, 0);
        spa_pod_builder_long(b, (int64_t)MOD_LINEAR);
        spa_pod_builder_long(b, (int64_t)MOD_LINEAR);
        spa_pod_builder_long(b, (int64_t)MOD_INVALID);
        spa_pod_builder_pop(b, &f[1]);
    }
    spa_pod_builder_add(b, SPA_FORMAT_VIDEO_size, SPA_POD_CHOICE_RANGE_Rectangle(&def, &min, &max), 0);
    spa_pod_builder_add(b, SPA_FORMAT_VIDEO_framerate, SPA_POD_CHOICE_RANGE_Fraction(&fr, &frmin, &frmax), 0);
    return spa_pod_builder_pop(b, &f[0]);
}

static void on_param_changed(void *data, uint32_t id, const struct spa_pod *param) {
    struct lm_video *v = data;
    if (!param || id != SPA_PARAM_Format) return;
    uint32_t mt, mst;
    if (spa_format_parse(param, &mt, &mst) < 0 || mt != SPA_MEDIA_TYPE_video || mst != SPA_MEDIA_SUBTYPE_raw) return;
    struct spa_video_info_raw info;
    spa_zero(info);
    if (spa_format_video_raw_parse(param, &info) < 0) return;
    v->w = (int)info.size.width;
    v->h = (int)info.size.height;
    v->spa_format = info.format;
    v->is_dmabuf = spa_pod_find_prop(param, NULL, SPA_FORMAT_VIDEO_modifier) != NULL;
    v->modifier = info.modifier;
    v->known = 1;
    v->damage_full = 1;

    uint8_t buf[1024];
    struct spa_pod_builder b = SPA_POD_BUILDER_INIT(buf, sizeof buf);
    const struct spa_pod *params[4];
    int n = 0;
    int types = v->is_dmabuf ? (1 << SPA_DATA_DmaBuf) : ((1 << SPA_DATA_MemFd) | (1 << SPA_DATA_MemPtr));
    params[n++] = spa_pod_builder_add_object(&b, SPA_TYPE_OBJECT_ParamBuffers, SPA_PARAM_Buffers,
                                             SPA_PARAM_BUFFERS_dataType, SPA_POD_CHOICE_FLAGS_Int(types));
    params[n++] = spa_pod_builder_add_object(&b, SPA_TYPE_OBJECT_ParamMeta, SPA_PARAM_Meta, SPA_PARAM_META_type,
                                             SPA_POD_Id(SPA_META_Header), SPA_PARAM_META_size,
                                             SPA_POD_Int(sizeof(struct spa_meta_header)));
    params[n++] = spa_pod_builder_add_object(
        &b, SPA_TYPE_OBJECT_ParamMeta, SPA_PARAM_Meta, SPA_PARAM_META_type, SPA_POD_Id(SPA_META_Cursor),
        SPA_PARAM_META_size,
        SPA_POD_CHOICE_RANGE_Int(CURSOR_META_SIZE(64, 64), CURSOR_META_SIZE(1, 1),
                                 CURSOR_META_SIZE(1024, 1024)));
    params[n++] = spa_pod_builder_add_object(
        &b, SPA_TYPE_OBJECT_ParamMeta, SPA_PARAM_Meta, SPA_PARAM_META_type, SPA_POD_Id(SPA_META_VideoDamage),
        SPA_PARAM_META_size,
        SPA_POD_CHOICE_RANGE_Int(sizeof(struct spa_meta_region) * 16, sizeof(struct spa_meta_region) * 1,
                                 sizeof(struct spa_meta_region) * 16));
    pw_stream_update_params(v->stream, params, (uint32_t)n);
    pw_thread_loop_signal(v->loop, false);
}

static void on_state_changed(void *data, enum pw_stream_state old, enum pw_stream_state state, const char *error) {
    struct lm_video *v = data;
    (void)old;
    (void)error;
    if (state == PW_STREAM_STATE_STREAMING) v->streaming = 1;
    /* The node went away (sharing stopped) or the stream broke. */
    if (state == PW_STREAM_STATE_ERROR || (state == PW_STREAM_STATE_UNCONNECTED && v->streaming)) {
        v->ended = 1;
        pw_thread_loop_signal(v->loop, false);
    }
}

/* ---- buffers ---- */

static void add_damage(struct lm_video *v, struct spa_buffer *buf) {
    struct spa_meta *m = spa_buffer_find_meta(buf, SPA_META_VideoDamage);
    if (!m) {
        v->damage_full = 1;
        return;
    }
    struct spa_meta_region *r;
    int any = 0;
    spa_meta_for_each(r, m) {
        if (!spa_meta_region_is_valid(r)) break;
        any = 1;
        if (v->damage_n >= LM_MAX_DAMAGE) {
            v->damage_full = 1;
            break;
        }
        lm_rect *d = &v->damage[v->damage_n++];
        d->x = r->region.position.x;
        d->y = r->region.position.y;
        d->w = (int32_t)r->region.size.width;
        d->h = (int32_t)r->region.size.height;
    }
    /* A damage meta with no region says nothing: take it as the whole picture. */
    if (!any) v->damage_full = 1;
}

static void read_cursor(struct lm_video *v, struct spa_buffer *buf) {
    struct spa_meta_cursor *mc = spa_buffer_find_meta_data(buf, SPA_META_Cursor, sizeof *mc);
    if (!mc) return;
    int valid = spa_meta_cursor_is_valid(mc);
    if (valid && mc->bitmap_offset >= sizeof *mc) {
        struct spa_meta_bitmap *bm = SPA_PTROFF(mc, mc->bitmap_offset, struct spa_meta_bitmap);
        int w = (int)bm->size.width, h = (int)bm->size.height;
        if (w <= 0 || h <= 0 || bm->format == SPA_VIDEO_FORMAT_UNKNOWN) {
            /* An empty bitmap: the pointer is hidden. */
            if (!v->shape_empty) v->cursor_new = 1;
            v->shape_empty = 1;
        } else if (w <= LM_CURSOR_MAX && h <= LM_CURSOR_MAX) {
            const uint8_t *px = SPA_PTROFF(bm, bm->offset, const uint8_t);
            int stride = bm->stride ? bm->stride : w * 4;
            /* Compositors hand premultiplied RGBA (mutter, KWin): only the byte order changes. */
            for (int y = 0; y < h; y++) row_to_bgrx(bm->format, px + (size_t)y * stride, v->shape + (size_t)y * w * 4, w);
            int has_alpha = bm->format == SPA_VIDEO_FORMAT_RGBA || bm->format == SPA_VIDEO_FORMAT_BGRA ||
                            bm->format == SPA_VIDEO_FORMAT_ARGB || bm->format == SPA_VIDEO_FORMAT_ABGR;
            if (has_alpha) {
                int ai = bm->format == SPA_VIDEO_FORMAT_ARGB || bm->format == SPA_VIDEO_FORMAT_ABGR ? 0 : 3;
                for (int y = 0; y < h; y++)
                    for (int x = 0; x < w; x++)
                        v->shape[((size_t)y * w + x) * 4 + 3] = px[(size_t)y * stride + (size_t)x * 4 + ai];
            }
            v->shape_w = w;
            v->shape_h = h;
            v->hot_x = mc->hotspot.x;
            v->hot_y = mc->hotspot.y;
            v->shape_new = 1;
            if (v->shape_empty) v->cursor_new = 1;
            v->shape_empty = 0;
        }
    }
    if (valid != v->cursor_valid || (valid && (mc->position.x != v->cx || mc->position.y != v->cy))) v->cursor_new = 1;
    v->cursor_valid = valid;
    if (valid) {
        v->cx = mc->position.x;
        v->cy = mc->position.y;
    }
}

static int has_picture(struct spa_buffer *buf) {
    struct spa_meta_header *hdr = spa_buffer_find_meta_data(buf, SPA_META_Header, sizeof *hdr);
    if (hdr && (hdr->flags & SPA_META_HEADER_FLAG_CORRUPTED)) return 0;
    if (buf->n_datas < 1) return 0;
    struct spa_data *d = &buf->datas[0];
    if (!d->chunk || d->chunk->size == 0 || (d->chunk->flags & SPA_CHUNK_FLAG_CORRUPTED)) return 0;
    return d->type == SPA_DATA_DmaBuf || d->data != NULL;
}

static void on_process(void *data) {
    struct lm_video *v = data;
    struct pw_buffer *b, *newest = NULL;
    int news = 0;
    while ((b = pw_stream_dequeue_buffer(v->stream))) {
        if (v->debug && v->debug_buffers < 5) {
            v->debug_buffers++;
            struct spa_meta_cursor *mc = spa_buffer_find_meta_data(b->buffer, SPA_META_Cursor, sizeof *mc);
            fprintf(stderr, "lm_video: buffer: %u datas, type %u, chunk %u, cursor meta %s (id %u, %d,%d, bitmap at %u), damage meta %s\n",
                    b->buffer->n_datas, b->buffer->datas[0].type, b->buffer->datas[0].chunk ? b->buffer->datas[0].chunk->size : 0,
                    mc ? "yes" : "no", mc ? mc->id : 0, mc ? mc->position.x : 0, mc ? mc->position.y : 0,
                    mc ? mc->bitmap_offset : 0, spa_buffer_find_meta(b->buffer, SPA_META_VideoDamage) ? "yes" : "no");
        }
        int cn = v->cursor_new, sn = v->shape_new;
        read_cursor(v, b->buffer);
        news |= v->cursor_new != cn || v->shape_new != sn;
        if (has_picture(b->buffer)) {
            add_damage(v, b->buffer);
            if (newest) pw_stream_queue_buffer(v->stream, newest);
            newest = b;
        } else {
            pw_stream_queue_buffer(v->stream, b);
        }
    }
    if (newest) {
        /* Nobody took the last one: it goes back, its damage stays in the sum. */
        if (v->pending) pw_stream_queue_buffer(v->stream, v->pending);
        v->pending = newest;
        news = 1;
    }
    if (news) pw_thread_loop_signal(v->loop, false);
}

static void on_remove_buffer(void *data, struct pw_buffer *b) {
    struct lm_video *v = data;
    if (v->pending == b) v->pending = NULL;
}

static const struct pw_stream_events stream_events = {
    PW_VERSION_STREAM_EVENTS,
    .remove_buffer = on_remove_buffer,
    .state_changed = on_state_changed,
    .param_changed = on_param_changed,
    .process = on_process,
};

static void on_core_error(void *data, uint32_t id, int seq, int res, const char *message) {
    struct lm_video *v = data;
    (void)seq;
    (void)message;
    if (id == PW_ID_CORE && res == -EPIPE) {
        v->ended = 1;
        pw_thread_loop_signal(v->loop, false);
    }
}

static const struct pw_core_events core_events = {
    PW_VERSION_CORE_EVENTS,
    .error = on_core_error,
};

/* ---- the API ---- */

lm_video *lm_video_open(int pw_fd, uint32_t node, int fps, int dmabuf, int *err) {
    pw_init(NULL, NULL);
    struct lm_video *v = calloc(1, sizeof *v);
    if (!v) {
        close(pw_fd);
        *err = -ENOMEM;
        return NULL;
    }
    v->fps = fps > 0 ? fps : 60;
    v->debug = getenv("SODIUM_REMOTE_HOST_DEBUG") != NULL;
    v->want_dmabuf = dmabuf;
    v->damage_full = 1;
    v->shape_empty = 1;
    v->loop = pw_thread_loop_new("sr-video", NULL);
    if (!v->loop || pw_thread_loop_start(v->loop) < 0) {
        close(pw_fd);
        *err = -EIO;
        lm_video_close(v);
        return NULL;
    }
    pw_thread_loop_lock(v->loop);
    v->ctx = pw_context_new(pw_thread_loop_get_loop(v->loop), NULL, 0);
    if (!v->ctx) {
        close(pw_fd);
        goto fail;
    }
    v->core = pw_context_connect_fd(v->ctx, pw_fd, NULL, 0);
    if (!v->core) goto fail;
    pw_core_add_listener(v->core, &v->core_listener, &core_events, v);
    struct pw_properties *props = pw_properties_new(PW_KEY_MEDIA_TYPE, "Video", PW_KEY_MEDIA_CATEGORY, "Capture",
                                                    PW_KEY_MEDIA_ROLE, "Screen", NULL);
    v->stream = pw_stream_new(v->core, "sodium-remote-host", props);
    if (!v->stream) goto fail;
    pw_stream_add_listener(v->stream, &v->stream_listener, &stream_events, v);

    uint8_t buf[4096];
    struct spa_pod_builder b = SPA_POD_BUILDER_INIT(buf, sizeof buf);
    const struct spa_pod *params[8];
    int n = 0;
    if (dmabuf) {
        static const uint32_t dfmts[] = {SPA_VIDEO_FORMAT_BGRx, SPA_VIDEO_FORMAT_BGRA, SPA_VIDEO_FORMAT_RGBx,
                                         SPA_VIDEO_FORMAT_RGBA};
        for (int i = 0; i < 4; i++) params[n++] = enum_format(&b, v->fps, &dfmts[i], 1, 1);
    }
    static const uint32_t fmts[] = {SPA_VIDEO_FORMAT_BGRx, SPA_VIDEO_FORMAT_BGRA, SPA_VIDEO_FORMAT_RGBx,
                                    SPA_VIDEO_FORMAT_RGBA, SPA_VIDEO_FORMAT_xRGB, SPA_VIDEO_FORMAT_ARGB,
                                    SPA_VIDEO_FORMAT_xBGR, SPA_VIDEO_FORMAT_ABGR};
    params[n++] = enum_format(&b, v->fps, fmts, 8, 0);
    int r = pw_stream_connect(v->stream, PW_DIRECTION_INPUT, node,
                              PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS, params, (uint32_t)n);
    if (r < 0) {
        pw_thread_loop_unlock(v->loop);
        *err = r;
        lm_video_close(v);
        return NULL;
    }
    pw_thread_loop_unlock(v->loop);
    *err = 0;
    return v;
fail:
    pw_thread_loop_unlock(v->loop);
    *err = errno ? -errno : -EIO;
    lm_video_close(v);
    return NULL;
}

int lm_video_size(lm_video *v, int *w, int *h, int *dmabuf) {
    pw_thread_loop_lock(v->loop);
    int r = v->ended ? -EPIPE : v->known;
    *w = v->w;
    *h = v->h;
    *dmabuf = v->is_dmabuf;
    pw_thread_loop_unlock(v->loop);
    return r;
}

int lm_video_next(lm_video *v, int timeout_ms, uint8_t *dst, int dst_cap, uint8_t *shape, int shape_cap,
                  lm_frame *out) {
    memset(out, 0, sizeof *out);
    pw_thread_loop_lock(v->loop);
    struct timespec until;
    pw_thread_loop_get_time(v->loop, &until, (int64_t)timeout_ms * SPA_NSEC_PER_MSEC);
    while (!v->ended && !v->pending && !v->cursor_new && !v->shape_new) {
        if (pw_thread_loop_timed_wait_full(v->loop, &until) != 0) break;
    }
    if (v->ended && !v->pending) {
        pw_thread_loop_unlock(v->loop);
        return -EPIPE;
    }
    int got = 0;
    if (v->pending) {
        struct pw_buffer *b = v->pending;
        struct spa_buffer *sb = b->buffer;
        out->width = v->w;
        out->height = v->h;
        if (v->is_dmabuf) {
            out->dmabuf = 1;
            out->drm_format = drm_fourcc(v->spa_format);
            out->modifier = v->modifier;
            out->planes = (int)SPA_MIN(sb->n_datas, (uint32_t)LM_MAX_PLANES);
            for (int i = 0; i < out->planes; i++) {
                out->fd[i] = (int)sb->datas[i].fd;
                out->offset[i] = sb->datas[i].chunk->offset;
                out->stride[i] = (uint32_t)sb->datas[i].chunk->stride;
            }
            out->buffer = b;
        } else {
            size_t need = (size_t)v->w * v->h * 4;
            if (!dst || (size_t)dst_cap < need) {
                pw_thread_loop_unlock(v->loop);
                return -ENOSPC;
            }
            struct spa_data *d = &sb->datas[0];
            uint32_t off = SPA_MIN(d->chunk->offset, d->maxsize);
            int stride = d->chunk->stride > 0 ? d->chunk->stride : v->w * 4;
            const uint8_t *src = SPA_PTROFF(d->data, off, const uint8_t);
            size_t avail = d->maxsize - off;
            for (int y = 0; y < v->h; y++) {
                if ((size_t)y * stride + (size_t)v->w * 4 > avail) break;
                row_to_bgrx(v->spa_format, src + (size_t)y * stride, dst + (size_t)y * v->w * 4, v->w);
            }
            pw_stream_queue_buffer(v->stream, b);
        }
        v->pending = NULL;
        out->frame = 1;
        out->damage_n = v->damage_full ? 0 : v->damage_n;
        memcpy(out->damage, v->damage, sizeof(lm_rect) * (size_t)out->damage_n);
        v->damage_full = 0;
        v->damage_n = 0;
        got = 1;
    }
    if (v->cursor_new) {
        out->cursor = 1;
        out->cursor_visible = v->cursor_valid && !v->shape_empty;
        out->cursor_x = v->cx;
        out->cursor_y = v->cy;
        v->cursor_new = 0;
        got = 1;
    }
    if (v->shape_new && shape && shape_cap >= v->shape_w * v->shape_h * 4) {
        out->shape = 1;
        out->shape_w = v->shape_w;
        out->shape_h = v->shape_h;
        out->hot_x = v->hot_x;
        out->hot_y = v->hot_y;
        memcpy(shape, v->shape, (size_t)v->shape_w * v->shape_h * 4);
        v->shape_new = 0;
        got = 1;
    }
    pw_thread_loop_unlock(v->loop);
    return got;
}

void lm_video_release(lm_video *v, void *buffer) {
    if (!v || !buffer) return;
    pw_thread_loop_lock(v->loop);
    if (v->stream) pw_stream_queue_buffer(v->stream, buffer);
    pw_thread_loop_unlock(v->loop);
}

void lm_video_close(lm_video *v) {
    if (!v) return;
    if (v->loop) {
        pw_thread_loop_lock(v->loop);
        if (v->stream) {
            spa_hook_remove(&v->stream_listener);
            pw_stream_destroy(v->stream);
            v->stream = NULL;
        }
        if (v->core) {
            spa_hook_remove(&v->core_listener);
            pw_core_disconnect(v->core);
        }
        if (v->ctx) pw_context_destroy(v->ctx);
        pw_thread_loop_unlock(v->loop);
        pw_thread_loop_stop(v->loop);
        pw_thread_loop_destroy(v->loop);
    }
    free(v);
}
