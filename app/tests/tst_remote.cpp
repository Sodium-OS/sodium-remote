// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// The models and the controller's event mapping, with no engine and no window.
#include "controller.h"
#include "firewallrules.h"
#ifdef REMOTE_LINUX_HOST
#include "hostcontroller.h"
#endif

#include <QDir>
#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QtTest>

static QJsonObject parse(const char *json)
{
    return QJsonDocument::fromJson(json).object();
}

class TstRemote : public QObject
{
    Q_OBJECT
private Q_SLOTS:
    void modelKeepsRowsInPlace();
    void hostsEventFillsTheModels();
    void renameWinsOverTheHostsOwnName();
    void pairingStates();
    void connectFailures();
    void welcomeFillsDisplays();
    void userDisconnectIsNotACard();
    void anUnpairedComputersThumbnailIsDeleted();
    void turningPreviewsOffDeletesThePicturesAndKeepsTheChoice();
    void aVersionMismatchSaysWhichSideIsOlder();
    void aLinkShowsWhatItSaysBeforeAnythingIsSaved();
    void typedAddressesAreCheckedAndPlainTextIsRefused();
    void aServerIsSavedExportedImportedAndRemoved();
    void withNoServerNoRelayStateIsShown();
    void aServersRefusalIsReadFromItsCodeNeverItsText();
    void theStreamWindowFitsTheScreenItOpensOn();
    void theFirewallAllowsTheProgramOnPrivateUnlessABlockRuleNamesIt();
#ifdef REMOTE_LINUX_HOST
    void theDesktopsPermissionStatesMapToThePanesState();
    void forgettingThePermissionDeletesTheSavedChoice();
#endif
};

// Settings › Server reads and writes the user's server.toml: point it at a folder of the test's own.
static QTemporaryDir g_config;
static const QString kLink = QStringLiteral("sodium-remote://server?url=wss%3A%2F%2Frelay.example.com%3A8443%2Fremote&token=t0k");
// The same with a certificate of its own (base64url of the bytes 1, 2, 3).
static const QString kLinkWithCa = kLink + QStringLiteral("&ca=AQID");

void TstRemote::modelKeepsRowsInPlace()
{
    ComputersModel m;
    QSignalSpy reset(&m, &QAbstractItemModel::modelReset);
    QSignalSpy changed(&m, &QAbstractItemModel::dataChanged);
    m.setRows({{{"hostId", "a"}, {"name", "A"}}, {{"hostId", "b"}, {"name", "B"}}});
    QCOMPARE(m.rowCount(), 2);
    m.setRows({{{"hostId", "b"}, {"name", "B"}}, {{"hostId", "a"}, {"name", "A2"}}, {{"hostId", "c"}, {"name", "C"}}});
    QCOMPARE(m.at(0).value("hostId").toString(), QStringLiteral("b"));
    QCOMPARE(m.get("a").value("name").toString(), QStringLiteral("A2"));
    QCOMPARE(m.rowCount(), 3);
    QCOMPARE(reset.count(), 0);
    QVERIFY(changed.count() >= 1);
    m.setRows({{{"hostId", "c"}, {"name", "C"}}});
    QCOMPARE(m.rowCount(), 1);
}

void TstRemote::hostsEventFillsTheModels()
{
    QTemporaryDir dir;
    RemoteController c(dir.path(), false, false);
    c.handleEvent(parse(R"({"ev":"hosts","paired":[
        {"id":"H1","code":"AAAA-BBBB","name":"Studio PC","paired_at":1,"last_connected":1700000000,"online":true,"on_lan":true,"last_seen":null,"asleep_since":null},
        {"id":"H2","code":"CCCC-DDDD","name":"Mac mini","paired_at":1,"last_connected":null,"online":false,"on_lan":false,"last_seen":1699990000,"asleep_since":1699999000}],
        "found":[{"id":"H3","code":"EEEE-FFFF","name":"Lab","addrs":["192.0.2.5"],"port":4000,"rendezvous":false}]})"));
    QCOMPARE(c.computers()->rowCount(), 2);
    QCOMPARE(c.computers()->get("H1").value("state").toString(), QStringLiteral("online"));
    QCOMPARE(c.computers()->get("H2").value("state").toString(), QStringLiteral("asleep"));
    QCOMPARE(c.nearby()->get("H3").value("port").toInt(), 4000);
}

void TstRemote::renameWinsOverTheHostsOwnName()
{
    QTemporaryDir dir;
    RemoteController c(dir.path(), false, false);
    c.handleEvent(parse(R"({"ev":"hosts","paired":[{"id":"H1","code":"X","name":"Studio PC","online":true}],"found":[]})"));
    c.rename("H1", "Desk");
    QCOMPARE(c.computers()->get("H1").value("name").toString(), QStringLiteral("Desk"));
    c.rename("H1", "");
    QCOMPARE(c.computers()->get("H1").value("name").toString(), QStringLiteral("Studio PC"));
}

void TstRemote::pairingStates()
{
    QTemporaryDir dir;
    RemoteController c(dir.path(), false, false);
    QSignalSpy spy(&c, &RemoteController::pairingChanged);
    c.handleEvent(parse(R"({"ev":"pair","state":"wrong","host":"H1","left":2})"));
    QCOMPARE(c.pairing().value("state").toString(), QStringLiteral("wrong"));
    QCOMPARE(c.pairing().value("left").toInt(), 2);
    c.handleEvent(parse(R"({"ev":"pair","state":"ok","host":"H1","name":"Studio PC"})"));
    QCOMPARE(c.pairing().value("name").toString(), QStringLiteral("Studio PC"));
    QCOMPARE(spy.count(), 2);
}

void TstRemote::connectFailures()
{
    QTemporaryDir dir;
    RemoteController c(dir.path(), false, false);
    c.handleEvent(parse(R"({"ev":"status","state":"connecting"})"));
    QCOMPARE(c.sessionState(), QStringLiteral("connecting"));
    c.handleEvent(parse(R"({"ev":"status","state":"failed","error":{"kind":"busy","by":"MacBook"}})"));
    QCOMPARE(c.sessionState(), QStringLiteral("failed"));
    QCOMPARE(c.failure().value("kind").toString(), QStringLiteral("busy"));
    // A refusal carries the host's reason: the version card.
    c.handleEvent(parse(R"({"ev":"status","state":"failed","error":{"kind":"refused","reason":{"kind":"version_mismatch","host":4,"client":5}}})"));
    QCOMPARE(c.failure().value("kind").toString(), QStringLiteral("version_mismatch"));
    // Nothing the host reported: the generic kind.
    c.handleEvent(parse(R"({"ev":"status","state":"failed","error":{"kind":"timeout"}})"));
    QCOMPARE(c.failure().value("kind").toString(), QStringLiteral("timeout"));
}

void TstRemote::welcomeFillsDisplays()
{
    QTemporaryDir dir;
    RemoteController c(dir.path(), false, false);
    c.handleEvent(parse(R"({"ev":"welcome","name":"Studio PC","host_os":"windows","display_mode":"physical","live_resize":false,
        "current_display":2,"displays":[{"id":1,"name":"","width":2560,"height":1440,"scale":1.0,"primary":true},
                                        {"id":2,"name":"DELL U2720Q","width":3840,"height":2160,"scale":1.5,"primary":false}]})"));
    QVERIFY(c.physicalDisplays());
    QVERIFY(!c.liveResize());
    QCOMPARE(c.displays()->rowCount(), 2);
    QCOMPARE(c.displays()->at(1).value("current").toBool(), true);
    QCOMPARE(c.displays()->at(0).value("number").toInt(), 1);
    QCOMPARE(c.hostOs(), QStringLiteral("windows"));
    QVERIFY(!c.viewOnly());
    // View only comes from the Welcome; the system is only ever an id.
    c.handleEvent(parse(R"({"ev":"welcome","name":"Mac","host_os":"Mac OS X","view_only":true,"displays":[]})"));
    QVERIFY(c.viewOnly());
    QCOMPARE(c.hostOs(), QString());
    // A Linux that says it is SodiumOS arrives as "sodiumos"; any other Linux stays "linux".
    c.handleEvent(parse(R"({"ev":"welcome","name":"Desk","host_os":"sodiumos","displays":[]})"));
    QCOMPARE(c.hostOs(), QStringLiteral("sodiumos"));
}

void TstRemote::userDisconnectIsNotACard()
{
    QTemporaryDir dir;
    RemoteController c(dir.path(), false, false);
    c.handleEvent(parse(R"({"ev":"status","state":"connected","route":"lan","rtt_ms":2.0})"));
    c.handleEvent(parse(R"({"ev":"status","state":"ended","reason":{"kind":"user"}})"));
    QCOMPARE(c.sessionState(), QStringLiteral("idle"));
    c.handleEvent(parse(R"({"ev":"status","state":"ended","reason":{"kind":"taken_over","by":"Laptop"}})"));
    QCOMPARE(c.sessionState(), QStringLiteral("ended"));
    QCOMPARE(c.failure().value("by").toString(), QStringLiteral("Laptop"));
}

void TstRemote::anUnpairedComputersThumbnailIsDeleted()
{
    QTemporaryDir dir;
    RemoteController c(dir.path(), false, false);
    QDir().mkpath(dir.path() + "/thumbs");
    for (const char *id : {"H1", "H2"}) {
        QFile f(dir.path() + "/thumbs/" + id + ".png");
        QVERIFY(f.open(QIODevice::WriteOnly));
    }
    // H2 is gone from the paired list (removed here, or revoked there): its picture goes, H1 keeps its own.
    c.handleEvent(parse(R"({"ev":"hosts","paired":[{"id":"H1","code":"X","name":"A","online":true}],"found":[]})"));
    QVERIFY(QFile::exists(dir.path() + "/thumbs/H1.png"));
    QVERIFY(!QFile::exists(dir.path() + "/thumbs/H2.png"));
}

void TstRemote::turningPreviewsOffDeletesThePicturesAndKeepsTheChoice()
{
    QTemporaryDir dir;
    QDir().mkpath(dir.path() + "/thumbs");
    QFile f(dir.path() + "/thumbs/H1.png");
    QVERIFY(f.open(QIODevice::WriteOnly));
    f.close();
    {
        RemoteController c(dir.path(), false, false);
        QVERIFY(c.showPreviews());   // on by default
        QSignalSpy off(&c, &RemoteController::previewsTurnedOff);
        c.handleEvent(parse(R"({"ev":"hosts","paired":[{"id":"H1","code":"X","name":"A","online":true}],"found":[]})"));
        QVERIFY(!c.computers()->get("H1").value("thumb").toString().isEmpty());
        c.setShowPreviews(false);
        QCOMPARE(off.count(), 1);
        QVERIFY(!QFile::exists(dir.path() + "/thumbs/H1.png"));
        QVERIFY(c.computers()->get("H1").value("thumb").toString().isEmpty());   // a flat tile
    }
    RemoteController again(dir.path(), false, false);   // app.json keeps it
    QVERIFY(!again.showPreviews());
}

void TstRemote::aVersionMismatchSaysWhichSideIsOlder()
{
    QTemporaryDir dir;
    RemoteController c(dir.path(), false, false);
    c.handleEvent(parse(R"({"ev":"status","state":"failed","error":{"kind":"refused","reason":{"kind":"version_mismatch","host":7,"client":6,"update":"client"}}})"));
    QCOMPARE(c.failure().value("kind").toString(), QStringLiteral("version_mismatch"));
    QCOMPARE(c.failure().value("update").toString(), QStringLiteral("client"));
}

void TstRemote::aLinkShowsWhatItSaysBeforeAnythingIsSaved()
{
    qputenv("XDG_CONFIG_HOME", g_config.path().toUtf8());
    QTemporaryDir dir;
    RemoteController c(dir.path(), true, false);
    c.removeServer();   // a clean start: the folder is shared by the tests
    QVariantMap l = c.checkServerLink(kLinkWithCa);
    QVERIFY(l.value("ok").toBool());
    QCOMPARE(l.value("host").toString(), QStringLiteral("relay.example.com"));
    QVERIFY(l.value("needs_ca_prompt").toBool());   // the confirmation says the link carries its own certificate
    QVERIFY(l.value("token").toBool());
    QVERIFY(!c.serverConfigured());                 // and nothing is saved by looking
    // The hosted service never pins a certificate, whatever the link says.
    l = c.checkServerLink(QStringLiteral("sodium-remote://server?url=wss%3A%2F%2Fremote.sodiumos.com%2Fremote&ca=AQID"));
    QVERIFY(l.value("official").toBool());
    QVERIFY(!l.value("needs_ca_prompt").toBool());
    QVERIFY(!c.checkServerLink(QStringLiteral("https://relay.example.org/join")).value("ok").toBool());
}

void TstRemote::typedAddressesAreCheckedAndPlainTextIsRefused()
{
    QTemporaryDir dir;
    RemoteController c(dir.path(), true, false);
    c.removeServer();   // a clean start: the folder is shared by the tests
    const QVariantMap ok = c.checkServer(QStringLiteral(" relay.example.org "), QString(), QString());
    QVERIFY(ok.value("ok").toBool());
    QCOMPARE(ok.value("url").toString(), QStringLiteral("wss://relay.example.org/remote"));   // wss:// and the default path
    QCOMPARE(c.checkServer(QStringLiteral("http://relay.example.org"), QString(), QString()).value("error").toString(), QStringLiteral("insecure"));
    QCOMPARE(c.checkServer(QStringLiteral("relay.example.org"), QStringLiteral("not a certificate"), QString()).value("error").toString(), QStringLiteral("certificate"));
}

void TstRemote::aServerIsSavedExportedImportedAndRemoved()
{
    qputenv("XDG_CONFIG_HOME", g_config.path().toUtf8());
    QTemporaryDir dir;
    RemoteController c(dir.path(), true, false);
    c.removeServer();   // a clean start: the folder is shared by the tests
    QVERIFY(!c.serverConfigured());
    QSignalSpy saved(&c, &RemoteController::serverSaved);
    QVERIFY(c.saveServer(c.checkServerLink(kLinkWithCa).value("link").toString()).value("ok").toBool());
    QCOMPARE(saved.count(), 1);
    QVERIFY(c.serverConfigured());
    QCOMPARE(c.server().value("host").toString(), QStringLiteral("relay.example.com"));
    QVERIFY(c.server().value("pinnedCa").toBool());
    QCOMPARE(c.serverState(), QStringLiteral("checking"));
    // Export, then import that file: the same server, with its certificate, as the confirmation's data.
    const QUrl out = QUrl::fromLocalFile(dir.path() + "/server.toml");
    QVERIFY(c.exportServer(out).value("ok").toBool());
    QVERIFY(QFile::exists(dir.path() + "/server-ca.pem"));
    const QVariantMap back = c.readServerFile(out);
    QVERIFY(back.value("ok").toBool());
    QCOMPARE(back.value("link").toString(), c.checkServerLink(kLinkWithCa).value("link").toString());
    QVERIFY(back.value("needs_ca_prompt").toBool());
    QVERIFY(c.removeServer().value("ok").toBool());
    QVERIFY(!c.serverConfigured());
    QCOMPARE(c.serverState(), QStringLiteral("none"));
}

void TstRemote::withNoServerNoRelayStateIsShown()
{
    qputenv("XDG_CONFIG_HOME", g_config.path().toUtf8());
    QTemporaryDir dir;
    // A relay path learned while a server was configured.
    QFile f(dir.path() + "/app.json");
    QVERIFY(f.open(QIODevice::WriteOnly));
    f.write(R"({"settings":{},"hosts":{"H1":{"route":"relay","rtt":26},"H2":{"route":"lan","rtt":2}}})");
    f.close();
    RemoteController c(dir.path(), true, false);
    c.removeServer();   // a clean start: the folder is shared by the tests
    const QByteArray hosts = R"({"ev":"hosts","paired":[{"id":"H1","code":"A","name":"Studio","online":true},{"id":"H2","code":"B","name":"Lab","online":true}],"found":[]})";
    c.handleEvent(parse(hosts));
    QVERIFY(!c.serverConfigured());
    QCOMPARE(c.computers()->get("H1").value("route").toString(), QString());   // nothing to name: no Relay chip
    QCOMPARE(c.computers()->get("H2").value("route").toString(), QStringLiteral("lan"));
    QVERIFY(c.saveServer(c.checkServerLink(kLink).value("link").toString()).value("ok").toBool());
    c.handleEvent(parse(hosts));
    QCOMPARE(c.computers()->get("H1").value("route").toString(), QStringLiteral("relay"));
    c.removeServer();
}

void TstRemote::aServersRefusalIsReadFromItsCodeNeverItsText()
{
    qputenv("XDG_CONFIG_HOME", g_config.path().toUtf8());
    QTemporaryDir dir;
    RemoteController c(dir.path(), true, false);
    c.removeServer();   // a clean start: the folder is shared by the tests
    QVERIFY(c.saveServer(c.checkServerLink(kLink).value("link").toString()).value("ok").toBool());
    c.handleEvent(parse(R"j({"ev":"rendezvous","online":false,"error":"rendezvous refused: payment_required (Pay us at evil.example)"})j"));
    QCOMPARE(c.serverState(), QStringLiteral("offline"));
    QCOMPARE(c.serverProblem(), QStringLiteral("paymentRequired"));
    // host_not_allowed means "enrol first": the card asks for an enrollment token.
    c.handleEvent(parse(R"j({"ev":"rendezvous","online":false,"error":"rendezvous refused: host_not_allowed (x)"})j"));
    QCOMPARE(c.serverProblem(), QStringLiteral("tokenNeeded"));
    c.handleEvent(parse(R"j({"ev":"rendezvous","online":false,"error":"rendezvous refused: rate_limited (too many new devices from this address)"})j"));
    QCOMPARE(c.serverProblem(), QStringLiteral("rateLimited"));
    c.handleEvent(parse(R"j({"ev":"rendezvous","online":false,"error":"rendezvous unreachable: connection refused"})j"));
    QCOMPARE(c.serverProblem(), QString());   // not a code the app words: the generic unreachable banner
    c.handleEvent(parse(R"j({"ev":"rendezvous","online":true,"error":null})j"));
    QCOMPARE(c.serverState(), QStringLiteral("online"));
    c.removeServer();
}

void TstRemote::theStreamWindowFitsTheScreenItOpensOn()
{
    // A roomy screen keeps the default size, centred; a 150% laptop (1280x680 available) shrinks it to 90%; the screen's own origin counts.
    QCOMPARE(RemoteController::fitWindow(QRect(0, 0, 2560, 1400), QSize(1100, 760)), QRect(730, 320, 1100, 760));
    QCOMPARE(RemoteController::fitWindow(QRect(0, 0, 1280, 680), QSize(1100, 760)), QRect(90, 34, 1100, 612));
    const QRect r = RemoteController::fitWindow(QRect(-1280, 0, 1280, 680), QSize(1100, 760));
    QVERIFY(QRect(-1280, 0, 1280, 680).contains(r));
}

void TstRemote::theFirewallAllowsTheProgramOnPrivateUnlessABlockRuleNamesIt()
{
    const QString exe = QStringLiteral("C:\\Program Files\\Sodium Remote\\remote.exe");
    auto rule = [&](const QString &app, bool allow, int profiles = 2, bool enabled = true, bool inbound = true) {
        return FirewallRule{app, enabled, inbound, allow, profiles};
    };
    QVERIFY(!firewallAllowsPrivate({}, exe));
    // An enabled inbound Allow for this path on Private, whatever the path's case or slashes.
    QVERIFY(firewallAllowsPrivate({rule(QStringLiteral("c:/program files/sodium remote/REMOTE.EXE"), true)}, exe));
    QVERIFY(firewallAllowsPrivate({rule(exe, true, 0x7FFFFFFF)}, exe));
    // Not Private only, other programs, disabled, outbound, or no program at all do not count.
    QVERIFY(!firewallAllowsPrivate({rule(exe, true, 4)}, exe));
    QVERIFY(!firewallAllowsPrivate({rule(QStringLiteral("C:\\other.exe"), true)}, exe));
    QVERIFY(!firewallAllowsPrivate({rule(exe, true, 2, false)}, exe));
    QVERIFY(!firewallAllowsPrivate({rule(exe, true, 2, true, false)}, exe));
    QVERIFY(!firewallAllowsPrivate({rule(QString(), true)}, exe));
    // A Block on Private wins, in either order; a Block on Public only does not.
    QVERIFY(!firewallAllowsPrivate({rule(exe, true), rule(exe, false)}, exe));
    QVERIFY(!firewallAllowsPrivate({rule(exe, false), rule(exe, true)}, exe));
    QVERIFY(firewallAllowsPrivate({rule(exe, true), rule(exe, false, 4)}, exe));
}

#ifdef REMOTE_LINUX_HOST
// "This computer" on Linux: the engine's host_consent states are the pane's, an unknown one is "ask", and a client that can only
// watch (GNOME without Allow Remote Interaction) is view_only in the controller map.
void TstRemote::theDesktopsPermissionStatesMapToThePanesState()
{
    QTemporaryDir dir;
    HostController h(dir.path());
    for (const char *state : {"ask", "asking", "remembered", "declined", "unavailable"}) {
        h.handleEvent(parse(QByteArray("{\"ev\":\"host_consent\",\"state\":\"").append(state).append("\"}").constData()));
        QCOMPARE(h.consent(), QString::fromLatin1(state));
    }
    h.handleEvent(parse(R"({"ev":"host_consent","state":"something new"})"));
    QCOMPARE(h.consent(), QStringLiteral("ask"));
    h.handleEvent(parse(R"({"ev":"host_status","controlled":true,"client":{"id":"a","name":"Laptop","os":"mac","since":1,"view_only":true}})"));
    QVERIFY(h.controlled());
    QVERIFY(h.controller().value("view_only").toBool());
    // A lost connection's held session isn't "connected now".
    h.handleEvent(parse(R"({"ev":"host_status","controlled":true,"held":true,"client":{"id":"a","name":"Laptop","os":"mac","since":1}})"));
    QVERIFY(!h.controlled());
    h.handleEvent(parse(R"({"ev":"host_status","controlled":false})"));
    QVERIFY(!h.controlled());
    QCOMPARE(HostController::desktopIdFor(QStringLiteral("ubuntu:GNOME")), QStringLiteral("gnome"));
    QCOMPARE(HostController::desktopIdFor(QStringLiteral("KDE")), QStringLiteral("kde"));
    QCOMPARE(HostController::desktopIdFor(QStringLiteral("Hyprland")), QStringLiteral("other"));
}

void TstRemote::forgettingThePermissionDeletesTheSavedChoice()
{
    QTemporaryDir dir;
    QFile token(dir.filePath("portal-restore-token"));
    QVERIFY(token.open(QIODevice::WriteOnly));
    token.write("t");
    token.close();
    HostController h(dir.path());
    h.forgetConsent();
    QVERIFY(!QFile::exists(token.fileName()));
    QTRY_COMPARE(h.consent(), QStringLiteral("ask")); // the engine's answer, as the page shows it
}
#endif

QTEST_MAIN(TstRemote)
#include "tst_remote.moc"
