// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "winapp.h"

#include "hostcontroller.h"
#include "brand.h"
#include "tray.h"

#include <QCoreApplication>
#include <QDir>
#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QQmlComponent>
#include <QQmlContext>
#include <QQuickWindow>
#include <QScreen>
#include <QTimer>

#include <windows.h>

WinApp::WinApp(HostController *host, QQmlApplicationEngine *engine, QObject *parent)
    : QObject(parent), m_host(host), m_engine(engine)
{
}

WinApp::~WinApp()
{
    delete m_tray;
}

bool WinApp::otherInstance(const QString &link)
{
    // One per user session: the mutex, then a nudge to the first one's tray window.
    static HANDLE mutex = CreateMutexW(nullptr, TRUE, L"Local\\SodiumRemote.Instance");
    if (GetLastError() != ERROR_ALREADY_EXISTS)
        return false;
    return Tray::pingRunning(link) || true;
}

void WinApp::registerLinkScheme()
{
    // HKCU\Software\Classes\sodium-remote: "URL Protocol", and shell\open\command naming this program, "--" (no more options) and the link ("%1").
    const QString exe = QDir::toNativeSeparators(QCoreApplication::applicationFilePath());
    const std::wstring scheme = QString::fromLatin1(brand::LinkScheme).toStdWString();
    const std::wstring root = L"Software\\Classes\\" + scheme;
    auto set = [](const std::wstring &key, const wchar_t *name, const std::wstring &value) {
        RegSetKeyValueW(HKEY_CURRENT_USER, key.c_str(), name, REG_SZ, value.c_str(), DWORD((value.size() + 1) * sizeof(wchar_t)));
    };
    set(root, nullptr, L"URL:Sodium Remote server link");
    set(root, L"URL Protocol", L"");
    set(root + L"\\shell\\open\\command", nullptr, L"\"" + exe.toStdWString() + L"\" -- \"%1\"");
}

void WinApp::setTab(int t)
{
    if (m_tab == t)
        return;
    m_tab = t;
    Q_EMIT tabChanged();
}

QQuickWindow *WinApp::load(const QString &type)
{
    QQmlComponent c(m_engine, QStringLiteral("remote"), type);
    QObject *o = c.create(m_engine->rootContext());
    if (!o) {
        qWarning("Sodium Remote: %s", qPrintable(c.errorString()));
        return nullptr;
    }
    return qobject_cast<QQuickWindow *>(o);
}

void WinApp::start(bool background)
{
    const auto roots = m_engine->rootObjects();
    for (QObject *o : roots)
        if (auto *w = qobject_cast<QQuickWindow *>(o)) {
            m_main = w;
            break;
        }
    m_tray = new Tray(this);
    // The tray menu and the notice bind to Host: they go before it does (else every binding reads null).
    connect(QCoreApplication::instance(), &QCoreApplication::aboutToQuit, this, [this] {
        delete m_menu.data();
        delete m_notice.data();
    });
    connect(m_tray, &Tray::activated, this, [this](const QPoint &at, bool context) {
        if (context)
            showTrayMenu(at);
        else
            openMain();
    });
    connect(m_tray, &Tray::raiseRequested, this, [this] { openMain(); });
    connect(m_tray, &Tray::linkReceived, this, [this](const QString &link) {
        openMain(0);
        Q_EMIT serverLinkReceived(link);
    });
    registerLinkScheme();
    connect(m_host, &HostController::stateChanged, this, &WinApp::updateTray);
    connect(m_host, &HostController::controlChanged, this, &WinApp::updateTray);
    connect(m_host, &HostController::controlStarted, this, &WinApp::showNotice);
    // The window's close button leaves the app in the tray while "Allow remote control" is on; otherwise it quits.
    QGuiApplication::setQuitOnLastWindowClosed(false);
    if (m_main)
        connect(m_main, &QQuickWindow::closing, this, [this] {
            QTimer::singleShot(0, this, [this] {
                if (!m_host->enabled())
                    QCoreApplication::quit();
            });
        });
    updateTray();
    if (background && m_host->enabled() && m_main)
        m_main->hide();
}

void WinApp::updateTray()
{
    if (!m_tray)
        return;
    const QString brand = QStringLiteral("Sodium Remote");
    if (m_host->controlled()) {
        m_tray->setState(QStringLiteral("controlled"));
        m_tray->setToolTip(tr("%1: %2 is controlling this computer").arg(brand, m_host->controller().value(QStringLiteral("name")).toString()));
    } else if (m_host->state() == QLatin1String("on")) {
        m_tray->setState(QStringLiteral("on"));
        m_tray->setToolTip(tr("%1: Remote control is on").arg(brand));
    } else {
        m_tray->setState(QStringLiteral("off"));
        m_tray->setToolTip(tr("%1: Remote control is off").arg(brand));
    }
}

void WinApp::openMain(int tab)
{
    if (tab == 0 || tab == 1)
        setTab(tab);
    if (!m_main)
        return;
    m_main->show();
    if (m_main->windowStates() & Qt::WindowMinimized)
        m_main->showNormal();
    m_main->raise();
    m_main->requestActivate();
    // A second launch (or the tray) must be able to take the foreground.
    SetForegroundWindow(reinterpret_cast<HWND>(m_main->winId()));
}

void WinApp::pairDevice()
{
    openMain(1);
    m_host->openPairing();
}

void WinApp::quit()
{
    // Hosting stops when the app ends (the controller's destructor shuts the session down, the matched display included).
    // The "Allow remote control" choice is the user's and stays saved: the next start hosts again.
    // Queued: the tray menu's Quit item calls this from its own signal handler, and quitting deletes the menu (aboutToQuit),
    // which Qt answers with a fatal error while the handler runs (an APPCRASH on every tray Quit, the matched display left up).
    QMetaObject::invokeMethod(QCoreApplication::instance(), &QCoreApplication::quit, Qt::QueuedConnection);
}

void WinApp::showTrayMenu(const QPoint &anchor)
{
    if (!m_menu)
        m_menu = load(QStringLiteral("TrayMenu"));
    if (!m_menu)
        return;
    // The icon's rectangle (else the click) says which screen and, against that screen's work area, which edge the
    // taskbar is on (TrayMenu.qml).
    const QRect icon = m_tray->geometry();
    const QPoint at = icon.isEmpty() ? anchor : icon.center();
    QScreen *s = QGuiApplication::screenAt(at);
    if (!s)
        s = QGuiApplication::primaryScreen();
    const qreal k = s->devicePixelRatio();
    const QRect work = s->availableGeometry();
    QMetaObject::invokeMethod(m_menu, "popupAt", Q_ARG(QVariant, at.x() / k), Q_ARG(QVariant, at.y() / k), Q_ARG(QVariant, work.x()),
                              Q_ARG(QVariant, work.y()), Q_ARG(QVariant, work.width()), Q_ARG(QVariant, work.height()));
}

void WinApp::showNotice(const QVariantMap &controller)
{
    if (!m_notice)
        m_notice = load(QStringLiteral("ControlledNotice"));
    if (!m_notice)
        return;
    QMetaObject::invokeMethod(m_notice, "announce", Q_ARG(QVariant, controller.value(QStringLiteral("name")).toString()),
                              Q_ARG(QVariant, controller.value(QStringLiteral("since")).toLongLong()));
    // Topmost, non-activating, a tool window (no taskbar entry, no Alt+Tab entry): set on the HWND itself, so it never
    // takes the focus however Qt shows it).
    if (HWND h = reinterpret_cast<HWND>(m_notice->winId())) {
        const LONG_PTR ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        SetWindowLongPtrW(h, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW);
        SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}
