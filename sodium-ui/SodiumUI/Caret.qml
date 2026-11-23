// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick

// The text caret of TextField and SearchField (Components · states F1): a
// 1.5 px accent bar, 17 px tall (search: 15), blinking at the system's cursor
// flash time and solid again whenever it moves; always solid with Settings ›
// Accessibility › "Blink the text caret" off (Theme.caretBlink). Internal to SodiumUI.
Item {
    id: caret

    property real barHeight: 17
    /// The text input it belongs to: its parent as a cursorDelegate; PasswordField sets it.
    property Item input: parent

    width: 1.5
    visible: input && input.activeFocus && !input.readOnly && input.selectionStart === input.selectionEnd

    Rectangle {
        id: bar
        anchors.centerIn: parent
        width: 1.5
        height: caret.barHeight
        radius: 0.75
        color: Theme.accentInk
    }

    Connections {
        target: caret.input
        function onCursorPositionChanged() {
            bar.opacity = 1
            blink.restart()
        }
    }

    Timer {
        id: blink
        running: caret.visible && Theme.caretBlink && Qt.styleHints.cursorFlashTime > 0
        repeat: true
        interval: Qt.styleHints.cursorFlashTime / 2
        onTriggered: bar.opacity = bar.opacity > 0 ? 0 : 1
        onRunningChanged: bar.opacity = 1
    }
}
