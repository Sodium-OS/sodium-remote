// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick

// A top-level window for our apps: frameless (we draw the 48 px TitleBar),
// window glass (#161821 at 84%; Hyprland blurs behind it, rounds the corners
// and draws the shadow and the active halo), and edge resizing through the
// compositor. Lay out the chrome yourself, so the title bar can hold the
// app's header:
//
//     AppWindow {
//         title: "Calendar"
//         width: 1260; height: 712
//         ColumnLayout {
//             anchors.fill: parent
//             spacing: 0
//             TitleBar { Layout.fillWidth: true; ... }
//             Item { Layout.fillWidth: true; Layout.fillHeight: true }   // content
//             StatusStrip { Layout.fillWidth: true; leftText: "8 items" }
//         }
//     }
//
// Every window is resizable (PLAN section 0): keep layouts working down to
// minimumWidth x minimumHeight.
Window {
    id: win

    /// The window glass; set its `tint` for a different material.
    readonly property alias glass: glass
    default property alias content: glass.data

    flags: Qt.Window | Qt.FramelessWindowHint
    color: "transparent"
    width: 960
    height: 640
    minimumWidth: 360
    minimumHeight: 240

    /// Windows only (null elsewhere): the frame, the DWM backdrop and the snap-layout button, from the native
    /// WindowChrome (docs/decisions/cross-platform.md, "Windows glass"). WindowControls reads it.
    property var chrome: null
    /// "mica" (list and settings windows) or "acrylic"; "none" is an opaque window.
    property string backdrop: "mica"
    Component.onCompleted: {
        if (Qt.platform.os === "windows") {
            try {
                chrome = Qt.createQmlObject("import SodiumUI; WindowChrome {}", win, "WindowChrome")
            } catch (e) {
                chrome = null
            }
        }
    }
    Binding { target: win.chrome; property: "window"; value: win; when: win.chrome !== null }
    Binding { target: win.chrome; property: "dark"; value: Theme.dark; when: win.chrome !== null }
    Binding { target: win.chrome; property: "backdrop"; value: Theme.reduceTransparency ? "none" : win.backdrop; when: win.chrome !== null }

    // Without a DWM backdrop (Windows before build 22621, transparency effects off) the glass has nothing behind it:
    // an opaque Graphite base.
    Rectangle {
        anchors.fill: parent
        visible: win.chrome !== null && !win.chrome.backdropActive
        color: Theme.surfaceSolid
    }

    Glass {
        id: glass
        anchors.fill: parent
        strength: "window"
        // Hyprland rounds tiled and maximised windows too (and tells tiled ones they are
        // maximised); only fullscreen windows have square corners.
        radius: win.visibility === Window.FullScreen || win.chrome !== null ? 0 : Theme.radius.window
    }

    ResizeEdges {
        anchors.fill: parent
        z: 1000
        visible: win.visibility === Window.Windowed && win.chrome === null
    }
}
