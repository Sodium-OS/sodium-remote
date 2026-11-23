// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts

// The one window chrome (PLAN section 5, Components board "Title bar"): 48 px,
// padding 12 left and 10 right, the window controls on the right and a 1 px
// divider below. Content headers (breadcrumbs, search, tabs) go in the bar and
// span its width; `trailing` actions sit before the controls, after a 1 x 18
// separator (Calendar, Task manager).
//
//     TitleBar { title: "SodiumOS" }                               // plain title
//     TitleBar {
//         SearchField { placeholderText: "Search settings" }
//         Text { text: "Appearance"; font: Theme.font("heading"); color: Theme.text }
//         Item { Layout.fillWidth: true }
//         trailing: [ Button { text: "New event"; variant: "primary"; compact: true } ]
//     }
//
// Dragging the bar moves the window (startSystemMove) and a double click maximises
// it (through sodiumwm while it runs, see WindowControls); a right click opens sodiumwm's
// window menu at the pointer (decision 15 in docs/decisions/phase3-sodiumwm.md), as the
// plugin's bars do. AppWindow adds edge resizing. Inactive windows gray the title and
// controls. Dialogs show Close only (decision 14): `showMinimize: false; showMaximize: false`,
// which also turns the double click off.
Item {
    id: bar

    /// The window it moves and controls (null: a picture of a title bar, as in galleries).
    property var window: Window.window
    property string title: window ? window.title : ""
    property bool active: Window.active
    /// Paint the title-bar tint (stand-alone bars); app windows leave it clear over their glass.
    property bool filled: false
    property bool showControls: true
    /// Minimise and maximise next to Close (dialogs: both false, decision 14).
    property bool showMinimize: true
    property bool showMaximize: true
    property bool showDivider: true
    property string previewState: ""
    /// Left-to-right content in a RowLayout; empty shows `title`.
    default property alias content: contentRow.data
    /// Actions right before the window controls.
    property alias trailing: trailingRow.data
    /// Its WindowControls (maximise state and sodiumwm: `controls.maximized`, `controls.wm`).
    readonly property alias controls: windowControls
    readonly property var metrics: Theme.metrics.titleBar

    implicitHeight: metrics.height
    implicitWidth: 320

    /// Maximise or restore (through sodiumwm while it runs: WindowControls).
    function toggleMaximized() {
        windowControls.toggleMaximize()
    }
    /// sodiumwm's window menu with its top-left at bar point (x, y) (the shell draws it).
    function showWindowMenu(x, y) {
        const wm = windowControls.wm
        if (!bar.window || wm === null || wm.showWindowMenu === undefined)
            return
        const p = bar.mapToItem(null, x, y)
        wm.showWindowMenu(p.x, p.y)
    }

    Rectangle {
        anchors.fill: parent
        visible: bar.filled
        color: bar.active ? Theme.windowTitleActive : Theme.windowTitleInactive
    }

    DragHandler {
        target: null
        grabPermissions: PointerHandler.CanTakeOverFromItems | PointerHandler.CanTakeOverFromHandlersOfDifferentType
        onActiveChanged: if (active && bar.window) bar.window.startSystemMove()
    }
    TapHandler {
        onDoubleTapped: if (bar.showMaximize) bar.toggleMaximized()
    }
    TapHandler {
        acceptedButtons: Qt.RightButton
        onTapped: point => bar.showWindowMenu(point.position.x, point.position.y)
    }

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: bar.metrics.paddingLeft
        anchors.rightMargin: bar.metrics.paddingRight
        spacing: bar.metrics.gap

        StyledText {
            style: "windowTitle"
            visible: contentRow.children.length === 0
            Layout.fillWidth: true
            leftPadding: 6
            text: bar.title
            font: Theme.font("windowTitle", { weight: bar.active ? 600 : 400 })
            color: bar.active ? Theme.text : Theme.titleInactive
            elide: Text.ElideRight
            I18nAudit.userContent: true   // the window's title: usually a document's name
        }

        RowLayout {
            id: contentRow
            visible: children.length > 0
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: bar.metrics.gap
        }

        RowLayout {
            id: trailingRow
            visible: children.length > 0
            spacing: bar.metrics.gap
        }

        Rectangle {
            visible: trailingRow.children.length > 0 && bar.showControls
            Layout.preferredWidth: bar.metrics.separator[0]
            Layout.preferredHeight: bar.metrics.separator[1]
            Layout.leftMargin: 4
            Layout.rightMargin: 4
            color: Theme.hover
        }

        WindowControls {
            id: windowControls
            visible: bar.showControls
            window: bar.window
            active: bar.active
            showMinimize: bar.showMinimize
            showMaximize: bar.showMaximize
            previewState: bar.previewState
        }
    }

    Rectangle {
        visible: bar.showDivider
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        height: 1
        color: Theme.divider
    }
}
