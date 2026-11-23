// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import SodiumUI

// The text pill that names a computer's system ("SodiumOS", "Windows", "macOS") with our own glyph for the kind of
// machine: product names, never translated, never a vendor's logo ("How does the client
// show a computer's OS"). Sits on a computer's picture, so it is a glass chip in the theme's surface.
Rectangle {
    id: pill

    property string os: ""

    visible: os !== ""
    implicitWidth: row.implicitWidth + 16
    implicitHeight: 22
    radius: 11
    color: Qt.rgba(Theme.surfaceSolid.r, Theme.surfaceSolid.g, Theme.surfaceSolid.b, 0.82)
    border.width: 1
    border.color: Theme.hairline

    RowLayout {
        id: row
        anchors.centerIn: parent
        spacing: 4
        Icon { name: pill.os === "mac" ? "computer-laptop-symbolic" : (pill.os === "linux" || pill.os === "sodiumos") ? "utilities-terminal-symbolic" : "video-display-symbolic"; size: 12; color: Theme.text2 }
        Text {
            text: Fmt.osName(pill.os)
            font: Theme.font("caption")
            color: Theme.text2
        }
    }
}
