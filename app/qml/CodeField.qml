// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import SodiumUI

// The 6-digit pairing code: six boxes over one hidden field (so paste works), checked by itself at the sixth digit.
FocusScope {
    id: field

    property bool error: false
    property bool busy: false
    readonly property alias text: input.text
    signal submitted(string code)

    implicitWidth: row.implicitWidth
    implicitHeight: 54
    activeFocusOnTab: true

    function clear() { input.text = "" }
    function focusInput() { input.forceActiveFocus() }

    TextInput {
        id: input
        objectName: "code-input"
        focus: true
        width: 1; height: 1; opacity: 0
        maximumLength: 6
        inputMethodHints: Qt.ImhDigitsOnly
        validator: RegularExpressionValidator { regularExpression: /[0-9]{0,6}/ }
        enabled: !field.busy
        Accessible.name: qsTr("Pairing code")
        onTextChanged: if (text.length === 6) field.submitted(text)
    }

    MouseArea { anchors.fill: parent; onClicked: input.forceActiveFocus() }

    Row {
        id: row
        spacing: 8
        Repeater {
            model: 6
            Rectangle {
                required property int index
                width: 42; height: 54
                radius: 10
                color: Theme.fill
                border.width: input.activeFocus && input.text.length === index ? 2 : 1
                border.color: field.error ? Theme.danger : input.activeFocus && input.text.length === index ? Theme.accent : Theme.hairline
                Text {
                    anchors.centerIn: parent
                    text: index < input.text.length ? input.text.charAt(index) : ""
                    font: Theme.font("title")
                    color: Theme.text
                }
            }
        }
    }
}
