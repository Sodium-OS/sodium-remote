// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import Foundation

/// The automatic click-to-photon test (client-linux's `latency.rs`).
/// The Mac clicks on the host's test surface (`spikes/remote-e2e`), which
/// flips a marker on every press; the first decoded frame whose marker has
/// flipped answers the click, and its present time closes the loop. Every time
/// is on the stats recorder's clock (µs).
public final class ClickTest {
    public struct Config {
        public var count: Int
        /// The marker in stream pixels; the luma is read from its middle half.
        public var marker: (x: Int, y: Int, w: Int, h: Int)
        /// Random gap between clicks, so clicks sample every phase of the frame grid.
        public var gapMs: ClosedRange<Int>
        public var timeoutMs: Int
        /// Still time afterwards, for idle bandwidth.
        public var idleMs: Int

        public init(count: Int = 100, marker: (x: Int, y: Int, w: Int, h: Int) = (64, 64, 256, 256),
                    gapMs: ClosedRange<Int> = 150...400, timeoutMs: Int = 1000, idleMs: Int = 3000) {
            self.count = count
            self.marker = marker
            self.gapMs = gapMs
            self.timeoutMs = timeoutMs
            self.idleMs = idleMs
        }
    }

    public enum Action: Equatable {
        case wait
        case click(id: Int)
        case finished
    }

    struct Result {
        var id: Int
        var tClick: Int64
        var frame: FrameRec?
        var present: Int64?
    }

    public let config: Config
    private let lock = NSLock()
    private var marker: Bool?
    private var luma = Double.nan
    private var pending: (id: Int, tClick: Int64, expect: Bool)?
    private var awaiting: [Result] = []
    private var done: [Result] = []
    private var sent = 0
    private let csv: FileHandle?
    private let epochNs: UInt64

    public init(config: Config, csvURL: URL, epochNs: UInt64) {
        self.config = config
        self.epochNs = epochNs
        FileManager.default.createFile(atPath: csvURL.path, contents: nil)
        csv = try? FileHandle(forWritingTo: csvURL)
        // i18n:off the click log and its summary are files for developers, in English
        let head = "# epoch_mono_ns=\(epochNs)\nclick,t_click_us,frame_id,host_rendered_us,host_captured_us,host_encoded_us,send_us,first_pkt_us,last_pkt_us,decoded_us,ready_us,present_us,input_to_ready_ms,input_to_present_ms\n"
        csv?.write(head.data(using: .utf8) ?? Data())
    }

    /// The point to click, stream pixels.
    public var target: (x: Double, y: Double) {
        let m = config.marker
        return (Double(m.x) + Double(m.w) / 2, Double(m.y) + Double(m.h) / 2)
    }

    /// The luma probe: the marker's middle half (x, y, w, h).
    public var probe: (x: Int, y: Int, w: Int, h: Int) {
        let m = config.marker
        return (m.x + m.w / 4, m.y + m.h / 4, max(m.w / 2, 1), max(m.h / 2, 1))
    }

    /// The decoder has a picture whose probe's mean luma is `luma` (0–255).
    public func onPicture(_ rec: FrameRec, luma l: Double) {
        lock.lock()
        defer { lock.unlock() }
        let state = l > 127.5
        luma = l
        marker = state
        if let p = pending, p.expect == state {
            pending = nil
            awaiting.append(Result(id: p.id, tClick: p.tClick, frame: rec, present: nil))
        }
    }

    /// `frameId` was presented. A flip shows in every later frame too, so a
    /// click whose own frame was skipped takes the first presented one after it.
    public func onPresent(frameId: UInt32, presentUs: Int64) {
        lock.lock()
        defer { lock.unlock() }
        var keep: [Result] = []
        for var r in awaiting {
            if let f = r.frame, frameId >= f.frameId {
                r.present = presentUs
                finish(r)
            } else {
                keep.append(r)
            }
        }
        awaiting = keep
    }

    /// Lock held.
    private func finish(_ r: Result) {
        func o(_ v: Int64?) -> String { v.map { String($0) } ?? "" }
        func ms(_ a: Int64, _ b: Int64?) -> String { b.map { String(format: "%.3f", Double($0 - a) / 1e3) } ?? "" }
        let f = r.frame
        let cols: [String] = [String(r.id), String(r.tClick), f.map { String($0.frameId) } ?? "", o(f?.hostRendered), o(f?.hostCaptured),
                              o(f?.hostEncoded), o(f?.send), o(f?.firstPkt), o(f?.lastPkt), o(f?.decoded), o(f?.ready), o(r.present),
                              ms(r.tClick, f?.ready), ms(r.tClick, r.present)]
        csv?.write((cols.joined(separator: ",") + "\n").data(using: .utf8) ?? Data())
        done.append(r)
    }

    /// The scheduler: what to do now.
    public func next(nowUs: Int64) -> Action {
        lock.lock()
        defer { lock.unlock() }
        let timeout = Int64(config.timeoutMs) * 1000
        if let p = pending, nowUs - p.tClick > timeout {
            pending = nil
            finish(Result(id: p.id, tClick: p.tClick, frame: nil, present: nil))
        }
        var keep: [Result] = []
        for r in awaiting {
            if nowUs - r.tClick > timeout { finish(r) } else { keep.append(r) }
        }
        awaiting = keep
        if sent >= config.count {
            return pending == nil && awaiting.isEmpty ? .finished : .wait
        }
        if pending != nil || marker == nil { return .wait }
        return .click(id: sent)
    }

    /// The button-down for click `id` was just handed to the core.
    public func clicked(id: Int, tClickUs: Int64) {
        lock.lock()
        defer { lock.unlock() }
        pending = (id: id, tClick: tClickUs, expect: !(marker ?? false))
        sent += 1
    }

    // i18n:on
    public func status() -> String {
        lock.lock()
        defer { lock.unlock() }
        let p = done.compactMap { r in r.present.map { Double($0 - r.tClick) / 1e3 } }
        let d = done.compactMap { r in r.frame.map { Double($0.ready - r.tClick) / 1e3 } }
        let fp = percentile(p, 0.5).map { String(format: "%.1f", $0) } ?? "–"
        let fd = percentile(d, 0.5).map { String(format: "%.1f", $0) } ?? "–"
        let finished = done.count, total = config.count
        return String(localized: "Latency test \(finished)/\(total) · input→decoded p50 \(fd) · input→present p50 \(fp) ms")
    }

    // i18n:off
    /// The summary in the shape of the table, plus where the click
    /// frames' time went.
    public func summary(context: String, rows: [(String, String, String)]) -> String {
        lock.lock()
        defer { lock.unlock() }
        let got = done.filter { $0.frame != nil }
        func pp(_ v: [Double]) -> String {
            guard let a = percentile(v, 0.5), let b = percentile(v, 0.95) else { return "n/a" }
            return String(format: "%.1f / %.1f ms (n=%d)", a, b, v.count)
        }
        func col(_ f: (Result) -> Double?) -> [Double] { got.compactMap(f) }
        var s = "# Click-to-photon, \(context)\n\n"
        s += "epoch_mono_ns \(epochNs) · clicks \(done.count), answered \(got.count)\n\n"
        s += "| Measure | Target | This run |\n|---|---|---|\n"
        s += "| Input → present, p50 / p95 | ≤ 35 / ≤ 50 ms at 60 Hz; ≤ 25 ms p50 on the home LAN | \(pp(col { r in r.present.map { Double($0 - r.tClick) / 1e3 } })) |\n"
        s += "| Input → decode done, p50 / p95 | (no target) | \(pp(col { r in r.frame.map { Double($0.ready - r.tClick) / 1e3 } })) |\n"
        for (a, b, c) in rows { s += "| \(a) | \(b) | \(c) |\n" }
        s += "\n**Where the time goes (click frames, p50 / p95):**\n\n| Stage | ms |\n|---|---|\n"
        func diff(_ a: Int64?, _ b: Int64?) -> Double? {
            guard let x = a, let y = b else { return nil }
            return Double(y - x) / 1e3
        }
        s += "| click sent → host frame rendered | \(pp(col { r in diff(r.tClick, r.frame?.hostRendered) })) |\n"
        s += "| rendered → captured | \(pp(col { r in diff(r.frame?.hostRendered, r.frame?.hostCaptured) })) |\n"
        s += "| captured → encoded | \(pp(col { r in diff(r.frame?.hostCaptured, r.frame?.hostEncoded) })) |\n"
        s += "| encoded → sent | \(pp(col { r in diff(r.frame?.hostEncoded, r.frame?.send) })) |\n"
        s += "| sent → last packet (pacing + network) | \(pp(col { r in diff(r.frame?.send, r.frame?.lastPkt) })) |\n"
        s += "| last packet → decoded | \(pp(col { r in diff(r.frame?.lastPkt, r.frame?.decoded) })) |\n"
        s += "| decoded → ready (Metal textures) | \(pp(col { r in diff(r.frame?.decoded, r.frame?.ready) })) |\n"
        s += "| ready → presented (display link, compositor) | \(pp(col { r in diff(r.frame?.ready, r.present) })) |\n"
        return s
    }
}
