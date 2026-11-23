// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// DesktopText: an installed app's desktop-entry text (Name, GenericName, Comment, X-GNOME-FullName) in this app's
// own language (docs/decisions/i18n.md, decision 105). Header-only, so code that doesn't link Sodium UI uses it too
// (Files' MimeApps, Settings' Default apps, Notes, the file chooser, Disks).
//
// Never GIO's or Qt's locale lookup for visible text (g_app_info_get_display_name, g_desktop_app_info_get_generic_name,
// g_app_info_get_description, Quickshell's DesktopEntry.name): they follow the process environment, are cached for the
// process's life and ignore a per-app language. `LANGUAGE=en_US:zh_CN` gave Name[zh_CN] under an English UI, since no
// desktop file has Name[en_US].
//
// The chain is decision 94's (I18n::contentChainFor), which Sodium UI's I18n publishes as the application property
// `sodiumosContentChain` before every language switch (and on a Region change). Each tag, "-" written "_", is tried as
// Key[tag], then the plain key:
//     en_US []  → Name                        de_DE ["de-DE", "de"] → Name[de_DE], Name[de], Name
//     zh_CN ["zh-Hans-CN", "zh-CN", "zh-Hans"] → Name[zh_Hans_CN], Name[zh_CN], Name[zh_Hans], Name
// Never a bare zh, never across scripts, never a third language. Without the property (a process with no I18n, the
// tests) the plain key: the English source.
//
// Nothing resolved here is stored: callers resolve when shown, and QML call sites depend on I18n.revision.
//
//     auto get = [&](const QString &key) { return entry.value(key); };   // a parsed [Desktop Entry], or GKeyFile
//     DesktopText::displayName(get)          // X-GNOME-FullName, else Name, in the app's language
//     DesktopText::localized(get, QStringLiteral("GenericName"))
#pragma once

#include <QCoreApplication>
#include <QString>
#include <QStringList>
#include <QVariant>

namespace DesktopText {

/// This process's content chain, as I18n publishes it; [] without I18n.
inline QStringList contentChain()
{
    const QCoreApplication *app = QCoreApplication::instance();
    return app ? app->property("sodiumosContentChain").toStringList() : QStringList();
}

/// `key` in the first chain language the entry has, else the plain key. `get(key)` returns a key's value or "".
template <typename Get>
QString localized(Get &&get, const QString &key, const QStringList &chain = contentChain())
{
    for (QString tag : chain) {
        tag.replace(QLatin1Char('-'), QLatin1Char('_'));
        const QString v = get(key + QLatin1Char('[') + tag + QLatin1Char(']'));
        if (!v.isEmpty())
            return v;
    }
    return get(key);
}

/// The name an app goes by: X-GNOME-FullName if the entry has one, else Name, each through the chain (as GIO's
/// g_app_info_get_display_name chooses, in the app's language).
template <typename Get>
QString displayName(Get &&get, const QStringList &chain = contentChain())
{
    const QString full = localized(get, QStringLiteral("X-GNOME-FullName"), chain);
    return full.isEmpty() ? localized(get, QStringLiteral("Name"), chain) : full;
}

} // namespace DesktopText
