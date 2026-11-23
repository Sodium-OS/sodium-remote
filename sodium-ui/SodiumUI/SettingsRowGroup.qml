// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts

// A titled group of rows (Settings' group): the 12 px uppercase header with an optional link at its trailing end,
// a raised panel of rows (ControlRow...) with hairlines between them, and a 12 px foot note.
//
//     SettingsRowGroup { title: qsTr("Reading"); footer: qsTr("…")
//         ControlRow { text: qsTr("Show preview"); Switch { } }
//     }
ColumnLayout {
    id: group

    property string title: ""
    property string footer: ""
    /// A link in the header ("Add address…"); empty hides it.
    property string linkText: ""
    property string linkIcon: ""
    signal linkClicked()
    default property alias rows: panel.data

    Layout.fillWidth: true
    spacing: 0

    RowLayout {
        visible: group.title !== "" || group.linkText !== ""
        Layout.fillWidth: true
        Layout.leftMargin: 4
        Layout.rightMargin: 4
        Layout.bottomMargin: 8
        StyledText { Layout.fillWidth: true; text: group.title; style: "label"; color: Theme.text3; wrapMode: Text.Wrap }
        Button {
            visible: group.linkText !== ""
            variant: "ghost"
            compact: true
            Layout.rightMargin: -8
            text: group.linkText
            icon.name: group.linkIcon
            palette.buttonText: Theme.accentInk
            onClicked: group.linkClicked()
        }
    }
    Rectangle {
        Layout.fillWidth: true
        implicitHeight: panel.implicitHeight
        radius: Theme.corner(14)
        color: Theme.raised
        border.width: 1
        border.color: Theme.hairline
        clip: true
        ColumnLayout {
            id: panel
            width: parent.width
            spacing: 0
        }
        Repeater {
            model: Math.max(0, panel.visibleChildren.length - 1)
            delegate: Rectangle {
                required property int index
                y: panel.visibleChildren[index + 1] ? panel.visibleChildren[index + 1].y : 0
                width: parent.width
                height: 1
                color: Theme.hairline
            }
        }
    }
    StyledText {
        visible: group.footer !== ""
        Layout.fillWidth: true
        Layout.leftMargin: 4
        Layout.rightMargin: 4
        Layout.topMargin: 8
        text: group.footer
        style: "caption"
        color: Theme.text2
        wrapMode: Text.Wrap
    }
}
