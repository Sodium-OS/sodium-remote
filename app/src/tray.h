// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// The notification-area icon: three states, off (dim), on (solid) and being controlled (accent with
// a lit dot). A hidden window receives Shell_NotifyIcon's callbacks; a click or a right click is reported with the
// icon's position, for the QML tray menu to open beside it. The menu itself is ours (Acrylic), not a Win32 menu.
#pragma once
#include <QObject>
#include <QPoint>
#include <QRect>

class Tray : public QObject
{
    Q_OBJECT
public:
    explicit Tray(QObject *parent = nullptr);
    ~Tray() override;

    /// "off", "on" or "controlled".
    void setState(const QString &state);
    void setToolTip(const QString &text);
    /// The icon's rectangle on screen (physical pixels), or an empty rect.
    QRect geometry() const;

    /// A second launch asks the running one to show its window (and to confirm `link`, a sodium-remote:// server link,
    /// when it has one): true when one is running.
    static bool pingRunning(const QString &link = QString());

Q_SIGNALS:
    void activated(const QPoint &anchor, bool contextMenu);
    void raiseRequested();
    /// A server link another launch was given (WM_COPYDATA).
    void linkReceived(const QString &link);

private:
    struct Impl;
    Impl *d;
};
