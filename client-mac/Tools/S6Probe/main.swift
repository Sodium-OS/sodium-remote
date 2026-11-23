// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// Decode probe: does VideoToolbox decode the host's HEVC 4:4:4 (and 4:2:0)
// in hardware on this Mac, and how long does a frame take?
//
//   s6-probe FILE.hevc [--loops N] [--software] [--csv OUT.csv]
//
// FILE is Annex-B HEVC as the host's NVENC writes it (Tools/samples/, made by
// Tools/make-s6-samples.sh with the S3 harness). Each loop decodes the file
// from its keyframe, one frame at a time and synchronously, as the app does.
// Prints the profile and chroma, whether VideoToolbox found a hardware
// decoder (it is asked to require one first), the output pixel format and
// decode time per frame (p50 / p95 / max).
import CoreMedia
import CoreVideo
import Foundation
import QuartzCore
import RemoteKit
import VideoToolbox

func fail(_ s: String) -> Never {
    FileHandle.standardError.write((s + "\n").data(using: .utf8) ?? Data())
    exit(1)
}

var args = Array(CommandLine.arguments.dropFirst())
guard let path = args.first, !path.hasPrefix("-") else {
    fail("usage: s6-probe FILE.hevc [--loops N] [--software] [--csv OUT.csv]")
}
args.removeFirst()
var loops = 30
var softwareOnly = false
var csvPath: String?
while !args.isEmpty {
    let a = args.removeFirst()
    switch a {
    case "--loops": loops = Int(args.isEmpty ? "30" : args.removeFirst()) ?? 30
    case "--software": softwareOnly = true
    case "--csv": csvPath = args.isEmpty ? nil : args.removeFirst()
    default: fail("unknown option \(a)")
    }
}

guard let data = FileManager.default.contents(atPath: path) else { fail("can't read \(path)") }
let nals = AnnexB.split([UInt8](data))
guard let ps = HEVC.parameterSets(nals) else { fail("no VPS/SPS/PPS in \(path)") }
let chroma = HEVC.chromaFormat(sps: ps.sps) ?? -1
let is444 = chroma == 3

/// Access units: a new one starts at a parameter set, an AUD or a prefix SEI
/// after a slice, or at a slice with first_slice_segment_in_pic_flag.
var units: [[[UInt8]]] = []
var current: [[UInt8]] = []
var hasSlice = false
for nal in nals {
    let t = HEVC.nalType(nal)
    let vcl = t < 32
    let first = vcl && nal.count > 2 && (nal[2] & 0x80) != 0
    let starter = (t >= 32 && t <= 35) || t == 39
    if hasSlice && (first || starter) {
        units.append(current)
        current = []
        hasSlice = false
    }
    current.append(nal)
    if vcl { hasSlice = true }
}
if !current.isEmpty { units.append(current) }

// The format description, from the parameter sets.
var format: CMFormatDescription?
let fdStatus: OSStatus = ps.vps.withUnsafeBufferPointer { v in
    ps.sps.withUnsafeBufferPointer { s in
        ps.pps.withUnsafeBufferPointer { p in
            let pointers: [UnsafePointer<UInt8>] = [v.baseAddress!, s.baseAddress!, p.baseAddress!]
            let sizes: [Int] = [v.count, s.count, p.count]
            return CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                allocator: kCFAllocatorDefault, parameterSetCount: 3, parameterSetPointers: pointers,
                parameterSetSizes: sizes, nalUnitHeaderLength: 4, extensions: nil, formatDescriptionOut: &format)
        }
    }
}
guard fdStatus == noErr, let fd = format else { fail("CMVideoFormatDescriptionCreateFromHEVCParameterSets: \(fdStatus)") }
let dims = CMVideoFormatDescriptionGetDimensions(fd)

print("file:     \(path) (\(data.count) bytes, \(units.count) frames)")
print("stream:   HEVC \(dims.width)×\(dims.height), chroma_format_idc \(chroma) (\(is444 ? "4:4:4" : chroma == 1 ? "4:2:0" : "?"))")
print("VTIsHardwareDecodeSupported(HEVC): \(VTIsHardwareDecodeSupported(kCMVideoCodecType_HEVC))")

let pixelFormat: OSType = is444 ? kCVPixelFormatType_444YpCbCr8BiPlanarFullRange : kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
let attrs: [String: Any] = [
    kCVPixelBufferPixelFormatTypeKey as String: pixelFormat,
    kCVPixelBufferMetalCompatibilityKey as String: true,
    kCVPixelBufferIOSurfacePropertiesKey as String: [String: Any](),
]

func makeSession(requireHardware: Bool) -> (VTDecompressionSession?, OSStatus) {
    var spec: [String: Any] = [kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder as String: true]
    if requireHardware {
        spec[kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder as String] = true
    }
    var session: VTDecompressionSession?
    let st = VTDecompressionSessionCreate(allocator: kCFAllocatorDefault, formatDescription: fd, decoderSpecification: spec as CFDictionary,
                                          imageBufferAttributes: attrs as CFDictionary, outputCallback: nil, decompressionSessionOut: &session)
    return (session, st)
}

var session: VTDecompressionSession?
if !softwareOnly {
    let (s, st) = makeSession(requireHardware: true)
    if let s = s {
        session = s
        print("hardware: YES (a session that requires a hardware decoder was created)")
    } else {
        print("hardware: NO (requiring a hardware decoder failed: OSStatus \(st)); timing software")
    }
}
if session == nil {
    let (s, st) = makeSession(requireHardware: false)
    guard let s2 = s else { fail("VTDecompressionSessionCreate: \(st)") }
    session = s2
}
guard let vt = session else { fail("no session") }
VTSessionSetProperty(vt, key: kVTDecompressionPropertyKey_RealTime, value: kCFBooleanTrue)
var usingHW: CFTypeRef?
VTSessionCopyProperty(vt, key: kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder, allocator: kCFAllocatorDefault, valueOut: &usingHW)
print("UsingHardwareAcceleratedVideoDecoder: \((usingHW as? Bool).map { String($0) } ?? "unknown")")

func sample(_ unit: [[UInt8]]) -> CMSampleBuffer? {
    let bytes = AnnexB.lengthPrefixed(HEVC.sampleNALs(unit))
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

let samples = units.compactMap(sample)
var times: [Double] = []
var csv = "loop,frame,bytes,decode_ms,status\n"
var outFormat: OSType = 0
var errors = 0
for loop in 0..<max(loops, 1) {
    for (i, sb) in samples.enumerated() {
        var gotStatus: OSStatus = 0
        var gotImage = false
        let t0 = CACurrentMediaTime()
        let st = VTDecompressionSessionDecodeFrame(vt, sampleBuffer: sb, flags: [], infoFlagsOut: nil) { status, _, image, _, _ in
            gotStatus = status
            if let img = image {
                gotImage = true
                outFormat = CVPixelBufferGetPixelFormatType(img)
            }
        }
        VTDecompressionSessionWaitForAsynchronousFrames(vt)
        let ms = (CACurrentMediaTime() - t0) * 1000
        let ok = st == noErr && gotStatus == noErr && gotImage
        if !ok { errors += 1 }
        // The first loop warms the decoder up; time the rest.
        if loop > 0 || loops == 1 { times.append(ms) }
        csv += "\(loop),\(i),\(CMSampleBufferGetTotalSampleSize(sb)),\(String(format: "%.3f", ms)),\(ok ? 0 : (st != noErr ? st : gotStatus))\n"
    }
}
VTDecompressionSessionInvalidate(vt)

func fourCC(_ v: OSType) -> String {
    let b = [UInt8((v >> 24) & 0xFF), UInt8((v >> 16) & 0xFF), UInt8((v >> 8) & 0xFF), UInt8(v & 0xFF)]
    return String(bytes: b, encoding: .ascii) ?? String(v)
}

let fmtMs = { (p: Double) -> String in percentile(times, p).map { String(format: "%.2f", $0) } ?? "n/a" }
print("output:   \(fourCC(outFormat)) (asked for \(fourCC(pixelFormat)))")
print("decode:   p50 \(fmtMs(0.5)) · p95 \(fmtMs(0.95)) · max \(fmtMs(1.0)) ms over \(times.count) frames; \(errors) errors")
if let p = csvPath {
    try? csv.write(toFile: p, atomically: true, encoding: .utf8)
    print("csv:      \(p)")
}
exit(errors == 0 ? 0 : 2)
