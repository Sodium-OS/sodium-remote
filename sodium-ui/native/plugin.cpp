// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// The native half of the SodiumUI QML module that Sodium Remote's app uses (derived from ui/native/plugin.cpp; MANIFEST).
// Registers ThemeStore, the image://sodiumicon provider, SodiumwmWindow (WindowControls), WindowChrome (Windows),
// LocaleFormats / Format, the language runtime I18n, the I18nAudit attached property and Match. Everything else in
// SodiumUI is plain QML next to the qmldir. Always a portable build: no D-Bus, Wayland, xkbcommon, librsvg or GLib.
#include "i18n.h"
#include "i18naudit.h"
#include "iconprovider.h"
#include "localeformats.h"
#include "modulepath.h"
#include "sodiumwmwindow.h"
#include "textmatch.h"
#include "themestore.h"
#ifdef Q_OS_WIN
#  include "portable/windowchrome.h"
#endif

#include <QCoreApplication>
#include <QDir>
#include <QDirIterator>
#include <QFileInfo>
#include <QFontDatabase>
#include <QIcon>
#include <QQmlEngine>
#include <QQmlExtensionPlugin>

static QString moduleDir()
{
    return sodiumuiModuleDir();
}

// Off SodiumOS there is no fontconfig with our fonts (Inter, JetBrains Mono, Noto Sans CJK; all OFL): a package carries
// them in <module>/fonts, or <module>/../fonts, and they are loaded once, before any text is laid out.
static void loadBundledFonts(const QString &moduleDir)
{
    static bool done = false;
    if (done || moduleDir.isEmpty())
        return;
    done = true;
    for (const QString &dir : {moduleDir + QStringLiteral("/fonts"), moduleDir + QStringLiteral("/../fonts")}) {
        QDirIterator it(QDir::cleanPath(dir), {QStringLiteral("*.ttf"), QStringLiteral("*.otf"), QStringLiteral("*.ttc")},
                        QDir::Files);
        while (it.hasNext())
            if (QFontDatabase::addApplicationFont(it.next()) < 0)
                qWarning("SodiumUI: cannot load the font %s", qPrintable(it.filePath()));
    }
}

class SodiumUIPlugin : public QQmlExtensionPlugin
{
    Q_OBJECT
    Q_PLUGIN_METADATA(IID QQmlExtensionInterface_iid)

public:
    void registerTypes(const char *uri) override
    {
        const QString defaults = moduleDir() + QStringLiteral("/theme.default.json");
        qmlRegisterSingletonType<ThemeStore>(uri, 1, 0, "ThemeStore",
            [defaults](QQmlEngine *, QJSEngine *) -> QObject * { return new ThemeStore(defaults); });
        // sodiumwm for the title bar: the maximised state, maximise, the snap grid (inert without sodiumwm).
        qmlRegisterType<SodiumwmWindow>(uri, 1, 0, "SodiumwmWindow");
#ifdef Q_OS_WIN
        // Windows: the frame, the DWM backdrop and the snap-layout button (AppWindow, WindowControls).
        qmlRegisterType<WindowChrome>(uri, 1, 0, "WindowChrome");
#endif
        // Dates and numbers (docs/translating.md); `LocaleFormats` stays for the QML that still names it.
        const auto formats = [](QQmlEngine *, QJSEngine *) -> QObject * {
            LocaleFormats *formats = LocaleFormats::instance();
            QJSEngine::setObjectOwnership(formats, QJSEngine::CppOwnership);
            return formats;
        };
        qmlRegisterSingletonType<LocaleFormats>(uri, 1, 0, "LocaleFormats", formats);
        qmlRegisterSingletonType<LocaleFormats>(uri, 1, 0, "Format", formats);
        // The language runtime (i18n.h): one per process, started by initializeEngine below.
        qmlRegisterSingletonType<I18n>(uri, 1, 0, "I18n", [](QQmlEngine *, QJSEngine *) -> QObject * {
            I18n *i18n = I18n::instance();
            QJSEngine::setObjectOwnership(i18n, QJSEngine::CppOwnership);
            return i18n;
        });
        qmlRegisterSingletonType<TextMatch>(uri, 1, 0, "Match", [](QQmlEngine *, QJSEngine *) -> QObject * {
            TextMatch *match = TextMatch::instance();
            QJSEngine::setObjectOwnership(match, QJSEngine::CppOwnership);
            return match;
        });
        qmlRegisterUncreatableType<I18nAudit>(uri, 1, 0, "I18nAudit",
                                             QStringLiteral("I18nAudit is an attached property: I18nAudit.userContent, I18nAudit.mayElide"));
    }

    void initializeEngine(QQmlEngine *engine, const char *) override
    {
        if (engine->thread() == qApp->thread()) {
            loadBundledFonts(moduleDir());
            // The language runtime, before any of this engine's qsTr runs.
            I18n::instance()->setModuleDir(moduleDir());
            I18n::instance()->attachEngine(engine);
        }
        // Icons we don't draw ourselves come from the app's own copy of the theme (<data>/icons/SodiumOS).
        if (QIcon::themeName() != QLatin1String("SodiumOS"))
            QIcon::setFallbackThemeName(QStringLiteral("SodiumOS"));
        if (!engine->imageProvider(QStringLiteral("sodiumicon")))
            engine->addImageProvider(QStringLiteral("sodiumicon"), new IconProvider);
    }
};

#include "plugin.moc"
