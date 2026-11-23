// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// The app's three list models: the computers you paired, the computers nearby that can be paired, and the displays of
// the connected computer. Each row is a map keyed by the role names; ids are stable, so a refresh updates rows in place
// instead of resetting the list (the cards keep their hover and focus). Rows never hold translated text: states are
// ids ("online", "asleep", ...), the QML words them.
#pragma once
#include <QAbstractListModel>
#include <QStringList>
#include <QVariantMap>

class MapListModel : public QAbstractListModel
{
    Q_OBJECT
    Q_PROPERTY(int count READ rowCount NOTIFY countChanged)
public:
    MapListModel(const QStringList &roles, const QString &idRole, QObject *parent = nullptr);
    int rowCount(const QModelIndex &parent = {}) const override;
    QVariant data(const QModelIndex &index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;

    /// Replaces the rows, keeping the order given: rows with the same id are updated in place, new ones inserted,
    /// gone ones removed.
    void setRows(const QList<QVariantMap> &rows);
    /// The row with this id, or an empty map.
    Q_INVOKABLE QVariantMap get(const QString &id) const;
    Q_INVOKABLE QVariantMap at(int row) const;
    Q_INVOKABLE int indexOf(const QString &id) const;
    const QList<QVariantMap> &rows() const { return m_rows; }

Q_SIGNALS:
    void countChanged();

private:
    QStringList m_roles;
    QString m_idRole;
    QList<QVariantMap> m_rows;
};

/// Paired computers. Roles: hostId, code, name (what the card shows), deviceName (the host's own), os ("linux", "mac",
/// "windows" or "": learned at the first connection), state ("online", "asleep", "offline", "connecting"), onLan,
/// asleepSince, lastSeen, lastConnected (unix seconds, 0 for never), route ("lan", "direct", "relay" or ""), rttMs,
/// thumb (a file URL of the last picture seen of it, or "").
class ComputersModel : public MapListModel
{
    Q_OBJECT
public:
    explicit ComputersModel(QObject *parent = nullptr);
};

/// Computers with an open pairing window. Roles: hostId, code, name, addresses (list), port.
class NearbyModel : public MapListModel
{
    Q_OBJECT
public:
    explicit NearbyModel(QObject *parent = nullptr);
};

/// The displays the connected computer offers. Roles: displayId, name (the host's, may be empty), width, height,
/// primary, current, number (1-based position).
class DisplaysModel : public MapListModel
{
    Q_OBJECT
public:
    explicit DisplaysModel(QObject *parent = nullptr);
};
