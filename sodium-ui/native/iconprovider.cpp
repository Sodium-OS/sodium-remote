// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// librsvg before Qt: gio's headers have a member called `signals`, which Qt's headers turn into a keyword.
// Not in the portable build (Windows, the Mac): Qt SVG draws every icon there.
#ifndef SODIUMUI_PORTABLE
#include <librsvg/rsvg.h>
#endif

#include "iconprovider.h"

#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QDateTime>
#include <QHash>
#include <QIcon>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QMutex>
#include <QPainter>
#include <QRegularExpression>
#include <QStandardPaths>
#include <QSvgRenderer>
#include <QUrl>

#include <array>
#include <cmath>

namespace {

QStringList themeRoots()
{
    QStringList roots;
    for (const QString &base : QStandardPaths::standardLocations(QStandardPaths::GenericDataLocation)) {
        const QString dir = base + QStringLiteral("/icons/SodiumOS");
        if (QFileInfo(dir).isDir() && !roots.contains(dir))
            roots << dir;
    }
    // After the system's theme: an app's own symbolic icons (a standalone app embeds them in its resources and lists
    // the folder in QIcon::fallbackSearchPaths(); the folder holds symbolic/<size>/<name>.svg like the theme).
    for (const QString &dir : QIcon::fallbackSearchPaths())
        if (QFileInfo(dir + QStringLiteral("/symbolic")).isDir() && !roots.contains(dir))
            roots << dir;
    return roots;
}

// The symbolic drawings exist at 16, 20 and 24 px; prefer the one drawn for this size.
QList<int> symbolicBuckets(int logical)
{
    if (logical <= 17)
        return {16, 20, 24};
    if (logical <= 21)
        return {20, 24, 16};
    return {24, 20, 16};
}

QString findSymbolic(const QString &name, int logical)
{
    const QString file = name.endsWith(QLatin1String("-symbolic")) ? name : name + QStringLiteral("-symbolic");
    for (const QString &root : themeRoots())
        for (int b : symbolicBuckets(logical)) {
            const QString p = QStringLiteral("%1/symbolic/%2/%3.svg").arg(root).arg(b).arg(file);
            if (QFileInfo::exists(p))
                return p;
        }
    return {};
}

QString findColour(const QString &name)
{
    static const QStringList contexts = {QStringLiteral("apps"), QStringLiteral("places"), QStringLiteral("mimetypes")};
    for (const QString &root : themeRoots())
        for (const QString &ctx : contexts) {
            const QString p = QStringLiteral("%1/scalable/%2/%3.svg").arg(root, ctx, name);
            if (QFileInfo::exists(p))
                return p;
        }
    return {};
}

QString findApp(const QString &name, bool light)
{
    if (light) {
        const QString p = findColour(name + QStringLiteral("-light"));
        if (!p.isEmpty())
            return p;
    }
    return findColour(name);
}

QImage renderSvg(QByteArray svg, const QSize &px, const QColor &color)
{
    if (color.isValid()) {
        const QByteArray rgb = color.name(QColor::HexRgb).toLatin1();
        static const QRegularExpression scheme(QStringLiteral(R"((\.ColorScheme-Text\s*\{\s*color\s*:\s*)#[0-9A-Fa-f]{3,8})"));
        svg = QString::fromUtf8(svg).replace(scheme, QStringLiteral("\\1") + QString::fromLatin1(rgb)).toUtf8();
        svg.replace("currentColor", rgb);
    }
    QSvgRenderer renderer(svg);
    QImage img(px, QImage::Format_ARGB32_Premultiplied);
    img.fill(Qt::transparent);
    if (!renderer.isValid())
        return img;
    renderer.setAspectRatioMode(Qt::KeepAspectRatio);
    QPainter p(&img);
    p.setRenderHint(QPainter::Antialiasing);
    if (color.isValid())
        p.setOpacity(color.alphaF());
    renderer.render(&p, QRectF(QPointF(0, 0), px));
    return img;
}

QImage renderFile(const QString &path, const QSize &px)
{
    if (path.endsWith(QLatin1String(".svg"), Qt::CaseInsensitive)) {
        QFile f(path);
        return f.open(QIODevice::ReadOnly) ? renderSvg(f.readAll(), px, QColor()) : QImage();
    }
    QImage img(path);
    return img.isNull() ? img : img.scaled(px, Qt::KeepAspectRatio, Qt::SmoothTransformation)
                                     .convertToFormat(QImage::Format_ARGB32_Premultiplied);
}

// ---------------------------------------------------------------- apps we don't draw, on our tile
//
// docs/decisions/icons.md G15, N6. The app's own icon goes onto our tile, as the brand tiles in
// themes/icons/src/brand are made (themes/icons/tools/draw_icons.py, brand_icon): a logo that brings
// its own tile is clipped to ours, over its edge colour, and covers it by 7 %; any other logo stands
// free on the theme's tile (graphite, or white on the light theme), or on the other one where the
// theme's would swallow it. The tile pieces are drawn by themes/icons/tools/tiles/art_runtime.py, on
// the 64-unit canvas whose tile is the 56-unit square at (4, 4).

constexpr int Probe = 256;               // the size the app's own icon is read at
constexpr qreal TileSide = 56.0;         // of 64
constexpr qreal Overscan = 1.07;         // logos with their own tile cover ours by 7 %
constexpr qreal LogoSide = 36.0;         // free-standing logos: their drawn part fills 36 of the 64 units
constexpr qreal SeenShare = 0.25;        // a logo holds on a tile where at least this much of it is seen

QImage piece(const QString &name, const QSize &px)
{
    const QString path = findColour(name);
    return path.isEmpty() ? QImage() : renderFile(path, px);
}

// Qt SVG ignores clipPath (G6), so an SVG that clips is drawn with everything it meant to cut away: OBS's
// two gradient squares in place of its disc, a black square behind GNOME's app icons. An app icon that
// clips is drawn by librsvg instead (N14).
bool clips(const QString &path)
{
    if (!path.endsWith(QLatin1String(".svg"), Qt::CaseInsensitive))
        return false;
    QFile f(path);
    return f.open(QIODevice::ReadOnly) && f.readAll().contains("clip-path");
}

// The SVG an icon theme holds for an app, when it clips. QIcon reads that file whenever no PNG directory
// matches the size asked for, which at a fractional scale is always.
QString clippedSvg(const QString &name)
{
    for (const QString &root : QIcon::themeSearchPaths())
        for (const QString &theme : {QIcon::themeName(), QStringLiteral("hicolor")}) {
            const QString svg = QStringLiteral("%1/%2/scalable/apps/%3.svg").arg(root, theme, name);
            if (QFileInfo::exists(svg))
                return clips(svg) ? svg : QString();
        }
    return {};
}

// librsvg's drawing of an SVG, fitted into px; null when librsvg can't read the file.
QImage renderClipped(const QString &path, const QSize &px)
{
#ifdef SODIUMUI_PORTABLE
    Q_UNUSED(path)
    Q_UNUSED(px)
    return {};
#else
    RsvgHandle *handle = rsvg_handle_new_from_file(QFile::encodeName(path).constData(), nullptr);
    if (!handle)
        return {};
    QImage img(px, QImage::Format_ARGB32_Premultiplied);   // cairo's ARGB32
    img.fill(Qt::transparent);
    cairo_surface_t *surface = cairo_image_surface_create_for_data(img.bits(), CAIRO_FORMAT_ARGB32, img.width(),
                                                                   img.height(), int(img.bytesPerLine()));
    cairo_t *cr = cairo_create(surface);
    const RsvgRectangle viewport = {0, 0, double(px.width()), double(px.height())};
    const bool drawn = rsvg_handle_render_document(handle, cr, &viewport, nullptr);
    cairo_destroy(cr);
    cairo_surface_destroy(surface);
    g_object_unref(handle);
    return drawn ? img : QImage();
#endif
}

// The app's own icon, as large as it comes (at most Probe px): an absolute path or a system theme name.
QImage ownIcon(const QString &name)
{
    const bool file = name.startsWith(QLatin1Char('/'));
    const QString clipped = !file ? clippedSvg(name) : clips(name) ? name : QString();
    if (!clipped.isEmpty()) {
        const QImage img = renderClipped(clipped, QSize(Probe, Probe));
        if (!img.isNull())
            return img.convertToFormat(QImage::Format_ARGB32);
    }
    if (file)
        return QFileInfo::exists(name) ? renderFile(name, QSize(Probe, Probe)) : QImage();
    const QIcon icon = QIcon::fromTheme(name);
    if (icon.isNull())
        return {};
    QImage img = icon.pixmap(QSize(Probe, Probe)).toImage();
    if (img.isNull())
        return img;
    if (img.width() < Probe && img.height() < Probe)
        img = img.scaled(Probe, Probe, Qt::KeepAspectRatio, Qt::SmoothTransformation);
    return img.convertToFormat(QImage::Format_ARGB32);
}

QRect alphaBounds(const QImage &img, int threshold)
{
    int x0 = img.width(), y0 = img.height(), x1 = -1, y1 = -1;
    for (int y = 0; y < img.height(); ++y) {
        const QRgb *row = reinterpret_cast<const QRgb *>(img.constScanLine(y));
        for (int x = 0; x < img.width(); ++x)
            if (qAlpha(row[x]) >= threshold) {
                x0 = qMin(x0, x);
                x1 = qMax(x1, x);
                y0 = qMin(y0, y);
                y1 = qMax(y1, y);
            }
    }
    return x1 < 0 ? QRect() : QRect(QPoint(x0, y0), QPoint(x1, y1));
}

// A logo that brings its own tile: opaque over at least 85 % of its opaque bounds, and those bounds
// close to all of what it draws.
bool bringsOwnTile(const QImage &img, const QRect &opaque, const QRect &drawn)
{
    if (opaque.isEmpty() || drawn.isEmpty())
        return false;
    qint64 n = 0;
    for (int y = opaque.top(); y <= opaque.bottom(); ++y) {
        const QRgb *row = reinterpret_cast<const QRgb *>(img.constScanLine(y));
        for (int x = opaque.left(); x <= opaque.right(); ++x)
            n += qAlpha(row[x]) >= 250;
    }
    const qreal area = qreal(opaque.width()) * opaque.height();
    return n >= 0.85 * area && area >= 0.8 * qreal(drawn.width()) * drawn.height()
        && qAbs(opaque.width() - opaque.height()) <= 0.15 * qMax(opaque.width(), opaque.height());
}

// The average colour just inside the top and bottom edges of a logo's own tile.
QColor edgeColour(const QImage &img, const QRect &r)
{
    const int inset = qMax(2, r.height() / 40);
    int acc[3] = {0, 0, 0}, n = 0;
    for (qreal fx : {0.35, 0.5, 0.65})
        for (int y : {r.top() + inset, r.bottom() - inset}) {
            const QRgb c = img.pixel(r.left() + int(fx * r.width()), y);
            acc[0] += qRed(c);
            acc[1] += qGreen(c);
            acc[2] += qBlue(c);
            ++n;
        }
    return QColor(acc[0] / n, acc[1] / n, acc[2] / n);
}

// Whether a free-standing logo holds on the graphite tile (or the white one): at least SeenShare of what it
// draws is seen there (N6). A pixel is seen when it has a colour of its own (OKLab chroma over 0.08) or is
// clearly lighter than graphite (lightness over 0.41; graphite is about 0.29) or darker than the white tile
// (under 0.83; white is about 0.95). draw_icons.py's holds_on applies the same rule to the brand tiles.
bool holdsOn(const QImage &img, bool graphite)
{
    static const std::array<float, 256> linear = [] {
        std::array<float, 256> t{};
        for (int i = 0; i < 256; ++i) {
            const float c = i / 255.0f;
            t[i] = c <= 0.04045f ? c / 12.92f : std::pow((c + 0.055f) / 1.055f, 2.4f);
        }
        return t;
    }();
    qint64 drawn = 0, seen = 0;
    for (int y = 0; y < img.height(); ++y) {
        const QRgb *row = reinterpret_cast<const QRgb *>(img.constScanLine(y));
        for (int x = 0; x < img.width(); ++x) {
            if (qAlpha(row[x]) < 128)
                continue;
            const float r = linear[qRed(row[x])], g = linear[qGreen(row[x])], b = linear[qBlue(row[x])];
            const float l = std::cbrt(0.4122214708f * r + 0.5363325363f * g + 0.0514459929f * b);
            const float m = std::cbrt(0.2119034982f * r + 0.6806995451f * g + 0.1073969566f * b);
            const float s = std::cbrt(0.0883024619f * r + 0.2817188376f * g + 0.6299787005f * b);
            const float lightness = 0.2104542553f * l + 0.7936177850f * m - 0.0040720468f * s;
            const float chroma = std::hypot(1.9779984951f * l - 2.4285922050f * m + 0.4505937099f * s,
                                            0.0259040371f * l + 0.7827717662f * m - 0.8086757660f * s);
            ++drawn;
            seen += chroma > 0.08f || (graphite ? lightness > 0.41f : lightness < 0.83f);
        }
    }
    return seen >= SeenShare * drawn;
}

// How a popular app's official logo sits on our tile, set by hand where the rule alone gets it wrong (N15):
// themes/icons/src/app-tiles.json, installed at the theme's root. Nothing is redrawn.
struct Tuning {
    QString dark, light;    // "graphite" or "white": the tile on that theme; empty: the rule decides
    bool own = false;       // the logo is its own rounded square: it covers our tile
    qreal side = LogoSide;  // free-standing: the units its drawn part fills
    qreal dx = 0, dy = 0;   // units: an optical nudge
};

Tuning tuningFor(const QString &name)
{
    static QMutex mutex;
    static QString loaded;
    static QDateTime stamp;
    static QHash<QString, Tuning> table;
    QString path;
    for (const QString &root : themeRoots()) {
        path = root + QStringLiteral("/app-tiles.json");
        if (QFileInfo::exists(path))
            break;
        path.clear();
    }
    const QDateTime modified = path.isEmpty() ? QDateTime() : QFileInfo(path).lastModified();
    QMutexLocker lock(&mutex);
    if (path != loaded || modified != stamp) {
        loaded = path;
        stamp = modified;
        table.clear();
        QFile f(path);
        if (!path.isEmpty() && f.open(QIODevice::ReadOnly)) {
            const QJsonArray apps = QJsonDocument::fromJson(f.readAll()).object().value(QLatin1String("apps")).toArray();
            for (const QJsonValue &v : apps) {
                const QJsonObject o = v.toObject();
                Tuning t;
                t.dark = o.value(QLatin1String("dark")).toString();
                t.light = o.value(QLatin1String("light")).toString();
                t.own = o.value(QLatin1String("own")).toBool();
                t.side = o.value(QLatin1String("side")).toDouble(LogoSide);
                t.dx = o.value(QLatin1String("dx")).toDouble();
                t.dy = o.value(QLatin1String("dy")).toDouble();
                for (const QJsonValue &n : o.value(QLatin1String("names")).toArray())
                    table.insert(n.toString(), t);
            }
        }
    }
    return table.value(name);
}

QImage onOurTile(const QImage &own, const QSize &px, bool light, const Tuning &tuning)
{
    const qreal u = px.width() / 64.0;
    QImage out(px, QImage::Format_ARGB32_Premultiplied);
    out.fill(Qt::transparent);
    QPainter p(&out);
    p.setRenderHints(QPainter::Antialiasing | QPainter::SmoothPixmapTransform);
    const QRect drawn = alphaBounds(own, 16);
    const QRect opaque = alphaBounds(own, 250);
    if (tuning.own ? !opaque.isEmpty() : bringsOwnTile(own, opaque, drawn)) {
        QImage layer(px, QImage::Format_ARGB32_Premultiplied);
        layer.fill(edgeColour(own, opaque));
        {
            QPainter lp(&layer);
            lp.setRenderHints(QPainter::Antialiasing | QPainter::SmoothPixmapTransform);
            const qreal k = TileSide * Overscan * u / opaque.width();
            const QPointF c = QRectF(opaque).center();
            lp.drawImage(QRectF(32 * u - c.x() * k, 32 * u - c.y() * k, own.width() * k, own.height() * k), own);
            lp.setCompositionMode(QPainter::CompositionMode_DestinationIn);
            lp.drawImage(0, 0, piece(QStringLiteral("sodiumshell-tile-mask"), px));
        }
        p.drawImage(0, 0, piece(QStringLiteral("sodiumshell-tile-shadow"), px));
        p.drawImage(0, 0, layer);
        p.drawImage(0, 0, piece(QStringLiteral("sodiumshell-tile-rims"), px));
    } else {
        const QString pinned = light ? tuning.light : tuning.dark;
        const bool white = pinned.isEmpty() ? (holdsOn(own, !light) ? light : !light) : pinned == QLatin1String("white");
        p.drawImage(0, 0, piece(!white ? QStringLiteral("sodiumshell-tile-graphite")
                                : light ? QStringLiteral("sodiumshell-tile-white-light")
                                        : QStringLiteral("sodiumshell-tile-white"), px));
        if (!drawn.isEmpty()) {
            const qreal k = tuning.side * u / qMax(drawn.width(), drawn.height());
            const QPointF c = QRectF(drawn).center();
            p.drawImage(QRectF((32 + tuning.dx) * u - c.x() * k, (32 + tuning.dy) * u - c.y() * k,
                               own.width() * k, own.height() * k), own);
        }
    }
    return out;
}

QImage appIcon(const QString &name, const QSize &px, bool light)
{
    static const QString generic = QStringLiteral("application-x-executable");
    if (name != generic && !name.startsWith(QLatin1Char('/'))) {
        const QString ours = findApp(name, light);
        if (!ours.isEmpty() && !ours.contains(QLatin1String("/mimetypes/")))
            return renderFile(ours, px);
    }
    const QImage own = name == generic ? QImage() : ownIcon(name);
    if (!own.isNull() && !alphaBounds(own, 16).isEmpty())
        return onOurTile(own, px, light, tuningFor(name));
    return piece(light ? QStringLiteral("sodiumshell-app-generic-light") : QStringLiteral("sodiumshell-app-generic"), px);
}

} // namespace

IconProvider::IconProvider()
    : QQuickImageProvider(QQuickImageProvider::Image)
{
}

QImage IconProvider::requestImage(const QString &id, QSize *size, const QSize &requestedSize)
{
    // <logical size>/<aarrggbb or ->/<encoded name>
    const int a = id.indexOf(QLatin1Char('/'));
    const int b = id.indexOf(QLatin1Char('/'), a + 1);
    const int logical = qMax(1, id.left(a).toInt());
    const QString colorText = id.mid(a + 1, b - a - 1);
    const QString name = QUrl::fromPercentEncoding(id.mid(b + 1).toUtf8());
    const QSize px = requestedSize.isValid() && !requestedSize.isEmpty() ? requestedSize : QSize(logical, logical);
    if (size)
        *size = px;
    // "app" / "app-light": an app's icon, always on our tile
    if (colorText == QLatin1String("app") || colorText == QLatin1String("app-light"))
        return appIcon(name, px, colorText == QLatin1String("app-light"));
    const QColor color = colorText == QLatin1String("-") ? QColor() : QColor::fromString(QLatin1Char('#') + colorText);

    QString path;
    bool symbolic = false;
    if (name.startsWith(QLatin1Char('/'))) {
        path = name;
        symbolic = name.contains(QLatin1String("-symbolic"));
    } else if (name.endsWith(QLatin1String("-symbolic"))) {
        path = findSymbolic(name, logical);
        symbolic = true;
    } else {
        path = findColour(name);
        if (path.isEmpty()) {
            path = findSymbolic(name, logical);
            symbolic = !path.isEmpty();
        }
    }

    if (!path.isEmpty() && path.endsWith(QLatin1String(".svg"), Qt::CaseInsensitive)) {
        QFile f(path);
        if (f.open(QIODevice::ReadOnly))
            return renderSvg(f.readAll(), px, symbolic ? color : QColor());
    }
    if (!path.isEmpty()) {
        QImage img(path);
        if (!img.isNull())
            return img.scaled(px, Qt::KeepAspectRatio, Qt::SmoothTransformation);
    }
    // Not ours: the system icon themes (no recolouring).
    const QIcon icon = QIcon::fromTheme(name);
    if (!icon.isNull())
        return icon.pixmap(px).toImage();
    QImage empty(px, QImage::Format_ARGB32_Premultiplied);
    empty.fill(Qt::transparent);
    return empty;
}
