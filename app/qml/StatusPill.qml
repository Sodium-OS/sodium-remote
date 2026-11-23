// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import SodiumUI

// A computer's state on its picture: Online, In use, Asleep, Offline, Connecting…. A glass chip in the theme's surface;
// only the text takes the state's colour.
Rectangle {
    id: pill

    /// "online", "busy", "asleep", "offline" or "connecting".
    property string state: "offline"

    implicitWidth: row.implicitWidth + 18
    implicitHeight: 22
    radius: 11
    color: Qt.rgba(Theme.surfaceSolid.r, Theme.surfaceSolid.g, Theme.surfaceSolid.b, 0.82)
    border.width: 1
    border.color: Theme.hairline

    readonly property color tone: state === "online" ? Theme.success
                                : state === "busy" ? Theme.warning
                                : state === "connecting" ? Theme.text : Theme.text2

    RowLayout {
        id: row
        anchors.centerIn: parent
        spacing: 4
        Icon { visible: pill.state === "asleep"; name: "system-suspend-symbolic"; size: 12; color: pill.tone }
        Text {
            text: Fmt.statusName(pill.state)
            font: Theme.font("caption", { weight: 600 })
            color: pill.tone
        }
    }
}
