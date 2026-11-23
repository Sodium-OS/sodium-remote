// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import SodiumUI

// Settings, for every computer: Display, Quality and Input. They apply at the next connection.
Sheet {
    id: sheet

    property int tab: 0
    function openNotices() { notices.open() }
    /// The tabs: Display 0, Quality 1, Input 2, Server 3, Advanced 4, About 5 (the board's order).
    readonly property int serverTab: 3
    readonly property int advancedTab: 4
    readonly property int aboutTab: 5
    /// Opens on Settings › Server with the confirmation of a link (one pasted or opened from outside the app).
    function openServerLink(link) {
        tab = serverTab
        open()
        serverSheet.beginLink(link)
    }
    /// Opens on Settings › Server.
    function openServer() { tab = serverTab; open() }
    /// The server's sheets, for scripts: add, change, remove, link TEXT.
    function serverAction(what, arg) {
        tab = serverTab
        open()
        if (what === "add") serverSheet.beginAdd()
        else if (what === "change") serverSheet.beginChange()
        else if (what === "remove") removeDialog.open()
        else if (what === "link") serverSheet.beginLink(arg)
        else if (what === "manual") { serverSheet.beginAdd(); serverSheet.stage = "manual"; serverSheet.address = arg }
    }
    readonly property var bandwidthSteps: [0, 5, 10, 20, 40, 80]
    readonly property var refreshSteps: [0, 30, 60, 90, 120, 144]

    width: Math.min(640, parent ? parent.width - 32 : 640)   // inside the window at its 480 px minimum
    height: Math.min(560, (parent ? parent.height : 560) - 72)

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 24
        spacing: 12

        ColumnLayout {
            spacing: 2
            Text { text: qsTr("Settings"); font: Theme.font("title"); color: Theme.text }
            Text {
                Layout.fillWidth: true
                text: qsTr("For every computer · changes apply at the next connection")
                font: Theme.font("body"); color: Theme.text2; wrapMode: Text.Wrap
            }
        }

        SegmentedControl {
            objectName: "settings-tabs"
            Layout.fillWidth: true
            tabs: true
            model: [qsTr("Display"), qsTr("Quality"), qsTr("Input"), qsTr("Server"), qsTr("Advanced"), qsTr("About")]
            currentIndex: sheet.tab
            onActivated: i => sheet.tab = i
        }

        Flickable {
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: width
            contentHeight: pages.implicitHeight
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            ScrollBar.vertical: ScrollBar { }

            ColumnLayout {
                id: pages
                width: parent.width
                spacing: 16

                // ---- Display
                SettingsRowGroup {
                    visible: sheet.tab === 0
                    Layout.fillWidth: true
                    title: qsTr("Resolution")
                    footer: qsTr("Windows and macOS computers keep their own resolution. The picture is scaled to fit this window.")
                    ControlRow {
                        Layout.fillWidth: true
                        text: qsTr("Match the window")
                        description: qsTr("SodiumOS computers only · follows every resize")
                        Switch {
                            objectName: "set-match-window"
                            checked: !Remote.fixedResolution
                            onToggled: Remote.fixedResolution = !checked
                        }
                    }
                    ControlRow {
                        Layout.fillWidth: true
                        text: qsTr("Refresh rate")
                        description: qsTr("Automatic follows this display")
                        DropDownButton {
                            objectName: "set-refresh"
                            model: sheet.refreshSteps.map(hz => hz === 0 ? qsTr("Automatic") : qsTr("%1 Hz").arg(Format.number(hz)))
                            currentIndex: Math.max(0, sheet.refreshSteps.indexOf(Remote.refreshHz))
                            onActivated: i => Remote.refreshHz = sheet.refreshSteps[i]
                        }
                    }
                }

                // ---- Quality
                SettingsRowGroup {
                    visible: sheet.tab === 1
                    Layout.fillWidth: true
                    title: qsTr("Video")
                    ControlRow {
                        Layout.fillWidth: true
                        text: qsTr("Video codec")
                        description: qsTr("Automatic uses HEVC when both computers support it, and H.264 otherwise.")
                        DropDownButton {
                            objectName: "set-codec"
                            readonly property var codecs: ["auto", "hevc", "h264"]
                            // AV1 is never forced: the host picks it by itself where both computers have it.
                            model: [qsTr("Automatic"), qsTr("HEVC"), qsTr("H.264")]
                            currentIndex: Math.max(0, codecs.indexOf(Remote.codec))
                            onActivated: i => Remote.codec = codecs[i]
                        }
                    }
                    ControlRow {
                        Layout.fillWidth: true
                        text: qsTr("Bandwidth limit")
                        description: qsTr("The stream never goes above this.")
                        DropDownButton {
                            objectName: "set-bandwidth"
                            model: sheet.bandwidthSteps.map(m => m === 0 ? qsTr("Automatic") : qsTr("%1 Mbit/s").arg(Format.number(m)))
                            currentIndex: Math.max(0, sheet.bandwidthSteps.indexOf(Remote.bandwidthMbps))
                            onActivated: i => Remote.bandwidthMbps = sheet.bandwidthSteps[i]
                        }
                    }
                }

                // ---- Input
                SettingsRowGroup {
                    visible: sheet.tab === 2
                    Layout.fillWidth: true
                    title: qsTr("Keyboard")
                    footer: qsTr("To give the keyboard back to this computer while streaming, press %1.").arg(Fmt.leaveChord)
                    ControlRow {
                        Layout.fillWidth: true
                        text: qsTr("Key mapping")
                        description: qsTr("Automatic maps ⌘ to Ctrl and Ctrl to ⌘ between a Mac and a PC.")
                        SegmentedControl {
                            objectName: "set-keymap"
                            readonly property var maps: ["auto", "pass"]
                            model: [qsTr("Automatic"), qsTr("Pass through")]
                            currentIndex: Math.max(0, maps.indexOf(Remote.keyMapping))
                            onActivated: i => Remote.keyMapping = maps[i]
                        }
                    }
                }

                // ---- Server
                ServerPage {
                    id: serverPage
                    visible: sheet.tab === sheet.serverTab
                    Layout.fillWidth: true
                    onAddRequested: { notice = ""; serverSheet.beginAdd() }
                    onChangeRequested: { notice = ""; serverSheet.beginChange() }
                    onRemoveRequested: { notice = ""; removeDialog.open() }
                    onImportRequested: { notice = ""; sheet.files("import") }
                    onExportRequested: { notice = ""; sheet.files("export") }
                }

                // ---- Advanced
                SettingsRowGroup {
                    visible: sheet.tab === sheet.advancedTab
                    Layout.fillWidth: true
                    title: qsTr("Computers")
                    ControlRow {
                        Layout.fillWidth: true
                        text: qsTr("Show previews")
                        description: qsTr("Shows a picture of each computer in the list. The pictures stay on this computer, and turning this off deletes them.")
                        Switch {
                            objectName: "set-previews"
                            checked: Remote.showPreviews
                            onToggled: Remote.showPreviews = checked
                        }
                    }
                }
                // Windows only: the one-time codec download; no other system fetches anything.
                SettingsRowGroup {
                    visible: sheet.tab === sheet.advancedTab && Qt.platform.os === "windows"
                    Layout.fillWidth: true
                    title: qsTr("Software video")
                    ControlRow {
                        Layout.fillWidth: true
                        text: qsTr("Download OpenH264 when needed")
                        description: qsTr("Encodes video on this computer when no graphics hardware can. It comes from Cisco and is downloaded once.")
                        Switch {
                            objectName: "set-software-video"
                            checked: Qt.platform.os === "windows" && !!Host && Host.softwareVideo
                            onToggled: Host.softwareVideo = checked
                        }
                    }
                }

                // ---- About
                AboutPage {
                    visible: sheet.tab === sheet.aboutTab
                    Layout.fillWidth: true
                    onNoticesRequested: notices.open()
                }
            }
        }

        RowLayout {
            Layout.fillWidth: true
            Item { Layout.fillWidth: true }
            Button { objectName: "settings-done"; text: qsTr("Done"); variant: "primary"; onClicked: sheet.close() }
        }
    }

    NoticesSheet { id: notices }

    // The file choosers are loaded when first asked for.
    function files(what) {
        pickers.active = true
        const p = pickers.item
        if (!p) return
        if (what === "import") p.chooseImport()
        else if (what === "export") p.chooseExport()
        else p.chooseCertificate()
    }
    Loader {
        id: pickers
        active: false
        source: "ServerFiles.qml"
        onLoaded: {
            item.certificateChosen.connect((name, pem) => serverSheet.setCertificate(name, pem))
            item.importChosen.connect(file => {
                const c = Remote.readServerFile(file)
                if (c.ok) serverSheet.beginConfirm(c)
                else serverPage.notice = qsTr("That file doesn’t describe a server.")
            })
            item.exportChosen.connect(file => {
                if (!Remote.exportServer(file).ok) serverPage.notice = qsTr("Couldn’t save the file.")
            })
        }
    }
    ServerSheet {
        id: serverSheet
        onChooseCertificate: sheet.files("certificate")
    }
    ContentDialog {
        id: removeDialog
        objectName: "server-remove-dialog"
        readonly property string name: Remote.server.official ? qsTr("Sodium Remote service") : (Remote.server.host ?? "")
        title: qsTr("Remove %1?").arg(name)
        text: qsTr("This computer stops using the server. Paired computers stay paired, and computers on this network and by address still work.")
        danger: true
        acceptText: qsTr("Remove")
        onAccepted: Remote.removeServer()
    }
}
