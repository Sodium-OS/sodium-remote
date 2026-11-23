// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// CalendarInfo: lunar days (农历) and public holidays, for the bar's clock popover and Calendar.
//
// Everything is worked out on this computer, offline: the Chinese and Islamic calendars come from
// ICU (the one Qt is built on), the rest are the holidays' own rules (fixed dates, "the third
// Monday", Easter, the equinoxes, Sunday substitutes). Settings › Date & Time › Holidays picks the
// region (`holidays`: "auto" follows Language & Region's region, "none" shows none).
//
//     CalendarInfo.lunarDay(date)              // "初五"; on the first day the month: "十月", "闰六月"
//     CalendarInfo.holiday("SG", date)         // { id, name, approx, observed } or {} (no holiday)
//     CalendarInfo.holidays("auto", 2026)      // { "2026-01-01": { id, name, approx, observed }, ... }
//
// **Names** are catalog messages in the context "Holidays", one per holiday id (the disambiguation,
// "sg.chinese_new_year"), shown in the UI language like every other string (decision 29); a day moved
// off a weekend is "%1 (observed)" (England and Wales: "%1 (substitute day)"). The Chinese and Japanese
// names this file used to carry are the catalogs' seeds (i18n/seed/<lang>-holidays.tsv).
//
// **Approximations** (`approx: true`), dates that are announced rather than computed:
//  - the Islamic holidays (Hari Raya Puasa, Hari Raya Haji): ICU's Umm al-Qura tables; the
//    announced day can be one later;
//  - Vesak Day: the 15th of the 4th lunar month, which it has matched in recent years;
//  - Japan's equinox days: the usual formula (the government announces them each February).
// **Left out** (they can't be computed honestly): Deepavali (Hindu calendar, announced), China's
// adjusted working days and its announced holiday spans beyond the statutory days, one-off
// holidays (coronations, jubilees, elections), and Taiwan's bridge days.
#pragma once

#include <QDate>
#include <QDateTime>
#include <QHash>
#include <QObject>
#include <QVariantMap>

class CalendarInfo : public QObject
{
    Q_OBJECT
    /// The regions with holiday rules: [{ value: "SG", label: "Singapore" }, ...] (labels in the UI language).
    Q_PROPERTY(QVariantList regions READ regions NOTIFY revisionChanged)
    /// Goes up on a language switch: holiday names are translated when asked for, so a binding that
    /// shows them reads it (`void CalendarInfo.revision`) to ask again.
    Q_PROPERTY(int revision READ revision NOTIFY revisionChanged)

public:
    static CalendarInfo *instance();

    QVariantList regions() const;

    /// The lunar day's label under a date: 初一…三十, with the month's name (十月, 闰六月) on 初一.
    Q_INVOKABLE QString lunarDay(const QDateTime &when) const;
    /// { year, month (1–12), day, leap, monthName, dayName } of the Chinese calendar.
    Q_INVOKABLE QVariantMap lunar(const QDateTime &when) const;
    /// A region's holidays in a year by "yyyy-MM-dd": { id, name, approx, observed }. region "auto"
    /// is Language & Region's region (LocaleFormats.region), "none" or an unknown one: {}. `name` is
    /// in the UI language (catalog context "Holidays", the id as disambiguation; decision 29);
    /// `id` ("cn.national_day") stays the same in every language: compare and store that.
    Q_INVOKABLE QVariantMap holidays(const QString &region, int year) const;
    /// The holiday on that date, or {} (see holidays()).
    Q_INVOKABLE QVariantMap holiday(const QString &region, const QDateTime &when) const;
    /// Another time zone now, for extra clocks: { wall (a Date carrying that zone's wall time, to
    /// format as a local one), city ("New York"), offsetMinutes (from here), dayDelta (-1, 0, 1),
    /// valid }.
    Q_INVOKABLE QVariantMap zoneTime(const QString &zone, const QDateTime &now) const;
    /// "auto" → the region's code if it has rules, else ""; a known code stays; anything else "".
    Q_INVOKABLE QString resolveRegion(const QString &region) const;

    // For tests and C++ callers.
    QDate lunarToGregorian(int gregorianYear, int lunarMonth, int lunarDay, bool leap = false) const;
    static QDate easter(int year);

    int revision() const { return m_revision; }

    /// One holiday as built: its message (source text, id), and how the day carries it.
    struct Day {
        const char *source = nullptr;
        const char *id = nullptr;
        bool approx = false;
        bool observed = false;
        int shown = 0;   // 0 the name, 1 "%1 (observed)", 2 "%1 (substitute day)"
    };

signals:
    void revisionChanged();

private:
    explicit CalendarInfo(QObject *parent = nullptr);
    QMap<QDate, Day> build(const QString &region, int year) const;
    QList<QDate> islamic(int gregorianYear, int month, int day) const;

    mutable QHash<QString, QMap<QDate, Day>> m_cache;   // "<region>|<year>": ids, translated when asked
    int m_revision = 0;
};
