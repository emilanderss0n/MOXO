# Moxo

A lightweight, offline-first image editor for Windows, written in Rust with [GPUI](https://www.gpui.rs/).

Early days: right now it can open PNG and JPEG images and zoom and pan around them.

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
cargo run              # debug build
cargo run --release    # optimised build
cargo test             # unit tests
```

The **first debug build takes a few minutes** (about 3 on the original development machine). `Cargo.toml` tells Cargo to fully optimise the libraries Moxo uses even in debug builds, because unoptimised image decoding is far too slow to work with (a 48-megapixel photo took 17 seconds to open instead of under half a second). Those libraries are only compiled once, so later debug builds are quick.

### Release builds and fxc.exe

Release builds compile GPUI's DirectX shaders ahead of time using `fxc.exe` from the Windows SDK. Debug builds don't need it.

`.cargo/config.toml` sets `GPUI_FXC_PATH` to an **absolute path that only exists on the original development machine** (SDK 10.0.19041). On another computer, a release build fails with `Failed to find fxc.exe` unless one of these is true:

- you edit the path in `.cargo/config.toml` to point at your own `fxc.exe` (look under `C:\Program Files (x86)\Windows Kits\10\bin\<version>\x64\`)
- you set `GPUI_FXC_PATH` as a system environment variable, which takes priority over the config file
- `fxc.exe` is on your `PATH`
- you have Windows SDK **10.0.26100** installed, which GPUI finds on its own
