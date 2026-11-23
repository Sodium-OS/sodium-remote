// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import QtQuick.Templates as T

// The dropdown button (P2Dialogs dropdown, the Settings rows; decision 20, LangComponents A05/B05):
// 32 px, the chosen option and a chevron; a click opens a Menu of the options with the chosen one
// checked, as wide as the button. **As wide as its longest option, from 150 to 320 px**, so a longer
// translation widens it instead of eliding it.
//
//     DropDownButton {
//         model: [qsTr("Auto"), qsTr("Faster"), qsTr("Highest quality")]
//         currentIndex: 0
//         onActivated: index => quality = index
//     }
//     DropDownButton {                                  // values of your own
//         model: [{ value: "c", label: qsTr("Celsius") }, { value: "f", label: qsTr("Fahrenheit") }]
//         value: Theme.settings.temperatureUnit ?? "c"
//         onActivated: index => Theme.setSetting("temperatureUnit", model[index].value)
//     }
//
// The name is DropDownButton, not DropDown, so Settings' and Calendar's own DropDown.qml keep working
// (an imported SodiumUI type wins over a file next to yours). SegmentedControl turns into one of these when
// its options don't fit. States: rest white 10%, hover 14%, pressed and open 20%; keyboard focus a 2 px
// ring outside; disabled 45%.
T.Button {
    id: control

    /// Option labels, or { value, label } objects.
    property var model: []
    /// The chosen option; -1 for none.
    property int currentIndex: -1
    /// The chosen option's value (`value` of an object option, else its label). Setting it picks that option.
    property var value: undefined
    /// The width it never goes below, and the most it grows to (decision 20: 150–320).
    property real minimumWidth: metrics.minWidth
    property real maximumWidth: metrics.maxWidth
    property string previewState: ""
    /// An option was chosen from the menu.
    signal activated(int index)

    readonly property var metrics: Theme.metrics.controls.dropdown ?? ({ height: 32, radius: 9, paddingX: 10, gap: 8, chevron: 12, minWidth: 150, maxWidth: 320 })
    readonly property var labels: Array.from(model ?? [], o => typeof o === "object" && o !== null ? String(o.label ?? "") : String(o))
    readonly property string currentLabel: currentIndex >= 0 && currentIndex < labels.length ? labels[currentIndex] : ""
    /// The longest option's width with the chevron and padding, or its open list's if that is wider (the
    /// list is as wide as the button), before the 150–320 clamp.
    readonly property real contentNeed: {
        void _metrics.font
        let w = 0
        for (const l of labels)
            w = Math.max(w, _metrics.advanceWidth(l))
        return Math.max(Math.ceil(w) + 1 + leftPadding + rightPadding + metrics.gap + metrics.chevron,
                        Math.ceil(menu.longestItemWidth + menu.leftPadding + menu.rightPadding))
    }
    readonly property bool showHover: enabled && (previewState === "hover" || hovered)
    readonly property bool showDown: enabled && (previewState === "pressed" || down || menu.visible)
    readonly property bool showFocus: previewState === "focus" || visualFocus || (Theme.focusRingAlways && activeFocus)

    function _valueOf(i) {
        const o = model[i]
        return typeof o === "object" && o !== null && o.value !== undefined ? o.value : labels[i]
    }
    onValueChanged: {
        for (let i = 0; i < labels.length; i++)
            if (_valueOf(i) === value || String(_valueOf(i)) === String(value)) {
                if (currentIndex !== i)
                    currentIndex = i
                return
            }
    }
    onCurrentIndexChanged: if (currentIndex >= 0 && currentIndex < labels.length && value !== _valueOf(currentIndex)) value = _valueOf(currentIndex)
    // options that arrive or change later (a list read in the background) re-pick the current value
    onLabelsChanged: if (value !== undefined) valueChanged()
    Component.onCompleted: {
        if (value !== undefined)
            valueChanged()
        else if (currentIndex >= 0)
            currentIndexChanged()
    }

    implicitWidth: Math.max(minimumWidth, Math.min(maximumWidth, contentNeed))
    implicitHeight: metrics.height
    leftPadding: metrics.paddingX
    rightPadding: metrics.paddingX
    topPadding: 0
    bottomPadding: 0
    hoverEnabled: true
    font: Theme.font("menu")
    opacity: enabled ? 1 : 0.45
    Accessible.role: Accessible.ComboBox
    Accessible.name: I18n.stripBreakMarks(currentLabel)

    property FontMetrics _metrics: FontMetrics { font: control.font }

    contentItem: RowLayout {
        spacing: control.metrics.gap
        StyledText {
            Layout.fillWidth: true
            text: control.currentLabel
            style: "menu"
            font: control.font
            color: control.enabled ? Theme.text : Theme.textMuted
            elide: Text.ElideRight
            verticalAlignment: Text.AlignVCenter
        }
        Icon {
            name: "pan-down-symbolic"
            size: control.metrics.chevron
            color: Theme.text2
        }
    }

    background: StateFill {
        implicitHeight: control.metrics.height
        radius: Theme.corner(control.metrics.radius)
        rest: Theme.hover
        hover: Theme.secondaryHover
        pressed: Theme.pressed
        hovered: control.showHover
        down: control.showDown

        FocusRing {
            shown: control.showFocus
            controlRadius: parent.radius
        }
    }

    onClicked: open()
    /// Opens the list under the button.
    function open() { menu.popup(control, 0, control.height + 4) }

    Menu {
        id: menu
        // "The open list is the same width" (A05).
        minimumWidth: control.width
        maximumWidth: Math.max(control.width, control.maximumWidth)
        Instantiator {
            model: control.labels
            delegate: MenuItem {
                required property string modelData
                required property int index
                text: modelData
                radio: true
                checked: index === control.currentIndex
                onTriggered: {
                    control.currentIndex = index
                    control.activated(index)
                }
            }
            onObjectAdded: (i, item) => menu.insertItem(i, item)
            onObjectRemoved: (i, item) => menu.removeItem(item)
        }
    }
}
