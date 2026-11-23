// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
/* See audio_out.h. C for the same reason as remoted's audio shim: SPA pods are
 * built by inline functions and macros, and bindgen would need libclang. */
#include "audio_out.h"

#include <errno.h>
#include <stdlib.h>
#include <string.h>

#include <pipewire/pipewire.h>
#include <spa/param/audio/format-utils.h>

#define RATE 48000
#define CHANNELS 2

struct ao_stream {
    struct pw_thread_loop *loop;
    struct pw_stream *stream;
    struct spa_hook listener;
    ao_fill_fn fill;
    void *user;
};

static void on_process(void *data) {
    ao_stream *s = data;
    struct pw_buffer *b = pw_stream_dequeue_buffer(s->stream);
    if (!b)
        return;
    struct spa_buffer *buf = b->buffer;
    float *dst = buf->datas[0].data;
    if (!dst)
        return;
    const int stride = sizeof(float) * CHANNELS;
    uint32_t frames = buf->datas[0].maxsize / stride;
    if (b->requested && b->requested < frames)
        frames = b->requested;
    s->fill(s->user, dst, (int)frames);
    buf->datas[0].chunk->offset = 0;
    buf->datas[0].chunk->stride = stride;
    buf->datas[0].chunk->size = frames * stride;
    pw_stream_queue_buffer(s->stream, b);
}

static const struct pw_stream_events stream_events = {
    PW_VERSION_STREAM_EVENTS,
    .process = on_process,
};

ao_stream *ao_open(const char *app_name, const char *app_id, const char *icon, ao_fill_fn fill, void *user, int *err) {
    pw_init(NULL, NULL);
    ao_stream *s = calloc(1, sizeof *s);
    if (!s) {
        *err = -ENOMEM;
        return NULL;
    }
    s->fill = fill;
    s->user = user;
    s->loop = pw_thread_loop_new("remote-audio", NULL);
    if (!s->loop) {
        *err = -errno;
        free(s);
        return NULL;
    }
    struct pw_properties *props = pw_properties_new(
        PW_KEY_MEDIA_TYPE, "Audio", PW_KEY_MEDIA_CATEGORY, "Playback", PW_KEY_MEDIA_ROLE, "Game",
        PW_KEY_APP_NAME, app_name, PW_KEY_APP_ID, app_id, PW_KEY_APP_ICON_NAME, icon,
        PW_KEY_NODE_NAME, "sodium-remote", PW_KEY_NODE_LATENCY, "480/48000", NULL);
    s->stream = pw_stream_new_simple(pw_thread_loop_get_loop(s->loop), app_name, props, &stream_events, s);
    if (!s->stream) {
        *err = -errno;
        pw_thread_loop_destroy(s->loop);
        free(s);
        return NULL;
    }
    uint8_t pod[1024];
    struct spa_pod_builder b = SPA_POD_BUILDER_INIT(pod, sizeof pod);
    struct spa_audio_info_raw info = SPA_AUDIO_INFO_RAW_INIT(.format = SPA_AUDIO_FORMAT_F32, .rate = RATE, .channels = CHANNELS,
                                                             .position = {SPA_AUDIO_CHANNEL_FL, SPA_AUDIO_CHANNEL_FR});
    const struct spa_pod *params[1] = {spa_format_audio_raw_build(&b, SPA_PARAM_EnumFormat, &info)};
    int r = pw_stream_connect(s->stream, PW_DIRECTION_OUTPUT, PW_ID_ANY,
                              PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS, params, 1);
    if (r < 0 || pw_thread_loop_start(s->loop) < 0) {
        *err = r < 0 ? r : -EIO;
        pw_stream_destroy(s->stream);
        pw_thread_loop_destroy(s->loop);
        free(s);
        return NULL;
    }
    *err = 0;
    return s;
}

void ao_close(ao_stream *s) {
    if (!s)
        return;
    pw_thread_loop_lock(s->loop);
    pw_stream_disconnect(s->stream);
    pw_stream_destroy(s->stream);
    pw_thread_loop_unlock(s->loop);
    pw_thread_loop_stop(s->loop);
    pw_thread_loop_destroy(s->loop);
    free(s);
}
