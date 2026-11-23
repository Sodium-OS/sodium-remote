// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import Foundation
import Metal
import QuartzCore
import RemoteKit
import simd

/// Called on the render thread (drawn) and on Metal's thread (presented).
protocol PresenterDelegate: AnyObject {
    /// `frame` was encoded into the drawable of the update with this drawing
    /// deadline (`targetTimestamp`) and vsync (`targetPresentationTimestamp`).
    func drawn(_ frame: DecodedFrame, commitHost: Double, deadlineHost: Double, vsyncHost: Double)
    /// That drawable was shown at `presentedHost` (nil: dropped).
    func presented(_ frame: DecodedFrame, deadlineHost: Double, presentedHost: Double?)
}

/// The YCbCr → RGB matrix and offsets, as the shader's four rows.
struct ColorParams {
    var r: SIMD4<Float>
    var g: SIMD4<Float>
    var b: SIMD4<Float>
    var off: SIMD4<Float>

    /// BT.709 (or 601), full or video range (the host sends BT.709 full range).
    static func make(fullRange: Bool, bt601: Bool) -> ColorParams {
        let kr: Float = bt601 ? 0.299 : 0.2126
        let kb: Float = bt601 ? 0.114 : 0.0722
        let kg: Float = 1 - kr - kb
        var ys: Float = 1
        var cs: Float = 1
        var off = SIMD4<Float>(0, 0.5, 0.5, 0)
        if !fullRange {
            ys = 255.0 / 219.0
            cs = 255.0 / 224.0
            off = SIMD4<Float>(16.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 0)
        }
        let crR: Float = 2 * (1 - kr) * cs
        let cbB: Float = 2 * (1 - kb) * cs
        let cbG: Float = -2 * (1 - kb) * kb / kg * cs
        let crG: Float = -2 * (1 - kr) * kr / kg * cs
        return ColorParams(r: SIMD4<Float>(ys, 0, crR, 0), g: SIMD4<Float>(ys, cbG, crG, 0), b: SIMD4<Float>(ys, cbB, 0, 0), off: off)
    }
}

/// Presents decoded frames on a CAMetalLayer in phase with the display
/// `CAMetalDisplayLink` with `preferredFrameLatency` 1 on
/// its own thread, latest frame wins, a late frame is shown at the next
/// update and never dropped for an older one, nothing is drawn when nothing
/// changed. The stream is drawn 1:1 or fitted (`StreamLayout`).
final class Presenter: NSObject {
    /// How frames reach the screen (`defaults write com.sodiumos.remote RemotePresentMode …`, read at each session):
    /// - `link`: `CAMetalDisplayLink`, drawn in its update for its vsync. On the M3 Air (macOS 15.3)
    ///   it targets 3 refreshes after the update even at `preferredFrameLatency` 1, about 50 ms.
    /// - `immediate`: drawn as soon as a frame is decoded and presented at the next vsync.
    /// - `nosync`: the same with `displaySyncEnabled` off: shown at the next composite, no vsync wait.
    /// - `paced` (the default): no vsync wait either, but the newest frame is drawn once a refresh, `RemotePaceMs` before the
    ///   screen's next vsync (the drawing deadline the host's phase lock aims at): even pacing, one frame queued.
    ///   `RemotePaceSweep` = 1 steps the offset through 1–9 ms, 4 s each, to find the latest that still makes it.
    enum Mode: String {
        case link, immediate, nosync, paced
    }

    let layer: CAMetalLayer
    let mode: Mode
    let device: MTLDevice
    private let queue: MTLCommandQueue
    private let pipeline: MTLRenderPipelineState
    private let sampler: MTLSamplerState
    weak var delegate: PresenterDelegate?

    private let lock = NSLock()
    private var pending: DecodedFrame?
    private var shown: DecodedFrame?
    private var redraw = false
    private var drawableSize = CGSize.zero
    private var fitAlways = false
    private var background = SIMD4<Double>(0.027, 0.031, 0.047, 1)
    private var stopped = false
    private var thread: Thread?
    private var preferredHz: Float = 60
    private let drawQueue = DispatchQueue(label: AppIdentity.label("present"), qos: .userInteractive)
    // The immediate modes' drawing deadlines (the phase lock's, integration.md 9): the screen's next vsync from a
    // CADisplayLink, less the time the window server needs before it to take a new frame for that vsync
    // (`RemoteCompositeLeadMs`, default 10: the nosync session's fastest drawn → shown was 8.6 ms).
    private var nextVsync: Double = 0
    private var period: Double = 0
    private let compositeLead: Double = {
        let ms = UserDefaults.standard.double(forKey: "RemoteCompositeLeadMs")
        return (ms > 0 ? ms : 10) / 1000
    }()
    private let paceOffset: Double = {
        // The M3 Air's window server takes a frame for a vsync about 9 ms before it (2026-11-07 sweep: 96.5 % made
        // it at 9 ms, 36–71 % at 1–7 ms); 10 leaves a little margin.
        let ms = UserDefaults.standard.double(forKey: "RemotePaceMs")
        return (ms > 0 ? ms : 10) / 1000
    }()
    private let paceSweep = UserDefaults.standard.bool(forKey: "RemotePaceSweep")
    private var sweepStart: Double = 0
    private static let sweepMs: [Double] = [1, 2, 3, 4, 5, 6, 7, 9]

    /// The offset before the next vsync at which `paced` draws (fixed, or the sweep's step).
    private func currentOffset(now: Double) -> Double {
        guard paceSweep else { return paceOffset }
        if sweepStart == 0 { sweepStart = now }
        let step = Int((now - sweepStart) / 4) % Presenter.sweepMs.count
        return Presenter.sweepMs[step] / 1000
    }

    static let shaderSource = """
    #include <metal_stdlib>
    using namespace metal;

    struct VOut {
        float4 pos [[position]];
        float2 uv;
    };

    vertex VOut remote_vertex(uint vid [[vertex_id]]) {
        float2 p = float2(float((vid << 1) & 2), float(vid & 2));
        VOut o;
        o.pos = float4(p * 2.0 - 1.0, 0.0, 1.0);
        o.uv = float2(p.x, 1.0 - p.y);
        return o;
    }

    struct Params {
        float4 r;
        float4 g;
        float4 b;
        float4 off;
    };

    fragment float4 remote_fragment(VOut in [[stage_in]],
                                    texture2d<float> luma [[texture(0)]],
                                    texture2d<float> chroma [[texture(1)]],
                                    sampler s [[sampler(0)]],
                                    constant Params& p [[buffer(0)]]) {
        float3 ycc = float3(luma.sample(s, in.uv).r, chroma.sample(s, in.uv).rg) - p.off.xyz;
        return float4(dot(p.r.xyz, ycc), dot(p.g.xyz, ycc), dot(p.b.xyz, ycc), 1.0);
    }
    """

    init?(layer: CAMetalLayer) {
        guard let dev = layer.device ?? MTLCreateSystemDefaultDevice(), let q = dev.makeCommandQueue() else { return nil }
        self.layer = layer
        mode = Mode(rawValue: UserDefaults.standard.string(forKey: "RemotePresentMode") ?? "") ?? .paced
        device = dev
        queue = q
        layer.device = dev
        layer.pixelFormat = .bgra8Unorm
        layer.framebufferOnly = true
        // The host's pixels are sRGB-encoded (BT.709 on the sRGB values):
        // macOS colour-manages them to the panel.
        layer.colorspace = CGColorSpace(name: CGColorSpace.sRGB)
        // The immediate modes keep one frame queued for the screen at most: two drawables, and the newest frame is
        // taken only once one is free. The host can send two frames in a refresh (refinement passes go half an
        // interval after a frame), and three drawables then stood as a queue of three refreshes (48 ms).
        layer.maximumDrawableCount = mode == .link ? 3 : 2
        layer.displaySyncEnabled = mode != .nosync && mode != .paced
        do {
            // Compiled at run time: the Command Line Tools have no Metal compiler.
            let lib = try dev.makeLibrary(source: Presenter.shaderSource, options: nil)
            let d = MTLRenderPipelineDescriptor()
            d.vertexFunction = lib.makeFunction(name: "remote_vertex")
            d.fragmentFunction = lib.makeFunction(name: "remote_fragment")
            d.colorAttachments[0].pixelFormat = .bgra8Unorm
            pipeline = try dev.makeRenderPipelineState(descriptor: d)
        } catch {
            Log.error("Metal pipeline: \(error)")
            return nil
        }
        let sd = MTLSamplerDescriptor()
        sd.minFilter = .linear
        sd.magFilter = .linear
        sd.sAddressMode = .clampToEdge
        sd.tAddressMode = .clampToEdge
        guard let s = dev.makeSamplerState(descriptor: sd) else { return nil }
        sampler = s
        super.init()
    }

    /// Start the display link on its own thread.
    func start(preferredHz: Int) {
        lock.lock()
        self.preferredHz = Float(max(30, preferredHz))
        stopped = false
        lock.unlock()
        Log.info("presenter: \(mode.rawValue), composite lead \(Int(compositeLead * 1000)) ms, pace \(paceSweep ? "sweep" : "\(paceOffset * 1000) ms")")
        let t = Thread { [weak self] in
            self?.runLoop()
        }
        t.name = AppIdentity.label("present")
        t.qualityOfService = .userInteractive
        thread = t
        t.start()
    }

    private func runLoop() {
        var metalLink: CAMetalDisplayLink?
        var screenLink: CADisplayLink?
        if mode == .link {
            let link = CAMetalDisplayLink(metalLayer: layer)
            link.delegate = self
            link.preferredFrameLatency = 1
            lock.lock()
            let hz = preferredHz
            lock.unlock()
            link.preferredFrameRateRange = CAFrameRateRange(minimum: min(30, hz), maximum: max(hz, 60), preferred: hz)
            link.add(to: RunLoop.current, forMode: .default)
            metalLink = link
        } else {
            // Timing only: the frames are drawn on decode (drawNow).
            screenLink = NSScreen.main?.displayLink(target: self, selector: #selector(vsyncTick(_:)))
            screenLink?.add(to: RunLoop.current, forMode: .default)
        }
        while true {
            lock.lock()
            let done = stopped
            lock.unlock()
            if done { break }
            _ = RunLoop.current.run(mode: .default, before: Date(timeIntervalSinceNow: 0.25))
        }
        metalLink?.invalidate()
        screenLink?.invalidate()
    }

    @objc private func vsyncTick(_ link: CADisplayLink) {
        lock.lock()
        nextVsync = link.targetTimestamp
        let p = link.targetTimestamp - link.timestamp
        if p > 0.002 && p < 0.05 { period = p }
        lock.unlock()
        guard mode == .paced else { return }
        // Draw the newest frame `offset` before the next vsync; a fire time already past goes at once.
        let now = CACurrentMediaTime()
        let vsync = link.targetTimestamp
        let due = vsync - currentOffset(now: now)
        drawQueue.asyncAfter(deadline: .now() + max(0, due - now), qos: .userInteractive) { [weak self] in
            self?.drawNow(paced: (due: due, vsync: vsync))
        }
    }

    /// The first drawing deadline after `ready` on the vsync grid; 0 before the display link has ticked.
    private func deadline(after ready: Double) -> Double {
        lock.lock()
        let v = nextVsync
        let p = period
        lock.unlock()
        guard v > 0, p > 0 else { return 0 }
        let d = v - compositeLead
        return d + ((ready - d) / p).rounded(.down) * p + p
    }

    func stop() {
        lock.lock()
        stopped = true
        pending = nil
        shown = nil
        lock.unlock()
    }

    /// A decoded frame: it replaces any frame not yet drawn (latest wins).
    func put(_ f: DecodedFrame) {
        lock.lock()
        let skipped = pending
        pending = f
        lock.unlock()
        if let s = skipped {
            delegate?.presented(s, deadlineHost: 0, presentedHost: nil)
        }
        if mode == .immediate || mode == .nosync {
            drawQueue.async { [weak self] in self?.drawNow() }
        }
    }

    /// The view's size in pixels; `fitAlways` forces a fitted placement (Fixed
    /// resolution) instead of 1:1. Redraws the last frame.
    func setLayout(drawableSize size: CGSize, fitAlways: Bool, background bg: SIMD4<Double>) {
        lock.lock()
        drawableSize = size
        self.fitAlways = fitAlways
        background = bg
        redraw = true
        lock.unlock()
        if mode == .immediate || mode == .nosync {
            drawQueue.async { [weak self] in self?.drawNow() }
        }
    }

    /// `immediate` and `nosync`: draw the newest frame now (or the shown one again after a layout change).
    private func drawNow(paced: (due: Double, vsync: Double)? = nil) {
        lock.lock()
        let anything = pending != nil || redraw
        lock.unlock()
        guard anything else { return }
        // A free drawable first (this waits while the other one is still queued for the screen), then the frame:
        // whatever arrived in the meantime replaced the older one (`put`).
        guard let drawable = layer.nextDrawable() else { return }
        lock.lock()
        let fresh = pending
        pending = nil
        let frame = fresh ?? (redraw ? shown : nil)
        redraw = false
        let size = drawableSize
        let fit = fitAlways
        let bg = background
        let done = stopped
        if let f = fresh { shown = f }
        lock.unlock()
        guard let f = frame, !done else { return }
        if size.width > 0 && size.height > 0 && layer.drawableSize != size {
            layer.drawableSize = size
        }
        guard let cb = encode(f, into: drawable.texture, fit: fit, background: bg) else { return }
        let due = fresh == nil ? 0 : paced?.due ?? deadline(after: f.readyHost)
        if fresh != nil {
            drawable.addPresentedHandler { [weak self] d in
                let t = d.presentedTime
                self?.delegate?.presented(f, deadlineHost: due, presentedHost: t > 0 ? t : nil)
            }
        }
        cb.present(drawable)
        cb.addCompletedHandler { _ in
            _ = f
        }
        cb.commit()
        if fresh != nil {
            let now = CACurrentMediaTime()
            let vsync = paced?.vsync ?? (due > 0 ? due + compositeLead : now)
            delegate?.drawn(f, commitHost: now, deadlineHost: due > 0 ? due : now, vsyncHost: vsync)
        }
    }

    /// The frame drawn into `tex`, placed 1:1 or fitted; nil if Metal refused.
    private func encode(_ f: DecodedFrame, into tex: MTLTexture, fit: Bool, background bg: SIMD4<Double>) -> MTLCommandBuffer? {
        let target = PixelSize(width: tex.width, height: tex.height)
        let stream = PixelSize(width: f.width, height: f.height)
        var p = StreamLayout.place(window: target, stream: stream)
        if fit {
            // Fixed resolution: always fit with the aspect kept.
            let k = min(Double(target.width) / Double(max(stream.width, 1)), Double(target.height) / Double(max(stream.height, 1)))
            let w = Double(stream.width) * k
            let h = Double(stream.height) * k
            p = Placement(x: ((Double(target.width) - w) / 2).rounded(.down), y: ((Double(target.height) - h) / 2).rounded(.down), w: w, h: h)
        }
        guard let cb = queue.makeCommandBuffer() else { return nil }
        let rp = MTLRenderPassDescriptor()
        rp.colorAttachments[0].texture = tex
        rp.colorAttachments[0].loadAction = .clear
        rp.colorAttachments[0].storeAction = .store
        rp.colorAttachments[0].clearColor = MTLClearColor(red: bg.x, green: bg.y, blue: bg.z, alpha: bg.w)
        guard let enc = cb.makeRenderCommandEncoder(descriptor: rp) else { return nil }
        enc.setViewport(MTLViewport(originX: p.x, originY: p.y, width: p.w, height: p.h, znear: 0, zfar: 1))
        enc.setRenderPipelineState(pipeline)
        enc.setFragmentTexture(f.luma, index: 0)
        enc.setFragmentTexture(f.chroma, index: 1)
        enc.setFragmentSamplerState(sampler, index: 0)
        var params = ColorParams.make(fullRange: f.fullRange, bt601: f.bt601)
        enc.setFragmentBytes(&params, length: MemoryLayout<ColorParams>.stride, index: 0)
        enc.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
        enc.endEncoding()
        return cb
    }
}

// The conformance lives in an extension, so an SDK that ever isolates the
// protocol can't make the whole presenter main-actor.
extension Presenter: CAMetalDisplayLinkDelegate {
    // The render thread.
    func metalDisplayLink(_ link: CAMetalDisplayLink, needsUpdate update: CAMetalDisplayLink.Update) {
        lock.lock()
        let fresh = pending
        pending = nil
        let frame = fresh ?? (redraw ? shown : nil)
        redraw = false
        let size = drawableSize
        let fit = fitAlways
        let bg = background
        if let f = fresh { shown = f }
        lock.unlock()
        guard let f = frame else { return }
        if size.width > 0 && size.height > 0 && layer.drawableSize != size {
            layer.drawableSize = size
        }
        let drawable = update.drawable
        guard let cb = encode(f, into: drawable.texture, fit: fit, background: bg) else { return }

        let deadline = update.targetTimestamp
        let vsync = update.targetPresentationTimestamp
        let isFresh = fresh != nil
        if isFresh {
            drawable.addPresentedHandler { [weak self] d in
                let t = d.presentedTime
                self?.delegate?.presented(f, deadlineHost: deadline, presentedHost: t > 0 ? t : nil)
            }
        }
        cb.present(drawable)
        // Keep the CVMetalTextures alive until the GPU has read them.
        cb.addCompletedHandler { _ in
            _ = f
        }
        cb.commit()
        if isFresh {
            delegate?.drawn(f, commitHost: CACurrentMediaTime(), deadlineHost: deadline, vsyncHost: vsync)
        }
    }
}
