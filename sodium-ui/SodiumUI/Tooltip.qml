// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Templates as T
import "popup.js" as MenuUtil

// Tooltip (Components board; the dock's app names on the Desktop board): tooltip glass,
// radius 8, 5 x 10 padding, 12 px text.
//
//     Button { id: b; HoverHandler { id: hh } Tooltip { text: "Reload"; visible: hh.hovered } }
//
// IconButton and Chip have a `tooltip` property that does this for you.
// Shown above its parent, centered, after 500 ms of hover (P4Motion DL2; the token's
// delayMs). Alpha only: in over 120 ms, out on the exit curve. None while a button is
// held: pressing its parent hides it until the pointer leaves and comes back. Like Menu,
// it stays in its window and blurs the window content behind it (blurBehind).
//
// Every language (decision 20, LangComponents A11/B11): it wraps at 320 px and never elides.
// `shortcut` adds a key hint after the text in text 3, 8 px on (the canonical tooltip's `kb`), its key
// names in the UI language (KeyCaps.format):
//
//     Tooltip { text: qsTr("Back"); shortcut: "Alt + ←" }
T.ToolTip {
    id: control

    /// above | below
    property string placement: "above"
    /// A key hint after the text ("Ctrl + F"), shown in the UI language's key names.
    property string shortcut: ""
    /// The widest it gets before the text wraps.
    property real maximumWidth: metrics.maxWidth ?? 320
    /// Blur the window content behind the tooltip (in-window glass); false in a layer of its own.
    property bool blurBehind: true
    /// What lies behind it: by default the top-level item of its parent's window.
    property Item backdrop: MenuUtil.backdropFor(control, parent)
    readonly property var metrics: Theme.metrics.controls.tooltip

    x: parent ? Math.round((parent.width - width) / 2) : 0
    y: placement === "above" ? -height - 6 : (parent ? parent.height + 6 : 0)
    margins: 6
    delay: metrics.delayMs
    timeout: -1
    leftPadding: metrics.paddingX
    rightPadding: metrics.paddingX
    topPadding: metrics.paddingY
    bottomPadding: metrics.paddingY
    font: Theme.font("caption")
    closePolicy: T.Popup.CloseOnEscape | T.Popup.CloseOnPressOutsideParent | T.Popup.CloseOnReleaseOutsideParent

    implicitWidth: Math.min(maximumWidth, Math.max(implicitBackgroundWidth + leftInset + rightInset,
                                                   implicitContentWidth + leftPadding + rightPadding))
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset, implicitContentHeight + topPadding + bottomPadding)

    contentItem: Item {
        implicitWidth: label.implicitWidth + (hint.visible ? 8 + hint.implicitWidth : 0)
        implicitHeight: Math.max(label.implicitHeight, hint.visible ? hint.implicitHeight : 0)

        StyledText {
            id: label
            anchors.left: parent.left
            anchors.right: hint.visible ? hint.left : parent.right
            anchors.rightMargin: hint.visible ? 8 : 0
            anchors.verticalCenter: parent.verticalCenter
            text: I18n.stripBreakMarks(control.text)   // a tooltip isn't a wrapped label (decision 92)
            style: "caption"
            font: control.font
            color: Theme.text
            wrapMode: Text.Wrap
        }
        StyledText {
            id: hint
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            visible: control.shortcut !== ""
            text: KeyCaps.format(control.shortcut)
            style: "caption"
            font: control.font
            color: Theme.text3
        }
    }

    background: Glass {
        strength: "tooltip"
        backdrop: control.blurBehind ? control.backdrop : null
        backdropTrack: [control.x, control.y]
    }

    // Between its enter and exit transitions.
    property bool _open: false
    // Its parent was pressed while it was up or on its way: it stays hidden until it closes.
    property bool _suppressed: false
    property Connections _pressWatch: Connections {
        target: control.parent
        ignoreUnknownSignals: true
        function onPressedChanged() {
            if (control.parent && control.parent.pressed === true && (control._open || control.visible))
                control._suppressed = true
        }
    }
    onClosed: _suppressed = false
    onAboutToShow: _suppressed = parent !== null && parent.pressed === true

    property Reveal _reveal: Reveal { preset: "snappy"; shown: control._open && !control._suppressed }
    opacity: _reveal.opacity
    enter: Transition {
        ScriptAction { script: control._open = true }
    }
    exit: Transition {
        SequentialAnimation {
            ScriptAction { script: control._open = false }
            PauseAnimation { duration: Theme.reduceMotion ? Theme.fadeMs : Theme.exit.durationMs }
        }
    }
}
