// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Templates as T

// Push button (Components board, Controls; states from Components · states B1):
// primary, secondary, ghost, destructive (soft red) and destructiveSolid (the
// red of close hover, for actions that can't be undone, P2Dialogs).
//
//     Button { text: "Reload"; variant: "primary" }
//     Button { text: "New event"; icon.name: "list-add-symbolic"; variant: "primary"; compact: true }
//
// compact is the 30 px header action of Calendar and Task manager. Icons are
// 14 px, 6 px from the label.
// Every language (decision 20, LangComponents A01): content-sized, at least 72 px wide with a label
// (compact header actions: their content), and the label never elides or wraps. A row of buttons that
// doesn't fit stacks: put them in a ButtonRow.
// States: primary and destructiveSolid take white 14% over their fill on hover
// and black 12% pressed; secondary white 10% -> 14% -> 20%; ghost none -> 10%
// -> 20%; destructive red 16% -> 24% -> 32%. Pressed shows at once, the rest
// fades over 120 ms. Keyboard focus: a 2 px ring 2 px outside. Disabled: white
// 5% with muted text (ghost keeps no fill), no hover.
T.Button {
    id: control
    // the name without the translator's break marks (decision 92); a name of your own replaces it
    Accessible.name: I18n.stripBreakMarks(text)

    /// primary | secondary | ghost | destructive | destructiveSolid
    property string variant: "secondary"
    property bool compact: false
    /// Forces a look for galleries and screenshots: "", hover, pressed, focus.
    property string previewState: ""

    readonly property var metrics: compact ? Theme.metrics.controls.buttonCompact : Theme.metrics.controls.button
    readonly property bool showHover: enabled && (previewState === "hover" || hovered)
    readonly property bool showDown: enabled && (previewState === "pressed" || down || checked)
    readonly property bool showFocus: previewState === "focus" || visualFocus || (Theme.focusRingAlways && activeFocus)
    readonly property color foreground: !enabled ? Theme.textMuted : (({
        primary: Theme.textOnAccent,
        secondary: Theme.text,
        ghost: Theme.accentInk,
        destructive: Theme.dangerText,
        destructiveSolid: "#ffffff"
    })[variant] ?? Theme.text)
    // rest, hover, pressed
    readonly property var fills: {
        if (!enabled)
            return variant === "ghost" ? ["transparent", "transparent", "transparent"]
                                       : [Theme.disabledFill, Theme.disabledFill, Theme.disabledFill]
        switch (variant) {
        case "primary":
            return [Theme.accent, Theme.over(Theme.accent, Theme.liftOverlay), Theme.over(Theme.accent, Theme.pressOverlay)]
        case "ghost":
            return ["transparent", Theme.hover, Theme.pressed]
        case "destructive":
            return [Theme.dangerSoft, Theme.dangerSoftHover, Theme.dangerSoftPressed]
        case "destructiveSolid":
            return [Theme.closeHover, Theme.over(Theme.closeHover, Theme.liftOverlay), Theme.over(Theme.closeHover, Theme.pressOverlay)]
        default:
            return [Theme.hover, Theme.secondaryHover, Theme.pressed]
        }
    }
    /// The rest fill (kept for compatibility).
    readonly property color fillColor: fills[0]

    implicitWidth: Math.max(implicitBackgroundWidth, implicitContentWidth + leftPadding + rightPadding,
                            !compact && text !== "" ? (metrics.minWidth ?? 72) : 0)
    implicitHeight: metrics.height
    leftPadding: metrics.paddingX
    rightPadding: metrics.paddingX
    topPadding: 0
    bottomPadding: 0
    spacing: 6
    hoverEnabled: true
    font: Theme.font(compact ? "controlSmall" : "control")
    icon.width: 14
    icon.height: 14

    contentItem: Item {
        implicitWidth: row.implicitWidth
        implicitHeight: row.implicitHeight

        Row {
            id: row
            anchors.centerIn: parent
            spacing: control.spacing

            Icon {
                anchors.verticalCenter: parent.verticalCenter
                visible: control.icon.name !== ""
                name: control.icon.name
                size: control.icon.width
                color: control.foreground
            }
            StyledText {
                anchors.verticalCenter: parent.verticalCenter
                visible: control.text !== ""
                text: control.text
                style: control.compact ? "controlSmall" : "control"
                font: control.font
                color: control.foreground
            }
        }
    }

    background: StateFill {
        implicitHeight: control.metrics.height
        implicitWidth: control.metrics.height
        radius: control.metrics.radius
        rest: control.fills[0]
        hover: control.fills[1]
        pressed: control.fills[2]
        hovered: control.showHover
        down: control.showDown

        FocusRing {
            shown: control.showFocus
            controlRadius: parent.radius
        }
    }
}
