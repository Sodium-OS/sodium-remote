// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "calendarinfo.h"

#include "i18n.h"
#include "localeformats.h"

#include <QCoreApplication>
#include <QLocale>
#include <QTimeZone>

#include <cmath>
#include <functional>
#include <memory>
#include <unicode/calendar.h>
#include <unicode/locid.h>
#include <unicode/timezone.h>

namespace {

// A calendar of `kind` ("chinese", "islamic-umalqura") on UTC, so a date's fields are its civil
// day: the dates go in as UTC noon.
std::unique_ptr<icu::Calendar> makeCalendar(const char *kind)
{
    UErrorCode st = U_ZERO_ERROR;
    const icu::Locale loc((QByteArray("zh_CN@calendar=") + kind).constData());
    std::unique_ptr<icu::Calendar> cal(icu::Calendar::createInstance(icu::TimeZone::createTimeZone("UTC"), loc, st));
    return U_SUCCESS(st) ? std::move(cal) : nullptr;
}

UDate noonUtc(const QDate &d)
{
    return UDate(QDateTime(d, QTime(12, 0), QTimeZone::UTC).toMSecsSinceEpoch());
}

QDate dateOf(icu::Calendar *cal)
{
    UErrorCode st = U_ZERO_ERROR;
    const UDate t = cal->getTime(st);
    return U_SUCCESS(st) ? QDateTime::fromMSecsSinceEpoch(qint64(t), QTimeZone::UTC).date() : QDate();
}

const char *const kMonths[] = {"正月", "二月", "三月", "四月", "五月", "六月", "七月", "八月", "九月", "十月", "冬月", "腊月"};
const char *const kDigits[] = {"", "一", "二", "三", "四", "五", "六", "七", "八", "九", "十"};

QString dayName(int d)
{
    if (d <= 10)
        return QStringLiteral("初") + QString::fromUtf8(kDigits[d]);
    if (d < 20)
        return QStringLiteral("十") + QString::fromUtf8(kDigits[d - 10]);
    if (d == 20)
        return QStringLiteral("二十");
    if (d < 30)
        return QStringLiteral("廿") + QString::fromUtf8(kDigits[d - 20]);
    return QStringLiteral("三十");
}

// The nth weekday (1 = Monday … 7 = Sunday) of a month; n = -1: the last one.
QDate nthWeekday(int year, int month, int dow, int n)
{
    if (n > 0) {
        QDate d(year, month, 1);
        d = d.addDays((dow - d.dayOfWeek() + 7) % 7);
        return d.addDays(7 * (n - 1));
    }
    QDate d(year, month, QDate(year, month, 1).daysInMonth());
    return d.addDays(-((d.dayOfWeek() - dow + 7) % 7));
}

// The equinoxes as Japan's Cabinet Office works them out ahead (1980–2099).
int springEquinox(int y) { return int(std::floor(20.8431 + 0.242194 * (y - 1980) - std::floor((y - 1980) / 4.0))); }
int autumnEquinox(int y) { return int(std::floor(23.2488 + 0.242194 * (y - 1980) - std::floor((y - 1980) / 4.0))); }

// Qingming (清明), the solar term at 15°: the usual 21st-century formula, [Y·0.2422 + 4.81] − [Y/4].
QDate qingming(int y)
{
    const int yy = y % 100;
    return QDate(y, 4, int(std::floor(yy * 0.2422 + 4.81)) - yy / 4);
}

// A holiday's name: its English source text and its id, which is also the message's disambiguation
// (context "Holidays"), so "National Day" in China and in Taiwan are two messages (decision 29).
struct Name {
    const char *source = nullptr;
    const char *id = nullptr;
};

// How a day carries a holiday's name: as it is, "%1 (observed)" (moved off a weekend) or
// "%1 (substitute day)" (England and Wales).
enum class Shown { Plain, Observed, Substitute };

struct Holiday {
    Name name;
    bool approx = false;
    bool observed = false;
    Shown shown = Shown::Plain;
};

QString translated(const Holiday &h)
{
    const QString name = QCoreApplication::translate("Holidays", h.name.source, h.name.id);
    switch (h.shown) {
    case Shown::Observed:
        //: A public holiday moved off a weekend; %1 is the holiday's name ("Christmas Day (observed)")
        return QCoreApplication::translate("Holidays", "%1 (observed)").arg(name);
    case Shown::Substitute:
        //: A bank holiday's substitute day in England and Wales; %1 is the holiday ("Boxing Day (substitute day)")
        return QCoreApplication::translate("Holidays", "%1 (substitute day)").arg(name);
    default:
        return name;
    }
}

} // namespace

CalendarInfo *CalendarInfo::instance()
{
    static CalendarInfo *self = new CalendarInfo;
    return self;
}

CalendarInfo::CalendarInfo(QObject *parent)
    : QObject(parent)
{
    // Names are translated when asked for: on a language switch callers only ask again (revision).
    connect(I18n::instance(), &I18n::languageChanged, this, [this] {
        ++m_revision;
        emit revisionChanged();
    });
}

QVariantList CalendarInfo::regions() const
{
    static const Name names[] = {
        QT_TRANSLATE_NOOP3("Holidays", "Singapore", "a region whose public holidays Calendar shows"),
        QT_TRANSLATE_NOOP3("Holidays", "China", "a region whose public holidays Calendar shows"),
        QT_TRANSLATE_NOOP3("Holidays", "Hong Kong", "a region whose public holidays Calendar shows"),
        QT_TRANSLATE_NOOP3("Holidays", "Taiwan", "a region whose public holidays Calendar shows"),
        QT_TRANSLATE_NOOP3("Holidays", "Japan", "a region whose public holidays Calendar shows"),
        QT_TRANSLATE_NOOP3("Holidays", "United States", "a region whose public holidays Calendar shows"),
        QT_TRANSLATE_NOOP3("Holidays", "United Kingdom", "a region whose public holidays Calendar shows"),
    };
    static const char *const codes[] = {"SG", "CN", "HK", "TW", "JP", "US", "GB"};
    QVariantList out;
    for (int i = 0; i < 7; ++i)
        out << QVariantMap{{QStringLiteral("value"), QString::fromLatin1(codes[i])},
                           {QStringLiteral("label"), QCoreApplication::translate("Holidays", names[i].source, names[i].id)}};
    return out;
}

QString CalendarInfo::resolveRegion(const QString &region) const
{
    static const QStringList known = {QStringLiteral("SG"), QStringLiteral("CN"), QStringLiteral("HK"), QStringLiteral("TW"),
                                      QStringLiteral("JP"), QStringLiteral("US"), QStringLiteral("GB")};
    const QString r = region == QLatin1String("auto") ? LocaleFormats::instance()->region() : region.toUpper();
    return known.contains(r) ? r : QString();
}

QVariantMap CalendarInfo::lunar(const QDateTime &when) const
{
    auto cal = makeCalendar("chinese");
    if (!cal || !when.isValid())
        return {};
    UErrorCode st = U_ZERO_ERROR;
    cal->setTime(noonUtc(when.toLocalTime().date()), st);
    const int month = cal->get(UCAL_MONTH, st) + 1;
    const int day = cal->get(UCAL_DATE, st);
    const bool leap = cal->get(UCAL_IS_LEAP_MONTH, st) != 0;
    const int year = cal->get(UCAL_EXTENDED_YEAR, st);
    if (U_FAILURE(st) || month < 1 || month > 12)
        return {};
    return {{QStringLiteral("year"), year}, {QStringLiteral("month"), month}, {QStringLiteral("day"), day}, {QStringLiteral("leap"), leap},
            {QStringLiteral("monthName"), (leap ? QStringLiteral("闰") : QString()) + QString::fromUtf8(kMonths[month - 1])},
            {QStringLiteral("dayName"), dayName(day)}};
}

QString CalendarInfo::lunarDay(const QDateTime &when) const
{
    const QVariantMap l = lunar(when);
    if (l.isEmpty())
        return {};
    return l.value(QStringLiteral("day")).toInt() == 1 ? l.value(QStringLiteral("monthName")).toString() : l.value(QStringLiteral("dayName")).toString();
}

QDate CalendarInfo::lunarToGregorian(int gregorianYear, int lunarMonth, int lunarDayOfMonth, bool leap) const
{
    auto cal = makeCalendar("chinese");
    if (!cal)
        return {};
    // The lunar year that begins in this Gregorian year: the one 1 July is in. (Its number is
    // asked, not assumed: ICU's extended Chinese year changed meaning between versions.)
    UErrorCode st = U_ZERO_ERROR;
    cal->setTime(noonUtc(QDate(gregorianYear, 7, 1)), st);
    const int year = cal->get(UCAL_EXTENDED_YEAR, st);
    if (U_FAILURE(st))
        return {};
    cal->clear();
    cal->set(UCAL_EXTENDED_YEAR, year);
    cal->set(UCAL_MONTH, lunarMonth - 1);
    cal->set(UCAL_IS_LEAP_MONTH, leap ? 1 : 0);
    cal->set(UCAL_DATE, lunarDayOfMonth);
    return dateOf(cal.get());
}

QDate CalendarInfo::easter(int y)
{
    // The anonymous Gregorian algorithm (Meeus/Jones/Butcher).
    const int a = y % 19, b = y / 100, c = y % 100, d = b / 4, e = b % 4, f = (b + 8) / 25, g = (b - f + 1) / 3;
    const int h = (19 * a + b - d - g + 15) % 30, i = c / 4, k = c % 4, l = (32 + 2 * e + 2 * i - h - k) % 7;
    const int m = (a + 11 * h + 22 * l) / 451;
    return QDate(y, (h + l - 7 * m + 114) / 31, (h + l - 7 * m + 114) % 31 + 1);
}

// Islamic dates (month 1–12) that fall in a Gregorian year: there can be two, or one.
QList<QDate> CalendarInfo::islamic(int gregorianYear, int month, int day) const
{
    QList<QDate> out;
    auto cal = makeCalendar("islamic-umalqura");
    if (!cal)
        return out;
    const int guess = int((gregorianYear - 622) * 33.0 / 32.0);
    for (int hy = guess - 1; hy <= guess + 2; ++hy) {
        cal->clear();
        cal->set(UCAL_EXTENDED_YEAR, hy);
        cal->set(UCAL_MONTH, month - 1);
        cal->set(UCAL_DATE, day);
        const QDate d = dateOf(cal.get());
        if (d.isValid() && d.year() == gregorianYear && !out.contains(d))
            out << d;
    }
    return out;
}

QMap<QDate, CalendarInfo::Day> CalendarInfo::build(const QString &r, int y) const
{
    QMap<QDate, Holiday> days;
    auto add = [&](const QDate &d, const Name &name, bool approx = false) {
        if (d.isValid() && d.year() == y && !days.contains(d))
            days.insert(d, Holiday{name, approx, false, Shown::Plain});
    };
    auto observed = [&](const QDate &d, const Holiday &h) {
        if (d.isValid() && d.year() == y && !days.contains(d))
            days.insert(d, Holiday{h.name, h.approx, true, Shown::Observed});
    };
    // A holiday on a Sunday moves to the next day that isn't one (SG, HK, JP).
    auto sundaysToNextFreeDay = [&] {
        const auto copy = days;
        for (auto it = copy.cbegin(); it != copy.cend(); ++it) {
            if (it.key().dayOfWeek() != 7 || it.value().observed)
                continue;
            QDate d = it.key().addDays(1);
            while (days.contains(d))
                d = d.addDays(1);
            observed(d, it.value());
        }
    };
    const QDate lny = lunarToGregorian(y, 1, 1);
    const QDate lnyNext = lunarToGregorian(y + 1, 1, 1);   // its eve can fall in this year

    // Every name is a message in the context "Holidays" with the holiday's id as its disambiguation
    // (decision 29): the official name in the holiday's own language is checked in native review.
    if (r == QLatin1String("US")) {
        // Federal holidays; Saturday ones are observed on Friday, Sunday ones on Monday.
        const Name newYear = QT_TRANSLATE_NOOP3("Holidays", "New Year’s Day", "us.new_years_day");
        const QList<QPair<QDate, Name>> fixed = {{QDate(y, 1, 1), newYear},
                                                 {QDate(y, 6, 19), QT_TRANSLATE_NOOP3("Holidays", "Juneteenth", "us.juneteenth")},
                                                 {QDate(y, 7, 4), QT_TRANSLATE_NOOP3("Holidays", "Independence Day", "us.independence_day")},
                                                 {QDate(y, 11, 11), QT_TRANSLATE_NOOP3("Holidays", "Veterans Day", "us.veterans_day")},
                                                 {QDate(y, 12, 25), QT_TRANSLATE_NOOP3("Holidays", "Christmas Day", "us.christmas_day")}};
        add(nthWeekday(y, 1, 1, 3), QT_TRANSLATE_NOOP3("Holidays", "Martin Luther King Jr. Day", "us.mlk_day"));
        add(nthWeekday(y, 2, 1, 3), QT_TRANSLATE_NOOP3("Holidays", "Washington’s Birthday", "us.washingtons_birthday"));
        add(nthWeekday(y, 5, 1, -1), QT_TRANSLATE_NOOP3("Holidays", "Memorial Day", "us.memorial_day"));
        add(nthWeekday(y, 9, 1, 1), QT_TRANSLATE_NOOP3("Holidays", "Labor Day", "us.labor_day"));
        add(nthWeekday(y, 10, 1, 2), QT_TRANSLATE_NOOP3("Holidays", "Columbus Day", "us.columbus_day"));
        add(nthWeekday(y, 11, 4, 4), QT_TRANSLATE_NOOP3("Holidays", "Thanksgiving Day", "us.thanksgiving_day"));
        for (const auto &[d, name] : fixed) {
            add(d, name);
            if (d.dayOfWeek() == 6)
                observed(d.addDays(-1), Holiday{name});
            else if (d.dayOfWeek() == 7)
                observed(d.addDays(1), Holiday{name});
        }
        // Next year's New Year's Day on a Saturday is observed on this year's 31 December.
        if (QDate(y + 1, 1, 1).dayOfWeek() == 6)
            observed(QDate(y, 12, 31), Holiday{newYear});
    } else if (r == QLatin1String("GB")) {
        // England and Wales; weekend ones get the next weekday that isn't a bank holiday.
        const QDate e = easter(y);
        const Name newYear = QT_TRANSLATE_NOOP3("Holidays", "New Year’s Day", "gb.new_years_day");
        const Name christmas = QT_TRANSLATE_NOOP3("Holidays", "Christmas Day", "gb.christmas_day");
        const Name boxing = QT_TRANSLATE_NOOP3("Holidays", "Boxing Day", "gb.boxing_day");
        add(e.addDays(-2), QT_TRANSLATE_NOOP3("Holidays", "Good Friday", "gb.good_friday"));
        add(e.addDays(1), QT_TRANSLATE_NOOP3("Holidays", "Easter Monday", "gb.easter_monday"));
        add(nthWeekday(y, 5, 1, 1), QT_TRANSLATE_NOOP3("Holidays", "Early May bank holiday", "gb.early_may"));
        add(nthWeekday(y, 5, 1, -1), QT_TRANSLATE_NOOP3("Holidays", "Spring bank holiday", "gb.spring"));
        add(nthWeekday(y, 8, 1, -1), QT_TRANSLATE_NOOP3("Holidays", "Summer bank holiday", "gb.summer"));
        add(QDate(y, 1, 1), newYear);
        add(QDate(y, 12, 25), christmas);
        add(QDate(y, 12, 26), boxing);
        auto substitute = [&](const QDate &d, const Name &name) {
            days.insert(d, Holiday{name, false, true, Shown::Substitute});
        };
        const int nyd = QDate(y, 1, 1).dayOfWeek();
        if (nyd >= 6)
            substitute(QDate(y, 1, nyd == 6 ? 3 : 2), newYear);
        switch (QDate(y, 12, 25).dayOfWeek()) {
        case 5: substitute(QDate(y, 12, 28), boxing); break;
        case 6: substitute(QDate(y, 12, 27), christmas); substitute(QDate(y, 12, 28), boxing); break;
        case 7: substitute(QDate(y, 12, 27), christmas); break;
        default: break;
        }
    } else if (r == QLatin1String("SG")) {
        const QDate e = easter(y);
        const Name cny = QT_TRANSLATE_NOOP3("Holidays", "Chinese New Year", "sg.chinese_new_year");
        add(QDate(y, 1, 1), QT_TRANSLATE_NOOP3("Holidays", "New Year’s Day", "sg.new_years_day"));
        add(lny, cny);
        add(lny.addDays(1), cny);
        for (const QDate &d : islamic(y, 10, 1))
            add(d, QT_TRANSLATE_NOOP3("Holidays", "Hari Raya Puasa", "sg.hari_raya_puasa"), true);
        add(e.addDays(-2), QT_TRANSLATE_NOOP3("Holidays", "Good Friday", "sg.good_friday"));
        add(QDate(y, 5, 1), QT_TRANSLATE_NOOP3("Holidays", "Labor Day", "sg.labor_day"));
        add(lunarToGregorian(y, 4, 15), QT_TRANSLATE_NOOP3("Holidays", "Vesak Day", "sg.vesak_day"), true);
        for (const QDate &d : islamic(y, 12, 10))
            add(d, QT_TRANSLATE_NOOP3("Holidays", "Hari Raya Haji", "sg.hari_raya_haji"), true);
        add(QDate(y, 8, 9), QT_TRANSLATE_NOOP3("Holidays", "National Day", "sg.national_day"));
        add(QDate(y, 12, 25), QT_TRANSLATE_NOOP3("Holidays", "Christmas Day", "sg.christmas_day"));
        sundaysToNextFreeDay();
    } else if (r == QLatin1String("CN")) {
        // The statutory days only: the State Council announces the spans and the adjusted
        // working days every year, and those aren't computed here.
        const Name eve = QT_TRANSLATE_NOOP3("Holidays", "Spring Festival Eve", "cn.spring_festival_eve");
        const Name spring = QT_TRANSLATE_NOOP3("Holidays", "Spring Festival", "cn.spring_festival");
        const Name labor = QT_TRANSLATE_NOOP3("Holidays", "Labor Day", "cn.labor_day");
        const Name national = QT_TRANSLATE_NOOP3("Holidays", "National Day", "cn.national_day");
        add(QDate(y, 1, 1), QT_TRANSLATE_NOOP3("Holidays", "New Year’s Day", "cn.new_years_day"));
        if (y >= 2025) {
            add(lny.addDays(-1), eve);
            if (lnyNext.addDays(-1).year() == y)
                add(lnyNext.addDays(-1), eve);
        }
        for (int i = 0; i < 3; ++i)
            add(lny.addDays(i), spring);
        add(qingming(y), QT_TRANSLATE_NOOP3("Holidays", "Qingming Festival", "cn.qingming"));
        add(QDate(y, 5, 1), labor);
        if (y >= 2025)
            add(QDate(y, 5, 2), labor);
        add(lunarToGregorian(y, 5, 5), QT_TRANSLATE_NOOP3("Holidays", "Dragon Boat Festival", "cn.dragon_boat"));
        add(lunarToGregorian(y, 8, 15), QT_TRANSLATE_NOOP3("Holidays", "Mid-Autumn Festival", "cn.mid_autumn"));
        for (int i = 1; i <= 3; ++i)
            add(QDate(y, 10, i), national);
    } else if (r == QLatin1String("HK")) {
        // General holidays; one on a Sunday, or on another holiday, moves to the next free day.
        auto addHk = [&](QDate d, const Name &name) {
            if (!d.isValid())
                return;
            while (days.contains(d))
                d = d.addDays(1);
            add(d, name);
        };
        const QDate e = easter(y);
        addHk(QDate(y, 1, 1), QT_TRANSLATE_NOOP3("Holidays", "New Year’s Day", "hk.new_years_day"));
        addHk(lny, QT_TRANSLATE_NOOP3("Holidays", "Lunar New Year’s Day", "hk.lunar_new_year_1"));
        addHk(lny.addDays(1), QT_TRANSLATE_NOOP3("Holidays", "Lunar New Year (day 2)", "hk.lunar_new_year_2"));
        addHk(lny.addDays(2), QT_TRANSLATE_NOOP3("Holidays", "Lunar New Year (day 3)", "hk.lunar_new_year_3"));
        addHk(e.addDays(-2), QT_TRANSLATE_NOOP3("Holidays", "Good Friday", "hk.good_friday"));
        addHk(e.addDays(-1), QT_TRANSLATE_NOOP3("Holidays", "The day after Good Friday", "hk.day_after_good_friday"));
        addHk(e.addDays(1), QT_TRANSLATE_NOOP3("Holidays", "Easter Monday", "hk.easter_monday"));
        addHk(qingming(y), QT_TRANSLATE_NOOP3("Holidays", "Ching Ming Festival", "hk.ching_ming"));
        addHk(QDate(y, 5, 1), QT_TRANSLATE_NOOP3("Holidays", "Labor Day", "hk.labor_day"));
        addHk(lunarToGregorian(y, 4, 8), QT_TRANSLATE_NOOP3("Holidays", "The Birthday of the Buddha", "hk.buddhas_birthday"));
        addHk(lunarToGregorian(y, 5, 5), QT_TRANSLATE_NOOP3("Holidays", "Tuen Ng Festival", "hk.tuen_ng"));
        addHk(QDate(y, 7, 1), QT_TRANSLATE_NOOP3("Holidays", "HKSAR Establishment Day", "hk.establishment_day"));
        addHk(lunarToGregorian(y, 8, 16), QT_TRANSLATE_NOOP3("Holidays", "The day after Mid-Autumn Festival", "hk.day_after_mid_autumn"));
        addHk(QDate(y, 10, 1), QT_TRANSLATE_NOOP3("Holidays", "National Day", "hk.national_day"));
        addHk(lunarToGregorian(y, 9, 9), QT_TRANSLATE_NOOP3("Holidays", "Chung Yeung Festival", "hk.chung_yeung"));
        addHk(QDate(y, 12, 25), QT_TRANSLATE_NOOP3("Holidays", "Christmas Day", "hk.christmas_day"));
        QDate after = QDate(y, 12, 26);
        while (after.dayOfWeek() == 7 || days.contains(after))
            after = after.addDays(1);
        add(after, QT_TRANSLATE_NOOP3("Holidays", "The first weekday after Christmas Day", "hk.after_christmas"));
        sundaysToNextFreeDay();
    } else if (r == QLatin1String("TW")) {
        // National holidays; a Saturday one is observed the Friday before and a Sunday one the
        // Monday after, except the Lunar New Year days, which add the weekdays after them.
        // Teachers' Day, Retrocession Day and Constitution Day are holidays again from 2025.
        add(QDate(y, 1, 1), QT_TRANSLATE_NOOP3("Holidays", "Founding Day", "tw.founding_day"));
        const Name lnyEve = QT_TRANSLATE_NOOP3("Holidays", "Lunar New Year’s Eve", "tw.lunar_new_years_eve");
        const Name lnyName = QT_TRANSLATE_NOOP3("Holidays", "Lunar New Year", "tw.lunar_new_year");
        const QDate eve = lny.addDays(-1);
        QList<QDate> lnyDays = {eve, lny, lny.addDays(1), lny.addDays(2)};
        if (y >= 2025)
            lnyDays.prepend(eve.addDays(-1));
        for (const QDate &d : std::as_const(lnyDays))
            add(d, d < lny ? lnyEve : lnyName);
        int weekendDays = 0;
        for (const QDate &d : std::as_const(lnyDays))
            weekendDays += d.dayOfWeek() >= 6;
        QDate extra = lnyDays.last().addDays(1);
        while (weekendDays > 0) {
            if (extra.dayOfWeek() < 6) {
                observed(extra, Holiday{lnyName});
                --weekendDays;
            }
            extra = extra.addDays(1);
        }
        QList<QPair<QDate, Holiday>> rest = {
            {QDate(y, 2, 28), {QT_TRANSLATE_NOOP3("Holidays", "Peace Memorial Day", "tw.peace_memorial_day")}},
            {qingming(y), {QT_TRANSLATE_NOOP3("Holidays", "Tomb Sweeping Day", "tw.tomb_sweeping_day")}},
            // Children's Day moves to the day before when it meets Tomb Sweeping Day.
            {qingming(y) == QDate(y, 4, 4) ? QDate(y, 4, 3) : QDate(y, 4, 4), {QT_TRANSLATE_NOOP3("Holidays", "Children’s Day", "tw.childrens_day")}},
            {QDate(y, 5, 1), {QT_TRANSLATE_NOOP3("Holidays", "Labor Day", "tw.labor_day")}},
            {lunarToGregorian(y, 5, 5), {QT_TRANSLATE_NOOP3("Holidays", "Dragon Boat Festival", "tw.dragon_boat")}},
            {lunarToGregorian(y, 8, 15), {QT_TRANSLATE_NOOP3("Holidays", "Mid-Autumn Festival", "tw.mid_autumn")}},
            {QDate(y, 10, 10), {QT_TRANSLATE_NOOP3("Holidays", "National Day", "tw.national_day")}}};
        if (y >= 2025) {
            rest << QPair<QDate, Holiday>{QDate(y, 9, 28), {QT_TRANSLATE_NOOP3("Holidays", "Teachers’ Day", "tw.teachers_day")}}
                 << QPair<QDate, Holiday>{QDate(y, 10, 25), {QT_TRANSLATE_NOOP3("Holidays", "Retrocession Day", "tw.retrocession_day")}}
                 << QPair<QDate, Holiday>{QDate(y, 12, 25), {QT_TRANSLATE_NOOP3("Holidays", "Constitution Day", "tw.constitution_day")}};
        }
        for (const auto &[d, h] : std::as_const(rest))
            add(d, h.name);
        for (const auto &[d, h] : std::as_const(rest)) {
            if (d.dayOfWeek() == 6)
                observed(d.addDays(-1), h);
            else if (d.dayOfWeek() == 7)
                observed(d.addDays(1), h);
        }
    } else if (r == QLatin1String("JP")) {
        add(QDate(y, 1, 1), QT_TRANSLATE_NOOP3("Holidays", "New Year’s Day", "jp.new_years_day"));
        add(nthWeekday(y, 1, 1, 2), QT_TRANSLATE_NOOP3("Holidays", "Coming of Age Day", "jp.coming_of_age_day"));
        add(QDate(y, 2, 11), QT_TRANSLATE_NOOP3("Holidays", "National Foundation Day", "jp.national_foundation_day"));
        add(QDate(y, 2, 23), QT_TRANSLATE_NOOP3("Holidays", "The Emperor’s Birthday", "jp.emperors_birthday"));
        add(QDate(y, 3, springEquinox(y)), QT_TRANSLATE_NOOP3("Holidays", "Vernal Equinox Day", "jp.vernal_equinox_day"), true);
        add(QDate(y, 4, 29), QT_TRANSLATE_NOOP3("Holidays", "Shōwa Day", "jp.showa_day"));
        add(QDate(y, 5, 3), QT_TRANSLATE_NOOP3("Holidays", "Constitution Memorial Day", "jp.constitution_memorial_day"));
        add(QDate(y, 5, 4), QT_TRANSLATE_NOOP3("Holidays", "Greenery Day", "jp.greenery_day"));
        add(QDate(y, 5, 5), QT_TRANSLATE_NOOP3("Holidays", "Children’s Day", "jp.childrens_day"));
        add(nthWeekday(y, 7, 1, 3), QT_TRANSLATE_NOOP3("Holidays", "Marine Day", "jp.marine_day"));
        add(QDate(y, 8, 11), QT_TRANSLATE_NOOP3("Holidays", "Mountain Day", "jp.mountain_day"));
        add(nthWeekday(y, 9, 1, 3), QT_TRANSLATE_NOOP3("Holidays", "Respect for the Aged Day", "jp.respect_for_the_aged_day"));
        add(QDate(y, 9, autumnEquinox(y)), QT_TRANSLATE_NOOP3("Holidays", "Autumnal Equinox Day", "jp.autumnal_equinox_day"), true);
        add(nthWeekday(y, 10, 1, 2), QT_TRANSLATE_NOOP3("Holidays", "Sports Day", "jp.sports_day"));
        add(QDate(y, 11, 3), QT_TRANSLATE_NOOP3("Holidays", "Culture Day", "jp.culture_day"));
        add(QDate(y, 11, 23), QT_TRANSLATE_NOOP3("Holidays", "Labor Thanksgiving Day", "jp.labor_thanksgiving_day"));
        // A weekday between two holidays is a citizens' holiday (国民の休日).
        const auto copy = days;
        for (auto it = copy.cbegin(); it != copy.cend(); ++it) {
            const QDate mid = it.key().addDays(1);
            if (copy.contains(mid.addDays(1)) && !copy.contains(mid) && mid.dayOfWeek() != 7)
                add(mid, QT_TRANSLATE_NOOP3("Holidays", "Citizens’ Holiday", "jp.citizens_holiday"));
        }
        // 振替休日: a Sunday holiday gives the next day that isn't a holiday.
        const auto withCitizens = days;
        for (auto it = withCitizens.cbegin(); it != withCitizens.cend(); ++it) {
            if (it.key().dayOfWeek() != 7)
                continue;
            QDate d = it.key().addDays(1);
            while (days.contains(d))
                d = d.addDays(1);
            if (d.year() == y)
                days.insert(d, Holiday{QT_TRANSLATE_NOOP3("Holidays", "Substitute Holiday", "jp.substitute_holiday"), false, true, Shown::Plain});
        }
    }

    QMap<QDate, Day> out;
    for (auto it = days.cbegin(); it != days.cend(); ++it) {
        const Holiday &h = it.value();
        out.insert(it.key(), Day{h.name.source, h.name.id, h.approx, h.observed, int(h.shown)});
    }
    return out;
}

QVariantMap CalendarInfo::holidays(const QString &region, int year) const
{
    const QString r = resolveRegion(region);
    if (r.isEmpty() || year < 1900 || year > 2099)
        return {};
    const QString key = r + QLatin1Char('|') + QString::number(year);
    auto hit = m_cache.constFind(key);
    if (hit == m_cache.cend())
        hit = m_cache.insert(key, build(r, year));
    // Names in the UI language, now: the cache holds ids, so a language switch needs no rebuild.
    QVariantMap out;
    for (auto it = hit->cbegin(); it != hit->cend(); ++it) {
        const Day &d = it.value();
        out.insert(it.key().toString(Qt::ISODate),
                   QVariantMap{{QStringLiteral("id"), QString::fromLatin1(d.id)},
                               {QStringLiteral("name"), translated(Holiday{Name{d.source, d.id}, d.approx, d.observed, Shown(d.shown)})},
                               {QStringLiteral("approx"), d.approx}, {QStringLiteral("observed"), d.observed}});
    }
    return out;
}

QVariantMap CalendarInfo::holiday(const QString &region, const QDateTime &when) const
{
    if (!when.isValid())
        return {};
    const QDate d = when.toLocalTime().date();
    return holidays(region, d.year()).value(d.toString(Qt::ISODate)).toMap();
}

QVariantMap CalendarInfo::zoneTime(const QString &zone, const QDateTime &now) const
{
    const QTimeZone tz(zone.toUtf8());
    if (!tz.isValid() || !now.isValid())
        return {{QStringLiteral("valid"), false}};
    const QDateTime there = now.toTimeZone(tz);
    const QDateTime here = now.toLocalTime();
    const int offset = (there.offsetFromUtc() - here.offsetFromUtc()) / 60;
    // The exemplar city in the UI language, the same as Format.timeZoneCity (decision 90).
    const QString city = LocaleFormats::instance()->timeZoneCity(zone);
    return {{QStringLiteral("valid"), true},
            {QStringLiteral("wall"), QDateTime(there.date(), there.time())},   // local, same wall clock
            {QStringLiteral("city"), city},
            {QStringLiteral("offsetMinutes"), offset},
            {QStringLiteral("dayDelta"), int(here.date().daysTo(there.date()))}};
}
