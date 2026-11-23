// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import ApplicationServices
import CoreGraphics
import Darwin
import Network
import RemoteKit

// macOS services, kept apart from the core's types.

/// Accessibility (for ⌘Tab and the other system shortcuts) and the Local
/// network permission (first run).
enum Permissions {
    /// Without a prompt.
    static var accessibilityAllowed: Bool {
        AXIsProcessTrusted()
    }

    /// Asks macOS (it opens System Settings › Accessibility).
    static func askAccessibility() {
        let opts = ["AXTrustedCheckOptionPrompt": true] as CFDictionary
        _ = AXIsProcessTrustedWithOptions(opts)
    }

    static func open(_ pane: String) {
        let s = "x-apple.systempreferences:com.apple.preference.security?" + pane
        if let u = URL(string: s) { NSWorkspace.shared.open(u) }
    }

    static func openAccessibility() { open("Privacy_Accessibility") }
    static func openLocalNetwork() { open("Privacy_LocalNetwork") }
    static func openMicrophone() { open("Privacy_Microphone") }
    static func openScreenRecording() { open("Privacy_ScreenCapture") }
}

/// Browsing `Brand.mdnsService` (`_sodium-remote._tcp`) makes macOS ask for the Local network permission
/// and tells whether it was refused (the browser waits with PolicyDenied).
final class LocalNetworkProbe {
    private var browser: NWBrowser?
    /// true allowed, false denied; called on main.
    var onResult: ((Bool) -> Void)?

    func start() {
        guard browser == nil else { return }
        let b = NWBrowser(for: .bonjour(type: Brand.mdnsService, domain: nil), using: .tcp)
        b.stateUpdateHandler = { [weak self] state in
            var result: Bool?
            switch state {
            case .ready:
                result = true
            case .waiting(let err), .failed(let err):
                if case .dns(let code) = err, code == -65570 {
                    result = false
                }
            default:
                break
            }
            if let r = result {
                DispatchQueue.main.async { self?.onResult?(r) }
            }
        }
        b.browseResultsChangedHandler = { [weak self] results, _ in
            if !results.isEmpty {
                DispatchQueue.main.async { self?.onResult?(true) }
            }
        }
        b.start(queue: .main)
        browser = b
    }

    func stop() {
        browser?.cancel()
        browser = nil
    }
}

/// The network at the Mac's end ("Waiting for the network…") and
/// its changes (→ the core's `NetworkChanged`).
final class NetworkWatch {
    private let monitor = NWPathMonitor()
    private var first = true
    private var lastKey = ""
    private(set) var satisfied = true
    private(set) var interfaceName = ""
    /// Called on main after the first update, when the path changed.
    var onChange: (() -> Void)?

    func start() {
        monitor.pathUpdateHandler = { [weak self] path in
            DispatchQueue.main.async { self?.update(path) }
        }
        monitor.start(queue: DispatchQueue(label: AppIdentity.label("path")))
    }

    private func update(_ path: NWPath) {
        satisfied = path.status == .satisfied
        let names = path.availableInterfaces.map { $0.name }.joined(separator: ",")
        if path.usesInterfaceType(.wifi) {
            interfaceName = "Wi-Fi"
        } else if path.usesInterfaceType(.wiredEthernet) {
            interfaceName = "Ethernet"
        } else if path.usesInterfaceType(.cellular) {
            interfaceName = String(localized: "a phone")
        } else {
            interfaceName = ""
        }
        let key = "\(satisfied)|\(names)"
        if first {
            first = false
            lastKey = key
            return
        }
        if key != lastKey {
            lastKey = key
            onChange?()
        }
    }

    /// AirDrop and Handoff (AWDL) are up: on Wi-Fi they hold packets back
    static func awdlActive() -> Bool {
        var addrs: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&addrs) == 0, let first = addrs else { return false }
        defer { freeifaddrs(addrs) }
        var p: UnsafeMutablePointer<ifaddrs>? = first
        while let cur = p {
            let name = String(cString: cur.pointee.ifa_name)
            let flags = Int32(truncatingIfNeeded: cur.pointee.ifa_flags)
            if name == "awdl0" && flags & IFF_UP != 0 && flags & IFF_RUNNING != 0 { return true }
            p = cur.pointee.ifa_next
        }
        return false
    }
}

/// macOS acts on ⌘Tab, ⌘Space, ⌃←, ⌘⇧3 and the like before any app sees
/// them; with Accessibility allowed, an event tap takes them first while the
/// stream has the keyboard ("Captured in full screen and when
/// focused"). `SystemKeys.decide` says which: in full screen every one but
/// ⌘Esc and ⌘⇧Esc, the ways out; in a window ⌘Tab, ⌘` and ⌘Space.
final class ShortcutTap {
    private var tap: CFMachPort?
    private var source: CFRunLoopSource?
    /// Take the keys now (the session keeps it current).
    var active = false {
        didSet { if !active { held.removeAll() } }
    }
    /// The stream is in full screen: every system key goes to the computer.
    var fullScreen = false
    /// The Mac's input method is in use: ⌃Space stays its switch.
    var keepInputSwitch = true
    /// (key code, down); called on main.
    var onKey: ((UInt16, Bool) -> Void)?
    /// ⌘Esc (false) or ⌘⇧Esc (true: disconnect); called on main.
    var onLeave: ((Bool) -> Void)?
    /// Keys taken down, whose key-up is taken too.
    private var held = Set<UInt16>()

    var running: Bool { tap != nil }

    /// False without Accessibility.
    func start() -> Bool {
        if tap != nil { return true }
        let mask = CGEventMask((1 << CGEventType.keyDown.rawValue) | (1 << CGEventType.keyUp.rawValue))
        let me = Unmanaged.passUnretained(self).toOpaque()
        guard let t = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap, options: .defaultTap,
                                        eventsOfInterest: mask, callback: shortcutTapCallback, userInfo: me) else { return false }
        let src = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, t, 0)
        CFRunLoopAddSource(CFRunLoopGetMain(), src, .commonModes)
        CGEvent.tapEnable(tap: t, enable: true)
        tap = t
        source = src
        return true
    }

    func stop() {
        if let t = tap { CGEvent.tapEnable(tap: t, enable: false) }
        if let s = source { CFRunLoopRemoveSource(CFRunLoopGetMain(), s, .commonModes) }
        tap = nil
        source = nil
        held.removeAll()
    }

    fileprivate func handle(_ type: CGEventType, _ event: CGEvent) -> Bool {
        if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
            if let t = tap { CGEvent.tapEnable(tap: t, enable: true) }
            return false
        }
        guard active, type == .keyDown || type == .keyUp else { return false }
        let code = UInt16(truncatingIfNeeded: event.getIntegerValueField(.keyboardEventKeycode))
        if type == .keyUp {
            guard held.remove(code) != nil else { return false }
            onKey?(code, false)
            return true
        }
        let flags = UInt(truncatingIfNeeded: event.flags.rawValue)
        let repeated = event.getIntegerValueField(.keyboardEventAutorepeat) != 0
        switch SystemKeys.decide(keyCode: code, flags: flags, fullScreen: fullScreen, keepInputSwitch: keepInputSwitch) {
        case .pass:
            return false
        case .take:
            held.insert(code)
            onKey?(code, true)
        case .leave:
            if !repeated { onLeave?(false) }
        case .disconnect:
            if !repeated { onLeave?(true) }
        }
        return true
    }
}

private func shortcutTapCallback(proxy: CGEventTapProxy, type: CGEventType, event: CGEvent, refcon: UnsafeMutableRawPointer?) -> Unmanaged<CGEvent>? {
    guard let r = refcon else { return Unmanaged.passUnretained(event) }
    let me = Unmanaged<ShortcutTap>.fromOpaque(r).takeUnretainedValue()
    return me.handle(type, event) ? nil : Unmanaged.passUnretained(event)
}

/// The cursor from the cursor channel (`msg::CursorImage`: premultiplied
/// BGRA in stream pixels), drawn by macOS at the stream's point size.
enum CursorFactory {
    static let hidden: NSCursor = NSCursor(image: NSImage(size: NSSize(width: 1, height: 1)), hotSpot: .zero)

    /// `pointsPerPixel`: stream pixels → points (1 / backing scale at 1:1).
    static func make(width: Int, height: Int, hotX: Int, hotY: Int, pixels: Data, pointsPerPixel: Double) -> NSCursor? {
        guard width > 0, height > 0, pixels.count >= width * height * 4,
              let cs = CGColorSpace(name: CGColorSpace.sRGB),
              let provider = CGDataProvider(data: pixels as CFData) else { return nil }
        let info = CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)
        guard let img = CGImage(width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: width * 4, space: cs,
                                bitmapInfo: info, provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent) else { return nil }
        let size = NSSize(width: Double(width) * pointsPerPixel, height: Double(height) * pointsPerPixel)
        let image = NSImage(cgImage: img, size: size)
        let hot = NSPoint(x: min(Double(hotX) * pointsPerPixel, size.width - 1), y: min(Double(hotY) * pointsPerPixel, size.height - 1))
        return NSCursor(image: image, hotSpot: NSPoint(x: max(hot.x, 0), y: max(hot.y, 0)))
    }
}
