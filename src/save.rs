// Writing an image to disk as PNG or JPEG, without ever leaving a half-written
// file in place of the original.

use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, ImageFormat, RgbImage, RgbaImage};

// JPEG quality used for every save. The image crate's default (75) shows
// visible artefacts; 90 keeps photos clean at a reasonable size.
pub const JPEG_QUALITY: u8 = 90;

// The formats Moxo can open and save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileFormat {
    Png,
    Jpeg,
}

impl FileFormat {
    pub fn from_image_format(format: ImageFormat) -> Option<Self> {
        match format {
            ImageFormat::Png => Some(Self::Png),
            ImageFormat::Jpeg => Some(Self::Jpeg),
            _ => None,
        }
    }

    fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            _ => None,
        }
    }

    // The extension added to a name that has none.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }
}

// Decides the file and format to write for a name chosen in Save As. The
// extension always decides the format, so a file never gets an extension that
// doesn't match its contents. A name without an extension gets `fallback`'s.
pub fn resolve_save_path(
    chosen: &Path,
    fallback: FileFormat,
) -> Result<(PathBuf, FileFormat), String> {
    match chosen.extension().and_then(|extension| extension.to_str()) {
        None | Some("") => {
            let path = chosen.with_extension(fallback.extension());
            // The dialog only asked about replacing the name it returned, not
            // this one, so don't silently replace a different existing file.
            if path.exists() {
                return Err(format!(
                    "{} already exists. To replace it, choose it by name in Save As",
                    display_name(&path)
                ));
            }
            Ok((path, fallback))
        }
        Some(extension) => match FileFormat::from_extension(extension) {
            Some(format) => Ok((chosen.to_path_buf(), format)),
            None => Err(format!(
                "Moxo can only save PNG and JPEG files, so the name must end in .png, .jpg or .jpeg (not .{extension})"
            )),
        },
    }
}

// Encodes the image and writes it to `path`, replacing any existing file only
// once the new one is completely written.
pub fn save(pixels: &RgbaImage, path: &Path, format: FileFormat) -> Result<(), String> {
    let bytes = encode(pixels, format)?;
    write_replacing(path, &bytes)
}

pub fn encode(pixels: &RgbaImage, format: FileFormat) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let (width, height) = pixels.dimensions();
    let result = match format {
        FileFormat::Png => PngEncoder::new(&mut bytes).write_image(
            pixels.as_raw(),
            width,
            height,
            ExtendedColorType::Rgba8,
        ),
        FileFormat::Jpeg => JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY).write_image(
            flatten_onto_white(pixels).as_raw(),
            width,
            height,
            ExtendedColorType::Rgb8,
        ),
    };
    result.map_err(|error| format!("the image couldn't be encoded ({error})"))?;
    Ok(bytes)
}

// JPEG can't store transparency. Blend every pixel onto white, the way
// Photoshop does, instead of letting the alpha channel be dropped (which would
// reveal whatever colour hides under fully transparent pixels).
pub fn flatten_onto_white(pixels: &RgbaImage) -> RgbImage {
    RgbImage::from_fn(pixels.width(), pixels.height(), |x, y| {
        let [red, green, blue, alpha] = pixels.get_pixel(x, y).0;
        let alpha = u32::from(alpha);
        let blend =
            |channel: u8| ((u32::from(channel) * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
        image::Rgb([blend(red), blend(green), blend(blue)])
    })
}

// Writes `bytes` to a temporary file next to `path`, makes sure it reached the
// disk, then renames it over `path`. On Windows the rename replaces the old file
// in one step (std uses MoveFileExW with MOVEFILE_REPLACE_EXISTING), so the
// original is either fully replaced or left untouched. If anything fails, the
// temporary file is removed.
pub fn write_replacing(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = temporary_path_for(path);
    let result = write_new_file(&temporary, bytes).and_then(|()| fs::rename(&temporary, path));
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(describe_io_error(&error, path));
    }
    Ok(())
}

fn write_new_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    // `create_new` refuses to touch a file that already exists.
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

// A unique name in the same folder, so the final rename stays on one drive.
fn temporary_path_for(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!(
        ".{}.moxo-{}-{count}.tmp",
        display_name(path),
        std::process::id()
    );
    path.with_file_name(name)
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

// Windows error code for a file another program has locked.
const ERROR_SHARING_VIOLATION: i32 = 32;

// Plain-language reasons. Like the loader's, they have no full stop, because
// the window wraps them in "Couldn't save <file>: <reason>."
fn describe_io_error(error: &io::Error, path: &Path) -> String {
    if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION) {
        return "the file is open in another program".into();
    }
    match error.kind() {
        ErrorKind::NotFound => "the folder doesn't exist".into(),
        ErrorKind::PermissionDenied if path.is_dir() => {
            "a folder with that name already exists".into()
        }
        ErrorKind::PermissionDenied => {
            "Windows denied access (the file may be read-only, open in another program, or in a protected folder)"
                .into()
        }
        ErrorKind::IsADirectory => "a folder with that name already exists".into(),
        ErrorKind::StorageFull => "the disk is full".into(),
        _ => error.to_string().trim_end_matches('.').to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    // A private folder for one test, deleted afterwards (even if the test fails).
    struct TempFolder(PathBuf);

    impl TempFolder {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("moxo-save-tests-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }

        // Everything in the folder, so tests can check no temporary file was left.
        fn names(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(&self.0)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for TempFolder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    // 2 × 2 with exact colours and every kind of transparency.
    fn sample() -> RgbaImage {
        let mut pixels = RgbaImage::new(2, 2);
        pixels.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        pixels.put_pixel(1, 0, Rgba([0, 128, 255, 128]));
        pixels.put_pixel(0, 1, Rgba([10, 20, 30, 0]));
        pixels.put_pixel(1, 1, Rgba([200, 100, 50, 255]));
        pixels
    }

    const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    const JPEG_START: [u8; 2] = [0xFF, 0xD8];

    #[test]
    fn the_extension_decides_the_format() {
        for (name, format) in [
            ("a.png", FileFormat::Png),
            ("a.PNG", FileFormat::Png),
            ("a.jpg", FileFormat::Jpeg),
            ("a.JPEG", FileFormat::Jpeg),
            ("my.photo.jpeg", FileFormat::Jpeg),
        ] {
            let (path, chosen) = resolve_save_path(Path::new(name), FileFormat::Png).unwrap();
            assert_eq!(chosen, format, "{name}");
            assert_eq!(path, Path::new(name));
        }
    }

    #[test]
    fn a_name_without_an_extension_gets_the_current_format() {
        let folder = TempFolder::new("no-extension");
        let (path, format) = resolve_save_path(&folder.file("photo"), FileFormat::Jpeg).unwrap();
        assert_eq!(path, folder.file("photo.jpg"));
        assert_eq!(format, FileFormat::Jpeg);

        // "photo." counts as no extension too.
        let (path, _) = resolve_save_path(&folder.file("photo."), FileFormat::Png).unwrap();
        assert_eq!(path, folder.file("photo.png"));
    }

    #[test]
    fn an_added_extension_never_silently_replaces_another_file() {
        let folder = TempFolder::new("added-extension-exists");
        fs::write(folder.file("photo.png"), b"existing").unwrap();
        let error = resolve_save_path(&folder.file("photo"), FileFormat::Png).unwrap_err();
        assert!(error.starts_with("photo.png already exists"), "{error}");
        assert_eq!(fs::read(folder.file("photo.png")).unwrap(), b"existing");
    }

    #[test]
    fn unsupported_extensions_are_refused_clearly() {
        for name in ["a.gif", "a.webp", "notes.txt", "my.photo"] {
            let error = resolve_save_path(Path::new(name), FileFormat::Png).unwrap_err();
            assert!(
                error.starts_with("Moxo can only save PNG and JPEG files"),
                "{error}"
            );
            assert!(!error.ends_with('.'), "{error}");
        }
        let error = resolve_save_path(Path::new("a.gif"), FileFormat::Png).unwrap_err();
        assert!(error.ends_with("(not .gif)"), "{error}");
    }

    #[test]
    fn a_png_round_trip_is_pixel_identical_including_transparency() {
        let folder = TempFolder::new("png-round-trip");
        let path = folder.file("out.png");
        save(&sample(), &path, FileFormat::Png).unwrap();

        let bytes = fs::read(&path).unwrap();
        assert_eq!(bytes[..8], PNG_SIGNATURE);
        let reloaded = image::load_from_memory(&bytes).unwrap().into_rgba8();
        assert_eq!(reloaded, sample());
    }

    #[test]
    fn the_file_contents_always_match_the_extension() {
        let folder = TempFolder::new("contents-match");
        let png = folder.file("x.png");
        let jpeg = folder.file("x.jpg");
        let (png_path, png_format) = resolve_save_path(&png, FileFormat::Jpeg).unwrap();
        let (jpeg_path, jpeg_format) = resolve_save_path(&jpeg, FileFormat::Png).unwrap();
        save(&sample(), &png_path, png_format).unwrap();
        save(&sample(), &jpeg_path, jpeg_format).unwrap();
        assert_eq!(fs::read(&png).unwrap()[..8], PNG_SIGNATURE);
        assert_eq!(fs::read(&jpeg).unwrap()[..2], JPEG_START);
    }

    #[test]
    fn flattening_blends_transparency_onto_white() {
        let flat = flatten_onto_white(&sample());
        // Opaque: unchanged.
        assert_eq!(flat.get_pixel(0, 0).0, [255, 0, 0]);
        assert_eq!(flat.get_pixel(1, 1).0, [200, 100, 50]);
        // Fully transparent: white, not the hidden colour underneath.
        assert_eq!(flat.get_pixel(0, 1).0, [255, 255, 255]);
        // Half transparent: halfway towards white.
        assert_eq!(flat.get_pixel(1, 0).0, [127, 191, 255]);
    }

    #[test]
    fn a_jpeg_is_flattened_and_survives_being_saved_twice() {
        let folder = TempFolder::new("jpeg");
        let path = folder.file("out.jpg");
        let pixels = RgbaImage::from_fn(16, 16, |x, _| {
            if x < 8 {
                Rgba([200, 100, 50, 255])
            } else {
                Rgba([0, 0, 0, 0])
            }
        });
        save(&pixels, &path, FileFormat::Jpeg).unwrap();
        let first = image::open(&path).unwrap().into_rgba8();
        // Save the reloaded image again, as Save does after edits.
        save(&first, &path, FileFormat::Jpeg).unwrap();
        let second = image::open(&path).unwrap().into_rgba8();

        for image in [&first, &second] {
            let left = image.get_pixel(3, 8).0;
            let right = image.get_pixel(12, 8).0;
            for (actual, expected) in left.into_iter().zip([200, 100, 50, 255]) {
                assert!(actual.abs_diff(expected) <= 8, "{left:?}");
            }
            for (actual, expected) in right.into_iter().zip([255, 255, 255, 255]) {
                assert!(actual.abs_diff(expected) <= 8, "{right:?}");
            }
        }
    }

    #[test]
    fn saving_replaces_an_existing_file_and_leaves_no_temporary_file() {
        let folder = TempFolder::new("replace");
        let path = folder.file("photo.png");
        fs::write(&path, b"old contents").unwrap();

        save(&sample(), &path, FileFormat::Png).unwrap();

        assert_eq!(folder.names(), ["photo.png"]);
        let reloaded = image::open(&path).unwrap().into_rgba8();
        assert_eq!(reloaded, sample());
    }

    #[test]
    fn a_missing_folder_fails_without_creating_anything() {
        let folder = TempFolder::new("missing-folder");
        let path = folder.file("does-not-exist").join("photo.png");
        let error = save(&sample(), &path, FileFormat::Png).unwrap_err();
        assert_eq!(error, "the folder doesn't exist");
        assert!(folder.names().is_empty());
    }

    // The original must survive every failure byte for byte, and no temporary
    // file may be left next to it.
    fn assert_original_untouched(folder: &TempFolder, path: &Path, original: &[u8]) {
        assert_eq!(fs::read(path).unwrap(), original);
        assert_eq!(folder.names(), [display_name(path)]);
    }

    #[test]
    fn a_read_only_file_is_left_untouched() {
        let folder = TempFolder::new("read-only");
        let path = folder.file("locked.png");
        fs::write(&path, b"original bytes").unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions.clone()).unwrap();

        let result = save(&sample(), &path, FileFormat::Png);

        // Allow cleanup whatever happened. On Windows this only clears the
        // read-only flag, which is what clippy's warning is about on Unix.
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(&path, permissions).unwrap();

        let error = result.unwrap_err();
        assert!(error.starts_with("Windows denied access"), "{error}");
        assert_original_untouched(&folder, &path, b"original bytes");
    }

    #[cfg(windows)]
    #[test]
    fn a_file_locked_by_another_program_is_left_untouched() {
        use std::os::windows::fs::OpenOptionsExt;

        let folder = TempFolder::new("locked");
        let path = folder.file("open-elsewhere.png");
        fs::write(&path, b"original bytes").unwrap();
        // Hold the file open without letting anyone else read, write or
        // replace it, like an exclusive lock from another program.
        let lock = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();

        let result = save(&sample(), &path, FileFormat::Png);
        drop(lock);

        let error = result.unwrap_err();
        assert!(
            error == "the file is open in another program"
                || error.starts_with("Windows denied access"),
            "{error}"
        );
        assert_original_untouched(&folder, &path, b"original bytes");
    }

    #[test]
    fn a_folder_with_the_same_name_is_left_untouched() {
        let folder = TempFolder::new("folder-in-the-way");
        let path = folder.file("photo.png");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("inside.txt"), b"keep me").unwrap();

        let error = save(&sample(), &path, FileFormat::Png).unwrap_err();

        assert_eq!(error, "a folder with that name already exists");
        assert_eq!(fs::read(path.join("inside.txt")).unwrap(), b"keep me");
        assert_eq!(folder.names(), ["photo.png"]);
    }

    #[test]
    fn temporary_files_get_unique_names_in_the_same_folder() {
        let path = Path::new(r"C:\pictures\photo.png");
        let first = temporary_path_for(path);
        let second = temporary_path_for(path);
        assert_ne!(first, second);
        assert_eq!(first.parent(), path.parent());
        let name = first.file_name().unwrap().to_string_lossy();
        assert!(
            name.starts_with(".photo.png.moxo-") && name.ends_with(".tmp"),
            "{name}"
        );
    }
}
