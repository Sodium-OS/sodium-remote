// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import SodiumUI

// Why a connection failed or ended. Only reasons the host reports get a card of their own (a version mismatch, taken
// over, in use...); "couldn't reach it" stays generic and carries the firewall hint.
Rectangle {
    id: card

    property var failure: ({})
    property string hostName: ""
    /// The computer's system ("linux", "mac", "windows" or ""), for naming the place to act in copy about it.
    property string hostOs: ""
    readonly property string kind: failure.kind ?? "failed"
    readonly property bool pairAgain: kind === "not_paired" || kind === "revoked"

    signal retry()
    signal takeover()
    signal pairNow()
    signal dismissed()
    /// "Server settings…": the generic card, with no server added.
    signal serverSettingsRequested()
    /// Nothing answered and no server is configured: a computer on another network can't be reached without one.
    readonly property bool suggestServer: info.act === "retry" && info.generic === true && !Remote.serverConfigured

    readonly property var info: {
        const h = hostName
        switch (kind) {
        case "busy":
            return { icon: "dialog-information-symbolic", title: qsTr("%1 is in use").arg(h),
                     body: qsTr("%1 is connected to it. Take over to disconnect them.").arg(failure.by ?? ""), action: qsTr("Take over"), act: "takeover" }
        case "taken_over":
            return { icon: "dialog-information-symbolic", title: qsTr("Taken over by %1").arg(failure.by ?? ""),
                     body: qsTr("%1 connected, so this computer was disconnected. Only one device streams at a time.").arg(failure.by ?? ""), action: qsTr("Take it back"), act: "takeover" }
        case "offline":
            return { icon: "dialog-warning-symbolic", title: qsTr("%1 isn’t online").arg(h),
                     body: qsTr("It may be turned off or without internet."), action: qsTr("Try again"), act: "retry" }
        case "not_paired": case "revoked":
            return { icon: "dialog-warning-symbolic", title: qsTr("%1 doesn’t trust this computer").arg(h),
                     body: qsTr("It was removed there. Pair again to connect."), action: qsTr("Pair again"), act: "pair" }
        case "version_mismatch":
            // The engine says which side is older ("update": "host" or "client").
            if (failure.update === "client")
                return { icon: "dialog-warning-symbolic", title: qsTr("Sodium Remote needs an update"),
                         body: qsTr("%1 runs a newer version that this one can’t talk to. Update Sodium Remote on this computer, then try again.").arg(h), action: qsTr("Try again"), act: "retry" }
            return { icon: "dialog-warning-symbolic", title: qsTr("%1 needs an update").arg(h),
                     body: qsTr("It runs an older version that can’t talk to this one. Update it, then try again."), action: qsTr("Try again"), act: "retry" }
        case "control_off":
            // Named as it is labelled where the person must act.
            return { icon: "dialog-warning-symbolic", title: qsTr("%1 isn’t allowing control").arg(h),
                     body: hostOs === "sodiumos" ? qsTr("Open Settings › Remote on that computer and allow control, then try again.")
                         : hostOs === "windows" || hostOs === "mac" || hostOs === "linux" ? qsTr("Open %1 on that computer and allow control, then try again.").arg(Fmt.otherHostAppName)
                         : qsTr("Allow control on that computer, then try again."),
                     action: qsTr("Try again"), act: "retry" }
        case "permission_missing": {
            // A Mac without a permission: the action is on the Mac, and the card says
            // which permission is missing (Screen Recording, Accessibility or both).
            const sr = failure.screen_recording ?? true
            const ax = failure.accessibility ?? false
            if (sr && ax)
                return { icon: "dialog-warning-symbolic", title: qsTr("%1 can’t share or be controlled yet").arg(h),
                         body: qsTr("On that Mac, open System Settings › Privacy & Security, allow %1 under Screen Recording and under Accessibility, then reopen it.").arg(Fmt.otherHostAppName),
                         action: qsTr("Try again"), act: "retry" }
            if (ax)
                return { icon: "dialog-warning-symbolic", title: qsTr("%1 can’t be controlled yet").arg(h),
                         body: qsTr("On that Mac, open System Settings › Privacy & Security › Accessibility, allow %1, then reopen it.").arg(Fmt.otherHostAppName),
                         action: qsTr("Try again"), act: "retry" }
            return { icon: "dialog-warning-symbolic", title: qsTr("%1 can’t share its screen yet").arg(h),
                     body: qsTr("On that Mac, open System Settings › Privacy & Security › Screen Recording, allow %1, then reopen it.").arg(Fmt.otherHostAppName),
                     action: qsTr("Try again"), act: "retry" }
        }
        case "host_shutdown":
            return { icon: "dialog-information-symbolic", title: qsTr("%1 shut down").arg(h), body: qsTr("It stopped sharing its screen."), action: qsTr("Reconnect"), act: "retry" }
        case "host_sleeping":
            return { icon: "dialog-information-symbolic", title: qsTr("%1 went to sleep").arg(h), body: qsTr("Wake it, then reconnect."), action: qsTr("Reconnect"), act: "retry" }
        case "ended_at_computer":
            return { icon: "dialog-information-symbolic", title: qsTr("Ended at %1").arg(h), body: qsTr("Someone ended the session at that computer."), action: qsTr("Reconnect"), act: "retry" }
        case "host_locked":
            return { icon: "dialog-information-symbolic", title: qsTr("%1 is locked").arg(h), body: qsTr("Unlock it at that computer, then reconnect."), action: qsTr("Reconnect"), act: "retry" }
        case "hold_expired":
            return { icon: "dialog-warning-symbolic", title: qsTr("Connection to %1 lost").arg(h), body: qsTr("It didn’t come back in time."), action: qsTr("Reconnect"), act: "retry" }
        case "error":
            return { icon: "dialog-warning-symbolic", title: qsTr("Disconnected from %1").arg(h), body: qsTr("The connection ended unexpectedly."), action: qsTr("Reconnect"), act: "retry" }
        default:
            // Nothing answered, or the reason isn't one the host reports: the generic card with the hint line.
            // With no server added, the likeliest cause is a computer on another network: say where to look (never relay words here).
            return { icon: "dialog-error-symbolic", title: qsTr("Couldn’t reach %1").arg(h), generic: true,
                     body: Remote.serverConfigured ? qsTr("Nothing answered. Check the address and that the computer is on. A firewall on that computer can look the same.")
                                                   : qsTr("Nothing answered. Check the address and that the computer is on. If it’s on another network, add a server in Settings › Server."),
                     action: qsTr("Try again"), act: "retry" }
        }
    }

    implicitWidth: 400
    implicitHeight: col.implicitHeight + 48
    radius: 16
    color: Theme.dialog
    border.width: 1
    border.color: Theme.hairline

    ColumnLayout {
        id: col
        anchors.fill: parent
        anchors.margins: 24
        spacing: 12

        Icon { Layout.alignment: Qt.AlignHCenter; name: card.info.icon; size: 36; color: card.info.icon === "dialog-error-symbolic" ? Theme.danger : card.info.icon === "dialog-warning-symbolic" ? Theme.warning : Theme.text2 }
        WrappedLabel {
            objectName: "failure-title"
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            text: card.info.title
            font: Theme.font("heading")
            color: Theme.text
        }
        // Balanced lines: Chinese and Japanese never leave one character or a bare "。" on the last line.
        WrappedLabel {
            objectName: "failure-body"
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            text: card.info.body
            style: "body"
            color: Theme.text2
        }
        ButtonRow {
            Layout.fillWidth: true
            Layout.topMargin: 6
            Button { objectName: "failure-close"; text: qsTr("Close"); onClicked: card.dismissed() }
            Button { objectName: "failure-server"; visible: card.suggestServer; text: qsTr("Server settings…"); onClicked: card.serverSettingsRequested() }
            Button {
                objectName: "failure-action"
                text: card.info.action
                variant: "primary"
                onClicked: card.info.act === "takeover" ? card.takeover() : card.info.act === "pair" ? card.pairNow() : card.retry()
            }
        }
    }
}
