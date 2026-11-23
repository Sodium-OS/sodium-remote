// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts

// A banner above content that is still usable (P2Dialogs G12): radius 10, 8 x 10
// padding, a 16 px icon, the message in the text color, and an optional 28 px
// action, on a soft status tint.
//
//     Banner {
//         Layout.fillWidth: true; Layout.margins: 12
//         text: qsTr("The network drive stopped answering.")
//         actionText: qsTr("Retry")
//         onActionTriggered: drive.retry()
//     }
//
// kind: warning (drawn: warning 14% with the warning icon), error (the danger tint,
// as G11's icon circle) and info (white 7%, text 2). error and info are PROVISIONAL:
// G12 draws only the warning.
//
// More actions go after the first as children, the same 28 px compact buttons:
//
//     Banner {
//         text: qsTr("This file isn’t UTF-8 text. It’s open read-only so nothing is lost.")
//         actionText: qsTr("Convert to UTF-8")
//         Button { compact: true; implicitHeight: 28; text: qsTr("Open with…") }
//     }
Rectangle {
    id: banner

    property string text: ""
    /// warning | error | info
    property string kind: "warning"
    property string iconName: kind === "error" ? "dialog-error-symbolic" : kind === "info" ? "dialog-information-symbolic" : "dialog-warning-symbolic"
    /// The action's label; "" for none.
    property string actionText: ""
    /// Further actions (Buttons), after the first.
    default property alias actions: moreActions.data
    readonly property var metrics: Theme.metrics.controls.banner ?? ({ radius: 10, paddingX: 10, paddingY: 8, gap: 8, icon: 16, button: 28 })
    readonly property color tone: kind === "error" ? Theme.danger : kind === "info" ? Theme.text2 : Theme.warning

    /// An optional text link at the right instead of a button (accent, 13 px semibold); "" for none.
    property string linkText: ""

    signal actionTriggered()
    signal linkActivated()

    implicitWidth: row.implicitWidth + 2 * metrics.paddingX
    implicitHeight: Math.max(row.implicitHeight, metrics.button) + 2 * metrics.paddingY
    radius: metrics.radius
    color: kind === "error" ? Theme.dangerSoft : kind === "info" ? Theme.fill : Theme.warningSoft
    Accessible.role: Accessible.AlertMessage
    Accessible.name: I18n.stripBreakMarks(text)

    RowLayout {
        id: row
        anchors.fill: parent
        anchors.leftMargin: banner.metrics.paddingX
        anchors.rightMargin: banner.metrics.paddingX
        spacing: banner.metrics.gap

        Icon {
            Layout.alignment: Qt.AlignVCenter
            name: banner.iconName
            size: banner.metrics.icon
            color: banner.tone
        }
        StyledText {
            Layout.fillWidth: true
            Layout.alignment: Qt.AlignVCenter
            text: banner.text
            style: "small"
            color: Theme.text
            wrapMode: Text.Wrap
        }
        Button {
            Layout.alignment: Qt.AlignVCenter
            visible: banner.actionText !== ""
            text: banner.actionText
            compact: true
            implicitHeight: banner.metrics.button
            onClicked: banner.actionTriggered()
        }
        Text {
            Layout.alignment: Qt.AlignVCenter
            visible: banner.linkText !== ""
            text: banner.linkText
            font: Theme.font("small", { size: 13, weight: 600 })
            color: linkHover.hovered ? Theme.text : Theme.accentInk
            HoverHandler { id: linkHover; cursorShape: Qt.PointingHandCursor }
            TapHandler { onTapped: banner.linkActivated() }
            Accessible.role: Accessible.Link
            Accessible.name: I18n.stripBreakMarks(text)
        }
        RowLayout {
            id: moreActions
            Layout.alignment: Qt.AlignVCenter
            visible: children.length > 0
            spacing: banner.metrics.gap
        }
    }
}
