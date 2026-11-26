// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import SodiumUI

// Remote: the computers you can use from here, adding and pairing one, settings, and the stream window.
// "No This computer tab here": the host side of SodiumOS is Settings › Remote.
AppWindow {
    id: win

    title: qsTr("Remote")
    // 760x620, or less on a small screen at a large scale (at most 90% of the primary screen's available area), centred.
    readonly property var openRect: Remote.fittedWindowRect(null, 760, 620)
    x: openRect.x
    y: openRect.y
    width: openRect.width
    height: openRect.height
    minimumWidth: 480
    minimumHeight: 420
    visible: true

    // "This computer": a tab on Windows, and on generic Linux when the app was built with the host (REMOTE_LINUX_HOST: the
    // context property Host is then the controller, else null). The tray and the notice are Windows only.
    readonly property bool hasHost: Qt.platform.os === "windows" || (Qt.platform.os === "linux" && !!Host)
    property int localTab: 0 // Linux; Windows keeps it in Win
    readonly property int tab: Qt.platform.os === "windows" ? (Win ? Win.tab : 0) : localTab
    function setTab(i) {
        if (Qt.platform.os === "windows") {
            if (Win) Win.tab = i
        } else {
            localTab = i
        }
    }

    function connectTo(hostId, fullscreen) {
        stream.begin(hostId, false, fullscreen)
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        TitleBar {
            Layout.fillWidth: true
            title: qsTr("Remote")
            // Windows only: the app icon and name before the tabs, as the board's Windows chrome has them.
            Image {
                visible: Qt.platform.os === "windows"
                source: "../windows/icon.svg"
                sourceSize: Qt.size(20, 20)
                Layout.preferredWidth: 20
                Layout.preferredHeight: 20
            }
            Text {
                visible: Qt.platform.os === "windows"
                text: Fmt.otherHostAppName
                font: Theme.font("heading")
                color: win.active ? Theme.text : Theme.text3
                Layout.leftMargin: -4
                Layout.rightMargin: 4
            }
            // Windows, and Linux with the host: Computers (connect) and This computer (allow control)
            // `Win` goes before the window at quit (main.cpp), hence the null checks.
            SegmentedControl {
                visible: win.hasHost
                objectName: "main-tabs"
                tabs: true
                model: [qsTr("Computers"), qsTr("This computer")]
                currentIndex: win.tab
                onActivated: i => win.setTab(i)
            }
            trailing: [
                Button {
                    objectName: "add-button"
                    visible: win.width >= 640
                    compact: true
                    icon.name: "list-add-symbolic"
                    text: qsTr("Add a computer")
                    onClicked: add.begin()
                },
                // In a narrow window (the 480 px minimum) the title bar has room for the icon only, in every language.
                IconButton {
                    objectName: "add-button-narrow"
                    visible: win.width < 640
                    icon.name: "list-add-symbolic"
                    tooltip: qsTr("Add a computer")
                    onClicked: add.begin()
                },
                IconButton {
                    objectName: "settings-button"
                    icon.name: "preferences-system-symbolic"
                    tooltip: qsTr("Settings")
                    onClicked: settings.open()
                }
            ]
        }

        Loader {
            id: thisPage
            visible: active
            active: win.hasHost && win.tab === 1
            Layout.fillWidth: true
            Layout.fillHeight: active
            source: "ThisComputerPage.qml"
        }

        ComputersPage {
            id: page
            visible: !win.hasHost || win.tab === 0
            Layout.fillWidth: true
            Layout.fillHeight: visible
            onConnectRequested: (id, fs) => win.connectTo(id, fs)
            onAddRequested: add.begin()
            onPairRequested: id => { add.begin(); const n = Remote.nearby.get(id); add.startPairing(id, n.name ?? "", "") }
            onRenameRequested: id => { renameDialog.hostId = id; renameField.text = Remote.computers.get(id).name ?? ""; renameDialog.open() }
            onForgetRequested: id => { forgetDialog.hostId = id; forgetDialog.hostName = Remote.computers.get(id).name ?? ""; forgetDialog.open() }
        }

        StatusStrip {
            Layout.fillWidth: true
            leftText: qsTr("This computer: %1 · ID %2").arg(Remote.deviceName).arg(Remote.selfCode)
            rightText: Remote.relayOnline ? qsTr("Remote server online") : ""
        }
    }

    AddSheet {
        id: add
        onConnectNow: id => win.connectTo(id, false)
    }
    SettingsSheet { id: settings }

    // A server link from outside the app (a click on one, the command line): Settings › Server asks before anything is saved.
    Connections {
        target: Remote
        function onServerLinkRequested(link) {
            Remote.takePendingServerLink()
            win.show(); win.raise(); win.requestActivate()
            settings.openServerLink(link)
        }
    }

    // "Show previews" was just turned off: the saved pictures are gone.
    Rectangle {
        id: previewsToast
        objectName: "previews-toast"
        function show() { visible = true; previewsTimer.restart() }
        visible: false
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: 56
        width: Math.min(parent.width - 32, previewsText.implicitWidth + 32)
        height: previewsText.implicitHeight + 20
        radius: 12
        color: Theme.surfaceSolid
        border.width: 1
        border.color: Theme.hairline
        Text {
            id: previewsText
            anchors.centerIn: parent
            width: parent.width - 32
            horizontalAlignment: Text.AlignHCenter
            text: qsTr("Previews are off. The saved pictures were deleted.")
            font: Theme.font("body"); color: Theme.text; wrapMode: Text.Wrap
        }
        Timer { id: previewsTimer; interval: 3500; onTriggered: previewsToast.visible = false }
    }
    Connections { target: Remote; function onPreviewsTurnedOff() { previewsToast.show() } }

    // Windows: the one-time notice when Sodium Remote starts downloading Cisco's OpenH264.
    ContentDialog {
        id: codecDialog
        objectName: "codec-dialog"
        title: qsTr("Downloading a video codec")
        text: qsTr("This computer has no hardware video encoder, so Sodium Remote is downloading OpenH264 from Cisco. It’s used for video on this computer only. You can turn this off in Settings › Advanced.")
        rejectText: ""
        acceptText: qsTr("OK")
        extraButtons: [ Button { objectName: "codec-off"; text: qsTr("Turn off downloads"); onClicked: { Host.softwareVideo = false; codecDialog.close() } } ]
        // Cisco's required wording, as Cisco has it, in every language.
        Text {
            Layout.fillWidth: true
            text: "OpenH264 Video Codec provided by Cisco Systems, Inc."
            font: Theme.font("caption"); color: Theme.text2; wrapMode: Text.Wrap
        }
    }
    Connections {
        enabled: Qt.platform.os === "windows" && !!Host
        target: Qt.platform.os === "windows" && Host ? Host : null
        function onCodecNoticeRequested() { codecDialog.open() }
    }

    ContentDialog {
        id: forgetDialog
        property string hostId: ""
        property string hostName: ""
        title: qsTr("Forget %1?").arg(hostName)
        text: qsTr("This computer stops trusting its key and can’t connect until you pair again.")
        danger: true
        acceptText: qsTr("Forget")
        onAccepted: Remote.forget(hostId)
    }
    ContentDialog {
        id: renameDialog
        property string hostId: ""
        title: qsTr("Rename")
        acceptText: qsTr("Rename")
        initialFocus: renameField
        onAccepted: Remote.rename(hostId, renameField.text)
        TextField {
            id: renameField
            Layout.fillWidth: true
            Accessible.name: qsTr("Name here")
            placeholderText: qsTr("Name here")
        }
    }

    StreamWindow {
        id: stream
        anchor: win
        onPairRequested: id => { add.begin(); add.startPairing(id, Remote.computers.get(id).name ?? "", "") }
        onSettingsRequested: { win.raise(); win.requestActivate(); settings.open() }
        onServerSettingsRequested: { win.raise(); win.requestActivate(); settings.openServer() }
    }

    // ---- screenshots and tests: "<ms> <command> [argument]" per line (--script), run once the window is up.
    property var _steps: []
    Timer {
        id: stepTimer
        repeat: false
        onTriggered: win.nextStep()
    }
    function nextStep() {
        if (_steps.length === 0)
            return
        const line = _steps.shift()
        const m = line.match(/^(\d+)\s+(\S+)\s*(.*)$/)
        if (m) {
            runStep(m[2], m[3])
        }
        if (_steps.length > 0) {
            const n = _steps[0].match(/^(\d+)/)
            // The screenshot of a `shot` step is taken by the runner, which needs a moment before the next step.
            stepTimer.interval = (n ? Number(n[1]) : 500) + (m && m[2] === "log" && m[3].startsWith("shot ") ? 2200 : 0)
            stepTimer.start()
        }
    }
    /// add, settings, tab N, server add|change|remove|link TEXT|manual ADDRESS, pair HOSTID, code DIGITS, address TEXT, connect HOSTID, connect-fs HOSTID, fullscreen,
    /// leave, disconnect, close, size WxH, menu HOSTID, menu-stream NAME, stream-settings, address-menu, notices, previews on|off, grab FILE (screenshots from an offscreen run), grab-page FILE (This computer, the whole page), codec-dialog, event JSON (as if the engine sent it), host-event JSON (the same for "This computer"), maintab N, allow on|off, forget-consent, log TEXT, quit
    function runStep(cmd, arg) {
        switch (cmd) {
        case "add": add.begin(); break
        case "close": add.close(); settings.close(); break
        case "maintab": if (win.hasHost) win.setTab(Number(arg)); break
        case "allow": if (win.hasHost) Host.setEnabled(arg !== "off"); break
        case "forget-consent": if (win.hasHost) Host.forgetConsent(); break
        case "settings": settings.open(); break
        case "notices": settings.tab = settings.aboutTab; settings.open(); settings.openNotices(); break
        case "server": { const sp = arg.indexOf(" "); settings.serverAction(sp < 0 ? arg : arg.slice(0, sp), sp < 0 ? "" : arg.slice(sp + 1)) } break
        case "previews": Remote.showPreviews = arg !== "off"; break
        case "codec-dialog": codecDialog.open(); break
        case "tab": settings.tab = Number(arg); break
        case "pair": { const n = Remote.nearby.get(arg); add.startPairing(arg, n.name ?? "", "") } break
        case "code": Remote.pair(add.targetId, arg, add.targetAddr); break
        case "address": Remote.resolveAddress(arg); break
        case "connect": page.closeMenus(); connectTo(arg, false); break
        case "connect-fs": connectTo(arg, true); break
        case "fullscreen": stream.visibility = stream.fullscreen ? Window.Windowed : Window.FullScreen; break
        case "disconnect": Remote.disconnectSession(); break
        case "size": { const wh = arg.split("x"); win.width = Number(wh[0]); win.height = Number(wh[1]) } break
        case "menu": page.menuAtHost(arg); break
        case "freeze": Remote.freezeRefresh(); break
        case "stats": stream.statsOn = !stream.statsOn; break
        case "menu-stream": stream.openMenu(arg); break
        case "stream-settings": stream.pressSettings(); break
        case "main-hide": win.hide(); break
        case "main-show": win.show(); win.raise(); break
        case "address-menu": if (thisPage.item) thisPage.item.openAddressMenu(); break
        case "event": Remote.injectEvent(arg); break
        case "host-event": if (win.hasHost) Host.injectEvent(arg); break
        case "forget-dialog": if (thisPage.item) thisPage.item.openForget(); break
        case "grab-page": if (thisPage.item) thisPage.item.grabAll(arg); break
        case "grab": win.contentItem.grabToImage(function (r) { r.saveToFile(arg) }); break
        case "log": console.log("remote-script:", arg); break
        case "quit": Qt.quit(); break
        }
    }
    Component.onCompleted: {
        if (typeof testScript === "string" && testScript !== "") {
            _steps = testScript.split("\n").filter(l => l.trim() !== "" && !l.startsWith("#"))
            stepTimer.interval = 800
            stepTimer.start()
        }
        Remote.refresh()
        const pending = Remote.takePendingServerLink()
        if (pending !== "")
            settings.openServerLink(pending)
    }
}
