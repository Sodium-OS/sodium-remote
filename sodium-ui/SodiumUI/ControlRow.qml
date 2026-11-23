// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts

// A label and its control on one row, the Settings way (decision 20; LangComponents A06/B06): the
// label (13 px) with an optional description under it (12 px, text 2, wraps with no limit) on the
// leading side, the control (a Switch, a DropDownButton, a Slider…) on the trailing side. 11 × 16 px
// padding, 12 px gap, at least 46 px high.
//
//     ControlRow {
//         Layout.fillWidth: true
//         text: qsTr("Show seconds in the bar")
//         description: qsTr("The clock ticks every second.")
//         Switch { checked: Theme.settings.clockSeconds === true }
//     }
//
// **When it doesn't fit**, the label wraps to two lines first, balanced (a WrappedLabel, decision 89). If
// it needs more, or the control would take over half the row, the control moves under the label
// (left-aligned, at its own width). Nothing elides. The name is ControlRow, not SettingRow or RowControl, so Settings' own files keep working.
Item {
    id: root

    /// The label: what the control does.
    property string text: ""
    /// More about it, under the label; wraps with no limit.
    property string description: ""
    /// The control, as the child (one item).
    default property Item control
    /// Lines the label may take beside the control before the control moves under it (2).
    property int labelLines: metrics.labelLines ?? 2

    readonly property var metrics: Theme.metrics.controls.controlRow ?? ({ paddingX: 16, paddingY: 11, gap: 12, minHeight: 46, labelLines: 2, descriptionGap: 2 })
    readonly property real _controlWidth: control && control.visible ? control.implicitWidth : 0
    /// The label's width beside the control.
    readonly property real _besideWidth: Math.max(40, width - 2 * metrics.paddingX - (_controlWidth > 0 ? _controlWidth + metrics.gap : 0))
    /// The control is under the label.
    readonly property bool stacked: width > 0 && _controlWidth > 0 && (probe.lineCount > labelLines || _controlWidth > width / 2)

    implicitWidth: label.implicitWidth + 2 * metrics.paddingX + (_controlWidth > 0 ? _controlWidth + metrics.gap : 0)
    implicitHeight: Math.max(metrics.minHeight, grid.implicitHeight + 2 * metrics.paddingY)

    onControlChanged: if (control) control.parent = controlSlot

    // How many lines the label takes beside the control.
    StyledText {
        id: probe
        visible: false
        width: root._besideWidth
        text: root.text
        style: "body"
        wrapMode: Text.Wrap
    }

    GridLayout {
        id: grid
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        anchors.leftMargin: root.metrics.paddingX
        anchors.rightMargin: root.metrics.paddingX
        columns: root.stacked ? 1 : 2
        columnSpacing: root.metrics.gap
        rowSpacing: 8

        ColumnLayout {
            Layout.fillWidth: true
            Layout.alignment: Qt.AlignVCenter
            spacing: root.metrics.descriptionGap ?? 2

            WrappedLabel {
                id: label
                Layout.fillWidth: true
                text: root.text
                style: "body"
            }
            StyledText {
                Layout.fillWidth: true
                visible: root.description !== ""
                text: root.description
                style: "caption"
                color: Theme.text2
                wrapMode: Text.Wrap
            }
        }
        Item {
            id: controlSlot
            Layout.alignment: root.stacked ? Qt.AlignLeft | Qt.AlignTop : Qt.AlignRight | Qt.AlignVCenter
            implicitWidth: root._controlWidth
            implicitHeight: root.control ? root.control.implicitHeight : 0
            // Always visible: the control's own `visible` is read through it (its width counts only while
            // it is visible). A slot hidden at zero width hid a control whose size was still 0 when the row
            // was made (a Row of Text), and it stayed hidden for good.
            visible: true
            Binding {
                target: root.control
                property: "width"
                value: controlSlot.width
                when: root.control !== null
            }
            Binding {
                target: root.control
                property: "height"
                value: controlSlot.height
                when: root.control !== null
            }
        }
    }
}
