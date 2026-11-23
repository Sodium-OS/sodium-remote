// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import "motion.js" as M

// A number that follows `to` on one of the motion tokens' springs, solved in
// closed form every frame (FrameAnimation), so it is smooth at 160 Hz and keeps
// its velocity when `to` changes mid-flight.
//
//     Spring { id: knob; to: control.checked ? 21 : 3; preset: "snappy" }
//     Rectangle { x: knob.value }
//
// With Reduce motion on (or enabled: false) the value jumps to `to`; pair it
// with a 120 ms opacity or color fade where a change should still be seen.
QtObject {
    id: spring

    /// Target value. The first value is taken without animating.
    property real to: 0
    /// Output: the animated value.
    property real value: to
    /// Output: current velocity (units per second).
    property real velocity: 0
    /// snappy | smooth | bouncy | glide (Theme.spring)
    property string preset: "snappy"
    /// Spring parameters { stiffness, damping, mass }; defaults to the preset's.
    property var params: Theme.spring[preset]
    /// false: jump straight to `to`.
    property bool enabled: true
    readonly property bool running: _frames.running

    signal settled()

    /// Jump to v (default: `to`) and stop.
    function snap(v) {
        _frames.running = false
        value = v === undefined ? to : v
        velocity = 0
    }

    property bool _ready: false
    property real _d0: 0
    property real _v0: 0
    property real _t: 0
    property real _scale: 1

    function _start() {
        if (!_ready)
            return
        if (!enabled || Theme.reduceMotion || !params) {
            snap(to)
            settled()
            return
        }
        _d0 = value - to
        _v0 = velocity
        _t = 0
        _scale = Math.max(Math.abs(_d0), Math.abs(_v0) / 20)
        if (M.atRest(_d0, _v0, _scale)) {
            snap(to)
            settled()
            return
        }
        _frames.running = true
    }

    function _step(dt) {
        _t += dt
        const s = M.spring(params.stiffness, params.damping, params.mass || 1, _d0, _v0, _t)
        if (M.atRest(s.d, s.v, _scale)) {
            _frames.running = false
            value = to
            velocity = 0
            settled()
            return
        }
        value = to + s.d
        velocity = s.v
    }

    property FrameAnimation _frames: FrameAnimation {
        onTriggered: spring._step(frameTime)
    }

    // Reduce motion turned on mid-flight: land at once.
    property Connections _reduceWatch: Connections {
        target: Theme
        function onReduceMotionChanged() {
            if (Theme.reduceMotion && spring._frames.running) {
                spring.snap(spring.to)
                spring.settled()
            }
        }
    }

    onToChanged: _start()
    Component.onCompleted: {
        value = to
        _ready = true
    }
}
