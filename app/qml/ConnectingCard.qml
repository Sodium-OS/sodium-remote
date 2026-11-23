// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import SodiumUI

// The connecting state: four steps; the engine tells us when the handshake is done and when the first picture is up.
Rectangle {
    id: card

    property string hostName: ""
    /// 0: finding it, 1: connected and waiting for the picture.
    property int phase: 0

    signal cancelled()

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
        spacing: 14

        Spinner { Layout.alignment: Qt.AlignHCenter; size: 28 }
        WrappedLabel {
            objectName: "connecting-title"
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            text: qsTr("Connecting to %1").arg(card.hostName)
            font: Theme.font("heading")
            color: Theme.text
        }
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 8
            Repeater {
                model: [
                    { text: qsTr("Find %1").arg(card.hostName), done: card.phase >= 1 },
                    { text: qsTr("Pick the fastest path"), done: card.phase >= 1 },
                    { text: qsTr("Check its key"), done: card.phase >= 1 },
                    { text: qsTr("Start the stream"), done: false }
                ]
                RowLayout {
                    required property var modelData
                    required property int index
                    Layout.fillWidth: true
                    spacing: 10
                    readonly property bool active: !modelData.done && (index === 0 || card.phase >= index)
                    Icon { name: modelData.done ? "object-select-symbolic" : "media-record-symbolic"; size: 14; color: modelData.done ? Theme.success : parent.active ? Theme.accent : Theme.text3 }
                    Text {
                        Layout.fillWidth: true
                        text: modelData.text
                        wrapMode: Text.Wrap
                        font: Theme.font("body")
                        color: modelData.done || parent.active ? Theme.text : Theme.text3
                    }
                }
            }
        }
        ButtonRow {
            Layout.fillWidth: true
            Layout.topMargin: 4
            Button { objectName: "connecting-cancel"; text: qsTr("Cancel"); onClicked: card.cancelled() }
        }
    }
}
