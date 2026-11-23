// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import RemoteCore

/// Clipboard both ways, automatic (session-api §1): text, HTML, RTF
/// and images. Items up to 64 KiB go inline with the offer; bigger ones are
/// asked for and come in 64 KiB chunks. Entries marked sensitive go on the
/// pasteboard with `org.nspasteboard.ConcealedType` (clipboard managers skip
/// them) and are never stored. Silent: no toast.
@MainActor
final class ClipboardSync {
    static let concealedType = NSPasteboard.PasteboardType("org.nspasteboard.ConcealedType")
    static let chunk = 64 * 1024
    /// Bigger items aren't offered.
    static let maxItem = 32 * 1024 * 1024

    private let core: RemoteClient
    private let pb = NSPasteboard.general
    private var lastChange: Int
    private var serial: UInt32 = 0
    private var timer: Timer?
    /// Our latest offer's items, for the host's requests.
    private var offered: [ClipFormat: Data] = [:]
    /// The host's offer being assembled.
    private var inSerial: UInt32?
    private var inConcealed = false
    private var inItems: [ClipFormat: Data] = [:]
    private var inWaiting: Set<ClipFormat> = []
    /// The size the computer offered for each item still being received: a chunk may not go past it.
    private var inSizes: [ClipFormat: UInt64] = [:]

    init(core: RemoteClient) {
        self.core = core
        lastChange = NSPasteboard.general.changeCount
    }

    /// Start polling the Mac's pasteboard; the current clipboard goes at once.
    func start() {
        timer?.invalidate()
        offer()
        timer = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.poll() }
        }
    }

    func stop() {
        timer?.invalidate()
        timer = nil
        offered = [:]
    }

    private func poll() {
        if pb.changeCount != lastChange { offer() }
    }

    static func type(for f: ClipFormat) -> NSPasteboard.PasteboardType? {
        switch f {
        case .text: return .string
        case .html: return .html
        case .rtf: return .rtf
        case .png: return .png
        case .jpeg: return NSPasteboard.PasteboardType("public.jpeg")
        case .mime: return nil
        }
    }

    /// Mac → computer: what the pasteboard holds now.
    private func offer() {
        lastChange = pb.changeCount
        var data: [ClipFormat: Data] = [:]
        if let s = pb.string(forType: .string), let d = s.data(using: .utf8) { data[.text] = d }
        if let d = pb.data(forType: .html) { data[.html] = d }
        if let d = pb.data(forType: .rtf) { data[.rtf] = d }
        if let d = pb.data(forType: .png) {
            data[.png] = d
        } else if let t = pb.data(forType: .tiff), let rep = NSBitmapImageRep(data: t), let png = rep.representation(using: .png, properties: [:]) {
            data[.png] = png
        }
        data = data.filter { $0.value.count <= ClipboardSync.maxItem }
        guard !data.isEmpty else { return }
        let concealed = pb.types?.contains(ClipboardSync.concealedType) ?? false
        serial &+= 1
        offered = data
        let items = data.map { (f, d) in
            ClipItem(format: f, size: UInt64(d.count), inline: d.count <= ClipboardSync.chunk ? d : nil)
        }
        core.sendControl(msg: .clipboardOffer(ClipboardOffer(serial: serial, items: items, concealed: concealed)))
    }

    /// The computer asks for one of our items.
    func onRequest(serial s: UInt32, format: ClipFormat) {
        guard s == serial, let d = offered[format] else { return }
        let total = UInt64(d.count)
        var off = 0
        repeat {
            let end = min(off + ClipboardSync.chunk, d.count)
            let part = d.subdata(in: off..<end)
            core.sendControl(msg: .clipboardData(ClipboardData(serial: s, format: format, offset: UInt64(off), total: total, bytes: part)))
            off = end
        } while off < d.count
    }

    /// Computer → Mac: a new clipboard.
    func onOffer(_ o: ClipboardOffer) {
        inSerial = o.serial
        inConcealed = o.concealed
        inItems = [:]
        inWaiting = []
        inSizes = [:]
        for item in o.items where ClipboardSync.type(for: item.format) != nil && item.size <= UInt64(ClipboardSync.maxItem) {
            if let d = item.inline {
                inItems[item.format] = d
            } else {
                inWaiting.insert(item.format)
                inSizes[item.format] = item.size
                inItems[item.format] = Data()
                core.sendControl(msg: .clipboardRequest(serial: o.serial, format: item.format))
            }
        }
        if inWaiting.isEmpty { write() }
    }

    func onData(_ d: ClipboardData) {
        guard d.serial == inSerial, inWaiting.contains(d.format) else { return }
        var buf = inItems[d.format] ?? Data()
        // The offset is the computer's number: Int(exactly:) never traps, and nothing is kept past the size it offered.
        guard let off = Int(exactly: d.offset), let size = inSizes[d.format],
              UInt64(off) <= size, UInt64(d.bytes.count) <= size - UInt64(off) else { return }
        if off == buf.count { buf.append(d.bytes) }
        inItems[d.format] = buf
        if UInt64(buf.count) >= size {
            inWaiting.remove(d.format)
            if inWaiting.isEmpty { write() }
        }
    }

    private func write() {
        guard !inItems.isEmpty else { return }
        pb.clearContents()
        var types: [NSPasteboard.PasteboardType] = []
        for f in inItems.keys {
            if let t = ClipboardSync.type(for: f) { types.append(t) }
        }
        if inConcealed { types.append(ClipboardSync.concealedType) }
        pb.declareTypes(types, owner: nil)
        for (f, d) in inItems {
            guard let t = ClipboardSync.type(for: f) else { continue }
            if f == .text, let s = String(data: d, encoding: .utf8) {
                pb.setString(s, forType: t)
            } else {
                pb.setData(d, forType: t)
            }
        }
        if inConcealed { pb.setData(Data(), forType: ClipboardSync.concealedType) }
        // Not echoed back.
        lastChange = pb.changeCount
        inItems = [:]
    }
}
