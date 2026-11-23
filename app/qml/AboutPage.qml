// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Layouts
import SodiumUI

// Settings › About: the release, its license, where the source is (AGPL-3.0 section 13), the
// third-party notices, and what the app doesn't do. Product and license names are never translated.
ColumnLayout {
    id: page

    signal noticesRequested()
    spacing: 16

    RowLayout {
        Layout.fillWidth: true
        spacing: 14
        Image {
            source: "../windows/icon.svg"
            sourceSize: Qt.size(56, 56)
            Layout.preferredWidth: 56
            Layout.preferredHeight: 56
        }
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2
            RowLayout {
                spacing: 8
                Text { text: "Sodium Remote"; font: Theme.font("title"); color: Theme.text }
                Rectangle {
                    implicitWidth: previewLabel.implicitWidth + 14
                    implicitHeight: 20
                    radius: 10
                    color: Theme.accentSoft
                    Text { id: previewLabel; anchors.centerIn: parent; text: qsTr("Preview"); font: Theme.font("caption"); color: Theme.accent }
                }
            }
            Text { text: qsTr("Version %1").arg(Remote.appVersion); font: Theme.font("body"); color: Theme.text2 }
        }
    }

    SettingsRowGroup {
        Layout.fillWidth: true
        ControlRow {
            objectName: "about-license"
            Layout.fillWidth: true
            text: qsTr("License")
            Text { text: qsTr("AGPL-3.0 or commercial license"); font: Theme.font("body"); color: Theme.text2 }
        }
        ControlRow {
            objectName: "about-source"
            Layout.fillWidth: true
            text: qsTr("Source code")
            // The address is a URL, never translated; it is what the AGPL asks the app to offer.
            description: Remote.sourceUrl.replace(/^https:\/\//, "")
            Button {
                objectName: "about-source-open"
                compact: true
                text: qsTr("Open")
                onClicked: Remote.openSource()
            }
        }
        ControlRow {
            objectName: "about-notices"
            Layout.fillWidth: true
            text: qsTr("Third-party notices")
            description: qsTr("The licenses of the software Sodium Remote includes")
            IconButton {
                objectName: "about-notices-open"
                icon.name: "pan-end-symbolic"
                tooltip: qsTr("Third-party notices")
                onClicked: page.noticesRequested()
            }
        }
    }

    Text {
        objectName: "about-footer"
        Layout.fillWidth: true
        text: qsTr("Sodium Remote doesn’t check for updates and doesn’t send usage data.")
        font: Theme.font("caption"); color: Theme.text2; wrapMode: Text.Wrap
    }
}
