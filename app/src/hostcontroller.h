// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// "This computer" (Windows, and generic Linux with REMOTE_LINUX_HOST): the in-process host (remote/client-qt `rh_*`, host-desktop's Session) as properties and
// slots for the QML: the switch, the pairing code, the paired devices, the name shown, start at login, the firewall
// and network state, and who is controlling this computer. It never stores translated text.
#pragma once
#include <QDateTime>
#include <QJsonObject>
#include <QObject>
#include <QTimer>
#include <QVariantList>
#include <QVariantMap>

typedef struct RhHandle RhHandle;

class HostController : public QObject
{
    Q_OBJECT
    Q_PROPERTY(bool enabled READ enabled NOTIFY stateChanged)                 // the switch (what the user chose)
    Q_PROPERTY(QString state READ state NOTIFY stateChanged)                  // off starting on error
    Q_PROPERTY(QString error READ error NOTIFY stateChanged)
    Q_PROPERTY(QString errorCode READ errorCode NOTIFY stateChanged)          // "" or in_use (another host serves from this data directory)
    Q_PROPERTY(QString name READ name NOTIFY nameChanged)                     // shown to other computers
    Q_PROPERTY(QString defaultName READ defaultName CONSTANT)
    Q_PROPERTY(bool startAtLogin READ startAtLogin NOTIFY loginChanged)
    Q_PROPERTY(QVariantMap pairing READ pairing NOTIFY pairingChanged)        // {state: closed|open|attempt|wrong, code, left, expiresAt}
    Q_PROPERTY(int pairingSecondsLeft READ pairingSecondsLeft NOTIFY tick)
    Q_PROPERTY(QVariantList paired READ paired NOTIFY pairedChanged)          // [{id, code, name, os, pairedAt, lastConnected, live}]
    Q_PROPERTY(bool controlled READ controlled NOTIFY controlChanged)
    Q_PROPERTY(QVariantMap controller READ controller NOTIFY controlChanged)  // {id, name, os, since, secure, held}
    Q_PROPERTY(QString address READ address NOTIFY stateChanged)              // ID@IP:PORT, for "connect by address"
    Q_PROPERTY(QString ipAddress READ ipAddress NOTIFY stateChanged)          // IP:PORT alone (core pairs by a bare address too)
    Q_PROPERTY(QStringList ipAddresses READ ipAddresses NOTIFY stateChanged)  // every IP:PORT (the "..." menu lists them when there are several)
    Q_PROPERTY(QString firewall READ firewall NOTIFY firewallChanged)         // ok blocked wait
    Q_PROPERTY(bool networkPublic READ networkPublic NOTIFY firewallChanged)  // every connected network is Public
    // Software video: Cisco's OpenH264 is fetched once when no hardware encoder exists. softwareVideo is the Advanced switch;
    // codec is {state: ""|needed|downloading|ready|failed|off_blocked, percent}.
    Q_PROPERTY(bool softwareVideo READ softwareVideo WRITE setSoftwareVideo NOTIFY codecChanged)
    Q_PROPERTY(QVariantMap codec READ codec NOTIFY codecChanged)
    // Linux: the desktop's screen-sharing permission (the portal). consent is ask asking remembered declined unavailable
    // (the engine's host_consent states); "" on Windows. desktop is "GNOME · Wayland" style text from the environment (a name, not a sentence).
    Q_PROPERTY(QString consent READ consent NOTIFY consentChanged)
    Q_PROPERTY(QString desktop READ desktop CONSTANT)
    // desktopId is "gnome", "kde" or "other" (machine-readable; the pane's limits text depends on it); "" on Windows.
    Q_PROPERTY(QString desktopId READ desktopId CONSTANT)

public:
    HostController(const QString &dataDir, QObject *parent = nullptr);
    ~HostController() override;

    bool enabled() const { return m_enabled; }
    QString state() const { return m_state; }
    QString error() const { return m_error; }
    QString errorCode() const { return m_errorCode; }
    QString name() const { return m_name.isEmpty() ? defaultName() : m_name; }
    QString defaultName() const;
    bool startAtLogin() const;
    QVariantMap pairing() const { return m_pairing; }
    int pairingSecondsLeft() const;
    QVariantList paired() const { return m_paired; }
    bool controlled() const { return m_controlled; }
    QVariantMap controller() const { return m_controller; }
    QString address() const { return m_address; }
    QString ipAddress() const { return m_ip; }
    QStringList ipAddresses() const { return m_ipPorts; }
    QString firewall() const { return m_firewall; }
    bool networkPublic() const { return m_public; }

    bool softwareVideo() const;
    void setSoftwareVideo(bool on);
    QVariantMap codec() const { return m_codec; }

    QString consent() const { return m_consent; }
    QString desktop() const;
    QString desktopId() const;
    /// The id for a colon-separated XDG_CURRENT_DESKTOP (case-insensitive).
    static QString desktopIdFor(const QString &currentDesktop);
    /// The engine's consent states; anything else is "ask".
    static QString consentFromEvent(const QJsonObject &ev);

    void handleEvent(const QJsonObject &ev);

public Q_SLOTS:
    void setEnabled(bool on);
    void setName(const QString &name);
    void setStartAtLogin(bool on);
    void openPairing();
    void closePairing();
    void disconnectClient();
    void revoke(const QString &id);
    void allowFirewall();
    void openNetworkSettings();
    void copyAddress(bool full = false);
    /// Opens the Linux host guide (docs/hosts/linux.md) in the browser.
    void openLinuxGuide();
    /// Copies one of `ipAddresses`.
    void copyIpAddress(const QString &ipPort);
    void refresh();
    /// After a failed download: try again.
    void retryCodec();
    /// The server changed: restart hosting to use it, now when nobody is controlling this computer, else when that ends.
    void restartForServer();
    /// Linux: forgets the saved permission; the desktop asks again at the next connection.
    void forgetConsent();
    /// Screenshots and tests (--script `host-event`): handle an engine event as if it had come from the host.
    void injectEvent(const QString &json);

Q_SIGNALS:
    void consentChanged();
    void codecChanged();
    /// The download started for the first time ever: the one-time notice.
    void codecNoticeRequested();
    void stateChanged();
    void nameChanged();
    void loginChanged();
    void pairingChanged();
    void pairedChanged();
    void controlChanged();
    void firewallChanged();
    void tick();
    /// A device started or ended controlling this computer (for the notice and the tray).
    void controlStarted(const QVariantMap &controller);
    void controlEnded(const QString &name);

private:
    static void trampoline(void *user, const char *json);
    void loadApp();
    void saveApp();
    void pollFirewall();
    void refreshPaired();
    void loadConsent();
    void setConsent(const QString &state);
    bool portalAvailable(QString *why) const;

    QString m_consent;
    RhHandle *m_rh = nullptr;
    QString m_dataDir;
    bool m_enabled = false;
    QString m_state = QStringLiteral("off");
    QString m_error;
    QString m_errorCode;
    QString m_name;
    QVariantMap m_pairing{{QStringLiteral("state"), QStringLiteral("closed")}};
    QDateTime m_pairingExpires;
    QVariantList m_paired;
    bool m_controlled = false;
    QVariantMap m_controller;
    QString m_address, m_ip;
    QStringList m_addresses, m_ipPorts;
    QString m_firewall = QStringLiteral("blocked");
    bool m_firewallReading = false;
    bool m_public = false;
    QVariantMap m_codec{{QStringLiteral("state"), QString()}, {QStringLiteral("percent"), 0}};
    QTimer m_tick;
    QTimer m_netTimer;
    QString m_lastControllerName;
    bool m_serverPending = false;
};
