// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// WindowChrome (Windows only): what AppWindow needs from DWM (docs/decisions/cross-platform.md, "Windows glass and leaving the stream").
//
//   - the frame: a frameless Qt window gets WS_THICKFRAME | WS_CAPTION back and its non-client area is removed in
//     WM_NCCALCSIZE, so DWM keeps the 8 px corners, the shadow and snapping;
//   - Mica (DWMSBT_MAINWINDOW) or Acrylic (DWMSBT_TRANSIENTWINDOW) behind a transparent window, from Windows 11
//     build 22621, and `DWMWA_USE_IMMERSIVE_DARK_MODE` following the theme; below that build, or with transparency
//     effects off (or Theme.reduceTransparency), `backdropActive` is false and AppWindow paints an opaque Graphite base;
//   - HTMAXBUTTON over the maximise button (snap layouts), with its hover and pressed states from the WM_NC* messages
//     (`maxState`), since QML no longer receives the pointer there;
//   - resize borders by hit test, and `opaque` windows (the stream frame) with no backdrop and square corners in full screen.
//
//     WindowChrome { window: win; backdrop: "mica"; dark: Theme.dark; maxButton: Qt.rect(x, y, w, h) }
#pragma once

#include <QAbstractNativeEventFilter>
#include <QObject>
#include <QPointer>
#include <QRectF>
#include <QWindow>

class WindowChrome : public QObject, public QAbstractNativeEventFilter
{
    Q_OBJECT
    Q_PROPERTY(QWindow *window READ window WRITE setWindow NOTIFY windowChanged)
    /// "mica", "acrylic" or "none" (an opaque window: the stream window).
    Q_PROPERTY(QString backdrop READ backdrop WRITE setBackdrop NOTIFY backdropChanged)
    Q_PROPERTY(bool dark READ dark WRITE setDark NOTIFY darkChanged)
    /// Content is drawn translucent over a DWM backdrop (false: paint an opaque base).
    Q_PROPERTY(bool backdropActive READ backdropActive NOTIFY backdropActiveChanged)
    /// The maximise button in window logical pixels; an empty rect for none.
    Q_PROPERTY(QRectF maxButton READ maxButton WRITE setMaxButton NOTIFY maxButtonChanged)
    /// "", "hover" or "pressed": the maximise button as the non-client messages see it.
    Q_PROPERTY(QString maxState READ maxState NOTIFY maxStateChanged)
    /// Resize by the window's edges (default on).
    Q_PROPERTY(bool resizable READ resizable WRITE setResizable NOTIFY resizableChanged)

public:
    explicit WindowChrome(QObject *parent = nullptr);
    ~WindowChrome() override;

    QWindow *window() const { return m_window; }
    void setWindow(QWindow *w);
    QString backdrop() const { return m_backdrop; }
    void setBackdrop(const QString &b);
    bool dark() const { return m_dark; }
    void setDark(bool d);
    bool backdropActive() const { return m_active; }
    QRectF maxButton() const { return m_maxButton; }
    void setMaxButton(const QRectF &r);
    QString maxState() const { return m_maxState; }
    bool resizable() const { return m_resizable; }
    void setResizable(bool r);

    /// Windows 11 build 22621 or later with "Transparency effects" on.
    static bool backdropsAvailable();

    bool nativeEventFilter(const QByteArray &type, void *message, qintptr *result) override;

Q_SIGNALS:
    void windowChanged();
    void backdropChanged();
    void darkChanged();
    void backdropActiveChanged();
    void maxButtonChanged();
    void maxStateChanged();
    void resizableChanged();

private:
    void attach();
    void apply();
    void setMaxState(const QString &s);

    QPointer<QWindow> m_window;
    quintptr m_hwnd = 0;
    QString m_backdrop = QStringLiteral("mica");
    bool m_dark = true;
    bool m_active = false;
    bool m_resizable = true;
    QRectF m_maxButton;
    QString m_maxState;
    bool m_tracking = false;
};
