// Draws the open image, and the transparency checkerboard behind it.
//
// The geometry is in plain functions so it can be tested without a window;
// `paint` just calls them and hands the results to GPUI.

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    Bounds, ContentMask, Corners, Pixels, Point, RenderImage, Size, Window, point, px, size,
};
use image::{Frame, Rgba, RgbaImage};

use crate::viewport::Viewport;

// Size of one checkerboard square, in GPUI's scaled units. It stays the same on
// screen at every zoom level, like Photoshop's.
const CHECKER_SQUARE: f32 = 8.0;
const CHECKER_LIGHT: u8 = 0x3a;
const CHECKER_DARK: u8 = 0x2e;
// Squares per side of the generated tile, which is repeated to cover the image.
const SQUARES_PER_TILE: u32 = 32;

// The area between the bars at the top and bottom of a window of this size.
pub fn canvas_bounds(
    window_size: Size<Pixels>,
    top_bar: Pixels,
    bottom_bar: Pixels,
) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(0.), top_bar),
        size: size(
            window_size.width.max(px(0.)),
            (window_size.height - top_bar - bottom_bar).max(px(0.)),
        ),
    }
}

// The canvas size in physical screen pixels, which is what `Viewport` works in.
pub fn area_in_screen_pixels(bounds: Bounds<Pixels>, scale: f32) -> Size<f32> {
    Size {
        width: f32::from(bounds.size.width) * scale,
        height: f32::from(bounds.size.height) * scale,
    }
}

// A window position as physical screen pixels from the canvas's top-left corner.
pub fn position_on_canvas(
    position: Point<Pixels>,
    canvas_origin: Point<Pixels>,
    scale: f32,
) -> Point<f32> {
    point(
        f32::from(position.x - canvas_origin.x) * scale,
        f32::from(position.y - canvas_origin.y) * scale,
    )
}

// Where to draw the image, in GPUI's scaled units. The top-left corner is
// rounded to a whole physical screen pixel, so at 100% every image pixel lands
// exactly on one screen pixel instead of being smeared across two.
pub fn image_bounds(canvas: Bounds<Pixels>, viewport: &Viewport, scale: f32) -> Bounds<Pixels> {
    let (origin, image_size) = viewport.image_rect(area_in_screen_pixels(canvas, scale));
    let left = (f32::from(canvas.origin.x) * scale + origin.x).round();
    let top = (f32::from(canvas.origin.y) * scale + origin.y).round();
    Bounds {
        origin: point(px(left / scale), px(top / scale)),
        size: size(px(image_size.width / scale), px(image_size.height / scale)),
    }
}

// Which tiles, counted from `image_start`, are needed to cover the visible
// stretch from `visible_start` to `visible_end` on one axis.
fn tile_range(
    visible_start: Pixels,
    visible_end: Pixels,
    image_start: Pixels,
    tile: Pixels,
) -> Range<i32> {
    let first = (f32::from(visible_start - image_start) / f32::from(tile)).floor() as i32;
    let last = (f32::from(visible_end - image_start) / f32::from(tile)).ceil() as i32;
    first..last
}

// One checkerboard tile whose squares are whole screen pixels at this display
// scale, so they stay sharp.
fn checker_pixels(scale: f32) -> RgbaImage {
    let square = (CHECKER_SQUARE * scale).round().max(1.0) as u32;
    let side = square * SQUARES_PER_TILE;
    RgbaImage::from_fn(side, side, |x, y| {
        let shade = if (x / square + y / square) % 2 == 0 {
            CHECKER_LIGHT
        } else {
            CHECKER_DARK
        };
        // Grey is the same in RGBA and BGRA order, so no channel swap is needed.
        Rgba([shade, shade, shade, 255])
    })
}

pub fn checker_tile(scale: f32) -> Arc<RenderImage> {
    Arc::new(RenderImage::new([Frame::new(checker_pixels(scale))]))
}

// GPUI's copy of the document for drawing. GPUI wants blue-green-red-alpha
// order, so this swaps red and blue in a copy; the document keeps its RGBA.
pub fn display_image(pixels: &RgbaImage) -> Arc<RenderImage> {
    Arc::new(RenderImage::new([Frame::new(bgra_copy(pixels))]))
}

fn bgra_copy(pixels: &RgbaImage) -> RgbaImage {
    let mut copy = pixels.clone();
    for pixel in copy.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    copy
}

pub fn paint(
    canvas: Bounds<Pixels>,
    viewport: &Viewport,
    image: &Arc<RenderImage>,
    checker: &Arc<RenderImage>,
    window: &mut Window,
) {
    let image_bounds = image_bounds(canvas, viewport, window.scale_factor());

    // Nothing may be drawn outside the canvas (over the menu bar, for example).
    window.with_content_mask(Some(ContentMask { bounds: canvas }), |window| {
        paint_checkerboard(image_bounds, canvas, checker, window);
        // Painting only fails if the GPU can't hold the image; the loader already
        // rejects images that are too large, so there's nothing useful to do here.
        let _ = window.paint_image(image_bounds, Corners::default(), image.clone(), 0, false);
    });
}

// Repeats the checker tile across the visible part of the image. The pattern
// starts at the image's corner, so it moves with the image when panning.
fn paint_checkerboard(
    image_bounds: Bounds<Pixels>,
    canvas: Bounds<Pixels>,
    checker: &Arc<RenderImage>,
    window: &mut Window,
) {
    let visible = image_bounds.intersect(&canvas);
    if visible.size.width <= px(0.) || visible.size.height <= px(0.) {
        return;
    }

    let tile_pixels = checker.size(0);
    let scale = window.scale_factor();
    let tile = size(
        px(tile_pixels.width.0 as f32 / scale),
        px(tile_pixels.height.0 as f32 / scale),
    );
    let columns = tile_range(
        visible.left(),
        visible.right(),
        image_bounds.left(),
        tile.width,
    );
    let rows = tile_range(
        visible.top(),
        visible.bottom(),
        image_bounds.top(),
        tile.height,
    );

    // Clip the tiles to the image, so the pattern doesn't spill past its edges.
    window.with_content_mask(
        Some(ContentMask {
            bounds: image_bounds,
        }),
        |window| {
            for row in rows {
                for column in columns.clone() {
                    let tile_bounds = Bounds {
                        origin: point(
                            image_bounds.left() + tile.width * column as f32,
                            image_bounds.top() + tile.height * row as f32,
                        ),
                        size: tile,
                    };
                    let _ = window.paint_image(
                        tile_bounds,
                        Corners::default(),
                        checker.clone(),
                        0,
                        false,
                    );
                }
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // Common Windows display scaling settings.
    const SCALES: [f32; 6] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25];

    fn assert_close(actual: f32, expected: f32, tolerance: f32) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "expected {expected}, got {actual}"
        );
    }

    fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(width), px(height)),
        }
    }

    // A canvas in the same place as Moxo's: below a 32-unit menu bar.
    fn moxo_canvas(width: f32, height: f32) -> Bounds<Pixels> {
        bounds(0.0, 32.0, width, height)
    }

    #[test]
    fn canvas_sits_between_the_bars() {
        let canvas = canvas_bounds(size(px(1000.), px(700.)), px(32.), px(26.));
        assert_eq!(canvas, bounds(0.0, 32.0, 1000.0, 642.0));
    }

    #[test]
    fn a_window_shorter_than_the_bars_gives_an_empty_canvas() {
        let canvas = canvas_bounds(size(px(300.), px(40.)), px(32.), px(26.));
        assert_eq!(canvas.size.height, px(0.));
    }

    #[test]
    fn canvas_size_converts_to_physical_pixels() {
        let area = area_in_screen_pixels(moxo_canvas(1000.0, 600.0), 1.25);
        assert_close(area.width, 1250.0, 0.001);
        assert_close(area.height, 750.0, 0.001);
    }

    #[test]
    fn pointer_positions_convert_to_physical_pixels_from_the_canvas_corner() {
        let origin = point(px(0.), px(32.));
        let at_corner = position_on_canvas(point(px(0.), px(32.)), origin, 1.5);
        assert_close(at_corner.x, 0.0, 0.001);
        assert_close(at_corner.y, 0.0, 0.001);
        let inside = position_on_canvas(point(px(100.), px(132.)), origin, 1.5);
        assert_close(inside.x, 150.0, 0.001);
        assert_close(inside.y, 150.0, 0.001);
    }

    // The core DPI promise: at 100%, one image pixel is one physical screen
    // pixel, and the image starts exactly on a pixel boundary.
    #[test]
    fn at_100_percent_each_image_pixel_is_one_screen_pixel_at_every_scaling() {
        for scale in SCALES {
            for (width, height) in [(1000.0, 600.0), (1001.0, 603.0), (777.0, 555.0)] {
                for (image_width, image_height) in [(200, 200), (201, 99), (3000, 2000), (1, 1)] {
                    let canvas = moxo_canvas(width, height);
                    let mut viewport = Viewport::new(image_width, image_height);
                    viewport.actual_size(area_in_screen_pixels(canvas, scale));
                    let drawn = image_bounds(canvas, &viewport, scale);

                    let left = f32::from(drawn.origin.x) * scale;
                    let top = f32::from(drawn.origin.y) * scale;
                    assert_close(left, left.round(), 0.01);
                    assert_close(top, top.round(), 0.01);
                    assert_close(
                        f32::from(drawn.size.width) * scale,
                        image_width as f32,
                        0.01,
                    );
                    assert_close(
                        f32::from(drawn.size.height) * scale,
                        image_height as f32,
                        0.01,
                    );
                }
            }
        }
    }

    #[test]
    fn the_snapped_image_is_never_more_than_half_a_pixel_from_the_exact_spot() {
        for scale in SCALES {
            let canvas = moxo_canvas(1003.0, 611.0);
            let area = area_in_screen_pixels(canvas, scale);
            let mut viewport = Viewport::new(4000, 3000);
            viewport.zoom_at(area, point(123.4, 56.7), 0.3719);
            let (exact, _) = viewport.image_rect(area);
            let drawn = image_bounds(canvas, &viewport, scale);
            let drawn_left = f32::from(drawn.origin.x) * scale - f32::from(canvas.origin.x) * scale;
            let drawn_top = f32::from(drawn.origin.y) * scale - f32::from(canvas.origin.y) * scale;
            assert_close(drawn_left, exact.x, 0.5 + 0.001);
            assert_close(drawn_top, exact.y, 0.5 + 0.001);
        }
    }

    #[test]
    fn a_pointer_position_maps_back_to_the_right_image_pixel() {
        // Put the pointer over image pixel (1234, 567) and check the conversion
        // chain (window position -> canvas pixels -> image pixel) agrees.
        for scale in SCALES {
            let canvas = moxo_canvas(1000.0, 600.0);
            let area = area_in_screen_pixels(canvas, scale);
            let mut viewport = Viewport::new(4000, 3000);
            viewport.zoom_at(area, point(area.width / 2.0, area.height / 2.0), 2.0);
            let (origin, _) = viewport.image_rect(area);
            let zoom = viewport.zoom(area);

            let target = point(1234.5, 567.5); // the middle of that pixel
            let window_position = point(
                px((origin.x + target.x * zoom) / scale),
                px((origin.y + target.y * zoom) / scale + 32.0),
            );
            let on_canvas = position_on_canvas(window_position, canvas.origin, scale);
            let image_x = (on_canvas.x - origin.x) / zoom;
            let image_y = (on_canvas.y - origin.y) / zoom;
            assert_eq!((image_x.floor(), image_y.floor()), (1234.0, 567.0));
        }
    }

    #[test]
    fn checker_squares_are_whole_screen_pixels() {
        for (scale, square) in [(1.0, 8), (1.25, 10), (1.5, 12), (1.75, 14), (2.0, 16)] {
            let tile = checker_pixels(scale);
            assert_eq!(tile.width(), square * SQUARES_PER_TILE);
            // Same shade within a square, alternating between neighbours.
            assert_eq!(tile.get_pixel(0, 0), tile.get_pixel(square - 1, square - 1));
            assert_ne!(tile.get_pixel(0, 0), tile.get_pixel(square, 0));
            assert_ne!(tile.get_pixel(0, 0), tile.get_pixel(0, square));
            assert_eq!(tile.get_pixel(0, 0), tile.get_pixel(square, square));
        }
    }

    #[test]
    fn checker_tile_repeats_seamlessly() {
        // An even number of squares per tile means the last square of one tile
        // and the first of the next are different shades, so no seam shows.
        let tile = checker_pixels(1.0);
        let side = tile.width();
        assert_ne!(tile.get_pixel(side - 1, 0), tile.get_pixel(0, 0));
    }

    #[test]
    fn the_display_copy_is_bgra_and_leaves_the_document_alone() {
        let mut pixels = RgbaImage::new(2, 1);
        pixels.put_pixel(0, 0, Rgba([10, 20, 30, 255]));
        pixels.put_pixel(1, 0, Rgba([200, 150, 100, 0]));
        let original = pixels.clone();

        let copy = bgra_copy(&pixels);
        assert_eq!(copy.dimensions(), (2, 1));
        // Red and blue swap places; green and alpha (including full
        // transparency) stay put.
        assert_eq!(copy.get_pixel(0, 0).0, [30, 20, 10, 255]);
        assert_eq!(copy.get_pixel(1, 0).0, [100, 150, 200, 0]);
        assert_eq!(pixels, original);
    }

    #[test]
    fn checker_is_fully_opaque_grey() {
        let tile = checker_pixels(1.25);
        assert!(
            tile.pixels()
                .all(|p| p[0] == p[1] && p[1] == p[2] && p[3] == 255)
        );
    }

    #[test]
    fn checker_tiles_cover_the_visible_part_of_the_image() {
        let tile = px(256.);
        // Image starting off-screen to the left (panned), visible from 0 to 1000.
        for image_start in [-1000.0_f32, -256.0, -255.5, 0.0, 13.0] {
            let range = tile_range(px(image_start.max(0.0)), px(1000.), px(image_start), tile);
            let first_tile_start = image_start + range.start as f32 * 256.0;
            let last_tile_end = image_start + range.end as f32 * 256.0;
            assert!(
                first_tile_start <= image_start.max(0.0),
                "gap at the start for {image_start}"
            );
            assert!(last_tile_end >= 1000.0, "gap at the end for {image_start}");
            // And no more than one spare tile at each end.
            assert!(range.len() as f32 <= (1000.0 - image_start.max(0.0)) / 256.0 + 2.0);
        }
    }
}
