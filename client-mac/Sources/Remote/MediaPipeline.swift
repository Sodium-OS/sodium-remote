// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import Foundation
import RemoteCore
import RemoteKit

/// The hot path of one stream, off the main thread: the core's frames →
/// VideoToolbox → the presenter, with every stamp into the stats file, the
/// receiver reports, and the phase lock's lead (integration.md 9: the display
/// link update's deadline minus the frame's ready time).
final class MediaPipeline: VideoSink, DecoderDelegate, PresenterDelegate {
    let core: RemoteClient
    let clock: ClockMap
    let stats: StatsRecorder
    let lead = LeadStats()
    let decoder: VideoDecoder
    let presenter: Presenter

    private let lock = NSLock()
    private var refreshNs: UInt32 = 16_666_667
    private var clickTest: ClickTest?
    /// The decoder's description changed (called on the decoder's queue).
    var onDecoderChanged: (@Sendable (String, Bool) -> Void)?
    /// The first frame of a connection was shown (called on Metal's thread).
    var onFirstPresent: (@Sendable () -> Void)?
    private var presentedAny = false

    init?(core: RemoteClient, clock: ClockMap, stats: StatsRecorder, presenter: Presenter) {
        guard let d = VideoDecoder(core: core, clock: clock, stats: stats, device: presenter.device) else { return nil }
        self.core = core
        self.clock = clock
        self.stats = stats
        self.presenter = presenter
        decoder = d
        d.delegate = self
        presenter.delegate = self
    }

    func setRefresh(hz: Double) {
        lock.lock()
        refreshNs = UInt32(1e9 / max(hz, 1))
        lock.unlock()
    }

    func setClickTest(_ t: ClickTest?) {
        lock.lock()
        clickTest = t
        lock.unlock()
        decoder.setClickTest(t)
    }

    /// A new connection: a new session clock, new frame ids.
    func reconnected() {
        lead.reset()
        decoder.reconnected()
        lock.lock()
        presentedAny = false
        lock.unlock()
    }

    // MARK: VideoSink (the core's thread)

    func videoFrame(_ f: VideoFrame) {
        decoder.videoFrame(f)
    }

    func videoFrameLost(_ id: UInt32) {
        stats.onLost()
    }

    // MARK: DecoderDelegate (the decoder's queue)

    func decoded(_ f: DecodedFrame, rec: FrameRec) {
        presenter.put(f)
    }

    func decoderChanged(description: String, hardware: Bool) {
        onDecoderChanged?(description, hardware)
    }

    // MARK: PresenterDelegate

    func drawn(_ frame: DecodedFrame, commitHost: Double, deadlineHost: Double, vsyncHost: Double) {
        stats.onDrawn(frameId: frame.frameId, commitUs: clock.recorderUs(host: commitHost), deadlineUs: clock.recorderUs(host: deadlineHost),
                      targetPresentUs: clock.recorderUs(host: vsyncHost))
    }

    func presented(_ frame: DecodedFrame, deadlineHost: Double, presentedHost: Double?) {
        lock.lock()
        let period = refreshNs
        let test = clickTest
        let first = !presentedAny && presentedHost != nil
        if presentedHost != nil { presentedAny = true }
        lock.unlock()
        let presentUs = presentedHost.map { clock.recorderUs(host: $0) }
        guard let r = stats.onPresented(frameId: frame.frameId, presentUs: presentUs, refreshNs: period) else { return }
        core.sendReceiverReport(frameId: frame.frameId, decodeUs: r.decodeUs, presentUs: r.presentUs)
        // The lead against the drawing deadline, on the session clock.
        // (none in the immediate modes: they have no drawing deadline)
        if deadlineHost > 0, let ready = clock.sessionUs(host: frame.readyHost), let deadline = clock.sessionUs(host: deadlineHost) {
            lead.add(readyUs: ready, presentedUs: deadline, periodNs: period)
        }
        if let p = presentUs {
            test?.onPresent(frameId: frame.frameId, presentUs: p)
        }
        if first {
            onFirstPresent?()
        }
    }

    /// About once a second: the phase lock's report and the client stats.
    func tick() {
        if let rep = lead.report() {
            core.sendControl(msg: .vsync(rep))
        }
        if let c = stats.clientStats() {
            let cs = ClientStats(decodeUs: c.decodeUs, presentUs: c.presentUs, fps: c.fps, framesLate: c.framesLate, framesDropped: c.framesDropped)
            core.sendControl(msg: .stats(Stats(host: nil, client: cs)))
        }
    }

    func stop() {
        presenter.stop()
        decoder.stop()
    }
}
