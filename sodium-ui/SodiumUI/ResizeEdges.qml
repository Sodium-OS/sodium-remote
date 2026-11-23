// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick

// Edge and corner grips for a frameless window: pressing one hands the resize
// to the compositor (startSystemResize), so it is as smooth as any window.
// AppWindow includes it; add it yourself to other frameless windows.
//
//     ResizeEdges { anchors.fill: parent; z: 1000 }
Item {
    id: edges

    property var window: Window.window
    /// Grip thickness along the edges, logical px.
    property int grip: 6
    /// Corner grip size.
    property int corner: 14

    function start(e) {
        if (edges.window)
            edges.window.startSystemResize(e)
    }

    component Grip: MouseArea {
        required property int edgesMask
        acceptedButtons: Qt.LeftButton
        hoverEnabled: true
        onPressed: edges.start(edgesMask)
    }

    Grip { edgesMask: Qt.TopEdge; cursorShape: Qt.SizeVerCursor
        x: edges.corner; y: 0; width: edges.width - 2 * edges.corner; height: edges.grip }
    Grip { edgesMask: Qt.BottomEdge; cursorShape: Qt.SizeVerCursor
        x: edges.corner; y: edges.height - edges.grip; width: edges.width - 2 * edges.corner; height: edges.grip }
    Grip { edgesMask: Qt.LeftEdge; cursorShape: Qt.SizeHorCursor
        x: 0; y: edges.corner; width: edges.grip; height: edges.height - 2 * edges.corner }
    Grip { edgesMask: Qt.RightEdge; cursorShape: Qt.SizeHorCursor
        x: edges.width - edges.grip; y: edges.corner; width: edges.grip; height: edges.height - 2 * edges.corner }
    Grip { edgesMask: Qt.TopEdge | Qt.LeftEdge; cursorShape: Qt.SizeFDiagCursor
        x: 0; y: 0; width: edges.corner; height: edges.corner }
    Grip { edgesMask: Qt.TopEdge | Qt.RightEdge; cursorShape: Qt.SizeBDiagCursor
        x: edges.width - edges.corner; y: 0; width: edges.corner; height: edges.corner }
    Grip { edgesMask: Qt.BottomEdge | Qt.LeftEdge; cursorShape: Qt.SizeBDiagCursor
        x: 0; y: edges.height - edges.corner; width: edges.corner; height: edges.corner }
    Grip { edgesMask: Qt.BottomEdge | Qt.RightEdge; cursorShape: Qt.SizeFDiagCursor
        x: edges.width - edges.corner; y: edges.height - edges.corner; width: edges.corner; height: edges.corner }
}
