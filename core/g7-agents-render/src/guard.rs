// SPDX-License-Identifier: MPL-2.0
// Parts ported from t3code (MIT); see THIRD-PARTY-T3CODE.md.
//! The typing guard: a request that arrives while the user is typing does
//! not take the dock's buttons until they pause, so a keystroke meant for
//! the composer can never answer it.

use std::collections::{HashSet, VecDeque};

use crate::bounds::TYPING_IDLE_MS;

/// Request ids remembered as seen; older ones are forgotten first. Ids are
/// single-use tokens, so forgetting one only matters if it came back.
const MAX_SEEN: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardEvent<'a> {
    /// The pending request ids of the open thread, now.
    Requests {
        ids: &'a [&'a str],
        now_ms: u64,
    },
    /// The user changed the composer's text.
    Typed {
        now_ms: u64,
    },
    Focus,
    Blur,
    /// The user sent a message or denied with one.
    Sent,
    /// The timer set for [`TypingGuard::idle_at`] fired.
    Idle,
}

/// State of the guard for one composer.
#[derive(Clone, Default)]
pub struct TypingGuard {
    seen: HashSet<String>,
    seen_order: VecDeque<String>,
    held: Vec<String>,
    focused: bool,
    last_typed_ms: Option<u64>,
}

impl std::fmt::Debug for TypingGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypingGuard")
            .field("seen", &self.seen.len())
            .field("held", &self.held.len())
            .field("focused", &self.focused)
            .field("last_typed_ms", &self.last_typed_ms)
            .finish()
    }
}

impl TypingGuard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on(&mut self, event: GuardEvent<'_>) {
        match event {
            GuardEvent::Requests { ids, now_ms } => {
                self.held.retain(|h| ids.contains(&h.as_str()));
                let typing = self.focused
                    && self.last_typed_ms.is_some_and(|t| now_ms.saturating_sub(t) < TYPING_IDLE_MS);
                for &id in ids {
                    if self.seen.contains(id) {
                        continue;
                    }
                    if typing && !self.held.iter().any(|h| h == id) {
                        self.held.push(id.to_owned());
                    }
                    self.remember(id);
                }
            }
            GuardEvent::Typed { now_ms } => self.last_typed_ms = Some(now_ms),
            GuardEvent::Focus => self.focused = true,
            GuardEvent::Blur | GuardEvent::Sent | GuardEvent::Idle => {
                self.held.clear();
                self.last_typed_ms = None;
                if matches!(event, GuardEvent::Blur) {
                    self.focused = false;
                }
            }
        }
    }

    fn remember(&mut self, id: &str) {
        self.seen.insert(id.to_owned());
        self.seen_order.push_back(id.to_owned());
        while self.seen_order.len() > MAX_SEEN {
            if let Some(old) = self.seen_order.pop_front() {
                self.seen.remove(&old);
            }
        }
    }

    /// Whether request `id` is held: shown, but its buttons are off.
    pub fn held(&self, id: &str) -> bool {
        self.held.iter().any(|h| h == id)
    }

    /// When to fire [`GuardEvent::Idle`]: the last keystroke plus
    /// `TYPING_IDLE_MS`, while anything is held.
    pub fn idle_at(&self) -> Option<u64> {
        if self.held.is_empty() {
            return None;
        }
        self.last_typed_ms.map(|t| t.saturating_add(TYPING_IDLE_MS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_only_new_requests_while_typing() {
        let mut g = TypingGuard::new();
        g.on(GuardEvent::Requests { ids: &["a"], now_ms: 0 });
        assert!(!g.held("a"));
        g.on(GuardEvent::Focus);
        g.on(GuardEvent::Typed { now_ms: 1_000 });
        g.on(GuardEvent::Requests { ids: &["a", "b"], now_ms: 2_000 });
        assert!(!g.held("a"));
        assert!(g.held("b"));
        assert_eq!(g.idle_at(), Some(2_500));
        g.on(GuardEvent::Requests { ids: &["a", "b", "c"], now_ms: 2_600 });
        assert!(!g.held("c"));
        g.on(GuardEvent::Requests { ids: &["a"], now_ms: 2_700 });
        assert!(!g.held("b"));
        assert_eq!(g.idle_at(), None);
    }

    #[test]
    fn blur_sent_and_idle_release() {
        for release in [GuardEvent::Blur, GuardEvent::Sent, GuardEvent::Idle] {
            let mut g = TypingGuard::new();
            g.on(GuardEvent::Focus);
            g.on(GuardEvent::Typed { now_ms: 10 });
            g.on(GuardEvent::Requests { ids: &["x"], now_ms: 20 });
            assert!(g.held("x"));
            g.on(release);
            assert!(!g.held("x"));
            g.on(GuardEvent::Requests { ids: &["y"], now_ms: 30 });
            assert!(!g.held("y"), "typing time is forgotten");
        }
    }
}
