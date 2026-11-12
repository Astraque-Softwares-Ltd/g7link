// SPDX-License-Identifier: GPL-3.0-or-later
//! Phone notifications on this PC: app icons are saved for QML and toasts,
//! notifications become desktop notifications (removed again when the phone clears
//! them), and what the user does with a toast or the feed goes back to the
//! phone.

use crate::i18n::tr;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime},
};

use g7link_core::{DeviceId, Error, NodeEvent, Notification};

use crate::{
    core_host, icons,
    state::{AppRule, Changes},
    sys::toast::{self, Toast, ToastEvent},
};

/// Local toast action ID for copying a detected one-time code.
pub const ACTION_COPY_OTP: &str = "copy_otp";

/// Whether incoming one-time codes are copied to the PC clipboard automatically.
static AUTO_COPY_OTP: AtomicBool = AtomicBool::new(false);

/// Whether phone Do Not Disturb quiets phone notification pop-ups on the PC.
static SYNC_DND: AtomicBool = AtomicBool::new(false);
/// Pop-ups are paused for now, from the tray, Home or the command palette.
static PAUSED: AtomicBool = AtomicBool::new(false);
/// No pop-ups while this PC is locked.
static QUIET_WHEN_LOCKED: AtomicBool = AtomicBool::new(false);
static HOLD_WHEN_LOCKED: AtomicBool = AtomicBool::new(false);
/// How often the lock is looked at, for the phone to stop within moments of it.
const LOCK_LOOK_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// Keys that have a toast, per device, so a snapshot can remove the toasts
/// of notifications that went away meanwhile.
static TOASTED: Mutex<Option<HashMap<DeviceId, HashSet<String>>>> = Mutex::new(None);

/// Keys of live notifications that already have a toast with a `<progress>` bar,
/// so subsequent updates call [`toast::update_progress`] in place instead of
/// popping up a new toast.
static LIVE_TOASTS: Mutex<Option<HashSet<(DeviceId, String)>>> = Mutex::new(None);

/// Detected OTP codes by `(device, notification_key)` for active toasts, so
/// the code never needs to be placed in notification action arguments.
static OTP_CODES: Mutex<Option<HashMap<(DeviceId, String), String>>> = Mutex::new(None);

/// Last auto-copied OTP and when it was copied, to avoid copying the same code
/// twice when both a notification and an SMS sync event arrive for one text.
static LAST_AUTO_OTP: Mutex<Option<(String, Instant)>> = Mutex::new(None);

fn toasted<T>(f: impl FnOnce(&mut HashMap<DeviceId, HashSet<String>>) -> T) -> T {
    f(TOASTED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new))
}

fn live_toasts<T>(f: impl FnOnce(&mut HashSet<(DeviceId, String)>) -> T) -> T {
    f(LIVE_TOASTS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashSet::new))
}

fn otp_codes<T>(f: impl FnOnce(&mut HashMap<(DeviceId, String), String>) -> T) -> T {
    f(OTP_CODES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new))
}

pub fn set_auto_copy_otp(on: bool) {
    AUTO_COPY_OTP.store(on, Ordering::Relaxed);
}

pub fn auto_copy_otp_enabled() -> bool {
    AUTO_COPY_OTP.load(Ordering::Relaxed)
}

pub fn paused() -> bool {
    PAUSED.load(Ordering::Relaxed)
}

pub fn set_paused(on: bool) {
    PAUSED.store(on, Ordering::Relaxed);
}

pub fn set_hold_when_locked(on: bool) {
    HOLD_WHEN_LOCKED.store(on, Ordering::Relaxed);
}

/// What a look at the lock changes: `Some` when phones need telling.
fn hold_change(holding: bool, wanted: bool, locked: impl FnOnce() -> bool) -> Option<bool> {
    let hold = wanted && locked();
    (hold != holding).then_some(hold)
}

/// Keeps phones' notifications off this PC while it's locked, if the user asked.
pub fn start_lock_watch() {
    core_host::spawn(async {
        let Some(node) = core_host::wait_for_node().await else { return };
        let mut holding = false;
        let mut look = tokio::time::interval(LOCK_LOOK_EVERY);
        loop {
            look.tick().await;
            let wanted = HOLD_WHEN_LOCKED.load(Ordering::Relaxed);
            if let Some(hold) = hold_change(holding, wanted, crate::sys::session_locked) {
                node.hold_notifications(hold).await;
                holding = hold;
            }
        }
    });
}

pub fn set_quiet_when_locked(on: bool) {
    QUIET_WHEN_LOCKED.store(on, Ordering::Relaxed);
}

/// Whether a notification stays in the feed without popping up. `locked` is
/// asked only when it matters: finding out can take a moment.
fn stays_quiet(paused: bool, quiet_when_locked: bool, locked: impl FnOnce() -> bool) -> bool {
    paused || (quiet_when_locked && locked())
}

pub fn set_sync_dnd(on: bool) {
    SYNC_DND.store(on, Ordering::Relaxed);
    if on {
        let dnd_devices: Vec<DeviceId> = core_host::host()
            .hub
            .read(|s| s.toggles.iter().filter(|(_, t)| t.dnd).map(|(id, _)| *id).collect());
        for device in dnd_devices {
            quiet_device_toasts(device);
        }
    }
}

fn quiet_device_toasts(device: DeviceId) {
    toasted(|t| t.remove(&device));
    live_toasts(|l| l.retain(|(d, _)| *d != device));
    toast::remove_device(&device.to_string());
}

/// Extracts a one-time code from a phone notification, if present.
///
/// Checks the notification body first. If the keyword is in the title and the
/// code is in the body (e.g. title `"Verification code"`, body `"482913"`),
/// matches the combined text only when the extracted digits come from the body
/// (so a phone number in the title is never mistaken for a code).
pub fn notification_otp(n: &Notification) -> Option<String> {
    let text = n.text.as_deref().unwrap_or_default();
    if let Some(code) = g7link_core::otp::one_time_code(text) {
        return Some(code);
    }
    if let Some(title) = n.title.as_deref().filter(|t| !t.trim().is_empty())
        && !text.trim().is_empty()
    {
        let combined = format!("{title}: {text}");
        if let Some(code) = g7link_core::otp::one_time_code(&combined)
            && (text.contains(&code)
                || (code.len() == 6 && text.contains(&format!("{}-{}", &code[..3], &code[3..]))))
        {
            return Some(code);
        }
    }
    None
}

/// Copies a one-time code to the PC clipboard without syncing it back to any
/// phone or keeping it in any clipboard history (ours or the desktop's). Never
/// logs the code.
pub fn copy_otp(code: &str) {
    let trimmed = code.trim();
    if trimmed.is_empty() {
        return;
    }
    if crate::sys::clipboard::write_private(trimmed).is_ok() {
        crate::bridge::app::show_message(tr!("Notifications", "Code copied"));
    }
}

/// Automatically copies `code` when the "Copy codes automatically" setting is
/// enabled, deduplicating identical codes within 10 seconds.
pub fn maybe_auto_copy_otp(code: &str) {
    if !AUTO_COPY_OTP.load(Ordering::Relaxed) {
        return;
    }
    let trimmed = code.trim();
    if trimmed.is_empty() {
        return;
    }
    let now = Instant::now();
    {
        let mut last = LAST_AUTO_OTP.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((prev, when)) = last.as_ref()
            && prev == trimmed
            && now.duration_since(*when) < Duration::from_secs(10)
        {
            return;
        }
        *last = Some((trimmed.to_owned(), now));
    }
    copy_otp(trimmed);
}

/// Pictures from notifications, kept for a couple of days (history lasts one).
const KEEP_IMAGES: std::time::Duration = std::time::Duration::from_secs(2 * 24 * 3600);
/// At most this many notification pictures and thumbnails are kept, and at
/// most this many bytes of them, newest first: a phone posting a new picture
/// every few seconds would otherwise fill the disk well within two days.
const MAX_IMAGE_FILES: usize = 1000;
const MAX_IMAGE_BYTES: u64 = 200 * 1024 * 1024;
/// How many new pictures are written between checks of those caps.
const CAP_CHECK_EVERY: u32 = 50;

pub(crate) fn images_dir() -> std::path::PathBuf {
    core_host::host().data_dir.join("cache").join("images")
}

/// A notification, by phone and key.
type NotificationKey = (DeviceId, String);

/// A notification's picture still to be written.
struct Picture {
    key: NotificationKey,
    path: PathBuf,
    bytes: Vec<u8>,
}

/// The picture file each notification shows once it's written, so a picture
/// written after its notification changed again isn't shown.
static WANTED_PICTURES: Mutex<Option<HashMap<NotificationKey, PathBuf>>> = Mutex::new(None);

fn wanted_pictures<T>(f: impl FnOnce(&mut HashMap<NotificationKey, PathBuf>) -> T) -> T {
    f(WANTED_PICTURES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new))
}

/// An event's notifications without a picture (`None`, for the hub), and
/// the pictures of the others, to write first. A picture is named after the
/// notification and the picture: an update with a new picture gets a new
/// file (the UI caches by name).
fn pictures_of(event: &NodeEvent) -> (Vec<(NotificationKey, Option<PathBuf>)>, Vec<Picture>) {
    let (device, items): (DeviceId, Vec<&Notification>) = match event {
        NodeEvent::NotificationsReset { device, items } => (*device, items.iter().collect()),
        NodeEvent::NotificationPosted { device, notification } => (*device, vec![notification]),
        _ => return (Vec::new(), Vec::new()),
    };
    let dir = images_dir();
    let mut without = Vec::new();
    let mut pictures = Vec::new();
    for n in items {
        let key = (device, n.key.clone());
        match &n.image {
            None => without.push((key, None)),
            Some(image) => {
                let name = format!(
                    "{:016x}.jpg",
                    fnv(&[device.to_string().as_bytes(), n.key.as_bytes(), image].concat())
                );
                pictures.push(Picture { key, path: dir.join(name), bytes: image.clone() });
            }
        }
    }
    (without, pictures)
}

/// The square version of a notification's picture, for the desktop's
/// notification, which draws a picture in a square: cut from the middle,
/// never squeezed.
pub(crate) fn thumb_path(picture: &std::path::Path) -> PathBuf {
    picture.with_extension("thumb.png")
}

/// Side of [`thumb_path`]'s square, in pixels.
const THUMB_SIDE: u32 = 192;

fn write_thumb(picture: &std::path::Path, bytes: &[u8]) {
    let thumb = thumb_path(picture);
    if thumb.exists() {
        return;
    }
    use crate::sys::image;
    let made = image::decode(bytes)
        .map_err(|e| e.to_string())
        .and_then(|bitmap| {
            let square = image::scale_to(&image::crop_square(&bitmap), THUMB_SIDE);
            image::encode_png(&square).map_err(|e| e.to_string())
        })
        .and_then(|png| std::fs::write(&thumb, png).map_err(|e| e.to_string()));
    if let Err(error) = made {
        tracing::debug!(%error, "can't make a notification picture's thumbnail");
    }
}

/// Writes pictures on the files thread; each shows on its notification once
/// it's there.
fn write_pictures(pictures: Vec<Picture>) {
    static WRITTEN_SINCE_CAP: AtomicU32 = AtomicU32::new(0);
    let dir = images_dir();
    let mut written = 0;
    for Picture { key, path, bytes } in pictures {
        let saved = std::fs::create_dir_all(&dir).and_then(|()| {
            if path.exists() {
                return Ok(());
            }
            written += 1;
            std::fs::write(&path, &bytes)
        });
        if let Err(e) = &saved {
            tracing::debug!(error = %e, "can't save a notification picture");
        } else {
            write_thumb(&path, &bytes);
        }
        wanted_pictures(|wanted| {
            if wanted.get(&key) != Some(&path) {
                return;
            }
            wanted.remove(&key);
            if saved.is_ok() {
                core_host::host().hub.update(|s| s.set_notification_image(key, path));
            }
        });
    }
    // Only ever called on the files thread, so the count can't race.
    let since_cap = WRITTEN_SINCE_CAP.load(Ordering::Relaxed).saturating_add(written);
    if since_cap >= CAP_CHECK_EVERY {
        cap_images();
        WRITTEN_SINCE_CAP.store(0, Ordering::Relaxed);
    } else {
        WRITTEN_SINCE_CAP.store(since_cap, Ordering::Relaxed);
    }
}

/// Deletes pictures older than [`KEEP_IMAGES`].
pub fn prune_images() {
    let Ok(entries) = std::fs::read_dir(images_dir()) else { return };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| now.duration_since(t).unwrap_or_default() > KEEP_IMAGES);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    cap_images();
}

/// Deletes the oldest notification pictures and thumbnails beyond
/// [`MAX_IMAGE_FILES`] or [`MAX_IMAGE_BYTES`].
fn cap_images() {
    let Ok(entries) = std::fs::read_dir(images_dir()) else { return };
    let files = entries
        .flatten()
        .filter_map(|e| {
            if !is_notification_image_file(e.file_name().to_str()?) {
                return None;
            }
            let meta = e.metadata().ok().filter(std::fs::Metadata::is_file)?;
            Some((e.path(), meta.modified().ok()?, meta.len()))
        })
        .collect();
    for path in over_cap(files, MAX_IMAGE_FILES, MAX_IMAGE_BYTES) {
        let _ = std::fs::remove_file(path);
    }
}

/// Of `files` (path, modified, length), those to delete so the newest fit
/// within `max_files` and `max_bytes`.
fn over_cap(mut files: Vec<(PathBuf, SystemTime, u64)>, max_files: usize, max_bytes: u64) -> Vec<PathBuf> {
    files.sort_by_key(|f| std::cmp::Reverse(f.1));
    let mut bytes = 0u64;
    files
        .into_iter()
        .enumerate()
        .filter_map(|(i, (path, _, len))| {
            bytes = bytes.saturating_add(len);
            (i >= max_files || bytes > max_bytes).then_some(path)
        })
        .collect()
}

fn is_notification_image_file(name: &str) -> bool {
    !name.starts_with("sms-")
        && !name.starts_with("mms-")
        && !name.starts_with("chat-")
        && !name.starts_with("photo-")
}

/// Immediately removes cached notification images from `cache/images/`.
pub fn clear_cached_images() {
    let Ok(entries) = std::fs::read_dir(images_dir()) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_notif = path.file_name().and_then(|n| n.to_str()).is_some_and(is_notification_image_file);
        if is_notif {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Total byte size of cached notification images on disk.
pub fn cached_image_bytes() -> u64 {
    let Ok(entries) = std::fs::read_dir(images_dir()) else { return 0 };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let name_str = name.to_str()?;
            if !is_notification_image_file(name_str) {
                return None;
            }
            let meta = e.metadata().ok()?;
            meta.is_file().then_some(meta.len())
        })
        .sum()
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

/// An app's icon still to be written: the package and its PNG.
type NewIcon = (String, Vec<u8>);

/// The app icons an event carries: where those saved already are, and the
/// others, to write first.
fn icons_of(event: &NodeEvent) -> (Vec<(String, PathBuf)>, Vec<NewIcon>) {
    let items: Vec<&Notification> = match event {
        NodeEvent::NotificationsReset { items, .. } => items.iter().collect(),
        NodeEvent::NotificationPosted { notification, .. } => vec![notification],
        _ => return (Vec::new(), Vec::new()),
    };
    let data_dir = &core_host::host().data_dir;
    let mut apps = HashSet::new();
    let mut saved = Vec::new();
    let mut new = Vec::new();
    for n in items {
        let Some(icon) = n.icon.as_ref() else { continue };
        if !apps.insert(n.app.as_str()) {
            continue;
        }
        match icons::already_saved(data_dir, &n.app, icon) {
            Some(path) => saved.push((n.app.clone(), path)),
            None => new.push((n.app.clone(), icon.clone())),
        }
    }
    (saved, new)
}

/// Writes app icons on the files thread, and records where they went.
fn write_icons(new: Vec<NewIcon>) {
    let host = core_host::host();
    for (app, png) in new {
        match icons::save(&host.data_dir, &app, &png) {
            Ok(path) => host.hub.update(|s| s.set_app_icon(&app, path)),
            Err(e) => tracing::debug!(app = %app, error = %e, "can't save an app icon"),
        }
    }
}

type FilesJob = Box<dyn FnOnce() + Send>;

/// Runs `job` on the thread that writes notifications' icons and pictures,
/// after the jobs before it: the core's event loop doesn't wait on the disk,
/// and a toast still comes after the files it shows.
fn on_files_thread(job: impl FnOnce() + Send + 'static) {
    static FILES: OnceLock<Option<mpsc::Sender<FilesJob>>> = OnceLock::new();
    let files = FILES.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<FilesJob>();
        let spawned = std::thread::Builder::new().name("notification-files".into()).spawn(move || {
            for job in rx {
                job();
            }
        });
        spawned.ok().map(|_| tx)
    });
    let job: FilesJob = Box::new(job);
    match files {
        Some(tx) => {
            // The thread is gone (a job panicked): done here instead.
            if let Err(mpsc::SendError(job)) = tx.send(job) {
                job();
            }
        }
        None => job(),
    }
}

/// Folds an event into the hub. Icons and pictures not saved yet are written
/// on the files thread, and show once they're there.
pub fn apply(event: &NodeEvent) {
    let (icons, new_icons) = icons_of(event);
    let (without_pictures, pictures) = pictures_of(event);
    // Under the lock the written pictures are shown with, so an older one
    // can't land after this.
    wanted_pictures(|wanted| {
        if let NodeEvent::DeviceRemoved(id) = event {
            wanted.retain(|(device, _), _| device != id);
        }
        for (key, _) in &without_pictures {
            wanted.remove(key);
        }
        for picture in &pictures {
            wanted.insert(picture.key.clone(), picture.path.clone());
        }
        core_host::host().hub.update(|s| {
            let mut changes = Changes::NONE;
            for (app, path) in icons {
                changes |= s.set_app_icon(&app, path);
            }
            changes |= s.apply_with_images(event, without_pictures);
            changes
        });
    });
    if !new_icons.is_empty() || !pictures.is_empty() {
        on_files_thread(move || {
            write_icons(new_icons);
            write_pictures(pictures);
        });
    }
}

/// Shows, updates or removes toasts after an event, on the files thread:
/// after the icon and picture they show are written.
pub fn update_toasts(event: &NodeEvent) {
    let for_toasts = matches!(
        event,
        NodeEvent::NotificationPosted { .. }
            | NodeEvent::NotificationRemoved { .. }
            | NodeEvent::NotificationsReset { .. }
            | NodeEvent::PhoneToggles { .. }
            | NodeEvent::DeviceRemoved(_)
    );
    if for_toasts {
        let event = event.clone();
        on_files_thread(move || toasts_after(&event));
    }
}

fn toasts_after(event: &NodeEvent) {
    match event {
        NodeEvent::NotificationPosted { device, notification } => show(*device, notification),
        NodeEvent::NotificationRemoved { device, key } => {
            toasted(|t| t.get_mut(device).map(|keys| keys.remove(key)));
            live_toasts(|l| l.remove(&(*device, key.clone())));
            otp_codes(|c| c.remove(&(*device, key.clone())));
            toast::remove(&device.to_string(), key);
        }
        NodeEvent::NotificationsReset { device, items } => {
            for n in items {
                if let Some(code) = notification_otp(n) {
                    otp_codes(|c| c.insert((*device, n.key.clone()), code));
                }
            }
            let keep: HashSet<&str> = items.iter().map(|n| n.key.as_str()).collect();
            let gone: Vec<String> = toasted(|t| {
                let keys = t.entry(*device).or_default();
                let gone = keys.iter().filter(|k| !keep.contains(k.as_str())).cloned().collect();
                keys.retain(|k| keep.contains(k.as_str()));
                gone
            });
            live_toasts(|l| l.retain(|(d, k)| d != device || keep.contains(k.as_str())));
            otp_codes(|c| c.retain(|(d, k), _| d != device || keep.contains(k.as_str())));
            for key in gone {
                toast::remove(&device.to_string(), &key);
            }
        }
        NodeEvent::PhoneToggles { device, toggles } => {
            if SYNC_DND.load(Ordering::Relaxed) && toggles.dnd {
                quiet_device_toasts(*device);
            }
        }
        NodeEvent::DeviceRemoved(device) => {
            toasted(|t| t.remove(device));
            live_toasts(|l| l.retain(|(d, _)| d != device));
            otp_codes(|c| c.retain(|(d, _), _| d != device));
            toast::remove_device(&device.to_string());
        }
        _ => {}
    }
}

fn live_toast_progress(n: &Notification) -> Option<toast::Progress> {
    let live = n.live.as_ref()?;
    let status = live
        .chip
        .as_deref()
        .filter(|s| !s.is_empty())
        .or(n.text.as_deref().filter(|s| !s.is_empty()))
        .unwrap_or("Live")
        .to_owned();
    let (value, label) = if live.indeterminate {
        (None, String::new())
    } else if let (Some(progress), Some(max)) = (live.progress, live.max)
        && max > 0
    {
        let ratio = (f64::from(progress) / f64::from(max)).clamp(0.0, 1.0);
        let pct = ((ratio * 100.0).round() as u32).min(100);
        (Some(ratio), format!("{pct}%"))
    } else {
        (None, live.chip.clone().unwrap_or_default())
    };
    Some(toast::Progress { status, value, label })
}

/// Whether a post pops up on the PC. A quiet update of one already shown
/// changes only the feed: WhatsApp re-posts its other chats each time a
/// message arrives, Android keeps those quiet, and GNOME would show every
/// replaced notification again. A live one (a timer, a download) updates its
/// pop-up in place.
fn pops_up(silent: bool, live: bool, already_shown: bool) -> bool {
    live || !(silent && already_shown)
}

fn show(device: DeviceId, n: &Notification) {
    // Cleared here already: the same post sent again doesn't pop up again.
    if core_host::host().hub.read(|s| s.is_cleared(device, n)) {
        return;
    }
    let already_shown = toasted(|t| t.get(&device).is_some_and(|keys| keys.contains(&n.key)));
    if !pops_up(n.silent, n.live.is_some(), already_shown) {
        return;
    }
    let otp = notification_otp(n);
    if let Some(code) = &otp {
        otp_codes(|c| c.insert((device, n.key.clone()), code.clone()));
        maybe_auto_copy_otp(code);
    }
    let (several_phones, device_name, icon, rule, image, phone_dnd) = core_host::host().hub.read(|s| {
        let dnd = s.toggles.get(&device).is_some_and(|t| t.dnd);
        (
            s.devices.len() > 1,
            s.name_of(&device),
            s.app_icons.get(&n.app).cloned(),
            s.app_rule(&n.app),
            s.notification_images.get(&(device, n.key.clone())).cloned(),
            dnd,
        )
    });
    // The user chose no pop-ups (or nothing at all) for this app, or the phone
    // is in Do Not Disturb while DND quieting is turned on.
    if rule != AppRule::Show || (SYNC_DND.load(Ordering::Relaxed) && phone_dnd) {
        return;
    }
    if stays_quiet(paused(), QUIET_WHEN_LOCKED.load(Ordering::Relaxed), crate::sys::session_locked) {
        return;
    }
    let progress = live_toast_progress(n);
    let id = (device, n.key.clone());
    if let Some(prog) = &progress {
        let already_shown = live_toasts(|l| l.contains(&id));
        if already_shown {
            toast::update_progress(&device.to_string(), &n.key, prog.clone());
            return;
        }
    } else {
        live_toasts(|l| l.remove(&id));
    }
    let mut actions: Vec<(String, String)> = Vec::new();
    if otp.is_some() {
        actions.push((ACTION_COPY_OTP.to_owned(), tr!("Notifications", "Copy code")));
    }
    actions.extend(n.actions.iter().filter(|a| !a.reply).map(|a| (a.id.clone(), a.title.clone())));
    let reply = n.actions.iter().find(|a| a.reply);
    let toast = Toast {
        device: device.to_string(),
        key: n.key.clone(),
        title: toast_title(
            n.title.as_deref(),
            &n.app_name,
            device_name.filter(|_| several_phones).as_deref(),
        ),
        body: n.text.clone().unwrap_or_default(),
        attribution: n.app_name.clone(),
        icon,
        // A square slot: the square version when it's made, so it isn't squeezed.
        image: image.map(|p| Some(thumb_path(&p)).filter(|t| t.exists()).unwrap_or(p)),
        actions,
        reply: reply.map(|a| (a.id.clone(), a.title.clone())),
        silent: n.silent,
        progress: progress.clone(),
        call: false,
    };
    toasted(|t| t.entry(device).or_default().insert(n.key.clone()));
    if progress.is_some() {
        live_toasts(|l| l.insert(id));
    }
    toast::show(toast);
}

/// A phone notification's title on the PC: who or what it's about, or the
/// app when it has no title, then the phone when more than one is paired.
fn toast_title(title: Option<&str>, app_name: &str, phone: Option<&str>) -> String {
    let main = title.map(str::trim).filter(|t| !t.is_empty()).unwrap_or(app_name.trim());
    match phone.map(str::trim).filter(|p| !p.is_empty()) {
        Some(phone) => format!("{main} · {phone}"),
        None => main.to_owned(),
    }
}

/// What the user did with a toast.
pub fn on_toast(event: ToastEvent) {
    // Received-file toasts: open the file, or show it in its folder.
    match &event {
        ToastEvent::Opened { device, key } if device == crate::transfers::TOAST_GROUP => {
            return crate::transfers::open(std::path::Path::new(key));
        }
        ToastEvent::Action { device, key, action } if device == crate::transfers::TOAST_GROUP => {
            if action == crate::transfers::ACTION_ACCEPT {
                return crate::transfers::accept(key.strip_prefix("req:").unwrap_or(key));
            }
            if action == crate::transfers::ACTION_DECLINE {
                return crate::transfers::cancel(key.strip_prefix("req:").unwrap_or(key));
            }
            if action == crate::transfers::ACTION_OPEN {
                return crate::transfers::open(std::path::Path::new(key));
            }
            return crate::transfers::show_in_folder(std::path::Path::new(key));
        }
        ToastEvent::Dismissed { device, .. } if device == crate::transfers::TOAST_GROUP => return,
        // Sends from Explorer: open the app, or cancel.
        ToastEvent::Action { device, key, action } if device == crate::send_to::TOAST_GROUP => {
            if action == crate::send_to::ACTION_CANCEL {
                crate::transfers::cancel(key);
            }
            return;
        }
        ToastEvent::Dismissed { device, .. } if device == crate::send_to::TOAST_GROUP => return,
        // "Update" on the update notice.
        ToastEvent::Action { device, .. } if device == crate::updater::TOAST_GROUP => {
            return crate::bridge::app::install_update_in_background();
        }
        ToastEvent::Dismissed { device, .. } if device == crate::updater::TOAST_GROUP => return,
        ToastEvent::Opened { device, .. } if device == crate::updater::TOAST_GROUP => {
            return crate::bridge::app::request_activation();
        }
        // New photos: open, save or copy one.
        ToastEvent::Opened { device, key } if device == crate::photos::TOAST_GROUP => {
            return crate::photos::on_toast(key, None);
        }
        ToastEvent::Action { device, key, action } if device == crate::photos::TOAST_GROUP => {
            return crate::photos::on_toast(key, Some(action));
        }
        ToastEvent::Dismissed { device, .. } if device == crate::photos::TOAST_GROUP => return,
        // Calls: answer, decline or silence; dismissing only hides it.
        ToastEvent::Action { device, key, action } if device == crate::calls::TOAST_GROUP => {
            return crate::calls::on_toast(key, action);
        }
        ToastEvent::Dismissed { device, .. } if device == crate::calls::TOAST_GROUP => return,
        ToastEvent::Dismissed { device, .. } | ToastEvent::Action { device, .. }
            if device == crate::battery::TOAST_GROUP =>
        {
            return;
        }
        // A phone's screenshot or recording of this screen: Stop ends a recording.
        ToastEvent::Action { device, action, .. } if device == crate::pc_capture::TOAST_GROUP => {
            if action == crate::pc_capture::ACTION_STOP {
                crate::pc_capture::stop_from_pc();
            }
            return;
        }
        ToastEvent::Dismissed { device, .. } if device == crate::pc_capture::TOAST_GROUP => return,
        ToastEvent::Action { device, action, .. } if device == crate::clipboard::TOAST_GROUP => {
            if action == crate::clipboard::ACTION_CLIP_SUGGESTION {
                crate::clipboard::run_last_suggestion();
            }
            return;
        }
        ToastEvent::Dismissed { device, .. } if device == crate::clipboard::TOAST_GROUP => return,
        _ => {}
    }
    match event {
        ToastEvent::Opened { device, key } => {
            if let Ok(dev) = device.parse::<DeviceId>() {
                let app_target = core_host::host().hub.read(|s| {
                    let can_open = s.matrices.get(&dev).and_then(|m| m.state("mirroring.app_windows"))
                        == Some(g7link_core::FeatureState::Available);
                    if !can_open {
                        return None;
                    }
                    s.notifications
                        .iter()
                        .find(|n| n.device == dev && n.notification.key == key)
                        .map(|n| (n.notification.app.clone(), n.notification.app_name.clone()))
                        .or_else(|| {
                            s.history
                                .iter()
                                .find(|h| h.device == dev && h.notification.key == key)
                                .map(|h| (h.notification.app.clone(), h.notification.app_name.clone()))
                        })
                        .filter(|(pkg, _)| !pkg.is_empty())
                });
                if let Some((pkg, app_name)) = app_target {
                    let label = if app_name.is_empty() { pkg.clone() } else { app_name };
                    crate::mirror::start_app(dev, pkg, label);
                    return;
                }
            }
            crate::bridge::app::request_activation();
        }
        ToastEvent::Action { device, key, action } if action == ACTION_COPY_OTP => {
            if let Ok(dev) = device.parse::<DeviceId>() {
                let code = otp_codes(|c| c.get(&(dev, key.clone())).cloned()).or_else(|| {
                    core_host::host().hub.read(|s| {
                        s.notifications
                            .iter()
                            .find(|n| n.device == dev && n.notification.key == key)
                            .and_then(|n| notification_otp(&n.notification))
                    })
                });
                if let Some(code) = code {
                    copy_otp(&code);
                }
            }
        }
        ToastEvent::Action { device, key, action } => run_action(&device, key, action, None),
        ToastEvent::Reply { device, key, action, text } => run_action(&device, key, action, Some(text)),
        ToastEvent::Dismissed { device, key } => dismiss(&device, key),
    }
}

/// Sets what an app's notifications do on this PC. Pop-ups it no longer
/// gets are taken away.
pub fn set_app_rule(app: &str, rule: AppRule) {
    let hub = &core_host::host().hub;
    hub.update(|s| s.set_app_rule(app, rule));
    if rule == AppRule::Show {
        return;
    }
    let shown: Vec<(DeviceId, String)> = hub.read(|s| {
        s.notifications
            .iter()
            .filter(|n| n.notification.app == app)
            .map(|n| (n.device, n.notification.key.clone()))
            .collect()
    });
    for (device, key) in shown {
        toasted(|t| t.get_mut(&device).map(|keys| keys.remove(&key)));
        live_toasts(|l| l.remove(&(device, key.clone())));
        toast::remove(&device.to_string(), &key);
    }
}

/// Dismisses a phone notification there (and here right away).
pub fn dismiss(device: &str, key: String) {
    let (Ok(device), Some(node)) = (device.parse::<DeviceId>(), core_host::node()) else { return };
    core_host::host().hub.update(|s| {
        s.mark_cleared(device, &key);
        s.apply(&NodeEvent::NotificationRemoved { device, key: key.clone() })
    });
    toasted(|t| t.get_mut(&device).map(|keys| keys.remove(&key)));
    live_toasts(|l| l.remove(&(device, key.clone())));
    otp_codes(|c| c.remove(&(device, key.clone())));
    toast::remove(&device.to_string(), &key);
    core_host::spawn(async move {
        if let Err(e) = node.dismiss_notification(device, key).await {
            report(&e);
        }
    });
}

/// Runs a notification's action on the phone (`reply` for a reply action).
pub fn run_action(device: &str, key: String, action: String, reply: Option<String>) {
    let (Ok(device), Some(node)) = (device.parse::<DeviceId>(), core_host::node()) else { return };
    let hub = &core_host::host().hub;
    // A reply shows under its notification right away, faded until the
    // phone has taken it.
    if let Some(text) = &reply {
        hub.update(|s| s.reply_sending(device, &key, text));
    }
    core_host::spawn(async move {
        let result = node.run_notification_action(device, key.clone(), action, reply.clone()).await;
        if let Some(text) = &reply {
            core_host::host().hub.update(|s| s.reply_done(device, &key, text, result.is_ok()));
        }
        if let Err(e) = result {
            report(&e);
        }
    });
}

fn report(error: &Error) {
    let message = match error {
        Error::NotFound => tr!("Notifications", "That notification is gone from the phone."),
        other => crate::bridge::app::describe(other),
    };
    crate::bridge::app::show_message(message);
}

#[cfg(test)]
mod tests {
    use g7link_core::NotificationLive;

    use super::*;

    #[test]
    fn the_oldest_pictures_go_first_once_over_the_caps() {
        let at = |secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
        let files = || {
            vec![
                (PathBuf::from("b.jpg"), at(20), 10),
                (PathBuf::from("a.jpg"), at(10), 10),
                (PathBuf::from("c.jpg"), at(30), 10),
            ]
        };
        assert!(over_cap(files(), 3, 30).is_empty());
        assert_eq!(over_cap(files(), 2, 30), [PathBuf::from("a.jpg")]);
        assert_eq!(over_cap(files(), 3, 15), [PathBuf::from("b.jpg"), PathBuf::from("a.jpg")]);
        assert_eq!(over_cap(files(), 0, 30).len(), 3);
        assert!(over_cap(Vec::new(), 0, 0).is_empty());
    }

    #[test]
    fn a_quiet_update_of_a_shown_notification_does_not_pop_up_again() {
        // WhatsApp re-posts an older chat, quietly, when another one gets a message.
        assert!(!pops_up(true, false, true));
        // A new message that alerts on the phone pops up, shown before or not.
        assert!(pops_up(false, false, true));
        assert!(pops_up(false, false, false));
        // Quiet but never shown here: it still goes to the list as before.
        assert!(pops_up(true, false, false));
        // A live one updates its pop-up in place.
        assert!(pops_up(true, true, true));
    }

    #[test]
    fn phones_are_told_only_when_the_hold_changes() {
        assert_eq!(hold_change(false, true, || true), Some(true));
        assert_eq!(hold_change(true, true, || true), None);
        assert_eq!(hold_change(true, true, || false), Some(false));
        assert_eq!(hold_change(false, true, || false), None);
        // Switched off while locked: carry on straight away.
        assert_eq!(hold_change(true, false, || true), Some(false));
        assert_eq!(hold_change(false, false, || panic!("no need to look")), None);
    }

    fn make_note(title: Option<&str>, text: Option<&str>) -> Notification {
        Notification {
            key: "k1".into(),
            app: "com.google.android.apps.messaging".into(),
            app_name: "Messages".into(),
            title: title.map(Into::into),
            text: text.map(Into::into),
            sub: None,
            when: 1_760_000_000_000,
            actions: Vec::new(),
            silent: false,
            ongoing: false,
            icon: None,
            image: None,
            live: None,
            conversation: None,
        }
    }

    #[test]
    fn paused_or_locked_notifications_stay_quiet() {
        assert!(stays_quiet(true, false, || false));
        assert!(stays_quiet(false, true, || true));
        assert!(!stays_quiet(false, true, || false));
        assert!(!stays_quiet(false, false, || panic!("no need to ask if it is locked")));
    }

    #[test]
    fn extracts_otp_from_body_or_title_plus_body_without_confusing_sender_number() {
        assert_eq!(
            notification_otp(&make_note(Some("5550100"), Some("Your verification code is 482913"))),
            Some("482913".into())
        );
        assert_eq!(
            notification_otp(&make_note(Some("Verification code"), Some("482913"))),
            Some("482913".into())
        );
        assert_eq!(
            notification_otp(&make_note(Some("5550100"), Some("Please enter your login code"))),
            None,
            "sender phone number in title must not be extracted as an OTP"
        );
    }

    #[test]
    fn builds_toast_progress_for_live_notifications() {
        let mut n = make_note(Some("Ride"), Some("Arriving in 4 min"));
        assert!(live_toast_progress(&n).is_none());

        n.live = Some(NotificationLive {
            v: 1,
            progress: Some(45),
            max: Some(100),
            indeterminate: false,
            chip: Some("4 min".into()),
            segments: Vec::new(),
            points: Vec::new(),
            chronometer: false,
            countdown: false,
        });
        let p = live_toast_progress(&n).unwrap();
        assert_eq!(p.status, "4 min");
        assert_eq!(p.value, Some(0.45));
        assert_eq!(p.label, "45%");
    }

    #[test]
    fn toast_title_is_the_sender_and_names_the_phone_only_when_there_are_several() {
        assert_eq!(toast_title(Some("Sharon Karwitha"), "WhatsApp", None), "Sharon Karwitha");
        assert_eq!(toast_title(Some("  "), "WhatsApp", None), "WhatsApp");
        assert_eq!(toast_title(None, "WhatsApp", Some("Pixel 8 Pro")), "WhatsApp · Pixel 8 Pro");
        assert_eq!(toast_title(Some("Sharon"), "WhatsApp", Some(" ")), "Sharon");
    }
}
