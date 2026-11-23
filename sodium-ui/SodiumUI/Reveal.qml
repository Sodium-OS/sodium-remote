// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import "motion.js" as M

// Appear/disappear motion for popups, panels and toasts (PLAN section 7, P4Motion DL2):
// in on a spring with the alpha rising linearly over 120 ms; out on the exit curve
// (bezier .3,0,.8,.15, 160 ms), alpha and movement together.
//
//     Reveal { id: reveal; shown: menu.opened; preset: "snappy" }
//     Item {
//         visible: reveal.visible
//         opacity: reveal.opacity
//         scale: 0.96 + 0.04 * reveal.progress          // pop from 96%
//         // or: y: (1 - reveal.progress) * -Theme.motion.panelSlide
//     }
//
// progress runs 0 -> 1 on the way in (it may overshoot with bouncy) and back to `exitTo`
// on the way out. Interruptions retarget: showing it while it leaves (or hiding it while
// it comes in) starts from where it is, keeping its speed; it never jumps or restarts.
// Each new appearance from fully hidden starts at progress 0.
//
// Staggered rows (a popover's or panel's content, 20 ms apart): set `rows` to how many
// there are and give row n (1, 2, 3…) `opacity: reveal.rowOpacity(n)`. They fade in
// n × Theme.motion.panelStaggerMs after the surface; on the way out they leave with it.
//
// With Reduce motion on, progress stays 1 and only opacity fades, 120 ms each way, rows
// included. Turning Reduce motion on or off mid-motion switches over at once.
//
// A shell layer that maps only while something is shown takes 75–130 ms to reach the
// screen (measured in a nested Hyprland): its window is made when it is shown, then waits
// for the compositor. Set `layerItem` to an item in that layer: a fresh way in then holds
// its first pose until the layer is on screen, so the motion isn't spent off screen.
QtObject {
    id: reveal

    property bool shown: false
    /// Spring for the way in: snappy | smooth | bouncy | glide
    property string preset: "snappy"
    /// An item in a layer that maps as this appears (a shell layer surface). A fresh way in
    /// starts once that layer has presented its first frame (at most 250 ms later). null:
    /// start at once.
    property Item layerItem: null
    /// Where the way out ends, in progress units: 0 is the reverse of the way in. The
    /// launcher pops from 94 % and leaves to 97 % (P4Motion LW1): exitTo 0.5.
    property real exitTo: 0
    /// Content rows that stagger in after the surface (see rowOpacity()).
    property int rows: 0
    /// Output: 0 hidden -> 1 shown. Map it to scale or a slide.
    property real progress: shown ? 1 : 0
    /// Output: 0 -> 1.
    property real opacity: shown ? 1 : 0
    /// Output: progress units per second (the spring's, or the exit curve's).
    property real velocity: 0
    /// Keep the item visible while it is shown or still fading out.
    readonly property bool visible: shown || opacity > 0
    readonly property bool running: _frames.running

    /// Emitted when the motion in either direction has finished.
    signal finished()

    /// Opacity of content row n (1, 2, 3…): it fades in over 120 ms, n × 20 ms after the
    /// surface started to appear. 1 outside a staggered entrance.
    function rowOpacity(n) {
        if (!_staggering)
            return 1
        const k = (_clock - n * _staggerMs) / Theme.fadeMs
        return k <= 0 ? 0 : k >= 1 ? 1 : k
    }

    property bool _ready: false
    property string _mode: ""     // enter | exit | fade
    property real _t: 0
    property real _p0: 0
    property real _v0: 0
    property real _o0: 0
    property real _restScale: 1
    property bool _springDone: false
    property bool _staggering: false
    property real _clock: 0       // ms into this entrance, for the rows
    readonly property real _staggerMs: Theme.motion.panelStaggerMs ?? 20
    // The layer's window (made when the layer is first shown, so often null until then).
    readonly property var _window: layerItem ? layerItem.Window.window : null
    // Frames the layer has drawn since it last became visible. Wayland holds the next
    // frame until the compositor has shown the last one, so the second means "on screen".
    property int _drawnFrames: 0
    readonly property bool _drawn: _drawnFrames >= 2
    property bool _waiting: false

    function _go() {
        if (!_ready)
            return
        if (shown && opacity <= 0 && layerItem && !_drawn) {
            // Hold the first pose until the layer is on screen.
            _waiting = true
            _waitLimit.restart()
            _frames.running = false
            _mode = ""
            progress = Theme.reduceMotion ? 1 : 0
            velocity = 0
            return
        }
        _waiting = false
        _waitLimit.stop()
        _t = 0
        _p0 = progress
        _v0 = velocity
        _o0 = opacity
        _springDone = false
        if (Theme.reduceMotion) {
            _mode = "fade"
            _staggering = false
            progress = 1
            velocity = 0
        } else if (shown) {
            _mode = "enter"
            if (opacity <= 0) {
                // A fresh appearance: from the start, still, with its rows staggered.
                _p0 = 0
                _v0 = 0
                progress = 0
                _staggering = rows > 0
            } else {
                _staggering = false
            }
            _clock = 0
            _restScale = Math.max(Math.abs(1 - _p0), Math.abs(_v0) / 20, 0.05)
        } else {
            _mode = "exit"
            _staggering = false
        }
        _frames.running = true
    }

    function _done() {
        _frames.running = false
        _staggering = false
        velocity = 0
        progress = shown || _mode === "fade" ? 1 : exitTo
        opacity = shown ? 1 : 0
        _mode = ""
        finished()
    }

    function _step(dt) {
        if (_mode === "")
            return
        _t += dt
        const ms = _t * 1000
        if (_mode === "fade") {
            const target = shown ? 1 : 0
            const k = Math.min(1, ms / Theme.fadeMs)
            opacity = _o0 + (target - _o0) * k
            if (k >= 1)
                _done()
        } else if (_mode === "enter") {
            // The alpha: linear from where it was, 120 ms for the whole way (DL2).
            opacity = Math.min(1, _o0 + ms / Theme.fadeMs)
            _clock = ms
            if (!_springDone) {
                const p = Theme.spring ? Theme.spring[preset] : undefined
                if (!p || !(p.stiffness > 0) || !(p.damping >= 0)) {
                    // An unknown preset (or a theme without springs): no movement rather than
                    // throwing on every frame and never finishing.
                    _springDone = true
                    progress = 1
                    velocity = 0
                } else {
                    const s = M.spring(p.stiffness, p.damping, p.mass || 1, _p0 - 1, _v0, _t)
                    if (M.atRest(s.d, s.v, _restScale)) {
                        _springDone = true
                        progress = 1
                        velocity = 0
                    } else {
                        progress = 1 + s.d
                        velocity = s.v
                    }
                }
            }
            const rowsDone = !_staggering || ms >= rows * _staggerMs + Theme.fadeMs
            if (_springDone && opacity >= 1 && rowsDone)
                _done()
        } else {
            const dur = Theme.exit && Theme.exit.durationMs > 0 ? Theme.exit.durationMs : 160
            const b = Theme.exit && Theme.exit.bezier && Theme.exit.bezier.length === 4 ? Theme.exit.bezier : [0.3, 0, 0.8, 0.15]
            const x = Math.min(1, ms / dur)
            const e = M.bezier(b[0], b[1], b[2], b[3], x)
            const before = progress
            progress = _p0 + (exitTo - _p0) * e
            opacity = _o0 * (1 - e)
            velocity = dt > 0 ? (progress - before) / dt : 0
            if (x >= 1)
                _done()
        }
    }

    property FrameAnimation _frames: FrameAnimation {
        onTriggered: reveal._step(frameTime)
    }

    // The layer's frames: afterAnimating comes on the GUI thread just before each frame is
    // drawn (frameSwapped would come from the render thread).
    property Connections _windowWatch: Connections {
        target: reveal._window
        ignoreUnknownSignals: true
        function onAfterAnimating() {
            if (reveal._drawn)
                return
            reveal._drawnFrames++
            if (reveal._drawn && reveal._waiting)
                reveal._go()
        }
        function onVisibleChanged() {
            if (reveal._window && !reveal._window.visible)
                reveal._drawnFrames = 0
        }
    }
    on_WindowChanged: _drawnFrames = 0
    property Timer _waitLimit: Timer {
        interval: 250
        onTriggered: {
            if (reveal._waiting) {
                reveal._drawnFrames = 2
                reveal._go()
            }
        }
    }

    // Reduce motion switched mid-motion: carry on from here in the other mode.
    property Connections _reduceWatch: Connections {
        target: Theme
        function onReduceMotionChanged() {
            if (reveal._mode !== "")
                reveal._go()
        }
    }

    onShownChanged: _go()
    Component.onCompleted: {
        progress = shown ? 1 : 0
        opacity = shown ? 1 : 0
        _ready = true
    }
}
