// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import QtQuick.Window
import SodiumUI

// The "being controlled" notification: a small own window above the taskbar's corner, Acrylic,
// that never takes the focus, with Disconnect and Dismiss. A Windows toast can't carry a button that works while the
// app is not in the foreground without a registered AppUserModelID, so this is our own window.
Window {
    id: win

    property string who: ""
    property real since: 0

    flags: Qt.Tool | Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint | Qt.WindowDoesNotAcceptFocus
    color: "transparent"
    width: 372
    height: body.implicitHeight + 32
    visible: false
    objectName: "controlled-notice"

    function announce(name, startedAt) {
        who = name
        since = startedAt
        const scr = Qt.application.screens[0]
        x = scr.virtualX + scr.width - width - 16
        y = scr.virtualY + (scr.desktopAvailableHeight > 0 ? scr.desktopAvailableHeight : scr.height) - height - 16
        show()
        hideTimer.restart()
    }

    Timer { id: hideTimer; interval: 10000; onTriggered: win.hide() }
    Connections {
        target: Host
        function onControlChanged() { if (!Host.controlled) win.hide() }
    }

    WindowChrome {
        id: chrome
        window: win
        dark: Theme.dark
        backdrop: Theme.reduceTransparency ? "none" : "acrylic"
        resizable: false
    }

    Rectangle {
        anchors.fill: parent
        color: chrome.backdropActive ? Theme.menu : Theme.surfaceSolid
        border.width: 1
        border.color: Theme.hairline
    }

    ColumnLayout {
        id: body
        x: 16
        y: 16
        width: parent.width - 32
        spacing: 10
        RowLayout {
            spacing: 10
            Icon { name: "sodiumshell-remote-symbolic"; size: 22; color: Theme.accent }
            WrappedLabel {
                Layout.fillWidth: true
                text: qsTr("%1 is controlling this computer").arg(win.who)
                font: Theme.font("heading")
                color: Theme.text
            }
        }
        Text {
            Layout.fillWidth: true
            text: win.since > 0 ? qsTr("Connected at %1. You can still use the keyboard and mouse.").arg(Format.timeOfDay(new Date(win.since * 1000))) : qsTr("You can still use the keyboard and mouse.")
            font: Theme.font("body")
            color: Theme.text2
            wrapMode: Text.Wrap
        }
        ButtonRow {
            Layout.fillWidth: true
            Button { objectName: "notice-disconnect"; variant: "destructive"; compact: true; text: qsTr("Disconnect"); onClicked: { Host.disconnectClient(); win.hide() } }
            Button { objectName: "notice-dismiss"; compact: true; text: qsTr("Dismiss"); onClicked: win.hide() }
        }
    }
}
