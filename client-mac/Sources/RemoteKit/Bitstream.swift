// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import Foundation

/// HEVC Annex-B (start codes), as the host's NVENC writes it, and the
/// length-prefixed form VideoToolbox decodes.
public enum AnnexB {
    /// The NAL units in an Annex-B buffer, without their start codes (and
    /// without the trailing zero byte of a following 4-byte start code).
    public static func split(_ b: [UInt8]) -> [[UInt8]] {
        var starts: [(code: Int, payload: Int)] = []
        let n = b.count
        var i = 0
        while i + 2 < n {
            if b[i] == 0 && b[i + 1] == 0 && b[i + 2] == 1 {
                let code = (i > 0 && b[i - 1] == 0) ? i - 1 : i
                starts.append((code, i + 3))
                i += 3
            } else {
                i += 1
            }
        }
        var out: [[UInt8]] = []
        out.reserveCapacity(starts.count)
        for (k, s) in starts.enumerated() {
            var end = k + 1 < starts.count ? starts[k + 1].code : n
            while end > s.payload && b[end - 1] == 0 { end -= 1 }
            if end > s.payload { out.append(Array(b[s.payload..<end])) }
        }
        return out
    }

    /// NAL units with 4-byte big-endian lengths (`nalUnitHeaderLength` 4).
    public static func lengthPrefixed(_ nals: [[UInt8]]) -> [UInt8] {
        var out: [UInt8] = []
        out.reserveCapacity(nals.reduce(0) { $0 + $1.count + 4 })
        for nal in nals {
            let l = UInt32(nal.count)
            out.append(UInt8(l >> 24))
            out.append(UInt8((l >> 16) & 0xFF))
            out.append(UInt8((l >> 8) & 0xFF))
            out.append(UInt8(l & 0xFF))
            out.append(contentsOf: nal)
        }
        return out
    }
}

/// H.264 (Annex B), the universal baseline codec.
public enum H264 {
    public static let sps: UInt8 = 7
    public static let pps: UInt8 = 8
    public static let aud: UInt8 = 9

    public static func nalType(_ nal: [UInt8]) -> UInt8 {
        nal.isEmpty ? 0xFF : nal[0] & 0x1F
    }

    /// SPS and PPS of a keyframe (the first of each).
    public struct ParameterSets: Equatable {
        public var sps: [UInt8]
        public var pps: [UInt8]
    }

    public static func parameterSets(_ nals: [[UInt8]]) -> ParameterSets? {
        var s: [UInt8]?, p: [UInt8]?
        for nal in nals {
            switch nalType(nal) {
            case sps: if s == nil { s = nal }
            case pps: if p == nil { p = nal }
            default: break
            }
        }
        guard let ss = s, let pp = p else { return nil }
        return ParameterSets(sps: ss, pps: pp)
    }

    /// The NAL units VideoToolbox gets as the sample: everything but the
    /// parameter sets (they go in the format description) and delimiters.
    public static func sampleNALs(_ nals: [[UInt8]]) -> [[UInt8]] {
        nals.filter {
            let t = nalType($0)
            return t != sps && t != pps && t != aud
        }
    }
}

public enum HEVC {
    public static let vps: UInt8 = 32
    public static let sps: UInt8 = 33
    public static let pps: UInt8 = 34
    public static let aud: UInt8 = 35

    public static func nalType(_ nal: [UInt8]) -> UInt8 {
        nal.isEmpty ? 0xFF : (nal[0] >> 1) & 0x3F
    }

    /// VPS, SPS and PPS of a keyframe (the first of each).
    public struct ParameterSets: Equatable {
        public var vps: [UInt8]
        public var sps: [UInt8]
        public var pps: [UInt8]
    }

    public static func parameterSets(_ nals: [[UInt8]]) -> ParameterSets? {
        var v: [UInt8]?, s: [UInt8]?, p: [UInt8]?
        for nal in nals {
            switch nalType(nal) {
            case vps: if v == nil { v = nal }
            case sps: if s == nil { s = nal }
            case pps: if p == nil { p = nal }
            default: break
            }
        }
        guard let vv = v, let ss = s, let pp = p else { return nil }
        return ParameterSets(vps: vv, sps: ss, pps: pp)
    }

    /// The NAL units VideoToolbox gets as the sample: everything but the
    /// parameter sets (they go in the format description) and delimiters.
    public static func sampleNALs(_ nals: [[UInt8]]) -> [[UInt8]] {
        nals.filter {
            let t = nalType($0)
            return t != vps && t != sps && t != pps && t != aud
        }
    }

    /// The SPS's chroma_format_idc (1 = 4:2:0, 3 = 4:4:4), read from the
    /// profile_tier_level that starts every SPS (no scaling lists before it).
    public static func chromaFormat(sps: [UInt8]) -> Int? {
        let rbsp = unescape(Array(sps.dropFirst(2)))
        var r = BitReader(rbsp)
        guard r.skip(4) else { return nil }                       // sps_video_parameter_set_id
        guard let maxSubLayersMinus1 = r.bits(3) else { return nil }
        guard r.skip(1) else { return nil }                       // temporal_id_nesting
        // profile_tier_level(1, maxSubLayersMinus1)
        guard r.skip(2 + 1 + 5 + 32 + 4 + 43 + 1 + 8) else { return nil }
        var subProfile: [Bool] = []
        var subLevel: [Bool] = []
        for _ in 0..<Int(maxSubLayersMinus1) {
            guard let p = r.bits(1), let l = r.bits(1) else { return nil }
            subProfile.append(p == 1)
            subLevel.append(l == 1)
        }
        if maxSubLayersMinus1 > 0 {
            for _ in Int(maxSubLayersMinus1)..<8 { guard r.skip(2) else { return nil } }
        }
        for i in 0..<Int(maxSubLayersMinus1) {
            if subProfile[i] { guard r.skip(88) else { return nil } }
            if subLevel[i] { guard r.skip(8) else { return nil } }
        }
        guard r.ue() != nil else { return nil }                   // sps_seq_parameter_set_id
        guard let chroma = r.ue() else { return nil }
        return Int(chroma)
    }

    /// RBSP from a NAL payload: drop emulation-prevention bytes (00 00 03).
    static func unescape(_ b: [UInt8]) -> [UInt8] {
        var out: [UInt8] = []
        out.reserveCapacity(b.count)
        var zeros = 0
        for x in b {
            if zeros >= 2 && x == 3 {
                zeros = 0
                continue
            }
            out.append(x)
            zeros = x == 0 ? zeros + 1 : 0
        }
        return out
    }
}

/// MSB-first bit reader (HEVC and AV1 headers).
public struct BitReader {
    let b: [UInt8]
    var pos = 0

    public init(_ b: [UInt8]) {
        self.b = b
    }

    public var bitsLeft: Int { b.count * 8 - pos }

    public mutating func bits(_ n: Int) -> UInt64? {
        guard n >= 0, n <= 64, bitsLeft >= n else { return nil }
        var v: UInt64 = 0
        for _ in 0..<n {
            let bit = (b[pos >> 3] >> (7 - UInt8(pos & 7))) & 1
            v = (v << 1) | UInt64(bit)
            pos += 1
        }
        return v
    }

    public mutating func flag() -> Bool? {
        bits(1).map { $0 == 1 }
    }

    public mutating func skip(_ n: Int) -> Bool {
        guard bitsLeft >= n else { return false }
        pos += n
        return true
    }

    /// Exp-Golomb ue(v).
    public mutating func ue() -> UInt64? {
        var zeros = 0
        while true {
            guard let bit = bits(1) else { return nil }
            if bit == 1 { break }
            zeros += 1
            if zeros > 32 { return nil }
        }
        guard let rest = bits(zeros) else { return nil }
        return (UInt64(1) << UInt64(zeros)) - 1 + rest
    }

    /// AV1 uvlc().
    public mutating func uvlc() -> UInt64? {
        var zeros = 0
        while true {
            guard let bit = bits(1) else { return nil }
            if bit == 1 { break }
            zeros += 1
            if zeros >= 32 { return UInt64(UInt32.max) }
        }
        guard let rest = bits(zeros) else { return nil }
        return rest + (UInt64(1) << UInt64(zeros)) - 1
    }
}

/// AV1 in the low-overhead OBU format (the host's NVENC output, one temporal
/// unit per frame), and what VideoToolbox needs: an `av1C` record and samples
/// without temporal delimiters (the ISOBMFF sample format).
public enum AV1 {
    public static let sequenceHeader: UInt8 = 1
    public static let temporalDelimiter: UInt8 = 2
    public static let padding: UInt8 = 15

    public struct OBU {
        public var type: UInt8
        /// The whole OBU, header and size field included.
        public var range: Range<Int>
    }

    /// The OBUs of a temporal unit (each must carry a size field).
    public static func obus(_ b: [UInt8]) -> [OBU] {
        var out: [OBU] = []
        var i = 0
        while i < b.count {
            let start = i
            let h = b[i]
            let type = (h >> 3) & 0x0F
            let ext = (h >> 2) & 1 == 1
            let hasSize = (h >> 1) & 1 == 1
            i += ext ? 2 : 1
            guard hasSize, i <= b.count else { break }
            guard let field = leb128(b, at: i) else { break }
            i += field.1
            let end = i + Int(field.0)
            guard end <= b.count else { break }
            out.append(OBU(type: type, range: start..<end))
            i = end
        }
        return out
    }

    static func leb128(_ b: [UInt8], at start: Int) -> (UInt64, Int)? {
        var v: UInt64 = 0
        for k in 0..<8 {
            guard start + k < b.count else { return nil }
            let x = b[start + k]
            v |= UInt64(x & 0x7F) << UInt64(7 * k)
            if x & 0x80 == 0 { return (v, k + 1) }
        }
        return nil
    }

    /// The sample for VideoToolbox: the temporal unit without temporal
    /// delimiters and padding.
    public static func sample(_ b: [UInt8]) -> [UInt8] {
        var out: [UInt8] = []
        out.reserveCapacity(b.count)
        for o in obus(b) where o.type != temporalDelimiter && o.type != padding {
            out.append(contentsOf: b[o.range])
        }
        return out
    }

    /// What `av1C` and the format description need from a sequence header.
    public struct SequenceInfo: Equatable {
        public var profile: UInt8
        public var level0: UInt8
        public var tier0: UInt8
        public var highBitdepth: Bool
        public var twelveBit: Bool
        public var monochrome: Bool
        public var subsamplingX: Bool
        public var subsamplingY: Bool
        public var chromaSamplePosition: UInt8
        public var fullRange: Bool
        public var maxWidth: Int
        public var maxHeight: Int
    }

    /// The first sequence header OBU in a temporal unit, whole.
    public static func sequenceHeaderOBU(_ b: [UInt8]) -> [UInt8]? {
        guard let o = obus(b).first(where: { $0.type == sequenceHeader }) else { return nil }
        return Array(b[o.range])
    }

    /// Parse a sequence header OBU (AV1 spec 5.5).
    public static func parse(sequenceHeaderOBU obu: [UInt8]) -> SequenceInfo? {
        guard !obu.isEmpty else { return nil }
        let ext = (obu[0] >> 2) & 1 == 1
        var i = ext ? 2 : 1
        guard let field = leb128(obu, at: i) else { return nil }
        i += field.1
        var r = BitReader(Array(obu[i...]))
        guard let profile = r.bits(3), r.skip(1), let reduced = r.flag() else { return nil }
        var level0: UInt64 = 0
        var tier0: UInt64 = 0
        if reduced {
            guard let l = r.bits(5) else { return nil }
            level0 = l
        } else {
            guard let timing = r.flag() else { return nil }
            var decoderModel = false
            var bufferDelayLen = 0
            if timing {
                guard r.skip(64), let equal = r.flag() else { return nil }
                if equal { guard r.uvlc() != nil else { return nil } }
                guard let dm = r.flag() else { return nil }
                decoderModel = dm
                if dm {
                    guard let bdl = r.bits(5), r.skip(32 + 5 + 5) else { return nil }
                    bufferDelayLen = Int(bdl) + 1
                }
            }
            guard let initialDelay = r.flag(), let cnt = r.bits(5) else { return nil }
            for k in 0...Int(cnt) {
                guard r.skip(12), let level = r.bits(5) else { return nil }
                var tier: UInt64 = 0
                if level > 7 { guard let t = r.bits(1) else { return nil }; tier = t }
                if k == 0 {
                    level0 = level
                    tier0 = tier
                }
                if decoderModel {
                    guard let present = r.flag() else { return nil }
                    if present { guard r.skip(bufferDelayLen * 2 + 1) else { return nil } }
                }
                if initialDelay {
                    guard let present = r.flag() else { return nil }
                    if present { guard r.skip(4) else { return nil } }
                }
            }
        }
        guard let fwb = r.bits(4), let fhb = r.bits(4) else { return nil }
        guard let mw = r.bits(Int(fwb) + 1), let mh = r.bits(Int(fhb) + 1) else { return nil }
        if !reduced {
            guard let frameIds = r.flag() else { return nil }
            if frameIds { guard r.skip(4 + 3) else { return nil } }
        }
        guard r.skip(3) else { return nil }   // 128×128 superblocks, filter intra, intra edge filter
        if !reduced {
            guard r.skip(4), let orderHint = r.flag() else { return nil }   // interintra, masked, warped, dual filter
            if orderHint { guard r.skip(2) else { return nil } }            // jnt_comp, ref_frame_mvs
            guard let choose = r.flag() else { return nil }
            var force: UInt64 = 2
            if !choose { guard let f = r.bits(1) else { return nil }; force = f }
            if force > 0 {
                guard let chooseMv = r.flag() else { return nil }
                if !chooseMv { guard r.skip(1) else { return nil } }
            }
            if orderHint { guard r.skip(3) else { return nil } }
        }
        guard r.skip(3) else { return nil }   // superres, cdef, restoration
        // color_config()
        guard let high = r.flag() else { return nil }
        var twelve = false
        if profile == 2 && high { guard let t = r.flag() else { return nil }; twelve = t }
        let bitDepth = profile == 2 && high ? (twelve ? 12 : 10) : (high ? 10 : 8)
        var mono = false
        if profile != 1 { guard let m = r.flag() else { return nil }; mono = m }
        guard let described = r.flag() else { return nil }
        var cp: UInt64 = 2, tc: UInt64 = 2, mc: UInt64 = 2
        if described {
            guard let a = r.bits(8), let b = r.bits(8), let c = r.bits(8) else { return nil }
            cp = a
            tc = b
            mc = c
        }
        var fullRange = false
        var sx = true, sy = true
        var csp: UInt64 = 0
        if mono {
            guard let fr = r.flag() else { return nil }
            fullRange = fr
        } else if cp == 1 && tc == 13 && mc == 0 {
            fullRange = true
            sx = false
            sy = false
        } else {
            guard let fr = r.flag() else { return nil }
            fullRange = fr
            if profile == 0 {
                sx = true
                sy = true
            } else if profile == 1 {
                sx = false
                sy = false
            } else if bitDepth == 12 {
                guard let x = r.flag() else { return nil }
                sx = x
                if x { guard let y = r.flag() else { return nil }; sy = y } else { sy = false }
            } else {
                sx = true
                sy = false
            }
            if sx && sy { guard let p = r.bits(2) else { return nil }; csp = p }
        }
        return SequenceInfo(profile: UInt8(profile), level0: UInt8(level0), tier0: UInt8(tier0), highBitdepth: high, twelveBit: twelve,
                            monochrome: mono, subsamplingX: sx, subsamplingY: sy, chromaSamplePosition: UInt8(csp), fullRange: fullRange,
                            maxWidth: Int(mw) + 1, maxHeight: Int(mh) + 1)
    }

    /// The AV1CodecConfigurationRecord (`av1C`) with the sequence header as
    /// its config OBU.
    public static func av1C(_ s: SequenceInfo, sequenceHeaderOBU: [UInt8]) -> [UInt8] {
        var out: [UInt8] = [0x81]
        out.append((s.profile << 5) | (s.level0 & 0x1F))
        var b: UInt8 = (s.tier0 & 1) << 7
        if s.highBitdepth { b |= 1 << 6 }
        if s.twelveBit { b |= 1 << 5 }
        if s.monochrome { b |= 1 << 4 }
        if s.subsamplingX { b |= 1 << 3 }
        if s.subsamplingY { b |= 1 << 2 }
        b |= s.chromaSamplePosition & 3
        out.append(b)
        out.append(0)
        out.append(contentsOf: sequenceHeaderOBU)
        return out
    }
}

/// Frame metadata (`ReceivedFrame.meta`): host-media's v1 meta
/// (`u8 version = 1 | u8 class | u16 n | n × (u16 x, y, w, h)`), then
/// tag-length-value records; tag 1 carries the host's stage stamps
/// (rendered, captured, converted, encoded: u64 µs on the host's session
/// clock). Big-endian (client-linux `proto.rs`).
public struct FrameMeta: Equatable {
    public struct Stamps: Equatable {
        public var renderedUs: UInt64
        public var capturedUs: UInt64
        public var convertedUs: UInt64
        public var encodedUs: UInt64
    }

    public static let passive: UInt8 = 0
    public static let interactive: UInt8 = 1
    public static let refine: UInt8 = 2

    public var frameClass: UInt8 = 0
    public var damageCount = 0
    public var stamps: Stamps?

    public init() {}

    public static func decode(_ b: [UInt8]) -> FrameMeta {
        var m = FrameMeta()
        var i = 0
        func u8() -> UInt8? {
            guard i < b.count else { return nil }
            defer { i += 1 }
            return b[i]
        }
        func u16() -> UInt16? {
            guard i + 2 <= b.count else { return nil }
            defer { i += 2 }
            return UInt16(b[i]) << 8 | UInt16(b[i + 1])
        }
        func u64() -> UInt64? {
            guard i + 8 <= b.count else { return nil }
            var v: UInt64 = 0
            for k in 0..<8 { v = v << 8 | UInt64(b[i + k]) }
            i += 8
            return v
        }
        guard u8() == 1, let cls = u8(), let n = u16() else { return m }
        m.frameClass = cls
        guard i + Int(n) * 8 <= b.count else { return m }
        i += Int(n) * 8
        m.damageCount = Int(n)
        while let tag = u8(), let len = u16() {
            let end = i + Int(len)
            guard end <= b.count else { break }
            if tag == 1 && len >= 32 {
                if let a = u64(), let c = u64(), let d = u64(), let e = u64() {
                    m.stamps = Stamps(renderedUs: a, capturedUs: c, convertedUs: d, encodedUs: e)
                }
            }
            i = end
        }
        return m
    }
}
