// SPDX-License-Identifier: GPL-3.0-or-later
//! The sidebar's Messages and Calls counts: what came in on a phone since
//! its page was last looked at on this PC. Looking at the page clears it.
//!
//! Counted by the dates the phone gives, not by how much of the history
//! happens to be loaded: loading older calls or texts never adds to a count.

use std::collections::HashMap;

use crate::settings::PagesSeen;

/// A page with a count in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Messages,
    Calls,
}

impl Page {
    pub const ALL: [Page; 2] = [Page::Messages, Page::Calls];

    /// The page named as QML names it (`AppController.currentPage`).
    pub fn named(name: &str) -> Option<Page> {
        match name {
            "messages" => Some(Page::Messages),
            "calls" => Some(Page::Calls),
            _ => None,
        }
    }

    fn of(self, seen: &mut PagesSeen) -> &mut i64 {
        match self {
            Page::Messages => &mut seen.messages,
            Page::Calls => &mut seen.calls,
        }
    }
}

/// Something counted: when it came (Unix ms, as the phone dates it) and who
/// it's from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Arrival {
    pub date: i64,
    pub from: String,
}

/// What a phone's counts are made from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inbox {
    /// Text conversations with unread messages, by their latest message.
    pub texts: Vec<Arrival>,
    /// Chat-app conversations with unread incoming messages.
    pub chats: Vec<Arrival>,
    /// Missed calls.
    pub missed: Vec<Arrival>,
}

impl Inbox {
    fn arrivals(&self, page: Page) -> Vec<&Arrival> {
        match page {
            Page::Messages => self.texts.iter().chain(&self.chats).collect(),
            Page::Calls => self.missed.iter().collect(),
        }
    }

    /// How many came after `page` was last looked at, and who the newest of
    /// them is from.
    pub fn count(&self, page: Page, seen: &PagesSeen) -> (u32, String) {
        let since = match page {
            Page::Messages => seen.messages,
            Page::Calls => seen.calls,
        };
        let newer: Vec<&Arrival> = self.arrivals(page).into_iter().filter(|a| a.date > since).collect();
        let from = newer.iter().max_by_key(|a| a.date).map(|a| a.from.clone()).unwrap_or_default();
        (u32::try_from(newer.len()).unwrap_or(u32::MAX), from)
    }
}

/// `device`'s entry, made the first time a phone is counted: from `now`, so
/// the history it already had doesn't count. Also says whether it's new.
pub fn entry<'a>(
    seen: &'a mut HashMap<String, PagesSeen>,
    device: &str,
    now: i64,
) -> (&'a mut PagesSeen, bool) {
    let new = !seen.contains_key(device);
    (seen.entry(device.to_owned()).or_insert(PagesSeen { messages: now, calls: now }), new)
}

/// Moves `seen` on for what's on screen at `now`: `opened` is a page just
/// opened or left, seen up to now; `showing` is the page on screen, which
/// sees what arrives while it shows. Everything listed counts as seen then,
/// even what the phone's clock dates a little ahead of this PC's. Returns
/// whether `seen` changed.
pub fn settle(
    seen: &mut PagesSeen,
    inbox: &Inbox,
    now: i64,
    opened: Option<Page>,
    showing: Option<Page>,
) -> bool {
    let mut changed = false;
    for page in Page::ALL {
        let since = page.of(seen);
        let arrivals = inbox.arrivals(page);
        let arrived = arrivals.iter().any(|a| a.date > *since);
        if opened == Some(page) || (showing == Some(page) && arrived) {
            let to = arrivals.iter().map(|a| a.date).fold((*since).max(now), i64::max);
            changed |= to != *since;
            *since = to;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(date: i64, from: &str) -> Arrival {
        Arrival { date, from: from.into() }
    }

    #[test]
    fn only_what_came_after_the_last_look_counts() {
        let inbox = Inbox {
            texts: vec![at(300, "Sam"), at(100, "Old promo")],
            chats: vec![at(250, "Family")],
            missed: vec![at(400, "Alex"), at(50, "Bank")],
        };
        let seen = PagesSeen { messages: 200, calls: 200 };
        assert_eq!(inbox.count(Page::Messages, &seen), (2, "Sam".to_owned()));
        assert_eq!(inbox.count(Page::Calls, &seen), (1, "Alex".to_owned()));
        // Older history loading doesn't add to it.
        let mut more = inbox.clone();
        more.missed.extend([at(20, "Older"), at(10, "Oldest")]);
        assert_eq!(more.count(Page::Calls, &seen), (1, "Alex".to_owned()));
    }

    #[test]
    fn opening_a_page_clears_its_count_and_only_its_own() {
        let inbox = Inbox { texts: vec![at(300, "Sam")], chats: vec![], missed: vec![at(400, "Alex")] };
        let mut seen = PagesSeen { messages: 200, calls: 200 };
        assert!(settle(&mut seen, &inbox, 1_000, Some(Page::Calls), None));
        assert_eq!(inbox.count(Page::Calls, &seen).0, 0);
        assert_eq!(inbox.count(Page::Messages, &seen).0, 1, "Messages wasn't opened");
        assert_eq!(seen.calls, 1_000);
    }

    #[test]
    fn a_phone_clock_ahead_of_the_pc_doesnt_leave_a_count() {
        let inbox = Inbox { texts: vec![at(5_000, "Sam")], ..Inbox::default() };
        let mut seen = PagesSeen { messages: 200, calls: 200 };
        settle(&mut seen, &inbox, 1_000, Some(Page::Messages), None);
        assert_eq!(seen.messages, 5_000);
        assert_eq!(inbox.count(Page::Messages, &seen).0, 0);
    }

    #[test]
    fn what_arrives_while_the_page_shows_is_seen() {
        let mut inbox = Inbox::default();
        let mut seen = PagesSeen { messages: 1_000, calls: 1_000 };
        // Nothing new: nothing to save.
        assert!(!settle(&mut seen, &inbox, 2_000, None, Some(Page::Messages)));
        inbox.texts.push(at(2_500, "Sam"));
        inbox.missed.push(at(2_500, "Alex"));
        assert!(settle(&mut seen, &inbox, 3_000, None, Some(Page::Messages)));
        assert_eq!(inbox.count(Page::Messages, &seen).0, 0);
        assert_eq!(inbox.count(Page::Calls, &seen).0, 1, "Calls isn't on screen");
    }

    #[test]
    fn a_phone_counted_the_first_time_has_no_count_for_its_history() {
        let mut map = HashMap::new();
        let (seen, new) = entry(&mut map, "phone", 10_000);
        assert!(new);
        let seen = *seen;
        let inbox = Inbox {
            texts: vec![at(9_000, "Old promo")],
            chats: vec![at(9_500, "Group")],
            missed: vec![at(8_000, "Bank")],
        };
        assert_eq!(inbox.count(Page::Messages, &seen).0, 0);
        assert_eq!(inbox.count(Page::Calls, &seen).0, 0);
        let (again, new) = entry(&mut map, "phone", 20_000);
        assert!(!new);
        assert_eq!(again.messages, 10_000, "kept, not moved on");
    }

    #[test]
    fn pages_are_named_as_qml_names_them() {
        assert_eq!(Page::named("messages"), Some(Page::Messages));
        assert_eq!(Page::named("calls"), Some(Page::Calls));
        assert_eq!(Page::named("home"), None);
    }
}
