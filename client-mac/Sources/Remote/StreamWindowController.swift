// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import Combine
import RemoteKit
import RemoteUI

/// A layer filled with a (dynamic) colour, redrawn when the appearance changes.
final class TintView: NSView {
    var color: NSColor = .clear {
        didSet { needsDisplay = true }
    }

    override var wantsUpdateLayer: Bool { true }

    override func updateLayer() {
        layer?.backgroundColor = color.cgColor
    }
}

/// What the stream window tells its session.
@MainActor
protocol StreamWindowOwner: AnyObject {
    func windowWillClose()
    func windowFocusChanged(key: Bool)
    func fullScreenChanged(_ on: Bool)
}

/// The stream window: macOS's own window buttons
/// in our glass bar (a unified toolbar keeps them centred, so the bar has the
/// unified toolbar's height: 52 pt on macOS 14 and 15; the design's
/// 48 is a drawing), the stream below it, overlays on top. 1280 × 830 pt by
/// default, 640 × 400 at the least.
@MainActor
final class StreamWindowController: NSWindowController, NSWindowDelegate, NSToolbarDelegate {
    let model: StreamModel
    let streamView = StreamView(frame: NSRect(x: 0, y: 0, width: 1280, height: 782))
    weak var streamOwner: StreamWindowOwner?

    private let container = NSView()
    private var toast: NSView!
    private var pill: NSView!
    private var cards: NSView!
    private var hudPanel: NSView!
    private var hudPill: NSView!
    private var reveal: NSView!
    private var badge: NSView!
    private var hudPanelWindowed: [NSLayoutConstraint] = []
    private var hudPanelFull: [NSLayoutConstraint] = []
    private var bar: [NSView] = []
    private var streamTopWindowed: NSLayoutConstraint!
    private var streamTopFull: NSLayoutConstraint!
    private var subs: Set<AnyCancellable> = []
    private var revealTimer: Timer?
    private var revealLeftAt: Double?
    private var toastTimer: Timer?
    private static let titleItem = NSToolbarItem.Identifier(AppIdentity.label("title"))
    private static let buttonsItem = NSToolbarItem.Identifier(AppIdentity.label("buttons"))
    private static let displayItem = NSToolbarItem.Identifier(AppIdentity.label("display"))

    init(model: StreamModel) {
        self.model = model
        let w = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1280, height: 830),
                         styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
                         backing: .buffered, defer: false)
        w.titleVisibility = .hidden
        w.titlebarAppearsTransparent = true
        w.titlebarSeparatorStyle = .none
        w.isMovableByWindowBackground = false
        w.collectionBehavior = [.fullScreenPrimary, .managed]
        w.contentMinSize = NSSize(width: 640, height: 400)
        w.backgroundColor = Theme.letterboxNS
        w.isReleasedWhenClosed = false
        w.tabbingMode = .disallowed
        super.init(window: w)
        w.delegate = self
        let tb = NSToolbar(identifier: NSToolbar.Identifier(AppIdentity.label("stream")))
        tb.delegate = self
        tb.displayMode = .iconOnly
        tb.allowsUserCustomization = false
        w.toolbar = tb
        w.toolbarStyle = .unified
        build(w)
        w.setFrameAutosaveName("RemoteStreamWindow")
        if !w.setFrameUsingName("RemoteStreamWindow") { w.center() }
        observe()
    }

    required init?(coder: NSCoder) {
        fatalError("not used")
    }

    // MARK: - Views

    private func build(_ w: NSWindow) {
        container.wantsLayer = true
        w.contentView = container

        // The glass bar behind the toolbar.
        let blur = NSVisualEffectView()
        blur.material = .titlebar
        blur.blendingMode = .behindWindow
        blur.state = .followsWindowActiveState
        let tint = TintView()
        tint.wantsLayer = true
        tint.color = Theme.winGlassNS
        for v in [blur, tint, streamView] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            container.addSubview(v)
        }
        bar = [blur, tint]
        let guide = w.contentLayoutGuide as? NSLayoutGuide
        let top = guide?.topAnchor ?? container.topAnchor
        // Under the bar in a window; the whole screen in full screen, where
        // the bar is hidden (`setFullScreenLayout`).
        streamTopWindowed = streamView.topAnchor.constraint(equalTo: top)
        streamTopFull = streamView.topAnchor.constraint(equalTo: container.topAnchor)
        NSLayoutConstraint.activate([
            streamTopWindowed,
            blur.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            blur.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            blur.topAnchor.constraint(equalTo: container.topAnchor),
            blur.bottomAnchor.constraint(equalTo: top),
            tint.leadingAnchor.constraint(equalTo: blur.leadingAnchor),
            tint.trailingAnchor.constraint(equalTo: blur.trailingAnchor),
            tint.topAnchor.constraint(equalTo: blur.topAnchor),
            tint.bottomAnchor.constraint(equalTo: blur.bottomAnchor),
            streamView.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            streamView.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            streamView.bottomAnchor.constraint(equalTo: container.bottomAnchor),
        ])

        toast = StreamUI.makeToast(model: model)
        pill = StreamUI.makePill(model: model)
        cards = StreamUI.makeCards(model: model)
        hudPanel = StreamUI.makeHudPanel(model: model)
        hudPill = StreamUI.makeHudPill(model: model)
        reveal = StreamUI.makeRevealMenu(model: model)
        badge = StreamUI.makeResizeBadge(model: model)
        for v in [cards, pill, toast, hudPanel, hudPill, badge, reveal] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            v.isHidden = true
            container.addSubview(v)
        }
        let s = streamView
        NSLayoutConstraint.activate([
            cards.leadingAnchor.constraint(equalTo: s.leadingAnchor),
            cards.trailingAnchor.constraint(equalTo: s.trailingAnchor),
            cards.topAnchor.constraint(equalTo: s.topAnchor),
            cards.bottomAnchor.constraint(equalTo: s.bottomAnchor),
            // Toasts: top centre, under the computer's 32 pt bar.
            toast.centerXAnchor.constraint(equalTo: s.centerXAnchor),
            toast.topAnchor.constraint(equalTo: s.topAnchor, constant: 44),
            pill.centerXAnchor.constraint(equalTo: s.centerXAnchor),
            pill.centerYAnchor.constraint(equalTo: s.centerYAnchor),
            badge.centerXAnchor.constraint(equalTo: s.centerXAnchor),
            badge.centerYAnchor.constraint(equalTo: s.centerYAnchor),
            hudPill.trailingAnchor.constraint(equalTo: s.trailingAnchor, constant: -12),
            hudPill.bottomAnchor.constraint(equalTo: s.bottomAnchor, constant: -12),
            reveal.centerXAnchor.constraint(equalTo: s.centerXAnchor),
            reveal.topAnchor.constraint(equalTo: s.topAnchor, constant: 8),
        ])
        // The expanded stats: under the chip in a window, up from the pill in full screen.
        hudPanelWindowed = [
            hudPanel.leadingAnchor.constraint(equalTo: s.leadingAnchor, constant: 80),
            hudPanel.topAnchor.constraint(equalTo: s.topAnchor, constant: 6),
        ]
        hudPanelFull = [
            hudPanel.trailingAnchor.constraint(equalTo: s.trailingAnchor, constant: -12),
            hudPanel.bottomAnchor.constraint(equalTo: s.bottomAnchor, constant: -48),
        ]
        NSLayoutConstraint.activate(hudPanelWindowed)
    }

    /// Show each overlay only while it has something to show.
    private func observe() {
        model.$toast.sink { [weak self] t in
            guard let self = self else { return }
            self.toast.isHidden = t == nil
            self.toastTimer?.invalidate()
            if let d = t?.duration {
                let id = t?.id
                self.toastTimer = Timer.scheduledTimer(withTimeInterval: d, repeats: false) { [weak self] _ in
                    MainActor.assumeIsolated {
                        if let s = self, s.model.toast?.id == id { s.model.toast = nil }
                    }
                }
            }
        }.store(in: &subs)
        model.$pill.sink { [weak self] p in self?.pill.isHidden = p == nil }.store(in: &subs)
        model.$card.sink { [weak self] c in
            self?.cards.isHidden = c == nil
            if c != nil { self?.model.revealMenu = false }
        }.store(in: &subs)
        model.$resizeBadge.sink { [weak self] b in self?.badge.isHidden = b == nil }.store(in: &subs)
        // The display picker joins the bar only for a Physical host with several displays.
        model.$displays.combineLatest(model.$hostVirtual).sink { [weak self] displays, virtual in
            self?.setDisplayPicker(shown: DisplayPicker.isShown(displays: displays.count, virtual: virtual))
        }.store(in: &subs)
        model.$revealMenu.sink { [weak self] r in self?.reveal.isHidden = !r }.store(in: &subs)
        model.$hudMode.combineLatest(model.$fullScreen).sink { [weak self] mode, full in
            guard let self = self else { return }
            self.hudPanel.isHidden = mode != .expanded
            self.hudPill.isHidden = !(full && mode == .compact)
            NSLayoutConstraint.deactivate(full ? self.hudPanelWindowed : self.hudPanelFull)
            NSLayoutConstraint.activate(full ? self.hudPanelFull : self.hudPanelWindowed)
        }.store(in: &subs)
    }

    private func setDisplayPicker(shown: Bool) {
        guard let tb = window?.toolbar else { return }
        let at = tb.items.firstIndex { $0.itemIdentifier == StreamWindowController.displayItem }
        if shown && at == nil {
            tb.insertItem(withItemIdentifier: StreamWindowController.displayItem, at: max(0, tb.items.count - 1))
        } else if !shown, let i = at {
            tb.removeItem(at: i)
        }
    }

    /// Resize in steps of the scale's pixel period, in points.
    func setResizeIncrement(points: Double) {
        guard let w = window else { return }
        let inc = max(1, points)
        w.contentResizeIncrements = NSSize(width: inc, height: inc)
    }

    func toggleFullScreen() {
        window?.toggleFullScreen(nil)
    }

    var isFullScreen: Bool {
        window?.styleMask.contains(.fullScreen) ?? false
    }

    // MARK: - NSToolbarDelegate

    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [StreamWindowController.titleItem, .flexibleSpace, StreamWindowController.displayItem, StreamWindowController.buttonsItem]
    }

    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [StreamWindowController.titleItem, .flexibleSpace, StreamWindowController.buttonsItem]
    }

    func toolbar(_ toolbar: NSToolbar, itemForItemIdentifier id: NSToolbarItem.Identifier, willBeInsertedIntoToolbar flag: Bool) -> NSToolbarItem? {
        let item = NSToolbarItem(itemIdentifier: id)
        if id == StreamWindowController.titleItem {
            item.view = StreamUI.makeTitleItem(model: model)
            item.label = String(localized: "Computer")
        } else if id == StreamWindowController.displayItem {
            item.view = StreamUI.makeDisplayItem(model: model)
            item.label = String(localized: "Display")
        } else if id == StreamWindowController.buttonsItem {
            item.view = StreamUI.makeButtonsItem(model: model)
            item.label = String(localized: "Controls")
        } else {
            return nil
        }
        return item
    }

    // MARK: - NSWindowDelegate

    func windowWillClose(_ notification: Notification) {
        revealTimer?.invalidate()
        streamOwner?.windowWillClose()
    }

    func windowDidBecomeKey(_ notification: Notification) {
        model.active = true
        streamOwner?.windowFocusChanged(key: true)
    }

    func windowDidResignKey(_ notification: Notification) {
        model.active = false
        streamOwner?.windowFocusChanged(key: false)
    }

    func windowWillStartLiveResize(_ notification: Notification) {
        updateBadge()
    }

    func windowDidResize(_ notification: Notification) {
        if window?.inLiveResize == true { updateBadge() }
    }

    func windowDidEndLiveResize(_ notification: Notification) {
        model.resizeBadge = nil
    }

    private func updateBadge() {
        let s = streamView.bounds.size
        model.resizeBadge = "\(Int(s.width.rounded())) × \(Int(s.height.rounded())) pt"
    }

    /// Full screen is the stream alone: no toolbar or glass bar (macOS still
    /// slides its menu bar down at the top edge, and our menu with it).
    private func setFullScreenLayout(_ on: Bool) {
        window?.toolbar?.isVisible = !on
        for v in bar { v.isHidden = on }
        if on {
            streamTopWindowed.isActive = false
            streamTopFull.isActive = true
        } else {
            streamTopFull.isActive = false
            streamTopWindowed.isActive = true
        }
    }

    func windowWillEnterFullScreen(_ notification: Notification) {
        setFullScreenLayout(true)
    }

    func windowDidFailToEnterFullScreen(_ window: NSWindow) {
        setFullScreenLayout(false)
    }

    func windowWillExitFullScreen(_ notification: Notification) {
        setFullScreenLayout(false)
    }

    func windowDidFailToExitFullScreen(_ window: NSWindow) {
        setFullScreenLayout(true)
    }

    func windowDidEnterFullScreen(_ notification: Notification) {
        model.fullScreen = true
        streamOwner?.fullScreenChanged(true)
        startRevealWatch()
    }

    func windowDidExitFullScreen(_ notification: Notification) {
        model.fullScreen = false
        model.revealMenu = false
        revealTimer?.invalidate()
        revealTimer = nil
        streamOwner?.fullScreenChanged(false)
    }

    // MARK: - The top-edge menu in full screen

    /// Pointing into the black row above the stream (the camera housing's,
    /// or the screen's top edge) shows our menu under macOS's menu bar; it
    /// leaves a second after the pointer does.
    private func startRevealWatch() {
        revealTimer?.invalidate()
        revealTimer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.checkReveal() }
        }
    }

    private func checkReveal() {
        guard let w = window, isFullScreen, model.card == nil else { return }
        let p = NSEvent.mouseLocation
        let top = w.frame.maxY
        if p.y >= top - 1 {
            model.revealMenu = true
            revealLeftAt = nil
            return
        }
        guard model.revealMenu else { return }
        // Inside the menu's band (the menu bar, 8 pt, 46 pt of menu, a margin).
        if p.y >= top - 70 {
            revealLeftAt = nil
            return
        }
        let now = hostNow()
        if let t = revealLeftAt {
            if now - t >= 1 {
                model.revealMenu = false
                revealLeftAt = nil
            }
        } else {
            revealLeftAt = now
        }
    }
}
