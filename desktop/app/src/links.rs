// SPDX-License-Identifier: GPL-3.0-or-later
//! Links and power between this PC and paired phones
//! (docs/protocol/actions.md): links a phone shares (web URLs, YouTube/video
//! links with playback timestamps, and `geo:` / map locations) open in the
//! PC's browser; a copied link, video, or address can be handed off to the
//! phone from the tray or Command Palette; a phone can lock this PC or put it
//! to sleep. A link a phone sent waits behind a notification with Open,
//! unless its user shared it from the phone's share sheet.

use crate::i18n::tr;
use g7link_core::{
    ClipKind, DeviceId, Error, FeatureState, HandoffKind, HandoffLink, LinkState, PowerAction,
    address_to_geo_uri, classify_clip, extract_handoff_link, format_video_timestamp, geo_to_maps_https,
    with_video_timestamp,
};
use std::{
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use crate::{
    bridge::app::describe,
    core_host,
    sys::{
        clipboard::{self, Clip},
        shell,
        toast::{self, Toast},
    },
};

/// The toast "device" for link notices.
pub const TOAST_GROUP: &str = "links";
/// The toast "device" for links a phone sent, waiting for Open.
pub const PROMPT_GROUP: &str = "link-prompts";
/// The Open button on a link a phone sent.
pub const ACTION_OPEN: &str = "open";
/// The most links one phone can have waiting; more are dropped.
const MAX_WAITING_PER_PHONE: usize = 5;
/// A link nobody answered stops waiting after this long: the notification
/// server may let its notification go without saying so.
const WAITING_FOR: Duration = Duration::from_secs(15 * 60);
/// The most of a link its notification shows.
const SHOWN_LINK_CHARS: usize = 300;

struct Waiting {
    key: String,
    peer: DeviceId,
    url: String,
    since: Instant,
}

static WAITING: Mutex<Vec<Waiting>> = Mutex::new(Vec::new());
static NEXT_KEY: AtomicU64 = AtomicU64::new(1);

/// Opens a link a phone sent (`http://`, `https://`, or `geo:`), in the default browser.
pub fn open_here(url: &str) -> Result<(), String> {
    if let Some(maps_url) = geo_to_maps_https(url) {
        return shell::open_url(&maps_url);
    }
    shell::open_url(url)
}

/// Takes out the links that waited too long, returning their keys.
fn expire(waiting: &mut Vec<Waiting>, now: Instant) -> Vec<String> {
    let (old, kept): (Vec<Waiting>, Vec<Waiting>) =
        std::mem::take(waiting).into_iter().partition(|w| now.duration_since(w.since) >= WAITING_FOR);
    *waiting = kept;
    old.into_iter().map(|w| w.key).collect()
}

fn has_room(waiting: &[Waiting], peer: &DeviceId) -> bool {
    waiting.iter().filter(|w| w.peer == *peer).count() < MAX_WAITING_PER_PHONE
}

/// Where a web link goes, without a user name before it (which could pass
/// for another site's name). `None` for map links.
fn link_host(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    (!host.is_empty()).then_some(host)
}

fn shown_link(url: &str) -> String {
    match url.char_indices().nth(SHOWN_LINK_CHARS) {
        Some((end, _)) => format!("{}…", &url[..end]),
        None => url.to_owned(),
    }
}

/// Asks before opening a link `peer` sent: a notification shows where it
/// goes, and only its Open button opens it. A phone with
/// `MAX_WAITING_PER_PHONE` links waiting has more dropped.
pub fn ask_to_open(peer: DeviceId, url: &str) -> Result<(), String> {
    let (key, expired) = {
        let mut waiting = WAITING.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        let expired = expire(&mut waiting, now);
        if !has_room(&waiting, &peer) {
            drop(waiting);
            tracing::info!(%peer, "dropped a link: too many from this phone are waiting");
            return Err("too many links are waiting".into());
        }
        let key = format!("link-{}", NEXT_KEY.fetch_add(1, Ordering::Relaxed));
        waiting.push(Waiting { key: key.clone(), peer, url: url.to_owned(), since: now });
        (key, expired)
    };
    for old in expired {
        toast::remove(PROMPT_GROUP, &old);
    }
    let name = core_host::host().hub.read(|s| s.name_of(&peer)).unwrap_or_else(|| tr!("Links", "Your phone"));
    let shown = shown_link(url);
    let body = match link_host(url) {
        Some(host) => format!("{host}\n{shown}"),
        None => shown,
    };
    toast::show(Toast {
        device: PROMPT_GROUP.into(),
        key,
        title: tr!("Links", "%1 sent a link", name),
        body,
        attribution: "G7 Link".into(),
        icon: None,
        image: None,
        actions: vec![(ACTION_OPEN.into(), tr!("Links", "Open"))],
        reply: None,
        silent: false,
        progress: None,
        call: false,
    });
    Ok(())
}

/// What the user did with a link's notification: Open opens the link,
/// anything else lets it go.
pub fn on_prompt(key: &str, action: Option<&str>) {
    let found = {
        let mut waiting = WAITING.lock().unwrap_or_else(PoisonError::into_inner);
        waiting.iter().position(|w| w.key == key).map(|i| waiting.remove(i))
    };
    let Some(link) = found else { return };
    if action == Some(ACTION_OPEN)
        && let Err(error) = open_here(&link.url)
    {
        tracing::warn!(error, "can't open a link a phone sent");
    }
}

/// Locks this PC, or puts it to sleep (a phone asked).
pub fn power(action: PowerAction) -> Result<(), String> {
    tracing::info!(action = action.as_str(), "a phone asked");
    match action {
        PowerAction::Lock => shell::lock(),
        PowerAction::Sleep => shell::sleep(),
    }
}

/// `text` as a link this app may open for someone else (a phone, or a Timeline entry
/// recorded from one): `http://`, `https://` or `geo:` only.
pub fn as_link(text: &str) -> Option<&str> {
    let text = text.trim();
    if text.len() > 4096 || text.chars().any(char::is_whitespace) {
        return None;
    }
    let lower = text.get(..8).map(str::to_ascii_lowercase).unwrap_or_default();
    let web = lower.starts_with("http://") || lower.starts_with("https://");
    let geo = text.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("geo:")) && text.len() > 4;
    (web || geo).then_some(text)
}

/// Enriches a Handoff link with the PC's active media playback timestamp if it
/// is a video link without an explicit timestamp.
pub fn enrich_handoff_link(mut link: HandoffLink) -> HandoffLink {
    if link.kind == HandoffKind::VideoLink
        && link.timestamp_secs.is_none()
        && let Some(secs) = crate::media::local_playback_position_secs()
    {
        link.url = with_video_timestamp(&link.url, secs);
        link.timestamp_secs = Some(secs);
        link.label = format!("{} · {}", link.label, format_video_timestamp(secs));
    }
    link
}

/// Extracts a rich [`HandoffLink`] from arbitrary text (supporting `http://`/`https://`
/// links, YouTube/video links with timestamps, `geo:` URIs, map links, and street addresses).
pub fn handoff_from_text(text: &str) -> Option<HandoffLink> {
    if let Some(h) = extract_handoff_link(text) {
        return Some(enrich_handoff_link(h));
    }
    if let Some(s) = classify_clip(text)
        && s.kind == ClipKind::StreetAddress
    {
        let geo = address_to_geo_uri(text.trim());
        return extract_handoff_link(&geo);
    }
    None
}

/// Whether `text` can be handed off as a link, video with timestamp, or map location.
pub fn is_handoff_text(text: &str) -> bool {
    handoff_from_text(text).is_some()
}

/// Extracts a rich [`HandoffLink`] from the current clipboard (supporting
/// `http://`/`https://` links, YouTube/video links with timestamps, `geo:` URIs,
/// map links, and copied street addresses).
pub fn handoff_from_clipboard() -> Option<HandoffLink> {
    let Clip::Text { text, .. } = clipboard::read() else { return None };
    handoff_from_text(&text)
}

fn notice(title: &str, body: &str) {
    toast::show(Toast {
        device: TOAST_GROUP.into(),
        key: "notice".into(),
        title: title.into(),
        body: body.into(),
        attribution: "G7 Link".into(),
        icon: None,
        image: None,
        actions: Vec::new(),
        reply: None,
        silent: true,
        progress: None,
        call: false,
    });
}

/// "Open copied link on phone" (tray / Command Palette): opens the link, video
/// with timestamp, or map location on the clipboard on a connected phone.
pub fn open_copied_link_on_phone() {
    let Clip::Text { .. } = clipboard::read() else {
        return notice(
            &tr!("Links", "Copy a link or address first"),
            &tr!(
                "Links",
                "Copy a web link, video URL, or street address, then choose this again to open it on your phone."
            ),
        );
    };
    let Some(handoff) = handoff_from_clipboard() else {
        return notice(
            &tr!("Links", "That's not a link or address"),
            &tr!(
                "Links",
                "Copy a web link (https://…), geo: URI, or street address, then choose this again."
            ),
        );
    };
    let node = core_host::node();
    let phone = core_host::host().hub.read(|s| {
        s.devices
            .iter()
            .filter(|d| matches!(d.link, LinkState::Online { .. }))
            .find(|d| {
                s.matrices.get(&d.id).and_then(|m| m.state("device.links_to_phone"))
                    == Some(FeatureState::Available)
            })
            .or_else(|| {
                s.devices.iter().find(|d| {
                    node.as_ref().and_then(|n| n.device_toggles(d.id).ok()).is_none_or(|toggles| {
                        toggles.into_iter().find(|(k, _)| *k == "links_to_phone").is_none_or(|(_, on)| on)
                    })
                })
            })
            .map(|d| (d.id, d.info.name.clone()))
    });
    let Some((phone, name)) = phone else {
        return notice(
            &tr!("Links", "No phone to open it on"),
            &tr!("Links", "Pair a phone first, or enable opening links on your phone."),
        );
    };
    send(phone, name, handoff);
}

/// Opens `url` (or a street address / link snippet) on `phone`, showing a notification with the outcome.
pub fn send_to(phone: DeviceId, url: String) {
    let name = core_host::host().hub.read(|s| s.name_of(&phone)).unwrap_or_else(|| "your phone".into());
    let handoff = handoff_from_text(&url).unwrap_or(HandoffLink {
        kind: HandoffKind::WebLink,
        label: url.clone(),
        url,
        timestamp_secs: None,
        map_query: None,
    });
    send(phone, name, handoff);
}

fn send(phone: DeviceId, name: String, handoff: HandoffLink) {
    let Some(node) = core_host::node() else { return };
    core_host::spawn(async move {
        let body = match handoff.kind {
            HandoffKind::MapLocation => {
                tr!("Links", "%1: tap the notification on your phone to open in Maps.", handoff.label)
            }
            HandoffKind::VideoLink if handoff.timestamp_secs.is_some() => {
                tr!("Links", "%1: tap the notification on your phone to continue watching.", handoff.label)
            }
            _ => tr!("Links", "Tap the notification on your phone to open it."),
        };
        match node.open_link(phone, handoff.url).await {
            Ok(()) => {
                if !crate::bridge::app::show_offline_queued_toast(phone, None) {
                    notice(&tr!("Links", "Sent to %1", name), &body);
                }
            }
            Err(Error::Unsupported) => {
                notice(&tr!("Links", "Couldn't send it"), &tr!("Links", "Update G7 Link on %1.", name))
            }
            Err(e) => notice(&tr!("Links", "Couldn't send it"), &describe(&e)),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waiting(peer: u8, key: &str, since: Instant) -> Waiting {
        Waiting { key: key.into(), peer: DeviceId([peer; 32]), url: "https://example.com".into(), since }
    }

    #[test]
    fn a_phone_has_at_most_five_links_waiting() {
        let now = Instant::now();
        let mut list: Vec<Waiting> = (0..5).map(|i| waiting(1, &format!("a{i}"), now)).collect();
        assert!(!has_room(&list, &DeviceId([1; 32])), "a sixth is dropped");
        assert!(has_room(&list, &DeviceId([2; 32])), "another phone isn't held up");
        assert!(expire(&mut list, now + Duration::from_secs(60)).is_empty());

        for later in &mut list[1..] {
            later.since = now + Duration::from_secs(1);
        }
        assert_eq!(expire(&mut list, now + WAITING_FOR), ["a0"]);
        assert_eq!(list.len(), 4);
        assert!(has_room(&list, &DeviceId([1; 32])));
    }

    #[test]
    fn a_links_notification_shows_where_it_really_goes() {
        assert_eq!(link_host("https://example.com/a?b#c"), Some("example.com"));
        assert_eq!(link_host("HTTP://Example.com:8080"), Some("Example.com:8080"));
        assert_eq!(link_host("https://bank.example@evil.example/login"), Some("evil.example"));
        assert_eq!(link_host("https://evil.example?@bank.example"), Some("evil.example"));
        assert_eq!(link_host("geo:37.7749,-122.4194?q=SF"), None);
        assert_eq!(link_host("https://"), None);

        let long = format!("https://example.com/{}", "é".repeat(400));
        let shown = shown_link(&long);
        assert_eq!(shown.chars().count(), SHOWN_LINK_CHARS + 1);
        assert!(shown.ends_with('…'));
        assert_eq!(shown_link("https://a.example"), "https://a.example");
    }

    #[test]
    fn copied_links_are_recognized() {
        assert_eq!(as_link("  https://example.com/a?b \n"), Some("https://example.com/a?b"));
        assert_eq!(as_link("HTTP://X.ORG"), Some("HTTP://X.ORG"));
        assert_eq!(as_link("geo:37.7749,-122.4194?q=SF"), Some("geo:37.7749,-122.4194?q=SF"));
        assert_eq!(as_link("see https://example.com"), None);
        assert_eq!(as_link("javascript:alert(1)"), None);
        assert_eq!(as_link("https://a.com b"), None);
    }
}
