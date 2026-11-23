// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// Sodium Remote's app (org.sodiumos.Remote; binary sodium-remote, SodiumRemote.exe on Windows): the client for other computers
//
//   sodium-remote [--data DIR] [--lan-only] [--script FILE] [--version]
//
// --data DIR     this device's key, paired.json and app.json (default $SODIUM_REMOTE_DATA, else the engine's own default:
//                ~/.local/share/sodium-remote/client, %LOCALAPPDATA%\SodiumRemote\client; the pre-rename folders are
//                copied on first start). Tests always pass their own.
// --lan-only     don't use a server (the nested test bed).
// --script FILE  screenshots and tests only (needs SODIUM_REMOTE_TEST=1): one step per line, "<ms> <command> [argument]", run by
//                Main.qml's runStep.
// A link (sodium-remote:...) on the command line is the only thing used; every option beside it is ignored.
// --version      the version, the license and where the source is (AGPL-3.0 section 13), then exit.
#include "brand.h"
#include "controller.h"
#ifdef Q_OS_WIN
#include "hostcontroller.h"
#include "winapp.h"
#else
#include "linkhandoff.h"
#ifdef REMOTE_LINUX_HOST
#include "hostcontroller.h"
#endif
#endif

#include <QCommandLineParser>
#include <QCoreApplication>
#include <QDir>
#include <QFile>
#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QQuickWindow>
#include <QWindow>
#include <QFileInfo>
#include <QIcon>
#include <QStandardPaths>

namespace {

// The portable and installed layouts look for their files beside the program: <exe>/<x> (the zip, a build directory) and
// <exe>/../share/sodium-remote/<x> (a package).
QStringList appDirs(const QString &sub)
{
    const QString exe = QCoreApplication::applicationDirPath();
    return {exe + QLatin1Char('/') + sub,
            exe + QStringLiteral("/../share/") + QString::fromLatin1(brand::LinuxDirName) + QLatin1Char('/') + sub};
}

// The app carries its own symbolic icons in its resources (<repo>/app/data/icons/sodium-remote), listed as a fallback icon
// search path: Sodium UI's icon provider looks there after the system's SodiumOS theme, where there is one.
void addOwnIcons()
{
    QIcon::setFallbackSearchPaths(QIcon::fallbackSearchPaths() << QStringLiteral(":/icons/sodium-remote"));
}

// Sodium UI's I18n finds the .qm files (remote_<lang>.qm, sodiumui_<lang>.qm) in SODIUMOS_I18N_DIR; left alone it looks in
// a SodiumOS checkout and /usr/share/sodiumos/translations. Here: translations/ beside the program, the package's folder,
// then the build directory's (i18n/i18n.sh release).
void pointI18nAtOwnCatalogs()
{
    if (qEnvironmentVariableIsSet("SODIUMOS_I18N_DIR"))
        return;
    QStringList dirs = appDirs(QStringLiteral("translations"));
    dirs << QStringLiteral(REMOTE_SOURCE_DIR "/build/i18n/qm");
    QStringList found;
    for (const QString &d : dirs)
        if (!QDir(d).entryList({QStringLiteral("remote_*.qm")}, QDir::Files).isEmpty())
            found << QDir::cleanPath(d);
    if (!found.isEmpty())
        qputenv("SODIUMOS_I18N_DIR", found.join(QDir::listSeparator()).toUtf8());
}


// The engine's own default (it honours $SODIUM_REMOTE_DATA, copies the pre-rename folder on first start); "" when unknown.
QString engineDefaultDataDir()
{
    char *d = rq_default_data_dir();
    const QString dir = QString::fromUtf8(d);
    rq_free_string(d);
    return dir;
}


// A server link given on the command line (the desktop entry's %u, Windows' "%1"): the first argument that is one.
QString linkArgument(int argc, char *argv[])
{
    const QString scheme = QString::fromLatin1(brand::LinkScheme) + QLatin1Char(':');
    for (int i = 1; i < argc; ++i) {
        const QString a = QString::fromLocal8Bit(argv[i]);
        if (a.startsWith(scheme, Qt::CaseInsensitive))
            return a;
    }
    return {};
}

}

int main(int argc, char *argv[])
{
    QGuiApplication app(argc, argv);
    app.setApplicationName(QString::fromLatin1(brand::AppBinaryName));
    app.setApplicationVersion(QStringLiteral(REMOTE_VERSION));
    app.setDesktopFileName(QStringLiteral(REMOTE_APP_ID));
    // The first thing a person may run: the license and the source (AGPL-3.0 section 13), before any window or engine.
    for (int i = 1; i < argc; ++i)
        if (qstrcmp(argv[i], "--version") == 0 || qstrcmp(argv[i], "-v") == 0) {
#ifdef Q_OS_WIN
            const char *bin = brand::BinAppWindows;
#else
            const char *bin = brand::BinApp;
#endif
            printf("%s %s\n%s, licensed %s.\nSource code: %s\n", bin, REMOTE_VERSION, brand::ProductName, brand::License, brand::SourceUrl);
            return 0;
        }
    addOwnIcons();
    pointI18nAtOwnCatalogs();
#ifdef Q_OS_WIN
    // "Sodium Remote", untranslated on every OS; one instance per user session (a second launch shows the first).
    app.setApplicationDisplayName(QString::fromLatin1(brand::ProductName));
    app.setOrganizationName(QString::fromLatin1(brand::WindowsDirName));
    if (WinApp::otherInstance(linkArgument(argc, argv)))
        return 0;
#endif
    // The window glass and the transparent hole over the video need an alpha buffer.
    QQuickWindow::setDefaultAlphaBuffer(true);

    QCommandLineParser p;
    QCommandLineOption dataOpt(QStringLiteral("data"), QStringLiteral("Data directory"), QStringLiteral("DIR"));
    QCommandLineOption lanOpt(QStringLiteral("lan-only"), QStringLiteral("Don't use the rendezvous server"));
    QCommandLineOption scriptOpt(QStringLiteral("script"), QStringLiteral("Test script"), QStringLiteral("FILE"));
    QCommandLineOption bgOpt(QStringLiteral("background"), QStringLiteral("Start in the tray (Start at login)"));
    p.addOptions({dataOpt, lanOpt, scriptOpt, bgOpt});
    p.addPositionalArgument(QStringLiteral("link"), QStringLiteral("A sodium-remote://server link to confirm"), QStringLiteral("[link]"));
    // A link is never a launch with options: when one is present (the handler's "%1"), everything else on the command line is
    // dropped, so a link that smuggles in "--data" or "--script" has nothing to run. Anything after "--" is positional.
    const QString link = linkArgument(argc, argv);
    QStringList args = app.arguments();
    if (!link.isEmpty())
        args = QStringList{args.value(0), QStringLiteral("--"), link};
    p.process(args);

    QString data = p.value(dataOpt);
#ifdef Q_OS_WIN
    // Two stores that never share a file (core's FileStore keeps device.key and paired.json in each):
    // this device as a client in <base>\client, this computer as a host in <base>\host. <base> is --data, or the folder
    // the engine's default client folder sits in (%LOCALAPPDATA%\SodiumRemote).
    QString base = data;
    if (base.isEmpty()) {
        const QString client = engineDefaultDataDir();
        base = client.isEmpty() ? qEnvironmentVariable("LOCALAPPDATA") + QLatin1Char('/') + QString::fromLatin1(brand::WindowsDirName)
                                : QFileInfo(client).absolutePath();
        data = client.isEmpty() ? base + QStringLiteral("/client") : client;
    } else {
        data = base + QStringLiteral("/client");
    }
    const QString hostDir = base + QStringLiteral("/host");
#else
    if (data.isEmpty())
        data = engineDefaultDataDir();
#ifdef REMOTE_LINUX_HOST
    // As on Windows, this device as a client and this computer as a host keep separate stores: <base>/client, <base>/host.
    // <base> is --data, else the folder the engine's default client folder sits in (~/.local/share/sodium-remote).
    QString hostDir;
    {
        char *d = rh_default_data_dir();
        hostDir = QString::fromUtf8(d);
        rq_free_string(d);
        if (!p.value(dataOpt).isEmpty())
            hostDir = p.value(dataOpt) + QStringLiteral("/host");
        else if (hostDir.isEmpty())
            hostDir = QFileInfo(data).absolutePath() + QStringLiteral("/host");
    }
#endif
#endif

#ifndef Q_OS_WIN
    // A second launch with a link hands it to the running app (Settings › Server shows it) and ends.
    if (!link.isEmpty() && LinkHandoff::forward(data, link))
        return 0;
#endif
    RemoteController controller(data, !p.isSet(lanOpt));
    if (!link.isEmpty())
        controller.openServerLink(link);
    QString script;
    // --script injects commands (allow hosting, files, servers): tests and screenshots only, so it needs SODIUM_REMOTE_TEST=1.
    if (p.isSet(scriptOpt) && qEnvironmentVariable("SODIUM_REMOTE_TEST") == QLatin1String("1")) {
        QFile f(p.value(scriptOpt));
        if (f.open(QIODevice::ReadOnly))
            script = QString::fromUtf8(f.readAll());
    }

    QQmlApplicationEngine engine;
#ifdef Q_OS_WIN
    // The package: SodiumUI/ and qml/ next to the program.
    engine.addImportPath(QCoreApplication::applicationDirPath());
    engine.addImportPath(QCoreApplication::applicationDirPath() + QStringLiteral("/qml"));
    HostController hostController(hostDir);
    WinApp win(&hostController, &engine);
    engine.rootContext()->setContextProperty(QStringLiteral("Win"), &win);
    engine.rootContext()->setContextProperty(QStringLiteral("Host"), &hostController);
#else
    engine.rootContext()->setContextProperty(QStringLiteral("Win"), nullptr); // Windows only: the tray, the notice, the tab
#ifdef REMOTE_LINUX_HOST
    HostController hostController(hostDir);
    engine.rootContext()->setContextProperty(QStringLiteral("Host"), &hostController);
#else
    engine.rootContext()->setContextProperty(QStringLiteral("Host"), nullptr);
#endif
#endif
#ifndef Q_OS_WIN
    // Sodium UI (the SodiumUI/ module): beside the program, in a package, or where the build put it.
    engine.addImportPath(QCoreApplication::applicationDirPath());
    engine.addImportPath(QCoreApplication::applicationDirPath() + QStringLiteral("/../lib/") + QString::fromLatin1(brand::LinuxDirName));
    engine.addImportPath(QStringLiteral(SODIUMUI_IMPORT_DIR));
#endif
    engine.rootContext()->setContextProperty(QStringLiteral("Remote"), &controller);
    engine.rootContext()->setContextProperty(QStringLiteral("testScript"), script);
    engine.loadFromModule(QStringLiteral("remote"), QStringLiteral("Main"));
    if (engine.rootObjects().isEmpty())
        return 1;
#ifdef Q_OS_WIN
    win.start(p.isSet(bgOpt));
    QObject::connect(&win, &WinApp::serverLinkReceived, &controller, &RemoteController::openServerLink);
    // "This computer" uses the same server: restart it after a change (when nobody is controlling; else after the session).
    QObject::connect(&controller, &RemoteController::serverSaved, &hostController, &HostController::restartForServer);
#else
    LinkHandoff handoff(data);
    QObject::connect(&handoff, &LinkHandoff::received, &controller, [&](const QString &l) {
        if (!l.isEmpty())
            controller.openServerLink(l);
        if (auto *w = qobject_cast<QWindow *>(engine.rootObjects().value(0))) {
            w->show();
            w->raise();
            w->requestActivate();
        }
    });
#endif
    return app.exec();
}
