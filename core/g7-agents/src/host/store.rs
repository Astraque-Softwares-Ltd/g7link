// SPDX-License-Identifier: MPL-2.0
//! The thread store (§3): `folders.json`, and per thread `meta.json` and an
//! append-only `items.jsonl`, under `$XDG_DATA_HOME/g7-agents/`.
//!
//! JSON files are replaced atomically (a unique temporary file created
//! exclusively, written in full, synced, renamed). A file that can't be read
//! is kept aside as `<name>.unreadable-<ms>`, never overwritten. All writes
//! run on one thread ([`StoreHandle`]) so they keep their order and never
//! block the async runtime.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::api::{self, ApprovalOutcome, Folder, HostSettings, Item, Origin, ThreadInfo};

/// Store format version.
const VERSION: u32 = 1;
/// `items.jsonl` is compacted past this many lines.
const COMPACT_LINES: usize = 4000;
/// A stored item line longer than this is skipped when reading.
const MAX_ITEM_LINE: usize = 1 << 20;

#[derive(Serialize, Deserialize)]
struct FoldersFile {
    version: u32,
    folders: Vec<StoredFolder>,
}

/// A folder as stored: without `exists`.
#[derive(Serialize, Deserialize)]
struct StoredFolder {
    id: String,
    label: String,
    path: String,
    added_ms: u64,
}

#[derive(Serialize, Deserialize)]
struct SettingsFile {
    version: u32,
    #[serde(default = "yes")]
    follow_sessions: bool,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize)]
struct MetaFile {
    version: u32,
    thread: ThreadInfo,
    next_seq: u64,
}

/// An answered approval, kept beside the thread's items so an export can say
/// who allowed or denied each step. Never the token or the request itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRecord {
    /// The tool step it was about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_seq: Option<u64>,
    pub outcome: ApprovalOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Origin>,
    pub ts_ms: u64,
}

/// `approvals.jsonl` is read up to this, from its end.
const MAX_APPROVALS_READ: u64 = 1 << 20;

/// A thread read back at start.
#[derive(Debug, Clone)]
pub struct StoredThread {
    pub info: ThreadInfo,
    pub next_seq: u64,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
}

/// Replaces `path` with `bytes`: a unique temporary file in the same
/// directory, created exclusively, written, synced and renamed over it.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| io::Error::other("no parent directory"))?;
    let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| io::Error::other("bad file name"))?;
    let tmp = loop {
        let candidate = dir.join(format!(".{name}.{:016x}.tmp", rand::random::<u64>()));
        match OpenOptions::new().write(true).create_new(true).mode(0o600).open(&candidate) {
            Ok(file) => break (candidate, file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };
    let (tmp_path, mut file) = tmp;
    let result =
        file.write_all(bytes).and_then(|()| file.sync_all()).and_then(|()| fs::rename(&tmp_path, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result?;
    if let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Moves an unreadable file aside so nothing overwrites it.
fn keep_unreadable(path: &Path) {
    let mut aside = path.as_os_str().to_owned();
    aside.push(format!(".unreadable-{}", now_ms()));
    match fs::rename(path, &aside) {
        Ok(()) => tracing::warn!("kept an unreadable store file aside"),
        Err(e) => tracing::warn!("couldn't keep an unreadable store file aside: {e}"),
    }
}

/// Reads and parses a JSON file. Missing: `None`. Unparseable: kept aside, `None`.
fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!("couldn't read a store file: {e}");
            return None;
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(value) => Some(value),
        Err(_) => {
            keep_unreadable(path);
            None
        }
    }
}

#[derive(Debug)]
pub struct Store {
    root: PathBuf,
    /// Lines in each thread's `items.jsonl`, counted on first append.
    lines: HashMap<String, usize>,
}

impl Store {
    pub fn open(root: PathBuf) -> io::Result<Store> {
        create_private_dir(&root.join("threads"))?;
        Ok(Store { root, lines: HashMap::new() })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn thread_dir(&self, thread_id: &str) -> io::Result<PathBuf> {
        if !api::is_valid_id(thread_id) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "bad thread id"));
        }
        Ok(self.root.join("threads").join(thread_id))
    }

    pub fn load_folders(&self) -> Vec<Folder> {
        let Some(file) = read_json::<FoldersFile>(&self.root.join("folders.json")) else { return Vec::new() };
        file.folders
            .into_iter()
            .filter(|f| api::is_valid_folder_id(&f.id) && Path::new(&f.path).is_absolute())
            .take(api::MAX_FOLDERS)
            .map(|f| Folder { id: f.id, label: f.label, path: f.path, exists: false, added_ms: f.added_ms })
            .collect()
    }

    pub fn save_folders(&self, folders: &[Folder]) -> io::Result<()> {
        let file = FoldersFile {
            version: VERSION,
            folders: folders
                .iter()
                .map(|f| StoredFolder {
                    id: f.id.clone(),
                    label: f.label.clone(),
                    path: f.path.clone(),
                    added_ms: f.added_ms,
                })
                .collect(),
        };
        let bytes = serde_json::to_vec(&file).map_err(io::Error::other)?;
        atomic_write(&self.root.join("folders.json"), &bytes)
    }

    /// The host's own settings (`settings.json`); the defaults when there are none.
    pub fn load_settings(&self) -> HostSettings {
        read_json::<SettingsFile>(&self.root.join("settings.json"))
            .map_or_else(HostSettings::default, |f| HostSettings { follow_sessions: f.follow_sessions })
    }

    pub fn save_settings(&self, settings: &HostSettings) -> io::Result<()> {
        let file = SettingsFile { version: VERSION, follow_sessions: settings.follow_sessions };
        let bytes = serde_json::to_vec(&file).map_err(io::Error::other)?;
        atomic_write(&self.root.join("settings.json"), &bytes)
    }

    /// Every stored thread whose directory name and meta are valid.
    pub fn load_threads(&self) -> Vec<StoredThread> {
        let Ok(entries) = fs::read_dir(self.root.join("threads")) else { return Vec::new() };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(id) = name.to_str() else { continue };
            if !api::is_valid_id(id) || !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let Some(meta) = read_json::<MetaFile>(&entry.path().join("meta.json")) else { continue };
            if meta.thread.id != id {
                continue;
            }
            // The items may be ahead of the meta after a crash.
            let max_seq =
                self.read_items(id).ok().and_then(|items| items.keys().next_back().copied()).unwrap_or(0);
            let next_seq = meta.next_seq.max(max_seq + 1).max(1);
            out.push(StoredThread { info: meta.thread, next_seq });
        }
        out
    }

    pub fn save_meta(&self, info: &ThreadInfo, next_seq: u64) -> io::Result<()> {
        let dir = self.thread_dir(&info.id)?;
        create_private_dir(&dir)?;
        let bytes = serde_json::to_vec(&MetaFile { version: VERSION, thread: info.clone(), next_seq })
            .map_err(io::Error::other)?;
        atomic_write(&dir.join("meta.json"), &bytes)
    }

    pub fn append_item(&mut self, thread_id: &str, item: &Item) -> io::Result<()> {
        let dir = self.thread_dir(thread_id)?;
        create_private_dir(&dir)?;
        let path = dir.join("items.jsonl");
        let count = match self.lines.get(thread_id) {
            Some(&n) => n,
            None => count_lines(&path),
        };
        let mut line = serde_json::to_vec(item).map_err(io::Error::other)?;
        line.push(b'\n');
        let mut file = OpenOptions::new().create(true).append(true).mode(0o600).open(&path)?;
        file.write_all(&line)?;
        let count = count + 1;
        self.lines.insert(thread_id.to_owned(), count);
        if count > COMPACT_LINES {
            self.compact(thread_id)?;
        }
        Ok(())
    }

    /// Rewrites `items.jsonl` with the newest [`api::MAX_ITEMS_KEPT`] items
    /// at their highest rev.
    pub fn compact(&mut self, thread_id: &str) -> io::Result<()> {
        let items = self.read_items(thread_id)?;
        let keep: Vec<&Item> = items.values().rev().take(api::MAX_ITEMS_KEPT).collect();
        let mut bytes = Vec::new();
        for item in keep.iter().rev() {
            serde_json::to_writer(&mut bytes, item).map_err(io::Error::other)?;
            bytes.push(b'\n');
        }
        atomic_write(&self.thread_dir(thread_id)?.join("items.jsonl"), &bytes)?;
        self.lines.insert(thread_id.to_owned(), keep.len());
        Ok(())
    }

    /// The thread's items by `seq`, each at its highest rev.
    pub fn read_items(&self, thread_id: &str) -> io::Result<BTreeMap<u64, Item>> {
        let path = self.thread_dir(thread_id)?.join("items.jsonl");
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(e) => return Err(e),
        };
        let mut reader = BufReader::new(file);
        let mut items: BTreeMap<u64, Item> = BTreeMap::new();
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match crate::lines::read_line_blocking(&mut reader, &mut buf, MAX_ITEM_LINE)? {
                crate::lines::LineRead::Eof => break,
                crate::lines::LineRead::TooLong => {
                    // Skip the rest of the oversized line.
                    let mut rest = Vec::new();
                    reader.read_until(b'\n', &mut rest)?;
                    continue;
                }
                crate::lines::LineRead::Line => {}
            }
            let Ok(item) = serde_json::from_slice::<Item>(&buf) else { continue };
            match items.get(&item.seq) {
                Some(old) if old.rev > item.rev => {}
                _ => {
                    items.insert(item.seq, item);
                }
            }
        }
        Ok(items)
    }

    pub fn append_approval(&mut self, thread_id: &str, record: &ApprovalRecord) -> io::Result<()> {
        let dir = self.thread_dir(thread_id)?;
        create_private_dir(&dir)?;
        let mut line = serde_json::to_vec(record).map_err(io::Error::other)?;
        line.push(b'\n');
        let mut file =
            OpenOptions::new().create(true).append(true).mode(0o600).open(dir.join("approvals.jsonl"))?;
        file.write_all(&line)
    }

    /// The thread's answered approvals, oldest first (the newest 1 MiB of them).
    pub fn read_approvals(&self, thread_id: &str) -> io::Result<Vec<ApprovalRecord>> {
        use std::io::{Read, Seek, SeekFrom};
        let path = self.thread_dir(thread_id)?.join("approvals.jsonl");
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let size = file.metadata()?.len();
        let from = size.saturating_sub(MAX_APPROVALS_READ);
        file.seek(SeekFrom::Start(from))?;
        let mut bytes = Vec::new();
        file.take(MAX_APPROVALS_READ).read_to_end(&mut bytes)?;
        let mut lines = bytes.split(|&b| b == b'\n');
        if from > 0 {
            lines.next();
        }
        Ok(lines.filter_map(|l| serde_json::from_slice(l).ok()).collect())
    }

    pub fn delete_thread(&mut self, thread_id: &str) -> io::Result<()> {
        let dir = self.thread_dir(thread_id)?;
        self.lines.remove(thread_id);
        match fs::remove_dir_all(dir) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}

fn count_lines(path: &Path) -> usize {
    match File::open(path) {
        Ok(file) => BufReader::new(file).split(b'\n').count(),
        Err(_) => 0,
    }
}

// ---- The store thread ----

enum Op {
    SaveFolders(Vec<Folder>, oneshot::Sender<io::Result<()>>),
    SaveSettings(HostSettings, oneshot::Sender<io::Result<()>>),
    SaveMeta(Box<ThreadInfo>, u64),
    AppendItem(String, Box<Item>),
    AppendApproval(String, ApprovalRecord),
    ReadApprovals(String, oneshot::Sender<io::Result<Vec<ApprovalRecord>>>),
    ReadItems(String, oneshot::Sender<io::Result<BTreeMap<u64, Item>>>),
    Delete(String),
    Flush(oneshot::Sender<()>),
}

/// The store, run on its own thread. Cheap to clone.
#[derive(Clone)]
pub struct StoreHandle {
    tx: mpsc::Sender<Op>,
}

impl std::fmt::Debug for StoreHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoreHandle")
    }
}

impl StoreHandle {
    pub fn spawn(mut store: Store) -> io::Result<StoreHandle> {
        let (tx, rx) = mpsc::channel::<Op>();
        std::thread::Builder::new().name("g7-agents-store".into()).spawn(move || {
            for op in rx {
                match op {
                    Op::SaveFolders(folders, reply) => {
                        let _ = reply.send(store.save_folders(&folders));
                    }
                    Op::SaveSettings(settings, reply) => {
                        let _ = reply.send(store.save_settings(&settings));
                    }
                    Op::SaveMeta(info, next_seq) => {
                        if let Err(e) = store.save_meta(&info, next_seq) {
                            tracing::warn!("couldn't save a thread: {e}");
                        }
                    }
                    Op::AppendItem(thread_id, item) => {
                        if let Err(e) = store.append_item(&thread_id, &item) {
                            tracing::warn!("couldn't save a thread item: {e}");
                        }
                    }
                    Op::ReadItems(thread_id, reply) => {
                        let _ = reply.send(store.read_items(&thread_id));
                    }
                    Op::AppendApproval(thread_id, record) => {
                        if let Err(e) = store.append_approval(&thread_id, &record) {
                            tracing::warn!("couldn't save an answered approval: {e}");
                        }
                    }
                    Op::ReadApprovals(thread_id, reply) => {
                        let _ = reply.send(store.read_approvals(&thread_id));
                    }
                    Op::Delete(thread_id) => {
                        if let Err(e) = store.delete_thread(&thread_id) {
                            tracing::warn!("couldn't delete a thread: {e}");
                        }
                    }
                    Op::Flush(reply) => {
                        let _ = reply.send(());
                    }
                }
            }
        })?;
        Ok(StoreHandle { tx })
    }

    fn send(&self, op: Op) {
        if self.tx.send(op).is_err() {
            tracing::warn!("the store thread is gone");
        }
    }

    pub async fn save_folders(&self, folders: Vec<Folder>) -> io::Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send(Op::SaveFolders(folders, tx));
        rx.await.unwrap_or_else(|_| Err(io::Error::other("store stopped")))
    }

    pub async fn save_settings(&self, settings: HostSettings) -> io::Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send(Op::SaveSettings(settings, tx));
        rx.await.unwrap_or_else(|_| Err(io::Error::other("store stopped")))
    }

    pub fn save_meta(&self, info: &ThreadInfo, next_seq: u64) {
        self.send(Op::SaveMeta(Box::new(info.clone()), next_seq));
    }

    pub fn append_item(&self, thread_id: &str, item: &Item) {
        self.send(Op::AppendItem(thread_id.to_owned(), Box::new(item.clone())));
    }

    pub async fn read_items(&self, thread_id: &str) -> io::Result<BTreeMap<u64, Item>> {
        let (tx, rx) = oneshot::channel();
        self.send(Op::ReadItems(thread_id.to_owned(), tx));
        rx.await.unwrap_or_else(|_| Err(io::Error::other("store stopped")))
    }

    pub fn append_approval(&self, thread_id: &str, record: ApprovalRecord) {
        self.send(Op::AppendApproval(thread_id.to_owned(), record));
    }

    pub async fn read_approvals(&self, thread_id: &str) -> io::Result<Vec<ApprovalRecord>> {
        let (tx, rx) = oneshot::channel();
        self.send(Op::ReadApprovals(thread_id.to_owned(), tx));
        rx.await.unwrap_or_else(|_| Err(io::Error::other("store stopped")))
    }

    pub fn delete(&self, thread_id: &str) {
        self.send(Op::Delete(thread_id.to_owned()));
    }

    /// Waits until every earlier operation is done.
    pub async fn flush(&self) {
        let (tx, rx) = oneshot::channel();
        self.send(Op::Flush(tx));
        let _ = rx.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::*;
    use std::os::unix::fs::PermissionsExt;

    pub(crate) fn info(id: &str, state: State) -> ThreadInfo {
        ThreadInfo {
            id: id.into(),
            agent: AgentId::Claude,
            folder_id: "f000000000001".into(),
            folder_label: "w".into(),
            title: "t".into(),
            state,
            mode: Mode::Supervised,
            origin: Origin::launcher(),
            created_ms: 1,
            updated_ms: 1,
            last_seq: 0,
            pending: 0,
            summary: None,
            error: None,
            native_id: None,
            step: None,
            plan: None,
            state_since_ms: 0,
            subdir: None,
            continued: false,
        }
    }

    fn item(seq: u64, rev: u32, text: &str) -> Item {
        Item {
            seq,
            id: format!("i{seq}"),
            kind: ItemKind::Assistant,
            ts_ms: 1,
            rev,
            text: Some(text.into()),
            truncated: false,
            origin: None,
            tool: None,
        }
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temporary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.json");
        atomic_write(&path, b"one").unwrap();
        atomic_write(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn an_unreadable_file_is_kept_aside_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().to_owned()).unwrap();
        fs::write(dir.path().join("folders.json"), b"{not json").unwrap();
        assert!(store.load_folders().is_empty());
        let aside: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("folders.json.unreadable-"))
            .collect();
        assert_eq!(aside.len(), 1);
        assert_eq!(fs::read(aside[0].path()).unwrap(), b"{not json");
        assert!(!dir.path().join("folders.json").exists());
    }

    #[test]
    fn folders_and_threads_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().to_owned()).unwrap();
        let folder = Folder {
            id: "f0123456789ab".into(),
            label: "w".into(),
            path: "/tmp".into(),
            exists: true,
            added_ms: 3,
        };
        store.save_folders(std::slice::from_ref(&folder)).unwrap();
        let loaded = store.load_folders();
        assert_eq!(loaded[0].id, folder.id);
        assert!(!loaded[0].exists);

        let id = "ab".repeat(16);
        store.save_meta(&info(&id, State::Done), 2).unwrap();
        store.append_item(&id, &item(1, 0, "a")).unwrap();
        store.append_item(&id, &item(2, 0, "b")).unwrap();
        store.append_item(&id, &item(2, 1, "b2")).unwrap();
        let items = store.read_items(&id).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[&2].text.as_deref(), Some("b2"));
        let threads = store.load_threads();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].next_seq, 3, "next_seq follows the items when they're ahead");
        store.delete_thread(&id).unwrap();
        assert!(store.load_threads().is_empty());
    }

    #[test]
    fn answered_approvals_are_kept_and_go_with_the_thread() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().to_owned()).unwrap();
        let id = "ef".repeat(16);
        assert!(store.read_approvals(&id).unwrap().is_empty());
        let record = ApprovalRecord {
            tool_seq: Some(3),
            outcome: ApprovalOutcome::Allowed,
            by: Some(Origin::phone("p", "Pixel")),
            ts_ms: 9,
        };
        store.append_approval(&id, &record).unwrap();
        store
            .append_approval(&id, &ApprovalRecord { outcome: ApprovalOutcome::Denied, ..record.clone() })
            .unwrap();
        let read = store.read_approvals(&id).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0], record);
        assert!(store.append_approval("../x", &record).is_err());
        store.delete_thread(&id).unwrap();
        assert!(store.read_approvals(&id).unwrap().is_empty());
    }

    #[test]
    fn settings_default_on_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().to_owned()).unwrap();
        assert!(store.load_settings().follow_sessions, "on by default");
        store.save_settings(&HostSettings { follow_sessions: false }).unwrap();
        assert!(!store.load_settings().follow_sessions);
        fs::write(dir.path().join("settings.json"), br#"{"version":1}"#).unwrap();
        assert!(store.load_settings().follow_sessions);
    }

    #[test]
    fn items_are_compacted_to_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().to_owned()).unwrap();
        let id = "cd".repeat(16);
        for seq in 1..=(COMPACT_LINES as u64 + 1) {
            store.append_item(&id, &item(seq, 0, "x")).unwrap();
        }
        let items = store.read_items(&id).unwrap();
        assert_eq!(items.len(), api::MAX_ITEMS_KEPT);
        assert_eq!(*items.keys().next().unwrap(), COMPACT_LINES as u64 + 2 - api::MAX_ITEMS_KEPT as u64);
        assert_eq!(
            count_lines(&dir.path().join("threads").join(&id).join("items.jsonl")),
            api::MAX_ITEMS_KEPT
        );
    }

    #[test]
    fn ids_that_arent_hex_never_reach_a_path() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().to_owned()).unwrap();
        for bad in ["../x", "", "A1", "a/b", ".."] {
            assert!(store.read_items(bad).is_err());
            assert!(store.append_item(bad, &item(1, 0, "x")).is_err());
            assert!(store.delete_thread(bad).is_err());
            assert!(store.save_meta(&info(bad, State::Done), 1).is_err());
        }
        fs::create_dir_all(dir.path().join("threads/NOT-HEX")).unwrap();
        assert!(store.load_threads().is_empty());
    }
}
