// SPDX-License-Identifier: GPL-3.0-or-later
//! Writing the clipboard through GNOME Shell, with G7 Linux's companion
//! extension. GNOME's Wayland gives other apps no clipboard protocol, so a
//! write through Qt reaches the clipboard only while a G7 Link window has the
//! focus; the extension sets it from inside the shell, focus or not.

use std::sync::Mutex;
use std::time::{Duration, Instant};

const SHELL: &str = "org.gnome.Shell";
const PATH: &str = "/org/gnome/Shell/Extensions/G7Linux/Clipboard";
const INTERFACE: &str = "org.gnome.Shell.Extensions.Clipboard";
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
