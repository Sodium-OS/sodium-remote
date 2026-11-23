// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick

// The text base of every SodiumUI component (docs/decisions/i18n.md, decisions 44, 45, 47, 52, 56, 57):
// a Text in a type style, with that style's pixel line height in the UI language.
//
//     StyledText { text: qsTr("Night light"); style: "control" }
//     StyledText { text: qsTr("Turns the screen warmer after sunset."); style: "body"; color: Theme.text2
//                  wrapMode: Text.Wrap; Layout.fillWidth: true }                 // multi-line: wraps
//     StyledText { text: Format.timeOfDay(now); style: "bar"; numeric: true }   // tabular digits
//
// - **One line** (wrapMode NoWrap): the style's `line` height (control 13/16, label 12/15, heading
//   15/18, title 20/25) in every language, so nothing changes height on a language switch.
// - **Wrapping text** (any wrapMode): Latin text keeps today's line height (the style's `lineHeight`
//   ratio, else the font's own); in zh, ja and ko, text on several lines takes the style's `lineCjk`
//   (body 13/20, caption 12/18, heading 15/22, title 20/28, message 1.5×) and a single line the Latin
//   height. Multi-line UI text uses Text.Wrap, never WordWrap (decision 57: it breaks at words, then
//   anywhere, so German compounds and URLs fit).
// - **Centring:** Text.FixedHeight with half the extra leading above (topPadding = floor(extra / 2),
//   bottomPadding = −that), so the glyphs sit in the middle of each line and the item's height is
//   lines × line height. Padding of your own: add `leading` to your topPadding and subtract it from
//   your bottomPadding.
// - **Font:** Theme.font(style): the language's CJK family after Inter, typographic line metrics, no
//   uppercase or tracking in CJK, Japanese `palt` on single-line styles only. Setting `font` yourself
//   keeps the line height of `style`.
// - **numeric:** tabular digits (decision 56): clocks, timers, live percentages, numeric columns.
// Apps never set lineHeight: they pick a style.
Text {
    id: root

    /// A type style of Theme.font(): body, caption, control, heading, title, message, label, ...
    property string style: "body"
    /// Tabular digits (decision 56). Never on body text.
    property bool numeric: false
    /// Latin multi-line text only: a line height ratio of its own (EmptyView's 1.5); 0: the style's.
    property real latinLineRatio: 0
    /// It wraps (wrapMode set): the multi-line heights apply.
    readonly property bool multiLine: wrapMode !== Text.NoWrap
    /// It wraps and needs more than one line at its width (its one-line width is wider, or it breaks
    /// lines itself). Only zh, ja and ko use it. Worked out after the text's layout, never inside it: a
    /// line height changed while Qt lays the text out would re-enter that layout.
    readonly property bool severalLines: _severalLines
    /// The pixel line height in effect; 0: the font's own.
    readonly property real styleLineHeight: Theme.lineHeight(style, multiLine, latinLineRatio, severalLines)
    /// The half-leading put above each line (px).
    readonly property real leading: styleLineHeight > 0 ? Math.max(0, Math.floor((styleLineHeight - _metrics.height) / 2)) : 0

    font: Theme.font(style, numeric || multiLine ? { numeric: numeric, palt: !multiLine } : undefined)
    color: Theme.text
    lineHeightMode: styleLineHeight > 0 ? Text.FixedHeight : Text.ProportionalHeight
    lineHeight: styleLineHeight > 0 ? styleLineHeight : 1
    topPadding: leading
    bottomPadding: -leading

    property FontMetrics _metrics: FontMetrics { font: root.font }

    property bool _severalLines: false
    function _checkLines() {
        const several = multiLine && I18n.isCjk && (implicitWidth > width + 0.5 || /\n|<br/i.test(text))
        if (several !== _severalLines)
            _severalLines = several
    }
    // The check runs after the event that scheduled it. A page swap can destroy this item first, and
    // the closure then finds a bare wrapper without _checkLines ("is not a function (delayed function
    // evaluation)"): only an item that is still alive answers `_alive === true`.
    property bool _alive: false
    function _scheduleCheck() {
        if (multiLine && (I18n.isCjk || _severalLines))
            Qt.callLater(() => { if (root._alive === true) root._checkLines() })
    }
    onImplicitWidthChanged: _scheduleCheck()
    onWidthChanged: _scheduleCheck()
    onTextChanged: _scheduleCheck()
    onMultiLineChanged: _scheduleCheck()
    readonly property bool _cjk: I18n.isCjk
    on_CjkChanged: _scheduleCheck()
    Component.onCompleted: {
        _alive = true
        _scheduleCheck()
    }
    Component.onDestruction: _alive = false
}
