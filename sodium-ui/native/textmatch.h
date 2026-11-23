// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// TextMatch: the shared search matcher (docs/decisions/i18n.md, decision 26). One implementation for every search
// that filters names: the launcher, Settings (its search and the time zone list), the Store, Files.
//
// A name becomes a small list of search keys, computed once and cached by the caller; a query is scored against
// the keys. What the keys hold:
//   - the text folded: case (full case folding), accents and width (NFKD, marks stripped, Latin-ASCII), soft
//     hyphens and phrase marks removed (TextBreaks::stripBreakMarks), katakana turned into hiragana (so
//     ふぁいる finds ファイル; dakuten stay), Hangul in syllables;
//   - for Han text: full pinyin without tones, with spaces ("wen jian": "jian" starts a word) and without
//     ("wenjian"), and the initials ("wj"); other words of the text keep their own letters in all three;
//   - for Hangul: the initial consonants (as conjoining choseong, which is what a query of "ㅍㅇ" folds to) and
//     the jamo decomposition (so a half-typed syllable still matches: "파이" is a prefix of 파일's jamo).
// A query is folded the same way and tried as syllables and as jamo, so a Han/kana/Hangul query is also a
// substring match over the original text. Preedit text (what an input method hasn't committed: pinyin
// letters, a lone jamo) is a query like any other (decision 26).
//
//     const keys = Match.keys("文件")                      // QML; keep them next to the item
//     Match.score("wj", keys)                              // > 0: a match, higher is better
//     Match.matches("files", "Files")                      // keys or plain text
//     TextMatch::instance()->score(query, keys)            // C++
#pragma once

#include <QObject>
#include <QStringList>
#include <QVariant>

class TextMatch : public QObject
{
    Q_OBJECT

public:
    static TextMatch *instance();

    /// Scores: exact 1000, prefix 900, word start 800, contains 600 (minus a little for position and length, and
    /// 10 for a key that isn't the folded text itself); 0 is no match.
    enum Score { Exact = 1000, Prefix = 900, WordStart = 800, Contains = 600 };

    /// The text folded alone (the first of keys()).
    Q_INVOKABLE QString fold(const QString &text) const;
    /// The search keys of a name; the first is always the folded text. Empty for empty text.
    Q_INVOKABLE QStringList keys(const QString &text) const;
    /// How well the query matches the keys (a list from keys(), or plain text, whose keys are computed here);
    /// 0 for none. A query of several words must match as one string; callers that want words split first and
    /// add the scores.
    Q_INVOKABLE int score(const QString &query, const QVariant &keysOrText) const;
    Q_INVOKABLE bool matches(const QString &query, const QVariant &keysOrText) const { return score(query, keysOrText) > 0; }

private:
    explicit TextMatch(QObject *parent = nullptr) : QObject(parent) {}
};
