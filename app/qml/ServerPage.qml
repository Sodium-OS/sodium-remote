// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import SodiumUI

// Settings › Server:
// no server (and what still works), or the server in use with its state, Change, Remove, and Import and Export of server.toml.
// Nothing here saves before the sheet's own step: typed settings save directly, a link or a file passes the confirmation first.
ColumnLayout {
    id: page

    /// The sheet that adds, changes and confirms (ServerSheet.qml), and the remove dialog, belong to the settings sheet.
    signal addRequested()
    signal changeRequested()
    signal removeRequested()
    signal importRequested()
    signal exportRequested()
    /// One line to say after Import… or Export… (an error); cleared by the next action.
    property string notice: ""

    readonly property var server: Remote.server
    readonly property bool configured: Remote.serverConfigured
    readonly property string state: Remote.serverState
    readonly property string problem: Remote.serverProblem
    /// How this server is named: the host, or the hosted service's name (the core's label is English, so the words are ours).
    readonly property string name: server.official ? qsTr("Sodium Remote service") : (server.host ?? "")

    spacing: 16

    // ---- no server
    SettingsRowGroup {
        visible: !page.configured
        Layout.fillWidth: true
        footer: qsTr("Sodium Remote doesn’t contact any server until you add one. A server helps computers on different networks find each other, and relays the picture when they can’t connect directly.")
        ControlRow {
            objectName: "server-none"
            Layout.fillWidth: true
            text: qsTr("No server")
            description: qsTr("Computers on this network and by address still work.")
            Icon { name: "network-server-symbolic"; size: 20; color: Theme.text3 }
        }
    }
    // A Flow, so the buttons wrap onto a second line when the sheet is narrow instead of pushing it wider.
    Flow {
        visible: !page.configured
        Layout.fillWidth: true
        spacing: 12
        Button {
            objectName: "server-add"
            text: qsTr("Add a server…")
            variant: "primary"
            icon.name: "list-add-symbolic"
            onClicked: page.addRequested()
        }
        Button {
            objectName: "server-guide"
            variant: "ghost"
            text: qsTr("How to run your own server")
            palette.buttonText: Theme.accentInk
            onClicked: Remote.openSelfHosting()
        }
        Button { objectName: "server-import"; text: qsTr("Import…"); onClicked: page.importRequested() }
    }

    // ---- a server
    SettingsRowGroup {
        visible: page.configured
        Layout.fillWidth: true
        ControlRow {
            objectName: "server-row"
            Layout.fillWidth: true
            text: page.name
            description: page.state === "online" ? qsTr("%1 reachable").arg(page.name)
                       : page.state === "offline" ? qsTr("%1 unreachable").arg(page.name)
                       : page.state === "checking" ? qsTr("Checking…") : ""
            RowLayout {
                spacing: 8
                Rectangle {
                    objectName: "server-pill"
                    visible: page.state !== "off"
                    implicitWidth: pillRow.implicitWidth + 18
                    implicitHeight: 22
                    radius: 11
                    color: page.state === "online" ? Theme.successSoft : page.state === "offline" ? Theme.warningSoft : Theme.fill
                    RowLayout {
                        id: pillRow
                        anchors.centerIn: parent
                        spacing: 4
                        Spinner { visible: page.state === "checking"; size: 12 }
                        Icon { visible: page.state === "online"; name: "object-select-symbolic"; size: 12; color: Theme.success }
                        Icon { visible: page.state === "offline"; name: "dialog-warning-symbolic"; size: 12; color: Theme.warning }
                        Text {
                            text: page.state === "online" ? qsTr("Reachable") : page.state === "offline" ? qsTr("Unreachable") : qsTr("Checking…")
                            font: Theme.font("caption", { weight: 600 })
                            color: page.state === "online" ? Theme.success : page.state === "offline" ? Theme.warning : Theme.text2
                        }
                    }
                }
                Button { objectName: "server-change"; compact: true; text: qsTr("Change…"); onClicked: page.changeRequested() }
                Button { objectName: "server-remove"; compact: true; variant: "destructive"; text: qsTr("Remove"); onClicked: page.removeRequested() }
            }
        }
    }

    // What the server said, in the app's own words; its text is never shown for these codes.
    Banner {
        objectName: "server-problem"
        visible: page.configured && page.problem !== ""
        Layout.fillWidth: true
        text: page.problem === "tokenNeeded" ? qsTr("This server needs an enrollment token. Enter the one you were given.")
            : page.problem === "tokenInvalid" ? qsTr("That enrollment token isn’t valid. Check it with the person who runs the server.")
            : page.problem === "tokenExpired" ? qsTr("That enrollment token has expired. Ask the person who runs the server for a new one.")
            : page.problem === "rateLimited" ? qsTr("Too many computers have registered from this network address lately. Sodium Remote will try again in a few minutes.")
            : page.problem === "quotaExceeded" ? qsTr("%1 has reached its limit for now. Computers on this network and by address still work.").arg(page.name)
            : page.problem === "paymentRequired" ? qsTr("%1 needs a payment before this computer can use it. Computers on this network and by address still work.").arg(page.name)
            : ""
        actionText: page.problem === "tokenNeeded" || page.problem === "tokenInvalid" || page.problem === "tokenExpired" ? qsTr("Change…") : ""
        onActionTriggered: page.changeRequested()
    }
    Banner {
        objectName: "server-down"
        visible: page.configured && page.problem === "" && page.state === "offline"
        Layout.fillWidth: true
        text: qsTr("Couldn’t reach %1. Computers on this network and by address still work. Sodium Remote keeps trying.").arg(page.name)
        actionText: qsTr("Try now")
        onActionTriggered: Remote.retryServer()
    }
    Text {
        objectName: "server-waits"
        visible: Remote.serverWaitsForSession
        Layout.fillWidth: true
        Layout.leftMargin: 4
        text: qsTr("The new server is used when this session ends.")
        font: Theme.font("caption"); color: Theme.text2; wrapMode: Text.Wrap
    }

    SettingsRowGroup {
        visible: page.configured
        Layout.fillWidth: true
        title: qsTr("Server")
        ControlRow {
            visible: !page.server.official
            Layout.fillWidth: true
            text: qsTr("Address")
            // A URL: never translated, never elided.
            Text { objectName: "server-address"; text: page.server.url ?? ""; font: Theme.font("mono"); color: Theme.text; wrapMode: Text.WrapAnywhere; Layout.maximumWidth: 300 }
        }
        ControlRow {
            Layout.fillWidth: true
            text: qsTr("Certificate")
            Text { objectName: "server-cert"; text: page.server.pinnedCa ? qsTr("Pinned") : qsTr("Public"); font: Theme.font("body"); color: Theme.text2 }
        }
        ControlRow {
            visible: page.state === "online"
            Layout.fillWidth: true
            text: qsTr("This computer")
            Rectangle {
                implicitWidth: enrolledRow.implicitWidth + 18
                implicitHeight: 22
                radius: 11
                color: Theme.successSoft
                RowLayout {
                    id: enrolledRow
                    anchors.centerIn: parent
                    spacing: 4
                    Icon { name: "object-select-symbolic"; size: 12; color: Theme.success }
                    Text { text: qsTr("Enrolled"); font: Theme.font("caption", { weight: 600 }); color: Theme.success }
                }
            }
        }
    }
    RowLayout {
        visible: page.configured
        Layout.fillWidth: true
        spacing: 12
        Button { objectName: "server-import"; text: qsTr("Import…"); onClicked: page.importRequested() }
        Button { objectName: "server-export"; text: qsTr("Export…"); onClicked: page.exportRequested() }
        Item { Layout.fillWidth: true }
        Button {
            variant: "ghost"
            text: qsTr("How to run your own server")
            palette.buttonText: Theme.accentInk
            onClicked: Remote.openSelfHosting()
        }
    }
    Text {
        visible: page.configured && (page.server.token ?? false)
        Layout.fillWidth: true
        Layout.leftMargin: 4
        text: qsTr("The exported file includes the enrollment token.")
        font: Theme.font("caption"); color: Theme.text2; wrapMode: Text.Wrap
    }
    Text {
        objectName: "server-notice"
        visible: page.notice !== ""
        Layout.fillWidth: true
        Layout.leftMargin: 4
        text: page.notice
        font: Theme.font("caption"); color: Theme.danger; wrapMode: Text.Wrap
    }
}
