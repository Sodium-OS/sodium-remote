// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
/* See audio_shim.h. */
#include "audio_shim.h"

#include <errno.h>
#include <poll.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>
#include <sys/eventfd.h>
#include <unistd.h>

#include <opus.h>
#include <pipewire/extensions/metadata.h>
#include <pipewire/pipewire.h>
#include <spa/param/audio/format-utils.h>
#include <spa/param/props.h>
#include <spa/pod/builder.h>
#include <spa/pod/iter.h>

#define RATE 48000
#define FRAME 480 /* 10 ms */
#define MAX_NODES 1024
#define MAX_META 64

/* Single-producer single-consumer float ring. */
struct ring {
    float *buf;
    size_t cap;
    _Atomic size_t w, r;
};

static int ring_init(struct ring *g, size_t cap) {
    g->buf = calloc(cap, sizeof(float));
    g->cap = cap;
    atomic_store(&g->w, 0);
    atomic_store(&g->r, 0);
    return g->buf ? 0 : -ENOMEM;
}
static size_t ring_len(struct ring *g) { return atomic_load(&g->w) - atomic_load(&g->r); }
/* Drops what doesn't fit (the reader fell behind). */
static void ring_push(struct ring *g, const float *s, size_t n) {
    size_t w = atomic_load(&g->w), room = g->cap - (w - atomic_load(&g->r));
    if (n > room) n = room;
    for (size_t i = 0; i < n; i++) g->buf[(w + i) % g->cap] = s[i];
    atomic_store(&g->w, w + n);
}
static size_t ring_pop(struct ring *g, float *d, size_t n) {
    size_t r = atomic_load(&g->r), have = atomic_load(&g->w) - r;
    if (n > have) n = have;
    for (size_t i = 0; i < n; i++) d[i] = g->buf[(r + i) % g->cap];
    atomic_store(&g->r, r + n);
    return n;
}
static void ring_free(struct ring *g) {
    free(g->buf);
    g->buf = NULL;
}

struct node_ent {
    uint32_t id;
    char name[256];
};
struct meta_ent {
    int used;
    char key[128];
    char value[1024];
};

struct ra_audio {
    struct pw_thread_loop *loop;
    struct pw_context *ctx;
    struct pw_core *core;
    struct spa_hook core_listener;
    struct pw_registry *registry;
    struct spa_hook registry_listener;
    int done_seq;
    int error;

    struct node_ent nodes[MAX_NODES];
    int n_nodes;

    struct pw_metadata *meta;
    uint32_t meta_id;
    struct spa_hook meta_listener;
    struct meta_ent metas[MAX_META];

    /* a node param query */
    float q_vols[SPA_AUDIO_MAX_CHANNELS];
    int q_n, q_mute;

    struct pw_stream *sink;
    struct spa_hook sink_listener;
    struct ring sink_ring;
    int sink_efd;
    _Atomic int sink_running;
    OpusEncoder *enc;

    struct pw_stream *mon;
    struct spa_hook mon_listener;
    struct ring mon_ring;
    int mon_efd;
    _Atomic int mon_running;

    struct pw_stream *mic;
    struct spa_hook mic_listener;
    struct ring mic_ring;
    OpusDecoder *dec;
};

/* ---- core and registry ---- */

static void on_core_done(void *data, uint32_t id, int seq) {
    struct ra_audio *ra = data;
    if (id == PW_ID_CORE) {
        ra->done_seq = seq;
        pw_thread_loop_signal(ra->loop, false);
    }
}

static void on_core_error(void *data, uint32_t id, int seq, int res, const char *message) {
    struct ra_audio *ra = data;
    (void)seq;
    (void)message;
    if (id == PW_ID_CORE) {
        ra->error = res;
        pw_thread_loop_signal(ra->loop, false);
    }
}

static const struct pw_core_events core_events = {
    PW_VERSION_CORE_EVENTS,
    .done = on_core_done,
    .error = on_core_error,
};

/* With the loop locked: waits until the server has handled everything sent so far. */
static int roundtrip(struct ra_audio *ra) {
    int seq = pw_core_sync(ra->core, PW_ID_CORE, 0);
    while (ra->done_seq != seq) {
        if (ra->error < 0) return ra->error;
        if (pw_thread_loop_timed_wait(ra->loop, 2) != 0) return -ETIMEDOUT;
    }
    return 0;
}

static int on_meta_property(void *data, uint32_t subject, const char *key, const char *type, const char *value) {
    struct ra_audio *ra = data;
    (void)type;
    if (subject != 0) return 0;
    if (key == NULL) {
        for (int i = 0; i < MAX_META; i++) ra->metas[i].used = 0;
        return 0;
    }
    int free_slot = -1;
    for (int i = 0; i < MAX_META; i++) {
        if (ra->metas[i].used && strcmp(ra->metas[i].key, key) == 0) {
            if (value) snprintf(ra->metas[i].value, sizeof ra->metas[i].value, "%s", value);
            else ra->metas[i].used = 0;
            return 0;
        }
        if (!ra->metas[i].used && free_slot < 0) free_slot = i;
    }
    if (value && free_slot >= 0) {
        struct meta_ent *m = &ra->metas[free_slot];
        m->used = 1;
        snprintf(m->key, sizeof m->key, "%s", key);
        snprintf(m->value, sizeof m->value, "%s", value);
    }
    return 0;
}

static const struct pw_metadata_events meta_events = {
    PW_VERSION_METADATA_EVENTS,
    .property = on_meta_property,
};

static void on_global(void *data, uint32_t id, uint32_t permissions, const char *type, uint32_t version,
                      const struct spa_dict *props) {
    struct ra_audio *ra = data;
    (void)permissions;
    (void)version;
    if (strcmp(type, PW_TYPE_INTERFACE_Node) == 0) {
        const char *name = props ? spa_dict_lookup(props, PW_KEY_NODE_NAME) : NULL;
        if (!name || ra->n_nodes >= MAX_NODES) return;
        struct node_ent *n = &ra->nodes[ra->n_nodes++];
        n->id = id;
        snprintf(n->name, sizeof n->name, "%s", name);
    } else if (strcmp(type, PW_TYPE_INTERFACE_Metadata) == 0 && !ra->meta) {
        const char *name = props ? spa_dict_lookup(props, PW_KEY_METADATA_NAME) : NULL;
        if (!name || strcmp(name, "default") != 0) return;
        ra->meta = pw_registry_bind(ra->registry, id, type, PW_VERSION_METADATA, 0);
        ra->meta_id = id;
        pw_metadata_add_listener(ra->meta, &ra->meta_listener, &meta_events, ra);
    }
}

static void on_global_remove(void *data, uint32_t id) {
    struct ra_audio *ra = data;
    for (int i = 0; i < ra->n_nodes; i++) {
        if (ra->nodes[i].id == id) {
            ra->nodes[i] = ra->nodes[--ra->n_nodes];
            break;
        }
    }
    if (ra->meta && ra->meta_id == id) {
        spa_hook_remove(&ra->meta_listener);
        pw_proxy_destroy((struct pw_proxy *)ra->meta);
        ra->meta = NULL;
        for (int i = 0; i < MAX_META; i++) ra->metas[i].used = 0;
    }
}

static const struct pw_registry_events registry_events = {
    PW_VERSION_REGISTRY_EVENTS,
    .global = on_global,
    .global_remove = on_global_remove,
};

ra_audio *ra_open(int *err) {
    pw_init(NULL, NULL);
    struct ra_audio *ra = calloc(1, sizeof *ra);
    if (!ra) {
        *err = -ENOMEM;
        return NULL;
    }
    ra->sink_efd = -1;
    ra->mon_efd = -1;
    ra->done_seq = -1;
    ra->loop = pw_thread_loop_new("remoted-audio", NULL);
    if (!ra->loop || pw_thread_loop_start(ra->loop) < 0) {
        *err = -EIO;
        free(ra);
        return NULL;
    }
    pw_thread_loop_lock(ra->loop);
    ra->ctx = pw_context_new(pw_thread_loop_get_loop(ra->loop), NULL, 0);
    ra->core = ra->ctx ? pw_context_connect(ra->ctx, NULL, 0) : NULL;
    if (!ra->core) {
        *err = errno ? -errno : -ECONNREFUSED;
        pw_thread_loop_unlock(ra->loop);
        ra_close(ra);
        return NULL;
    }
    pw_core_add_listener(ra->core, &ra->core_listener, &core_events, ra);
    ra->registry = pw_core_get_registry(ra->core, PW_VERSION_REGISTRY, 0);
    pw_registry_add_listener(ra->registry, &ra->registry_listener, &registry_events, ra);
    /* Twice: the first fills the registry and binds the metadata, the second gets its properties. */
    int r = roundtrip(ra);
    if (r == 0) r = roundtrip(ra);
    pw_thread_loop_unlock(ra->loop);
    if (r < 0) {
        *err = r;
        ra_close(ra);
        return NULL;
    }
    *err = 0;
    return ra;
}

int ra_connected(ra_audio *ra) {
    pw_thread_loop_lock(ra->loop);
    int up = ra->core && ra->error >= 0;
    pw_thread_loop_unlock(ra->loop);
    return up;
}

void ra_close(ra_audio *ra) {
    if (!ra) return;
    ra_sink_stop(ra);
    ra_sink_free(ra);
    ra_mic_stop(ra);
    ra_monitor_stop(ra);
    ring_free(&ra->mon_ring);
    if (ra->mon_efd >= 0) close(ra->mon_efd);
    if (ra->loop) {
        pw_thread_loop_lock(ra->loop);
        if (ra->meta) {
            spa_hook_remove(&ra->meta_listener);
            pw_proxy_destroy((struct pw_proxy *)ra->meta);
        }
        if (ra->registry) {
            spa_hook_remove(&ra->registry_listener);
            pw_proxy_destroy((struct pw_proxy *)ra->registry);
        }
        if (ra->core) {
            spa_hook_remove(&ra->core_listener);
            pw_core_disconnect(ra->core);
        }
        if (ra->ctx) pw_context_destroy(ra->ctx);
        pw_thread_loop_unlock(ra->loop);
        pw_thread_loop_stop(ra->loop);
        pw_thread_loop_destroy(ra->loop);
    }
    free(ra);
}

/* ---- metadata ---- */

int ra_meta_get(ra_audio *ra, const char *key, char *out, int cap) {
    int len = 0;
    pw_thread_loop_lock(ra->loop);
    int r = roundtrip(ra);
    if (r == 0 && !ra->meta) r = -ENOENT;
    for (int i = 0; r == 0 && i < MAX_META; i++) {
        if (ra->metas[i].used && strcmp(ra->metas[i].key, key) == 0) {
            len = (int)strlen(ra->metas[i].value);
            if (len >= cap) r = -ENOSPC;
            else memcpy(out, ra->metas[i].value, (size_t)len + 1);
            break;
        }
    }
    pw_thread_loop_unlock(ra->loop);
    return r < 0 ? r : len;
}

int ra_meta_set(ra_audio *ra, const char *key, const char *json) {
    pw_thread_loop_lock(ra->loop);
    int r = ra->meta ? 0 : -ENOENT;
    if (r == 0) {
        pw_metadata_set_property(ra->meta, 0, key, json ? "Spa:String:JSON" : NULL, json);
        r = roundtrip(ra);
    }
    pw_thread_loop_unlock(ra->loop);
    return r;
}

/* ---- node volumes ---- */

static int find_node(struct ra_audio *ra, const char *name, uint32_t *id) {
    for (int i = 0; i < ra->n_nodes; i++) {
        if (strcmp(ra->nodes[i].name, name) == 0) {
            *id = ra->nodes[i].id;
            return 0;
        }
    }
    return -ENOENT;
}

int ra_node_exists(ra_audio *ra, const char *node_name) {
    uint32_t id;
    pw_thread_loop_lock(ra->loop);
    roundtrip(ra);
    int r = find_node(ra, node_name, &id) == 0;
    pw_thread_loop_unlock(ra->loop);
    return r;
}

static void on_node_param(void *data, int seq, uint32_t id, uint32_t index, uint32_t next, const struct spa_pod *param) {
    struct ra_audio *ra = data;
    (void)seq;
    (void)index;
    (void)next;
    if (id != SPA_PARAM_Props || !spa_pod_is_object(param) || ra->q_n > 0) return;
    const struct spa_pod_prop *prop;
    const struct spa_pod_object *obj = (const struct spa_pod_object *)param;
    int n = 0, mute = -1;
    SPA_POD_OBJECT_FOREACH(obj, prop) {
        if (prop->key == SPA_PROP_channelVolumes) {
            n = (int)spa_pod_copy_array(&prop->value, SPA_TYPE_Float, ra->q_vols, SPA_AUDIO_MAX_CHANNELS);
        } else if (prop->key == SPA_PROP_mute) {
            bool m = false;
            if (spa_pod_get_bool(&prop->value, &m) == 0) mute = m;
        }
    }
    if (n > 0) {
        ra->q_n = n;
        ra->q_mute = mute < 0 ? 0 : mute;
    }
}

static const struct pw_node_events node_events = {
    PW_VERSION_NODE_EVENTS,
    .param = on_node_param,
};

int ra_node_volume_get(ra_audio *ra, const char *node_name, float *vols, int max, int *mute) {
    uint32_t id;
    pw_thread_loop_lock(ra->loop);
    int r = roundtrip(ra);
    if (r == 0) r = find_node(ra, node_name, &id);
    if (r == 0) {
        struct pw_node *node = pw_registry_bind(ra->registry, id, PW_TYPE_INTERFACE_Node, PW_VERSION_NODE, 0);
        struct spa_hook l;
        spa_zero(l);
        pw_node_add_listener(node, &l, &node_events, ra);
        ra->q_n = 0;
        pw_node_enum_params(node, 0, SPA_PARAM_Props, 0, UINT32_MAX, NULL);
        r = roundtrip(ra);
        spa_hook_remove(&l);
        pw_proxy_destroy((struct pw_proxy *)node);
        if (r == 0 && ra->q_n == 0) r = -ENODATA;
        if (r == 0) {
            int n = ra->q_n < max ? ra->q_n : max;
            memcpy(vols, ra->q_vols, (size_t)n * sizeof(float));
            *mute = ra->q_mute;
            r = n;
        }
    }
    pw_thread_loop_unlock(ra->loop);
    return r;
}

int ra_node_volume_set(ra_audio *ra, const char *node_name, const float *vols, int n, int mute) {
    uint32_t id;
    pw_thread_loop_lock(ra->loop);
    int r = roundtrip(ra);
    if (r == 0) r = find_node(ra, node_name, &id);
    if (r == 0) {
        struct pw_node *node = pw_registry_bind(ra->registry, id, PW_TYPE_INTERFACE_Node, PW_VERSION_NODE, 0);
        uint8_t buf[512];
        struct spa_pod_builder b = SPA_POD_BUILDER_INIT(buf, sizeof buf);
        struct spa_pod_frame f;
        spa_pod_builder_push_object(&b, &f, SPA_TYPE_OBJECT_Props, SPA_PARAM_Props);
        spa_pod_builder_prop(&b, SPA_PROP_channelVolumes, 0);
        spa_pod_builder_array(&b, sizeof(float), SPA_TYPE_Float, (uint32_t)n, vols);
        spa_pod_builder_prop(&b, SPA_PROP_mute, 0);
        spa_pod_builder_bool(&b, mute != 0);
        struct spa_pod *pod = spa_pod_builder_pop(&b, &f);
        pw_node_set_param(node, SPA_PARAM_Props, 0, pod);
        r = roundtrip(ra);
        pw_proxy_destroy((struct pw_proxy *)node);
    }
    pw_thread_loop_unlock(ra->loop);
    return r;
}

/* ---- the Remote sink ---- */

static void sink_process(void *data) {
    struct ra_audio *ra = data;
    struct pw_buffer *b = pw_stream_dequeue_buffer(ra->sink);
    if (!b) return;
    struct spa_data *d = &b->buffer->datas[0];
    if (d->data && d->chunk && d->chunk->size > 0) {
        uint32_t off = SPA_MIN(d->chunk->offset, d->maxsize);
        uint32_t size = SPA_MIN(d->chunk->size, d->maxsize - off);
        ring_push(&ra->sink_ring, SPA_PTROFF(d->data, off, const float), size / sizeof(float));
        uint64_t one = 1;
        (void)!write(ra->sink_efd, &one, sizeof one);
    }
    pw_stream_queue_buffer(ra->sink, b);
}

static const struct pw_stream_events sink_events = {
    PW_VERSION_STREAM_EVENTS,
    .process = sink_process,
};

static struct pw_stream *make_stream(struct ra_audio *ra, const char *cls, const char *name, const char *desc,
                                     int channels, struct spa_hook *hook, const struct pw_stream_events *ev,
                                     enum pw_direction dir) {
    struct pw_properties *props = pw_properties_new(PW_KEY_MEDIA_TYPE, "Audio", PW_KEY_NODE_NAME, name,
                                                    PW_KEY_NODE_DESCRIPTION, desc, PW_KEY_NODE_LATENCY, "480/48000",
                                                    NULL);
    if (cls) pw_properties_set(props, PW_KEY_MEDIA_CLASS, cls);
    else {
        /* The default sink's monitor: a capture stream WirePlumber links to it. */
        pw_properties_set(props, PW_KEY_MEDIA_CATEGORY, "Capture");
        pw_properties_set(props, PW_KEY_STREAM_CAPTURE_SINK, "true");
    }
    pw_properties_setf(props, PW_KEY_AUDIO_CHANNELS, "%d", channels);
    pw_properties_set(props, SPA_KEY_AUDIO_POSITION, channels == 2 ? "[ FL FR ]" : "[ MONO ]");
    struct pw_stream *s = pw_stream_new(ra->core, name, props);
    if (!s) return NULL;
    pw_stream_add_listener(s, hook, ev, ra);
    uint8_t buf[1024];
    struct spa_pod_builder b = SPA_POD_BUILDER_INIT(buf, sizeof buf);
    struct spa_audio_info_raw info = {.format = SPA_AUDIO_FORMAT_F32, .rate = RATE, .channels = (uint32_t)channels};
    if (channels == 2) {
        info.position[0] = SPA_AUDIO_CHANNEL_FL;
        info.position[1] = SPA_AUDIO_CHANNEL_FR;
    } else {
        info.position[0] = SPA_AUDIO_CHANNEL_MONO;
    }
    const struct spa_pod *params[1] = {spa_format_audio_raw_build(&b, SPA_PARAM_EnumFormat, &info)};
    /* The virtual devices wait to be linked; the monitor capture asks to be linked. */
    enum pw_stream_flags flags = PW_STREAM_FLAG_MAP_BUFFERS | PW_STREAM_FLAG_RT_PROCESS;
    if (!cls) flags |= PW_STREAM_FLAG_AUTOCONNECT;
    if (pw_stream_connect(s, dir, PW_ID_ANY, flags, params, 1) < 0) {
        pw_stream_destroy(s);
        return NULL;
    }
    return s;
}

int ra_sink_start(ra_audio *ra, const char *node_name, const char *description, int bitrate) {
    if (ra->sink) return -EBUSY;
    int oerr = 0;
    ra->enc = opus_encoder_create(RATE, 2, OPUS_APPLICATION_RESTRICTED_LOWDELAY, &oerr);
    if (!ra->enc) return -EINVAL;
    opus_encoder_ctl(ra->enc, OPUS_SET_BITRATE(bitrate));
    if (ring_init(&ra->sink_ring, RATE * 2 / 2) < 0) return -ENOMEM; /* 500 ms */
    ra->sink_efd = eventfd(0, EFD_CLOEXEC | EFD_NONBLOCK);
    atomic_store(&ra->sink_running, 1);
    pw_thread_loop_lock(ra->loop);
    ra->sink = make_stream(ra, "Audio/Sink", node_name, description, 2, &ra->sink_listener, &sink_events,
                           PW_DIRECTION_INPUT);
    int r = ra->sink ? roundtrip(ra) : -EIO;
    pw_thread_loop_unlock(ra->loop);
    return r;
}

int ra_sink_read(ra_audio *ra, uint8_t *out, int cap, int timeout_ms) {
    float pcm[FRAME * 2];
    while (ring_len(&ra->sink_ring) < FRAME * 2) {
        if (!atomic_load(&ra->sink_running)) return -EPIPE;
        struct pollfd p = {.fd = ra->sink_efd, .events = POLLIN};
        int n = poll(&p, 1, timeout_ms);
        if (n == 0) return 0;
        if (n < 0 && errno != EINTR) return -errno;
        uint64_t v;
        (void)!read(ra->sink_efd, &v, sizeof v);
    }
    ring_pop(&ra->sink_ring, pcm, FRAME * 2);
    int len = opus_encode_float(ra->enc, pcm, FRAME, out, cap);
    return len < 0 ? -EINVAL : len;
}

void ra_sink_stop(ra_audio *ra) {
    if (!ra || !ra->loop) return;
    pw_thread_loop_lock(ra->loop);
    if (ra->sink) {
        spa_hook_remove(&ra->sink_listener);
        pw_stream_destroy(ra->sink);
        ra->sink = NULL;
    }
    pw_thread_loop_unlock(ra->loop);
    atomic_store(&ra->sink_running, 0);
    if (ra->sink_efd >= 0) {
        uint64_t one = 1;
        (void)!write(ra->sink_efd, &one, sizeof one);
    }
}

void ra_sink_free(ra_audio *ra) {
    if (!ra) return;
    if (ra->enc) opus_encoder_destroy(ra->enc);
    ra->enc = NULL;
    ring_free(&ra->sink_ring);
    if (ra->sink_efd >= 0) close(ra->sink_efd);
    ra->sink_efd = -1;
}

/* ---- the Remote microphone ---- */

static void mic_process(void *data) {
    struct ra_audio *ra = data;
    struct pw_buffer *b = pw_stream_dequeue_buffer(ra->mic);
    if (!b) return;
    struct spa_data *d = &b->buffer->datas[0];
    if (d->data) {
        uint32_t n = d->maxsize / sizeof(float);
        if (b->requested && b->requested < n) n = (uint32_t)b->requested;
        float *dst = d->data;
        size_t got = ring_pop(&ra->mic_ring, dst, n);
        memset(dst + got, 0, (n - got) * sizeof(float));
        d->chunk->offset = 0;
        d->chunk->stride = sizeof(float);
        d->chunk->size = n * sizeof(float);
    }
    pw_stream_queue_buffer(ra->mic, b);
}

static const struct pw_stream_events mic_events = {
    PW_VERSION_STREAM_EVENTS,
    .process = mic_process,
};

int ra_mic_start(ra_audio *ra, const char *node_name, const char *description) {
    if (ra->mic) return -EBUSY;
    int oerr = 0;
    ra->dec = opus_decoder_create(RATE, 1, &oerr);
    if (!ra->dec) return -EINVAL;
    if (ring_init(&ra->mic_ring, RATE / 5) < 0) return -ENOMEM; /* 200 ms */
    pw_thread_loop_lock(ra->loop);
    ra->mic = make_stream(ra, "Audio/Source", node_name, description, 1, &ra->mic_listener, &mic_events,
                          PW_DIRECTION_OUTPUT);
    int r = ra->mic ? roundtrip(ra) : -EIO;
    pw_thread_loop_unlock(ra->loop);
    return r;
}

int ra_mic_write(ra_audio *ra, const uint8_t *opus, int len) {
    if (!ra->dec) return -EPIPE;
    float pcm[5760];
    int n = opus_decode_float(ra->dec, opus, len, pcm, 5760, 0);
    if (n < 0) return -EINVAL;
    ring_push(&ra->mic_ring, pcm, (size_t)n);
    return n;
}

void ra_mic_stop(ra_audio *ra) {
    if (!ra || !ra->loop) return;
    pw_thread_loop_lock(ra->loop);
    if (ra->mic) {
        spa_hook_remove(&ra->mic_listener);
        pw_stream_destroy(ra->mic);
        ra->mic = NULL;
    }
    pw_thread_loop_unlock(ra->loop);
    if (ra->dec) opus_decoder_destroy(ra->dec);
    ra->dec = NULL;
    ring_free(&ra->mic_ring);
}

/* ---- the default sink's monitor (generic Linux host) ---- */

static void mon_process(void *data) {
    struct ra_audio *ra = data;
    struct pw_buffer *b = pw_stream_dequeue_buffer(ra->mon);
    if (!b) return;
    struct spa_data *d = &b->buffer->datas[0];
    if (d->data && d->chunk && d->chunk->size > 0) {
        uint32_t off = SPA_MIN(d->chunk->offset, d->maxsize);
        uint32_t size = SPA_MIN(d->chunk->size, d->maxsize - off);
        ring_push(&ra->mon_ring, SPA_PTROFF(d->data, off, const float), size / sizeof(float));
        uint64_t one = 1;
        (void)!write(ra->mon_efd, &one, sizeof one);
    }
    pw_stream_queue_buffer(ra->mon, b);
}

static const struct pw_stream_events mon_events = {
    PW_VERSION_STREAM_EVENTS,
    .process = mon_process,
};

int ra_monitor_start(ra_audio *ra, const char *node_name) {
    if (ra->mon) return -EBUSY;
    ring_free(&ra->mon_ring);
    if (ra->mon_efd >= 0) close(ra->mon_efd);
    if (ring_init(&ra->mon_ring, RATE * 2 / 2) < 0) return -ENOMEM; /* 500 ms */
    ra->mon_efd = eventfd(0, EFD_CLOEXEC | EFD_NONBLOCK);
    atomic_store(&ra->mon_running, 1);
    pw_thread_loop_lock(ra->loop);
    ra->mon = make_stream(ra, NULL, node_name, node_name, 2, &ra->mon_listener, &mon_events, PW_DIRECTION_INPUT);
    int r = ra->mon ? roundtrip(ra) : -EIO;
    pw_thread_loop_unlock(ra->loop);
    return r;
}

int ra_monitor_read(ra_audio *ra, float *out, int frames, int timeout_ms) {
    size_t want = (size_t)frames * 2;
    while (ring_len(&ra->mon_ring) < want) {
        if (!atomic_load(&ra->mon_running)) return -EPIPE;
        struct pollfd p = {.fd = ra->mon_efd, .events = POLLIN};
        int n = poll(&p, 1, timeout_ms);
        if (n == 0) return 0;
        if (n < 0 && errno != EINTR) return -errno;
        uint64_t v;
        (void)!read(ra->mon_efd, &v, sizeof v);
    }
    ring_pop(&ra->mon_ring, out, want);
    return frames;
}

void ra_monitor_stop(ra_audio *ra) {
    if (!ra || !ra->loop) return;
    pw_thread_loop_lock(ra->loop);
    if (ra->mon) {
        spa_hook_remove(&ra->mon_listener);
        pw_stream_destroy(ra->mon);
        ra->mon = NULL;
    }
    pw_thread_loop_unlock(ra->loop);
    atomic_store(&ra->mon_running, 0);
    if (ra->mon_efd >= 0) {
        uint64_t one = 1;
        (void)!write(ra->mon_efd, &one, sizeof one);
    }
}
