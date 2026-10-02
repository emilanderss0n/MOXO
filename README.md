# Moxo

A lightweight, offline-first image editor for Windows, written in Rust with [GPUI](https://www.gpui.rs/).

The goal is a focused editor for everyday image editing, not a full Photoshop clone. Early days: right now it can open PNG and JPEG images and zoom and pan around them.

## Controls

| Action | Shortcut |
|---|---|
| Open an image | Ctrl+O |
| Zoom in / out | Ctrl++ / Ctrl+- (main keyboard or numpad), or Ctrl + mouse wheel |
| Fit on screen | Ctrl+0 |
| Actual size (100%, one image pixel per screen pixel) | Ctrl+1 |
| Pan | Mouse wheel (Shift + wheel for sideways), Space + drag, or middle-button drag |

## Building on Windows

You need:

- **Rust** (stable) with the `x86_64-pc-windows-msvc` toolchain, installed via [rustup](https://rustup.rs/).
- **Visual Studio 2022** (Community or Build Tools) with the "Desktop development with C++" workload. This provides the linker and the Windows SDK.

Then:

```
cargo run                    # debug build
cargo run --release          # optimised build
cargo test                   # unit tests
cargo fmt --check            # formatting
cargo clippy --all-targets   # lints
```

RustRover users get the same commands as shared run configurations in `.run/`.

The **first debug build takes a few minutes** (about 3 on the original development machine). `Cargo.toml` tells Cargo to fully optimise the libraries Moxo uses even in debug builds, because unoptimised image decoding is far too slow to work with (a 48-megapixel photo took 17 seconds to open instead of under half a second). Those libraries are only compiled once, so later debug builds are quick.

### Release builds and fxc.exe

Release builds compile GPUI's DirectX shaders ahead of time using `fxc.exe` from the Windows SDK. Debug builds don't need it.

`.cargo/config.toml` sets `GPUI_FXC_PATH` to an **absolute path that only exists on the original development machine** (SDK 10.0.19041). On another computer, a release build fails with `Failed to find fxc.exe` unless one of these is true:

- you edit the path in `.cargo/config.toml` to point at your own `fxc.exe` (look under `C:\Program Files (x86)\Windows Kits\10\bin\<version>\x64\`)
- you set `GPUI_FXC_PATH` as a system environment variable, which takes priority over the config file
- `fxc.exe` is on your `PATH`
- you have Windows SDK **10.0.26100** installed, which GPUI finds on its own

## Project structure

- `src/main.rs`: app state, menus, shortcuts and window chrome (menu bar, empty state, error message, status bar).
- `src/viewport.rs`: zoom, pan, fit and clamping maths, in physical screen pixels. 100% means one image pixel per physical screen pixel at any Windows display scaling.
- `src/navigation.rs`: turns mouse wheel, touchpad and drag input into pan and zoom.
- `src/canvas.rs`: canvas coordinates, pixel-snapped image placement, the transparency checkerboard, and painting.
- `src/image_loader.rs`: decodes PNG and JPEG off the UI thread.
- `src/file_dialog.rs`: the native Open dialog.

Logic lives in plain functions where possible, so it can be unit-tested without opening a window.

## GPUI version

Moxo is pinned to `gpui = "0.2.2"`, the published crates.io release it's built and tested against. Zed's main branch has moved to an unpublished `gpui_platform` API, so examples from it won't compile here. Upgrading is a deliberate decision, not a routine update.

Things to know about GPUI 0.2.2 on Windows:

- Asking a GPUI window for its display handle panics; `file_dialog.rs` works around this.
- `cx.set_menus()` doesn't show a menu bar, so Moxo draws its own.
- Images are stored as BGRA, at most 16384 px per side, and must be freed with `cx.drop_image` when replaced.
- Images are always scaled smoothly, so pixels look soft above 100%.

## Testing

`cargo test` runs unit tests that never open a window. They cover the viewport maths, input handling, canvas geometry, image decoding, the file dialog's file types, and error messages. The window flow itself (opening, menus, keyboard and drag state) is checked by hand in the running app.

## Product name

"Moxo" is a provisional name. Anything users see gets it from `APP_NAME` in `src/main.rs`, so renaming the product starts there. Technical names (the `moxo` package and binary, type and module names, the repository) stay as they are.
