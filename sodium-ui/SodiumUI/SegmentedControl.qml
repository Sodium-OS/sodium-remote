// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Templates as T

// Segmented control (Components, Calendar's Day / Week / Month, Calculator's
// modes and DEG / RAD, Settings): a white 8% well, radius 10, 3 px padding;
// the current segment is white 16% with white 600 text, the others #D5D9E2.
// The selection slides on the snappy spring. Also the boards' radio group.
//
//     SegmentedControl { model: ["Day", "Week", "Month"]; currentIndex: 2; onActivated: i => view = i }
//
// Left / Right move the selection when it has focus. States (Components ·
// states V1): another segment shows white 6% on hover and 10% pressed; the
// current one slides there on release. Keyboard focus: a 2 px ring 2 px
// outside the well. Disabled: 45% opacity.
//
// Every language (decision 20, LangComponents A04/B04): its implicitWidth is what the segments need;
// given less (a layout squeezing it), it **turns into a DropDownButton** showing the chosen option, and
// back when there is room again (`collapsed`). Segment labels never elide.
T.Control {
    id: control

    property var model: []
    property int currentIndex: 0
    /// Accessible role of the options: radio (a choice) or tab (switches a view).
    property bool tabs: false
    property string previewState: ""

    /// The user picked a segment (click, arrow key, or the dropdown it turns into).
    signal activated(int index)

    readonly property var metrics: Theme.metrics.controls.segmented
    readonly property bool showFocus: previewState === "focus" || visualFocus || (Theme.focusRingAlways && activeFocus)
    /// The width the segments need.
    readonly property real segmentsWidth: row.implicitWidth + leftPadding + rightPadding
    /// The segments don't fit the width it has: it shows a dropdown instead.
    readonly property bool collapsed: width > 0 && width + 0.5 < segmentsWidth
    /// The dropdown it turns into (for tests and screenshots).
    readonly property alias dropDown: dropDown

    function select(i) {
        if (i < 0 || i >= repeater.count || i === currentIndex)
            return
        currentIndex = i
        activated(i)
    }

    implicitWidth: segmentsWidth
    implicitHeight: metrics.height + topPadding + bottomPadding
    padding: metrics.padding
    focusPolicy: collapsed ? Qt.NoFocus : Qt.StrongFocus
    hoverEnabled: true
    font: Theme.font("small")
    opacity: enabled ? 1 : 0.45
    Accessible.role: tabs ? Accessible.PageTabList : Accessible.RadioButton

    Keys.onLeftPressed: select(currentIndex - (mirrored ? -1 : 1))
    Keys.onRightPressed: select(currentIndex + (mirrored ? -1 : 1))

    background: Rectangle {
        visible: !control.collapsed
        radius: control.metrics.outerRadius
        color: Theme.segmentWell

        FocusRing {
            shown: control.showFocus
            controlRadius: parent.radius
        }
    }

    contentItem: Item {
        implicitWidth: row.implicitWidth
        implicitHeight: control.metrics.height
        visible: !control.collapsed

        Spring { id: selX; to: row.currentItem ? row.currentItem.x : 0; preset: "snappy" }
        Spring { id: selW; to: row.currentItem ? row.currentItem.width : 0; preset: "snappy" }

        Rectangle {
            x: selX.value
            width: selW.value
            height: parent.height
            radius: control.metrics.radius
            color: Theme.segmentCurrent
            visible: row.currentItem !== null
        }

        Row {
            id: row

            readonly property Item currentItem: repeater.count > control.currentIndex && control.currentIndex >= 0
                                                ? repeater.itemAt(control.currentIndex) : null
            height: parent.height
            spacing: 2

            Repeater {
                id: repeater
                model: control.model

                delegate: T.AbstractButton {
                    id: seg

                    required property int index
                    required property var modelData
                    readonly property bool current: index === control.currentIndex

                    height: control.metrics.height
                    implicitWidth: label.implicitWidth + 24
                    hoverEnabled: true
                    focusPolicy: Qt.NoFocus
                    Accessible.role: control.tabs ? Accessible.PageTab : Accessible.RadioButton
                    Accessible.name: I18n.stripBreakMarks(String(modelData))
                    Accessible.checked: current
                    onClicked: control.select(index)

                    readonly property bool preview: control.previewState !== "" && seg.index === (control.currentIndex + 1) % repeater.count
                    background: StateFill {
                        radius: control.metrics.radius
                        hover: seg.current ? "transparent" : Theme.segmentHover
                        pressed: seg.current ? "transparent" : Theme.segmentPressed
                        hovered: control.enabled && (seg.hovered || (seg.preview && control.previewState === "hover"))
                        down: control.enabled && (seg.down || (seg.preview && control.previewState === "pressed"))
                    }
                    contentItem: StyledText {
                        id: label
                        text: String(seg.modelData)
                        style: "small"
                        font: Theme.font("small", { weight: seg.current ? 600 : 400 })
                        color: seg.current ? Theme.text : Theme.menuText
                        horizontalAlignment: Text.AlignHCenter
                        verticalAlignment: Text.AlignVCenter
                    }
                }
            }
        }
    }

    DropDownButton {
        id: dropDown
        visible: control.collapsed
        anchors.verticalCenter: parent.verticalCenter
        width: Math.min(control.width, implicitWidth)
        minimumWidth: Math.min(Theme.metrics.controls.dropdown?.minWidth ?? 150, control.width)
        model: control.model && control.model.length !== undefined ? Array.from(control.model, o => String(o)) : []
        currentIndex: control.currentIndex
        onActivated: index => control.select(index)
    }
}
