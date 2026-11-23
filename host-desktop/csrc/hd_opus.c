// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// See hd_shim.h: the Opus half, built on every OS (the macOS build has no FFmpeg or
// OpenH264, so it compiles only this file; docs/reference/host-mac.md "Opus").
#include "hd_shim.h"

#include <stdlib.h>

#include <opus.h>

// ---------------------------------------------------------------------------
// Opus

struct hd_opus_enc {
    OpusEncoder *enc;
};
struct hd_opus_dec {
    OpusDecoder *dec;
};

hd_opus_enc *hd_opus_enc_new(int bitrate) {
    int r = 0;
    OpusEncoder *enc = opus_encoder_create(48000, 2, OPUS_APPLICATION_RESTRICTED_LOWDELAY, &r);
    if (!enc) return NULL;
    opus_encoder_ctl(enc, OPUS_SET_BITRATE(bitrate));
    hd_opus_enc *o = calloc(1, sizeof *o);
    if (!o) {
        opus_encoder_destroy(enc);
        return NULL;
    }
    o->enc = enc;
    return o;
}

int hd_opus_encode(hd_opus_enc *o, const float *pcm, uint8_t *out, int cap) {
    return opus_encode_float(o->enc, pcm, 480, out, cap);
}

void hd_opus_enc_free(hd_opus_enc *o) {
    if (!o) return;
    opus_encoder_destroy(o->enc);
    free(o);
}

hd_opus_dec *hd_opus_dec_new(void) {
    int r = 0;
    OpusDecoder *dec = opus_decoder_create(48000, 2, &r);
    if (!dec) return NULL;
    hd_opus_dec *o = calloc(1, sizeof *o);
    if (!o) {
        opus_decoder_destroy(dec);
        return NULL;
    }
    o->dec = dec;
    return o;
}

int hd_opus_decode(hd_opus_dec *o, const uint8_t *data, int len, float *pcm, int max_samples) {
    return opus_decode_float(o->dec, data, len, pcm, max_samples, 0);
}

void hd_opus_dec_free(hd_opus_dec *o) {
    if (!o) return;
    opus_decoder_destroy(o->dec);
    free(o);
}
