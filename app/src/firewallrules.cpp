// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "firewallrules.h"

namespace {
QString samePath(const QString &p)
{
    QString s = p.trimmed();
    s.replace(QLatin1Char('/'), QLatin1Char('\\'));
    return s.toLower();
}
} // namespace

bool firewallAllowsPrivate(const QList<FirewallRule> &rules, const QString &exe)
{
    const QString want = samePath(exe);
    bool allowed = false;
    for (const FirewallRule &r : rules) {
        if (!r.enabled || !r.inbound || !(r.profiles & kFirewallProfilePrivate) || r.application.isEmpty() || samePath(r.application) != want)
            continue;
        if (!r.allow)
            return false;
        allowed = true;
    }
    return allowed;
}
