// Turns mouse-wheel and drag input into "pan by this much" or "zoom by this
// factor". Kept apart from the window code so it can be tested without one.

use gpui::{MouseButton, Pixels, Point, ScrollDelta, point, px};

// One "line" of wheel scrolling, in GPUI's scaled units. A wheel notch is
// normally 3 lines.
pub const SCROLL_LINE: f32 = 40.0;
// Scrolling this far (in scaled units) with Ctrl held doubles or halves the zoom.
const SCROLL_PER_ZOOM_DOUBLING: f32 = 480.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScrollAction {
    // Move the image by this many physical screen pixels.
    Pan(Point<f32>),
    // Multiply the zoom by this, keeping the point under the pointer still.
    Zoom(f32),
}

// Mouse wheels report whole or fractional "lines"; precision touchpads may
// report exact pixel distances instead. Positive y means scrolling up (wheel
// turned away from the user). Shift+wheel already arrives as sideways (x)
// scrolling from GPUI.
pub fn scroll_action(delta: ScrollDelta, ctrl: bool, scale: f32) -> ScrollAction {
    let delta = delta.pixel_delta(px(SCROLL_LINE));
    let delta = point(f32::from(delta.x), f32::from(delta.y));
    if ctrl {
        ScrollAction::Zoom(2f32.powf(delta.y / SCROLL_PER_ZOOM_DOUBLING))
    } else {
        ScrollAction::Pan(point(delta.x * scale, delta.y * scale))
    }
}

// Tracks a pan-by-dragging gesture: middle button, or left button with Space held.
#[derive(Clone, Copy, Debug, Default)]
pub struct PanDrag {
    // Where the pointer was at the last move, while a drag is active.
    last: Option<Point<Pixels>>,
}

impl PanDrag {
    pub fn is_active(&self) -> bool {
        self.last.is_some()
    }

    // Starts a drag if this button should pan. Returns whether it did.
    pub fn start(
        &mut self,
        button: MouseButton,
        space_held: bool,
        position: Point<Pixels>,
    ) -> bool {
        let pans = match button {
            MouseButton::Middle => true,
            MouseButton::Left => space_held,
            _ => false,
        };
        if pans {
            self.last = Some(position);
        }
        pans
    }

    // How far to move the image (physical screen pixels) since the last move.
    // `None` when no drag is active, including the stray move Windows often
    // sends just after the button is released.
    pub fn move_to(&mut self, position: Point<Pixels>, scale: f32) -> Option<Point<f32>> {
        let last = self.last?;
        self.last = Some(position);
        Some(point(
            f32::from(position.x - last.x) * scale,
            f32::from(position.y - last.y) * scale,
        ))
    }

    pub fn end(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.001,
            "expected {expected}, got {actual}"
        );
    }

    fn zoom_factor(action: ScrollAction) -> f32 {
        match action {
            ScrollAction::Zoom(factor) => factor,
            other => panic!("expected a zoom, got {other:?}"),
        }
    }

    fn pan_delta(action: ScrollAction) -> Point<f32> {
        match action {
            ScrollAction::Pan(delta) => delta,
            other => panic!("expected a pan, got {other:?}"),
        }
    }

    // One wheel notch, as Windows reports it with the default "3 lines" setting.
    const NOTCH: f32 = 3.0;

    fn lines(x: f32, y: f32) -> ScrollDelta {
        ScrollDelta::Lines(point(x, y))
    }

    fn pixels(x: f32, y: f32) -> ScrollDelta {
        ScrollDelta::Pixels(point(px(x), px(y)))
    }

    #[test]
    fn plain_scroll_pans_in_physical_pixels() {
        // Wheel towards the user at 125% display scaling: the image moves up.
        let delta = pan_delta(scroll_action(lines(0.0, -NOTCH), false, 1.25));
        assert_close(delta.x, 0.0);
        assert_close(delta.y, -150.0);
    }

    #[test]
    fn sideways_scroll_pans_sideways() {
        let delta = pan_delta(scroll_action(lines(NOTCH, 0.0), false, 1.0));
        assert_close(delta.x, 120.0);
        assert_close(delta.y, 0.0);
    }

    #[test]
    fn ctrl_scroll_zooms_about_a_fifth_per_notch() {
        assert_close(
            zoom_factor(scroll_action(lines(0.0, NOTCH), true, 1.0)),
            1.1892,
        );
        assert_close(
            zoom_factor(scroll_action(lines(0.0, -NOTCH), true, 1.0)),
            1.0 / 1.1892,
        );
    }

    #[test]
    fn ctrl_scroll_zoom_does_not_depend_on_display_scaling() {
        let at_100 = zoom_factor(scroll_action(lines(0.0, NOTCH), true, 1.0));
        let at_150 = zoom_factor(scroll_action(lines(0.0, NOTCH), true, 1.5));
        assert_close(at_100, at_150);
    }

    #[test]
    fn ctrl_scroll_in_then_out_returns_to_the_same_zoom() {
        let zoom_in = zoom_factor(scroll_action(lines(0.0, NOTCH), true, 1.0));
        let zoom_out = zoom_factor(scroll_action(lines(0.0, -NOTCH), true, 1.0));
        assert_close(zoom_in * zoom_out, 1.0);
    }

    #[test]
    fn eight_notches_of_ctrl_scroll_double_the_zoom_twice() {
        let factor = zoom_factor(scroll_action(lines(0.0, 8.0 * NOTCH), true, 1.0));
        assert_close(factor, 4.0);
    }

    #[test]
    fn touchpad_pixel_scrolling_pans_by_that_many_pixels() {
        let delta = pan_delta(scroll_action(pixels(-7.5, 3.0), false, 1.25));
        assert_close(delta.x, -9.375);
        assert_close(delta.y, 3.75);
    }

    #[test]
    fn fractional_lines_from_precision_touchpads_pan_smoothly() {
        let delta = pan_delta(scroll_action(lines(0.0, 0.25), false, 1.0));
        assert_close(delta.y, 0.25 * SCROLL_LINE);
    }

    #[test]
    fn many_small_touchpad_zoom_steps_add_up_to_one_big_one() {
        // 30 small Ctrl+touchpad steps should zoom exactly as much as one step of
        // the same total size, so the zoom speed doesn't depend on how the
        // touchpad slices up the gesture.
        let small = zoom_factor(scroll_action(pixels(0.0, 4.0), true, 1.25));
        let big = zoom_factor(scroll_action(pixels(0.0, 120.0), true, 1.25));
        assert_close(small.powi(30), big);
    }

    #[test]
    fn an_empty_scroll_changes_nothing() {
        assert_eq!(
            scroll_action(lines(0.0, 0.0), false, 1.0),
            ScrollAction::Pan(point(0.0, 0.0))
        );
        assert_eq!(
            scroll_action(pixels(0.0, 0.0), true, 1.0),
            ScrollAction::Zoom(1.0)
        );
    }

    #[test]
    fn ctrl_zoom_ignores_sideways_touchpad_movement() {
        let factor = zoom_factor(scroll_action(pixels(50.0, 0.0), true, 1.0));
        assert_close(factor, 1.0);
    }

    #[test]
    fn middle_button_drag_pans() {
        let mut drag = PanDrag::default();
        assert!(drag.start(MouseButton::Middle, false, point(px(100.), px(100.))));
        let delta = drag.move_to(point(px(80.), px(130.)), 1.25).unwrap();
        assert_close(delta.x, -25.0);
        assert_close(delta.y, 37.5);
    }

    #[test]
    fn left_drag_only_pans_while_space_is_held() {
        let mut drag = PanDrag::default();
        assert!(!drag.start(MouseButton::Left, false, point(px(0.), px(0.))));
        assert!(!drag.is_active());
        assert!(drag.start(MouseButton::Left, true, point(px(0.), px(0.))));
        assert!(drag.is_active());
    }

    #[test]
    fn right_button_never_pans() {
        let mut drag = PanDrag::default();
        assert!(!drag.start(MouseButton::Right, true, point(px(0.), px(0.))));
    }

    #[test]
    fn moves_are_measured_from_the_previous_move() {
        let mut drag = PanDrag::default();
        drag.start(MouseButton::Middle, false, point(px(0.), px(0.)));
        drag.move_to(point(px(10.), px(0.)), 1.0);
        let delta = drag.move_to(point(px(15.), px(0.)), 1.0).unwrap();
        assert_close(delta.x, 5.0);
    }

    // The bug found during the GUI tests: a move that arrived just after the
    // button was released quietly restarted the drag.
    #[test]
    fn a_stray_move_after_release_does_not_restart_the_drag() {
        let mut drag = PanDrag::default();
        drag.start(MouseButton::Middle, false, point(px(0.), px(0.)));
        drag.end();
        assert_eq!(drag.move_to(point(px(50.), px(50.)), 1.0), None);
        assert!(!drag.is_active());
        assert_eq!(drag.move_to(point(px(60.), px(60.)), 1.0), None);
    }

    #[test]
    fn the_drag_keeps_following_the_pointer_outside_the_canvas() {
        // GPUI keeps sending moves after the pointer leaves the canvas (or the
        // window), with positions outside it, including negative ones.
        let mut drag = PanDrag::default();
        drag.start(MouseButton::Middle, false, point(px(10.), px(10.)));
        let delta = drag.move_to(point(px(-40.), px(-25.)), 2.0).unwrap();
        assert_close(delta.x, -100.0);
        assert_close(delta.y, -70.0);
        let delta = drag.move_to(point(px(5000.), px(-25.)), 1.0).unwrap();
        assert_close(delta.x, 5040.0);
    }

    #[test]
    fn releasing_outside_the_canvas_ends_the_drag() {
        let mut drag = PanDrag::default();
        drag.start(MouseButton::Middle, false, point(px(10.), px(10.)));
        drag.move_to(point(px(-300.), px(900.)), 1.0);
        // The release is handled wherever the pointer is.
        drag.end();
        assert!(!drag.is_active());
        assert_eq!(drag.move_to(point(px(10.), px(10.)), 1.0), None);
    }

    #[test]
    fn a_new_press_during_a_drag_restarts_from_the_new_position() {
        let mut drag = PanDrag::default();
        drag.start(MouseButton::Middle, false, point(px(0.), px(0.)));
        drag.start(MouseButton::Middle, false, point(px(100.), px(100.)));
        let delta = drag.move_to(point(px(101.), px(100.)), 1.0).unwrap();
        assert_close(delta.x, 1.0);
    }

    #[test]
    fn a_press_that_does_not_pan_leaves_a_running_drag_alone() {
        let mut drag = PanDrag::default();
        drag.start(MouseButton::Middle, false, point(px(0.), px(0.)));
        drag.start(MouseButton::Right, false, point(px(500.), px(500.)));
        let delta = drag.move_to(point(px(1.), px(0.)), 1.0).unwrap();
        assert_close(delta.x, 1.0);
    }

    #[test]
    fn moving_without_a_drag_does_nothing() {
        let mut drag = PanDrag::default();
        assert_eq!(drag.move_to(point(px(5.), px(5.)), 1.0), None);
    }
}
