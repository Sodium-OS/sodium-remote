// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts

// The status strip at the bottom of a window (Files on the Desktop board, Task
// manager): 30 px, 16 px padding, 11.5 px #9EA4B2, a 1 px divider above,
// spanning the full width.
//
//     StatusStrip { leftText: "8 items"; rightText: "1.4 TB free" }
//     StatusStrip { Text { ... } Item { Layout.fillWidth: true } ProgressBar { ... } }
//
// Every language (decision 20, LangComponents A14/B14): when its two ends don't fit side by side, they
// wrap onto two lines and the strip grows from 30 px. The leading text (names of files and selections)
// is user content and may elide on its own line.
Item {
    id: strip

    property string leftText: ""
    property string rightText: ""
    /// Custom content in a RowLayout (replaces the two texts).
    default property alias content: row.data
    readonly property var metrics: Theme.metrics.statusStrip
    /// The two texts don't fit side by side: they are on two lines.
    readonly property bool wrapped: row.children.length === 0 && leftText !== "" && rightText !== ""
                                    && left.implicitWidth + right.implicitWidth + 16 > width - 2 * metrics.paddingX

    implicitHeight: wrapped ? Math.max(metrics.height, texts.implicitHeight + 14) : metrics.height
    implicitWidth: 320

    Rectangle {
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        height: 1
        color: Theme.divider
    }

    GridLayout {
        id: texts
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        anchors.leftMargin: strip.metrics.paddingX
        anchors.rightMargin: strip.metrics.paddingX
        visible: row.children.length === 0
        columns: strip.wrapped ? 1 : 2
        columnSpacing: 16
        rowSpacing: 2

        StyledText {
            id: left
            Layout.fillWidth: true
            text: strip.leftText
            style: "status"
            color: Theme.text3
            elide: Text.ElideRight
            I18nAudit.userContent: true   // the elidable side: names of files and selections
        }
        StyledText {
            id: right
            Layout.alignment: strip.wrapped ? Qt.AlignLeft : Qt.AlignRight
            Layout.fillWidth: strip.wrapped
            text: strip.rightText
            style: "status"
            color: Theme.text3
            wrapMode: Text.Wrap
        }
    }

    RowLayout {
        id: row
        anchors.fill: parent
        anchors.leftMargin: strip.metrics.paddingX
        anchors.rightMargin: strip.metrics.paddingX
        spacing: 12
    }
}
