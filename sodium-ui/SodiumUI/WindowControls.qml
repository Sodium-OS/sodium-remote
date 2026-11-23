// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick

// The window controls on the right of every title bar (Components board, P3TitleBars):
// the pin (Always on top), minimise, a square maximise, close; 30 x 28, gap 4, radius 8,
// 14 px glyphs. The pin shows only under sodiumwm (it keeps windows on top) and stays lit,
// in the checked fill, while the window is kept on top; a click toggles it.
// Hover: white 10% with a white glyph; close turns #E5484D. Inactive windows
// draw the glyphs in #7F8594.
//
//     WindowControls {}                 // acts on its own window
//
// Under sodiumwm (docs/decisions/sodiumwm-api.md) the maximise glyph comes from sodiumwm's
// state, since Hyprland tells tiled windows they are maximised; maximise and restore
// go through sodiumwm; and resting the pointer on maximise for 400 ms asks the shell for
// the snap grid (P3Snap). Without sodiumwm, or before it has found this window, it uses
// the toplevel state as before.
Row {
    id: controls

    /// Window to act on; defaults to the one this item is in.
    property var window: Window.window
    property bool active: Window.active
    /// The pin (only while sodiumwm runs: nothing else keeps a window on top). Close-only
    /// dialogs have none, as sodiumwm's dialog bars.
    property bool showPin: showMinimize || showMaximize
    property bool showMinimize: true
    property bool showMaximize: true
    /// Forces a look for galleries: "", hover, pressed.
    property string previewState: ""
    /// The native SodiumwmWindow (null with a SodiumUI plugin from before it existed).
    readonly property QtObject wm: _wm
    readonly property bool managed: _wm !== null && _wm.running && _wm.address !== ""
    readonly property bool onTop: managed && _wm.onTop
    readonly property bool maximized: managed ? _wm.maximised
        : window ? window.visibility === Window.Maximized || window.visibility === Window.FullScreen : false
    readonly property var metrics: Theme.metrics.titleBar.button

    /// Maximise or restore: through sodiumwm while it runs, else the toplevel state.
    function toggleMaximize() {
        if (_wm !== null && _wm.toggleMaximise())
            return
        _toggleToplevel()
    }
    function _toggleToplevel() {
        if (!window)
            return
        if (window.visibility === Window.Maximized)
            window.showNormal()
        else
            window.showMaximized()
    }

    // Made by name, so a SodiumUI plugin built before SodiumwmWindow existed keeps today's
    // behavior (rebuild with ui/build.sh).
    property QtObject _wm: null
    Component.onCompleted: {
        try {
            _wm = Qt.createQmlObject("import SodiumUI; SodiumwmWindow {}", controls, "SodiumwmWindow")
        } catch (e) {
            _wm = null
        }
    }
    Binding { target: controls._wm; property: "window"; value: controls.window ?? null; when: controls._wm !== null }
    Connections {
        target: controls._wm
        function onFallbackNeeded() { controls._toggleToplevel() }
    }

    spacing: metrics.gap

    IconButton {
        visible: controls.showPin && controls._wm !== null && controls._wm.running
        icon.name: "view-pin-symbolic"
        icon.width: controls.metrics.icon
        icon.height: controls.metrics.icon
        iconColor: controls.active ? Theme.control : Theme.controlInactive
        checked: controls.onTop
        previewState: controls.previewState
        Accessible.name: qsTr("Always on top")
        Accessible.checkable: true
        Accessible.checked: controls.onTop
        onClicked: if (controls._wm !== null) controls._wm.toggleOnTop()
    }
    IconButton {
        visible: controls.showMinimize
        icon.name: "window-minimize-symbolic"
        icon.width: controls.metrics.icon
        icon.height: controls.metrics.icon
        iconColor: controls.active ? Theme.control : Theme.controlInactive
        previewState: controls.previewState
        Accessible.name: qsTr("Minimize")
        onClicked: if (controls.window) controls.window.showMinimized()
    }
    // Windows: WindowChrome answers HTMAXBUTTON over this button (snap layouts), so the pointer never reaches it
    // here; its hover and pressed looks come from the non-client messages.
    readonly property var _chrome: window && window.chrome !== undefined && window.chrome !== null ? window.chrome : null
    function _publishMax() {
        if (_chrome !== null && maxButton.visible && maxButton.width > 0)
            _chrome.maxButton = maxButton.mapToItem(null, 0, 0, maxButton.width, maxButton.height)
    }
    onXChanged: _publishMax()
    onWidthChanged: _publishMax()
    on_ChromeChanged: _publishMax()
    Connections {
        target: controls.window
        ignoreUnknownSignals: true
        function onWidthChanged() { controls._publishMax() }
    }

    IconButton {
        id: maxButton
        visible: controls.showMaximize
        onXChanged: controls._publishMax()
        onVisibleChanged: controls._publishMax()
        icon.name: controls.maximized ? "window-restore-symbolic" : "window-maximize-symbolic"
        icon.width: controls.metrics.icon
        icon.height: controls.metrics.icon
        iconColor: controls.active ? Theme.control : Theme.controlInactive
        previewState: controls._chrome !== null && controls._chrome.maxState !== "" ? controls._chrome.maxState : controls.previewState
        Accessible.name: controls.maximized ? qsTr("Restore") : qsTr("Maximize")
        onClicked: {
            snapHover.stop()
            controls.toggleMaximize()
        }

        // P3Snap: the snap grid opens after the pointer rests 400 ms on maximise, 8 px
        // under the button and centered on it; the shell owns it. We pass the button's
        // center, as the shell's SnapGrid expects.
        Timer {
            id: snapHover
            interval: 400
            running: maxButton.hovered && !maxButton.pressed && controls._wm !== null && controls._wm.running
            onTriggered: {
                const p = maxButton.mapToItem(null, maxButton.width / 2, maxButton.height / 2)
                controls._wm.showSnapGrid(p.x, p.y)
            }
        }
    }
    IconButton {
        icon.name: "window-close-symbolic"
        icon.width: controls.metrics.icon
        icon.height: controls.metrics.icon
        iconColor: controls.active ? Theme.control : Theme.controlInactive
        danger: true
        previewState: controls.previewState
        Accessible.name: qsTr("Close")
        onClicked: if (controls.window) controls.window.close()
    }
}
