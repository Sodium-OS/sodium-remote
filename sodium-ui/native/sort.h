// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// Sort: comparing text for display the way the app's UI language sorts it (docs/decisions/i18n.md,
// decision 25): QCollator in the UI language, numeric ("File 2" before "File 10"), case-insensitive.
// zh_CN sorts by pinyin, zh_TW by stroke, ja in gojūon order, ko by Hangul (CLDR's defaults).
// Follows a live language switch: `revision` goes up, so bindings that sort re-run. Soft hyphens and phrase marks
// (decisions 89, 92) don't count; C++ that uses collator() directly strips them first (TextBreaks::stripBreakMarks).
//
//     names.sort((a, b) => Sort.compare(a, b))         // QML
//     Sort.sorted(["File 10", "file 2", "Äpfel"])
//     Sort::instance()->collator().compare(a, b)       // C++ (models)
#pragma once

#include <QCollator>
#include <QObject>
#include <QStringList>

class Sort : public QObject
{
    Q_OBJECT
    /// The language the collator follows (I18n.language; the pseudo-locales sort as en_US).
    Q_PROPERTY(QString language READ language NOTIFY changed)
    Q_PROPERTY(int revision READ revision NOTIFY changed)

public:
    /// The process-wide instance. Call it in the GUI thread.
    static Sort *instance();

    QString language() const { return m_language; }
    int revision() const { return m_revision; }
    const QCollator &collator() const { return m_collator; }

    /// < 0, 0 or > 0, as a sort comparator wants.
    Q_INVOKABLE int compare(const QString &a, const QString &b) const;
    /// A sorted copy.
    Q_INVOKABLE QStringList sorted(const QStringList &list) const;

    /// From I18n on every switch.
    void setLanguage(const QString &language);

signals:
    void changed();

private:
    explicit Sort(QObject *parent = nullptr);

    QString m_language;
    QCollator m_collator;
    int m_revision = 0;
};
