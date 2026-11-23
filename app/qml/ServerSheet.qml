// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import SodiumUI

// Add a server, Change the server, and the confirmation every link and every imported file passes before anything is saved
//.
// Typed settings (the details by hand) are saved at once: the person has just entered every value. A pasted or opened link and an
// imported server.toml are somebody else's words, so they show what they say first: the host, whether a certificate comes
// with them and whether a token is inside.
Sheet {
    id: sheet

    /// link | manual | confirm
    property string stage: "link"
    property bool changing: false
    property string link: ""
    property string address: ""
    property string caName: ""
    property string caPem: ""
    property string token: ""
    /// The server in the confirmation: Remote.checkServer*'s reply, with `source` link or file.
    property var candidate: ({})
    /// "" or notALink, insecure, address, certificate, saving
    property string problem: ""

    signal chooseCertificate()
    signal saved()

    width: Math.min(520, parent ? parent.width - 32 : 520)   // inside the window at its 480 px minimum
    height: Math.min(Math.max(body.implicitHeight + 48, 300), (parent ? parent.height : 520) - 72)

    function clearFields() {
        problem = ""; link = ""; address = ""; caName = ""; caPem = ""; token = ""; candidate = ({}); changing = false
    }
    function beginAdd() { clearFields(); stage = "link"; open(); linkField.forceActiveFocus() }
    function beginChange() {
        clearFields()
        changing = true
        stage = "manual"
        const s = Remote.server
        address = s.url ?? ""
        token = s.tokenText ?? ""
        caPem = Remote.serverCertificate()
        caName = caPem !== "" ? qsTr("Current certificate") : ""
        open()
    }
    /// A link from outside the app, or pasted: its confirmation (a link that isn't one lands on the link field with the error).
    function beginLink(text) {
        clearFields()
        link = text.trim()
        const c = Remote.checkServerLink(link)
        if (c.ok) {
            c.source = "link"
            candidate = c
            stage = "confirm"
        } else {
            stage = "link"
            problem = "notALink"
        }
        open()
    }
    /// An imported server.toml's server (already checked).
    function beginConfirm(c) {
        clearFields()
        c.source = "file"
        candidate = c
        stage = "confirm"
        open()
    }
    function setCertificate(name, pem) { caName = name; caPem = pem; problem = "" }

    readonly property bool canSubmit: stage === "link" ? link.trim() !== "" : stage === "manual" ? address.trim() !== "" : !!candidate.ok

    function submit() {
        if (!canSubmit)
            return
        if (stage === "link") {
            const c = Remote.checkServerLink(link)
            if (!c.ok) { problem = "notALink"; return }
            problem = ""
            c.source = "link"
            candidate = c
            stage = "confirm"
        } else if (stage === "manual") {
            const c = Remote.checkServer(address, caPem, token)
            if (!c.ok) { problem = c.error; return }
            save(c.link)
        } else {
            save(candidate.link)
        }
    }
    function save(l) {
        const r = Remote.saveServer(l)
        if (r.ok) {
            close()
            saved()
        } else {
            problem = "saving"
        }
    }

    ColumnLayout {
        id: body
        anchors.fill: parent
        anchors.margins: 24
        spacing: 14

        ColumnLayout {
            spacing: 2
            Text {
                Layout.fillWidth: true
                objectName: "server-sheet-title"
                text: sheet.stage === "confirm" ? qsTr("Add %1?").arg(sheet.candidate.official ? qsTr("Sodium Remote service") : (sheet.candidate.host ?? ""))
                    : sheet.changing ? qsTr("Change the server") : qsTr("Add a server")
                font: Theme.font("title"); color: Theme.text; wrapMode: Text.Wrap
            }
            Text {
                visible: sheet.stage !== "confirm"
                Layout.fillWidth: true
                text: sheet.stage === "link" ? qsTr("Paste the link from the person who runs the server.")
                    : sheet.changing ? qsTr("Enter the details of the new server. Paired computers stay paired.")
                    : qsTr("Enter the details the person who runs the server gave you.")
                font: Theme.font("body"); color: Theme.text2; wrapMode: Text.Wrap
            }
        }

        // ---- link
        ColumnLayout {
            visible: sheet.stage === "link"
            Layout.fillWidth: true
            spacing: 6
            Text { text: qsTr("Server link"); font: Theme.font("caption", { weight: 600 }); color: Theme.text2 }
            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                TextField {
                    id: linkField
                    objectName: "server-link-field"
                    Layout.fillWidth: true
                    text: sheet.link
                    placeholderText: "sodium-remote://server?…"
                    error: sheet.problem === "notALink"
                    errorText: qsTr("That isn’t a Sodium Remote server link.")
                    font: Theme.font("mono")
                    Accessible.name: qsTr("Server link")
                    onTextEdited: { sheet.link = text; sheet.problem = "" }
                    onAccepted: sheet.submit()
                }
                Button {
                    objectName: "server-paste"
                    text: qsTr("Paste")
                    onClicked: { linkField.clear(); linkField.paste(); sheet.link = linkField.text.trim(); sheet.problem = "" }
                }
            }
            RowLayout {
                Layout.fillWidth: true
                Layout.topMargin: sheet.problem === "notALink" ? 14 : 0
                Item { Layout.fillWidth: true }
                Button {
                    objectName: "server-by-hand"
                    variant: "ghost"
                    text: qsTr("Enter the details yourself…")
                    palette.buttonText: Theme.accentInk
                    onClicked: { sheet.problem = ""; sheet.stage = "manual" }
                }
            }
        }

        // ---- the details by hand
        ColumnLayout {
            visible: sheet.stage === "manual"
            Layout.fillWidth: true
            spacing: 14
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 6
                Text { text: qsTr("Server address"); font: Theme.font("caption", { weight: 600 }); color: Theme.text2 }
                TextField {
                    objectName: "server-address-field"
                    Layout.fillWidth: true
                    text: sheet.address
                    placeholderText: "relay.example.org"
                    error: sheet.problem === "address" || sheet.problem === "insecure"
                    errorText: sheet.problem === "insecure" ? qsTr("Sodium Remote only connects to a server over an encrypted connection. Use an address that starts with https:// or wss://, or just the server’s name.")
                                                            : qsTr("That isn’t a valid server address.")
                    font: Theme.font("mono")
                    Accessible.name: qsTr("Server address")
                    onTextEdited: { sheet.address = text; sheet.problem = "" }
                    onAccepted: sheet.submit()
                }
            }
            ColumnLayout {
                Layout.fillWidth: true
                Layout.topMargin: sheet.problem === "address" || sheet.problem === "insecure" ? 22 : 0
                spacing: 6
                Text { text: qsTr("Certificate authority file"); font: Theme.font("caption", { weight: 600 }); color: Theme.text2 }
                Rectangle {
                    Layout.fillWidth: true
                    implicitHeight: Math.max(36, caRow.implicitHeight + 8)
                    radius: 10
                    color: Theme.fill
                    border.width: sheet.problem === "certificate" ? 1.5 : 0
                    border.color: Theme.warning
                    RowLayout {
                        id: caRow
                        anchors.fill: parent
                        anchors.leftMargin: 12
                        anchors.rightMargin: 4
                        spacing: 6
                        Text {
                            objectName: "server-ca-name"
                            Layout.fillWidth: true
                            text: sheet.caName !== "" ? sheet.caName : qsTr("None chosen")
                            font: Theme.font("body"); color: sheet.caName !== "" ? Theme.text : Theme.text3; wrapMode: Text.WrapAnywhere
                        }
                        IconButton {
                            visible: sheet.caName !== ""
                            icon.name: "window-close-symbolic"
                            tooltip: qsTr("Remove")
                            onClicked: sheet.setCertificate("", "")
                        }
                        Button { objectName: "server-ca-choose"; compact: true; text: qsTr("Choose a file…"); onClicked: sheet.chooseCertificate() }
                    }
                }
                Text {
                    Layout.fillWidth: true
                    text: sheet.problem === "certificate" ? qsTr("That file doesn’t hold a certificate. Choose the certificate authority’s PEM file.")
                                                          : qsTr("Only if the server uses its own certificate.")
                    font: Theme.font("caption"); color: sheet.problem === "certificate" ? Theme.warning : Theme.text2; wrapMode: Text.Wrap
                }
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 6
                Text { text: qsTr("Enrollment token"); font: Theme.font("caption", { weight: 600 }); color: Theme.text2 }
                // A token is a secret but not a password: the field hides it and keeps input methods from learning it.
                TextField {
                    objectName: "server-token-field"
                    Layout.fillWidth: true
                    text: sheet.token
                    placeholderText: qsTr("Optional")
                    echoMode: TextInput.Password
                    inputMethodHints: Qt.ImhHiddenText | Qt.ImhNoPredictiveText | Qt.ImhSensitiveData | Qt.ImhNoAutoUppercase
                    font: Theme.font("mono")
                    Accessible.name: qsTr("Enrollment token")
                    onTextEdited: sheet.token = text
                    onAccepted: sheet.submit()
                }
                Text { Layout.fillWidth: true; text: qsTr("Only if the server asks for one."); font: Theme.font("caption"); color: Theme.text2; wrapMode: Text.Wrap }
            }
        }

        // ---- the confirmation
        ColumnLayout {
            visible: sheet.stage === "confirm"
            Layout.fillWidth: true
            spacing: 10
            SettingsRowGroup {
                Layout.fillWidth: true
                title: sheet.candidate.source === "file" ? qsTr("Server in the file") : qsTr("Server in the link")
                ControlRow {
                    visible: !sheet.candidate.official
                    Layout.fillWidth: true
                    text: qsTr("Address")
                    Text { objectName: "confirm-address"; text: sheet.candidate.url ?? ""; font: Theme.font("mono"); color: Theme.text; wrapMode: Text.WrapAnywhere; Layout.maximumWidth: 260 }
                }
                ControlRow {
                    Layout.fillWidth: true
                    text: qsTr("Certificate")
                    Text {
                        objectName: "confirm-cert"
                        text: !sheet.candidate.needs_ca_prompt ? qsTr("Public") : sheet.candidate.source === "file" ? qsTr("Pinned by the file") : qsTr("Pinned by the link")
                        font: Theme.font("body"); color: sheet.candidate.needs_ca_prompt ? Theme.warning : Theme.text2
                    }
                }
                ControlRow {
                    visible: sheet.candidate.token ?? false
                    Layout.fillWidth: true
                    text: qsTr("Enrollment token")
                    Text { text: sheet.candidate.source === "file" ? qsTr("Included") : qsTr("Included in the link"); font: Theme.font("body"); color: Theme.text2 }
                }
            }
            RowLayout {
                visible: sheet.candidate.needs_ca_prompt ?? false
                objectName: "confirm-ca-warning"
                Layout.fillWidth: true
                spacing: 8
                Icon { Layout.alignment: Qt.AlignTop; name: "dialog-warning-symbolic"; size: 14; color: Theme.warning }
                Text {
                    Layout.fillWidth: true
                    text: sheet.candidate.source === "file" ? qsTr("Sodium Remote will trust this certificate for this server only.")
                        : qsTr("This link carries its own certificate. Sodium Remote will trust it for this server only, so continue only if you trust whoever sent the link.")
                    font: Theme.font("caption"); color: Theme.text; wrapMode: Text.Wrap
                }
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                Icon { Layout.alignment: Qt.AlignTop; name: "dialog-information-symbolic"; size: 14; color: Theme.text3 }
                Text {
                    Layout.fillWidth: true
                    text: qsTr("Only add a server you trust. It can see which computers connect, and when.")
                    font: Theme.font("caption"); color: Theme.text2; wrapMode: Text.Wrap
                }
            }
        }

        Text {
            visible: sheet.problem === "saving"
            Layout.fillWidth: true
            text: qsTr("Couldn’t save the server settings.")
            font: Theme.font("caption"); color: Theme.danger; wrapMode: Text.Wrap
        }

        ButtonRow {
            Layout.fillWidth: true
            Layout.topMargin: 4
            Button {
                visible: sheet.stage === "manual" && !sheet.changing
                objectName: "server-back"
                variant: "ghost"
                text: qsTr("Back to the link")
                palette.buttonText: Theme.accentInk
                onClicked: { sheet.problem = ""; sheet.stage = "link" }
            }
            Button { objectName: "server-cancel"; text: qsTr("Cancel"); onClicked: sheet.close() }
            Button {
                objectName: "server-accept"
                variant: "primary"
                enabled: sheet.canSubmit
                text: sheet.changing ? qsTr("Save") : qsTr("Add")
                onClicked: sheet.submit()
            }
        }
    }
}
