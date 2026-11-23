// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Templates as T

// A section title inside a Menu (P2Bar menus): 26 px, 11 px 600 uppercase,
// tracking 0.3, #9EA4B2, 10 px inset. Not focusable, so keyboard navigation
// skips it. (Not a MenuSeparator: Qt collapses those at the top of a menu
// and next to each other.)
//
//     MenuSection { text: "Move to" }
T.Control {
    id: control

    property string text: ""
    readonly property var metrics: Theme.metrics.controls.menu

    implicitWidth: implicitContentWidth + leftPadding + rightPadding
    implicitHeight: metrics.sectionHeight
    leftPadding: metrics.itemPaddingX
    rightPadding: metrics.itemPaddingX
    topPadding: 0
    bottomPadding: 4
    focusPolicy: Qt.NoFocus
    Accessible.role: Accessible.Heading
    Accessible.name: I18n.stripBreakMarks(text)

    // Never elides (decision 20): the menu is as wide as its longest row, this one included.
    contentItem: StyledText {
        text: control.text
        style: "section"
        font: Theme.font("section", { tracking: 0.3 })
        color: Theme.text3
        verticalAlignment: Text.AlignBottom
    }
}
