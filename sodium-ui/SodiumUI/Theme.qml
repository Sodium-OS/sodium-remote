// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
pragma Singleton

import QtQuick

// The live Graphite Glass theme for the shell and our apps.
//
// Values come from tokens/tokens.json through tokens/generate.py, which writes
// <state>/theme/sodiumui/theme.json (scripts/apply-theme.sh). ThemeStore (C++) watches
// that file and <state>/settings.json, so an accent or Settings change reaches every
// running process at once. Colors cross-fade to the new theme over
// Theme.motion.retintMs (400 ms, PLAN section 7). With no live file, the committed
// SodiumUI/theme.default.json is used.
//
// Color names are the tokens' names (tokens.json themes.dark.color).
QtObject {
    id: theme

    // ---------------------------------------------------------------- source
    readonly property var store: ThemeStore
    readonly property var data: ThemeStore.theme
    /// true while the live theme file is in use; false on the committed defaults
    readonly property bool live: ThemeStore.live
    readonly property string name: data.theme ?? "dark"
    readonly property bool dark: name !== "light"

    // ---------------------------------------------------------------- settings
    /// Every user setting (tokens.json "settings" defaults + settings.json overrides).
    readonly property var settings: ThemeStore.settings
    /// The clock in use (decision 95): the user's choice (settings.json `clock24h`, a bool), else the Region's
    /// own clock (Format.regionClock24h, from LC_TIME). Re-evaluated when the Region changes. Applies everywhere at once.
    readonly property bool clock24h: {
        void Format.revision
        return typeof settings.clock24h === "boolean" ? settings.clock24h : Format.regionClock24h
    }
    /// What the user picked, for Settings' Time format row: "auto" (no key: the Region's clock), "24" or "12".
    /// Set it with Theme.setSetting("clock24h", true | false | null): null removes the key (Automatic).
    readonly property string clockChoice: typeof settings.clock24h === "boolean" ? (settings.clock24h ? "24" : "12") : "auto"
    /// Reduce motion: springs and slides become 120 ms cross-fades.
    readonly property bool reduceMotion: settings.reduceMotion === true
    /// Settings › Mouse › "Middle-click to scroll" (on): AutoScroll in every scrollable view.
    readonly property bool middleClickScroll: settings.middleClickScroll !== false
    /// A time of day as the clock setting and the Region say: Format.timeOfDay (Settings' 24/12-hour
    /// clock in the Region's pattern, AM/PM in the UI language). Use it for every time shown.
    function formatTime(date) {
        void LocaleFormats.revision
        void clock24h
        return LocaleFormats.timeOfDay(date, false)
    }

    // ---------------------------------------------------------------- accessibility
    // Settings › Accessibility (P5Settings2). Every SodiumUI process follows them live.
    /// Larger text: text and controls at 125 % (font() and metrics.controls scale by it). Layouts
    /// of their own (the lock's geometry) multiply by textScale; font sizes never do (font() does).
    readonly property bool largerText: settings.largerText === true
    readonly property real textScale: largerText ? 1.25 : 1
    /// Increase contrast: secondary text closer to the primary, lines about twice as strong,
    /// hover/press/selection fills about 1.6 times stronger, glass tints halfway to solid (applied to the colors below, with the usual cross-fade).
    readonly property bool increaseContrast: settings.increaseContrast === true
    /// Reduce transparency: every glass tint solid and no in-window blur (Glass). The compositor's
    /// blur off is tokens/generate.py's (phase5-settings §15: "100 % everywhere and blur off").
    readonly property bool reduceTransparency: settings.reduceTransparency === true
    /// Blink the text caret (Caret; ThemeStore sets Qt's cursor flash time for other text fields).
    readonly property bool caretBlink: settings.caretBlink !== false
    /// Always show the focus ring: on focus from a click too, not only after a key press.
    readonly property bool focusRingAlways: settings.focusRingAlways === true

    // ---------------------------------------------------------------- look
    /// Settings › Appearance › Look (docs/decisions/petal-glass.md 22): "graphite" (default) or "petal".
    /// Colours, radii, fonts and springs below already carry the look's values; read this only where a
    /// look changes geometry or a setting's meaning.
    readonly property string look: settings.look === "petal" ? "petal" : "graphite"
    readonly property bool petal: look === "petal"
    /// The accent the settings ask for, as scripts/apply-theme.sh --accent takes it: "wallpaper" or a preset
    /// name. Each look keeps its own choice (Petal: petalAccent, from the wallpaper off by default).
    readonly property string accentChoice: petal
        ? (settings.petalAccentFromWallpaper === true ? "wallpaper" : String(settings.petalAccent ?? "blush"))
        : (settings.accentFromWallpaper === false ? String(settings.accent ?? "periwinkle") : "wallpaper")

    // ---------------------------------------------------------------- corners
    /// Settings › Appearance › Corner shape (phase5-settings §17): "squircle" (default), "round" or "square".
    /// The radius tokens below (radius, metrics) and the compositor already follow it; a size of your own
    /// goes through corner().
    readonly property string cornerShape: settings.cornerShape === "round" || settings.cornerShape === "square"
                                          ? settings.cornerShape : "squircle"
    /// Square corners: no rounding anywhere, controls included.
    readonly property bool squareCorners: cornerShape === "square"
    /// A corner radius of `px` for the current shape: `px` with Squircle or Round (they differ in the curve,
    /// not the size), 0 with Square. Use it for every radius that has no token: radius: Theme.corner(12).
    /// Circles and capsules (avatars, dots, knobs, tracks: radius: height / 2) stay circles; they don't use it.
    function corner(px) { return squareCorners ? 0 : px }

    // ---------------------------------------------------------------- locale formats
    /// Settings › Language & Region (locale.conf, watched): LC_TIME for dates, LC_NUMERIC for
    /// numbers, the region (LC_MONETARY) as a territory code. Times stay with clock24h above.
    /// The whole set (sizes, durations, percent, relative times) is the Format singleton.
    readonly property var formats: LocaleFormats
    /// A date in the user's format from an ICU skeleton: "EEEdMMM" (en_GB "Wed 4 Nov", en_US
    /// "Wed, Nov 4", zh_CN "11月4日周三"), "EEEEdMMMM", "dMMM", "dMMMMy", "MMMMy", "MMMM", "EEEE",
    /// "EEE", "y", "yMd" (short, numeric). The Region's order, the UI language's month and day
    /// names. Bindings re-run when the format or the language changes.
    function formatDate(date, skeleton) {
        void LocaleFormats.revision
        return LocaleFormats.formatDate(date, skeleton ?? "yMd")
    }
    /// A number with the user's separators; decimals omitted: as few as it needs.
    function formatNumber(value, decimals) {
        void LocaleFormats.revision
        return LocaleFormats.formatNumber(value, decimals ?? -1)
    }
    /// Writes a user setting to settings.json; every SodiumUI process picks it up.
    function setSetting(key, value) { return ThemeStore.setSetting(key, value) }

    // ---------------------------------------------------------------- colors
    // Surfaces (glass tints; Hyprland draws the blur)
    readonly property color ground: _c("ground")
    readonly property color window: _c("window")
    /// The inactive window's glass tint (dark #181A24 at 76 %, light 82 %). Nothing paints it yet: Glass "window" always uses `window`.
    readonly property color windowInactive: _c("windowInactive")
    readonly property color windowTitleActive: _c("windowTitleActive")
    readonly property color windowTitleInactive: _c("windowTitleInactive")
    readonly property color raised: _c("raised")
    readonly property color bar: _c("bar")
    readonly property color panel: _c("panel")
    readonly property color menu: _c("menu")
    readonly property color tooltip: _c("tooltip")
    readonly property color surfaceSolid: _c("surfaceSolid")
    readonly property color surfaceHeader: _c("surfaceHeader")
    readonly property color surfaceView: _c("surfaceView")
    readonly property color surfaceCard: _c("surfaceCard")
    readonly property color surfaceSidebar: _c("surfaceSidebar")
    // Text and icons
    readonly property color text: _c("text")
    readonly property color text2: _c("text2")
    readonly property color text3: _c("text3")
    readonly property color textMuted: _c("textMuted")
    readonly property color label: _c("label")
    readonly property color barText: _c("barText")
    readonly property color menuText: _c("menuText")
    readonly property color titleInactive: _c("titleInactive")
    readonly property color control: _c("control")
    readonly property color controlInactive: _c("controlInactive")
    readonly property color link: _c("link")
    // Accent (from the wallpaper or a preset)
    readonly property color accent: _c("accent")
    readonly property color accentSoft: _c("accentSoft")
    readonly property color accentHalo: _c("accentHalo")
    readonly property color accentLine: _c("accentLine")
    readonly property color textOnAccent: _c("textOnAccent")
    // Status
    readonly property color success: _c("success")
    readonly property color warning: _c("warning")
    readonly property color danger: _c("danger")
    readonly property color dangerText: _c("dangerText")
    readonly property color dangerSoft: _c("dangerSoft")
    readonly property color closeHover: _c("closeHover")
    readonly property color badge: _c("badge")
    // Lines and fills (white at low alpha in dark)
    readonly property color hairline: _c("hairline")
    readonly property color divider: _c("divider")
    readonly property color glassBorder: _c("glassBorder")
    readonly property color highlight: _c("highlight")
    readonly property color fill: _c("fill")
    readonly property color hover: _c("hover")
    readonly property color selected: _c("selected")
    readonly property color pressed: _c("pressed")
    readonly property color disabledFill: _c("disabledFill")
    readonly property color raisedTint: _c("raisedTint")
    readonly property color track: _c("track")
    readonly property color switchOff: _c("switchOff")
    readonly property color sidebar: _c("sidebar")
    readonly property color shadow: _c("shadow")
    readonly property color knob: _c("knob")
    readonly property color neutralFill: _c("neutralFill")
    // SodiumUI v1.2 surfaces (P2Dialogs, P2Switch; tokens.json themes.<theme>.ui). The
    // fallbacks cover a live theme written before they existed, and light until Phase 5.
    /// In-app dialogs and the file chooser sheet: #20222E at 97% (light #FCFCFE 97%).
    readonly property color dialog: _s("dialog", "#f720222e", "#f7fcfcfe")
    /// Popovers and system prompts (sheet glass): #1A1C26 at 80% (light #FAFBFE 86%).
    readonly property color sheet: _s("sheet", "#cc1a1c26", "#dbfafbfe")
    /// Over a window under its dialog or sheet: #080910 50% (light ink 22%).
    readonly property color scrimWindow: _s("scrimWindow", "#80080910", "#38141828")
    /// Over the screen under a system prompt (the compositor blurs it): #06070E 55% (light ink 30%).
    readonly property color scrimScreen: _s("scrimScreen", "#8c06070e", "#4c141828")
    /// Under the launcher: #080910 38% (light #E8ECF6 42%).
    readonly property color launcherScrim: _s("launcherScrim", "#61080910", "#6be8ecf6")
    /// Under the spaces overview: #080912 50% (light #E2E8F4 60%).
    readonly property color overviewDim: _s("overviewDim", "#80080912", "#99e2e8f4")
    /// Outside a screenshot selection: #06070E 58% (light ink 38%).
    readonly property color screenshotDim: _s("screenshotDim", "#9406070e", "#61141828")
    /// Banner and status tints: warning 14% (light 10%), success 14% (light 12%).
    readonly property color warningSoft: _s("warningSoft", "#24ffc79e", "#1a8f5124")
    readonly property color successSoft: _s("successSoft", "#248ee0b6", "#1f1e6d45")

    /// Any color token by name (also ones added to the tokens later).
    function color(name) { return _c(name) }

    // ---------------------------------------------------------------- states
    // The Components · states board's ladder (dark · light: "white n%" and "ink n%",
    // ink #141828). A token of the same name in theme.json wins, so these move to
    // tokens.json without a SodiumUI change.
    /// The light theme's darker accent for fills, marks and rings (PLAN 5b); the accent in dark.
    readonly property color accentInk: _s("accentInk", accent, accent)
    /// Keyboard focus rings.
    readonly property color focusRing: _s("focusRing", accent, accentInk)
    readonly property color rowHover: _s("rowHover", "#0fffffff", "#0d141828")                 // 6 · 5
    readonly property color rowPressed: _s("rowPressed", "#1affffff", "#12141828")             // 10 · 7
    readonly property color rowSelected: _s("rowSelected", "#14ffffff", "#12141828")           // 8 · 7
    readonly property color rowSelectedHover: _s("rowSelectedHover", "#1fffffff", "#17141828") // 12 · 9
    readonly property color fieldHover: _s("fieldHover", "#1affffff", "#12141828")             // 10 · 7
    readonly property color secondaryHover: _s("secondaryHover", "#24ffffff", "#1a141828")     // 14 · 10
    /// Over a colored fill (primary, switch on, checks, solid red): hover and pressed.
    readonly property color liftOverlay: _s("liftOverlay", "#24ffffff", "#24ffffff")           // white 14
    readonly property color pressOverlay: _s("pressOverlay", "#1f000000", "#1f000000")         // black 12
    readonly property color dangerSoftHover: _s("dangerSoftHover", "#3df28f8f", "#24b32d2d")   // 24 · 14
    readonly property color dangerSoftPressed: _s("dangerSoftPressed", "#52f28f8f", "#33b32d2d") // 32 · 20
    readonly property color switchOffHover: _s("switchOffHover", "#3dffffff", "#42141828")     // 24 · 26
    readonly property color switchOffPressed: _s("switchOffPressed", "#4dffffff", "#52141828") // 30 · 32
    readonly property color trackHover: _s("trackHover", "#2effffff", "#29141828")             // 18 · 16
    readonly property color trackLarge: _s("trackLarge", "#1affffff", "#1a141828")             // 10 · 10
    readonly property color trackLargeHover: _s("trackLargeHover", "#24ffffff", "#24141828")   // 14 · 14
    readonly property color roundFill: _s("roundFill", "#14ffffff", "#0f141828")               // 8 · 6
    readonly property color roundHover: _s("roundHover", "#29ffffff", "#1a141828")             // 16 · 10
    readonly property color segmentWell: _s("segmentWell", "#14ffffff", "#0f141828")           // 8 · 6
    readonly property color segmentHover: _s("segmentHover", "#0fffffff", "#0d141828")         // 6 · 5
    readonly property color segmentPressed: _s("segmentPressed", "#1affffff", "#14141828")     // 10 · 8
    readonly property color segmentCurrent: _s("segmentCurrent", "#29ffffff", "#ffffffff")     // 16 · white card
    readonly property color checkRing: _s("checkRing", "#ff9ea4b2", "#ff575d6c")
    readonly property color checkRingHover: _s("checkRingHover", "#ffb8bdc9", "#ff3d4251")
    readonly property color scrollThumb: _s("scrollThumb", "#66ffffff", "#5c141828")           // 40 · 36
    readonly property color scrollThumbHover: _s("scrollThumbHover", "#8fffffff", "#80141828") // 56 · 50
    readonly property color scrollThumbDrag: _s("scrollThumbDrag", "#b8ffffff", "#a3141828")   // 72 · 64
    readonly property color scrollOutline: _s("scrollOutline", "#3d000000", "#80ffffff")       // black 24 · white 50
    readonly property color scrollTrack: _s("scrollTrack", "#0fffffff", "#0d141828")           // 6 · 5
    /// The bar's spaces pill, and nothing else: white 7 % (light ink 7 %). `fill` stays 5 % in light.
    readonly property color spacesPillFill: _s("spacesPillFill", "#12ffffff", "#12141828")    // 7 · 7
    /// Section-nav sidebar current item: accent 22%, + hover 28%.
    readonly property color accentSelected: Qt.rgba(accentInk.r, accentInk.g, accentInk.b, 0.22)
    readonly property color accentSelectedHover: Qt.rgba(accentInk.r, accentInk.g, accentInk.b, 0.28)

    /// `layer` drawn over `base` (source over), as one color: Theme.over(Theme.accent, Theme.liftOverlay).
    function over(base, layer) {
        const b = Qt.color(base), l = Qt.color(layer)
        const a = l.a + b.a * (1 - l.a)
        if (a <= 0)
            return Qt.rgba(0, 0, 0, 0)
        return Qt.rgba((l.r * l.a + b.r * b.a * (1 - l.a)) / a, (l.g * l.a + b.g * b.a * (1 - l.a)) / a,
                       (l.b * l.a + b.b * b.a * (1 - l.a)) / a, a)
    }

    function _s(n, darkValue, lightValue) {
        if (_to[n] !== undefined)
            return _c(n)
        const v = dark ? darkValue : lightValue
        if (increaseContrast && _stateFills.indexOf(n) >= 0) {
            const c = Qt.color(v)
            return Qt.rgba(c.r, c.g, c.b, Math.min(1, c.a * _fillBoost))
        }
        return v
    }

    // ---------------------------------------------------------------- type, sizes
    readonly property var fonts: data.fonts ?? ({ ui: "Inter", mono: "JetBrains Mono" })
    readonly property var typeScale: data.type ?? ({})
    readonly property var radius: data.radius ?? ({})
    readonly property var materials: data.materials ?? ({})
    readonly property var shadows: data.shadows ?? ({})
    /// The tokens' metrics; with Larger text every size in metrics.controls × textScale
    /// (durations and focus-ring widths stay). Chrome (title bar, bar, dock) keeps its size.
    readonly property var metrics: _scaledMetrics(data.metrics ?? ({}), textScale)
    readonly property var accentPresets: data.accentPresets ?? ({})

    /// A font for a type-scale style: Theme.font("body"), Theme.font("label", { weight: 400 }).
    /// Sizes are logical px; they go through pointSize so 12.5 px stays 12.5 px. Larger text
    /// scales every size here, the overrides' too: always pass the drawn size.
    /// `numeric: true` in the overrides: tabular digits (clocks, timers, live percentages, columns);
    /// `palt: false`: no Japanese proportional punctuation (multi-line text, decision 47).
    /// The UI language's rules (I18n; docs/decisions/i18n.md, decisions 21, 43, 45, 47, 52): Chinese,
    /// Japanese and Korean (and the qps-ploca pseudo-locale) get no uppercase and no tracking on any
    /// style, an override's too; label and section are 12 px Medium there; 600 is asked as 700 (static
    /// CJK fonts have no 600: Qt would pick Medium); Japanese single-line styles get `palt`. Every font
    /// lists the Han group's CJK family and emoji after its own, with typographic line metrics.
    function font(style, overrides) {
        const cjk = I18n.isCjk
        void I18n.fontFamilies   // re-run when the families change
        let s = typeScale[style] ?? typeScale.body ?? { size: 13, weight: 400 }
        if (cjk && (style === "label" || style === "section"))
            s = typeScale[style + "Cjk"] ?? Object.assign({}, s, { size: 12, weight: 500 })
        else if (cjk && typeScale[style + "Cjk"] !== undefined)
            s = typeScale[style + "Cjk"]
        const k = textScale
        const f = {
            family: style === "mono" ? fonts.mono : fonts.ui,
            pointSize: s.size * 0.75 * k,
            weight: s.weight ?? 400,
            letterSpacing: (s.tracking ?? 0) * k,
            capitalization: s.uppercase ? Font.AllUppercase : Font.MixedCase
        }
        let numeric = false, paltOff = false
        if (overrides) {
            for (const n in overrides) {
                if (n === "size")
                    f.pointSize = overrides.size * 0.75 * k
                else if (n === "tracking")
                    f.letterSpacing = overrides.tracking * k
                else if (n === "numeric")
                    numeric = overrides.numeric === true
                else if (n === "palt")
                    paltOff = overrides.palt === false
                else
                    f[n] = overrides[n]
            }
        }
        if (cjk) {
            f.letterSpacing = 0
            f.capitalization = Font.MixedCase
            if (f.weight > 550 && f.weight < 650)
                f.weight = 700
        }
        const palt = !paltOff && I18n.scriptGroup === "ja" && _paltStyles.indexOf(style) >= 0
        return I18n.font(Qt.font(f), style === "mono", palt, numeric)
    }
    /// Rich text with `<b>` that draws real bold (decision 72). Qt fakes bold for `<b>` (weight 700) on variable
    /// fonts such as Nunito and Resource Han Rounded, on top of their Bold instance; weight 699 picks the same
    /// instance without it. Wrap every string with `<b>` shown in a Text: `text: Theme.rich(qsTr("Open <b>%1</b>")...)`,
    /// with textFormat RichText or AutoText (StyledText ignores <span>, and Rich text doesn't elide: split such
    /// text into two Texts, the bold one on Theme.font(style, { weight: 700 })).
    function rich(html) {
        return String(html).replace(/<b>/g, "<span style=\"font-weight:699\">").replace(/<\/b>/g, "</span>")
    }
    /// A type style's tokens in the UI language: zh, ja and ko use "<style>Cjk" where there is one
    /// (label and section: 12 px Medium).
    function typeStyle(style) {
        const s = typeScale[style] ?? typeScale.body ?? { size: 13, weight: 400 }
        if (I18n.isCjk && typeScale[style + "Cjk"] !== undefined)
            return typeScale[style + "Cjk"]
        return s
    }
    /// The pixel line height of a type style (decision 45), Larger text included; 0: the font's own.
    /// - Text that doesn't wrap (`wraps` false): the style's `line`, the same in every language, so no
    ///   control changes height on a language switch (control 13/16, label 12/15, heading 15/18, title 20/25).
    /// - Wrapping text in a Latin language keeps today's height: the style's `lineHeight` ratio (message
    ///   1.55; `latinRatio` > 0 instead), else 0 (the font's own).
    /// - Wrapping text in zh, ja and ko on several lines (`severalLines`): `lineCjk` (body 13/20, caption
    ///   12/18, heading 15/22, title 20/28, message 1.5×; a style without one: 1.5× its size). On one
    ///   line: the Latin height, so a one-line message is as tall as in English.
    /// StyledText applies it with half the extra leading above; apps never set lineHeight.
    function lineHeight(style, wraps, latinRatio, severalLines) {
        const s = typeStyle(style)
        const k = textScale
        if (!wraps)
            return s.line ? s.line * k : 0
        const ratio = latinRatio > 0 ? latinRatio : (s.lineHeight ?? 0)
        const latin = ratio > 0 ? Math.round(s.size * ratio) : 0
        if (I18n.isCjk)
            return (severalLines ? (s.lineCjk ?? Math.ceil(s.size * 1.5)) : Math.max(s.line ?? 0, latin)) * k
        return latin * k
    }
    /// The UI language is Chinese, Japanese or Korean: font() drops uppercase and tracking. Never italics for emphasis.
    readonly property bool cjk: I18n.isCjk
    // Japanese `palt` (decision 47): the single-line UI styles. PROVISIONAL: controlSmall and section
    // follow control and label; the type board (L5) settles the list.
    readonly property var _paltStyles: ["control", "controlSmall", "label", "section", "heading", "title"]

    function _scaledMetrics(m, k) {
        if (k === 1 || !m.controls)
            return m
        const scale = (v, key) => {
            if (typeof v === "number")
                return /Ms$|focusRing|Share$|Lines$/.test(key) ? v : Math.round(v * k * 2) / 2
            if (Array.isArray(v))
                return v.map(x => scale(x, key))
            if (v && typeof v === "object") {
                const o = {}
                for (const n in v)
                    o[n] = scale(v[n], n)
                return o
            }
            return v
        }
        const out = Object.assign({}, m)
        out.controls = scale(m.controls, "controls")
        return out
    }

    // ---------------------------------------------------------------- motion
    /// Springs by name (snappy, smooth, bouncy, glide): stiffness k, damping c, mass m,
    /// plus zeta, overshootPct and settleMs. Spring.qml and Reveal.qml run them per frame.
    readonly property var spring: data.springs ?? ({})
    /// The exit curve: { bezier: [x1, y1, x2, y2], durationMs }.
    readonly property var exit: data.exit ?? ({ bezier: [0.3, 0, 0.8, 0.15], durationMs: 160 })
    /// For NumberAnimation: easing.type: Easing.BezierSpline; easing.bezierCurve: Theme.exitCurve
    readonly property var exitCurve: exit.bezier.concat([1, 1])
    /// reduceMotionCrossfadeMs, retintMs, panelSlide, panelStaggerMs, windowOpenFrom, ...
    readonly property var motion: data.motion ?? ({ reduceMotionCrossfadeMs: 120, retintMs: 400 })
    /// Duration for plain color/opacity fades (hover tints, focus rings).
    readonly property int fadeMs: motion.reduceMotionCrossfadeMs ?? 120

    // ---------------------------------------------------------------- re-tint
    // Colors are shown as a mix between the previous palette (_from) and the
    // current one (_to); _t runs 0 -> 1 over retintMs after every theme change.
    property var _from: ({})
    property var _to: ({})
    property real _t: 1

    function _c(n) {
        const b = _to[n]
        if (b === undefined)
            return "transparent"
        if (_t >= 1)
            return b
        const a = _from[n]
        if (a === undefined)
            return b
        const t = _t
        return Qt.rgba(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t,
                       a.b + (b.b - a.b) * t, a.a + (b.a - a.a) * t)
    }

    function _retint(colors) {
        const next = _adjust(colors)
        const shown = {}
        for (const n in next)
            shown[n] = _c(n)
        _from = shown
        _to = next
        _t = 0
        _anim.restart()
    }

    // Increase contrast and Reduce transparency, applied to the theme's colors.
    readonly property var _glassTints: ["window", "windowInactive", "windowTitleActive", "windowTitleInactive", "bar", "panel", "menu",
                                        "tooltip", "dialog", "sheet"]
    readonly property var _softText: ["text2", "text3", "textMuted", "label", "titleInactive", "control",
                                      "controlInactive", "menuText", "barText", "checkRing", "checkRingHover"]
    // Hover, press and selection fills: alpha × 1.6 with Increase contrast, so the states read.
    readonly property real _fillBoost: 1.6
    readonly property var _fills: ["fill", "hover", "selected", "pressed"].concat(_stateFills)
    // the ladder below (also when an older theme.json lacks them: _s)
    readonly property var _stateFills: ["rowHover", "rowPressed", "rowSelected", "rowSelectedHover", "fieldHover",
                                        "secondaryHover", "roundFill", "roundHover", "segmentWell", "segmentHover",
                                        "segmentPressed"]
    readonly property var _lines: ["hairline", "divider", "glassBorder"]
    readonly property var _offTracks: ["track", "switchOff"]
    function _adjust(raw) {
        if (!increaseContrast && !reduceTransparency)
            return raw
        const out = Object.assign({}, raw)
        const withAlpha = (c, a) => Qt.rgba(c.r, c.g, c.b, Math.min(1, a))
        if (increaseContrast) {
            // Secondary text halfway to the primary.
            const t = raw.text
            if (t !== undefined)
                for (const n of _softText)
                    if (raw[n] !== undefined)
                        out[n] = Qt.rgba(raw[n].r + (t.r - raw[n].r) * 0.5, raw[n].g + (t.g - raw[n].g) * 0.5,
                                         raw[n].b + (t.b - raw[n].b) * 0.5, raw[n].a)
            // Lines × 2.2: white 9 % / 7 % become 20 % / 15 % (the ladder's pressed and selected steps).
            for (const n of _lines)
                if (raw[n] !== undefined)
                    out[n] = withAlpha(raw[n], raw[n].a * 2.2)
            // State fills (fill, hover, selected, pressed and the ladder).
            for (const n of _fills)
                if (raw[n] !== undefined)
                    out[n] = withAlpha(raw[n], raw[n].a * _fillBoost)
            // Off tracks (sliders, switches) × 1.5, so a control's shape reads.
            for (const n of _offTracks)
                if (raw[n] !== undefined)
                    out[n] = withAlpha(raw[n], raw[n].a * 1.5)
            // Glass halfway to solid: the bar 42 % → 71 %, panels 60 % → 80 %, windows 84 % → 92 %.
            for (const n of _glassTints)
                if (raw[n] !== undefined)
                    out[n] = withAlpha(raw[n], raw[n].a + (1 - raw[n].a) * 0.5)
        }
        if (reduceTransparency)
            for (const n of _glassTints)
                if (raw[n] !== undefined && raw[n].a > 0)
                    out[n] = withAlpha(raw[n], 1)
        return out
    }

    property NumberAnimation _anim: NumberAnimation {
        target: theme
        property: "_t"
        from: 0
        to: 1
        duration: theme.motion.retintMs ?? 400
        easing.type: Easing.InOutQuad
    }

    property Connections _watch: Connections {
        target: ThemeStore
        function onThemeChanged() { theme._retint(ThemeStore.theme.colors ?? {}) }
    }
    onIncreaseContrastChanged: _retint(ThemeStore.theme.colors ?? {})
    onReduceTransparencyChanged: _retint(ThemeStore.theme.colors ?? {})

    Component.onCompleted: _to = _adjust(ThemeStore.theme.colors ?? {})
}
