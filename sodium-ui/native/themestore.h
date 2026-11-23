// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// ThemeStore: the live theme and user settings behind SodiumUI's Theme singleton.
//
// Reads two JSON files and watches both, so a change re-tints every running
// shell module and app without a restart:
//
//   <state>/theme/sodiumui/theme.json   written by tokens/generate.py (scripts/apply-theme.sh)
//   <state>/settings.json            user settings, written by the Settings app (or setSetting())
//
// <state> is $SODIUMOS_STATE, else ${XDG_STATE_HOME:-~/.local/state}/sodiumos.
// With no live theme file the committed defaults next to the qmldir
// (SodiumUI/theme.default.json) are used. A file that fails to parse (caught
// mid-write) keeps the last good values and is read again shortly after.
#pragma once

#include <QColor>
#include <QFileSystemWatcher>
#include <QObject>
#include <QTimer>
#include <QUrl>
#include <QVariantMap>

class ThemeStore : public QObject
{
    Q_OBJECT
    // The parsed theme.json; "colors" holds QColor values.
    Q_PROPERTY(QVariantMap theme READ theme NOTIFY themeChanged)
    // Defaults from theme.json "settings", overridden by settings.json.
    Q_PROPERTY(QVariantMap settings READ settings NOTIFY settingsChanged)
    // True while the live theme file is in use (false: committed defaults).
    Q_PROPERTY(bool live READ live NOTIFY themeChanged)
    // Counts theme loads; handy for tests and for "re-tint happened" checks.
    Q_PROPERTY(int revision READ revision NOTIFY themeChanged)
    Q_PROPERTY(QString themePath READ themePath CONSTANT)
    Q_PROPERTY(QString settingsPath READ settingsPath CONSTANT)
    Q_PROPERTY(QString defaultPath READ defaultPath CONSTANT)

public:
    explicit ThemeStore(const QString &defaultPath, QObject *parent = nullptr);

    QVariantMap theme() const { return m_theme; }
    QVariantMap settings() const { return m_settings; }
    bool live() const { return m_live; }
    int revision() const { return m_revision; }
    QString themePath() const { return m_themePath; }
    QString settingsPath() const { return m_settingsPath; }
    QString defaultPath() const { return m_defaultPath; }

    // Writes one user setting to settings.json (atomically) and applies it. undefined or null removes the
    // key: the setting is back to its default (tokens.json, or none: the clock follows the Region).
    Q_INVOKABLE bool setSetting(const QString &key, const QVariant &value);
    // Re-reads both files now.
    Q_INVOKABLE void reload();

    static QString stateDir();

signals:
    void themeChanged();
    void settingsChanged();

private:
    void scheduleRescan(int ms = 40);
    void rescan();
    void rewatch();
    bool loadTheme();      // false: parse failure (retry)
    bool loadSettings();   // false: parse failure (retry)
    void mergeSettings();
    void applyCaretBlink();

    QString m_defaultPath;
    QString m_themePath;
    QString m_settingsPath;
    QByteArray m_themeBytes;
    QByteArray m_settingsBytes;
    QVariantMap m_theme;
    QVariantMap m_userSettings;
    QVariantMap m_settings;
    bool m_live = false;
    int m_revision = 0;
    int m_retries = 0;
    QFileSystemWatcher m_watcher;
    QTimer m_debounce;
};
