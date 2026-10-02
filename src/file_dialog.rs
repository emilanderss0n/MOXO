// The native Windows "Open" dialog, via the rfd crate.

use std::future::Future;

use gpui::Window;
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
};
use rfd::{AsyncFileDialog, FileHandle};

// The file types the dialog offers. Each must be one `image_loader` can open.
const IMAGE_EXTENSIONS: [&str; 3] = ["png", "jpg", "jpeg"];

// Shows the dialog on top of `window`. The result is `None` if the user cancels.
pub fn pick_image(window: &Window) -> impl Future<Output = Option<FileHandle>> + use<> {
    let mut dialog = AsyncFileDialog::new()
        .set_title("Open Image")
        .add_filter("Images (PNG, JPEG)", &IMAGE_EXTENSIONS);

    // With a parent, the dialog stays in front of Moxo and Moxo can't be clicked
    // until it closes. Without one it still works, it just floats freely.
    if let Ok(handle) = HasWindowHandle::window_handle(window) {
        dialog = dialog.set_parent(&DialogParent(handle));
    }

    dialog.pick_file()
}

// rfd asks the parent window for a "window handle" and a "display handle".
// GPUI 0.2.2 crashes when asked for the display handle on Windows (it's an
// unfinished `unimplemented!()`), so this wrapper passes on the window handle
// and answers the display question itself. On Windows that answer is always
// the same fixed value.
struct DialogParent<'a>(WindowHandle<'a>);

impl HasWindowHandle for DialogParent<'_> {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        Ok(self.0)
    }
}

impl HasDisplayHandle for DialogParent<'_> {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::windows())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, RgbImage};
    use raw_window_handle::{RawDisplayHandle, RawWindowHandle, Win32WindowHandle};
    use std::fs;
    use std::num::NonZeroIsize;
    use std::path::PathBuf;

    // A file in a temporary folder, deleted when the test ends (even if it fails).
    struct TempFile(PathBuf);

    impl TempFile {
        fn new(name: &str) -> Self {
            let folder =
                std::env::temp_dir().join(format!("moxo-file-dialog-tests-{}", std::process::id()));
            fs::create_dir_all(&folder).unwrap();
            Self(folder.join(name))
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    // Every extension the dialog offers must open in the loader, otherwise people
    // could pick files that always fail.
    #[test]
    fn every_offered_file_type_can_be_opened() {
        let mut formats = Vec::new();
        for extension in IMAGE_EXTENSIONS {
            let format = ImageFormat::from_extension(extension)
                .unwrap_or_else(|| panic!("'{extension}' isn't an image file extension"));
            formats.push(format);

            let file = TempFile::new(&format!("tiny.{extension}"));
            RgbImage::new(2, 2)
                .save_with_format(&file.0, format)
                .unwrap();
            let loaded = crate::image_loader::load(&file.0)
                .unwrap_or_else(|reason| panic!("a .{extension} file didn't open: {reason}"));
            assert_eq!((loaded.width, loaded.height), (2, 2));
        }

        // And both formats the loader supports are offered.
        assert!(
            formats.contains(&ImageFormat::Png),
            "the dialog offers no PNG extension"
        );
        assert!(
            formats.contains(&ImageFormat::Jpeg),
            "the dialog offers no JPEG extension"
        );
    }

    // The workaround for GPUI's display-handle crash: the wrapper must answer the
    // display question itself and pass the window handle on untouched.
    #[test]
    fn dialog_parent_reports_a_windows_display_and_passes_the_window_on() {
        let raw =
            RawWindowHandle::Win32(Win32WindowHandle::new(NonZeroIsize::new(0x1234).unwrap()));
        // SAFETY: `borrow_raw` promises whoever receives the handle that it points
        // at a real window. This one is made up, which is fine only because nothing
        // here gives it to Windows or rfd: no dialog is opened, and `DialogParent`
        // just copies the value back out. Never pass this handle to real code.
        let window = unsafe { WindowHandle::borrow_raw(raw) };
        let parent = DialogParent(window);

        let display = parent
            .display_handle()
            .expect("asking for the display handle must not fail or crash");
        assert!(matches!(display.as_raw(), RawDisplayHandle::Windows(_)));

        let passed_on = parent.window_handle().unwrap();
        assert_eq!(passed_on.as_raw(), raw);
    }
}
