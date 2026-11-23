// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Window

// A Graphite Glass surface: the material's tint (Hyprland's blur shows through
// it), a 1 px hairline, a 1 px top highlight and, for floating surfaces, a soft
// shadow drawn outside the shape only. Children are the surface's content.
//
//     Glass { strength: "panel"; width: 380; height: 420 }
//
// strength sets the tint, radius and shadow from the tokens:
//   bar      bar tint, square, no hairline or shadow (draw the bar's bottom line yourself)
//   panel    control center, launcher, notifications: radius 22, panel shadow
//   window   our app windows: radius 16 squircle (Hyprland's corners), no shadow
//            (the compositor draws window shadows and the active halo)
//   menu     menus and popovers: radius 14, popup shadow
//   tooltip  tooltips: radius 8, tooltip shadow
//   toast    notification toasts: radius 18, the menu tint, panel shadow (Components · states C3)
//   dialog   in-app dialogs and sheets (P2Dialogs G1, G6, G7): radius 16, the dialog tint (97%),
//            dialog shadow (0 30 80, 60%)
//   sheet    popovers and system prompts (P2Bar B9, P2Dialogs G8): radius 18, the sheet tint,
//            sheet shadow (0 30 80, 55%)
// The shadow reaches `shadowMargin` px beyond the item: leave that much room
// (a layer surface or popup must be that much larger than the glass).
//
// In-window blur: Hyprland frosts only what is behind a whole window or layer,
// so a surface floating over its own window's content (a menu, tooltip or
// popover inside an app) sets `backdrop` to the item drawn behind it. The glass
// then lays its tint over a blurred, saturated copy of that item's pixels under
// it (the boards' blur 30, saturate 170%). Only the glass's rect (plus the blur's
// reach) is rendered, and only while the glass is visible. Menu and Tooltip do
// this by themselves (their `blurBehind`); shell surfaces in their own layer
// leave it off, since the compositor blurs them.
//
//     Glass { strength: "menu"; backdrop: page; backdropTrack: [x, y] }
Item {
    id: glass

    property string strength: "panel"
    property real radius: ({
        bar: 0,
        panel: Theme.radius.panel,
        window: Theme.radius.window,
        menu: Theme.radius.menu,
        tooltip: Theme.metrics.controls.tooltip.radius,
        toast: Theme.radius.toast,
        dialog: Theme.radius.dialog ?? 16,
        sheet: Theme.radius.popover ?? 18
    })[strength] ?? Theme.radius.panel
    property color tint: ({
        bar: Theme.bar,
        panel: Theme.panel,
        window: Theme.window,
        menu: Theme.menu,
        tooltip: Theme.tooltip,
        toast: Theme.menu,
        dialog: Theme.dialog,
        sheet: Theme.sheet
    })[strength] ?? Theme.panel
    /// 1 px edge; transparent for none.
    property color hairline: strength === "bar" ? "transparent" : Theme.glassBorder
    /// 1 px band just inside the top edge; transparent for none.
    property color topHighlight: strength === "bar" ? "transparent" : Theme.highlight
    /// { y, blur, alpha } from Theme.shadows, or null for none.
    property var shadow: ({
        panel: Theme.shadows.panel,
        menu: Theme.shadows.popup,
        tooltip: Theme.shadows.tooltip,
        toast: Theme.shadows.panel,
        dialog: Theme.shadows.dialog ?? ({ y: 30, blur: 80, alpha: 0.6 }),
        sheet: Theme.shadows.sheet ?? ({ y: 30, blur: 80, alpha: 0.55 })
    })[strength] ?? null
    /// Corner curve: 2 is a circle; windows use Hyprland's squircle power.
    property real cornerPower: strength === "window" ? (Theme.radius.squirclePower ?? 4) : 2
    readonly property real shadowMargin: shadow ? Math.ceil(shadow.blur + Math.abs(shadow.y)) : 0

    /// In-window blur: the item behind this glass in the same window (not one that contains
    /// the glass); null for none.
    property Item backdrop: null
    /// The backdrop's CSS blur radius and saturation (Components board .glass).
    property real backdropBlur: 30
    property real backdropSaturate: 1.7
    /// Change it whenever the glass moves relative to the backdrop (a popup's x, y or slide),
    /// so the blurred sample follows.
    property var backdropTrack
    /// The backdrop is being rendered and blurred now.
    readonly property bool blurring: backdrop !== null && !Theme.reduceTransparency && visible && width > 0 && height > 0
    /// Hand the colors to the shader straight. Qt's ShaderEffect premultiplies a `color`
    /// uniform and glass.frag premultiplies again, so a translucent tint comes out too dark:
    /// rgb · a² instead of rgb · a (a light 60 % glass turns gray; dark tints and the white
    /// hairline and highlight barely show). On for every surface since 2026-11-04 (the light
    /// theme's widgets came out gray-blue instead of the boards' near-white).
    property bool straightColors: true

    // How far the blur reaches past the edges (2 sigma), so they blur real content.
    readonly property real _reach: Math.ceil(Math.min(2 * backdropBlur, 64))
    readonly property rect _sample: {
        void backdropTrack
        void glass.x; void glass.y; void glass.width; void glass.height
        if (!blurring)
            return Qt.rect(0, 0, 0, 0)
        const r = backdrop.mapFromItem(glass, -_reach, -_reach, width + 2 * _reach, height + 2 * _reach)
        return Qt.rect(Math.floor(r.x), Math.floor(r.y), Math.ceil(r.width), Math.ceil(r.height))
    }

    ShaderEffectSource {
        id: behind
        visible: false
        sourceItem: glass.blurring ? glass.backdrop : null
        sourceRect: glass._sample
        // One texel per logical px is plenty under a 30 px blur; mipmaps prefilter it.
        textureSize: Qt.size(Math.max(1, glass._sample.width), Math.max(1, glass._sample.height))
        mipmap: true
        smooth: true
        live: glass.blurring
        hideSource: false
        recursive: false
    }

    ShaderEffect {
        id: fx

        readonly property real m: glass.shadowMargin
        x: -m
        y: -m
        width: glass.width + 2 * m
        height: glass.height + 2 * m
        z: -1

        property size itemSize: Qt.size(width, height)
        property rect body: Qt.rect(m, m, glass.width, glass.height)
        property real radius: glass.radius
        property real power: glass.cornerPower
        property real aa: 1 / Math.max(1, glass.Window.window ? glass.Window.window.devicePixelRatio : Screen.devicePixelRatio)
        property real borderWidth: 1
        property var tint: glass.straightColors ? fx.straight(fx.solid(glass.tint)) : fx.solid(glass.tint)
        // Reduce transparency: a tint of its own (the lock's glass) goes solid too.
        function solid(c) { return Theme.reduceTransparency && c.a > 0 ? Qt.rgba(c.r, c.g, c.b, 1) : c }
        property var border: glass.straightColors ? fx.straight(glass.hairline) : glass.hairline
        property var highlight: glass.straightColors ? fx.straight(glass.topHighlight) : glass.topHighlight
        function straight(c) { return Qt.vector4d(c.r, c.g, c.b, c.a) }
        property var shadowColor: glass.shadow ? Qt.vector4d(Theme.shadow.r, Theme.shadow.g, Theme.shadow.b, glass.shadow.alpha) : Qt.vector4d(0, 0, 0, 0)   // straight, like tint: a `color` uniform arrives premultiplied and the shader premultiplies again (the light shadow #1E243C came out near black)
        property point shadowOffset: Qt.point(0, glass.shadow ? glass.shadow.y : 0)
        property real shadowSigma: glass.shadow ? glass.shadow.blur / 2 : 0
        // In-window blur (glassblur.frag.qsb only).
        property var backdrop: behind
        // The sampled rect is the glass grown by the blur's reach on every side.
        property rect backdropRect: Qt.rect(m - glass._reach, m - glass._reach,
                                            glass.width + 2 * glass._reach, glass.height + 2 * glass._reach)
        property real blurSigma: Math.max(1, glass.backdropBlur)
        property real blurLod: Math.max(0, Math.log2(blurSigma * 0.6) - 0.5)
        property real saturation: glass.backdropSaturate

        fragmentShader: glass.blurring ? "qrc:/sodiumui/shaders/glassblur.frag.qsb" : "qrc:/sodiumui/shaders/glass.frag.qsb"
    }
}
