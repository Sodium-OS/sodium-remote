// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Templates as T

// Text field (Components board: "Workspace name"): 36 px, radius 10, white 7%,
// 13 px text (body); focused it gets the 1.5 px inset accent ring, and `error` makes
// that ring danger red (Phase 2 dialogs: a wrong password).
//
//     TextField { placeholderText: "Name"; text: "Work" }
//
// Input methods work as in any Qt text input: fcitx5 + Rime show their
// preedit and candidate window at the caret (QT_IM_MODULE=fcitx or wayland).
//
// Never use it for a password: use PasswordField. A TextField with
// `echoMode: TextInput.Password` lets Rime compose (and learn) what is typed,
// whatever inputMethodHints say: Qt 6.12 drops ImhHiddenText from Password
// fields, and fcitx5 only keeps its input method out of fields with that hint.
//
// States (Components · states F1): hover white 10%; focused keeps the rest fill
// with the ring and a 1.5 px accent caret; error is a danger ring, focused or
// not, with `errorText` in 12 px danger 6 px below the field (leave 18 px of
// room under it). Disabled: 45% opacity, no hover, no caret.
//
//     TextField { error: badName; errorText: "A space with this name exists" }
T.TextField {
    id: control

    property bool error: false
    /// Shown under the field while `error` is set.
    property string errorText: ""
    /// errorText goes under the field, outside its height (F1). PasswordField sets this false
    /// and shows errorText in its own status lines instead.
    property bool errorTextBelow: true
    property string previewState: ""
    readonly property var metrics: Theme.metrics.controls.input
    readonly property bool showHover: enabled && (previewState === "hover" || hovered)
    readonly property bool showFocus: previewState === "focus" || activeFocus

    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset, 200)
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset,
                             contentHeight + topPadding + bottomPadding)
    leftPadding: metrics.paddingX
    rightPadding: metrics.paddingX
    topPadding: 0
    bottomPadding: 0
    verticalAlignment: TextInput.AlignVCenter
    hoverEnabled: true
    font: Theme.font("body")
    color: Theme.text
    placeholderTextColor: Theme.text3
    selectionColor: Theme.accentLine
    selectedTextColor: Theme.text
    opacity: enabled ? 1 : 0.45
    cursorDelegate: Caret { barHeight: 17 }

    Text {
        x: control.leftPadding
        y: control.topPadding
        width: control.width - control.leftPadding - control.rightPadding
        height: control.height - control.topPadding - control.bottomPadding
        text: control.placeholderText
        font: control.font
        color: control.placeholderTextColor
        verticalAlignment: control.verticalAlignment
        horizontalAlignment: control.horizontalAlignment
        elide: Text.ElideRight
        visible: control.length === 0 && control.preeditText.length === 0
        I18nAudit.mayElide: true   // a hint in a narrow field: the typed text replaces it
    }

    background: Rectangle {
        implicitHeight: control.metrics.height
        implicitWidth: 200
        radius: control.metrics.radius
        color: control.showHover && !control.showFocus ? Theme.fieldHover : Theme.fill
        Behavior on color { ColorAnimation { duration: Theme.fadeMs } }

        FocusRing {
            shown: control.showFocus || control.error
            ringColor: control.error ? Theme.danger : Theme.focusRing
            controlRadius: parent.radius
            lineWidth: Theme.metrics.controls.input.focusRing
            gap: -lineWidth
        }

        StyledText {
            style: "caption"
            y: parent.height + 6
            width: parent.width
            visible: control.error && control.errorText !== "" && control.errorTextBelow
            text: control.errorText
            font: Theme.font("caption", { weight: 400 })
            color: Theme.danger
            wrapMode: Text.Wrap
        }
    }
}
