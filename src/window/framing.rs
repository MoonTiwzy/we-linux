//! The device editor's framing: which part of the wallpaper a device shows, and
//! how the editor's field follows a drag or the wheel without waiting for the
//! renderer.
//!
//! A device's look is the renderer's (`engine/src/lib.rs`, `Frame::matrix`): a
//! point of the wallpaper `w` lands at `zoom * w + shift` on the device, in
//! percent of the device from its middle, right and up. The field shows a
//! snapshot taken with the look divided by `k`, so the device's own picture is
//! the middle `1 / k` of it and the rest is what lies around. The last snapshot
//! is drawn moved and scaled by the difference between the look it was taken
//! with and the current one, until the next one arrives.

use crate::desktop::{self, Device};

/// How much more of the wallpaper the field shows than the device does, each way.
pub const WIDE: f32 = 2.0;
/// The zoom the editor allows, wheel and slider alike, in percent.
pub const ZOOM: (f32, f32) = (10.0, 500.0);
/// What the wheel reports for one notch (Slint gives a wheel's line as 60
/// pixels): one notch is 10 % more or less, a touchpad's pixels in proportion.
pub const NOTCH: f32 = 60.0;

/// A device's look in percent, as `Device` has it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Look {
    pub zoom: f32,
    pub shift_x: f32,
    pub shift_y: f32,
}

impl From<&Device> for Look {
    fn from(device: &Device) -> Self {
        Self { zoom: device.zoom, shift_x: device.shift_x, shift_y: device.shift_y }
    }
}

/// Where the snapshot goes in the field: its size as a multiple of the field's,
/// and how far its middle is from the field's middle, in the field's widths and
/// heights, right and down.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Placement {
    pub scale: f32,
    pub x: f32,
    pub y: f32,
}

/// The look to take the field's snapshot with.
pub fn view_of(device: Look, k: f32) -> Look {
    Look { zoom: device.zoom / k, shift_x: device.shift_x / k, shift_y: device.shift_y / k }
}

/// What the renderer's answer to a snapshot says: the device's look the snapshot
/// stands for, and its `k`. A renderer without `snapshot-look` answers nothing and
/// draws the device's own look: that is `asked`, with nothing around it.
pub fn drawn(reply: &serde_json::Value, asked: Look, k: f32) -> (Look, f32) {
    let part = |key: &str| reply.get(key).and_then(serde_json::Value::as_f64).map(|v| v as f32 * 100.0 * k);
    match (part("zoom"), part("shift_x"), part("shift_y")) {
        (Some(zoom), Some(shift_x), Some(shift_y)) if zoom > 0.0 => (Look { zoom, shift_x, shift_y }, k),
        _ => (asked, 1.0),
    }
}

/// Where a snapshot taken with `shot` goes while the device's look is `now`.
pub fn placement(shot: Look, now: Look, k: f32) -> Placement {
    let scale = now.zoom / shot.zoom;
    Placement {
        scale,
        x: (now.shift_x - scale * shot.shift_x) / 100.0 / k,
        y: -(now.shift_y - scale * shot.shift_y) / 100.0 / k,
    }
}

fn shift(percent: f32) -> f32 {
    percent.clamp(desktop::SHIFT_RANGE.0, desktop::SHIFT_RANGE.1)
}

/// The look after the wallpaper was dragged by `dx`, `dy` of the field (right, down)
/// from where it was at `from`.
pub fn drag(from: Look, dx: f32, dy: f32, k: f32) -> Look {
    Look {
        zoom: from.zoom,
        shift_x: shift(from.shift_x + dx * 100.0 * k),
        shift_y: shift(from.shift_y - dy * 100.0 * k),
    }
}

/// The look after the wheel turned by `delta` (up enlarges) over the point `at_x`,
/// `at_y` of the field (0 to 1, from the top left): that point stays where it is.
pub fn wheel(look: Look, at_x: f32, at_y: f32, delta: f32, k: f32) -> Look {
    // A zoom already outside the range (set elsewhere) is a limit of its own: a
    // turn never moves it the other way.
    let zoom = (look.zoom * 1.1f32.powf(delta / NOTCH)).clamp(ZOOM.0.min(look.zoom), ZOOM.1.max(look.zoom));
    let by = zoom / look.zoom;
    if by == 1.0 {
        // At a limit: nothing moves, not even by a rounding.
        return look;
    }
    let (ax, ay) = ((at_x - 0.5) * 100.0 * k, (0.5 - at_y) * 100.0 * k);
    Look {
        zoom,
        shift_x: shift(ax - (ax - look.shift_x) * by),
        shift_y: shift(ay - (ay - look.shift_y) * by),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(zoom: f32, shift_x: f32, shift_y: f32) -> Look {
        Look { zoom, shift_x, shift_y }
    }

    #[track_caller]
    fn near(a: Look, b: Look) {
        let close = |x: f32, y: f32| (x - y).abs() < 1e-3;
        assert!(
            close(a.zoom, b.zoom) && close(a.shift_x, b.shift_x) && close(a.shift_y, b.shift_y),
            "{a:?} != {b:?}"
        );
    }

    #[track_caller]
    fn near_placement(a: Placement, b: Placement) {
        let close = |x: f32, y: f32| (x - y).abs() < 1e-3;
        assert!(close(a.scale, b.scale) && close(a.x, b.x) && close(a.y, b.y), "{a:?} != {b:?}");
    }

    #[test]
    fn view_of_divides_by_k() {
        near(view_of(look(145.0, 30.0, -10.0), 2.0), look(72.5, 15.0, -5.0));
    }

    #[test]
    fn drawn_comes_back_to_the_device() {
        let reply = serde_json::json!({ "zoom": 0.725, "shift_x": 0.15, "shift_y": -0.05 });
        let (shot, k) = drawn(&reply, look(1.0, 2.0, 3.0), 2.0);
        near(shot, look(145.0, 30.0, -10.0));
        assert_eq!(k, 2.0);
    }

    #[test]
    fn drawn_without_the_renderer_feature_is_the_device_view() {
        let asked = look(145.0, 30.0, -10.0);
        assert_eq!(drawn(&serde_json::Value::Null, asked, 2.0), (asked, 1.0));
    }

    #[test]
    fn placement_of_the_same_look_is_identity() {
        let now = look(145.0, 30.0, -10.0);
        near_placement(placement(now, now, 2.0), Placement { scale: 1.0, x: 0.0, y: 0.0 });
    }

    #[test]
    fn placement_follows_zoom_and_shift() {
        near_placement(
            placement(look(100.0, 0.0, 0.0), look(200.0, 0.0, 0.0), 2.0),
            Placement { scale: 2.0, x: 0.0, y: 0.0 },
        );
        near_placement(
            placement(look(100.0, 0.0, 0.0), look(100.0, 50.0, 20.0), 2.0),
            Placement { scale: 1.0, x: 0.25, y: -0.1 },
        );
    }

    #[test]
    fn drag_moves_the_wallpaper() {
        near(drag(look(100.0, 0.0, 0.0), 0.25, 0.1, 2.0), look(100.0, 50.0, -20.0));
    }

    #[test]
    fn drag_is_per_axis() {
        near(drag(look(100.0, 0.0, 0.0), 0.25, 0.25, 1.0), look(100.0, 25.0, -25.0));
    }

    #[test]
    fn wheel_steps_by_the_notch() {
        let plain = look(100.0, 0.0, 0.0);
        near(wheel(plain, 0.5, 0.5, NOTCH, 2.0), look(110.0, 0.0, 0.0));
        near(wheel(plain, 0.5, 0.5, -NOTCH, 2.0), look(90.909, 0.0, 0.0));
        near(wheel(plain, 0.5, 0.5, NOTCH / 2.0, 2.0), look(104.881, 0.0, 0.0));
    }

    #[test]
    fn wheel_keeps_the_point_under_the_pointer() {
        near(wheel(look(100.0, 0.0, 0.0), 1.0, 0.5, NOTCH, 2.0), look(110.0, -10.0, 0.0));
    }

    #[test]
    fn wheel_outside_the_range_never_turns_the_other_way() {
        // Set above or below the editor's range elsewhere (the CLI allows 5 to 2000).
        let big = look(800.0, 0.0, 0.0);
        assert_eq!(wheel(big, 0.5, 0.5, NOTCH, 2.0), big, "up does not shrink it to the range");
        near(wheel(big, 0.5, 0.5, -NOTCH, 2.0), look(727.273, 0.0, 0.0));
        let small = look(7.0, 0.0, 0.0);
        assert_eq!(wheel(small, 0.5, 0.5, -NOTCH, 2.0), small, "down does not grow it to the range");
        near(wheel(small, 0.5, 0.5, NOTCH, 2.0), look(7.7, 0.0, 0.0));
    }

    #[test]
    fn wheel_at_the_limit_changes_nothing() {
        let top = look(500.0, 12.0, 3.0);
        assert_eq!(wheel(top, 0.9, 0.1, NOTCH, 2.0), top);
    }
}
