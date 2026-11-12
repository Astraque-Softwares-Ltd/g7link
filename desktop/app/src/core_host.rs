// SPDX-License-Identifier: GPL-3.0-or-later
//! Runs `g7link-core` beside Qt: a small Tokio runtime owns the [`Node`],
//! core events are folded into the [`Hub`], and Qt objects issue commands
//! through [`spawn`]. The UI starts before the node, which comes up in the
//! background (startup is never blocked on the network).

use crate::i18n::tr;
use std::{
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use g7link_core::{DeviceInfo, DeviceKind, LinkState, Node, NodeConfig, Platform};
use tokio::{
    runtime::Runtime,
    sync::{Notify, broadcast::error::RecvError},
};

use crate::state::{Changes, CoreStatus, Hub};

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub struct CoreHost {
    pub(crate) runtime: Runtime,
    node: OnceLock<Node>,
    /// Signaled once the node is up, or has failed to start.
    started: Notify,
    pub hub: Arc<Hub>,
    pub data_dir: PathBuf,
}

static HOST: OnceLock<CoreHost> = OnceLock::new();
/// Whether the main window is showing, for a node that starts later.
static INTERACTIVE: Mutex<bool> = Mutex::new(false);

/// The running host. Panics if called before [`start`].
pub fn host() -> &'static CoreHost {
    HOST.get().expect("core host is started in main before Qt")
}

/// The running host, once main has started it.
pub fn try_host() -> Option<&'static CoreHost> {
    HOST.get()
}

/// The running host's data directory, if the host has been started.
pub fn try_data_dir() -> Option<&'static std::path::Path> {
    HOST.get().map(|h| h.data_dir.as_path())
}

/// The node, once it has started.
pub fn node() -> Option<Node> {
    HOST.get()?.node.get().cloned()
}

/// The node once it has started, or `None` if it couldn't.
pub async fn wait_for_node() -> Option<Node> {
    let host = host();
    loop {
        // Created before checking, so a signal in between isn't missed.
        let started = host.started.notified();
        if let Some(node) = node() {
            return Some(node);
        }
        if host.hub.read(|s| matches!(s.status, Some(CoreStatus::Failed(_)))) {
            return None;
        }
        started.await;
    }
}

/// Tells the node whether the main window is showing (it checks links more
/// often then), now or once it has started. Only an atomic store in the node,
/// so it's fine on the GUI thread.
pub fn set_interactive(on: bool) {
    let mut interactive = INTERACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    *interactive = on;
    if let Some(node) = node() {
        node.set_interactive(on);
    }
}

/// Runs a command on the core's runtime.
pub fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
    host().runtime.spawn(fut);
}

/// Creates the runtime and starts the node in the background.
pub fn start(data_dir: PathBuf, platform: Arc<dyn Platform>) -> std::io::Result<&'static CoreHost> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("g7link-core")
        .enable_all()
        .build()?;
    let host = CoreHost {
        runtime,
        node: OnceLock::new(),
        started: Notify::new(),
        hub: Hub::new(),
        data_dir: data_dir.clone(),
    };
    if HOST.set(host).is_err() {
        panic!("core host started twice");
    }
    let host = self::host();
    host.hub.update(|s| {
        s.status = Some(CoreStatus::Starting);
        Changes::STATUS
    });
    host.runtime.spawn(run(data_dir, platform));
    Ok(host)
}

/// Computes the PC's advertised capabilities, advertising capture (`mirror.capture`)
/// and input injection (`mirror.input`, `input.inject`) only when enabled for at
/// least one paired device (or actively sharing).
pub fn compute_capabilities(node: Option<&Node>) -> Vec<String> {
    let mut caps = vec![
        "pc.power".into(),
        g7link_core::PHOTOS_SHOW.into(),
        g7link_core::CAMERA_CONTINUITY.into(),
        g7link_core::CALLS_SHOW.into(),
        g7link_core::CONTACTS_SHOW.into(),
        g7link_core::SMS_SHOW.into(),
        g7link_core::TOGGLES_SHOW.into(),
    ];
    {
        caps.push(g7link_core::PC_WAKE.into());
        caps.push("media.control".into());
        caps.push(g7link_core::STORAGE_MOUNT.into());
        caps.push(g7link_core::STORAGE_READ.into());
        caps.push(g7link_core::STORAGE_WRITE.into());
        caps.push(g7link_core::MIRROR_VIEW.into());
        caps.push(g7link_core::MIRROR_LISTEN.into());
        caps.push(g7link_core::DECK_ACTIONS.into());
        caps.push(g7link_core::PC_AUDIO.into());
        caps.push(g7link_core::RECORDER.into());
        {
            if crate::sys::h264::supports_hevc_hardware() {
                caps.push(g7link_core::MIRROR_HEVC.into());
            }
            if crate::sys::vcam::dll_path().is_some() || crate::sys::vcam::is_registered() {
                caps.push(g7link_core::WEBCAM_VIRTUAL.into());
            }
            if crate::sys::window_capture::is_available() && crate::pc_mirror::is_capture_available() {
                caps.push(g7link_core::MEDIA_STREAM.into());
            }
        }
        caps.push(g7link_core::MIC_LISTEN.into());
    }
    let mut pc_mirror_enabled = crate::pc_mirror::is_active();
    let mut remote_input_enabled = false;
    if let Some(node) = node
        && let Ok(devices) = node.paired_devices()
    {
        for d in devices {
            if let Ok(toggles) = node.device_toggles(d.id) {
                for (name, on) in toggles {
                    if on && name == g7link_core::PC_MIRROR_TOGGLE {
                        pc_mirror_enabled = true;
                    }
                    if on && name == "remote_input" {
                        remote_input_enabled = true;
                    }
                }
            }
        }
    }
    if pc_mirror_enabled && crate::pc_mirror::is_capture_available() {
        caps.push(g7link_core::MIRROR_CAPTURE.into());
    }
    let input_available = crate::sys::input::is_available();
    if remote_input_enabled && input_available {
        caps.push(g7link_core::MIRROR_INPUT.into());
        caps.push(g7link_core::INPUT_INJECT.into());
    }
    if crate::sys::vcam::is_registered() {
        caps.push(g7link_core::WEBCAM_ADDON_VCAM.into());
    }
    caps
}

/// Recomputes and broadcasts this PC's advertised capabilities when feature toggles
/// (`pc_mirror`, `remote_input`) or active sessions change.
pub fn refresh_capabilities() {
    let Some(node) = node() else { return };
    spawn(async move {
        let caps = compute_capabilities(Some(&node));
        node.update_power(g7link_core::PowerLevel::Basic, caps).await;
    });
}

async fn run(data_dir: PathBuf, platform: Arc<dyn Platform>) {
    let host = host();
    let wake = crate::sys::wol::query_wake_status();
    let mut config = NodeConfig::new(&data_dir, this_device(), env!("CARGO_PKG_VERSION"));
    config.downloads_dir = Some(downloads_dir());
    // G7LINK_NO_ADB=1 keeps a development copy from reaching phones plugged in over USB,
    // unless G7LINK_ADB_ONLY_SERIAL explicitly restricts ADB to an Android emulator.
    let only_emulator =
        std::env::var("G7LINK_ADB_ONLY_SERIAL").is_ok_and(|s| s.trim().starts_with("emulator-"));
    let adb_disabled = !only_emulator && std::env::var_os("G7LINK_NO_ADB").is_some_and(|v| v == "1");
    config.adb_server = (!adb_disabled).then_some(g7link_core::DEFAULT_ADB_ADDR);
    config.away_mode = crate::settings::Settings::load(&data_dir).away_mode;
    // This PC's own players, for phones (crate::sys::media_sessions).
    config.capabilities = compute_capabilities(None);
    let initial_caps = config.capabilities.clone();
    let node = match Node::start(config, platform).await {
        Ok(node) => node,
        Err(e) => {
            tracing::error!(error = %e, "core failed to start");
            host.hub.update(|s| {
                s.status = Some(CoreStatus::Failed(e.to_string()));
                Changes::STATUS
            });
            host.started.notify_waiters();
            return;
        }
    };
    let post_start_caps = compute_capabilities(Some(&node));
    if post_start_caps != initial_caps {
        node.update_power(g7link_core::PowerLevel::Basic, post_start_caps).await;
    }
    node.set_wake_info(wake.info.clone()).await;
    // Subscribe before reading the initial state so no event is missed.
    let mut events = node.events();
    let devices = node.paired_devices().unwrap_or_else(|e| {
        tracing::warn!(error = %e, "can't list paired devices");
        Vec::new()
    });
    // A device that never connected hasn't told us what it can do; its
    // features stay "unknown" (not "locked") until its first hello.
    let matrices: Vec<_> = devices
        .iter()
        .filter(|d| !matches!(d.link, LinkState::Offline { last_seen: None }))
        .filter_map(|d| node.capabilities(d.id).ok())
        .collect();
    let toggles: Vec<_> =
        devices.iter().filter_map(|d| node.phone_toggles(d.id).map(|t| (d.id, t))).collect();
    let send_queue = node.send_queue(None).unwrap_or_default();
    let status = CoreStatus::Ready { device_id: node.device_id(), name: this_device().name };
    crate::clipboard::apply_history_setting(&node);
    let mut saved_settings = crate::settings::Settings::load(&data_dir);
    let walkthrough_reconciled = saved_settings.reconcile_walkthrough_for_paired_devices(devices.len());
    if walkthrough_reconciled {
        let _ = saved_settings.save(&data_dir);
    }
    let _ = node.set_timeline_retention(g7link_core::TimelineRetention {
        max_days: saved_settings.timeline_retention_days,
        max_entries: g7link_core::DEFAULT_TIMELINE_MAX_ENTRIES,
    });
    let _ = host.node.set(node);
    if let Some(node) = host.node.get() {
        // Under the lock, so a change from the window in between isn't lost.
        let interactive = INTERACTIVE.lock().unwrap_or_else(|e| e.into_inner());
        node.set_interactive(*interactive);
    }
    host.hub.update(|s| {
        let mut changes = s.set_devices(devices);
        for m in matrices {
            changes |= s.set_matrix(m);
        }
        for (id, t) in toggles {
            changes |= s.set_toggles(id, Some(t));
        }
        changes |= s.set_send_queue(send_queue);
        s.status = Some(status);
        s.wake = wake;
        changes |= Changes::STATUS | Changes::CAPABILITIES | Changes::CLIPBOARD | Changes::TIMELINE;
        if walkthrough_reconciled {
            changes |= Changes::PREFS;
        }
        changes
    });
    host.started.notify_waiters();

    host.runtime.spawn(async {
        let mut was_active = false;
        let mut last_sent_at = std::time::Instant::now() - std::time::Duration::from_secs(60);
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            let Some(node) = self::node() else { continue };
            let online_peers: Vec<_> = host.hub.read(|s| {
                s.devices
                    .iter()
                    .filter(|d| matches!(d.link, LinkState::Online { .. }))
                    .map(|d| d.id)
                    .collect()
            });
            if online_peers.is_empty() {
                was_active = false;
                continue;
            }
            let active = crate::sys::input::is_user_active_within_ms(30_000);
            let should_send = active != was_active
                || (active && last_sent_at.elapsed() >= std::time::Duration::from_secs(30));
            if should_send {
                was_active = active;
                last_sent_at = std::time::Instant::now();
                for peer in online_peers {
                    let node = node.clone();
                    tokio::spawn(async move {
                        let _ = node
                            .set_phone_toggle(
                                peer,
                                "pc_active".into(),
                                g7link_core::PhoneToggleValue::Bool(active),
                            )
                            .await;
                    });
                }
            }
        }
    });

    loop {
        match events.recv().await {
            Ok(event) => {
                if matches!(
                    &event,
                    g7link_core::NodeEvent::QueueChanged
                        | g7link_core::NodeEvent::QueueDroppedOnUnpair { .. }
                ) && let Some(node) = self::node()
                {
                    let items = node.send_queue(None).unwrap_or_default();
                    host.hub.update(|s| s.set_send_queue(items));
                }
                if let g7link_core::NodeEvent::QueueDroppedOnUnpair { peer_name, dropped_count, .. } = &event
                    && *dropped_count > 0
                {
                    crate::bridge::app::show_message(tr!(
                        "CoreHost",
                        "Dropped %1 queued item(s) for %2 (device unpaired).",
                        dropped_count,
                        peer_name
                    ));
                }
                crate::notifications::apply(&event);
                crate::media::on_event(&event);
                crate::notifications::update_toasts(&event);
                crate::clipboard::on_event(&event);
                crate::photos::on_event(&event);
                crate::photo_backup::on_event(&event);
                crate::continuity_camera::on_event(&event);
                crate::calls::on_event(&event);
                crate::messages::on_event(&event);
                crate::mirror::on_event(&event);
                crate::pc_mirror::on_event(&event);
                crate::remote::on_event(&event);
                crate::transfers::on_event(&event);
                crate::send_to::on_event(&event);
                crate::battery::on_event(&event);
                crate::update_relay::on_event(&event);
                crate::storage::on_event(&event);
                crate::webcam::on_event(&event);
                crate::phone_mic::on_event(&event);
                crate::automations::on_event(&event);
                crate::bridge::app::on_home_event(&event);
            }
            Err(RecvError::Lagged(missed)) => {
                // Resynchronize what can be re-read; transient events are lost.
                tracing::warn!(missed, "UI fell behind core events; resyncing");
                let devices =
                    host.node.get().map(|n| n.paired_devices().unwrap_or_default()).unwrap_or_default();
                let toggles: Vec<_> = host
                    .node
                    .get()
                    .map(|n| {
                        devices.iter().filter_map(|d| n.phone_toggles(d.id).map(|t| (d.id, t))).collect()
                    })
                    .unwrap_or_default();
                let send_queue =
                    host.node.get().map(|n| n.send_queue(None).unwrap_or_default()).unwrap_or_default();
                host.hub.update(|s| {
                    let mut changes = s.set_devices(devices);
                    for (id, t) in toggles {
                        changes |= s.set_toggles(id, Some(t));
                    }
                    changes |= s.set_send_queue(send_queue);
                    changes
                });
            }
            Err(RecvError::Closed) => return,
        }
    }
}

/// Where files from phones go: `Downloads\G7Link`.
pub fn downloads_dir() -> PathBuf {
    dirs::download_dir().unwrap_or_else(|| host().data_dir.join("received")).join("G7Link")
}

/// How this PC presents itself to phones.
fn this_device() -> DeviceInfo {
    let name = hostname::get().ok().and_then(|h| h.into_string().ok()).unwrap_or_else(|| "PC".into());
    DeviceInfo {
        name,
        kind: if crate::sys::is_laptop() { DeviceKind::Laptop } else { DeviceKind::Desktop },
        os: std::env::consts::OS.into(),
        os_ver: crate::sys::os_version(),
        model: None,
        accent: None,
        screen: None,
        install_id: crate::sys::machine_guid().as_deref().and_then(g7link_core::install_id_from),
    }
}

/// Re-reads physical network adapters and their Wake-on-LAN settings, updates
/// the UI state, and sends updated `pc.wake_info` to connected phones if changed.
pub async fn refresh_wake() {
    let wake = crate::sys::wol::query_wake_status();
    let info = wake.info.clone();
    // With no adapter up (Wi-Fi dropping as the PC goes to sleep, a network
    // change in progress) there's nothing to read, not a PC that can't be
    // woken: phones keep the addresses they have, which they need most then.
    let any_up = !wake.adapters.is_empty();
    host().hub.update(|s| s.set_wake(wake));
    if any_up && let Some(node) = node() {
        node.set_wake_info(info).await;
    }
}

/// Stops the node (telling connected devices) and the runtime.
pub fn shutdown() {
    let Some(host) = HOST.get() else { return };
    if let Some(node) = host.node.get().cloned() {
        let done =
            host.runtime.block_on(async { tokio::time::timeout(SHUTDOWN_TIMEOUT, node.shutdown()).await });
        if done.is_err() {
            tracing::warn!("core shutdown timed out");
        }
    }
}
