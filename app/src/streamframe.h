// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// The Windows stream frame: an opaque top-level Win32 window that the engine's
// D3D11 swapchain sits on, as a child HWND. The stream window's QML (the bar, cards and toasts) is another,
// owned, transparent top-level Qt window above it, the "HUD"; its region covers only what the app draws, so the
// pointer reaches the picture everywhere else, and the two windows share one rectangle.
//
// The frame keeps WS_THICKFRAME | WS_CAPTION with the non-client area removed in WM_NCCALCSIZE (DWM corners, shadow,
// snapping), no backdrop and an opaque Graphite-black client, and DWMWCP_DONOTROUND in full screen.
#pragma once

#include <QObject>
#include <QPointer>
#include <QRect>
#include <QVariantList>

class QWindow;

class StreamFrame : public QObject
{
    Q_OBJECT
public:
    /// `hud` is the stream window's QML window. The frame is created hidden at its position.
    explicit StreamFrame(QWindow *hud, const QString &title, QObject *parent = nullptr);
    ~StreamFrame() override;

    /// The HWND the engine parents its child window to.
    void *hwnd() const;
    void setTitle(const QString &title);
    /// Opening the window: the HUD goes to `logical` (Qt's pixels) and the frame with it. Otherwise the frame owns
    /// the geometry and the HUD follows it.
    void place(const QRect &logical);
    /// What the app draws over the picture, HUD-window logical pixels: the HUD window's region.
    void setRegion(const QVariantList &rects);
    /// A key the engine saw while an app menu is open (the keyboard has no Qt focus: the frame is foreground and the
    /// HUD never activates). Escape, the arrows, Enter and Space go to the HUD as key events.
    void sendMenuKey(unsigned vk);

Q_SIGNALS:
    /// The frame lost the foreground (Alt+Tab, another window): an open menu closes.
    void deactivated();

private:
    friend struct StreamFrameImpl;
    struct Impl;
    Impl *d;
};
