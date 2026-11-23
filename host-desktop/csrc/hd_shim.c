// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// See hd_shim.h.
#include "hd_shim.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <libavcodec/avcodec.h>
#include <libavutil/opt.h>
#include <libavutil/imgutils.h>
#include <libswscale/swscale.h>
#include <wels/codec_api.h>

#ifdef HD_VAAPI
// VA-API (Linux): FFmpeg's *_vaapi encoders on a VAAPI device. CPU pictures are
// converted to NV12 with swscale and uploaded; DMA-BUF pictures (a portal's PipeWire
// buffers) are imported as DRM-PRIME frames and converted on the GPU by a filter graph
// (hwmap to VAAPI, scale_vaapi to NV12), as FFmpeg's kmsgrab pipeline does.
#include <fcntl.h>
#include <unistd.h>
#include <libavfilter/avfilter.h>
#include <libavfilter/buffersink.h>
#include <libavfilter/buffersrc.h>
#include <libavutil/hwcontext.h>
#include <libavutil/hwcontext_drm.h>
#endif

#ifdef _WIN32
#include <windows.h>
#else
#include <dlfcn.h>
#endif

#ifdef _WIN32
static double now_ms(void) {
    static LARGE_INTEGER f;
    LARGE_INTEGER c;
    if (!f.QuadPart) QueryPerformanceFrequency(&f);
    QueryPerformanceCounter(&c);
    return (double)c.QuadPart * 1000.0 / (double)f.QuadPart;
}
#else
#include <time.h>
static double now_ms(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1000.0 + t.tv_nsec / 1e6;
}
#endif

// OpenH264 is never linked: its library is loaded by path at run time (Cisco's DLL on Windows,
// downloaded on first need; the system's libopenh264 elsewhere), so only its headers are used.
typedef int (*create_fn)(ISVCEncoder **);
typedef void (*destroy_fn)(ISVCEncoder *);
static create_fn g_create;
static destroy_fn g_destroy;

static void set_err(char *err, int errlen, const char *fmt, const char *a, int code) {
    if (!err || errlen <= 0) return;
    char av[128] = "";
    if (code) av_strerror(code, av, sizeof av);
    snprintf(err, errlen, fmt, a ? a : "", av);
}

int hd_openh264_loaded(void) { return g_create != NULL && g_destroy != NULL; }

int hd_openh264_load(const char *path, char *err, int errlen) {
    if (hd_openh264_loaded()) return 0;
    void *lib = NULL;
#ifdef _WIN32
    if (!path || !*path) {
        set_err(err, errlen, "openh264: no library path%s%s", "", 0);
        return -1;
    }
    wchar_t w[1024];
    if (!MultiByteToWideChar(CP_UTF8, 0, path, -1, w, 1024)) {
        set_err(err, errlen, "openh264: bad library path%s%s", "", 0);
        return -1;
    }
    // An absolute path (the caller has just checked the file's SHA-256 on a handle that keeps it from changing), and only
    // System32 for the DLL's own imports: nothing is taken from the user-writable folder it sits in.
    if (w[0] == 0 || w[1] != L':' || (w[2] != L'\\' && w[2] != L'/')) {
        set_err(err, errlen, "openh264: the library path is not absolute%s%s", "", 0);
        return -1;
    }
    HMODULE m = LoadLibraryExW(w, NULL, LOAD_LIBRARY_SEARCH_SYSTEM32);
    lib = (void *)m;
    create_fn c = lib ? (create_fn)(void *)GetProcAddress(m, "WelsCreateSVCEncoder") : NULL;
    destroy_fn d = lib ? (destroy_fn)(void *)GetProcAddress(m, "WelsDestroySVCEncoder") : NULL;
#else
    static const char *const names[] = {"libopenh264.so.8", "libopenh264.so.7", "libopenh264.so", NULL};
    if (path && *path) lib = dlopen(path, RTLD_NOW | RTLD_LOCAL);
    for (int i = 0; !lib && !(path && *path) && names[i]; i++) lib = dlopen(names[i], RTLD_NOW | RTLD_LOCAL);
    create_fn c = lib ? (create_fn)dlsym(lib, "WelsCreateSVCEncoder") : NULL;
    destroy_fn d = lib ? (destroy_fn)dlsym(lib, "WelsDestroySVCEncoder") : NULL;
#endif
    if (!lib || !c || !d) {
        set_err(err, errlen, "openh264: cannot load the library%s%s", "", 0);
        return -1;
    }
    g_destroy = d;
    g_create = c;
    return 0;
}

struct hd_enc {
    int openh264;  // 1: OpenH264 directly; 0: FFmpeg
    int codec;
    int width, height, fps;
    int64_t bitrate;
    int64_t pts;
    // Conversion: BGRA at the source size -> the encoder's planar format and size.
    struct SwsContext *sws;
    int src_w, src_h;
    enum AVPixelFormat fmt;
    // FFmpeg
    AVCodecContext *ctx;
    AVFrame *frame;
    AVPacket *pkt;
    // VA-API: the device, the encoder's NV12 surfaces (CPU path), and the DMA-BUF graph
#ifdef HD_VAAPI
    AVBufferRef *hw_dev;
    AVBufferRef *drm_dev, *drm_frames;
    AVFilterGraph *graph;
    AVFilterContext *src, *sink;
    AVFrame *hw;
#endif
    int vaapi, drm_mode, drm_w, drm_h;
    uint32_t drm_fourcc;
    char name[32];
    int64_t open_bitrate;
    // OpenH264
    ISVCEncoder *oh;
    uint8_t *planes[3];
    int strides[3];
    uint8_t *buf;
    int buf_cap;
};

void hd_log_quiet(int quiet) { av_log_set_level(quiet ? AV_LOG_QUIET : AV_LOG_ERROR); }

int hd_enc_codec(const hd_enc *e) { return e->codec; }
int hd_enc_dynamic_rate(const hd_enc *e) { return e->openh264; }

// ---------------------------------------------------------------------------
// OpenH264, used directly: per-frame bitrate (SetOption), screen-content coding, no
// frame skipping, no scene-change IDRs. FFmpeg's libopenh264 wrapper fixes the rate at
// open.

static int open_openh264(hd_enc *e, const hd_enc_config *cfg, char *err, int errlen) {
    if (!hd_openh264_loaded()) {
        set_err(err, errlen, "openh264: the library is not loaded%s%s", "", 0);
        return -1;
    }
    if (g_create(&e->oh) != 0 || !e->oh) {
        set_err(err, errlen, "openh264: WelsCreateSVCEncoder failed%s%s", "", 0);
        return -1;
    }
    int level = WELS_LOG_ERROR;
    (*e->oh)->SetOption(e->oh, ENCODER_OPTION_TRACE_LEVEL, &level);
    SEncParamExt p;
    memset(&p, 0, sizeof p);
    (*e->oh)->GetDefaultParams(e->oh, &p);
    p.iUsageType = SCREEN_CONTENT_REAL_TIME;
    p.iPicWidth = cfg->width;
    p.iPicHeight = cfg->height;
    p.iTargetBitrate = (int)cfg->bitrate;
    p.iMaxBitrate = UNSPECIFIED_BIT_RATE;
    p.iRCMode = RC_BITRATE_MODE;
    p.fMaxFrameRate = (float)cfg->fps;
    p.uiIntraPeriod = 0;  // keyframes on request only
    // Its rate control needs frame skipping: a skipped frame gives no bitstream, and the
    // media loop sends the latest picture at the next interval instead.
    p.bEnableFrameSkip = true;
    // Screen content wants scene-change detection (it picks references with it) and
    // has no adaptive quantisation or background detection.
    p.bEnableSceneChangeDetect = true;
    p.bEnableLongTermReference = false;
    p.bEnableDenoise = false;
    p.bEnableBackgroundDetection = false;
    p.bEnableAdaptiveQuant = false;
    p.iComplexityMode = LOW_COMPLEXITY;
    p.eSpsPpsIdStrategy = CONSTANT_ID;
    p.iNumRefFrame = 1;
    p.iEntropyCodingModeFlag = 0;
    p.iMultipleThreadIdc = cfg->threads > 0 ? cfg->threads : 4;
    p.iSpatialLayerNum = 1;
    p.iTemporalLayerNum = 1;
    SSpatialLayerConfig *l = &p.sSpatialLayers[0];
    l->iVideoWidth = cfg->width;
    l->iVideoHeight = cfg->height;
    l->fFrameRate = (float)cfg->fps;
    l->iSpatialBitrate = (int)cfg->bitrate;
    l->iMaxSpatialBitrate = UNSPECIFIED_BIT_RATE;
    l->uiProfileIdc = PRO_BASELINE;
    l->sSliceArgument.uiSliceMode = p.iMultipleThreadIdc > 1 ? SM_FIXEDSLCNUM_SLICE : SM_SINGLE_SLICE;
    l->sSliceArgument.uiSliceNum = p.iMultipleThreadIdc > 1 ? p.iMultipleThreadIdc : 1;
    l->bVideoSignalTypePresent = true;
    l->uiVideoFormat = VF_UNDEF;
    l->bFullRange = false;
    l->bColorDescriptionPresent = true;
    l->uiColorPrimaries = CP_BT709;
    l->uiTransferCharacteristics = TRC_BT709;
    l->uiColorMatrix = CM_BT709;
    int r = (*e->oh)->InitializeExt(e->oh, &p);
    if (r != 0) {
        char m[64];
        snprintf(m, sizeof m, "%d", r);
        set_err(err, errlen, "openh264: InitializeExt failed (%s)%s", m, 0);
        return -1;
    }
    int fmt = videoFormatI420;
    (*e->oh)->SetOption(e->oh, ENCODER_OPTION_DATAFORMAT, &fmt);
    e->fmt = AV_PIX_FMT_YUV420P;
    e->strides[0] = (cfg->width + 63) & ~63;
    e->strides[1] = e->strides[2] = e->strides[0] / 2;
    size_t y = (size_t)e->strides[0] * cfg->height, c = (size_t)e->strides[1] * (cfg->height / 2);
    e->planes[0] = av_malloc(y + 2 * c);
    if (!e->planes[0]) return -1;
    e->planes[1] = e->planes[0] + y;
    e->planes[2] = e->planes[1] + c;
    e->codec = HD_CODEC_H264;
    return 0;
}

static int encode_openh264(hd_enc *e, int force_key, int64_t bitrate, const uint8_t **out, int *out_len,
                           int *is_key, char *err, int errlen) {
    if (bitrate > 0 && bitrate != e->bitrate) {
        SBitrateInfo bi = {SPATIAL_LAYER_ALL, (int)bitrate};
        (*e->oh)->SetOption(e->oh, ENCODER_OPTION_BITRATE, &bi);
        e->bitrate = bitrate;
    }
    if (force_key) (*e->oh)->ForceIntraFrame(e->oh, true);
    SSourcePicture pic;
    memset(&pic, 0, sizeof pic);
    pic.iColorFormat = videoFormatI420;
    pic.iPicWidth = e->width;
    pic.iPicHeight = e->height;
    for (int i = 0; i < 3; i++) {
        pic.iStride[i] = e->strides[i];
        pic.pData[i] = e->planes[i];
    }
    pic.uiTimeStamp = (long long)(e->pts++ * 1000 / e->fps);
    SFrameBSInfo info;
    memset(&info, 0, sizeof info);
    int r = (*e->oh)->EncodeFrame(e->oh, &pic, &info);
    if (r != cmResultSuccess) {
        char m[64];
        snprintf(m, sizeof m, "%d", r);
        set_err(err, errlen, "openh264: EncodeFrame failed (%s)%s", m, 0);
        return -1;
    }
    int total = 0;
    for (int i = 0; i < info.iLayerNum; i++)
        for (int n = 0; n < info.sLayerInfo[i].iNalCount; n++) total += info.sLayerInfo[i].pNalLengthInByte[n];
    if (total > e->buf_cap) {
        av_free(e->buf);
        e->buf_cap = total * 2;
        e->buf = av_malloc(e->buf_cap);
        if (!e->buf) return -1;
    }
    int at = 0;
    for (int i = 0; i < info.iLayerNum; i++) {
        int len = 0;
        for (int n = 0; n < info.sLayerInfo[i].iNalCount; n++) len += info.sLayerInfo[i].pNalLengthInByte[n];
        memcpy(e->buf + at, info.sLayerInfo[i].pBsBuf, len);
        at += len;
    }
    *out = e->buf;
    *out_len = info.eFrameType == videoFrameTypeSkip ? 0 : at;
    *is_key = info.eFrameType == videoFrameTypeIDR;
    return 0;
}

// ---------------------------------------------------------------------------
// FFmpeg: the hardware HEVC encoders and Media Foundation's H.264, each set up for
// low latency: no B-frames, no lookahead, an endless GOP with keyframes on request
// (pict_type I, as an IDR), CBR with a one-frame VBV.

static int open_ffmpeg(hd_enc *e, const hd_enc_config *cfg, char *err, int errlen) {
    const AVCodec *c = avcodec_find_encoder_by_name(cfg->name);
    if (!c) {
        set_err(err, errlen, "%s: not in this FFmpeg%s", cfg->name, 0);
        return -1;
    }
    e->codec = c->id == AV_CODEC_ID_HEVC ? HD_CODEC_HEVC : HD_CODEC_H264;
    AVCodecContext *x = avcodec_alloc_context3(c);
    if (!x) return -1;
    e->ctx = x;
    x->width = cfg->width;
    x->height = cfg->height;
    x->time_base = (AVRational){1, cfg->fps};
    x->framerate = (AVRational){cfg->fps, 1};
    x->pix_fmt = AV_PIX_FMT_NV12;
#ifdef HD_VAAPI
    if (strstr(cfg->name, "_vaapi")) {
        // SODIUM_REMOTE_VAAPI_DEVICE picks the render node (default: the first that opens).
        int r = av_hwdevice_ctx_create(&e->hw_dev, AV_HWDEVICE_TYPE_VAAPI, getenv("SODIUM_REMOTE_VAAPI_DEVICE"), NULL, 0);
        if (r < 0) {
            set_err(err, errlen, "%s: no VA-API device: %s", cfg->name, r);
            return -1;
        }
        AVBufferRef *frames = av_hwframe_ctx_alloc(e->hw_dev);
        if (!frames) return -1;
        AVHWFramesContext *fc = (AVHWFramesContext *)frames->data;
        fc->format = AV_PIX_FMT_VAAPI;
        fc->sw_format = AV_PIX_FMT_NV12;
        fc->width = cfg->width;
        fc->height = cfg->height;
        fc->initial_pool_size = 4;
        if ((r = av_hwframe_ctx_init(frames)) < 0) {
            av_buffer_unref(&frames);
            set_err(err, errlen, "%s: VA-API surfaces: %s", cfg->name, r);
            return -1;
        }
        x->pix_fmt = AV_PIX_FMT_VAAPI;
        x->hw_frames_ctx = frames;
        e->vaapi = 1;
    }
#endif
    x->bit_rate = cfg->bitrate;
    x->rc_max_rate = cfg->bitrate;
    x->rc_buffer_size = (int)(cfg->bitrate / cfg->fps);
    x->max_b_frames = 0;
    x->gop_size = 0x7fffffff;  // no periodic IDR (mf: 0 would leave the MFT's own, an IDR a second)
    x->flags |= AV_CODEC_FLAG_LOW_DELAY;
    x->colorspace = AVCOL_SPC_BT709;
    x->color_primaries = AVCOL_PRI_BT709;
    x->color_trc = AVCOL_TRC_BT709;
    x->color_range = AVCOL_RANGE_MPEG;
    if (cfg->threads > 0) x->thread_count = cfg->threads;
    AVDictionary *o = NULL;
    if (strstr(cfg->name, "_nvenc")) {
        av_dict_set(&o, "preset", "p2", 0);
        av_dict_set(&o, "tune", "ull", 0);
        av_dict_set(&o, "rc", "cbr", 0);
        av_dict_set(&o, "zerolatency", "1", 0);
        av_dict_set(&o, "delay", "0", 0);
        av_dict_set(&o, "forced-idr", "1", 0);
        av_dict_set(&o, "rc-lookahead", "0", 0);
    } else if (strstr(cfg->name, "_amf")) {
        av_dict_set(&o, "usage", "ultralowlatency", 0);
        av_dict_set(&o, "rc", "cbr", 0);
        av_dict_set(&o, "quality", "speed", 0);
    } else if (strstr(cfg->name, "_qsv")) {
        av_dict_set(&o, "preset", "veryfast", 0);
        av_dict_set(&o, "async_depth", "1", 0);
        av_dict_set(&o, "forced_idr", "1", 0);
        av_dict_set(&o, "low_delay_brc", "1", 0);
    } else if (strstr(cfg->name, "_mf")) {
        // The GPU's MFT only. Microsoft's software MFT answers one forced keyframe with
        // seven IDRs (measured in the VM) and fixes its rate at open; OpenH264 does better.
        av_dict_set(&o, "hw_encoding", "1", 0);
        av_dict_set(&o, "rate_control", "cbr", 0);
        av_dict_set(&o, "scenario", "display_remoting", 0);
    } else if (strstr(cfg->name, "_vaapi")) {
        av_dict_set(&o, "rc_mode", "CBR", 0);
        av_dict_set(&o, "async_depth", "1", 0);
    } else if (strstr(cfg->name, "libx264")) {
        av_dict_set(&o, "preset", "ultrafast", 0);
        av_dict_set(&o, "tune", "zerolatency", 0);
        // A forced keyframe is an IDR (the client can start from it), not a bare I-frame.
        av_dict_set(&o, "forced-idr", "1", 0);
    } else if (strstr(cfg->name, "libopenh264")) {
        av_dict_set(&o, "allow_skip_frames", "0", 0);
    }
    int r = avcodec_open2(x, c, &o);
    av_dict_free(&o);
    if (r < 0) {
        set_err(err, errlen, "%s: %s", cfg->name, r);
        return -1;
    }
    e->fmt = e->vaapi ? AV_PIX_FMT_NV12 : x->pix_fmt;
    e->frame = av_frame_alloc();
    e->pkt = av_packet_alloc();
    if (!e->frame || !e->pkt) return -1;
    e->frame->format = e->fmt;
    e->frame->width = x->width;
    e->frame->height = x->height;
    if ((r = av_frame_get_buffer(e->frame, 64)) < 0) {
        set_err(err, errlen, "%s: frame buffer: %s", cfg->name, r);
        return -1;
    }
    return 0;
}

static int encode_ffmpeg_frame(hd_enc *e, AVFrame *f, int force_key, const uint8_t **out, int *out_len,
                               int *is_key, char *err, int errlen);

static int encode_ffmpeg(hd_enc *e, int force_key, const uint8_t **out, int *out_len, int *is_key, char *err,
                         int errlen) {
    AVFrame *f = e->frame;
#ifdef HD_VAAPI
    if (e->vaapi) {
        if (!e->hw) e->hw = av_frame_alloc();
        av_frame_unref(e->hw);
        int r = av_hwframe_get_buffer(e->ctx->hw_frames_ctx, e->hw, 0);
        if (r >= 0) r = av_hwframe_transfer_data(e->hw, e->frame, 0);
        if (r < 0) {
            set_err(err, errlen, "%s: upload: %s", e->ctx->codec->name, r);
            return -1;
        }
        f = e->hw;
    }
#endif
    return encode_ffmpeg_frame(e, f, force_key, out, out_len, is_key, err, errlen);
}

static int encode_ffmpeg_frame(hd_enc *e, AVFrame *f, int force_key, const uint8_t **out, int *out_len,
                               int *is_key, char *err, int errlen) {
    f->pts = e->pts++;
    f->pict_type = force_key ? AV_PICTURE_TYPE_I : AV_PICTURE_TYPE_NONE;
    if (force_key) f->flags |= AV_FRAME_FLAG_KEY;
    else f->flags &= ~AV_FRAME_FLAG_KEY;
    int r = avcodec_send_frame(e->ctx, f);
    if (r < 0) {
        set_err(err, errlen, "%s: send: %s", e->ctx->codec->name, r);
        return -1;
    }
    av_packet_unref(e->pkt);
    // A low-delay encoder answers at once; Media Foundation's may take a moment.
    for (int tries = 0;; tries++) {
        r = avcodec_receive_packet(e->ctx, e->pkt);
        if (r == AVERROR(EAGAIN) && tries < 50) {
#ifdef _WIN32
            Sleep(1);
#else
            struct timespec t = {0, 1000000};
            nanosleep(&t, NULL);
#endif
            continue;
        }
        break;
    }
    if (r == AVERROR(EAGAIN)) {
        *out_len = 0;
        return 0;
    }
    if (r < 0) {
        set_err(err, errlen, "%s: receive: %s", e->ctx->codec->name, r);
        return -1;
    }
    *out = e->pkt->data;
    *out_len = e->pkt->size;
    *is_key = (e->pkt->flags & AV_PKT_FLAG_KEY) != 0;
    return 0;
}

// ---------------------------------------------------------------------------

int hd_enc_open(const hd_enc_config *cfg, hd_enc **out, char *err, int errlen) {
    *out = NULL;
    if (cfg->width <= 0 || cfg->height <= 0 || (cfg->width & 1) || (cfg->height & 1) || cfg->fps <= 0) {
        set_err(err, errlen, "%s: bad size or rate%s", cfg->name, 0);
        return -1;
    }
    hd_enc *e = calloc(1, sizeof *e);
    if (!e) return -1;
    e->width = cfg->width;
    e->height = cfg->height;
    e->fps = cfg->fps;
    e->bitrate = cfg->bitrate;
    e->openh264 = strcmp(cfg->name, "openh264") == 0;
    snprintf(e->name, sizeof e->name, "%s", cfg->name);
    e->open_bitrate = cfg->bitrate;
    int r = e->openh264 ? open_openh264(e, cfg, err, errlen) : open_ffmpeg(e, cfg, err, errlen);
    if (r < 0) {
        hd_enc_close(e);
        return -1;
    }
    *out = e;
    return 0;
}

static int convert(hd_enc *e, const uint8_t *bgra, int w, int h, int stride, char *err, int errlen) {
    if (!e->sws || w != e->src_w || h != e->src_h) {
        int flags = (w == e->width && h == e->height) ? SWS_POINT : SWS_BILINEAR;
        e->sws = sws_getCachedContext(e->sws, w, h, AV_PIX_FMT_BGRA, e->width, e->height, e->fmt, flags, NULL, NULL, NULL);
        if (!e->sws) {
            set_err(err, errlen, "swscale: no conversion to the encoder's format%s%s", "", 0);
            return -1;
        }
        // RGB in full range to BT.709 limited range, as the stream's VUI says.
        const int *coef = sws_getCoefficients(SWS_CS_ITU709);
        sws_setColorspaceDetails(e->sws, coef, 1, coef, 0, 0, 1 << 16, 1 << 16);
        e->src_w = w;
        e->src_h = h;
    }
    const uint8_t *src[4] = {bgra, NULL, NULL, NULL};
    int src_stride[4] = {stride, 0, 0, 0};
    uint8_t *dst[4] = {0};
    int dst_stride[4] = {0};
    if (e->openh264) {
        for (int i = 0; i < 3; i++) {
            dst[i] = e->planes[i];
            dst_stride[i] = e->strides[i];
        }
    } else {
        int r = av_frame_make_writable(e->frame);
        if (r < 0) {
            set_err(err, errlen, "frame: %s%s", "", r);
            return -1;
        }
        for (int i = 0; i < 4; i++) {
            dst[i] = e->frame->data[i];
            dst_stride[i] = e->frame->linesize[i];
        }
    }
    sws_scale(e->sws, src, src_stride, 0, h, dst, dst_stride);
    return 0;
}

int hd_enc_encode(hd_enc *e, const uint8_t *bgra, int w, int h, int stride, int force_key, int64_t bitrate,
                  const uint8_t **out, int *out_len, int *is_key, double *convert_ms, double *encode_ms, char *err,
                  int errlen) {
    *out = NULL;
    *out_len = 0;
    *is_key = 0;
    double t0 = now_ms();
    if (convert(e, bgra, w, h, stride, err, errlen) < 0) return -1;
    double t1 = now_ms();
    int r = e->openh264 ? encode_openh264(e, force_key, bitrate, out, out_len, is_key, err, errlen)
                        : encode_ffmpeg(e, force_key, out, out_len, is_key, err, errlen);
    *convert_ms = t1 - t0;
    *encode_ms = now_ms() - t1;
    return r;
}

void hd_enc_close(hd_enc *e) {
    if (!e) return;
    if (e->oh) {
        (*e->oh)->Uninitialize(e->oh);
        g_destroy(e->oh);
    }
    av_free(e->planes[0]);
    av_free(e->buf);
    if (e->ctx) avcodec_free_context(&e->ctx);
#ifdef HD_VAAPI
    avfilter_graph_free(&e->graph);
    av_frame_free(&e->hw);
    av_buffer_unref(&e->drm_frames);
    av_buffer_unref(&e->drm_dev);
    av_buffer_unref(&e->hw_dev);
#endif
    av_frame_free(&e->frame);
    av_packet_free(&e->pkt);
    sws_freeContext(e->sws);
    free(e);
}

// ---------------------------------------------------------------------------
// DMA-BUF pictures into VA-API (Linux)

int hd_enc_takes_drm(const hd_enc *e) { return e->vaapi; }

#ifdef HD_VAAPI
static void free_desc(void *opaque, uint8_t *data) {
    (void)opaque;
    av_free(data);
}

// The first DMA-BUF picture (or a new size or format): DRM frames on a DRM device
// derived from the encoder's VAAPI device, a graph hwmap -> scale_vaapi (NV12, the
// encoder's size), and the encoder re-opened on the graph's surfaces.
static int drm_setup(hd_enc *e, uint32_t fourcc, int w, int h, char *err, int errlen) {
    avfilter_graph_free(&e->graph);
    av_buffer_unref(&e->drm_frames);
    int r = 0;
    if (!e->drm_dev && (r = av_hwdevice_ctx_create_derived(&e->drm_dev, AV_HWDEVICE_TYPE_DRM, e->hw_dev, 0)) < 0) {
        set_err(err, errlen, "%s: no DRM device for DMA-BUF: %s", e->name, r);
        return -1;
    }
    e->drm_frames = av_hwframe_ctx_alloc(e->drm_dev);
    if (!e->drm_frames) return -1;
    AVHWFramesContext *fc = (AVHWFramesContext *)e->drm_frames->data;
    fc->format = AV_PIX_FMT_DRM_PRIME;
    // XR24/AR24 are B,G,R,X in memory; XB24/AB24 R,G,B,X.
    int rgb = fourcc == (uint32_t)('X' | 'B' << 8 | '2' << 16 | '4' << 24) || fourcc == (uint32_t)('A' | 'B' << 8 | '2' << 16 | '4' << 24);
    fc->sw_format = rgb ? AV_PIX_FMT_RGB0 : AV_PIX_FMT_BGR0;
    fc->width = w;
    fc->height = h;
    if ((r = av_hwframe_ctx_init(e->drm_frames)) < 0) {
        set_err(err, errlen, "%s: DRM frames: %s", e->name, r);
        return -1;
    }
    e->graph = avfilter_graph_alloc();
    if (!e->graph) return -1;
    char args[256];
    snprintf(args, sizeof args, "video_size=%dx%d:pix_fmt=%d:time_base=1/%d:pixel_aspect=1/1", w, h, AV_PIX_FMT_DRM_PRIME,
             e->fps);
    if ((r = avfilter_graph_create_filter(&e->src, avfilter_get_by_name("buffer"), "in", args, NULL, e->graph)) < 0 ||
        (r = avfilter_graph_create_filter(&e->sink, avfilter_get_by_name("buffersink"), "out", NULL, NULL, e->graph)) <
            0) {
        set_err(err, errlen, "%s: filter graph: %s", e->name, r);
        return -1;
    }
    AVBufferSrcParameters *par = av_buffersrc_parameters_alloc();
    par->format = AV_PIX_FMT_DRM_PRIME;
    par->width = w;
    par->height = h;
    par->time_base = (AVRational){1, e->fps};
    par->hw_frames_ctx = e->drm_frames;
    r = av_buffersrc_parameters_set(e->src, par);
    av_free(par);
    if (r < 0) {
        set_err(err, errlen, "%s: filter source: %s", e->name, r);
        return -1;
    }
    char chain[160];
    snprintf(chain, sizeof chain, "hwmap=mode=read:derive_device=vaapi,scale_vaapi=w=%d:h=%d:format=nv12", e->width,
             e->height);
    AVFilterInOut *outs = avfilter_inout_alloc(), *ins = avfilter_inout_alloc();
    outs->name = av_strdup("in");
    outs->filter_ctx = e->src;
    ins->name = av_strdup("out");
    ins->filter_ctx = e->sink;
    r = avfilter_graph_parse_ptr(e->graph, chain, &ins, &outs, NULL);
    avfilter_inout_free(&ins);
    avfilter_inout_free(&outs);
    for (unsigned i = 0; r >= 0 && i < e->graph->nb_filters; i++)
        if (strstr(e->graph->filters[i]->name, "hwmap"))
            e->graph->filters[i]->hw_device_ctx = av_buffer_ref(e->hw_dev);
    if (r >= 0) r = avfilter_graph_config(e->graph, NULL);
    if (r < 0) {
        set_err(err, errlen, "%s: hwmap/scale_vaapi: %s", e->name, r);
        return -1;
    }
    // The encoder on the graph's NV12 surfaces.
    const AVCodec *c = e->ctx->codec;
    AVCodecContext *x = avcodec_alloc_context3(c);
    x->width = e->width;
    x->height = e->height;
    x->time_base = e->ctx->time_base;
    x->framerate = e->ctx->framerate;
    x->pix_fmt = AV_PIX_FMT_VAAPI;
    x->bit_rate = x->rc_max_rate = e->open_bitrate;
    x->rc_buffer_size = (int)(e->open_bitrate / e->fps);
    x->max_b_frames = 0;
    x->gop_size = 0x7fffffff;
    x->flags |= AV_CODEC_FLAG_LOW_DELAY;
    x->colorspace = AVCOL_SPC_BT709;
    x->color_primaries = AVCOL_PRI_BT709;
    x->color_trc = AVCOL_TRC_BT709;
    x->color_range = AVCOL_RANGE_MPEG;
    x->hw_frames_ctx = av_buffer_ref(av_buffersink_get_hw_frames_ctx(e->sink));
    AVDictionary *o = NULL;
    av_dict_set(&o, "rc_mode", "CBR", 0);
    av_dict_set(&o, "async_depth", "1", 0);
    r = avcodec_open2(x, c, &o);
    av_dict_free(&o);
    if (r < 0) {
        avcodec_free_context(&x);
        set_err(err, errlen, "%s: re-open on DMA-BUF surfaces: %s", e->name, r);
        return -1;
    }
    avcodec_free_context(&e->ctx);
    e->ctx = x;
    e->drm_mode = 1;
    e->drm_w = w;
    e->drm_h = h;
    e->drm_fourcc = fourcc;
    return 0;
}
#endif

int hd_enc_encode_drm(hd_enc *e, uint32_t fourcc, uint64_t modifier, int nplanes, const int *fds,
                      const uint32_t *offsets, const uint32_t *pitches, int w, int h, int force_key,
                      const uint8_t **out, int *out_len, int *is_key, double *convert_ms, double *encode_ms, char *err,
                      int errlen) {
    *out = NULL;
    *out_len = 0;
    *is_key = 0;
#ifdef HD_VAAPI
    if (!e->vaapi || nplanes < 1 || nplanes > AV_DRM_MAX_PLANES) {
        set_err(err, errlen, "%s: takes no DMA-BUF%s", e->name, 0);
        return -1;
    }
    double t0 = now_ms();
    if (!e->drm_mode || w != e->drm_w || h != e->drm_h || fourcc != e->drm_fourcc) {
        if (drm_setup(e, fourcc, w, h, err, errlen) < 0) return -1;
        force_key = 1;
    }
    AVDRMFrameDescriptor *d = av_mallocz(sizeof *d);
    if (!d) return -1;
    // One object per distinct fd.
    for (int i = 0; i < nplanes; i++) {
        int obj = -1;
        for (int k = 0; k < d->nb_objects; k++)
            if (d->objects[k].fd == fds[i]) obj = k;
        if (obj < 0) {
            obj = d->nb_objects++;
            d->objects[obj].fd = fds[i];
            off_t size = lseek(fds[i], 0, SEEK_END);
            d->objects[obj].size = size > 0 ? (size_t)size : 0;
            d->objects[obj].format_modifier = modifier;
        }
        d->layers[0].planes[i].object_index = obj;
        d->layers[0].planes[i].offset = offsets[i];
        d->layers[0].planes[i].pitch = pitches[i];
    }
    d->nb_layers = 1;
    d->layers[0].format = fourcc;
    d->layers[0].nb_planes = nplanes;
    AVFrame *f = av_frame_alloc();
    f->format = AV_PIX_FMT_DRM_PRIME;
    f->width = w;
    f->height = h;
    f->data[0] = (uint8_t *)d;
    f->buf[0] = av_buffer_create((uint8_t *)d, sizeof *d, free_desc, NULL, 0);
    f->hw_frames_ctx = av_buffer_ref(e->drm_frames);
    f->pts = e->pts;
    int r = av_buffersrc_add_frame_flags(e->src, f, AV_BUFFERSRC_FLAG_KEEP_REF);
    av_frame_free(&f);
    if (r < 0) {
        set_err(err, errlen, "%s: DMA-BUF import: %s", e->name, r);
        return -1;
    }
    if (!e->hw) e->hw = av_frame_alloc();
    av_frame_unref(e->hw);
    if ((r = av_buffersink_get_frame(e->sink, e->hw)) < 0) {
        set_err(err, errlen, "%s: scale_vaapi: %s", e->name, r);
        return -1;
    }
    double t1 = now_ms();
    r = encode_ffmpeg_frame(e, e->hw, force_key, out, out_len, is_key, err, errlen);
    *convert_ms = t1 - t0;
    *encode_ms = now_ms() - t1;
    return r;
#else
    (void)fourcc, (void)modifier, (void)nplanes, (void)fds, (void)offsets, (void)pitches, (void)w, (void)h,
        (void)force_key, (void)convert_ms, (void)encode_ms;
    set_err(err, errlen, "%s: takes no DMA-BUF%s", e->name, 0);
    return -1;
#endif
}
