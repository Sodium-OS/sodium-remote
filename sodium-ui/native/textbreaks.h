// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// Line breaks for short wrapped labels (docs/decisions/i18n.md, decisions 89 and 92): what WrappedLabel and the
// overflow audit share. QML reaches them through I18n (balancedWidth, joinPhrases, stripBreakMarks).
//
// They lay text out the way a QML Text does by default (QTextLayout, design metrics: Text.QtRendering), so the
// lines they measure are the lines the Text draws.
//
// What Qt 6.12 does on its own (measured in QTextLayout and QML Text, Inter, Nunito, Noto Sans CJK):
// - U+200B is a zero-width break opportunity; Chinese and Japanese still break between any two characters.
// - U+2060 (word joiner) is zero-width and forbids a break; Text.Wrap still breaks there when the joined run
//   is wider than the line (its "anywhere" fallback), Text.WordWrap lets it overflow.
// - Hangul: Qt keeps a Korean word together and breaks at spaces (QTextBoundaryFinder still lists every
//   syllable boundary; the layout doesn't use them); Text.Wrap breaks inside a word only when it is wider
//   than the line. Decision 89's Korean rule needs nothing more.
// - U+00AD: invisible and zero-width unbroken; a break opportunity; at a break the line ends with a drawn
//   hyphen whose width the line's naturalTextWidth includes (PlainText and StyledText, Inter and Nunito).
#pragma once

#include <QFont>
#include <QList>
#include <QString>

namespace TextBreaks {

/// The soft hyphen (decision 92), the phrase mark (decision 89), and the word joiner WrappedLabel puts inside phrases.
constexpr char16_t kSoftHyphen = 0x00ad;
constexpr char16_t kPhraseMark = 0x200b;
constexpr char16_t kWordJoiner = 0x2060;

/// The narrowest width (≤ `width`) that lays `text` out in as many lines as `width` does, without breaking a word
/// that the full width doesn't break; `width` itself when the text fits on one line, when a word is wider than
/// the line already, or for rich text. A little over the widest line, so the Text breaks where this measured.
qreal balancedWidth(const QString &text, const QFont &font, qreal width);

/// The text with word joiners between the characters of each phrase when it carries phrase marks (U+200B), so
/// it breaks only at the marks (and spaces); unchanged without marks.
QString joinPhrases(const QString &text);

/// Without soft hyphens, phrase marks and word joiners: for anything that isn't the wrapped label itself
/// (accessible names, tooltips, search, sorting, copying).
QString stripBreakMarks(const QString &text);

struct Line
{
    int start = 0;
    int length = 0;
};

/// The lines a Text with this font, width and wrap mode (QTextOption's values, as Text.WrapMode has them) draws
/// `text` in; designMetrics: Text.QtRendering (the default) and CurveRendering. Plain text only ("\n" breaks).
QList<Line> lines(const QString &text, const QFont &font, qreal width, int wrapMode, bool designMetrics = true);

} // namespace TextBreaks
