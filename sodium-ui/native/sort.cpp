// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "sort.h"

#include "textbreaks.h"

#include <QLocale>

#include <algorithm>

Sort *Sort::instance()
{
    static Sort *self = new Sort;
    return self;
}

Sort::Sort(QObject *parent)
    : QObject(parent)
{
    setLanguage(QStringLiteral("en_US"));
}

void Sort::setLanguage(const QString &language)
{
    if (language == m_language)
        return;
    m_language = language;
    m_collator = QCollator(QLocale(language.startsWith(QLatin1String("qps-")) ? QStringLiteral("en_US") : language));
    m_collator.setNumericMode(true);
    m_collator.setCaseSensitivity(Qt::CaseInsensitive);
    ++m_revision;
    emit changed();
}

// A translator's soft hyphens and phrase marks are never part of the sort key (decision 92).
int Sort::compare(const QString &a, const QString &b) const
{
    return m_collator.compare(TextBreaks::stripBreakMarks(a), TextBreaks::stripBreakMarks(b));
}

QStringList Sort::sorted(const QStringList &list) const
{
    QStringList out = list;
    std::sort(out.begin(), out.end(), [this](const QString &a, const QString &b) { return compare(a, b) < 0; });
    return out;
}
