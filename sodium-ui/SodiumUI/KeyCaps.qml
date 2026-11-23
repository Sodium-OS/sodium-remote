// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
pragma Singleton

import QtQuick

// Key names in the UI language (docs/decisions/i18n.md, decisions 32 and 37; the key caps table, L4e2).
//
//     KeyCaps.format("Ctrl + Shift + Esc")    // de "Strg + Umschalt + Esc", fr "Ctrl + Maj + Échap"
//     KeyCaps.format("Win + ←")               // "Super + ←" in every language
//     KeyCaps.format("Ctrl+N")                // "Ctrl + N": the separator is always " + "
//     KeyCaps.format("18:50 – 06:46")         // not a shortcut: unchanged
//     KeyCaps.name("Delete")                  // de "Entf", es "Supr"; ja/zh/ko "Delete"
//     qsTr("Press %1 to search").arg(KeyCaps.format("Super + S"))   // into a sentence only through %1
//
// The names are one table in the `KeyCaps` catalog context, looked up when shown (they follow a live
// language switch). Shortcut letters, F-keys, arrows and symbols are never translated. The logo key is
// `superKey`, "Super", everywhere and untranslated (decision 37): Win, Super, Meta and Logo all become
// it. MenuItem's shortcut, Kbd, Tooltip's shortcut and IconButton's tooltipShortcut go through format().
// A string that isn't made of key names (a time range, "default") comes back unchanged. The app menus'
// D-Bus labels (sodiumuiShortcutLabel) stay English; whoever shows them formats them here.
QtObject {
    id: keys

    /// The logo key's one name (decision 37: one constant; change it here to rename it everywhere).
    readonly property string superKey: "Super"
    /// Between the keys of a chord, in every language (decision 32).
    readonly property string separator: " + "

    // The key names translators see (context KeyCaps; the glossary's key_* terms). zh, ja and ko print
    // the caps as on the keyboard and translate only Space.
    readonly property var _names: [
        //: Key cap: the Control key (glossary key_ctrl; de "Strg")
        QT_TRANSLATE_NOOP("KeyCaps", "Ctrl"),
        //: Key cap: the Shift key (glossary key_shift; de "Umschalt", fr "Maj", es "Mayús")
        QT_TRANSLATE_NOOP("KeyCaps", "Shift"),
        //: Key cap: the Alt key (glossary key_alt)
        QT_TRANSLATE_NOOP("KeyCaps", "Alt"),
        //: Key cap: the Enter or Return key (glossary key_enter; de "Eingabe", fr "Entrée", es "Intro")
        QT_TRANSLATE_NOOP("KeyCaps", "Enter"),
        //: Key cap: the Delete key (glossary key_delete; de "Entf", fr "Suppr", es "Supr")
        QT_TRANSLATE_NOOP("KeyCaps", "Delete"),
        //: Key cap: the Escape key (glossary key_esc; fr "Échap")
        QT_TRANSLATE_NOOP("KeyCaps", "Esc"),
        //: Key cap: the space bar (glossary key_space; de "Leertaste", ja "スペース")
        QT_TRANSLATE_NOOP("KeyCaps", "Space"),
        //: Key cap: the Tab key (glossary key_tab)
        QT_TRANSLATE_NOOP("KeyCaps", "Tab"),
        //: Key cap: the Backspace key (glossary key_backspace; de "Rücktaste")
        QT_TRANSLATE_NOOP("KeyCaps", "Backspace")
    ]
    // Spellings that mean one of the names above (lower case), as Qt's portable text and older labels write them.
    readonly property var _aliases: ({
        "ctrl": "Ctrl", "control": "Ctrl", "strg": "Ctrl",
        "shift": "Shift", "alt": "Alt",
        "enter": "Enter", "return": "Enter",
        "delete": "Delete", "del": "Delete",
        "esc": "Esc", "escape": "Esc",
        "space": "Space", "tab": "Tab", "backspace": "Backspace",
        "win": "Super", "super": "Super", "meta": "Super", "logo": "Super", "windows": "Super"
    })

    /// One key's name in the UI language ("Delete" → de "Entf"); anything that isn't a key name unchanged.
    function name(key) {
        void I18n.revision
        const k = String(key ?? "")
        const canonical = _aliases[k.toLowerCase()]
        if (canonical === undefined)
            return k
        if (canonical === "Super")
            return superKey
        return qsTranslate("KeyCaps", canonical)
    }

    /// A shortcut ("Ctrl + Shift + S", "Ctrl+N", "Win + ←", chords "Ctrl + K, Ctrl + S") with its key
    /// names in the UI language and " + " between the keys. Text that isn't a shortcut is returned as it is.
    function format(text) {
        void I18n.revision
        const s = String(text ?? "")
        if (s === "")
            return s
        const chords = s.split(/,\s+/)
        const out = []
        for (const chord of chords) {
            const parts = _split(chord)
            if (parts === null)
                return s
            out.push(parts.map(p => name(p)).join(separator))
        }
        return out.join(", ")
    }

    // A chord's keys, or null when it isn't one: every key a single character or a known name, at least one
    // known name or two keys, no spaces inside a key ("18:50 – 06:46" and "default" are not shortcuts).
    function _split(chord) {
        const c = chord.trim()
        if (c === "")
            return null
        const parts = []
        let rest = c
        // "+" as the last key: "Ctrl + +", "Ctrl++"
        let plusKey = false
        if (/(^|\+)\s*\+$/.test(rest) && rest.length > 1) {
            plusKey = true
            rest = rest.replace(/\s*\+\s*\+$/, "").replace(/^\+$/, "")
        }
        for (const raw of rest.split("+")) {
            const p = raw.trim()
            if (p === "" || /\s/.test(p))
                return null
            parts.push(p)
        }
        if (plusKey)
            parts.push("+")
        const known = parts.filter(p => _aliases[p.toLowerCase()] !== undefined).length
        if (known === 0)
            return null
        for (const p of parts)
            if (_aliases[p.toLowerCase()] === undefined && [...p].length > 1 && !/^F\d{1,2}$/.test(p) && !_named(p))
                return null
        return parts
    }

    // Other keys a shortcut may name, printed as on the keyboard and never translated.
    function _named(p) {
        return ["Insert", "Home", "End", "PgUp", "PgDown", "PageUp", "PageDown", "Print", "Pause", "Menu", "CapsLock", "NumLock"].indexOf(p) >= 0
    }
}
