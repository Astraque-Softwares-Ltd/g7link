# Performance Budgets

G7 Link treats startup latency, idle resource consumption, UI frame pacing, and bulk transfer throughput as regression-tested engineering budgets. Budgets and Android measurements come from [`docs/PERFORMANCE.md`](../PERFORMANCE.md) (Android on a Samsung SM-S908E, Android 14). The desktop budgets have not been measured on the Linux desktop yet.

## Target Budgets vs Measured Results

### Linux Desktop (`g7link-desktop`)

| Metric | Budget |
|---|---|
| Cold start to first visible frame | `< 800 ms` |
| Cold start (`--minimized` to system tray) | `< 400 ms` |
| Idle CPU (minimized to tray, 60 s window) | `< 0.2%` |
| Idle CPU (window open, connected, 30 s) | `< 0.5%` |
| Idle resident memory (minimized to tray) | `< 50 MB` |
| Idle resident memory (window open, all pages visited) | `< 170 MB` |
| Command Palette (`Ctrl+Alt+Space`) open + filter | `< 30 ms` |
| Phone-to-PC screen mirror glass-to-glass latency | `< 45 ms` |
| PC-to-phone audio streaming latency (PipeWire monitor capture → `NLPA` QUIC datagrams → Android `AudioTrack`) | `< 40 ms` |

### Scale Benchmarks (`g7link-core`)

The scale benchmarks move gigabytes of data and hold timing limits, so they are opt-in rather than part of every CI run. The measurements below are from a release build on 2026-10-14, before the desktop app became Linux-only; they have not been repeated since.

```sh
cargo test -p g7link-core --release --test scale_tests -- --ignored --nocapture
```

| Benchmark (`tests/scale_tests.rs`) | Limit in the test | Measured |
|---|---|---|
| `scale_4gb_file_transfer_with_pause_resume_interruption_and_blake3`: a 4 GiB file between two paired nodes, paused and resumed twice mid-stream, then BLAKE3 verified | Transfer `< 180 s` | **`16.0 s`** transfer, **`1.74 s`** BLAKE3 verify |
| `scale_10000_notifications_in_history_and_search`: 10,000 notifications ingested, a 200-item page listed and searched | List and search `< 100 ms` | **`13.8 ms`** ingest, **`0.15 ms`** list, **`2.43 ms`** search |
| `scale_5000_timeline_entries_and_200_chat_threads`: 5,000 timeline entries and 200 chat threads | Page and thread queries `< 50 ms`, search `< 100 ms` | **`0.89 ms`** page, **`4.60 ms`** search, **`0.37 ms`** threads, **`0.06 ms`** messages |
| `scale_50_large_clips_encrypted_history`: 50 clipboard items of 64 KiB each, encrypted at rest | Reopen, decrypt, list and search `< 250 ms` (release) | **`3.06 s`** to write all 50, **`182 ms`** to reopen and search |

### Android Client (`app.g7link.android`)

| Metric | Budget | Measured | Verification Method |
|---|---|---|---|
| Cold start (`TotalTime` via `am start -W`) | `< 1,000 ms` | **`479 ms - 750 ms`** | `adb shell am start -W -S app.g7link.android/.MainActivity` |
| Warm start (`TotalTime` via `am start -W`) | `< 100 ms` | **`14 ms - 30 ms`** | `adb shell am start -W app.g7link.android/.MainActivity` |
| Memory footprint (`TOTAL PSS`) | `< 110 MB` | **`75.66 MB - 86.48 MB`** | `adb shell dumpsys meminfo app.g7link.android` |
| Home tab scroll jank (`gfxinfo`) | `< 5.0%` | **`0.26%`** (`1 / 382` frames) | `adb shell dumpsys gfxinfo app.g7link.android` |
| Settings tab scroll jank (`gfxinfo`) | `< 5.0%` | **`4.09%`** (`11 / 269` frames, `p95 = 11 ms`) | `adb shell dumpsys gfxinfo app.g7link.android` |
| Idle background CPU (screen off, connected) | `< 0.5%` | **`0.0% - 0.1%`** | `adb shell top -n 1 -b` |

## Key Engineering Optimizations

The budgets above are enforced through architectural choices across the Rust core, Qt desktop client, and Android app:

### 1. Asynchronous Runtime Bootstrap (`desktop/app/src/main.rs`)

The Qt `QGuiApplication` and QML engine initialize on the main thread while `g7link_core::Node::start` (SQLite migration, unsealing the device key with the Secret Service, `iroh` QUIC socket binding, mDNS discovery, and ADB watcher startup) runs concurrently on the background Tokio runtime. The first QML frame paints before network sockets finish binding, and the `NodeBridge` attaches via an asynchronous channel once ready.

### 2. Lazy QML Loading (`Loader` Pattern in `App.qml`)

Rather than instantiating every page at startup, `desktop/app/qml/App.qml` loads the main window only when it is shown, and `MainWindow.qml` loads pages (`MessagesPage`, `CallsPage`, `PhotosPage`, `SettingsPage` and the rest) when they are opened on demand through QML `Loader` components.

### 3. Tray Memory Trimming (`hide_to_tray`)

When the user closes the window to the system tray (`trim_working_set` in `desktop/app/src/bridge/app.rs`):

- Idle caches for Home, the webcam, photos, messages and calls are released.
- `trim_memory_caches` (`desktop/app/cpp/app_helpers.cpp`) drops idle video frames, clears `QPixmapCache`, and runs the QML engine's garbage collection and `trimComponentCache()`, while the background Tokio QUIC node stays responsive.

### 4. Event-Driven Zero-Polling Subsystems

To keep tray idle CPU at **`0.00%`**:

- **Send Queue Coordinator (`core/g7link-core/src/node.rs`)**: Uses `tokio::sync::Notify` (`queue_notify`) plus calculated `tokio::time::sleep_until` deadlines (`Store::next_queue_wake_ms`); when no items are waiting, the task parks indefinitely on `std::future::pending()`.
- **ADB USB Watcher (`core/g7link-adb` & `core/g7link-core/src/usb.rs`)**: Maintains a single long-lived TCP connection to `127.0.0.1:5037` using the event-driven `host:track-devices` stream instead of spawning `adb` on a timer.
- **QML Animations**: Indeterminate spinners and pulse animations in QML bind `running` to item visibility and active window state so Qt's scene graph ticks `0` frames per second when minimized or on another tab.

### 5. Off-UI-Thread Android Service Startup (`MainActivity.kt` & `ConnectionService.kt`)

- `G7LinkNode` creation, Android Keystore AES-GCM unsealing, and SQLite initialization run on `Dispatchers.IO` inside `ConnectionService`.
- Compose lists (`HomeTab`, `SettingsTab`, `SendQueueSection`) pre-compute formatted timestamps and strings outside `LazyColumn` item scopes and supply stable `key` lambdas to avoid layout recomposition jank during flings.
