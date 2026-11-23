// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// I18n: Sodium UI's language runtime (docs/i18n.md; docs/decisions/i18n.md, decisions 5, 9, 10, 12, 16, 19, 21).
//
// One per process, started by the SodiumUI plugin when the first engine imports SodiumUI, so every app,
// the shell, the greeter and the file chooser get it with no code of their own.
//
// The UI language, first that gives one of the SodiumOS languages (`languages`):
//   0. `overrideLanguage`, set at run time (the greeter shows the selected user's language);
//   1. $SODIUMOS_UI_LANGUAGE (tests and the sweep; any id, the pseudo-locales too);
//   2. this app's own language: the setting `appLanguages` ({ "<desktop file id>": "<language>" }, in
//      <state>/settings.json, as ThemeStore reads it; watched). Not for the shell or the greeter;
//   3. the system language: the first SodiumOS language in Preferred languages (SODIUMOS_LANGUAGES, else LANGUAGE;
//      decision 106), then LC_MESSAGES, LANG, of
//      $XDG_CONFIG_HOME/locale.conf, else /etc/locale.conf, else this process's environment (watched,
//      through LocaleFormats);
//   4. en_US.
// "de", "de_AT.UTF-8" and "de-CH" all mean de_DE; zh_HK/zh_MO/zh_Hant mean zh_TW; any other English en_US.
//
// Catalogs are Qt .qm files named <catalog>_<language>.qm, looked up in $SODIUMOS_I18N_DIR (colon-separated; when set it is the whole search path),
// then <repo>/build/i18n/qm (found from this plugin's own folder, as the repo runs in place), then
// /usr/share/sodiumos/translations. A missing file is skipped silently: the source text shows.
// Every process loads `sodiumui`; an app its own (org.sodiumos.Calculator → calculator, PreviewPanel →
// preview), Settings also settings-pages; the shell `shell`, `settings-pages` and `desktop` (the
// desktop surface runs in it); the greeter `greeter`. Settings and the shell also load the input method's
// `sodiumos-input` (its source, language and skin names, empty context: qsTranslate("", msgid)) and
// `sodiumos-input-qml` (its QML module); the qApp property sodiumosI18n tells that module to install no
// translator of its own (GL5). loadCatalog() adds one by name.
// Fallback (decision 10): en_AU loads en_GB, then en_AU over it; every other language only itself
// (zh_CN and zh_TW never borrow from each other); a message no catalog has shows the English source,
// with its "(s)" plural resolved by %n. en_US loads <catalog>_en_US.qm if present (plural forms).
//
// A change switches live: the translators are swapped, then every engine of the process gets
// setUiLanguage() and retranslate(); languageChanged() follows. QLocale::setDefault is never called:
// formats come from Region (LocaleFormats / Format).
//
// Fonts (decisions 21, 40, 46): Han, kana, Hangul and Bopomofo glyphs come from the Han group's CJK
// family (the look's font table in theme.json "fonts", from tokens/tokens.json; a Latin UI language takes the first CJK language in
// Preferred languages, else the Region's, else SC), process-wide through Qt's application fallback
// families; Theme.font() also lists it after Inter (font()), so text re-lays out on a switch.
#pragma once

#include <QFont>
#include <QHash>
#include <QList>
#include <QMap>
#include <QMutex>
#include <QObject>
#include <QPointer>
#include <QStringList>
#include <QTranslator>

#include <memory>
#include <vector>

class QQmlEngine;
class QQuickItem;
class ThemeStore;

/// The one translator I18n installs: its catalogs in search order, then the pseudo-locale or the
/// English source's plural forms. Read by QCoreApplication::translate from any thread; changed only
/// while it is not installed.
class I18nTranslator : public QTranslator
{
public:
    QString translate(const char *context, const char *sourceText, const char *disambiguation, int n) const override;
    bool isEmpty() const override { return false; }

    std::vector<std::unique_ptr<QTranslator>> catalogs;   // first match wins
    QString pseudo;                                       // "", "qps-ploc" or "qps-ploca"
    bool recordSources = false;                           // the overflow audit: shown text → source

    QString sourceFor(const QString &shown) const;

private:
    mutable QMutex m_mutex;
    mutable QHash<QString, QString> m_sources;
};

class I18n : public QObject
{
    Q_OBJECT
    /// The UI language in effect: "de_DE", "zh_TW", "qps-ploc" ...
    Q_PROPERTY(QString language READ language NOTIFY languageChanged)
    /// What the system says (locale.conf or the environment), whatever this app's own choice.
    Q_PROPERTY(QString systemLanguage READ systemLanguage NOTIFY languageChanged)
    /// Where `language` came from: "override", "environment", "app", "system" or "default".
    Q_PROPERTY(QString languageSource READ languageSource NOTIFY languageChanged)
    /// A language set at run time (the greeter shows the selected user's): beats every other source,
    /// and a change switches live like a system-language change. "de", "ja_JP" ... as match() reads
    /// them; "" (or anything that isn't a SodiumOS language) goes back to the normal sources.
    Q_PROPERTY(QString overrideLanguage READ overrideLanguage WRITE setOverrideLanguage NOTIFY languageChanged)
    /// This app's id for its own language ("org.sodiumos.Calculator"); "" for the shell and greeter.
    Q_PROPERTY(QString appId READ appId NOTIFY languageChanged)
    /// "app", "shell", "greeter" or "other".
    Q_PROPERTY(QString role READ role NOTIFY languageChanged)
    /// zh, ja, ko or qps-ploca: no uppercase, no tracking, a CJK font after Inter.
    Q_PROPERTY(bool isCjk READ isCjk NOTIFY languageChanged)
    Q_PROPERTY(bool isPseudo READ isPseudo NOTIFY languageChanged)
    /// "latin", "zh-Hans", "zh-Hant", "ja" or "ko" (qps-ploca: "ja").
    Q_PROPERTY(QString scriptGroup READ scriptGroup NOTIFY languageChanged)
    /// The families that follow the UI font: the Han group's CJK family, then emoji.
    Q_PROPERTY(QStringList fontFamilies READ fontFamilies NOTIFY fontsChanged)
    /// The same for the mono font: the group's Noto Sans Mono CJK, then emoji.
    Q_PROPERTY(QStringList monoFontFamilies READ monoFontFamilies NOTIFY fontsChanged)
    /// The Han group in effect: "zh-Hans", "zh-Hant", "zh-Hant-HK", "ja" or "ko" (decision 46).
    Q_PROPERTY(QString hanGroup READ currentHanGroup NOTIFY fontsChanged)
    /// The catalogs this process asks for, and the .qm files loaded (in install order).
    Q_PROPERTY(QStringList catalogs READ catalogs NOTIFY catalogsChanged)
    Q_PROPERTY(QStringList loadedFiles READ loadedFiles NOTIFY catalogsChanged)
    Q_PROPERTY(QStringList catalogDirs READ catalogDirs NOTIFY catalogsChanged)
    /// The SodiumOS UI languages, and the pseudo-locales.
    Q_PROPERTY(QStringList languages READ languages CONSTANT)
    Q_PROPERTY(QStringList pseudoLanguages READ pseudoLanguages CONSTANT)
    /// $SODIUMOS_I18N_AUDIT=1: the overflow audit runs in this process.
    Q_PROPERTY(bool auditActive READ auditActive CONSTANT)
    /// Goes up on every switch.
    Q_PROPERTY(int revision READ revision NOTIFY languageChanged)

public:
    /// The process-wide instance. Call it in the GUI thread.
    static I18n *instance();

    /// From the plugin: its folder (the repo's build/i18n/qm is found from it).
    void setModuleDir(const QString &dir) { m_moduleDir = dir; }
    QString moduleDir() const { return m_moduleDir; }
    /// From the plugin, for every engine that imports SodiumUI: starts the runtime the first time,
    /// and keeps the engine's uiLanguage and translations current from then on.
    void attachEngine(QQmlEngine *engine);

    QString language() const { return m_language; }
    QString systemLanguage() const { return m_systemLanguage; }
    QString languageSource() const { return m_source; }
    QString overrideLanguage() const { return m_override; }
    void setOverrideLanguage(const QString &tag);
    QString appId() const { return m_appId; }
    QString role() const { return m_role; }
    bool isCjk() const { return scriptGroupOf(m_language) != QLatin1String("latin"); }
    bool isPseudo() const { return m_language.startsWith(QLatin1String("qps-")); }
    QString scriptGroup() const { return scriptGroupOf(m_language); }
    QStringList fontFamilies() const { return m_cjkUi; }
    QStringList monoFontFamilies() const { return m_cjkMono; }
    QString currentHanGroup() const { return m_fontGroup; }
    QStringList catalogs() const;
    QStringList loadedFiles() const { return m_loaded; }
    QStringList catalogDirs() const;
    QStringList languages() const;
    QStringList pseudoLanguages() const;
    bool auditActive() const;
    int revision() const { return m_revision; }
    /// The process's name for logs ("calculator", "shell").
    QString processName() const { return m_name; }

    /// Loads one more catalog by name ("filechooser") for the current language and every later one.
    /// True when a .qm file was found for it now. Missing catalogs are fine.
    Q_INVOKABLE bool loadCatalog(const QString &name);
    /// Reads every source again (tests: after changing the environment or the state folder).
    Q_INVOKABLE void reload();
    /// `base` (Theme.font's) with the Han group's CJK family and emoji after its own family, and the
    /// typographic line metrics; `palt` (Japanese single-line styles) and `numeric` (tabular digits)
    /// add those OpenType features.
    Q_INVOKABLE QFont font(const QFont &base, bool mono = false, bool palt = false, bool numeric = false) const;
    /// The audit: the source string a shown text came from ("" when unknown).
    QString sourceFor(const QString &shown) const;

    /// Decision 94's content chain for `language` (omitted: this app's): the language tags to look for in text other
    /// projects translate (AppStream, shared-mime-info), best first; the untagged source text comes after them. Tags
    /// compare case-insensitively with "_" as "-". Never across scripts, never a bare "zh", never a third language:
    ///     en_US []                     en_GB ["en-GB"]                en_AU ["en-AU", "en-GB"]
    ///     de_DE ["de-DE", "de"]        (es, fr, ja, ko the same)      zh_CN ["zh-Hans-CN", "zh-CN", "zh-Hans"]
    ///     zh_TW ["zh-Hant-TW", "zh-TW", "zh-Hant", "zh-Hant-HK", "zh-HK"]   (Region HK or MO: the HK pair first)
    ///     qps-ploc, qps-ploca []
    /// This app's chain is also the application property `sodiumosContentChain`, set before every revision bump, for
    /// C++ that doesn't link Sodium UI (decision 105: installed apps' names, ui/native/desktoptext.h).
    Q_INVOKABLE QStringList contentChain(const QString &language = QString()) const;
    /// The same for a language and a Region territory ("HK"), from C++.
    static QStringList contentChainFor(const QString &language, const QString &region);

    /// Decision 107: an installed app's desktop-entry text by desktop id (Quickshell's DesktopEntry.id), in this app's
    /// language through the content chain; never Quickshell's DesktopEntry.name, which follows the process's LANG and
    /// doesn't switch. Call from a binding that also reads I18n.revision. The plain Name (the shell never shows
    /// X-GNOME-FullName); `fallback`, else the id, when no applications folder has the entry.
    ///     text: { void I18n.revision; return AppNames.display(entry.id, I18n.appName(entry.id, entry.name)) }
    Q_INVOKABLE QString appName(const QString &desktopId, const QString &fallback = QString()) const;
    /// GenericName, Comment or X-GNOME-FullName the same way; `fallback` when the entry or the key is missing.
    Q_INVOKABLE QString desktopText(const QString &desktopId, const QString &key, const QString &fallback = QString()) const;
    /// Keywords for search: the first chain language's list whole, then the plain (English) list; [] when none.
    Q_INVOKABLE QStringList desktopKeywords(const QString &desktopId) const;
    /// A [Desktop Action] Name (jump lists); `fallback` when missing.
    Q_INVOKABLE QString desktopActionName(const QString &desktopId, const QString &action, const QString &fallback = QString()) const;

    /// Short wrapped labels (decisions 89 and 92), for WrappedLabel and anyone drawing one of their own; what Qt does
    /// with U+200B, U+2060, Hangul and U+00AD is in textbreaks.h. The narrowest width (≤ `width`) that keeps the line
    /// count `text` has at `width` in `font`, never breaking a word the full width doesn't break: lay the lines out in
    /// it to balance them. `width` when the text fits on one line.
    Q_INVOKABLE qreal balancedWidth(const QString &text, const QFont &font, qreal width) const;
    /// The text held together inside its phrases when a zh or ja translator marked them with U+200B (word joiners
    /// between the characters): it then breaks only at the marks, or anywhere in a phrase wider than the line.
    Q_INVOKABLE QString joinPhrases(const QString &text) const;
    /// The text without soft hyphens (U+00AD), phrase marks (U+200B) and the word joiners joinPhrases adds: for every
    /// use of a translated string that isn't the wrapped label itself (accessible names, tooltips, search indexes and
    /// matching, sort keys, copying, generated files). C++: TextBreaks::stripBreakMarks.
    Q_INVOKABLE QString stripBreakMarks(const QString &text) const;
    /// What the overflow audit says about where a wrapped Text breaks, in this process's language, with the audit
    /// on or off: "" or "mid-word break: \"Energiemo|dus\"" (decision 92), "one-character last line: \"ン\"" (zh, ja,
    /// short labels; decision 89). For tests and sweeps: `I18n.lineBreakFault(label.textItem)`.
    Q_INVOKABLE QString lineBreakFault(QQuickItem *text) const;

    /// Decision 22's names. A language's name in this process's UI language: "de_DE" (or "de-DE", "de") → "German"
    /// (in a German UI "Deutsch"), "pt_BR" → "Portuguese (Brazil)". The ten SodiumOS languages by their catalog names
    /// (context LanguageNames: "English (UK)", "Chinese, Simplified"; ICU's while no catalog has them), others by ICU
    /// (the territory only where it isn't the language's main one). C++: languageNameIn().
    Q_INVOKABLE QString languageName(const QString &tag) const;
    /// A language's own name: "Deutsch", "日本語", "简体中文", "português (Brasil)" (ICU for others).
    Q_INVOKABLE QString languageEndonym(const QString &tag) const;
    static QString languageNameIn(const QString &tag, const QString &uiLanguage);
    static QString languageEndonymOf(const QString &tag);

    /// "de-AT", "de_DE.UTF-8", "de" → "de_DE"; "" for anything that isn't a SodiumOS language.
    static QString match(const QString &tag);
    /// The languages whose catalogs a language loads, in install order (the last one is searched first).
    static QStringList chainFor(const QString &language);
    static QString scriptGroupOf(const QString &language);
    /// Decision 106: the Preferred languages of a locale.conf, SODIUMOS_LANGUAGES, else its LANGUAGE.
    static QStringList preferredFrom(const QMap<QString, QString> &conf);

signals:
    void languageChanged();
    void catalogsChanged();
    void fontsChanged();

private:
    explicit I18n(QObject *parent = nullptr);
    void start();
    void identify();
    void ensureStore();
    void resolve();
    void apply();
    void rebuildTranslator();
    void applyFonts();
    /// Sets the application property sodiumosContentChain (decision 105); true when it changed.
    bool publishChain();
    void syncHanOrder();
    void retranslateEngines();
    QString findCatalog(const QString &name, const QString &language) const;
    QString systemLanguageFrom(QString *source) const;
    QString appLanguage() const;
    QString hanGroup() const;

    QString m_moduleDir;
    bool m_started = false;
    bool m_resolving = false;
    bool m_installed = false;
    bool m_reapply = false;
    QString m_language;
    QString m_systemLanguage;
    QString m_source;
    QString m_override;
    QString m_appId;
    QString m_role;
    QString m_name;
    QStringList m_baseCatalogs;
    QStringList m_extraCatalogs;
    QStringList m_loaded;
    QString m_fontGroup;
    QStringList m_cjkUi;
    QStringList m_cjkMono;
    QStringList m_latin;
    QStringList m_cjk;
    QStringList m_monoCjk;
    QString m_emoji;
    int m_revision = 0;
    std::unique_ptr<I18nTranslator> m_translator;
    ThemeStore *m_store = nullptr;
    QString m_storeDir;
    QList<QPointer<QQmlEngine>> m_engines;
};
