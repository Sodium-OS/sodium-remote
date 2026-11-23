// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Templates as T

// A sheet dropping from its window's title bar (P2Dialogs G6, G7: the file chooser
// in our apps): it covers most of the window, 40 px under the title bar, centered,
// with 16 px kept clear at the sides and bottom. Dialog tint at 97%, radius 16, the
// dialog shadow; the window body dims under it. The content lays itself out (header,
// list, footer buttons): fill the sheet with a ColumnLayout.
//
//     Sheet {
//         id: chooser
//         width: 880; height: 470                  // the board's sizes; the window clamps them
//         onAccepted: save(nameField.text)
//         ColumnLayout {
//             anchors.fill: parent
//             ...                                  // title, crumbs, sidebar and list
//             RowLayout { Button { text: "Cancel"; onClicked: chooser.reject() }
//                         Button { text: "Save"; variant: "primary"; onClicked: chooser.accept() } }
//         }
//     }
//     chooser.open()
//
// Esc rejects. Enter is the content's to handle (the name field's Enter saves).
// Motion (P4Motion DL2): it drops 12 px from under the title bar on smooth, its alpha
// rising over 120 ms, while the dim fades in; it leaves back up 12 px on the exit curve.
// G7 hangs it 40 px under the title bar, so the drop never reaches the bar (the board's
// clip at the bar's bottom edge has nothing to cut). Reduce motion: a 120 ms fade.
T.Dialog {
    id: control

    /// The dim over the window body (Theme.scrimWindow).
    property color scrimColor: Theme.scrimWindow
    /// Where the window body starts: under an AppWindow's title bar, else the top.
    property real dimTop: _appWindow ? Theme.metrics.titleBar.height : 0
    readonly property var metrics: Theme.metrics.controls.sheet ?? ({ top: 40, margin: 16, width: 880, height: 470 })
    default property alias content: holder.data

    readonly property var _appWindow: parent && parent.Window.window && parent.Window.window.glass !== undefined ? parent.Window.window : null
    readonly property real _top: dimTop + metrics.top
    readonly property real _availableWidth: parent ? parent.width - 2 * metrics.margin : metrics.width
    readonly property real _availableHeight: parent ? parent.height - _top - metrics.margin : metrics.height

    /// The narrowest it gets (the board's 880 px; decision 20: today's widths are minimums), and the widest
    /// it grows to for content whose implicitWidth asks for more (at least 640). Set minimumWidth, not
    /// width: a width of your own fixes it as before.
    property real minimumWidth: metrics.width
    property real maximumWidth: Math.max(640, minimumWidth)
    /// The height it opens at (the board's 470 px), kept inside the window.
    property real preferredHeight: metrics.height
    /// What the content asks for: the widest child's implicitWidth.
    readonly property real contentNeed: {
        let w = 0
        for (const c of holder.children)
            if (c.visible)
                w = Math.max(w, c.implicitWidth)
        return w
    }

    parent: T.Overlay.overlay
    // The board's G7 size, growing to its content up to maximumWidth, kept inside the window.
    width: Math.min(Math.max(minimumWidth, Math.min(maximumWidth, contentNeed)), _availableWidth)
    height: Math.min(preferredHeight, _availableHeight)
    x: parent ? Math.round((parent.width - width) / 2) : 0
    y: _top
    margins: -1                                      // placed by hand; never pushed around
    padding: 0
    modal: true
    dim: true
    focus: true
    popupType: T.Popup.Item
    closePolicy: T.Popup.CloseOnEscape

    T.Overlay.modal: Scrim {
        color: control.scrimColor
        dimTop: control.dimTop
        radius: control._appWindow ? control._appWindow.glass.radius : 0
    }

    property Reveal _reveal: Reveal { preset: "smooth" }
    property Translate _drop: Translate { y: -(1 - control._reveal.progress) * (Theme.motion.sheetDrop ?? 12) }
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

    background: Glass {
        strength: "dialog"
        transform: control._drop
    }

    contentItem: Item {
        id: holder
        transform: control._drop
    }
}
