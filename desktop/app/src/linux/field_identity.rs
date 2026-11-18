// SPDX-License-Identifier: GPL-3.0-or-later
//! Telling text fields apart over time, so a phone's typing box can follow
//! the PC's focus from one field to another and find each one again.
//!
//! A field is known by its accessibility object (app connection and object
//! path), which is exact while the object lives, and by a stable key made of
//! what describes it (app, role, name, description, the web page's `id`,
//! and where it sits in its parents when it has no name). The ids handed out
//! are hashes keyed per run of the app, so they mean nothing anywhere else,
//! and no text is kept.

use std::{
    collections::{HashMap, VecDeque, hash_map::RandomState},
    hash::{BuildHasher, Hash},
};

/// Fields remembered at most; the least recently focused goes first.
const MAX_FIELDS: usize = 64;

/// What describes a field, for finding it again when its object was rebuilt.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct StableKey {
    pub app: String,
    /// The window it's in, when known.
    pub window: Option<u64>,
    /// The address of the web page it's on (hashed with the rest, not kept).
    pub doc: String,
    pub role: u32,
    pub name: String,
    pub description: String,
    /// A web page's `id` attribute.
    pub id: String,
    /// Index in each parent, nearest first, when it has no name or `id`.
    pub path: Vec<i32>,
}

/// The fields seen in this run, by object and by description.
pub struct FieldTable {
    hasher: RandomState,
    exact: HashMap<(String, String), u64>,
    stable: HashMap<u64, u64>,
    epochs: HashMap<u64, u32>,
    /// Field ids, most recently focused last.
    order: VecDeque<u64>,
}

impl Default for FieldTable {
    fn default() -> Self {
        Self {
            hasher: RandomState::new(),
            exact: HashMap::new(),
            stable: HashMap::new(),
            epochs: HashMap::new(),
            order: VecDeque::new(),
        }
    }
}

impl FieldTable {
    /// The field the object `(sender, path)` is, described by `key`: the
    /// same id as before when either was seen, a new one otherwise. Never 0.
    pub fn identify(&mut self, sender: &str, path: &str, key: &StableKey) -> u64 {
        let exact = (sender.to_owned(), path.to_owned());
        let stable = self.hash(key);
        let field = match self.exact.get(&exact) {
            Some(&field) => field,
            None => self.stable.get(&stable).copied().unwrap_or_else(|| self.hash(&(stable, "field")).max(1)),
        };
        self.exact.insert(exact, field);
        self.stable.insert(stable, field);
        self.touch(field);
        field
    }

    /// The field of a window that tells nothing about what's in it.
    pub fn window_field(&mut self, class: &str, window: u64) -> u64 {
        let field = self.hash(&(class, window, "window")).max(1);
        self.touch(field);
        field
    }

    /// This field's epoch: changed by [`FieldTable::doubt`].
    pub fn epoch(&self, field: u64) -> u32 {
        self.epochs.get(&field).copied().unwrap_or(0)
    }

    /// Something happened on the PC that a phone's record of what it typed
    /// into `field` doesn't know about. Returns the new epoch.
    pub fn doubt(&mut self, field: u64) -> u32 {
        let epoch = self.epochs.entry(field).or_insert(0);
        *epoch = epoch.wrapping_add(1);
        *epoch
    }

    fn hash<T: Hash>(&self, value: &T) -> u64 {
        self.hasher.hash_one(value)
    }

    fn touch(&mut self, field: u64) {
        self.order.retain(|&f| f != field);
        self.order.push_back(field);
        while self.order.len() > MAX_FIELDS {
            let Some(old) = self.order.pop_front() else { break };
            self.exact.retain(|_, f| *f != old);
            self.stable.retain(|_, f| *f != old);
            self.epochs.remove(&old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> StableKey {
        StableKey { app: "Firefox".into(), role: 79, name: name.into(), ..StableKey::default() }
    }

    #[test]
    fn the_same_object_or_a_rebuilt_one_is_the_same_field() {
        let mut table = FieldTable::default();
        let search = table.identify(":1.5", "/a/1", &key("Search"));
        assert_ne!(search, 0);
        assert_eq!(table.identify(":1.5", "/a/1", &key("Search")), search, "same object");
        assert_eq!(table.identify(":1.5", "/a/9", &key("Search")), search, "rebuilt with the same name");
        assert_eq!(table.identify(":1.5", "/a/1", &key("Search results")), search, "same object, renamed");
        let message = table.identify(":1.5", "/a/2", &key("Message"));
        assert_ne!(message, search);
    }

    #[test]
    fn unnamed_siblings_differ_by_position() {
        let mut table = FieldTable::default();
        let first = StableKey { path: vec![0, 3], ..key("") };
        let second = StableKey { path: vec![1, 3], ..key("") };
        assert_ne!(table.identify(":1.5", "/a/1", &first), table.identify(":1.5", "/a/2", &second));
    }

    #[test]
    fn epochs_count_doubts() {
        let mut table = FieldTable::default();
        let field = table.identify(":1.5", "/a/1", &key("Search"));
        assert_eq!(table.epoch(field), 0);
        assert_eq!(table.doubt(field), 1);
        assert_eq!(table.epoch(field), 1);
        // The app restarted: its description still finds the field.
        assert_eq!(table.identify(":1.7", "/b/1", &key("Search")), field);
    }

    #[test]
    fn old_fields_are_forgotten() {
        let mut table = FieldTable::default();
        let first = table.identify(":1.5", "/a/0", &key("0"));
        for n in 1..=MAX_FIELDS {
            table.identify(":1.5", &format!("/a/{n}"), &key(&n.to_string()));
        }
        assert!(table.order.len() <= MAX_FIELDS);
        assert!(!table.order.contains(&first));
        let windowed = table.window_field("org.gnome.Ptyxis", 7);
        assert_eq!(table.window_field("org.gnome.Ptyxis", 7), windowed);
        assert_ne!(table.window_field("org.gnome.Ptyxis", 8), windowed);
    }
}
