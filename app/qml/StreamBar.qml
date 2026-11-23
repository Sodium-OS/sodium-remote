// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import SodiumUI

// The stream window's title bar: the computer's name and system, the path chip (amber for
// "Secure screen"; with Stats on it shows the numbers), the display picker (only for a computer showing real displays,
// when it has more than one), keyboard, Stats, Settings, full screen and Disconnect. Solid: the video never sits under
// translucency (the Windows stream window is opaque too, Windows glass).
Rectangle {
    id: bar

    property var window
    property bool fullscreen: false
    /// Stats on: the chip shows round trip, frames per second and the path.
    property bool statsOn: false
    /// The popups this bar opens: the window keeps the pointer from the host while one is open.
    readonly property bool menuOpen: kbMenu.opened || kbMenuMac.opened || displayMenu.opened
    /// The menu that is open, and its rectangle with room for its shadow, in the stream window (Windows: with the
    /// bar, the HUD's region while the menu is open, so the picture stays visible around it).
    readonly property var openMenu: kbMenu.opened ? kbMenu : kbMenuMac.opened ? kbMenuMac : displayMenu.opened ? displayMenu : null
    readonly property rect openMenuRect: {
        const m = openMenu
        if (!m)
            return Qt.rect(0, 0, 0, 0)
        const p = (m.parent ?? bar).mapToItem(null, m.x, m.y)
        const pad = 32
        return Qt.rect(p.x - pad, p.y - pad, m.width + 2 * pad, m.height + 2 * pad)
    }
    /// A wide bar has room for the system's name and the picker's label; a narrow one drops them first.
    readonly property bool roomy: width >= 860
    readonly property string quality: Remote.secureScreen ? "fair" : Fmt.routeQuality(Remote.route, Remote.rttMs)

    /// Screenshots and tests (--script): open a menu.
    function openDisplays() { displayMenu.popup(displayButton, 0, displayButton.height + 4) }
    function openKeyboard() { (Remote.hostOs === "sodiumos" ? kbMenu : kbMenuMac).popup(kbButton, 0, kbButton.height + 4) }
    function closeMenus() { displayMenu.close(); kbMenu.close(); kbMenuMac.close() }

    signal fullscreenToggled()
    signal disconnectRequested()
    signal settingsRequested()

    implicitHeight: 44
    color: Theme.surfaceSolid
    objectName: "stream-bar"

    Rectangle { anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom; height: 1; color: Theme.divider }

    // Dragging the bar moves the window; a double click toggles full screen.
    DragHandler {
        target: null
        onActiveChanged: if (active && bar.window && !bar.fullscreen) bar.window.startSystemMove()
    }
    TapHandler { onDoubleTapped: bar.fullscreenToggled() }

    // The numbers come from the engine once a second while they are shown.
    Timer {
        interval: 1000
        repeat: true
        running: bar.statsOn && bar.visible
        triggeredOnStart: true
        onTriggered: Remote.pollStats()
    }

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: 14
        anchors.rightMargin: 8
        spacing: 8

        Text {
            text: Remote.hostName
            font: Theme.font("windowTitle")
            color: Theme.text
            elide: Text.ElideRight // a computer's own name is user content
            Layout.maximumWidth: 220
        }
        Text {
            objectName: "host-os"
            visible: bar.roomy && text !== ""
            text: Fmt.osName(Remote.hostOs)
            font: Theme.font("caption")
            color: Theme.text3
        }

        // A Mac that hasn't allowed Accessibility: the picture comes, the keyboard and mouse don't.
        Rectangle {
            objectName: "view-only-chip"
            visible: Remote.viewOnly
            implicitWidth: viewOnlyText.implicitWidth + 18
            implicitHeight: 24
            radius: 12
            color: Qt.rgba(Theme.warning.r, Theme.warning.g, Theme.warning.b, 0.18)
            Text {
                id: viewOnlyText
                anchors.centerIn: parent
                text: qsTr("View only")
                font: Theme.font("caption", { weight: 600 })
                color: Theme.warning
            }
        }

        // The path chip, the numbers (Stats), or the secure-screen chip.
        Rectangle {
            objectName: "route-chip"
            readonly property string tone: Remote.secureScreen ? "amber" : Fmt.routeTone(Remote.route)
            readonly property bool numbers: bar.statsOn && !Remote.secureScreen && Remote.stats.fps !== undefined
            visible: chipText.text !== "" || numbers
            implicitWidth: chipRow.implicitWidth + 18
            implicitHeight: 24
            radius: 12
            color: tone === "amber" ? Qt.rgba(Theme.warning.r, Theme.warning.g, Theme.warning.b, 0.18)
                 : tone === "red" ? Theme.dangerSoft : Theme.fill
            HoverHandler { id: chipHover }
            Tooltip {
                objectName: "quality-tooltip"
                text: Fmt.qualityText(Remote.route, Remote.rttMs)
                visible: !Remote.secureScreen && chipHover.hovered
            }
            Accessible.name: Remote.secureScreen ? "" : Fmt.qualityText(Remote.route, Remote.rttMs)
            RowLayout {
                id: chipRow
                anchors.centerIn: parent
                spacing: 7
                Rectangle {
                    // Path quality: green is good, amber fair or poor, grey unknown.
                    visible: !Remote.secureScreen
                    Layout.preferredWidth: 6; Layout.preferredHeight: 6; radius: 3
                    color: Remote.route === "lost" ? Theme.danger : bar.quality === "good" ? Theme.success : bar.quality === "none" ? Theme.text3 : Theme.warning
                }
                Icon { visible: Remote.secureScreen; name: "system-lock-screen-symbolic"; size: 13; color: Theme.warning }
                Text {
                    visible: parent.parent.numbers
                    text: qsTr("%1 ms").arg(Format.number(Math.round((Remote.stats.rtt_ms ?? 0) > 0 ? Remote.stats.rtt_ms : Remote.rttMs)))
                    font: Theme.font("caption", { weight: 600, numeric: true })
                    color: Theme.text
                }
                Rectangle { visible: parent.parent.numbers; Layout.preferredWidth: 1; Layout.preferredHeight: 12; color: Theme.divider }
                Text {
                    visible: parent.parent.numbers
                    text: qsTr("%1 fps").arg(Format.number(Remote.stats.presented ?? 0))
                    font: Theme.font("caption", { numeric: true })
                    color: Theme.text2
                }
                Rectangle { visible: parent.parent.numbers; Layout.preferredWidth: 1; Layout.preferredHeight: 12; color: Theme.divider }
                Text {
                    id: chipText
                    text: Remote.secureScreen ? qsTr("Secure screen")
                        : parent.parent.numbers ? Fmt.routeName(Remote.route) : Fmt.routeChip(Remote.route, Remote.rttMs)
                    font: Theme.font("caption", { weight: Remote.secureScreen ? 600 : 400 })
                    color: parent.parent.tone === "amber" ? Theme.warning : parent.parent.tone === "red" ? Theme.dangerText : Theme.text2
                }
            }
        }

        Item { Layout.fillWidth: true }

        // The display picker: a computer with real displays and more than one of them.
        Button {
            id: displayButton
            objectName: "display-picker"
            visible: Remote.physicalDisplays && Remote.displays.count > 1
            compact: true
            icon.name: "video-display-symbolic"
            readonly property var current: {
                for (let i = 0; i < Remote.displays.count; i++) {
                    const d = Remote.displays.at(i)
                    if (d.current) return d
                }
                return Remote.displays.at(0)
            }
            text: bar.roomy ? Fmt.displayName(current) : ""
            Accessible.name: qsTr("Displays")
            onClicked: displayMenu.popup(displayButton, 0, displayButton.height + 4)
        }
        IconButton {
            id: kbButton
            objectName: "keyboard-button"
            icon.name: "input-keyboard-symbolic"
            tooltip: qsTr("Keyboard. Press %1 to give the keyboard back to this computer.").arg(Fmt.leaveChord)
            onClicked: bar.openKeyboard()
        }
        IconButton {
            objectName: "stats-button"
            icon.name: "utilities-system-monitor-symbolic"
            checked: bar.statsOn
            tooltip: qsTr("Stats")
            onClicked: bar.statsOn = !bar.statsOn
        }
        IconButton {
            objectName: "stream-settings-button"
            icon.name: "preferences-system-symbolic"
            tooltip: qsTr("Settings")
            onClicked: bar.settingsRequested()
        }
        IconButton {
            objectName: "fullscreen-button"
            icon.name: bar.fullscreen ? "view-restore-symbolic" : "view-fullscreen-symbolic"
            tooltip: bar.fullscreen ? qsTr("Exit full screen") : qsTr("Full screen")
            onClicked: bar.fullscreenToggled()
        }
        Button {
            objectName: "disconnect-button"
            compact: true
            variant: "destructive"
            text: qsTr("Disconnect")
            onClicked: bar.disconnectRequested()
        }
    }

    // The host's displays, as the host names them ("Display 1" when it sends no name); the current one is marked.
    Menu {
        id: displayMenu
        MenuSection { text: qsTr("Displays") }
        Repeater {
            model: Remote.displays
            MenuItem {
                required property var model
                text: model.primary ? qsTr("%1 · Main").arg(Fmt.displayName(model)) : Fmt.displayName(model)
                shortcut: Fmt.displaySize(model)
                radio: true
                checked: model.current
                onTriggered: Remote.selectDisplay(model.displayId)
            }
        }
    }
    // The keyboard menu. Ctrl+Alt+Del is offered for SodiumOS computers only (as the Mac app does), so every other host gets
    // its own menu without the item (a hidden item would leave a gap).
    Menu {
        id: kbMenu
        MenuSection { text: qsTr("Key mapping") }
        MenuItem { text: qsTr("Automatic"); radio: true; checked: Remote.keyMapping === "auto"; onTriggered: Remote.keyMapping = "auto" }
        MenuItem { text: qsTr("Pass through"); radio: true; checked: Remote.keyMapping === "pass"; onTriggered: Remote.keyMapping = "pass" }
        // Only SodiumOS computers: an injected Ctrl+Alt+Del does not raise Windows security screen (that needs SendSAS from
        // a service, for the installer milestone), a Mac has no such chord, and generic Linux has no handler for it.
        MenuSeparator { visible: Remote.hostOs === "sodiumos" }
        MenuItem { objectName: "send-cad"; visible: Remote.hostOs === "sodiumos"; text: qsTr("Send Ctrl+Alt+Del"); onTriggered: Remote.sendCtrlAltDel() }
        MenuSeparator { }
        MenuSection { text: qsTr("Give the keyboard back with %1").arg(Fmt.leaveChord) }
    }
    Menu {
        id: kbMenuMac
        MenuSection { text: qsTr("Key mapping") }
        MenuItem { text: qsTr("Automatic"); radio: true; checked: Remote.keyMapping === "auto"; onTriggered: Remote.keyMapping = "auto" }
        MenuItem { text: qsTr("Pass through"); radio: true; checked: Remote.keyMapping === "pass"; onTriggered: Remote.keyMapping = "pass" }
        MenuSeparator { }
        MenuSection { text: qsTr("Give the keyboard back with %1").arg(Fmt.leaveChord) }
    }
}
