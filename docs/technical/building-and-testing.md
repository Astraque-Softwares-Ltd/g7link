# Building and Testing

This page covers the toolchains, build commands and test suites for the G7
Link Linux desktop app and the Android app. CI
([`.github/workflows/ci.yml`](../../.github/workflows/ci.yml)) runs the same
commands on Ubuntu.

## Toolchains

| Component | Version | Where it's set |
|---|---|---|
| **Rust** | `stable`, at least 1.97 (edition 2024) | [`rust-toolchain.toml`](../../rust-toolchain.toml), `rust-version` in [`Cargo.toml`](../../Cargo.toml) |
| **Qt 6** | 6.12 (Qt Quick, Quick Controls 2, Shader Tools, Wayland) | `QT_VERSION` in the workflows |
| **FFmpeg** | `libavcodec` and `libavutil` development files (optional) | found with `pkg-config` by [`desktop/app/build.rs`](../../desktop/app/build.rs) |
| **JDK** | 17 or newer (CI uses 21) | `JAVA_HOME` |
| **Android SDK** | Platform 37 (`compileSdk = 37`, `targetSdk = 37`, `minSdk = 26`) | [`android/app/build.gradle.kts`](../../android/app/build.gradle.kts) |
| **Android NDK** | `30.0.16248370` | `ndkVersion` in `android/app/build.gradle.kts` |
| **`cargo-ndk`** | latest (`cargo install --locked cargo-ndk`) | Used by Gradle to cross-compile `libg7link_ffi.so` |

## Setting up the desktop build

On Ubuntu, CI installs these packages:

```sh
sudo apt-get install --no-install-recommends \
  libgl1-mesa-dev libxkbcommon-dev libvulkan-dev libx11-dev \
  ffmpeg libavcodec-dev libavutil-dev libopenh264-dev qt6-l10n-tools
```

Qt 6.12 comes from [aqtinstall](https://github.com/miurahr/aqtinstall)
(`linux_gcc_64`; the release build adds the `qtshadertools` and
`qtwaylandcompositor` modules). A system Qt 6.12 works too. The build finds
Qt through `qmake` on `PATH`, or through `QMAKE`:

```sh
export QMAKE="$HOME/Qt/6.12.0/gcc_64/bin/qmake"
export PATH="$HOME/Qt/6.12.0/gcc_64/bin:$PATH"
```

With Nix, `nix develop` opens a shell with Rust, Qt, FFmpeg and `QMAKE`
already set ([`flake.nix`](../../flake.nix)).

When `pkg-config` finds `libavcodec` and `libavutil`, the desktop app is built
with its FFmpeg video backend (`linux_ffmpeg_video`). Without them it still
builds, without that backend.

## Building

### Core crates and the CLI

The core crates (`g7link-protocol`, `g7link-core`, `g7link-ffi`,
`g7link-adb`, `g7link-cli`) don't need Qt:

```sh
cargo build -p g7link-protocol -p g7link-core -p g7link-ffi -p g7link-adb -p g7link-cli
cargo run -p g7link-cli -- --help
```

### Desktop app

```sh
cargo build -p g7link-desktop
cargo build --release -p g7link-desktop -p g7link-vcam
```

The app is `target/release/g7link-desktop`. `g7link-vcam`
([`desktop/vcam`](../../desktop/vcam)) is the shared-memory frame transport
between the app and the `v4l2loopback` virtual camera.

### Design tokens, strings and notices

[`tools/xtask`](../../tools/xtask/src/main.rs) generates files that are
checked in:

```sh
cargo xtask tokens            # Tokens.qml and Tokens.kt from docs/design/tokens.json
cargo xtask tokens --check    # fail if they are out of date
cargo xtask strings           # translatable strings the desktop app builds in Rust
cargo xtask strings --check
cargo xtask notices           # THIRD-PARTY-NOTICES.txt
```

### Android app

[`android/app/build.gradle.kts`](../../android/app/build.gradle.kts) builds
the Rust library with `cargo ndk` and generates the UniFFI Kotlin bindings
before compiling Kotlin:

```sh
cd android
./gradlew assembleDebug                          # arm64-v8a and x86_64
./gradlew assembleDebug -Pg7link.abis=arm64-v8a  # one ABI, faster
./gradlew assembleRelease
```

### Linux packages

The scripts in [`desktop/packaging/linux`](../../desktop/packaging/linux)
write to `dist/`. The `.deb`, AppImage and tarball scripts build the release
binary first if it's missing.

```sh
cargo build --release -p g7link-desktop
bash desktop/packaging/linux/build-ffmpeg-backends.sh  # FFmpeg adapters for each supported distribution
desktop/packaging/linux/build-deb.sh       # dist/g7link_<version>_<arch>.deb
desktop/packaging/linux/build-rpm.sh
desktop/packaging/linux/build-appimage.sh  # needs appimagetool on PATH
desktop/packaging/linux/build-tarball.sh   # dist/G7Link-<version>-linux-<arch>.tar.gz
```

[`test-install.sh`](../../desktop/packaging/linux/test-install.sh) installs
and removes each package with `install.sh`, as CI does.

## Tests

### What CI runs

```sh
# Core crates
cargo fmt --all --check
cargo clippy --locked -p g7link-protocol -p g7link-core -p g7link-ffi -p g7link-cli -p g7link-adb -p xtask --all-targets -- -D warnings
cargo test --locked -p g7link-protocol -p g7link-core -p g7link-ffi -p g7link-cli -p g7link-adb -p xtask
cargo xtask tokens --check
cargo xtask strings --check

# Desktop app (needs Qt)
cargo clippy --locked -p g7link-desktop -p g7link-vcam --all-targets -- -D warnings
cargo test --locked -p g7link-desktop -p g7link-vcam
bash desktop/app/tests/linux_video_dispatch_test.sh
bash desktop/app/tests/qt_thread_sync_test.sh
LIBAVCODEC_NO_PKG_CONFIG=1 cargo check --locked -p g7link-desktop --tests

# Packages
shellcheck -s sh desktop/packaging/linux/install.sh
desktop/packaging/linux/test-install.sh

# Licenses and dependencies
pwsh ./scripts/check-headers.ps1
cargo deny check

# Android
cd android && ./gradlew testDebugUnitTest lintDebug assembleDebug
```

`linux_video_dispatch_test.sh` checks that video falls back cleanly when the
FFmpeg adapters can't load; `qt_thread_sync_test.sh` checks waiting on the Qt
thread. The `cargo check` without `pkg-config` makes sure the app still builds
without the FFmpeg backend.

### Targeted suites

```sh
cargo test -p g7link-protocol                     # framing, pairing proofs, video packets
cargo test -p g7link-core --lib                   # store, send queue, USB tunnel, palette, clipboard history
cargo test -p g7link-core --test two_devices      # two nodes over QUIC: pairing, capabilities, transfers
cargo test -p g7link-core --release --test scale_tests -- --ignored  # opt-in scale benchmarks
cargo test -p g7link-ffi
```

QML component tests (Qt Quick Test) live in
[`desktop/app/tests/qml`](../../desktop/app/tests/qml), with stubs for the
Rust singletons in `stubs/`. CI doesn't run them yet.
