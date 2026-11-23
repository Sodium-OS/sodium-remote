// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import SodiumUI

// Settings › About › Third-party notices: a short list of what Sodium Remote is built on, and
// the notices file that carries the full text (cargo-about's list of the Rust libraries, then the text of every other license; stage-app.sh writes it for the Windows zip; it sits next to the program).
// The component names and license ids are proper names and are never translated.
ContentDialog {
    id: dialog

    /// The components of this build: [name, licenses]. The Rust libraries line is the one cargo-about expands in the file.
    readonly property var components: Qt.platform.os === "windows"
        ? [["Qt 6", "LGPL-3.0"], ["FFmpeg", "LGPL-2.1+"], ["OpenH264", "BSD-2-Clause"], ["Opus", "BSD-3-Clause"], ["Rust libraries", "MIT, Apache-2.0, others"]]
        : [["Qt 6", "LGPL-3.0"], ["FFmpeg", "LGPL-2.1+"], ["Opus", "BSD-3-Clause"], ["Rust libraries", "MIT, Apache-2.0, others"]]

    title: qsTr("Third-party notices")
    text: qsTr("Sodium Remote is built on this software.")
    rejectText: ""
    acceptText: qsTr("Done")
    extraButtons: [
        Button {
            objectName: "notices-open"
            text: qsTr("Open the notices file")
            enabled: Remote.noticesPath !== ""
            onClicked: Remote.openNotices()
        }
    ]

    ColumnLayout {
        Layout.fillWidth: true
        spacing: 10

        SettingsRowGroup {
            Layout.fillWidth: true
            Repeater {
                model: dialog.components
                delegate: ControlRow {
                    Layout.fillWidth: true
                    text: modelData[0]
                    Text { text: modelData[1]; font: Theme.font("body"); color: Theme.text2 }
                }
            }
        }
        Text {
            Layout.fillWidth: true
            text: qsTr("The full text of every license is in the notices file next to the app.")
            font: Theme.font("caption"); color: Theme.text2; wrapMode: Text.Wrap
        }
        // Cisco's required wording, as Cisco has it, in every language (Windows: the codec can be downloaded).
        Text {
            visible: Qt.platform.os === "windows"
            Layout.fillWidth: true
            text: "OpenH264 Video Codec provided by Cisco Systems, Inc."
            font: Theme.font("caption"); color: Theme.text2; wrapMode: Text.Wrap
        }
    }
}
