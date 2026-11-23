// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "models.h"

MapListModel::MapListModel(const QStringList &roles, const QString &idRole, QObject *parent)
    : QAbstractListModel(parent), m_roles(roles), m_idRole(idRole)
{
}

int MapListModel::rowCount(const QModelIndex &parent) const
{
    return parent.isValid() ? 0 : int(m_rows.size());
}

QVariant MapListModel::data(const QModelIndex &index, int role) const
{
    if (!index.isValid() || index.row() >= m_rows.size())
        return {};
    const int r = role - Qt::UserRole;
    if (r < 0 || r >= m_roles.size())
        return {};
    return m_rows.at(index.row()).value(m_roles.at(r));
}

QHash<int, QByteArray> MapListModel::roleNames() const
{
    QHash<int, QByteArray> h;
    for (int i = 0; i < m_roles.size(); ++i)
        h.insert(Qt::UserRole + i, m_roles.at(i).toUtf8());
    return h;
}

int MapListModel::indexOf(const QString &id) const
{
    for (int i = 0; i < m_rows.size(); ++i)
        if (m_rows.at(i).value(m_idRole).toString() == id)
            return i;
    return -1;
}

QVariantMap MapListModel::get(const QString &id) const
{
    const int i = indexOf(id);
    return i < 0 ? QVariantMap() : m_rows.at(i);
}

QVariantMap MapListModel::at(int row) const
{
    return row >= 0 && row < m_rows.size() ? m_rows.at(row) : QVariantMap();
}

void MapListModel::setRows(const QList<QVariantMap> &rows)
{
    const int before = int(m_rows.size());
    // Removed rows first, then each wanted row to its place.
    QStringList want;
    for (const auto &r : rows)
        want << r.value(m_idRole).toString();
    for (int i = int(m_rows.size()) - 1; i >= 0; --i) {
        if (!want.contains(m_rows.at(i).value(m_idRole).toString())) {
            beginRemoveRows({}, i, i);
            m_rows.removeAt(i);
            endRemoveRows();
        }
    }
    for (int to = 0; to < rows.size(); ++to) {
        const QString id = want.at(to);
        int from = -1;
        for (int i = to; i < m_rows.size(); ++i) {
            if (m_rows.at(i).value(m_idRole).toString() == id) {
                from = i;
                break;
            }
        }
        if (from < 0) {
            beginInsertRows({}, to, to);
            m_rows.insert(to, rows.at(to));
            endInsertRows();
            continue;
        }
        if (from != to) {
            beginMoveRows({}, from, from, {}, to);
            m_rows.move(from, to);
            endMoveRows();
        }
        if (m_rows.at(to) != rows.at(to)) {
            m_rows[to] = rows.at(to);
            Q_EMIT dataChanged(index(to), index(to));
        }
    }
    if (before != m_rows.size())
        Q_EMIT countChanged();
}

ComputersModel::ComputersModel(QObject *parent)
    : MapListModel({QStringLiteral("hostId"), QStringLiteral("code"), QStringLiteral("name"), QStringLiteral("deviceName"),
                    QStringLiteral("os"), QStringLiteral("state"), QStringLiteral("onLan"), QStringLiteral("asleepSince"),
                    QStringLiteral("lastSeen"), QStringLiteral("lastConnected"), QStringLiteral("route"), QStringLiteral("rttMs"), QStringLiteral("thumb")},
                   QStringLiteral("hostId"), parent)
{
}

NearbyModel::NearbyModel(QObject *parent)
    : MapListModel({QStringLiteral("hostId"), QStringLiteral("code"), QStringLiteral("name"), QStringLiteral("addresses"),
                    QStringLiteral("port")},
                   QStringLiteral("hostId"), parent)
{
}

DisplaysModel::DisplaysModel(QObject *parent)
    : MapListModel({QStringLiteral("displayId"), QStringLiteral("name"), QStringLiteral("width"), QStringLiteral("height"),
                    QStringLiteral("primary"), QStringLiteral("current"), QStringLiteral("number")},
                   QStringLiteral("displayId"), parent)
{
}
