// SPDX-License-Identifier: MPL-2.0
//! The host's files under `$XDG_DATA_HOME/g7-agents/`: `folders.json`,
//! `settings.json`, `drafts.json` and `history.jsonl`; and the store thread
//! ([`StoreHandle`]) that owns the thread database ([`Db`], `agents.db`).
//!
//! JSON files are replaced atomically (a unique temporary file created
//! exclusively, written in full, synced, renamed). A file that can't be read
//! is kept aside as `<name>.unreadable-<ms>`, never overwritten. All writes
//! run on one thread so they keep their order and never block the async
//! runtime; the database batches item writes and commits at least every
//! [`db::BATCH_WINDOW`].

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::api::{self, ApprovalOutcome, Draft, Folder, HostSettings, Origin};
use crate::host::db::{self, Db};

/// Store format version.
const VERSION: u32 = 1;

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
    #[serde(default = "yes")]
    mcp: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    mcp_seen: Vec<api::FolderMcp>,
    #[serde(default = "yes")]
    checkpoints: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    follow_remote: bool,
    /// Phase 6: the folder's own commands, and "Phones can run these".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    scripts: Vec<api::Script>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    phone_scripts: bool,
}

#[derive(Serialize, Deserialize)]
struct SettingsFile {
    version: u32,
    #[serde(default = "yes")]
    follow_sessions: bool,
    #[serde(default = "yes")]
    keep_awake: bool,
    #[serde(default = "yes")]
    limit_alarm: bool,
}

fn yes() -> bool {
    true
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

#[derive(Serialize, Deserialize)]
struct DraftsFile {
    version: u32,
    /// Oldest first.
    drafts: Vec<Draft>,
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
}

impl Store {
    pub fn open(root: PathBuf) -> io::Result<Store> {
        create_private_dir(&root)?;
        Ok(Store { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn load_folders(&self) -> Vec<Folder> {
        let Some(file) = read_json::<FoldersFile>(&self.root.join("folders.json")) else { return Vec::new() };
        file.folders
            .into_iter()
            .filter(|f| api::is_valid_folder_id(&f.id) && Path::new(&f.path).is_absolute())
            .take(api::MAX_FOLDERS)
            .map(|f| Folder {
                id: f.id,
                label: f.label,
                path: f.path,
                exists: false,
                added_ms: f.added_ms,
                mcp: f.mcp,
                mcp_seen: f.mcp_seen,
                checkpoints: f.checkpoints,
                follow_remote: f.follow_remote,
                scripts: crate::host::scripts::loaded(f.scripts),
                phone_scripts: f.phone_scripts,
            })
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
                    mcp: f.mcp,
                    mcp_seen: f.mcp_seen.clone(),
                    checkpoints: f.checkpoints,
                    follow_remote: f.follow_remote,
                    scripts: f.scripts.clone(),
                    phone_scripts: f.phone_scripts,
                })
                .collect(),
        };
        let bytes = serde_json::to_vec(&file).map_err(io::Error::other)?;
        atomic_write(&self.root.join("folders.json"), &bytes)
    }

    /// The host's own settings (`settings.json`); the defaults when there are none.
    pub fn load_settings(&self) -> HostSettings {
        read_json::<SettingsFile>(&self.root.join("settings.json")).map_or_else(HostSettings::default, |f| {
            HostSettings {
                follow_sessions: f.follow_sessions,
                keep_awake: f.keep_awake,
                limit_alarm: f.limit_alarm,
            }
        })
    }

    pub fn save_settings(&self, settings: &HostSettings) -> io::Result<()> {
        let file = SettingsFile {
            version: VERSION,
            follow_sessions: settings.follow_sessions,
            keep_awake: settings.keep_awake,
            limit_alarm: settings.limit_alarm,
        };
        let bytes = serde_json::to_vec(&file).map_err(io::Error::other)?;
        atomic_write(&self.root.join("settings.json"), &bytes)
    }

    /// The composers' drafts (`drafts.json`), oldest first.
    pub fn load_drafts(&self) -> Vec<Draft> {
        let mut drafts: Vec<Draft> = read_json::<DraftsFile>(&self.root.join("drafts.json"))
            .map(|f| f.drafts)
            .unwrap_or_default()
            .into_iter()
            .filter(|d| api::is_valid_draft_key(&d.key) && d.text.len() <= api::MAX_PROMPT)
            .collect();
        let excess = drafts.len().saturating_sub(api::MAX_DRAFTS);
        drafts.drain(..excess);
        drafts
    }

    pub fn save_drafts(&self, drafts: &[Draft]) -> io::Result<()> {
        let bytes = serde_json::to_vec(&DraftsFile { version: VERSION, drafts: drafts.to_vec() })
            .map_err(io::Error::other)?;
        atomic_write(&self.root.join("drafts.json"), &bytes)
    }

    /// The prompts sent from this PC (`history.jsonl`), oldest first, at
    /// most [`api::MAX_HISTORY`].
    pub fn load_history(&self) -> Vec<String> {
        let path = self.root.join("history.jsonl");
        let Ok(file) = File::open(&path) else { return Vec::new() };
        let mut lines: std::collections::VecDeque<String> = std::collections::VecDeque::new();
        for line in BufReader::new(file.take(4 * 1024 * 1024)).lines() {
            let Ok(line) = line else { break };
            if let Ok(text) = serde_json::from_str::<String>(&line) {
                lines.push_back(api::cut(&text, api::MAX_HISTORY_PROMPT).to_owned());
                if lines.len() > api::MAX_HISTORY {
                    lines.pop_front();
                }
            }
        }
        lines.into()
    }

    /// Appends one prompt to `history.jsonl` (0600).
    pub fn append_history(&self, text: &str) -> io::Result<()> {
        let mut line = serde_json::to_string(text).map_err(io::Error::other)?;
        line.push('\n');
        let mut file =
            OpenOptions::new().create(true).append(true).mode(0o600).open(self.root.join("history.jsonl"))?;
        file.write_all(line.as_bytes())
    }

    /// Replaces `history.jsonl` with `lines`, atomically.
    pub fn rewrite_history(&self, lines: &[String]) -> io::Result<()> {
        let mut bytes = Vec::new();
        for line in lines {
            bytes.extend_from_slice(serde_json::to_string(line).map_err(io::Error::other)?.as_bytes());
            bytes.push(b'\n');
        }
        atomic_write(&self.root.join("history.jsonl"), &bytes)
    }
}

// ---- The store thread ----

/// Work for the thread database, run on the store thread.
type DbWork = Box<dyn FnOnce(Option<&mut Db>) + Send>;

enum Op {
    SaveFolders(Vec<Folder>, oneshot::Sender<io::Result<()>>),
    SaveSettings(HostSettings, oneshot::Sender<io::Result<()>>),
    Db(DbWork),
    Flush(oneshot::Sender<()>),
    SaveDrafts(Vec<Draft>),
    AppendHistory(String),
    RewriteHistory(Vec<String>),
}

/// The store, run on its own thread. Cheap to clone.
#[derive(Clone)]
pub struct StoreHandle {
    tx: mpsc::Sender<Op>,
    /// The database's read-only connection: `None` when it couldn't be opened.
    reader: Option<std::sync::Arc<db::Reader>>,
}

impl std::fmt::Debug for StoreHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoreHandle")
    }
}

/// What a thread method answers when the database couldn't be opened.
pub fn no_store() -> io::Error {
    io::Error::other("G7 couldn't open its thread store")
}

impl StoreHandle {
    /// Runs `store` and `db` (`None`: the database couldn't be opened, and
    /// every thread write fails) on a thread of their own.
    pub fn spawn(store: Store, db: Option<Db>) -> io::Result<StoreHandle> {
        let reader = match db.as_ref().map(Db::reader) {
            Some(Ok(reader)) => Some(std::sync::Arc::new(reader)),
            Some(Err(e)) => {
                tracing::warn!("couldn't open a reader on the thread store: {e}");
                None
            }
            None => None,
        };
        let (tx, rx) = mpsc::channel::<Op>();
        std::thread::Builder::new().name("g7-agents-store".into()).spawn(move || run(store, db, &rx))?;
        Ok(StoreHandle { tx, reader })
    }

    fn send(&self, op: Op) {
        if self.tx.send(op).is_err() {
            tracing::warn!("the store thread is gone");
        }
    }

    /// The database is open.
    pub fn has_db(&self) -> bool {
        self.reader.is_some()
    }

    /// The read-only connection, for searches and sizes off the store thread.
    pub fn reader(&self) -> Option<std::sync::Arc<db::Reader>> {
        self.reader.clone()
    }

    /// Runs `work` on the database, after every earlier write, and waits
    /// for its answer. Reads made here see writes not yet committed.
    pub async fn db<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Db) -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        let (tx, rx) = oneshot::channel();
        self.send(Op::Db(Box::new(move |db| {
            let _ = tx.send(db.ok_or_else(no_store).and_then(work));
        })));
        rx.await.unwrap_or_else(|_| Err(io::Error::other("store stopped")))
    }

    /// [`StoreHandle::db`] for a blocking thread (never the async runtime's).
    pub fn db_blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Db) -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send(Op::Db(Box::new(move |db| {
            let _ = tx.send(db.ok_or_else(no_store).and_then(work));
        })));
        rx.recv().unwrap_or_else(|_| Err(io::Error::other("store stopped")))
    }

    /// Runs `work` on the database later, in order; a failure is logged as `what`.
    pub fn db_later(
        &self,
        what: &'static str,
        work: impl FnOnce(&mut Db) -> io::Result<()> + Send + 'static,
    ) {
        self.send(Op::Db(Box::new(move |db| {
            if let Err(e) = db.ok_or_else(no_store).and_then(work) {
                tracing::warn!("couldn't {what}: {e}");
            }
        })));
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

    pub fn save_thread(&self, info: &api::ThreadInfo, cols: db::ThreadCols) {
        // Runs live as long as the host: never stored.
        let info = api::ThreadInfo { runs: Vec::new(), ..info.clone() };
        self.db_later("save a thread", move |db| db.save_thread(&info, &cols));
    }

    pub fn append_item(&self, thread_id: &str, item: &api::Item) {
        let (thread, item) = (thread_id.to_owned(), item.clone());
        self.db_later("save a thread item", move |db| db.append_item(&thread, &item));
    }

    /// Every item of a thread that wasn't rewound away, by `seq`.
    pub async fn all_items(&self, thread_id: &str) -> io::Result<std::collections::BTreeMap<u64, api::Item>> {
        let thread = thread_id.to_owned();
        self.db(move |db| db.all_items(&thread)).await
    }

    pub fn append_approval(&self, thread_id: &str, record: ApprovalRecord) {
        let thread = thread_id.to_owned();
        self.db_later("save an answered approval", move |db| db.append_approval(&thread, &record));
    }

    pub async fn read_approvals(&self, thread_id: &str) -> io::Result<Vec<ApprovalRecord>> {
        let thread = thread_id.to_owned();
        self.db(move |db| db.read_approvals(&thread)).await
    }

    pub fn delete(&self, thread_id: &str) {
        let thread = thread_id.to_owned();
        self.db_later("delete a thread", move |db| db.delete_thread(&thread));
    }

    pub fn save_drafts(&self, drafts: Vec<Draft>) {
        self.send(Op::SaveDrafts(drafts));
    }

    pub fn append_history(&self, text: String) {
        self.send(Op::AppendHistory(text));
    }

    pub fn rewrite_history(&self, lines: Vec<String>) {
        self.send(Op::RewriteHistory(lines));
    }

    /// Waits until every earlier operation is done and committed.
    pub async fn flush(&self) {
        let (tx, rx) = oneshot::channel();
        self.send(Op::Flush(tx));
        let _ = rx.await;
    }
}

/// The store thread: each operation in order; the database's batch is
/// committed when its window ends, on a flush, and when the host stops.
fn run(store: Store, mut db: Option<Db>, rx: &mpsc::Receiver<Op>) {
    loop {
        let deadline = db.as_ref().and_then(Db::flush_deadline);
        let op = match deadline {
            Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(op) => Some(op),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(op) => Some(op),
                Err(_) => break,
            },
        };
        match op {
            Some(Op::SaveFolders(folders, reply)) => {
                let _ = reply.send(store.save_folders(&folders));
            }
            Some(Op::SaveSettings(settings, reply)) => {
                let _ = reply.send(store.save_settings(&settings));
            }
            Some(Op::Db(work)) => work(db.as_mut()),
            Some(Op::Flush(reply)) => {
                if let Some(Err(e)) = db.as_mut().map(Db::flush) {
                    tracing::warn!("couldn't commit the thread store: {e}");
                }
                let _ = reply.send(());
            }
            Some(Op::SaveDrafts(drafts)) => {
                if let Err(e) = store.save_drafts(&drafts) {
                    tracing::warn!("couldn't save the drafts: {e}");
                }
            }
            Some(Op::AppendHistory(text)) => {
                if let Err(e) = store.append_history(&text) {
                    tracing::warn!("couldn't save a prompt to the history: {e}");
                }
            }
            Some(Op::RewriteHistory(lines)) => {
                if let Err(e) = store.rewrite_history(&lines) {
                    tracing::warn!("couldn't rewrite the history: {e}");
                }
            }
            None => {}
        }
        if let Some(Err(e)) = db.as_mut().map(Db::tick) {
            tracing::warn!("couldn't commit the thread store: {e}");
        }
    }
    if let Some(Err(e)) = db.as_mut().map(Db::flush) {
        tracing::warn!("couldn't commit the thread store: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::*;
    use std::os::unix::fs::PermissionsExt;

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
    fn folders_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().to_owned()).unwrap();
        let folder = Folder {
            id: "f0123456789ab".into(),
            label: "w".into(),
            path: "/tmp".into(),
            exists: true,
            added_ms: 3,
            mcp: false,
            mcp_seen: vec![FolderMcp { agent: AgentId::Claude, names: vec!["github".into()] }],
            checkpoints: false,
            follow_remote: false,
            scripts: Vec::new(),
            phone_scripts: false,
        };
        store.save_folders(std::slice::from_ref(&folder)).unwrap();
        let loaded = store.load_folders();
        assert_eq!(loaded[0].id, folder.id);
        assert!(!loaded[0].exists);
        assert!(!loaded[0].checkpoints, "the restore points switch is kept");
        let plain = fs::read_to_string(dir.path().join("folders.json")).unwrap();
        assert!(!plain.contains("scripts"), "not written while empty and off: {plain}");

        // Scripts and the phones' switch are kept as they were.
        let script =
            api::Script { id: "s0123abcd".into(), name: "dev".into(), command: "npm run dev".into() };
        let with = Folder { scripts: vec![script.clone()], phone_scripts: true, ..folder.clone() };
        store.save_folders(std::slice::from_ref(&with)).unwrap();
        let loaded = store.load_folders();
        assert_eq!(
            (loaded[0].scripts.as_slice(), loaded[0].phone_scripts),
            (std::slice::from_ref(&script), true)
        );
        let file: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join("folders.json")).unwrap()).unwrap();
        assert_eq!(file["version"], 1, "no migration");
    }

    #[test]
    fn an_older_folders_file_loads_and_a_script_out_of_bounds_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().to_owned()).unwrap();
        let long = "n".repeat(api::MAX_SCRIPT_NAME + 1);
        let file = serde_json::json!({"version": 1, "folders": [
            {"id": "f0123456789ab", "label": "old", "path": "/tmp", "added_ms": 1},
            {"id": "f0123456789ac", "label": "new", "path": "/var", "added_ms": 2, "phone_scripts": true,
             "scripts": [
                {"id": "s00000001", "name": "dev", "command": "npm run dev"},
                {"id": "nope", "name": "bad id", "command": "x"},
                {"id": "s00000002", "name": long, "command": "x"},
                {"id": "s00000003", "name": "two", "command": "a\nb"},
                {"id": "s00000004", "name": "DEV", "command": "again"},
                {"id": "s00000005", "name": "test", "command": "make test"}
             ]}
        ]});
        fs::write(dir.path().join("folders.json"), file.to_string()).unwrap();
        let loaded = store.load_folders();
        assert_eq!(loaded.len(), 2);
        assert!(loaded[0].scripts.is_empty() && !loaded[0].phone_scripts, "an older file: neither field");
        let names: Vec<&str> = loaded[1].scripts.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["dev", "test"]);
        assert!(loaded[1].phone_scripts);
    }

    #[tokio::test]
    async fn a_threads_runs_are_never_stored() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let store = StoreHandle::spawn(Store::open(dir.path().to_owned()).unwrap(), Some(db)).unwrap();
        let brief = api::RunBrief {
            id: "0123456789abcdef".into(),
            script_id: "s00000001".into(),
            name: "dev".into(),
            phase: api::RunPhase::Running,
            exit: None,
        };
        let info = api::ThreadInfo { runs: vec![brief], ..crate::host::threads::test_info(AgentId::Claude) };
        store.save_thread(&info, db::ThreadCols { next_seq: 1, next_turn: 1, ..Default::default() });
        let loaded = store.db(|db| db.load_threads()).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].info.runs.is_empty(), "no run outlives its host");
    }

    #[test]
    fn settings_default_on_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().to_owned()).unwrap();
        assert!(store.load_settings().follow_sessions, "on by default");
        store.save_settings(&HostSettings { follow_sessions: false, ..HostSettings::default() }).unwrap();
        assert!(!store.load_settings().follow_sessions);
        fs::write(dir.path().join("settings.json"), br#"{"version":1}"#).unwrap();
        assert!(store.load_settings().follow_sessions);
    }
}
