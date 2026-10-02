// Hide the extra console window when running a release build.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod canvas;
mod document;
mod file_dialog;
mod image_loader;
mod navigation;
mod save;
mod viewport;

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    Action, App, Application, Bounds, Context, CursorStyle, DispatchPhase, FocusHandle, Hsla,
    KeyBinding, KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, RenderImage, ScrollWheelEvent, SharedString, Size, Task,
    TitlebarOptions, Window, WindowBounds, WindowOptions, actions, deferred, div, hsla, prelude::*,
    px, rgb, size,
};

use document::{Document, Transform};
use file_dialog::SaveChoice;
use navigation::{PanDrag, ScrollAction};
use save::FileFormat;
use viewport::Viewport;

// The product's display name, shown in window titles and messages. It's
// provisional: to rename the product, change it here rather than in
// individual strings. (Technical names like the Cargo package stay as they are.)
const APP_NAME: &str = "Moxo";

actions!(
    moxo,
    [
        OpenImage,
        Save,
        SaveAs,
        Undo,
        Redo,
        RotateClockwise,
        RotateCounterClockwise,
        FlipHorizontal,
        FlipVertical,
        ZoomIn,
        ZoomOut,
        FitOnScreen,
        ActualSize
    ]
);

// Colours for the dark theme.
const CHROME_BG: u32 = 0x262626; // menu bar, status bar
const CANVAS_BG: u32 = 0x1b1b1b; // the area around the image
const MENU_BG: u32 = 0x303030;
const TEXT: u32 = 0xdedede;
const TEXT_MUTED: u32 = 0x8e8e8e;
const TEXT_DISABLED: u32 = 0x5c5c5c;
const ERROR_TEXT: u32 = 0xf28b82;

// The canvas fills the window between these two bars.
const MENU_BAR_HEIGHT: Pixels = px(32.);
const STATUS_BAR_HEIGHT: Pixels = px(26.);

fn hover_tint() -> Hsla {
    hsla(0., 0., 1., 0.07)
}

// The area between the menu bar and the status bar, in GPUI's scaled units.
fn canvas_bounds(window: &Window) -> Bounds<Pixels> {
    canvas::canvas_bounds(window.viewport_size(), MENU_BAR_HEIGHT, STATUS_BAR_HEIGHT)
}

// The canvas size in physical screen pixels.
fn canvas_area(window: &Window) -> Size<f32> {
    canvas::area_in_screen_pixels(canvas_bounds(window), window.scale_factor())
}

// The message shown when a file can't be opened. `reason` comes from the image
// loader, which leaves off the full stop so this sentence can add it.
fn open_error_message(file_name: &str, reason: &str) -> String {
    format!("Couldn't open {file_name}: {reason}.")
}

// The same for saving; `reason` comes from `save.rs`, also without a full stop.
fn save_error_message(file_name: &str, reason: &str) -> String {
    format!("Couldn't save {file_name}: {reason}.")
}

// "photo.png - Moxo", with a leading * while there are unsaved changes (the
// usual Windows convention, as in Notepad).
fn window_title(file_name: &str, unsaved: bool) -> String {
    let marker = if unsaved { "*" } else { "" };
    format!("{marker}{file_name} - {APP_NAME}")
}

// The open document, plus how it's shown: GPUI's copy of the pixels and the
// zoom and pan. The document itself knows nothing about either.
struct OpenedImage {
    // Tells documents apart, so a save that finishes after another image was
    // opened can't mark the new one as saved.
    id: u64,
    document: Document,
    display: Arc<RenderImage>,
    viewport: Viewport,
}

// What a running save needs to record its result.
struct PendingSave {
    document_id: u64,
    state: u64,
    path: PathBuf,
    format: FileFormat,
    file_name: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Menu {
    File,
    Edit,
    Image,
    View,
}

struct MenuItem {
    label: &'static str,
    shortcut: &'static str,
    action: Box<dyn Action>,
    enabled: bool,
}

struct Moxo {
    focus_handle: FocusHandle,
    open_menu: Option<Menu>,
    opened: Option<OpenedImage>,
    // Name of the file being decoded right now, if any.
    loading: Option<SharedString>,
    error: Option<SharedString>,
    // The running "pick a file, then decode it" job. Replacing it cancels the old one.
    open_task: Option<Task<()>>,
    // Holding Space turns a left-drag into panning, like Photoshop's hand tool.
    space_held: bool,
    pan_drag: PanDrag,
    // The checkerboard tile, and the display scale it was made for.
    checker: Option<(f32, Arc<RenderImage>)>,
    next_document_id: u64,
    // Only one save runs at a time, and opening another image waits for it.
    saving: bool,
    // The window was asked to close while a save was running.
    close_after_save: bool,
    // The user already chose to close, so the next close request goes through.
    close_confirmed: bool,
}

impl Moxo {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // If Moxo loses focus while Space or a mouse button is held, we never see
        // it being released, so forget about it.
        cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.space_held = false;
                this.pan_drag.end();
                cx.notify();
            }
        })
        .detach();

        // The X button, Alt+F4 and closing from the taskbar all ask this first;
        // answering `false` keeps the window open.
        let this = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            this.update(cx, |this, cx| this.should_close(window, cx))
                .unwrap_or(true)
        });

        Self {
            focus_handle: cx.focus_handle(),
            open_menu: None,
            opened: None,
            loading: None,
            error: None,
            open_task: None,
            space_held: false,
            pan_drag: PanDrag::default(),
            checker: None,
            next_document_id: 0,
            saving: false,
            close_after_save: false,
            close_confirmed: false,
        }
    }

    fn open_image(&mut self, _: &OpenImage, window: &mut Window, cx: &mut Context<Self>) {
        self.open_menu = None;
        cx.notify();
        // Replacing `open_task` mid-save would cancel the save, so wait for it.
        if self.saving {
            return;
        }

        let ask_first = self
            .opened
            .as_ref()
            .filter(|opened| opened.document.has_unsaved_changes())
            .map(|opened| {
                file_dialog::ask_to_save_changes(
                    window,
                    &opened.document.file_name(),
                    "before opening another image",
                )
            });

        self.open_task = Some(cx.spawn_in(window, async move |this, cx| {
            if let Some(answer) = ask_first {
                match answer.await {
                    SaveChoice::Cancel => return,
                    SaveChoice::Discard => {}
                    SaveChoice::Save => {
                        let Ok(saving) = this
                            .update_in(cx, |this, window, cx| this.start_save(None, window, cx))
                        else {
                            return;
                        };
                        if !saving.await {
                            return;
                        }
                    }
                }
            }

            let Ok(picked_file) =
                this.update_in(cx, |_, window, _| file_dialog::pick_image(window))
            else {
                return;
            };
            // `None` means the user cancelled the dialog: keep everything as it was.
            let Some(file) = picked_file.await else {
                return;
            };
            let path = file.path().to_path_buf();
            let file_name: SharedString = file.file_name().into();

            this.update(cx, |this, cx| {
                this.loading = Some(file_name.clone());
                cx.notify();
            })
            .ok();

            // Decoding a big photo can take a moment, so do it off the UI thread.
            let load_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { image_loader::load(&load_path) })
                .await;

            this.update_in(cx, |this, window, cx| {
                this.loading = None;
                match result {
                    Ok(loaded) => {
                        // Free the previous image's GPU memory.
                        if let Some(previous) = this.opened.take() {
                            cx.drop_image(previous.display, Some(window));
                        }
                        this.error = None;
                        let document =
                            Document::new(loaded.pixels, path, loaded.format, loaded.source);
                        this.next_document_id += 1;
                        this.opened = Some(OpenedImage {
                            id: this.next_document_id,
                            display: canvas::display_image(document.pixels()),
                            viewport: Viewport::new(document.width(), document.height()),
                            document,
                        });
                        this.update_title(window);
                    }
                    Err(reason) => {
                        this.error = Some(open_error_message(&file_name, &reason).into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        self.open_menu = None;
        let unsaved = self
            .opened
            .as_ref()
            .is_some_and(|opened| opened.document.has_unsaved_changes());
        if unsaved {
            self.start_save(None, window, cx).detach();
        }
        cx.notify();
    }

    fn save_as(&mut self, _: &SaveAs, window: &mut Window, cx: &mut Context<Self>) {
        self.open_menu = None;
        cx.notify();
        let Some(opened) = &self.opened else {
            return;
        };
        if self.saving {
            return;
        }
        let document = &opened.document;
        let fallback = document.format();
        let picked = file_dialog::pick_save_path(window, document.path(), fallback);

        cx.spawn_in(window, async move |this, cx| {
            let Some(chosen) = picked.await else {
                return;
            };
            match save::resolve_save_path(&chosen, fallback) {
                Ok(target) => {
                    if let Ok(saving) = this.update_in(cx, |this, window, cx| {
                        this.start_save(Some(target), window, cx)
                    }) {
                        saving.await;
                    }
                }
                Err(reason) => {
                    let name = chosen
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    this.update(cx, |this, cx| {
                        this.error = Some(save_error_message(&name, &reason).into());
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    // Saves the document to `target`, or to its own file. The task resolves to
    // whether the file was written. The encoding and writing run in the
    // background; edits made meanwhile aren't included and stay unsaved.
    fn start_save(
        &mut self,
        target: Option<(PathBuf, FileFormat)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let Some(opened) = &self.opened else {
            return Task::ready(false);
        };
        if self.saving {
            return Task::ready(false);
        }
        let document = &opened.document;
        let (path, format) =
            target.unwrap_or_else(|| (document.path().to_path_buf(), document.format()));
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let losses = document.losses_when_saving(&path, format);
        let confirm = (!losses.is_empty())
            .then(|| file_dialog::confirm_information_loss(window, &file_name, &losses));
        let document_id = opened.id;
        self.saving = true;
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            if let Some(confirm) = confirm
                && !confirm.await
            {
                this.update(cx, |this, cx| {
                    this.saving = false;
                    cx.notify();
                })
                .ok();
                return false;
            }

            // Take the pixels and the version number now. Later edits change the
            // document's own copy, not this one.
            let snapshot = this
                .update(cx, |this, _| {
                    this.opened
                        .as_ref()
                        .filter(|opened| opened.id == document_id)
                        .map(|opened| opened.document.save_snapshot())
                })
                .ok()
                .flatten();
            let Some(snapshot) = snapshot else {
                // The document is gone; don't leave saving switched off for good.
                this.update(cx, |this, cx| {
                    this.saving = false;
                    cx.notify();
                })
                .ok();
                return false;
            };

            let pixels = snapshot.pixels;
            let write_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { save::save(&pixels, &write_path, format) })
                .await;

            let pending = PendingSave {
                document_id,
                state: snapshot.state,
                path,
                format,
                file_name,
            };
            this.update_in(cx, |this, window, cx| {
                this.finish_save(pending, result, window, cx)
            })
            .unwrap_or(false)
        })
    }

    fn finish_save(
        &mut self,
        pending: PendingSave,
        result: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.saving = false;
        let saved = match result {
            Ok(()) => {
                if let Some(opened) = self
                    .opened
                    .as_mut()
                    .filter(|opened| opened.id == pending.document_id)
                {
                    // Marks the version that was written as saved. If the image
                    // was edited meanwhile, it stays marked as unsaved.
                    opened
                        .document
                        .mark_saved(pending.state, pending.path, pending.format);
                }
                self.error = None;
                true
            }
            Err(reason) => {
                self.error = Some(save_error_message(&pending.file_name, &reason).into());
                false
            }
        };
        self.update_title(window);
        cx.notify();

        if self.close_after_save {
            self.close_after_save = false;
            if saved && self.should_close(window, cx) {
                self.close_window(window);
            }
        }
        saved
    }

    // Asked before the window closes. Returning `false` keeps it open; with
    // unsaved changes, Moxo asks first and closes the window itself afterwards.
    fn should_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.close_confirmed {
            return true;
        }
        if self.saving {
            // Let the save finish first; `finish_save` tries again.
            self.close_after_save = true;
            return false;
        }
        let Some(opened) = self
            .opened
            .as_ref()
            .filter(|opened| opened.document.has_unsaved_changes())
        else {
            return true;
        };

        let answer = file_dialog::ask_to_save_changes(
            window,
            &opened.document.file_name(),
            "before closing",
        );
        cx.spawn_in(window, async move |this, cx| {
            let close = match answer.await {
                SaveChoice::Cancel => false,
                SaveChoice::Discard => true,
                SaveChoice::Save => {
                    match this.update_in(cx, |this, window, cx| this.start_save(None, window, cx)) {
                        Ok(saving) => saving.await,
                        Err(_) => false,
                    }
                }
            };
            if close {
                this.update_in(cx, |this, window, _| this.close_window(window))
                    .ok();
            }
        })
        .detach();
        false
    }

    fn close_window(&mut self, window: &mut Window) {
        self.close_confirmed = true;
        window.remove_window();
    }

    fn edit(&mut self, transform: Transform, window: &mut Window, cx: &mut Context<Self>) {
        self.open_menu = None;
        let Some(opened) = &mut self.opened else {
            return;
        };
        opened.document.apply(transform);
        self.show_change(transform, window, cx);
    }

    fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        self.open_menu = None;
        if let Some(applied) = self
            .opened
            .as_mut()
            .and_then(|opened| opened.document.undo())
        {
            self.show_change(applied, window, cx);
        }
        cx.notify();
    }

    fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        self.open_menu = None;
        if let Some(applied) = self
            .opened
            .as_mut()
            .and_then(|opened| opened.document.redo())
        {
            self.show_change(applied, window, cx);
        }
        cx.notify();
    }

    // Refreshes GPUI's copy after the document's pixels changed. A rotation
    // changes the proportions, so the view is refitted; flips keep zoom and pan.
    fn show_change(&mut self, applied: Transform, window: &mut Window, cx: &mut Context<Self>) {
        let Some(opened) = &mut self.opened else {
            return;
        };
        let previous = std::mem::replace(
            &mut opened.display,
            canvas::display_image(opened.document.pixels()),
        );
        cx.drop_image(previous, Some(window));
        if applied.changes_size() {
            opened.viewport = Viewport::new(opened.document.width(), opened.document.height());
        }
        self.update_title(window);
        cx.notify();
    }

    fn rotate_clockwise(
        &mut self,
        _: &RotateClockwise,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit(Transform::RotateClockwise, window, cx);
    }

    fn rotate_counter_clockwise(
        &mut self,
        _: &RotateCounterClockwise,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit(Transform::RotateCounterClockwise, window, cx);
    }

    fn flip_horizontal(&mut self, _: &FlipHorizontal, window: &mut Window, cx: &mut Context<Self>) {
        self.edit(Transform::FlipHorizontal, window, cx);
    }

    fn flip_vertical(&mut self, _: &FlipVertical, window: &mut Window, cx: &mut Context<Self>) {
        self.edit(Transform::FlipVertical, window, cx);
    }

    fn update_title(&self, window: &mut Window) {
        let title = match &self.opened {
            Some(opened) => window_title(
                &opened.document.file_name(),
                opened.document.has_unsaved_changes(),
            ),
            None => APP_NAME.to_string(),
        };
        window.set_window_title(&title);
    }

    // Runs a change on the open image's viewport, if there is one.
    fn navigate(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut Viewport, Size<f32>),
    ) {
        if let Some(opened) = &mut self.opened {
            change(&mut opened.viewport, canvas_area(window));
            cx.notify();
        }
    }

    fn zoom_in(&mut self, _: &ZoomIn, window: &mut Window, cx: &mut Context<Self>) {
        self.navigate(window, cx, |viewport, area| viewport.zoom_in(area));
    }

    fn zoom_out(&mut self, _: &ZoomOut, window: &mut Window, cx: &mut Context<Self>) {
        self.navigate(window, cx, |viewport, area| viewport.zoom_out(area));
    }

    fn fit_on_screen(&mut self, _: &FitOnScreen, window: &mut Window, cx: &mut Context<Self>) {
        self.navigate(window, cx, |viewport, _| viewport.fit());
    }

    fn actual_size(&mut self, _: &ActualSize, window: &mut Window, cx: &mut Context<Self>) {
        self.navigate(window, cx, |viewport, area| viewport.actual_size(area));
    }

    // Plain scroll pans, Shift+scroll pans sideways, and Ctrl+scroll zooms
    // around the mouse pointer.
    fn on_scroll(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let scale = window.scale_factor();
        let action = navigation::scroll_action(event.delta, event.modifiers.control, scale);
        let anchor =
            canvas::position_on_canvas(event.position, canvas_bounds(window).origin, scale);

        self.navigate(window, cx, |viewport, area| match action {
            ScrollAction::Pan(delta) => viewport.pan_by(area, delta),
            ScrollAction::Zoom(factor) => {
                viewport.zoom_at(area, anchor, viewport.zoom(area) * factor)
            }
        });
    }

    fn on_canvas_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.opened.is_some()
            && self
                .pan_drag
                .start(event.button, self.space_held, event.position)
        {
            cx.notify();
        }
    }

    fn drag_to(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(delta) = self.pan_drag.move_to(position, window.scale_factor()) {
            self.navigate(window, cx, |viewport, area| viewport.pan_by(area, delta));
        }
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        self.pan_drag.end();
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" if self.open_menu.is_some() => {
                self.open_menu = None;
                cx.notify();
            }
            "space" if !self.space_held => {
                self.space_held = true;
                cx.notify();
            }
            _ => {}
        }
    }

    fn on_key_up(&mut self, event: &KeyUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "space" {
            self.space_held = false;
            cx.notify();
        }
    }

    // Makes the checkerboard tile for the current display scale. It's rebuilt if
    // the window moves to a monitor with a different scale.
    fn update_checker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let scale = window.scale_factor();
        if self
            .checker
            .as_ref()
            .is_some_and(|(made_for, _)| *made_for == scale)
        {
            return;
        }
        if let Some((_, old)) = self.checker.replace((scale, canvas::checker_tile(scale))) {
            cx.drop_image(old, Some(window));
        }
    }

    fn menu_items(&self, menu: Menu) -> Vec<MenuItem> {
        let has_image = self.opened.is_some();
        let document = self.opened.as_ref().map(|opened| &opened.document);
        let unsaved = document.is_some_and(Document::has_unsaved_changes);
        let can_undo = document.is_some_and(Document::can_undo);
        let can_redo = document.is_some_and(Document::can_redo);
        let item = |label, shortcut, action: Box<dyn Action>, enabled| MenuItem {
            label,
            shortcut,
            action,
            enabled,
        };
        match menu {
            Menu::File => vec![
                item("Open Image…", "Ctrl+O", Box::new(OpenImage), !self.saving),
                item("Save", "Ctrl+S", Box::new(Save), unsaved && !self.saving),
                item(
                    "Save As…",
                    "Ctrl+Shift+S",
                    Box::new(SaveAs),
                    has_image && !self.saving,
                ),
            ],
            Menu::Edit => vec![
                item("Undo", "Ctrl+Z", Box::new(Undo), can_undo),
                item("Redo", "Ctrl+Y", Box::new(Redo), can_redo),
            ],
            Menu::Image => vec![
                item(
                    "Rotate 90° Clockwise",
                    "",
                    Box::new(RotateClockwise),
                    has_image,
                ),
                item(
                    "Rotate 90° Counter-clockwise",
                    "",
                    Box::new(RotateCounterClockwise),
                    has_image,
                ),
                item("Flip Horizontal", "", Box::new(FlipHorizontal), has_image),
                item("Flip Vertical", "", Box::new(FlipVertical), has_image),
            ],
            Menu::View => vec![
                item("Zoom In", "Ctrl++", Box::new(ZoomIn), has_image),
                item("Zoom Out", "Ctrl+-", Box::new(ZoomOut), has_image),
                item("Fit on Screen", "Ctrl+0", Box::new(FitOnScreen), has_image),
                item("Actual Size", "Ctrl+1", Box::new(ActualSize), has_image),
            ],
        }
    }

    fn render_menu_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_none()
            .h(MENU_BAR_HEIGHT)
            .px_1()
            .flex()
            .items_center()
            .bg(rgb(CHROME_BG))
            .child(self.render_menu(Menu::File, "File", cx))
            .child(self.render_menu(Menu::Edit, "Edit", cx))
            .child(self.render_menu(Menu::Image, "Image", cx))
            .child(self.render_menu(Menu::View, "View", cx))
    }

    fn render_menu(
        &self,
        menu: Menu,
        title: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let was_open = self.open_menu == Some(menu);

        let button = div()
            .id(title)
            .px_2()
            .py_1()
            .rounded_md()
            .when(was_open, |button| button.bg(hover_tint()))
            .hover(|style| style.bg(hover_tint()))
            .child(title)
            // Like native menus, open on mouse down. Clicking a menu's title while it
            // is open first triggers the panel's "click outside", which closes it, so
            // decide from the state the menu had when this frame was drawn.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.open_menu = if was_open { None } else { Some(menu) };
                    cx.notify();
                }),
            );

        let panel = div()
            .absolute()
            .top(px(30.))
            .left_0()
            .w(px(220.))
            .p(px(5.))
            .rounded_lg()
            .bg(rgb(MENU_BG))
            .shadow_lg()
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.open_menu = None;
                cx.notify();
            }))
            .children(
                self.menu_items(menu)
                    .into_iter()
                    .map(|item| self.render_menu_item(item, cx)),
            );

        div()
            .relative()
            .child(button)
            // `deferred` draws the menu after the rest of the window, so it sits on top.
            .when(was_open, |wrapper| wrapper.child(deferred(panel)))
    }

    // `use<>` tells Rust the returned row doesn't keep borrowing `self` or `cx`,
    // so several rows can be built one after another.
    fn render_menu_item(&self, item: MenuItem, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let MenuItem {
            label,
            shortcut,
            action,
            enabled,
        } = item;

        div()
            .id(label)
            .h(px(30.))
            .px_2()
            .flex()
            .items_center()
            .justify_between()
            .rounded_md()
            .when(!enabled, |row| row.text_color(rgb(TEXT_DISABLED)))
            .when(enabled, |row| {
                row.hover(|style| style.bg(hover_tint()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_menu = None;
                        cx.notify();
                        window.dispatch_action(action.boxed_clone(), cx);
                    }))
            })
            .child(label)
            .child(
                div()
                    .text_color(rgb(if enabled { TEXT_MUTED } else { TEXT_DISABLED }))
                    .child(shortcut),
            )
    }

    fn render_canvas(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match (&self.opened, &self.checker) {
            (Some(opened), Some((_, checker))) => {
                let viewport = opened.viewport;
                let image = opened.display.clone();
                let checker = checker.clone();
                let dragging = self.pan_drag.is_active();
                let this = cx.weak_entity();

                gpui::canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        canvas::paint(bounds, &viewport, &image, &checker, window);

                        // While dragging, follow the mouse everywhere, even outside
                        // the canvas or the window, until the button is released.
                        if dragging {
                            let this_for_move = this.clone();
                            window.on_mouse_event(
                                move |event: &MouseMoveEvent, phase, window, cx| {
                                    if phase == DispatchPhase::Bubble {
                                        this_for_move
                                            .update(cx, |this, cx| {
                                                this.drag_to(event.position, window, cx)
                                            })
                                            .ok();
                                    }
                                },
                            );
                            let this_for_up = this.clone();
                            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                                if phase == DispatchPhase::Bubble {
                                    this_for_up.update(cx, |this, cx| this.end_drag(cx)).ok();
                                }
                            });
                        }
                    },
                )
                .absolute()
                .size_full()
                .into_any_element()
            }
            _ => self.render_empty_state(cx).into_any_element(),
        };

        let has_image = self.opened.is_some();

        div()
            .flex_1()
            .min_h_0()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(CANVAS_BG))
            .when(has_image, |canvas| {
                canvas
                    .on_scroll_wheel(cx.listener(Self::on_scroll))
                    .on_any_mouse_down(cx.listener(Self::on_canvas_mouse_down))
            })
            .when(has_image && self.pan_drag.is_active(), |canvas| {
                canvas.cursor(CursorStyle::ClosedHand)
            })
            .when(
                has_image && self.space_held && !self.pan_drag.is_active(),
                |canvas| canvas.cursor(CursorStyle::OpenHand),
            )
            .child(content)
            .when_some(self.error.clone(), |canvas, message| {
                canvas.child(self.render_error(message, cx))
            })
    }

    fn render_empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(file_name) = &self.loading {
            return div()
                .text_color(rgb(TEXT_MUTED))
                .child(format!("Opening {file_name}…"));
        }

        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .child(
                div()
                    .id("open-image-empty")
                    .px_4()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(MENU_BG))
                    .hover(|style| style.bg(rgb(0x3a3a3a)))
                    .child("Open Image…")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_image(&OpenImage, window, cx)),
                    ),
            )
            .child(div().text_color(rgb(TEXT_MUTED)).child("or press Ctrl+O"))
    }

    fn render_error(&self, message: SharedString, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .top_3()
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .max_w(px(560.))
                    .flex()
                    .items_center()
                    .gap_3()
                    .pl_4()
                    .pr_1()
                    .py_1()
                    .rounded_lg()
                    .bg(rgb(MENU_BG))
                    .shadow_lg()
                    // `min_w_0` lets the text shrink below its full length, so it wraps.
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(rgb(ERROR_TEXT))
                            .child(message),
                    )
                    .child(
                        div()
                            .id("dismiss-error")
                            .flex_none()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_color(rgb(TEXT_MUTED))
                            .hover(|style| style.bg(hover_tint()))
                            .child("Dismiss")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.error = None;
                                cx.notify();
                            })),
                    ),
            )
    }

    fn render_status_bar(&self, window: &Window) -> impl IntoElement {
        let file_label = match (&self.loading, &self.opened) {
            (Some(loading), _) => format!("Opening {loading}…"),
            (None, Some(opened)) if self.saving => {
                format!("Saving {}…", opened.document.file_name())
            }
            (None, Some(opened)) => opened.document.file_name(),
            (None, None) => String::new(),
        };

        div()
            .flex_none()
            .h(STATUS_BAR_HEIGHT)
            .px_3()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .bg(rgb(CHROME_BG))
            .text_xs()
            .text_color(rgb(TEXT_MUTED))
            .child(file_label)
            .when_some(self.opened.as_ref(), |bar, opened| {
                let zoom = opened.viewport.zoom(canvas_area(window));
                bar.child(
                    div()
                        .flex()
                        .gap_4()
                        .child(viewport::zoom_label(zoom))
                        .child(format!(
                            "{} × {} px",
                            opened.document.width(),
                            opened.document.height()
                        )),
                )
            })
    }
}

impl Render for Moxo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.update_checker(window, cx);

        div()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::open_image))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::save_as))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::rotate_clockwise))
            .on_action(cx.listener(Self::rotate_counter_clockwise))
            .on_action(cx.listener(Self::flip_horizontal))
            .on_action(cx.listener(Self::flip_vertical))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::fit_on_screen))
            .on_action(cx.listener(Self::actual_size))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_key_up(cx.listener(Self::on_key_up))
            .size_full()
            .flex()
            .flex_col()
            .font_family("Segoe UI")
            .text_size(px(13.))
            .text_color(rgb(TEXT))
            .child(self.render_menu_bar(cx))
            .child(self.render_canvas(cx))
            .child(self.render_status_bar(window))
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("ctrl-o", OpenImage, None),
            KeyBinding::new("ctrl-s", Save, None),
            KeyBinding::new("ctrl-shift-s", SaveAs, None),
            KeyBinding::new("ctrl-z", Undo, None),
            KeyBinding::new("ctrl-y", Redo, None),
            KeyBinding::new("ctrl-shift-z", Redo, None),
            KeyBinding::new("ctrl-=", ZoomIn, None),
            KeyBinding::new("ctrl-+", ZoomIn, None),
            KeyBinding::new("ctrl--", ZoomOut, None),
            KeyBinding::new("ctrl-0", FitOnScreen, None),
            KeyBinding::new("ctrl-1", ActualSize, None),
        ]);

        let bounds = Bounds::centered(None, size(px(1024.0), px(700.0)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(APP_NAME.into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| Moxo::new(window, cx));
                // Give the view keyboard focus so the shortcuts work straight away.
                window.focus(&view.read(cx).focus_handle);
                view
            },
        )
        .expect("failed to open window");

        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    // A file in a temporary folder, deleted when the test ends (even if it fails).
    struct TempFile(PathBuf);

    impl TempFile {
        fn new(name: &str) -> Self {
            let folder =
                std::env::temp_dir().join(format!("moxo-main-tests-{}", std::process::id()));
            fs::create_dir_all(&folder).unwrap();
            Self(folder.join(name))
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    // The reason the real image loader gives for this file.
    fn loader_reason(file: &TempFile) -> String {
        match image_loader::load(&file.0) {
            Ok(_) => panic!("expected {} to fail to load", file.0.display()),
            Err(reason) => reason,
        }
    }

    #[test]
    fn open_error_message_wording() {
        assert_eq!(
            open_error_message("photo.png", "the file is incomplete or damaged"),
            "Couldn't open photo.png: the file is incomplete or damaged."
        );
    }

    // Real loader errors must come out as one sentence with exactly one full
    // stop. An early version showed "Invalid PNG signature.." because both the
    // loader and this message added one.
    #[test]
    fn real_loader_errors_read_as_one_sentence() {
        let missing = TempFile::new("missing.png");

        let not_an_image = TempFile::new("not an image.png");
        fs::write(&not_an_image.0, "just text").unwrap();

        let too_wide = TempFile::new("too wide.png");
        image::RgbaImage::new(16385, 1).save(&too_wide.0).unwrap();

        for file in [&missing, &not_an_image, &too_wide] {
            let file_name = file.0.file_name().unwrap().to_str().unwrap();
            let reason = loader_reason(file);
            let message = open_error_message(file_name, &reason);

            assert!(
                message.starts_with(&format!("Couldn't open {file_name}: ")),
                "{message}"
            );
            assert!(message.contains(&reason), "{message}");
            assert!(
                message.ends_with('.') && !message.ends_with(".."),
                "{message}"
            );
        }
    }

    #[test]
    fn unusual_file_names_and_reasons_are_kept_exactly() {
        for file_name in ["my photo.final.v2 – kopia.PNG", "bild_åäö.jpg", ".png"] {
            let reason = "it's 17000 × 100 px (too wide); try a smaller one";
            assert_eq!(
                open_error_message(file_name, reason),
                format!("Couldn't open {file_name}: {reason}.")
            );
        }
    }

    #[test]
    fn the_title_marks_unsaved_changes_and_uses_the_app_name() {
        assert_eq!(
            window_title("photo.png", false),
            format!("photo.png - {APP_NAME}")
        );
        assert_eq!(
            window_title("photo.png", true),
            format!("*photo.png - {APP_NAME}")
        );
    }

    // Real save failures must read as one sentence with exactly one full stop.
    #[test]
    fn real_save_errors_read_as_one_sentence() {
        let missing_folder = std::env::temp_dir()
            .join(format!("moxo-main-no-such-folder-{}", std::process::id()))
            .join("photo.png");
        let reasons = [
            save::save(
                &image::RgbaImage::new(1, 1),
                &missing_folder,
                FileFormat::Png,
            )
            .unwrap_err(),
            save::resolve_save_path(std::path::Path::new("photo.gif"), FileFormat::Png)
                .unwrap_err(),
        ];
        for reason in reasons {
            let message = save_error_message("photo.png", &reason);
            assert!(
                message.starts_with("Couldn't save photo.png: "),
                "{message}"
            );
            assert!(message.contains(&reason), "{message}");
            assert!(
                message.ends_with('.') && !message.ends_with(".."),
                "{message}"
            );
        }
    }
}
