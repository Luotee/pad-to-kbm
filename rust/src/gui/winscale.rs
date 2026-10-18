//! Whole-pixel window size for the drawn window (v3.3.0).
//!
//! The page draws the MapleStory window (frame, title bar, board) as pixel art, BASE logical
//! pixels at 1x. It is only sharp when every art pixel covers a whole number `n` of physical
//! pixels, so the window is always `BASE * n` physical pixels and the frontend
//! (`web/chrome.js` `fitWindow`) scales the page by `n / devicePixelRatio`. Windows display
//! scales are fractional (125 %, 250 %…): `n` is the whole number nearest the display scale
//! that still fits the monitor's work area, never below 1.
//!
//! Tauri-free so `cargo test` covers it without the `gui` feature.

/// The drawn window at 1x: the 980x640 board plus a 7 px frame and the 22 px title bar.
pub const BASE: (u32, u32) = (994, 669);

const MAX_SCALE: u32 = 8;

/// Physical size of the window at whole scale `n`.
pub fn size_for(n: u32) -> (u32, u32) {
    (BASE.0 * n, BASE.1 * n)
}

fn fits(n: u32, work_w: u32, work_h: u32) -> bool {
    let (w, h) = size_for(n);
    w <= work_w && h <= work_h
}

/// The whole scale nearest `target` that fits the work area (a tie goes to the larger), at least 1.
fn nearest_fitting(target: f64, work_w: u32, work_h: u32) -> u32 {
    let mut best = 1;
    let mut best_d = f64::INFINITY;
    for n in 1..=MAX_SCALE {
        if !fits(n, work_w, work_h) {
            continue;
        }
        let d = (n as f64 - target).abs();
        if d <= best_d {
            best = n;
            best_d = d;
        }
    }
    best
}

/// Scale for a new window on a monitor with display scale `dpr` and a work area of
/// `work_w` x `work_h` physical pixels.
pub fn pick_scale(dpr: f64, work_w: u32, work_h: u32) -> u32 {
    if !dpr.is_finite() {
        return 1;
    }
    nearest_fitting(dpr, work_w, work_h)
}

/// Scale a window the user resized to `phys_w` x `phys_h` snaps to: the whole scale nearest
/// its size that fits the work area.
pub fn snap_scale(phys_w: u32, phys_h: u32, work_w: u32, work_h: u32) -> u32 {
    let r = (phys_w as f64 / BASE.0 as f64).min(phys_h as f64 / BASE.1 as f64);
    nearest_fitting(r, work_w, work_h)
}
