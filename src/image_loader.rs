// Reads an image file from disk into editable RGBA pixels, and notes what the
// file holds that Moxo can't keep.

use std::io::ErrorKind;
use std::path::Path;

use image::{DynamicImage, ImageDecoder, ImageError, ImageReader, RgbaImage};

use crate::APP_NAME;
use crate::document::SourceDetails;
use crate::save::FileFormat;

// GPUI can't upload an image larger than this on either side to the GPU.
const MAX_SIDE: u32 = 16384;

pub struct LoadedImage {
    // 8 bits per channel, red-green-blue-alpha, upright.
    pub pixels: RgbaImage,
    pub format: FileFormat,
    pub source: SourceDetails,
}

// Errors are returned as plain text because they go straight into the UI.
pub fn load(path: &Path) -> Result<LoadedImage, String> {
    // Work out the format from the file's contents, not its extension.
    let mut reader = ImageReader::open(path)
        .and_then(|reader| reader.with_guessed_format())
        .map_err(|error| format!("the file couldn't be read ({error})"))?;

    let Some(format) = reader.format().and_then(FileFormat::from_image_format) else {
        return Err("it isn't a PNG or JPEG file".into());
    };

    // The image crate refuses images over 512 MB by default. We do our own size check below.
    reader.no_limits();
    let mut decoder = reader.into_decoder().map_err(describe)?;

    let (width, height) = decoder.dimensions();
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(format!(
            "it's {width} × {height} px, and {APP_NAME} can only show images up to {MAX_SIDE} px on each side"
        ));
    }

    // Record what saving over this file would lose: Moxo edits in 8 bits per
    // channel and doesn't write colour profiles.
    let colour_type = decoder.color_type();
    let source = SourceDetails {
        high_bit_depth: colour_type.bytes_per_pixel() > colour_type.channel_count(),
        colour_profile: decoder.icc_profile().map_err(describe)?.is_some(),
    };

    // Phone photos often store "rotate this" in the file instead of rotating the pixels.
    let orientation = decoder.orientation().map_err(describe)?;
    let mut decoded = DynamicImage::from_decoder(decoder).map_err(describe)?;
    decoded.apply_orientation(orientation);

    Ok(LoadedImage {
        pixels: decoded.into_rgba8(),
        format,
        source,
    })
}

// Turns the decoder's technical errors into something a person can act on.
fn describe(error: ImageError) -> String {
    match error {
        ImageError::Decoding(_) => "the file is damaged or isn't really a PNG or JPEG".into(),
        ImageError::IoError(error) if error.kind() == ErrorKind::UnexpectedEof => {
            "the file is incomplete or damaged".into()
        }
        other => other.to_string().trim_end_matches('.').to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, ImageFormat, Luma, Rgb, RgbImage, Rgba, RgbaImage};
    use std::fs;
    use std::path::PathBuf;

    // A file in a temporary folder that is deleted when the test ends. `Drop`
    // runs automatically when the value goes out of scope, a bit like PHP's
    // `__destruct`.
    struct TempFile(PathBuf);

    impl TempFile {
        fn new(name: &str) -> Self {
            let folder = std::env::temp_dir()
                .join(format!("moxo-image-loader-tests-{}", std::process::id()));
            fs::create_dir_all(&folder).unwrap();
            Self(folder.join(name))
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    // The loaded pixel at (x, y): red, green, blue, alpha.
    fn rgba_at(loaded: &LoadedImage, x: u32, y: u32) -> [u8; 4] {
        loaded.pixels.get_pixel(x, y).0
    }

    // JPEG is lossy, so its colours come back slightly different.
    fn assert_colour_close(actual: [u8; 4], expected: [u8; 4]) {
        for (a, e) in actual.into_iter().zip(expected) {
            assert!(
                a.abs_diff(e) <= 10,
                "expected about {expected:?}, got {actual:?}"
            );
        }
    }

    fn load_error(path: &Path) -> String {
        match load(path) {
            Ok(loaded) => panic!(
                "expected an error, got a {}x{} image",
                loaded.pixels.width(),
                loaded.pixels.height()
            ),
            Err(message) => message,
        }
    }

    #[test]
    fn a_png_loads_with_its_size_and_exact_pixels() {
        let file = TempFile::new("colours.png");
        let mut png = RgbaImage::new(3, 2);
        png.put_pixel(0, 0, Rgba([10, 20, 30, 255]));
        png.put_pixel(2, 1, Rgba([200, 150, 100, 255]));
        png.save(&file.0).unwrap();

        let loaded = load(&file.0).unwrap();
        assert_eq!(loaded.pixels.dimensions(), (3, 2));
        assert_eq!(rgba_at(&loaded, 0, 0), [10, 20, 30, 255]);
        assert_eq!(rgba_at(&loaded, 2, 1), [200, 150, 100, 255]);
    }

    #[test]
    fn png_transparency_is_kept() {
        let file = TempFile::new("transparent.png");
        let mut png = RgbaImage::new(2, 1);
        png.put_pixel(0, 0, Rgba([255, 0, 0, 0]));
        png.put_pixel(1, 0, Rgba([0, 255, 0, 128]));
        png.save(&file.0).unwrap();

        let loaded = load(&file.0).unwrap();
        assert_eq!(rgba_at(&loaded, 0, 0)[3], 0);
        assert_eq!(rgba_at(&loaded, 1, 0), [0, 255, 0, 128]);
    }

    #[test]
    fn a_jpeg_loads_with_its_size_and_colours() {
        let file = TempFile::new("photo.jpg");
        RgbImage::from_pixel(40, 30, Rgb([200, 100, 50]))
            .save(&file.0)
            .unwrap();

        let loaded = load(&file.0).unwrap();
        assert_eq!(loaded.pixels.dimensions(), (40, 30));
        // JPEGs have no transparency, so alpha is always fully opaque.
        assert_colour_close(rgba_at(&loaded, 20, 15), [200, 100, 50, 255]);
    }

    #[test]
    fn the_format_comes_from_the_contents_not_the_file_name() {
        let file = TempFile::new("really-a-png.jpg");
        RgbaImage::from_pixel(4, 4, Rgba([1, 2, 3, 255]))
            .save_with_format(&file.0, ImageFormat::Png)
            .unwrap();

        let loaded = load(&file.0).unwrap();
        // Exact colours prove it was read as a lossless PNG, not a JPEG.
        assert_eq!(rgba_at(&loaded, 0, 0), [1, 2, 3, 255]);
    }

    #[test]
    fn a_greyscale_png_becomes_grey_pixels() {
        let file = TempFile::new("grey.png");
        ImageBuffer::<Luma<u8>, _>::from_pixel(2, 2, Luma([77]))
            .save(&file.0)
            .unwrap();

        let loaded = load(&file.0).unwrap();
        assert_eq!(rgba_at(&loaded, 1, 1), [77, 77, 77, 255]);
    }

    #[test]
    fn a_16_bit_png_is_reduced_to_8_bits_per_channel() {
        let file = TempFile::new("deep.png");
        ImageBuffer::<Rgba<u16>, _>::from_pixel(2, 2, Rgba([65535, 32896, 0, 65535]))
            .save(&file.0)
            .unwrap();

        let loaded = load(&file.0).unwrap();
        assert_eq!(rgba_at(&loaded, 0, 0), [255, 128, 0, 255]);
    }

    // Builds a JPEG whose EXIF data says "rotate 90° clockwise to display",
    // which is how phones store portrait photos.
    fn jpeg_with_rotation_tag(pixels: &RgbImage) -> Vec<u8> {
        let mut jpeg = Vec::new();
        pixels
            .write_to(&mut std::io::Cursor::new(&mut jpeg), ImageFormat::Jpeg)
            .unwrap();

        #[rustfmt::skip]
        let exif: [u8; 36] = [
            0xFF, 0xE1, 0x00, 0x22,             // APP1 block, 34 bytes long
            b'E', b'x', b'i', b'f', 0, 0,       // "Exif" marker
            b'I', b'I', 0x2A, 0x00,             // little-endian TIFF header
            0x08, 0x00, 0x00, 0x00,             // first tag list starts at byte 8
            0x01, 0x00,                         // one tag
            0x12, 0x01, 0x03, 0x00,             // tag 0x0112 (orientation), a number
            0x01, 0x00, 0x00, 0x00,             // one value
            0x06, 0x00, 0x00, 0x00,             // value 6: rotate 90° clockwise
            0x00, 0x00, 0x00, 0x00,             // no more tag lists
        ];
        // Insert it straight after the 2-byte start-of-image marker.
        jpeg.splice(2..2, exif);
        jpeg
    }

    #[test]
    fn a_phone_photo_rotation_tag_is_applied() {
        // 32 wide, 16 tall: left half red, right half blue.
        let pixels = RgbImage::from_fn(32, 16, |x, _| {
            if x < 16 {
                Rgb([255, 0, 0])
            } else {
                Rgb([0, 0, 255])
            }
        });
        let file = TempFile::new("rotated.jpg");
        fs::write(&file.0, jpeg_with_rotation_tag(&pixels)).unwrap();

        let loaded = load(&file.0).unwrap();
        // Turned a quarter clockwise: now 16 wide and 32 tall, with the red
        // (formerly left) half on top and the blue half below.
        assert_eq!(loaded.pixels.dimensions(), (16, 32));
        assert_colour_close(rgba_at(&loaded, 8, 4), [255, 0, 0, 255]);
        assert_colour_close(rgba_at(&loaded, 8, 27), [0, 0, 255, 255]);
    }

    #[test]
    fn images_up_to_the_size_limit_load() {
        let file = TempFile::new("widest.png");
        RgbaImage::new(MAX_SIDE, 1).save(&file.0).unwrap();
        assert_eq!(load(&file.0).unwrap().pixels.width(), MAX_SIDE);
    }

    #[test]
    fn images_over_the_size_limit_are_rejected_with_their_size() {
        let wide = TempFile::new("too-wide.png");
        RgbaImage::new(MAX_SIDE + 1, 1).save(&wide.0).unwrap();
        assert_eq!(
            load_error(&wide.0),
            format!(
                "it's 16385 × 1 px, and {APP_NAME} can only show images up to 16384 px on each side"
            )
        );

        let tall = TempFile::new("too-tall.png");
        RgbaImage::new(1, MAX_SIDE + 1).save(&tall.0).unwrap();
        assert!(load_error(&tall.0).starts_with("it's 1 × 16385 px"));
    }

    #[test]
    fn a_text_file_named_png_is_reported_as_not_an_image() {
        let file = TempFile::new("not-an-image.png");
        fs::write(&file.0, "this is not really an image").unwrap();
        assert_eq!(
            load_error(&file.0),
            "the file is damaged or isn't really a PNG or JPEG"
        );
    }

    #[test]
    fn a_cut_off_png_is_reported_as_incomplete() {
        let complete = TempFile::new("complete.png");
        RgbaImage::from_fn(64, 64, |x, y| {
            Rgba([x as u8 * 4, y as u8 * 4, (x ^ y) as u8, 255])
        })
        .save(&complete.0)
        .unwrap();
        let bytes = fs::read(&complete.0).unwrap();

        let cut_off = TempFile::new("cut-off.png");
        fs::write(&cut_off.0, &bytes[..bytes.len() / 2]).unwrap();
        assert_eq!(load_error(&cut_off.0), "the file is incomplete or damaged");
    }

    #[test]
    fn other_image_formats_are_rejected() {
        // The first bytes of a GIF file, which is all format detection looks at.
        let file = TempFile::new("animation.gif");
        fs::write(&file.0, b"GIF89a\x01\x00\x01\x00\x00\x00\x00;").unwrap();
        assert_eq!(load_error(&file.0), "it isn't a PNG or JPEG file");
    }

    #[test]
    fn a_missing_file_is_reported() {
        let file = TempFile::new("does-not-exist.png");
        let message = load_error(&file.0);
        // The rest of the message comes from Windows, in the system language.
        assert!(
            message.starts_with("the file couldn't be read ("),
            "{message}"
        );
    }

    #[test]
    fn error_messages_never_end_with_a_full_stop() {
        // The window adds its own full stop after "Couldn't open <file>: <reason>".
        let file = TempFile::new("does-not-exist-either.png");
        assert!(!load_error(&file.0).ends_with('.'));
    }

    #[test]
    fn the_format_is_recorded_for_saving() {
        let png = TempFile::new("format.png");
        RgbaImage::new(2, 2).save(&png.0).unwrap();
        assert_eq!(load(&png.0).unwrap().format, FileFormat::Png);

        let jpeg = TempFile::new("format.jpg");
        RgbImage::new(2, 2).save(&jpeg.0).unwrap();
        assert_eq!(load(&jpeg.0).unwrap().format, FileFormat::Jpeg);

        // The contents decide, not the name.
        let disguised = TempFile::new("disguised.jpg");
        RgbaImage::new(2, 2)
            .save_with_format(&disguised.0, ImageFormat::Png)
            .unwrap();
        assert_eq!(load(&disguised.0).unwrap().format, FileFormat::Png);
    }

    #[test]
    fn an_ordinary_8_bit_image_has_nothing_that_saving_would_lose() {
        let file = TempFile::new("plain.png");
        RgbaImage::new(2, 2).save(&file.0).unwrap();
        assert_eq!(load(&file.0).unwrap().source, SourceDetails::default());
    }

    #[test]
    fn a_16_bit_png_is_flagged() {
        let file = TempFile::new("sixteen.png");
        ImageBuffer::<Rgba<u16>, _>::new(2, 2)
            .save(&file.0)
            .unwrap();
        let source = load(&file.0).unwrap().source;
        assert!(source.high_bit_depth);
        assert!(!source.colour_profile);
    }

    #[test]
    fn an_embedded_colour_profile_is_flagged() {
        use image::ImageEncoder;
        use image::codecs::png::PngEncoder;

        let mut bytes = Vec::new();
        let mut encoder = PngEncoder::new(&mut bytes);
        // Moxo only checks whether a profile is present, not what's in it.
        encoder.set_icc_profile(vec![0; 128]).unwrap();
        encoder
            .write_image(&[0; 16], 2, 2, image::ExtendedColorType::Rgba8)
            .unwrap();
        let file = TempFile::new("profile.png");
        fs::write(&file.0, bytes).unwrap();

        let source = load(&file.0).unwrap().source;
        assert!(source.colour_profile);
        assert!(!source.high_bit_depth);
    }
}
