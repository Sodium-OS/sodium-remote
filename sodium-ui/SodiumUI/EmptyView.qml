// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts

// An empty or failed pane (P2Dialogs G10, G11), centered in the space it fills: a 48 px
// icon circle, a 15 / 600 title, one line of help in text 2 (240 px wide at most), and
// at most one or two actions. With `error` the circle is danger-tinted, and the title
// names the thing that failed; the actions retry or explain.
//
//     EmptyView {
//         anchors.fill: parent
//         iconName: "folder-symbolic"
//         title: qsTr("This folder is empty")
//         text: qsTr("Drop files here, or make something new.")
//         Button { text: qsTr("New folder"); compact: true }
//     }
//     EmptyView {
//         anchors.fill: parent
//         error: true                                   // icon defaults to dialog-error
//         title: qsTr("Can’t open “Projects”")
//         text: qsTr("You do not have permission to read this folder.")
//         Button { text: qsTr("Try again"); compact: true; onClicked: reload() }
//     }
//
// The name is EmptyView, not EmptyState, so apps with an EmptyState.qml keep theirs.
Item {
    id: view

    property string iconName: error ? "dialog-error-symbolic" : ""
    property string title: ""
    property string text: ""
    /// The danger-tinted error pane (G11).
    property bool error: false
    /// Its actions: Buttons (compact secondary on the boards).
    default property alias actions: actionRow.data
    readonly property var metrics: Theme.metrics.controls.emptyView ?? ({ icon: 48, glyph: 22, gap: 8, textWidth: 240, padding: 20 })

    implicitWidth: column.implicitWidth + 2 * metrics.padding
    implicitHeight: column.implicitHeight

    ColumnLayout {
        id: column
        anchors.centerIn: parent
        width: Math.min(implicitWidth, view.width - 2 * view.metrics.padding)
        spacing: view.metrics.gap

        Rectangle {
            Layout.alignment: Qt.AlignHCenter
            visible: view.iconName !== ""
            implicitWidth: view.metrics.icon
            implicitHeight: view.metrics.icon
            radius: width / 2
            color: view.error ? Theme.dangerSoft : Theme.fill
            Icon {
                anchors.centerIn: parent
                name: view.iconName
                size: view.metrics.glyph
                color: view.error ? Theme.dangerText : Theme.text2
            }
        }
        WrappedLabel {
            Layout.alignment: Qt.AlignHCenter
            Layout.topMargin: 2
            Layout.maximumWidth: Math.max(0, view.width - 2 * view.metrics.padding)
            visible: view.title !== ""
            text: view.title
            style: "heading"
            color: Theme.text
            horizontalAlignment: Text.AlignHCenter
        }
        StyledText {
            Layout.alignment: Qt.AlignHCenter
            Layout.maximumWidth: Math.min(view.metrics.textWidth, Math.max(0, view.width - 2 * view.metrics.padding))
            visible: view.text !== ""
            text: I18n.joinPhrases(view.text)      // zh and ja phrase marks (U+200B) hold a phrase together, as WrappedLabel does
            Accessible.name: I18n.stripBreakMarks(view.text)
            style: "small"
            latinLineRatio: 1.5           // G10's 12.5 / 19
            color: Theme.text2
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.Wrap
        }
        RowLayout {
            id: actionRow
            Layout.alignment: Qt.AlignHCenter
            Layout.topMargin: 6
            visible: children.length > 0
            spacing: 8
        }
    }
}
