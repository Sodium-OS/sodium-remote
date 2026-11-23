// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "sodiumwmwindow.h"

#include <QCoreApplication>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QHash>
#include <QJsonArray>
#include <QJsonDocument>
#include <QLocalSocket>
#include <QProcess>
#include <QRectF>

#include <climits>
#include <memory>
#ifdef Q_OS_UNIX
#include <unistd.h>
#endif

#include "modulepath.h"

// ---------------------------------------------------------------- the state file

// $XDG_RUNTIME_DIR's default (there is no sodiumwm off SodiumOS: the file is never there).
static QByteArray runtimeFallback()
{
#ifdef Q_OS_UNIX
    return QByteArray("/run/user/") + QByteArray::number(getuid());
#else
    return QFile::encodeName(QDir::tempPath());
#endif
}

// ${SODIUMWM_RUNTIME_DIR:-${XDG_RUNTIME_DIR}/sodiumwm}/state.json, as the shell's Sodiumwm service reads it.
static QString defaultStatePath()
{
    const QByteArray dir = qgetenv("SODIUMWM_RUNTIME_DIR");
    if (!dir.isEmpty())
        return QFile::decodeName(dir) + QStringLiteral("/state.json");
    QByteArray runtime = qgetenv("XDG_RUNTIME_DIR");
    if (runtime.isEmpty())
        runtime = runtimeFallback();
    return QFile::decodeName(runtime) + QStringLiteral("/sodiumwm/state.json");
}

SodiumwmState *SodiumwmState::forPath(const QString &path)
{
    static QHash<QString, SodiumwmState *> states;
    SodiumwmState *&s = states[path];
    if (!s)
        s = new SodiumwmState(path);      // lives as long as the process, like ThemeStore's watcher
    return s;
}

SodiumwmState::SodiumwmState(const QString &path)
    : m_path(path)
{
    m_debounce.setSingleShot(true);
    m_debounce.setInterval(30);
    connect(&m_debounce, &QTimer::timeout, this, &SodiumwmState::load);
    connect(&m_watcher, &QFileSystemWatcher::fileChanged, this, [this] { m_debounce.start(); });
    connect(&m_watcher, &QFileSystemWatcher::directoryChanged, this, [this] { m_debounce.start(); });
    load();
}

// The file, its directory, and the nearest existing ancestor: a file sodiumwm creates
// later or replaces by a rename is still seen.
void SodiumwmState::rewatch()
{
    QStringList wanted;
    if (QFileInfo::exists(m_path))
        wanted << m_path;
    QDir d = QFileInfo(m_path).absoluteDir();
    while (!d.exists() && d.cdUp()) {}
    wanted << d.absolutePath();
    const QStringList watched = m_watcher.files() + m_watcher.directories();
    for (const QString &p : wanted)
        if (!watched.contains(p))
            m_watcher.addPath(p);
}

QDateTime SodiumwmState::hyprlandStarted()
{
    const QByteArray sig = qgetenv("HYPRLAND_INSTANCE_SIGNATURE");
    if (sig.isEmpty())
        return {};
    QByteArray runtime = qgetenv("XDG_RUNTIME_DIR");
    if (runtime.isEmpty())
        runtime = runtimeFallback();
    // Hyprland writes its lock file as it starts, before it reads its config.
    const QFileInfo lock(QFile::decodeName(runtime + "/hypr/" + sig + "/hyprland.lock"));
    if (lock.exists())
        return lock.lastModified();
    // <commit>_<unix time>_<random>
    const QList<QByteArray> parts = sig.split('_');
    bool ok = false;
    const qint64 t = parts.size() >= 3 ? parts.at(parts.size() - 2).toLongLong(&ok) : 0;
    return ok && t > 0 ? QDateTime::fromSecsSinceEpoch(t) : QDateTime();
}

bool SodiumwmState::isStale(const QDateTime &written, const QDateTime &started)
{
    // A second of slack: the signature's time has whole seconds.
    return written.isValid() && started.isValid() && written < started.addSecs(-1);
}

void SodiumwmState::markUnanswered()
{
    if (m_unanswered)
        return;
    m_unanswered = true;
    emit changed();
}

void SodiumwmState::load()
{
    rewatch();
    QFile f(m_path);
    bool present = false;
    QSet<QString> maximised, onTop;
    const QDateTime written = QFileInfo(m_path).lastModified();
    if (f.open(QIODevice::ReadOnly)) {
        QJsonParseError err{};
        const QJsonDocument doc = QJsonDocument::fromJson(f.readAll(), &err);
        if (err.error != QJsonParseError::NoError || !doc.isObject()) {
            // Half written (sodiumwm writes atomically, but be safe): keep the last state.
            if (QFileInfo::exists(m_path)) {
                m_debounce.start(100);
                return;
            }
        } else {
            present = true;
            for (const QJsonValue &v : doc.object().value(QStringLiteral("maximised")).toArray())
                maximised.insert(SodiumwmWindow::normalise(v.toString()));
            for (const QJsonValue &v : doc.object().value(QStringLiteral("onTop")).toArray())
                onTop.insert(SodiumwmWindow::normalise(v.toString()));
        }
    }
    static const QDateTime started = hyprlandStarted();
    const bool stale = present && isStale(written, started);
    // A new write means sodiumwm is there to answer again.
    const bool unanswered = m_unanswered && written == m_written;
    m_written = written;
    if (present == m_present && maximised == m_maximised && onTop == m_onTop && stale == m_stale && unanswered == m_unanswered)
        return;
    m_present = present;
    m_stale = stale;
    m_unanswered = unanswered;
    m_maximised = maximised;
    m_onTop = onTop;
    emit changed();
}

// ---------------------------------------------------------------- Hyprland's socket

// One request on Hyprland's request socket ("j/clients"); `done` gets the reply, or an
// empty one when Hyprland isn't there.
static void hyprRequest(QObject *context, const QByteArray &request, std::function<void(const QByteArray &)> done)
{
    const QByteArray sig = qgetenv("HYPRLAND_INSTANCE_SIGNATURE");
    const QByteArray runtime = qgetenv("XDG_RUNTIME_DIR");
    if (sig.isEmpty() || runtime.isEmpty()) {
        done({});
        return;
    }
    auto *sock = new QLocalSocket(context);
    auto reply = std::make_shared<QByteArray>();
    auto finished = std::make_shared<bool>(false);
    auto finish = [sock, reply, finished, done](bool ok) {
        if (*finished)
            return;
        *finished = true;
        sock->deleteLater();
        done(ok ? *reply : QByteArray());
    };
    QObject::connect(sock, &QLocalSocket::connected, sock, [sock, request] {
        sock->write(request);
        sock->flush();
    });
    QObject::connect(sock, &QLocalSocket::readyRead, sock, [sock, reply] { reply->append(sock->readAll()); });
    QObject::connect(sock, &QLocalSocket::disconnected, sock, [sock, reply, finish] {
        reply->append(sock->readAll());
        finish(true);
    });
    QObject::connect(sock, &QLocalSocket::errorOccurred, sock, [finish](QLocalSocket::LocalSocketError e) {
        if (e != QLocalSocket::PeerClosedError)
            finish(false);
    });
    sock->connectToServer(QFile::decodeName(runtime + "/hypr/" + sig + "/.socket.sock"));
}

// `hyprctl <args>` (for this process's Hyprland instance); `done` gets whether it ran and
// what it printed.
static void hyprctl(QObject *context, const QStringList &args, std::function<void(bool, const QByteArray &)> done)
{
    auto *p = new QProcess(context);
    QObject::connect(p, &QProcess::finished, p, [p, done](int code, QProcess::ExitStatus status) {
        p->deleteLater();
        done(status == QProcess::NormalExit && code == 0, p->readAllStandardOutput().trimmed());
    });
    QObject::connect(p, &QProcess::errorOccurred, p, [p, done](QProcess::ProcessError e) {
        if (e != QProcess::FailedToStart)
            return;
        p->deleteLater();
        done(false, {});
    });
    p->start(QStringLiteral("hyprctl"), args);
}

// ---------------------------------------------------------------- SodiumwmWindow

static QString moduleDir()
{
    return sodiumuiModuleDir();
}

static QString defaultShellPath()
{
    const QByteArray env = qgetenv("SODIUMSHELL_DIR");
    if (!env.isEmpty())
        return QFile::decodeName(env);
    // ui/SodiumUI/libsodiumuinative.so -> <repo>/shell
    const QString repoShell = QDir::cleanPath(moduleDir() + QStringLiteral("/../../shell"));
    if (QFileInfo::exists(repoShell + QStringLiteral("/shell.qml")))
        return repoShell;
    return QDir::homePath() + QStringLiteral("/Code/SodiumOS/shell");
}

SodiumwmWindow::SodiumwmWindow(QObject *parent)
    : QObject(parent)
    , m_state(SodiumwmState::forPath(defaultStatePath()))
    , m_shellPath(defaultShellPath())
{
    m_running = running();
    connect(m_state, &SodiumwmState::changed, this, &SodiumwmWindow::stateChanged);
    // A window maps a moment after it is shown; look again a few times until Hyprland lists it.
    m_retry.setInterval(300);
    connect(&m_retry, &QTimer::timeout, this, [this] {
        if (!m_address.isEmpty() || ++m_retries > 10)
            m_retry.stop();
        else
            resolve();
    });
}

bool SodiumwmWindow::running() const
{
    return m_state->present() && !qEnvironmentVariableIsEmpty("HYPRLAND_INSTANCE_SIGNATURE");
}

void SodiumwmWindow::stateChanged()
{
    const bool run = running();
    if (run != m_running) {
        m_running = run;
        emit runningChanged();
        if (run && m_address.isEmpty())
            resolve();
    }
    const bool max = maximised();
    if (max != m_maximised) {
        m_maximised = max;
        emit maximisedChanged();
    }
    const bool top = onTop();
    if (top != m_onTop) {
        m_onTop = top;
        emit onTopChanged();
    }
}

void SodiumwmWindow::setWindow(QWindow *w)
{
    if (m_window == w)
        return;
    if (m_window)
        m_window->disconnect(this);
    m_window = w;
    if (w) {
        // Active: Hyprland's focused window is this one, so the lookup is exact.
        connect(w, &QWindow::activeChanged, this, [this] {
            if (m_window && m_window->isActive())
                resolve();
        });
        connect(w, &QWindow::visibleChanged, this, [this] {
            if (m_window && m_window->isVisible()) {
                m_retries = 0;
                m_retry.start();
            }
        });
        if (w->isVisible()) {
            m_retries = 0;
            m_retry.start();
        }
    }
    emit windowChanged();
}

void SodiumwmWindow::setAddress(const QString &a)
{
    const QString n = normalise(a);
    if (n == m_address)
        return;
    m_address = n;
    emit addressChanged();
    stateChanged();
}

void SodiumwmWindow::setShellPath(const QString &p)
{
    if (p == m_shellPath)
        return;
    m_shellPath = p;
    emit shellPathChanged();
}

QString SodiumwmWindow::normalise(const QString &address)
{
    QString s = address.trimmed().toLower();
    if (s.startsWith(QLatin1String("0x")))
        s = s.mid(2);
    return s.isEmpty() ? QString() : QStringLiteral("0x") + s;
}

QJsonObject SodiumwmWindow::pick(const QJsonArray &clients, qint64 pid, bool active, const QPointF *cursor,
                              const QString &title, const QSize &size)
{
    QList<QJsonObject> mine;
    for (const QJsonValue &v : clients) {
        const QJsonObject o = v.toObject();
        if (o.value(QStringLiteral("pid")).toInteger(-1) == pid && o.value(QStringLiteral("mapped")).toBool(true))
            mine << o;
    }
    if (mine.size() <= 1)
        return mine.isEmpty() ? QJsonObject() : mine.first();

    const auto history = [](const QJsonObject &o) {
        const QJsonValue h = o.value(QStringLiteral("focusHistoryID"));
        return h.isDouble() && h.toInt() >= 0 ? h.toInt() : INT_MAX;
    };
    const auto mostRecent = [&history](const QList<QJsonObject> &list) {
        QJsonObject best;
        int bestH = INT_MAX;
        for (const QJsonObject &o : list)
            if (best.isEmpty() || history(o) < bestH) {
                best = o;
                bestH = history(o);
            }
        return best;
    };
    const auto rectOf = [](const QJsonObject &o) {
        const QJsonArray at = o.value(QStringLiteral("at")).toArray();
        const QJsonArray sz = o.value(QStringLiteral("size")).toArray();
        return QRectF(at.at(0).toDouble(), at.at(1).toDouble(), sz.at(0).toDouble(), sz.at(1).toDouble());
    };

    // The title narrows it first: Qt's idea of "active" or of the pointer can lag
    // Hyprland's by an event, but a window's title is its own.
    QList<QJsonObject> titled;
    for (const QJsonObject &o : mine)
        if (o.value(QStringLiteral("title")).toString() == title)
            titled << o;
    const QList<QJsonObject> pool = titled.isEmpty() ? mine : titled;
    if (pool.size() == 1)
        return pool.first();

    if (active)
        for (const QJsonObject &o : pool)
            if (history(o) == 0)
                return o;
    if (cursor) {
        QList<QJsonObject> under;
        for (const QJsonObject &o : pool)
            if (rectOf(o).contains(*cursor))
                under << o;
        if (!under.isEmpty())
            return mostRecent(under);
    }
    QList<QJsonObject> sized;
    for (const QJsonObject &o : titled) {
        const QRectF r = rectOf(o);
        if (qAbs(r.width() - size.width()) <= 2 && qAbs(r.height() - size.height()) <= 2)
            sized << o;
    }
    if (sized.size() == 1)
        return sized.first();
    return titled.isEmpty() ? QJsonObject() : mostRecent(titled);
}

void SodiumwmWindow::withClient(bool pointer, std::function<void(const QJsonObject &)> done)
{
    hyprRequest(this, QByteArrayLiteral("j/clients"), [this, pointer, done](const QByteArray &data) {
        const QJsonArray clients = QJsonDocument::fromJson(data).array();
        const qint64 pid = QCoreApplication::applicationPid();
        const auto finish = [this, clients, pid, done](const QPointF *cursor) {
            const QJsonObject c = pick(clients, pid, m_window && m_window->isActive(), cursor,
                                       m_window ? m_window->title() : QString(), m_window ? m_window->size() : QSize());
            if (!c.isEmpty())
                setAddress(c.value(QStringLiteral("address")).toString());
            done(c);
        };
        int mine = 0;
        for (const QJsonValue &v : clients)
            if (v.toObject().value(QStringLiteral("pid")).toInteger(-1) == pid)
                ++mine;
        if (!pointer || mine <= 1) {
            finish(nullptr);
            return;
        }
        hyprRequest(this, QByteArrayLiteral("j/cursorpos"), [finish](const QByteArray &pos) {
            const QJsonObject o = QJsonDocument::fromJson(pos).object();
            if (!o.contains(QStringLiteral("x"))) {
                finish(nullptr);
                return;
            }
            const QPointF p(o.value(QStringLiteral("x")).toDouble(), o.value(QStringLiteral("y")).toDouble());
            finish(&p);
        });
    });
}

void SodiumwmWindow::resolve()
{
    if (!running() || !m_window)
        return;
    withClient(false, [](const QJsonObject &) {});
}

bool SodiumwmWindow::toggleMaximise()
{
    // Not under Hyprland: the toplevel state (WindowControls).
    if (qEnvironmentVariableIsEmpty("HYPRLAND_INSTANCE_SIGNATURE"))
        return false;
    withClient(true, [this](const QJsonObject &c) {
        if (c.isEmpty()) {
            emit fallbackNeeded();
            return;
        }
        if (!running()) {
            // No sodiumwm (or a stale state file): Hyprland's own maximise. Qt's request would do
            // nothing on a tiled window, which Hyprland already calls maximised.
            hyprlandMaximise(c);
            return;
        }
        const QString address = normalise(c.value(QStringLiteral("address")).toString());
        QPointer<SodiumwmWindow> self(this);
        hyprctl(this, {QStringLiteral("repl"), QStringLiteral("return sodiumwm.api.toggleMaximise(\"%1\")").arg(address)},
                [self, c](bool ok, const QByteArray &out) {
                    if (!self || (ok && out.contains("\"ok\":true")))
                        return;
                    // sodiumwm isn't there after all (it failed to load, or a reload dropped it).
                    qWarning("SodiumUI: sodiumwm didn't answer toggleMaximise (%s); using Hyprland's maximise",
                             out.left(200).constData());
                    self->m_state->markUnanswered();
                    self->hyprlandMaximise(c);
                });
    });
    return true;
}

bool SodiumwmWindow::toggleOnTop()
{
    if (!running())
        return false;
    withClient(true, [this](const QJsonObject &c) {
        if (c.isEmpty() || !running())
            return;
        const QString address = normalise(c.value(QStringLiteral("address")).toString());
        const bool on = !m_state->isOnTop(address);
        QPointer<SodiumwmWindow> self(this);
        // the pin follows state.json, which sodiumwm rewrites after the call
        hyprctl(this, {QStringLiteral("repl"), QStringLiteral("return sodiumwm.api.keepOnTop(\"%1\", %2)").arg(address, on ? QStringLiteral("true") : QStringLiteral("false"))},
                [self](bool ok, const QByteArray &out) {
                    if (!self || (ok && out.contains("\"ok\":true")))
                        return;
                    qWarning("SodiumUI: sodiumwm didn't answer keepOnTop (%s)", out.left(200).constData());
                    self->m_state->markUnanswered();
                });
    });
    return true;
}

void SodiumwmWindow::hyprlandMaximise(const QJsonObject &client)
{
    const QString address = normalise(client.value(QStringLiteral("address")).toString());
    // fullscreen 1 is Hyprland's maximise (decision 1 in phase3-sodiumwm.md).
    const bool maximised = client.value(QStringLiteral("fullscreen")).toInt() == 1;
    const QString dispatch = QStringLiteral("hl.dsp.window.fullscreen({ mode = \"maximized\", action = \"%1\", window = \"address:%2\" })")
                                 .arg(maximised ? QStringLiteral("unset") : QStringLiteral("set"), address);
    QPointer<SodiumwmWindow> self(this);
    hyprctl(this, {QStringLiteral("dispatch"), dispatch}, [self](bool ok, const QByteArray &out) {
        if (!self || (ok && out == "ok"))
            return;
        qWarning("SodiumUI: Hyprland refused to maximise (%s)", out.left(200).constData());
        emit self->fallbackNeeded();
    });
}

void SodiumwmWindow::shellCall(const QString &target, const QString &function, qreal x, qreal y)
{
    withClient(true, [this, target, function, x, y](const QJsonObject &c) {
        if (c.isEmpty())
            return;
        const QJsonArray at = c.value(QStringLiteral("at")).toArray();
        const int gx = qRound(at.at(0).toDouble() + x);
        const int gy = qRound(at.at(1).toDouble() + y);
        QProcess::startDetached(QStringLiteral("qs"),
                                {QStringLiteral("-p"), m_shellPath, QStringLiteral("ipc"), QStringLiteral("call"), target, function,
                                 normalise(c.value(QStringLiteral("address")).toString()),
                                 QString::number(gx), QString::number(gy)});
    });
}

void SodiumwmWindow::showSnapGrid(qreal x, qreal y)
{
    if (!running())
        return;
    shellCall(QStringLiteral("snap"), QStringLiteral("grid"), x, y);
}

void SodiumwmWindow::showWindowMenu(qreal x, qreal y)
{
    // The menu is the shell's (it works on any Hyprland window), so only Hyprland is needed.
    if (qEnvironmentVariableIsEmpty("HYPRLAND_INSTANCE_SIGNATURE"))
        return;
    shellCall(QStringLiteral("windowmenu"), QStringLiteral("open"), x, y);
}
