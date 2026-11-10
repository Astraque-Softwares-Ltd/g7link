// SPDX-License-Identifier: GPL-3.0-or-later
//! Which paired device the PC is controlling (a mirror window the mouse is in,
//! or a remote control session), and the clipboard following along: with
//! `flow_follow_clipboard` on, the PC's clipboard goes to the device when
//! control moves to it.

use std::sync::{Mutex, OnceLock};

use g7link_core::DeviceId;

use crate::{core_host, settings};

static ACTIVE_REMOTE: OnceLock<Mutex<Option<DeviceId>>> = OnceLock::new();

fn active_slot() -> &'static Mutex<Option<DeviceId>> {
    ACTIVE_REMOTE.get_or_init(|| Mutex::new(None))
}

/// Transitions Flow cursor/keyboard focus from the PC to `peer`.
/// When `flow_follow_clipboard` is enabled (default `true`), pushes the current
/// PC clipboard to `peer`. Returns `true` if clipboard sync was triggered.
pub fn enter_remote(peer: DeviceId) -> bool {
    enter_remote_with_setting(peer, settings::flow_follow_clipboard())
}

/// Transitions Flow cursor/keyboard focus to `peer` with an explicit
/// `follow_clipboard` flag. Returns `true` if clipboard sync was triggered.
pub fn enter_remote_with_setting(peer: DeviceId, follow_clipboard: bool) -> bool {
    let changed = {
        let mut guard = active_slot().lock().unwrap_or_else(|e| e.into_inner());
        let is_new = *guard != Some(peer);
        *guard = Some(peer);
        is_new
    };
    if changed && follow_clipboard {
        if core_host::node().is_some() {
            crate::mirror::sync_clipboard_for_device(peer);
        }
        true
    } else {
        false
    }
}

/// Transitions Flow cursor/keyboard focus back from the remote phone/tablet to the PC.
pub fn leave_remote() -> Option<DeviceId> {
    active_slot().lock().unwrap_or_else(|e| e.into_inner()).take()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_remote() -> Option<DeviceId> {
        *active_slot().lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn entering_a_device_syncs_the_clipboard_when_enabled() {
        let _ = leave_remote();
        assert_eq!(active_remote(), None);

        let peer = DeviceId([0x42; 32]);

        // Entering remote via enter_remote uses settings::flow_follow_clipboard() (default true).
        settings::set_flow_follow_clipboard(true);
        assert!(enter_remote(peer));
        assert_eq!(active_remote(), Some(peer));

        // Re-entering the same active peer without leaving does not duplicate sync.
        assert!(!enter_remote_with_setting(peer, true));

        // Leaving clears the active remote peer.
        assert_eq!(leave_remote(), Some(peer));
        assert_eq!(active_remote(), None);

        // Entering remote with flow_follow_clipboard = false does not trigger clipboard sync.
        assert!(!enter_remote_with_setting(peer, false));
        assert_eq!(active_remote(), Some(peer));
        let _ = leave_remote();
    }
}
