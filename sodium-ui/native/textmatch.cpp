// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "textmatch.h"

#include "textbreaks.h"

#include <QHash>

#include <unicode/translit.h>
#include <unicode/uchar.h>
#include <unicode/unistr.h>
#include <unicode/uscript.h>

#include <memory>

namespace {

icu::UnicodeString toIcu(const QString &s)
{
    return icu::UnicodeString(reinterpret_cast<const char16_t *>(s.utf16()), int32_t(s.size()));
}

QString fromIcu(const icu::UnicodeString &s)
{
    return QString(reinterpret_cast<const QChar *>(s.getBuffer()), int(s.length()));
}

// Created once, in the GUI thread (a Transliterator isn't thread-safe).
icu::Transliterator *make(const char *id)
{
    UErrorCode err = U_ZERO_ERROR;
    icu::Transliterator *t = icu::Transliterator::createInstance(icu::UnicodeString::fromUTF8(id), UTRANS_FORWARD, err);
    return U_SUCCESS(err) ? t : nullptr;
}

// Accents and width off, katakana to hiragana; the voiced marks of kana (U+3099, U+309A) stay.
QString foldAll(const QString &text)
{
    static std::unique_ptr<icu::Transliterator> trans(
        make("NFKD; [[:Mn:]-[\\u3099\\u309A]] Remove; Latin-ASCII; NFKC; Katakana-Hiragana"));
    icu::UnicodeString u = toIcu(text);
    if (trans)
        trans->transliterate(u);
    u.foldCase();
    return fromIcu(u);
}

// One Han character's pinyin without tones; "" when ICU has none.
QString pinyinOf(char32_t c)
{
    static std::unique_ptr<icu::Transliterator> trans(make("Han-Latin; Latin-ASCII; Lower"));
    static QHash<char32_t, QString> cache;
    const auto it = cache.constFind(c);
    if (it != cache.constEnd())
        return it.value();
    QString out;
    if (trans) {
        icu::UnicodeString u;
        u.append(UChar32(c));
        trans->transliterate(u);
        out = fromIcu(u).trimmed();
        for (QChar ch : out)
            if (ch.unicode() > 0x7f || !ch.isLetter())
                out.clear(); // no reading: the character stays as it is
    }
    cache.insert(c, out);
    return out;
}

bool isHan(char32_t c)
{
    UErrorCode err = U_ZERO_ERROR;
    return uscript_getScript(UChar32(c), &err) == USCRIPT_HAN;
}

bool isSyllable(char16_t c) { return c >= 0xac00 && c <= 0xd7a3; }

void addUnique(QStringList &list, const QString &s)
{
    if (!s.isEmpty() && !list.contains(s))
        list.append(s);
}

// Best score of one folded query against one folded key; 0 for none.
int scoreOne(const QString &q, const QString &key)
{
    if (key == q)
        return TextMatch::Exact;
    const int lengthPenalty = int(qMin<qsizetype>(60, key.size() - q.size()));
    if (key.startsWith(q))
        return TextMatch::Prefix - lengthPenalty;
    const qsizetype first = key.indexOf(q);
    if (first < 0)
        return 0;
    for (qsizetype i = first; i >= 0; i = key.indexOf(q, i + 1))
        if (i > 0 && !key[i - 1].isLetterOrNumber())
            return TextMatch::WordStart - int(qMin<qsizetype>(60, i));
    return TextMatch::Contains - int(qMin<qsizetype>(100, first));
}

} // namespace

TextMatch *TextMatch::instance()
{
    static TextMatch *self = new TextMatch;
    return self;
}

QString TextMatch::fold(const QString &text) const
{
    return foldAll(TextBreaks::stripBreakMarks(text)).simplified();
}

QStringList TextMatch::keys(const QString &text) const
{
    QStringList out;
    const QString folded = fold(text);
    if (folded.isEmpty())
        return out;
    out << folded;

    // Han: pinyin, spaced, joined and as initials. Words around the characters keep their letters.
    bool hasHan = false, hasHangul = false;
    QStringList tokens;
    QString word;
    const auto flush = [&] {
        if (!word.isEmpty())
            tokens << word;
        word.clear();
    };
    for (qsizetype i = 0; i < folded.size(); ++i) {
        char32_t c = folded[i].unicode();
        if (QChar::isHighSurrogate(c) && i + 1 < folded.size()) {
            c = QChar::surrogateToUcs4(folded[i], folded[i + 1]);
            ++i;
        }
        if (isSyllable(char16_t(c)))
            hasHangul = true;
        if (isHan(c)) {
            const QString py = pinyinOf(c);
            if (!py.isEmpty()) {
                flush();
                tokens << py;
                hasHan = true;
                continue;
            }
        }
        if (c < 0x10000 && QChar(char16_t(c)).isSpace())
            flush();
        else
            word += QString::fromUcs4(&c, 1);
    }
    flush();
    if (hasHan) {
        QString initials;
        for (const QString &t : tokens)
            initials += t[0];
        addUnique(out, tokens.join(QLatin1Char(' ')));
        addUnique(out, tokens.join(QString()));
        addUnique(out, initials);
    }

    // Hangul: initial consonants (conjoining choseong, as a query's NFKD makes them) and the jamo.
    if (hasHangul) {
        QString initials;
        for (QChar ch : folded) {
            if (isSyllable(ch.unicode()))
                initials += QChar(char16_t(0x1100 + (ch.unicode() - 0xac00) / 588));
            else if (!ch.isSpace())
                initials += ch;
        }
        addUnique(out, initials);
        addUnique(out, folded.normalized(QString::NormalizationForm_KD));
    }
    return out;
}

int TextMatch::score(const QString &query, const QVariant &keysOrText) const
{
    const QString q = fold(query);
    if (q.isEmpty())
        return 0;
    // Syllables and jamo: "파이" is a prefix of the jamo of 파일; "ㅍㅇ" (choseong after folding) of its initials.
    QStringList forms{q};
    addUnique(forms, q.normalized(QString::NormalizationForm_KD));

    const QStringList keys = keysOrText.metaType().id() == QMetaType::QString ? this->keys(keysOrText.toString())
                                                                               : keysOrText.toStringList();
    int best = 0;
    for (int i = 0; i < keys.size(); ++i) {
        for (const QString &form : std::as_const(forms)) {
            const int s = scoreOne(form, keys[i]);
            if (s > 0)
                best = qMax(best, i == 0 ? s : s - 10);
        }
    }
    return best;
}
