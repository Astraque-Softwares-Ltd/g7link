// SPDX-License-Identifier: GPL-3.0-or-later
//! The clipboard through GNOME Shell, with G7 Linux's companion extension.
//! GNOME's Wayland gives other apps no clipboard protocol, so through Qt a
//! write reaches the clipboard, and a copy is seen, only while a G7 Link
//! window has the focus. The extension sets the clipboard from inside the
//! shell and tells of every copy (`ClipboardChanged`), focus or not.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::clipboard::{Clip, Image};

const SHELL: &str = "org.gnome.Shell";
const BUS: &str = "org.freedesktop.DBus";
const PATH: &str = "/org/gnome/Shell/Extensions/G7Linux/Clipboard";
const INTERFACE: &str = "org.gnome.Shell.Extensions.Clipboard";
const CHANGED: &str = "ClipboardChanged";
/// What the extension reports for a copy its app marked private. It carries
/// no content.
const CONCEALED: &str = "application/x-vicinae-concealed";
const CALL_TIMEOUT: Duration = Duration::from_secs(2);
/// How long an answer to "is the extension there" holds: it can be turned
/// on or off at any time.
const RECHECK: Duration = Duration::from_secs(30);
/// The largest clip the extension takes.
const MAX_BYTES: usize = 10 * 1024 * 1024;

static AVAILABLE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);

fn session() -> Option<zbus::blocking::Connection> {
    zbus::blocking::connection::Builder::session().ok()?.method_timeout(CALL_TIMEOUT).build().ok()
}

/// Whether the extension answers on the session bus now.
pub fn available() -> bool {
    if let Some((at, known)) = *AVAILABLE.lock().unwrap_or_else(|e| e.into_inner())
        && at.elapsed() < RECHECK
    {
        return known;
    }
    let known = super::non_tokio(|| {
        session().is_some_and(|conn| {
            conn.call_method(Some(SHELL), PATH, Some(INTERFACE), "GetClipboardMimeTypes", &()).is_ok()
        })
    });
    *AVAILABLE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), known));
    known
}

fn call<B: serde::Serialize + zbus::zvariant::DynamicType + Sync>(method: &str, body: &B) -> bool {
    let sent = super::non_tokio(|| {
        session().map(|conn| conn.call_method(Some(SHELL), PATH, Some(INTERFACE), method, body).map(|_| ()))
    });
    match sent {
        Some(Ok(())) => true,
        Some(Err(error)) => {
            tracing::info!(%error, method, "the shell couldn't set the clipboard");
            *AVAILABLE.lock().unwrap_or_else(|e| e.into_inner()) = None;
            false
        }
        None => false,
    }
}

/// Puts `text` on the clipboard. False when the extension isn't there or
/// refused, so the caller can try another way.
pub fn write_text(text: &str) -> bool {
    text.len() <= MAX_BYTES && available() && call("SetContent", &(text,))
}

/// Puts a PNG image on the clipboard.
pub fn write_png(png: &[u8]) -> bool {
    png.len() <= MAX_BYTES && available() && call("SetContentBinary", &(png, "image/png"))
}

/// Follows copies made in any app through the extension, from when it
/// answers, on a thread of its own for the app's lifetime.
pub fn follow(changed: fn(Clip)) {
    let spawned = std::thread::Builder::new().name("clipboard-shell".into()).spawn(move || {
        loop {
            if available() {
                follow_shell(changed);
            }
            std::thread::sleep(RECHECK);
        }
    });
    if let Err(error) = spawned {
        tracing::warn!(%error, "can't follow the clipboard through GNOME Shell");
    }
}

/// Follows `ClipboardChanged` until the session bus connection ends.
fn follow_shell(changed: fn(Clip)) {
    let Some(conn) = session() else { return };
    // Made before subscribing, so it has every signal from then on.
    let messages = zbus::blocking::MessageIterator::from(&conn);
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&conn) else { return };
    for rule in watched_signals() {
        if let Err(error) = dbus.add_match_rule(rule) {
            tracing::debug!(%error, "can't follow the shell's clipboard");
            return;
        }
    }
    // Asked after subscribing, so a restart in between still comes as a signal.
    let mut owner = shell_owner(&conn);
    listen(&conn);
    for message in messages {
        let Ok(message) = message else { break };
        let Some(signal) = shell_signal(&message) else { continue };
        let header = message.header();
        if !trusted_sender(&signal, header.sender().map(|s| s.as_str()), owner.as_deref()) {
            continue;
        }
        match signal {
            Signal::Owner(new) => {
                let restarted = new.is_some();
                owner = new;
                if restarted {
                    listen(&conn);
                }
            }
            Signal::Copied { content, mime } => {
                if let Some(clip) = clip_from(content, &mime) {
                    changed(clip);
                }
            }
        }
    }
}

/// The unique name GNOME Shell has on the bus now.
fn shell_owner(conn: &zbus::blocking::Connection) -> Option<String> {
    let reply = conn
        .call_method(Some(BUS), "/org/freedesktop/DBus", Some(BUS), "GetNameOwner", &(SHELL,))
        .inspect_err(|error| tracing::debug!(%error, "can't find GNOME Shell on the bus"))
        .ok()?;
    reply.body().deserialize::<String>().ok()
}

/// Whether a signal came from who may send it: the bus itself for the shell's
/// name changing hands, the shell's current owner for a copy. Any client on
/// the session bus can send signals straight to this connection, past the
/// match rules, so a fake copy must not reach the phone.
fn trusted_sender(signal: &Signal, sender: Option<&str>, shell_owner: Option<&str>) -> bool {
    match signal {
        Signal::Owner(_) => sender == Some(BUS),
        Signal::Copied { .. } => sender.is_some() && sender == shell_owner,
    }
}

/// Asks the extension for its signals as this connection, which keeps them on
/// while it lasts. Never stopped: that could silence other apps.
fn listen(conn: &zbus::blocking::Connection) {
    if let Err(error) = conn.call_method(Some(SHELL), PATH, Some(INTERFACE), "ListenToClipboardChanges", &())
    {
        tracing::debug!(%error, "the shell didn't take the clipboard listener");
    }
}

fn watched_signals() -> Vec<zbus::MatchRule<'static>> {
    let signal = || zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal);
    [
        signal()
            .sender(BUS)
            .and_then(|b| b.interface(BUS))
            .and_then(|b| b.member("NameOwnerChanged"))
            .and_then(|b| b.arg(0, SHELL)),
        signal()
            .sender(SHELL)
            .and_then(|b| b.path(PATH))
            .and_then(|b| b.interface(INTERFACE))
            .and_then(|b| b.member(CHANGED)),
    ]
    .into_iter()
    .filter_map(|rule| rule.map(|b| b.build()).ok())
    .collect()
}

#[derive(Debug, PartialEq, Eq)]
enum Signal {
    /// `org.gnome.Shell` changed hands: GNOME Shell's new unique name, or
    /// `None` when it left the bus. A new shell's extension no longer knows
    /// who listens.
    Owner(Option<String>),
    Copied {
        content: Vec<u8>,
        mime: String,
    },
}

fn shell_signal(message: &zbus::Message) -> Option<Signal> {
    let header = message.header();
    let (Some(interface), Some(member)) = (header.interface(), header.member()) else {
        return None;
    };
    let body = message.body();
    match (interface.as_str(), member.as_str()) {
        (BUS, "NameOwnerChanged") => body
            .deserialize::<(String, String, String)>()
            .ok()
            .filter(|(name, _, _)| name == SHELL)
            .map(|(_, _, owner)| Signal::Owner(Some(owner).filter(|o| !o.is_empty()))),
        (INTERFACE, CHANGED) if header.path().is_some_and(|path| path.as_str() == PATH) => {
            let (content, mime, _app) = body.deserialize::<(Vec<u8>, String, String)>().ok()?;
            Some(Signal::Copied { content, mime })
        }
        _ => None,
    }
}

/// What a `ClipboardChanged` holds, as a clip: text (`text/html` is the
/// copied text too), an image as PNG, or a private copy, which carries
/// nothing. Files and other types aren't followed.
pub(crate) fn clip_from(content: Vec<u8>, mime: &str) -> Option<Clip> {
    let mime = mime.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
    if mime == CONCEALED {
        return Some(Clip::Private);
    }
    if content.is_empty() || content.len() > MAX_BYTES {
        return None;
    }
    match mime.as_str() {
        "text/plain" | "text/html" => {
            let text = String::from_utf8(content).ok()?;
            let text = text.trim_matches('\0');
            if text.trim().is_empty() || super::clipboard::is_copied_file_uri_text(text) {
                return None;
            }
            Some(Clip::Text { text: text.to_owned(), html: None, rtf: None })
        }
        "image/png" => Some(Clip::Image(Image::Png(content))),
        image if image.starts_with("image/") => {
            let bitmap = super::image::decode(&content).ok()?;
            super::image::encode_png(&bitmap).ok().map(|png| Clip::Image(Image::Png(png)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signals_map_to_clips() {
        let text = |mime| clip_from(b"Meeting at 3".to_vec(), mime);
        let expected = Some(Clip::Text { text: "Meeting at 3".into(), html: None, rtf: None });
        assert_eq!(text("text/plain"), expected);
        assert_eq!(text("text/plain;charset=utf-8"), expected);
        assert_eq!(text("text/html"), expected);

        // A private copy carries nothing, and whatever comes with it is dropped.
        assert_eq!(clip_from(Vec::new(), CONCEALED), Some(Clip::Private));
        assert_eq!(clip_from(b"hunter2".to_vec(), CONCEALED), Some(Clip::Private));

        // Files, other types, empty or oversized content and broken text aren't clips.
        assert_eq!(clip_from(b"file:///home/me/a.txt".to_vec(), "text/uri-list"), None);
        assert_eq!(clip_from(b"file:///home/me/a.txt".to_vec(), "text/plain"), None);
        assert_eq!(clip_from(b"x".to_vec(), "application/x-thing"), None);
        assert_eq!(clip_from(Vec::new(), "text/plain"), None);
        assert_eq!(clip_from(b" \n".to_vec(), "text/plain"), None);
        assert_eq!(clip_from(vec![b'a'; MAX_BYTES + 1], "text/plain"), None);
        assert_eq!(clip_from(vec![0xff, 0xfe, 0x00], "text/plain"), None);

        // A PNG goes as it is; another image that can't be read is dropped.
        let bitmap = super::super::image::Bitmap { width: 1, height: 1, bgra: vec![0, 0, 255, 255] };
        let png = super::super::image::encode_png(&bitmap).unwrap();
        assert_eq!(clip_from(png.clone(), "image/png"), Some(Clip::Image(Image::Png(png))));
        assert_eq!(clip_from(b"not an image".to_vec(), "image/webp"), None);
    }

    #[test]
    fn follows_copies_and_the_shell_coming_back() {
        let signal = |path: &str, body: &(Vec<u8>, &str, &str)| {
            zbus::Message::signal(path, INTERFACE, CHANGED).unwrap().build(body).unwrap()
        };
        let copied = signal(PATH, &(b"hi".to_vec(), "text/plain", "org.gnome.TextEditor"));
        let expected = Signal::Copied { content: b"hi".to_vec(), mime: "text/plain".into() };
        assert_eq!(shell_signal(&copied), Some(expected));
        let elsewhere = signal("/org/example", &(b"hi".to_vec(), "text/plain", ""));
        assert_eq!(shell_signal(&elsewhere), None);

        let owner = |name: &str, new: &str| {
            zbus::Message::signal("/org/freedesktop/DBus", "org.freedesktop.DBus", "NameOwnerChanged")
                .unwrap()
                .build(&(name, ":1.4", new))
                .unwrap()
        };
        assert_eq!(shell_signal(&owner(SHELL, ":1.9")), Some(Signal::Owner(Some(":1.9".into()))));
        assert_eq!(shell_signal(&owner(SHELL, "")), Some(Signal::Owner(None)));
        assert_eq!(shell_signal(&owner("org.example", ":1.9")), None);
    }

    #[test]
    fn only_the_shell_and_the_bus_are_believed() {
        let copied = Signal::Copied { content: b"hi".to_vec(), mime: "text/plain".into() };
        assert!(trusted_sender(&copied, Some(":1.9"), Some(":1.9")));
        // Another client on the session bus, or a copy before the shell is known.
        assert!(!trusted_sender(&copied, Some(":1.42"), Some(":1.9")));
        assert!(!trusted_sender(&copied, Some(":1.9"), None));
        assert!(!trusted_sender(&copied, None, None));
        // The bus can't send a copy, nor a client a name change.
        assert!(!trusted_sender(&copied, Some(BUS), Some(":1.9")));
        let restarted = Signal::Owner(Some(":1.10".into()));
        assert!(trusted_sender(&restarted, Some(BUS), Some(":1.9")));
        assert!(!trusted_sender(&restarted, Some(":1.42"), Some(":1.9")));
        assert!(!trusted_sender(&Signal::Owner(None), Some(":1.9"), Some(":1.9")));
    }
}
