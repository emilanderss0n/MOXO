// Where the image sits on the canvas: zoom level, panning, and fit-to-window.
//
// Everything here is measured in physical screen pixels, so a zoom of 1.0 (100%)
// means one image pixel per screen pixel, whatever Windows' display scaling is.
// The caller converts to and from GPUI's scaled units.

use gpui::{Point, Size, point};

pub const MIN_ZOOM: f32 = 0.01;
pub const MAX_ZOOM: f32 = 32.0;

// The zoom levels Ctrl+= and Ctrl+- step through, the same as Photoshop's.
const ZOOM_STEPS: [f32; 25] = [
    0.01, 0.02, 0.03, 0.04, 0.05, 0.0625, 0.0833, 0.125, 0.1667, 0.25, 0.3333, 0.5, 0.6667, 1.0,
    2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 12.0, 16.0, 24.0, 32.0,
];

// Empty space kept around the image when fitting it to the canvas.
const FIT_MARGIN: f32 = 32.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    // Fit the whole image, but never above 100%. Used when an image is opened.
    FitNoEnlarge,
    // Fit the whole image, enlarging small images too (Ctrl+0).
    Fit,
    // The user has zoomed or panned. `center` is the image point shown in the
    // middle of the canvas.
    Manual { zoom: f32, center: Point<f32> },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    image: Size<f32>,
    mode: Mode,
}

impl Viewport {
    pub fn new(image_width: u32, image_height: u32) -> Self {
        Self {
            image: Size {
                width: image_width as f32,
                height: image_height as f32,
            },
            mode: Mode::FitNoEnlarge,
        }
    }

    #[cfg(test)]
    fn is_fit(&self) -> bool {
        !matches!(self.mode, Mode::Manual { .. })
    }

    // The current zoom for a canvas of `area` screen pixels.
    pub fn zoom(&self, area: Size<f32>) -> f32 {
        match self.mode {
            Mode::FitNoEnlarge => self.fit_zoom(area).min(1.0),
            Mode::Fit => self.fit_zoom(area),
            Mode::Manual { zoom, .. } => zoom,
        }
    }

    // The image point shown in the middle of the canvas.
    pub fn center(&self, area: Size<f32>) -> Point<f32> {
        match self.mode {
            Mode::Manual { zoom, center } => self.clamp_center(area, zoom, center),
            _ => self.image_middle(),
        }
    }

    // Where the image is drawn, relative to the canvas's top-left corner:
    // (top-left corner, size), both in screen pixels.
    pub fn image_rect(&self, area: Size<f32>) -> (Point<f32>, Size<f32>) {
        let zoom = self.zoom(area);
        let center = self.center(area);
        let origin = point(
            area.width / 2.0 - center.x * zoom,
            area.height / 2.0 - center.y * zoom,
        );
        let size = Size {
            width: self.image.width * zoom,
            height: self.image.height * zoom,
        };
        (origin, size)
    }

    pub fn fit(&mut self) {
        self.mode = Mode::Fit;
    }

    pub fn actual_size(&mut self, area: Size<f32>) {
        self.zoom_at(area, canvas_middle(area), 1.0);
    }

    pub fn zoom_in(&mut self, area: Size<f32>) {
        let current = self.zoom(area);
        let next = ZOOM_STEPS
            .into_iter()
            .find(|&step| step > current * 1.001)
            .unwrap_or(MAX_ZOOM);
        self.zoom_at(area, canvas_middle(area), next);
    }

    pub fn zoom_out(&mut self, area: Size<f32>) {
        let current = self.zoom(area);
        let next = ZOOM_STEPS
            .into_iter()
            .rev()
            .find(|&step| step < current * 0.999)
            .unwrap_or(MIN_ZOOM);
        self.zoom_at(area, canvas_middle(area), next);
    }

    // Changes the zoom while keeping the image point under `anchor` (a position
    // on the canvas) in the same place, unless clamping has to move it.
    pub fn zoom_at(&mut self, area: Size<f32>, anchor: Point<f32>, new_zoom: f32) {
        if !new_zoom.is_finite() || !anchor.x.is_finite() || !anchor.y.is_finite() {
            return;
        }
        let zoom = self.zoom(area);
        let center = self.center(area);
        let new_zoom = new_zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        // Nothing would change (an empty touchpad step, or already at a limit):
        // leave the view, including fit mode, exactly as it is.
        if new_zoom == zoom {
            return;
        }

        // How far the anchor is from the canvas middle, in screen pixels.
        let offset = point(anchor.x - area.width / 2.0, anchor.y - area.height / 2.0);
        // The image point currently under the anchor...
        let under_anchor = point(center.x + offset.x / zoom, center.y + offset.y / zoom);
        // ...and the new middle that puts it back under the anchor.
        let new_center = point(
            under_anchor.x - offset.x / new_zoom,
            under_anchor.y - offset.y / new_zoom,
        );

        self.set_manual(area, new_zoom, new_center);
    }

    // Moves the image by `delta` screen pixels.
    pub fn pan_by(&mut self, area: Size<f32>, delta: Point<f32>) {
        if !delta.x.is_finite() || !delta.y.is_finite() {
            return;
        }
        let zoom = self.zoom(area);
        let center = self.center(area);
        let new_center = point(center.x - delta.x / zoom, center.y - delta.y / zoom);
        // If the image can't move (it already fits), stay in fit mode, so a stray
        // scroll doesn't stop the image from refitting when the window is resized.
        if self.clamp_center(area, zoom, new_center) == center {
            return;
        }
        self.set_manual(area, zoom, new_center);
    }

    fn set_manual(&mut self, area: Size<f32>, zoom: f32, center: Point<f32>) {
        self.mode = Mode::Manual {
            zoom,
            center: self.clamp_center(area, zoom, center),
        };
    }

    fn fit_zoom(&self, area: Size<f32>) -> f32 {
        let width = (area.width - 2.0 * FIT_MARGIN).max(1.0);
        let height = (area.height - 2.0 * FIT_MARGIN).max(1.0);
        (width / self.image.width)
            .min(height / self.image.height)
            .clamp(MIN_ZOOM, MAX_ZOOM)
    }

    // On each axis: if the zoomed image is smaller than the canvas, centre it.
    // Otherwise stop its edges from being dragged inside the canvas.
    fn clamp_center(&self, area: Size<f32>, zoom: f32, center: Point<f32>) -> Point<f32> {
        point(
            clamp_axis(center.x, self.image.width, area.width / zoom),
            clamp_axis(center.y, self.image.height, area.height / zoom),
        )
    }

    fn image_middle(&self) -> Point<f32> {
        point(self.image.width / 2.0, self.image.height / 2.0)
    }
}

// `visible` is how many image pixels fit across the canvas on this axis.
fn clamp_axis(center: f32, image: f32, visible: f32) -> f32 {
    if image <= visible {
        image / 2.0
    } else {
        center.clamp(visible / 2.0, image - visible / 2.0)
    }
}

fn canvas_middle(area: Size<f32>) -> Point<f32> {
    point(area.width / 2.0, area.height / 2.0)
}

// "100%", "66.7%", "3200%".
pub fn zoom_label(zoom: f32) -> String {
    let percent = zoom * 100.0;
    if (percent - percent.round()).abs() < 0.05 {
        format!("{percent:.0}%")
    } else {
        format!("{percent:.1}%")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(width: f32, height: f32) -> Size<f32> {
        Size { width, height }
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.01,
            "expected {expected}, got {actual}"
        );
    }

    // The screen position of an image point, given the current view.
    fn screen_position(
        viewport: &Viewport,
        area: Size<f32>,
        image_point: Point<f32>,
    ) -> Point<f32> {
        let (origin, _) = viewport.image_rect(area);
        let zoom = viewport.zoom(area);
        point(
            origin.x + image_point.x * zoom,
            origin.y + image_point.y * zoom,
        )
    }

    #[test]
    fn opening_fits_large_images_and_centres_them() {
        let canvas = area(1064.0, 664.0); // 1000 x 600 after the margins
        let viewport = Viewport::new(4000, 2000);
        assert_close(viewport.zoom(canvas), 0.25);
        let (origin, size) = viewport.image_rect(canvas);
        assert_close(size.width, 1000.0);
        assert_close(origin.x, 32.0);
        assert_close(origin.y, (664.0 - 500.0) / 2.0);
    }

    #[test]
    fn opening_never_enlarges_small_images() {
        let viewport = Viewport::new(120, 80);
        assert_close(viewport.zoom(area(1500.0, 900.0)), 1.0);
    }

    #[test]
    fn fit_enlarges_small_images() {
        let mut viewport = Viewport::new(120, 80);
        viewport.fit();
        let canvas = area(1264.0, 864.0); // 1200 x 800 after the margins
        assert_close(viewport.zoom(canvas), 10.0);
    }

    #[test]
    fn fit_handles_portrait_and_landscape() {
        let canvas = area(1064.0, 664.0);
        // Portrait: height is the limit.
        assert_close(Viewport::new(1000, 3000).zoom(canvas), 0.2);
        // Landscape: width is the limit.
        assert_close(Viewport::new(5000, 1000).zoom(canvas), 0.2);
    }

    #[test]
    fn fit_follows_the_canvas_size_until_the_user_navigates() {
        let mut viewport = Viewport::new(2000, 2000);
        assert_close(viewport.zoom(area(1064.0, 1064.0)), 0.5);
        assert_close(viewport.zoom(area(564.0, 564.0)), 0.25);

        viewport.zoom_in(area(564.0, 564.0));
        assert!(!viewport.is_fit());
        let zoom = viewport.zoom(area(564.0, 564.0));
        // Resizing no longer changes the zoom...
        assert_close(viewport.zoom(area(1064.0, 1064.0)), zoom);
        // ...until Fit on Screen is chosen again.
        viewport.fit();
        assert!(viewport.is_fit());
        assert_close(viewport.zoom(area(1064.0, 1064.0)), 0.5);
    }

    #[test]
    fn zoom_steps_go_up_and_down_through_the_presets() {
        let canvas = area(1000.0, 1000.0);
        let mut viewport = Viewport::new(100, 100);
        viewport.actual_size(canvas);
        viewport.zoom_in(canvas);
        assert_close(viewport.zoom(canvas), 2.0);
        viewport.zoom_out(canvas);
        viewport.zoom_out(canvas);
        assert_close(viewport.zoom(canvas), 0.6667);
    }

    #[test]
    fn zoom_stops_at_the_limits() {
        let canvas = area(1000.0, 1000.0);
        let mut viewport = Viewport::new(100, 100);
        for _ in 0..100 {
            viewport.zoom_in(canvas);
        }
        assert_close(viewport.zoom(canvas), MAX_ZOOM);
        for _ in 0..100 {
            viewport.zoom_out(canvas);
        }
        assert_close(viewport.zoom(canvas), MIN_ZOOM);
        viewport.zoom_at(canvas, point(0.0, 0.0), 1000.0);
        assert_close(viewport.zoom(canvas), MAX_ZOOM);
    }

    #[test]
    fn zooming_keeps_the_point_under_the_cursor_still() {
        let canvas = area(800.0, 600.0);
        let mut viewport = Viewport::new(4000, 3000);
        viewport.actual_size(canvas);

        let cursor = point(650.0, 120.0);
        let zoom = viewport.zoom(canvas);
        let (origin, _) = viewport.image_rect(canvas);
        let under_cursor = point((cursor.x - origin.x) / zoom, (cursor.y - origin.y) / zoom);

        for new_zoom in [2.0, 7.5, 0.6] {
            viewport.zoom_at(canvas, cursor, new_zoom);
            let after = screen_position(&viewport, canvas, under_cursor);
            assert_close(after.x, cursor.x);
            assert_close(after.y, cursor.y);
        }
    }

    #[test]
    fn zooming_near_an_edge_is_clamped_instead_of_showing_empty_space() {
        let canvas = area(800.0, 600.0);
        let mut viewport = Viewport::new(4000, 3000);
        viewport.actual_size(canvas);
        viewport.pan_by(canvas, point(10_000.0, 10_000.0)); // go to the top-left corner

        // Zooming out around the top-left corner of the canvas: the image's
        // top-left stays pinned to the canvas's top-left.
        viewport.zoom_at(canvas, point(0.0, 0.0), 0.5);
        let (origin, _) = viewport.image_rect(canvas);
        assert_close(origin.x, 0.0);
        assert_close(origin.y, 0.0);

        // Zooming out around the middle of the canvas while in the corner:
        // the point can't stay put because the edge would come into view.
        viewport.zoom_at(canvas, point(400.0, 300.0), 0.25);
        let (origin, _) = viewport.image_rect(canvas);
        assert_close(origin.x, 0.0);
        assert_close(origin.y, 0.0);
    }

    #[test]
    fn panning_stops_at_the_image_edges() {
        let canvas = area(800.0, 600.0);
        let mut viewport = Viewport::new(4000, 3000);
        viewport.actual_size(canvas);

        viewport.pan_by(canvas, point(-1_000_000.0, 0.0)); // far to the right
        let (origin, size) = viewport.image_rect(canvas);
        assert_close(origin.x + size.width, 800.0);

        viewport.pan_by(canvas, point(1_000_000.0, 0.0)); // far to the left
        let (origin, _) = viewport.image_rect(canvas);
        assert_close(origin.x, 0.0);
    }

    #[test]
    fn an_image_smaller_than_the_canvas_stays_centred_when_panned() {
        let canvas = area(800.0, 600.0);
        let mut viewport = Viewport::new(200, 100);
        viewport.actual_size(canvas);
        viewport.pan_by(canvas, point(300.0, -200.0));
        let (origin, _) = viewport.image_rect(canvas);
        assert_close(origin.x, 300.0);
        assert_close(origin.y, 250.0);
    }

    #[test]
    fn panning_a_fitted_image_that_cannot_move_keeps_fit_mode() {
        let mut viewport = Viewport::new(2000, 1000);
        viewport.pan_by(area(1064.0, 664.0), point(0.0, -120.0));
        assert!(viewport.is_fit());
    }

    #[test]
    fn a_wide_image_pans_sideways_but_stays_centred_vertically() {
        let canvas = area(800.0, 600.0);
        let mut viewport = Viewport::new(3000, 200);
        viewport.actual_size(canvas);
        viewport.pan_by(canvas, point(100.0, 100.0));
        let (origin, _) = viewport.image_rect(canvas);
        assert_close(origin.x, 800.0 / 2.0 - 1500.0 + 100.0);
        assert_close(origin.y, 200.0);
    }

    #[test]
    fn resizing_in_manual_mode_keeps_the_same_middle_point() {
        let mut viewport = Viewport::new(4000, 3000);
        viewport.zoom_at(area(800.0, 600.0), point(400.0, 300.0), 1.0);
        viewport.pan_by(area(800.0, 600.0), point(-500.0, -400.0));
        let before = viewport.center(area(800.0, 600.0));
        let after = viewport.center(area(1200.0, 900.0));
        assert_close(after.x, before.x);
        assert_close(after.y, before.y);
    }

    #[test]
    fn a_one_pixel_image_is_handled() {
        let mut viewport = Viewport::new(1, 1);
        let canvas = area(800.0, 600.0);
        assert_close(viewport.zoom(canvas), 1.0);
        viewport.fit();
        assert_close(viewport.zoom(canvas), MAX_ZOOM);
    }

    #[test]
    fn a_tiny_canvas_does_not_break_fitting() {
        let viewport = Viewport::new(16384, 16384);
        assert_close(viewport.zoom(area(10.0, 10.0)), MIN_ZOOM);
    }

    // A tiny deterministic random-number generator, so the tests below try
    // many different situations but give the same result on every run.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }

        fn between(&mut self, low: f32, high: f32) -> f32 {
            low + (high - low) * self.next()
        }
    }

    // The rules the view must always follow: zoom within limits, numbers
    // finite, and on each axis the image either covers the whole canvas or,
    // if it's smaller, sits exactly in the middle.
    fn assert_view_is_valid(viewport: &Viewport, canvas: Size<f32>) {
        let zoom = viewport.zoom(canvas);
        assert!(
            (MIN_ZOOM..=MAX_ZOOM).contains(&zoom),
            "zoom {zoom} out of range"
        );
        let (origin, size) = viewport.image_rect(canvas);
        for (start, length, available) in [
            (origin.x, size.width, canvas.width),
            (origin.y, size.height, canvas.height),
        ] {
            assert!(start.is_finite() && length.is_finite());
            if length <= available {
                let centred = (available - length) / 2.0;
                assert!(
                    (start - centred).abs() < 0.01,
                    "a smaller image should be centred: start {start}, length {length}, canvas {available}"
                );
            } else {
                assert!(start <= 0.01, "gap before the image: start {start}");
                assert!(
                    start + length >= available - 0.01,
                    "gap after the image: end {}, canvas {available}",
                    start + length
                );
            }
        }
    }

    #[test]
    fn zoom_anchoring_holds_for_any_pointer_position_and_zoom_change() {
        // A large image, zoomed in, so nothing hits an edge.
        let canvas = area(1250.0, 750.0);
        for anchor_x in [0.0, 1.0, 333.3, 625.0, 1249.0] {
            for anchor_y in [0.0, 200.0, 374.5, 749.0] {
                for (from, to) in [
                    (1.0, 1.5),
                    (2.0, 8.0),
                    (8.0, 3.0),
                    (32.0, 1.0),
                    (1.3, 1.3001),
                ] {
                    let mut viewport = Viewport::new(16000, 12000);
                    viewport.zoom_at(canvas, point(625.0, 375.0), from);
                    let anchor = point(anchor_x, anchor_y);
                    let (origin, _) = viewport.image_rect(canvas);
                    let under = point((anchor.x - origin.x) / from, (anchor.y - origin.y) / from);

                    viewport.zoom_at(canvas, anchor, to);
                    let after = screen_position(&viewport, canvas, under);
                    assert_close(after.x, anchor.x);
                    assert_close(after.y, anchor.y);
                }
            }
        }
    }

    #[test]
    fn any_sequence_of_navigation_keeps_the_view_valid() {
        let mut random = Random(42);
        let images = [
            (3000, 2000),
            (800, 1600),
            (120, 80),
            (1, 1),
            (16384, 1),
            (1, 16384),
            (16384, 16384),
        ];
        for (image_width, image_height) in images {
            let mut viewport = Viewport::new(image_width, image_height);
            for _ in 0..400 {
                let canvas = area(random.between(1.0, 3000.0), random.between(1.0, 2000.0));
                let anchor = point(
                    random.between(-100.0, canvas.width + 100.0),
                    random.between(-100.0, canvas.height + 100.0),
                );
                match (random.next() * 7.0) as u32 {
                    0 => {
                        let zoom = viewport.zoom(canvas) * random.between(0.2, 5.0);
                        viewport.zoom_at(canvas, anchor, zoom);
                    }
                    1 => {
                        let delta = point(
                            random.between(-5000.0, 5000.0),
                            random.between(-5000.0, 5000.0),
                        );
                        viewport.pan_by(canvas, delta);
                    }
                    2 => viewport.zoom_in(canvas),
                    3 => viewport.zoom_out(canvas),
                    4 => viewport.actual_size(canvas),
                    5 => viewport.fit(),
                    _ => {} // just resize the canvas
                }
                assert_view_is_valid(&viewport, canvas);
            }
        }
    }

    #[test]
    fn fit_mode_always_shows_the_whole_image_inside_the_margin() {
        let mut random = Random(7);
        for _ in 0..500 {
            let image_width = random.between(1.0, 16384.0) as u32;
            let image_height = random.between(1.0, 16384.0) as u32;
            let canvas = area(random.between(200.0, 4000.0), random.between(200.0, 3000.0));
            let mut viewport = Viewport::new(image_width, image_height);
            viewport.fit();
            let (origin, size) = viewport.image_rect(canvas);
            let zoom = viewport.zoom(canvas);
            // Unless a zoom limit gets in the way, the image fills the space
            // inside the margin on one axis and fits within it on the other.
            if zoom > MIN_ZOOM && zoom < MAX_ZOOM {
                let inner_width = canvas.width - 2.0 * FIT_MARGIN;
                let inner_height = canvas.height - 2.0 * FIT_MARGIN;
                assert!(origin.x >= FIT_MARGIN - 0.01 && origin.y >= FIT_MARGIN - 0.01);
                assert!(size.width <= inner_width + 0.01 && size.height <= inner_height + 0.01);
                let fills_width = (size.width - inner_width).abs() < 0.01;
                let fills_height = (size.height - inner_height).abs() < 0.01;
                assert!(fills_width || fills_height);
            }
            assert_view_is_valid(&viewport, canvas);
        }
    }

    #[test]
    fn extreme_shapes_fit() {
        let canvas = area(1064.0, 664.0);
        // One pixel tall: the width is the limit.
        assert_close(Viewport::new(16384, 1).zoom(canvas), 1000.0 / 16384.0);
        // One pixel wide: the height is the limit.
        assert_close(Viewport::new(1, 16384).zoom(canvas), 600.0 / 16384.0);
    }

    #[test]
    fn opening_a_square_image_that_exactly_fits_shows_it_at_100_percent() {
        let viewport = Viewport::new(1000, 1000);
        assert_close(viewport.zoom(area(1064.0, 1064.0)), 1.0);
    }

    #[test]
    fn opening_and_fit_differ_only_for_images_smaller_than_the_canvas() {
        let canvas = area(1064.0, 664.0);
        let mut large = Viewport::new(4000, 2000);
        large.fit();
        assert_close(Viewport::new(4000, 2000).zoom(canvas), large.zoom(canvas));
        let mut small = Viewport::new(100, 100);
        small.fit();
        assert!(small.zoom(canvas) > Viewport::new(100, 100).zoom(canvas));
    }

    #[test]
    fn zoom_steps_work_from_in_between_zoom_levels() {
        let canvas = area(1064.0, 664.0);
        let mut viewport = Viewport::new(3000, 2000); // fits at 30%
        viewport.zoom_in(canvas);
        assert_close(viewport.zoom(canvas), 0.3333);
        let mut viewport = Viewport::new(3000, 2000);
        viewport.zoom_out(canvas);
        assert_close(viewport.zoom(canvas), 0.25);
    }

    #[test]
    fn actual_size_keeps_the_middle_of_the_view_in_place() {
        let canvas = area(1000.0, 800.0);
        let mut viewport = Viewport::new(8000, 6000);
        viewport.zoom_at(canvas, point(500.0, 400.0), 0.5);
        viewport.pan_by(canvas, point(-700.0, 300.0));
        let before = viewport.center(canvas);
        viewport.actual_size(canvas);
        let after = viewport.center(canvas);
        assert_close(after.x, before.x);
        assert_close(after.y, before.y);
    }

    #[test]
    fn fit_after_navigating_centres_the_image_again() {
        let canvas = area(1064.0, 664.0);
        let mut viewport = Viewport::new(3000, 2000);
        viewport.zoom_at(canvas, point(10.0, 10.0), 4.0);
        viewport.pan_by(canvas, point(400.0, -250.0));
        viewport.fit();
        let center = viewport.center(canvas);
        assert_close(center.x, 1500.0);
        assert_close(center.y, 1000.0);
    }

    #[test]
    fn zooming_by_nothing_keeps_fit_mode() {
        let canvas = area(1064.0, 664.0);
        let mut viewport = Viewport::new(3000, 2000);
        let zoom = viewport.zoom(canvas);
        viewport.zoom_at(canvas, point(100.0, 100.0), zoom);
        assert!(viewport.is_fit());
    }

    #[test]
    fn zooming_past_the_limit_keeps_fit_mode() {
        // A one-pixel image fitted to the window is already at the maximum zoom.
        let canvas = area(800.0, 600.0);
        let mut viewport = Viewport::new(1, 1);
        viewport.fit();
        viewport.zoom_at(canvas, point(10.0, 10.0), MAX_ZOOM * 2.0);
        viewport.zoom_in(canvas);
        assert!(viewport.is_fit());
    }

    #[test]
    fn nonsense_input_is_ignored() {
        let canvas = area(800.0, 600.0);
        let mut viewport = Viewport::new(3000, 2000);
        viewport.actual_size(canvas);
        let before = viewport;
        viewport.zoom_at(canvas, point(10.0, 10.0), f32::NAN);
        viewport.zoom_at(canvas, point(10.0, 10.0), f32::INFINITY);
        viewport.zoom_at(canvas, point(f32::NAN, 10.0), 2.0);
        viewport.pan_by(canvas, point(f32::INFINITY, 0.0));
        viewport.pan_by(canvas, point(0.0, f32::NAN));
        assert_eq!(viewport, before);
    }

    #[test]
    fn an_empty_canvas_does_not_break_anything() {
        // A minimised window can report a zero-size canvas.
        let canvas = area(0.0, 0.0);
        let mut viewport = Viewport::new(3000, 2000);
        assert_view_is_valid(&viewport, canvas);
        viewport.zoom_in(canvas);
        viewport.pan_by(canvas, point(100.0, 100.0));
        viewport.zoom_at(canvas, point(0.0, 0.0), 3.0);
        assert_view_is_valid(&viewport, canvas);
    }

    #[test]
    fn zoom_labels_are_readable() {
        assert_eq!(zoom_label(1.0), "100%");
        assert_eq!(zoom_label(0.6667), "66.7%");
        assert_eq!(zoom_label(32.0), "3200%");
        assert_eq!(zoom_label(0.0833), "8.3%");
    }
}
