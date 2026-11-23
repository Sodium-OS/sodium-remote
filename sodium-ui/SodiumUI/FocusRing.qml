// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick

// The keyboard focus ring (Components · states, "Keyboard focus"): an accent
// line (light: accent ink) around a control. Put it inside the control's
// background. It shows at once and fades out over 120 ms; it never changes
// the fill. Controls show it on keyboard focus (visualFocus), or on any focus while
// Settings › Accessibility › "Always show the focus ring" is on (Theme.focusRingAlways):
// shown: control.visualFocus || (Theme.focusRingAlways && control.activeFocus).
//
//     FocusRing { shown: control.visualFocus; controlRadius: 10 }                 // outside
//     FocusRing { shown: control.visualFocus; controlRadius: 8; gap: -lineWidth } // inside
//
// Three kinds, by control:
//   outside  2 px with a 2 px gap (the default): buttons, switch, checkbox,
//            radio, the slider knob, the segmented well. Its radius is the
//            control's + 4.
//   inside   2 px, gap: -lineWidth: chips and icon buttons, which sit 2-4 px apart.
//   inset    1.5 px, lineWidth: 1.5; gap: -lineWidth: fields, list rows, tabs,
//            sidebar items.
// gap is the space between the control's edge and the ring.
Rectangle {
    id: ring

    property bool shown: false
    property real controlRadius: Theme.radius.control
    property real gap: 2
    property real lineWidth: 2
    property color ringColor: Theme.focusRing

    anchors.fill: parent
    anchors.margins: -(gap + lineWidth)
    radius: Theme.squareCorners ? 0 : Math.max(0, controlRadius + gap + lineWidth)
    color: "transparent"
    border.width: lineWidth
    border.color: ringColor
    opacity: shown ? 1 : 0
    visible: opacity > 0

    Behavior on opacity {
        id: fade
        NumberAnimation { duration: fade.targetValue > 0 ? 0 : Theme.fadeMs }
    }
}
