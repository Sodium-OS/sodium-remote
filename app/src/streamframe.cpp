// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "streamframe.h"

#include <QGuiApplication>
#include <QKeyEvent>
#include <QWindow>

#include <windows.h>
#include <windowsx.h>

#include <cmath>

namespace {
constexpr DWORD kDarkMode = 20;
constexpr DWORD kCorner = 33;
constexpr DWORD kBackdrop = 38;

using DwmSet = HRESULT(WINAPI *)(HWND, DWORD, LPCVOID, DWORD);
DwmSet dwmSet()
{
    static DwmSet f = [] {
        HMODULE m = LoadLibraryW(L"dwmapi.dll");
        return m ? reinterpret_cast<DwmSet>(GetProcAddress(m, "DwmSetWindowAttribute")) : nullptr;
    }();
    return f;
}

UINT dpiOf(HWND h)
{
    using Fn = UINT(WINAPI *)(HWND);
    static Fn f = reinterpret_cast<Fn>(GetProcAddress(GetModuleHandleW(L"user32.dll"), "GetDpiForWindow"));
    return f ? f(h) : 96;
}

RECT monitorRect(HWND h)
{
    MONITORINFO mi{};
    mi.cbSize = sizeof(mi);
    GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), &mi);
    return mi.rcMonitor;
}

int frameThickness(HWND h)
{
    using Fn = int(WINAPI *)(int, UINT);
    static Fn f = reinterpret_cast<Fn>(GetProcAddress(GetModuleHandleW(L"user32.dll"), "GetSystemMetricsForDpi"));
    const UINT dpi = dpiOf(h);
    return f ? f(SM_CXSIZEFRAME, dpi) + f(SM_CXPADDEDBORDER, dpi) : 8;
}
} // namespace

struct StreamFrame::Impl
{
    StreamFrame *q = nullptr;
    QPointer<QWindow> hud;
    HWND frame = nullptr;
    HWND hudHwnd = nullptr;
    bool syncing = false;
    bool fullscreen = false;
    /// Where the frame goes when full screen ends (physical pixels), and whether it was maximised.
    RECT normalRect{};
    bool wasZoomed = false;
    QByteArray cls;

    static LRESULT CALLBACK proc(HWND h, UINT m, WPARAM w, LPARAM l)
    {
        auto *d = reinterpret_cast<Impl *>(GetWindowLongPtrW(h, GWLP_USERDATA));
        if (!d)
            return DefWindowProcW(h, m, w, l);
        return d->on(h, m, w, l);
    }

    LRESULT on(HWND h, UINT m, WPARAM w, LPARAM l)
    {
        switch (m) {
        case WM_NCCALCSIZE:
            if (w) {
                auto *p = reinterpret_cast<NCCALCSIZE_PARAMS *>(l);
                if (IsZoomed(h) && !fullscreen) {
                    const int f = frameThickness(h);
                    p->rgrc[0].left += f;
                    p->rgrc[0].right -= f;
                    p->rgrc[0].top += f;
                    p->rgrc[0].bottom -= f;
                }
                return 0;
            }
            break;
        case WM_GETMINMAXINFO: {
            // 640x420 logical, in this window's pixels.
            const double k = dpiOf(h) / 96.0;
            auto *mm = reinterpret_cast<MINMAXINFO *>(l);
            mm->ptMinTrackSize.x = int(std::ceil(640 * k));
            mm->ptMinTrackSize.y = int(std::ceil(420 * k));
            return 0;
        }
        case WM_DPICHANGED: {
            // Moved to a monitor with another scale: the size Windows suggests keeps the window the same logical size.
            const RECT r = fullscreen ? monitorRect(h) : *reinterpret_cast<RECT *>(l);
            if (!IsZoomed(h) || fullscreen)
                SetWindowPos(h, nullptr, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            return 0;
        }
        case WM_NCACTIVATE:
            return DefWindowProcW(h, m, w, -1);
        case WM_ACTIVATE:
            if (LOWORD(w) == WA_INACTIVE)
                QMetaObject::invokeMethod(q, [this] { Q_EMIT q->deactivated(); }, Qt::QueuedConnection);
            break;
        case WM_NCHITTEST: {
            if (fullscreen || IsZoomed(h))
                return HTCLIENT;
            RECT r;
            GetWindowRect(h, &r);
            const int x = GET_X_LPARAM(l) - r.left, y = GET_Y_LPARAM(l) - r.top;
            const int width = r.right - r.left, height = r.bottom - r.top;
            const double k = dpiOf(h) / 96.0;
            const int b = int(6 * k), c = int(12 * k);
            const bool left = x < b, right = x >= width - b, top = y < b, bottom = y >= height - b;
            if (top && x < c) return HTTOPLEFT;
            if (top && x >= width - c) return HTTOPRIGHT;
            if (bottom && x < c) return HTBOTTOMLEFT;
            if (bottom && x >= width - c) return HTBOTTOMRIGHT;
            if (left) return HTLEFT;
            if (right) return HTRIGHT;
            if (top) return HTTOP;
            if (bottom) return HTBOTTOM;
            return HTCLIENT;
        }
        case WM_ERASEBKGND: {
            RECT r;
            GetClientRect(h, &r);
            FillRect(reinterpret_cast<HDC>(w), &r, static_cast<HBRUSH>(GetStockObject(BLACK_BRUSH)));
            return 1;
        }
        case WM_SETFOCUS:
            // The keyboard belongs to the picture's window (the hook is installed while this frame is foreground).
            if (HWND child = GetWindow(h, GW_CHILD))
                SetFocus(child);
            return 0;
        case WM_WINDOWPOSCHANGED:
        case WM_SIZE:
            toHud();
            break;
        case WM_CLOSE:
            if (hud)
                QMetaObject::invokeMethod(hud, "close", Qt::QueuedConnection);
            return 0;
        case WM_SYSCOMMAND:
            if ((w & 0xFFF0) == SC_KEYMENU)
                return 0;
            break;
        default:
            break;
        }
        return DefWindowProcW(h, m, w, l);
    }

    /// The HUD covers the frame's client area exactly. Both are HWND rectangles, so this is all physical pixels;
    /// Qt's logical geometry is only what it derives from them.
    void toHud()
    {
        if (syncing || !hud || !hudHwnd || !IsWindow(hudHwnd))
            return;
        RECT cr;
        GetClientRect(frame, &cr);
        POINT p{0, 0};
        ClientToScreen(frame, &p);
        syncing = true;
        // Qt may answer a move to a monitor with another scale by resizing the HUD itself: once more settles it.
        for (int i = 0; i < 2; ++i) {
            RECT cur;
            GetWindowRect(hudHwnd, &cur);
            if (cur.left == p.x && cur.top == p.y && cur.right - cur.left == cr.right && cur.bottom - cur.top == cr.bottom)
                break;
            SetWindowPos(hudHwnd, nullptr, p.x, p.y, cr.right, cr.bottom, SWP_NOZORDER | SWP_NOACTIVATE);
        }
        syncing = false;
    }

    /// The HUD was moved or resized (its own bar drag, its edges): the frame follows. Never in or around full
    /// screen: the frame owns that geometry (Qt's own saved rectangle for the HUD is not to be trusted).
    void fromHud()
    {
        if (syncing || !hudHwnd || !IsWindow(hudHwnd) || fullscreen || !hud || (hud->windowStates() & Qt::WindowFullScreen))
            return;
        RECT r;
        GetWindowRect(hudHwnd, &r);
        RECT cur;
        if (IsZoomed(frame)) {
            // The HUD sits on the client area of a maximised frame: that is not a move.
            GetClientRect(frame, &cur);
            POINT p{0, 0};
            ClientToScreen(frame, &p);
            OffsetRect(&cur, p.x, p.y);
        } else {
            GetWindowRect(frame, &cur);
        }
        if (cur.left == r.left && cur.top == r.top && cur.right == r.right && cur.bottom == r.bottom)
            return;
        syncing = true;
        if (IsZoomed(frame))
            ShowWindow(frame, SW_RESTORE);
        SetWindowPos(frame, nullptr, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
        syncing = false;
        toHud();
    }

    /// Full screen: the monitor the frame is on, borderless; leaving puts the frame back where it was.
    void setFullscreen(bool on)
    {
        if (fullscreen == on)
            return;
        if (on) {
            wasZoomed = IsZoomed(frame);
            if (wasZoomed) {
                WINDOWPLACEMENT wp{};
                wp.length = sizeof(wp);
                GetWindowPlacement(frame, &wp);
                normalRect = wp.rcNormalPosition;
                ShowWindow(frame, SW_RESTORE);
            } else {
                GetWindowRect(frame, &normalRect);
            }
        }
        fullscreen = on;
        LONG_PTR style = GetWindowLongPtrW(frame, GWL_STYLE);
        const LONG_PTR deco = WS_THICKFRAME | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;
        style = on ? (style & ~deco) : (style | deco);
        SetWindowLongPtrW(frame, GWL_STYLE, style);
        const int corner = on ? 1 : 2; // DWMWCP_DONOTROUND in full screen, round otherwise
        if (auto set = dwmSet())
            set(frame, kCorner, &corner, sizeof(corner));
        const RECT r = on ? monitorRect(frame) : normalRect;
        SetWindowPos(frame, on ? HWND_TOP : nullptr, r.left, r.top, r.right - r.left, r.bottom - r.top,
                     SWP_FRAMECHANGED | SWP_NOACTIVATE | (on ? 0 : SWP_NOZORDER));
        if (!on && wasZoomed)
            ShowWindow(frame, SW_MAXIMIZE);
        toHud();
    }
};

StreamFrame::StreamFrame(QWindow *hud, const QString &title, QObject *parent)
    : QObject(parent), d(new Impl)
{
    d->q = this;
    d->hud = hud;
    d->cls = QByteArrayLiteral("SodiumRemoteStreamFrame");
    HINSTANCE inst = GetModuleHandleW(nullptr);
    WNDCLASSEXW wc{};
    wc.cbSize = sizeof(wc);
    wc.lpfnWndProc = &Impl::proc;
    wc.hInstance = inst;
    wc.hCursor = LoadCursorW(nullptr, IDC_ARROW);
    wc.hbrBackground = static_cast<HBRUSH>(GetStockObject(BLACK_BRUSH));
    wc.lpszClassName = L"SodiumRemoteStreamFrame";
    RegisterClassExW(&wc);

    hud->create();
    d->hudHwnd = reinterpret_cast<HWND>(hud->winId());
    // The frame starts where the HUD is.
    RECT r;
    GetWindowRect(d->hudHwnd, &r);
    const std::wstring wtitle = title.toStdWString();
    d->frame = CreateWindowExW(WS_EX_APPWINDOW, L"SodiumRemoteStreamFrame", wtitle.c_str(), WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN, r.left, r.top, r.right - r.left, r.bottom - r.top, nullptr, nullptr, inst, nullptr);
    SetWindowLongPtrW(d->frame, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(d));
    // Opaque, no backdrop, immersive dark: the picture is never under translucency.
    if (auto set = dwmSet()) {
        const BOOL dark = TRUE;
        set(d->frame, kDarkMode, &dark, sizeof(dark));
        const int none = 1;
        set(d->frame, kBackdrop, &none, sizeof(none));
        const int round = 2;
        set(d->frame, kCorner, &round, sizeof(round));
    }
    SetWindowPos(d->frame, nullptr, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);

    // The HUD: owned by the frame (always above it, minimised with it), a tool window that never takes the
    // activation, so the frame stays foreground and the keyboard hook keeps the keys.
    LONG_PTR ex = GetWindowLongPtrW(d->hudHwnd, GWL_EXSTYLE);
    SetWindowLongPtrW(d->hudHwnd, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW);
    SetWindowLongPtrW(d->hudHwnd, GWLP_HWNDPARENT, reinterpret_cast<LONG_PTR>(d->frame));

    auto follow = [this] { d->fromHud(); };
    connect(hud, &QWindow::xChanged, this, follow);
    connect(hud, &QWindow::yChanged, this, follow);
    connect(hud, &QWindow::widthChanged, this, follow);
    connect(hud, &QWindow::heightChanged, this, follow);
    connect(hud, &QWindow::visibilityChanged, this, [this](QWindow::Visibility v) {
        d->setFullscreen(v == QWindow::FullScreen);
    });
    connect(hud, &QWindow::visibleChanged, this, [this](bool v) {
        if (!v && d->frame)
            ShowWindow(d->frame, SW_HIDE);
        else if (v && d->frame) {
            if (!d->fullscreen)
                d->fromHud();
            ShowWindow(d->frame, SW_SHOW);
            SetForegroundWindow(d->frame);
        }
    });
    if (hud->visibility() == QWindow::FullScreen) {
        // Opened in full screen: the windowed rectangle to come back to is the middle 80% of the monitor.
        const RECT m = monitorRect(d->frame);
        const int w = m.right - m.left, h = m.bottom - m.top;
        d->normalRect = {m.left + w / 10, m.top + h / 10, m.right - w / 10, m.bottom - h / 10};
        ShowWindow(d->frame, SW_SHOW);
        d->setFullscreen(true);
    } else {
        ShowWindow(d->frame, SW_SHOW);
    }
    SetForegroundWindow(d->frame);
    d->toHud();
}

StreamFrame::~StreamFrame()
{
    if (d->hudHwnd && IsWindow(d->hudHwnd))
        SetWindowLongPtrW(d->hudHwnd, GWLP_HWNDPARENT, 0);
    if (d->frame) {
        SetWindowLongPtrW(d->frame, GWLP_USERDATA, 0);
        DestroyWindow(d->frame);
    }
    delete d;
}

void *StreamFrame::hwnd() const
{
    return d->frame;
}

void StreamFrame::setTitle(const QString &title)
{
    SetWindowTextW(d->frame, title.toStdWString().c_str());
}

void StreamFrame::sendMenuKey(unsigned vk)
{
    int key = 0;
    switch (vk) {
    case VK_ESCAPE: key = Qt::Key_Escape; break;
    case VK_UP: key = Qt::Key_Up; break;
    case VK_DOWN: key = Qt::Key_Down; break;
    case VK_RETURN: key = Qt::Key_Return; break;
    case VK_SPACE: key = Qt::Key_Space; break;
    default: return;
    }
    if (!d->hud)
        return;
    QKeyEvent press(QEvent::KeyPress, key, Qt::NoModifier);
    QKeyEvent release(QEvent::KeyRelease, key, Qt::NoModifier);
    QCoreApplication::sendEvent(d->hud, &press);
    QCoreApplication::sendEvent(d->hud, &release);
}

void StreamFrame::setRegion(const QVariantList &rects)
{
    if (!d->hudHwnd || !d->hud)
        return;
    const double k = d->hud->devicePixelRatio();
    HRGN all = CreateRectRgn(0, 0, 0, 0);
    for (const QVariant &v : rects) {
        const QVariantMap m = v.toMap();
        const int x = int(std::floor(m.value(QStringLiteral("x")).toDouble() * k));
        const int y = int(std::floor(m.value(QStringLiteral("y")).toDouble() * k));
        const int w = int(std::ceil(m.value(QStringLiteral("w")).toDouble() * k));
        const int h = int(std::ceil(m.value(QStringLiteral("h")).toDouble() * k));
        HRGN r = CreateRectRgn(x, y, x + w, y + h);
        CombineRgn(all, all, r, RGN_OR);
        DeleteObject(r);
    }
    SetWindowRgn(d->hudHwnd, all, TRUE); // the window owns the region now
}
