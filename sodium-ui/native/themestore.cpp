// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "themestore.h"

#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QGuiApplication>
#include <QJsonDocument>
#include <QJsonObject>
#include <QLockFile>
#include <QSaveFile>
#include <QStandardPaths>
#include <QStyleHints>

namespace {

QByteArray readAll(const QString &path, bool *exists)
{
    QFile f(path);
    *exists = f.exists();
    if (!*exists || !f.open(QIODevice::ReadOnly))
        return {};
    return f.readAll();
}

bool parseObject(const QByteArray &bytes, QVariantMap *out)
{
    QJsonParseError err{};
    const QJsonDocument doc = QJsonDocument::fromJson(bytes, &err);
    if (err.error != QJsonParseError::NoError || !doc.isObject())
        return false;
    *out = doc.object().toVariantMap();
    return true;
}

// "#AARRGGBB" / "#RRGGBB" strings in "colors" become QColor, so QML sees real colors.
void convertColors(QVariantMap *theme)
{
    QVariantMap colors = theme->value(QStringLiteral("colors")).toMap();
    for (auto it = colors.begin(); it != colors.end(); ++it)
        it.value() = QColor::fromString(it.value().toString());
    theme->insert(QStringLiteral("colors"), colors);
}

// Keys the live file doesn't have yet (written by an older generator) come from the
// committed default, so new tokens work before apply-theme.sh runs again. Colors are
// only filled in from a default of the same theme (dark or light).
void fillMissing(QVariantMap *live, const QVariantMap &fallback, bool top = true)
{
    const bool sameTheme = live->value(QStringLiteral("theme")) == fallback.value(QStringLiteral("theme"));
    for (auto it = fallback.cbegin(); it != fallback.cend(); ++it) {
        if (top && it.key() == QLatin1String("colors") && !sameTheme)
            continue;
        auto mine = live->find(it.key());
        if (mine == live->end()) {
            live->insert(it.key(), it.value());
        } else if (mine.value().typeId() == QMetaType::QVariantMap && it.value().typeId() == QMetaType::QVariantMap) {
            QVariantMap sub = mine.value().toMap();
            fillMissing(&sub, it.value().toMap(), false);
            mine.value() = sub;
        }
    }
}

} // namespace

QString ThemeStore::stateDir()
{
    const QString env = qEnvironmentVariable("SODIUMOS_STATE");
    if (!env.isEmpty())
        return QDir::cleanPath(env);
    QString base = qEnvironmentVariable("XDG_STATE_HOME");
#ifdef Q_OS_WIN
    // %LOCALAPPDATA%\sodiumos (the same place the Windows host keeps its key), not a dotted folder in the profile.
    if (base.isEmpty())
        base = QStandardPaths::writableLocation(QStandardPaths::GenericDataLocation);
#endif
    if (base.isEmpty())
        base = QDir::homePath() + QStringLiteral("/.local/state");
    return QDir::cleanPath(base + QStringLiteral("/sodiumos"));
}

ThemeStore::ThemeStore(const QString &defaultPath, QObject *parent)
    : QObject(parent)
    , m_defaultPath(defaultPath)
    , m_themePath(stateDir() + QStringLiteral("/theme/sodiumui/theme.json"))
    , m_settingsPath(stateDir() + QStringLiteral("/settings.json"))
{
    m_debounce.setSingleShot(true);
    connect(&m_debounce, &QTimer::timeout, this, &ThemeStore::rescan);
    connect(&m_watcher, &QFileSystemWatcher::fileChanged, this, [this] { scheduleRescan(); });
    connect(&m_watcher, &QFileSystemWatcher::directoryChanged, this, [this] { scheduleRescan(); });
    rewatch();
    loadTheme();
    loadSettings();
    mergeSettings();
}

void ThemeStore::scheduleRescan(int ms)
{
    m_debounce.start(ms);
}

// Watch the files and every directory on the way to them that exists, so a
// file that is created later, or replaced by a rename, is still noticed.
void ThemeStore::rewatch()
{
    const QString root = stateDir();
    const QStringList wanted = {
        root,
        root + QStringLiteral("/theme"),
        root + QStringLiteral("/theme/sodiumui"),
        m_themePath,
        m_settingsPath,
    };
    QStringList existing;
    for (const QString &p : wanted)
        if (QFileInfo::exists(p))
            existing << p;
    // The nearest existing ancestor, for a state dir that doesn't exist yet.
    if (existing.isEmpty()) {
        QDir d(root);
        while (!d.exists() && d.cdUp()) {}
        existing << d.absolutePath();
    }
    const QStringList watched = m_watcher.files() + m_watcher.directories();
    for (const QString &p : existing)
        if (!watched.contains(p))
            m_watcher.addPath(p);
}

void ThemeStore::rescan()
{
    rewatch();
    const bool themeOk = loadTheme();
    const bool settingsOk = loadSettings();
    mergeSettings();
    if (themeOk && settingsOk) {
        m_retries = 0;
    } else if (m_retries++ < 20) {
        scheduleRescan(100);  // caught mid-write: read again
    }
}

void ThemeStore::reload()
{
    m_themeBytes.clear();
    m_settingsBytes.clear();
    rescan();
}

bool ThemeStore::loadTheme()
{
    bool exists = false;
    QByteArray bytes = readAll(m_themePath, &exists);
    bool live = exists;
    if (!exists || bytes.isEmpty()) {
        if (exists)  // empty: truncated mid-write; keep what we have
            return false;
        bytes = readAll(m_defaultPath, &exists);
        live = false;
    }
    if (bytes == m_themeBytes && live == m_live)
        return true;
    QVariantMap parsed;
    if (!parseObject(bytes, &parsed))
        return false;
    if (live) {
        bool defaultExists = false;
        QVariantMap fallback;
        if (parseObject(readAll(m_defaultPath, &defaultExists), &fallback))
            fillMissing(&parsed, fallback);
    }
    convertColors(&parsed);
    m_themeBytes = bytes;
    m_theme = parsed;
    m_live = live;
    ++m_revision;
    emit themeChanged();
    return true;
}

bool ThemeStore::loadSettings()
{
    bool exists = false;
    const QByteArray bytes = readAll(m_settingsPath, &exists);
    if (bytes == m_settingsBytes)
        return true;
    QVariantMap parsed;
    if (exists && !bytes.isEmpty() && !parseObject(bytes, &parsed))
        return false;
    if (exists && bytes.isEmpty())
        return false;  // truncated mid-write
    m_settingsBytes = bytes;
    m_userSettings = parsed;
    return true;
}

void ThemeStore::mergeSettings()
{
    QVariantMap merged = m_theme.value(QStringLiteral("settings")).toMap();
    for (auto it = m_userSettings.cbegin(); it != m_userSettings.cend(); ++it)
        merged.insert(it.key(), it.value());
    if (merged != m_settings) {
        m_settings = merged;
        applyCaretBlink();
        emit settingsChanged();
    }
}

// Settings › Accessibility › "Blink the text caret": Qt's own text fields in this process (the
// ones without SodiumUI's Caret) follow the platform's cursor flash time, so it goes to 0 while off.
void ThemeStore::applyCaretBlink()
{
    auto *app = qobject_cast<QGuiApplication *>(QCoreApplication::instance());
    if (!app)
        return;
    QStyleHints *hints = QGuiApplication::styleHints();
    static const int platform = hints->cursorFlashTime();
    const int want = m_settings.value(QStringLiteral("caretBlink"), true).toBool() ? platform : 0;
    if (hints->cursorFlashTime() != want)
        hints->setCursorFlashTime(want);
}

bool ThemeStore::setSetting(const QString &key, const QVariant &value)
{
    QDir().mkpath(QFileInfo(m_settingsPath).absolutePath());
    // Read, change one key, write: under a lock shared by every SodiumUI process, from what is on
    // disk now (another process may have written since we last read it), and atomically.
    QLockFile lock(m_settingsPath + QStringLiteral(".lock"));
    lock.setStaleLockTime(5000);
    if (!lock.tryLock(2000)) {
        qWarning("SodiumUI: %s is locked; %s not saved", qPrintable(m_settingsPath), qPrintable(key));
        return false;
    }
    bool exists = false;
    const QByteArray current = readAll(m_settingsPath, &exists);
    if (exists && !QFileInfo(m_settingsPath).isReadable())
        return false;
    QJsonObject user;
    if (exists && !current.trimmed().isEmpty()) {
        QJsonParseError err{};
        const QJsonDocument doc = QJsonDocument::fromJson(current, &err);
        if (err.error != QJsonParseError::NoError || !doc.isObject()) {
            // Never replace a file we can't read: it may hold the user's other settings.
            qWarning("SodiumUI: %s can't be read (%s); %s not saved", qPrintable(m_settingsPath),
                     qPrintable(err.errorString()), qPrintable(key));
            return false;
        }
        user = doc.object();
    }
    // undefined or null removes the key: the setting is back to its default (decision 95: Automatic clock).
    if (!value.isValid() || value.metaType().id() == QMetaType::Nullptr)
        user.remove(key);
    else
        user.insert(key, QJsonValue::fromVariant(value));
    const QByteArray bytes = QJsonDocument(user).toJson(QJsonDocument::Indented);
    QSaveFile f(m_settingsPath);
    if (!f.open(QIODevice::WriteOnly) || f.write(bytes) != bytes.size() || !f.commit())
        return false;
    m_settingsBytes = bytes;
    m_userSettings = user.toVariantMap();
    rewatch();
    mergeSettings();
    return true;
}
