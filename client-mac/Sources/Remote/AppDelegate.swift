// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import RemoteKit
import RemoteUI

/// The app delegate and the macOS menu bar (seven menus; Remote's
/// own shortcuts are ⌃⌥⌘ + a key). The stream view takes key
/// equivalents first, so Edit never takes ⌘C from the computer.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuItemValidation {
    let app = AppController()
    private let remoteMods: NSEvent.ModifierFlags = [.control, .option, .command]

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSEvent.isMouseCoalescingEnabled = false
        NSApp.mainMenu = buildMenu()
        if let icon = AppIcon.image(size: 512) { NSApp.applicationIconImage = icon }
        app.start()
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { showComputers(nil) }
        return true
    }

    /// `sodium-remote://server?…` (Info.plist's CFBundleURLTypes): a link from Safari, Mail or `open`. Settings › Server
    /// opens on its confirmation; nothing is saved before the person says yes.
    func application(_ application: NSApplication, open urls: [URL]) {
        guard let link = urls.first(where: { $0.scheme?.lowercased() == Brand.linkScheme }) else { return }
        app.openServerLink(link.absoluteString)
    }

    func applicationWillTerminate(_ notification: Notification) {
        app.shutdown()
    }

    // MARK: - Building the menus

    private func item(_ title: LocalizedStringResource, _ action: Selector?, _ key: String = "", mods: NSEvent.ModifierFlags = [.command], tag: Int = 0,
                      target: AnyObject? = nil) -> NSMenuItem {
        let i = NSMenuItem(title: String(localized: title), action: action, keyEquivalent: key)
        i.keyEquivalentModifierMask = mods
        i.tag = tag
        i.target = target ?? (action == nil ? nil : self)
        return i
    }

    /// A standard item, sent down the responder chain.
    private func std(_ title: LocalizedStringResource, _ action: Selector, _ key: String = "", mods: NSEvent.ModifierFlags = [.command]) -> NSMenuItem {
        let i = NSMenuItem(title: String(localized: title), action: action, keyEquivalent: key)
        i.keyEquivalentModifierMask = mods
        return i
    }

    private func submenu(_ title: LocalizedStringResource, _ items: [NSMenuItem]) -> NSMenuItem {
        let text = String(localized: title)
        let i = NSMenuItem(title: text, action: nil, keyEquivalent: "")
        let m = NSMenu(title: text)
        for x in items { m.addItem(x) }
        i.submenu = m
        return i
    }

    private func top(_ title: LocalizedStringResource, _ items: [NSMenuItem]) -> (NSMenuItem, NSMenu) {
        let text = String(localized: title)
        let i = NSMenuItem(title: text, action: nil, keyEquivalent: "")
        let m = NSMenu(title: text)
        for x in items { m.addItem(x) }
        i.submenu = m
        return (i, m)
    }

    private func buildMenu() -> NSMenu {
        let main = NSMenu(title: "Main")  // i18n:ok not shown
        let sep = { NSMenuItem.separator() }

        let remote = top("Sodium Remote", [
            item("About Sodium Remote", #selector(showAbout(_:)), mods: []),
            sep(),
            item("Settings…", #selector(showSettings(_:)), ","),
            sep(),
            std("Hide Sodium Remote", #selector(NSApplication.hide(_:)), "h"),
            std("Hide Others", #selector(NSApplication.hideOtherApplications(_:)), "h", mods: [.command, .option]),
            std("Show All", #selector(NSApplication.unhideAllApplications(_:))),
            sep(),
            std("Quit Sodium Remote", #selector(NSApplication.terminate(_:)), "q"),
        ])
        main.addItem(remote.0)

        let homeItems = [item("Mirror", #selector(homeMonitor(_:)), mods: [], tag: 0),
                         item("Privacy Screen", #selector(homeMonitor(_:)), mods: [], tag: 1),
                         item("Off", #selector(homeMonitor(_:)), mods: [], tag: 2)]
        let sendKeys = [item("Win", #selector(sendKeys(_:)), mods: [], tag: 0),
                        item("Win + L (lock)", #selector(sendKeys(_:)), mods: [], tag: 1),
                        item("Alt + Tab", #selector(sendKeys(_:)), mods: [], tag: 2),
                        item("Alt + F4", #selector(sendKeys(_:)), mods: [], tag: 3),
                        item("Print Screen", #selector(sendKeys(_:)), mods: [], tag: 4),
                        sep(),
                        item("Ctrl + Alt + F2 (console)", #selector(sendKeys(_:)), mods: [], tag: 5)]
        let sendFiles = item("Send Files…", nil)
        sendFiles.isEnabled = false
        let mic = item("Microphone", nil, "u", mods: remoteMods)
        mic.isEnabled = false
        let connection = top("Connection", [
            item("Computers", #selector(showComputers(_:)), "0"),
            item("Pair a Computer…", #selector(pairComputer(_:))),
            sep(),
            item("Disconnect", #selector(disconnect(_:)), "d", mods: remoteMods),
            item("Reconnect", #selector(reconnect(_:))),
            sep(),
            submenu("Home Monitor", homeItems),
            item("Lock", #selector(lock(_:)), "l", mods: remoteMods),
            submenu("Send Keys", sendKeys),
            sendFiles,
            sep(),
            item("Play Sound Here", #selector(togglePlaySound(_:))),
            mic,
        ])
        connection.1.autoenablesItems = true
        main.addItem(connection.0)

        let edit = top("Edit", [
            std("Undo", Selector(("undo:")), "z"),
            std("Redo", Selector(("redo:")), "z", mods: [.command, .shift]),
            sep(),
            std("Cut", #selector(NSText.cut(_:)), "x"),
            std("Copy", #selector(NSText.copy(_:)), "c"),
            std("Paste", #selector(NSText.paste(_:)), "v"),
            std("Select All", #selector(NSText.selectAll(_:)), "a"),
            sep(),
            std("Emoji & Symbols", #selector(NSApplication.orderFrontCharacterPalette(_:)), " ", mods: [.command, .control]),
        ])
        main.addItem(edit.0)

        let stats = [item("Off", #selector(statsMode(_:)), mods: [], tag: 0),
                     item("Compact", #selector(statsMode(_:)), "s", mods: remoteMods, tag: 1),
                     item("Expanded", #selector(statsMode(_:)), mods: [], tag: 2)]
        let resolution = [item("Match the Window", #selector(resolution(_:)), mods: [], tag: 0),
                          item("Match the Display", #selector(resolution(_:)), mods: [], tag: 1),
                          sep(),
                          item("1280 × 800", #selector(resolution(_:)), mods: [], tag: 10),
                          item("1680 × 1050", #selector(resolution(_:)), mods: [], tag: 11),
                          item("1920 × 1200", #selector(resolution(_:)), mods: [], tag: 12),
                          item("2560 × 1600", #selector(resolution(_:)), mods: [], tag: 13),
                          sep(),
                          item("Custom…", #selector(showSettings(_:)), mods: [])]
        let refresh = [30, 60, 90, 120, 144].map { hz in item("\(hz) Hz", #selector(refreshRate(_:)), mods: [], tag: hz) }
        let uiSize = [item("Match Mac", #selector(uiSize(_:)), mods: [], tag: 0),
                      item("Larger", #selector(uiSize(_:)), mods: [], tag: 1),
                      item("Smaller", #selector(uiSize(_:)), mods: [], tag: 2),
                      item("Same as Desk", #selector(uiSize(_:)), mods: [], tag: 3)]
        let look = [item("On", #selector(optimisedLook(_:)), mods: [], tag: 1),
                    item("Off", #selector(optimisedLook(_:)), mods: [], tag: 0)]
        let view = top("View", [
            item("Enter Full Screen", #selector(fullScreen(_:)), "f", mods: remoteMods),
            item("In-Stream Menu", #selector(inStreamMenu(_:)), "m", mods: remoteMods),
            sep(),
            submenu("Stats", stats),
            sep(),
            submenu("Resolution", resolution),
            submenu("Refresh Rate", refresh),
            submenu("UI Size", uiSize),
            item("Fixed Resolution", #selector(fixedResolution(_:))),
            sep(),
            submenu("Remote-Optimized Look", look),
        ])
        main.addItem(view.0)

        let input = top("Input", [
            item("Mac Keys", #selector(keyboard(_:)), mods: [], tag: 0),
            item("PC Keys", #selector(keyboard(_:)), mods: [], tag: 1),
            sep(),
            item("Use Mac Input Method", #selector(toggleInputMethod(_:))),
            item("Send System Shortcuts", #selector(toggleSystemShortcuts(_:))),
            item("Release Keys", #selector(releaseKeys(_:)), "k", mods: remoteMods),
            sep(),
            item("Edit Key Table…", #selector(showSettings(_:))),
        ])
        main.addItem(input.0)

        let window = top("Window", [
            std("Minimize", #selector(NSWindow.performMiniaturize(_:)), "m"),
            std("Zoom", #selector(NSWindow.performZoom(_:))),
            sep(),
            item("Computers", #selector(showComputers(_:)), "0"),
            sep(),
            std("Bring All to Front", #selector(NSApplication.arrangeInFront(_:))),
        ])
        main.addItem(window.0)
        NSApp.windowsMenu = window.1

        let help = top("Help", [
            item("Sodium Remote Help", #selector(remoteHelp(_:))),
            item("Keyboard Shortcuts", #selector(remoteHelp(_:))),
            sep(),
            item("Run the Latency Test…", #selector(runLatencyTest(_:))),
            item("Show Stats Files", #selector(showStatsFiles(_:))),
            item("Copy Diagnostics", #selector(copyDiagnostics(_:))),
        ])
        main.addItem(help.0)
        NSApp.helpMenu = help.1
        return main
    }

    // MARK: - Actions

    private var session: StreamSession? { app.session }

    @objc func showSettings(_ sender: Any?) {
        app.showSettings(over: NSApp.keyWindow)
    }

    @objc func showComputers(_ sender: Any?) {
        NSApp.windows.first { $0.windowController is HostListWindowController }?.makeKeyAndOrderFront(nil)
    }

    @objc func pairComputer(_ sender: Any?) {
        app.openPairing(hostID: nil)
    }

    @objc func disconnect(_ sender: Any?) {
        session?.disconnect()
    }

    @objc func reconnect(_ sender: Any?) {
        app.reconnectLast()
    }

    @objc func homeMonitor(_ sender: NSMenuItem) {
        let modes: [HomeMode] = [.mirror, .privacy, .off]
        let m = modes[max(0, min(sender.tag, 2))]
        if let s = session { s.setHomeMode(m) } else { app.model.settings.homeMode = m }
    }

    @objc func lock(_ sender: Any?) {
        session?.sendLock()
    }

    @objc func sendKeys(_ sender: NSMenuItem) {
        guard let s = session else { return }
        switch sender.tag {
        case 0: s.sendChord(KeyMods.gui, 0)
        case 1: s.sendChord(KeyMods.gui, 0x0F)
        case 2: s.sendChord(KeyMods.alt, 0x2B)
        case 3: s.sendChord(KeyMods.alt, 0x3D)
        case 4: s.sendChord(0, 0x46)
        case 5: s.sendChord(KeyMods.ctrl | KeyMods.alt, 0x3B)
        default: break
        }
    }

    @objc func togglePlaySound(_ sender: Any?) {
        app.model.settings.playSound.toggle()
    }

    @objc func fullScreen(_ sender: Any?) {
        session?.toggleFullScreen()
    }

    @objc func inStreamMenu(_ sender: Any?) {
        guard let s = session, s.window.isFullScreen else { return }
        s.model.revealMenu.toggle()
    }

    @objc func statsMode(_ sender: NSMenuItem) {
        let modes: [HudMode] = [.off, .compact, .expanded]
        let m = modes[max(0, min(sender.tag, 2))]
        app.model.settings.hud = m
    }

    @objc func resolution(_ sender: NSMenuItem) {
        switch sender.tag {
        case 0: app.model.settings.resolution = .matchWindow
        case 1: app.model.settings.resolution = .matchDisplay
        default:
            let sizes = [PixelSize(width: 1280, height: 800), PixelSize(width: 1680, height: 1050),
                         PixelSize(width: 1920, height: 1200), PixelSize(width: 2560, height: 1600)]
            let i = max(0, min(sender.tag - 10, sizes.count - 1))
            app.model.settings.fixedSize = sizes[i]
            app.model.settings.resolution = .fixed
        }
    }

    @objc func refreshRate(_ sender: NSMenuItem) {
        let display = DisplayInfo.of(NSApp.keyWindow?.screen).hz
        app.model.settings.refreshHz = sender.tag == display ? 0 : sender.tag
    }

    @objc func uiSize(_ sender: NSMenuItem) {
        let sizes: [UISizeChoice] = [.matchMac, .larger, .smaller, .sameAsDesk]
        app.model.settings.uiSize = sizes[max(0, min(sender.tag, 3))]
    }

    @objc func fixedResolution(_ sender: Any?) {
        app.model.settings.fixedResolution.toggle()
    }

    @objc func optimisedLook(_ sender: NSMenuItem) {
        app.model.settings.optimisedLook = sender.tag == 1
    }

    @objc func keyboard(_ sender: NSMenuItem) {
        app.model.settings.keyboard = sender.tag == 0 ? .mac : .pc
    }

    @objc func toggleInputMethod(_ sender: Any?) {
        app.model.settings.inputMethod = app.model.settings.inputMethod == .mac ? .computer : .mac
    }

    @objc func toggleSystemShortcuts(_ sender: Any?) {
        app.model.settings.systemShortcuts = app.model.settings.systemShortcuts == .never ? .fullScreenAndFocused : .never
    }

    @objc func releaseKeys(_ sender: Any?) {
        session?.releaseKeyboard()
    }

    @objc func showAbout(_ sender: Any?) {
        app.showAbout()
    }

    @objc func remoteHelp(_ sender: Any?) {
        app.showHelp(anchor: "help")
    }

    @objc func runLatencyTest(_ sender: Any?) {
        app.runLatencyTest()
    }

    @objc func showStatsFiles(_ sender: Any?) {
        app.showStatsFolder()
    }

    @objc func copyDiagnostics(_ sender: Any?) {
        app.copyDiagnostics()
    }

    // MARK: - Menu state

    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        let s = app.model.settings
        let live = session?.connected ?? false
        switch item.action {
        case #selector(disconnect(_:)):
            return session != nil
        case #selector(lock(_:)), #selector(sendKeys(_:)), #selector(releaseKeys(_:)):
            return live
        case #selector(reconnect(_:)):
            return session != nil || app.model.hosts.isEmpty == false
        case #selector(fullScreen(_:)):
            item.title = (session?.window.isFullScreen ?? false) ? String(localized: "Exit Full Screen") : String(localized: "Enter Full Screen")
            return session != nil
        case #selector(inStreamMenu(_:)):
            return session?.window.isFullScreen ?? false
        case #selector(homeMonitor(_:)):
            let modes: [HomeMode] = [.mirror, .privacy, .off]
            item.state = modes[max(0, min(item.tag, 2))] == s.homeMode ? .on : .off
            return app.model.hostOffersHomeMonitor
        case #selector(togglePlaySound(_:)):
            item.state = s.playSound ? .on : .off
            return true
        case #selector(statsMode(_:)):
            let modes: [HudMode] = [.off, .compact, .expanded]
            item.state = modes[max(0, min(item.tag, 2))] == s.hud ? .on : .off
            return session != nil
        case #selector(resolution(_:)):
            switch item.tag {
            case 0: item.state = s.resolution == .matchWindow ? .on : .off
            case 1: item.state = s.resolution == .matchDisplay ? .on : .off
            default:
                let sizes = [PixelSize(width: 1280, height: 800), PixelSize(width: 1680, height: 1050),
                             PixelSize(width: 1920, height: 1200), PixelSize(width: 2560, height: 1600)]
                let i = max(0, min(item.tag - 10, sizes.count - 1))
                item.state = s.resolution == .fixed && s.fixedSize == sizes[i] ? .on : .off
            }
            return true
        case #selector(refreshRate(_:)):
            let display = DisplayInfo.of(NSApp.keyWindow?.screen).hz
            let current = s.refreshHz == 0 ? display : s.refreshHz
            item.state = item.tag == current ? .on : .off
            return true
        case #selector(uiSize(_:)):
            let sizes: [UISizeChoice] = [.matchMac, .larger, .smaller, .sameAsDesk]
            item.state = sizes[max(0, min(item.tag, 3))] == s.uiSize ? .on : .off
            return true
        case #selector(fixedResolution(_:)):
            item.state = s.fixedResolution ? .on : .off
            return true
        case #selector(optimisedLook(_:)):
            item.state = (item.tag == 1) == s.optimisedLook ? .on : .off
            return true
        case #selector(keyboard(_:)):
            item.state = (item.tag == 0) == (s.keyboard == .mac) ? .on : .off
            return true
        case #selector(toggleInputMethod(_:)):
            item.state = s.inputMethod == .mac ? .on : .off
            return true
        case #selector(toggleSystemShortcuts(_:)):
            item.state = s.systemShortcuts == .never ? .off : .on
            return true
        case #selector(runLatencyTest(_:)):
            return live
        default:
            return true
        }
    }
}
