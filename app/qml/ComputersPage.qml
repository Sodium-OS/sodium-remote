// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import SodiumUI

// The Computers page: a grid of cards, or, with none paired, the empty state with the computers nearby.
Item {
    id: page

    signal connectRequested(string hostId, bool fullscreen)
    signal addRequested()
    signal pairRequested(string hostId)
    signal renameRequested(string hostId)
    signal forgetRequested(string hostId)

    readonly property int cardWidth: 212
    readonly property int gap: 14
    readonly property int columns: Math.max(1, Math.floor((width - 56 + gap) / (cardWidth + gap)))

    /// For screenshots and tests: the card menu of a computer.
    function closeMenus() { cardMenu.close() }
    function menuAtHost(id) { cardMenu.hostId = id; cardMenu.popup(page, 40, 150) }

    Flickable {
        id: flick
        anchors.fill: parent
        contentWidth: width
        contentHeight: Math.max(height, content.implicitHeight + 40)
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        ScrollBar.vertical: ScrollBar { }

        ColumnLayout {
            id: content
            x: 28
            y: 20
            width: flick.width - 56
            spacing: 18

            // With computers: the heading and the grid.
            RowLayout {
                Layout.fillWidth: true
                Layout.bottomMargin: -6
                visible: Remote.computers.count > 0
                spacing: 12
                Text {
                    text: qsTr("Computers")
                    font: Theme.font("section")
                    color: Theme.label
                }
                Text {
                    Layout.fillWidth: true
                    horizontalAlignment: Text.AlignRight
                    wrapMode: Text.Wrap
                    text: qsTr("Double-click or Return connects")
                    font: Theme.font("caption")
                    color: Theme.text3
                }
            }
            Flow {
                Layout.fillWidth: true
                visible: Remote.computers.count > 0
                spacing: page.gap
                Repeater {
                    model: Remote.computers
                    ComputerCard {
                        required property var model
                        width: page.cardWidth
                        computer: model
                        onConnectRequested: (id, fs) => page.connectRequested(id, fs)
                        onMenuRequested: (item, id) => cardMenu.openFor(item, id)
                    }
                }
            }

            // Without: the empty state.
            ColumnLayout {
                Layout.fillWidth: true
                Layout.topMargin: 16
                visible: Remote.computers.count === 0
                spacing: 8
                Icon { Layout.alignment: Qt.AlignHCenter; name: "video-display-symbolic"; size: 44; color: Theme.text3 }
                WrappedLabel {
                    Layout.fillWidth: true
                    horizontalAlignment: Text.AlignHCenter
                    text: qsTr("No computers yet")
                    font: Theme.font("heading")
                    color: Theme.text
                }
                Text {
                    Layout.fillWidth: true
                    Layout.maximumWidth: 420
                    Layout.alignment: Qt.AlignHCenter
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                    text: qsTr("Add a computer on this network, or type its address.")
                    font: Theme.font("body")
                    color: Theme.text2
                }
                Button {
                    objectName: "empty-add"
                    Layout.alignment: Qt.AlignHCenter
                    variant: "primary"
                    text: qsTr("Add a computer")
                    onClicked: page.addRequested()
                }
            }

            // Computers nearby that aren't paired yet.
            ColumnLayout {
                Layout.fillWidth: true
                visible: Remote.nearby.count > 0
                spacing: 8
                Text {
                    text: qsTr("On this network")
                    font: Theme.font("section")
                    color: Theme.label
                }
                Repeater {
                    model: Remote.nearby
                    Rectangle {
                        id: row
                        required property var model
                        Layout.fillWidth: true
                        Layout.maximumWidth: 520
                        implicitHeight: 52
                        radius: 12
                        color: Theme.surfaceCard
                        border.width: 1
                        border.color: Theme.hairline
                        RowLayout {
                            anchors.fill: parent
                            anchors.leftMargin: 14
                            anchors.rightMargin: 10
                            spacing: 10
                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: 0
                                Text { Layout.fillWidth: true; text: row.model.name; font: Theme.font("body"); color: Theme.text; elide: Text.ElideRight }
                                Text { Layout.fillWidth: true; text: qsTr("Found nearby · not paired yet"); font: Theme.font("caption"); color: Theme.text2; wrapMode: Text.Wrap }
                            }
                            Button {
                                objectName: "nearby-pair-" + row.model.hostId
                                compact: true
                                text: qsTr("Pair…")
                                onClicked: page.pairRequested(row.model.hostId)
                            }
                        }
                    }
                }
            }
        }
    }

    // The card's menu.
    Menu {
        id: cardMenu
        property string hostId: ""
        function openFor(item, id) {
            hostId = id
            popup(item, 8, item.height - 8)
        }
        MenuItem { text: qsTr("Connect"); onTriggered: page.connectRequested(cardMenu.hostId, false) }
        MenuItem { text: qsTr("Connect in full screen"); onTriggered: page.connectRequested(cardMenu.hostId, true) }
        MenuSeparator { }
        MenuItem { text: qsTr("Rename…"); onTriggered: page.renameRequested(cardMenu.hostId) }
        MenuItem {
            text: qsTr("Copy ID")
            onTriggered: {
                idCopy.text = Remote.computers.get(cardMenu.hostId).code ?? ""
                idCopy.selectAll()
                idCopy.copy()
            }
        }
        MenuSeparator { }
        MenuItem { text: qsTr("Forget this computer…"); destructive: true; onTriggered: page.forgetRequested(cardMenu.hostId) }
    }
    TextInput { id: idCopy; visible: false }
}
