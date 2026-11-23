// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Templates as T

// Menu separator (Components board): a 1 px white 8% line, inset 8 px, with
// 5 px above and below. Keyboard navigation skips it.
T.MenuSeparator {
    id: control

    readonly property var metrics: Theme.metrics.controls.menu

    implicitWidth: Math.max(implicitBackgroundWidth, implicitContentWidth + leftPadding + rightPadding)
    implicitHeight: implicitContentHeight + topPadding + bottomPadding
    leftPadding: metrics.separatorInset
    rightPadding: metrics.separatorInset
    topPadding: metrics.separatorGap
    bottomPadding: metrics.separatorGap

    contentItem: Rectangle {
        implicitWidth: 100
        implicitHeight: 1
        color: Theme.divider
    }
}
