// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// Whether the Windows firewall lets this program in on a Private network, from the rules INetFwPolicy2 lists. The matching is a
// pure function (no Windows calls) so it is tested everywhere; hostcontroller.cpp fills the list on Windows.
#pragma once

#include <QList>
#include <QString>

/// The parts of an INetFwRule the decision reads.
struct FirewallRule {
    QString application; ///< ApplicationName: the program's path ("" for any program)
    bool enabled = false;
    bool inbound = false; ///< Direction is NET_FW_RULE_DIR_IN
    bool allow = false;   ///< Action is NET_FW_ACTION_ALLOW (else Block)
    int profiles = 0;     ///< Profiles: NET_FW_PROFILE2_DOMAIN 1, PRIVATE 2, PUBLIC 4, ALL 0x7FFFFFFF
};

constexpr int kFirewallProfilePrivate = 2;

/// True when an enabled inbound Allow rule names `exe` (compared case-insensitively, with `/` and `\` alike) and covers the Private
/// profile, and no enabled inbound Block rule names it on Private (a Block wins over an Allow in Windows).
bool firewallAllowsPrivate(const QList<FirewallRule> &rules, const QString &exe);
