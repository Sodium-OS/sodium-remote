// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// SodiumwmWindow: what one of our windows needs from sodiumwm, SodiumOS's window manager
// (docs/decisions/sodiumwm-api.md, "Our apps' title bars"). WindowControls makes one.
//
//     SodiumwmWindow { id: wm; window: Window.window }
//     wm.running           sodiumwm runs: its state file is there, under Hyprland
//     wm.address           this window's Hyprland address ("0x…"), once found
//     wm.maximised         sodiumwm has it maximised (state.json's `maximised`). Use it
//                          instead of the toplevel state while running && address:
//                          Hyprland tells tiled windows they are maximised.
//     wm.toggleMaximise()  sodiumwm.api.toggleMaximise(address); false when not running
//     wm.onTop             sodiumwm keeps it on top (state.json's `onTop`); false when not running
//     wm.toggleOnTop()     sodiumwm.api.keepOnTop(address, !onTop) (the pin, P3TitleBars B6);
//                          false when not running
//     wm.showSnapGrid(x, y)  the shell's snap grid for this window, anchored at window
//                          point (x, y): `qs -p <shell> ipc call snap grid <address> <gx> <gy>`
//                          with the point in global logical px
//     wm.showWindowMenu(x, y)  sodiumwm's window menu (a right click on the title bar, decision 15):
//                          `qs -p <shell> ipc call windowmenu open <address> <gx> <gy>`, as the
//                          plugin's bars do
//
// The state file is ${XDG_RUNTIME_DIR}/sodiumwm/state.json (SODIUMWM_RUNTIME_DIR replaces the dir, for
// tests), watched like ThemeStore's files: a file sodiumwm replaces by a rename, or creates
// later, is still seen. It is stale, and sodiumwm counts as not running, when it was last written
// before this Hyprland instance started (sodiumwm writes it whenever it loads, so an older file is
// a previous session's: `require "sodiumwm"` failed this time), or when sodiumwm didn't answer a call
// since it was last written. Maximise then uses Hyprland's own maximise (fullscreen state 1).
//
// The address comes from Hyprland's socket (j/clients), among the windows of this
// process: the only one, or the only one with this window's title; among several, the
// active one while this window is active (it has focus-history 0); else the one under
// the pointer (hover and clicks pass it); else the one with this title and size.
#pragma once

#include <QDateTime>
#include <QFileSystemWatcher>
#include <QJsonObject>
#include <QObject>
#include <QPointer>
#include <QSet>
#include <QTimer>
#include <QWindow>

#include <functional>

/// One watched state file, shared by every SodiumwmWindow that reads it.
class SodiumwmState : public QObject
{
    Q_OBJECT

public:
    static SodiumwmState *forPath(const QString &path);

    QString path() const { return m_path; }
    /// There, readable, written by this Hyprland instance's sodiumwm, and sodiumwm has answered.
    bool present() const { return m_present && !m_stale && !m_unanswered; }
    bool isMaximised(const QString &address) const { return m_maximised.contains(address); }
    bool isOnTop(const QString &address) const { return m_onTop.contains(address); }
    /// sodiumwm didn't answer a call: treat the file as stale until sodiumwm writes it again.
    void markUnanswered();

    /// When the Hyprland instance this process runs under started (invalid when unknown):
    /// its lock file's time, else the time in HYPRLAND_INSTANCE_SIGNATURE.
    static QDateTime hyprlandStarted();
    /// A state file written at `written` is older than the instance that started at `started`.
    static bool isStale(const QDateTime &written, const QDateTime &started);

signals:
    void changed();

private:
    explicit SodiumwmState(const QString &path);
    void rewatch();
    void load();

    QString m_path;
    QFileSystemWatcher m_watcher;
    QTimer m_debounce;
    bool m_present = false;
    bool m_stale = false;
    bool m_unanswered = false;
    QDateTime m_written;
    QSet<QString> m_maximised;
    QSet<QString> m_onTop;
};

class SodiumwmWindow : public QObject
{
    Q_OBJECT
    Q_PROPERTY(QWindow *window READ window WRITE setWindow NOTIFY windowChanged)
    Q_PROPERTY(bool running READ running NOTIFY runningChanged)
    /// Found by itself (see the header); writable for tests.
    Q_PROPERTY(QString address READ address WRITE setAddress NOTIFY addressChanged)
    Q_PROPERTY(bool maximised READ maximised NOTIFY maximisedChanged)
    Q_PROPERTY(bool onTop READ onTop NOTIFY onTopChanged)
    /// The sodiumshell config dir for `qs -p`: $SODIUMSHELL_DIR, else the repo's shell/ next to
    /// SodiumUI, else ~/Code/SodiumOS/shell.
    Q_PROPERTY(QString shellPath READ shellPath WRITE setShellPath NOTIFY shellPathChanged)
    Q_PROPERTY(QString statePath READ statePath CONSTANT)

public:
    explicit SodiumwmWindow(QObject *parent = nullptr);

    QWindow *window() const { return m_window; }
    void setWindow(QWindow *w);
    bool running() const;
    QString address() const { return m_address; }
    void setAddress(const QString &a);
    bool maximised() const { return running() && !m_address.isEmpty() && m_state->isMaximised(m_address); }
    bool onTop() const { return running() && !m_address.isEmpty() && m_state->isOnTop(m_address); }
    QString shellPath() const { return m_shellPath; }
    void setShellPath(const QString &p);
    QString statePath() const { return m_state->path(); }

    /// Looks the address up again.
    Q_INVOKABLE void resolve();
    /// Asks sodiumwm to maximise or restore this window. False when sodiumwm isn't running
    /// (toggle the toplevel state instead); `fallbackNeeded()` follows if the window
    /// can't be found.
    Q_INVOKABLE bool toggleMaximise();
    /// Asks sodiumwm to keep this window on top, or to stop. False when sodiumwm isn't running
    /// (there is nothing else to ask: Hyprland has no always-on-top).
    Q_INVOKABLE bool toggleOnTop();
    /// Asks the shell for the snap grid under window point (x, y) (logical px).
    Q_INVOKABLE void showSnapGrid(qreal x, qreal y);
    /// Asks the shell for sodiumwm's window menu with its top-left at window point (x, y).
    Q_INVOKABLE void showWindowMenu(qreal x, qreal y);

    /// "0x" + lower-case hex.
    static QString normalise(const QString &address);
    /// The client of `pid` that is `window` among Hyprland's j/clients, as the header says.
    static QJsonObject pick(const QJsonArray &clients, qint64 pid, bool active, const QPointF *cursor,
                            const QString &title, const QSize &size);

signals:
    void windowChanged();
    void runningChanged();
    void addressChanged();
    void maximisedChanged();
    void onTopChanged();
    void shellPathChanged();
    void fallbackNeeded();

private:
    /// Finds this window's client (with the cursor position when `pointer`), then calls back
    /// with it (empty when not found).
    void withClient(bool pointer, std::function<void(const QJsonObject &)> done);
    void stateChanged();
    /// Hyprland's own maximise or restore of `client`; fallbackNeeded() if Hyprland refuses.
    void hyprlandMaximise(const QJsonObject &client);
    /// `qs -p <shell> ipc call <args>` for this window, at window point (x, y) in global px.
    void shellCall(const QString &target, const QString &function, qreal x, qreal y);

    QPointer<QWindow> m_window;
    SodiumwmState *m_state = nullptr;
    QString m_address;
    QString m_shellPath;
    bool m_running = false;
    bool m_maximised = false;
    bool m_onTop = false;
    QTimer m_retry;
    int m_retries = 0;
};
