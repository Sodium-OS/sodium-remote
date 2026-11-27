// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "hostcontroller.h"

#include "brand.h"
#include "firewallrules.h"
#include "remote_qt.h"

#include <QClipboard>
#include <QCoreApplication>
#include <QDesktopServices>
#include <QDir>
#include <QFile>
#include <QGuiApplication>
#include <QHostAddress>
#include <QHostInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QNetworkInterface>
#include <QProcess>
#include <QSettings>
#include <QUrl>

#include <algorithm>

#ifdef Q_OS_WIN
#include <windows.h>
#include <shellapi.h>
#include <netfw.h>
#include <oleauto.h>
#include <QThread>
#else
#include <QDBusConnection>
#include <QDBusInterface>
#include <QDBusReply>
#endif

#ifdef Q_OS_WIN
namespace {
const QString kRunKey = QStringLiteral("HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const QString kRunName = QStringLiteral("SodiumRemote");
const QString kRule = QStringLiteral("Sodium Remote");

// The firewall's rules through INetFwPolicy2. Hundreds of rules are common, so this runs on a worker thread of its own.
QList<FirewallRule> readFirewallRules(bool *ok)
{
    *ok = false;
    QList<FirewallRule> out;
    const HRESULT init = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    INetFwPolicy2 *policy = nullptr;
    INetFwRules *rules = nullptr;
    IUnknown *unknown = nullptr;
    IEnumVARIANT *items = nullptr;
    if (SUCCEEDED(CoCreateInstance(__uuidof(NetFwPolicy2), nullptr, CLSCTX_INPROC_SERVER, __uuidof(INetFwPolicy2), reinterpret_cast<void **>(&policy)))
        && SUCCEEDED(policy->get_Rules(&rules)) && SUCCEEDED(rules->get__NewEnum(&unknown)) && unknown
        && SUCCEEDED(unknown->QueryInterface(__uuidof(IEnumVARIANT), reinterpret_cast<void **>(&items)))) {
        *ok = true;
        VARIANT item;
        ULONG got = 0;
        VariantInit(&item);
        while (items->Next(1, &item, &got) == S_OK && got == 1) {
            INetFwRule *rule = nullptr;
            if (item.vt == VT_DISPATCH && item.pdispVal
                && SUCCEEDED(item.pdispVal->QueryInterface(__uuidof(INetFwRule), reinterpret_cast<void **>(&rule)))) {
                FirewallRule r;
                BSTR app = nullptr;
                VARIANT_BOOL enabled = VARIANT_FALSE;
                NET_FW_RULE_DIRECTION dir = NET_FW_RULE_DIR_OUT;
                NET_FW_ACTION action = NET_FW_ACTION_BLOCK;
                long profiles = 0;
                if (SUCCEEDED(rule->get_ApplicationName(&app)) && app)
                    r.application = QString::fromWCharArray(app, int(SysStringLen(app)));
                SysFreeString(app);
                rule->get_Enabled(&enabled);
                rule->get_Direction(&dir);
                rule->get_Action(&action);
                rule->get_Profiles(&profiles);
                r.enabled = enabled != VARIANT_FALSE;
                r.inbound = dir == NET_FW_RULE_DIR_IN;
                r.allow = action == NET_FW_ACTION_ALLOW;
                r.profiles = int(profiles);
                if (!r.application.isEmpty())
                    out.append(r);
                rule->Release();
            }
            VariantClear(&item);
        }
    }
    if (items)
        items->Release();
    if (unknown)
        unknown->Release();
    if (rules)
        rules->Release();
    if (policy)
        policy->Release();
    if (SUCCEEDED(init))
        CoUninitialize();
    return out;
}
} // namespace
#endif

HostController::HostController(const QString &dataDir, QObject *parent)
    : QObject(parent), m_dataDir(dataDir)
{
    QDir().mkpath(m_dataDir);
    loadApp();
    m_rh = rh_new(m_dataDir.toUtf8().constData(), &HostController::trampoline, this);
    refreshPaired();
    loadConsent();
    connect(&m_tick, &QTimer::timeout, this, [this] {
        if (m_pairing.value(QStringLiteral("state")).toString() == QLatin1String("open")) {
            if (pairingSecondsLeft() <= 0 && m_pairing.value(QStringLiteral("state")).toString() == QLatin1String("open")) {
                m_pairing = {{QStringLiteral("state"), QStringLiteral("closed")}, {QStringLiteral("reason"), QStringLiteral("expired")}};
                Q_EMIT pairingChanged();
            }
            Q_EMIT tick();
        }
    });
    m_tick.start(1000);
    connect(&m_netTimer, &QTimer::timeout, this, [this] { pollFirewall(); });
    m_netTimer.start(20000);
    pollFirewall();
    // Linux without a portal that can share the screen: the switch stays off (the page says why).
    if (m_consent == QLatin1String("unavailable"))
        m_enabled = false;
    // The switch was on when the app last ran: it is on again (the point of "Start at login").
    if (m_enabled) {
        m_state = QStringLiteral("starting");
        rh_enable(m_rh, 1, m_name.toUtf8().constData());
    }
}

HostController::~HostController()
{
    if (m_rh)
        rh_free(m_rh);
}

QString HostController::defaultName() const
{
    return QHostInfo::localHostName();
}

void HostController::loadApp()
{
    QFile f(m_dataDir + QStringLiteral("/host-app.json"));
    if (!f.open(QIODevice::ReadOnly))
        return;
    const QJsonObject o = QJsonDocument::fromJson(f.readAll()).object();
    m_enabled = o.value(QStringLiteral("enabled")).toBool(false);
    m_name = o.value(QStringLiteral("name")).toString();
}

void HostController::saveApp()
{
    QFile f(m_dataDir + QStringLiteral("/host-app.json"));
    if (f.open(QIODevice::WriteOnly | QIODevice::Truncate))
        f.write(QJsonDocument(QJsonObject{{QStringLiteral("enabled"), m_enabled}, {QStringLiteral("name"), m_name}}).toJson());
}

bool HostController::startAtLogin() const
{
#ifdef Q_OS_WIN
    return QSettings(kRunKey, QSettings::NativeFormat).contains(kRunName);
#else
    return false; // no start at sign-in on Linux yet (the page hides the row)
#endif
}

void HostController::setStartAtLogin(bool on)
{
#ifdef Q_OS_WIN
    QSettings s(kRunKey, QSettings::NativeFormat);
    if (on)
        s.setValue(kRunName, QStringLiteral("\"%1\" --background").arg(QDir::toNativeSeparators(QCoreApplication::applicationFilePath())));
    else
        s.remove(kRunName);
    s.sync();
    Q_EMIT loginChanged();
#else
    Q_UNUSED(on)
#endif
}

int HostController::pairingSecondsLeft() const
{
    if (!m_pairingExpires.isValid())
        return 0;
    return int(qMax<qint64>(0, QDateTime::currentDateTime().secsTo(m_pairingExpires)));
}

void HostController::trampoline(void *user, const char *json)
{
    auto *self = static_cast<HostController *>(user);
    const QByteArray text(json);
    QMetaObject::invokeMethod(self, [self, text] { self->handleEvent(QJsonDocument::fromJson(text).object()); }, Qt::QueuedConnection);
}

void HostController::setEnabled(bool on)
{
    if (!m_rh || on == m_enabled || (on && m_consent == QLatin1String("unavailable")))
        return;
    m_enabled = on;
    saveApp();
    m_error.clear();
    m_errorCode.clear();
    if (on) {
        m_state = QStringLiteral("starting");
        rh_enable(m_rh, 1, m_name.toUtf8().constData());
    } else {
        rh_enable(m_rh, 0, "");
    }
    Q_EMIT stateChanged();
    pollFirewall();
}

void HostController::restartForServer()
{
    if (!m_rh || !m_enabled)
        return;
    if (m_controlled) {
        m_serverPending = true;
        return;
    }
    m_serverPending = false;
    // Off, then on again once the old session has let go of its port.
    rh_enable(m_rh, 0, "");
    QTimer::singleShot(1500, this, [this] {
        if (m_enabled && m_rh)
            rh_enable(m_rh, 1, m_name.toUtf8().constData());
    });
}

void HostController::setName(const QString &name)
{
    const QString n = name.trimmed();
    if (n == m_name || (n == defaultName() && m_name.isEmpty()))
        return;
    m_name = n == defaultName() ? QString() : n;
    saveApp();
    if (m_rh)
        rh_set_name(m_rh, this->name().toUtf8().constData());
    Q_EMIT nameChanged();
}

void HostController::openPairing()
{
    if (m_rh && m_state == QLatin1String("on"))
        rh_open_pairing(m_rh);
}

void HostController::closePairing()
{
    if (m_rh)
        rh_close_pairing(m_rh);
}

void HostController::disconnectClient()
{
    if (m_rh)
        rh_disconnect(m_rh);
}

void HostController::revoke(const QString &id)
{
    if (m_rh)
        rh_revoke(m_rh, id.toUtf8().constData());
}

void HostController::refreshPaired()
{
    if (!m_rh)
        return;
    char *s = rh_paired(m_rh);
    const QJsonArray a = QJsonDocument::fromJson(QByteArray(s)).array();
    rq_free_string(s);
    QVariantList out;
    const QString live = m_controlled ? m_controller.value(QStringLiteral("id")).toString() : QString();
    for (const QJsonValue &v : a) {
        const QJsonObject o = v.toObject();
        QVariantMap m = o.toVariantMap();
        m.insert(QStringLiteral("pairedAt"), o.value(QStringLiteral("paired_at")).toVariant());
        m.insert(QStringLiteral("lastConnected"), o.value(QStringLiteral("last_connected")).toVariant());
        m.insert(QStringLiteral("live"), !live.isEmpty() && o.value(QStringLiteral("id")).toString() == live);
        out << m;
    }
    m_paired = out;
    Q_EMIT pairedChanged();
}

bool HostController::softwareVideo() const
{
    return !m_rh || rh_software_video(m_rh) != 0;
}

void HostController::setSoftwareVideo(bool on)
{
    if (!m_rh || (rh_software_video(m_rh) != 0) == on)
        return;
    rh_set_software_video(m_rh, on ? 1 : 0);
    Q_EMIT codecChanged();
}

void HostController::retryCodec()
{
    if (m_rh)
        rh_codec_retry(m_rh);
}

void HostController::refresh()
{
    refreshPaired();
    pollFirewall();
}

void HostController::handleEvent(const QJsonObject &ev)
{
    const QString kind = ev.value(QStringLiteral("ev")).toString();
    if (kind == QLatin1String("codec")) {
        // The engine's states are ids; the text is in the QML.
        const QString st = ev.value(QStringLiteral("state")).toString();
        // reason (for "failed"): network, rejected, unpinned or cancelled; anything else is read as network.
        QString reason = ev.value(QStringLiteral("reason")).toString();
        if (st == QLatin1String("failed") && !QStringList{QStringLiteral("network"), QStringLiteral("rejected"), QStringLiteral("unpinned"), QStringLiteral("cancelled")}.contains(reason))
            reason = QStringLiteral("network");
        m_codec = {{QStringLiteral("state"), st}, {QStringLiteral("percent"), ev.value(QStringLiteral("percent")).toInt()}, {QStringLiteral("reason"), reason}};
        Q_EMIT codecChanged();
        if (st == QLatin1String("needed"))
            Q_EMIT codecNoticeRequested();
        return;
    }
    if (kind == QLatin1String("host_matched")) {
        m_matchedFailed = ev.value(QStringLiteral("state")).toString() == QLatin1String("failed");
        Q_EMIT matchedChanged();
        return;
    }
    if (kind == QLatin1String("host_consent")) {
        setConsent(consentFromEvent(ev));
        if (m_consent == QLatin1String("unavailable") && m_enabled) {
            m_enabled = false; // no portal: the switch can't stay on
            saveApp();
            Q_EMIT stateChanged();
        }
        return;
    }
    if (kind == QLatin1String("host")) {
        const QString st = ev.value(QStringLiteral("state")).toString();
        if (st == QLatin1String("on")) {
            m_state = QStringLiteral("on");
            const QString id = ev.value(QStringLiteral("id")).toString();
            const int port = ev.value(QStringLiteral("port")).toInt();
            // Every IPv4 address of this computer, the likeliest LAN one first (192.168, then 172.16-31, then 10.).
            QStringList ips;
            for (const QHostAddress &a : QNetworkInterface::allAddresses())
                if (a.protocol() == QAbstractSocket::IPv4Protocol && !a.isLoopback() && !a.isLinkLocal())
                    ips << a.toString();
            auto rank = [](const QString &ip) { return ip.startsWith(QLatin1String("192.168.")) ? 0 : ip.startsWith(QLatin1String("172.")) ? 1 : 2; };
            std::stable_sort(ips.begin(), ips.end(), [&](const QString &x, const QString &y) { return rank(x) < rank(y); });
            m_addresses = ips;
            m_address = ips.isEmpty() ? QString() : QStringLiteral("%1@%2:%3").arg(id, ips.first()).arg(port);
            m_ipPorts.clear();
            for (const QString &ip : ips)
                m_ipPorts << QStringLiteral("%1:%2").arg(ip).arg(port);
            m_ip = ips.isEmpty() ? QString() : QStringLiteral("%1:%2").arg(ips.first()).arg(port);
            m_error.clear();
            m_errorCode.clear();
            // Nobody paired yet: a code is open for the first device (the board's first state).
            if (m_paired.isEmpty())
                openPairing();
        } else if (st == QLatin1String("off")) {
            m_state = QStringLiteral("off");
            m_address.clear();
            m_ip.clear();
            m_ipPorts.clear();
            m_controlled = false;
            m_controller.clear();
            m_pairing = {{QStringLiteral("state"), QStringLiteral("closed")}};
            Q_EMIT pairingChanged();
            Q_EMIT controlChanged();
        } else {
            m_state = QStringLiteral("error");
            m_enabled = false;
            m_error = ev.value(QStringLiteral("detail")).toString();
            m_errorCode = ev.value(QStringLiteral("code")).toString();
        }
        Q_EMIT stateChanged();
        refreshPaired();
    } else if (kind == QLatin1String("host_pairing")) {
        const QString st = ev.value(QStringLiteral("state")).toString();
        if (st == QLatin1String("open")) {
            m_pairingExpires = QDateTime::currentDateTime().addSecs(ev.value(QStringLiteral("expires_in")).toInt(300));
            m_pairing = {{QStringLiteral("state"), QStringLiteral("open")}, {QStringLiteral("code"), ev.value(QStringLiteral("code")).toString()}};
        } else if (st == QLatin1String("closed")) {
            m_pairing = {{QStringLiteral("state"), QStringLiteral("closed")}, {QStringLiteral("reason"), ev.value(QStringLiteral("reason")).toString()}};
        } else if (st == QLatin1String("wrong")) {
            m_pairing.insert(QStringLiteral("left"), ev.value(QStringLiteral("left")).toInt());
        }
        Q_EMIT pairingChanged();
    } else if (kind == QLatin1String("host_paired")) {
        refreshPaired();
    } else if (kind == QLatin1String("host_status")) {
        // A held session (the connection was lost; the host waits for the client to come back) isn't controlling
        // anything: the page, the tray and the paired list stop showing it as connected. A client that returns is a new "controlling".
        const bool now = ev.value(QStringLiteral("controlled")).toBool() && !ev.value(QStringLiteral("held")).toBool();
        QVariantMap c;
        if (now) {
            c = ev.value(QStringLiteral("client")).toObject().toVariantMap();
            c.insert(QStringLiteral("secure"), ev.value(QStringLiteral("secure")).toBool());
            c.insert(QStringLiteral("held"), ev.value(QStringLiteral("held")).toBool());
        }
        const bool started = now && (!m_controlled || c.value(QStringLiteral("id")) != m_controller.value(QStringLiteral("id")));
        const QString was = m_controller.value(QStringLiteral("name")).toString();
        const bool ended = !now && m_controlled;
        m_controlled = now;
        m_controller = c;
        Q_EMIT controlChanged();
        refreshPaired();
        if (started)
            Q_EMIT controlStarted(c);
        if (ended) {
            if (m_matchedFailed) {
                m_matchedFailed = false;
                Q_EMIT matchedChanged();
            }
            Q_EMIT controlEnded(was);
            if (m_serverPending)
                restartForServer();
        }
    }
}

void HostController::pollFirewall()
{
#ifdef Q_OS_WIN
    // Allowed when an enabled inbound Allow rule names this program on the Private profile and no enabled inbound Block rule does:
    // whoever made the rule (Windows' own prompt, the Allow button, an installer or an administrator). The list is read off the
    // UI thread; a firewall that can't be read leaves the row as it was.
    if (!m_firewallReading) {
        m_firewallReading = true;
        const QString exe = QDir::toNativeSeparators(QCoreApplication::applicationFilePath());
        QThread *reader = QThread::create([this, exe] {
            bool ok = false;
            const QList<FirewallRule> rules = readFirewallRules(&ok);
            const bool allowed = ok && firewallAllowsPrivate(rules, exe);
            QMetaObject::invokeMethod(this, [this, ok, allowed] {
                m_firewallReading = false;
                if (!ok)
                    return;
                const QString fw = allowed ? QStringLiteral("ok") : QStringLiteral("blocked");
                if (fw != m_firewall) {
                    m_firewall = fw;
                    Q_EMIT firewallChanged();
                }
            }, Qt::QueuedConnection);
        });
        connect(reader, &QThread::finished, reader, &QObject::deleteLater);
        reader->start();
    }

    auto *net = new QProcess(this);
    connect(net, &QProcess::finished, this, [this, net] {
        const QString out = QString::fromLocal8Bit(net->readAllStandardOutput()).trimmed();
        net->deleteLater();
        // "Public" only: every connected network is Public. Unknown (no output) is not alarming.
        const bool pub = !out.isEmpty() && !out.contains(QLatin1String("Private")) && !out.contains(QLatin1String("Domain")) && out.contains(QLatin1String("Public"));
        if (pub != m_public) {
            m_public = pub;
            Q_EMIT firewallChanged();
        }
    });
    net->setProgram(QStringLiteral("powershell"));
    net->setArguments({QStringLiteral("-NoProfile"), QStringLiteral("-NonInteractive"), QStringLiteral("-Command"),
                       QStringLiteral("(Get-NetConnectionProfile | ForEach-Object { $_.NetworkCategory }) -join ','")});
    net->start();
#endif
}

void HostController::allowFirewall()
{
#ifdef Q_OS_WIN
    // Windows' own elevation prompt (an administrator's approval): one inbound rule for this program, private networks.
    const QString exe = QDir::toNativeSeparators(QCoreApplication::applicationFilePath());
    const QString args = QStringLiteral("advfirewall firewall add rule name=\"%1\" dir=in action=allow program=\"%2\" profile=private enable=yes").arg(kRule, exe);
    ShellExecuteW(nullptr, L"runas", L"netsh.exe", reinterpret_cast<const wchar_t *>(args.utf16()), nullptr, SW_HIDE);
    m_firewall = QStringLiteral("wait");
    Q_EMIT firewallChanged();
    QTimer::singleShot(4000, this, [this] { pollFirewall(); });
#endif
}

void HostController::openNetworkSettings()
{
#ifdef Q_OS_WIN
    QDesktopServices::openUrl(QUrl(QStringLiteral("ms-settings:network-status")));
#endif
}

// ---- Linux: the desktop's permission to share the screen (the portal) ----------------------------------------------

QString HostController::consentFromEvent(const QJsonObject &ev)
{
    const QString s = ev.value(QStringLiteral("state")).toString();
    static const QStringList known{QStringLiteral("ask"), QStringLiteral("asking"), QStringLiteral("remembered"),
                                   QStringLiteral("declined"), QStringLiteral("unavailable")};
    return known.contains(s) ? s : QStringLiteral("ask");
}

QString HostController::desktop() const
{
#ifdef Q_OS_WIN
    return {};
#else
    // Names from the environment ("GNOME", "KDE", "Hyprland"; "wayland", "x11"), shown as they are.
    QString d = qEnvironmentVariable("XDG_CURRENT_DESKTOP").section(QLatin1Char(':'), -1);
    if (d.isEmpty())
        d = qEnvironmentVariable("XDG_SESSION_DESKTOP");
    const QString t = qEnvironmentVariable("XDG_SESSION_TYPE");
    const QString type = t == QLatin1String("x11") ? QStringLiteral("X11") : t == QLatin1String("wayland") ? QStringLiteral("Wayland") : QString();
    return d.isEmpty() ? type : type.isEmpty() ? d : d + QStringLiteral(" · ") + type;
#endif
}

QString HostController::desktopIdFor(const QString &currentDesktop)
{
    const QStringList parts = currentDesktop.toLower().split(QLatin1Char(':'), Qt::SkipEmptyParts);
    if (parts.contains(QLatin1String("gnome")))
        return QStringLiteral("gnome");
    if (parts.contains(QLatin1String("kde")))
        return QStringLiteral("kde");
    return QStringLiteral("other");
}

QString HostController::desktopId() const
{
#ifdef Q_OS_WIN
    return {};
#else
    return desktopIdFor(qEnvironmentVariable("XDG_CURRENT_DESKTOP"));
#endif
}

#ifndef Q_OS_WIN
// Does the desktop's portal offer RemoteDesktop and ScreenCast, on a Wayland session? (X11 isn't supported in 0.1.)
bool HostController::portalAvailable(QString *why) const
{
    if (qEnvironmentVariable("XDG_SESSION_TYPE") == QLatin1String("x11")) {
        *why = QStringLiteral("x11");
        return false;
    }
    auto bus = QDBusConnection::sessionBus();
    if (!bus.isConnected()) {
        *why = QStringLiteral("no session bus");
        return false;
    }
    for (const char *iface : {"org.freedesktop.portal.RemoteDesktop", "org.freedesktop.portal.ScreenCast"}) {
        QDBusInterface props(QStringLiteral("org.freedesktop.portal.Desktop"), QStringLiteral("/org/freedesktop/portal/desktop"),
                             QStringLiteral("org.freedesktop.DBus.Properties"), bus);
        const QDBusReply<QDBusVariant> r = props.call(QStringLiteral("Get"), QString::fromLatin1(iface), QStringLiteral("version"));
        if (!r.isValid() || r.value().variant().toUInt() == 0) {
            *why = QString::fromLatin1(iface);
            return false;
        }
    }
    return true;
}
#else
bool HostController::portalAvailable(QString *) const { return true; }
#endif

void HostController::setConsent(const QString &state)
{
    if (state == m_consent)
        return;
    m_consent = state;
    Q_EMIT consentChanged();
}

void HostController::loadConsent()
{
#ifndef Q_OS_WIN
    if (!m_rh)
        return;
    QString why;
    if (!portalAvailable(&why)) {
        setConsent(QStringLiteral("unavailable"));
        return;
    }
    char *s = rh_consent(m_rh);
    if (s) {
        setConsent(consentFromEvent(QJsonDocument::fromJson(QByteArray(s)).object()));
        rq_free_string(s);
    }
#endif
}

// ---- Windows: "Match the display to the connecting computer" -------------------------------------------------------

bool HostController::matchedDisplay() const
{
    if (m_matchedBusy)
        return m_matchedWant;
    return m_rh && rh_matched_display(m_rh) != 0;
}

void HostController::setMatchedDisplay(bool on)
{
#ifdef Q_OS_WIN
    if (!m_rh || m_matchedBusy || on == matchedDisplay())
        return;
    m_matchedBusy = true;
    m_matchedWant = on;
    m_matchedInstallFailed = false;
    Q_EMIT matchedChanged();
    // The driver goes in or out in an elevated copy of this program (one UAC prompt), which checks the driver's files against
    // their pins first. The setting is saved only when that worked; a No at the prompt changes nothing.
    const QString exe = QDir::toNativeSeparators(QCoreApplication::applicationFilePath());
    const QString log = QDir::toNativeSeparators(m_dataDir + QStringLiteral("/matched-display-setup.log"));
    const QString args = QStringLiteral("--matched-display %1 --log \"%2\"").arg(on ? QStringLiteral("install") : QStringLiteral("uninstall"), log);
    HWND owner = GetForegroundWindow();
    QThread *t = QThread::create([this, exe, args, on, owner] {
        SHELLEXECUTEINFOW sei{};
        sei.cbSize = sizeof sei;
        sei.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        sei.hwnd = owner;
        sei.lpVerb = L"runas";
        sei.lpFile = reinterpret_cast<const wchar_t *>(exe.utf16());
        sei.lpParameters = reinterpret_cast<const wchar_t *>(args.utf16());
        sei.nShow = SW_HIDE;
        DWORD code = 1;
        bool cancelled = false;
        if (ShellExecuteExW(&sei) && sei.hProcess) {
            WaitForSingleObject(sei.hProcess, INFINITE);
            GetExitCodeProcess(sei.hProcess, &code);
            CloseHandle(sei.hProcess);
        } else {
            cancelled = GetLastError() == ERROR_CANCELLED; // No at the UAC prompt
        }
        QMetaObject::invokeMethod(this, [this, on, code, cancelled] {
            m_matchedBusy = false;
            if (code == 0 && m_rh)
                rh_set_matched_display(m_rh, on ? 1 : 0);
            // Silent on a No; the reason for a real failure is in the log.
            m_matchedInstallFailed = on && code != 0 && !cancelled;
            Q_EMIT matchedChanged();
        }, Qt::QueuedConnection);
    });
    connect(t, &QThread::finished, t, &QObject::deleteLater);
    t->start();
#else
    Q_UNUSED(on)
#endif
}

void HostController::injectEvent(const QString &json)
{
    const QJsonObject ev = QJsonDocument::fromJson(json.toUtf8()).object();
    // As if the switch had been turned on.
    if (ev.value(QStringLiteral("ev")).toString() == QLatin1String("host") && ev.value(QStringLiteral("state")).toString() == QLatin1String("on"))
        m_enabled = true;
    handleEvent(ev);
}

void HostController::forgetConsent()
{
#ifndef Q_OS_WIN
    if (m_rh)
        rh_forget_consent(m_rh); // the engine answers with a host_consent event
#endif
}

void HostController::copyIpAddress(const QString &ipPort)
{
    if (m_ipPorts.contains(ipPort))
        QGuiApplication::clipboard()->setText(ipPort);
}

void HostController::openLinuxGuide()
{
    QDesktopServices::openUrl(QUrl(QString::fromLatin1(brand::LinuxHostUrl)));
}

void HostController::copyAddress(bool full)
{
    // IP:PORT pairs by itself (the client learns the ID from the handshake); the full ID@IP:PORT is the option.
    const QString &a = full ? m_address : m_ip;
    if (!a.isEmpty())
        QGuiApplication::clipboard()->setText(a);
}
