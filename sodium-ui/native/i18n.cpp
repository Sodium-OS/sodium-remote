// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "i18n.h"

#include "desktopindex.h"

#include "i18naudit.h"
#include "localeformats.h"
#include "pseudolocale.h"
#include "sort.h"
#include "textbreaks.h"
#include "themestore.h"

#include <QCoreApplication>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QFontDatabase>
#include <QHash>
#include <QLocale>
#include <QGuiApplication>
#include <QProcess>
#include <QMutexLocker>
#include <QQmlEngine>
#include <QScopedValueRollback>

#include <unicode/locdspnm.h>
#include <unicode/unistr.h>

#include <algorithm>
#include <memory>

namespace {

const char *const kLanguages[] = {"en_US", "en_GB", "en_AU", "zh_CN", "zh_TW", "ja_JP", "ko_KR", "es_ES", "fr_FR", "de_DE"};
const char *const kPseudo[] = {"qps-ploc", "qps-ploca"};

// settings.json's clock24h: a bool is the user's choice; absent, null or anything else is Automatic (the Region's clock).
std::optional<bool> clockChoice(const QVariantMap &settings)
{
    const QVariant v = settings.value(QStringLiteral("clock24h"));
    return v.typeId() == QMetaType::Bool ? std::optional<bool>(v.toBool()) : std::nullopt;
}

// The Han groups' suffixes: only for a theme.json written before the fonts table (tokens/tokens.json "fonts";
// scripts/apply-theme.sh writes the new one), which gets Noto Sans CJK <suffix>.
const char *suffixOf(const QString &group)
{
    if (group == QLatin1String("zh-Hant"))
        return "TC";
    if (group == QLatin1String("zh-Hant-HK"))
        return "HK";
    if (group == QLatin1String("ja"))
        return "JP";
    if (group == QLatin1String("ko"))
        return "KR";
    return "SC";
}

QStringList listOf(const QVariant &v)
{
    return v.toStringList();
}

// Desktop file ids that belong to another app: the same catalog and the same own language.
const QHash<QString, QString> &appAliases()
{
    static const QHash<QString, QString> map{
        {QStringLiteral("org.sodiumos.PreviewPanel"), QStringLiteral("org.sodiumos.Preview")}};
    return map;
}

QString envValue(const char *name)
{
    return qEnvironmentVariable(name).trimmed();
}

} // namespace

// ---------------------------------------------------------------- the translator

QString I18nTranslator::translate(const char *context, const char *sourceText, const char *disambiguation, int n) const
{
    QString out;
    for (const auto &catalog : catalogs) {
        out = catalog->translate(context, sourceText, disambiguation, n);
        if (!out.isNull())
            break;
    }
    if (out.isNull() && sourceText && *sourceText) {
        const QString source = QString::fromUtf8(sourceText);
        if (pseudo == QLatin1String("qps-ploc")) {
            out = pseudo::accented(pseudo::resolvePlural(source, n));
        } else if (pseudo == QLatin1String("qps-ploca")) {
            out = pseudo::cjk(pseudo::resolvePlural(source, n));
        } else if (n >= 0) {
            // No catalog has it: the English source, with its plural form picked as en_US.ts would.
            const QString resolved = pseudo::resolvePlural(source, n);
            if (resolved != source)
                out = resolved;
        }
    }
    if (recordSources && sourceText && *sourceText) {
        const QString source = QString::fromUtf8(sourceText);
        QMutexLocker lock(&m_mutex);
        m_sources.insert(out.isNull() ? source : out, source);
    }
    return out;
}

QString I18nTranslator::sourceFor(const QString &shown) const
{
    QMutexLocker lock(&m_mutex);
    const auto hit = m_sources.constFind(shown);
    if (hit != m_sources.cend())
        return hit.value();
    // "%1 items" shown as "8 items": the longest translation whose fixed start the text begins with.
    QString best, bestKey;
    for (auto it = m_sources.cbegin(); it != m_sources.cend(); ++it) {
        const QString &key = it.key();
        const int cut = int(key.indexOf(QLatin1Char('%')));
        if (cut < 0)
            continue;
        const QStringView head = QStringView(key).left(cut);
        if (head.size() >= 3 && shown.startsWith(head) && head.size() > bestKey.size()) {
            bestKey = head.toString();
            best = it.value();
        }
    }
    return best;
}

// ---------------------------------------------------------------- I18n

I18n *I18n::instance()
{
    static I18n *self = new I18n;
    return self;
}

I18n::I18n(QObject *parent)
    : QObject(parent)
    , m_language(QStringLiteral("en_US"))
    , m_systemLanguage(QStringLiteral("en_US"))
    , m_source(QStringLiteral("default"))
    , m_translator(std::make_unique<I18nTranslator>())
{
}

QStringList I18n::languages() const
{
    QStringList out;
    for (const char *l : kLanguages)
        out << QLatin1String(l);
    return out;
}

QStringList I18n::pseudoLanguages() const
{
    QStringList out;
    for (const char *l : kPseudo)
        out << QLatin1String(l);
    return out;
}

bool I18n::auditActive() const
{
    return I18nAudit::enabled();
}

QString I18n::match(const QString &tag)
{
    QString t = tag.trimmed();
    t = t.section(QLatin1Char('.'), 0, 0).section(QLatin1Char('@'), 0, 0);
    t.replace(QLatin1Char('-'), QLatin1Char('_'));
    const QString lower = t.toLower();
    if (lower == QLatin1String("qps_ploc"))
        return QStringLiteral("qps-ploc");
    if (lower == QLatin1String("qps_ploca"))
        return QStringLiteral("qps-ploca");
    const QStringList parts = t.split(QLatin1Char('_'), Qt::SkipEmptyParts);
    if (parts.isEmpty())
        return {};
    const QString lang = parts.first().toLower();
    QString script, territory;
    for (int i = 1; i < parts.size(); ++i) {
        if (parts[i].size() == 4)
            script = parts[i].toLower();
        else if (territory.isEmpty())
            territory = parts[i].toUpper();
    }
    if (lang == QLatin1String("en")) {
        if (territory == QLatin1String("GB"))
            return QStringLiteral("en_GB");
        if (territory == QLatin1String("AU"))
            return QStringLiteral("en_AU");
        return QStringLiteral("en_US");   // PROVISIONAL: en_NZ, en_IE ... read as US English (decision 36)
    }
    if (lang == QLatin1String("zh")) {
        if (script == QLatin1String("hant") || territory == QLatin1String("TW") || territory == QLatin1String("HK")
            || territory == QLatin1String("MO"))
            return QStringLiteral("zh_TW");
        return QStringLiteral("zh_CN");   // zh, zh_CN, zh_SG, zh_Hans (decision 2)
    }
    static const QHash<QString, QString> one{{QStringLiteral("ja"), QStringLiteral("ja_JP")},
                                             {QStringLiteral("ko"), QStringLiteral("ko_KR")},
                                             {QStringLiteral("es"), QStringLiteral("es_ES")},
                                             {QStringLiteral("fr"), QStringLiteral("fr_FR")},
                                             {QStringLiteral("de"), QStringLiteral("de_DE")}};
    return one.value(lang);
}

QStringList I18n::chainFor(const QString &language)
{
    if (language.isEmpty() || language.startsWith(QLatin1String("qps-")))
        return {};
    if (language == QLatin1String("en_AU"))
        return {QStringLiteral("en_GB"), QStringLiteral("en_AU")};
    return {language};
}

QString I18n::scriptGroupOf(const QString &language)
{
    if (language == QLatin1String("zh_CN"))
        return QStringLiteral("zh-Hans");
    if (language == QLatin1String("zh_TW"))
        return QStringLiteral("zh-Hant");
    if (language == QLatin1String("ja_JP") || language == QLatin1String("qps-ploca"))
        return QStringLiteral("ja");
    if (language == QLatin1String("ko_KR"))
        return QStringLiteral("ko");
    return QStringLiteral("latin");
}

QStringList I18n::catalogs() const
{
    QStringList out = m_baseCatalogs;
    for (const QString &c : m_extraCatalogs)
        if (!out.contains(c))
            out << c;
    return out;
}

QStringList I18n::catalogDirs() const
{
    QStringList dirs;
    for (const QString &d : qEnvironmentVariable("SODIUMOS_I18N_DIR").split(QDir::listSeparator(), Qt::SkipEmptyParts))
        dirs << QDir::cleanPath(d);
    // Set, it replaces the search path (tests and the live checks rely on seeing only their own catalogs, even
    // where the checkout has built .qm files); to add a scratch folder, list build/i18n/qm after it.
    if (qEnvironmentVariableIsSet("SODIUMOS_I18N_DIR"))
        return dirs;
    if (!m_moduleDir.isEmpty()) {
        dirs << QDir::cleanPath(m_moduleDir + QStringLiteral("/../../build/i18n/qm"));
#ifdef SODIUMUI_PORTABLE
        // A packaged app: <app>/translations next to the SodiumUI module folder (or inside it).
        dirs << QDir::cleanPath(m_moduleDir + QStringLiteral("/../translations"))
             << QDir::cleanPath(m_moduleDir + QStringLiteral("/translations"));
#endif
    }
    dirs << QStringLiteral("/usr/share/sodiumos/translations");
    return dirs;
}

QString I18n::findCatalog(const QString &name, const QString &language) const
{
    const QString file = name + QLatin1Char('_') + language + QStringLiteral(".qm");
    for (const QString &dir : catalogDirs()) {
        const QString path = dir + QLatin1Char('/') + file;
        if (QFileInfo::exists(path))
            return path;
    }
    return {};
}

// Which process this is: an app (its desktop file id), the shell or the greeter (Quickshell: the
// config folder on its command line), or something else (a test runner).
void I18n::identify()
{
    m_appId.clear();
    // Quickshell first: it names itself org.quickshell as a desktop file too.
    if (QCoreApplication::applicationName() == QLatin1String("quickshell")) {
        // qs -p <folder or file>: Quickshell doesn't pass its arguments on to Qt.
        QFile f(QStringLiteral("/proc/self/cmdline"));
        QString config;
        if (f.open(QIODevice::ReadOnly)) {
            const QList<QByteArray> args = f.readAll().split('\0');
            for (int i = 0; i + 1 < args.size(); ++i)
                if (args[i] == "-p" || args[i] == "--path" || args[i] == "-c" || args[i] == "--config")
                    config = QString::fromLocal8Bit(args[i + 1]);
        }
        QString base = QDir::cleanPath(config);
        if (base.endsWith(QLatin1String(".qml")))
            base = QFileInfo(base).absolutePath();
        if (QFileInfo(base).fileName() == QLatin1String("greeter")) {
            m_role = QStringLiteral("greeter");
            m_baseCatalogs = {QStringLiteral("sodiumui"), QStringLiteral("greeter")};
        } else {
            m_role = QStringLiteral("shell");
            m_baseCatalogs = {QStringLiteral("sodiumui"), QStringLiteral("shell"), QStringLiteral("settings-pages"),
                              QStringLiteral("desktop"), QStringLiteral("sodiumos-input"), QStringLiteral("sodiumos-input-qml")};
        }
        m_name = m_role;
        return;
    }
    const QString desktop = QGuiApplication::desktopFileName();
    if (!desktop.isEmpty()) {
        QString id = desktop;
        if (id.endsWith(QLatin1String(".desktop")))
            id.chop(8);
        id = appAliases().value(id, id);
        const QString shortName = id.section(QLatin1Char('.'), -1).toLower();
        m_role = QStringLiteral("app");
        m_appId = id;
        m_name = shortName;
        m_baseCatalogs = {QStringLiteral("sodiumui"), shortName};
        if (shortName == QLatin1String("settings"))   // its Input pages use the input method's module and names (GL5)
            m_baseCatalogs << QStringLiteral("settings-pages") << QStringLiteral("sodiumos-input")
                           << QStringLiteral("sodiumos-input-qml");
        return;
    }
    m_role = QStringLiteral("other");
    m_name = QCoreApplication::applicationName().isEmpty() ? QStringLiteral("process") : QCoreApplication::applicationName();
    m_baseCatalogs = {QStringLiteral("sodiumui")};
}

void I18n::ensureStore()
{
    const QString dir = ThemeStore::stateDir();
    if (m_store && dir == m_storeDir)
        return;
    delete m_store;
    m_storeDir = dir;
    m_store = new ThemeStore(m_moduleDir + QStringLiteral("/theme.default.json"), this);
    connect(m_store, &ThemeStore::settingsChanged, this, [this] {
        LocaleFormats::instance()->setClock24h(clockChoice(m_store->settings()));
        resolve();
    });
    // The look changed (apply-theme.sh wrote another maxui/theme.json): its font table, live.
    connect(m_store, &ThemeStore::themeChanged, this, [this] { applyFonts(); });
    LocaleFormats::instance()->setClock24h(clockChoice(m_store->settings()));
}

void I18n::start()
{
    if (m_started)
        return;
    m_started = true;
    // Marks the process as one whose translations I18n runs, so a module with a catalogue of its own (the input
    // method's org.sodiumos.input, GL5) leaves it to loadCatalog() and follows the app's language live instead of
    // installing a translator for QLocale().
    if (QCoreApplication *app = QCoreApplication::instance())
        app->setProperty("sodiumosI18n", true);
    identify();
    ensureStore();
    m_translator->recordSources = I18nAudit::enabled();
    connect(LocaleFormats::instance(), &LocaleFormats::changed, this, &I18n::resolve);
    resolve();
    if (I18nAudit::enabled())
        I18nAudit::instance()->start();
}

void I18n::attachEngine(QQmlEngine *engine)
{
    start();
    m_engines.removeAll(QPointer<QQmlEngine>());
    for (const auto &e : std::as_const(m_engines))
        if (e == engine)
            return;
    m_engines.append(engine);
    engine->setUiLanguage(m_language);
}

void I18n::reload()
{
    if (!m_started) {
        start();
        return;
    }
    identify();
    ensureStore();
    m_store->reload();
    LocaleFormats::instance()->reload();
    m_reapply = true;   // load the catalogs again: their folders or this process's name may have changed
    resolve();
}

// Decision 106: the Preferred languages list is SODIUMOS_LANGUAGES; LANGUAGE (derived from it, ending at English)
// only when the file predates it, or for the process environment.
QStringList I18n::preferredFrom(const QMap<QString, QString> &conf)
{
    const QString list = conf.value(QStringLiteral("SODIUMOS_LANGUAGES"));
    return (list.isEmpty() ? conf.value(QStringLiteral("LANGUAGE")) : list).split(QLatin1Char(':'), Qt::SkipEmptyParts);
}

QString I18n::systemLanguageFrom(QString *source) const
{
    const auto firstOf = [](const QMap<QString, QString> &conf, bool environment) -> QString {
        QStringList candidates = preferredFrom(conf);
        if (environment)
            candidates << conf.value(QStringLiteral("LC_ALL"));
        candidates << conf.value(QStringLiteral("LC_MESSAGES")) << conf.value(QStringLiteral("LANG"));
        for (const QString &c : std::as_const(candidates)) {
            const QString m = match(c);   // a pseudo-locale counts too: a developer's own choice
            if (!m.isEmpty())
                return m;
        }
        return {};
    };
    LocaleFormats *formats = LocaleFormats::instance();
    for (const auto &conf : {formats->userValues(), formats->systemValues()}) {
        const QString m = firstOf(conf, false);
        if (!m.isEmpty()) {
            *source = QStringLiteral("system");
            return m;
        }
    }
    QMap<QString, QString> env;
    for (const char *k : {"LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"})
        env.insert(QLatin1String(k), envValue(k));
#ifdef Q_OS_WIN
    // Windows has no LANG: the display language the user set (Settings > Time & language), as de_DE.
    if (env.value(QStringLiteral("LANGUAGE")).isEmpty() && env.value(QStringLiteral("LC_ALL")).isEmpty()
        && env.value(QStringLiteral("LC_MESSAGES")).isEmpty() && env.value(QStringLiteral("LANG")).isEmpty()) {
        const QStringList ui = QLocale::system().uiLanguages();
        if (!ui.isEmpty())
            env.insert(QStringLiteral("LANG"), QString(ui.first()).replace(QLatin1Char('-'), QLatin1Char('_')));
    }
#endif
    const QString m = firstOf(env, true);
    if (!m.isEmpty()) {
        *source = QStringLiteral("system");
        return m;
    }
    *source = QStringLiteral("default");
    return QStringLiteral("en_US");
}

QString I18n::appLanguage() const
{
    if (m_role != QLatin1String("app") || m_appId.isEmpty() || !m_store)
        return {};
    const QVariantMap map = m_store->settings().value(QStringLiteral("appLanguages")).toMap();
    return match(map.value(m_appId).toString());
}

void I18n::setOverrideLanguage(const QString &tag)
{
    const QString language = match(tag);
    if (language == m_override)
        return;
    m_override = language;
    resolve();
}

void I18n::resolve()
{
    if (m_resolving || !m_started)
        return;
    QScopedValueRollback<bool> guard(m_resolving, true);
    QString systemSource;
    const QString system = systemLanguageFrom(&systemSource);
    QString language, source;
    const QString env = match(envValue("SODIUMOS_UI_LANGUAGE"));
    const QString own = appLanguage();
    if (!m_override.isEmpty()) {
        language = m_override;
        source = QStringLiteral("override");
    } else if (!env.isEmpty()) {
        language = env;
        source = QStringLiteral("environment");
    } else if (!own.isEmpty()) {
        language = own;
        source = QStringLiteral("app");
    } else {
        language = system;
        source = systemSource;
    }
    const bool detailsChanged = system != m_systemLanguage || source != m_source;
    m_systemLanguage = system;
    m_source = source;
    if (language != m_language || !m_installed || m_reapply) {
        m_reapply = false;
        m_language = language;
        apply();
    } else {
        applyFonts();   // Region Hong Kong picks other Han forms for zh_TW
        const bool chainChanged = publishChain();   // and puts zh_TW's HK tags first in the content chain
        if (chainChanged)
            ++m_revision;
        if (detailsChanged || chainChanged)
            emit languageChanged();
    }
}

void I18n::apply()
{
    rebuildTranslator();
    applyFonts();
    LocaleFormats::instance()->setUiLanguage(m_language);
    Sort::instance()->setLanguage(m_language);
    publishChain();
    ++m_revision;
    retranslateEngines();
    emit catalogsChanged();
    emit languageChanged();
}

void I18n::rebuildTranslator()
{
    if (m_installed)
        QCoreApplication::removeTranslator(m_translator.get());
    m_translator->catalogs.clear();
    m_loaded.clear();
    std::vector<std::unique_ptr<QTranslator>> loaded;
    const QStringList names = catalogs();
    for (const QString &language : chainFor(m_language)) {
        for (const QString &name : names) {
            const QString path = findCatalog(name, language);
            if (path.isEmpty())
                continue;
            auto t = std::make_unique<QTranslator>();
            if (!t->load(path)) {
                qWarning("SodiumUI i18n: %s can't be read", qPrintable(path));
                continue;
            }
            m_loaded << path;
            loaded.push_back(std::move(t));
        }
    }
    // Qt searches the last-installed first: en_AU before en_GB.
    std::reverse(loaded.begin(), loaded.end());
    m_translator->catalogs = std::move(loaded);
    m_translator->pseudo = isPseudo() ? m_language : QString();
    QCoreApplication::installTranslator(m_translator.get());   // sends LanguageChange
    m_installed = true;
}

// Which Han forms (decision 46): the UI language's; a Latin UI language takes the first Chinese,
// Japanese or Korean entry in Preferred languages (SODIUMOS_LANGUAGES, decision 106), else the Region's (JP, KR, TW, HK, MO),
// else Simplified Chinese. zh_TW with Region Hong Kong or Macau takes HK.
QString I18n::hanGroup() const
{
    LocaleFormats *formats = LocaleFormats::instance();
    const QString region = formats->region();
    QString group = scriptGroupOf(m_language);
    if (group == QLatin1String("latin")) {
        QStringList preferred;
        for (const auto &conf : {formats->userValues(), formats->systemValues()}) {
            if (preferred.isEmpty())
                preferred = preferredFrom(conf);
        }
        if (preferred.isEmpty())
            preferred = envValue("LANGUAGE").split(QLatin1Char(':'), Qt::SkipEmptyParts);
        for (const QString &entry : std::as_const(preferred)) {
            const QString g = scriptGroupOf(match(entry));
            if (g != QLatin1String("latin") && !match(entry).startsWith(QLatin1String("qps-"))) {
                group = g;
                break;
            }
        }
        if (group == QLatin1String("latin")) {
            static const QHash<QString, QString> byRegion{{QStringLiteral("JP"), QStringLiteral("ja")},
                                                          {QStringLiteral("KR"), QStringLiteral("ko")},
                                                          {QStringLiteral("TW"), QStringLiteral("zh-Hant")},
                                                          {QStringLiteral("HK"), QStringLiteral("zh-Hant")},
                                                          {QStringLiteral("MO"), QStringLiteral("zh-Hant")}};
            group = byRegion.value(region, QStringLiteral("zh-Hans"));
        }
    }
    if (group == QLatin1String("zh-Hant") && (region == QLatin1String("HK") || region == QLatin1String("MO")))
        group = QStringLiteral("zh-Hant-HK");
    return group;
}

void I18n::applyFonts()
{
    // The look's font table (tokens/tokens.json "fonts", with tokens/looks/<look>.json laid over it) from
    // theme.json: one source for fontconfig and for these lists (decisions 41, 46, 48, 66).
    const QVariantMap fonts = m_store ? m_store->theme().value(QStringLiteral("fonts")).toMap() : QVariantMap();
    const QString group = hanGroup();
    const QString emoji = fonts.value(QStringLiteral("emoji"), QStringLiteral("Noto Color Emoji")).toString();
    QStringList latin = listOf(fonts.value(QStringLiteral("latin")));
    if (latin.isEmpty())
        latin << fonts.value(QStringLiteral("ui"), QStringLiteral("Inter")).toString();
    const auto groupList = [&](const char *key, const char *legacy) {
        const QVariant v = fonts.value(QLatin1String(key));
        QStringList list = v.typeId() == QMetaType::QVariantMap ? listOf(v.toMap().value(group)) : QStringList();
        if (list.isEmpty())
            list << QStringLiteral("%1 %2").arg(QLatin1String(legacy), QLatin1String(suffixOf(group)));
        return list;
    };
    const QStringList cjk = groupList("cjk", "Noto Sans CJK");
    const QStringList monoCjk = groupList("monoCjk", "Noto Sans Mono CJK");
    if (group == m_fontGroup && latin == m_latin && cjk == m_cjk && monoCjk == m_monoCjk && emoji == m_emoji)
        return;
    m_fontGroup = group;
    m_latin = latin;
    m_cjk = cjk;
    m_monoCjk = monoCjk;
    m_emoji = emoji;
    // What follows the UI font: the look's other Latin fonts (Petal: Inter after Nunito), the group's CJK, emoji.
    m_cjkUi = QStringList(latin.mid(1)) + cjk + QStringList{emoji};
    m_cjkMono = monoCjk + QStringList{emoji};
    // Every text in the process, Theme.font or not, gets these Han forms. Qt drops its cached font
    // engines here, so this switches live (a QFont's own family list alone wouldn't: Qt reuses the
    // engine of the first font with the same primary family).
    if (qobject_cast<QGuiApplication *>(QCoreApplication::instance())) {
        for (QChar::Script s : {QChar::Script_Han, QChar::Script_Hiragana, QChar::Script_Katakana, QChar::Script_Hangul,
                                QChar::Script_Bopomofo})
            QFontDatabase::setApplicationFallbackFontFamilies(s, cjk);
    }
    emit fontsChanged();
    syncHanOrder();
}

// The Han group is the shell's to publish (one process, not every app): the value file the input addon
// reads (~/.config/sodiumos/han-group), 59-sodiumos-han-order.conf for third-party apps and kitty's
// cjk.conf, through tokens/generate.py --han-order. Only when the value changed (or the file is missing).
void I18n::syncHanOrder()
{
    if (m_role != QLatin1String("shell") && qEnvironmentVariable("SODIUMOS_HAN_ORDER") != QLatin1String("1"))
        return;
    if (m_moduleDir.isEmpty() || m_fontGroup.isEmpty())
        return;
    const QString generate = QDir::cleanPath(m_moduleDir + QStringLiteral("/../../tokens/generate.py"));
    if (!QFileInfo::exists(generate))
        return;
    const QString config = qEnvironmentVariable("XDG_CONFIG_HOME").isEmpty()
        ? QDir::homePath() + QStringLiteral("/.config") : qEnvironmentVariable("XDG_CONFIG_HOME");
    QFile f(config + QStringLiteral("/sodiumos/han-group"));
    if (f.open(QIODevice::ReadOnly) && QString::fromUtf8(f.readAll()).trimmed() == m_fontGroup
        && QFileInfo::exists(config + QStringLiteral("/fontconfig/conf.d/59-sodiumos-han-order.conf")))
        return;
    QStringList args{generate, QStringLiteral("--han-order"), m_fontGroup, QStringLiteral("--settings"),
                     ThemeStore::stateDir() + QStringLiteral("/settings.json")};
    if (qEnvironmentVariable("SODIUMOS_HAN_RELOAD") != QLatin1String("0"))
        args << QStringLiteral("--reload-kitty");   // SIGUSR1 to kitty when its cjk.conf changed; tests set it to 0
    auto *process = new QProcess(this);
    connect(process, &QProcess::finished, process, &QObject::deleteLater);
    connect(process, &QProcess::errorOccurred, process, &QObject::deleteLater);
    process->start(QStringLiteral("python3"), args);
}

void I18n::retranslateEngines()
{
    m_engines.removeAll(QPointer<QQmlEngine>());
    for (const auto &engine : std::as_const(m_engines)) {
        engine->setUiLanguage(m_language);
        engine->retranslate();
    }
}

bool I18n::loadCatalog(const QString &name)
{
    const QString n = name.trimmed();
    if (n.isEmpty())
        return false;
    bool found = false;
    for (const QString &language : chainFor(m_language))
        found = found || !findCatalog(n, language).isEmpty();
    if (catalogs().contains(n))
        return found;
    m_extraCatalogs << n;
    if (found && m_started) {
        // Queued: this is often called while a component is being created.
        QMetaObject::invokeMethod(this, [this] {
            rebuildTranslator();
            retranslateEngines();
            emit catalogsChanged();
        }, Qt::QueuedConnection);
    } else {
        emit catalogsChanged();
    }
    return found;
}

QFont I18n::font(const QFont &base, bool mono, bool palt, bool numeric) const
{
    QFont f = base;
    // Decision 72: Qt 6.12 fakes bold (FT_GlyphSlot_Embolden) for any request at weight >= 700 on a variable font
    // whose default instance isn't bold (Nunito, Resource Han Rounded), on top of the real Bold instance. 699 picks
    // the same named instance without it, and static fonts resolve 699 and 700 alike. Unconditional: the UI font is
    // the look's (no user-picked UI font), so every family here is one of those.
    if (f.weight() >= QFont::Bold)
        f.setWeight(QFont::Weight(f.weight() - 1));
    QStringList families{base.family()};
    if (mono) {
        families += m_cjkMono;
    } else {
        for (const QString &l : m_latin)   // the look's Latin fonts after the UI font: Nunito, then Inter
            if (l != base.family())
                families << l;
        families += m_cjk + QStringList{m_emoji};
    }
    f.setFamilies(families);
    // Decision 45: the typographic metrics in every language, so a line keeps the Latin height and
    // baseline whatever script falls back into it.
    f.setStyleStrategy(QFont::StyleStrategy(f.styleStrategy() | QFont::PreferTypoLineMetrics));
    if (palt)
        f.setFeature(QFont::Tag("palt"), 1);   // decision 47: Japanese proportional punctuation
    if (numeric)
        f.setFeature(QFont::Tag("tnum"), 1);   // decision 56: tabular digits
    return f;
}

QString I18n::sourceFor(const QString &shown) const
{
    return m_translator->sourceFor(shown);
}

QStringList I18n::contentChain(const QString &language) const
{
    const QString l = language.isEmpty() ? m_language : match(language);
    return contentChainFor(l, LocaleFormats::instance()->region());
}

QString I18n::appName(const QString &desktopId, const QString &fallback) const
{
    const QString name = DesktopIndex::instance()->text(desktopId, QStringLiteral("Name"));
    if (!name.isEmpty())
        return name;
    return fallback.isEmpty() ? desktopId : fallback;
}

QString I18n::desktopText(const QString &desktopId, const QString &key, const QString &fallback) const
{
    const QString text = DesktopIndex::instance()->text(desktopId, key);
    return text.isEmpty() ? fallback : text;
}

QStringList I18n::desktopKeywords(const QString &desktopId) const
{
    DesktopIndex *index = DesktopIndex::instance();
    const QString shown = index->text(desktopId, QStringLiteral("Keywords"));
    const QString plain = index->text(desktopId, QStringLiteral("Keywords"), QStringList());
    QStringList out;
    for (const QString &list : {shown, plain}) {
        for (const QString &k : list.split(QLatin1Char(';'), Qt::SkipEmptyParts)) {
            const QString t = k.trimmed();
            if (!t.isEmpty() && !out.contains(t))
                out << t;
        }
    }
    return out;
}

QString I18n::desktopActionName(const QString &desktopId, const QString &action, const QString &fallback) const
{
    const QString text = DesktopIndex::instance()->text(desktopId, QStringLiteral("Action ") + action + QStringLiteral("/Name"));
    return text.isEmpty() ? fallback : text;
}

// Decision 105: the chain for code that doesn't link Sodium UI (DesktopText, ui/native/desktoptext.h), set before the
// revision goes up so a binding that re-reads on I18n.revision gets the new one.
bool I18n::publishChain()
{
    QCoreApplication *app = QCoreApplication::instance();
    if (!app)
        return false;
    const QStringList chain = contentChain();
    const QVariant old = app->property("sodiumosContentChain");
    if (old.isValid() && old.toStringList() == chain)
        return false;
    app->setProperty("sodiumosContentChain", chain);
    return true;
}

QStringList I18n::contentChainFor(const QString &language, const QString &region)
{
    if (language == QLatin1String("en_GB"))
        return {QStringLiteral("en-GB")};
    if (language == QLatin1String("en_AU"))
        return {QStringLiteral("en-AU"), QStringLiteral("en-GB")};
    if (language == QLatin1String("zh_CN"))
        return {QStringLiteral("zh-Hans-CN"), QStringLiteral("zh-CN"), QStringLiteral("zh-Hans")};
    if (language == QLatin1String("zh_TW")) {
        const QStringList tw{QStringLiteral("zh-Hant-TW"), QStringLiteral("zh-TW")};
        const QStringList hk{QStringLiteral("zh-Hant-HK"), QStringLiteral("zh-HK")};
        if (region == QLatin1String("HK") || region == QLatin1String("MO"))
            return hk + QStringList{QStringLiteral("zh-Hant")} + tw;
        return tw + QStringList{QStringLiteral("zh-Hant")} + hk;
    }
    for (const char *l : {"de_DE", "es_ES", "fr_FR", "ja_JP", "ko_KR"}) {
        if (language == QLatin1String(l)) {
            QString tag = language;
            tag.replace(QLatin1Char('_'), QLatin1Char('-'));
            return {tag, language.section(QLatin1Char('_'), 0, 0)};
        }
    }
    return {};   // en_US and the pseudo-locales: the source
}

qreal I18n::balancedWidth(const QString &text, const QFont &font, qreal width) const
{
    return TextBreaks::balancedWidth(text, font, width);
}

QString I18n::joinPhrases(const QString &text) const
{
    return TextBreaks::joinPhrases(text);
}

QString I18n::stripBreakMarks(const QString &text) const
{
    return TextBreaks::stripBreakMarks(text);
}

// ---- Language names (decision 22; ported from apps/settings/src/languages.cpp, which keeps its own copy for now)

namespace {

struct KnownLanguage {
    const char *code;
    const char *endonym;
    const char *name; // the English source of its name in the UI language (context LanguageNames)
};

// The SodiumOS languages in decision 22's order, the boards' names (LangSettings L1a, L1d).
const KnownLanguage kKnownLanguages[] = {
    {"en_US", "English", QT_TRANSLATE_NOOP("LanguageNames", "English")},
    {"en_GB", "English (UK)", QT_TRANSLATE_NOOP("LanguageNames", "English (UK)")},
    {"en_AU", "English (Australia)", QT_TRANSLATE_NOOP("LanguageNames", "English (Australia)")},
    {"de_DE", "Deutsch", QT_TRANSLATE_NOOP("LanguageNames", "German")},
    {"es_ES", "Español", QT_TRANSLATE_NOOP("LanguageNames", "Spanish")},
    {"fr_FR", "Français", QT_TRANSLATE_NOOP("LanguageNames", "French")},
    {"ja_JP", "日本語", QT_TRANSLATE_NOOP("LanguageNames", "Japanese")},
    {"ko_KR", "한국어", QT_TRANSLATE_NOOP("LanguageNames", "Korean")},
    {"zh_CN", "简体中文", QT_TRANSLATE_NOOP("LanguageNames", "Chinese, Simplified")},
    {"zh_TW", "繁體中文", QT_TRANSLATE_NOOP("LanguageNames", "Chinese, Traditional")},
};

/// "de-DE", "de_DE.UTF-8@euro" → "de_DE"
QString languageCode(const QString &tag)
{
    return tag.trimmed().section(QLatin1Char('.'), 0, 0).section(QLatin1Char('@'), 0, 0).replace(QLatin1Char('-'), QLatin1Char('_'));
}

const KnownLanguage *knownLanguage(const QString &code)
{
    for (const KnownLanguage &k : kKnownLanguages)
        if (code == QLatin1String(k.code))
            return &k;
    return nullptr;
}

/// One ICU display-names object per language (slow to make), for names in a list or menu.
icu::LocaleDisplayNames *displayNamesIn(const QString &in)
{
    static QMutex mutex;
    static QHash<QString, std::shared_ptr<icu::LocaleDisplayNames>> cache;
    const QString key = in.isEmpty() || in.startsWith(QLatin1String("qps")) ? QStringLiteral("en_US") : in;
    QMutexLocker lock(&mutex);
    auto it = cache.find(key);
    if (it == cache.end()) {
        UDisplayContext ctx[] = {UDISPCTX_STANDARD_NAMES, UDISPCTX_CAPITALIZATION_FOR_UI_LIST_OR_MENU};
        it = cache.insert(key, std::shared_ptr<icu::LocaleDisplayNames>(
                                   icu::LocaleDisplayNames::createInstance(icu::Locale(key.toLatin1().constData()), ctx, 2)));
    }
    return it->get();
}

/// ICU's name for `code` in `in`: the language alone, or with its territory.
QString icuLanguageName(const QString &code, const QString &in, bool languageOnly)
{
    icu::LocaleDisplayNames *names = displayNamesIn(in);
    if (!names)
        return code;
    icu::UnicodeString out;
    const QByteArray c = (languageOnly ? code.section(QLatin1Char('_'), 0, 0) : code).toLatin1();
    if (languageOnly)
        names->languageDisplayName(c.constData(), out);
    else
        names->localeDisplayName(c.constData(), out);
    QString s = QString::fromUtf16(reinterpret_cast<const char16_t *>(out.getBuffer()), out.length());
    if (!s.isEmpty() && s.at(0).isLower())
        s[0] = s.at(0).toUpper();
    return s;
}

/// The territory is the language's main one (de_DE, ja_JP), or there is none: the language alone names it.
bool mainTerritory(const QString &code)
{
    if (!code.contains(QLatin1Char('_')))
        return true;
    const QLocale whole(code);
    return QLocale(whole.language()).territory() == whole.territory();
}

} // namespace

QString I18n::languageEndonymOf(const QString &tag)
{
    const QString code = languageCode(tag);
    if (const KnownLanguage *k = knownLanguage(code))
        return QString::fromUtf8(k->endonym);
    return icuLanguageName(code, code, mainTerritory(code));
}

QString I18n::languageNameIn(const QString &tag, const QString &uiLanguage)
{
    const QString code = languageCode(tag);
    const KnownLanguage *k = knownLanguage(code);
    if (!k)
        return icuLanguageName(code, uiLanguage, mainTerritory(code));
    const QString name = QCoreApplication::translate("LanguageNames", k->name);
    // No catalog has it yet: ICU's name in that language, rather than English in a German UI.
    const bool english = uiLanguage.isEmpty() || uiLanguage.startsWith(QLatin1String("en")) || uiLanguage.startsWith(QLatin1String("qps"));
    if (english || name != QLatin1String(k->name))
        return name;
    const bool zh = code.startsWith(QLatin1String("zh_"));
    const bool variant = zh || (code.startsWith(QLatin1String("en_")) && code != QLatin1String("en_US"));
    return icuLanguageName(code == QLatin1String("zh_CN") ? QStringLiteral("zh_Hans") : code == QLatin1String("zh_TW") ? QStringLiteral("zh_Hant") : code,
                           uiLanguage, !variant);
}

QString I18n::languageName(const QString &tag) const
{

    return languageNameIn(tag, m_language);
}

QString I18n::languageEndonym(const QString &tag) const
{
    return languageEndonymOf(tag);
}

QString I18n::lineBreakFault(QQuickItem *text) const
{
    return I18nAudit::lineBreakFault(text);
}
