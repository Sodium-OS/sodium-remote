// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Dialogs

// The file choosers of Settings › Server: a certificate authority file, a server.toml to import and where to export one. Loaded on
// demand (a Loader in SettingsSheet), so the Qt Quick Dialogs module is only needed when a person asks for a file.
// The system's own chooser where the platform has one (Windows, a desktop portal), Qt's otherwise.
Item {
    id: files

    signal certificateChosen(string name, string pem)
    signal importChosen(url file)
    signal exportChosen(url file)

    function chooseCertificate() { certificate.open() }
    function chooseImport() { importer.open() }
    function chooseExport() { exporter.open() }

    FileDialog {
        id: certificate
        title: qsTr("Choose the server’s certificate authority file.")
        fileMode: FileDialog.OpenFile
        nameFilters: [qsTr("Certificates (*.pem *.crt *.cer)"), qsTr("All files (*)")]
        onAccepted: {
            const c = Remote.readCertificate(selectedFile)
            files.certificateChosen(c.name, c.pem)
        }
    }
    FileDialog {
        id: importer
        title: qsTr("Choose a server.toml file")
        fileMode: FileDialog.OpenFile
        nameFilters: [qsTr("Server settings (*.toml)"), qsTr("All files (*)")]
        onAccepted: files.importChosen(selectedFile)
    }
    FileDialog {
        id: exporter
        title: qsTr("Export the server settings")
        fileMode: FileDialog.SaveFile
        defaultSuffix: "toml"
        selectedFile: "file:///server.toml"
        nameFilters: [qsTr("Server settings (*.toml)")]
        onAccepted: files.exportChosen(selectedFile)
    }
}
