// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "tray.h"

#include <QColor>
#include <QImage>
#include <QPainter>

#include <windows.h>
#include <shellapi.h>
#include <windowsx.h>

namespace {
constexpr UINT kCallback = WM_APP + 1;
constexpr UINT kId = 1;
constexpr UINT kRaise = WM_APP + 2;
/// COPYDATASTRUCT::dwData of a server link sent by a second launch ("SRLK").
constexpr ULONG_PTR kLinkData = 0x4B4C5253;

HICON iconFrom(const QImage &src)
{
    const QImage img = src.convertToFormat(QImage::Format_ARGB32);
    BITMAPV5HEADER bi{};
    bi.bV5Size = sizeof(bi);
    bi.bV5Width = img.width();
    bi.bV5Height = -img.height();
    bi.bV5Planes = 1;
    bi.bV5BitCount = 32;
    bi.bV5Compression = BI_BITFIELDS;
    bi.bV5RedMask = 0x00FF0000;
    bi.bV5GreenMask = 0x0000FF00;
    bi.bV5BlueMask = 0x000000FF;
    bi.bV5AlphaMask = 0xFF000000;
    void *bits = nullptr;
    HDC dc = GetDC(nullptr);
    HBITMAP color = CreateDIBSection(dc, reinterpret_cast<BITMAPINFO *>(&bi), DIB_RGB_COLORS, &bits, nullptr, 0);
    ReleaseDC(nullptr, dc);
    if (!color || !bits)
        return nullptr;
    memcpy(bits, img.constBits(), size_t(img.sizeInBytes()));
    HBITMAP mask = CreateBitmap(img.width(), img.height(), 1, 1, nullptr);
    ICONINFO ii{TRUE, 0, 0, mask, color};
    HICON icon = CreateIconIndirect(&ii);
    DeleteObject(color);
    DeleteObject(mask);
    return icon;
}

/// The taskbar is light (Settings > Personalization > Colors > "Choose your mode" for Windows: Light).
bool taskbarLight()
{
    DWORD v = 0, size = sizeof(v);
    if (RegGetValueW(HKEY_CURRENT_USER, L"Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize", L"SystemUsesLightTheme", RRF_RT_REG_DWORD, nullptr, &v, &size) != ERROR_SUCCESS)
        return false;
    return v != 0;
}

/// A monitor with a stand, in the state's colours: dim when off, solid when on, accent with a lit dot when controlled.
QImage glyph(const QString &state, int size)
{
    QImage img(size, size, QImage::Format_ARGB32_Premultiplied);
    img.fill(Qt::transparent);
    QPainter p(&img);
    p.setRenderHint(QPainter::Antialiasing);
    const double u = size / 16.0;
    const bool light = taskbarLight();
    QColor ink = state == QLatin1String("off") ? (light ? QColor(90, 95, 110, 150) : QColor(160, 165, 178, 150))
               : state == QLatin1String("controlled") ? (light ? QColor(30, 90, 210) : QColor(110, 168, 255))
               : (light ? QColor(30, 32, 42) : QColor(240, 242, 248));
    p.setPen(QPen(ink, 1.5 * u, Qt::SolidLine, Qt::RoundCap, Qt::RoundJoin));
    p.setBrush(Qt::NoBrush);
    p.drawRoundedRect(QRectF(1.5 * u, 2.5 * u, 13 * u, 8.5 * u), 1.5 * u, 1.5 * u);
    p.drawLine(QPointF(5.5 * u, 13.5 * u), QPointF(10.5 * u, 13.5 * u));
    p.drawLine(QPointF(8 * u, 11 * u), QPointF(8 * u, 13.5 * u));
    p.setPen(Qt::NoPen);
    if (state == QLatin1String("controlled")) {
        p.setBrush(QColor(110, 168, 255));
        p.drawEllipse(QRectF(5.5 * u, 4.8 * u, 5 * u, 4 * u));
    }
    if (state != QLatin1String("off")) {
        p.setBrush(QColor(52, 211, 153));
        p.drawEllipse(QRectF(11.2 * u, 0.4 * u, 4.2 * u, 4.2 * u));
    }
    return img;
}
} // namespace

struct Tray::Impl
{
    Tray *q = nullptr;
    HWND hwnd = nullptr;
    HICON icons[3] = {};
    QString state = QStringLiteral("off");
    QString tip = QStringLiteral("Sodium Remote");
    UINT created = 0;

    static LRESULT CALLBACK proc(HWND h, UINT m, WPARAM w, LPARAM l)
    {
        auto *d = reinterpret_cast<Impl *>(GetWindowLongPtrW(h, GWLP_USERDATA));
        if (d) {
            if (m == kCallback) {
                const UINT ev = LOWORD(l);
                const QPoint at(GET_X_LPARAM(w), GET_Y_LPARAM(w));
                if (ev == WM_LBUTTONUP || ev == NIN_SELECT || ev == NIN_KEYSELECT)
                    QMetaObject::invokeMethod(d->q, [d, at] { Q_EMIT d->q->activated(at, false); }, Qt::QueuedConnection);
                else if (ev == WM_RBUTTONUP || ev == WM_CONTEXTMENU)
                    QMetaObject::invokeMethod(d->q, [d, at] { Q_EMIT d->q->activated(at, true); }, Qt::QueuedConnection);
                return 0;
            }
            if (m == WM_COPYDATA) {
                const auto *cd = reinterpret_cast<const COPYDATASTRUCT *>(l);
                if (cd && cd->dwData == kLinkData && cd->lpData) {
                    const QString link = QString::fromUtf8(static_cast<const char *>(cd->lpData), int(cd->cbData));
                    QMetaObject::invokeMethod(d->q, [d, link] { Q_EMIT d->q->linkReceived(link); }, Qt::QueuedConnection);
                    return TRUE;
                }
            }
            if (m == kRaise) {
                QMetaObject::invokeMethod(d->q, [d] { Q_EMIT d->q->raiseRequested(); }, Qt::QueuedConnection);
                return 0;
            }
            if (m == d->created)
                d->add();
        }
        return DefWindowProcW(h, m, w, l);
    }

    int index() const { return state == QLatin1String("controlled") ? 2 : state == QLatin1String("on") ? 1 : 0; }

    NOTIFYICONDATAW data() const
    {
        NOTIFYICONDATAW n{};
        n.cbSize = sizeof(n);
        n.hWnd = hwnd;
        n.uID = kId;
        n.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        n.uCallbackMessage = kCallback;
        n.hIcon = icons[index()];
        wcsncpy(n.szTip, reinterpret_cast<const wchar_t *>(tip.utf16()), 127);
        return n;
    }

    void add()
    {
        NOTIFYICONDATAW n = data();
        Shell_NotifyIconW(NIM_ADD, &n);
        n.uVersion = NOTIFYICON_VERSION_4;
        Shell_NotifyIconW(NIM_SETVERSION, &n);
    }
};

Tray::Tray(QObject *parent)
    : QObject(parent), d(new Impl)
{
    d->q = this;
    HINSTANCE inst = GetModuleHandleW(nullptr);
    WNDCLASSEXW wc{};
    wc.cbSize = sizeof(wc);
    wc.lpfnWndProc = &Impl::proc;
    wc.hInstance = inst;
    wc.lpszClassName = L"SodiumRemoteTray";
    RegisterClassExW(&wc);
    d->hwnd = CreateWindowExW(0, L"SodiumRemoteTray", L"Sodium Remote", 0, 0, 0, 0, 0, HWND_MESSAGE, nullptr, inst, nullptr);
    SetWindowLongPtrW(d->hwnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(d));
    d->created = RegisterWindowMessageW(L"TaskbarCreated");
    int i = 0;
    for (const char *s : {"off", "on", "controlled"})
        d->icons[i++] = iconFrom(glyph(QString::fromLatin1(s), GetSystemMetrics(SM_CXSMICON)));
    d->add();
}

Tray::~Tray()
{
    NOTIFYICONDATAW n = d->data();
    Shell_NotifyIconW(NIM_DELETE, &n);
    SetWindowLongPtrW(d->hwnd, GWLP_USERDATA, 0);
    DestroyWindow(d->hwnd);
    for (HICON i : d->icons)
        if (i)
            DestroyIcon(i);
    delete d;
}

void Tray::setState(const QString &state)
{
    d->state = state;
    NOTIFYICONDATAW n = d->data();
    Shell_NotifyIconW(NIM_MODIFY, &n);
}

void Tray::setToolTip(const QString &text)
{
    d->tip = text;
    NOTIFYICONDATAW n = d->data();
    Shell_NotifyIconW(NIM_MODIFY, &n);
}

QRect Tray::geometry() const
{
    NOTIFYICONIDENTIFIER id{};
    id.cbSize = sizeof(id);
    id.hWnd = d->hwnd;
    id.uID = kId;
    RECT r;
    if (FAILED(Shell_NotifyIconGetRect(&id, &r)))
        return {};
    return QRect(r.left, r.top, r.right - r.left, r.bottom - r.top);
}

bool Tray::pingRunning(const QString &link)
{
    HWND other = FindWindowExW(HWND_MESSAGE, nullptr, L"SodiumRemoteTray", nullptr);
    if (!other)
        return false;
    PostMessageW(other, kRaise, 0, 0);
    if (!link.isEmpty()) {
        const QByteArray utf8 = link.toUtf8();
        COPYDATASTRUCT cd{kLinkData, DWORD(utf8.size()), const_cast<char *>(utf8.constData())};
        DWORD_PTR result = 0;
        SendMessageTimeoutW(other, WM_COPYDATA, 0, reinterpret_cast<LPARAM>(&cd), SMTO_ABORTIFHUNG, 2000, &result);
    }
    return true;
}
