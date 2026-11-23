// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick

// A short UI label that may wrap (docs/decisions/i18n.md, decisions 20, 89 and 92; LangRules2 R89): dialog and
// page titles, Control Center tiles, Settings row and form labels. Running text (descriptions, messages, help)
// stays a StyledText with Text.Wrap, and buttons never wrap.
//
//     WrappedLabel { text: qsTr("Lock and log in"); style: "control"; Layout.fillWidth: true }
//     WrappedLabel { text: qsTr("Energy mode"); style: "tile"; horizontalAlignment: Text.AlignHCenter; width: 92 }
//
// - **Balanced lines**, every language: when the label wraps, its lines are laid out in the narrowest width that
//   keeps their count ("ロックとログイ / ン" becomes "ロックと / ログイン", "Show seconds in the bar / now" becomes
//   "Show seconds in / the bar now"). The item keeps the width it is given; `lineWidth` is the width the lines use,
//   placed by horizontalAlignment. implicitWidth is the label on one line, as a Text's is.
// - **Korean** breaks at spaces, inside a word only when the word is wider than the line: Qt's own breaking.
// - **Phrase marks** (U+200B, which zh and ja translators place only where the visual review asks): the label then
//   breaks only at them, or anywhere in a phrase wider than the line.
// - **Soft hyphens** (U+00AD at a compound joint, de, fr, es and English translators, decision 92): Qt breaks there
//   and draws the hyphen, and the balancing counts it. Otherwise a word breaks only at a space or a hyphen;
//   Text.Wrap's break anywhere stays as the no-clip safety net, and the overflow audit fails it.
// - Never a smaller font, never elided, no maximum line count. The accessible name is the text without its break
//   marks (I18n.stripBreakMarks); the overflow audit treats it as a short label (I18nAudit.shortLabel): in zh and ja
//   a one-character last line is a hit.
// - Plain text: a label with markup wraps as a StyledText does, unbalanced.
Item {
    id: root

    /// The label (a UI string).
    property string text: ""
    /// A type style of Theme.font(): body (row and form labels), dialogTitle, heading, control ...
    property alias style: label.style
    property alias color: label.color
    property alias font: label.font
    property alias horizontalAlignment: label.horizontalAlignment
    /// Balance the lines (true). False: Qt's greedy lines, as a StyledText.
    property bool balanced: true
    readonly property alias lineCount: label.lineCount
    /// The width the lines are laid out in: `width`, or less when balanced.
    readonly property real lineWidth: label.width
    /// The half-leading above each line (StyledText's), for padding of your own.
    readonly property alias leading: label.leading
    /// The Text it draws (tests: I18n.lineBreakFault(label.textItem)).
    readonly property alias textItem: label

    implicitWidth: label.implicitWidth
    implicitHeight: label.implicitHeight

    Accessible.role: Accessible.StaticText
    Accessible.name: I18n.stripBreakMarks(text)

    readonly property string _shown: I18n.joinPhrases(text)
    // Measured only when it wraps; a label on one line costs nothing.
    readonly property real _lineWidth: balanced && width > 0 && label.implicitWidth > width + 0.5
        ? I18n.balancedWidth(_shown, label.font, width) : width

    StyledText {
        id: label
        text: root._shown
        wrapMode: Text.Wrap
        width: root._lineWidth
        x: effectiveHorizontalAlignment === Text.AlignRight ? root.width - width
         : effectiveHorizontalAlignment === Text.AlignHCenter ? (root.width - width) / 2 : 0
        Accessible.ignored: true
        I18nAudit.shortLabel: true
    }
}
