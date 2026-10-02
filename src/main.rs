// Hide the extra console window when running a release build.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod canvas;
mod file_dialog;
mod image_loader;
mod navigation;
mod viewport;

use std::sync::Arc;

use gpui::{
    Action, App, Application, Bounds, Context, CursorStyle, DispatchPhase, FocusHandle, Hsla,
    KeyBinding, KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, RenderImage, ScrollWheelEvent, SharedString, Size, Task,
    TitlebarOptions, Window, WindowBounds, WindowOptions, actions, deferred, div, hsla, prelude::*,
    px, rgb, size,
};

use navigation::{PanDrag, ScrollAction};
use viewport::Viewport;

// The product's display name, shown in window titles and messages. It's
// provisional: to rename the product, change it here rather than in
// individual strings. (Technical names like the Cargo package stay as they are.)
const APP_NAME: &str = "Moxo";

actions!(moxo, [OpenImage, ZoomIn, ZoomOut, FitOnScreen, ActualSize]);

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

// The image currently on screen.
struct OpenedImage {
    file_name: SharedString,
    image: Arc<RenderImage>,
    width: u32,
    height: u32,
    viewport: Viewport,
}

#[derive(Clone, Copy, PartialEq)]
enum Menu {
    File,
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
        }
    }

    fn open_image(&mut self, _: &OpenImage, window: &mut Window, cx: &mut Context<Self>) {
        self.open_menu = None;
        cx.notify();

        let picked_file = file_dialog::pick_image(window);

        self.open_task = Some(cx.spawn_in(window, async move |this, cx| {
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
            let result = cx
                .background_executor()
                .spawn(async move { image_loader::load(&path) })
                .await;

            this.update_in(cx, |this, window, cx| {
                this.loading = None;
                match result {
                    Ok(loaded) => {
                        // Free the previous image's GPU memory.
                        if let Some(previous) = this.opened.take() {
                            cx.drop_image(previous.image, Some(window));
                        }
                        window.set_window_title(&format!("{file_name} - {APP_NAME}"));
                        this.error = None;
                        this.opened = Some(OpenedImage {
                            file_name,
                            image: loaded.image,
                            width: loaded.width,
                            height: loaded.height,
                            viewport: Viewport::new(loaded.width, loaded.height),
                        });
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
        let item = |label, shortcut, action: Box<dyn Action>, enabled| MenuItem {
            label,
            shortcut,
            action,
            enabled,
        };
        match menu {
            Menu::File => vec![item("Open Image…", "Ctrl+O", Box::new(OpenImage), true)],
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
                let image = opened.image.clone();
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
            (None, Some(opened)) => opened.file_name.to_string(),
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
                        .child(format!("{} × {} px", opened.width, opened.height)),
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
}
