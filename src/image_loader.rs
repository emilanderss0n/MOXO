// Reads an image file from disk and turns it into something GPUI can draw.

use std::io::ErrorKind;
use std::path::Path;
use std::sync::Arc;

use gpui::RenderImage;
use image::{DynamicImage, Frame, ImageDecoder, ImageError, ImageFormat, ImageReader};

// GPUI can't upload an image larger than this on either side to the GPU.
const MAX_SIDE: u32 = 16384;

pub struct LoadedImage {
    pub image: Arc<RenderImage>,
    pub width: u32,
    pub height: u32,
}

// Errors are returned as plain text because they go straight into the UI.
pub fn load(path: &Path) -> Result<LoadedImage, String> {
    // Work out the format from the file's contents, not its extension.
    let mut reader = ImageReader::open(path)
        .and_then(|reader| reader.with_guessed_format())
        .map_err(|error| format!("the file couldn't be read ({error})"))?;

    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg)) {
        return Err("it isn't a PNG or JPEG file".into());
    }

    // The image crate refuses images over 512 MB by default. We do our own size check below.
    reader.no_limits();
    let mut decoder = reader.into_decoder().map_err(describe)?;

    let (width, height) = decoder.dimensions();
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(format!(
            "it's {width} × {height} px, and Moxo can only show images up to {MAX_SIDE} px on each side"
        ));
    }

    // Phone photos often store "rotate this" in the file instead of rotating the pixels.
    let orientation = decoder.orientation().map_err(describe)?;
    let mut decoded = DynamicImage::from_decoder(decoder).map_err(describe)?;
    decoded.apply_orientation(orientation);

    let mut pixels = decoded.into_rgba8();
    let (width, height) = pixels.dimensions();

    // GPUI expects pixels in BGRA order; the decoder gives RGBA.
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    Ok(LoadedImage {
        image: Arc::new(RenderImage::new([Frame::new(pixels)])),
        width,
        height,
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
