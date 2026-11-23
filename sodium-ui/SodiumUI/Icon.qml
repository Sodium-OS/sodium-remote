// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Window

// An icon from our icon theme (~/.local/share/icons/SodiumOS) by name.
//
//     Icon { name: "window-close-symbolic"; size: 14; color: Theme.control }
//     Icon { name: "org.sodiumos.Files"; size: 52 }          // full-color app tile
//     Icon { name: entry.icon; size: 52; app: true }         // any app's icon, on our tile
//
// Symbolic icons (names ending in -symbolic) are recoloured to `color` on the
// GPU, so they follow hover tints and re-tints smoothly. Other names are drawn
// in their own colors. Names we don't draw fall back to the system icon themes;
// an absolute path loads that file.
Item {
    id: icon

    property string name: ""
    /// Logical px (square).
    property real size: 16
    property color color: Theme.control
    /// Recolour to `color`. Default: for -symbolic names.
    property bool symbolic: name.endsWith("-symbolic")
    /// An app's icon: always a tile. Apps we don't draw get their own icon put on our tile
    /// (docs/decisions/icons.md G15), and an app with no icon at all gets the generic app tile.
    property bool app: false
    readonly property bool ready: image.status === Image.Ready

    implicitWidth: size
    implicitHeight: size

    Image {
        id: image

        readonly property real dpr: icon.Window.window ? icon.Window.window.devicePixelRatio : Screen.devicePixelRatio

        anchors.fill: parent
        visible: !icon.symbolic
        source: icon.name === "" ? "" : "image://sodiumicon/" + Math.round(icon.size) + "/"
            + (icon.symbolic ? "ffffffff" : icon.app ? (Theme.dark ? "app" : "app-light") : "-") + "/"
            + encodeURIComponent(icon.name)
        sourceSize: Qt.size(Math.ceil(icon.size * dpr), Math.ceil(icon.size * dpr))
        fillMode: Image.PreserveAspectFit
        smooth: true
        mipmap: !icon.symbolic
    }

    ShaderEffect {
        anchors.fill: parent
        visible: icon.symbolic && image.status === Image.Ready
        property var source: image
        property color color: icon.color
        fragmentShader: "qrc:/sodiumui/shaders/tint.frag.qsb"
    }
}
