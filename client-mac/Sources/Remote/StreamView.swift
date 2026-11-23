// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import QuartzCore
import RemoteCore
import RemoteKit

/// What the stream view needs from its session.
@MainActor
protocol StreamInputSink: AnyObject {
    /// `eventTime`: the NSEvent's timestamp (host seconds), or nil for now.
    func send(_ e: InputEvent, eventTime: Double?)
    /// A point in the view (flipped, points) as stream pixels, clamped.
    func streamPoint(_ p: NSPoint) -> (x: Double, y: Double)?
    /// The caret (stream pixels) as a rect in the view (points), for the candidate window.
    func caretInView() -> NSRect?
    /// Keys go to the computer (not released with ⌃⌥⌘K).
    var capturingKeys: Bool { get }
    /// Compose with this Mac's input method.
    var macInputMethod: Bool { get }
    /// A password field has focus on the computer.
    var passwordFocus: Bool { get }
    /// Pointer lock: relative motion.
    var pointerLocked: Bool { get }
    /// ⌃⌥⌘ + a key, Remote's own; true when handled.
    func remoteShortcut(_ e: NSEvent) -> Bool
    /// A click in the stream (takes the keyboard back after ⌃⌥⌘K).
    func streamClicked()
    /// The view's size or backing scale changed.
    func viewResized()
    /// Keyboard focus left the stream: everything held is released.
    func focusLost()
}

/// The stream: a CAMetalLayer the presenter draws into, and every input
/// event: absolute pointer with coalescing off, buttons, trackpad
/// scroll with phases and momentum, pinch, rotate, 3-finger swipe and hold,
/// keys as HID usages exactly as pressed, and the Mac's input method.
final class StreamView: NSView, NSTextInputClient {
    weak var sink: StreamInputSink?
    let metalLayer = CAMetalLayer()
    private var tracking: NSTrackingArea?
    private var cursor: NSCursor = NSCursor.arrow

    // Keys.
    private var keysDown = Set<UInt16>()
    private var modifiersDown = Set<UInt16>()
    private var keyUpMonitor: Any?
    // The input method, around one key event.
    private var currentKey: NSEvent?
    private var markedBefore = false
    private var plainKey = false
    private var consumed = false
    private var marked = NSMutableAttributedString()
    private var markedSelection = NSRange(location: 0, length: 0)
    // Scrolling.
    private var scrollMovedX = false
    private var scrollMovedY = false
    private var awaitingMomentum = false
    private var inMomentum = false
    private var stopTimer: Timer?
    // Gestures.
    private var pinchActive = false
    private var pinchScale: Double = 1
    private var swipeActive = false
    private var swipeFingers = 0
    private var swipeLast: NSPoint?
    private var holdActive = false

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layerContentsRedrawPolicy = .never
        allowedTouchTypes = [.indirect]
        wantsRestingTouches = false
    }

    required init?(coder: NSCoder) {
        fatalError("not used")
    }

    override func makeBackingLayer() -> CALayer {
        metalLayer
    }

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override var isOpaque: Bool { true }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool {
        true
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        metalLayer.contentsScale = window?.backingScaleFactor ?? 2
        sink?.viewResized()
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        sink?.viewResized()
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        metalLayer.contentsScale = window?.backingScaleFactor ?? 2
        installKeyUpMonitor(window != nil)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = tracking { removeTrackingArea(t) }
        let t = NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect, .cursorUpdate],
                               owner: self, userInfo: nil)
        addTrackingArea(t)
        tracking = t
    }

    // MARK: - The cursor (drawn locally from the cursor channel)

    func setCursor(_ c: NSCursor) {
        cursor = c
        window?.invalidateCursorRects(for: self)
        if let w = window, w.isKeyWindow {
            let p = convert(w.mouseLocationOutsideOfEventStream, from: nil)
            if bounds.contains(p) { c.set() }
        }
    }

    override func resetCursorRects() {
        addCursorRect(bounds, cursor: cursor)
    }

    override func cursorUpdate(with event: NSEvent) {
        cursor.set()
    }

    /// Pointer lock (games): the Mac's cursor stays put and hidden; motion goes relative.
    func setPointerLock(_ on: Bool) {
        CGAssociateMouseAndMouseCursorPosition(on ? 0 : 1)
    }

    // MARK: - Pointer

    private func pointerAbs(_ e: NSEvent) {
        guard let s = sink else { return }
        if s.pointerLocked {
            let scale = Double(window?.backingScaleFactor ?? 2)
            let dx = Double(e.deltaX) * scale
            let dy = Double(e.deltaY) * scale
            s.send(.pointerRel(dx: dx, dy: dy, dxUnaccel: dx, dyUnaccel: dy), eventTime: e.timestamp)
            return
        }
        let p = convert(e.locationInWindow, from: nil)
        if let q = s.streamPoint(p) {
            s.send(.pointerAbs(x: q.x, y: q.y), eventTime: e.timestamp)
        }
    }

    private func button(_ e: NSEvent) -> RemoteCore.Button {
        switch e.buttonNumber {
        case 0: return .left
        case 1: return .right
        case 2: return .middle
        case 3: return .back
        case 4: return .forward
        default: return .other(UInt32(0x110 + e.buttonNumber))
        }
    }

    private func press(_ e: NSEvent, down: Bool) {
        if down {
            sink?.streamClicked()
            if window?.firstResponder !== self { window?.makeFirstResponder(self) }
        }
        pointerAbs(e)
        sink?.send(.button(button: button(e), down: down), eventTime: e.timestamp)
    }

    override func mouseMoved(with e: NSEvent) { pointerAbs(e) }
    override func mouseDragged(with e: NSEvent) { pointerAbs(e) }
    override func rightMouseDragged(with e: NSEvent) { pointerAbs(e) }
    override func otherMouseDragged(with e: NSEvent) { pointerAbs(e) }
    override func mouseDown(with e: NSEvent) { press(e, down: true) }
    override func mouseUp(with e: NSEvent) { press(e, down: false) }
    override func rightMouseDown(with e: NSEvent) { press(e, down: true) }
    override func rightMouseUp(with e: NSEvent) { press(e, down: false) }
    override func otherMouseDown(with e: NSEvent) { press(e, down: true) }
    override func otherMouseUp(with e: NSEvent) { press(e, down: false) }

    // MARK: - Scrolling (finger source with phases; wheels in value120)

    private func axis(_ source: AxisSource, dx: Double, dy: Double, v120x: Int32, v120y: Int32, t: Double) {
        guard let s = sink else { return }
        let both = dx != 0 && dy != 0
        if dy != 0 {
            s.send(.axis(source: source, axis: .vertical, value: dy, value120: v120y, stop: false, more: both), eventTime: t)
        }
        if dx != 0 {
            s.send(.axis(source: source, axis: .horizontal, value: dx, value120: v120x, stop: false, more: false), eventTime: t)
        }
    }

    /// The end of a trackpad scroll: a stop on each axis that moved.
    private func sendStop(_ t: Double?) {
        stopTimer?.invalidate()
        stopTimer = nil
        awaitingMomentum = false
        inMomentum = false
        guard let s = sink, scrollMovedX || scrollMovedY else { return }
        if scrollMovedY {
            s.send(.axis(source: .finger, axis: .vertical, value: 0, value120: 0, stop: true, more: scrollMovedX), eventTime: t)
        }
        if scrollMovedX {
            s.send(.axis(source: .finger, axis: .horizontal, value: 0, value120: 0, stop: true, more: false), eventTime: t)
        }
        scrollMovedX = false
        scrollMovedY = false
    }

    override func scrollWheel(with e: NSEvent) {
        guard sink != nil else { return }
        // Natural scrolling is already in the deltas; Wayland's positive is down/right.
        let dx = -Double(e.scrollingDeltaX)
        let dy = -Double(e.scrollingDeltaY)
        guard e.hasPreciseScrollingDeltas else {
            // A mouse wheel: 120 per notch, `value` from it (capture-ipc: value120 / 8).
            let vy = Int32((-Double(e.deltaY) * 120).rounded())
            let vx = Int32((-Double(e.deltaX) * 120).rounded())
            axis(.wheel, dx: Double(vx) / 8, dy: Double(vy) / 8, v120x: vx, v120y: vy, t: e.timestamp)
            return
        }
        cancelHold()
        if !e.momentumPhase.isEmpty {
            // macOS's momentum continues the finger scroll; one stop at its end, so
            // the host's toolkits add no kinetic of their own. If one
            // flings from the tail anyway, end the finger scroll when a momentum
            // event falls under 1 px and drop the rest; never send a stop between
            // `ended` and the momentum's `began`, which would fling twice.
            if e.momentumPhase.contains(.began) {
                stopTimer?.invalidate()
                stopTimer = nil
                awaitingMomentum = false
                inMomentum = true
            }
            if inMomentum {
                if dx != 0 { scrollMovedX = true }
                if dy != 0 { scrollMovedY = true }
                axis(.finger, dx: dx, dy: dy, v120x: 0, v120y: 0, t: e.timestamp)
            }
            if e.momentumPhase.contains(.ended) || e.momentumPhase.contains(.cancelled) {
                sendStop(e.timestamp)
            }
            return
        }
        if e.phase.contains(.began) || e.phase.contains(.mayBegin) {
            if awaitingMomentum || inMomentum { sendStop(e.timestamp) }
        }
        if e.phase.contains(.began) || e.phase.contains(.changed) {
            if dx != 0 { scrollMovedX = true }
            if dy != 0 { scrollMovedY = true }
            axis(.finger, dx: dx, dy: dy, v120x: 0, v120y: 0, t: e.timestamp)
        } else if e.phase.contains(.ended) {
            // Momentum may follow within a few ms; stop if it doesn't.
            awaitingMomentum = true
            stopTimer?.invalidate()
            stopTimer = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: false) { [weak self] _ in
                MainActor.assumeIsolated {
                    guard let self = self, self.awaitingMomentum else { return }
                    self.sendStop(nil)
                }
            }
        } else if e.phase.contains(.cancelled) {
            sendStop(e.timestamp)
        } else if e.phase.isEmpty {
            // A precise device with no phases (some mice): continuous.
            axis(.continuous, dx: dx, dy: dy, v120x: 0, v120y: 0, t: e.timestamp)
        }
    }

    // MARK: - Gestures (pointer-gestures v3)

    private func gesture(_ kind: GestureKind, _ phase: GesturePhase, fingers: UInt32, dx: Double = 0, dy: Double = 0, rotation: Double = 0, t: Double?) {
        sink?.send(.gesture(kind: kind, phase: phase, fingers: fingers, dx: dx, dy: dy, scale: pinchScale, rotation: rotation), eventTime: t)
    }

    override func magnify(with e: NSEvent) {
        cancelHold()
        if e.phase.contains(.began) {
            pinchScale = 1
            pinchActive = true
            gesture(.pinch, .begin, fingers: 2, t: e.timestamp)
        }
        if e.phase.contains(.changed) {
            if !pinchActive {
                pinchScale = 1
                pinchActive = true
                gesture(.pinch, .begin, fingers: 2, t: e.timestamp)
            }
            pinchScale *= 1 + Double(e.magnification)
            gesture(.pinch, .update, fingers: 2, t: e.timestamp)
        }
        if e.phase.contains(.ended) && pinchActive {
            pinchActive = false
            gesture(.pinch, .end, fingers: 2, t: e.timestamp)
        }
        if e.phase.contains(.cancelled) && pinchActive {
            pinchActive = false
            gesture(.pinch, .cancel, fingers: 2, t: e.timestamp)
        }
    }

    override func rotate(with e: NSEvent) {
        // macOS: degrees counter-clockwise; pointer-gestures: clockwise.
        guard pinchActive else { return }
        gesture(.pinch, .update, fingers: 2, rotation: -Double(e.rotation), t: e.timestamp)
    }

    private func touching(_ e: NSEvent) -> [NSTouch] {
        Array(e.touches(matching: .touching, in: self))
    }

    private func centroid(_ t: [NSTouch]) -> NSPoint {
        var x: CGFloat = 0
        var y: CGFloat = 0
        for k in t {
            x += k.normalizedPosition.x * k.deviceSize.width
            y += k.normalizedPosition.y * k.deviceSize.height
        }
        let n = CGFloat(max(t.count, 1))
        return NSPoint(x: x / n, y: y / n)
    }

    override func touchesBegan(with e: NSEvent) {
        let t = touching(e)
        if t.count >= 3 {
            cancelHold()
            if !swipeActive {
                swipeActive = true
                swipeFingers = t.count
                swipeLast = centroid(t)
                gesture(.swipe, .begin, fingers: UInt32(t.count), t: e.timestamp)
            }
        } else if t.count == 2 && !holdActive && !swipeActive && !pinchActive {
            holdActive = true
            gesture(.hold, .begin, fingers: 2, t: e.timestamp)
        }
    }

    override func touchesMoved(with e: NSEvent) {
        let t = touching(e)
        guard swipeActive, t.count >= 3, let last = swipeLast else { return }
        let c = centroid(t)
        // Trackpad points (about 1/72 in) → libinput's units. The target: one unit
        // is what libinput sends for the same finger travel on a touchpad at the
        // desk, so sodiumwm's `workspace_swipe_distance` (300) means the same from
        // both. 1.5 is the starting guess, PROVISIONAL until measured
        // on the Mac: a 3-finger swipe across about a third of the trackpad should
        // move one space. Set k to what gives that and write the measurement here.
        let k: CGFloat = 1.5
        let dx = Double((c.x - last.x) * k)
        let dy = Double(-(c.y - last.y) * k)
        swipeLast = c
        if dx != 0 || dy != 0 {
            gesture(.swipe, .update, fingers: UInt32(swipeFingers), dx: dx, dy: dy, t: e.timestamp)
        }
    }

    override func touchesEnded(with e: NSEvent) {
        let t = touching(e)
        if swipeActive && t.count < 3 {
            swipeActive = false
            swipeLast = nil
            gesture(.swipe, .end, fingers: UInt32(swipeFingers), t: e.timestamp)
        }
        if holdActive && t.isEmpty {
            holdActive = false
            gesture(.hold, .end, fingers: 2, t: e.timestamp)
        }
    }

    override func touchesCancelled(with e: NSEvent) {
        if swipeActive {
            swipeActive = false
            swipeLast = nil
            gesture(.swipe, .cancel, fingers: UInt32(swipeFingers), t: e.timestamp)
        }
        cancelHold()
    }

    /// A hold turns into something else (a scroll, a pinch, a swipe).
    private func cancelHold() {
        if holdActive {
            holdActive = false
            gesture(.hold, .cancel, fingers: 2, t: nil)
        }
    }

    // MARK: - Keys (HID usages exactly as pressed)

    private func sendKey(_ keyCode: UInt16, down: Bool, t: Double?) {
        guard let usage = KeyUsage.fromMacKeyCode[keyCode] else { return }
        if down {
            keysDown.insert(keyCode)
        } else {
            keysDown.remove(keyCode)
        }
        sink?.send(.key(usage: usage, down: down), eventTime: t)
    }

    /// A modifier held on the computer is about to be let go with no key in
    /// between: tap F24, which nothing binds, so a release-bound modifier (Win
    /// alone opens the launcher) doesn't fire.
    func cancelModifierTap() {
        guard !modifiersDown.isEmpty else { return }
        sink?.send(.key(usage: KeyUsage.f24, down: true), eventTime: nil)
        sink?.send(.key(usage: KeyUsage.f24, down: false), eventTime: nil)
    }

    /// Release everything held (focus lost, ⌃⌥⌘K, the window closing).
    func releaseAll() {
        for k in keysDown.union(modifiersDown) {
            if let usage = KeyUsage.fromMacKeyCode[k] { sink?.send(.key(usage: usage, down: false), eventTime: nil) }
        }
        keysDown.removeAll()
        modifiersDown.removeAll()
        sink?.send(.releaseAll, eventTime: nil)
        if hasMarkedText() {
            inputContext?.discardMarkedText()
            marked = NSMutableAttributedString()
        }
    }

    private var capturing: Bool {
        sink?.capturingKeys ?? false
    }

    override func keyDown(with e: NSEvent) {
        if sink?.remoteShortcut(e) == true { return }
        guard capturing else {
            super.keyDown(with: e)
            return
        }
        let flags = e.modifierFlags.rawValue
        let shortcut = flags & (KeyMods.flagCommand | KeyMods.flagControl) != 0
        let useIME = (sink?.macInputMethod ?? true) && !(sink?.passwordFocus ?? false) && !shortcut
        if e.isARepeat && !hasMarkedText() {
            // The computer repeats keys itself (capture-ipc: no client repeat).
            if keysDown.contains(e.keyCode) { return }
        }
        guard useIME, let ctx = inputContext else {
            if !e.isARepeat { sendKey(e.keyCode, down: true, t: e.timestamp) }
            return
        }
        currentKey = e
        markedBefore = hasMarkedText()
        plainKey = false
        consumed = false
        let handled = ctx.handleEvent(e)
        currentKey = nil
        if plainKey || (!handled && !hasMarkedText()) {
            if !e.isARepeat || !keysDown.contains(e.keyCode) { sendKey(e.keyCode, down: true, t: e.timestamp) }
        }
    }

    override func keyUp(with e: NSEvent) {
        if keysDown.contains(e.keyCode) {
            sendKey(e.keyCode, down: false, t: e.timestamp)
        } else if !capturing {
            super.keyUp(with: e)
        }
    }

    override func flagsChanged(with e: NSEvent) {
        guard capturing else {
            super.flagsChanged(with: e)
            return
        }
        let code = e.keyCode
        guard let usage = KeyUsage.fromMacKeyCode[code] else { return }
        let raw = e.modifierFlags.rawValue
        let down: Bool
        if code == 0x39 {
            // Caps Lock: macOS reports toggles, not presses: send a tap.
            sink?.send(.key(usage: usage, down: true), eventTime: e.timestamp)
            sink?.send(.key(usage: usage, down: false), eventTime: e.timestamp)
            return
        } else if code == 0x3F {
            down = raw & KeyMods.flagFunction != 0
        } else if let bit = KeyUsage.modifierDeviceBits[code] {
            down = raw & bit != 0
        } else {
            down = !modifiersDown.contains(code)
        }
        if down == modifiersDown.contains(code) { return }
        if down {
            modifiersDown.insert(code)
        } else {
            modifiersDown.remove(code)
        }
        sink?.send(.key(usage: usage, down: down), eventTime: e.timestamp)
    }

    /// Key equivalents (⌘ + a key) reach the stream before the menus
    ///, except ⌘Esc, ⌘⇧Esc and Remote's own ⌃⌥⌘ shortcuts.
    override func performKeyEquivalent(with e: NSEvent) -> Bool {
        guard window?.firstResponder === self, e.type == .keyDown else { return super.performKeyEquivalent(with: e) }
        if sink?.remoteShortcut(e) == true { return true }
        let f = e.modifierFlags.rawValue
        let remote = KeyMods.flagControl | KeyMods.flagOption | KeyMods.flagCommand
        if f & remote == remote {
            return super.performKeyEquivalent(with: e)
        }
        guard capturing else { return super.performKeyEquivalent(with: e) }
        keyDown(with: e)
        return true
    }

    /// AppKit drops the key-up of a key pressed with ⌘ held: catch it first.
    private func installKeyUpMonitor(_ on: Bool) {
        if let m = keyUpMonitor {
            NSEvent.removeMonitor(m)
            keyUpMonitor = nil
        }
        guard on else { return }
        keyUpMonitor = NSEvent.addLocalMonitorForEvents(matching: [.keyUp]) { [weak self] e in
            let code = e.keyCode
            let t = e.timestamp
            let handled: Bool = MainActor.assumeIsolated {
                guard let self = self, self.window?.firstResponder === self, self.keysDown.contains(code) else { return false }
                self.sendKey(code, down: false, t: t)
                return true
            }
            return handled ? nil : e
        }
    }

    /// Keys from the system-shortcut tap (⌘Tab, ⌘Space…), which never reach
    /// the window.
    func tappedKey(keyCode: UInt16, down: Bool, t: Double) {
        guard capturing else { return }
        if down {
            if !keysDown.contains(keyCode) { sendKey(keyCode, down: true, t: t) }
        } else if keysDown.contains(keyCode) {
            sendKey(keyCode, down: false, t: t)
        }
    }

    override func resignFirstResponder() -> Bool {
        releaseAll()
        sink?.focusLost()
        return super.resignFirstResponder()
    }

    // MARK: - Secure input (password fields)

    func setPasswordMode(_ on: Bool) {
        inputContext?.allowedInputSourceLocales = on ? [NSAllRomanInputSourcesLocaleIdentifier] : nil
        if on && hasMarkedText() {
            inputContext?.discardMarkedText()
            marked = NSMutableAttributedString()
        }
    }

    /// The computer ended the composition (`ImeReset`): discard, don't commit.
    func discardComposition() {
        guard hasMarkedText() else { return }
        marked = NSMutableAttributedString()
        inputContext?.discardMarkedText()
    }

    // MARK: - NSTextInputClient (ime-and-dnd.md's mapping)

    private func plainString(_ s: Any) -> String {
        if let a = s as? NSAttributedString { return a.string }
        if let t = s as? String { return t }
        return ""
    }

    func insertText(_ string: Any, replacementRange: NSRange) {
        let text = plainString(string)
        if let e = currentKey, !markedBefore, text == (e.characters ?? "") {
            // A plain key, not consumed by the input method: it goes as a key.
            plainKey = true
            return
        }
        consumed = true
        marked = NSMutableAttributedString()
        guard !text.isEmpty else { return }
        for ev in imeCommitChunks(text: text) {
            sink?.send(ev, eventTime: currentKey?.timestamp)
        }
    }

    override func doCommand(by selector: Selector) {
        // A key the input method didn't consume (Return, arrows, Backspace…).
        if currentKey != nil { plainKey = true }
    }

    func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {
        consumed = true
        if let a = string as? NSAttributedString {
            marked = NSMutableAttributedString(attributedString: a)
        } else {
            marked = NSMutableAttributedString(string: plainString(string))
        }
        markedSelection = selectedRange
        let text = marked.string
        // UTF-16 offset → characters (code points).
        let utf16 = Array(text.utf16)
        let upto = min(max(selectedRange.location, 0), utf16.count)
        let prefix = String(decoding: utf16[0..<upto], as: UTF16.self)
        let cursor = Int32(prefix.unicodeScalars.count)
        sink?.send(.imePreedit(text: text, cursor: text.isEmpty ? -1 : cursor), eventTime: currentKey?.timestamp)
    }

    func unmarkText() {
        let text = marked.string
        marked = NSMutableAttributedString()
        guard !text.isEmpty else { return }
        for ev in imeCommitChunks(text: text) {
            sink?.send(ev, eventTime: nil)
        }
    }

    func selectedRange() -> NSRange {
        NSRange(location: NSNotFound, length: 0)
    }

    func markedRange() -> NSRange {
        marked.length > 0 ? NSRange(location: 0, length: marked.length) : NSRange(location: NSNotFound, length: 0)
    }

    func hasMarkedText() -> Bool {
        marked.length > 0
    }

    func attributedSubstring(forProposedRange range: NSRange, actualRange: NSRangePointer?) -> NSAttributedString? {
        nil
    }

    func validAttributesForMarkedText() -> [NSAttributedString.Key] {
        [.underlineStyle]
    }

    func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
        guard let w = window else { return .zero }
        let r = sink?.caretInView() ?? NSRect(x: bounds.midX, y: bounds.midY, width: 0, height: 20)
        return w.convertToScreen(convert(r, to: nil))
    }

    func characterIndex(for point: NSPoint) -> Int {
        NSNotFound
    }
}
