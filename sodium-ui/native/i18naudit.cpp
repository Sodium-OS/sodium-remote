// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "i18naudit.h"

#include "i18n.h"
#include "textbreaks.h"

#include <QDir>
#include <QFileInfo>
#include <QFontMetricsF>
#include <QGuiApplication>
#include <QPainter>
#include <QPainterPath>
#include <QQmlEngine>
#include <QQuickItem>
#include <QQuickWindow>
#include <QStandardPaths>
#include <QTextBoundaryFinder>
#include <QTextDocument>
#include <QTextOption>

#include <cstdio>

namespace {

const QColor kRed(0xe5, 0x48, 0x4d);   // Theme.closeHover, the board's audit red

// Text's enums, as QQuickText's properties return them.
constexpr int kElideNone = 3;
constexpr int kAlignRight = 0x2, kAlignHCenter = 0x4;
constexpr int kAlignBottom = 0x40, kAlignVCenter = 0x80;

QString typeName(const QObject *o)
{
    QString n = QString::fromLatin1(o->metaObject()->className());
    for (const char *cut : {"_QMLTYPE_", "_QML_"}) {
        const int i = int(n.indexOf(QLatin1String(cut)));
        if (i > 0)
            n = n.left(i);
    }
    if (n.startsWith(QLatin1String("QQuick")))
        n = n.mid(6);
    return n;
}

bool userContent(QQuickItem *item)
{
    for (QQuickItem *a = item; a; a = a->parentItem()) {
        auto *att = qobject_cast<I18nAuditAttached *>(qmlAttachedPropertiesObject<I18nAudit>(a, false));
        if (att && att->userContent())
            return true;
    }
    return false;
}

bool mayElide(QQuickItem *item)
{
    for (QQuickItem *a = item; a; a = a->parentItem()) {
        auto *att = qobject_cast<I18nAuditAttached *>(qmlAttachedPropertiesObject<I18nAudit>(a, false));
        if (att && att->mayElide())
            return true;
    }
    return false;
}

// Where the text's ink sits in the item: its alignment within the box.
QRectF paintedRect(QQuickItem *item, qreal cw, qreal ch)
{
    const qreal w = item->width(), h = item->height();
    const int ha = item->property("effectiveHorizontalAlignment").toInt();
    const int va = item->property("verticalAlignment").toInt();
    const qreal x = (ha & kAlignRight) ? w - cw : (ha & kAlignHCenter) ? (w - cw) / 2 : 0;
    const qreal y = (va & kAlignBottom) ? h - ch : (va & kAlignVCenter) ? (h - ch) / 2 : 0;
    return QRectF(x, y, cw, ch);
}

bool shortLabel(QQuickItem *item)
{
    auto *att = qobject_cast<I18nAuditAttached *>(qmlAttachedPropertiesObject<I18nAudit>(item, false));
    return att && att->shortLabel();
}

// A letter or digit of a script that puts spaces between its words: a break between two of them is inside a word.
// Han, kana and Bopomofo break between any characters; Hangul doesn't (decision 89).
bool spacedWordChar(QChar c)
{
    if (!c.isLetterOrNumber())
        return false;
    switch (c.script()) {
    case QChar::Script_Han:
    case QChar::Script_Hiragana:
    case QChar::Script_Katakana:
    case QChar::Script_Bopomofo:
    case QChar::Script_Thai:
    case QChar::Script_Lao:
    case QChar::Script_Khmer:
    case QChar::Script_Myanmar:
        return false;
    default:
        return true;
    }
}

QString logPath()
{
    const QString env = qEnvironmentVariable("SODIUMOS_I18N_AUDIT_LOG");
    if (!env.isEmpty())
        return env;
    const QString name = I18n::instance()->processName() + QStringLiteral(".log");
    const QString repo = QDir::cleanPath(I18n::instance()->moduleDir() + QStringLiteral("/../.."));
    if (QFileInfo::exists(repo + QStringLiteral("/.git")))
        return repo + QStringLiteral("/build/i18n-audit/") + name;
    QString state = qEnvironmentVariable("XDG_STATE_HOME");
    if (state.isEmpty())
        state = QDir::homePath() + QStringLiteral("/.local/state");
    return state + QStringLiteral("/sodiumos/i18n-audit/") + name;
}

} // namespace

// ---------------------------------------------------------------- the overlay

I18nAuditOverlay::I18nAuditOverlay(QQuickItem *parent)
    : QQuickPaintedItem(parent)
{
    setZ(1e9);
    setAcceptedMouseButtons(Qt::NoButton);
    setAcceptHoverEvents(false);
    setAcceptTouchEvents(false);
    setEnabled(false);
    setObjectName(QStringLiteral("sodiumosI18nAuditOverlay"));
    connect(parent, &QQuickItem::widthChanged, this, &I18nAuditOverlay::follow);
    connect(parent, &QQuickItem::heightChanged, this, &I18nAuditOverlay::follow);
    follow();
}

void I18nAuditOverlay::follow()
{
    setSize(parentItem()->size());
    update();
}

void I18nAuditOverlay::setHits(const QList<I18nAuditHit> &hits)
{
    QString sig;
    for (const I18nAuditHit &h : hits)
        sig += QStringLiteral("%1,%2,%3,%4,%5|").arg(h.box.x()).arg(h.box.y()).arg(h.box.width()).arg(qRound(h.over)).arg(h.text);
    if (sig == m_signature)
        return;
    m_signature = sig;
    m_hits = hits;
    update();
}

void I18nAuditOverlay::paint(QPainter *p)
{
    if (m_hits.isEmpty())
        return;
    p->setRenderHint(QPainter::Antialiasing);
    QFont tagFont(QStringLiteral("JetBrains Mono"));
    tagFont.setPointSizeF(10.5 * 0.75);
    tagFont.setWeight(QFont::DemiBold);
    const QFontMetricsF tm(tagFont);
    QPen dash(kRed, 1.5);
    dash.setDashPattern({3, 2});   // × the width: 4.5 px dashes, 3 px gaps
    for (const I18nAuditHit &h : std::as_const(m_hits)) {
        const QRectF r = h.box.adjusted(-2, -2, 2, 2);
        p->setPen(dash);
        p->setBrush(Qt::NoBrush);
        p->drawRoundedRect(r, 4, 4);
        // The tag: 18 px, above the outline (below it, or over it, where there is no room).
        const QString shown = h.source.isEmpty() ? h.text : h.source;
        QString label = h.detail.isEmpty() ? QStringLiteral("“%1” · +%2 px").arg(shown).arg(qRound(h.over))
                                           : QStringLiteral("“%1” · %2").arg(shown, h.kind);
        label = tm.elidedText(label, Qt::ElideMiddle, qMax(60.0, qMin(420.0, width() - 16)));
        const qreal tw = tm.horizontalAdvance(label) + 12, th = 18;
        qreal ty = r.top() - th - 6;
        if (ty < 0)
            ty = r.bottom() + 6;
        if (ty + th > height())
            ty = qMax(0.0, r.top());
        const qreal tx = qBound(2.0, r.left(), qMax(2.0, width() - tw - 2));
        const QRectF tag(tx, ty, tw, th);
        p->setPen(Qt::NoPen);
        p->setBrush(kRed);
        p->drawRoundedRect(tag, 5, 5);
        p->setPen(Qt::white);
        p->setFont(tagFont);
        p->drawText(tag, Qt::AlignCenter, label);
    }
    // The counter chip, bottom right: a warning triangle, then the count.
    QFont chipFont(QStringLiteral("JetBrains Mono"));
    chipFont.setPointSizeF(11 * 0.75);
    chipFont.setWeight(QFont::DemiBold);
    const QFontMetricsF cm(chipFont);
    const int n = int(m_hits.size());
    const QString label = QStringLiteral("I18N AUDIT · %1 %2").arg(n).arg(n == 1 ? QStringLiteral("overflow") : QStringLiteral("overflows"));
    const qreal ch = qMin(24.0, height() - 4), cw = cm.horizontalAdvance(label) + 20 + 18;
    const bool roomy = height() >= 60;
    const QRectF chip(width() - cw - (roomy ? 16 : 2), height() - ch - (roomy ? 14 : 2), cw, ch);
    p->setPen(Qt::NoPen);
    p->setBrush(kRed);
    p->drawRoundedRect(chip, ch / 2, ch / 2);
    const QPointF c(chip.left() + 10 + 6, chip.center().y());
    QPainterPath tri;
    tri.moveTo(c.x(), c.y() - 5.5);
    tri.lineTo(c.x() + 6, c.y() + 5);
    tri.lineTo(c.x() - 6, c.y() + 5);
    tri.closeSubpath();
    p->setPen(QPen(Qt::white, 1.4, Qt::SolidLine, Qt::RoundCap, Qt::RoundJoin));
    p->setBrush(Qt::NoBrush);
    p->drawPath(tri);
    p->drawLine(QPointF(c.x(), c.y() - 1.5), QPointF(c.x(), c.y() + 1.5));
    p->drawPoint(QPointF(c.x(), c.y() + 3.3));
    p->setFont(chipFont);
    p->drawText(chip.adjusted(10 + 18, 0, -10, 0), Qt::AlignVCenter | Qt::AlignLeft, label);
}

// ---------------------------------------------------------------- the audit

bool I18nAudit::enabled()
{
    static const bool on = qEnvironmentVariable("SODIUMOS_I18N_AUDIT") == QLatin1String("1");
    return on;
}

I18nAudit *I18nAudit::instance()
{
    static I18nAudit *self = new I18nAudit;
    return self;
}

I18nAudit::I18nAudit(QObject *parent)
    : QObject(parent)
{
    m_timer.setInterval(700);
    connect(&m_timer, &QTimer::timeout, this, &I18nAudit::scan);
}

void I18nAudit::start()
{
    if (m_timer.isActive())
        return;
    m_timer.start();
    fprintf(stderr, "I18N-AUDIT on in %s [%s]; log: %s\n", qPrintable(I18n::instance()->processName()),
            qPrintable(I18n::instance()->language()), qPrintable(logPath()));
}

void I18nAudit::scan()
{
    QSet<QString> seen;
    const QWindowList windows = QGuiApplication::allWindows();
    for (QWindow *w : windows) {
        auto *window = qobject_cast<QQuickWindow *>(w);
        if (!window || !window->isVisible() || !window->contentItem())
            continue;
        QPointer<I18nAuditOverlay> &overlay = m_overlays[window];
        if (!overlay) {
            overlay = new I18nAuditOverlay(window->contentItem());
            connect(window, &QObject::destroyed, this, [this, window] { m_overlays.remove(window); });
        }
        QList<I18nAuditHit> hits;
        collect(window, window->contentItem(), overlay, &hits);
        QList<I18nAuditHit> stable;
        for (const I18nAuditHit &h : std::as_const(hits)) {
            const QString key = QString::number(quintptr(h.item.data()), 16) + QLatin1Char('|') + h.text;
            seen << key;
            if (!m_seen.contains(key))
                continue;
            stable << h;
            const QString once = key + QLatin1Char('|') + QString::number(qRound(h.over));
            if (!m_logged.contains(once)) {
                m_logged << once;
                log(window, h);
            }
        }
        overlay->setHits(stable);
    }
    m_seen = seen;
}

void I18nAudit::collect(QQuickWindow *window, QQuickItem *item, I18nAuditOverlay *overlay, QList<I18nAuditHit> *hits)
{
    if (item == overlay || !item->isVisible() || item->opacity() < 0.01)
        return;
    if (item->inherits("QQuickText"))
        check(window, item, overlay, hits);
    const QList<QQuickItem *> children = item->childItems();
    for (QQuickItem *child : children)
        collect(window, child, overlay, hits);
}

void I18nAudit::check(QQuickWindow *window, QQuickItem *item, I18nAuditOverlay *overlay, QList<I18nAuditHit> *hits)
{
    const QString text = item->property("text").toString();
    const qreal w = item->width(), h = item->height();
    if (text.trimmed().isEmpty() || w < 1 || h < 1 || userContent(item))
        return;
    QQuickItem *root = window->contentItem();
    if (!item->mapRectToItem(root, QRectF(0, 0, w, h)).intersects(QRectF(0, 0, root->width(), root->height())))
        return;   // not on screen at all (a panel parked outside its window)
    const qreal cw = item->property("contentWidth").toReal(), ch = item->property("contentHeight").toReal();
    const QRectF painted = paintedRect(item, cw, ch);
    I18nAuditHit hit;
    if (item->property("truncated").toBool()) {
        hit.kind = item->property("elide").toInt() != kElideNone ? QStringLiteral("elided") : QStringLiteral("truncated");
        if (item->implicitWidth() > w + 0.5) {
            hit.content = item->implicitWidth();
            hit.room = w;
        } else {
            hit.content = item->implicitHeight();
            hit.room = h;
        }
    } else if (cw > w + 1) {
        hit.kind = item->clip() ? QStringLiteral("clipped") : QStringLiteral("overflows");
        hit.content = cw;
        hit.room = w;
    } else if (item->clip() && ch > h + 1.5) {
        hit.kind = QStringLiteral("clipped");
        hit.content = ch;
        hit.room = h;
    }
    // Cut by a clipping parent, or by the window's edge. Inside a Flickable only its own content's
    // clipping counts: what is scrolled away isn't an overflow.
    for (QQuickItem *a = item->parentItem(); a; a = a->parentItem()) {
        if (a->inherits("QQuickFlickable")) {
            if (!item->mapRectToItem(a, QRectF(0, 0, w, h)).intersects(QRectF(0, 0, a->width(), a->height())))
                return;   // scrolled out of view
            break;
        }
        if (!hit.kind.isEmpty())
            continue;   // already a hit: only looking for a Flickable it may be scrolled out of
        const bool edge = a == window->contentItem();
        if (!a->clip() && !edge)
            continue;
        const QRectF r = item->mapRectToItem(a, painted);
        const QRectF bounds(0, 0, a->width(), a->height());
        if (!r.intersects(bounds))
            return;   // hidden by a clipping parent
        const qreal overX = qMax(0.0, bounds.left() - r.left()) + qMax(0.0, r.right() - bounds.right());
        const qreal overY = qMax(0.0, bounds.top() - r.top()) + qMax(0.0, r.bottom() - bounds.bottom());
        if (overX > 1) {
            hit.kind = QStringLiteral("clipped");
            hit.content = r.width();
            hit.room = r.width() - overX;
        } else if (overY > 1.5) {
            hit.kind = QStringLiteral("clipped");
            hit.content = r.height();
            hit.room = r.height() - overY;
        }
        if (edge)
            break;
    }
    if (hit.kind.isEmpty())
        lineBreakFault(item, &hit.kind, &hit.detail);
    if (hit.kind.isEmpty())
        return;
    if (hit.kind == QLatin1String("elided") && mayElide(item))
        return;   // a sidebar item or table header: it elides with a tooltip (decision 20)
    hit.item = item;
    hit.over = hit.content - hit.room;
    hit.text = text;
    hit.source = I18n::instance()->sourceFor(text);
    if (hit.source.isEmpty() && text.contains(QChar(TextBreaks::kWordJoiner)))
        hit.source = I18n::instance()->sourceFor(QString(text).remove(QChar(TextBreaks::kWordJoiner)));   // a WrappedLabel's phrases
    hit.box = item->mapRectToItem(overlay, QRectF(0, 0, w, h));
    QStringList path;
    for (QQuickItem *a = item; a && a != window->contentItem(); a = a->parentItem())
        path.prepend(a->objectName().isEmpty() ? typeName(a) : typeName(a) + QLatin1Char('#') + a->objectName());
    if (path.size() > 7)
        path = path.mid(path.size() - 7);
    hit.path = path.join(QLatin1Char('/'));
    hits->append(hit);
}

void I18nAudit::log(QQuickWindow *window, const I18nAuditHit &hit)
{
    const QString windowName = !window->title().isEmpty() ? window->title()
                             : !window->objectName().isEmpty() ? window->objectName() : typeName(window);
    const auto quoted = [](QString s) { return QLatin1Char('"') + s.replace(QLatin1Char('\n'), QLatin1String("\\n")) + QLatin1Char('"'); };
    const QString where = hit.detail.isEmpty() ? QStringLiteral("%1 px > box %2 px (+%3 px)").arg(qRound(hit.content)).arg(qRound(hit.room)).arg(qRound(hit.over))
                                               : quoted(hit.detail);
    const QString line = QStringLiteral("I18N-AUDIT %1 %2 %3  source=%4  text=%5  %6: %7  [%8]")
                             .arg(I18n::instance()->processName(), quoted(windowName), hit.path,
                                  quoted(hit.source), quoted(hit.text), hit.kind, where, I18n::instance()->language());
    fprintf(stderr, "%s\n", line.toUtf8().constData());
    if (!m_logOpened) {
        m_logOpened = true;
        const QString path = logPath();
        QDir().mkpath(QFileInfo(path).absolutePath());
        m_log.setFileName(path);
        if (!m_log.open(QIODevice::WriteOnly | QIODevice::Truncate | QIODevice::Text))
            fprintf(stderr, "I18N-AUDIT can't write %s\n", qPrintable(path));
    }
    if (m_log.isOpen()) {
        m_log.write(line.toUtf8() + '\n');
        m_log.flush();
    }
}

QString I18nAudit::lineBreakFault(QQuickItem *item, QString *kind, QString *detail)
{
    if (!item || !item->inherits("QQuickText"))
        return {};
    const QString text = item->property("text").toString();
    const int wrap = item->property("wrapMode").toInt();   // Text.WrapMode is QTextOption::WrapMode
    const int lineCount = item->property("lineCount").toInt();
    if (wrap == QTextOption::NoWrap || lineCount < 2 || text.isEmpty())
        return {};
    // Plain text only: PlainText (0), or AutoText (2) and StyledText (4) without markup.
    const int format = item->property("textFormat").toInt();
    if (!(format == 0 || ((format == 2 || format == 4) && !Qt::mightBeRichText(text))))
        return {};
    const qreal width = item->width() - item->property("leftPadding").toReal() - item->property("rightPadding").toReal();
    const bool designMetrics = item->property("renderType").toInt() != 1;   // all but Text.NativeRendering
    const QList<TextBreaks::Line> lines = TextBreaks::lines(text, item->property("font").value<QFont>(), width, wrap, designMetrics);
    if (lines.size() != lineCount)
        return {};   // not the layout the Text drew: say nothing rather than guess
    QString k, d;
    for (int i = 0; i + 1 < lines.size() && k.isEmpty(); ++i) {
        const int at = lines.at(i + 1).start;
        if (at <= 0 || at >= text.size())
            continue;
        const QChar before = text.at(at - 1), after = text.at(at);
        if (!spacedWordChar(before) || !spacedWordChar(after))
            continue;   // a space, a hyphen, a soft hyphen (not a letter), a forced break, or CJK
        int from = at, to = at;
        while (from > 0 && spacedWordChar(text.at(from - 1)))
            --from;
        while (to < text.size() && spacedWordChar(text.at(to)))
            ++to;
        k = QStringLiteral("mid-word break");
        d = text.mid(from, at - from) + QLatin1Char('|') + text.mid(at, to - at);
    }
    const QString language = I18n::instance()->language();
    const bool zhJa = language.startsWith(QLatin1String("zh_")) || language == QLatin1String("ja_JP") || language == QLatin1String("qps-ploca");
    if (k.isEmpty() && zhJa && shortLabel(item)) {
        const QString last = TextBreaks::stripBreakMarks(text.mid(lines.last().start, lines.last().length)).trimmed();
        QTextBoundaryFinder graphemes(QTextBoundaryFinder::Grapheme, last);
        int clusters = 0;
        while (graphemes.toNextBoundary() > 0)
            ++clusters;
        bool marksOnly = !last.isEmpty();
        for (const QChar c : last)
            marksOnly = marksOnly && (c.isPunct() || c == QChar(0x30fc) || c == QChar(0xff70));   // ー, ｰ
        if (clusters == 1 || marksOnly) {
            k = QStringLiteral("one-character last line");
            d = last;
        }
    }
    if (kind)
        *kind = k;
    if (detail)
        *detail = d;
    return k.isEmpty() ? QString() : k + QStringLiteral(": \"") + d + QLatin1Char('"');
}
