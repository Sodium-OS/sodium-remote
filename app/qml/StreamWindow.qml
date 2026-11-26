// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import SodiumUI

// The stream window. The engine puts the picture in a subsurface *below* this window's surface, so
// this window draws only what goes over it: the bar, menus, cards and toasts. Everything else is transparent, a hole
// the picture shows through. Until the first picture arrives (and for the cards) a solid backdrop covers it.
//
// The backdrop is a plain opaque rectangle and the bar is solid on purpose: the Windows port draws the stream window
// opaque too and puts the picture on a child window, so nothing here relies on translucency.
Window {
    id: win

    /// The bar's real height: the video starts right below it (and at 0 in full screen).
    readonly property real barHeight: Math.round(bar.height)
    property string pendingHost: ""
    property bool pendingTakeover: false
    /// The main window: the stream window opens on its screen, fitted to it.
    property var anchor: null
    readonly property bool fullscreen: visibility === Window.FullScreen
    /// Stats on in the bar (screenshots and tests drive it from the script).
    property alias statsOn: bar.statsOn
    /// Screenshots and tests (--script): the bar's Settings button.
    function pressSettings() { bar.settingsRequested() }
    function openMenu(which) { which === "keyboard" ? bar.openKeyboard() : which === "displays" ? bar.openDisplays() : bar.closeMenus() }
    readonly property string state: Remote.sessionState
    readonly property bool showsPicture: state === "streaming" || state === "reconnecting"
    /// In full screen the bar hides until the pointer reaches the top edge, or the keyboard was given back.
    property bool reveal: false
    readonly property bool barShown: !fullscreen || reveal || !Remote.capture || bar.menuOpen
    readonly property real videoY: fullscreen ? 0 : barHeight

    flags: Qt.Window | Qt.FramelessWindowHint
    color: "transparent"
    width: 1100
    height: 760
    minimumWidth: 640
    minimumHeight: 420
    title: Remote.hostName !== "" ? Remote.hostName : qsTr("Remote")
    objectName: "stream-window"

    /// Opens the window and starts the session once its surface exists.
    function begin(hostId, takeover, fullscreenOn) {
        pendingHost = hostId
        pendingTakeover = takeover
        // 1100x760, or less when the screen's available area is smaller (at most 90% of it), centred there.
        visibility = Window.Windowed
        const r = Remote.fittedWindowRect(anchor, 1100, 760)
        minimumWidth = Math.min(640, r.width)
        minimumHeight = Math.min(420, r.height)
        Remote.placeStreamWindow(win, r.x, r.y, r.width, r.height)
        if (fullscreenOn)
            visibility = Window.FullScreen
        show()
        startTimer.restart()
    }
    function pushView() {
        Remote.updateView(0, videoY, width, Math.max(1, height - videoY), devicePixelRatio, fullscreen)
        const rects = []
        if (barShown)
            rects.push({ x: 0, y: 0, w: width, h: barHeight })
        // A menu, the secure-screen card or a card over the picture: the pointer belongs to the app, not the computer.
        // On Windows an open menu takes only its own rectangle (and shadow), so the picture stays visible around it; a
        // press in the picture then closes the menu (the engine reports it) and goes no further.
        if (Qt.platform.os === "windows" && bar.menuOpen && showsPicture && !Remote.secureScreen && menuRegion) {
            const r = bar.openMenuRect
            rects.push({ x: Math.max(0, r.x), y: Math.max(0, r.y), w: r.width, h: r.height })
        } else if (bar.menuOpen || Remote.secureScreen || !showsPicture)
            rects.push({ x: 0, y: 0, w: width, h: height })
        // Windows: this window's region is these rectangles, so the toasts and the pill need theirs too.
        if (Qt.platform.os === "windows" && (backToast.visible || hint.visible || state === "reconnecting"))
            rects.push({ x: 0, y: videoY, w: width, h: 110 })
        Remote.setHudRects(rects)
    }

    onWidthChanged: pushView()
    onHeightChanged: pushView()
    onVisibilityChanged: pushView()
    onFullscreenChanged: { pushView(); if (fullscreen) hint.show() }
    onBarShownChanged: pushView()
    onShowsPictureChanged: pushView()
    // A menu opening: no input reaches the computer while it is open, and held keys go up when it opens; the keyboard
    // is the app's until it closes (Escape or a click outside closes it), then goes back to the computer if it was there.
    property bool captureBeforeMenu: false
    /// Windows: the HUD's region is the bar and the menu only (false: the whole window, the black-backdrop fallback).
    readonly property bool menuRegion: true
    Connections {
        target: bar
        function onMenuOpenChanged() {
            if (bar.menuOpen) {
                win.captureBeforeMenu = Remote.capture
                Remote.setCapture(false)
                Remote.setMenuBlock(true)
            } else {
                Remote.setMenuBlock(false)
                if (win.captureBeforeMenu && win.showsPicture)
                    Remote.setCapture(true)
                win.captureBeforeMenu = false
            }
            win.pushView()
        }
        function onOpenMenuRectChanged() { if (bar.menuOpen) win.pushView() }
    }
    Connections {
        target: Remote
        function onMenuCloseRequested() { bar.closeMenus() }
    }
    onBarHeightChanged: pushView()
    onClosing: Remote.clearSession()

    Timer {
        id: startTimer
        interval: 150
        onTriggered: {
            if (!Remote.connectTo(win.pendingHost, win, win.pendingTakeover, win.fullscreen, win.videoY) && Remote.sessionState === "idle")
                win.hide()
        }
    }
    Timer {
        // A fresh start right after a failure: the old session needs a moment to finish.
        id: retryTimer
        interval: 400
        onTriggered: Remote.connectTo(win.pendingHost, win, win.pendingTakeover, win.fullscreen, win.videoY)
    }

    Connections {
        target: Remote
        function onSessionChanged() {
            if (Remote.sessionState === "idle" && win.visible)
                win.hide()
            win.pushView()
        }
        function onSecureChanged() {
            win.pushView()
            if (!Remote.secureScreen && win.showsPicture)
                backToast.show()
        }
        function onLeaveChord() {
            // Leave full screen with the bar shown; a click in the video takes the keyboard back.
            if (win.fullscreen)
                win.visibility = Window.Windowed
        }
    }

    Item {
        anchors.fill: parent

        // Solid until there is a picture to show through.
        Rectangle {
            anchors.fill: parent
            color: "#000000"
            visible: !win.showsPicture
        }

        // Cards: connecting, failed, ended.
        Item {
            anchors.fill: parent
            anchors.topMargin: win.videoY
            visible: !win.showsPicture

            ConnectingCard {
                anchors.centerIn: parent
                width: Math.min(400, parent.width - 48)
                visible: win.state === "connecting" || win.state === "connected"
                hostName: Remote.hostName
                phase: win.state === "connected" ? 1 : 0
                onCancelled: Remote.disconnectSession()
            }
            FailureCard {
                anchors.centerIn: parent
                width: Math.min(400, parent.width - 48)
                visible: win.state === "failed" || win.state === "ended"
                failure: Remote.failure
                hostName: Remote.hostName
                hostOs: Remote.hostOs
                onRetry: { Remote.clearSession(); win.pendingTakeover = false; retryTimer.restart() }
                onTakeover: { Remote.clearSession(); win.pendingTakeover = true; retryTimer.restart() }
                onPairNow: { win.hide(); Remote.clearSession(); win.pairRequested(win.pendingHost) }
                onDismissed: win.hide()
                onServerSettingsRequested: { win.hide(); Remote.clearSession(); win.serverSettingsRequested() }
            }
        }

        // The "reconnecting" pill over the picture.
        Rectangle {
            visible: win.state === "reconnecting"
            anchors.horizontalCenter: parent.horizontalCenter
            y: win.videoY + 14
            implicitWidth: pillText.implicitWidth + 28
            implicitHeight: 32
            radius: 16
            color: Theme.surfaceSolid
            border.width: 1
            border.color: Theme.hairline
            Text {
                id: pillText
                anchors.centerIn: parent
                text: Remote.route === "lost" ? qsTr("Connection lost · retrying") : qsTr("Reconnecting…")
                font: Theme.font("body")
                color: Theme.text
            }
        }

        // The secure screen (a lock, sign-in or UAC screen on the computer): nothing to show or drive.
        Rectangle {
            objectName: "secure-screen"
            anchors.fill: parent
            anchors.topMargin: win.videoY
            visible: Remote.secureScreen && win.showsPicture
            color: Qt.rgba(0, 0, 0, 0.72)
            Rectangle {
                anchors.centerIn: parent
                width: Math.min(460, parent.width - 48)
                implicitHeight: secureCol.implicitHeight + 48
                height: implicitHeight
                radius: 16
                color: Theme.dialog
                border.width: 1
                border.color: Theme.hairline
                ColumnLayout {
                    id: secureCol
                    anchors.fill: parent
                    anchors.margins: 24
                    spacing: 10
                    Icon { Layout.alignment: Qt.AlignHCenter; name: "system-lock-screen-symbolic"; size: 32; color: Theme.warning }
                    WrappedLabel {
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        text: qsTr("%1 is showing a secure screen").arg(Remote.hostName)
                        font: Theme.font("heading")
                        color: Theme.text
                    }
                    Text {
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.Wrap
                        text: Remote.hostOs === "mac"
                            ? qsTr("Its lock screen can’t be shown or controlled from here. The session resumes when the desktop is back.")
                            : qsTr("Its lock, sign-in or UAC screen can’t be shown or controlled from here. The session resumes when the desktop is back.")
                        font: Theme.font("body")
                        color: Theme.text2
                    }
                    Text {
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        text: qsTr("Waiting for the desktop…")
                        font: Theme.font("caption")
                        color: Theme.text3
                    }
                    ButtonRow {
                        Layout.fillWidth: true
                        Button { text: qsTr("Disconnect"); onClicked: Remote.disconnectSession() }
                    }
                }
            }
        }

        // Toasts: "Back on the desktop", and the full-screen hint.
        Rectangle {
            id: backToast
            onVisibleChanged: win.pushView()
            function show() { visible = true; backTimer.restart() }
            visible: false
            anchors.horizontalCenter: parent.horizontalCenter
            y: win.videoY + 14
            implicitWidth: backText.implicitWidth + 32
            implicitHeight: 36
            radius: 12
            color: Theme.surfaceSolid
            border.width: 1
            border.color: Theme.hairline
            Text { id: backText; anchors.centerIn: parent; text: qsTr("Back on %1’s desktop").arg(Remote.hostName); font: Theme.font("body"); color: Theme.text }
            Timer { id: backTimer; interval: 3000; onTriggered: backToast.visible = false }
        }
        Rectangle {
            id: hint
            onVisibleChanged: win.pushView()
            objectName: "fullscreen-hint"
            function show() { visible = true; hintTimer.restart() }
            visible: false
            anchors.horizontalCenter: parent.horizontalCenter
            y: win.videoY + 64
            implicitWidth: hintText.implicitWidth + 32
            implicitHeight: 36
            radius: 12
            color: Theme.surfaceSolid
            border.width: 1
            border.color: Theme.hairline
            Text {
                id: hintText
                anchors.centerIn: parent
                text: qsTr("Press %1 to leave the stream.").arg(Fmt.leaveChord)
                font: Theme.font("body")
                color: Theme.text
            }
            Timer { id: hintTimer; interval: 4000; onTriggered: hint.visible = false }
        }

        StreamBar {
            id: bar
            width: parent.width
            y: win.barShown ? 0 : -height
            window: win
            fullscreen: win.fullscreen
            visible: y > -height
            Behavior on y { NumberAnimation { duration: 140; easing.type: Easing.OutCubic } }
            onFullscreenToggled: win.visibility = win.fullscreen ? Window.Windowed : Window.FullScreen
            onDisconnectRequested: Remote.disconnectSession()
            onSettingsRequested: {
                // First what the leave chord does: give the keyboard back (the engine sends key-ups for what is held)
                // and leave full screen, then open Settings.
                Remote.setCapture(false)
                if (win.fullscreen)
                    win.visibility = Window.Windowed
                win.settingsRequested()
            }
            HoverHandler {
                id: barHover
                onHoveredChanged: if (hovered) hideTimer.stop(); else hideTimer.restart()
            }
        }
        // The top edge in full screen: pointing at it reveals the bar.
        Item {
            visible: win.fullscreen
            width: parent.width
            height: 4
            HoverHandler { onHoveredChanged: if (hovered) { win.reveal = true; hideTimer.stop() } }
        }
        Timer { id: hideTimer; interval: 900; onTriggered: win.reveal = false }

        ResizeEdges {
            anchors.fill: parent
            z: 1000
            visible: win.visibility === Window.Windowed
        }
    }

    signal pairRequested(string hostId)
    /// The bar's Settings button: the computers window opens its settings.
    signal settingsRequested()
    /// "Server settings…" on the Couldn't reach it card.
    signal serverSettingsRequested()
}
