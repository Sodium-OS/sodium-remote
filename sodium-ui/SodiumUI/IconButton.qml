// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Templates as T

// Icon-only button: the 30 x 28 title-bar button (Components, every app header),
// or the 40 px round button of the Control center (round: true).
//
//     IconButton { icon.name: "go-previous-symbolic"; tooltip: "Previous month" }
//     IconButton { icon.name: "window-close-symbolic"; danger: true }   // turns #E5484D on hover
//
// States (Components · states B2): rest no fill, icon #B9BECA; hover white 10%
// and the icon in the text color; pressed white 20%; checked white 16%. Round:
// white 8% -> 16% -> 20%. danger (close): hover #E5484D with a white glyph,
// pressed that + black 12%. Pressed shows at once, the rest fades over 120 ms.
// Keyboard focus: a 2 px ring inside the edge. Disabled: 45% opacity.
T.Button {
    id: control

    property bool round: false
    /// Close-style: the hover fill is Theme.closeHover with a white icon.
    property bool danger: false
    property color iconColor: Theme.control
    /// Shown after Theme.metrics.controls.tooltip.delayMs while hovered.
    property string tooltip: ""
    /// A key hint in the tooltip, after its text ("Alt + ←"): never concatenate it into `tooltip`.
    property string tooltipShortcut: ""
    property string previewState: ""

    readonly property var metrics: Theme.metrics.titleBar.button
    readonly property bool showHover: enabled && (previewState === "hover" || hovered)
    readonly property bool showDown: enabled && (previewState === "pressed" || down)
    readonly property bool showChecked: previewState === "checked" || checked
    readonly property bool showFocus: previewState === "focus" || visualFocus || (Theme.focusRingAlways && activeFocus)

    implicitWidth: round ? Theme.metrics.controls.roundButton.size : metrics.width
    implicitHeight: round ? Theme.metrics.controls.roundButton.size : metrics.height
    padding: 0
    hoverEnabled: true
    focusPolicy: Qt.TabFocus
    icon.width: 16
    icon.height: 16
    opacity: enabled ? 1 : 0.45
    Accessible.name: I18n.stripBreakMarks(tooltip !== "" ? tooltip : text)

    contentItem: Item {
        Icon {
            anchors.centerIn: parent
            name: control.icon.name
            size: control.icon.width
            color: control.danger && (control.showHover || control.showDown) ? "#ffffff"
                 : control.showHover || control.showDown || control.showChecked ? Theme.text : control.iconColor
            Behavior on color { ColorAnimation { duration: Theme.fadeMs } }
        }
    }

    background: StateFill {
        radius: control.round ? height / 2 : control.metrics.radius
        rest: control.round ? Theme.roundFill : control.showChecked ? Theme.selected : "transparent"
        hover: control.danger ? Theme.closeHover : control.round ? Theme.roundHover
             : control.showChecked ? Theme.selected : Theme.hover
        pressed: control.danger ? Theme.over(Theme.closeHover, Theme.pressOverlay) : Theme.pressed
        hovered: control.showHover
        down: control.showDown

        FocusRing {
            shown: control.showFocus
            controlRadius: parent.radius
            gap: -lineWidth
        }
    }

    Tooltip {
        text: control.tooltip
        shortcut: control.tooltipShortcut
        visible: control.tooltip !== "" && control.hovered
    }
}
