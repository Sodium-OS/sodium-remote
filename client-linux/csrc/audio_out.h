// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
/* remote-client's audio out (src/audio.rs): one PipeWire playback stream,
 * 48 kHz stereo float, about 10 ms of latency. The stream pulls: its process
 * callback asks `fill` for exactly the frames it wants, on the PipeWire thread
 * loop's thread. Returns NULL with *err a negative errno on failure.
 */
#pragma once

typedef struct ao_stream ao_stream;
/* Writes `frames` interleaved stereo frames to `out`. */
typedef void (*ao_fill_fn)(void *user, float *out, int frames);

/* `app_name`, `app_id` and `icon` are what the volume mixer shows. */
ao_stream *ao_open(const char *app_name, const char *app_id, const char *icon, ao_fill_fn fill, void *user, int *err);
/* Stops the stream; `fill` is never called after this returns. */
void ao_close(ao_stream *s);
