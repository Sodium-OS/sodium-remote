// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "windowchrome.h"

#include <QCoreApplication>
#include <QGuiApplication>

#include <windows.h>
#include <windowsx.h>

#include <algorithm>

// dwmapi.h in mingw-w64 lacks the Windows 11 attributes: the values are the SDK's.
namespace {
constexpr DWORD kUseImmersiveDarkMode = 20;
constexpr DWORD kWindowCornerPreference = 33;
constexpr DWORD kSystemBackdropType = 38;
constexpr int kBackdropNone = 1;
constexpr int kBackdropMain = 2;       // Mica
constexpr int kBackdropTransient = 3;  // Acrylic
constexpr int kCornerDoNotRound = 1;
constexpr int kCornerRound = 2;

struct Margins
{
    int l, r, t, b;
};

using DwmSetAttr = HRESULT(WINAPI *)(HWND, DWORD, LPCVOID, DWORD);
using DwmExtend = HRESULT(WINAPI *)(HWND, const Margins *);

DwmSetAttr dwmSetAttr()
{
    static DwmSetAttr f = [] {
        HMODULE m = LoadLibraryW(L"dwmapi.dll");
        return m ? reinterpret_cast<DwmSetAttr>(GetProcAddress(m, "DwmSetWindowAttribute")) : nullptr;
    }();
    return f;
}

DwmExtend dwmExtend()
{
    static DwmExtend f = [] {
        HMODULE m = LoadLibraryW(L"dwmapi.dll");
        return m ? reinterpret_cast<DwmExtend>(GetProcAddress(m, "DwmExtendFrameIntoClientArea")) : nullptr;
    }();
    return f;
}

DWORD buildNumber()
{
    using Fn = LONG(WINAPI *)(OSVERSIONINFOW *);
    static DWORD build = [] {
        auto f = reinterpret_cast<Fn>(GetProcAddress(GetModuleHandleW(L"ntdll.dll"), "RtlGetVersion"));
        OSVERSIONINFOW v{};
        v.dwOSVersionInfoSize = sizeof(v);
        return f && f(&v) == 0 ? v.dwBuildNumber : 0;
    }();
    return build;
}

bool transparencyOn()
{
    DWORD v = 1, size = sizeof(v);
    if (RegGetValueW(HKEY_CURRENT_USER, L"Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize", L"EnableTransparency", RRF_RT_REG_DWORD, nullptr, &v, &size) != ERROR_SUCCESS)
        return true;
    return v != 0;
}

UINT dpiFor(HWND h)
{
    using Fn = UINT(WINAPI *)(HWND);
    static Fn f = reinterpret_cast<Fn>(GetProcAddress(GetModuleHandleW(L"user32.dll"), "GetDpiForWindow"));
    return f ? f(h) : 96;
}

int frameFor(UINT dpi)
{
    using Fn = int(WINAPI *)(int, UINT);
    static Fn f = reinterpret_cast<Fn>(GetProcAddress(GetModuleHandleW(L"user32.dll"), "GetSystemMetricsForDpi"));
    if (!f)
        return GetSystemMetrics(SM_CXSIZEFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER);
    return f(SM_CXSIZEFRAME, dpi) + f(SM_CXPADDEDBORDER, dpi);
}
} // namespace

WindowChrome::WindowChrome(QObject *parent)
    : QObject(parent)
{
    QCoreApplication::instance()->installNativeEventFilter(this);
}

WindowChrome::~WindowChrome()
{
    if (QCoreApplication::instance())
        QCoreApplication::instance()->removeNativeEventFilter(this);
}

bool WindowChrome::backdropsAvailable()
{
    return buildNumber() >= 22621 && transparencyOn();
}

void WindowChrome::setWindow(QWindow *w)
{
    if (m_window == w)
        return;
    if (m_window)
        disconnect(m_window, nullptr, this, nullptr);
    m_window = w;
    m_hwnd = 0;
    if (w) {
        connect(w, &QWindow::visibleChanged, this, [this] { attach(); });
        connect(w, &QWindow::visibilityChanged, this, [this] { apply(); });
        attach();
    }
    Q_EMIT windowChanged();
}

void WindowChrome::attach()
{
    if (!m_window || !m_window->isVisible())
        return;
    m_hwnd = quintptr(m_window->winId());
    apply();
}

void WindowChrome::setBackdrop(const QString &b)
{
    if (m_backdrop == b)
        return;
    m_backdrop = b;
    apply();
    Q_EMIT backdropChanged();
}

void WindowChrome::setDark(bool d)
{
    if (m_dark == d)
        return;
    m_dark = d;
    apply();
    Q_EMIT darkChanged();
}

void WindowChrome::setMaxButton(const QRectF &r)
{
    if (m_maxButton == r)
        return;
    m_maxButton = r;
    Q_EMIT maxButtonChanged();
}

void WindowChrome::setResizable(bool r)
{
    if (m_resizable == r)
        return;
    m_resizable = r;
    Q_EMIT resizableChanged();
}

void WindowChrome::setMaxState(const QString &s)
{
    if (m_maxState == s)
        return;
    m_maxState = s;
    Q_EMIT maxStateChanged();
}

void WindowChrome::apply()
{
    if (!m_hwnd || !m_window)
        return;
    HWND h = reinterpret_cast<HWND>(m_hwnd);
    // The frame: caption and thick frame back (snap, shadow, DWM corners); WM_NCCALCSIZE removes them from the client.
    LONG_PTR style = GetWindowLongPtrW(h, GWL_STYLE);
    const bool full = m_window->visibility() == QWindow::FullScreen;
    const LONG_PTR want = WS_THICKFRAME | WS_CAPTION | WS_MINIMIZEBOX | WS_MAXIMIZEBOX; // no WS_SYSMENU: DWM would draw its own caption buttons into the transparent client
    if (!full && (style & want) != want) {
        SetWindowLongPtrW(h, GWL_STYLE, style | want);
        SetWindowPos(h, nullptr, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
    const bool wantBackdrop = (m_backdrop == QLatin1String("mica") || m_backdrop == QLatin1String("acrylic")) && backdropsAvailable();
    const BOOL dark = m_dark;
    if (auto set = dwmSetAttr()) {
        set(h, kUseImmersiveDarkMode, &dark, sizeof(dark));
        const int corner = full ? kCornerDoNotRound : kCornerRound;
        set(h, kWindowCornerPreference, &corner, sizeof(corner));
        const int type = wantBackdrop ? (m_backdrop == QLatin1String("acrylic") ? kBackdropTransient : kBackdropMain) : kBackdropNone;
        set(h, kSystemBackdropType, &type, sizeof(type));
    }
    if (auto ext = dwmExtend()) {
        const Margins m = wantBackdrop ? Margins{-1, -1, -1, -1} : Margins{0, 0, 0, 0};
        ext(h, &m);
    }
    if (m_active != wantBackdrop) {
        m_active = wantBackdrop;
        Q_EMIT backdropActiveChanged();
    }
}

bool WindowChrome::nativeEventFilter(const QByteArray &type, void *message, qintptr *result)
{
    if (!m_hwnd || type != "windows_generic_MSG")
        return false;
    // Qt may call the filter without a result slot (a hook that only watches): never write through null.
    qintptr unused = 0;
    if (!result)
        result = &unused;
    MSG *msg = static_cast<MSG *>(message);
    if (reinterpret_cast<quintptr>(msg->hwnd) != m_hwnd)
        return false;
    HWND h = msg->hwnd;
    switch (msg->message) {
    case WM_NCCALCSIZE: {
        if (!msg->wParam)
            return false;
        auto *p = reinterpret_cast<NCCALCSIZE_PARAMS *>(msg->lParam);
        if (IsZoomed(h)) {
            // A maximised thick-frame window overhangs the monitor by its frame: pull the client back in.
            const int f = frameFor(dpiFor(h));
            p->rgrc[0].left += f;
            p->rgrc[0].right -= f;
            p->rgrc[0].top += f;
            p->rgrc[0].bottom -= f;
        }
        *result = 0;
        return true;
    }
    case WM_NCACTIVATE:
        // No classic caption to repaint.
        *result = DefWindowProcW(h, msg->message, msg->wParam, -1);
        return true;
    case WM_NCHITTEST: {
        RECT wr;
        GetWindowRect(h, &wr);
        const int x = GET_X_LPARAM(msg->lParam) - wr.left;
        const int y = GET_Y_LPARAM(msg->lParam) - wr.top;
        const int w = wr.right - wr.left;
        const int ht = wr.bottom - wr.top;
        const UINT dpi = dpiFor(h);
        const qreal k = dpi / 96.0;
        if (!m_maxButton.isEmpty()) {
            const QRectF r(m_maxButton.x() * k, m_maxButton.y() * k, m_maxButton.width() * k, m_maxButton.height() * k);
            if (r.contains(x, y)) {
                *result = HTMAXBUTTON;
                return true;
            }
        }
        if (m_resizable && !IsZoomed(h) && m_window && m_window->visibility() != QWindow::FullScreen) {
            const int b = int(6 * k), c = int(12 * k);
            const bool left = x < b, right = x >= w - b, top = y < b, bottom = y >= ht - b;
            int code = 0;
            if (top && x < c)
                code = HTTOPLEFT;
            else if (top && x >= w - c)
                code = HTTOPRIGHT;
            else if (bottom && x < c)
                code = HTBOTTOMLEFT;
            else if (bottom && x >= w - c)
                code = HTBOTTOMRIGHT;
            else if (left && y < c)
                code = HTTOPLEFT;
            else if (left && y >= ht - c)
                code = HTBOTTOMLEFT;
            else if (right && y < c)
                code = HTTOPRIGHT;
            else if (right && y >= ht - c)
                code = HTBOTTOMRIGHT;
            else if (left)
                code = HTLEFT;
            else if (right)
                code = HTRIGHT;
            else if (top)
                code = HTTOP;
            else if (bottom)
                code = HTBOTTOM;
            if (code) {
                *result = code;
                return true;
            }
        }
        return false;
    }
    case WM_NCMOUSEMOVE:
        if (msg->wParam == HTMAXBUTTON) {
            if (!m_tracking) {
                TRACKMOUSEEVENT t{sizeof(t), TME_LEAVE | TME_NONCLIENT, h, 0};
                TrackMouseEvent(&t);
                m_tracking = true;
            }
            if (m_maxState != QLatin1String("pressed"))
                setMaxState(QStringLiteral("hover"));
            *result = 0;
            return true;
        }
        setMaxState(QString());
        return false;
    case WM_NCMOUSELEAVE:
        m_tracking = false;
        setMaxState(QString());
        return false;
    case WM_NCLBUTTONDOWN:
        if (msg->wParam == HTMAXBUTTON) {
            setMaxState(QStringLiteral("pressed"));
            *result = 0;
            return true;
        }
        return false;
    case WM_NCLBUTTONUP:
        if (msg->wParam == HTMAXBUTTON) {
            setMaxState(QStringLiteral("hover"));
            ShowWindow(h, IsZoomed(h) ? SW_RESTORE : SW_MAXIMIZE);
            *result = 0;
            return true;
        }
        return false;
    case WM_SETTINGCHANGE:
    case WM_THEMECHANGED:
        apply();
        return false;
    default:
        return false;
    }
}
