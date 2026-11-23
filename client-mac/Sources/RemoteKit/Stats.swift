// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import Foundation

/// The p-th percentile (0…1) by rank, rounded (client-linux's `percentile`);
/// nil when there is nothing.
public func percentile(_ values: [Double], _ p: Double) -> Double? {
    guard !values.isEmpty else { return nil }
    let v = values.sorted()
    let i = Int((Double(v.count - 1) * p).rounded())
    return v[min(max(i, 0), v.count - 1)]
}

/// One frame's life on the Mac, plus the host's stages. Every time is on the
/// recorder's clock (µs since its epoch).
public struct FrameRec {
    public var frameId: UInt32
    public var keyframe: Bool
    public var bytes: Int
    public var hostRendered: Int64?
    public var hostCaptured: Int64?
    public var hostConverted: Int64?
    public var hostEncoded: Int64?
    public var send: Int64?
    public var firstPkt: Int64
    public var lastPkt: Int64
    /// Handed to VideoToolbox.
    public var decodeStart: Int64
    /// VideoToolbox gave the picture back.
    public var decoded: Int64
    /// Its Metal textures exist: ready for the presenter (integration.md 9).
    public var ready: Int64
    /// The command buffer that draws it was committed.
    public var commit: Int64?
    /// `MTLDrawable.presentedTime` (nil: never shown).
    public var present: Int64?
    /// The display link update's `targetTimestamp`: the drawing deadline the
    /// lead is measured against (integration.md 9).
    public var deadline: Int64?
    /// The update's `targetPresentationTimestamp`: the vsync it was drawn for.
    public var targetPresent: Int64?
    /// Never shown: a newer frame won, or the drawable was dropped.
    public var skipped = false

    public init(frameId: UInt32, keyframe: Bool, bytes: Int, firstPkt: Int64, lastPkt: Int64, decodeStart: Int64, decoded: Int64, ready: Int64) {
        self.frameId = frameId
        self.keyframe = keyframe
        self.bytes = bytes
        self.firstPkt = firstPkt
        self.lastPkt = lastPkt
        self.decodeStart = decodeStart
        self.decoded = decoded
        self.ready = ready
    }

    /// The host's stamps on our clock; `offsetUs` is host minus client.
    public mutating func setHost(_ s: FrameMeta.Stamps, offsetUs: Int64) {
        func conv(_ v: UInt64) -> Int64? {
            guard v != 0, let x = Int64(exactly: v) else { return nil }
            return x &- offsetUs
        }
        hostRendered = conv(s.renderedUs)
        hostCaptured = conv(s.capturedUs)
        hostConverted = conv(s.convertedUs)
        hostEncoded = conv(s.encodedUs)
    }
}

/// The host's own numbers (`msg::HostStats`), for the HUD.
public struct HostStatsSnapshot: Equatable {
    public var captureUs: UInt32
    public var convertUs: UInt32
    public var encodeUs: UInt32
    public var sendUs: UInt32
    public var fps: Float
    public var bitrateBps: UInt64
    public var qp: UInt8
    public var optimisedLook: Bool
    public var readableMs: UInt32
    public var losslessMs: UInt32
    public var settledMs: UInt32
    public var settledEnd: UInt8
    /// The controller's target, C_est and a refused probe's ceiling, bit/s (0: none).
    public var targetBps: UInt64
    public var cEstBps: UInt64
    public var probeCeilingBps: UInt64

    public init(captureUs: UInt32, convertUs: UInt32, encodeUs: UInt32, sendUs: UInt32, fps: Float, bitrateBps: UInt64, qp: UInt8,
                optimisedLook: Bool, readableMs: UInt32, losslessMs: UInt32, settledMs: UInt32, settledEnd: UInt8,
                targetBps: UInt64, cEstBps: UInt64, probeCeilingBps: UInt64) {
        self.captureUs = captureUs
        self.convertUs = convertUs
        self.encodeUs = encodeUs
        self.sendUs = sendUs
        self.fps = fps
        self.bitrateBps = bitrateBps
        self.qp = qp
        self.optimisedLook = optimisedLook
        self.readableMs = readableMs
        self.losslessMs = losslessMs
        self.settledMs = settledMs
        self.settledEnd = settledEnd
        self.targetBps = targetBps
        self.cEstBps = cEstBps
        self.probeCeilingBps = probeCeilingBps
    }
}

/// `msg::ClientStats`, for the host.
public struct ClientStatsSnapshot: Equatable {
    public var decodeUs: UInt32
    public var presentUs: UInt32
    public var fps: Float
    public var framesLate: UInt32
    public var framesDropped: UInt32
}

/// Where a stage happens, for the HUD's colours (host, network, this Mac).
public enum StageGroup: Equatable {
    case host, net, mac
}

/// One row of the HUD's stage list (the budget order).
public struct HudStage: Equatable, Identifiable {
    public var label: String
    public var group: StageGroup
    /// Milliseconds; nil: not measured yet.
    public var ms: Double?
    /// From a model, not a stamp (input one way, the frame wait, scan-out).
    public var estimated: Bool
    public var id: String { label }

    public init(label: String, group: StageGroup, ms: Double?, estimated: Bool) {
        self.label = label
        self.group = group
        self.ms = ms
        self.estimated = estimated
    }
}

/// Everything the stats HUD shows that comes from frames (the app adds the
/// route, the paths and Wi-Fi).
public struct HudFrames: Equatable {
    public var stages: [HudStage] = []
    /// Input to photon, estimated: the sum of the stages.
    public var totalMs: Double?
    public var fpsDecoded = 0
    public var fpsPresented = 0
    public var mbps: Double = 0
    public var late = 0
    public var dropped = 0
    /// Mb/s, one value a second, the last 30 s.
    public var bitrate: [Double] = []
    public var host: HostStatsSnapshot?
    public var decoder = ""
    public var stream = ""
    public init() {}
}

/// Per-frame timestamps, the session's stats files and the HUD's
/// numbers: client-linux's `stats.rs`, plus the phase lock's deadline columns
/// (integration.md 9). Thread-safe.
///
/// Files, in the directory given (`~/Library/Logs/Remote`):
/// - `session-<unix>-frames.csv`: one row per frame, client-linux's columns
///   then `deadline_us,target_present_us`; the first line is the clock's epoch
///   (`mach_absolute_time` in ns on the Mac).
/// - `session-<unix>-summary.json`: per-stage p50/p95/max and counts, and the
///   lead check of integration.md 9.
public final class StatsRecorder {
    public let sessionName: String
    public let directory: URL
    public let epochNs: UInt64

    private let lock = NSLock()
    private var csv: FileHandle?
    private var pending: [UInt32: FrameRec] = [:]
    private var stages: [[Double]]
    private var framesDecoded = 0
    private var framesPresented = 0
    private var framesSkipped = 0
    private var framesLost = 0
    private var framesUndecodable = 0
    private var decodeErrors = 0
    private var bytes = 0
    private var connectStartUs: Int64?
    private var connectedUs: Int64?
    private var firstFrameUs: Int64?
    private var decoder = ""
    private var stream = ""
    private var route = "none"
    private var rttMs = 0.0
    private var loss = 0.0
    private var mtu = 0
    private var bytesReceived: UInt64 = 0
    private var bytesReceivedAt: Int64 = 0
    private var recentTimes: [Int64] = []
    private var recentBytes: [Int] = []
    private var recentPresent: [Int64] = []
    private var recentRecs: [FrameRec] = []
    private var presentIntervals: [Double] = []
    private var recentLate: [Int64] = []
    private var lastPresent: Int64?
    private var refreshNs: UInt32 = 0
    private var host: HostStatsSnapshot?
    private var bitrate: [Double] = []
    private var bucketSecond: Int64 = -1
    private var bucketBytes = 0

    /// A stage: its name and its endpoints in a frame.
    struct Stage {
        let name: String
        let span: (FrameRec) -> (Int64, Int64)?
    }

    static func both(_ a: Int64?, _ b: Int64?) -> (Int64, Int64)? {
        guard let x = a, let y = b else { return nil }
        return (x, y)
    }

    static let stageList: [Stage] = [
        Stage(name: "host render→capture", span: { f in StatsRecorder.both(f.hostRendered, f.hostCaptured) }),
        Stage(name: "host capture→encoded", span: { f in StatsRecorder.both(f.hostCaptured, f.hostEncoded) }),
        Stage(name: "host encoded→sent", span: { f in StatsRecorder.both(f.hostEncoded, f.send) }),
        Stage(name: "network (sent→last packet)", span: { f in StatsRecorder.both(f.send, f.lastPkt) }),
        Stage(name: "decode", span: { f in StatsRecorder.both(f.lastPkt, f.decoded) }),
        Stage(name: "copy to presenter", span: { f in StatsRecorder.both(f.decoded, f.ready) }),
        Stage(name: "wait for present", span: { f in StatsRecorder.both(f.ready, f.present) }),
        Stage(name: "host render→decoded", span: { f in StatsRecorder.both(f.hostRendered, f.ready) }),
        Stage(name: "host render→present", span: { f in StatsRecorder.both(f.hostRendered, f.present) }),
        Stage(name: "lead (ready→deadline)", span: { f in StatsRecorder.both(f.ready, f.deadline) }),
        Stage(name: "deadline→vsync", span: { f in StatsRecorder.both(f.deadline, f.targetPresent) }),
        Stage(name: "deadline→presented", span: { f in StatsRecorder.both(f.deadline, f.present) }),
    ]
    // Indices into stageList.
    static let iRenderCapture = 0
    static let iCaptureEncoded = 1
    static let iEncodedSent = 2
    static let iNetwork = 3
    static let iDecode = 4
    static let iCopy = 5
    static let iWait = 6
    static let iLead = 9
    static let iDeadlineVsync = 10

    static let csvHeader = "frame_id,keyframe,bytes,host_rendered_us,host_captured_us,host_converted_us,host_encoded_us,send_us,first_pkt_us,last_pkt_us,decode_start_us,decoded_us,ready_us,transfer_ms,commit_us,present_us,skipped,deadline_us,target_present_us"

    /// `keep`: how many sessions' files to keep in the directory (0: all).
    public init(directory: URL, epochNs: UInt64, keep: Int = 50) throws {
        self.directory = directory
        self.epochNs = epochNs
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        StatsRecorder.prune(directory, keep: keep)
        let unix = Int(Date().timeIntervalSince1970)
        sessionName = "session-\(unix)"
        stages = Array(repeating: [], count: StatsRecorder.stageList.count)
        let url = directory.appendingPathComponent("\(sessionName)-frames.csv")
        FileManager.default.createFile(atPath: url.path, contents: nil)
        csv = try FileHandle(forWritingTo: url)
        write("# epoch_mono_ns=\(epochNs) (the Mac's mach_absolute_time in ns; add us*1000)\n\(StatsRecorder.csvHeader)\n")
    }

    /// Keep the newest `keep - 1` sessions before a new one starts.
    static func prune(_ dir: URL, keep: Int) {
        guard keep > 0, let names = try? FileManager.default.contentsOfDirectory(atPath: dir.path) else { return }
        var sessions = Set<String>()
        for n in names where n.hasPrefix("session-") {
            let parts = n.split(separator: "-", maxSplits: 2)
            if parts.count == 3 { sessions.insert("session-" + parts[1]) }
        }
        let sorted = sessions.sorted()
        guard sorted.count >= keep else { return }
        for s in sorted.prefix(sorted.count - keep + 1) {
            for n in names where n.hasPrefix(s + "-") {
                try? FileManager.default.removeItem(at: dir.appendingPathComponent(n))
            }
        }
    }

    public func path(_ suffix: String) -> URL {
        directory.appendingPathComponent("\(sessionName)-\(suffix)")
    }

    private func write(_ s: String) {
        if let d = s.data(using: .utf8) { csv?.write(d) }
    }

    public func onConnectStart(nowUs: Int64) {
        lock.lock()
        defer { lock.unlock() }
        if connectStartUs == nil { connectStartUs = nowUs }
    }

    /// A connection came up. Frames of an earlier one never shown are skipped
    /// (frame ids start over with every connection).
    public func onConnected(nowUs: Int64) {
        lock.lock()
        defer { lock.unlock() }
        if connectedUs == nil { connectedUs = nowUs }
        flushPending()
    }

    public func onRoute(_ name: String) {
        lock.lock()
        defer { lock.unlock() }
        route = name
    }

    public func onLink(rttMs: Double, loss: Double, mtu: Int, bytesReceived: UInt64, nowUs: Int64) {
        lock.lock()
        defer { lock.unlock() }
        self.rttMs = rttMs
        self.loss = loss
        self.mtu = mtu
        self.bytesReceived = bytesReceived
        self.bytesReceivedAt = nowUs
    }

    /// Bytes received on the session (every channel) and when the core
    /// counted them (about once a second): idle bandwidth.
    public func bytesReceivedSample() -> (bytes: UInt64, atUs: Int64) {
        lock.lock()
        defer { lock.unlock() }
        return (bytes: bytesReceived, atUs: bytesReceivedAt)
    }

    public func onUndecodable() {
        lock.lock()
        defer { lock.unlock() }
        framesUndecodable += 1
    }

    public func onLost() {
        lock.lock()
        defer { lock.unlock() }
        framesLost += 1
    }

    public func onDecodeError() {
        lock.lock()
        defer { lock.unlock() }
        decodeErrors += 1
    }

    public func setDecoder(_ d: String, stream s: String) {
        lock.lock()
        defer { lock.unlock() }
        decoder = d
        if !s.isEmpty { stream = s }
    }

    public func onHostStats(_ h: HostStatsSnapshot) {
        lock.lock()
        defer { lock.unlock() }
        host = h
    }

    /// (connected, first frame ready), µs after `connect` was called.
    public func connectTimes() -> (connected: Int64?, firstFrame: Int64?) {
        lock.lock()
        defer { lock.unlock() }
        let t0 = connectStartUs ?? 0
        let c: Int64? = connectedUs.map { $0 - t0 }
        let f: Int64? = firstFrameUs.map { $0 - t0 }
        return (connected: c, firstFrame: f)
    }

    /// A frame is decoded and handed to the presenter.
    public func onDecoded(_ rec: FrameRec) {
        lock.lock()
        defer { lock.unlock() }
        framesDecoded += 1
        bytes += rec.bytes
        if firstFrameUs == nil { firstFrameUs = rec.ready }
        recentTimes.append(rec.ready)
        recentBytes.append(rec.bytes)
        while let t = recentTimes.first, rec.ready - t > 1_000_000 {
            recentTimes.removeFirst()
            recentBytes.removeFirst()
        }
        let sec = rec.ready / 1_000_000
        if sec != bucketSecond {
            if bucketSecond >= 0 {
                bitrate.append(Double(bucketBytes) * 8 / 1e6)
                // Seconds with no frame count as 0.
                let gap = Int(min(sec - bucketSecond - 1, 30))
                if gap > 0 { bitrate.append(contentsOf: Array(repeating: 0.0, count: gap)) }
                if bitrate.count > 30 { bitrate.removeFirst(bitrate.count - 30) }
            }
            bucketSecond = sec
            bucketBytes = 0
        }
        bucketBytes += rec.bytes
        pending[rec.frameId] = rec
    }

    /// The presenter drew `frameId` for the update with this deadline and
    /// vsync; older frames it never drew are skipped (latest wins).
    public func onDrawn(frameId: UInt32, commitUs: Int64, deadlineUs: Int64, targetPresentUs: Int64) {
        lock.lock()
        defer { lock.unlock() }
        let older = pending.values.filter { $0.frameId < frameId && $0.commit == nil }.map { $0.frameId }
        for k in older.sorted() {
            if var r = pending.removeValue(forKey: k) {
                r.skipped = true
                finishRec(r)
            }
        }
        pending[frameId]?.commit = commitUs
        pending[frameId]?.deadline = deadlineUs
        pending[frameId]?.targetPresent = targetPresentUs
    }

    /// The drawable carrying `frameId` was presented at `presentUs` (nil: it
    /// was dropped). Returns the receiver report's decode and present µs
    /// (last packet → ready, ready → presented) and the frame's ready time.
    public func onPresented(frameId: UInt32, presentUs: Int64?, refreshNs: UInt32) -> (decodeUs: UInt32, presentUs: UInt32, ready: Int64)? {
        lock.lock()
        defer { lock.unlock() }
        self.refreshNs = refreshNs
        guard var r = pending.removeValue(forKey: frameId) else { return nil }
        guard let p = presentUs else {
            r.skipped = true
            finishRec(r)
            return nil
        }
        r.present = p
        if let l = lastPresent {
            let d = Double(p - l) / 1e3
            presentIntervals.append(d)
            if refreshNs > 0 && d > 1.5 * Double(refreshNs) / 1e6 {
                recentLate.append(p)
            }
        }
        lastPresent = p
        framesPresented += 1
        recentPresent.append(p)
        recentPresent.removeAll { p - $0 > 1_000_000 }
        recentLate.removeAll { p - $0 > 1_000_000 }
        let decodeUs = UInt32(clamping: max(r.ready - r.lastPkt, 0))
        let waitUs = UInt32(clamping: max(p - r.ready, 0))
        let ready = r.ready
        finishRec(r)
        return (decodeUs: decodeUs, presentUs: waitUs, ready: ready)
    }

    /// Lock held: close every pending frame (shown or skipped).
    private func flushPending() {
        let old = pending.values.sorted { $0.frameId < $1.frameId }
        pending.removeAll()
        for var r in old {
            r.skipped = r.present == nil
            finishRec(r)
        }
    }

    /// Lock held.
    private func finishRec(_ r: FrameRec) {
        if r.skipped { framesSkipped += 1 }
        for (n, st) in StatsRecorder.stageList.enumerated() {
            if let span = st.span(r) { stages[n].append(Double(span.1 - span.0) / 1e3) }
        }
        func o(_ v: Int64?) -> String { v.map { String($0) } ?? "" }
        let cols: [String] = [String(r.frameId), r.keyframe ? "1" : "0", String(r.bytes), o(r.hostRendered), o(r.hostCaptured), o(r.hostConverted),
                              o(r.hostEncoded), o(r.send), String(r.firstPkt), String(r.lastPkt), String(r.decodeStart), String(r.decoded),
                              String(r.ready), "0.000", o(r.commit), o(r.present), r.skipped ? "1" : "0", o(r.deadline), o(r.targetPresent)]
        write(cols.joined(separator: ",") + "\n")
        let now = r.ready
        recentRecs.append(r)
        recentRecs.removeAll { now - $0.ready > 1_000_000 }
    }

    /// Lock held: the last second's median of one stage, ms.
    private func median(_ n: Int) -> Double? {
        var v: [Double] = []
        for r in recentRecs {
            if let span = StatsRecorder.stageList[n].span(r) { v.append(Double(span.1 - span.0) / 1e3) }
        }
        return percentile(v, 0.5)
    }

    /// The last second's numbers for the host (`msg::ClientStats`).
    public func clientStats() -> ClientStatsSnapshot? {
        lock.lock()
        defer { lock.unlock() }
        guard !recentTimes.isEmpty else { return nil }
        let decode = (median(StatsRecorder.iDecode) ?? 0) + max(median(StatsRecorder.iCopy) ?? 0, 0)
        let present = median(StatsRecorder.iWait) ?? 0
        return ClientStatsSnapshot(decodeUs: UInt32(clamping: Int(decode * 1000)), presentUs: UInt32(clamping: Int(present * 1000)),
                                   fps: Float(recentPresent.count), framesLate: UInt32(recentLate.count),
                                   framesDropped: UInt32(clamping: framesSkipped + framesLost + framesUndecodable))
    }

    /// The HUD's frame numbers. `rttMs` and `periodMs` feed the estimated
    /// stages (input one way, the wait for the host's frame, scan-out).
    public func hud(rttMs: Double, periodMs: Double) -> HudFrames {
        lock.lock()
        defer { lock.unlock() }
        var h = HudFrames()
        let half: Double? = periodMs > 0 ? periodMs / 2 : nil
        let input: Double? = rttMs > 0 ? rttMs / 2 : nil
        var decode = median(StatsRecorder.iDecode)
        if let d = decode, let c = median(StatsRecorder.iCopy) { decode = d + max(c, 0) }
        h.stages = [
            HudStage(label: "Input to the computer", group: .net, ms: input, estimated: true),
            HudStage(label: "Wait for the next frame", group: .host, ms: half, estimated: true),
            HudStage(label: "Capture", group: .host, ms: median(StatsRecorder.iRenderCapture), estimated: false),
            HudStage(label: "Convert and encode", group: .host, ms: median(StatsRecorder.iCaptureEncoded), estimated: false),
            HudStage(label: "Pace and send", group: .host, ms: median(StatsRecorder.iEncodedSent), estimated: false),
            HudStage(label: "Network", group: .net, ms: median(StatsRecorder.iNetwork), estimated: false),
            HudStage(label: "Decode", group: .mac, ms: decode, estimated: false),
            HudStage(label: "Wait for the display", group: .mac, ms: median(StatsRecorder.iWait), estimated: false),
            HudStage(label: "Panel scan-out", group: .mac, ms: half, estimated: true),
        ]
        var total = 0.0
        var any = false
        for s in h.stages {
            if let ms = s.ms {
                total += ms
                any = true
            }
        }
        h.totalMs = any ? total : nil
        h.fpsDecoded = recentTimes.count
        h.fpsPresented = recentPresent.count
        h.mbps = Double(recentBytes.reduce(0, +)) * 8 / 1e6
        h.late = recentLate.count
        h.dropped = framesSkipped + framesLost + framesUndecodable
        h.bitrate = bitrate
        h.host = host
        h.decoder = decoder
        h.stream = stream
        return h
    }

    /// Flush the CSV and write `summary.json`. Returns its text.
    public func finish(extra: [(String, String)]) -> String {
        lock.lock()
        defer { lock.unlock() }
        flushPending()
        try? csv?.synchronize()
        try? csv?.close()
        csv = nil
        func f(_ v: Double?) -> String {
            guard let x = v else { return "null" }
            return String(format: "%.3f", x)
        }
        func n(_ v: Int64?) -> String {
            guard let x = v else { return "null" }
            return String(x)
        }
        var st: [String] = []
        for (k, s) in StatsRecorder.stageList.enumerated() where !stages[k].isEmpty {
            let v = stages[k]
            st.append("\"\(s.name)\": {\"n\": \(v.count), \"p50\": \(f(percentile(v, 0.5))), \"p95\": \(f(percentile(v, 0.95))), \"max\": \(f(percentile(v, 1)))}")
        }
        let refreshMs = Double(refreshNs) / 1e6
        let late = refreshMs > 0 ? presentIntervals.filter { $0 > 1.5 * refreshMs }.count : 0
        // integration.md 9: ready → present p50 ≈ lead p50 + deadline → vsync p50, within 3 ms.
        var check = "null"
        if let w = percentile(stages[StatsRecorder.iWait], 0.5), let l = percentile(stages[StatsRecorder.iLead], 0.5),
           let g = percentile(stages[StatsRecorder.iDeadlineVsync], 0.5) {
            let pass = abs(w - (l + g)) <= 3
            check = "{\"ready_to_present_p50\": \(f(w)), \"lead_p50\": \(f(l)), \"deadline_to_vsync_p50\": \(f(g)), \"pass\": \(pass)}"
        }
        var lines: [String] = []
        lines.append("\"session\": \"\(sessionName)\"")
        lines.append("\"epoch_mono_ns\": \(epochNs)")
        lines.append("\"client\": \"mac\"")
        lines.append("\"decoder\": \"\(decoder)\"")
        lines.append("\"stream\": \"\(stream)\"")
        lines.append("\"route\": \"\(route)\"")
        lines.append("\"connect_start_us\": \(n(connectStartUs))")
        lines.append("\"connected_us\": \(n(connectedUs))")
        lines.append("\"first_frame_us\": \(n(firstFrameUs))")
        lines.append("\"frames_decoded\": \(framesDecoded)")
        lines.append("\"frames_presented\": \(framesPresented)")
        lines.append("\"frames_skipped\": \(framesSkipped)")
        lines.append("\"frames_lost\": \(framesLost)")
        lines.append("\"frames_undecodable\": \(framesUndecodable)")
        lines.append("\"decode_errors\": \(decodeErrors)")
        lines.append("\"bytes\": \(bytes)")
        lines.append("\"rtt_ms\": \(f(rttMs))")
        lines.append("\"loss\": \(String(format: "%.5f", loss))")
        lines.append("\"mtu\": \(mtu)")
        lines.append("\"present_interval_ms_p50\": \(f(percentile(presentIntervals, 0.5)))")
        lines.append("\"present_intervals_over_1.5_refresh\": \(late)")
        lines.append("\"refresh_ms\": \(f(refreshMs))")
        lines.append("\"host_target_bps\": \(host?.targetBps ?? 0)")
        lines.append("\"host_c_est_bps\": \(host?.cEstBps ?? 0)")
        lines.append("\"host_probe_ceiling_bps\": \(host?.probeCeilingBps ?? 0)")
        lines.append("\"lead_check\": \(check)")
        lines.append("\"stages_ms\": {\n    " + st.joined(separator: ",\n    ") + "\n  }")
        for (k, v) in extra { lines.append("\"\(k)\": \(v)") }
        let out = "{\n  " + lines.joined(separator: ",\n  ") + "\n}\n"
        try? out.write(to: path("summary.json"), atomically: true, encoding: .utf8)
        return out
    }

    /// Stage percentiles for the latency summary: (name, p50, p95, n).
    public func stageTable() -> [(name: String, p50: Double?, p95: Double?, n: Int)] {
        lock.lock()
        defer { lock.unlock() }
        var out: [(name: String, p50: Double?, p95: Double?, n: Int)] = []
        for (k, s) in StatsRecorder.stageList.enumerated() {
            out.append((name: s.name, p50: percentile(stages[k], 0.5), p95: percentile(stages[k], 0.95), n: stages[k].count))
        }
        return out
    }
}
