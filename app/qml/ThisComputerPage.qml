// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import SodiumUI

// "This computer" (Windows and generic Linux): the switch, the pairing code, the paired devices, start at login, the
// name shown to other computers, the firewall and network states, and who is controlling this computer. Everything
// the page says comes from the Host controller as ids and numbers. On Linux the desktop's permission (Host.consent: ask asking
// remembered declined unavailable) takes the firewall's place, and the tray, start at login and network rows are not there.
Item {
    id: page

    /// The product's name on a Windows computer: never translated, passed as %1.
    readonly property string brand: "Sodium Remote"
    readonly property bool on: Host.state === "on"
    readonly property bool linux: Qt.platform.os === "linux"
    /// Linux without a portal that offers RemoteDesktop and ScreenCast (X11, Sway, Hyprland): the page says so and offers no switch.
    readonly property bool unavailable: linux && Host.consent === "unavailable"

    function since(secs) {
        return secs > 0 ? qsTr("Since %1").arg(Format.timeOfDay(new Date(secs * 1000))) : ""
    }
    function lastUsed(secs) {
        return secs > 0 ? qsTr("Last used %1").arg(Format.relativeTime(new Date(secs * 1000))) : qsTr("Never used")
    }
    function clock(s) {
        const m = Math.floor(s / 60), r = s % 60
        return m + ":" + (r < 10 ? "0" : "") + r
    }

    Flickable {
        anchors.fill: parent
        contentWidth: width
        contentHeight: col.implicitHeight + 40
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        ScrollBar.vertical: ScrollBar { }

        ColumnLayout {
            id: col
            x: 20
            y: 16
            width: parent.width - 40
            spacing: 14

            // ---- being controlled
            Rectangle {
                objectName: "controlled-card"
                visible: Host.controlled
                Layout.fillWidth: true
                implicitHeight: ctl.implicitHeight + 28
                radius: 12
                color: Theme.accentSoft
                border.width: 1
                border.color: Theme.hairline
                RowLayout {
                    id: ctl
                    anchors.fill: parent
                    anchors.margins: 14
                    spacing: 14
                    Icon { name: "sodiumshell-remote-symbolic"; size: 28; color: Theme.accent }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2
                        WrappedLabel {
                            Layout.fillWidth: true
                            text: qsTr("%1 is controlling this computer").arg(Host.controller.name ?? "")
                            font: Theme.font("heading")
                            color: Theme.text
                        }
                        Text {
                            Layout.fillWidth: true
                            text: page.since(Host.controller.since ?? 0)
                            font: Theme.font("caption")
                            color: Theme.text2
                            wrapMode: Text.Wrap
                        }
                        Text {
                            objectName: "view-only-note"
                            visible: page.linux && !!Host.controller.view_only
                            Layout.fillWidth: true
                            text: qsTr("%1 can only watch: remote interaction wasn’t allowed in your desktop’s dialog.").arg(Host.controller.name ?? "")
                            font: Theme.font("caption")
                            color: Theme.text2
                            wrapMode: Text.Wrap
                        }
                    }
                    Button {
                        objectName: "disconnect-button"
                        variant: "destructive"
                        text: qsTr("Disconnect")
                        onClicked: Host.disconnectClient()
                    }
                }
            }

            // ---- the switch
            SettingsRowGroup {
                ControlRow {
                    Layout.fillWidth: true
                    text: qsTr("Allow remote control of this computer")
                    description: page.unavailable ? qsTr("Not available on this desktop")
                                                  : page.linux ? qsTr("Computers you’ve paired can see this screen and use its keyboard and mouse. Your desktop asks for permission when a device first connects.")
                                                  : qsTr("Computers you’ve paired can see this screen and use its keyboard and mouse.")
                    RowLayout {
                        spacing: 10
                        Text {
                            visible: !page.unavailable
                            text: Host.state === "starting" ? qsTr("Starting…") : page.on ? qsTr("On") : qsTr("Off")
                            font: Theme.font("caption")
                            color: page.on ? Theme.accent : Theme.text3
                        }
                        Switch {
                            objectName: "allow-switch"
                            enabled: !page.unavailable
                            checked: Host.enabled
                            onToggled: Host.setEnabled(checked)
                        }
                    }
                }
            }
            Banner {
                visible: Host.state === "error" && !page.unavailable
                Layout.fillWidth: true
                kind: "error"
                text: Host.errorCode === "in_use"
                    ? qsTr("Another %1 host is already sharing this computer. Turn it off or quit it, then try again.").arg(page.brand)
                    : qsTr("Remote control couldn’t start: %1").arg(Host.error)
            }

            // ---- Linux: this desktop can't share its screen (no portal with RemoteDesktop and ScreenCast)
            EmptyView {
                objectName: "unavailable-card"
                visible: page.unavailable
                Layout.fillWidth: true
                Layout.preferredHeight: implicitHeight
                iconName: "video-display-symbolic"
                title: qsTr("This desktop can’t share its screen yet")
                text: qsTr("%1 can’t share the screen on this desktop, because it doesn’t offer the screen sharing %1 needs. You can still connect to other computers from here.").arg(page.brand)
            }
            SettingsRowGroup {
                visible: page.unavailable
                ControlRow {
                    visible: Host.desktop !== ""
                    Layout.fillWidth: true
                    text: qsTr("Your desktop")
                    Text { text: Host.desktop; font: Theme.font("body"); color: Theme.text2; I18nAudit.userContent: true }
                }
                ControlRow {
                    Layout.fillWidth: true
                    text: qsTr("Works with")
                    Text { text: qsTr("GNOME and KDE Plasma on Wayland"); font: Theme.font("body"); color: Theme.text2; wrapMode: Text.Wrap }
                }
            }

            // ---- Linux: the desktop's permission to share the screen (the portal's own dialog, and its saved choice)
            SettingsRowGroup {
                objectName: "screen-sharing"
                visible: page.linux && !page.unavailable
                title: qsTr("Screen sharing")
                ControlRow {
                    Layout.fillWidth: true
                    text: qsTr("Screen sharing")
                    description: Host.consent === "asking" ? qsTr("Your desktop is asking to share the screen. Choose a display and Share in its dialog.")
                               : Host.consent === "remembered" ? qsTr("Other computers see the display you chose.")
                               : Host.consent === "declined" ? qsTr("Screen sharing wasn’t allowed, so other computers can’t see this screen. Your desktop asks again the next time a paired device connects.")
                               : qsTr("The first time a paired device connects, your desktop asks to share the screen. Someone needs to be at this computer to answer.")
                    Text {
                        objectName: "consent-state"
                        text: Host.consent === "asking" ? qsTr("Waiting for you")
                            : Host.consent === "remembered" ? qsTr("Allowed")
                            : Host.consent === "declined" ? qsTr("Not allowed")
                            : qsTr("Not asked yet")
                        font: Theme.font("caption", { weight: 600 })
                        color: Host.consent === "remembered" ? Theme.success : Host.consent === "declined" ? Theme.warning : Host.consent === "asking" ? Theme.accent : Theme.text3
                    }
                }
                ControlRow {
                    visible: Host.consent === "remembered"
                    Layout.fillWidth: true
                    text: qsTr("Permission")
                    description: qsTr("Your desktop remembers your choice, so connecting doesn’t ask again.")
                    RowLayout {
                        spacing: 10
                        Text { text: qsTr("Allowed until you forget it"); font: Theme.font("caption"); color: Theme.text2; wrapMode: Text.Wrap }
                        Button { objectName: "forget-consent"; text: qsTr("Forget permission…"); onClicked: forgetDialog.open() }
                    }
                }
            }

            // ---- software video: the download's progress, its failure, and the off state
            Rectangle {
                objectName: "codec-progress"
                visible: Host.codec.state === "downloading"
                Layout.fillWidth: true
                implicitHeight: dl.implicitHeight + 20
                radius: 10
                color: Theme.accentSoft
                border.width: 1
                border.color: Theme.hairline
                ColumnLayout {
                    id: dl
                    anchors.fill: parent
                    anchors.margins: 10
                    spacing: 6
                    RowLayout {
                        spacing: 10
                        Text {
                            Layout.fillWidth: true
                            text: qsTr("Downloading the video codec… %1").arg(Format.percent(Host.codec.percent ?? 0))
                            font: Theme.font("body"); color: Theme.text; wrapMode: Text.Wrap
                        }
                        Button { compact: true; text: qsTr("Cancel"); onClicked: Host.softwareVideo = false }
                    }
                    Rectangle {
                        Layout.fillWidth: true
                        implicitHeight: 4
                        radius: 2
                        color: Theme.hairline
                        Rectangle { width: parent.width * Math.min(1, (Host.codec.percent ?? 0) / 100); height: parent.height; radius: 2; color: Theme.accent }
                    }
                }
            }
            Banner {
                objectName: "codec-failed"
                // The engine says why: network (try again), rejected (the file is deleted and fetched again), unpinned (this build
                // can't download at all: no button) or cancelled (software video was switched off: no banner).
                readonly property string reason: Host.codec.reason ?? "network"
                visible: Host.codec.state === "failed" && reason !== "cancelled"
                Layout.fillWidth: true
                text: reason === "rejected" ? qsTr("The downloaded video codec couldn’t be used. Try again.")
                    : reason === "unpinned" ? qsTr("This build of %1 can’t download the video codec. Other computers can connect only if this computer has a hardware video encoder.").arg(page.brand)
                    : qsTr("Couldn’t download the video codec. Check this computer’s internet connection.")
                actionText: reason === "unpinned" ? "" : qsTr("Try again")
                onActionTriggered: Host.retryCodec()
            }
            Banner {
                objectName: "codec-off"
                visible: Host.codec.state === "off_blocked"
                Layout.fillWidth: true
                text: qsTr("Software video is off, and this computer has no hardware video encoder, so other computers can’t connect.")
                actionText: qsTr("Turn on")
                onActionTriggered: Host.softwareVideo = true
            }

            // ---- off: what that means
            EmptyView {
                visible: !Host.enabled && !page.unavailable
                Layout.fillWidth: true
                Layout.preferredHeight: 150
                iconName: "system-lock-screen-symbolic"
                title: qsTr("Remote control is off")
                text: qsTr("Nobody can connect to this computer. Paired devices stay paired.")
            }

            // ---- pairing
            SettingsRowGroup {
                visible: page.on
                title: qsTr("Pair a device")
                Item {
                    Layout.fillWidth: true
                    implicitHeight: pairCol.implicitHeight + 28
                    ColumnLayout {
                        id: pairCol
                        anchors.fill: parent
                        anchors.margins: 14
                        spacing: 12
                        // a code is open
                        ColumnLayout {
                            visible: Host.pairing.state === "open" || Host.pairing.state === "wrong"
                            Layout.fillWidth: true
                            spacing: 10
                            WrappedLabel {
                                Layout.fillWidth: true
                                text: qsTr("On the other computer, choose Add a computer and type this code.")
                                font: Theme.font("body")
                                color: Theme.text2
                            }
                            Flow {
                                Layout.fillWidth: true
                                spacing: 16
                                Row {
                                    spacing: 8
                                    Repeater {
                                        model: 6
                                        Rectangle {
                                            required property int index
                                            width: 40; height: 52
                                            radius: 10
                                            color: Theme.fill
                                            border.width: 1
                                            border.color: Theme.hairline
                                            Text {
                                                objectName: "code-digit-" + parent.index
                                                anchors.centerIn: parent
                                                text: (Host.pairing.code ?? "").charAt(parent.index)
                                                font: Theme.font("mono", { size: 26, weight: Font.DemiBold })
                                                color: Theme.text
                                            }
                                        }
                                    }
                                }
                                Text {
                                    objectName: "code-expires"
                                    anchors.verticalCenter: undefined
                                    text: qsTr("Expires in %1").arg(page.clock(Host.pairingSecondsLeft))
                                    font: Theme.font("caption")
                                    color: Theme.text2
                                    wrapMode: Text.Wrap
                                }
                            }
                            ButtonRow {
                                Layout.fillWidth: true
                                Button { text: qsTr("New code"); icon.name: "view-refresh-symbolic"; onClicked: Host.openPairing() }
                                Button { text: qsTr("Close pairing"); onClicked: Host.closePairing() }
                            }
                        }
                        // no code open
                        RowLayout {
                            visible: !(Host.pairing.state === "open" || Host.pairing.state === "wrong")
                            Layout.fillWidth: true
                            spacing: 14
                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: 2
                                // Three wrong codes closed it: it stays closed until it is opened here again.
                                readonly property bool wrongCodes: Host.pairing.state === "closed" && Host.pairing.reason === "attempts_used"
                                WrappedLabel {
                                    objectName: "pairing-heading"
                                    Layout.fillWidth: true
                                    text: parent.wrongCodes ? qsTr("Pairing closed") : qsTr("No pairing code is open")
                                    font: Theme.font("heading")
                                    color: Theme.text
                                }
                                WrappedLabel {
                                    Layout.fillWidth: true
                                    text: parent.wrongCodes ? qsTr("Someone typed three wrong codes, so pairing closed. Nobody can pair until you open it again.")
                                                            : qsTr("A code works once, for 5 minutes, and three wrong tries withdraw it.")
                                    font: Theme.font("body")
                                    color: Theme.text2
                                }
                            }
                            Button { objectName: "pair-button"; variant: "primary"; text: qsTr("Pair a device…"); onClicked: Host.openPairing() }
                        }
                    }
                }
            }

            // ---- paired devices
            SettingsRowGroup {
                title: qsTr("Paired devices")
                Text {
                    visible: Host.paired.length === 0
                    Layout.fillWidth: true
                    Layout.margins: 18
                    horizontalAlignment: Text.AlignHCenter
                    text: qsTr("No paired devices yet")
                    font: Theme.font("body")
                    color: Theme.text2
                    wrapMode: Text.Wrap
                }
                Repeater {
                    model: Host.paired
                    ControlRow {
                        required property var modelData
                        Layout.fillWidth: true
                        text: modelData.name
                        description: (Fmt.osName(modelData.os ?? "") !== "" ? Fmt.osName(modelData.os) + " · " : "") + page.lastUsed(modelData.lastConnected ?? 0)
                        RowLayout {
                            spacing: 8
                            Text { visible: modelData.live; text: qsTr("Connected now"); font: Theme.font("caption"); color: Theme.accent }
                            Button {
                                objectName: "remove-" + modelData.id
                                variant: "destructive"
                                compact: true
                                text: qsTr("Remove")
                                Accessible.name: qsTr("Remove %1").arg(modelData.name)
                                onClicked: { removeDialog.deviceId = modelData.id; removeDialog.deviceName = modelData.name; removeDialog.open() }
                            }
                        }
                    }
                }
            }

            // ---- general
            SettingsRowGroup {
                ControlRow {
                    visible: !page.linux // Windows: the Run key. Linux has no start at sign-in yet.
                    Layout.fillWidth: true
                    text: qsTr("Start at login")
                    description: qsTr("Keeps this computer available after you sign in.")
                    Switch { objectName: "login-switch"; checked: Host.startAtLogin; onToggled: Host.setStartAtLogin(checked) }
                }
                // Windows: a virtual display the size of the other computer's window; the driver goes in with one UAC prompt.
                ControlRow {
                    visible: !page.linux
                    Layout.fillWidth: true
                    text: qsTr("Match the display to the connecting computer")
                    description: qsTr("While someone is connected, Windows uses a virtual display the size of their window, and this computer’s screens turn off. This needs a display driver, installed once with administrator permission.")
                    Switch {
                        objectName: "matched-switch"
                        checked: Host.matchedDisplay
                        enabled: !Host.matchedBusy && !Host.controlled
                        // Back to the controller's state: it changes only once the driver is in or out.
                        onToggled: { Host.setMatchedDisplay(checked); checked = Qt.binding(() => Host.matchedDisplay) }
                    }
                }
                ControlRow {
                    objectName: "matched-failed"
                    visible: !page.linux && Host.matchedFailed
                    Layout.fillWidth: true
                    text: qsTr("The virtual display couldn’t start. Showing this computer’s screen instead.")
                }
                ControlRow {
                    Layout.fillWidth: true
                    text: qsTr("Name shown to other computers")
                    TextField {
                        id: nameField
                        objectName: "name-field"
                        implicitWidth: 200
                        text: Host.name
                        I18nAudit.userContent: true
                        Accessible.name: qsTr("Name shown to other computers")
                        onEditingFinished: Host.setName(text)
                    }
                }
            }

            // ---- network
            SettingsRowGroup {
                visible: !page.linux
                title: qsTr("Network")
                // the firewall rule
                ControlRow {
                    visible: Host.networkPublic
                    Layout.fillWidth: true
                    text: qsTr("This network is Public")
                    description: qsTr("Remote control needs a Private network. Set this network to Private in Windows network settings.")
                    Button { text: qsTr("Network settings"); onClicked: Host.openNetworkSettings() }
                }
                ControlRow {
                    visible: !Host.networkPublic && Host.firewall === "blocked"
                    Layout.fillWidth: true
                    text: qsTr("The firewall is blocking %1").arg(page.brand)
                    description: qsTr("Other computers on your network can’t reach this one until you allow it.")
                    Button { objectName: "allow-firewall"; variant: "primary"; text: qsTr("Allow…"); onClicked: Host.allowFirewall() }
                }
                ControlRow {
                    visible: !Host.networkPublic && Host.firewall === "wait"
                    Layout.fillWidth: true
                    text: qsTr("Waiting for Windows to ask…")
                    description: qsTr("Other computers on your network can’t reach this one until you allow it.")
                    Spinner { color: Theme.accent }
                }
                ControlRow {
                    visible: !Host.networkPublic && Host.firewall === "ok"
                    Layout.fillWidth: true
                    text: qsTr("Allowed on private networks")
                    description: qsTr("Windows Firewall allows it. Public networks stay blocked.")
                    Icon { name: "object-select-symbolic"; size: 18; color: Theme.accent }
                }
                ControlRow {
                    visible: Host.address !== ""
                    Layout.fillWidth: true
                    text: qsTr("Address for connecting directly")
                    description: Host.ipAddress
                    I18nAudit.userContent: true
                    Row {
                        spacing: 6
                        Button { objectName: "copy-address"; text: qsTr("Copy"); onClicked: Host.copyAddress(false) }
                        IconButton {
                            id: addressMore
                            objectName: "address-more"
                            icon.name: "view-more-horizontal-symbolic"
                            tooltip: Host.address
                            onClicked: addressMenu.popup(this, 0, height + 4)
                        }
                    }
                }
            }

            // ---- Linux: what the portal can't do
            SettingsRowGroup {
                objectName: "limits"
                visible: page.linux && !page.unavailable
                title: qsTr("Limits")
                Repeater {
                    // The lock line depends on the desktop: GNOME ends the session on lock, KDE keeps it and shows the lock screen.
                    model: {
                        const lines = [qsTr("This computer keeps its own resolution. The picture is scaled to fit the other computer’s window."),
                            Host.desktopId === "kde"
                                ? qsTr("While this computer is locked, a paired device sees the lock screen and can unlock it with your password. Nobody can connect before you sign in.")
                                : qsTr("Locking this computer ends the session. Nobody can connect while it’s locked or before you sign in."),
                            qsTr("Your desktop shows its own indicator while the screen is shared."),
                            qsTr("Remote control works only while %1 is open.").arg(page.brand)]
                        if (Host.desktopId === "gnome")
                            lines.push(qsTr("Only characters on this computer’s keyboard layout can be typed from the other computer. Paste other text instead."))
                        return lines
                    }
                    ControlRow {
                        required property string modelData
                        Layout.fillWidth: true
                        text: modelData
                    }
                }
                ControlRow {
                    Layout.fillWidth: true
                    text: qsTr("To share without keeping the app open, see the Linux guide.")
                    Button { objectName: "linux-guide"; text: qsTr("Open guide"); onClicked: Host.openLinuxGuide() }
                }
            }

            Text {
                visible: !page.linux
                Layout.fillWidth: true
                Layout.leftMargin: 4
                text: qsTr("Lock, sign-in and UAC screens can’t be shown or controlled remotely.")
                font: Theme.font("caption")
                color: Theme.text2
                wrapMode: Text.Wrap
            }
        }
    }

    /// Screenshots (--script `forget-dialog`): the confirmation.
    function openForget() { forgetDialog.open() }

    /// Screenshots (--script `grab-page FILE`): the whole page, however tall.
    function grabAll(file) { col.grabToImage(function (r) { r.saveToFile(file) }) }

    /// Screenshots and tests (--script): the address menu.
    function openAddressMenu() { addressMenu.popup(addressMore, 0, addressMore.height + 4) }

    // With more than one address (a VPN, a virtual switch) every IP:PORT is its own item; the full ID@IP:PORT is the last.
    Menu {
        id: addressMenu
        Repeater {
            model: Host.ipAddresses.length > 1 ? Host.ipAddresses : []
            MenuItem {
                required property string modelData
                objectName: "copy-ip-item"
                text: qsTr("Copy %1").arg(modelData)
                I18nAudit.userContent: true
                onTriggered: Host.copyIpAddress(modelData)
            }
        }
        MenuSeparator { visible: Host.ipAddresses.length > 1 }
        MenuItem { objectName: "copy-full-item"; text: qsTr("Copy full address"); onTriggered: Host.copyAddress(true) }
    }

    ContentDialog {
        id: forgetDialog
        objectName: "forget-dialog"
        title: qsTr("Forget the screen sharing permission?")
        text: qsTr("Your desktop asks again the next time a paired device connects.")
        danger: true
        acceptText: qsTr("Forget permission")
        onAccepted: Host.forgetConsent()
    }

    ContentDialog {
        id: removeDialog
        property string deviceId: ""
        property string deviceName: ""
        title: qsTr("Remove %1?").arg(deviceName)
        text: qsTr("It can’t connect to this computer again until you pair it again. If it’s connected now, it’s disconnected.")
        danger: true
        acceptText: qsTr("Remove")
        onAccepted: Host.revoke(deviceId)
    }
}
