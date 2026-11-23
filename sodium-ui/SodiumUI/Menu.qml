// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Templates as T
import "popup.js" as MenuUtil

// Menu and context menu (Components board "Context menu"; the Super + X menu):
// menu glass, radius 14, 6 px padding, 30 px items. The current item is accent
// with on-accent text; separators are white 8% lines.
// Width (decision 20, LangComponents A08/B08): as wide as its longest item, from minimumWidth (250 px,
// today's width; 200 is the smallest a menu should ask for) to maximumWidth (420 px); an item longer
// than that wraps onto a second line, never elides. Set minimumWidth, not width: a width of your own
// fixes it as before.
//
//     Menu {
//         id: menu
//         MenuItem { text: "New window"; shortcut: "Ctrl + N" }
//         Menu { title: "Move to space"; MenuItem { text: "Space 1" } }   // submenu
//         MenuItem { text: "Always on top"; checkable: true }
//         MenuSeparator {}
//         MenuSection { text: "Danger zone" }
//         MenuItem { text: "End task"; destructive: true }
//     }
//     Button { id: more; onClicked: menu.popup(more, 0, more.height + 4) }   // below a button
//     // or use ContextMenu for right clicks, which opens at the click
//
// Keyboard: Up / Down move (skipping separators, sections and disabled items),
// Right or Enter opens a submenu, Left or Escape closes it, Enter / Space
// triggers.
//
// Motion (P4Motion DL2): a menu grows from 98 % on snappy out of its corner at the
// pointer (or at the button it drops from; the other corner when Qt flips it at a
// screen edge), its alpha rising over 120 ms; a submenu slides 4 px out of its parent
// row instead. Both leave the same way back on the exit curve. Reduce motion: a 120 ms
// fade.
//
// Menus are in-window popups (Popup.Item): Qt 6.12's popup windows
// (Popup.Window) fail on this Hyprland + NVIDIA setup (EGL_BAD_ALLOC, see
// spikes/2-sodiumui/popup-window.qml). So a menu stays inside its window, where
// the compositor can't frost what is under it; instead the menu blurs its own
// window's content behind it (blurBehind, on by default; Glass.backdrop). A
// menu alone in a shell layer (a Quickshell PopupWindow, to leave a bar or
// panel) sets blurBehind: false: Hyprland blurs that layer.
T.Menu {
    id: control

    /// Blur the window content behind the menu (in-window glass). false when the menu sits
    /// alone in its own layer surface, which the compositor blurs.
    property bool blurBehind: true
    /// What lies behind the menu: by default the top-level item of the window that holds
    /// the menu's parent (a submenu uses its parent menu's).
    property Item backdrop: MenuUtil.backdropFor(control, parent)

    readonly property var metrics: Theme.metrics.controls.menu
    /// Keep the 16 px lead column in every item even when no item has an icon, check or radio.
    property bool reserveIconSpace: false
    /// Every item keeps the lead column when one of them uses it (P2Bar).
    readonly property bool leadColumn: reserveIconSpace || _anyLead(count)

    function _anyLead(n) {
        for (let i = 0; i < n; i++) {
            const it = itemAt(i)
            if (it && ((it.icon && it.icon.name !== "") || it.checkable === true))
                return true
        }
        return false
    }

    /// The narrowest it gets (decision 20: menus are 200–420 px wide, as wide as their longest item).
    /// Defaults to today's 250 px. A `width` of your own still fixes it: use minimumWidth instead.
    property real minimumWidth: metrics.width
    /// The widest it gets; a longer item wraps onto a second line (items never elide).
    property real maximumWidth: Math.max(minimumWidth, metrics.maxWidth ?? 420)
    /// The longest item's width (its label, shortcut and lead column on one line).
    readonly property real longestItemWidth: {
        let w = 0
        for (let i = 0; i < count; i++) {
            const it = itemAt(i)
            if (it && it.visible !== false)
                w = Math.max(w, it.implicitWidth)
        }
        return w
    }

    implicitWidth: Math.max(minimumWidth, Math.min(maximumWidth, longestItemWidth + leftPadding + rightPadding))
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset,
                             implicitContentHeight + topPadding + bottomPadding)
    padding: metrics.padding
    margins: 8
    // A submenu's glass meets its parent's: Qt places it `overlap` px into the parent item.
    overlap: metrics.padding
    popupType: T.Popup.Item
    font: Theme.font("menu")

    delegate: MenuItem {}

    /// Where it was opened, in its parent's coordinates: the pointer, for a menu opened at
    /// a click (ContextMenu and popupAt() set it). It grows from its corner there, or from
    /// the corner Qt puts there when it flips the menu at the window's edge. Unset (NaN):
    /// from its top-left corner, or the corner next to its parent item when it ends up
    /// above or left of it (a menu dropping from a button, flipped).
    property point openPoint: Qt.point(NaN, NaN)

    /// popup(item, x, y) at a point (the pointer), growing from the menu's corner there.
    function popupAt(item, x, y) {
        openPoint = Qt.point(x, y)
        popup(item, x, y)
    }

    // A submenu's parent is the row that opened it.
    readonly property bool _isSubmenu: parent !== null && parent.subMenu === control
    // The popup's own item, where Qt placed it (Qt rewrites x and y when it flips a menu).
    readonly property Item _box: background ? background.parent : null
    // Which way the menu lies from where it opened: x 1 right / -1 left, y 1 below / -1 above.
    readonly property point _side: {
        if (!_box || !_box.parent || !parent)
            return Qt.point(1, 1)
        const cx = _box.x + _box.width / 2, cy = _box.y + _box.height / 2
        if (_isSubmenu) {
            const row = parent.mapToItem(_box.parent, parent.width / 2, 0)
            return Qt.point(cx >= row.x ? 1 : -1, 1)
        }
        if (!isNaN(openPoint.x)) {
            const at = parent.mapToItem(_box.parent, openPoint.x, openPoint.y)
            return Qt.point(cx >= at.x ? 1 : -1, cy >= at.y ? 1 : -1)
        }
        const r = parent.mapToItem(_box.parent, 0, 0, parent.width, parent.height)
        return Qt.point(_box.x + _box.width <= r.x + 1 ? -1 : 1, _box.y + _box.height <= r.y + 1 ? -1 : 1)
    }
    transformOrigin: _side.y > 0 ? (_side.x > 0 ? Item.TopLeft : Item.TopRight)
                                 : (_side.x > 0 ? Item.BottomLeft : Item.BottomRight)

    /// The menu's window maps as the menu opens (a shell layer holding just the menu): the
    /// motion starts once that layer is on screen (Reveal.layerItem).
    property bool waitsForWindow: false

    property Reveal _reveal: Reveal {
        preset: "snappy"
        layerItem: control.waitsForWindow ? control.background : null
    }
    readonly property real _popFrom: Theme.motion.menuPopFrom ?? 0.98
    property Translate _slide: Translate {
        x: control._isSubmenu ? -control._side.x * (1 - control._reveal.progress) * (Theme.motion.submenuSlide ?? 4) : 0
    }
    scale: _isSubmenu ? 1 : _popFrom + (1 - _popFrom) * _reveal.progress
    opacity: _reveal.opacity

    enter: Transition {
        ScriptAction { script: control._reveal.shown = true }
    }
    exit: Transition {
        SequentialAnimation {
            ScriptAction { script: control._reveal.shown = false }
            PauseAnimation { duration: Theme.reduceMotion ? Theme.fadeMs : Theme.exit.durationMs }
        }
    }

    contentItem: ListView {
        implicitHeight: contentHeight
        model: control.contentModel
        interactive: Window.window ? contentHeight + control.topPadding + control.bottomPadding > control.height : false
        clip: true
        currentIndex: control.currentIndex
        transform: control._slide
        ScrollBar.vertical: ScrollBar {}
    }

    background: Glass {
        id: glass
        strength: "menu"
        backdrop: control.blurBehind ? control.backdrop : null
        backdropTrack: [control.x, control.y, control._reveal.progress]
        implicitWidth: control.metrics.width
        implicitHeight: control.metrics.itemHeight + 2 * control.metrics.padding
        transform: control._slide
    }
}
