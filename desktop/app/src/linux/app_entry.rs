// SPDX-License-Identifier: GPL-3.0-or-later
//! Which installed app a window belongs to, from its desktop entry: the app's
//! name, and its icon as a small PNG for a phone to show over its typing box.
//!
//! A window is matched by its WM class (or Wayland app id): to the entry of
//! that id, then to an entry whose `StartupWMClass` is that class, then by
//! the systemd unit its process runs in, which desktops name after the app.
//! Entries and `/proc` are read as untrusted files: regular files only, never
//! more than a bound, and never by an id that could leave its folder.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::OsString,
    fmt::Write as _,
    fs::{self, File, OpenOptions},
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use g7link_core::remote_limits;

/// Folders taken from `XDG_DATA_DIRS` at most.
const MAX_DATA_DIRS: usize = 16;
/// Most bytes of a desktop entry read; a bigger one is passed over.
const MAX_ENTRY_BYTES: u64 = 64 * 1024;
/// Most folder entries looked at when indexing `StartupWMClass`.
const MAX_SCANNED: usize = 4000;
/// The `StartupWMClass` index is built again at most this often, on a miss.
const INDEX_REFRESH: Duration = Duration::from_secs(60);
/// A window that matched nothing is looked up again after this, and an app
/// whose icon couldn't be drawn is drawn again.
const RETRY: Duration = Duration::from_secs(60);
/// Apps remembered at most; the least recently used goes first.
const MAX_KNOWN: usize = 32;
/// Windows that matched nothing remembered at most.
const MAX_MISSES: usize = 256;
/// Most bytes of a desktop id or a WM class.
const MAX_ID_LEN: usize = 255;
/// Most bytes of an entry's `Icon`.
const MAX_ICON_VALUE: usize = 1024;
/// Largest icon file drawn from an absolute path.
const MAX_ICON_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Most bytes of `/proc/<pid>/cgroup` read.
const MAX_CGROUP_BYTES: u64 = 16 * 1024;
/// Locales taken from `LANGUAGE` at most, and the longest one.
const MAX_LANGUAGES: usize = 8;
const MAX_LOCALE_LEN: usize = 32;
/// The icon's edge in pixels; the second when the first makes too big a PNG.
const ICON_EDGES: [u32; 2] = [96, 64];
/// Shortest hex run taken as the random part of a unit name.
const MIN_RANDOM_HEX: usize = 16;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// An installed app, as its desktop entry describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppIdentity {
    /// The desktop entry's id, without ".desktop" ("com.anthropic.Claude").
    pub id: String,
    /// Its `Name`, in the user's language where the entry has one.
    pub name: String,
    pub icon: Option<AppIcon>,
}

/// An app's icon as the phone gets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppIcon {
    /// 16 lowercase hex characters: the first 8 bytes of the PNG's BLAKE3 hash.
    pub key: String,
    /// At most `g7link_core::remote_limits::MAX_ICON_BYTES` (16 KiB).
    pub png: Arc<Vec<u8>>,
}

static CACHE: LazyLock<Cache> = LazyLock::new(Cache::default);

/// The app a window belongs to, by its WM class (or Wayland app id) and the
/// pid of its process. Cached; may read files and render an icon the first
/// time for an app, so call it off the GUI thread. `None` when nothing matches.
pub fn identify(class: &str, pid: u32) -> Option<Arc<AppIdentity>> {
    let dirs = application_dirs();
    let languages = languages();
    let src =
        Sources { dirs: &dirs, proc: Path::new("/proc"), languages: &languages, render: render_with_qt };
    identify_in(&CACHE, &src, class, pid, Instant::now())
}

/// The icon of the app a media player's MPRIS `DesktopEntry` names (its
/// desktop id, with or without ".desktop"): its key and PNG, as
/// [`AppIcon`]. Cached with the rest; may draw the icon the first time, so
/// call it off the GUI thread.
pub(crate) fn icon_for_desktop_entry(entry: &str) -> Option<(String, Vec<u8>)> {
    let dirs = application_dirs();
    let languages = languages();
    let src =
        Sources { dirs: &dirs, proc: Path::new("/proc"), languages: &languages, render: render_with_qt };
    icon_for_entry_in(&CACHE, &src, entry, Instant::now())
}

fn icon_for_entry_in(
    cache: &Cache,
    src: &Sources<'_>,
    entry: &str,
    now: Instant,
) -> Option<(String, Vec<u8>)> {
    let entry = entry.trim();
    let id = entry.strip_suffix(".desktop").unwrap_or(entry);
    let app = identify_in(cache, src, id, 0, now)?;
    let icon = app.icon.as_ref()?;
    Some((icon.key.clone(), icon.png.to_vec()))
}

fn render_with_qt(icon: &str, edge: u32) -> Vec<u8> {
    crate::bridge::native::ffi::app_icon_png(icon, edge)
}

/// Where lookups read from: the system, or a test's folders.
struct Sources<'a> {
    /// Application folders, the first holding an id winning.
    dirs: &'a [PathBuf],
    /// Where `<pid>/cgroup` is read.
    proc: &'a Path,
    /// Locales for `Name[...]`, best first.
    languages: &'a [String],
    /// Draws an icon (a theme name or an absolute path) as a PNG of at most
    /// `edge` pixels a side; empty when it can't.
    render: fn(&str, u32) -> Vec<u8>,
}

fn identify_in(
    cache: &Cache,
    src: &Sources<'_>,
    class: &str,
    pid: u32,
    now: Instant,
) -> Option<Arc<AppIdentity>> {
    let class = class.trim();
    let class = if class.len() <= MAX_ID_LEN { class } else { "" };
    if class.is_empty() && pid == 0 {
        return None;
    }
    let missed = cache.lock().missed(class, pid, now);
    if missed {
        return None;
    }
    let found = find(cache, src, class, pid, now);
    if found.is_none() {
        cache.lock().miss(class, pid, now);
    }
    found
}

fn find(cache: &Cache, src: &Sources<'_>, class: &str, pid: u32, now: Instant) -> Option<Arc<AppIdentity>> {
    if !class.is_empty() {
        if let Some(found) = cache.by_id(src, class, class, now) {
            return Some(found);
        }
        let lower = class.to_lowercase();
        if lower != class
            && let Some(found) = cache.by_id(src, class, &lower, now)
        {
            return Some(found);
        }
        if let Some(found) = cache.by_wm_class(src, class, &lower, now) {
            return Some(found);
        }
    }
    if pid == 0 {
        return None;
    }
    let id = process_app_id(src.proc, pid)?;
    cache.by_id(src, class, &id, now)
}

/// Desktop entries by their `StartupWMClass` in lowercase.
type Index = HashMap<String, Indexed>;

struct Indexed {
    id: String,
    entry: Entry,
}

/// An app found before.
struct Known {
    class: String,
    id: String,
    identity: Arc<AppIdentity>,
    /// When its icon couldn't be drawn, which may change.
    icon_failed: Option<Instant>,
}

#[derive(Default)]
struct Cache {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The most recently used last.
    apps: VecDeque<Known>,
    /// When a class and pid last matched nothing.
    misses: HashMap<(String, u32), Instant>,
    index: Option<Arc<Index>>,
    /// When the index was last built, or a build started.
    scanned: Option<Instant>,
}

impl Cache {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The app `<id>.desktop` describes, for a window of `class`.
    fn by_id(&self, src: &Sources<'_>, class: &str, id: &str, now: Instant) -> Option<Arc<AppIdentity>> {
        if !valid_id(id) {
            return None;
        }
        let known = self.lock().known(class, id, now);
        if known.is_some() {
            return known;
        }
        let entry = read_entry(src, id)?;
        Some(self.build(src, class, id, entry, now))
    }

    /// The app whose `StartupWMClass` is `lower` in lowercase.
    fn by_wm_class(
        &self,
        src: &Sources<'_>,
        class: &str,
        lower: &str,
        now: Instant,
    ) -> Option<Arc<AppIdentity>> {
        let mut index = self.index(src, now);
        if !index.contains_key(lower) {
            index = self.rebuilt_index(src, now)?;
        }
        let Indexed { id, entry } = index.get(lower)?;
        let known = self.lock().known(class, id, now);
        if known.is_some() {
            return known;
        }
        Some(self.build(src, class, id, entry.clone(), now))
    }

    fn build(
        &self,
        src: &Sources<'_>,
        class: &str,
        id: &str,
        entry: Entry,
        now: Instant,
    ) -> Arc<AppIdentity> {
        let rendered = entry.icon.as_deref().map(|icon| make_icon(src.render, icon));
        let failed = matches!(rendered, Some(Rendered::Failed)).then_some(now);
        let icon = match rendered {
            Some(Rendered::Icon(icon)) => Some(icon),
            _ => None,
        };
        let identity = Arc::new(AppIdentity { id: id.to_owned(), name: entry.name, icon });
        self.lock().remember(class, id, Arc::clone(&identity), failed);
        identity
    }

    /// The `StartupWMClass` index, built the first time it's needed.
    fn index(&self, src: &Sources<'_>, now: Instant) -> Arc<Index> {
        let current = self.lock().index.clone();
        if let Some(index) = current {
            return index;
        }
        self.lock().scanned = Some(now);
        self.store(scan(src))
    }

    /// The index built again, unless it was less than [`INDEX_REFRESH`] ago.
    fn rebuilt_index(&self, src: &Sources<'_>, now: Instant) -> Option<Arc<Index>> {
        {
            let mut state = self.lock();
            if state.scanned.is_some_and(|at| now.saturating_duration_since(at) < INDEX_REFRESH) {
                return None;
            }
            state.scanned = Some(now);
        }
        Some(self.store(scan(src)))
    }

    fn store(&self, index: Index) -> Arc<Index> {
        let index = Arc::new(index);
        self.lock().index = Some(Arc::clone(&index));
        index
    }
}

impl State {
    fn missed(&self, class: &str, pid: u32, now: Instant) -> bool {
        let at = self.misses.get(&(class.to_owned(), pid));
        at.is_some_and(|&at| now.saturating_duration_since(at) < RETRY)
    }

    fn miss(&mut self, class: &str, pid: u32, now: Instant) {
        if self.misses.len() >= MAX_MISSES {
            self.misses.retain(|_, at| now.saturating_duration_since(*at) < RETRY);
            if self.misses.len() >= MAX_MISSES {
                self.misses.clear();
            }
        }
        self.misses.insert((class.to_owned(), pid), now);
    }

    /// An app found before, now the most recently used. `None` once its
    /// icon is due to be drawn again.
    fn known(&mut self, class: &str, id: &str, now: Instant) -> Option<Arc<AppIdentity>> {
        let at = self.apps.iter().position(|app| app.class == class && app.id == id)?;
        let app = self.apps.remove(at)?;
        if app.icon_failed.is_some_and(|failed| now.saturating_duration_since(failed) >= RETRY) {
            return None;
        }
        let identity = Arc::clone(&app.identity);
        self.apps.push_back(app);
        Some(identity)
    }

    fn remember(&mut self, class: &str, id: &str, identity: Arc<AppIdentity>, icon_failed: Option<Instant>) {
        self.apps.retain(|app| app.class != class || app.id != id);
        self.apps.push_back(Known { class: class.to_owned(), id: id.to_owned(), identity, icon_failed });
        while self.apps.len() > MAX_KNOWN {
            self.apps.pop_front();
        }
    }
}

/// The XDG application folders, in the order an id is looked up in them.
fn application_dirs() -> Vec<PathBuf> {
    application_dirs_in(dirs::data_dir(), std::env::var_os("XDG_DATA_DIRS"), dirs::home_dir())
}

fn application_dirs_in(
    data_home: Option<PathBuf>,
    data_dirs: Option<OsString>,
    home: Option<PathBuf>,
) -> Vec<PathBuf> {
    let data_dirs = data_dirs.filter(|value| !value.is_empty());
    let data_dirs = data_dirs.unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let system: Vec<PathBuf> =
        std::env::split_paths(&data_dirs).filter(|dir| dir.is_absolute()).take(MAX_DATA_DIRS).collect();
    let candidates = data_home
        .into_iter()
        .chain(system)
        .map(|dir| dir.join("applications"))
        .chain([PathBuf::from("/var/lib/flatpak/exports/share/applications")])
        .chain(home.map(|home| home.join(".local/share/flatpak/exports/share/applications")))
        .chain([PathBuf::from("/var/lib/snapd/desktop/applications")]);
    let mut found: Vec<PathBuf> = Vec::new();
    for dir in candidates {
        if !found.contains(&dir) {
            found.push(dir);
        }
    }
    found
}

/// The user's locales for `Name[...]`, from the first of `LANGUAGE`,
/// `LC_ALL`, `LC_MESSAGES` and `LANG` that's set.
fn languages() -> Vec<String> {
    ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.trim().is_empty())
        .map(|value| locale_candidates(&value))
        .unwrap_or_default()
}

/// `ll_CC` then `ll` for each locale in a colon-separated list, without
/// encoding or modifier; nothing for `C` and `POSIX`.
fn locale_candidates(value: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for locale in value.split(':').take(MAX_LANGUAGES) {
        let locale = locale.split(['.', '@']).next().unwrap_or_default().trim();
        if locale.is_empty() || locale.len() > MAX_LOCALE_LEN || locale == "C" || locale == "POSIX" {
            continue;
        }
        let language = locale.split('_').next().unwrap_or(locale);
        for candidate in [locale, language] {
            if !candidate.is_empty() && !found.iter().any(|known| known == candidate) {
                found.push(candidate.to_owned());
            }
        }
    }
    found
}

/// Whether `id` can name a file in a folder without leaving it.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && !id.starts_with('.')
        && !id.contains("..")
        && !id.contains(['/', '\\'])
        && !id.chars().any(char::is_control)
}

/// A file's bytes, when it's a regular file of at most `max` bytes. Opened
/// without blocking, so a FIFO put in its place can't hang the read.
fn read_capped(path: &Path, max: u64) -> Option<Vec<u8>> {
    let file: File =
        OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY).open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= max).then_some(bytes)
}

/// The entry `<id>.desktop` from the first folder with a readable one.
/// `None` when there's none, or it's hidden: deleted, as far as the user is
/// concerned.
fn read_entry(src: &Sources<'_>, id: &str) -> Option<Entry> {
    let file = format!("{id}.desktop");
    let parsed = src.dirs.iter().find_map(|dir| {
        let bytes = read_capped(&dir.join(&file), MAX_ENTRY_BYTES)?;
        parse_entry(&String::from_utf8_lossy(&bytes), src.languages)
    })?;
    match parsed {
        Parsed::App(entry) => Some(entry),
        Parsed::Hidden => None,
    }
}

/// Every entry with a `StartupWMClass`, by that class in lowercase. An id
/// in an earlier folder hides the same id in later ones.
fn scan(src: &Sources<'_>) -> Index {
    let mut index = Index::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut budget = MAX_SCANNED;
    for dir in src.dirs {
        if budget == 0 {
            break;
        }
        let Ok(listing) = fs::read_dir(dir) else { continue };
        let listed: Vec<_> = listing.take(budget).collect();
        budget = budget.saturating_sub(listed.len());
        let mut ids: Vec<String> = listed
            .into_iter()
            .flatten()
            .filter_map(|item| item.file_name().into_string().ok())
            .filter_map(|name| name.strip_suffix(".desktop").map(str::to_owned))
            .filter(|id| valid_id(id) && !seen.contains(id))
            .collect();
        ids.sort_unstable();
        for id in ids {
            let Some(bytes) = read_capped(&dir.join(format!("{id}.desktop")), MAX_ENTRY_BYTES) else {
                continue;
            };
            let Some(parsed) = parse_entry(&String::from_utf8_lossy(&bytes), src.languages) else { continue };
            seen.insert(id.clone());
            if let Parsed::App(entry) = parsed
                && let Some(class) = entry.wm_class.as_deref().map(str::to_lowercase)
            {
                index.entry(class).or_insert(Indexed { id, entry });
            }
        }
    }
    index
}

/// What a desktop entry says about its app.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    name: String,
    icon: Option<String>,
    wm_class: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    App(Entry),
    /// `Hidden=true`.
    Hidden,
}

/// The `[Desktop Entry]` group of a desktop file, its name in the first of
/// `languages` it has. `None` without that group or a name.
fn parse_entry(text: &str, languages: &[String]) -> Option<Parsed> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (mut in_group, mut found_group) = (false, false);
    let (mut name, mut icon, mut wm_class, mut hidden) = (None, None, None, None);
    let mut localized: Option<(usize, &str)> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            if in_group {
                break;
            }
            in_group = line == "[Desktop Entry]";
            found_group |= in_group;
            continue;
        }
        if !in_group || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "Name" => name = name.or(Some(value)),
            "Icon" => icon = icon.or(Some(value)),
            "StartupWMClass" => wm_class = wm_class.or(Some(value)),
            "Hidden" => hidden = hidden.or(Some(value == "true")),
            _ => {
                let locale = key.strip_prefix("Name[").and_then(|rest| rest.strip_suffix(']'));
                if let Some(rank) =
                    locale.and_then(|locale| languages.iter().position(|known| known == locale))
                    && !value.is_empty()
                    && localized.is_none_or(|(best, _)| rank < best)
                {
                    localized = Some((rank, value));
                }
            }
        }
    }
    if !found_group {
        return None;
    }
    if hidden == Some(true) {
        return Some(Parsed::Hidden);
    }
    let name =
        localized.and_then(|(_, value)| display_name(value)).or_else(|| name.and_then(display_name))?;
    let icon = icon.map(unescape_value).filter(|icon| !icon.is_empty());
    let wm_class =
        wm_class.map(|class| unescape_value(class).trim().to_owned()).filter(|class| !class.is_empty());
    Some(Parsed::App(Entry { name, icon, wm_class }))
}

/// A desktop entry string value with its escapes (`\s`, `\n`, `\t`, `\r`,
/// `\\`) undone.
fn unescape_value(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') | None => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    out
}

/// A name for display: one line, at most `MAX_STATE_APP` bytes.
fn display_name(raw: &str) -> Option<String> {
    let text: String = unescape_value(raw).chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let max = remote_limits::MAX_STATE_APP;
    if text.len() <= max {
        return Some(text.to_owned());
    }
    let cut = (0..=max).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
    Some(text[..cut].trim_end().to_owned())
}

/// The app id in the systemd unit `pid` runs in, from its cgroup.
fn process_app_id(proc: &Path, pid: u32) -> Option<String> {
    let bytes = read_capped(&proc.join(pid.to_string()).join("cgroup"), MAX_CGROUP_BYTES)?;
    String::from_utf8_lossy(&bytes).lines().find_map(cgroup_app_id)
}

/// The app id in a line of `/proc/<pid>/cgroup` (cgroup v2, or v1's systemd
/// hierarchy): from the innermost app unit on its path.
fn cgroup_app_id(line: &str) -> Option<String> {
    let mut fields = line.splitn(3, ':');
    let (_hierarchy, controllers, path) = (fields.next()?, fields.next()?, fields.next()?);
    if !controllers.is_empty() && controllers != "name=systemd" {
        return None;
    }
    path.rsplit('/').find_map(unit_app_id)
}

/// The app id in the name of a unit an app runs in:
/// `app[-<launcher>]-<id>-<random>.scope`, `app[-<launcher>]-<id>[@<random>].service`,
/// or G7 Linux's `app-g7linux-<id>-<random>.service`. Ids escape "-" as `\x2d`.
fn unit_app_id(unit: &str) -> Option<String> {
    let body = unit.strip_prefix("app-")?;
    let name = if let Some(scope) = body.strip_suffix(".scope") {
        let (name, random) = scope.rsplit_once('-')?;
        if random.is_empty() {
            return None;
        }
        name
    } else {
        let service = body.strip_suffix(".service")?;
        match service.split_once('@') {
            Some((name, _)) => name,
            None => match service.rsplit_once('-') {
                Some((name, random)) if is_random_suffix(random) => name,
                _ => service,
            },
        }
    };
    let escaped = match name.split_once('-') {
        None => name,
        Some((_launcher, id)) if !id.contains('-') => id,
        Some(_) => return None,
    };
    let id = unescape_unit(escaped)?;
    let id = id.strip_suffix(".desktop").unwrap_or(&id);
    valid_id(id).then(|| id.to_owned())
}

/// A pid, or a long run of lowercase hex: the random part launchers end a
/// unit name with.
fn is_random_suffix(text: &str) -> bool {
    let digits = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    let hex = text.len() >= MIN_RANDOM_HEX
        && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    digits || hex
}

/// A unit name part with systemd's `\xHH` escapes undone; `None` when one is
/// malformed or the result isn't UTF-8.
fn unescape_unit(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        if byte != b'\\' {
            out.push(byte);
            i += 1;
            continue;
        }
        let Some(&[b'x', high, low]) = bytes.get(i + 1..i + 4) else { return None };
        let digit = |b: u8| char::from(b).to_digit(16);
        let value = digit(high)? * 16 + digit(low)?;
        out.push(u8::try_from(value).ok()?);
        i += 4;
    }
    String::from_utf8(out).ok()
}

#[derive(Debug, PartialEq, Eq)]
enum Rendered {
    Icon(AppIcon),
    /// No usable icon: none that can be drawn, or none small enough.
    Missing,
    /// Nothing was drawn, which may change: say, the GUI wasn't running yet.
    Failed,
}

/// An entry's `Icon` drawn as a PNG the phone takes.
fn make_icon(render: fn(&str, u32) -> Vec<u8>, value: &str) -> Rendered {
    let Some(source) = icon_source(value) else { return Rendered::Missing };
    for edge in ICON_EDGES {
        let png = render(&source, edge);
        if png.is_empty() {
            return Rendered::Failed;
        }
        if png.len() <= remote_limits::MAX_ICON_BYTES {
            if !png.starts_with(PNG_SIGNATURE) {
                return Rendered::Missing;
            }
            return Rendered::Icon(AppIcon { key: icon_key(&png), png: Arc::new(png) });
        }
    }
    Rendered::Missing
}

/// What to draw for an `Icon` value: an absolute path to a regular file, or
/// a theme name (without the extension some entries give it).
fn icon_source(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_ICON_VALUE || value.chars().any(char::is_control) {
        return None;
    }
    let path = Path::new(value);
    if path.is_absolute() {
        let metadata = fs::metadata(path).ok()?;
        return (metadata.is_file() && metadata.len() <= MAX_ICON_FILE_BYTES).then(|| value.to_owned());
    }
    if value.starts_with('.') || value.contains(['/', '\\']) {
        return None;
    }
    let name = [".png", ".svg", ".xpm"].into_iter().find_map(|ext| value.strip_suffix(ext)).unwrap_or(value);
    (!name.is_empty()).then(|| name.to_owned())
}

/// The first 8 bytes of the PNG's BLAKE3 hash, in lowercase hex.
fn icon_key(png: &[u8]) -> String {
    let hash = blake3::hash(png);
    let mut key = String::with_capacity(remote_limits::ICON_KEY_LEN);
    for byte in &hash.as_bytes()[..remote_limits::ICON_KEY_LEN / 2] {
        let _ = write!(key, "{byte:02x}");
    }
    key
}

#[cfg(test)]
mod tests {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    use super::*;

    struct Fixture {
        root: tempfile::TempDir,
        dirs: Vec<PathBuf>,
        proc: PathBuf,
        cache: Cache,
        render: fn(&str, u32) -> Vec<u8>,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let dirs = vec![root.path().join("user"), root.path().join("system")];
            let proc = root.path().join("proc");
            for dir in dirs.iter().chain([&proc]) {
                fs::create_dir_all(dir).unwrap();
            }
            Self { root, dirs, proc, cache: Cache::default(), render: stub_png }
        }

        fn write(&self, folder: usize, id: &str, text: &str) {
            fs::write(self.dirs[folder].join(format!("{id}.desktop")), text).unwrap();
        }

        fn cgroup(&self, pid: u32, unit: &str) {
            let dir = self.proc.join(pid.to_string());
            fs::create_dir_all(&dir).unwrap();
            let line = format!("0::/user.slice/user-1000.slice/user@1000.service/app.slice/{unit}\n");
            fs::write(dir.join("cgroup"), line).unwrap();
        }

        fn identify_at(&self, class: &str, pid: u32, now: Instant) -> Option<Arc<AppIdentity>> {
            let languages = ["de_DE".to_owned(), "de".to_owned()];
            let src =
                Sources { dirs: &self.dirs, proc: &self.proc, languages: &languages, render: self.render };
            identify_in(&self.cache, &src, class, pid, now)
        }

        fn identify(&self, class: &str, pid: u32) -> Option<Arc<AppIdentity>> {
            self.identify_at(class, pid, Instant::now())
        }

        fn id(&self, class: &str, pid: u32) -> Option<String> {
            self.identify(class, pid).map(|app| app.id.clone())
        }

        fn name(&self, class: &str, pid: u32) -> Option<String> {
            self.identify(class, pid).map(|app| app.name.clone())
        }
    }

    fn app(name: &str, extra: &str) -> String {
        format!("[Desktop Entry]\nType=Application\nName={name}\nIcon=app\n{extra}")
    }

    fn stub_png(icon: &str, edge: u32) -> Vec<u8> {
        let mut png = PNG_SIGNATURE.to_vec();
        png.extend_from_slice(format!("{icon}@{edge}").as_bytes());
        png
    }

    fn no_png(_: &str, _: u32) -> Vec<u8> {
        Vec::new()
    }

    fn entry(text: &str, languages: &[&str]) -> Option<Parsed> {
        let languages: Vec<String> = languages.iter().map(|&l| l.to_owned()).collect();
        parse_entry(text, &languages)
    }

    fn name_of(text: &str, languages: &[&str]) -> String {
        match entry(text, languages) {
            Some(Parsed::App(entry)) => entry.name,
            other => panic!("not an app: {other:?}"),
        }
    }

    #[test]
    fn finds_an_entry_by_its_id() {
        let fx = Fixture::new();
        fx.write(1, "com.anthropic.Claude", &app("Claude", ""));
        fx.write(1, "chrome-kelvin", &app("Kelvin - Chrome", ""));
        fx.write(1, "gimp", &app("GIMP", ""));
        fx.write(0, "org.example.Mine", &app("Mine", ""));
        fx.write(1, "org.example.Mine", &app("Theirs", ""));

        let claude = fx.identify("com.anthropic.Claude", 0).unwrap();
        assert_eq!((claude.id.as_str(), claude.name.as_str()), ("com.anthropic.Claude", "Claude"));
        assert_eq!(fx.name("chrome-kelvin", 0).as_deref(), Some("Kelvin - Chrome"));
        assert_eq!(fx.id("Gimp", 0).as_deref(), Some("gimp"), "the class in lowercase");
        assert_eq!(fx.name("org.example.Mine", 0).as_deref(), Some("Mine"), "the first folder wins");
        assert_eq!(fx.identify("org.example.Missing", 0), None);
        assert_eq!(fx.identify("", 0), None);
    }

    #[test]
    fn finds_an_entry_by_startup_wm_class_ignoring_case() {
        let fx = Fixture::new();
        fx.write(1, "chrome-profile-1", &app("Kelvin", "StartupWMClass=Chrome-Kelvin\n"));
        fx.write(1, "org.example.Plain", &app("Plain", ""));
        assert_eq!(fx.id("chrome-KELVIN", 0).as_deref(), Some("chrome-profile-1"));
        assert_eq!(fx.id("CHROME-KELVIN", 0).as_deref(), Some("chrome-profile-1"));
        assert_eq!(fx.identify("Plain", 0), None);
    }

    #[test]
    fn the_wm_class_index_is_rebuilt_at_most_once_a_minute() {
        let fx = Fixture::new();
        let start = Instant::now();
        assert_eq!(fx.identify_at("Later", 0, start), None);
        fx.write(0, "later-app", &app("Later", "StartupWMClass=Later\n"));
        fx.write(0, "other-app", &app("Other", "StartupWMClass=Other\n"));
        assert_eq!(fx.identify_at("Other", 0, start + Duration::from_secs(10)), None, "not rebuilt yet");
        let later = fx.identify_at("Later", 0, start + Duration::from_secs(61));
        assert_eq!(later.map(|app| app.id.clone()).as_deref(), Some("later-app"));
        let other = fx.identify_at("Other", 0, start + Duration::from_secs(71));
        assert_eq!(other.map(|app| app.id.clone()).as_deref(), Some("other-app"));
    }

    #[test]
    fn finds_an_entry_by_the_unit_the_process_runs_in() {
        let fx = Fixture::new();
        fx.write(1, "chrome-kelvin", &app("Kelvin", ""));
        fx.write(1, "org.mozilla.firefox", &app("Firefox", ""));
        fx.cgroup(4242, r"app-gnome-chrome\x2dkelvin-4242.scope");
        fx.cgroup(5000, "app-flatpak-org.mozilla.firefox-12.scope");
        assert_eq!(fx.id("", 4242).as_deref(), Some("chrome-kelvin"));
        assert_eq!(fx.id("electron", 4242).as_deref(), Some("chrome-kelvin"));
        assert_eq!(fx.id("electron", 5000).as_deref(), Some("org.mozilla.firefox"), "per process");
        assert_eq!(fx.identify("electron", 6000), None, "no such process");
    }

    #[test]
    fn reads_the_app_id_from_a_cgroup_line() {
        let id = |unit: &str| {
            cgroup_app_id(&format!("0::/user.slice/user-1000.slice/user@1000.service/app.slice/{unit}"))
        };
        assert_eq!(id(r"app-gnome-chrome\x2dkelvin-60921.scope").as_deref(), Some("chrome-kelvin"));
        assert_eq!(id("app-gnome-com.anthropic.Claude-7096.scope").as_deref(), Some("com.anthropic.Claude"));
        assert_eq!(id("app-flatpak-org.mozilla.firefox-1234.scope").as_deref(), Some("org.mozilla.firefox"));
        assert_eq!(id("app-dex-org.kde.konsole-8a1f2c.scope").as_deref(), Some("org.kde.konsole"));
        assert_eq!(id("app-com.anthropic.Claude-7096.scope").as_deref(), Some("com.anthropic.Claude"));
        assert_eq!(
            id(r"app-org.gnome.Evolution\x2dalarm\x2dnotify@autostart.service").as_deref(),
            Some("org.gnome.Evolution-alarm-notify")
        );
        assert_eq!(id("app-kde-org.kde.dolphin@a1b2.service").as_deref(), Some("org.kde.dolphin"));
        assert_eq!(id("app-org.gnome.Calculator.service").as_deref(), Some("org.gnome.Calculator"));
        assert_eq!(
            id("app-g7linux-g7link-f980c7374cbe46bc9a3ddaada2478590.service").as_deref(),
            Some("g7link"),
            "G7 Linux's own launches"
        );
        assert_eq!(id(r"app-gnome-org.gnome.Ptyxis-77.scope/sub.scope").as_deref(), Some("org.gnome.Ptyxis"));
        assert_eq!(
            cgroup_app_id("1:name=systemd:/user.slice/user-1000.slice/user@1000.service/app.slice/app-gnome-gimp-77.scope")
                .as_deref(),
            Some("gimp")
        );

        for unit in [
            "",
            "app.slice",
            "session-2.scope",
            r"app-gnome\x2dsession\x2dmanager.slice",
            "app-gnome-a-b-c-1.scope",
            "app-gnome-foo-.scope",
            r"app-gnome-bad\x2-1.scope",
            r"app-gnome-bad\q2d-1.scope",
            r"app-gnome-..\x2f..\x2fetc\x2fpasswd-1.scope",
            r"app-gnome-a\x2fb-1.scope",
            "app-gnome-..-1.scope",
            "app-gnome-.hidden-1.scope",
            r"app-gnome-a\x00b-1.scope",
        ] {
            assert_eq!(id(unit), None, "{unit}");
        }
        assert_eq!(
            cgroup_app_id("4:cpu:/user.slice/app-gnome-gimp-77.scope"),
            None,
            "not systemd's hierarchy"
        );
        assert_eq!(cgroup_app_id("garbage"), None);
    }

    #[test]
    fn picks_the_name_in_the_users_language() {
        let text = "# A comment\n[Desktop Entry]\nName=Files\nName[de]=Dateien\nName[de_DE]=Dateien (DE)\n\
                    Name[fr]=Fichiers\nName[es]=\n\n[Desktop Action new]\nName[de_DE]=Neues Fenster\n";
        assert_eq!(name_of(text, &["de_DE", "de"]), "Dateien (DE)");
        assert_eq!(name_of(text, &["de_AT", "de"]), "Dateien");
        assert_eq!(name_of(text, &["fr_FR", "fr"]), "Fichiers");
        assert_eq!(name_of(text, &["es_ES", "es"]), "Files", "empty translations don't count");
        assert_eq!(name_of(text, &[]), "Files");
        assert_eq!(name_of("[Desktop Entry]\nName= Tab\\tName\\s2 \n", &[]), "Tab Name 2");

        let long = name_of(&format!("[Desktop Entry]\nName={}\n", "é".repeat(40)), &[]);
        assert_eq!(long, "é".repeat(32));
        assert!(long.len() <= remote_limits::MAX_STATE_APP);

        assert_eq!(entry("Name=Files\n", &[]), None, "no group");
        assert_eq!(entry("[Desktop Entry]\nIcon=x\n", &[]), None, "no name");

        assert_eq!(locale_candidates("de_DE.UTF-8"), ["de_DE", "de"]);
        assert_eq!(locale_candidates("pt_BR:pt:en_US"), ["pt_BR", "pt", "en_US", "en"]);
        assert_eq!(locale_candidates("sr_RS@latin"), ["sr_RS", "sr"]);
        assert!(locale_candidates("C.UTF-8").is_empty());
        assert!(locale_candidates("POSIX").is_empty());
    }

    #[test]
    fn hidden_entries_count_as_removed() {
        let fx = Fixture::new();
        fx.write(0, "org.example.Gone", "[Desktop Entry]\nHidden=true\n");
        fx.write(1, "org.example.Gone", &app("Gone", "StartupWMClass=GoneClass\n"));
        fx.write(1, "org.example.Kept", &app("Kept", "StartupWMClass=KeptClass\nHidden=false\n"));
        assert_eq!(fx.identify("org.example.Gone", 0), None);
        assert_eq!(fx.identify("GoneClass", 0), None);
        assert_eq!(fx.name("keptclass", 0).as_deref(), Some("Kept"));
        assert_eq!(entry("[Desktop Entry]\nName=A\nHidden=true\n", &[]), Some(Parsed::Hidden));
    }

    #[test]
    fn ids_never_leave_the_application_folders() {
        let fx = Fixture::new();
        fs::write(fx.root.path().join("outside.desktop"), app("Outside", "")).unwrap();
        for class in ["../outside", "..", "user/../../outside", ".hidden", "a\0b", r"..\outside"] {
            assert_eq!(fx.identify(class, 0), None, "{class:?}");
        }
        fx.cgroup(9, r"app-gnome-..\x2foutside-9.scope");
        assert_eq!(fx.identify("", 9), None);
        assert!(!valid_id("../outside"));
        assert!(!valid_id(&"a".repeat(MAX_ID_LEN + 1)));
        assert!(valid_id("org.gnome.Nautilus"));
    }

    #[test]
    fn oversized_entries_are_passed_over() {
        let fx = Fixture::new();
        let padding = "x".repeat(usize::try_from(MAX_ENTRY_BYTES).unwrap());
        let big = format!("{}# {padding}\n", app("Big", "StartupWMClass=BigClass\n"));
        fx.write(0, "org.example.Big", &big);
        fx.write(1, "org.example.Big", &app("Small", ""));
        assert_eq!(fx.name("org.example.Big", 0).as_deref(), Some("Small"));
        assert_eq!(fx.identify("BigClass", 0), None);

        let path = fx.root.path().join("exact");
        fs::write(&path, vec![b'a'; 100]).unwrap();
        assert_eq!(read_capped(&path, 100).map(|bytes| bytes.len()), Some(100));
        assert_eq!(read_capped(&path, 99), None);
        assert_eq!(read_capped(fx.root.path(), 100), None, "a folder");
    }

    #[test]
    fn a_fifo_in_place_of_an_entry_does_not_hang() {
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let fx = Fixture::new();
            let fifo = fx.dirs[0].join("org.example.Fifo.desktop");
            let path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
            // SAFETY: `path` is a NUL-terminated path.
            assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
            fx.write(1, "org.example.Other", &app("Other", "StartupWMClass=OtherClass\n"));
            let _ = done.send((fx.identify("org.example.Fifo", 0), fx.name("OtherClass", 0)));
        });
        let (fifo, other) = finished.recv_timeout(Duration::from_secs(10)).expect("lookups finish");
        assert_eq!(fifo, None);
        assert_eq!(other.as_deref(), Some("Other"));
    }

    #[test]
    fn icon_keys_are_16_lowercase_hex_digits_of_blake3() {
        assert_eq!(icon_key(b""), "af1349b9f5f9a1a6");
        let key = icon_key(&stub_png("app", 96));
        assert_eq!(key.len(), remote_limits::ICON_KEY_LEN);
        assert!(key.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "{key}");

        let fx = Fixture::new();
        fx.write(0, "org.example.App", &app("App", ""));
        let icon = fx.identify("org.example.App", 0).unwrap().icon.clone().unwrap();
        assert_eq!(*icon.png, stub_png("app", 96));
        assert_eq!(icon.key, icon_key(&icon.png));
    }

    #[test]
    fn icons_fit_the_phones_limit() {
        fn big_at_96(icon: &str, edge: u32) -> Vec<u8> {
            let mut png = stub_png(icon, edge);
            if edge == 96 {
                png.resize(remote_limits::MAX_ICON_BYTES + 1, 0);
            }
            png
        }
        fn always_big(_: &str, _: u32) -> Vec<u8> {
            let mut png = PNG_SIGNATURE.to_vec();
            png.resize(remote_limits::MAX_ICON_BYTES + 1, 0);
            png
        }
        fn not_png(_: &str, _: u32) -> Vec<u8> {
            b"GIF89a".to_vec()
        }
        let png = |rendered: Rendered| match rendered {
            Rendered::Icon(icon) => Some(icon.png.to_vec()),
            _ => None,
        };
        assert_eq!(png(make_icon(big_at_96, "app")), Some(stub_png("app", 64)));
        assert_eq!(make_icon(always_big, "app"), Rendered::Missing);
        assert_eq!(make_icon(not_png, "app"), Rendered::Missing);
        assert_eq!(make_icon(no_png, "app"), Rendered::Failed);
        assert_eq!(png(make_icon(stub_png, "org.gnome.Foo.svg")), Some(stub_png("org.gnome.Foo", 96)));
        for hostile in ["../icon", "icons/app", "", ".hidden", "/nonexistent/g7link/icon.png", "a\nb"] {
            assert_eq!(make_icon(stub_png, hostile), Rendered::Missing, "{hostile:?}");
        }
    }

    #[test]
    fn a_media_players_desktop_entry_gives_its_apps_icon() {
        let fx = Fixture::new();
        fx.write(0, "org.example.Player", &app("Player", ""));
        fx.write(1, "com.example.Flat", &app("Flat", "StartupWMClass=FlatPlayer\n"));
        fx.write(0, "org.example.Plain", "[Desktop Entry]\nType=Application\nName=Plain\n");
        let languages: Vec<String> = Vec::new();
        let src = Sources { dirs: &fx.dirs, proc: &fx.proc, languages: &languages, render: stub_png };
        let icon = |entry: &str| icon_for_entry_in(&fx.cache, &src, entry, Instant::now());

        let (key, png) = icon("org.example.Player").unwrap();
        assert_eq!(png, stub_png("app", 96));
        assert_eq!(key, icon_key(&png));
        assert_eq!(key.len(), remote_limits::ICON_KEY_LEN);
        assert!(key.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "{key}");
        assert_eq!(icon(" org.example.Player.desktop ").map(|(k, _)| k), Some(key), "with its extension");
        assert!(icon("flatplayer").is_some(), "by the entry's StartupWMClass");
        assert_eq!(icon("org.example.Plain"), None, "an entry without an icon");
        for hostile in ["", "../org.example.Player", "org.example.Missing", "a\nb"] {
            assert_eq!(icon(hostile), None, "{hostile:?}");
        }
    }

    #[test]
    fn an_icon_that_could_not_be_drawn_is_tried_again() {
        let mut fx = Fixture::new();
        fx.write(0, "org.example.App", &app("App", ""));
        fx.render = no_png;
        let start = Instant::now();
        assert_eq!(fx.identify_at("org.example.App", 0, start).unwrap().icon, None);
        fx.render = stub_png;
        assert_eq!(fx.identify_at("org.example.App", 0, start + Duration::from_secs(10)).unwrap().icon, None);
        let later = fx.identify_at("org.example.App", 0, start + Duration::from_secs(61)).unwrap();
        assert!(later.icon.is_some());
    }

    #[test]
    fn misses_are_remembered_for_a_minute() {
        let fx = Fixture::new();
        let start = Instant::now();
        assert_eq!(fx.identify_at("org.example.App", 0, start), None);
        fx.write(0, "org.example.App", &app("App", ""));
        assert_eq!(fx.identify_at("org.example.App", 0, start + Duration::from_secs(30)), None);
        assert!(
            fx.identify_at("org.example.App", 7, start + Duration::from_secs(30)).is_some(),
            "another process"
        );
        assert!(fx.identify_at("org.example.App", 0, start + Duration::from_secs(61)).is_some());
    }

    #[test]
    fn identities_are_cached_and_bounded() {
        let fx = Fixture::new();
        fx.write(0, "org.example.App", &app("App", ""));
        let first = fx.identify("org.example.App", 0).unwrap();
        fs::remove_file(fx.dirs[0].join("org.example.App.desktop")).unwrap();
        let again = fx.identify("org.example.App", 0).unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        for n in 0..MAX_KNOWN {
            fx.write(0, &format!("org.example.App{n}"), &app("App", ""));
            assert!(fx.identify(&format!("org.example.App{n}"), 0).is_some());
        }
        assert_eq!(fx.cache.lock().apps.len(), MAX_KNOWN);
        assert_eq!(fx.identify("org.example.App", 0), None, "forgotten, and its file is gone");
    }

    #[test]
    fn lists_the_application_folders_in_order() {
        let dirs = application_dirs_in(
            Some("/home/u/.local/share".into()),
            Some("/usr/share:relative:/var/lib/flatpak/exports/share/".into()),
            Some("/home/u".into()),
        );
        let want: Vec<PathBuf> = [
            "/home/u/.local/share/applications",
            "/usr/share/applications",
            "/var/lib/flatpak/exports/share/applications",
            "/home/u/.local/share/flatpak/exports/share/applications",
            "/var/lib/snapd/desktop/applications",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(dirs, want);
        let defaults = application_dirs_in(None, Some(OsString::new()), None);
        assert_eq!(
            defaults[..2],
            [PathBuf::from("/usr/local/share/applications"), "/usr/share/applications".into()]
        );
    }
}
