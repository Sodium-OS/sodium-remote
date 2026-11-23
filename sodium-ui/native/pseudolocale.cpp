// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "pseudolocale.h"

#include <QHash>
#include <QRegularExpression>

namespace {

// The design's accent table (design/i18n/kitl.py, the LangComponents board): "Save" → "Šåvé".
QChar accent(QChar c)
{
    static const QHash<QChar, QChar> table = [] {
        const QString from = QStringLiteral("aeioucnsyAEIOUCNSZz");
        const QString to = QStringLiteral("åéîöüçñšýÅÉÎÖÜÇÑŠŽž");
        QHash<QChar, QChar> t;
        for (int i = 0; i < from.size(); ++i)
            t.insert(from.at(i), to.at(i));
        return t;
    }();
    return table.value(c, c);
}

// How many characters at s[i] are kept as they are: %1 %99 %n %L1 %Ln %%, an HTML tag, an entity
// (&amp; &#38; &#x26;), "&&", or a mnemonic ("&S": the letter is a shortcut and never localised).
int keptLength(const QString &s, int i)
{
    const int n = int(s.size());
    const QChar c = s.at(i);
    if (c == QLatin1Char('%') && i + 1 < n) {
        int j = i + 1;
        if (s.at(j) == QLatin1Char('%'))
            return 2;
        if (s.at(j) == QLatin1Char('L') && j + 1 < n && (s.at(j + 1).isDigit() || s.at(j + 1) == QLatin1Char('n')))
            ++j;
        if (s.at(j) == QLatin1Char('n'))
            return j + 1 - i;
        if (s.at(j).isDigit()) {
            while (j < n && s.at(j).isDigit())
                ++j;
            return j - i;
        }
        return 0;
    }
    if (c == QLatin1Char('<') && i + 1 < n) {
        const QChar next = s.at(i + 1);
        const int end = int(s.indexOf(QLatin1Char('>'), i));
        if (end > i && (next.isLetter() || next == QLatin1Char('/') || next == QLatin1Char('!')))
            return end + 1 - i;
        return 0;
    }
    if (c == QLatin1Char('&') && i + 1 < n) {
        static const QRegularExpression entity(QStringLiteral("&(#[0-9]+|#[xX][0-9a-fA-F]+|[A-Za-z][A-Za-z0-9]*);"));
        const QRegularExpressionMatch m = entity.match(s, i, QRegularExpression::NormalMatch,
                                                       QRegularExpression::AnchorAtOffsetMatchOption);
        if (m.hasMatch())
            return int(m.capturedLength());
        if (!s.at(i + 1).isSpace())
            return 2;   // "&&" or a mnemonic
    }
    return 0;
}

// Characters a reader sees: tags don't count; an entity, "&&" and a mnemonic ("&S") count as one.
int visibleLength(const QString &s)
{
    int len = 0;
    for (int i = 0; i < s.size();) {
        const int k = keptLength(s, i);
        if (k > 0 && s.at(i) == QLatin1Char('<')) {
            i += k;
            continue;
        }
        if (k > 0 && s.at(i) == QLatin1Char('&')) {
            ++len;
            i += k;
            continue;
        }
        ++len;
        ++i;
    }
    return len;
}

// A stable hash (FNV-1a) so a word always gets the same filler, in every process and Qt build.
quint32 fnv(const QString &s)
{
    quint32 h = 2166136261u;
    for (const QChar c : s) {
        h ^= c.unicode();
        h *= 16777619u;
    }
    return h;
}

QString filler(const QString &word)
{
    // Han with regional glyph forms (直 骨 写 説 画 …) and katakana: height, line height and
    // spaceless line breaking get exercised in the font the UI language picks.
    static const QString han = QStringLiteral("保存檔案設定語言檢視編輯列印複製貼上刪除搜尋顯示隱藏開啟關閉直骨写説画");
    static const QString kana = QStringLiteral("アイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモラリルレロワン");
    const int count = qMax(1, int((word.size() * 7 + 5) / 10));
    const quint32 h = fnv(word);
    QString out;
    out.reserve(count);
    for (int k = 0; k < count; ++k) {
        const quint32 v = h / (k + 1) + quint32(k) * 2654435761u;
        out += (v % 3 == 2) ? kana.at(int(v % quint32(kana.size()))) : han.at(int(v % quint32(han.size())));
    }
    return out;
}

QChar fullWidth(QChar c)
{
    if (c == QLatin1Char('.'))
        return QChar(0x3002);   // 。
    if (c == QLatin1Char(','))
        return QChar(0x3001);   // 、
    if (c.unicode() >= 0x21 && c.unicode() <= 0x7e)
        return QChar(c.unicode() + 0xfee0);
    return c;
}

} // namespace

namespace pseudo {

int padFor(int length)
{
    if (length <= 10)
        return qMax(4, length);
    if (length <= 20)
        return (length * 8 + 5) / 10;
    if (length <= 30)
        return (length * 6 + 5) / 10;
    if (length <= 70)
        return (length * 4 + 5) / 10;
    return (length * 3 + 5) / 10;
}

QString resolvePlural(const QString &source, int n)
{
    if (n < 0 || !source.contains(QLatin1Char('(')))
        return source;
    QString out = source;
    out.replace(QLatin1String("(es)"), n == 1 ? QString() : QStringLiteral("es"));
    out.replace(QLatin1String("(s)"), n == 1 ? QString() : QStringLiteral("s"));
    return out;
}

QString accented(const QString &source)
{
    if (source.isEmpty())
        return source;
    QString body;
    body.reserve(source.size() * 2 + 4);
    for (int i = 0; i < source.size();) {
        const int k = keptLength(source, i);
        if (k > 0) {
            body += QStringView(source).mid(i, k);
            i += k;
        } else {
            body += accent(source.at(i));
            ++i;
        }
    }
    return QLatin1Char('[') + body + QString(padFor(visibleLength(source)), QChar(0x00b7)) + QLatin1Char(']');
}

QString cjk(const QString &source)
{
    if (source.isEmpty())
        return source;
    QString body;
    for (int i = 0; i < source.size();) {
        const int k = keptLength(source, i);
        if (k > 0) {
            body += QStringView(source).mid(i, k);
            i += k;
            continue;
        }
        const QChar c = source.at(i);
        if (c.isLetter()) {
            int j = i;
            while (j < source.size() && (source.at(j).isLetter() || source.at(j) == QLatin1Char('\'')
                                         || source.at(j) == QChar(0x2019)))
                ++j;
            body += filler(source.mid(i, j - i));
            i = j;
        } else if (c.isSpace() && c != QLatin1Char('\n')) {
            ++i;   // no spaces: the line may break anywhere
        } else {
            body += c.isDigit() || c == QLatin1Char('\n') ? c : fullWidth(c);
            ++i;
        }
    }
    return QChar(0xff3b) + body + QStringLiteral("・・") + QChar(0xff3d);   // ［…・・］
}

} // namespace pseudo
