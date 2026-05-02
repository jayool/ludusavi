# Installation

## Requirements

* Windows, Linux, or Mac.
* For best GUI performance, a system with DirectX, Vulkan, or Metal
  support. On systems without one of those, set the `ICED_BACKEND`
  environment variable to `tiny-skia` to fall back to software rendering.

## Methods

Ludusavi Sync is distributed as a portable executable. Drop it anywhere
on your system and run it.

If you want to build from source, you'll need [Rust](https://www.rust-lang.org)
and (on Linux) the GUI development packages, e.g. on Ubuntu:

```
sudo apt-get install -y gcc cmake libx11-dev libxcb-composite0-dev \
    libfreetype6-dev libexpat1-dev libfontconfig1-dev libgtk-3-dev
```

Then `cargo build --release` produces both the GUI binary (`ludusavi`)
and the daemon binary (`ludusavi-daemon`).

## First-run notes

* **Windows:** the OS may show a "Windows protected your PC" popup
  because the binary is not signed by a recognised publisher. Click
  "more info" → "run anyway".
* **Mac:** Mac may show a "can't be opened because it is from an
  unidentified developer" popup. See
  [Apple's instructions](https://support.apple.com/en-us/102445)
  for how to allow the application to run.
