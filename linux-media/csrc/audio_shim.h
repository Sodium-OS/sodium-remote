// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
/* The PipeWire audio shim shared by remoted and the
 * generic Linux host: PipeWire and Opus. Part of remote-linux-media.
 *
 * - The "Remote" sink: a PipeWire stream with media.class Audio/Sink (what
 *   module-loopback makes), so apps play into it like any output. Its mix comes
 *   out as 10 ms Opus packets (48 kHz stereo) from ra_sink_read.
 * - The "Remote microphone" source: a stream with media.class Audio/Source, fed by
 *   the client's Opus packets (48 kHz mono) through ra_mic_write.
 * - The "default" metadata (WirePlumber's default devices) and node volumes, read
 *   and written with their exact values, so the previous default and its volume
 *   come back untouched.
 *
 * Every function but ra_sink_read locks the PipeWire thread loop itself. Returns:
 * 0 or a count on success, a negative errno on failure.
 */
#pragma once
#include <stdint.h>

typedef struct ra_audio ra_audio;

/* Connects to PipeWire (PIPEWIRE_REMOTE / PIPEWIRE_RUNTIME_DIR pick the instance). */
ra_audio *ra_open(int *err);
void ra_close(ra_audio *ra);
/* 1 while the connection to PipeWire is up. */
int ra_connected(ra_audio *ra);

/* "default" metadata, subject 0. get: the value's length (0 when absent). set:
 * json NULL removes the key. */
int ra_meta_get(ra_audio *ra, const char *key, char *out, int cap);
int ra_meta_set(ra_audio *ra, const char *key, const char *json);

/* A node's Props, by node.name: channel volumes (at most max) and mute. get:
 * the channel count; -ENOENT when there is no such node. */
int ra_node_volume_get(ra_audio *ra, const char *node_name, float *vols, int max, int *mute);
int ra_node_volume_set(ra_audio *ra, const char *node_name, const float *vols, int n, int mute);
/* 1 if a node with that name exists. */
int ra_node_exists(ra_audio *ra, const char *node_name);

int ra_sink_start(ra_audio *ra, const char *node_name, const char *description, int bitrate);
/* The next Opus packet: its length, 0 after timeout_ms with nothing, -EPIPE once stopped. */
int ra_sink_read(ra_audio *ra, uint8_t *out, int cap, int timeout_ms);
/* Stops the stream and wakes a blocked ra_sink_read; ra_sink_free after the reader is done. */
void ra_sink_stop(ra_audio *ra);
void ra_sink_free(ra_audio *ra);

int ra_mic_start(ra_audio *ra, const char *node_name, const char *description);
/* Decodes one Opus packet into the source's queue. */
int ra_mic_write(ra_audio *ra, const uint8_t *opus, int len);
void ra_mic_stop(ra_audio *ra);

/* The default sink's monitor (the generic Linux host): a capture stream with
 * stream.capture.sink, which WirePlumber links to the default output's monitor and
 * moves when the default changes. 48 kHz stereo float. */
int ra_monitor_start(ra_audio *ra, const char *node_name);
/* Waits up to timeout_ms for `frames` interleaved stereo frames: `frames` when they were
 * copied, 0 on a timeout, -EPIPE once stopped. */
int ra_monitor_read(ra_audio *ra, float *out, int frames, int timeout_ms);
void ra_monitor_stop(ra_audio *ra);
