// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick

// A row of buttons that never squeezes them (decision 20; LangComponents A01/B01): side by side, 8 px
// apart, at the trailing end, while they fit; when they don't, they stack full-width with the primary
// (the last button) on top. Buttons themselves never elide or wrap.
//
//     ButtonRow {
//         Layout.fillWidth: true                         // it stacks only when its width is too small
//         Button { text: qsTr("Cancel"); onClicked: dialog.reject() }
//         Button { text: qsTr("Save changes"); variant: "primary"; onClicked: dialog.accept() }
//     }
//
// Left to right is the order side by side (primary last, as dialogs have it); stacked, the order is
// reversed. `items` instead of children gives the buttons explicitly (ContentDialog: its extra buttons,
// Cancel, the primary). implicitWidth is what side by side needs, so a layout gives it that when it
// can. It places and sizes its buttons itself: don't bind their x, y or width.
Item {
    id: root

    /// Between the buttons, both ways.
    property real spacing: Theme.metrics.controls.dialog?.buttonGap ?? 8
    /// Side by side: Qt.AlignRight (dialogs, the default) or Qt.AlignLeft.
    property int alignment: Qt.AlignRight
    /// The buttons in order, instead of children (each is taken into the row).
    property list<Item> items

    /// The width the buttons need side by side.
    readonly property real sideBySideWidth: {
        const shown = _shown ?? []
        let w = 0
        for (const b of shown)
            w += b.implicitWidth
        return w + Math.max(0, shown.length - 1) * spacing
    }
    /// They don't fit side by side: stacked full-width, primary on top.
    readonly property bool stacked: width > 0 && sideBySideWidth > width + 0.5

    readonly property var _all: items.length > 0 ? Array.from(items) : Array.from(children)
    readonly property var _shown: (_all ?? []).filter(b => b.visible)
    readonly property var _heights: (_shown ?? []).map(b => b.implicitHeight)

    implicitWidth: sideBySideWidth
    implicitHeight: {
        const hs = _heights ?? []
        if (hs.length === 0)
            return 0
        return stacked ? hs.reduce((a, h) => a + h, 0) + (hs.length - 1) * spacing : Math.max(...hs)
    }

    onItemsChanged: {
        for (const b of items)
            if (b.parent !== root)
                b.parent = root
        _layout()
    }
    onWidthChanged: _layout()
    onStackedChanged: _layout()
    onSideBySideWidthChanged: _layout()
    onImplicitHeightChanged: _layout()
    on_ShownChanged: _layout()
    Component.onCompleted: _layout()

    function _layout() {
        const shown = _shown ?? []
        const n = shown.length
        if (stacked) {
            let y = 0
            for (let i = n - 1; i >= 0; i--) {
                const b = shown[i]
                b.x = 0
                b.y = y
                b.width = width
                b.height = b.implicitHeight
                y += b.height + spacing
            }
            return
        }
        const rowHeight = implicitHeight
        let x = (alignment & Qt.AlignRight) ? width - sideBySideWidth : 0
        for (const b of shown) {
            b.x = x
            b.width = b.implicitWidth
            b.height = b.implicitHeight
            b.y = Math.round((rowHeight - b.height) / 2)
            x += b.width + spacing
        }
    }
}
