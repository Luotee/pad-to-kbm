//! The drawn window (v3.3.0) is pixel art: it is only sharp at a whole number of physical pixels per
//! art pixel. `winscale` picks that number and the window size; the frontend (chrome.js fitWindow)
//! reads it back from the size.

use dualsense_mapper::gui::winscale::{pick_scale, size_for, snap_scale, BASE};

#[test]
fn base_is_the_drawn_window() {
    assert_eq!(BASE, (994, 669));
    assert_eq!(size_for(3), (2982, 2007));
}

#[test]
fn picks_the_whole_scale_nearest_the_display_scale_that_fits() {
    assert_eq!(pick_scale(1.0, 1920, 1032), 1);
    assert_eq!(pick_scale(1.25, 1920, 1032), 1);
    assert_eq!(pick_scale(1.5, 1920, 1032), 1, "2x (1988 wide) does not fit a 1080p work area");
    assert_eq!(pick_scale(1.5, 2560, 1392), 2);
    assert_eq!(pick_scale(1.75, 2560, 1392), 2);
    assert_eq!(pick_scale(2.0, 3840, 2088), 2);
    assert_eq!(pick_scale(2.25, 3840, 2088), 2);
    assert_eq!(pick_scale(2.5, 3840, 2040), 3, "a tie goes to the larger scale when it fits");
    assert_eq!(pick_scale(3.0, 3840, 2040), 3);
}

#[test]
fn never_below_one() {
    assert_eq!(pick_scale(2.5, 1000, 700), 1);
    assert_eq!(pick_scale(1.0, 0, 0), 1);
    assert_eq!(pick_scale(f64::NAN, 1920, 1032), 1);
    assert_eq!(snap_scale(300, 200, 1920, 1032), 1);
}

#[test]
fn a_resize_snaps_to_the_nearest_whole_scale_that_fits() {
    assert_eq!(snap_scale(2386, 1606, 3840, 2040), 2);    // 2.4x → 2x
    assert_eq!(snap_scale(2700, 1800, 3840, 2040), 3);    // 2.7x → 3x
    assert_eq!(snap_scale(3900, 2600, 3840, 2040), 3);    // bigger than the screen → largest that fits
    assert_eq!(snap_scale(994, 669, 1920, 1032), 1);
}
