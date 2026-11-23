// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AVFoundation
import CoreAudio
import Foundation
import RemoteCore

/// A small PCM ring (stereo, Float32) between the feeder and the audio
/// device's render callback.
final class AudioRing {
    private let lock = NSLock()
    private var left: [Float]
    private var right: [Float]
    private var readPos = 0
    private var count = 0

    init(capacity: Int) {
        left = Array(repeating: 0, count: capacity)
        right = Array(repeating: 0, count: capacity)
    }

    var available: Int {
        lock.lock()
        defer { lock.unlock() }
        return count
    }

    func write(_ l: UnsafePointer<Float>, _ r: UnsafePointer<Float>, frames: Int) {
        lock.lock()
        defer { lock.unlock() }
        let cap = left.count
        for i in 0..<frames {
            if count == cap {
                // Full: drop the oldest.
                readPos = (readPos + 1) % cap
                count -= 1
            }
            let w = (readPos + count) % cap
            left[w] = l[i]
            right[w] = r[i]
            count += 1
        }
    }

    func writeSilence(frames: Int) {
        let z = [Float](repeating: 0, count: frames)
        z.withUnsafeBufferPointer { p in write(p.baseAddress!, p.baseAddress!, frames: frames) }
    }

    /// Fills `frames` of the device's buffers; silence past what's queued.
    func read(into abl: UnsafeMutableAudioBufferListPointer, frames: Int) {
        lock.lock()
        defer { lock.unlock() }
        let cap = left.count
        let n = min(frames, count)
        for (ch, buf) in abl.enumerated() {
            guard let raw = buf.mData else { continue }
            let out = raw.assumingMemoryBound(to: Float.self)
            let src = ch == 0 ? left : right
            for i in 0..<frames {
                out[i] = i < n ? src[(readPos + i) % cap] : 0
            }
        }
        readPos = (readPos + n) % cap
        count -= n
    }
}

/// Desktop audio to the Mac: the core's jitter buffer (about 20 ms)
/// → Opus (Apple's decoder) → a 10–20 ms ring → the output device. The feeder
/// pops one 10 ms frame each time the ring drains below 20 ms, so the jitter
/// buffer is drained at the device's pace.
///
/// Apple's Opus decoder has no in-band FEC or concealment API: an FEC or
/// conceal slot plays 10 ms of silence (a later version puts
/// libopus in the core, which brings FEC and concealment to every client).
final class AudioPlayer {
    private let core: RemoteClient
    private let engine = AVAudioEngine()
    private var source: AVAudioSourceNode?
    private let pcm: AVAudioFormat
    private let opus: AVAudioFormat
    private let converter: AVAudioConverter
    private let ring = AudioRing(capacity: 48_000)
    private let queue = DispatchQueue(label: AppIdentity.label("audio"), qos: .userInteractive)
    private var timer: DispatchSourceTimer?
    private static let frame = 480
    private static let keep = 960

    init?(core: RemoteClient) {
        self.core = core
        var asbd = AudioStreamBasicDescription(mSampleRate: 48_000, mFormatID: kAudioFormatOpus, mFormatFlags: 0, mBytesPerPacket: 0,
                                               mFramesPerPacket: UInt32(AudioPlayer.frame), mBytesPerFrame: 0, mChannelsPerFrame: 2,
                                               mBitsPerChannel: 0, mReserved: 0)
        guard let o = AVAudioFormat(streamDescription: &asbd),
              let p = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 48_000, channels: 2, interleaved: false),
              let c = AVAudioConverter(from: o, to: p) else {
            Log.error("audio: no Opus decoder")
            return nil
        }
        opus = o
        pcm = p
        converter = c
    }

    func start(deviceUID: String?) {
        guard source == nil else { return }
        let r = ring
        let node = AVAudioSourceNode(format: pcm) { _, _, frameCount, abl in
            r.read(into: UnsafeMutableAudioBufferListPointer(abl), frames: Int(frameCount))
            return noErr
        }
        engine.attach(node)
        engine.connect(node, to: engine.mainMixerNode, format: pcm)
        source = node
        if let uid = deviceUID, let id = AudioDevices.deviceID(uid: uid), let unit = engine.outputNode.audioUnit {
            var dev = id
            let st = AudioUnitSetProperty(unit, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0, &dev,
                                          UInt32(MemoryLayout<AudioDeviceID>.size))
            if st != noErr { Log.error("audio: couldn't select \(uid): \(st)") }
        }
        engine.prepare()
        do {
            try engine.start()
        } catch {
            Log.error("audio: \(error)")
            return
        }
        let t = DispatchSource.makeTimerSource(queue: queue)
        t.schedule(deadline: .now(), repeating: .milliseconds(5), leeway: .milliseconds(1))
        t.setEventHandler { [weak self] in self?.feed() }
        t.resume()
        timer = t
    }

    func stop() {
        timer?.cancel()
        timer = nil
        engine.stop()
        if let s = source {
            engine.detach(s)
            source = nil
        }
    }

    /// The feeder (its queue).
    private func feed() {
        var guardCount = 0
        while ring.available < AudioPlayer.keep && guardCount < 4 {
            guardCount += 1
            guard let f = core.popAudio() else { return }
            switch f.play {
            case .packet(let data):
                if !decode(data) { ring.writeSilence(frames: AudioPlayer.frame) }
            case .fec, .conceal:
                ring.writeSilence(frames: AudioPlayer.frame)
            }
        }
    }

    private func decode(_ packet: Data) -> Bool {
        guard !packet.isEmpty else { return false }
        let comp = AVAudioCompressedBuffer(format: opus, packetCapacity: 1, maximumPacketSize: max(packet.count, 1500))
        packet.withUnsafeBytes { raw in
            if let b = raw.baseAddress { comp.data.copyMemory(from: b, byteCount: packet.count) }
        }
        comp.byteLength = UInt32(packet.count)
        comp.packetCount = 1
        comp.packetDescriptions?.pointee = AudioStreamPacketDescription(mStartOffset: 0, mVariableFramesInPacket: 0,
                                                                        mDataByteSize: UInt32(packet.count))
        guard let out = AVAudioPCMBuffer(pcmFormat: pcm, frameCapacity: AVAudioFrameCount(AudioPlayer.frame * 2)) else { return false }
        var given = false
        var err: NSError?
        let st = converter.convert(to: out, error: &err) { _, status in
            if given {
                status.pointee = .noDataNow
                return nil
            }
            given = true
            status.pointee = .haveData
            return comp
        }
        guard st != .error, out.frameLength > 0, let ch = out.floatChannelData else {
            if let e = err { Log.error("audio decode: \(e)") }
            return false
        }
        ring.write(ch[0], ch[1], frames: Int(out.frameLength))
        return true
    }
}

/// Output devices (Settings › Audio).
enum AudioDevices {
    static func all() -> [(uid: String, name: String)] {
        var addr = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyDevices, mScope: kAudioObjectPropertyScopeGlobal,
                                              mElement: kAudioObjectPropertyElementMain)
        var size: UInt32 = 0
        guard AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil, &size) == noErr else { return [] }
        var ids = [AudioDeviceID](repeating: 0, count: Int(size) / MemoryLayout<AudioDeviceID>.size)
        guard AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil, &size, &ids) == noErr else { return [] }
        var out: [(uid: String, name: String)] = []
        for id in ids where hasOutput(id) {
            if let uid = string(id, kAudioDevicePropertyDeviceUID), let name = string(id, kAudioObjectPropertyName) {
                out.append((uid: uid, name: name))
            }
        }
        return out
    }

    static func deviceID(uid: String) -> AudioDeviceID? {
        all().isEmpty ? nil : findID(uid)
    }

    private static func findID(_ uid: String) -> AudioDeviceID? {
        var addr = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyDevices, mScope: kAudioObjectPropertyScopeGlobal,
                                              mElement: kAudioObjectPropertyElementMain)
        var size: UInt32 = 0
        guard AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil, &size) == noErr else { return nil }
        var ids = [AudioDeviceID](repeating: 0, count: Int(size) / MemoryLayout<AudioDeviceID>.size)
        guard AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil, &size, &ids) == noErr else { return nil }
        return ids.first { string($0, kAudioDevicePropertyDeviceUID) == uid }
    }

    private static func hasOutput(_ id: AudioDeviceID) -> Bool {
        var addr = AudioObjectPropertyAddress(mSelector: kAudioDevicePropertyStreams, mScope: kAudioDevicePropertyScopeOutput,
                                              mElement: kAudioObjectPropertyElementMain)
        var size: UInt32 = 0
        return AudioObjectGetPropertyDataSize(id, &addr, 0, nil, &size) == noErr && size > 0
    }

    private static func string(_ id: AudioDeviceID, _ selector: AudioObjectPropertySelector) -> String? {
        var addr = AudioObjectPropertyAddress(mSelector: selector, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var value: Unmanaged<CFString>?
        var size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        let st = withUnsafeMutablePointer(to: &value) { p in
            AudioObjectGetPropertyData(id, &addr, 0, nil, &size, p)
        }
        guard st == noErr, let v = value else { return nil }
        return v.takeRetainedValue() as String
    }
}
