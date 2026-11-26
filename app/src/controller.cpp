// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "controller.h"
#include "brand.h"

#include <QCoreApplication>
#include <QDesktopServices>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QUrl>
#include <QGuiApplication>
#include <QHostInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QMetaObject>
#include <QSet>
#include <QScreen>
#include <QWindow>
#ifdef Q_OS_WIN
#include "streamframe.h"
#else
#include <qpa/qplatformnativeinterface.h>
#endif

#include <cmath>

namespace {

QVariantMap jsonMap(const QJsonObject &o)
{
    return o.toVariantMap();
}

QString str(const QJsonObject &o, const char *k)
{
    return o.value(QLatin1String(k)).toString();
}

} // namespace

RemoteController::RemoteController(const QString &dataDir, bool rendezvous, bool engine, QObject *parent)
    : QObject(parent), m_dataDir(dataDir), m_useServer(rendezvous)
{
    QDir().mkpath(m_dataDir);
    m_deviceName = QHostInfo::localHostName();
    loadApp();
    loadServer();
    if (engine) {
        char *d = rq_decode_support();
        m_decode = QJsonDocument::fromJson(QByteArray(d)).object().toVariantMap();
        rq_free_string(d);
        m_rq = rq_new(m_dataDir.toUtf8().constData(), m_deviceName.toUtf8().constData(), rendezvous ? 1 : 0, &RemoteController::trampoline, this);
        if (m_rq)
            rq_key_passthrough(m_rq, m_keyMapping == QLatin1String("pass") ? 1 : 0);
    }
    m_refreshTimer.setInterval(5000);
    connect(&m_refreshTimer, &QTimer::timeout, this, [this] {
        if (m_state == QLatin1String("idle"))
            refresh();
    });
    m_refreshTimer.start();
}

RemoteController::~RemoteController()
{
    if (m_rq)
        rq_free(m_rq);
}

void RemoteController::trampoline(void *user, const char *json)
{
    // Engine threads: hand the text to the Qt thread.
    auto *self = static_cast<RemoteController *>(user);
    const QByteArray bytes(json);
    QMetaObject::invokeMethod(self, [self, bytes] { self->handleEvent(QJsonDocument::fromJson(bytes).object()); }, Qt::QueuedConnection);
}

// ---- app.json: settings and what we learned about each computer ---------------------------------------------

void RemoteController::loadApp()
{
    QFile f(m_dataDir + QStringLiteral("/app.json"));
    if (!f.open(QIODevice::ReadOnly))
        return;
    const QJsonObject o = QJsonDocument::fromJson(f.readAll()).object();
    const QJsonObject s = o.value(QStringLiteral("settings")).toObject();
    m_codec = s.value(QStringLiteral("codec")).toString(QStringLiteral("auto"));
    m_bandwidth = s.value(QStringLiteral("bandwidthMbps")).toDouble(0);
    m_keyMapping = s.value(QStringLiteral("keyMapping")).toString(QStringLiteral("auto"));
    m_fixed = s.value(QStringLiteral("fixedResolution")).toBool(false);
    m_refresh = s.value(QStringLiteral("refreshHz")).toInt(0);
    m_previews = s.value(QStringLiteral("showPreviews")).toBool(true);
    m_hosts = o.value(QStringLiteral("hosts")).toObject().toVariantMap();
}

void RemoteController::saveApp()
{
    QJsonObject s{{QStringLiteral("codec"), m_codec},
                  {QStringLiteral("bandwidthMbps"), m_bandwidth},
                  {QStringLiteral("keyMapping"), m_keyMapping},
                  {QStringLiteral("fixedResolution"), m_fixed},
                  {QStringLiteral("refreshHz"), m_refresh},
                  {QStringLiteral("showPreviews"), m_previews}};
    QJsonObject o{{QStringLiteral("settings"), s}, {QStringLiteral("hosts"), QJsonObject::fromVariantMap(m_hosts)}};
    QFile f(m_dataDir + QStringLiteral("/app.json"));
    if (f.open(QIODevice::WriteOnly | QIODevice::Truncate))
        f.write(QJsonDocument(o).toJson(QJsonDocument::Compact));
}

QVariantMap RemoteController::hostInfo(const QString &id) const
{
    return m_hosts.value(id).toMap();
}

void RemoteController::setHostInfo(const QString &id, const QString &key, const QVariant &value)
{
    QVariantMap h = hostInfo(id);
    if (h.value(key) == value)
        return;
    h.insert(key, value);
    m_hosts.insert(id, h);
    saveApp();
}

#define SETTING(setter, field, type)                                                                                   \
    void RemoteController::setter(type v)                                                                              \
    {                                                                                                                  \
        if (field == v)                                                                                                \
            return;                                                                                                    \
        field = v;                                                                                                     \
        saveApp();                                                                                                     \
        Q_EMIT settingsChanged();                                                                                      \
    }
SETTING(setCodec, m_codec, const QString &)
SETTING(setBandwidthMbps, m_bandwidth, double)
SETTING(setFixedResolution, m_fixed, bool)
SETTING(setRefreshHz, m_refresh, int)

void RemoteController::setShowPreviews(bool on)
{
    if (m_previews == on)
        return;
    m_previews = on;
    if (!on) {
        // Off deletes what is saved; the engine writes nothing more (connectTo leaves the picture out of the options).
        const QDir dir(m_dataDir + QStringLiteral("/thumbs"));
        for (const QString &f : dir.entryList(QDir::Files))
            QFile::remove(dir.filePath(f));
    }
    saveApp();
    Q_EMIT settingsChanged();
    if (!m_lastHosts.isEmpty())
        onHosts(m_lastHosts);
    if (!on)
        Q_EMIT previewsTurnedOff();
}

QString RemoteController::appVersion() const { return QStringLiteral(REMOTE_VERSION); }
QString RemoteController::licenseId() const { return QString::fromLatin1(brand::License); }
QString RemoteController::sourceUrl() const { return QString::fromLatin1(brand::SourceUrl); }

QString RemoteController::noticesPath() const
{
    const QString exe = QCoreApplication::applicationDirPath();
    const QStringList dirs{exe, exe + QStringLiteral("/.."), exe + QStringLiteral("/../share/doc/") + QString::fromLatin1(brand::LinuxDirName),
                           exe + QStringLiteral("/../share/") + QString::fromLatin1(brand::LinuxDirName), QStringLiteral(REMOTE_SOURCE_DIR)};
    for (const QString &d : dirs)
        for (const QString &n : {QStringLiteral("THIRD-PARTY-NOTICES"), QStringLiteral("THIRD-PARTY-NOTICES.md"), QStringLiteral("THIRD-PARTY-NOTICES.txt")}) {
            const QFileInfo fi(d + QLatin1Char('/') + n);
            if (fi.isFile())
                return fi.canonicalFilePath();
        }
    return {};
}

QString RemoteController::noticesText() const
{
    QFile f(noticesPath());
    return f.open(QIODevice::ReadOnly) ? QString::fromUtf8(f.readAll()) : QString();
}

bool RemoteController::openNotices()
{
    const QString p = noticesPath();
    return !p.isEmpty() && QDesktopServices::openUrl(QUrl::fromLocalFile(p));
}

void RemoteController::openSource()
{
    QDesktopServices::openUrl(QUrl(sourceUrl()));
}

void RemoteController::setKeyMapping(const QString &m)
{
    if (m_keyMapping == m)
        return;
    m_keyMapping = m;
    if (m_rq)
        rq_key_passthrough(m_rq, m == QLatin1String("pass") ? 1 : 0);
    saveApp();
    Q_EMIT settingsChanged();
}

// ---- events -------------------------------------------------------------------------------------------------

void RemoteController::handleEvent(const QJsonObject &ev)
{
    const QString kind = str(ev, "ev");
    if (kind == QLatin1String("hosts")) {
        onHosts(ev);
    } else if (kind == QLatin1String("self")) {
        m_selfCode = str(ev, "code");
        Q_EMIT selfChanged();
    } else if (kind == QLatin1String("rendezvous")) {
        m_relayOnline = ev.value(QStringLiteral("online")).toBool();
        Q_EMIT relayChanged();
        if (serverConfigured()) {
            m_serverState = m_relayOnline ? QStringLiteral("online") : QStringLiteral("offline");
            // The server's refusal: "rendezvous refused: <code> (<its text>)". Only the code is read; the text is never shown.
            const QString err = ev.value(QStringLiteral("error")).toString();
            static const QString key = QStringLiteral("rendezvous refused: ");
            QString problem;
            const int at = err.indexOf(key);
            if (!m_relayOnline && at >= 0) {
                const QString code = err.mid(at + key.size()).section(QLatin1Char(' '), 0, 0);
                if (code == QLatin1String("token_invalid")) problem = QStringLiteral("tokenInvalid");
                else if (code == QLatin1String("token_expired")) problem = QStringLiteral("tokenExpired");
                else if (code == QLatin1String("host_not_allowed")) problem = QStringLiteral("tokenNeeded");
                else if (code == QLatin1String("quota_exceeded")) problem = QStringLiteral("quotaExceeded");
                else if (code == QLatin1String("payment_required")) problem = QStringLiteral("paymentRequired");
            }
            m_serverProblem = problem;
            Q_EMIT serverChanged();
        }
    } else if (kind == QLatin1String("server_checking")) {
        // Screenshots and tests only (--script event): the state before the first answer.
        m_serverState = QStringLiteral("checking");
        m_serverProblem.clear();
        Q_EMIT serverChanged();
    } else if (kind == QLatin1String("pair")) {
        QVariantMap p = jsonMap(ev);
        p.remove(QStringLiteral("ev"));
        m_pairing = p;
        Q_EMIT pairingChanged();
    } else if (kind == QLatin1String("address")) {
        QVariantMap a = jsonMap(ev);
        a.remove(QStringLiteral("ev"));
        m_address = a;
        Q_EMIT addressChanged();
    } else if (kind == QLatin1String("status")) {
        onStatus(ev);
    } else if (kind == QLatin1String("welcome")) {
        onWelcome(ev);
    } else if (kind == QLatin1String("stream")) {
        if (m_state != QLatin1String("streaming")) {
            setState(QStringLiteral("streaming"));
            Q_EMIT streamStarted();
        }
    } else if (kind == QLatin1String("secure")) {
        m_secure = ev.value(QStringLiteral("on")).toBool();
        Q_EMIT secureChanged();
    } else if (kind == QLatin1String("capture")) {
        m_capture = ev.value(QStringLiteral("on")).toBool();
        Q_EMIT captureChanged();
    } else if (kind == QLatin1String("video_click")) {
        Q_EMIT menuCloseRequested();
    } else if (kind == QLatin1String("menu_key")) {
#ifdef Q_OS_WIN
        if (m_frame)
            m_frame->sendMenuKey(unsigned(ev.value(QStringLiteral("vk")).toInt()));
#endif
    } else if (kind == QLatin1String("leave_chord")) {
        Q_EMIT leaveChord();
    } else if (kind == QLatin1String("session_over")) {
        // The status event said why; a session that ended with nothing said is "ended".
        if (m_state != QLatin1String("failed") && m_state != QLatin1String("ended") && m_state != QLatin1String("idle"))
            setState(QStringLiteral("ended"), {{QStringLiteral("kind"), QStringLiteral("error")}});
        m_secure = false;
        Q_EMIT secureChanged();
        if (m_serverWaits) {
            // The browse client started again with the session's end and read the new server.
            m_serverWaits = false;
            applyServer();
        }
        refresh();
    }
}

void RemoteController::onHosts(const QJsonObject &ev)
{
    m_lastHosts = ev;
    QList<QVariantMap> rows;
    for (const auto &v : ev.value(QStringLiteral("paired")).toArray()) {
        const QJsonObject d = v.toObject();
        const QString id = str(d, "id");
        const QVariantMap info = hostInfo(id);
        const QString route = info.value(QStringLiteral("route")).toString();
        const bool relayRoute = !serverConfigured() && route.startsWith(QLatin1String("relay"));
        const QString own = str(d, "name");
        QString state = d.value(QStringLiteral("online")).toBool() ? QStringLiteral("online") : QStringLiteral("offline");
        if (!d.value(QStringLiteral("asleep_since")).isNull())
            state = QStringLiteral("asleep");
        if (id == m_sessionHost && m_state == QLatin1String("connecting"))
            state = QStringLiteral("connecting");
        rows.append({{QStringLiteral("hostId"), id},
                     {QStringLiteral("code"), str(d, "code")},
                     {QStringLiteral("name"), info.value(QStringLiteral("name")).toString().isEmpty() ? own : info.value(QStringLiteral("name")).toString()},
                     {QStringLiteral("deviceName"), own},
                     {QStringLiteral("os"), info.value(QStringLiteral("os")).toString()},
                     {QStringLiteral("state"), state},
                     {QStringLiteral("onLan"), d.value(QStringLiteral("on_lan")).toBool()},
                     {QStringLiteral("asleepSince"), qint64(d.value(QStringLiteral("asleep_since")).toDouble())},
                     {QStringLiteral("lastSeen"), qint64(d.value(QStringLiteral("last_seen")).toDouble())},
                     {QStringLiteral("lastConnected"), qint64(d.value(QStringLiteral("last_connected")).toDouble())},
                     // With no server there is no relay to name, so a relay path learned earlier is not shown.
                     {QStringLiteral("route"), relayRoute ? QString() : route},
                     {QStringLiteral("rttMs"), relayRoute ? 0.0 : info.value(QStringLiteral("rtt")).toDouble()},
                     {QStringLiteral("thumb"), m_previews ? thumbUrl(id) : QString()}});
    }
    m_computers.setRows(rows);
    pruneThumbs(ev.value(QStringLiteral("paired")).toArray());
    QList<QVariantMap> near;
    for (const auto &v : ev.value(QStringLiteral("found")).toArray()) {
        const QJsonObject d = v.toObject();
        near.append({{QStringLiteral("hostId"), str(d, "id")},
                     {QStringLiteral("code"), str(d, "code")},
                     {QStringLiteral("name"), str(d, "name")},
                     {QStringLiteral("addresses"), d.value(QStringLiteral("addrs")).toArray().toVariantList()},
                     {QStringLiteral("port"), d.value(QStringLiteral("port")).toInt()}});
    }
    m_nearby.setRows(near);
}

void RemoteController::setState(const QString &state, const QVariantMap &failure)
{
    m_state = state;
    m_failure = failure;
    Q_EMIT sessionChanged();
}

void RemoteController::onStatus(const QJsonObject &ev)
{
    const QString state = str(ev, "state");
    if (state == QLatin1String("connecting")) {
        setState(QStringLiteral("connecting"));
    } else if (state == QLatin1String("connected")) {
        m_route = str(ev, "route");
        m_rtt = ev.value(QStringLiteral("rtt_ms")).toDouble();
        if (!m_sessionHost.isEmpty()) {
            setHostInfo(m_sessionHost, QStringLiteral("route"), m_route);
            setHostInfo(m_sessionHost, QStringLiteral("rtt"), m_rtt);
        }
        Q_EMIT routeChanged();
        if (m_state != QLatin1String("streaming"))
            setState(QStringLiteral("connected"));
    } else if (state == QLatin1String("reconnecting")) {
        m_route = QStringLiteral("lost");
        Q_EMIT routeChanged();
        setState(QStringLiteral("reconnecting"));
    } else if (state == QLatin1String("failed")) {
        QVariantMap f = jsonMap(ev.value(QStringLiteral("error")).toObject());
        // A refusal carries the host's reason: the version card, taken over...
        if (f.value(QStringLiteral("kind")).toString() == QLatin1String("refused")) {
            const QVariantMap r = f.value(QStringLiteral("reason")).toMap();
            if (!r.isEmpty())
                f = r;
        }
        setState(QStringLiteral("failed"), f);
    } else if (state == QLatin1String("ended")) {
        const QVariantMap r = jsonMap(ev.value(QStringLiteral("reason")).toObject());
        // The user's own disconnect is not a card.
        if (r.value(QStringLiteral("kind")).toString() == QLatin1String("user"))
            setState(QStringLiteral("idle"));
        else
            setState(QStringLiteral("ended"), r);
    }
}

void RemoteController::onWelcome(const QJsonObject &ev)
{
    {
        const QString custom = hostInfo(m_sessionHost).value(QStringLiteral("name")).toString();
        const QString own = str(ev, "name");
        if (!custom.isEmpty())
            m_hostName = custom;
        else if (!own.isEmpty())
            m_hostName = own;
    }
    // The system is stored and shown as an id: "sodiumos" (a Linux that says so), "linux", "mac" or
    // "windows", nothing else; the engine has already folded the Linux flavor into it.
    m_hostOs = str(ev, "host_os");
    if (m_hostOs != QLatin1String("sodiumos") && m_hostOs != QLatin1String("linux") && m_hostOs != QLatin1String("mac")
        && m_hostOs != QLatin1String("windows"))
        m_hostOs.clear();
    m_viewOnly = ev.value(QStringLiteral("view_only")).toBool();
    m_physical = str(ev, "display_mode") == QLatin1String("physical");
    m_liveResize = ev.value(QStringLiteral("live_resize")).toBool(true);
    if (!m_sessionHost.isEmpty())
        setHostInfo(m_sessionHost, QStringLiteral("os"), m_hostOs);
    QList<QVariantMap> rows;
    int n = 0;
    const int cur = ev.value(QStringLiteral("current_display")).toInt();
    for (const auto &v : ev.value(QStringLiteral("displays")).toArray()) {
        const QJsonObject d = v.toObject();
        rows.append({{QStringLiteral("displayId"), d.value(QStringLiteral("id")).toInt()},
                     {QStringLiteral("name"), str(d, "name")},
                     {QStringLiteral("width"), d.value(QStringLiteral("width")).toInt()},
                     {QStringLiteral("height"), d.value(QStringLiteral("height")).toInt()},
                     {QStringLiteral("primary"), d.value(QStringLiteral("primary")).toBool()},
                     {QStringLiteral("current"), d.value(QStringLiteral("id")).toInt() == cur},
                     {QStringLiteral("number"), ++n}});
    }
    m_displays.setRows(rows);
    Q_EMIT hostChanged();
}

// ---- commands -----------------------------------------------------------------------------------------------

void RemoteController::refresh()
{
    if (m_rq)
        rq_refresh(m_rq);
}

void RemoteController::pair(const QString &hostId, const QString &code, const QString &addr)
{
    m_pairing = {{QStringLiteral("state"), QStringLiteral("checking")}, {QStringLiteral("host"), hostId}};
    Q_EMIT pairingChanged();
    if (m_rq)
        rq_pair(m_rq, hostId.toUtf8().constData(), code.toUtf8().constData(), addr.toUtf8().constData());
}

void RemoteController::resetPairing()
{
    m_pairing = {{QStringLiteral("state"), QStringLiteral("idle")}};
    Q_EMIT pairingChanged();
}

void RemoteController::resolveAddress(const QString &text)
{
    m_address = {{QStringLiteral("state"), QStringLiteral("checking")}, {QStringLiteral("addr"), text}};
    Q_EMIT addressChanged();
    if (m_rq)
        rq_resolve_address(m_rq, text.toUtf8().constData());
}

void RemoteController::resetAddress()
{
    m_address = {{QStringLiteral("state"), QStringLiteral("idle")}};
    Q_EMIT addressChanged();
}

QString RemoteController::thumbPath(const QString &hostId) const
{
    QString safe = hostId;
    for (QChar &c : safe)
        if (!c.isLetterOrNumber() && c != u'-' && c != u'_')
            c = u'_';
    return m_dataDir + QStringLiteral("/thumbs/") + safe + QStringLiteral(".png");
}

QString RemoteController::thumbUrl(const QString &hostId) const
{
    const QFileInfo fi(thumbPath(hostId));
    if (!fi.exists())
        return {};
    // The stamp makes a new picture a new URL, so the card reloads it.
    return QUrl::fromLocalFile(fi.absoluteFilePath()).toString() + QStringLiteral("?t=") + QString::number(fi.lastModified().toMSecsSinceEpoch());
}

void RemoteController::pruneThumbs(const QJsonArray &paired)
{
    // A computer that is no longer paired (removed here, or revoked from its side) keeps no picture.
    QSet<QString> keep;
    for (const auto &v : paired)
        keep.insert(QFileInfo(thumbPath(v.toObject().value(QStringLiteral("id")).toString())).fileName());
    const QDir dir(m_dataDir + QStringLiteral("/thumbs"));
    for (const QString &f : dir.entryList({QStringLiteral("*.png")}, QDir::Files))
        if (!keep.contains(f))
            QFile::remove(dir.filePath(f));
}

void RemoteController::forget(const QString &hostId)
{
    QFile::remove(thumbPath(hostId));
    m_hosts.remove(hostId);
    saveApp();
    if (m_rq)
        rq_forget(m_rq, hostId.toUtf8().constData());
}

void RemoteController::rename(const QString &hostId, const QString &name)
{
    setHostInfo(hostId, QStringLiteral("name"), name.trimmed());
    if (!m_lastHosts.isEmpty())
        onHosts(m_lastHosts);
}

QRect RemoteController::fitWindow(const QRect &avail, const QSize &want)
{
    const int w = qMin(want.width(), avail.width() * 9 / 10);
    const int h = qMin(want.height(), avail.height() * 9 / 10);
    return QRect(avail.x() + (avail.width() - w) / 2, avail.y() + (avail.height() - h) / 2, w, h);
}

QVariantMap RemoteController::streamWindowRect(QWindow *anchor, int width, int height) const
{
    QScreen *screen = anchor ? anchor->screen() : nullptr;
    if (!screen)
        screen = QGuiApplication::primaryScreen();
    const QRect r = screen ? fitWindow(screen->availableGeometry(), QSize(width, height)) : QRect(0, 0, width, height);
    return {{QStringLiteral("x"), r.x()}, {QStringLiteral("y"), r.y()}, {QStringLiteral("width"), r.width()}, {QStringLiteral("height"), r.height()}};
}

bool RemoteController::connectTo(const QString &hostId, QWindow *window, bool takeover, bool fullscreen, int videoY)
{
    if (!m_rq || !window || (m_state != QLatin1String("idle") && m_state != QLatin1String("failed") && m_state != QLatin1String("ended")))
        return false;
#ifdef Q_OS_WIN
    if (!m_frame)
    {
        m_frame = new StreamFrame(window, m_computers.get(hostId).value(QStringLiteral("name")).toString(), this);
        connect(m_frame, &StreamFrame::deactivated, this, &RemoteController::menuCloseRequested);
    }
    void *dpy = nullptr;
    void *surf = m_frame->hwnd();
    if (!surf)
        return false;
#else
    auto *ni = QGuiApplication::platformNativeInterface();
    void *dpy = ni ? ni->nativeResourceForIntegration("wl_display") : nullptr;
    void *surf = ni ? ni->nativeResourceForWindow("surface", window) : nullptr;
    if (!dpy || !surf)
        return false;
#endif
    m_sessionHost = hostId;
    m_secure = false;
    m_capture = true;
    m_route.clear();
    m_displays.setRows({});
    // Until Welcome says: what we learned last time (an enum, never text).
    m_hostOs = hostInfo(hostId).value(QStringLiteral("os")).toString();
    m_viewOnly = false;
    m_hostName = m_computers.get(hostId).value(QStringLiteral("name")).toString();
#ifdef Q_OS_WIN
    m_frame->setTitle(m_hostName);
#endif
    Q_EMIT secureChanged();
    Q_EMIT captureChanged();
    Q_EMIT hostChanged();
    // The window's video area is set by the stream window as soon as it lays out (updateView); start with the window.
    const qreal dpr = window->devicePixelRatio();
    QJsonObject o{{QStringLiteral("host"), hostId},
                  {QStringLiteral("takeover"), takeover},
                  {QStringLiteral("codec"), m_codec},
                  {QStringLiteral("max_mbps"), m_bandwidth},
                  {QStringLiteral("key_passthrough"), m_keyMapping == QLatin1String("pass")},
                  {QStringLiteral("ppp"), dpr},
                  {QStringLiteral("scale"), int(std::ceil(dpr))},
                  {QStringLiteral("x"), 0},
                  {QStringLiteral("y"), videoY},
                  {QStringLiteral("width"), window->width()},
                  {QStringLiteral("height"), qMax(1, window->height() - videoY)},
                  {QStringLiteral("fullscreen"), fullscreen},
                  {QStringLiteral("fixed"), m_fixed},
                  {QStringLiteral("thumb"), m_previews ? thumbPath(hostId) : QString()}};
    if (m_refresh > 0)
        o.insert(QStringLiteral("fps"), m_refresh);
    setState(QStringLiteral("connecting"));
    if (!m_lastHosts.isEmpty())
        onHosts(m_lastHosts);
    const int r = rq_start(m_rq, QJsonDocument(o).toJson(QJsonDocument::Compact).constData(), dpy, surf);
    if (r != 0) {
        setState(QStringLiteral("failed"), {{QStringLiteral("kind"), QStringLiteral("failed")}});
        return false;
    }
    return true;
}

void RemoteController::disconnectSession()
{
    if (m_rq && m_state != QLatin1String("idle"))
        rq_stop(m_rq);
}

void RemoteController::clearSession()
{
    if (m_rq && m_state != QLatin1String("idle") && m_state != QLatin1String("failed") && m_state != QLatin1String("ended"))
        rq_stop(m_rq);
    m_sessionHost.clear();
    setState(QStringLiteral("idle"));
    refresh();
}

void RemoteController::updateView(int x, int y, int w, int h, qreal scale, bool fullscreen)
{
    if (m_rq)
        rq_view(m_rq, x, y, w, h, int(std::ceil(scale)), fullscreen ? 1 : 0);
}

void RemoteController::setHudRects(const QVariantList &rects)
{
#ifdef Q_OS_WIN
    if (m_frame)
        m_frame->setRegion(rects);
#endif
    if (!m_rq)
        return;
    QList<int> v;
    for (const QVariant &r : rects) {
        const QVariantMap m = r.toMap();
        v << int(m.value(QStringLiteral("x")).toDouble()) << int(m.value(QStringLiteral("y")).toDouble())
          << int(std::ceil(m.value(QStringLiteral("w")).toDouble())) << int(std::ceil(m.value(QStringLiteral("h")).toDouble()));
    }
    rq_hud_rects(m_rq, v.constData(), int(v.size() / 4));
}

void RemoteController::setCapture(bool on)
{
    if (m_rq)
        rq_capture(m_rq, on ? 1 : 0);
    if (m_capture != on) {
        m_capture = on;
        Q_EMIT captureChanged();
    }
}

void RemoteController::setMenuBlock(bool on)
{
    if (m_rq)
        rq_menu_block(m_rq, on ? 1 : 0);
}

void RemoteController::selectDisplay(int id)
{
    if (m_rq)
        rq_select_display(m_rq, uint32_t(id));
    for (int i = 0; i < m_displays.rowCount(); ++i) {
        QList<QVariantMap> rows = m_displays.rows();
        for (auto &r : rows)
            r.insert(QStringLiteral("current"), r.value(QStringLiteral("displayId")).toInt() == id);
        m_displays.setRows(rows);
        break;
    }
}

void RemoteController::sendCtrlAltDel()
{
    if (m_rq)
        rq_send_cad(m_rq);
}

void RemoteController::pollStats()
{
    if (!m_rq)
        return;
    char *s = rq_stats(m_rq);
    const QVariantMap st = QJsonDocument::fromJson(QByteArray(s)).object().toVariantMap();
    rq_free_string(s);
    if (st != m_stats) {
        m_stats = st;
        Q_EMIT statsChanged();
    }
}

void RemoteController::injectEvent(const QString &json)
{
    handleEvent(QJsonDocument::fromJson(json.toUtf8()).object());
}

// ---- Settings › Server ----------------------------------------------------------------

namespace {

QVariantMap parseReply(char *json)
{
    const QVariantMap m = QJsonDocument::fromJson(QByteArray(json)).object().toVariantMap();
    rq_free_string(json);
    return m;
}

}

void RemoteController::loadServer()
{
    char *j = rq_server_info();
    const QVariantMap info = parseReply(j);
    m_server = {{QStringLiteral("configured"), info.value(QStringLiteral("configured")).toBool()},
                {QStringLiteral("url"), info.value(QStringLiteral("url"))},
                {QStringLiteral("host"), info.value(QStringLiteral("host"))},
                {QStringLiteral("official"), info.value(QStringLiteral("official")).toBool()},
                {QStringLiteral("pinnedCa"), info.value(QStringLiteral("pinned_ca")).toBool()},
                {QStringLiteral("token"), info.value(QStringLiteral("token")).toBool()},
                {QStringLiteral("tokenText"), info.value(QStringLiteral("token_text"))},
                {QStringLiteral("caFile"), info.value(QStringLiteral("ca_file"))},
                {QStringLiteral("error"), info.value(QStringLiteral("error"))}};
    m_serverState = !serverConfigured() ? QStringLiteral("none") : m_useServer ? QStringLiteral("checking") : QStringLiteral("off");
    m_serverProblem.clear();
    m_relayOnline = false;
}

// A server was saved or removed: read it again, and start the engine's browse client anew when no session is running.
void RemoteController::applyServer()
{
    loadServer();
    Q_EMIT relayChanged();
    Q_EMIT serverChanged();
    if (!m_lastHosts.isEmpty())
        onHosts(m_lastHosts);
}

QVariantMap RemoteController::checkServerLink(const QString &text)
{
    return parseReply(rq_server_link_parse(text.toUtf8().constData()));
}

QVariantMap RemoteController::checkServer(const QString &address, const QString &caPem, const QString &token)
{
    return parseReply(rq_server_check(address.toUtf8().constData(), caPem.toUtf8().constData(), token.trimmed().toUtf8().constData()));
}

QVariantMap RemoteController::readServerFile(const QUrl &file)
{
    const QString path = file.isLocalFile() ? file.toLocalFile() : file.toString();
    return parseReply(rq_server_file_parse(path.toUtf8().constData()));
}

QVariantMap RemoteController::readCertificate(const QUrl &file)
{
    const QString path = file.isLocalFile() ? file.toLocalFile() : file.toString();
    QFile f(path);
    // A file that can't be read comes back as text that holds no certificate: the sheet says so on the field.
    const bool ok = f.open(QIODevice::ReadOnly);
    return {{QStringLiteral("ok"), ok}, {QStringLiteral("name"), QFileInfo(path).fileName()}, {QStringLiteral("pem"), ok ? QString::fromUtf8(f.read(1 << 20)) : QString()}};
}

QString RemoteController::serverCertificate() const
{
    const QString path = m_server.value(QStringLiteral("caFile")).toString();
    QFile f(path);
    return !path.isEmpty() && f.open(QIODevice::ReadOnly) ? QString::fromUtf8(f.read(1 << 20)) : QString();
}

QVariantMap RemoteController::saveServer(const QString &link)
{
    const QVariantMap r = parseReply(rq_server_import(link.toUtf8().constData()));
    if (!r.value(QStringLiteral("ok")).toBool())
        return r;
    m_serverWaits = m_rq && !rq_restart_browse(m_rq);
    applyServer();
    Q_EMIT serverSaved();
    return r;
}

QVariantMap RemoteController::removeServer()
{
    const QVariantMap r = parseReply(rq_server_remove());
    if (!r.value(QStringLiteral("ok")).toBool())
        return r;
    m_serverWaits = m_rq && !rq_restart_browse(m_rq);
    applyServer();
    Q_EMIT serverSaved();
    return r;
}

QVariantMap RemoteController::exportServer(const QUrl &file)
{
    const QString path = file.isLocalFile() ? file.toLocalFile() : file.toString();
    return parseReply(rq_server_export(path.toUtf8().constData()));
}

void RemoteController::retryServer()
{
    if (!m_rq || !serverConfigured())
        return;
    if (rq_restart_browse(m_rq)) {
        m_serverState = QStringLiteral("checking");
        m_serverProblem.clear();
        Q_EMIT serverChanged();
    }
}

void RemoteController::openSelfHosting()
{
    QDesktopServices::openUrl(QUrl(QString::fromLatin1(brand::SelfHostingUrl)));
}

void RemoteController::openServerLink(const QString &link)
{
    m_pendingLink = link;
    Q_EMIT serverLinkRequested(link);
}

QString RemoteController::takePendingServerLink()
{
    return std::exchange(m_pendingLink, QString());
}
