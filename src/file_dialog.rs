// Native Windows dialogs (Open, Save As and questions), via the rfd crate.

use std::future::Future;
use std::path::{Path, PathBuf};

use gpui::Window;
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
};
use rfd::{
    AsyncFileDialog, AsyncMessageDialog, FileHandle, MessageButtons, MessageDialogResult,
    MessageLevel,
};

use crate::APP_NAME;
use crate::document::Loss;
use crate::save::FileFormat;

// The file types the dialog offers. Each must be one `image_loader` can open.
const IMAGE_EXTENSIONS: [&str; 3] = ["png", "jpg", "jpeg"];

// With a parent, a dialog stays in front of Moxo and Moxo can't be clicked
// until it closes. Without one it still works, it just floats freely.
fn parent_of(window: &Window) -> Option<DialogParent<'_>> {
    HasWindowHandle::window_handle(window)
        .ok()
        .map(DialogParent)
}

// Shows the dialog on top of `window`. The result is `None` if the user cancels.
pub fn pick_image(window: &Window) -> impl Future<Output = Option<FileHandle>> + use<> {
    let mut dialog = AsyncFileDialog::new()
        .set_title("Open Image")
        .add_filter("Images (PNG, JPEG)", &IMAGE_EXTENSIONS);
    if let Some(parent) = parent_of(window) {
        dialog = dialog.set_parent(&parent);
    }
    dialog.pick_file()
}

// Asks where to save, starting from the document's current file. The current
// format's filter comes first, so Windows adds that extension to a name typed
// without one. Moxo still checks the final name itself (`save::resolve_save_path`).
pub fn pick_save_path(
    window: &Window,
    current: &Path,
    format: FileFormat,
) -> impl Future<Output = Option<PathBuf>> + use<> {
    let png = ("PNG image", &["png"][..]);
    let jpeg = ("JPEG image", &["jpg", "jpeg"][..]);
    let (first, second) = match format {
        FileFormat::Png => (png, jpeg),
        FileFormat::Jpeg => (jpeg, png),
    };
    let mut dialog = AsyncFileDialog::new()
        .set_title("Save Image As")
        .add_filter(first.0, first.1)
        .add_filter(second.0, second.1);
    if let Some(folder) = current.parent() {
        dialog = dialog.set_directory(folder);
    }
    if let Some(name) = current.file_name() {
        dialog = dialog.set_file_name(name.to_string_lossy());
    }
    if let Some(parent) = parent_of(window) {
        dialog = dialog.set_parent(&parent);
    }
    let picked = dialog.save_file();
    async move { picked.await.map(|file| file.path().to_path_buf()) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveChoice {
    Save,
    Discard,
    Cancel,
}

// Asks whether to save unsaved changes. The buttons are Windows' standard
// Yes / No / Cancel (in the system language; custom labels would need another
// rfd feature), so the question is worded to match them.
pub fn ask_to_save_changes(
    window: &Window,
    file_name: &str,
    before: &str,
) -> impl Future<Output = SaveChoice> + use<> {
    let mut dialog = AsyncMessageDialog::new()
        .set_level(MessageLevel::Warning)
        .set_title(APP_NAME)
        .set_description(format!(
            "Do you want to save the changes to {file_name} {before}?"
        ))
        .set_buttons(MessageButtons::YesNoCancel);
    if let Some(parent) = parent_of(window) {
        dialog = dialog.set_parent(&parent);
    }
    let answer = dialog.show();
    async move {
        match answer.await {
            MessageDialogResult::Yes => SaveChoice::Save,
            MessageDialogResult::No => SaveChoice::Discard,
            _ => SaveChoice::Cancel,
        }
    }
}

// Warns before a save that would lose information Moxo can detect. Resolves to
// `true` if the user chooses to save anyway.
pub fn confirm_information_loss(
    window: &Window,
    file_name: &str,
    losses: &[Loss],
) -> impl Future<Output = bool> + use<> {
    let mut dialog = AsyncMessageDialog::new()
        .set_level(MessageLevel::Warning)
        .set_title(APP_NAME)
        .set_description(information_loss_message(file_name, losses))
        .set_buttons(MessageButtons::OkCancel);
    if let Some(parent) = parent_of(window) {
        dialog = dialog.set_parent(&parent);
    }
    let answer = dialog.show();
    async move { answer.await == MessageDialogResult::Ok }
}

// Lists only what Moxo detected, and says separately what it never checks.
fn information_loss_message(file_name: &str, losses: &[Loss]) -> String {
    let list: Vec<String> = losses
        .iter()
        .map(|loss| format!("\u{2022} {}", loss.describe()))
        .collect();
    format!(
        "Saving {file_name} will lose information:\n\n{}\n\n{APP_NAME} also doesn't keep other metadata, such as camera details.\n\nSave anyway?",
        list.join("\n")
    )
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
            assert_eq!(loaded.pixels.dimensions(), (2, 2));
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

    #[test]
    fn the_loss_warning_lists_only_what_was_detected() {
        let message = information_loss_message("photo.png", &[Loss::BitDepth, Loss::Transparency]);
        assert!(message.starts_with("Saving photo.png will lose information:"));
        assert!(message.contains(Loss::BitDepth.describe()));
        assert!(message.contains(Loss::Transparency.describe()));
        assert!(!message.contains(Loss::ColourProfile.describe()));
        // Metadata Moxo never inspects is mentioned separately, not as a detected loss.
        assert!(message.contains("doesn't keep other metadata"));
        assert!(message.ends_with("Save anyway?"));
    }
}
