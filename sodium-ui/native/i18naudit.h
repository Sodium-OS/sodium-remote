// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// The developer overflow audit (docs/decisions/i18n.md, decision 19 step 2; the LangComponents board, L4d).
//
// With SODIUMOS_I18N_AUDIT=1 (never otherwise: nothing is created, nothing runs), every UI label of
// the process that is elided, clipped or wider than its box gets a 1.5 px red dashed outline and an
// 18 px red tag with its source string and the overflow in px; a chip at the window's bottom right
// counts them ("I18N AUDIT · 3 overflows"). The overlay takes no pointer events.
//
// A label is any visible Text (SodiumUI's components draw theirs with Text). A hit must hold for two
// scans in a row (about 1.4 s), so a panel in mid-slide isn't one. Text inside a Flickable (a list,
// a scrolled page) is checked against its own clipping parents only: scrolled-away content isn't an
// overflow. User content (file names, window titles, what the user wrote) is skipped: mark it with
//
//     Text { text: model.fileName; I18nAudit.userContent: true }     // the item or any parent
//
// Wrapped text (any wrapMode) is also checked where it breaks (decisions 89 and 92): a line break between two
// letters or digits of a script that spaces its words (Latin, Greek, Cyrillic, Hangul ...) with no hyphen or soft
// hyphen there is a "mid-word break" (Text.Wrap's no-clip fallback: shorten the term, place a soft hyphen in the
// catalog, or change the layout); in zh and ja, a short label (I18nAudit.shortLabel, set by WrappedLabel) whose
// last line is one character, or only punctuation or a long-vowel mark, is a "one-character last line". Plain text
// only; the lines are laid out again as the Text lays them out (textbreaks.h), and skipped if the count differs.
//
// Sidebar items and table headers are the only UI labels that may elide (decision 20), always with a
// tooltip showing the full text; SodiumUI's SidebarItem and TableHeader mark theirs, so their eliding isn't
// a hit (clipping still is):
//
//     Text { text: qsTr("Keyboard & Shortcuts"); elide: Text.ElideRight; I18nAudit.mayElide: true }
//
// Each hit is logged once, as one line, to stderr and to $SODIUMOS_I18N_AUDIT_LOG, else
// <repo>/build/i18n-audit/<process>.log (emptied when the process starts; outside the repo
// ${XDG_STATE_HOME:-~/.local/state}/sodiumos/i18n-audit/<process>.log):
//
//     I18N-AUDIT calculator "Calculator" Main/Item/Button/Text  source="Save changes"
//         text="[Šåvé çĥåñĝéš········]"  elided: 152 px > box 96 px (+56 px)  [qps-ploc]
//     I18N-AUDIT shell "Control Center" .../Tile/WrappedLabel/StyledText  source="Energy mode"
//         text="Energiemodus"  mid-word break: "Energiemo|dus"  [de_DE]
#pragma once

#include <QFile>
#include <QHash>
#include <QObject>
#include <QPointer>
#include <QQuickPaintedItem>
#include <QSet>
#include <QTimer>
#include <QtQml/qqmlregistration.h>

class QQuickItem;
class QQuickWindow;

class I18nAuditAttached : public QObject
{
    Q_OBJECT
    /// User content: this item and everything inside it is never audited.
    Q_PROPERTY(bool userContent READ userContent WRITE setUserContent NOTIFY userContentChanged)
    /// A label that may elide, with a tooltip showing the full text (sidebar items, table headers):
    /// its eliding isn't a hit. This item and everything inside it.
    Q_PROPERTY(bool mayElide READ mayElide WRITE setMayElide NOTIFY mayElideChanged)
    /// A short wrapped label (decision 89: titles, tiles, row and form labels; WrappedLabel sets it): in zh and ja a
    /// last line of one character, or only punctuation or a long-vowel mark, is a hit. This item only.
    Q_PROPERTY(bool shortLabel READ shortLabel WRITE setShortLabel NOTIFY shortLabelChanged)

public:
    explicit I18nAuditAttached(QObject *parent)
        : QObject(parent)
    {
    }
    bool userContent() const { return m_userContent; }
    void setUserContent(bool on)
    {
        if (on != m_userContent) {
            m_userContent = on;
            emit userContentChanged();
        }
    }

    bool mayElide() const { return m_mayElide; }
    void setMayElide(bool on)
    {
        if (on != m_mayElide) {
            m_mayElide = on;
            emit mayElideChanged();
        }
    }

    bool shortLabel() const { return m_shortLabel; }
    void setShortLabel(bool on)
    {
        if (on != m_shortLabel) {
            m_shortLabel = on;
            emit shortLabelChanged();
        }
    }

signals:
    void userContentChanged();
    void mayElideChanged();
    void shortLabelChanged();

private:
    bool m_userContent = false;
    bool m_mayElide = false;
    bool m_shortLabel = false;
};

struct I18nAuditHit
{
    QPointer<QQuickItem> item;
    QRectF box;        // in the overlay's coordinates
    QString source;    // the source string when known
    QString text;      // what is shown
    QString kind;      // "elided", "truncated", "clipped", "overflows", "mid-word break", "one-character last line"
    QString detail;    // a line-break hit: where ("Energiemo|dus")
    QString path;
    qreal content = 0; // px the text needs
    qreal room = 0;    // px its box gives
    qreal over = 0;
};

/// The outlines, tags and counter chip over one window.
class I18nAuditOverlay : public QQuickPaintedItem
{
    Q_OBJECT

public:
    explicit I18nAuditOverlay(QQuickItem *parent);
    void setHits(const QList<I18nAuditHit> &hits);
    void paint(QPainter *painter) override;

private:
    void follow();
    QList<I18nAuditHit> m_hits;
    QString m_signature;
};

class I18nAudit : public QObject
{
    Q_OBJECT
    QML_ATTACHED(I18nAuditAttached)

public:
    /// $SODIUMOS_I18N_AUDIT is 1 (read once).
    static bool enabled();
    /// Made only when enabled() (I18n::start does).
    static I18nAudit *instance();
    static I18nAuditAttached *qmlAttachedProperties(QObject *object) { return new I18nAuditAttached(object); }

    void start();
    /// Scans every window of the process now (also runs on a timer).
    void scan();
    /// The line-break hit of one wrapped Text, as the audit sees it in this process's language: "" or the kind and
    /// where, "mid-word break: \"Energiemo|dus\"", "one-character last line: \"ン\"". Works without the audit on
    /// (I18n.lineBreakFault, for tests and sweeps).
    static QString lineBreakFault(QQuickItem *text, QString *kind = nullptr, QString *detail = nullptr);

private:
    explicit I18nAudit(QObject *parent = nullptr);
    void collect(QQuickWindow *window, QQuickItem *item, I18nAuditOverlay *overlay, QList<I18nAuditHit> *hits);
    void check(QQuickWindow *window, QQuickItem *text, I18nAuditOverlay *overlay, QList<I18nAuditHit> *hits);
    void log(QQuickWindow *window, const I18nAuditHit &hit);

    QTimer m_timer;
    QHash<QQuickWindow *, QPointer<I18nAuditOverlay>> m_overlays;
    QSet<QString> m_seen;     // hits of the last scan (a hit counts from its second scan)
    QSet<QString> m_logged;
    QFile m_log;
    bool m_logOpened = false;
};
