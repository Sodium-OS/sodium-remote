// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "textbreaks.h"

#include <QTextBoundaryFinder>
#include <QTextDocument>
#include <QTextLayout>

namespace TextBreaks {

namespace {

struct Pass
{
    int lines = 0;
    qreal widest = 0;
    bool overflow = false;   // a line wider than the width: a word that doesn't fit
};

Pass layOut(QTextLayout &layout, qreal width)
{
    Pass pass;
    layout.beginLayout();
    for (;;) {
        QTextLine line = layout.createLine();
        if (!line.isValid())
            break;
        line.setLineWidth(width);
        ++pass.lines;
        const qreal w = line.naturalTextWidth();
        pass.widest = qMax(pass.widest, w);
        if (w > width + 0.01)
            pass.overflow = true;
    }
    layout.endLayout();
    return pass;
}

QString plain(const QString &text)
{
    QString t = text;
    t.replace(QLatin1Char('\n'), QChar::LineSeparator);   // as QQuickText does
    return t;
}

void setUp(QTextLayout &layout, int wrapMode, bool designMetrics)
{
    QTextOption option;
    option.setWrapMode(QTextOption::WrapMode(wrapMode));
    option.setUseDesignMetrics(designMetrics);
    layout.setTextOption(option);
    layout.setCacheEnabled(true);   // shaped once, laid out many times
}

} // namespace

qreal balancedWidth(const QString &text, const QFont &font, qreal width)
{
    if (width <= 0 || text.isEmpty() || Qt::mightBeRichText(text))
        return width;
    QTextLayout layout(plain(text), font);
    // WordWrap: a narrower width that would need a break inside a word shows as an overflow and is refused.
    setUp(layout, QTextOption::WordWrap, true);
    const Pass full = layOut(layout, width);
    if (full.lines <= 1 || full.overflow)
        return width;
    qreal lo = 0, hi = full.widest;   // at the widest line's width the breaks are the full width's
    while (hi - lo > 0.25) {
        const qreal mid = (lo + hi) / 2;
        const Pass p = layOut(layout, mid);
        if (p.lines == full.lines && !p.overflow)
            hi = p.widest;
        else
            lo = mid;
    }
    return qMin(width, hi + 0.5);
}

QString joinPhrases(const QString &text)
{
    if (!text.contains(QChar(kPhraseMark)))
        return text;
    const auto open = [](QChar c) { return c != QChar(kPhraseMark) && c != QChar(kWordJoiner) && !c.isSpace(); };
    QString out;
    out.reserve(text.size() * 2);
    QTextBoundaryFinder graphemes(QTextBoundaryFinder::Grapheme, text);
    qsizetype from = 0;
    for (qsizetype to = graphemes.toNextBoundary(); to > 0; to = graphemes.toNextBoundary()) {
        const QStringView cluster = QStringView(text).mid(from, to - from);
        if (!out.isEmpty() && open(out.back()) && open(cluster.front()))
            out += QChar(kWordJoiner);
        out += cluster;
        from = to;
    }
    return out;
}

QString stripBreakMarks(const QString &text)
{
    if (!text.contains(QChar(kSoftHyphen)) && !text.contains(QChar(kPhraseMark)) && !text.contains(QChar(kWordJoiner)))
        return text;
    QString out = text;
    out.remove(QChar(kSoftHyphen));
    out.remove(QChar(kPhraseMark));
    out.remove(QChar(kWordJoiner));
    return out;
}

QList<Line> lines(const QString &text, const QFont &font, qreal width, int wrapMode, bool designMetrics)
{
    QList<Line> out;
    if (text.isEmpty() || width <= 0)
        return out;
    QTextLayout layout(plain(text), font);
    setUp(layout, wrapMode, designMetrics);
    layout.beginLayout();
    for (;;) {
        QTextLine line = layout.createLine();
        if (!line.isValid())
            break;
        line.setLineWidth(width);
        out.append({line.textStart(), line.textLength()});
    }
    layout.endLayout();
    return out;
}

} // namespace TextBreaks
