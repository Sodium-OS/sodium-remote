// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import QtQuick.Window
import SodiumUI

// The tray icon's menu: who is connected (a card with a red Disconnect) or the state, then
// Open Sodium Remote, Allow remote control, Pair a device…, Start at login, Quit. Its own small window with an
// Acrylic backdrop (DWMSBT_TRANSIENTWINDOW), not a Win32 menu; it closes when it loses the focus.
Window {
    id: win

    readonly property string brand: "Sodium Remote"

    flags: Qt.Tool | Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint
    color: "transparent"
    width: 288
    height: col.implicitHeight + 16
    visible: false
    objectName: "tray-menu"

    // (ax, ay): where the icon is (its centre, else the click), (wx, wy, ww, wh): the work area of its screen, all in
    // logical pixels. The taskbar is on the side the icon lies beyond the work area (any edge); the menu opens away
    // from it, next to the icon and inside the work area.
    function popupAt(ax, ay, wx, wy, ww, wh) {
        const m = 8
        const h = col.implicitHeight + 16   // not `height` yet: it only follows while the window is shown
        const clampX = v => Math.max(wx + m, Math.min(v, wx + ww - width - m))
        const clampY = v => Math.max(wy + m, Math.min(v, wy + wh - h - m))
        if (ay < wy) {                      // taskbar on top
            win.x = clampX(ax - width / 2); win.y = wy + m
        } else if (ax < wx) {               // on the left
            win.x = wx + m; win.y = clampY(ay - h / 2)
        } else if (ax >= wx + ww) {         // on the right
            win.x = wx + ww - width - m; win.y = clampY(ay - h / 2)
        } else {                            // at the bottom (the default), or the icon is inside the work area
            win.x = clampX(ax - width / 2); win.y = wy + wh - h - m
        }
        show()
        raise()
        requestActivate()
    }
    onActiveChanged: if (!active && visible) hide()

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

    component Item2: Rectangle {
        id: item
        property string label
        property bool check: false
        property bool checked: false
        signal triggered()
        Layout.fillWidth: true
        implicitHeight: 34
        radius: 8
        color: mouse.containsMouse ? Theme.fill : "transparent"
        Text {
            anchors.verticalCenter: parent.verticalCenter
            x: 12
            width: parent.width - 24 - (item.check ? 24 : 0)
            text: item.label
            font: Theme.font("body")
            color: Theme.text
            wrapMode: Text.Wrap
        }
        Icon {
            visible: item.check && item.checked
            anchors.verticalCenter: parent.verticalCenter
            x: parent.width - 30
            name: "object-select-symbolic"
            size: 16
            color: Theme.accent
        }
        MouseArea { id: mouse; anchors.fill: parent; hoverEnabled: true; onClicked: { item.triggered(); } }
    }

    ColumnLayout {
        id: col
        x: 8
        y: 8
        width: parent.width - 16
        spacing: 2

        // the state, or who is connected
        Rectangle {
            Layout.fillWidth: true
            implicitHeight: stateCol.implicitHeight + 20
            radius: 10
            color: Host.controlled ? Theme.accentSoft : Theme.fill
            ColumnLayout {
                id: stateCol
                anchors.fill: parent
                anchors.margins: 10
                spacing: 6
                RowLayout {
                    spacing: 8
                    Rectangle {
                        width: 8; height: 8; radius: 4
                        color: Host.controlled || Host.state === "on" ? "#34D399" : Theme.text3
                    }
                    Text {
                        Layout.fillWidth: true
                        text: Host.controlled ? qsTr("%1 is connected").arg(Host.controller.name ?? "")
                            : Host.state === "on" ? qsTr("Remote control is on") : qsTr("Remote control is off")
                        font: Theme.font("heading")
                        color: Theme.text
                        wrapMode: Text.Wrap
                    }
                }
                Text {
                    visible: Host.controlled
                    Layout.fillWidth: true
                    text: Host.controlled && (Host.controller.since ?? 0) > 0 ? qsTr("Since %1").arg(Format.timeOfDay(new Date(Host.controller.since * 1000))) : ""
                    font: Theme.font("caption")
                    color: Theme.text2
                }
                Button {
                    visible: Host.controlled
                    objectName: "tray-disconnect"
                    Layout.fillWidth: true
                    variant: "destructiveSolid"
                    compact: true
                    text: qsTr("Disconnect")
                    onClicked: { Host.disconnectClient(); win.hide() }
                }
            }
        }
        Item2 { label: qsTr("Open %1").arg(win.brand); onTriggered: { win.hide(); Win.openMain() } }
        Item2 { label: qsTr("Allow remote control"); check: true; checked: Host.enabled; onTriggered: { Host.setEnabled(!Host.enabled); win.hide() } }
        Item2 { visible: Host.state === "on"; Layout.preferredHeight: visible ? 34 : 0; label: qsTr("Pair a device…"); onTriggered: { win.hide(); Win.pairDevice() } }
        Item2 { label: qsTr("Start at login"); check: true; checked: Host.startAtLogin; onTriggered: Host.setStartAtLogin(!Host.startAtLogin) }
        Rectangle { Layout.fillWidth: true; Layout.topMargin: 2; Layout.bottomMargin: 2; height: 1; color: Theme.divider }
        Item2 { label: qsTr("Quit %1").arg(win.brand); onTriggered: Win.quit() }
    }
}
