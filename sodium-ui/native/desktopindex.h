// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// DesktopIndex: installed apps' desktop-entry text by desktop id, for QML (decisions 105 and 107). I18n.appName and
// I18n.desktopText are its QML face; C++ that already holds a GDesktopAppInfo uses desktopgio.h instead.
//
// The id is Quickshell's DesktopEntry.id (and GIO's without ".desktop"): the file's path under an applications folder
// of XDG_DATA_HOME or XDG_DATA_DIRS, "/" written "-" (kde/org.kde.foo.desktop → "kde-org.kde.foo"); the first folder
// that has it wins, as in the XDG spec. Each file is parsed with GLib's key file keeping every translation (Quickshell's
// and GIO's own parsers keep only the process environment's languages, desktopgio.h) and its keys are kept until the
// file's mtime or size changes or the folders change. What's kept is the file's keys, never a resolved string: every
// call resolves through this process's content chain (desktoptext.h), so a language switch needs no flush.
#pragma once

#include <QDateTime>
#include <QHash>
#include <QObject>
#include <QString>
#include <QStringList>

class QFileSystemWatcher;

class DesktopIndex : public QObject
{
    Q_OBJECT

public:
    static DesktopIndex *instance();

    /// `key` (Name, GenericName, Comment, X-GNOME-FullName, Keywords, or "Action <action>/Name" for a [Desktop Action])
    /// of entry `id` through the chain; "" when the entry or the key isn't there. `chain` omitted: this process's (DesktopText::contentChain).
    QString text(const QString &id, const QString &key);
    QString text(const QString &id, const QString &key, const QStringList &chain);
    /// The entry's file, "" when no applications folder has `id`.
    QString pathOf(const QString &id);

private:
    explicit DesktopIndex(QObject *parent = nullptr);
    void scan();
    void scanDir(const QString &root, const QString &dir, const QString &prefix, int depth);
    /// The entry's text keys ("Name", "Name[de]", …), parsed again when the file changed.
    const QHash<QString, QString> *keysOf(const QString &id);

    struct Parsed {
        QDateTime mtime;
        qint64 size = -1;
        QHash<QString, QString> keys;
    };

    bool m_dirty = true;
    QHash<QString, QString> m_paths;      // id → file
    QHash<QString, Parsed> m_parsed;      // file → keys
    QStringList m_roots;
    QFileSystemWatcher *m_watcher = nullptr;
};
