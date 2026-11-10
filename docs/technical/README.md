# G7 Link Technical Documentation

This directory contains engineering documentation for contributors, security reviewers, and systems engineers working on G7 Link. It covers the multi-crate Rust workspace, the Linux Qt 6 + `cxx-qt` desktop application, the Android Jetpack Compose + UniFFI client, the QUIC + TLS 1.3 protocol, the SQLite persistence layer, and the performance verification suite.

For end-user setup and feature guides, see [docs/guide/README.md](../guide/README.md).

## Contents

| Document | Scope |
|---|---|
| [Architecture](architecture.md) | Crate layout, runtime topology, discovery and transport (`Lan`, `Usb`, `Relay`), Android elevated helper, `v4l2loopback` virtual camera transport, and perceptual OKLab color engine |
| [Security Model](security-model.md) | Ed25519 device identity, key protection at rest (AES-256-GCM under a Secret Service key on Linux, Android Keystore AES-256-GCM), QR and 6-digit pairing protocols, `CapabilityMatrix`, consent gates, update checks, and localhost HTTP API isolation |
| [Protocol Overview](protocol-overview.md) | `g7link/0` wire framing (LEB128 + CBOR), QUIC stream taxonomy, control handshake, resumable BLAKE3 file transfers, real-time media framing, and index of the 18 specification files in [`docs/protocol/`](../protocol/v0.md) |
| [Storage and Retention](storage-and-retention.md) | SQLite schema v8 (`core/g7link-core/src/store.rs`), timeline and `MessagingStyle` conversation caches, persistent FIFO send queue (`send_queue`), and `KeyProtector`-encrypted clipboard history |
| [Performance Budgets](performance-budgets.md) | Startup, memory, CPU, transport, and latency targets from [`docs/PERFORMANCE.md`](../PERFORMANCE.md), measured benchmarks, and key root-cause engineering optimizations |
| [Building and Testing](building-and-testing.md) | Pinned toolchains (`Rust 1.97+`, `Qt 6.12`, `Android compileSdk 37 / minSdk 26`, `JDK 17+`), `cargo xtask` automation, Linux packaging scripts, and the test suites CI runs |
| [Releasing](releasing.md) | Versioning, the tag-driven release workflow, Android signing, the Linux packages, and how both apps update themselves |
| [Contributing](contributing.md) | Coding conventions, licensing (`MPL-2.0`, `GPL-3.0-or-later`, `CC BY 4.0`), module header policy, protocol evolution rules, and pre-submission checks |

## Repository Map

```text
g7link/
├── Cargo.toml                  Workspace manifest and shared dependency versions
├── rust-toolchain.toml         Pinned stable Rust toolchain (1.97+ MSRV, 2024 edition)
├── flake.nix                   Nix package and development shell
├── core/
│   ├── g7link-protocol/        Wire framing, CBOR envelopes, capability constants, SAS derivation
│   ├── g7link-core/            QUIC node, session engine, SQLite store, send queue, keyring, OKLab palette
│   ├── g7link-adb/             ADB host protocol client for the USB tunnel
│   ├── g7link-ffi/             UniFFI 0.32 bindings exposing g7link-core to Kotlin
│   └── g7link-cli/             Headless CLI (g7link) for pairing, file transfer, notify-when, and testing
├── desktop/
│   ├── app/                    Linux desktop app (Rust + cxx-qt 0.10 bridges + Qt 6.12 QML)
│   ├── vcam/                   Shared-memory frame transport for the v4l2loopback virtual camera
│   └── packaging/linux/        .deb, .rpm, AppImage and tarball build scripts, install.sh, desktop entry
├── android/
│   └── app/                    Android client (Kotlin, Jetpack Compose, foreground service, elevated helper)
├── tools/
│   └── xtask/                  Design token, string and third-party notice generators
├── scripts/                    License header check (check-headers.ps1)
└── docs/
    ├── guide/                  User-facing setup and feature documentation
    ├── technical/              Engineering architecture, security, storage, and build reference
    ├── protocol/               Normative wire protocol specifications
    ├── adr/                    Architecture decision records
    ├── PERFORMANCE.md          Measured performance report and verification commands
    └── SECURITY-REVIEW.md      Security audit findings and hardening verification
```

## Related Specification Directories

- [`docs/protocol/`](../protocol/v0.md): Normative wire-level specifications for `g7link/0` and `g7link-pair/0`, plus per-capability message schemas.
- [`docs/adr/`](../adr/README.md): Architecture decision records covering the shared Rust core (`0002`), `iroh` QUIC transport (`0003`), CBOR wire framing (`0004`), the Qt Quick UI stack (`0005`), Kotlin + Jetpack Compose on Android (`0006`), licensing (`0007`), and platform distribution (`0008`).
- [`docs/architecture/`](../architecture/capabilities.md): Capability matrix reference and core Rust API design notes.
- [`docs/design/`](../design/README.md): Visual design system tokens, typography, and motion guidelines.
