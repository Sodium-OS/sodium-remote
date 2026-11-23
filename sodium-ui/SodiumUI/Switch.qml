// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Effects
import QtQuick.Templates as T

// Switch (Components, Settings; states from Components · states T1): 44 x 26
// track, accent when on, white 18% off, a 20 px white knob that moves on the
// snappy spring. Off: hover white 24%, pressed 30%; on: + white 14% hover,
// + black 12% pressed. Pressed, the knob stretches 20 -> 24 px away from its
// end (snappy). Keyboard focus: a 2 px ring 2 px outside. Disabled: 45%.
//
//     Switch { checked: true; Accessible.name: "Reduce motion" }
//     Switch { text: "Wobbly windows while dragging" }      // label on the right
T.Switch {
    id: control

    property string previewState: ""
    /// The Task manager's 40 x 24 switch (knob 18).
    property bool small: false
    readonly property var metrics: small ? Theme.metrics.controls.switchSmall : Theme.metrics.controls.switch
    readonly property bool showHover: enabled && (previewState === "hover" || hovered)
    readonly property bool showDown: enabled && (previewState === "pressed" || down)
    readonly property bool showFocus: previewState === "focus" || visualFocus || (Theme.focusRingAlways && activeFocus)

    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset,
                            implicitContentWidth + leftPadding + rightPadding,
                            implicitIndicatorWidth)
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset,
                             implicitContentHeight + topPadding + bottomPadding,
                             implicitIndicatorHeight + topPadding + bottomPadding)
    padding: 0
    spacing: 10
    hoverEnabled: true
    font: Theme.font("body")
    opacity: enabled ? 1 : 0.45
    leftPadding: text !== "" && mirrored ? indicator.width + spacing : 0
    rightPadding: text !== "" && !mirrored ? indicator.width + spacing : 0

    indicator: StateFill {
        id: track

        readonly property real inset: (control.metrics.height - control.metrics.knob) / 2

        implicitWidth: control.metrics.width
        implicitHeight: control.metrics.height
        x: control.text === "" ? (control.width - width) / 2 : (control.mirrored ? 0 : control.width - width)
        y: (control.height - height) / 2
        radius: height / 2
        rest: control.checked ? Theme.accentInk : Theme.switchOff
        hover: control.checked ? Theme.over(Theme.accentInk, Theme.liftOverlay) : Theme.switchOffHover
        pressed: control.checked ? Theme.over(Theme.accentInk, Theme.pressOverlay) : Theme.switchOffPressed
        hovered: control.showHover
        down: control.showDown

        // 0 off .. 1 on, following a drag too; the knob stretches while pressed.
        Spring { id: knobPos; to: control.visualPosition; preset: "snappy" }
        Spring { id: knobW; to: control.metrics.knob + (control.showDown ? 4 : 0); preset: "snappy" }

        RectangularShadow {
            anchors.fill: knob
            radius: knob.radius
            offset.y: 1
            blur: 3
            color: Qt.rgba(Theme.shadow.r, Theme.shadow.g, Theme.shadow.b, 0.35)
        }
        Rectangle {
            id: knob
            x: track.inset + knobPos.value * (track.width - 2 * track.inset - width)
            y: track.inset
            width: knobW.value
            height: control.metrics.knob
            radius: height / 2
            color: Theme.knob
        }

        FocusRing {
            shown: control.showFocus
            controlRadius: track.radius
        }
    }

    // The label wraps rather than elide (decision 20); the control grows taller.
    contentItem: StyledText {
        text: control.text
        style: "menu"
        font: control.font
        color: Theme.text
        verticalAlignment: Text.AlignVCenter
        wrapMode: Text.Wrap
    }
}
