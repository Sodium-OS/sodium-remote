// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// Linux: one running app receives the server links that other launches were given (the desktop entry passes the
// clicked sodium-remote:// link as an argument). The first instance listens on a socket in $XDG_RUNTIME_DIR named for
// its data folder; a launch with a link sends it there and exits. Windows does the same through the tray window
// (WM_COPYDATA, winapp.cpp).
#pragma once
#include <QLocalServer>
#include <QObject>

class LinkHandoff : public QObject
{
    Q_OBJECT
public:
    /// Listens for links on the socket for `dataDir`. Nothing is listened on when the socket can't be made.
    explicit LinkHandoff(const QString &dataDir, QObject *parent = nullptr);
    /// Sends `link` to the instance running for `dataDir`: true when one took it.
    static bool forward(const QString &dataDir, const QString &link);
    static QString socketPath(const QString &dataDir);

Q_SIGNALS:
    /// A link arrived (an empty line asks only to show the window).
    void received(const QString &link);

private:
    QLocalServer m_server;
};
