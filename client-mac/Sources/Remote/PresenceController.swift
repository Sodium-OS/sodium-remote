// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import Foundation
import RemoteKit
import RemoteUI
import UserNotifications

/// The menu-bar extra and the "being controlled" notification: the
/// icon is hidden while "Allow remote control" is off
/// and shown whenever it is on, solid, or accent with a dot while someone is connected; the notification on connect
/// carries Disconnect (and Dismiss); if notifications are denied the board's own panel is shown instead.
@MainActor
final class PresenceController: NSObject, NSMenuDelegate {
    private var item: NSStatusItem?
    private let menu = NSMenu()
    private var allowed = false
    private var controller: ControllerModel?
    private var startAtLogin = false
    private let notes = ControlledNotifications()
    private var panel: NSPanel?
    private var panelTimer: Timer?

    // What the menu asks the app to do.
    var onOpen: (() -> Void)?
    var onOpenThisComputer: (() -> Void)?
    var onToggleAllowed: ((Bool) -> Void)?
    var onPair: (() -> Void)?
    var onToggleLogin: ((Bool) -> Void)?
    var onDisconnect: (() -> Void)?
    var onQuit: (() -> Void)?

    func start() {
        menu.delegate = self
        menu.autoenablesItems = false
        notes.onDisconnect = { [weak self] in self?.onDisconnect?() }
        notes.setUp()
        refreshIcon()
    }

    func update(allowed: Bool, controller: ControllerModel?, startAtLogin: Bool) {
        self.allowed = allowed
        self.controller = controller
        self.startAtLogin = startAtLogin
        if controller == nil { closePanel() }
        refreshIcon()
    }

    private var state: PresenceState { PresenceState.of(allowed: allowed, controlled: controller != nil) }

    /// Shows the status item while the switch is on and removes it while it is off.
    private func refreshIcon() {
        if state == .off {
            if let i = item {
                NSStatusBar.system.removeStatusItem(i)
                item = nil
            }
            return
        }
        if item == nil {
            let i = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
            i.menu = menu
            item = i
        }
        guard let b = item?.button else { return }
        b.image = PresenceIcon.image(state)
        b.setAccessibilityLabel("Sodium Remote")
        if let c = controller {
            b.toolTip = String(localized: "Sodium Remote: \(c.name) is controlling this computer")
        } else {
            b.toolTip = "Sodium Remote"
        }
    }

    // MARK: - Notification

    /// Notification permission is asked the first time the switch is turned on.
    func askForNotificationsOnce() {
        notes.requestAuthorizationOnce()
    }

    /// Someone connected: a notification with Disconnect, or the panel when notifications are denied.
    func notifyControlled(name: String, since: Date) {
        let title = String(localized: "\(name) is controlling this computer")
        let time = since.formatted(date: .omitted, time: .shortened)
        let body = String(localized: "Connected at \(time). You can still use the keyboard and mouse.")
        notes.post(title: title, body: body) { [weak self] shown in
            if !shown { self?.showPanel(title: title, body: body) }
        }
    }

    private func showPanel(title: String, body: String) {
        closePanel()
        let v = PresenceUI.makeBanner(title: title, detail: body, onDisconnect: { [weak self] in
            self?.closePanel()
            self?.onDisconnect?()
        }, onDismiss: { [weak self] in
            self?.closePanel()
        })
        let size = v.frame.size
        let p = NSPanel(contentRect: NSRect(origin: .zero, size: size), styleMask: [.borderless, .nonactivatingPanel],
                        backing: .buffered, defer: false)
        p.isOpaque = false
        p.backgroundColor = .clear
        p.hasShadow = false
        p.level = .floating
        p.isReleasedWhenClosed = false
        p.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        p.contentView = v
        if let f = NSScreen.main?.visibleFrame {
            p.setFrameOrigin(NSPoint(x: f.maxX - size.width - 12, y: f.maxY - size.height - 8))
        }
        p.orderFrontRegardless()
        panel = p
        panelTimer = Timer.scheduledTimer(withTimeInterval: 10, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.closePanel() }
        }
    }

    private func closePanel() {
        panelTimer?.invalidate()
        panelTimer = nil
        panel?.orderOut(nil)
        panel = nil
    }

    // MARK: - The menu (rebuilt each time it opens, so it is always current)

    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        let head = NSMenuItem(title: "Sodium Remote", action: nil, keyEquivalent: "")
        head.isEnabled = false
        head.attributedTitle = NSAttributedString(string: "Sodium Remote", attributes: [.font: NSFont.menuFont(ofSize: 0).withWeight(.semibold)])
        menu.addItem(head)

        let statusText: String
        switch state {
        case .off: statusText = String(localized: "Remote control is off")
        case .on: statusText = String(localized: "Remote control is on")
        case .controlled: statusText = String(localized: "\(controller?.name ?? "") is connected")  // i18n:ok
        }
        let status = NSMenuItem(title: statusText, action: nil, keyEquivalent: "")
        status.isEnabled = false
        status.image = Self.dot(state)
        menu.addItem(status)

        if let c = controller {
            let since = NSMenuItem(title: String(localized: "Since \(c.since.formatted(date: .omitted, time: .shortened))"), action: nil, keyEquivalent: "")
            since.isEnabled = false
            menu.addItem(since)
            let d = NSMenuItem(title: String(localized: "Disconnect"), action: #selector(disconnect), keyEquivalent: "")
            d.target = self
            d.attributedTitle = NSAttributedString(string: d.title, attributes: [.foregroundColor: NSColor.systemRed])
            menu.addItem(d)
        }
        menu.addItem(.separator())
        menu.addItem(entry(String(localized: "Open Sodium Remote"), #selector(openApp)))
        menu.addItem(entry(String(localized: "Settings…"), #selector(openThisComputer)))
        menu.addItem(.separator())
        let allow = entry(String(localized: "Allow remote control"), #selector(toggleAllowed))
        allow.state = allowed ? .on : .off
        menu.addItem(allow)
        let pair = entry(String(localized: "Pair a device…"), #selector(pairDevice))
        pair.isEnabled = allowed
        menu.addItem(pair)
        let login = entry(String(localized: "Start at login"), #selector(toggleLogin))
        login.state = startAtLogin ? .on : .off
        menu.addItem(login)
        menu.addItem(.separator())
        menu.addItem(entry(String(localized: "Quit Sodium Remote"), #selector(quit)))
    }

    private func entry(_ title: String, _ action: Selector) -> NSMenuItem {
        let i = NSMenuItem(title: title, action: action, keyEquivalent: "")
        i.target = self
        return i
    }

    /// The state dot beside the status line: dim, green or accent.
    private static func dot(_ s: PresenceState) -> NSImage {
        let color: NSColor = s == .off ? .tertiaryLabelColor : (s == .on ? .systemGreen : NSColor(srgbRed: 0.604, green: 0.659, blue: 1, alpha: 1))
        let img = NSImage(size: NSSize(width: 10, height: 10), flipped: false) { r in
            color.setFill()
            NSBezierPath(ovalIn: r.insetBy(dx: 2, dy: 2)).fill()
            return true
        }
        return img
    }

    @objc private func disconnect() { onDisconnect?() }
    @objc private func openApp() { onOpen?() }
    @objc private func openThisComputer() { onOpenThisComputer?() }
    @objc private func toggleAllowed() { onToggleAllowed?(!allowed) }
    @objc private func pairDevice() { onPair?() }
    @objc private func toggleLogin() { onToggleLogin?(!startAtLogin) }
    @objc private func quit() { onQuit?() }
}

private extension NSFont {
    func withWeight(_ w: NSFont.Weight) -> NSFont {
        NSFont.systemFont(ofSize: pointSize, weight: w)
    }
}

/// UNUserNotificationCenter for "being controlled": a banner with Disconnect and Dismiss, time-sensitive when the build may.
final class ControlledNotifications: NSObject, UNUserNotificationCenterDelegate, @unchecked Sendable {
    private static let category = AppIdentity.label("controlled")
    private static let disconnectAction = "disconnect"
    private static let dismissAction = "dismiss"
    private static let askedKey = "RemoteNotificationsAsked"
    /// Called on main.
    var onDisconnect: (@MainActor () -> Void)?

    private var center: UNUserNotificationCenter { UNUserNotificationCenter.current() }

    func setUp() {
        center.delegate = self
        let disconnect = UNNotificationAction(identifier: Self.disconnectAction, title: String(localized: "Disconnect"), options: [.destructive])
        let dismiss = UNNotificationAction(identifier: Self.dismissAction, title: String(localized: "Dismiss"), options: [])
        let cat = UNNotificationCategory(identifier: Self.category, actions: [disconnect, dismiss], intentIdentifiers: [], options: [])
        center.setNotificationCategories([cat])
    }

    func requestAuthorizationOnce() {
        guard !UserDefaults.standard.bool(forKey: Self.askedKey) else { return }
        UserDefaults.standard.set(true, forKey: Self.askedKey)
        center.requestAuthorization(options: [.alert, .sound]) { granted, error in
            if let error = error { Log.error("notification permission: \(error)") }
            Log.info("notifications \(granted ? "allowed" : "not allowed")")
        }
    }

    /// `done(true)` when a notification was handed to macOS, `done(false)` when it can't be shown (denied).
    func post(title: String, body: String, done: @escaping @MainActor (Bool) -> Void) {
        let center = self.center
        center.getNotificationSettings { settings in
            let ok = settings.authorizationStatus == .authorized || settings.authorizationStatus == .provisional
            guard ok else {
                onMain { done(false) }
                return
            }
            let c = UNMutableNotificationContent()
            c.title = title
            c.body = body
            c.categoryIdentifier = Self.category
            // Time-sensitive only where the build can use it (it needs an entitlement a self-signed test build can't
            // carry: the setting then isn't .enabled); otherwise the normal level.
            c.interruptionLevel = settings.timeSensitiveSetting == .enabled ? .timeSensitive : .active
            let req = UNNotificationRequest(identifier: AppIdentity.label("controlled.\(UUID().uuidString)"), content: c, trigger: nil)
            center.add(req) { error in
                if let error = error { Log.error("notification: \(error)") }
                onMain { done(error == nil) }
            }
        }
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        completionHandler([.banner, .list, .sound])
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                withCompletionHandler completionHandler: @escaping () -> Void) {
        if response.actionIdentifier == Self.disconnectAction {
            onMain { [weak self] in self?.onDisconnect?() }
        }
        completionHandler()
    }
}
