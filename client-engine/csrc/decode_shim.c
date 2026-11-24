// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// See decode_shim.h. Low-latency settings: no frame threading (it delays output
// by a frame per thread), AV_CODEC_FLAG_LOW_DELAY, and dav1d's frame delay at 1,
// so every access unit in gives its picture out at once.
//
// Windows zero copy: the presenter shares its D3D11 device (rc_d3d11_set_device); the D3D11VA
// decoder then decodes on it into an NV12 texture array that is also a shader resource, and a
// picture is handed over as its texture and slice, never copied to system memory. Whatever can't
// do that (no shared device, a format other than NV12, a driver that refuses shader-readable
// decoder textures, the presenter turning it off) takes the copy path as before.
#ifdef _WIN32
#define COBJMACROS
#endif
#include "decode_shim.h"

// ffnvcodec's loader first: it defines the CUDA types, so hwcontext_cuda.h
// doesn't look for the CUDA toolkit's cuda.h.
#include <ffnvcodec/dynlink_loader.h>
#include <libavcodec/avcodec.h>
#include <libavutil/hwcontext.h>
#include <libavutil/hwcontext_cuda.h>
#include <libavutil/opt.h>
#include <libavutil/pixdesc.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#ifdef _WIN32
#include <windows.h>
#include <d3d11.h>
#include <libavutil/hwcontext_d3d11va.h>
#endif

struct rc_dec {
    AVCodecContext *ctx;
    AVBufferRef *hwdev;
    enum AVPixelFormat hw_fmt;
    AVPacket *pkt;
    AVFrame *out; // the newest picture from the decoder
    AVFrame *tmp;
    AVFrame *sw;  // its copy in system memory
    int have;
    int keep_cuda;
    int shared_d3d11; // the D3D11VA device is the presenter's (rc_d3d11_set_device)
};

// libcuda, loaded once for the GL interop and the luma probe.
static CudaFunctions *g_cu;

static int cuda_fns(char *err, int errlen) {
    if (g_cu) return 0;
    if (cuda_load_functions(&g_cu, NULL) < 0) {
        g_cu = NULL;
        snprintf(err, errlen, "could not load libcuda");
        return -1;
    }
    return 0;
}

static int cu_check(CUresult r, const char *what, char *err, int errlen) {
    if (r == CUDA_SUCCESS) return 0;
    const char *name = "?";
    if (g_cu && g_cu->cuGetErrorName) g_cu->cuGetErrorName(r, &name);
    snprintf(err, errlen, "%s: %s", what, name);
    return -1;
}

static void set_err(char *err, int len, const char *what, int r) {
    char buf[128] = "";
    if (r) av_strerror(r, buf, sizeof buf);
    snprintf(err, len, "%s%s%s", what, r ? ": " : "", buf);
}

static double now_ms(void) {
#ifdef _WIN32
    LARGE_INTEGER f, c;
    QueryPerformanceFrequency(&f);
    QueryPerformanceCounter(&c);
    return (double)c.QuadPart * 1e3 / (double)f.QuadPart;
#else
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1e3 + t.tv_nsec / 1e6;
#endif
}

#ifdef _WIN32
// The presenter's device, and the lock FFmpeg and the presenter share around its immediate context.
static ID3D11Device *g_d3d;
static volatile LONG g_zero_copy = 1;
static CRITICAL_SECTION g_d3d_cs;
static INIT_ONCE g_d3d_once = INIT_ONCE_STATIC_INIT;
// IID_ID3D10Multithread, spelled out so nothing has to link dxguid.
static const GUID rc_IID_ID3D10Multithread = {0x9b7e4e00, 0x342c, 0x4106, {0xa1, 0x9f, 0x4f, 0x27, 0x04, 0xf6, 0x89, 0xf0}};

static BOOL CALLBACK d3d_cs_init(PINIT_ONCE once, PVOID param, PVOID *ctx) {
    (void)once;
    (void)param;
    (void)ctx;
    InitializeCriticalSection(&g_d3d_cs);
    return TRUE;
}

static void d3d_lock(void *ctx) {
    (void)ctx;
    InitOnceExecuteOnce(&g_d3d_once, d3d_cs_init, NULL, NULL);
    EnterCriticalSection(&g_d3d_cs);
}

static void d3d_unlock(void *ctx) {
    (void)ctx;
    LeaveCriticalSection(&g_d3d_cs);
}

void rc_d3d11_lock(void) { d3d_lock(NULL); }
void rc_d3d11_unlock(void) { d3d_unlock(NULL); }
void rc_d3d11_zero_copy(int on) { InterlockedExchange(&g_zero_copy, on ? 1 : 0); }

int rc_d3d11_set_device(void *device) {
    d3d_lock(NULL);
    if (g_d3d) ID3D11Device_Release(g_d3d);
    g_d3d = device;
    if (g_d3d) {
        ID3D11Device_AddRef(g_d3d);
        // The decoder thread and the window thread both use the device's immediate context.
        ID3D10Multithread *mt = NULL;
        if (SUCCEEDED(ID3D11Device_QueryInterface(g_d3d, &rc_IID_ID3D10Multithread, (void **)&mt)) && mt) {
            ID3D10Multithread_SetMultithreadProtected(mt, TRUE);
            ID3D10Multithread_Release(mt);
        }
    }
    d3d_unlock(NULL);
    return 0;
}

// A D3D11VA device context on the presenter's device; NULL when there is none or FFmpeg refuses it
// (e.g. a device made without video support).
static AVBufferRef *shared_d3d11_device(void) {
    d3d_lock(NULL);
    ID3D11Device *dev = g_d3d;
    if (dev) ID3D11Device_AddRef(dev);
    d3d_unlock(NULL);
    if (!dev) return NULL;
    AVBufferRef *ref = av_hwdevice_ctx_alloc(AV_HWDEVICE_TYPE_D3D11VA);
    if (!ref) {
        ID3D11Device_Release(dev);
        return NULL;
    }
    AVD3D11VADeviceContext *hw = ((AVHWDeviceContext *)ref->data)->hwctx;
    hw->device = dev; // FFmpeg releases it with the context
    hw->lock = d3d_lock;
    hw->unlock = d3d_unlock;
    hw->lock_ctx = NULL;
    if (av_hwdevice_ctx_init(ref) < 0) {
        av_buffer_unref(&ref);
        return NULL;
    }
    return ref;
}

// Decoder textures the presenter can sample: FFmpeg's own frame parameters plus
// D3D11_BIND_SHADER_RESOURCE, NV12 only. 0 when that worked (ctx->hw_frames_ctx is set).
static int shader_readable_frames(rc_dec *d, AVCodecContext *ctx) {
    if (!d->shared_d3d11 || !g_zero_copy) return -1;
    AVBufferRef *fr = NULL;
    if (avcodec_get_hw_frames_parameters(ctx, d->hwdev, AV_PIX_FMT_D3D11, &fr) < 0) return -1;
    AVHWFramesContext *fc = (AVHWFramesContext *)fr->data;
    if (fc->sw_format != AV_PIX_FMT_NV12) {
        av_buffer_unref(&fr);
        return -1;
    }
    AVD3D11VAFramesContext *fh = fc->hwctx;
    // The pool already counts extra_hw_frames: room for the picture the presenter keeps
    // and the one waiting in the latest-frame slot.
    fh->BindFlags |= D3D11_BIND_SHADER_RESOURCE;
    if (av_hwframe_ctx_init(fr) < 0) {
        fprintf(stderr, "remote-client: shader-readable D3D11 decoder textures refused; copying pictures\n");
        av_buffer_unref(&fr);
        return -1;
    }
    av_buffer_unref(&ctx->hw_frames_ctx);
    ctx->hw_frames_ctx = fr;
    return 0;
}

// The frame is a slice of a shader-readable texture on the presenter's device.
static int zero_copy_frame(const AVFrame *f) {
    if (!g_zero_copy || f->format != AV_PIX_FMT_D3D11 || !f->hw_frames_ctx) return 0;
    AVHWFramesContext *fc = (AVHWFramesContext *)f->hw_frames_ctx->data;
    AVD3D11VAFramesContext *fh = fc->hwctx;
    AVD3D11VADeviceContext *dh = fc->device_ctx->hwctx;
    return fc->sw_format == AV_PIX_FMT_NV12 && (fh->BindFlags & D3D11_BIND_SHADER_RESOURCE) && g_d3d && dh->device == g_d3d;
}

// Mean luma over a rectangle of a D3D11 picture: that rectangle (2-aligned for NV12) through a
// small staging texture.
static int d3d11_luma(const rc_frame *f, int x, int y, int w, int h, uint8_t *buf, char *err, int errlen) {
    const AVFrame *fr = f->ref;
    AVHWFramesContext *fc = (AVHWFramesContext *)fr->hw_frames_ctx->data;
    AVD3D11VADeviceContext *dh = fc->device_ctx->hwctx;
    ID3D11Texture2D *tex = f->texture;
    D3D11_TEXTURE2D_DESC td;
    ID3D11Texture2D_GetDesc(tex, &td);
    UINT x0 = (UINT)x & ~1u, y0 = (UINT)y & ~1u, x1 = ((UINT)(x + w) + 1) & ~1u, y1 = ((UINT)(y + h) + 1) & ~1u;
    if (x1 > td.Width) x1 = td.Width;
    if (y1 > td.Height) y1 = td.Height;
    D3D11_TEXTURE2D_DESC sd = {0};
    sd.Width = x1 - x0;
    sd.Height = y1 - y0;
    sd.MipLevels = 1;
    sd.ArraySize = 1;
    sd.Format = td.Format;
    sd.SampleDesc.Count = 1;
    sd.Usage = D3D11_USAGE_STAGING;
    sd.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
    ID3D11Texture2D *st = NULL;
    HRESULT hr = ID3D11Device_CreateTexture2D(dh->device, &sd, NULL, &st);
    if (FAILED(hr)) {
        snprintf(err, errlen, "staging texture: 0x%08lx", (unsigned long)hr);
        return -1;
    }
    D3D11_BOX box = {x0, y0, 0, x1, y1, 1};
    D3D11_MAPPED_SUBRESOURCE m;
    dh->lock(dh->lock_ctx);
    ID3D11DeviceContext_CopySubresourceRegion(dh->device_context, (ID3D11Resource *)st, 0, 0, 0, 0, (ID3D11Resource *)tex, (UINT)f->slice, &box);
    hr = ID3D11DeviceContext_Map(dh->device_context, (ID3D11Resource *)st, 0, D3D11_MAP_READ, 0, &m);
    if (SUCCEEDED(hr)) {
        for (int row = 0; row < h; row++) memcpy(buf + (size_t)row * w, (const uint8_t *)m.pData + (size_t)(y - (int)y0 + row) * m.RowPitch + (x - (int)x0), w);
        ID3D11DeviceContext_Unmap(dh->device_context, (ID3D11Resource *)st, 0);
    }
    dh->unlock(dh->lock_ctx);
    ID3D11Texture2D_Release(st);
    if (FAILED(hr)) {
        snprintf(err, errlen, "mapping the staging texture: 0x%08lx", (unsigned long)hr);
        return -1;
    }
    return 0;
}
#endif

static enum AVPixelFormat get_format(AVCodecContext *ctx, const enum AVPixelFormat *fmts) {
    rc_dec *d = ctx->opaque;
    const enum AVPixelFormat *p;
#ifdef _WIN32
    for (p = fmts; *p != AV_PIX_FMT_NONE; p++)
        if (*p == AV_PIX_FMT_D3D11 && d->hw_fmt == AV_PIX_FMT_D3D11) {
            // Shader-readable textures when they can be had, else FFmpeg's own (copied down).
            if (shader_readable_frames(d, ctx) < 0) av_buffer_unref(&ctx->hw_frames_ctx);
            return *p;
        }
#endif
    for (p = fmts; *p != AV_PIX_FMT_NONE; p++)
        if (d->hw_fmt != AV_PIX_FMT_NONE && *p == d->hw_fmt) return *p;
    // No hardware (or its setup failed and FFmpeg asks again): the first software format.
    for (p = fmts; *p != AV_PIX_FMT_NONE; p++) {
        const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get(*p);
        if (desc && !(desc->flags & AV_PIX_FMT_FLAG_HWACCEL)) return *p;
    }
    return AV_PIX_FMT_NONE;
}

static void free_ctx(rc_dec *d) {
    avcodec_free_context(&d->ctx);
    av_buffer_unref(&d->hwdev);
    d->hw_fmt = AV_PIX_FMT_NONE;
    d->shared_d3d11 = 0;
}

// One attempt: `type` is AV_HWDEVICE_TYPE_NONE for software.
static int try_open(rc_dec *d, int codec, enum AVHWDeviceType type, char *backend, int blen, char *err, int errlen) {
    const AVCodec *c;
    if (codec == RC_CODEC_HEVC)
        c = avcodec_find_decoder(AV_CODEC_ID_HEVC);
    else if (codec == RC_CODEC_H264)
        c = avcodec_find_decoder(AV_CODEC_ID_H264);
    else if (type != AV_HWDEVICE_TYPE_NONE)
        c = avcodec_find_decoder_by_name("av1"); // FFmpeg's own AV1 decoder is hardware-only
    else
        c = avcodec_find_decoder_by_name("libdav1d");
    if (!c) {
        set_err(err, errlen, "no decoder for this codec", 0);
        return -1;
    }
    d->hw_fmt = AV_PIX_FMT_NONE;
    if (type != AV_HWDEVICE_TYPE_NONE) {
        for (int i = 0;; i++) {
            const AVCodecHWConfig *cfg = avcodec_get_hw_config(c, i);
            if (!cfg) break;
            if ((cfg->methods & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX) && cfg->device_type == type) {
                d->hw_fmt = cfg->pix_fmt;
                break;
            }
        }
        if (d->hw_fmt == AV_PIX_FMT_NONE) {
            set_err(err, errlen, "decoder has no such hardware config", 0);
            return -1;
        }
        int r = 0;
#ifdef _WIN32
        if (type == AV_HWDEVICE_TYPE_D3D11VA && (d->hwdev = shared_d3d11_device())) d->shared_d3d11 = 1;
#endif
        if (!d->hwdev) r = av_hwdevice_ctx_create(&d->hwdev, type, NULL, NULL, 0);
        if (r < 0) {
            set_err(err, errlen, "hardware device", r);
            d->hw_fmt = AV_PIX_FMT_NONE;
            return r;
        }
    }
    d->ctx = avcodec_alloc_context3(c);
    if (!d->ctx) {
        free_ctx(d);
        set_err(err, errlen, "avcodec_alloc_context3", 0);
        return -1;
    }
    d->ctx->opaque = d;
    d->ctx->get_format = get_format;
    d->ctx->flags |= AV_CODEC_FLAG_LOW_DELAY;
    if (d->hwdev) {
        d->ctx->hw_device_ctx = av_buffer_ref(d->hwdev);
        d->ctx->thread_count = 1;
        // Pictures handed to the presenter stay in the decoder's pool for a while.
        d->ctx->extra_hw_frames = 4;
    } else {
        d->ctx->thread_type = FF_THREAD_SLICE; // frame threads would hold pictures back
        d->ctx->thread_count = 0;
    }
    AVDictionary *opts = NULL;
    if (!d->hwdev && codec == RC_CODEC_AV1) av_dict_set(&opts, "max_frame_delay", "1", 0);
    int r = avcodec_open2(d->ctx, c, &opts);
    av_dict_free(&opts);
    if (r < 0) {
        free_ctx(d);
        set_err(err, errlen, "avcodec_open2", r);
        return r;
    }
    snprintf(backend, blen, "%s%s%s%s", c->name, d->hwdev ? "+" : "", d->hwdev ? av_hwdevice_get_type_name(type) : "", d->shared_d3d11 ? " (shared device)" : "");
    return 0;
}

int rc_dec_open(int codec, int hw, rc_dec **out, char *backend, int backend_len, char *err, int errlen) {
    rc_dec *d = calloc(1, sizeof *d);
    if (!d) return -1;
    d->hw_fmt = AV_PIX_FMT_NONE;
    d->pkt = av_packet_alloc();
    d->out = av_frame_alloc();
    d->tmp = av_frame_alloc();
    d->sw = av_frame_alloc();
    enum AVHWDeviceType order[3];
    int n = 0;
    switch (hw) {
    case RC_HW_AUTO:
#ifdef _WIN32
        // Windows: Direct3D 11 video decode (any vendor), then software.
        order[n++] = AV_HWDEVICE_TYPE_D3D11VA;
#else
        order[n++] = AV_HWDEVICE_TYPE_CUDA;
        order[n++] = AV_HWDEVICE_TYPE_VAAPI;
#endif
        order[n++] = AV_HWDEVICE_TYPE_NONE;
        break;
    case RC_HW_D3D11: order[n++] = AV_HWDEVICE_TYPE_D3D11VA; break;
    case RC_HW_CUDA: order[n++] = AV_HWDEVICE_TYPE_CUDA; break;
#ifndef _WIN32
    case RC_HW_VAAPI: order[n++] = AV_HWDEVICE_TYPE_VAAPI; break;
#endif
    default: order[n++] = AV_HWDEVICE_TYPE_NONE; break;
    }
    int r = -1;
    for (int i = 0; i < n && r < 0; i++) r = try_open(d, codec, order[i], backend, backend_len, err, errlen);
    if (r < 0) {
        rc_dec_close(d);
        return r;
    }
    *out = d;
    return 0;
}

int rc_dec_decode(rc_dec *d, const uint8_t *data, int size, int *got, char *err, int errlen) {
    *got = 0;
    d->pkt->data = (uint8_t *)data; // not refcounted: libavcodec copies it, with padding
    d->pkt->size = size;
    int r = avcodec_send_packet(d->ctx, d->pkt);
    d->pkt->data = NULL;
    d->pkt->size = 0;
    if (r < 0 && r != AVERROR(EAGAIN)) {
        set_err(err, errlen, "avcodec_send_packet", r);
        return r;
    }
    for (;;) {
        r = avcodec_receive_frame(d->ctx, d->tmp);
        if (r == AVERROR(EAGAIN) || r == AVERROR_EOF) break;
        if (r < 0) {
            set_err(err, errlen, "avcodec_receive_frame", r);
            return r;
        }
        av_frame_unref(d->out);
        av_frame_move_ref(d->out, d->tmp);
        d->have = 1;
        *got = 1;
    }
    return 0;
}

int rc_dec_frame(rc_dec *d, rc_frame *o, char *err, int errlen) {
    if (!d->have) {
        set_err(err, errlen, "no picture yet", 0);
        return -1;
    }
    const AVFrame *f = d->out, *src = d->out;
    memset(o, 0, sizeof *o);
    if (f->hw_frames_ctx && f->format == AV_PIX_FMT_CUDA && d->keep_cuda) {
        // Stay on the GPU: device pointers, for rc_gl_copy and rc_frame_luma.
        AVHWFramesContext *fc = (AVHWFramesContext *)f->hw_frames_ctx->data;
        AVCUDADeviceContext *dc = fc->device_ctx->hwctx;
        switch (fc->sw_format) {
        case AV_PIX_FMT_NV12: o->format = RC_FMT_NV12; break;
        case AV_PIX_FMT_YUV444P: o->format = RC_FMT_YUV444P; break;
        default: {
            char what[96];
            snprintf(what, sizeof what, "unsupported CUDA format %s", av_get_pix_fmt_name(fc->sw_format));
            set_err(err, errlen, what, 0);
            return -1;
        }
        }
        AVFrame *ref = av_frame_clone(f);
        if (!ref) {
            set_err(err, errlen, "av_frame_clone", 0);
            return -1;
        }
        o->width = f->width;
        o->height = f->height;
        o->full_range = f->color_range == AVCOL_RANGE_JPEG;
        o->matrix = f->colorspace;
        o->hw = 1;
        o->cuda = 1;
        o->cuctx = dc->cuda_ctx;
        int planes = o->format == RC_FMT_NV12 ? 2 : 3;
        for (int i = 0; i < planes; i++) {
            o->data[i] = ref->data[i];
            o->linesize[i] = ref->linesize[i];
        }
        o->ref = ref;
        return 0;
    }
#ifdef _WIN32
    if (d->keep_cuda && zero_copy_frame(f)) {
        // Stay on the GPU: the texture and slice, for the presenter and rc_frame_luma.
        AVFrame *ref = av_frame_clone(f);
        if (!ref) {
            set_err(err, errlen, "av_frame_clone", 0);
            return -1;
        }
        D3D11_TEXTURE2D_DESC td;
        ID3D11Texture2D_GetDesc((ID3D11Texture2D *)ref->data[0], &td);
        o->format = RC_FMT_NV12;
        o->width = f->width;
        o->height = f->height;
        o->full_range = f->color_range == AVCOL_RANGE_JPEG;
        o->matrix = f->colorspace;
        o->hw = 1;
        o->d3d11 = 1;
        o->texture = ref->data[0];
        o->slice = (int)(intptr_t)ref->data[1];
        o->tex_width = (int)td.Width;
        o->tex_height = (int)td.Height;
        o->ref = ref;
        return 0;
    }
#endif
    if (f->hw_frames_ctx) {
        double t0 = now_ms();
        av_frame_unref(d->sw);
        int r = av_hwframe_transfer_data(d->sw, f, 0);
        if (r < 0) {
            set_err(err, errlen, "av_hwframe_transfer_data", r);
            return r;
        }
        o->transfer_ms = now_ms() - t0;
        o->hw = 1;
        src = d->sw;
    }
    int full = f->color_range == AVCOL_RANGE_JPEG;
    switch (src->format) {
    case AV_PIX_FMT_NV12: o->format = RC_FMT_NV12; break;
    case AV_PIX_FMT_YUVJ420P: full = 1; /* fallthrough */
    case AV_PIX_FMT_YUV420P: o->format = RC_FMT_YUV420P; break;
    case AV_PIX_FMT_YUVJ444P: full = 1; /* fallthrough */
    case AV_PIX_FMT_YUV444P: o->format = RC_FMT_YUV444P; break;
    default: {
        char what[96];
        snprintf(what, sizeof what, "unsupported pixel format %s", av_get_pix_fmt_name(src->format));
        set_err(err, errlen, what, 0);
        return -1;
    }
    }
    o->width = f->width;
    o->height = f->height;
    o->full_range = full;
    o->matrix = f->colorspace;
    AVFrame *ref = av_frame_clone(src);
    if (!ref) {
        set_err(err, errlen, "av_frame_clone", 0);
        return -1;
    }
    int planes = o->format == RC_FMT_NV12 ? 2 : 3;
    for (int i = 0; i < planes; i++) {
        o->data[i] = ref->data[i];
        o->linesize[i] = ref->linesize[i];
    }
    o->ref = ref;
    return 0;
}

void rc_dec_keep_cuda(rc_dec *d, int on) { d->keep_cuda = on; }

int rc_frame_luma(const rc_frame *f, int x, int y, int w, int h, double *mean, char *err, int errlen) {
    if (x < 0 || y < 0 || w <= 0 || h <= 0 || x + w > f->width || y + h > f->height) {
        set_err(err, errlen, "probe outside the picture", 0);
        return -1;
    }
    uint8_t *buf = malloc((size_t)w * h);
    if (!buf) return -1;
#ifdef _WIN32
    if (f->d3d11) {
        int r = d3d11_luma(f, x, y, w, h, buf, err, errlen);
        if (r < 0) {
            free(buf);
            return -1;
        }
    } else
#endif
    if (f->cuda) {
        if (cuda_fns(err, errlen) < 0) {
            free(buf);
            return -1;
        }
        CUDA_MEMCPY2D c = {0};
        c.srcMemoryType = CU_MEMORYTYPE_DEVICE;
        c.srcDevice = (CUdeviceptr)(f->data[0] + (size_t)y * f->linesize[0] + x);
        c.srcPitch = f->linesize[0];
        c.dstMemoryType = CU_MEMORYTYPE_HOST;
        c.dstHost = buf;
        c.dstPitch = w;
        c.WidthInBytes = w;
        c.Height = h;
        CUcontext dummy;
        if (cu_check(g_cu->cuCtxPushCurrent(f->cuctx), "cuCtxPushCurrent", err, errlen) < 0) {
            free(buf);
            return -1;
        }
        int r = cu_check(g_cu->cuMemcpy2D(&c), "cuMemcpy2D", err, errlen);
        g_cu->cuCtxPopCurrent(&dummy);
        if (r < 0) {
            free(buf);
            return -1;
        }
    } else {
        for (int row = 0; row < h; row++) memcpy(buf + (size_t)row * w, f->data[0] + (size_t)(y + row) * f->linesize[0] + x, w);
    }
    uint64_t sum = 0;
    for (size_t i = 0; i < (size_t)w * h; i++) sum += buf[i];
    free(buf);
    *mean = (double)sum / ((double)w * h);
    return 0;
}

struct rc_gl {
    CUcontext ctx;
    CUgraphicsResource res[3];
};

int rc_gl_open(void *cuctx, rc_gl **out, char *err, int errlen) {
    if (cuda_fns(err, errlen) < 0) return -1;
    rc_gl *g = calloc(1, sizeof *g);
    if (!g) return -1;
    g->ctx = cuctx;
    *out = g;
    return 0;
}

int rc_gl_register(rc_gl *g, int plane, unsigned tex, char *err, int errlen) {
    CUcontext dummy;
    if (cu_check(g_cu->cuCtxPushCurrent(g->ctx), "cuCtxPushCurrent", err, errlen) < 0) return -1;
    if (g->res[plane]) g_cu->cuGraphicsUnregisterResource(g->res[plane]);
    g->res[plane] = NULL;
    int r = cu_check(g_cu->cuGraphicsGLRegisterImage(&g->res[plane], tex, 0x0DE1 /* GL_TEXTURE_2D */,
                                                     CU_GRAPHICS_REGISTER_FLAGS_WRITE_DISCARD),
                     "cuGraphicsGLRegisterImage", err, errlen);
    g_cu->cuCtxPopCurrent(&dummy);
    return r;
}

int rc_gl_copy(rc_gl *g, const rc_frame *f, char *err, int errlen) {
    int planes = f->format == RC_FMT_NV12 ? 2 : 3;
    int cw = f->format == RC_FMT_YUV444P ? f->width : (f->width + 1) / 2;
    int ch = f->format == RC_FMT_YUV444P ? f->height : (f->height + 1) / 2;
    CUcontext dummy;
    if (cu_check(g_cu->cuCtxPushCurrent(g->ctx), "cuCtxPushCurrent", err, errlen) < 0) return -1;
    int r = cu_check(g_cu->cuGraphicsMapResources(planes, g->res, 0), "cuGraphicsMapResources", err, errlen);
    for (int i = 0; i < planes && r == 0; i++) {
        CUarray arr;
        r = cu_check(g_cu->cuGraphicsSubResourceGetMappedArray(&arr, g->res[i], 0, 0), "GetMappedArray", err, errlen);
        if (r) break;
        CUDA_MEMCPY2D c = {0};
        c.srcMemoryType = CU_MEMORYTYPE_DEVICE;
        c.srcDevice = (CUdeviceptr)f->data[i];
        c.srcPitch = f->linesize[i];
        c.dstMemoryType = CU_MEMORYTYPE_ARRAY;
        c.dstArray = arr;
        c.WidthInBytes = i == 0 ? f->width : (f->format == RC_FMT_NV12 ? cw * 2 : cw);
        c.Height = i == 0 ? f->height : ch;
        r = cu_check(g_cu->cuMemcpy2DAsync(&c, 0), "cuMemcpy2DAsync", err, errlen);
    }
    // Unmapping orders the copies before any GL work issued after it.
    g_cu->cuGraphicsUnmapResources(planes, g->res, 0);
    g_cu->cuCtxPopCurrent(&dummy);
    return r;
}

void rc_gl_close(rc_gl *g) {
    if (!g) return;
    CUcontext dummy;
    if (g_cu->cuCtxPushCurrent(g->ctx) == CUDA_SUCCESS) {
        for (int i = 0; i < 3; i++)
            if (g->res[i]) g_cu->cuGraphicsUnregisterResource(g->res[i]);
        g_cu->cuCtxPopCurrent(&dummy);
    }
    free(g);
}

void rc_frame_free(void *ref) {
    AVFrame *f = ref;
    av_frame_free(&f);
}

void rc_dec_close(rc_dec *d) {
    if (!d) return;
    free_ctx(d);
    av_packet_free(&d->pkt);
    av_frame_free(&d->out);
    av_frame_free(&d->tmp);
    av_frame_free(&d->sw);
    free(d);
}
