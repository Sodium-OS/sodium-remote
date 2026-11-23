// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import QtQuick.Templates as T

// The in-app dialog (P2Dialogs G1–G5): drawn inside its window over a dimmed body,
// never a window of its own, so it has no title bar and can't be minimised apart
// from its window. Dialog tint at 97%, radius 16, the dialog shadow; a 16 / 600 title,
// a 13 px body in text 2, optional content under it, buttons right-aligned with the
// primary last.
//
// Width (decision 20, LangComponents A09/B09): `minimumWidth` (420 px, today's width; 360 is the
// smallest a dialog should ask for), growing to `maximumWidth` (640) so its title stays on one line
// and its buttons side by side; past that the title (balanced, a WrappedLabel) and body wrap, and the buttons stack full-width,
// primary on top (ButtonRow). Narrow windows keep 24 px each side. Set minimumWidth, not width: a
// width of your own fixes it as before.
//
//     ContentDialog {
//         id: confirm
//         title: qsTr("Empty the Trash?")
//         text: qsTr("48 items are deleted for good. This cannot be undone.")
//         iconName: "dialog-warning-symbolic"
//         danger: true                                  // solid red primary; focus starts on Cancel
//         acceptText: qsTr("Empty Trash")
//         onAccepted: trash.empty()
//     }
//     confirm.open()
//
//     ContentDialog {                                   // with content (G4): a field, a list…
//         title: qsTr("Go to folder"); text: qsTr("Type a path; Tab completes.")
//         acceptText: qsTr("Go"); acceptEnabled: path.text !== ""
//         initialFocus: path
//         TextField { id: path; Layout.fillWidth: true }
//     }
//
// The name is ContentDialog, not Dialog, so apps that have a Dialog.qml of their own
// keep it (an imported SodiumUI type wins over a file next to yours).
//
// Keys: Enter presses the focused button, else the primary (never in a danger
// dialog); Esc cancels (rejected()). Focus starts on the primary, on Cancel for
// danger (drawn with its ring, G2), or on `initialFocus`. A field's Enter reaches
// the dialog, so Enter in G4's field means Go. Placement: centered in the window,
// whose body dims (Theme.scrimWindow) below the title bar. Motion (P4Motion DL2): the
// dim fades in over 120 ms while the card pops from 96 % on snappy around its center,
// its alpha rising over 120 ms; closing, card and dim go together on the exit curve
// (160 ms), the card back to 96 %. Reduce motion: both fade, 120 ms.
T.Dialog {
    id: control

    /// The body: one or two sentences saying what happens and what it affects.
    property string text: ""
    /// An optional 36 px tile, only for warnings and errors (G1).
    property string iconName: ""
    property color iconColor: danger ? Theme.dangerText : Theme.barText
    /// An action that can't be undone (G2): the primary is solid red and focus starts on Cancel.
    property bool danger: false
    property string acceptText: qsTr("OK")
    /// "" hides the Cancel button.
    property string rejectText: qsTr("Cancel")
    property bool acceptEnabled: true
    /// What takes the keyboard when it opens (a field); null: the primary, or Cancel for danger.
    property Item initialFocus: null
    /// Content under the body (a field, suggestions, a list, a check box), in a ColumnLayout.
    default property alias content: extra.data
    /// More buttons, left of Cancel (e.g. "Keep both" in a conflict). The last visible button of the
    /// row is the primary: on top when the row stacks.
    property list<Item> extraButtons
    /// The narrowest it gets (today's 420 px), and the widest it grows to (640 px).
    property real minimumWidth: metrics.width
    property real maximumWidth: Math.max(minimumWidth, metrics.maxWidth ?? 640)
    /// What it needs: the title on one line, the buttons side by side.
    readonly property real neededWidth: Math.ceil(Math.max(
        titleWidth.advanceWidth + (iconName !== "" ? metrics.icon + metrics.gap : 0),
        buttonRow.sideBySideWidth) + 2 * metrics.padding)
    /// The dim over the window: Theme.scrimWindow; a full-screen layer may use Theme.scrimScreen.
    property color scrimColor: Theme.scrimWindow
    /// Where the dim starts: under an AppWindow's title bar, else the top.
    property real dimTop: _appWindow ? Theme.metrics.titleBar.height : 0
    readonly property alias acceptButton: acceptBtn
    readonly property alias rejectButton: rejectBtn
    readonly property var metrics: Theme.metrics.controls.dialog ?? ({ width: 420, minWidth: 360, maxWidth: 640, padding: 22, gap: 14, titleGap: 6, contentGap: 12, buttonsTop: 20, buttonGap: 8, icon: 36, iconRadius: 10, iconGlyph: 20, margin: 24 })

    readonly property var _appWindow: parent && parent.Window.window && parent.Window.window.glass !== undefined ? parent.Window.window : null

    parent: T.Overlay.overlay
    anchors.centerIn: parent
    // minimumWidth–maximumWidth by what it needs. Narrow windows keep 24 px each side.
    width: Math.min(Math.max(minimumWidth, Math.min(maximumWidth, neededWidth)),
                    parent ? parent.width - 2 * metrics.margin : maximumWidth)
    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset, implicitContentWidth + leftPadding + rightPadding)
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset, implicitContentHeight + topPadding + bottomPadding)
    /// The most the dialog is tall: the window's body less the margins (a short window, larger text).
    /// Past that, the title, text and content scroll and the buttons stay put.
    readonly property real maxHeight: parent ? Math.max(0, parent.height - dimTop - 2 * metrics.margin) : implicitHeight
    height: Math.min(implicitHeight, maxHeight)
    margins: metrics.margin
    padding: 0
    modal: true
    dim: true
    focus: true
    popupType: T.Popup.Item
    closePolicy: T.Popup.CloseOnEscape

    T.Overlay.modal: Scrim {
        color: control.scrimColor
        dimTop: control.dimTop
        radius: control._appWindow ? control._appWindow.glass.radius : 0
    }

    property Reveal _reveal: Reveal { preset: "snappy" }
    readonly property real _popFrom: Theme.motion.dialogPopFrom ?? 0.96
    opacity: _reveal.opacity
    scale: _popFrom + (1 - _popFrom) * _reveal.progress
    enter: Transition {
        ScriptAction { script: control._reveal.shown = true }
    }
    exit: Transition {
        SequentialAnimation {
            ScriptAction { script: control._reveal.shown = false }
            PauseAnimation { duration: Theme.reduceMotion ? Theme.fadeMs : Theme.exit.durationMs }
        }
    }

    onOpened: {
        if (initialFocus)
            initialFocus.forceActiveFocus(Qt.OtherFocusReason)
        else if (danger && rejectBtn.visible)
            rejectBtn.forceActiveFocus(Qt.TabFocusReason)     // G2 draws Cancel's ring
        else
            acceptBtn.forceActiveFocus(Qt.OtherFocusReason)
    }

    function _enter() {
        for (const b of [acceptBtn, rejectBtn].concat(Array.from(extraButtons))) {
            if (b && b.activeFocus && b.clicked !== undefined) {
                b.clicked()
                return
            }
        }
        if (!danger && acceptEnabled)
            accept()
    }

    background: Glass {
        strength: "dialog"
        implicitWidth: control.metrics.width
    }

    contentItem: ColumnLayout {
        spacing: 0
        Keys.onReturnPressed: control._enter()
        Keys.onEnterPressed: control._enter()

        // Title, text and content: as tall as they are, scrolling when the window is too short for them.
        Flickable {
            id: scroller
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.preferredHeight: contentHeight
            contentWidth: width
            contentHeight: headerRow.implicitHeight + control.metrics.padding
            boundsBehavior: Flickable.StopAtBounds
            clip: contentHeight > height
            ScrollBar.vertical: ScrollBar {}

            RowLayout {
                id: headerRow
                x: control.metrics.padding
                y: control.metrics.padding
                width: scroller.width - 2 * control.metrics.padding
                spacing: control.metrics.gap

                Rectangle {
                    Layout.alignment: Qt.AlignTop
                    visible: control.iconName !== ""
                    implicitWidth: control.metrics.icon
                    implicitHeight: control.metrics.icon
                    radius: control.metrics.iconRadius
                    color: Theme.fill
                    Icon {
                        anchors.centerIn: parent
                        name: control.iconName
                        size: control.metrics.iconGlyph
                        color: control.iconColor
                    }
                }

                ColumnLayout {
                    Layout.fillWidth: true
                    Layout.alignment: Qt.AlignTop
                    spacing: 0

                    WrappedLabel {
                        Layout.fillWidth: true
                        text: control.title
                        style: "dialogTitle"
                        color: Theme.text
                        Accessible.role: Accessible.Heading
                    }
                    StyledText {
                        Layout.fillWidth: true
                        Layout.topMargin: control.metrics.titleGap
                        visible: control.text !== ""
                        text: control.text
                        style: "message"
                        color: Theme.text2
                        wrapMode: Text.Wrap
                    }
                    ColumnLayout {
                        id: extra
                        Layout.fillWidth: true
                        Layout.topMargin: children.length > 0 ? control.metrics.contentGap : 0
                        visible: children.length > 0
                        spacing: 8
                    }
                }
            }
        }

        // The extra buttons, Cancel, the primary: side by side at the trailing end, stacked full-width
        // with the primary on top when they don't fit (decision 20).
        ButtonRow {
            id: buttonRow
            Layout.fillWidth: true
            Layout.leftMargin: control.metrics.padding
            Layout.rightMargin: control.metrics.padding
            Layout.topMargin: control.metrics.buttonsTop
            Layout.bottomMargin: control.metrics.buttonsTop
            spacing: control.metrics.buttonGap
            items: Array.from(control.extraButtons).concat([rejectBtn, acceptBtn])
        }
    }

    property Button _rejectButton: Button {
        id: rejectBtn
        visible: control.rejectText !== ""
        text: control.rejectText
        onClicked: control.reject()
        KeyNavigation.right: acceptBtn
        KeyNavigation.tab: acceptBtn
    }
    property Button _acceptButton: Button {
        id: acceptBtn
        text: control.acceptText
        variant: control.danger ? "destructiveSolid" : "primary"
        enabled: control.acceptEnabled
        onClicked: control.accept()
        KeyNavigation.left: rejectBtn
    }
    // The title on one line, for neededWidth.
    property TextMetrics _titleWidth: TextMetrics {
        id: titleWidth
        font: Theme.font("dialogTitle")
        text: control.title
    }
}
