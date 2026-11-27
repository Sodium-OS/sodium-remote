// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import SodiumUI

// Add a computer: choose one on this network or type its address, then pair with the 6-digit code it shows.
Sheet {
    id: sheet

    /// choose | pair | done
    property string step: "choose"
    property string targetId: ""
    property string targetName: ""
    /// "IP:PORT" when the computer was found by its address.
    property string targetAddr: ""

    signal connectNow(string hostId)

    width: Math.min(560, parent ? parent.width - 32 : 560)   // inside the window at its 480 px minimum
    // As tall as the step needs (the pairing code step is the tallest), so no step has a blank half.
    height: Math.min(Math.max(body.implicitHeight + 48, 300), (parent ? parent.height : 520) - 72)

    function begin() {
        step = "choose"
        Remote.resetPairing()
        Remote.resetAddress()
        addressField.text = ""
        Remote.refresh()
        open()
    }
    function startPairing(id, name, addr) {
        targetId = id
        targetName = name !== "" ? name : id !== "" ? id.slice(0, 8) : (addr ?? "")
        targetAddr = addr ?? ""
        Remote.resetPairing()
        code.clear()
        step = "pair"
        code.focusInput()
    }

    readonly property var pairing: Remote.pairing
    readonly property var address: Remote.address
    onPairingChanged: {
        if (step === "pair" && pairing.state === "ok") {
            targetName = pairing.name !== "" ? pairing.name : targetName
            // Paired by address alone: the pairing says which computer it was.
            if (pairing.id)
                targetId = pairing.id
            step = "done"
        } else if (step === "pair" && (pairing.state === "wrong" || pairing.state === "withdrawn" || pairing.state === "closed" || pairing.state === "failed" || pairing.state === "busy")) {
            code.clear()
            code.focusInput()
        }
    }
    onAddressChanged: {
        if (step === "choose" && address.state === "found")
            startPairing(address.host, address.name ?? "", address.ip ? address.ip + ":" + address.port : "")
    }

    ColumnLayout {
        id: body
        anchors.fill: parent
        anchors.margins: 24
        spacing: 14

        // ---- choose
        ColumnLayout {
            visible: sheet.step === "choose"
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 14

            ColumnLayout {
                spacing: 2
                Text { text: qsTr("Add a computer"); font: Theme.font("title"); color: Theme.text }
                Text {
                    Layout.fillWidth: true
                    text: qsTr("Choose a computer on this network, or type its address.")
                    font: Theme.font("body"); color: Theme.text2; wrapMode: Text.Wrap
                }
            }

            Text { text: qsTr("On this network"); font: Theme.font("section"); color: Theme.label }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 6
                Repeater {
                    model: Remote.nearby
                    Rectangle {
                        id: nrow
                        required property var model
                        Layout.fillWidth: true
                        implicitHeight: 48
                        radius: 10
                        color: Theme.fill
                        RowLayout {
                            anchors.fill: parent
                            anchors.leftMargin: 14
                            anchors.rightMargin: 8
                            spacing: 8
                            Text { Layout.fillWidth: true; text: nrow.model.name; font: Theme.font("body"); color: Theme.text; elide: Text.ElideRight }
                            Button {
                                objectName: "sheet-pair-" + nrow.model.hostId
                                compact: true
                                text: qsTr("Pair…")
                                onClicked: sheet.startPairing(nrow.model.hostId, nrow.model.name, "")
                            }
                        }
                    }
                }
                RowLayout {
                    visible: Remote.nearby.count === 0
                    spacing: 8
                    Spinner { size: 14 }
                    Text { text: qsTr("Looking for computers…"); font: Theme.font("body"); color: Theme.text2 }
                }
            }

            Text { Layout.topMargin: 6; text: qsTr("Connect by address"); font: Theme.font("section"); color: Theme.label }
            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                TextField {
                    id: addressField
                    objectName: "address-field"
                    Layout.fillWidth: true
                    placeholderText: qsTr("Name or IP address")
                    Accessible.name: qsTr("Connect by address")
                    error: sheet.address.state === "unreachable"
                    onAccepted: if (text.trim() !== "") Remote.resolveAddress(text)
                }
                Button {
                    objectName: "address-go"
                    text: qsTr("Continue")
                    variant: "primary"
                    enabled: addressField.text.trim() !== "" && sheet.address.state !== "checking"
                    onClicked: Remote.resolveAddress(addressField.text)
                }
            }
            Text {
                Layout.fillWidth: true
                visible: sheet.address.state !== "unreachable"
                text: qsTr("For a computer on another network, type its address and port, for example %1.").arg("192.0.2.4:47311")
                font: Theme.font("caption"); color: Theme.text2; wrapMode: Text.Wrap
            }
            Banner {
                Layout.fillWidth: true
                objectName: "address-error"
                visible: sheet.address.state === "unreachable"
                kind: "error"
                text: qsTr("Couldn’t reach %1. Check the address, and that Allow remote control is on there.").arg(sheet.address.addr ?? "")
            }
            Item { Layout.fillHeight: true }
            RowLayout {
                Layout.fillWidth: true
                Item { Layout.fillWidth: true }
                Button { text: qsTr("Close"); onClicked: sheet.close() }
            }
        }

        // ---- pair
        ColumnLayout {
            visible: sheet.step === "pair"
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 14

            ColumnLayout {
                spacing: 2
                Text { Layout.fillWidth: true; text: qsTr("Pair with %1").arg(sheet.targetName); font: Theme.font("title"); color: Theme.text; wrapMode: Text.Wrap }
                Text {
                    Layout.fillWidth: true
                    text: qsTr("Type the 6-digit code that %1 shows.").arg(sheet.targetName)
                    font: Theme.font("body"); color: Theme.text2; wrapMode: Text.Wrap
                }
            }
            CodeField {
                id: code
                Layout.alignment: Qt.AlignHCenter
                Layout.topMargin: 10
                error: sheet.pairing.state === "wrong" || sheet.pairing.state === "withdrawn"
                busy: sheet.pairing.state === "checking"
                onSubmitted: c => Remote.pair(sheet.targetId, c, sheet.targetAddr)
            }
            Text {
                objectName: "pair-message"
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignHCenter
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
                font: Theme.font("body")
                color: sheet.pairing.state === "wrong" || sheet.pairing.state === "withdrawn" || sheet.pairing.state === "closed" || sheet.pairing.state === "failed" || sheet.pairing.state === "busy" ? Theme.dangerText : Theme.text2
                text: {
                    switch (sheet.pairing.state) {
                    case "checking": return qsTr("Checking with the computer…")
                    case "wrong": return qsTr("That code didn’t match. Check the computer and try again: %n attempt(s) left.", "", sheet.pairing.left ?? 0)
                    case "withdrawn": return qsTr("Too many wrong codes, so the computer withdrew it. Make a new code there.")
                    case "closed": return qsTr("This code expired. On the computer, choose New code.")
                    case "busy": return qsTr("The server is busy. Try again in a minute.")
                    case "failed": return qsTr("Couldn’t pair with %1. Check that it is on and on this network.").arg(sheet.targetName)
                    default: return qsTr("The code is shown on that computer, where pairing is switched on.")
                    }
                }
            }
            Item { Layout.fillHeight: true }
            RowLayout {
                Layout.fillWidth: true
                Button { text: qsTr("Back"); onClicked: { Remote.resetPairing(); sheet.step = "choose" } }
                Item { Layout.fillWidth: true }
                Button {
                    visible: sheet.pairing.state === "withdrawn" || sheet.pairing.state === "closed"
                    text: qsTr("Try a new code")
                    onClicked: { Remote.resetPairing(); code.clear(); code.focusInput() }
                }
            }
        }

        // ---- done
        ColumnLayout {
            visible: sheet.step === "done"
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 14
            Item { Layout.fillHeight: true }
            Icon { Layout.alignment: Qt.AlignHCenter; name: "object-select-symbolic"; size: 40; color: Theme.success }
            Text {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
                text: qsTr("%1 is paired").arg(sheet.targetName)
                font: Theme.font("title"); color: Theme.text
            }
            Text {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
                text: qsTr("Its key is saved on this computer.")
                font: Theme.font("body"); color: Theme.text2
            }
            Item { Layout.fillHeight: true }
            ButtonRow {
                Layout.fillWidth: true
                Button { objectName: "paired-done"; text: qsTr("Done"); onClicked: sheet.close() }
                Button {
                    objectName: "paired-connect"
                    text: qsTr("Connect now")
                    variant: "primary"
                    onClicked: { const id = sheet.targetId; sheet.close(); sheet.connectNow(id) }
                }
            }
        }
    }
}
