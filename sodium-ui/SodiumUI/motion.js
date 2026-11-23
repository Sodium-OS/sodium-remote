// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
.pragma library

// Motion math for SodiumUI, in closed form so it is exact at any frame rate
// (160 Hz here) instead of SpringAnimation's fixed 16 ms steps.

// Damped spring  m*d'' + c*d' + k*d = 0  for the displacement d from the target.
// Returns { d, v } at time t (seconds) from displacement d0 and velocity v0.
// These are Hyprland's spring parameters (hyprutils solves the same equation),
// so the shell and the compositor move alike.
function spring(k, c, m, d0, v0, t) {
    const gamma = c / (2 * m)
    const w0 = Math.sqrt(k / m)
    const e = Math.exp(-gamma * t)
    if (Math.abs(gamma - w0) <= 1e-6 * w0) {            // critically damped (glide)
        const b = v0 + gamma * d0
        return { d: e * (d0 + b * t), v: e * (v0 - gamma * b * t) }
    }
    if (gamma < w0) {                                    // underdamped (snappy, smooth, bouncy)
        const wd = Math.sqrt(w0 * w0 - gamma * gamma)
        const cs = Math.cos(wd * t), sn = Math.sin(wd * t)
        return {
            d: e * (d0 * cs + (v0 + gamma * d0) / wd * sn),
            v: e * (v0 * cs - (w0 * w0 * d0 + gamma * v0) / wd * sn)
        }
    }
    const r = Math.sqrt(gamma * gamma - w0 * w0)         // overdamped
    const r1 = -gamma + r, r2 = -gamma - r
    const c2 = (v0 - r1 * d0) / (r2 - r1), c1 = d0 - c2
    const e1 = Math.exp(r1 * t), e2 = Math.exp(r2 * t)
    return { d: c1 * e1 + c2 * e2, v: r1 * c1 * e1 + r2 * c2 * e2 }
}

// True when a spring that started `scale` away is close enough to stop:
// within 0.1% of the distance (at least 1e-4) and nearly still.
function atRest(d, v, scale) {
    const eps = Math.max(Math.abs(scale) * 1e-3, 1e-4)
    return Math.abs(d) < eps && Math.abs(v) < eps * 10
}

// CSS cubic-bezier(x1, y1, x2, y2) at progress x in [0, 1].
function bezier(x1, y1, x2, y2, x) {
    if (x <= 0)
        return 0
    if (x >= 1)
        return 1
    const cx = 3 * x1, bx = 3 * (x2 - x1) - cx, ax = 1 - cx - bx
    const cy = 3 * y1, by = 3 * (y2 - y1) - cy, ay = 1 - cy - by
    const sx = s => ((ax * s + bx) * s + cx) * s
    let s = x
    for (let i = 0; i < 8; i++) {                       // Newton
        const err = sx(s) - x
        if (Math.abs(err) < 1e-7)
            break
        const dx = (3 * ax * s + 2 * bx) * s + cx
        if (Math.abs(dx) < 1e-6)
            break
        s -= err / dx
    }
    if (!(s >= 0 && s <= 1) || Math.abs(sx(s) - x) > 1e-6) {
        let lo = 0, hi = 1                               // bisection fallback
        s = x
        for (let i = 0; i < 50 && Math.abs(sx(s) - x) > 1e-7; i++) {
            if (sx(s) < x)
                lo = s
            else
                hi = s
            s = (lo + hi) / 2
        }
    }
    return ((ay * s + by) * s + cy) * s
}

// Linear mix of two colors (QML color values), t in [0, 1].
function mix(a, b, t) {
    return Qt.rgba(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t,
                   a.b + (b.b - a.b) * t, a.a + (b.a - a.a) * t)
}
