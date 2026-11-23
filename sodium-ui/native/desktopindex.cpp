// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "desktopindex.h"

#include "desktoptext.h"

#include <QDir>
#include <QFileInfo>
#include <QFileSystemWatcher>
#include <QStandardPaths>

// Not in the portable build (Windows, the Mac): there are no desktop entries to read, so pathOf finds none.
#ifndef SODIUMUI_PORTABLE
#include <glib.h>
#endif

namespace {

/// The keys that are shown as text; nothing else is kept.
bool isTextKey(const QString &key)
{
    const QString base = key.section(QLatin1Char('['), 0, 0);
    return base == QLatin1String("Name") || base == QLatin1String("GenericName") || base == QLatin1String("Comment")
        || base == QLatin1String("X-GNOME-FullName") || base == QLatin1String("Keywords");
}

QStringList applicationRoots()
{
    QStringList roots;
    for (const QString &dir : QStandardPaths::standardLocations(QStandardPaths::ApplicationsLocation)) {
        const QString clean = QDir::cleanPath(dir);
        if (!roots.contains(clean))
            roots << clean;
    }
    return roots;
}

} // namespace

DesktopIndex *DesktopIndex::instance()
{
    static DesktopIndex *index = new DesktopIndex;
    return index;
}

DesktopIndex::DesktopIndex(QObject *parent)
    : QObject(parent)
    , m_watcher(new QFileSystemWatcher(this))
{
    // An app installed, removed or updated (a package replaces the file by renaming over it): scan again on next use.
    connect(m_watcher, &QFileSystemWatcher::directoryChanged, this, [this] { m_dirty = true; });
}

void DesktopIndex::scan()
{
    m_dirty = false;
    m_paths.clear();
    const QStringList watched = m_watcher->directories();
    if (!watched.isEmpty())
        m_watcher->removePaths(watched);
    for (const QString &root : m_roots)
        scanDir(root, root, QString(), 0);
}

void DesktopIndex::scanDir(const QString &root, const QString &dir, const QString &prefix, int depth)
{
    const QDir d(dir);
    if (!d.exists())
        return;
    m_watcher->addPath(dir);
    const QFileInfoList files = d.entryInfoList({QStringLiteral("*.desktop")}, QDir::Files | QDir::Readable);
    for (const QFileInfo &f : files) {
        const QString id = prefix + f.completeBaseName();
        if (!m_paths.contains(id))   // an earlier folder (XDG_DATA_HOME first) wins
            m_paths.insert(id, f.absoluteFilePath());
    }
    if (depth >= 4)
        return;
    for (const QString &sub : d.entryList(QDir::Dirs | QDir::NoDotAndDotDot))
        scanDir(root, d.filePath(sub), prefix + sub + QLatin1Char('-'), depth + 1);
}

QString DesktopIndex::pathOf(const QString &id)
{
    const QStringList roots = applicationRoots();
    if (roots != m_roots) {
        m_roots = roots;
        m_dirty = true;
    }
    if (m_dirty)
        scan();
    QString key = id;
    if (key.endsWith(QLatin1String(".desktop")))
        key.chop(8);
    return m_paths.value(key);
}

const QHash<QString, QString> *DesktopIndex::keysOf(const QString &id)
{
    const QString path = pathOf(id);
    if (path.isEmpty())
        return nullptr;
    const QFileInfo info(path);
    if (!info.exists()) {
        m_parsed.remove(path);
        m_dirty = true;
        return nullptr;
    }
    auto it = m_parsed.find(path);
    if (it != m_parsed.end() && it->mtime == info.lastModified() && it->size == info.size())
        return &it->keys;

    Parsed parsed;
    parsed.mtime = info.lastModified();
    parsed.size = info.size();
#ifndef SODIUMUI_PORTABLE
    GKeyFile *file = g_key_file_new();
    if (g_key_file_load_from_file(file, QFile::encodeName(path).constData(), G_KEY_FILE_KEEP_TRANSLATIONS, nullptr)) {
        gsize n = 0;
        gchar **keys = g_key_file_get_keys(file, G_KEY_FILE_DESKTOP_GROUP, &n, nullptr);
        for (gsize i = 0; keys && i < n; ++i) {
            const QString key = QString::fromUtf8(keys[i]);
            if (!isTextKey(key))
                continue;
            gchar *v = g_key_file_get_string(file, G_KEY_FILE_DESKTOP_GROUP, keys[i], nullptr);
            if (v && *v)
                parsed.keys.insert(key, QString::fromUtf8(v));
            g_free(v);
        }
        g_strfreev(keys);
        // Jump-list actions: "[Desktop Action new-window]" Name[de] is kept as "Action new-window/Name[de]".
        gsize groupCount = 0;
        gchar **groups = g_key_file_get_groups(file, &groupCount);
        for (gsize g = 0; groups && g < groupCount; ++g) {
            const QString group = QString::fromUtf8(groups[g]);
            if (!group.startsWith(QLatin1String("Desktop Action ")))
                continue;
            const QString action = group.mid(15);
            gsize m = 0;
            gchar **akeys = g_key_file_get_keys(file, groups[g], &m, nullptr);
            for (gsize i = 0; akeys && i < m; ++i) {
                const QString key = QString::fromUtf8(akeys[i]);
                if (key.section(QLatin1Char('['), 0, 0) != QLatin1String("Name"))
                    continue;
                gchar *v = g_key_file_get_string(file, groups[g], akeys[i], nullptr);
                if (v && *v)
                    parsed.keys.insert(QStringLiteral("Action ") + action + QLatin1Char('/') + key, QString::fromUtf8(v));
                g_free(v);
            }
            g_strfreev(akeys);
        }
        g_strfreev(groups);
    }
    g_key_file_free(file);
#endif
    it = m_parsed.insert(path, parsed);
    return &it->keys;
}

QString DesktopIndex::text(const QString &id, const QString &key)
{
    return text(id, key, DesktopText::contentChain());
}

QString DesktopIndex::text(const QString &id, const QString &key, const QStringList &chain)
{
    const QHash<QString, QString> *keys = keysOf(id);
    if (!keys)
        return {};
    const auto get = [keys](const QString &k) { return keys->value(k); };
    return DesktopText::localized(get, key, chain);
}
