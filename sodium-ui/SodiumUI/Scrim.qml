// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick

// The dim behind a ContentDialog or Sheet (P2Dialogs: scrimWindow over the window
// body). It starts below the title bar (`dimTop`), so the chrome stays clear as drawn,
// and follows the window's squircle corners at the bottom. Fades with its popup
// (P4Motion DL2): in linearly over 120 ms, out on the exit curve (160 ms); with Reduce
// motion 120 ms both ways. Internal: ContentDialog and Sheet use it as their
// T.Overlay.modal (Qt sets its opacity to 1 and 0; this Behavior animates that).
Item {
    id: scrim

    property color color: Theme.scrimWindow
    /// Where the dim starts: the title bar's height in an AppWindow, 0 for a layer.
    property real dimTop: 0
    /// The window's corner radius (0: square, e.g. a full-screen layer).
    property real radius: 0

    Behavior on opacity {
        id: fade
        NumberAnimation {
            readonly property bool leaving: fade.targetValue < scrim.opacity && !Theme.reduceMotion
            duration: leaving ? Theme.exit.durationMs : Theme.fadeMs
            // Setting a bezierCurve makes any easing a BezierSpline, so the way in is the
            // straight line through (0, 0) and (1, 1): linear.
            easing.type: Easing.BezierSpline
            easing.bezierCurve: leaving ? Theme.exitCurve : [0, 0, 1, 1, 1, 1]
        }
    }

    Item {
        y: scrim.dimTop
        width: parent.width
        height: Math.max(0, parent.height - y)
        clip: true

        Glass {
            visible: scrim.radius > 0
            y: -scrim.dimTop
            width: scrim.width
            height: scrim.height
            strength: "window"
            radius: scrim.radius
            tint: scrim.color
            hairline: "transparent"
            topHighlight: "transparent"
        }
        Rectangle {
            visible: scrim.radius <= 0
            anchors.fill: parent
            color: scrim.color
        }
    }
}
