# Architecture

G7 Link is built around a shared Rust core library (`g7link-core`) that implements peer discovery, cryptographic identity, QUIC session management, capability negotiation, persistent storage, and media/file framing. Native platform shells on Linux (`desktop/app`) and Android (`android/app`) bind directly to this core and provide OS integration and UI rendering.

## System Topology

```text
+-------------------------------------------------------------------------------+
| Linux PC                                                                      |
|                                                                               |
|  +-------------------------------------------------------------------------+  |
|  | desktop/app (g7link-desktop)                                        |  |
|  |  +---------------------------+   +-----------------------------------+  |  |
|  |  | Qt 6.12 QML UI            |   | Linux Integrations                |  |  |
|  |  | (desktop/app/qml/*.qml)   |   | PipeWire/PulseAudio, FFmpeg, D-Bus|  |  |
|  |  +-------------+-------------+   | portals, X11, uinput, FUSE        |  |  |
|  |                | cxx-qt 0.10     +-----------------+-----------------+  |  |
|  |  +-------------v-----------------------------------v-----------------+  |  |
|  |  | Rust Bridge & Services (desktop/app/src/bridge/, services)        |  |  |
|  |  +-------------+-----------------------------------+-----------------+  |  |
|  |                |                                   | Shared Memory      |  |
|  |                |                                   | NLVC v2            |  |
|  |                |                     +-------------v-----------------+  |  |
|  |                |                     | desktop/vcam                  |  |  |
|  |                |                     | (frames → v4l2loopback)       |  |  |
|  |                |                     +-------------------------------+  |  |
|  |  +-------------v-----------------------------------------------------+  |  |
|  |  | core/g7link-core (incl. adb.rs & usb.rs) + g7link-protocol|  |  |
|  |  +-------------+-------------------+-------------------+-------------+  |  |
|  +----------------|-------------------|-------------------|----------------+  |
+-------------------|-------------------|-------------------|-------------------+
                    | LAN QUIC          | USB ADB TCP       | Relay QUIC
                    | (UDP + mDNS)      | (127.0.0.1:53319) | (away_mode)
+-------------------v-------------------v-------------------v-------------------+
| Android Device                                                                |
|                                                                               |
|  +-------------------------------------------------------------------------+  |
|  | android/app (app.g7link.android)                                    |  |
|  |  +-------------------------------------------------------------------+  |  |
|  |  | core/g7link-ffi (UniFFI 0.32 → libg7link_ffi.so)          |  |  |
|  |  | embeds core/g7link-core + core/g7link-protocol            |  |  |
|  |  +-------------+-----------------------------------+-----------------+  |  |
|  |                | Kotlin Callbacks / Events         | Loopback TCP      |  |
|  |  +-------------v-----------------+   +-------------v-----------------+  |  |
|  |  | Jetpack Compose UI &          |   | Elevated Helper (app_process) |  |  |
|  |  | ConnectionService (Foreground)|   | InputServer / AppDisplay      |  |  |
|  |  | NotificationListener, Camera2 |   | (UID 2000 shell via ADB TLS)  |  |  |
|  |  +-------------------------------+   +-------------------------------+  |  |
|  +-------------------------------------------------------------------------+  |
+-------------------------------------------------------------------------------+
```

## Workspace Crates and Components

### 1. `core/g7link-protocol`

Pure wire-protocol crate with no runtime IO or platform dependencies beyond `tokio::io::{AsyncRead, AsyncWrite}` for stream framing.

- **Envelope and Framing (`src/frame.rs`, `src/messages.rs`)**: Implements unsigned LEB128 length-prefixed CBOR `Envelope` encoding (`ciborium`), with a 1 MiB frame ceiling (`MAX_FRAME_LEN = 1,048,576` bytes).
- **Capabilities (`src/messages.rs`)**: Each service's capability names (`files.transfer`, `sms.read`, `mirror.capture` and so on) live with its message types. [`docs/protocol/capabilities.md`](../protocol/capabilities.md) lists them all.
- **Pairing Primitives (`src/pairing.rs`)**: Defines the `g7link://pair` URI parser/serializer, `ALPN_PAIR` (`"g7link-pair/0"`), `ALPN_SESSION` (`"g7link/0"`), QR HMAC proof calculation, and 6-digit Short Authentication String (SAS) derivation.
- **Video Packetizer (`src/video.rs`)**: Encodes and decodes real-time video packets (`VideoKind::Config`, `VideoKind::Frame`, `VideoKind::Keyframe`) with microsecond timestamps and an 8 MiB packet limit (`MAX_VIDEO_PACKET`).

### 2. `core/g7link-core`

The central networking, USB/ADB transport, persistence, and feature runtime.

- **Node and Session Engine (`src/node.rs`, `src/session.rs`)**: Manages the `iroh::Endpoint`, accepts incoming `ALPN_PAIR` and `ALPN_SESSION` connections, runs the `hello` / `hello.update` capability handshake, maintains per-peer connection state, and multiplexes control messages, bulk streams, and QUIC datagrams.
- **Zero-Shell-Out ADB & USB Tunnel (`core/g7link-adb`, `src/usb.rs`)**: Speaks the ADB host protocol directly over TCP (`127.0.0.1:5037`) to track attached USB Android devices via `host:track-devices` without spawning `adb` subprocesses per poll, configures port forwards (`host-serial:<serial>:forward:tcp:0;tcp:53319`), and opens a TCP-carried UDP tunnel (`NLQU` header) so `iroh` QUIC datagrams traverse the USB cable with full TLS 1.3 encryption and congestion control.
- **Persistence (`src/store.rs`)**: Embedded SQLite database (`g7link.db`, schema version 7) storing paired devices, per-device capability toggles, key-value settings, unified timeline entries, `MessagingStyle` chat threads/messages, and the persistent FIFO send queue (`send_queue`).
- **Resumable Transfers (`src/transfer.rs`)**: Implements `files.v0` multi-file transfers with BLAKE3 integrity verification (`blake3`), `.part` byte-offset resume, pause/resume/cancel semantics, and automatic queueing when a peer is offline or mid-disconnect.
- **Perceptual Color Engine (`src/palette.rs`)**: Extracts Material You HCT + OKLab 15-role light and dark color palettes (`ThemeScheme`) from album cover art.
- **Encrypted Clipboard Vault (`src/clipboard_history.rs`)**: Stores recent text and image clips in encrypted files (`clipboard_history.enc` with `NLCB\x01` header and `clipboard_images/<id>.enc` with `NLCI\x01` header) sealed directly with the platform `KeyProtector` (AES-256-GCM under a Secret Service key on Linux, Android Keystore AES-256-GCM on Android).
- **Interoperability (`src/localsend.rs`, `src/handoff.rs`)**: Implements LocalSend v2 HTTP/HTTPS discovery and file transfer compatibility (`UDP/TCP 53317`) and cross-device reading/document continuation (`handoff`).

### 3. `core/g7link-ffi`

UniFFI `0.32` bridge (`src/lib.rs`) compiled via `cargo-ndk` into `libg7link_ffi.so` for `arm64-v8a` and `x86_64`.

- Wraps `g7link_core::Node` in an FFI-safe `G7LinkNode` handle.
- Marshals async Rust events into Kotlin `EventCallback` interfaces (`NodeEvent`) and exposes synchronous/asynchronous methods for pairing, messaging, file transfers, send queue inspection, media sessions, and stream creation.
- Accepts a Kotlin-implemented `ForeignKeyProtector` so Android's `KeystoreKeyProtector` (AES-256-GCM inside `AndroidKeyStore`, hardware-backed when supported by the device) seals `identity.key` and `clipboard_history.enc` inside Rust.

### 4. `core/g7link-cli` (`g7link-cli`)

Command-line binary (`g7link`) in `core/g7link-cli/` for headless operation, scripted integration testing, `notify-when` task completion alerts, and diagnostic commands (`id`, `pair`, `qr-host`, `send`, `status`).

### 5. `desktop/app` (`g7link-desktop`)

The Linux desktop client, combining a Rust service host with a Qt 6.12 Quick/QML user interface bridged via `cxx-qt` `0.10`.

- **Qt Bridges (`desktop/app/src/bridge/`)**: One bridge per area (`app.rs`, `devices.rs`, `messages.rs`, `calls.rs`, `photos.rs`, `media.rs`, `mirror.rs`, `transfers.rs`, `prefs.rs` and others) exposes state and actions to QML. Design tokens come from `qml/Tokens.qml` and `qml/Theme.qml`.
- **Platform Modules (`desktop/app/src/linux/`)**: Linux integration: audio (PipeWire, PulseAudio), clipboard (Wayland and X11), input (`/dev/uinput`, the RemoteDesktop portal, XTest), tray and global shortcuts, desktop notifications, MPRIS, video (FFmpeg, OpenH264), the `v4l2loopback` camera, the FUSE storage mount and so on.
- **Shared Desktop Modules (`desktop/app/src/`)**:
  - `mirror.rs`: Phone screen mirroring and app windows, with hardware video decoding.
  - `pc_audio_stream.rs`, `phone_sound.rs` and `phone_mic.rs`: PC sound to the phone, the phone's sound on the PC, and the phone as a microphone.
  - `pc_mirror.rs` and `remote.rs`: Sharing the PC's screen with a phone (the ScreenCast portal with PipeWire, or X11), and input from a phone (`/dev/uinput`, the RemoteDesktop portal or XTest).
  - `media.rs` and `linux/media_sessions.rs`: Now Playing metadata and transport controls over MPRIS, in both directions.
  - `linux/ble_beacon.rs`: Bluetooth LE beacon through BlueZ, for waking the phone's connection.
  - `local_api.rs`: Opt-in localhost HTTP/SSE server (`127.0.0.1:53320`) for the CLI and local scripts.
  - `linux/share_target.rs` and `send_to.rs`: Files, links and text shared to G7 Link from the desktop entry or the command line, and files sent from a Linux file manager's context menu.
  - `updater.rs` and `feed.rs`: Update checks against the channel feed (see the [security model](security-model.md#automatic-updates-and-release-verification)), SHA-256 verified downloads, and the package for the kind of install.

### 6. `desktop/vcam` (`g7link-vcam`)

The shared-memory frame transport for the virtual camera. The desktop app (`linux/vcam.rs`) reads frames from it and writes them as YUYV to a `v4l2loopback` device named `G7 Link Webcam`, which video apps see as a camera.

- The mapping is a file in `$XDG_RUNTIME_DIR` (or `/dev/shm`) with `0600` permissions, `vcam_frame_v2` or `vcam_frame_v2_local` (`desktop/vcam/src/shm.rs`).
- Supports `RGB32` and `NV12` frames up to `3840x2160` (`MAX_WIDTH = 3840`, `MAX_HEIGHT = 2160`, `MAX_FRAME_BYTES = 33,177,600`) behind a `SharedHeader` (signature `0x4E4C5643` / `"NLVC"`, `VERSION = 1`).
- Writers update payload bytes between odd and even sequence numbers (`seq`), so readers get whole frames without cross-process mutexes.

### 7. `android/app` (`app.g7link.android`)

The Android application written in Kotlin with Jetpack Compose (`compileSdk = 37`, `targetSdk = 37`, `minSdk = 26`).

- **Foreground Service (`service/ConnectionService.kt`)**: Hosts the `G7LinkNode` singleton, maintains wake/Wi-Fi locks as needed, and coordinates the feature handlers, one package per area (`sms`, `calls`, `contacts`, `media`, `photos`, `storage`, `recorder`, `toggles`, `webcam`, `mirror`).
- **Notification Listener (`notifications/NotificationListener.kt`)**: Implements `NotificationListenerService` to stream active notifications, `MessagingStyle` conversation threads, inline replies, and action invocations.
- **Elevated Helper (`elevated/Elevated.kt`, `elevated/InputServer.kt`, `elevated/AppDisplay.kt`, `elevated/LoopbackAdb.kt`, `elevated/ShizukuShell.kt`)**: An optional privileged helper process launched via `app_process` (`app.g7link.android.elevated.InputServer <token>`) under `shell` (`UID 2000` via on-device Wireless Debugging TLS pairing or USB ADB). It binds a loopback TCP socket (`127.0.0.1:<ephemeral_port>`) and requires a 24-byte (48-hex-char) per-session `SecureRandom` token on the first line (verified in constant time with `MessageDigest.isEqual`), injects touch/key/scroll events through `android.hardware.input.IInputManager` and `/dev/uhid`, and creates virtual displays (`DisplayManager.createVirtualDisplay` in `AppDisplay.kt`) for per-app window streaming (`mirror.apps.v0`).

## Discovery and Transport Architecture

G7 Link uses `iroh` (`1.3`) to run a single QUIC + TLS 1.3 endpoint per device. Every paired connection is classified into one of three `ConnectionPath` variants (`core/g7link-core/src/node.rs`):

| Path | Discovery Mechanism | Transport Path | Latency & Bandwidth Profile |
|---|---|---|---|
| `ConnectionPath::Lan` | mDNS (`iroh-mdns-address-lookup`, service name `"g7link"`; on PCs on every LAN interface, through a patched copy in `third_party/`) + cached direct socket addresses from `peers.last_addrs` | Direct UDP QUIC between LAN IPv4/IPv6 sockets | Lowest latency, full Wi-Fi / Ethernet throughput |
| `ConnectionPath::Usb` | `core/g7link-adb` device tracker (`127.0.0.1:5037`) forwarding to `127.0.0.1:53319` (`DEFAULT_USB_PORT`) | QUIC UDP datagrams framed over TCP (`NLQU` header in `src/usb.rs`) via `adb forward` | Zero Wi-Fi dependency, deterministic wired latency |
| `ConnectionPath::Relay` | Opt-in **Connect over mobile data** (`away_mode = true`) enabling `n0_dns` Pkarr lookup and `RelayMode::Default` | End-to-end TLS 1.3 encrypted QUIC relayed via `iroh` DERP/relay servers | Works across cellular and different networks; subject to relay capability guards |

### QUIC Transport Parameters

Configured in `core/g7link-core/src/node.rs`:

- `KEEP_ALIVE_INTERVAL`: `1,000 ms` (1 second)
- `IDLE_TIMEOUT`: `8s` (detects network drops and triggers path failover within seconds)
- `STREAM_WINDOW`: `16 MiB` per stream
- `CONNECTION_WINDOW`: `32 MiB` per connection

### Automatic Path Failover and Deduping

When both USB and Wi-Fi are available, or when a phone transitions between Wi-Fi and a **Connect over mobile data** (`away_mode`) relay path, both peers may attempt simultaneous connections. `Node` resolves concurrent dials deterministically and updates the active session handle in place so upper-layer features (`Timeline`, `SendQueue`, `Media`, `Notifications`) continue operating against the peer's `DeviceId`.

## Perceptual OKLab Color Engine

Implemented in `core/g7link-core/src/palette.rs`, the color engine derives cohesive light and dark UI palettes (`ThemeScheme`) from album artwork (`extract_theme_from_encoded_image`, `extract_theme_from_rgba`).

```text
Encoded Image / RGBA Buffer
  │
  ├─► 1. Border Trim (trim_borders: strips up to 22% uniform/black/white letterbox bars)
  │
  ├─► 2. Area-Box Downsample (downsample_rgba: max 112x112 = 12,544 pixels)
  │
  ├─► 3. Alpha & Near-Black Filter (ignores alpha < 128 and CIE L* < 1.5)
  │
  ├─► 4. Celebi Quantization in CIE L*a*b* (QuantizerCelebi, up to 128 clusters)
  │
  ├─► 5. Achromatic Check & 360-deg Hue Neighborhood Scoring
  │      (weights population^0.38 * chroma * mid-tone lightness; penalizes skin-tone bands)
  │
  ├─► 6. OKLab Centroid Seed Calculation (sRGB → linear RGB → LMS → OKLab mean)
  │
  └─► 7. HCT TonalPalette Generation + WCAG AA/AAA Contrast Enforcement
         Produces 15-role dark & light RolePalette (cached in 64-entry LRU)
```

### Pipeline Stages

1. **Letterbox and Border Trimming (`trim_borders`)**: Scans inward from all four edges (up to 22% of width/height) and removes rows/columns that are near-black, near-white, or low-variance solid frames so letterboxed video thumbnails do not skew the dominant color.
2. **Area-Box Downsampling (`downsample_rgba` / `downsample_rgb`)**: Averages pixel blocks into at most `112x112` (`MAX_SAMPLE_SIDE = 112`) pixels, suppressing compression noise while preserving proportional color area.
3. **Celebi Clustering (`QuantizerCelebi`)**: Quantizes filtered pixels in CIE L\*a\*b\* space into at most 128 clusters (`QUANTIZE_COLORS = 128`).
4. **Monochrome Detection (`is_mostly_achromatic`)**: If at least 92% of non-dark pixels have HCT chroma `< 8.0` and no vibrant accent exists, the engine selects the neutral zinc seed (`ACHROMATIC_SEED_RGB = 0x5E5E62`) to avoid inventing false color casts on black-and-white covers.
5. **Hue Family Scoring (`select_perceptual_seed`)**: Evaluates each candidate cluster across a 15-degree Gaussian hue neighborhood, weighting `population_ratio^0.38`, HCT chroma peak (`48.0`), mid-tone lightness preference, and a skin-tone de-bias factor (`35..52` deg hue with moderate chroma) so background/graphic art beats portrait skin tones.
6. **OKLab Centroid (`oklab_centroid_rgb`)**: Computes the population-weighted mean of all clusters within 25 degrees of the winning hue in perceptual OKLab space (`srgb_to_oklab` → `oklab_to_srgb`), preventing muddy RGB averaging.
7. **Tonal Palette and Contrast Enforcement (`scheme_from_seed`, `enforce_contrast`)**: Builds six HCT `TonalPalette` ramps (`primary`, `secondary`, `tertiary`, `neutral`, `neutral_variant`, `error`) and derives 15 semantic tokens (`surface`, `surface_container`, `surface_bright`, `primary`, `on_primary`, `primary_container`, `on_primary_container`, `secondary`, `on_secondary`, `secondary_container`, `tertiary`, `on_tertiary`, `on_surface`, `on_surface_variant`, `outline`) with guaranteed WCAG contrast ratios (`>= 4.5:1` for primary/surface text, `>= 3.0:1` for muted text) and a 64-entry FNV-1a hashed LRU cache (`CACHE_CAPACITY = 64`).
