// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import SodiumUI

// One paired computer: the last picture the client saw of it (a placeholder until there is
// one) with its state pill and its system pill on it, the name and its menu, when it was last connected, and the path
// it took with the round trip. Double-click or Return connects; the Connect button on the picture (under the pointer)
// too; the menu button and a right click open the card's menu.
Rectangle {
    id: card

    /// The model row.
    property var computer: ({})
    readonly property bool dimmed: computer.state === "asleep" || computer.state === "offline"
    readonly property bool busy: computer.state === "connecting"
    readonly property string quality: computer.state === "offline" ? "none" : Fmt.routeQuality(computer.route ?? "", computer.rttMs ?? 0)

    signal connectRequested(string hostId, bool fullscreen)
    signal menuRequested(var item, string hostId)

    implicitWidth: 212
    implicitHeight: col.implicitHeight + 20
    radius: 14
    color: hover.hovered || activeFocus ? Theme.selected : Theme.surfaceCard
    border.width: activeFocus ? 2 : 1
    border.color: activeFocus ? Theme.accent : Theme.hairline
    activeFocusOnTab: true
    objectName: "card-" + computer.hostId
    Accessible.role: Accessible.Button
    Accessible.name: computer.name ?? ""
    Accessible.description: Fmt.stateText(computer)

    Behavior on color { ColorAnimation { duration: 120 } }

    HoverHandler { id: hover }
    TapHandler {
        acceptedButtons: Qt.LeftButton
        onDoubleTapped: card.connectRequested(card.computer.hostId, false)
    }
    TapHandler {
        acceptedButtons: Qt.RightButton
        onTapped: card.menuRequested(card, card.computer.hostId)
    }
    Keys.onReturnPressed: connectRequested(computer.hostId, false)
    Keys.onEnterPressed: connectRequested(computer.hostId, false)

    ColumnLayout {
        id: col
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: 8
        spacing: 8

        Rectangle {
            id: picture
            Layout.fillWidth: true
            Layout.preferredHeight: Math.round(width * 10 / 16)
            radius: 10
            clip: true
            gradient: Gradient {
                GradientStop { position: 0; color: Qt.darker(Theme.accent, 3.2) }
                GradientStop { position: 1; color: Qt.darker(Theme.accent, 6) }
            }

            // Until there is a picture of it: the generic screen.
            Icon {
                anchors.centerIn: parent
                visible: shot.status !== Image.Ready
                name: "video-display-symbolic"
                size: 40
                color: Qt.rgba(1, 1, 1, 0.55)
            }
            Image {
                id: shot
                anchors.fill: parent
                source: card.computer.thumb ?? ""
                fillMode: Image.PreserveAspectCrop
                asynchronous: true
                cache: false
                smooth: true
                opacity: card.dimmed ? 0.45 : 1
                Behavior on opacity { NumberAnimation { duration: 120 } }
            }

            StatusPill {
                anchors.left: parent.left
                anchors.top: parent.top
                anchors.margins: 8
                state: card.computer.state ?? "offline"
            }
            OsPill {
                anchors.left: parent.left
                anchors.bottom: parent.bottom
                anchors.margins: 8
                os: card.computer.os ?? ""
            }

            // Pointing at the picture: Connect, on it.
            Rectangle {
                anchors.fill: parent
                visible: card.busy || (hover.hovered && !card.dimmed) || connectButton.activeFocus
                color: Qt.rgba(0, 0, 0, 0.28)
                Spinner { anchors.centerIn: parent; visible: card.busy }
                Button {
                    id: connectButton
                    objectName: "card-connect-" + card.computer.hostId
                    anchors.centerIn: parent
                    visible: !card.busy
                    variant: "primary"
                    text: qsTr("Connect")
                    onClicked: card.connectRequested(card.computer.hostId, false)
                }
            }
        }

        ColumnLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 4
            Layout.rightMargin: 2
            Layout.bottomMargin: 4
            spacing: 2

            RowLayout {
                Layout.fillWidth: true
                spacing: 4
                Text {
                    Layout.fillWidth: true
                    text: card.computer.name ?? ""
                    font: Theme.font("heading")
                    color: Theme.text
                    elide: Text.ElideRight // a computer's own name is user content
                }
                IconButton {
                    objectName: "card-menu-" + card.computer.hostId
                    icon.name: "view-more-horizontal-symbolic"
                    tooltip: qsTr("More for %1").arg(card.computer.name ?? "")
                    onClicked: card.menuRequested(card, card.computer.hostId)
                }
            }
            Text {
                Layout.fillWidth: true
                text: Fmt.stateText(card.computer)
                font: Theme.font("caption")
                color: Theme.text2
                wrapMode: Text.Wrap
            }
            RowLayout {
                Layout.fillWidth: true
                visible: routeText.text !== ""
                spacing: 7
                // Path quality: three rising bars, like Wi-Fi strength; the lit ones follow the path.
                Row {
                    spacing: 2
                    Layout.alignment: Qt.AlignVCenter
                    Repeater {
                        model: 3
                        Rectangle {
                            required property int index
                            width: 3
                            height: 5 + index * 3
                            anchors.bottom: parent.bottom
                            radius: 1
                            color: index < Fmt.qualityBars(card.quality)
                                ? (card.quality === "good" ? Theme.text2 : Theme.warning) : Theme.fill
                        }
                    }
                }
                Text {
                    id: routeText
                    Layout.fillWidth: true
                    text: Fmt.routeChip(card.computer.route ?? "", card.computer.rttMs ?? 0)
                    font: Theme.font("caption")
                    color: Theme.text2
                    wrapMode: Text.Wrap
                }
            }
        }
    }
}
