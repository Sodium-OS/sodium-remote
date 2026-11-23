// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! REMOTE-1's scale.
//!
//! Hyprland takes only a scale k/120 that gives a whole-pixel logical size, and
//! otherwise substitutes its own with a red notification that would show in the
//! stream. So remoted snaps first: **the nearest valid scale by ratio, ties to
//! the smaller UI** (the smaller scale), and `debug:disable_scale_checks` stays off.
//! "Larger" and "Smaller" step along the same list, which keeps only scales whose
//! pixel period is at most 8.
//!
//! **Scale first, then the size**: the list comes from the size the
//! stream would have in full screen (the client's display, or a fixed size
//! itself), never from a window's size; in windowed mode REMOTE-1 then becomes
//! the largest size at or below the window that is a whole multiple of the
//! scale's pixel period on each axis ([`fit`]), and the client letterboxes the
//! difference (at most p−1 px).

use serde::{Deserialize, Serialize};

/// The UI size the client asked for (the client's "UI size" setting).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiSize {
    /// The Mac's own UI size: its panel pixels ÷ its "looks like" points.
    #[default]
    Match,
    /// One step larger than Match on the valid list.
    Larger,
    /// One step smaller than Match.
    Smaller,
    /// The desk monitor's scale.
    Desk,
}

/// The range remoted chooses from: 1.0 to 3.0.
const K_MIN: u32 = 120;
const K_MAX: u32 = 360;
/// The largest pixel period a listed scale may have:
/// a larger one means resize steps over 8 px and a letterbox band up to p−1 px.
const P_MAX: u32 = 8;

/// Every scale Hyprland accepts for a `w`×`h` output whose pixel period is at
/// most [`P_MAX`], ascending. 1.0 (p = 1) is always in it.
pub fn valid_scales(w: u32, h: u32) -> Vec<f64> {
    let (w, h) = (w as u64 * 120, h as u64 * 120);
    (K_MIN..=K_MAX)
        .filter(|&k| w.is_multiple_of(k as u64) && h.is_multiple_of(k as u64))
        .map(|k| k as f64 / 120.0)
        .filter(|&s| period(s) <= P_MAX)
        .collect()
}

/// Index of the nearest valid scale to `wanted` by ratio; ties go to the smaller.
fn nearest(list: &[f64], wanted: f64) -> usize {
    let mut best = 0;
    let mut best_d = f64::INFINITY;
    for (i, &s) in list.iter().enumerate() {
        let d = (s / wanted).ln().abs();
        // Ascending list: a later scale replaces the best only when strictly nearer.
        if d < best_d - 1e-12 {
            best = i;
            best_d = d;
        }
    }
    best
}

/// The scale for REMOTE-1 at `w`×`h`. `wanted` is the client's UI ratio (Match),
/// `desk` the desk monitor's scale (UiSize::Desk). 1.0 is always valid.
pub fn choose(w: u32, h: u32, ui: UiSize, wanted: f64, desk: f64) -> f64 {
    let list = valid_scales(w, h);
    let wanted = if wanted.is_finite() && wanted > 0.0 { wanted } else { 1.0 };
    match ui {
        UiSize::Match => list[nearest(&list, wanted)],
        UiSize::Desk => list[nearest(&list, desk)],
        UiSize::Larger => list[(nearest(&list, wanted) + 1).min(list.len() - 1)],
        UiSize::Smaller => list[nearest(&list, wanted).saturating_sub(1)],
    }
}

/// The scale's pixel period: p = k / gcd(k, 120). A size is valid at this
/// scale when both sides are multiples of p (8 at 1.6, 2 at 2.0, 4 at 1.333).
pub fn period(scale: f64) -> u32 {
    let k = (scale * 120.0).round() as u32;
    let (mut a, mut b) = (k, 120);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    k / a.max(1)
}

/// The largest size at or below `w`×`h` that is valid at `scale`.
pub fn fit(w: u32, h: u32, scale: f64) -> (u32, u32) {
    let p = period(scale);
    (w - w % p, h - h % p)
}

/// The scale as a monitor rule wants it: k/120 written out exactly enough.
pub fn rule_value(scale: f64) -> String {
    let k = (scale * 120.0).round() as u32;
    if k.is_multiple_of(120) {
        format!("{}", k / 120)
    } else {
        // 8 decimals: Hyprland rounds to the nearest k/120 itself, and this is far closer.
        format!("{:.8}", k as f64 / 120.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_air_13_matches_at_1_6_and_steps_along_the_list() {
        // 2560 / 1470 points: 1.741.
        let wanted = 2560.0 / 1470.0;
        assert_eq!(choose(2560, 1664, UiSize::Match, wanted, 1.5), 1.6);
        assert_eq!(choose(2560, 1664, UiSize::Larger, wanted, 1.5), 2.0);
        assert_eq!(choose(2560, 1664, UiSize::Smaller, wanted, 1.5), 1920.0 / 1440.0);
        // The desk's 1.5 isn't valid at 2560×1664: 1.6 is the nearer neighbour.
        assert_eq!(choose(2560, 1664, UiSize::Desk, wanted, 1.5), 1.6);
        // 2.133 (p = 32) is no longer listed: after 2.0 comes 2.667 (p = 8).
        let list = valid_scales(2560, 1664);
        let i = list.iter().position(|&s| s == 1.6).unwrap();
        assert_eq!(&list[i - 1..=i + 2], &[160.0 / 120.0, 1.6, 2.0, 320.0 / 120.0]);
        assert!(!list.contains(&(256.0 / 120.0)));
        assert_eq!(rule_value(1.6), "1.60000000");
        assert_eq!(rule_value(2.0), "2");
    }

    #[test]
    fn mac_air_13_streaming_backing_pixels_matches_at_2_0() {
        // 2940×1848 (1470×924 points at 2×): Match Mac is exactly 2.0, and its
        // neighbours are the scales with p ≤ 8: 1.75 and 2.333 (p = 7), not
        // 1.867 (p = 28) or 2.1 (p = 21).
        assert_eq!(choose(2940, 1848, UiSize::Match, 2.0, 1.5), 2.0);
        assert_eq!(choose(2940, 1848, UiSize::Smaller, 2.0, 1.5), 1.75);
        assert_eq!(choose(2940, 1848, UiSize::Larger, 2.0, 1.5), 280.0 / 120.0);
        let list = valid_scales(2940, 1848);
        let want = [120, 140, 144, 160, 168, 180, 210, 240, 280, 360].map(|k| k as f64 / 120.0);
        assert_eq!(list, want);
        assert!(list.iter().all(|&s| period(s) <= 8));
    }

    #[test]
    fn mac_air_15_and_a_tie() {
        // The 15" Air: 2880×1864 at 2880/1710 = 1.684 → 1.6.
        assert_eq!(choose(2880, 1864, UiSize::Match, 2880.0 / 1710.0, 1.5), 1.6);
        // Exactly between 1.6 and 2.0 by ratio (their geometric mean): the smaller UI wins.
        let mid = (1.6f64 * 2.0).sqrt();
        assert_eq!(choose(2560, 1664, UiSize::Match, mid, 1.0), 1.6);
    }

    #[test]
    fn an_odd_window_keeps_the_display_scale_and_rounds_down() {
        // A windowed 2236×1349 has no fractional scale of its own, so the scale
        // comes from the 13" Air's display, then the size rounds down.
        assert_eq!(valid_scales(2236, 1349), vec![1.0]);
        let s = choose(2560, 1664, UiSize::Match, 2560.0 / 1470.0, 1.5);
        assert_eq!((period(s), fit(2236, 1349, s)), (8, (2232, 1344)));
        assert_eq!([period(2.0), period(160.0 / 120.0), period(1.5), period(256.0 / 120.0)], [2, 4, 3, 32]);
        assert_eq!(fit(2560, 1664, s), (2560, 1664));
    }
}
