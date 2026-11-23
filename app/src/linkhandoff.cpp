// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "linkhandoff.h"

#include <QCryptographicHash>
#include <QDir>
#include <QLocalSocket>
#include <QStandardPaths>

QString LinkHandoff::socketPath(const QString &dataDir)
{
    // The per-user runtime folder only: a shared folder such as /tmp would let another user create the socket first and
    // receive the links (with their tokens). Without it there is no hand-off, and a second launch just starts normally.
    const QString dir = qEnvironmentVariable("XDG_RUNTIME_DIR");
    if (dir.isEmpty())
        return {};
    const QByteArray h = QCryptographicHash::hash(QDir::cleanPath(dataDir).toUtf8(), QCryptographicHash::Sha1).toHex().left(12);
    return dir + QStringLiteral("/sodium-remote-") + QString::fromLatin1(h) + QStringLiteral(".sock");
}

bool LinkHandoff::forward(const QString &dataDir, const QString &link)
{
    const QString path = socketPath(dataDir);
    if (path.isEmpty())
        return false;
    QLocalSocket s;
    s.connectToServer(path);
    if (!s.waitForConnected(500))
        return false;
    s.write(link.toUtf8().replace('\n', ' ') + '\n');
    return s.waitForBytesWritten(1000);
}

LinkHandoff::LinkHandoff(const QString &dataDir, QObject *parent) : QObject(parent)
{
    m_server.setSocketOptions(QLocalServer::UserAccessOption);
    const QString path = socketPath(dataDir);
    if (path.isEmpty()) {
        qWarning("Sodium Remote: XDG_RUNTIME_DIR is not set; links from other programs are not handed to this window");
        return;
    }
    // forward() found nobody: a leftover file from a crash is stale.
    QLocalServer::removeServer(path);
    if (!m_server.listen(path))
        return;
    connect(&m_server, &QLocalServer::newConnection, this, [this] {
        while (QLocalSocket *c = m_server.nextPendingConnection()) {
            connect(c, &QLocalSocket::disconnected, c, &QObject::deleteLater);
            connect(c, &QLocalSocket::readyRead, this, [this, c] {
                while (c->canReadLine())
                    Q_EMIT received(QString::fromUtf8(c->readLine()).trimmed());
            });
        }
    });
}
