// SPDX-License-Identifier: GPL-3.0-or-later
//! Starting with the computer (an XDG autostart entry): an installed copy
//! starts at login unless the user turns it off; a build run from its folder
//! only when turned on. Only the instance with the usual data folder manages
//! this (a test instance with its own leaves it alone).

use std::sync::atomic::{AtomicBool, Ordering};

use crate::win::autostart;

static MANAGED: AtomicBool = AtomicBool::new(false);

/// Whether the app starts at login, given what the user chose (if
/// anything).
pub fn effective(choice: Option<bool>) -> bool {
    choice.unwrap_or_else(|| std::env::current_exe().is_ok_and(|exe| autostart::is_installed(&exe)))
}

/// Brings the autostart entry in line with the setting, from now on.
pub fn start(choice: Option<bool>) {
    MANAGED.store(true, Ordering::Relaxed);
    apply(effective(choice));
}

/// Starts this copy at login, or stops doing so.
pub fn apply(on: bool) {
    if !MANAGED.load(Ordering::Relaxed) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    if let Err(e) = autostart::set(on, &exe) {
        tracing::warn!(error = %e, "can't change starting at login");
    }
}

/// Removes the entry (when uninstalling).
pub fn remove() {
    if let Ok(exe) = std::env::current_exe()
        && let Err(e) = autostart::set(false, &exe)
    {
        tracing::warn!(error = %e, "can't stop starting at login");
    }
}
