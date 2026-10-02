# Moxo

A lightweight, offline-first image editor for Windows: our own focused take on
Photoshop, with fewer features and less clutter. Written in Rust with GPUI.

## Product principles
- Focused, not bloated: build what everyday editing needs, not all of Photoshop.
- Polished, compact dark interface with its own identity.
- Native, responsive Windows app; works fully offline, no account.
- Straightforward Rust and few, well-chosen dependencies.
- Implement only the task asked. The long-term list (layers, layer styles,
  blending, selections, transforms, painting, text, filters, undo) is
  direction, not scope.

## Architecture (src/)
- `main.rs`: the app's state (open image, loading, errors, menus), actions and
  shortcuts, event wiring, and drawing the window chrome: menu bar, empty
  state, error message and status bar. It's currently the largest file.
- `viewport.rs`: zoom, pan, fit and clamping maths, in physical screen pixels
  (100% = one image pixel per physical screen pixel, at any display scaling).
- `navigation.rs`: turns wheel, touchpad and drag input into pan/zoom.
- `canvas.rs`: canvas position and pointer-to-screen-pixel conversion,
  pixel-snapped image placement, the checkerboard, and painting.
- `image_loader.rs`: decodes PNG/JPEG off the UI thread into GPUI's BGRA format.
- `file_dialog.rs`: native Open dialog (rfd, which runs it on its own thread so
  the window keeps redrawing) plus a GPUI workaround.

Goal: keep `main.rs` a thin entry point. Put logic in plain functions that can
be tested without a window, and move logic out of `main.rs` when it grows.

## GPUI (pinned to 0.2.2)
- Pinned to `gpui = "0.2.2"` because it's the published crates.io release the
  project is built and tested against. Zed's main branch changes daily and its
  newer `gpui_platform` API isn't published, so examples from it won't compile
  here. Use `Application::new()`.
- Check APIs against `~/.cargo/registry/src/*/gpui-0.2.2/`.
- Upgrading GPUI requires approval; propose it only for a concrete limitation.
- Known Windows quirks: `Window::display_handle()` panics (see file_dialog.rs);
  `cx.set_menus()` shows nothing (Moxo draws its own menus); `prompt_for_paths`
  has no file filters; `RenderImage` is BGRA, max 16384 px per side; replaced
  images must be freed with `cx.drop_image`; images always scale smoothly.
- Release builds need `fxc.exe`; `.cargo/config.toml` holds a machine-specific
  path (see README). Don't remove it.

## Commands
`cargo fmt --check`, `cargo clippy --all-targets`, `cargo test`, `cargo check`,
`cargo build`, `cargo build --release`. None of them open a window.
The first debug build takes a few minutes (dependencies are optimised in debug).
Clippy currently reports one known warning (`manual_is_multiple_of` in
`canvas.rs`). Don't fix it, or other unrelated warnings, unless asked.

## Testing policy
- Never control the mouse or keyboard, and don't launch the app (`cargo run`
  or the built `moxo.exe`) without asking first.
- Test logic with unit tests. Covered today: viewport maths, coordinate
  conversion, input handling, the checkerboard and image decoding
  (`viewport.rs`, `navigation.rs`, `canvas.rs`, `image_loader.rs`). Not yet
  tested: `file_dialog.rs` and the app flow in `main.rs`.
- Never weaken or remove tests to avoid GUI automation; replace GUI-dependent
  tests with meaningful non-GUI tests where practical.
- Every bug fix gets a regression test.
- A test that needs a real window must be `#[ignore]` and run only when asked.
- Reports list anything that still needs checking by hand in the app.

## Product name
- "Moxo" is a provisional display name. Anything users see (window titles,
  messages, UI text) must get it from `APP_NAME` in `src/main.rs`; never
  hardcode it in a new string.
- Technical identifiers that contain the name stay as they are: the Cargo
  package and binary `moxo` (so `moxo.exe`), the `Moxo` view type, the `moxo`
  action namespace, file and module names, and the repository. Renaming those
  is a separate, deliberate task.
- To rename the product: change `APP_NAME`, then deliberately update the
  README and CLAUDE.md titles, code comments that name the app, and any
  branding assets or Windows file metadata (icon, version info) once they exist.

## Conventions
- Simple, readable Rust. The main developer knows PHP/JavaScript well and is
  newer to Rust, so briefly explain Rust-specific ideas when they come up,
  comparing them with PHP and JavaScript where that helps (for example:
  ownership and borrowing vs. JS references, traits vs. PHP interfaces,
  `Result` vs. exceptions, Cargo vs. Composer/npm, build scripts).
- Comments, docs, commit messages and UI text: plain English that explains why.
- No new dependency without explaining why and getting approval; prefer crates
  already in the build (`cargo tree`).
- Stay within the task; don't redesign unrelated parts.

## Git
- Never commit or push without explicit approval in the current conversation.
- Keep `target/`, `.idea/` and `.mcp.json` out of the repo.
