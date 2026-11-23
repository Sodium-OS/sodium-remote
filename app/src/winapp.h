// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// Sodium Remote on Windows, the parts that are not the shared client: single instance, the tray and its menu, the
// "being controlled" notice and the host ("This computer"). Exposed to QML as the context property `Win`.
#pragma once
#include <QObject>
#include <QPointer>

class HostController;
class QQmlApplicationEngine;
class QQuickWindow;
class Tray;

class WinApp : public QObject
{
    Q_OBJECT
    Q_PROPERTY(HostController *host READ host CONSTANT)
    /// The tab the main window shows: 0 Computers, 1 This computer.
    Q_PROPERTY(int tab READ tab WRITE setTab NOTIFY tabChanged)

public:
    WinApp(HostController *host, QQmlApplicationEngine *engine, QObject *parent = nullptr);
    ~WinApp() override;

    HostController *host() const { return m_host; }
    int tab() const { return m_tab; }
    void setTab(int t);

    /// Called once the QML is loaded: finds the main window and creates the tray.
    void start(bool background);

    /// True when another instance runs (it was asked to show itself, and to confirm `link` when there is one).
    static bool otherInstance(const QString &link = QString());
    /// Makes sodium-remote:// links open this program: HKCU\\Software\\Classes\\sodium-remote (per user, no administrator).
    static void registerLinkScheme();

public Q_SLOTS:
    /// Shows the main window, on `tab` when it is 0 or 1 (else leaves it).
    void openMain(int tab = -1);
    void quit();
    /// The tray menu's "Pair a device…": the page, with a code open.
    void pairDevice();

Q_SIGNALS:
    void tabChanged();
    /// A server link reached this instance (a second launch's argument): Settings › Server confirms it.
    void serverLinkReceived(const QString &link);

private:
    void updateTray();
    void showTrayMenu(const QPoint &anchor);
    void showNotice(const QVariantMap &controller);
    QQuickWindow *load(const QString &type);

    HostController *m_host;
    QQmlApplicationEngine *m_engine;
    Tray *m_tray = nullptr;
    QPointer<QQuickWindow> m_main, m_menu, m_notice;
    int m_tab = 0;
};
