// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Templates as T

// Scroll bar (Components · states Y1): an overlay, no gutter. Hidden until
// the content scrolls, then a 6 px thumb (radius 3, 3 px from the edge, at
// least 32 px long) at white 40% with a 1 px black 24% outline, so it reads on
// any content. With the pointer in its 14 px strip (the hit zone, even while
// hidden) the thumb widens to 10 px toward the content on snappy, at white
// 56%, over a white 6% track; dragging, white 72%, 1:1, expanded until
// release. A click on the track pages. It fades in over 120 ms and hides 1 s
// after the last scroll unless hovered, on the exit curve. Not focusable.
//
//     ListView { ScrollBar.vertical: ScrollBar {} }
T.ScrollBar {
    id: control

    readonly property bool hot: hovered || pressed
    readonly property real strip: 14
    readonly property real edge: 3

    implicitWidth: horizontal ? 40 : strip
    implicitHeight: horizontal ? strip : 40
    padding: 0
    minimumSize: Math.min(1, 32 / Math.max(1, horizontal ? width : height))
    hoverEnabled: true
    focusPolicy: Qt.NoFocus
    visible: policy !== T.ScrollBar.AlwaysOff && size < 1

    property bool _shown: false
    function _update() {
        if (policy === T.ScrollBar.AlwaysOn || active || hot) {
            _hide.stop()
            _shown = true
        } else if (_shown) {
            _hide.restart()
        }
    }
    onActiveChanged: _update()
    onHotChanged: _update()
    onPolicyChanged: _update()
    Component.onCompleted: _update()

    property Timer _hide: Timer {
        interval: 1000
        onTriggered: if (!control.active && !control.hot && control.policy !== T.ScrollBar.AlwaysOn) control._shown = false
    }
    property Spring _thickness: Spring { to: control.hot ? 10 : 6; preset: "snappy" }

    opacity: _shown ? 1 : 0
    Behavior on opacity {
        id: fade
        NumberAnimation {
            duration: fade.targetValue > 0 ? Theme.fadeMs : Theme.exit.durationMs
            // A bezierCurve makes any easing a BezierSpline: the way in is the straight line.
            easing.type: Easing.BezierSpline
            easing.bezierCurve: fade.targetValue > 0 ? [0, 0, 1, 1, 1, 1] : Theme.exitCurve
        }
    }

    // The track, under the pointer only; a press on it pages toward the press.
    background: Rectangle {
        radius: control.strip / 2
        color: control.hot ? Theme.scrollTrack : "transparent"
        Behavior on color { ColorAnimation { duration: Theme.fadeMs } }

        MouseArea {
            anchors.fill: parent
            onPressed: mouse => {
                const along = control.horizontal ? mouse.x / width : mouse.y / height
                if (along >= control.position && along <= control.position + control.size) {
                    mouse.accepted = false              // on the thumb: T.ScrollBar drags it
                    return
                }
                const step = along < control.position ? -control.size : control.size
                control.position = Math.max(0, Math.min(1 - control.size, control.position + step))
            }
        }
    }

    contentItem: Item {
        implicitWidth: control.horizontal ? 40 : control.strip
        implicitHeight: control.horizontal ? control.strip : 40

        Rectangle {
            readonly property real thickness: control._thickness.value
            x: control.horizontal ? 0 : (control.mirrored ? control.edge : parent.width - control.edge - thickness)
            y: control.horizontal ? parent.height - control.edge - thickness : 0
            width: control.horizontal ? parent.width : thickness
            height: control.horizontal ? thickness : parent.height
            radius: Math.min(width, height) / 2
            color: control.pressed ? Theme.scrollThumbDrag : control.hovered ? Theme.scrollThumbHover : Theme.scrollThumb
            border.width: 1
            border.color: Theme.scrollOutline
            Behavior on color { ColorAnimation { duration: Theme.fadeMs } }
        }
    }
}
