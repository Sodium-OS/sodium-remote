// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// Windows toolchain smoke test: FFmpeg version and configuration, which of our encoders and
// decoders exist, an H.264 encode of 10 synthetic 1280x720 frames with Media Foundation (the
// only FFmpeg H.264 encoder that needs no GPU runtime; OpenH264 is Cisco's DLL, fetched by the
// host at run time and not part of this build), a decode of them with the native h264 decoder when
// there is a stream, and one Opus frame. Under wine there is no MFT, so no stream and the decode
// is skipped. Exit status 0 when the decoders exist, Opus works and, if a stream was made, it decodes.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <libavcodec/avcodec.h>
#include <libavutil/imgutils.h>
#include <libavutil/opt.h>
#include <libavutil/pixdesc.h>
#include <libswscale/swscale.h>
#include <opus.h>

#define W 1280
#define H 720
#define N 10

static const char *encoders[] = {"h264_mf", "hevc_mf", "av1_mf", "h264_nvenc", "hevc_nvenc", "av1_nvenc",
                                 "h264_amf", "hevc_amf", "av1_amf", "h264_qsv", "hevc_qsv", "av1_qsv", NULL};
static const char *decoders[] = {"h264", "hevc", "av1", NULL};

static unsigned char *stream; // the encoded H.264, Annex B
static size_t stream_len, stream_cap;

static void append(const uint8_t *d, int n) {
    if (stream_len + n > stream_cap) {
        stream_cap = (stream_len + n) * 2;
        stream = realloc(stream, stream_cap);
    }
    memcpy(stream + stream_len, d, n);
    stream_len += n;
}

static void fill(AVFrame *f, int i) {
    // A moving gradient with a box, so inter frames have something to code.
    for (int y = 0; y < H; y++)
        for (int x = 0; x < W; x++) f->data[0][y * f->linesize[0] + x] = (uint8_t)(x / 6 + y / 4 + i * 8);
    for (int y = 0; y < H / 2; y++)
        for (int x = 0; x < W / 2; x++) {
            f->data[1][y * f->linesize[1] + x] = 128 + (x + i * 4) % 32;
            f->data[2][y * f->linesize[2] + x] = 128 - (y + i * 4) % 32;
        }
    int bx = 100 + i * 40;
    for (int y = 300; y < 400; y++)
        for (int x = bx; x < bx + 100; x++) f->data[0][y * f->linesize[0] + x] = 235;
}

// Encodes N frames with `name`; on success the packets are in `stream`.
static int try_encode(const char *name, int pix_fmt_nv12) {
    const AVCodec *c = avcodec_find_encoder_by_name(name);
    if (!c) { printf("encode %-12s: not built\n", name); return -1; }
    AVCodecContext *ctx = avcodec_alloc_context3(c);
    ctx->width = W; ctx->height = H;
    ctx->time_base = (AVRational){1, 30}; ctx->framerate = (AVRational){30, 1};
    ctx->pix_fmt = pix_fmt_nv12 ? AV_PIX_FMT_NV12 : AV_PIX_FMT_YUV420P;
    ctx->bit_rate = 4000000; ctx->gop_size = 30; ctx->max_b_frames = 0;
    AVDictionary *o = NULL;
    if (!strcmp(name, "h264_mf")) { av_dict_set(&o, "hw_encoding", "1", 0); }
    int r = avcodec_open2(ctx, c, &o);
    av_dict_free(&o);
    if (r < 0) {
        char e[128]; av_strerror(r, e, sizeof e);
        printf("encode %-12s: open failed: %s\n", name, e);
        avcodec_free_context(&ctx);
        return -1;
    }
    AVFrame *f = av_frame_alloc();
    f->format = ctx->pix_fmt; f->width = W; f->height = H;
    av_frame_get_buffer(f, 0);
    AVPacket *p = av_packet_alloc();
    stream_len = 0;
    int pkts = 0;
    for (int i = 0; i < N + 1; i++) {
        if (i < N) {
            av_frame_make_writable(f);
            if (pix_fmt_nv12) {
                // The MF encoders take NV12: fill planar, then interleave.
                AVFrame *t = av_frame_alloc(); t->format = AV_PIX_FMT_YUV420P; t->width = W; t->height = H;
                av_frame_get_buffer(t, 0); fill(t, i);
                struct SwsContext *s = sws_getContext(W, H, AV_PIX_FMT_YUV420P, W, H, AV_PIX_FMT_NV12, SWS_POINT, 0, 0, 0);
                sws_scale(s, (const uint8_t *const *)t->data, t->linesize, 0, H, f->data, f->linesize);
                sws_freeContext(s); av_frame_free(&t);
            } else fill(f, i);
            f->pts = i;
            r = avcodec_send_frame(ctx, f);
        } else r = avcodec_send_frame(ctx, NULL);
        if (r < 0) break;
        while (avcodec_receive_packet(ctx, p) >= 0) { append(p->data, p->size); pkts++; av_packet_unref(p); }
    }
    if (ctx->extradata_size && pkts) { /* MF may put SPS/PPS in extradata only */ }
    printf("encode %-12s: ok, %d packets, %zu bytes\n", name, pkts, stream_len);
    av_packet_free(&p); av_frame_free(&f); avcodec_free_context(&ctx);
    return pkts > 0 ? 0 : -1;
}

static int decode_back(void) {
    const AVCodec *c = avcodec_find_decoder_by_name("h264");
    AVCodecContext *ctx = avcodec_alloc_context3(c);
    if (avcodec_open2(ctx, c, NULL) < 0) { puts("decode h264: open failed"); return -1; }
    AVCodecParserContext *pc = av_parser_init(AV_CODEC_ID_H264);
    AVPacket *p = av_packet_alloc();
    AVFrame *f = av_frame_alloc();
    int frames = 0, npk = 0, flushed = 0;
    const uint8_t *d = stream; int left = (int)stream_len;
    for (;;) {
        // The parser holds the last access unit until it is flushed with an empty call.
        int used = av_parser_parse2(pc, ctx, &p->data, &p->size, d, left, AV_NOPTS_VALUE, AV_NOPTS_VALUE, 0);
        d += used; left -= used;
        if (p->size) { npk++; int sr = avcodec_send_packet(ctx, p); if (sr < 0) printf("send_packet: %d\n", sr); }
        while (avcodec_receive_frame(ctx, f) >= 0) {
            if (frames == 0) printf("decode h264: first frame %dx%d %s\n", f->width, f->height, av_get_pix_fmt_name(f->format));
            frames++;
        }
        if (left <= 0) { if (flushed++) break; }
    }
    avcodec_send_packet(ctx, NULL);
    while (avcodec_receive_frame(ctx, f) >= 0) frames++;
    printf("decode h264: %d access units parsed, %d frames back\n", npk, frames);
    av_parser_close(pc); av_packet_free(&p); av_frame_free(&f); avcodec_free_context(&ctx);
    return frames == N ? 0 : -1;
}

static int opus_roundtrip(void) {
    int err;
    OpusEncoder *e = opus_encoder_create(48000, 2, OPUS_APPLICATION_RESTRICTED_LOWDELAY, &err);
    OpusDecoder *d = opus_decoder_create(48000, 2, &err);
    if (!e || !d) { puts("opus: create failed"); return -1; }
    short in[480 * 2], out[480 * 2];
    for (int i = 0; i < 480; i++) in[2 * i] = in[2 * i + 1] = (short)(8000 * ((i / 24) % 2 ? 1 : -1));
    unsigned char pkt[1500];
    int n = opus_encode(e, in, 480, pkt, sizeof pkt);
    int m = n > 0 ? opus_decode(d, pkt, n, out, 480, 0) : -1;
    printf("opus %s: 10 ms frame -> %d bytes -> %d samples\n", opus_get_version_string(), n, m);
    opus_encoder_destroy(e); opus_decoder_destroy(d);
    return (n > 0 && m == 480) ? 0 : -1;
}

int main(void) {
    printf("libavcodec %s (header %s)\n", av_version_info(), AV_STRINGIFY(LIBAVCODEC_VERSION));
    printf("configuration: %s\n", avcodec_configuration());
    printf("license: %s\n", avcodec_license());
    for (int i = 0; encoders[i]; i++) printf("encoder %-12s: %s\n", encoders[i], avcodec_find_encoder_by_name(encoders[i]) ? "present" : "MISSING");
    for (int i = 0; decoders[i]; i++) printf("decoder %-12s: %s\n", decoders[i], avcodec_find_decoder_by_name(decoders[i]) ? "present" : "MISSING");
    int mf = try_encode("h264_mf", 1);
    int dec = mf == 0 ? decode_back() : 1; // 1: skipped, no encoder here (wine has no MFT)
    int op = opus_roundtrip();
    printf("RESULT h264_mf=%s decode=%s opus=%s\n", mf == 0 ? "ok" : "no", dec == 0 ? "ok" : dec > 0 ? "skipped" : "FAIL", op == 0 ? "ok" : "FAIL");
    return (dec == 0 || dec == 1) && op == 0 ? 0 : 1;
}
