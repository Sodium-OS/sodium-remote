// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// LocaleFormats (QML: Format, and LocaleFormats for older callers): dates, times, numbers, sizes,
// durations, time zones and file types the way Settings › Language & Region says (locale.conf), in the
// app's UI language. Apps never call QLocale for display; they call this (docs/i18n.md § 4).
//
// Reads ${XDG_CONFIG_HOME:-~/.config}/locale.conf (what Settings writes), then /etc/locale.conf,
// then this process's environment, with locale(7)'s precedence: LC_<category> beats LANG, and the
// user's file beats the system's. The user's file is watched, so a change in Settings reaches
// every running shell module and app at once.
//
// Which locale gives what (docs/decisions/i18n.md, decisions 12, 34 and 91):
// - **Dates and times** follow the UI language, with the Region's territory: one ICU locale,
//   `dateLocale`, made of the UI language (I18n.language) and LC_TIME's territory:
//   `<lang>[_<Script>]_<TERRITORY>` ("zh_Hans_US", "en_SG", "en_DE", "es_US") when ICU has data for it,
//   else the UI language's own locale with the Region as `@rg=<territory>zzzz` ("de_DE@rg=uszzzz",
//   "ja_JP@rg=sgzzzz"). Skeleton patterns, month and day names and AM/PM all come from it, so Chinese
//   under Region US reads "11月14日星期六", never the Region's pattern with translated names
//   ("星期六, 十一月 14"). C/POSIX counts as Region US: en_US_POSIX in English.
// - **The Region** (LC_TIME itself) gives the first day of the week and the default clock
//   (regionClock24h); timeOfDay() shows the Region's clock until Settings' clock choice (clock24h, a bool) says
//   otherwise (decision 95).
// - **Numbers** keep LC_NUMERIC's separators (ICU's de_US would write "1.234,5"), money LC_MONETARY,
//   units LC_MEASUREMENT. Unit words and number patterns ("38 %", "2 h 5 min", "1.2万") come from the
//   UI language; unit words go through the catalogs (context "Format", with %n), so translators write
//   them. Western digits always.
//
//     Format.date(new Date(), "MMMMdEEEE")   // zh_CN + US "11月14日星期六"; en_US + SG "Saturday, 14 November";
//                                            // de_DE + US "Samstag, 14. November"; en_US + US "Saturday, November 14"
//     Format.date(d, "EEEdMMM")              // en_GB "Sat 14 Nov", en_US "Sat, Nov 14", zh_CN "11月14日周六"
//     Format.timeOfDay(new Date())           // Settings' clock: "23:05"; 12-hour: "11:05 PM", de + US "11:05 PM",
//                                            // zh + US "下午11:05"; timeOfDay(d, true): with seconds
//     Format.number(1234.5, 1)               // the Region's: "1,234.5" (US, whatever the UI language), "1.234,5" (DE)
//     Format.count(12480)                    // "12,480"
//     Format.compactNumber(123456)           // "123K"; zh/ja "12万", ko "12만", de "123.456", fr "123 k"
//     Format.percent(38)                     // "38%"; fr, de, es "38 %" (narrow no-break space)
//     Format.size(1200000)                   // "1.2 MB" (no-break space); French "1,2 Mo"
//     Format.currency(1234.56)               // the Region's money: "$1,234.56", "1.234,56 €"
//     Format.duration(7500)                  // "2 h 5 min"; Format.duration(7500, "long"): "2 hours 5 minutes"
//     Format.relativeTime(when)              // "5 minutes ago", "Yesterday", then a date
//     Format.timeZoneCity("Asia/Singapore")  // "Singapore", ja "シンガポール", zh_CN "新加坡" (decision 90)
//     Format.timeZoneRegion("America/Argentina/Buenos_Aires")   // "Argentina", de "Argentinien"
//     Format.timeZoneOffset("America/Argentina/Buenos_Aires")   // "GMT-3" (now); fr "UTC−3"
//     Format.typeName("text/plain")          // "Plain text document", de "Einfache Textdatei", ja "プレーンテキストドキュメント"
//
// Bindings that call the formatters re-run on a change through `revision`:
//
//     text: (Format.revision, Format.date(now, "yMMMd"))
#pragma once

#include <QDateTime>
#include <QFileSystemWatcher>
#include <QHash>
#include <QMap>
#include <QObject>
#include <QStringList>
#include <QTimer>
#include <QVariantMap>

#include <memory>
#include <optional>
#include <unicode/uversion.h>

U_NAMESPACE_BEGIN
class LocaleDisplayNames;
class TimeZoneNames;
U_NAMESPACE_END

class LocaleFormats : public QObject
{
    Q_OBJECT
    /// The locales in effect, without codeset ("en_SG"): LC_TIME (the Region's dates: territory, first
    /// weekday, clock), LC_NUMERIC (numbers), LC_MONETARY (the region: Settings' Region sets it), LC_MEASUREMENT.
    Q_PROPERTY(QString time READ time NOTIFY changed)
    Q_PROPERTY(QString numeric READ numeric NOTIFY changed)
    Q_PROPERTY(QString monetary READ monetary NOTIFY changed)
    Q_PROPERTY(QString measurement READ measurement NOTIFY changed)
    /// The ICU locale dates and times are written in (decision 91): "zh_Hans_US", "en_SG", "de_DE@rg=uszzzz".
    Q_PROPERTY(QString dateLocale READ dateLocale NOTIFY changed)
    /// The region's two-letter territory code ("SG"; "" for C/POSIX): Holidays › From the region.
    Q_PROPERTY(QString region READ region NOTIFY changed)
    /// True when LC_MEASUREMENT is US customary.
    Q_PROPERTY(bool usMeasurement READ usMeasurement NOTIFY changed)
    /// The UI language names and unit words come in (I18n.language; set by I18n).
    Q_PROPERTY(QString uiLanguage READ uiLanguage NOTIFY changed)
    /// The Region's first day of the week, from LC_TIME: Qt.Monday (1) … Qt.Sunday (7).
    Q_PROPERTY(int firstDayOfWeek READ firstDayOfWeek NOTIFY changed)
    /// The Region's own clock (true: 24-hour), from LC_TIME. timeOfDay() uses it unless the user picked a clock
    /// (settings.json clock24h is a bool); Theme.clock24h is the resolved value.
    Q_PROPERTY(bool regionClock24h READ regionClock24h NOTIFY changed)
    /// The Region's number symbols (LC_NUMERIC): "." and "," for en_US, "," and "." for de_DE, "," and U+202F
    /// for fr_FR, "." and U+2019 for de_CH. For code that edits numbers as text (the Calculator), never for display
    /// of a double (use number()).
    Q_PROPERTY(QString decimalSeparator READ decimalSeparator NOTIFY changed)
    Q_PROPERTY(QString groupSeparator READ groupSeparator NOTIFY changed)
    /// Goes up on every change, for bindings that call the formatters.
    Q_PROPERTY(int revision READ revision NOTIFY changed)

public:
    /// The process-wide instance. Call it in the GUI thread.
    static LocaleFormats *instance();

    QString time() const { return m_time; }
    QString numeric() const { return m_numeric; }
    QString monetary() const { return m_monetary; }
    QString measurement() const { return m_measurement; }
    QString dateLocale() const { return m_dateLocale; }
    QString region() const;
    bool usMeasurement() const;
    QString uiLanguage() const { return m_uiLanguage; }
    int firstDayOfWeek() const;
    bool regionClock24h() const;
    /// The clock of any LC_TIME value ("de_DE.UTF-8": true).
    bool regionClock24hIn(const QString &timeLocale) const;
    int revision() const { return m_revision; }
    QString decimalSeparator() const;
    QString groupSeparator() const;

    /// The parsed user and system locale.conf (I18n reads LANGUAGE from them).
    QMap<QString, QString> userValues() const { return m_user; }
    QMap<QString, QString> systemValues() const { return m_system; }
    /// From I18n: the UI language, and Settings' clock choice (nullopt: Automatic, the Region's clock).
    void setUiLanguage(const QString &language);
    void setClock24h(std::optional<bool> on);

    /// A date from an ICU skeleton ("EEEdMMM", "dMMMM", "MMMMy", "yMd", "EEEE") in `dateLocale`: the UI
    /// language's pattern, names and AM/PM with the Region's territory (decision 91).
    Q_INVOKABLE QString formatDate(const QDateTime &when, const QString &skeleton) const;
    /// The ICU pattern a skeleton gives in `dateLocale` (tests, and callers that need it).
    Q_INVOKABLE QString datePattern(const QString &skeleton) const;
    /// A number with LC_NUMERIC's separators; decimals < 0: as few as it needs.
    Q_INVOKABLE QString formatNumber(double value, int decimals = -1) const;

    // ------------------------------------------------------------ Format (decisions 34, 91)
    /// formatDate().
    Q_INVOKABLE QString date(const QDateTime &when, const QString &skeleton = QStringLiteral("yMd")) const;
    /// A time of day: Settings' 24/12-hour clock in the UI language's pattern for the Region ("11:05 PM", "下午11:05").
    Q_INVOKABLE QString timeOfDay(const QDateTime &when, bool seconds = false) const;
    /// timeOfDay() in two parts for the big clocks, which draw the AM/PM mark smaller than the digits:
    /// { time: "11:05", mark: "PM", markFirst: false }; Korean and Chinese put the mark first
    /// ({ time: "11:05", mark: "오후", markFirst: true }); a 24-hour clock has mark "" (decision 95).
    Q_INVOKABLE QVariantMap timeParts(const QDateTime &when, bool seconds = false) const;
    /// formatNumber().
    Q_INVOKABLE QString number(double value, int decimals = -1) const;
    /// A whole count with the Region's grouping: "12,480".
    Q_INVOKABLE QString count(qint64 n) const;
    /// A count shortened the UI language's way (ICU's compact short form, two significant digits) with the
    /// Region's separators: "999", "1.2K", "34K", "1.2M"; zh and ja "1234", "3.4万"; ko "1.2천", "12만"; de "12.345",
    /// "1,2 Mio."; fr "1,2 k". For counts too big to read whole (the Store's downloads); never for exact numbers.
    /// A list of names joined as the UI language writes one ("A, B, and C"; "A、B、C"), through ICU's list patterns.
    Q_INVOKABLE QString list(const QStringList &items) const;
    Q_INVOKABLE QString compactNumber(double value) const;
    /// A percentage of 0–100 (not a fraction): "38%"; French, German, Spanish "38 %".
    Q_INVOKABLE QString percent(double value, int decimals = 0) const;
    /// A size in bytes, in 1000s: "512 bytes", "36 KB", "1.2 MB", "812 GB"; French ko, Mo, Go.
    Q_INVOKABLE QString size(qint64 bytes) const;
    /// An amount of money in the Region's currency, the Region's way (LC_MONETARY): "$1,234.56", "1.234,56 €".
    Q_INVOKABLE QString currency(double value) const;
    /// A duration in seconds, its two largest units: "short" "2 h 5 min", "long" "2 hours 5 minutes".
    Q_INVOKABLE QString duration(double seconds, const QString &style = QStringLiteral("short")) const;
    /// How long ago (or until): "Just now", "5 minutes ago", "In 2 hours", "Yesterday", "3 days ago",
    /// then a date ("Nov 4"; "Nov 4, 2025" in another year). `now` omitted: the current time.
    Q_INVOKABLE QString relativeTime(const QDateTime &when, const QDateTime &now = QDateTime()) const;
    /// A month's name in the UI language: month 1–12; width "long", "short" or "narrow".
    Q_INVOKABLE QString monthName(int month, const QString &width = QStringLiteral("long"), bool standalone = true) const;
    /// A weekday's name in the UI language: Qt.Monday (1) … Qt.Sunday (7).
    Q_INVOKABLE QString dayName(int day, const QString &width = QStringLiteral("long"), bool standalone = true) const;

    // ------------------------------------------------------------ previews (decision 91; Settings)
    /// What Format gives with formats that aren't applied yet: Settings › Language & Region's format card, and the
    /// samples in its Region, Date and Numbers lists. `formats` sets any of
    ///     time, numeric, monetary   locale.conf values for LC_TIME, LC_NUMERIC, LC_MONETARY ("en_SG.UTF-8", "de_DE")
    ///     uiLanguage                a SodiumOS language ("zh_CN"); Settings passes I18n.systemLanguage, what apps show
    ///     clock24h                  Settings' clock choice: a bool, or null/left out for the Region's (a candidate Region's own clock)
    /// and what it leaves out is what is in effect. The output is exactly date(), timeOfDay(), number() and currency()'s
    /// with those values. Bindings re-run on `revision` (and I18n.revision for a language).
    ///
    ///     Format.previewDate(now, "yMMMMEEEEd", { time: "en_SG.UTF-8", uiLanguage: I18n.systemLanguage })
    ///     Format.previewNumber(1234567.89, 2, { numeric: "de_CH.UTF-8" })     // "1’234’567.89"
    Q_INVOKABLE QString previewDate(const QDateTime &when, const QString &skeleton, const QVariantMap &formats) const;
    Q_INVOKABLE QString previewTimeOfDay(const QDateTime &when, const QVariantMap &formats, bool seconds = false) const;
    Q_INVOKABLE QString previewNumber(double value, int decimals, const QVariantMap &formats) const;
    Q_INVOKABLE QString previewCurrency(double value, const QVariantMap &formats) const;
    /// The date locale those formats give ("zh_Hans_CN@rg=uszzzz"): two Date options with the same one show the same.
    Q_INVOKABLE QString previewDateLocale(const QVariantMap &formats) const;

    // ------------------------------------------------------------ time zones (decision 90)
    // The IANA id is stored and searched, never shown. A row shows the city with the region and the offset;
    // the Date & Time row and the bar "%1 (%2)" (city, offset).
    /// The CLDR exemplar city in the UI language ("Singapore", ja "シンガポール", zh_CN "新加坡", de "Kiew"); ICU's
    /// own name from the id where CLDR has none ("New Salem, North Dakota"), else the id's last part with spaces.
    /// "UTC": Coordinated Universal Time in the UI language.
    Q_INVOKABLE QString timeZoneCity(const QString &id) const;
    /// The zone's country or region as Settings' Region list names it, in the UI language ("Argentinien"); "" for UTC.
    Q_INVOKABLE QString timeZoneRegion(const QString &id) const;
    /// The zone's localized GMT offset at `when` (omitted: now), as ICU writes it: "GMT-3", "GMT+5:30", "GMT"; fr "UTC−3".
    Q_INVOKABLE QString timeZoneOffset(const QString &id, const QDateTime &when = QDateTime()) const;
    /// The zones to offer: the canonical IANA location zones this system has (no links, no Etc/*), and "UTC";
    /// sorted by timeZoneCity() with Sort (the UI language's collation). Call again on a language change.
    Q_INVOKABLE QStringList timeZoneIds() const;
    /// For search: every other id ICU knows (old names and links) → the canonical id timeZoneIds() lists:
    /// "Asia/Calcutta" → "Asia/Kolkata", "US/Eastern" → "America/New_York", "Etc/GMT" → "UTC".
    Q_INVOKABLE QVariantMap timeZoneAliases() const;
    /// The id timeZoneIds() lists for any zone id: itself, its canonical id, or "" for an unknown id.
    Q_INVOKABLE QString timeZoneCanonical(const QString &id) const;

    // ------------------------------------------------------------ file types (cleanup item 56)
    /// A MIME type's name in the UI language, from shared-mime-info's translated comments (xml:lang, through
    /// I18n.contentChain: zh_TW never shows Simplified text, en_AU takes en-GB), else its English comment; the first
    /// letter in upper case, as a label starts: "text/plain" → "Plain text document", de "Einfache Textdatei",
    /// ja "プレーンテキストドキュメント", zh_TW "純文字文件". An alias reads as its type; an unknown type is its name.
    Q_INVOKABLE QString typeName(const QString &mimeName) const;

    /// Re-reads locale.conf now.
    Q_INVOKABLE void reload();

    /// "en_US.utf8" / "en_US.UTF-8@euro" → "en_US"; empty and "C"/"POSIX" stay as they are.
    static QString base(const QString &locale);
    static QMap<QString, QString> parseConf(const QString &text);
    /// Decision 91's date locale for a SodiumOS UI language ("zh_CN"; a pseudo-locale counts as en_US) and an
    /// LC_TIME value ("en_US.UTF-8", "C"): "zh_Hans_US" when ICU has it, else "zh_Hans_CN@rg=uszzzz".
    static QString dateLocaleFor(const QString &uiLanguage, const QString &timeLocale);

signals:
    void changed();

private:
    explicit LocaleFormats(QObject *parent = nullptr);
    ~LocaleFormats() override;
    void rewatch();
    QString effective(const QString &key) const;
    void bump();

    struct Spec
    {
        QString time, numeric, monetary, uiLanguage;
        std::optional<bool> clock24h;   // nullopt: the clock of `time` (decision 95)
    };
    Spec current() const;
    Spec with(const QVariantMap &formats) const;
    QString patternIn(const QString &locale, const QString &skeleton) const;
    QString dateIn(const QString &locale, const QDateTime &when, const QString &skeleton) const;
    QString timeOfDayIn(const Spec &spec, const QDateTime &when, bool seconds) const;
    static QString numberIn(const QString &numeric, double value, int decimals);
    static QString currencyIn(const QString &monetary, double value);

    QString nameLocale() const;
    QString unitLanguage() const;
    icu::TimeZoneNames *zoneNames() const;
    icu::LocaleDisplayNames *regionNames() const;

    QMap<QString, QString> m_user, m_system;
    QString m_time, m_numeric, m_monetary, m_measurement;
    QString m_uiLanguage = QStringLiteral("en_US");
    QString m_dateLocale = QStringLiteral("en_US_POSIX");   // dateLocaleFor(m_uiLanguage, m_time)
    std::optional<bool> m_clock24h;   // nullopt: the Region's clock
    int m_revision = 0;
    QFileSystemWatcher m_watcher;
    QTimer m_debounce;
    mutable QHash<QString, QString> m_patterns;   // "<locale>|<skeleton>" → ICU pattern
    mutable QHash<QString, QHash<QString, QString>> m_typeComments;   // MIME type → {normalized xml:lang ("" the source): comment}
    mutable std::unique_ptr<icu::TimeZoneNames> m_zoneNames;
    mutable std::unique_ptr<icu::LocaleDisplayNames> m_regionNames;
    mutable QString m_namesLanguage;                  // the language m_zoneNames and m_regionNames are in
    mutable QStringList m_zoneIds;                    // timeZoneIds(), for m_zoneIdsLanguage
    mutable QString m_zoneIdsLanguage;
    mutable QVariantMap m_zoneAliases;
};
