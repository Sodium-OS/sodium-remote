// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Templates as T

// One item of a Menu (Components board; P2Bar "Radio, check and disabled
// items"): 30 px, radius 7, 13 px text, a right-aligned shortcut hint (12 px,
// #9EA4B2) or a submenu chevron. Icons, checks and radio dots sit in a 16 px
// lead column, 10 px from the text; when one item of a menu has one, every
// item keeps the column. The keyboard row, and a row whose submenu is open, are
// accent with on-accent 500 text; the row under the pointer is white 10%. Disabled items gray the text and the
// shortcut.
// Every language (decision 20, 32): the label never elides; in a menu at its widest (420 px) it wraps
// and the item grows. The shortcut's key names are shown in the UI language (KeyCaps.format: de
// "Strg + C"; "Win + ←" is "Super + ←"); a hint that isn't a shortcut ("18:50 – 06:46") shows as it is.
//
//     MenuItem { text: "Snap left"; shortcut: "Super + ←"; onTriggered: snapLeft() }
//     MenuItem { text: "Settings"; icon.name: "preferences-system-symbolic" }
//     MenuItem { text: "End task"; destructive: true }                          // #F7A8A8
//     MenuItem { text: "Show hidden files"; checkable: true; checked: true }
//     MenuItem { text: "as Icons"; radio: true; checked: true }                 // exclusive with its siblings
T.MenuItem {
    id: control
    // the name without the translator's break marks (decision 92); a name of your own replaces it
    Accessible.name: I18n.stripBreakMarks(text)

    /// Shortcut hint shown on the right, e.g. "Ctrl + N" (display only).
    property string shortcut: ""
    /// Danger text color (#F7A8A8), for End task, Delete and the like.
    property bool destructive: false
    /// A radio item: a dot instead of a check, exclusive with the other radio items of its menu.
    property bool radio: false
    /// Shows the submenu chevron; set automatically for submenus.
    property bool hasSubmenu: subMenu !== null
    /// Forces a look: "", hover (pointer row), focus (keyboard row), pressed.
    property string previewState: ""

    readonly property var metrics: Theme.metrics.controls.menu
    // The keyboard row: Qt highlights the hovered item too, so the pointer row is told apart by `hovered`.
    // The row whose submenu is open is accent as well, under the pointer or not (P2Bar).
    readonly property bool showAccent: previewState === "focus" || previewState === "pressed" || down
                                       || (highlighted && !hovered && previewState !== "hover")
                                       || (subMenu !== null && subMenu.opened)
    readonly property bool showHover: !showAccent && (previewState === "hover" || (hovered && enabled))
    readonly property bool leading: icon.name !== "" || checkable || (menu !== null && menu.leadColumn === true)
    readonly property color foreground: !enabled ? Theme.textMuted : showAccent ? Theme.textOnAccent
                                        : destructive ? Theme.dangerText : Theme.text

    /// The shortcut as shown: its key names in the UI language (KeyCaps.format).
    readonly property string shortcutText: KeyCaps.format(shortcut)

    implicitWidth: Math.max(implicitBackgroundWidth, implicitContentWidth + leftPadding + rightPadding)
    // One line: 30 px. A label that wraps (a menu at its widest) adds its extra lines.
    implicitHeight: Math.max(metrics.itemHeight, implicitContentHeight)
    padding: 0
    leftPadding: metrics.itemPaddingX
    rightPadding: metrics.itemPaddingX
    spacing: metrics.iconGap
    hoverEnabled: true
    checkable: radio
    autoExclusive: radio
    font: Theme.font("menu", { weight: showAccent ? 500 : 400 })
    icon.width: 16
    icon.height: 16

    contentItem: Item {
        // The label's width at the accent weight (500), so the menu doesn't change width under the pointer.
        implicitWidth: (control.leading ? 16 + control.spacing : 0) + Math.ceil(Math.max(label.implicitWidth, accentWidth.advanceWidth))
                       + (hint.visible ? 24 + hint.implicitWidth : 0) + (control.hasSubmenu ? 24 : 0)
        // 7 px above and below the label: 30 px for one line, more when it wraps.
        implicitHeight: Math.max(control.metrics.itemHeight, label.implicitHeight + control.metrics.itemHeight - (label.styleLineHeight > 0 ? label.styleLineHeight : 16))

        TextMetrics {
            id: accentWidth
            font: Theme.font("menu", { weight: 500 })
            text: control.text
        }

        Item {
            id: lead
            width: control.leading ? 16 : 0
            height: 16
            anchors.verticalCenter: parent.verticalCenter

            Icon {
                anchors.centerIn: parent
                visible: control.icon.name !== "" && !(control.checkable && control.checked)
                name: control.icon.name
                size: 16
                color: control.foreground
            }
            Icon {
                anchors.centerIn: parent
                visible: control.checkable && control.checked && !control.radio
                name: "object-select-symbolic"
                size: 16
                color: control.foreground
            }
            Rectangle {
                anchors.centerIn: parent
                visible: control.radio && control.checked
                width: 6
                height: 6
                radius: 3
                color: control.foreground
            }
        }

        StyledText {
            id: label
            anchors.left: lead.right
            anchors.leftMargin: control.leading ? control.spacing : 0
            anchors.right: hint.visible ? hint.left : chevron.visible ? chevron.left : parent.right
            anchors.rightMargin: hint.visible || chevron.visible ? 12 : 0
            anchors.verticalCenter: parent.verticalCenter
            text: control.text
            style: "menu"
            font: control.font
            color: control.foreground
            wrapMode: Text.Wrap
        }

        StyledText {
            id: hint
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            visible: control.shortcut !== "" && !control.hasSubmenu
            text: control.shortcutText
            style: "shortcut"
            color: !control.enabled ? Theme.textMuted : control.showAccent ? Theme.textOnAccent : Theme.text3
            opacity: control.showAccent ? 0.7 : 1
        }

        Icon {
            id: chevron
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            visible: control.hasSubmenu
            name: "pan-end-symbolic"
            size: 14
            color: control.foreground
        }
    }

    // T.MenuItem positions these itself; ours are drawn in contentItem.
    indicator: Item {}
    arrow: Item {}

    background: Rectangle {
        implicitHeight: control.metrics.itemHeight
        radius: control.metrics.itemRadius
        color: control.showAccent ? Theme.accent : control.showHover ? Theme.hover : "transparent"
    }
}
