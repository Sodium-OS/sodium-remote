// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
.pragma library

// Helpers for SodiumUI's in-window popups (Menu, Tooltip).

// The item a popup's in-window blur samples (Glass.backdrop): the top-level
// item of the window (a child of its contentItem) that holds the popup's
// parent. Popups live in the window's overlay, a sibling of that item, so the
// backdrop never contains the popup itself. A submenu's parent is a row of its
// parent menu, inside the overlay, so it takes that menu's backdrop. Anything
// else inside the overlay (a tooltip in a popup) gets none.
function backdropFor(popup, parent) {
    if (!parent)
        return null
    if (parent.menu && parent.menu !== popup && parent.menu.backdrop !== undefined)
        return parent.menu.backdrop
    let it = parent
    while (it.parent && it.parent.parent)
        it = it.parent
    if (!it.parent)
        return null                                  // not in a window yet
    if (String(it).startsWith("QQuickOverlay") || contains(it, popup.background))
        return null
    return it
}

function contains(ancestor, item) {
    for (let it = item; it; it = it.parent)
        if (it === ancestor)
            return true
    return false
}
