// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "localeformats.h"

#include "i18n.h"
#include "sort.h"

#include <QCoreApplication>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QLocale>
#include <QMimeDatabase>
#include <QRegularExpression>
#include <QSet>
#include <QStandardPaths>
#include <QTimeZone>
#include <QXmlStreamReader>

#include <algorithm>
#include <cmath>
#include <memory>
#include <unicode/dcfmtsym.h>
#include <unicode/dtfmtsym.h>
#include <unicode/dtptngen.h>
#include <unicode/locdspnm.h>
#include <unicode/listformatter.h>
#include <unicode/locid.h>
#include <unicode/numberformatter.h>
#include <unicode/smpdtfmt.h>
#include <unicode/timezone.h>
#include <unicode/tznames.h>
#include <unicode/ucal.h>
#include <unicode/uloc.h>
#include <unicode/uvernum.h>

namespace {

QString userConf()
{
    return QStandardPaths::writableLocation(QStandardPaths::GenericConfigLocation) + QStringLiteral("/locale.conf");
}

QString readText(const QString &path)
{
    QFile f(path);
    return f.open(QIODevice::ReadOnly) ? QString::fromUtf8(f.readAll()) : QString();
}

icu::UnicodeString toIcu(const QString &s)
{
    return icu::UnicodeString(reinterpret_cast<const UChar *>(s.utf16()), int32_t(s.size()));
}

QString fromIcu(const icu::UnicodeString &s)
{
    return QString(reinterpret_cast<const QChar *>(s.getBuffer()), s.length());
}

// Western digits always (decision 34): ٣٨ → 38.
QString latinDigits(QString s)
{
    for (QChar &c : s)
        if (c.isDigit() && c.unicode() > 0x7f && c.digitValue() >= 0)
            c = QChar(u'0' + c.digitValue());
    return s;
}

bool isPosix(const QString &name)
{
    return name.isEmpty() || name == QLatin1String("C") || name == QLatin1String("POSIX");
}

icu::Locale icuLocale(const QString &name)
{
    return icu::Locale(isPosix(name) ? "en_US_POSIX" : name.toLatin1().constData());
}

// A glibc locale for QLocale: C and POSIX read as en_US, as they always have here.
QString qtLocaleName(const QString &name)
{
    return isPosix(name) ? QStringLiteral("en_US") : name;
}

// The locales ICU has formatting data for ("en_SG", "zh_Hans_SG", "es_US"; not "de_US" or "zh_Hans_US").
bool icuHasLocale(const QString &name)
{
    static const QSet<QString> available = [] {
        QSet<QString> out;
        const int32_t n = uloc_countAvailable();
        for (int32_t i = 0; i < n; ++i)
            out.insert(QString::fromLatin1(uloc_getAvailable(i)));
        return out;
    }();
    return available.contains(name);
}

// "en_SG" → "SG", "es_419" → "419"; "" when the locale names no territory.
QString territoryOf(const QString &locale)
{
    const QStringList parts = LocaleFormats::base(locale).split(QLatin1Char('_'), Qt::SkipEmptyParts);
    for (int i = 1; i < parts.size(); ++i) {
        const QString &p = parts.at(i);
        if ((p.size() == 2 && p.at(0).isLetter() && p.at(1).isLetter()) || (p.size() == 3 && p.at(0).isDigit()))
            return p.toUpper();
    }
    return {};
}

// shared-mime-info's xml:lang and AppStream tags compare case-insensitively, "_" as "-" (decision 94).
QString normalizedTag(const QString &tag)
{
    QString t = tag.toLower();
    t.replace(QLatin1Char('_'), QLatin1Char('-'));
    return t;
}

// ICU's canonical (CLDR) id for a zone id, "" for an unknown one: "Asia/Kolkata" → "Asia/Calcutta".
QString cldrZoneId(const QString &id)
{
    UErrorCode status = U_ZERO_ERROR;
    icu::UnicodeString canonical;
    UBool system = false;
    icu::TimeZone::getCanonicalID(toIcu(id.trimmed()), canonical, system, status);
    if (U_FAILURE(status) || canonical.isEmpty() || canonical == UNICODE_STRING_SIMPLE("Etc/Unknown"))
        return {};
    return fromIcu(canonical);
}

bool isUtc(const QString &cldr)
{
    return cldr == QLatin1String("Etc/UTC") || cldr == QLatin1String("Etc/GMT");
}

} // namespace

LocaleFormats *LocaleFormats::instance()
{
    static LocaleFormats *self = new LocaleFormats;
    return self;
}

LocaleFormats::LocaleFormats(QObject *parent)
    : QObject(parent)
{
    m_debounce.setSingleShot(true);
    m_debounce.setInterval(60);
    connect(&m_debounce, &QTimer::timeout, this, &LocaleFormats::reload);
    connect(&m_watcher, &QFileSystemWatcher::fileChanged, &m_debounce, qOverload<>(&QTimer::start));
    connect(&m_watcher, &QFileSystemWatcher::directoryChanged, &m_debounce, qOverload<>(&QTimer::start));
    m_system = parseConf(readText(QStringLiteral("/etc/locale.conf")));
    reload();
}

LocaleFormats::~LocaleFormats() = default;

QString LocaleFormats::base(const QString &locale)
{
    return locale.section(QLatin1Char('.'), 0, 0).section(QLatin1Char('@'), 0, 0).trimmed();
}

QMap<QString, QString> LocaleFormats::parseConf(const QString &text)
{
    QMap<QString, QString> out;
    for (const QString &raw : text.split(QLatin1Char('\n'))) {
        const QString l = raw.trimmed();
        if (l.isEmpty() || l.startsWith(QLatin1Char('#')) || !l.contains(QLatin1Char('=')))
            continue;
        QString v = l.section(QLatin1Char('='), 1).trimmed();
        if (v.size() >= 2 && (v.startsWith(QLatin1Char('"')) || v.startsWith(QLatin1Char('\''))))
            v = v.mid(1, v.size() - 2);
        out.insert(l.section(QLatin1Char('='), 0, 0).trimmed(), v);
    }
    return out;
}

// Decision 91: the UI language's pattern, names and AM/PM, with the Region's territory.
QString LocaleFormats::dateLocaleFor(const QString &uiLanguage, const QString &timeLocale)
{
    QString ui = I18n::match(uiLanguage);
    if (ui.isEmpty() || ui.startsWith(QLatin1String("qps-")))
        ui = QStringLiteral("en_US");   // the pseudo-locales write the English source's dates
    const QString lang = ui.section(QLatin1Char('_'), 0, 0);
    const QString uiTerritory = ui.section(QLatin1Char('_'), 1, 1);
    QString prefix = lang;
    if (lang == QLatin1String("zh"))
        prefix += uiTerritory == QLatin1String("TW") ? QStringLiteral("_Hant") : QStringLiteral("_Hans");
    const QString own = prefix + QLatin1Char('_') + uiTerritory;
    const QString time = base(timeLocale);
    const bool posix = isPosix(time);
    if (posix && lang == QLatin1String("en"))
        return QStringLiteral("en_US_POSIX");
    const QString territory = posix ? QStringLiteral("US") : territoryOf(time);
    if (territory.isEmpty())
        return own;
    const QString combined = prefix + QLatin1Char('_') + territory;
    if (combined == own || icuHasLocale(combined))
        return combined;
    // No CLDR data for the pair (de_US, zh_Hans_US): the UI language's own, the Region as ICU's region override.
    return own + QStringLiteral("@rg=") + territory.toLower() + QStringLiteral("zzzz");
}

// locale(7): LC_<x> beats LANG; the user's file beats the system's (as Settings' LanguageRegion
// reads them). With neither file saying anything, this process's own environment.
QString LocaleFormats::effective(const QString &key) const
{
    for (const auto *src : {&m_user, &m_system})
        if (!src->value(key).isEmpty())
            return base(src->value(key));
    QString lang = m_user.value(QStringLiteral("LANG"), m_system.value(QStringLiteral("LANG")));
    if (lang.isEmpty())
        lang = qEnvironmentVariable(key.toLatin1().constData(), qEnvironmentVariable("LANG"));
#ifdef Q_OS_WIN
    if (lang.isEmpty())
        lang = QLocale::system().name();   // Windows has no LANG: the user's regional format (de_DE)
#endif
    lang = base(lang);
    return lang.isEmpty() ? QStringLiteral("C") : lang;
}

void LocaleFormats::rewatch()
{
    const QString file = userConf();
    QStringList want;
    if (QFileInfo::exists(file))
        want << file;
    // The folder too: Settings replaces the file by a rename, and it may not exist yet.
    QDir d(QFileInfo(file).absolutePath());
    while (!d.exists() && d.cdUp()) {}
    want << d.absolutePath();
    const QStringList watched = m_watcher.files() + m_watcher.directories();
    for (const QString &p : want)
        if (!watched.contains(p))
            m_watcher.addPath(p);
}

void LocaleFormats::bump()
{
    m_dateLocale = dateLocaleFor(m_uiLanguage, m_time);
    ++m_revision;
    emit changed();
}

void LocaleFormats::reload()
{
    rewatch();
    const QMap<QString, QString> user = parseConf(readText(userConf()));
    // Any change counts (LANGUAGE too: I18n follows it from here).
    const bool confChanged = user != m_user;
    m_user = user;
    const QString t = effective(QStringLiteral("LC_TIME")), n = effective(QStringLiteral("LC_NUMERIC")),
                  mo = effective(QStringLiteral("LC_MONETARY")), me = effective(QStringLiteral("LC_MEASUREMENT"));
    if (t == m_time && n == m_numeric && mo == m_monetary && me == m_measurement && !confChanged && m_revision > 0)
        return;
    m_time = t;
    m_numeric = n;
    m_monetary = mo;
    m_measurement = me;
    bump();
}

QString LocaleFormats::region() const
{
    const QLocale l(m_monetary);
    return l.territory() == QLocale::AnyTerritory || isPosix(m_monetary) ? QString() : QLocale::territoryToCode(l.territory());
}

bool LocaleFormats::usMeasurement() const
{
    return QLocale(m_measurement).measurementSystem() == QLocale::ImperialUSSystem;
}

QString LocaleFormats::patternIn(const QString &locale, const QString &skeleton) const
{
    const QString key = locale + QLatin1Char('|') + skeleton;
    const auto hit = m_patterns.constFind(key);
    if (hit != m_patterns.cend())
        return hit.value();
    UErrorCode status = U_ZERO_ERROR;
    std::unique_ptr<icu::DateTimePatternGenerator> gen(icu::DateTimePatternGenerator::createInstance(icuLocale(locale), status));
    QString pattern;
    if (U_SUCCESS(status) && gen) {
        const icu::UnicodeString p = gen->getBestPattern(toIcu(skeleton), status);
        if (U_SUCCESS(status))
            pattern = fromIcu(p);
    }
    m_patterns.insert(key, pattern);
    return pattern;
}

QString LocaleFormats::datePattern(const QString &skeleton) const
{
    return patternIn(m_dateLocale, skeleton);
}

QString LocaleFormats::dateIn(const QString &locale, const QDateTime &when, const QString &skeleton) const
{
    if (!when.isValid())
        return {};
    const QString pattern = patternIn(locale, skeleton);
    if (pattern.isEmpty())
        return QLocale(qtLocaleName(m_time)).toString(when.date(), QLocale::ShortFormat);
    UErrorCode status = U_ZERO_ERROR;
    icu::SimpleDateFormat fmt(toIcu(pattern), icuLocale(locale), status);
    if (U_FAILURE(status))
        return QLocale(qtLocaleName(m_time)).toString(when.date(), QLocale::ShortFormat);
    // The day it is here: the system zone as Qt sees it now (it follows a time-zone change).
    const QByteArray zone = QTimeZone::systemTimeZoneId();
    if (!zone.isEmpty())
        fmt.adoptTimeZone(icu::TimeZone::createTimeZone(icu::UnicodeString::fromUTF8(zone.constData())));
    icu::UnicodeString out;
    fmt.format(UDate(when.toMSecsSinceEpoch()), out);
    return latinDigits(fromIcu(out));
}

QString LocaleFormats::formatDate(const QDateTime &when, const QString &skeleton) const
{
    return dateIn(m_dateLocale, when, skeleton);
}

QString LocaleFormats::numberIn(const QString &numeric, double value, int decimals)
{
    const QLocale l(qtLocaleName(base(numeric)));
    return latinDigits(decimals < 0 ? l.toString(value, 'g', QLocale::FloatingPointShortest) : l.toString(value, 'f', decimals));
}

QString LocaleFormats::currencyIn(const QString &monetary, double value)
{
    return latinDigits(QLocale(qtLocaleName(base(monetary))).toCurrencyString(value));
}

QString LocaleFormats::formatNumber(double value, int decimals) const
{
    return numberIn(m_numeric, value, decimals);
}

QString LocaleFormats::decimalSeparator() const
{
    return QLocale(qtLocaleName(m_numeric)).decimalPoint();
}

QString LocaleFormats::groupSeparator() const
{
    return QLocale(qtLocaleName(m_numeric)).groupSeparator();
}

// ---------------------------------------------------------------- Format (decisions 34, 91)

void LocaleFormats::setUiLanguage(const QString &language)
{
    if (language.isEmpty() || language == m_uiLanguage)
        return;
    m_uiLanguage = language;
    bump();
}

void LocaleFormats::setClock24h(std::optional<bool> on)
{
    if (on == m_clock24h)
        return;
    m_clock24h = on;
    bump();
}

// Names come in the UI language; the pseudo-locales use the English source's.
QString LocaleFormats::nameLocale() const
{
    return m_uiLanguage.startsWith(QLatin1String("qps-")) ? QStringLiteral("en_US") : m_uiLanguage;
}

// "fr", "de" ...: the language part of nameLocale().
QString LocaleFormats::unitLanguage() const
{
    return nameLocale().section(QLatin1Char('_'), 0, 0);
}

int LocaleFormats::firstDayOfWeek() const
{
    return int(QLocale(qtLocaleName(m_time)).firstDayOfWeek());
}

// The Region's own clock: LC_TIME's "j" (de_US is a 12-hour region; en_DE, en_GB 24-hour).
bool LocaleFormats::regionClock24h() const
{
    return regionClock24hIn(m_time);
}

bool LocaleFormats::regionClock24hIn(const QString &timeLocale) const
{
    const QString p = patternIn(timeLocale, QStringLiteral("j"));
    return p.contains(QLatin1Char('H')) || p.contains(QLatin1Char('k'));
}

QString LocaleFormats::date(const QDateTime &when, const QString &skeleton) const
{
    return formatDate(when, skeleton);
}

QString LocaleFormats::timeOfDayIn(const Spec &spec, const QDateTime &when, bool seconds) const
{
    // No explicit choice (decision 95): the clock of the spec's own Region.
    const bool h24 = spec.clock24h.value_or(regionClock24hIn(spec.time));
    QString skeleton = h24 ? QStringLiteral("Hm") : QStringLiteral("hm");
    if (seconds)
        skeleton += QLatin1Char('s');
    return dateIn(dateLocaleFor(spec.uiLanguage, spec.time), when, skeleton);
}

QString LocaleFormats::timeOfDay(const QDateTime &when, bool seconds) const
{
    return timeOfDayIn(current(), when, seconds);
}

QVariantMap LocaleFormats::timeParts(const QDateTime &when, bool seconds) const
{
    const Spec spec = current();
    const QString full = timeOfDayIn(spec, when, seconds);
    const QString mark = dateIn(dateLocaleFor(spec.uiLanguage, spec.time), when, QStringLiteral("a"));
    const bool h24 = spec.clock24h.value_or(regionClock24hIn(spec.time));
    QVariantMap out{{QStringLiteral("time"), full}, {QStringLiteral("mark"), QString()}, {QStringLiteral("markFirst"), false}};
    if (h24 || mark.isEmpty() || !full.contains(mark))
        return out;
    const bool first = full.startsWith(mark);
    // Take the mark out with the space (U+0020, U+00A0, U+202F) that separates it from the digits.
    QString rest = first ? full.mid(mark.size()) : full.left(full.size() - mark.size());
    if (!first && !full.endsWith(mark))
        return out;
    static const QString spaces = QStringLiteral(" \u00a0\u202f");
    while (!rest.isEmpty() && spaces.contains(first ? rest.front() : rest.back()))
        rest = first ? rest.mid(1) : rest.left(rest.size() - 1);
    out[QStringLiteral("time")] = rest;
    out[QStringLiteral("mark")] = mark;
    out[QStringLiteral("markFirst")] = first;
    return out;
}

QString LocaleFormats::number(double value, int decimals) const
{
    return formatNumber(value, decimals);
}

QString LocaleFormats::count(qint64 n) const
{
    return latinDigits(QLocale(qtLocaleName(m_numeric)).toString(n));
}

QString LocaleFormats::list(const QStringList &items) const
{
    if (items.size() <= 1)
        return items.value(0);
    UErrorCode status = U_ZERO_ERROR;
    const QByteArray ui = nameLocale().toLatin1();
    std::unique_ptr<icu::ListFormatter> f(icu::ListFormatter::createInstance(icu::Locale(ui.constData()), status));
    if (U_FAILURE(status) || !f)
        return items.join(QStringLiteral(", "));
    std::vector<icu::UnicodeString> parts;
    for (const QString &i : items)
        parts.emplace_back(reinterpret_cast<const char16_t *>(i.utf16()), int32_t(i.size()));
    icu::UnicodeString out;
    f->format(parts.data(), int32_t(parts.size()), out, status);
    if (U_FAILURE(status))
        return items.join(QStringLiteral(", "));
    return QString(reinterpret_cast<const QChar *>(out.getBuffer()), out.length());
}

QString LocaleFormats::compactNumber(double value) const
{
    // The UI language's compact patterns ("K", "万", "만", "Mio."), the Region's symbols, Latin digits.
    UErrorCode status = U_ZERO_ERROR;
    const QByteArray numeric = (qtLocaleName(m_numeric) + QStringLiteral("@numbers=latn")).toLatin1();
    const icu::DecimalFormatSymbols symbols{icu::Locale(numeric.constData()), status};
    const QByteArray ui = (nameLocale() + QStringLiteral("@numbers=latn")).toLatin1();
    if (U_FAILURE(status))
        return count(qRound64(value));
    const icu::UnicodeString out = icu::number::NumberFormatter::withLocale(icu::Locale(ui.constData()))
                                       .notation(icu::number::Notation::compactShort())
                                       .symbols(symbols)
                                       .formatDouble(value, status)
                                       .toString(status);
    return U_SUCCESS(status) ? latinDigits(fromIcu(out)) : count(qRound64(value));
}

QString LocaleFormats::percent(double value, int decimals) const
{
    const QString num = formatNumber(value, qMax(0, decimals));
    // "38%" in English, Chinese, Japanese and Korean; "38 %" (U+202F) in French, German and Spanish.
    const QString lang = unitLanguage();
    if (lang == QLatin1String("fr") || lang == QLatin1String("de") || lang == QLatin1String("es"))
        return num + QChar(0x202f) + QLatin1Char('%');
    return num + QLatin1Char('%');
}

QString LocaleFormats::size(qint64 bytes) const
{
    if (bytes < 0)
        return QStringLiteral("—");
    if (bytes < 1000)
        return QCoreApplication::translate("Format", "%n byte(s)", "a file size under 1 KB", int(bytes));
    // As Files has always shown them: 1000s; KB whole, bigger units one decimal below 100.
    double v = bytes / 1000.0;
    int u = 0;
    while (v >= 999.5 && u < 4) {
        v /= 1000.0;
        ++u;
    }
    int decimals = (u == 0 || v >= 99.95) ? 0 : 1;
    if (decimals == 1 && std::fmod(std::round(v * 10), 10.0) == 0)
        decimals = 0;
    // Units stay untranslated (decision 34), except French.
    static const char *const units[] = {"KB", "MB", "GB", "TB", "PB"};
    static const char *const french[] = {"ko", "Mo", "Go", "To", "Po"};
    const char *unit = unitLanguage() == QLatin1String("fr") ? french[u] : units[u];
    return formatNumber(decimals ? std::round(v * 10) / 10 : std::round(v), decimals) + QChar(0x00a0) + QLatin1String(unit);
}

QString LocaleFormats::currency(double value) const
{
    return currencyIn(m_monetary, value);
}

QString LocaleFormats::duration(double seconds, const QString &style) const
{
    const qint64 total = qAbs(qRound64(seconds));
    const qint64 values[] = {total / 86400, total % 86400 / 3600, total % 3600 / 60, total % 60};
    const bool longStyle = style == QLatin1String("long");
    const auto unit = [longStyle](int which, qint64 v) -> QString {
        const int n = int(v);
        if (longStyle) {
            switch (which) {
            case 0: return QCoreApplication::translate("Format", "%n day(s)", "a duration, long", n);
            case 1: return QCoreApplication::translate("Format", "%n hour(s)", "a duration, long", n);
            case 2: return QCoreApplication::translate("Format", "%n minute(s)", "a duration, long", n);
            default: return QCoreApplication::translate("Format", "%n second(s)", "a duration, long", n);
            }
        }
        switch (which) {
        case 0: return QCoreApplication::translate("Format", "%n d", "a duration, short: days", n);
        case 1: return QCoreApplication::translate("Format", "%n h", "a duration, short: hours", n);
        case 2: return QCoreApplication::translate("Format", "%n min", "a duration, short: minutes", n);
        default: return QCoreApplication::translate("Format", "%n s", "a duration, short: seconds", n);
        }
    };
    int first = 0;
    while (first < 3 && values[first] == 0)
        ++first;
    const QString big = unit(first, values[first]);
    if (first == 3 || values[first + 1] == 0)
        return big;
    //: A duration's two units, largest first: "2 h 5 min", "2 hours 5 minutes". Japanese "%1%2".
    return QCoreApplication::translate("Format", "%1 %2", "a duration: two units").arg(big, unit(first + 1, values[first + 1]));
}

QString LocaleFormats::relativeTime(const QDateTime &when, const QDateTime &now) const
{
    if (!when.isValid())
        return {};
    const QDateTime ref = now.isValid() ? now : QDateTime::currentDateTime();
    const qint64 diff = when.secsTo(ref);   // > 0: in the past
    const bool past = diff >= 0;
    const qint64 a = qAbs(diff);
    if (a < 60)
        return past ? QCoreApplication::translate("Format", "Just now") : QCoreApplication::translate("Format", "In a moment");
    if (a < 3600) {
        const int m = int(a / 60);
        return past ? QCoreApplication::translate("Format", "%n minute(s) ago", nullptr, m)
                    : QCoreApplication::translate("Format", "In %n minute(s)", nullptr, m);
    }
    if (a < 86400) {
        const int h = int(a / 3600);
        return past ? QCoreApplication::translate("Format", "%n hour(s) ago", nullptr, h)
                    : QCoreApplication::translate("Format", "In %n hour(s)", nullptr, h);
    }
    const qint64 days = when.date().daysTo(ref.date());
    if (days == 1)
        return QCoreApplication::translate("Format", "Yesterday");
    if (days == -1)
        return QCoreApplication::translate("Format", "Tomorrow");
    if (qAbs(days) < 7)
        return past ? QCoreApplication::translate("Format", "%n day(s) ago", nullptr, int(days))
                    : QCoreApplication::translate("Format", "In %n day(s)", nullptr, int(-days));
    return formatDate(when, when.date().year() == ref.date().year() ? QStringLiteral("MMMd") : QStringLiteral("yMMMd"));
}

namespace {

icu::DateFormatSymbols::DtWidthType widthOf(const QString &width)
{
    if (width == QLatin1String("short"))
        return icu::DateFormatSymbols::ABBREVIATED;
    if (width == QLatin1String("narrow"))
        return icu::DateFormatSymbols::NARROW;
    return icu::DateFormatSymbols::WIDE;
}

} // namespace

QString LocaleFormats::monthName(int month, const QString &width, bool standalone) const
{
    if (month < 1 || month > 12)
        return {};
    UErrorCode status = U_ZERO_ERROR;
    icu::DateFormatSymbols symbols(icuLocale(m_dateLocale), status);
    if (U_FAILURE(status))
        return {};
    int32_t n = 0;
    const icu::UnicodeString *names = symbols.getMonths(
        n, standalone ? icu::DateFormatSymbols::STANDALONE : icu::DateFormatSymbols::FORMAT, widthOf(width));
    return names && month <= n ? fromIcu(names[month - 1]) : QString();
}

QString LocaleFormats::dayName(int day, const QString &width, bool standalone) const
{
    if (day < 1 || day > 7)
        return {};
    UErrorCode status = U_ZERO_ERROR;
    icu::DateFormatSymbols symbols(icuLocale(m_dateLocale), status);
    if (U_FAILURE(status))
        return {};
    int32_t n = 0;
    const icu::UnicodeString *names = symbols.getWeekdays(
        n, standalone ? icu::DateFormatSymbols::STANDALONE : icu::DateFormatSymbols::FORMAT, widthOf(width));
    // ICU: 1 = Sunday … 7 = Saturday; Qt: 1 = Monday … 7 = Sunday.
    const int icuDay = day == 7 ? 1 : day + 1;
    return names && icuDay < n ? fromIcu(names[icuDay]) : QString();
}

// ---------------------------------------------------------------- previews (decision 91)

LocaleFormats::Spec LocaleFormats::current() const
{
    return Spec{m_time, m_numeric, m_monetary, m_uiLanguage, m_clock24h};
}

LocaleFormats::Spec LocaleFormats::with(const QVariantMap &formats) const
{
    Spec spec = current();
    const auto locale = [&formats](const char *key, QString *into) {
        const QString v = base(formats.value(QLatin1String(key)).toString());
        if (!v.isEmpty())
            *into = v;
    };
    locale("time", &spec.time);
    locale("numeric", &spec.numeric);
    locale("monetary", &spec.monetary);
    const QString ui = I18n::match(formats.value(QStringLiteral("uiLanguage")).toString());
    if (!ui.isEmpty())
        spec.uiLanguage = ui;
    if (formats.contains(QStringLiteral("clock24h"))) {
        // A bool is a choice; null, undefined or anything else is Automatic (the candidate Region's clock).
        const QVariant v = formats.value(QStringLiteral("clock24h"));
        spec.clock24h = v.typeId() == QMetaType::Bool ? std::optional<bool>(v.toBool()) : std::nullopt;
    }
    return spec;
}

QString LocaleFormats::previewDate(const QDateTime &when, const QString &skeleton, const QVariantMap &formats) const
{
    const Spec spec = with(formats);
    return dateIn(dateLocaleFor(spec.uiLanguage, spec.time), when, skeleton);
}

QString LocaleFormats::previewTimeOfDay(const QDateTime &when, const QVariantMap &formats, bool seconds) const
{
    return timeOfDayIn(with(formats), when, seconds);
}

QString LocaleFormats::previewNumber(double value, int decimals, const QVariantMap &formats) const
{
    return numberIn(with(formats).numeric, value, decimals);
}

QString LocaleFormats::previewCurrency(double value, const QVariantMap &formats) const
{
    return currencyIn(with(formats).monetary, value);
}

QString LocaleFormats::previewDateLocale(const QVariantMap &formats) const
{
    const Spec spec = with(formats);
    return dateLocaleFor(spec.uiLanguage, spec.time);
}

// ---------------------------------------------------------------- time zones (decision 90)

icu::TimeZoneNames *LocaleFormats::zoneNames() const
{
    const QString language = nameLocale();
    if (!m_zoneNames || m_namesLanguage != language) {
        m_regionNames.reset();
        m_namesLanguage = language;
        UErrorCode status = U_ZERO_ERROR;
        m_zoneNames.reset(icu::TimeZoneNames::createInstance(icuLocale(language), status));
        if (U_FAILURE(status))
            m_zoneNames.reset();
    }
    return m_zoneNames.get();
}

icu::LocaleDisplayNames *LocaleFormats::regionNames() const
{
    zoneNames();   // both follow the UI language
    if (!m_regionNames) {
        // As Settings' Region list names them (apps/settings/src/languages.cpp).
        UDisplayContext context[] = {UDISPCTX_STANDARD_NAMES, UDISPCTX_CAPITALIZATION_FOR_UI_LIST_OR_MENU};
        m_regionNames.reset(icu::LocaleDisplayNames::createInstance(icuLocale(m_namesLanguage), context, 2));
    }
    return m_regionNames.get();
}

QString LocaleFormats::timeZoneCanonical(const QString &id) const
{
    const QString cldr = cldrZoneId(id);
    if (cldr.isEmpty())
        return {};
    if (isUtc(cldr))
        return QStringLiteral("UTC");
    if (cldr.startsWith(QLatin1String("Etc/")))
        return {};   // offset zones (Etc/GMT+3) aren't offered
#if U_ICU_VERSION_MAJOR_NUM >= 74
    UErrorCode status = U_ZERO_ERROR;
    icu::UnicodeString iana;
    icu::TimeZone::getIanaID(toIcu(cldr), iana, status);   // CLDR keeps old names: Asia/Calcutta is Asia/Kolkata
    if (U_SUCCESS(status) && !iana.isEmpty())
        return fromIcu(iana);
#endif
    return cldr;
}

QString LocaleFormats::timeZoneCity(const QString &id) const
{
    const QString cldr = cldrZoneId(id);
    QString name;
    if (icu::TimeZoneNames *names = cldr.isEmpty() ? nullptr : zoneNames()) {
        icu::UnicodeString out;
        if (isUtc(cldr))
            names->getDisplayName(toIcu(cldr), UTZNM_LONG_STANDARD, UDate(QDateTime::currentMSecsSinceEpoch()), out);
        else
            names->getExemplarLocationName(toIcu(cldr), out);
        name = fromIcu(out);
    }
    if (name.isEmpty() && (cldr.isEmpty() ? id : cldr).contains(QLatin1Char('/')))
        name = id.section(QLatin1Char('/'), -1).replace(QLatin1Char('_'), QLatin1Char(' '));
    if (name.isEmpty())
        name = id.trimmed();   // "UTC" and the like, as written
    return name;
}

QString LocaleFormats::timeZoneRegion(const QString &id) const
{
    UErrorCode status = U_ZERO_ERROR;
    char region[8] = {};
    icu::TimeZone::getRegion(toIcu(id.trimmed()), region, sizeof region, status);
    if (U_FAILURE(status) || !region[0] || qstrcmp(region, "001") == 0)
        return {};
    icu::LocaleDisplayNames *names = regionNames();
    if (!names)
        return QString::fromLatin1(region);
    icu::UnicodeString out;
    names->regionDisplayName(region, out);
    return fromIcu(out);
}

QString LocaleFormats::timeZoneOffset(const QString &id, const QDateTime &when) const
{
    const QString canonical = timeZoneCanonical(id);
    std::unique_ptr<icu::TimeZone> zone(icu::TimeZone::createTimeZone(toIcu(canonical.isEmpty() ? id.trimmed() : canonical)));
    if (!zone || *zone == icu::TimeZone::getUnknown())
        return {};
    UErrorCode status = U_ZERO_ERROR;
    icu::SimpleDateFormat fmt(UNICODE_STRING_SIMPLE("O"), icuLocale(nameLocale()), status);
    if (U_FAILURE(status))
        return {};
    fmt.adoptTimeZone(zone.release());
    icu::UnicodeString out;
    fmt.format(UDate((when.isValid() ? when : QDateTime::currentDateTime()).toMSecsSinceEpoch()), out);
    return latinDigits(fromIcu(out));
}

QStringList LocaleFormats::timeZoneIds() const
{
    const QString language = nameLocale();
    if (m_zoneIdsLanguage == language && !m_zoneIds.isEmpty())
        return m_zoneIds;
    UErrorCode status = U_ZERO_ERROR;
    std::unique_ptr<icu::StringEnumeration> zones(
        icu::TimeZone::createTimeZoneIDEnumeration(UCAL_ZONE_TYPE_CANONICAL_LOCATION, nullptr, nullptr, status));
    QStringList ids;
    QSet<QString> seen;
    while (zones && U_SUCCESS(status)) {
        const icu::UnicodeString *z = zones->snext(status);
        if (!z)
            break;
        const QString iana = timeZoneCanonical(fromIcu(*z));
        // Only what this system's tz database has: the id is what timedatectl is given.
        if (iana.isEmpty() || seen.contains(iana) || !QTimeZone::isTimeZoneIdAvailable(iana.toLatin1()))
            continue;
        seen.insert(iana);
        ids << iana;
    }
    ids << QStringLiteral("UTC");
    QList<QPair<QString, QString>> keyed;
    keyed.reserve(ids.size());
    for (const QString &id : std::as_const(ids))
        keyed.append({timeZoneCity(id), id});
    const QCollator &collator = Sort::instance()->collator();
    std::sort(keyed.begin(), keyed.end(), [&collator](const auto &a, const auto &b) {
        const int c = collator.compare(a.first, b.first);
        return c != 0 ? c < 0 : a.second < b.second;
    });
    m_zoneIds.clear();
    for (const auto &k : std::as_const(keyed))
        m_zoneIds << k.second;
    m_zoneIdsLanguage = language;
    return m_zoneIds;
}

QVariantMap LocaleFormats::timeZoneAliases() const
{
    if (!m_zoneAliases.isEmpty())
        return m_zoneAliases;
    UErrorCode status = U_ZERO_ERROR;
    std::unique_ptr<icu::StringEnumeration> zones(icu::TimeZone::createEnumeration(status));
    while (zones && U_SUCCESS(status)) {
        const icu::UnicodeString *z = zones->snext(status);
        if (!z)
            break;
        const QString id = fromIcu(*z);
        const QString canonical = timeZoneCanonical(id);
        if (!canonical.isEmpty() && canonical != id)
            m_zoneAliases.insert(id, canonical);
    }
    return m_zoneAliases;
}

// ---------------------------------------------------------------- file types (cleanup item 56)

QString LocaleFormats::typeName(const QString &mimeName) const
{
    const QMimeDatabase db;
    const QMimeType type = db.mimeTypeForName(mimeName.trimmed());
    if (!type.isValid())
        return mimeName;
    const QString name = type.name();   // an alias reads as its type
    auto comments = m_typeComments.find(name);
    if (comments == m_typeComments.end()) {
        // update-mime-database's per-type file holds every translation: <comment xml:lang="de">…</comment>.
        QHash<QString, QString> found;
        QFile f(QStandardPaths::locate(QStandardPaths::GenericDataLocation, QStringLiteral("mime/") + name + QStringLiteral(".xml")));
        if (!f.fileName().isEmpty() && f.open(QIODevice::ReadOnly)) {
            QXmlStreamReader xml(&f);
            while (!xml.atEnd()) {
                xml.readNext();
                if (!xml.isStartElement() || xml.name() != QLatin1String("comment"))
                    continue;
                const QString tag = normalizedTag(xml.attributes().value(QLatin1String("xml:lang")).toString());
                const QString text = xml.readElementText().trimmed();
                if (!text.isEmpty() && !found.contains(tag))
                    found.insert(tag, text);
            }
        }
        comments = m_typeComments.insert(name, found);
    }
    QString text;
    for (const QString &tag : I18n::contentChainFor(m_uiLanguage, region())) {
        text = comments->value(normalizedTag(tag));
        if (!text.isEmpty())
            break;
    }
    if (text.isEmpty())
        text = comments->value(QString());
    if (text.isEmpty())
        text = type.comment();
    // shared-mime-info writes some in lower case ("einfache Textdatei"); a label starts with a capital.
    if (!text.isEmpty() && text.at(0).isLower())
        text.replace(0, 1, text.at(0).toUpper());
    return text;
}
