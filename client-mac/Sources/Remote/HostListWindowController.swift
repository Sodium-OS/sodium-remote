// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import RemoteUI

/// The main window: the welcome, then the computers. macOS's
/// window buttons sit in the unified toolbar with our title and Pair button;
/// the content draws its own glass under it. 1000 × 452 pt by default,
/// 420 × 400 at the least.
@MainActor
final class HostListWindowController: NSWindowController, NSToolbarDelegate, NSWindowDelegate {
    let model: AppModel
    private static let titleItem = NSToolbarItem.Identifier(AppIdentity.label("hosts.title"))
    private static let pairItem = NSToolbarItem.Identifier(AppIdentity.label("hosts.pair"))

    init(model: AppModel) {
        self.model = model
        let w = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1000, height: 452),
                         styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
                         backing: .buffered, defer: false)
        w.title = "Sodium Remote"
        w.titleVisibility = .hidden
        w.titlebarAppearsTransparent = true
        w.titlebarSeparatorStyle = .none
        w.contentMinSize = NSSize(width: 420, height: 400)
        w.isReleasedWhenClosed = false
        w.tabbingMode = .disallowed
        w.isOpaque = false
        w.backgroundColor = .clear
        super.init(window: w)
        w.delegate = self
        let tb = NSToolbar(identifier: NSToolbar.Identifier(AppIdentity.label("hosts")))
        tb.delegate = self
        tb.displayMode = .iconOnly
        tb.allowsUserCustomization = false
        w.toolbar = tb
        w.toolbarStyle = .unified
        w.contentView = HostListUI.makeContentView(model: model)
        w.setFrameAutosaveName("RemoteHostsWindow")
        if !w.setFrameUsingName("RemoteHostsWindow") {
            // Centred on first launch, near the top, then remembered.
            w.center()
        }
    }

    required init?(coder: NSCoder) {
        fatalError("not used")
    }

    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [HostListWindowController.titleItem, .flexibleSpace, HostListWindowController.pairItem]
    }

    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [HostListWindowController.titleItem, .flexibleSpace, HostListWindowController.pairItem]
    }

    func toolbar(_ toolbar: NSToolbar, itemForItemIdentifier id: NSToolbarItem.Identifier, willBeInsertedIntoToolbar flag: Bool) -> NSToolbarItem? {
        let item = NSToolbarItem(itemIdentifier: id)
        if id == HostListWindowController.titleItem {
            item.view = HostListUI.makeToolbarTitle(model: model)
            item.label = "Sodium Remote"
        } else if id == HostListWindowController.pairItem {
            item.view = HostListUI.makeToolbarPairButton(model: model)
            item.label = "Pair a computer"
        } else {
            return nil
        }
        return item
    }
}
