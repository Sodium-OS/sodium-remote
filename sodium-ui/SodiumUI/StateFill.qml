// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick

// A control's state fill (Components · states, Motion): the pressed fill shows
// at once (0 ms); hover, release and every other change fade over 120 ms.
// Internal to SodiumUI's controls.
//
//     background: StateFill { rest: Theme.hover; hover: Theme.secondaryHover; pressed: Theme.pressed
//                             hovered: control.showHover; down: control.showDown }
Rectangle {
    id: fill

    property color rest: "transparent"
    property color hover: rest
    property color pressed: hover
    property bool hovered: false
    property bool down: false

    color: down ? pressed : hovered ? hover : rest

    Behavior on color {
        id: fade
        ColorAnimation { duration: fill.down && Qt.colorEqual(fade.targetValue, fill.pressed) ? 0 : Theme.fadeMs }
    }
}
