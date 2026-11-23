// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import CoreMedia
import CoreVideo
import Foundation
import Metal
import RemoteCore
import RemoteKit
import VideoToolbox

/// A decoded picture, ready for the presenter: its planes as Metal textures.
final class DecodedFrame {
    let frameId: UInt32
    let pixelBuffer: CVPixelBuffer
    /// Kept alive until the GPU is done with them.
    let lumaRef: CVMetalTexture
    let chromaRef: CVMetalTexture
    let luma: MTLTexture
    let chroma: MTLTexture
    let width: Int
    let height: Int
    let fullRange: Bool
    let bt601: Bool
    /// Host time it was ready (decode-done on the presenter's side: integration.md 9).
    let readyHost: Double

    init(frameId: UInt32, pixelBuffer: CVPixelBuffer, lumaRef: CVMetalTexture, chromaRef: CVMetalTexture, luma: MTLTexture, chroma: MTLTexture,
         width: Int, height: Int, fullRange: Bool, bt601: Bool, readyHost: Double) {
        self.frameId = frameId
        self.pixelBuffer = pixelBuffer
        self.lumaRef = lumaRef
        self.chromaRef = chromaRef
        self.luma = luma
        self.chroma = chroma
        self.width = width
        self.height = height
        self.fullRange = fullRange
        self.bt601 = bt601
        self.readyHost = readyHost
    }
}

/// Called on the decoder's queue.
protocol DecoderDelegate: AnyObject {
    func decoded(_ f: DecodedFrame, rec: FrameRec)
    /// "VideoToolbox HEVC 4:4:4 · hardware".
    func decoderChanged(description: String, hardware: Bool)
}

/// VideoToolbox, one frame at a time on its own queue (client-linux's
/// `decode_loop`): the network's chain (`decodable`), a keyframe after a
/// fresh decoder, and the watchdog (three decodable frames with no picture
/// ask for a keyframe, at most every 500 ms; integration.md 2).
final class VideoDecoder: VideoSink {
    let queue = DispatchQueue(label: AppIdentity.label("decode"), qos: .userInteractive)
    private let core: RemoteClient
    private let clock: ClockMap
    private let stats: StatsRecorder
    private let cache: CVMetalTextureCache
    weak var delegate: DecoderDelegate?

    // Queue-confined state.
    private var codec: Codec?
    private var session: VTDecompressionSession?
    private var format: CMFormatDescription?
    private var paramKey: [UInt8] = []
    private var needKey = true
    private var lastKeyRequest = 0.0
    private var noPicture = 0
    private var okFrames = 0
    private var hardware = false

    private let lock = NSLock()
    private var latestBuffer: CVPixelBuffer?
    private var clickTest: ClickTest?

    init?(core: RemoteClient, clock: ClockMap, stats: StatsRecorder, device: MTLDevice) {
        self.core = core
        self.clock = clock
        self.stats = stats
        var c: CVMetalTextureCache?
        guard CVMetalTextureCacheCreate(kCFAllocatorDefault, nil, device, nil, &c) == kCVReturnSuccess, let tc = c else { return nil }
        cache = tc
    }

    /// The stream's codec (`Welcome`, `Format`): a change needs a new decoder.
    func setCodec(_ c: Codec) {
        queue.async {
            if self.codec != c {
                self.dropSession()
                self.needKey = true
            }
            self.codec = c
        }
    }

    /// A new connection: frame ids start over, and the first frame is a keyframe.
    func reconnected() {
        queue.async {
            self.needKey = true
            self.noPicture = 0
        }
    }

    func setClickTest(_ t: ClickTest?) {
        lock.lock()
        clickTest = t
        lock.unlock()
    }

    /// The last picture, for the card's thumbnail.
    func latestPicture() -> CVPixelBuffer? {
        lock.lock()
        defer { lock.unlock() }
        return latestBuffer
    }

    func stop() {
        queue.sync { self.dropSession() }
    }

    // MARK: VideoSink (the core's thread)

    func videoFrame(_ f: VideoFrame) {
        queue.async { self.decode(f) }
    }

    func videoFrameLost(_ id: UInt32) {
        stats.onLost()
    }

    // MARK: Decoding (the queue)

    private func requestKeyframe() {
        let now = hostNow()
        if now - lastKeyRequest > 0.5 {
            lastKeyRequest = now
            core.sendControl(msg: .keyframeRequest)
        }
    }

    private func dropSession() {
        if let s = session {
            VTDecompressionSessionWaitForAsynchronousFrames(s)
            VTDecompressionSessionInvalidate(s)
        }
        session = nil
        format = nil
        paramKey = []
        okFrames = 0
    }

    private func decode(_ f: VideoFrame) {
        guard f.decodable else {
            stats.onUndecodable()
            return
        }
        if needKey && !f.keyframe {
            requestKeyframe()
            return
        }
        let bytes = [UInt8](f.data)
        let c = codec ?? sniff(bytes)
        // The sample, and on a keyframe the format description.
        let sampleBytes: [UInt8]
        if c == .av1 {
            if f.keyframe, let obu = AV1.sequenceHeaderOBU(bytes) {
                updateFormatAV1(obu)
            }
            sampleBytes = AV1.sample(bytes)
        } else if c == .h264 {
            let nals = AnnexB.split(bytes)
            if f.keyframe, let ps = H264.parameterSets(nals) {
                updateFormatH264(ps)
            }
            sampleBytes = AnnexB.lengthPrefixed(H264.sampleNALs(nals))
        } else {
            let nals = AnnexB.split(bytes)
            if f.keyframe, let ps = HEVC.parameterSets(nals) {
                updateFormatHEVC(ps, is444: c == .hevc444)
            }
            sampleBytes = AnnexB.lengthPrefixed(HEVC.sampleNALs(nals))
        }
        guard let fd = format else {
            needKey = true
            requestKeyframe()
            return
        }
        if session == nil {
            createSession(fd, codec: c)
            guard session != nil else {
                needKey = true
                requestKeyframe()
                return
            }
        }
        guard let vt = session, let sb = makeSample(sampleBytes, fd) else { return }

        let decodeStartHost = hostNow()
        var outImage: CVImageBuffer?
        var outStatus: OSStatus = noErr
        let st = VTDecompressionSessionDecodeFrame(vt, sampleBuffer: sb, flags: [], infoFlagsOut: nil) { status, _, image, _, _ in
            outStatus = status
            outImage = image
        }
        VTDecompressionSessionWaitForAsynchronousFrames(vt)
        let decodedHost = hostNow()

        if st != noErr || outStatus != noErr {
            Log.error("decode error in frame \(f.frameId): \(st) / \(outStatus)")
            stats.onDecodeError()
            // A reset decoder has no references: it needs a keyframe.
            dropSession()
            needKey = true
            lastKeyRequest = 0
            requestKeyframe()
            return
        }
        guard let pb = outImage else {
            noPicture += 1
            if noPicture >= 3 && okFrames > 0 {
                Log.info("\(noPicture) frames gave no picture: asking for a keyframe")
                noPicture = 0
                needKey = true
                lastKeyRequest = 0
                requestKeyframe()
            }
            return
        }
        noPicture = 0
        needKey = false
        okFrames += 1
        guard let frame = makeTextures(pb, frameId: f.frameId) else { return }
        if okFrames == 1 {
            let what = "VideoToolbox \(c.label) · \(hardware ? "hardware" : "software")"  // i18n:ok a log line
            Log.info("first picture: \(what), \(frame.width)×\(frame.height)")
            delegate?.decoderChanged(description: what, hardware: hardware)
        }

        // The frame's life on the recorder's clock.
        let toRec = { (s: UInt64) -> Int64 in self.clock.recorderUs(session: Int64(s)) }
        var rec = FrameRec(frameId: f.frameId, keyframe: f.keyframe, bytes: f.data.count, firstPkt: toRec(f.firstArrivalUs),
                           lastPkt: toRec(f.lastArrivalUs), decodeStart: clock.recorderUs(host: decodeStartHost),
                           decoded: clock.recorderUs(host: decodedHost), ready: clock.recorderUs(host: frame.readyHost))
        if let local = f.sendLocalUs {
            let sendRec = clock.recorderUs(session: local)
            rec.send = sendRec
            let meta = FrameMeta.decode([UInt8](f.meta))
            if let s = meta.stamps {
                // Host clock minus ours, through the frame's own send time.
                rec.setHost(s, offsetUs: Int64(f.sendUs) - sendRec)
            }
        }
        lock.lock()
        latestBuffer = pb
        let test = clickTest
        lock.unlock()
        if let t = test {
            let p = t.probe
            if let l = meanLuma(pb, x: p.x, y: p.y, w: p.w, h: p.h) { t.onPicture(rec, luma: l) }
        }
        stats.onDecoded(rec)
        delegate?.decoded(frame, rec: rec)
    }

    private func sniff(_ b: [UInt8]) -> Codec {
        // Annex-B starts with a start code; AV1 with an OBU header.
        if b.count > 3 && b[0] == 0 && b[1] == 0 && (b[2] == 1 || (b[2] == 0 && b[3] == 1)) { return .hevc420 }
        return .av1
    }

    private func updateFormatHEVC(_ ps: HEVC.ParameterSets, is444: Bool) {
        let key = ps.vps + ps.sps + ps.pps
        if key == paramKey && format != nil { return }
        var fd: CMFormatDescription?
        let st: OSStatus = ps.vps.withUnsafeBufferPointer { v in
            ps.sps.withUnsafeBufferPointer { s in
                ps.pps.withUnsafeBufferPointer { p in
                    let pointers: [UnsafePointer<UInt8>] = [v.baseAddress!, s.baseAddress!, p.baseAddress!]
                    let sizes: [Int] = [v.count, s.count, p.count]
                    return CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                        allocator: kCFAllocatorDefault, parameterSetCount: 3, parameterSetPointers: pointers,
                        parameterSetSizes: sizes, nalUnitHeaderLength: 4, extensions: nil, formatDescriptionOut: &fd)
                }
            }
        }
        guard st == noErr, let newFd = fd else {
            Log.error("HEVC format description: \(st)")
            return
        }
        replaceFormat(newFd)
        paramKey = key
    }

    /// H.264: a standard VideoToolbox format, built from the SPS and PPS
    /// (CoreMedia writes the avcC from them).
    private func updateFormatH264(_ ps: H264.ParameterSets) {
        let key = ps.sps + ps.pps
        if key == paramKey && format != nil { return }
        var fd: CMFormatDescription?
        let st: OSStatus = ps.sps.withUnsafeBufferPointer { s in
            ps.pps.withUnsafeBufferPointer { p in
                let pointers: [UnsafePointer<UInt8>] = [s.baseAddress!, p.baseAddress!]
                let sizes: [Int] = [s.count, p.count]
                return CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    allocator: kCFAllocatorDefault, parameterSetCount: 2, parameterSetPointers: pointers,
                    parameterSetSizes: sizes, nalUnitHeaderLength: 4, formatDescriptionOut: &fd)
            }
        }
        guard st == noErr, let newFd = fd else {
            Log.error("H.264 format description: \(st)")
            return
        }
        replaceFormat(newFd)
        paramKey = key
    }

    private func updateFormatAV1(_ obu: [UInt8]) {
        if obu == paramKey && format != nil { return }
        guard let info = AV1.parse(sequenceHeaderOBU: obu) else {
            Log.error("AV1: couldn't read the sequence header")
            return
        }
        let av1c = Data(AV1.av1C(info, sequenceHeaderOBU: obu))
        let atoms: [String: Any] = ["av1C": av1c]
        let ext: [String: Any] = [
            kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms as String: atoms,
            kCMFormatDescriptionExtension_FullRangeVideo as String: info.fullRange,
        ]
        var fd: CMFormatDescription?
        // 'av01'
        let av01 = CMVideoCodecType(0x6176_3031)
        let st = CMVideoFormatDescriptionCreate(allocator: kCFAllocatorDefault, codecType: av01, width: Int32(info.maxWidth),
                                                height: Int32(info.maxHeight), extensions: ext as CFDictionary, formatDescriptionOut: &fd)
        guard st == noErr, let newFd = fd else {
            Log.error("AV1 format description: \(st)")
            return
        }
        replaceFormat(newFd)
        paramKey = obu
    }

    private func replaceFormat(_ fd: CMFormatDescription) {
        if let s = session, !VTDecompressionSessionCanAcceptFormatDescription(s, formatDescription: fd) {
            dropSession()
        }
        format = fd
    }

    private func createSession(_ fd: CMFormatDescription, codec c: Codec) {
        let pixelFormat: OSType = c == .hevc444 ? kCVPixelFormatType_444YpCbCr8BiPlanarFullRange : kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
        let attrs: [String: Any] = [
            kCVPixelBufferPixelFormatTypeKey as String: pixelFormat,
            kCVPixelBufferMetalCompatibilityKey as String: true,
            kCVPixelBufferIOSurfacePropertiesKey as String: [String: Any](),
        ]
        let spec: [String: Any] = [kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder as String: true]
        var s: VTDecompressionSession?
        let st = VTDecompressionSessionCreate(allocator: kCFAllocatorDefault, formatDescription: fd, decoderSpecification: spec as CFDictionary,
                                              imageBufferAttributes: attrs as CFDictionary, outputCallback: nil, decompressionSessionOut: &s)
        guard st == noErr, let vt = s else {
            Log.error("VTDecompressionSessionCreate: \(st)")
            return
        }
        VTSessionSetProperty(vt, key: kVTDecompressionPropertyKey_RealTime, value: kCFBooleanTrue)
        var hw: CFTypeRef?
        VTSessionCopyProperty(vt, key: kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder, allocator: kCFAllocatorDefault, valueOut: &hw)
        hardware = (hw as? Bool) ?? false
        session = vt
        okFrames = 0
    }

    private func makeSample(_ bytes: [UInt8], _ fd: CMFormatDescription) -> CMSampleBuffer? {
        guard !bytes.isEmpty else { return nil }
        var block: CMBlockBuffer?
        guard CMBlockBufferCreateWithMemoryBlock(allocator: kCFAllocatorDefault, memoryBlock: nil, blockLength: bytes.count,
                                                 blockAllocator: kCFAllocatorDefault, customBlockSource: nil, offsetToData: 0,
                                                 dataLength: bytes.count, flags: kCMBlockBufferAssureMemoryNowFlag, blockBufferOut: &block) == noErr,
              let bb = block else { return nil }
        let copied: OSStatus = bytes.withUnsafeBytes { raw in
            CMBlockBufferReplaceDataBytes(with: raw.baseAddress!, blockBuffer: bb, offsetIntoDestination: 0, dataLength: bytes.count)
        }
        guard copied == noErr else { return nil }
        var out: CMSampleBuffer?
        var sizes: [Int] = [bytes.count]
        guard CMSampleBufferCreateReady(allocator: kCFAllocatorDefault, dataBuffer: bb, formatDescription: fd, sampleCount: 1,
                                        sampleTimingEntryCount: 0, sampleTimingArray: nil, sampleSizeEntryCount: 1,
                                        sampleSizeArray: &sizes, sampleBufferOut: &out) == noErr else { return nil }
        return out
    }

    private func makeTextures(_ pb: CVPixelBuffer, frameId: UInt32) -> DecodedFrame? {
        guard CVPixelBufferGetPlaneCount(pb) >= 2 else { return nil }
        let w = CVPixelBufferGetWidthOfPlane(pb, 0)
        let h = CVPixelBufferGetHeightOfPlane(pb, 0)
        let cw = CVPixelBufferGetWidthOfPlane(pb, 1)
        let ch = CVPixelBufferGetHeightOfPlane(pb, 1)
        var y: CVMetalTexture?
        var c: CVMetalTexture?
        guard CVMetalTextureCacheCreateTextureFromImage(kCFAllocatorDefault, cache, pb, nil, .r8Unorm, w, h, 0, &y) == kCVReturnSuccess,
              CVMetalTextureCacheCreateTextureFromImage(kCFAllocatorDefault, cache, pb, nil, .rg8Unorm, cw, ch, 1, &c) == kCVReturnSuccess,
              let yRef = y, let cRef = c, let yt = CVMetalTextureGetTexture(yRef), let ct = CVMetalTextureGetTexture(cRef) else {
            Log.error("CVMetalTextureCacheCreateTextureFromImage failed")
            return nil
        }
        let pf = CVPixelBufferGetPixelFormatType(pb)
        let full = pf == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange || pf == kCVPixelFormatType_444YpCbCr8BiPlanarFullRange
        var bt601 = false
        if let m = CVBufferCopyAttachment(pb, kCVImageBufferYCbCrMatrixKey, nil) as? String {
            bt601 = m == (kCVImageBufferYCbCrMatrix_ITU_R_601_4 as String)
        }
        return DecodedFrame(frameId: frameId, pixelBuffer: pb, lumaRef: yRef, chromaRef: cRef, luma: yt, chroma: ct, width: w, height: h,
                            fullRange: full, bt601: bt601, readyHost: hostNow())
    }

    /// The click test's probe: the mean of the Y plane over a rect (every
    /// fourth pixel), 0–255; nil outside the picture.
    private func meanLuma(_ pb: CVPixelBuffer, x: Int, y: Int, w: Int, h: Int) -> Double? {
        let pw = CVPixelBufferGetWidthOfPlane(pb, 0)
        let ph = CVPixelBufferGetHeightOfPlane(pb, 0)
        guard x >= 0, y >= 0, x + w <= pw, y + h <= ph, w > 0, h > 0 else { return nil }
        guard CVPixelBufferLockBaseAddress(pb, .readOnly) == kCVReturnSuccess else { return nil }
        defer { CVPixelBufferUnlockBaseAddress(pb, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddressOfPlane(pb, 0) else { return nil }
        let stride = CVPixelBufferGetBytesPerRowOfPlane(pb, 0)
        let p = base.assumingMemoryBound(to: UInt8.self)
        var sum = 0
        var n = 0
        var row = y
        while row < y + h {
            var col = x
            while col < x + w {
                sum += Int(p[row * stride + col])
                n += 1
                col += 4
            }
            row += 4
        }
        return n > 0 ? Double(sum) / Double(n) : nil
    }
}

/// Does this Mac decode the host's HEVC 4:4:4 in hardware (the Auto,
/// the Auto choice)? A session that requires a hardware decoder is created for the
/// parameter sets of an NVENC 4:4:4 stream (Tools/samples/yuv444.hevc).
enum HardwareProbe {
    static let vps: [UInt8] = hex("40010c01ffff2408000003009e08000003000096970240")
    static let sps: [UInt8] = hex("4201012408000003009e080000030000969000280400d02cb2e94842322ea8602dc04340410000030001000003003c08")
    static let pps: [UInt8] = hex("4401c1f718300cc9")

    static func hex(_ s: String) -> [UInt8] {
        var out: [UInt8] = []
        var i = s.startIndex
        while i < s.endIndex {
            let j = s.index(i, offsetBy: 2)
            out.append(UInt8(s[i..<j], radix: 16) ?? 0)
            i = j
        }
        return out
    }

    static func hevc444() -> Bool {
        var fd: CMFormatDescription?
        let st: OSStatus = vps.withUnsafeBufferPointer { v in
            sps.withUnsafeBufferPointer { s in
                pps.withUnsafeBufferPointer { p in
                    let pointers: [UnsafePointer<UInt8>] = [v.baseAddress!, s.baseAddress!, p.baseAddress!]
                    let sizes: [Int] = [v.count, s.count, p.count]
                    return CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                        allocator: kCFAllocatorDefault, parameterSetCount: 3, parameterSetPointers: pointers,
                        parameterSetSizes: sizes, nalUnitHeaderLength: 4, extensions: nil, formatDescriptionOut: &fd)
                }
            }
        }
        guard st == noErr, let f = fd else { return false }
        let spec: [String: Any] = [
            kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder as String: true,
            kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder as String: true,
        ]
        let attrs: [String: Any] = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_444YpCbCr8BiPlanarFullRange]
        var s: VTDecompressionSession?
        let ok = VTDecompressionSessionCreate(allocator: kCFAllocatorDefault, formatDescription: f, decoderSpecification: spec as CFDictionary,
                                              imageBufferAttributes: attrs as CFDictionary, outputCallback: nil, decompressionSessionOut: &s) == noErr
        if let vt = s { VTDecompressionSessionInvalidate(vt) }
        return ok && s != nil
    }

    static func av1() -> Bool {
        VTIsHardwareDecodeSupported(CMVideoCodecType(0x6176_3031))
    }
}
