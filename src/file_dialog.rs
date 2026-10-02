// The native Windows "Open" dialog, via the rfd crate.

use std::future::Future;

use gpui::Window;
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
};
use rfd::{AsyncFileDialog, FileHandle};

// Shows the dialog on top of `window`. The result is `None` if the user cancels.
pub fn pick_image(window: &Window) -> impl Future<Output = Option<FileHandle>> + use<> {
    let mut dialog = AsyncFileDialog::new()
        .set_title("Open Image")
        .add_filter("Images (PNG, JPEG)", &["png", "jpg", "jpeg"]);

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
