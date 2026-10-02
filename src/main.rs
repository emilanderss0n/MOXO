// Hide the extra console window when running a release build.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod file_dialog;
mod image_loader;

use std::sync::Arc;

use gpui::{
    App, Application, Bounds, Context, FocusHandle, Hsla, KeyBinding, KeyDownEvent, MouseButton,
    ObjectFit, RenderImage, SharedString, Task, TitlebarOptions, Window, WindowBounds,
    WindowOptions, actions, deferred, div, hsla, img, prelude::*, px, rgb, size,
};

actions!(moxo, [OpenImage]);

// Colours for the dark theme.
const CHROME_BG: u32 = 0x262626; // menu bar, status bar
const CANVAS_BG: u32 = 0x1b1b1b; // the area around the image
const MENU_BG: u32 = 0x303030;
const TEXT: u32 = 0xdedede;
const TEXT_MUTED: u32 = 0x8e8e8e;
const ERROR_TEXT: u32 = 0xf28b82;

fn hover_tint() -> Hsla {
    hsla(0., 0., 1., 0.07)
}

// The image currently on screen.
struct OpenedImage {
    file_name: SharedString,
    image: Arc<RenderImage>,
    width: u32,
    height: u32,
}

struct Moxo {
    focus_handle: FocusHandle,
    file_menu_open: bool,
    opened: Option<OpenedImage>,
    // Name of the file being decoded right now, if any.
    loading: Option<SharedString>,
    error: Option<SharedString>,
    // The running "pick a file, then decode it" job. Replacing it cancels the old one.
    open_task: Option<Task<()>>,
}

impl Moxo {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            file_menu_open: false,
            opened: None,
            loading: None,
            error: None,
            open_task: None,
        }
    }

    fn open_image(&mut self, _: &OpenImage, window: &mut Window, cx: &mut Context<Self>) {
        self.file_menu_open = false;
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
                        window.set_window_title(&format!("{file_name} - Moxo"));
                        this.error = None;
                        this.opened = Some(OpenedImage {
                            file_name,
                            image: loaded.image,
                            width: loaded.width,
                            height: loaded.height,
                        });
                    }
                    Err(reason) => {
                        this.error = Some(format!("Couldn't open {file_name}: {reason}.").into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" && self.file_menu_open {
            self.file_menu_open = false;
            cx.notify();
        }
    }

    fn render_menu_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let menu_was_open = self.file_menu_open;

        let file_button = div()
            .id("file-menu")
            .px_2()
            .py_1()
            .rounded_md()
            .when(menu_was_open, |button| button.bg(hover_tint()))
            .hover(|style| style.bg(hover_tint()))
            .child("File")
            // Like native menus, open on mouse down. Clicking "File" while the menu is
            // open first triggers the menu's "click outside", so toggle from the state
            // the menu had when this frame was drawn.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.file_menu_open = !menu_was_open;
                    cx.notify();
                }),
            );

        let file_menu = div()
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
                this.file_menu_open = false;
                cx.notify();
            }))
            .child(
                div()
                    .id("open-image")
                    .h(px(30.))
                    .px_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .rounded_md()
                    .hover(|style| style.bg(hover_tint()))
                    .child("Open Image…")
                    .child(div().text_color(rgb(TEXT_MUTED)).child("Ctrl+O"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_image(&OpenImage, window, cx)
                    })),
            );

        div()
            .flex_none()
            .h(px(32.))
            .px_1()
            .flex()
            .items_center()
            .bg(rgb(CHROME_BG))
            .child(
                div()
                    .relative()
                    .child(file_button)
                    // `deferred` draws the menu after the rest of the window, so it sits on top.
                    .when(menu_was_open, |wrapper| wrapper.child(deferred(file_menu))),
            )
    }

    fn render_canvas(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.opened {
            Some(opened) => img(opened.image.clone())
                .size_full()
                // Shrink large images to fit, but never blow small ones up.
                .object_fit(ObjectFit::ScaleDown)
                .into_any_element(),
            None => self.render_empty_state(cx).into_any_element(),
        };

        div()
            .flex_1()
            .min_h_0()
            .relative()
            .p_6()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(CANVAS_BG))
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
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_image(&OpenImage, window, cx)
                    })),
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

    fn render_status_bar(&self) -> impl IntoElement {
        let file_label = match (&self.loading, &self.opened) {
            (Some(loading), _) => format!("Opening {loading}…"),
            (None, Some(opened)) => opened.file_name.to_string(),
            (None, None) => String::new(),
        };

        div()
            .flex_none()
            .h(px(26.))
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
                bar.child(format!("{} × {} px", opened.width, opened.height))
            })
    }
}

impl Render for Moxo {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::open_image))
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .flex()
            .flex_col()
            .font_family("Segoe UI")
            .text_size(px(13.))
            .text_color(rgb(TEXT))
            .child(self.render_menu_bar(cx))
            .child(self.render_canvas(cx))
            .child(self.render_status_bar())
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.bind_keys([KeyBinding::new("ctrl-o", OpenImage, None)]);

        let bounds = Bounds::centered(None, size(px(1024.0), px(700.0)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Moxo".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(Moxo::new);
                // Give the view keyboard focus so Ctrl+O works straight away.
                window.focus(&view.read(cx).focus_handle);
                view
            },
        )
        .expect("failed to open window");

        cx.activate(true);
    });
}
