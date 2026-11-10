# Contributing

This guide summarizes the repository conventions, module header requirements, protocol change rules, and pre-submission checks for contributing to G7 Link. See also the root [`CONTRIBUTING.md`](../../CONTRIBUTING.md) and [`LICENSE`](../../LICENSE).

## License

G7 Link uses three licenses across the repository (see [`LICENSE`](../../LICENSE)):
- **Core libraries** (`core/g7link-protocol`, `core/g7link-core`, `core/g7link-ffi`): [`MPL-2.0`](../../LICENSES/MPL-2.0.txt)
- **Applications and CLI** (`desktop/app`, `desktop/vcam`, `android`, `core/g7link-cli`): [`GPL-3.0-or-later`](../../LICENSES/GPL-3.0-or-later.txt) with an [`app-store exception`](../../LICENSES/APP-STORE-EXCEPTION.md)
- **Protocol specification and documentation** (`docs/`): [`CC BY 4.0`](../../LICENSES/CC-BY-4.0.txt)

Contributions are accepted under the [`Developer Certificate of Origin`](../../DCO.txt).

## SPDX Header Policy

Every Rust (`.rs`), Kotlin (`.kt`), C++ (`.cpp`, `.h`), and QML (`.qml`) source file must start with the appropriate `SPDX-License-Identifier` comment:

```rust
// SPDX-License-Identifier: MPL-2.0
```

or for application and CLI files:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
```

- Verified automatically by [`scripts/check-headers.ps1`](../../scripts/check-headers.ps1) (run with `pwsh`, as CI does).
- Never log message text, phone numbers, contact names, file paths, or clipboard contents (see [`docs/protocol/v0.md`](../protocol/v0.md) section 11).

## Code Style and Static Analysis

### Rust

- **Formatting**: Format all Rust code with `cargo fmt --all`. Configuration lives in [`rustfmt.toml`](../../rustfmt.toml).
- **Lints**: Workspace-wide lints are configured in the root [`Cargo.toml`](../../Cargo.toml) (`unsafe_op_in_unsafe_fn = "warn"`, `dbg_macro = "warn"`, `todo = "warn"`). All code must pass `cargo clippy --workspace --all-targets -- -D warnings` with zero warnings.
- **Error Handling**:
  - Use typed `thiserror` enums (`CoreError`, `FrameError`, `PairError`) across `g7link-protocol` and `g7link-core` public APIs.
  - Never unwrap fallible network, CBOR, file I/O, or SQLite operations on untrusted peer inputs.
- **`unsafe` Blocks**:
  - Avoid `unsafe` in `g7link-protocol` and `g7link-core`.
  - Where FFI or system calls require `unsafe` (such as `desktop/vcam` shared memory, or `desktop/app/src/linux` calls into libc, X11, V4L2 and FFmpeg), keep the `unsafe` block minimal and document the pointer/lifetime invariants.

### Kotlin (Android)

- Follow standard Kotlin conventions (`kotlin.code.style=official` in `android/gradle.properties`).
- Run blocking disk, SQLite, or FFI calls on `Dispatchers.IO`, never on the main UI thread.
- Keep `Compose` list items free of per-frame allocations (such as `SimpleDateFormat` creation inside `LazyColumn` item blocks) to preserve the `< 5%` scroll jank budget.

### QML (Desktop)

- Read colors, spacing, radii, and typography exclusively from the `Theme` singleton (`desktop/app/qml/Theme.qml`, built on the generated `Tokens.qml`) rather than hardcoding hex literals.
- Gate continuous animations (`NumberAnimation`, `RotationAnimator`) on item visibility and active window state so minimized tray mode consumes `0.00%` CPU.

## Adding or Modifying a Capability

When introducing a new cross-device capability or extending an existing wire message:

1. **Specification First (`docs/protocol/`)**: Document the capability ID, stream framing, CBOR message schemas, and error handling in [`docs/protocol/`](../protocol/v0.md) and register the capability in [`docs/protocol/capabilities.md`](../protocol/capabilities.md).
2. **Protocol Constants & Types (`core/g7link-protocol`)**:
   - Add the capability's name next to its message types in `src/messages.rs`, and list it in [`docs/protocol/capabilities.md`](../protocol/capabilities.md).
   - Define the serde-serializable structs in `src/messages.rs` using `#[serde(default)]` / `#[serde(skip_serializing_if = "Option::is_none")]` so wire changes remain backward-compatible.
3. **Authorization Mapping (`core/g7link-core/src/features.rs`)**:
   - Gate it on an existing or new toggle in `DEVICE_TOGGLES` (`Shared::toggle_on` in the handler), and add it to the matrix in `compute`.
   - Decide whether the feature is default-on or high-sensitivity opt-in (`default_enabled: false`), and whether it must be restricted over `ConnectionPath::Relay` (**Connect over mobile data** / `away_mode`).
4. **Core & FFI Plumbing (`core/g7link-core`, `core/g7link-ffi`)**:
   - Implement the handler in `g7link-core` and enforce `self.capabilities.is_allowed(&peer, cap::...)` on both inbound and outbound paths.
   - If Android needs the API, expose it in `core/g7link-ffi/src/lib.rs`. The Android build regenerates the Kotlin bindings (`uniffiBindgen<Variant>` tasks), so `./gradlew assembleDebug` in `android/` picks it up.
5. **Two-Device Integration Test (`core/g7link-core/tests/two_devices.rs`)**:
   - Add an end-to-end test spinning up two in-process `Node` instances over QUIC to verify capability negotiation, happy-path execution, and toggle denial behavior.

## Database Migrations (`core/g7link-core/src/store.rs`)

If a change alters the SQLite schema:

1. Increment `SCHEMA_VERSION` in `core/g7link-core/src/store.rs`.
2. Add an idempotent migration step in `Store::migrate` (using `ALTER TABLE` or `CREATE TABLE IF NOT EXISTS`) so existing user databases upgrade without data loss.
3. Add a unit test in `store.rs` verifying both fresh database creation and migration from the previous version.

## Pre-Submission Checklist

Before opening a pull request, run the checks CI runs (the full list is in
[building and testing](building-and-testing.md#what-ci-runs)). Verify that:
- `cargo fmt --all --check` produces no diff.
- `cargo clippy` with `-D warnings` exits cleanly for the crates you touched.
- `cargo test` passes for those crates.
- `pwsh ./scripts/check-headers.ps1` reports no missing headers.
- `cargo xtask tokens --check` and `cargo xtask strings --check` confirm generated files are in sync.
- `./gradlew testDebugUnitTest lintDebug assembleDebug` in `android/` succeeds when Android or FFI code is touched.
