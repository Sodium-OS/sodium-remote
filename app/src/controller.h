// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// The Remote app's controller: wraps the Rust engine (remote_qt.h), marshals its JSON events onto the Qt thread, and
// exposes models, properties and slots to the QML. It never stores translated text: states are ids.
#pragma once
#include "models.h"
#include "remote_qt.h"

#include <QJsonArray>
#include <QJsonObject>
#include <QObject>
#include <QRect>
#include <QSize>
#include <QTimer>
#include <QUrl>
#include <QVariantMap>

class QWindow;
class StreamFrame;

class RemoteController : public QObject
{
    Q_OBJECT
    Q_PROPERTY(ComputersModel *computers READ computers CONSTANT)
    Q_PROPERTY(NearbyModel *nearby READ nearby CONSTANT)
    Q_PROPERTY(DisplaysModel *displays READ displays CONSTANT)
    Q_PROPERTY(QString selfCode READ selfCode NOTIFY selfChanged)
    Q_PROPERTY(QString deviceName READ deviceName CONSTANT)
    Q_PROPERTY(bool relayOnline READ relayOnline NOTIFY relayChanged)

    // pairing: {state: idle|checking|wrong|withdrawn|closed|failed|ok, host, name, left}
    Q_PROPERTY(QVariantMap pairing READ pairing NOTIFY pairingChanged)
    // connect by address: {state: idle|checking|found|unreachable, addr, host, code, name}
    Q_PROPERTY(QVariantMap address READ address NOTIFY addressChanged)

    // the session
    Q_PROPERTY(QString sessionState READ sessionState NOTIFY sessionChanged)   // idle connecting connected streaming reconnecting failed ended
    Q_PROPERTY(QString sessionHost READ sessionHost NOTIFY sessionChanged)     // the host id being connected to
    Q_PROPERTY(QVariantMap failure READ failure NOTIFY sessionChanged)         // {kind, by, host, client, detail}, empty unless failed or ended
    Q_PROPERTY(QString hostName READ hostName NOTIFY hostChanged)
    Q_PROPERTY(QString hostOs READ hostOs NOTIFY hostChanged)
    Q_PROPERTY(bool viewOnly READ viewOnly NOTIFY hostChanged)                 // Welcome.view_only: the Mac has not allowed Accessibility
    Q_PROPERTY(bool physicalDisplays READ physicalDisplays NOTIFY hostChanged)
    Q_PROPERTY(bool liveResize READ liveResize NOTIFY hostChanged)
    Q_PROPERTY(QString route READ route NOTIFY routeChanged)                   // lan direct vpn tailscale relay "" and "lost"
    Q_PROPERTY(double rttMs READ rttMs NOTIFY routeChanged)
    Q_PROPERTY(bool secureScreen READ secureScreen NOTIFY secureChanged)
    Q_PROPERTY(bool capture READ capture NOTIFY captureChanged)
    Q_PROPERTY(QVariantMap stats READ stats NOTIFY statsChanged)
    Q_PROPERTY(QVariantMap decodeSupport READ decodeSupport CONSTANT)

    // settings (kept in app.json; applied at the next connection)
    Q_PROPERTY(QString codec READ codec WRITE setCodec NOTIFY settingsChanged)             // auto hevc h264
    Q_PROPERTY(double bandwidthMbps READ bandwidthMbps WRITE setBandwidthMbps NOTIFY settingsChanged) // 0 is Automatic
    Q_PROPERTY(QString keyMapping READ keyMapping WRITE setKeyMapping NOTIFY settingsChanged)         // auto pass
    Q_PROPERTY(bool fixedResolution READ fixedResolution WRITE setFixedResolution NOTIFY settingsChanged)
    Q_PROPERTY(int refreshHz READ refreshHz WRITE setRefreshHz NOTIFY settingsChanged)                // 0 is the display's rate
    Q_PROPERTY(bool showPreviews READ showPreviews WRITE setShowPreviews NOTIFY settingsChanged)     // the computers' pictures; off deletes them

    // Settings › Server: {configured, url, host, label, official, pinnedCa, token, error}; serverState none|checking|online|offline;
    // serverProblem is "" or the apps' own reading of the server's refusal: tokenInvalid tokenExpired tokenNeeded rateLimited quotaExceeded paymentRequired
    Q_PROPERTY(QVariantMap server READ server NOTIFY serverChanged)
    Q_PROPERTY(bool serverConfigured READ serverConfigured NOTIFY serverChanged)
    Q_PROPERTY(QString serverState READ serverState NOTIFY serverChanged)
    Q_PROPERTY(QString serverProblem READ serverProblem NOTIFY serverChanged)
    Q_PROPERTY(bool serverWaitsForSession READ serverWaitsForSession NOTIFY serverChanged)

    // About (AGPL section 13): the release, its license and where the source is; the notices file shipped beside the program
    Q_PROPERTY(QString appVersion READ appVersion CONSTANT)
    Q_PROPERTY(QString licenseId READ licenseId CONSTANT)
    Q_PROPERTY(QString sourceUrl READ sourceUrl CONSTANT)
    Q_PROPERTY(QString noticesPath READ noticesPath CONSTANT)

public:
    /// `engine` false builds the controller without the Rust engine (tests feed it events through `handleEvent`).
    explicit RemoteController(const QString &dataDir, bool rendezvous, bool engine = true, QObject *parent = nullptr);
    ~RemoteController() override;

    ComputersModel *computers() { return &m_computers; }
    NearbyModel *nearby() { return &m_nearby; }
    DisplaysModel *displays() { return &m_displays; }
    QString selfCode() const { return m_selfCode; }
    QString deviceName() const { return m_deviceName; }
    bool relayOnline() const { return m_relayOnline; }
    QVariantMap pairing() const { return m_pairing; }
    QVariantMap address() const { return m_address; }
    QString sessionState() const { return m_state; }
    QString sessionHost() const { return m_sessionHost; }
    QVariantMap failure() const { return m_failure; }
    QString hostName() const { return m_hostName; }
    QString hostOs() const { return m_hostOs; }
    bool viewOnly() const { return m_viewOnly; }
    bool physicalDisplays() const { return m_physical; }
    bool liveResize() const { return m_liveResize; }
    QString route() const { return m_route; }
    double rttMs() const { return m_rtt; }
    bool secureScreen() const { return m_secure; }
    bool capture() const { return m_capture; }
    QVariantMap stats() const { return m_stats; }
    QVariantMap decodeSupport() const { return m_decode; }

    QString codec() const { return m_codec; }
    void setCodec(const QString &c);
    double bandwidthMbps() const { return m_bandwidth; }
    void setBandwidthMbps(double v);
    QString keyMapping() const { return m_keyMapping; }
    void setKeyMapping(const QString &m);
    bool fixedResolution() const { return m_fixed; }
    void setFixedResolution(bool f);
    int refreshHz() const { return m_refresh; }
    void setRefreshHz(int hz);
    bool showPreviews() const { return m_previews; }
    void setShowPreviews(bool on);
    QVariantMap server() const { return m_server; }
    bool serverConfigured() const { return m_server.value(QStringLiteral("configured")).toBool(); }
    QString serverState() const { return m_serverState; }
    QString serverProblem() const { return m_serverProblem; }
    bool serverWaitsForSession() const { return m_serverWaits; }
    QString appVersion() const;
    QString licenseId() const;
    QString sourceUrl() const;
    /// The THIRD-PARTY-NOTICES file next to the program, or "" when there is none.
    QString noticesPath() const;

    /// One engine event (also what the tests feed in).
    void handleEvent(const QJsonObject &ev);

public Q_SLOTS:
    void refresh();
    void pair(const QString &hostId, const QString &code, const QString &addr = QString());
    void resetPairing();
    void resolveAddress(const QString &text);
    void resetAddress();
    void forget(const QString &hostId);
    /// A name of your own for a computer ("" goes back to the computer's own).
    void rename(const QString &hostId, const QString &name);
    /// The notices file in the system's viewer, the source repository in the browser. False when there is nothing to open.
    bool openNotices();
    /// The notices file's text ("" when missing), for the sheet's list.
    QString noticesText() const;
    void openSource();

    // ---- Settings › Server. The checks return {ok, url, host, label, official, needs_ca_prompt, token, link} or {ok: false, error}
    // (error: insecure, address, certificate, none, file or the core's own text); nothing is saved before saveServer, which the
    // sheets call after typed input or after the confirmation of a link or a file.
    QVariantMap checkServerLink(const QString &text);
    QVariantMap checkServer(const QString &address, const QString &caPem, const QString &token);
    QVariantMap readServerFile(const QUrl &file);
    /// {ok, name, pem} for a certificate authority file (the sheet's chooser); `ok` is false when it can't be read.
    QVariantMap readCertificate(const QUrl &file);
    /// The saved server's CA as PEM ("" for none), for Change….
    QString serverCertificate() const;
    /// Saves a candidate's `link` as server.toml and starts using it. {ok, error}.
    QVariantMap saveServer(const QString &link);
    QVariantMap removeServer();
    QVariantMap exportServer(const QUrl &file);
    /// Try the server again now (Try now).
    void retryServer();
    void openSelfHosting();
    /// A `sodium-remote://server?...` link from outside the app (a click, the command line): the Server tab shows its confirmation.
    void openServerLink(const QString &link);
    /// The link opened before the window was up ("" for none).
    QString takePendingServerLink();

    /// Where the stream window opens: `want` (logical pixels) shrunk to at most 90% of `avail` (a screen's available
    /// geometry), centred on it, so it never opens larger than the screen or partly outside it.
    static QRect fitWindow(const QRect &avail, const QSize &want);
    /// The same for the screen `anchor` (the main window) is on, as {x, y, width, height}.
    Q_INVOKABLE QVariantMap fittedWindowRect(QWindow *anchor, int width, int height) const;
    /// Places the stream window (logical pixels); on Windows its frame follows.
    Q_INVOKABLE void placeStreamWindow(QWindow *window, int x, int y, int width, int height);

    /// Starts a session whose picture goes under `window` (the stream window).
    /// `videoY`: where the picture starts in the window (below the title bar; 0 in full screen).
    bool connectTo(const QString &hostId, QWindow *window, bool takeover, bool fullscreen, int videoY = 0);
    void disconnectSession();
    /// Forgets a finished or failed session (the stream window closed).
    void clearSession();
    void updateView(int x, int y, int w, int h, qreal scale, bool fullscreen);
    void setHudRects(const QVariantList &rects);
    void setCapture(bool on);
    /// A menu of the app is open over the picture: no input reaches the computer until it closes (Windows).
    void setMenuBlock(bool on);
    void selectDisplay(int id);
    void sendCtrlAltDel();
    void pollStats();
    /// Screenshots and tests only (--script): one engine event as JSON, as if the engine had sent it.
    void injectEvent(const QString &json);
    /// Screenshots and tests only: stops the periodic refresh, so an injected list stays.
    void freezeRefresh() { m_refreshTimer.stop(); }

Q_SIGNALS:
    void selfChanged();
    void relayChanged();
    void serverChanged();
    void pairingChanged();
    void addressChanged();
    void sessionChanged();
    void hostChanged();
    void routeChanged();
    void secureChanged();
    void captureChanged();
    void statsChanged();
    void settingsChanged();
    /// The local leave chord (Ctrl+Alt+Shift+Esc) was pressed.
    void leaveChord();
    /// A press in the video or the window losing the foreground while a menu is open (Windows): the menu closes.
    void menuCloseRequested();
    /// The first picture of the stream is up.
    void streamStarted();
    /// A server was saved or removed (the host half restarts to use it).
    void serverSaved();
    /// A server link arrived from outside; the window shows Settings › Server with its confirmation.
    void serverLinkRequested(const QString &link);
    /// Show previews was just turned off and the saved pictures were deleted (a toast says so).
    void previewsTurnedOff();

private:
    void loadServer();
    void applyServer();
    QVariantMap serverReply(char *json) const;
    void onHosts(const QJsonObject &ev);
    void onStatus(const QJsonObject &ev);
    void onWelcome(const QJsonObject &ev);
    void setState(const QString &state, const QVariantMap &failure = {});
    void loadApp();
    void saveApp();
    QVariantMap hostInfo(const QString &id) const;
    /// Where the picture of the last session with a computer is kept (the engine writes it), and its file URL.
    QString thumbPath(const QString &hostId) const;
    QString thumbUrl(const QString &hostId) const;
    void pruneThumbs(const QJsonArray &paired);
    void setHostInfo(const QString &id, const QString &key, const QVariant &value);
    static void trampoline(void *user, const char *json);

    RqHandle *m_rq = nullptr;
    /// Windows: the opaque frame the picture is a child window of (the stream window is the HUD above it).
    StreamFrame *m_frame = nullptr;
    QString m_dataDir;
    QString m_deviceName;
    ComputersModel m_computers;
    NearbyModel m_nearby;
    DisplaysModel m_displays;
    QString m_selfCode;
    bool m_relayOnline = false;
    QVariantMap m_pairing{{QStringLiteral("state"), QStringLiteral("idle")}};
    QVariantMap m_address{{QStringLiteral("state"), QStringLiteral("idle")}};
    QString m_state = QStringLiteral("idle");
    QString m_sessionHost;
    QVariantMap m_failure;
    QString m_hostName, m_hostOs, m_route;
    bool m_viewOnly = false, m_physical = false, m_liveResize = true, m_secure = false, m_capture = true;
    double m_rtt = 0;
    QVariantMap m_stats;
    QVariantMap m_decode;
    QString m_codec = QStringLiteral("auto");
    double m_bandwidth = 0;
    QString m_keyMapping = QStringLiteral("auto");
    bool m_fixed = false;
    int m_refresh = 0;
    bool m_previews = true;
    /// Per computer, learned: own name, os, last route and round trip. (app.json)
    QVariantMap m_hosts;
    QJsonObject m_lastHosts;
    QTimer m_refreshTimer;
    bool m_useServer = true;
    QVariantMap m_server;
    QString m_serverState = QStringLiteral("none");
    QString m_serverProblem;
    bool m_serverWaits = false;
    QString m_pendingLink;
};
