// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// CUDA + NVENC for remoted (Route A: Vulkan exports memory as an opaque fd,
// CUDA imports it, NVENC encodes from the CUDA pointer). See nvenc_shim.h.
#include "nvenc_shim.h"

#include <ffnvcodec/dynlink_loader.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#define MAX_SLOTS 8

struct RdCuda {
    CudaFunctions *cu;
    CUcontext ctx;
};

struct RdExtMem {
    CUexternalMemory ext;
    CUdeviceptr ptr;
};

struct RdEnc {
    RdCuda *cuda;
    NvencFunctions *nvf;
    NV_ENCODE_API_FUNCTION_LIST api;
    void *enc;
    NV_ENC_INITIALIZE_PARAMS init;
    NV_ENC_CONFIG cfg;
    RdEncParams p;
    NV_ENC_REGISTERED_PTR reg[MAX_SLOTS];
    int nslots;
    NV_ENC_OUTPUT_PTR bs;
    uint8_t *buf;
    size_t cap;
    NVENC_EXTERNAL_ME_HINT *hevc_hints;
    NVENC_EXTERNAL_ME_SB_HINT *av1_hints;
    uint32_t n_hints;
    uint32_t max_qp; // the RC's ceiling before the delta map; 0: none
    int max_qp_used; // a ceiling was set: enableMaxQP stays on (at the top QP for none)
};

static void fail(char *err, int errlen, const char *fmt, ...)
{
    if (!err || errlen <= 0)
        return;
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(err, errlen, fmt, ap);
    va_end(ap);
}

static double now_ms(void)
{
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1e3 + t.tv_nsec / 1e6;
}

static int push(RdCuda *c) { return c->cu->cuCtxPushCurrent(c->ctx); }
static void pop(RdCuda *c)
{
    CUcontext dummy;
    c->cu->cuCtxPopCurrent(&dummy);
}

int rd_cuda_open(const uint8_t uuid[16], RdCuda **out, char *err, int errlen)
{
    RdCuda *c = calloc(1, sizeof(*c));
    if (cuda_load_functions(&c->cu, NULL) < 0) {
        fail(err, errlen, "cannot load libcuda.so.1");
        free(c);
        return -1;
    }
    CUresult r = c->cu->cuInit(0);
    if (r) {
        fail(err, errlen, "cuInit: %d", r);
        goto bad;
    }
    int n = 0;
    c->cu->cuDeviceGetCount(&n);
    CUdevice dev = -1;
    for (int i = 0; i < n; i++) {
        CUdevice d;
        CUuuid id;
        if (c->cu->cuDeviceGet(&d, i) || !c->cu->cuDeviceGetUuid || c->cu->cuDeviceGetUuid(&id, d))
            continue;
        if (!memcmp(id.bytes, uuid, 16)) {
            dev = d;
            break;
        }
    }
    if (dev < 0) {
        fail(err, errlen, "no CUDA device matches the Vulkan device UUID");
        goto bad;
    }
    r = c->cu->cuCtxCreate(&c->ctx, 0, dev);
    if (r) {
        fail(err, errlen, "cuCtxCreate: %d", r);
        goto bad;
    }
    pop(c);
    *out = c;
    return 0;
bad:
    cuda_free_functions(&c->cu);
    free(c);
    return -1;
}

void rd_cuda_close(RdCuda *c)
{
    if (!c)
        return;
    c->cu->cuCtxDestroy(c->ctx);
    cuda_free_functions(&c->cu);
    free(c);
}

int rd_cuda_import(RdCuda *c, int fd, uint64_t size, int dedicated, RdExtMem **out, uint64_t *devptr, char *err,
                   int errlen)
{
    if (!c->cu->cuImportExternalMemory) {
        fail(err, errlen, "driver lacks cuImportExternalMemory");
        return -1;
    }
    push(c);
    RdExtMem *m = calloc(1, sizeof(*m));
    CUDA_EXTERNAL_MEMORY_HANDLE_DESC hd = {.type = CU_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD,
                                           .handle.fd = fd,
                                           .size = size,
                                           .flags = dedicated ? 0x1 /* CUDA_EXTERNAL_MEMORY_DEDICATED */ : 0};
    CUresult r = c->cu->cuImportExternalMemory(&m->ext, &hd);
    if (r) {
        fail(err, errlen, "cuImportExternalMemory: %d", r);
        free(m);
        pop(c);
        return -1;
    }
    CUDA_EXTERNAL_MEMORY_BUFFER_DESC bd = {.offset = 0, .size = size};
    r = c->cu->cuExternalMemoryGetMappedBuffer(&m->ptr, m->ext, &bd);
    pop(c);
    if (r) {
        fail(err, errlen, "cuExternalMemoryGetMappedBuffer: %d", r);
        c->cu->cuDestroyExternalMemory(m->ext);
        free(m);
        return -1;
    }
    *devptr = m->ptr;
    *out = m;
    return 0;
}

void rd_cuda_release(RdCuda *c, RdExtMem *m)
{
    if (!m)
        return;
    push(c);
    c->cu->cuMemFree(m->ptr);
    c->cu->cuDestroyExternalMemory(m->ext);
    pop(c);
    free(m);
}

static const char *last_error(RdEnc *e)
{
    const char *s = e->enc && e->api.nvEncGetLastErrorString ? e->api.nvEncGetLastErrorString(e->enc) : NULL;
    return s ? s : "";
}

static void set_rc(RdEnc *e, uint32_t kbps)
{
    if (kbps < 100)
        kbps = 100;
    NV_ENC_RC_PARAMS *rc = &e->cfg.rcParams;
    rc->rateControlMode = NV_ENC_PARAMS_RC_CBR;
    rc->averageBitRate = kbps * 1000;
    rc->maxBitRate = kbps * 1000;
    rc->vbvBufferSize = (uint32_t)((uint64_t)kbps * 10 * e->p.vbv_pct / e->p.fps);
    rc->vbvInitialDelay = rc->vbvBufferSize;
    // Once a ceiling was set it stays enabled, at the codec's top QP for none: a reconfigure
    // with enableMaxQP off keeps the last maxQP, and maxQP 0 (or a stale ceiling) then holds
    // the frames at that QP whatever they cost (measured live: QP 0, 270 kB frames at 7 Mbit/s).
    uint32_t q = e->max_qp ? e->max_qp : e->p.codec == RD_CODEC_AV1 ? 255 : 51;
    rc->enableMaxQP = e->max_qp_used;
    rc->maxQP = (NV_ENC_QP){q, q, q};
}

static int open_(RdCuda *c, const RdEncParams *p, RdEnc **out, char *err, int errlen)
{
    RdEnc *e = calloc(1, sizeof(*e));
    e->cuda = c;
    e->p = *p;
    if (!e->p.vbv_pct)
        e->p.vbv_pct = 100;
    if (!e->p.fps)
        e->p.fps = 60;
    if (nvenc_load_functions(&e->nvf, NULL) < 0) {
        fail(err, errlen, "cannot load libnvidia-encode.so.1");
        free(e);
        return -1;
    }
    e->api.version = NV_ENCODE_API_FUNCTION_LIST_VER;
    NVENCSTATUS st = e->nvf->NvEncodeAPICreateInstance(&e->api);
    if (st) {
        fail(err, errlen, "NvEncodeAPICreateInstance: %d", st);
        goto bad;
    }
    NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS op = {.version = NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER,
                                               .deviceType = NV_ENC_DEVICE_TYPE_CUDA,
                                               .device = c->ctx,
                                               .apiVersion = NVENCAPI_VERSION};
    st = e->api.nvEncOpenEncodeSessionEx(&op, &e->enc);
    if (st) {
        fail(err, errlen, "nvEncOpenEncodeSessionEx: %d", st);
        goto bad;
    }
    GUID codec = p->codec == RD_CODEC_AV1 ? NV_ENC_CODEC_AV1_GUID : NV_ENC_CODEC_HEVC_GUID;
    NV_ENC_PRESET_CONFIG pc = {.version = NV_ENC_PRESET_CONFIG_VER, .presetCfg = {.version = NV_ENC_CONFIG_VER}};
    st = e->api.nvEncGetEncodePresetConfigEx(e->enc, codec, NV_ENC_PRESET_P1_GUID, NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY,
                                             &pc);
    if (st) {
        fail(err, errlen, "nvEncGetEncodePresetConfigEx: %d %s", st, last_error(e));
        goto bad;
    }
    //: P1, ultra-low-latency, CBR with a one-frame VBV, infinite
    // GOP, no B-frames, forced 2-way split, per-frame bitrate changes.
    e->cfg = pc.presetCfg;
    e->cfg.version = NV_ENC_CONFIG_VER;
    e->cfg.gopLength = NVENC_INFINITE_GOPLENGTH;
    e->cfg.frameIntervalP = 1;
    set_rc(e, p->kbps);
    int chroma = p->chroma444 ? 3 : 1;
    if (p->codec == RD_CODEC_HEVC) {
        NV_ENC_CONFIG_HEVC *h = &e->cfg.encodeCodecConfig.hevcConfig;
        e->cfg.profileGUID = chroma == 3 ? NV_ENC_HEVC_PROFILE_FREXT_GUID : NV_ENC_HEVC_PROFILE_MAIN_GUID;
        h->chromaFormatIDC = chroma;
        h->idrPeriod = NVENC_INFINITE_GOPLENGTH;
        h->repeatSPSPPS = 1;
        h->maxNumRefFramesInDPB = 8; // room for reference invalidation (integration.md 2)
        if (p->m1) {
            h->maxCUSize = NV_ENC_HEVC_CUSIZE_32x32; // the QP delta map is per 32x32 CTB
            // On-demand intra refresh (forceIntraRefreshWithFrameCnt), the period never
            // reached. Unused now: a lost frame older than the DPB gets an IDR (integration.md 2).
            h->enableIntraRefresh = 1;
            h->intraRefreshPeriod = 1u << 30;
            h->intraRefreshCnt = 8;
            h->outputRecoveryPointSEI = 1;
        }
        NV_ENC_CONFIG_HEVC_VUI_PARAMETERS *v = &h->hevcVUIParameters;
        v->videoSignalTypePresentFlag = 1;
        v->videoFormat = NV_ENC_VUI_VIDEO_FORMAT_UNSPECIFIED;
        v->videoFullRangeFlag = 1;
        v->colourDescriptionPresentFlag = 1;
        v->colourPrimaries = NV_ENC_VUI_COLOR_PRIMARIES_BT709;
        v->transferCharacteristics = NV_ENC_VUI_TRANSFER_CHARACTERISTIC_SRGB;
        v->colourMatrix = NV_ENC_VUI_MATRIX_COEFFS_BT709;
        if (chroma == 1 && p->input != RD_INPUT_ARGB) {
            // The shader box-filters 2x2: chroma sits at the centre (type 1), not HEVC's default left (type 0).
            v->chromaSampleLocationFlag = 1;
            v->chromaSampleLocationTop = 1;
            v->chromaSampleLocationBot = 1;
        }
    } else {
        NV_ENC_CONFIG_AV1 *a = &e->cfg.encodeCodecConfig.av1Config;
        e->cfg.profileGUID = NV_ENC_AV1_PROFILE_MAIN_GUID;
        a->chromaFormatIDC = 1;
        a->idrPeriod = NVENC_INFINITE_GOPLENGTH;
        a->repeatSeqHdr = 1;
        a->maxNumRefFramesInDPB = 8;
        if (p->m1) {
            a->maxPartSize = NV_ENC_AV1_PART_SIZE_64x64;
            a->enableIntraRefresh = 1;
            a->intraRefreshPeriod = 1u << 30;
            a->intraRefreshCnt = 8;
        }
        a->colorPrimaries = NV_ENC_VUI_COLOR_PRIMARIES_BT709;
        a->transferCharacteristics = NV_ENC_VUI_TRANSFER_CHARACTERISTIC_SRGB;
        a->matrixCoefficients = NV_ENC_VUI_MATRIX_COEFFS_BT709;
        a->colorRange = 1;
    }
    NV_ENC_INITIALIZE_PARAMS *in = &e->init;
    in->version = NV_ENC_INITIALIZE_PARAMS_VER;
    in->encodeGUID = codec;
    in->presetGUID = NV_ENC_PRESET_P1_GUID;
    in->tuningInfo = NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY;
    in->encodeWidth = in->darWidth = p->width;
    in->encodeHeight = in->darHeight = p->height;
    in->frameRateNum = e->p.fps;
    in->frameRateDen = 1;
    in->enablePTD = 1;
    in->splitEncodeMode = p->split;
    in->encodeConfig = &e->cfg;
    if (p->m1) {
        e->cfg.rcParams.qpMapMode = NV_ENC_QP_MAP_DELTA;
        in->enableExternalMEHints = 1;
        if (p->codec == RD_CODEC_AV1)
            in->maxMEHintCountsPerBlock[0].numCandsPerSb = 1;
        else
            in->maxMEHintCountsPerBlock[0].numCandsPerBlk16x16 = 1;
    }
    st = e->api.nvEncInitializeEncoder(e->enc, in);
    if (st) {
        fail(err, errlen, "nvEncInitializeEncoder: %d %s", st, last_error(e));
        goto bad;
    }
    NV_ENC_CREATE_BITSTREAM_BUFFER b = {.version = NV_ENC_CREATE_BITSTREAM_BUFFER_VER};
    st = e->api.nvEncCreateBitstreamBuffer(e->enc, &b);
    if (st) {
        fail(err, errlen, "nvEncCreateBitstreamBuffer: %d %s", st, last_error(e));
        goto bad;
    }
    e->bs = b.bitstreamBuffer;
    *out = e;
    return 0;
bad:
    if (e->enc)
        e->api.nvEncDestroyEncoder(e->enc);
    nvenc_free_functions(&e->nvf);
    free(e);
    return -1;
}

static int register_(RdEnc *e, uint64_t devptr, uint32_t pitch, int *slot, char *err, int errlen)
{
    if (e->nslots == MAX_SLOTS) {
        fail(err, errlen, "too many input surfaces");
        return -1;
    }
    NV_ENC_BUFFER_FORMAT fmt = e->p.input == RD_INPUT_NV12     ? NV_ENC_BUFFER_FORMAT_NV12
                               : e->p.input == RD_INPUT_YUV444 ? NV_ENC_BUFFER_FORMAT_YUV444
                                                               : NV_ENC_BUFFER_FORMAT_ARGB;
    NV_ENC_REGISTER_RESOURCE r = {.version = NV_ENC_REGISTER_RESOURCE_VER,
                                  .resourceType = NV_ENC_INPUT_RESOURCE_TYPE_CUDADEVICEPTR,
                                  .width = e->p.width,
                                  .height = e->p.height,
                                  .pitch = pitch,
                                  .resourceToRegister = (void *)(uintptr_t)devptr,
                                  .bufferFormat = fmt,
                                  .bufferUsage = NV_ENC_INPUT_IMAGE};
    NVENCSTATUS st = e->api.nvEncRegisterResource(e->enc, &r);
    if (st) {
        fail(err, errlen, "nvEncRegisterResource: %d %s", st, last_error(e));
        return -1;
    }
    e->reg[e->nslots] = r.registeredResource;
    *slot = e->nslots++;
    return 0;
}

static int encode_(RdEnc *e, int slot, uint64_t pts, const RdPicOpts *o, RdPacket *out, char *err, int errlen)
{
    int force_idr = o && o->force_idr;
    NV_ENC_MAP_INPUT_RESOURCE map = {.version = NV_ENC_MAP_INPUT_RESOURCE_VER, .registeredResource = e->reg[slot]};
    NVENCSTATUS st = e->api.nvEncMapInputResource(e->enc, &map);
    if (st) {
        fail(err, errlen, "nvEncMapInputResource: %d %s", st, last_error(e));
        return -1;
    }
    NV_ENC_PIC_PARAMS pp = {.version = NV_ENC_PIC_PARAMS_VER,
                            .inputWidth = e->p.width,
                            .inputHeight = e->p.height,
                            .inputBuffer = map.mappedResource,
                            .bufferFmt = map.mappedBufferFmt,
                            .outputBitstream = e->bs,
                            .pictureStruct = NV_ENC_PIC_STRUCT_FRAME,
                            .inputTimeStamp = pts};
    if (force_idr)
        pp.encodePicFlags = NV_ENC_PIC_FLAG_FORCEIDR | NV_ENC_PIC_FLAG_OUTPUT_SPSPPS;
    if (o && o->force_ir_frames && !force_idr) {
        if (e->p.codec == RD_CODEC_AV1)
            pp.codecPicParams.av1PicParams.forceIntraRefreshWithFrameCnt = o->force_ir_frames;
        else
            pp.codecPicParams.hevcPicParams.forceIntraRefreshWithFrameCnt = o->force_ir_frames;
    }
    if (o && o->qp_delta && e->p.m1) {
        pp.qpDeltaMap = (int8_t *)o->qp_delta;
        pp.qpDeltaMapSize = o->qp_delta_size;
        // the ceiling bounds the rate controller's QP, not each cell's QP after its delta
        if (e->max_qp_used)
            pp.encodePicFlags |= NV_ENC_PIC_FLAG_PREQPDELTAMAXQP;
    }
    // With hints enabled at init, every P frame must carry them ("SetupCEAHints failed"
    // otherwise): blocks without a known vector are marked invalid (refidx 31), which
    // a probe measured as equal to zero vectors.
    if (e->p.m1 && !force_idr) {
        uint32_t blk = e->p.codec == RD_CODEC_AV1 ? 64 : 16;
        uint32_t n = ((e->p.width + blk - 1) / blk) * ((e->p.height + blk - 1) / blk);
        int have = o && o->mv_xy && o->n_mv == n;
        if (n > e->n_hints) {
            free(e->hevc_hints), free(e->av1_hints);
            e->hevc_hints = calloc(n, sizeof(*e->hevc_hints));
            e->av1_hints = calloc(n, sizeof(*e->av1_hints));
            e->n_hints = n;
        }
        for (uint32_t k = 0; k < n; k++) {
            int valid = have && o->mv_valid[k];
            int x = valid ? o->mv_xy[2 * k] : 0, y = valid ? o->mv_xy[2 * k + 1] : 0;
            if (e->p.codec == RD_CODEC_AV1) {
                NVENC_EXTERNAL_ME_SB_HINT *h = &e->av1_hints[k];
                memset(h, 0, sizeof(*h));
                h->refidx = valid ? 0 : 31;
                h->cu_size = 3; // one 64x64 CU per SB
                h->last_of_cu = 1, h->last_of_sb = 1;
                x = x < -1023 ? -1023 : x > 1023 ? 1023 : x;
                y = y < -511 ? -511 : y > 511 ? 511 : y;
                h->mvx = x * 4, h->mvy = y * 4; // quarter pel
            } else {
                NVENC_EXTERNAL_ME_HINT *h = &e->hevc_hints[k];
                memset(h, 0, sizeof(*h));
                h->refidx = valid ? 0 : 31;
                h->mvx = x < -2047 ? -2047 : x > 2047 ? 2047 : x;
                h->mvy = y < -511 ? -511 : y > 511 ? 511 : y;
                h->lastofPart = 1, h->lastOfMB = 1;
            }
        }
        if (e->p.codec == RD_CODEC_AV1) {
            pp.meHintCountsPerBlock[0].numCandsPerSb = 1;
            pp.meExternalSbHints = e->av1_hints;
            pp.meSbHintsCount = n;
        } else {
            pp.meHintCountsPerBlock[0].numCandsPerBlk16x16 = 1;
            pp.meExternalHints = e->hevc_hints;
        }
    }
    double t0 = now_ms();
    st = e->api.nvEncEncodePicture(e->enc, &pp);
    if (st) {
        fail(err, errlen, "nvEncEncodePicture: %d %s", st, last_error(e));
        e->api.nvEncUnmapInputResource(e->enc, map.mappedResource);
        return -1;
    }
    NV_ENC_LOCK_BITSTREAM lb = {.version = NV_ENC_LOCK_BITSTREAM_VER, .outputBitstream = e->bs};
    st = e->api.nvEncLockBitstream(e->enc, &lb);
    double t1 = now_ms();
    if (st) {
        fail(err, errlen, "nvEncLockBitstream: %d %s", st, last_error(e));
        e->api.nvEncUnmapInputResource(e->enc, map.mappedResource);
        return -1;
    }
    if (lb.bitstreamSizeInBytes > e->cap) {
        e->cap = lb.bitstreamSizeInBytes * 2;
        e->buf = realloc(e->buf, e->cap);
    }
    memcpy(e->buf, lb.bitstreamBufferPtr, lb.bitstreamSizeInBytes);
    out->data = e->buf;
    out->size = lb.bitstreamSizeInBytes;
    out->pic_type = lb.pictureType;
    out->avg_qp = lb.frameAvgQP;
    out->submit_to_ready_ms = t1 - t0;
    e->api.nvEncUnlockBitstream(e->enc, e->bs);
    e->api.nvEncUnmapInputResource(e->enc, map.mappedResource);
    return 0;
}

int rd_enc_set_bitrate(RdEnc *e, uint32_t kbps, char *err, int errlen)
{
    set_rc(e, kbps);
    NV_ENC_RECONFIGURE_PARAMS rp = {.version = NV_ENC_RECONFIGURE_PARAMS_VER, .reInitEncodeParams = e->init};
    rp.reInitEncodeParams.encodeConfig = &e->cfg;
    push(e->cuda);
    NVENCSTATUS st = e->api.nvEncReconfigureEncoder(e->enc, &rp);
    pop(e->cuda);
    if (st) {
        fail(err, errlen, "nvEncReconfigureEncoder: %d %s", st, last_error(e));
        return -1;
    }
    return 0;
}

int rd_enc_set_max_qp(RdEnc *e, uint32_t max_qp, char *err, int errlen)
{
    e->max_qp = max_qp;
    e->max_qp_used |= max_qp != 0;
    set_rc(e, e->cfg.rcParams.averageBitRate / 1000);
    NV_ENC_RECONFIGURE_PARAMS rp = {.version = NV_ENC_RECONFIGURE_PARAMS_VER, .reInitEncodeParams = e->init};
    rp.reInitEncodeParams.encodeConfig = &e->cfg;
    push(e->cuda);
    NVENCSTATUS st = e->api.nvEncReconfigureEncoder(e->enc, &rp);
    pop(e->cuda);
    if (st) {
        fail(err, errlen, "nvEncReconfigureEncoder (max QP %u): %d %s", max_qp, st, last_error(e));
        return -1;
    }
    return 0;
}

int rd_enc_set_rate(RdEnc *e, uint32_t kbps, uint32_t vbv_pct, int split, char *err, int errlen)
{
    e->p.vbv_pct = vbv_pct ? vbv_pct : 100;
    set_rc(e, kbps);
    NV_ENC_RECONFIGURE_PARAMS rp = {.version = NV_ENC_RECONFIGURE_PARAMS_VER, .reInitEncodeParams = e->init};
    rp.reInitEncodeParams.splitEncodeMode = split;
    rp.reInitEncodeParams.encodeConfig = &e->cfg;
    push(e->cuda);
    NVENCSTATUS st = e->api.nvEncReconfigureEncoder(e->enc, &rp);
    pop(e->cuda);
    if (st) {
        fail(err, errlen, "nvEncReconfigureEncoder (split %d, VBV %u%%): %d %s", split, vbv_pct, st, last_error(e));
        return -1;
    }
    e->init.splitEncodeMode = split;
    e->p.split = split;
    return 0;
}

void rd_enc_grid(RdEnc *e, uint32_t *qp_cell, uint32_t *mv_block)
{
    *qp_cell = e->p.codec == RD_CODEC_AV1 ? 64 : 32;
    *mv_block = e->p.codec == RD_CODEC_AV1 ? 64 : 16;
}

int rd_enc_invalidate(RdEnc *e, uint64_t pts, char *err, int errlen)
{
    push(e->cuda);
    NVENCSTATUS st = e->api.nvEncInvalidateRefFrames(e->enc, pts);
    pop(e->cuda);
    if (st) {
        fail(err, errlen, "nvEncInvalidateRefFrames: %d %s", st, last_error(e));
        return -1;
    }
    return 0;
}

void rd_enc_close(RdEnc *e)
{
    if (!e)
        return;
    for (int i = 0; i < e->nslots; i++)
        e->api.nvEncUnregisterResource(e->enc, e->reg[i]);
    if (e->bs)
        e->api.nvEncDestroyBitstreamBuffer(e->enc, e->bs);
    e->api.nvEncDestroyEncoder(e->enc);
    nvenc_free_functions(&e->nvf);
    free(e->buf);
    free(e->hevc_hints);
    free(e->av1_hints);
    free(e);
}

int rd_enc_register(RdEnc *e, uint64_t devptr, uint32_t pitch, int *slot, char *err, int errlen)
{
    push(e->cuda);
    int r = register_(e, devptr, pitch, slot, err, errlen);
    pop(e->cuda);
    return r;
}

int rd_enc_encode(RdEnc *e, int slot, uint64_t pts, int force_idr, RdPacket *out, char *err, int errlen)
{
    RdPicOpts o = {.force_idr = force_idr};
    push(e->cuda);
    int r = encode_(e, slot, pts, &o, out, err, errlen);
    pop(e->cuda);
    return r;
}

int rd_enc_encode2(RdEnc *e, int slot, uint64_t pts, const RdPicOpts *o, RdPacket *out, char *err, int errlen)
{
    push(e->cuda);
    int r = encode_(e, slot, pts, o, out, err, errlen);
    pop(e->cuda);
    return r;
}

int rd_enc_open(RdCuda *c, const RdEncParams *p, RdEnc **out, char *err, int errlen)
{
    push(c);
    int r = open_(c, p, out, err, errlen);
    pop(c);
    return r;
}
