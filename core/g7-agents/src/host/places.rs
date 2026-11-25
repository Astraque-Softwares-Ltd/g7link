// SPDX-License-Identifier: MPL-2.0
//! Places (new thread §2.4): where a phone may look for a folder and add it
//! as an agent folder. A place is a root's name followed by `/`-separated
//! plain names, so a phone never sees or sends an absolute path. The roots
//! are fixed: Home, then each disk mounted under `/run/media/$USER` or
//! `/media/$USER`. Hidden folders and links are never listed or added.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::api::{
    ErrorCode, MAX_FOLDER_PLACE, MAX_FOLDER_PLACE_ENTRIES, MAX_FOLDER_PLACE_NAME, MAX_FOLDER_PLACE_READ,
    MAX_FOLDER_PLACE_SEGMENTS, Place, PlacesListResult, RpcError, is_stripped_char,
};

/// The root for the user's home folder.
pub const HOME: &str = "Home";
/// Roots listed at most.
pub const MAX_ROOTS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceError {
    /// Not a well-formed place.
    Bad,
    /// Its root or one of its folders isn't there (or is a link).
    NotFound,
    /// A root itself, or inside or holding one of G7's own directories.
    Refused,
    Unreadable,
}

impl PlaceError {
    pub fn into_rpc(self) -> RpcError {
        match self {
            Self::Bad => RpcError::new(ErrorCode::BadRequest, "That place isn't valid."),
            Self::NotFound => RpcError::new(ErrorCode::NotFound, "That folder isn't there any more."),
            Self::Refused => {
                RpcError::new(ErrorCode::Denied, "Agents can't work in that folder from a phone.")
            }
            Self::Unreadable => RpcError::new(ErrorCode::BadRequest, "That folder can't be read."),
        }
    }
}

/// A name a place may hold: not hidden, valid text without control or
/// direction characters, no separator, not too long.
pub fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_FOLDER_PLACE_NAME
        && !name.starts_with('.')
        && !name.contains(['/', '\\'])
        && !name.chars().any(|c| c.is_control() || is_stripped_char(c))
}

/// Home first, then each mounted disk by its directory name, sorted; a name
/// already taken gets " (2)", " (3)"… Blocking: run on the blocking pool.
pub fn roots(home: Option<&Path>, media: &[PathBuf]) -> Vec<Root> {
    let mut roots: Vec<Root> = Vec::new();
    if let Some(home) = home.filter(|h| h.is_absolute() && h.is_dir()) {
        roots.push(Root { name: HOME.to_owned(), path: home.to_path_buf() });
    }
    for dir in media {
        let Ok(read) = std::fs::read_dir(dir) else { continue };
        let mut found: Vec<(String, PathBuf)> = read
            .flatten()
            .take(MAX_ROOTS * 4)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                plain_name(&name).then(|| (name, e.path()))
            })
            .collect();
        found.sort_by_cached_key(|(name, _)| (name.to_lowercase(), name.clone()));
        for (name, path) in found {
            let name = unique_name(&roots, name);
            roots.push(Root { name, path });
        }
    }
    roots.truncate(MAX_ROOTS);
    roots
}

fn unique_name(roots: &[Root], name: String) -> String {
    if !roots.iter().any(|r| r.name == name) {
        return name;
    }
    (2..)
        .map(|n| format!("{name} ({n})"))
        .find(|candidate| !roots.iter().any(|r| r.name == *candidate))
        .unwrap_or(name)
}

/// A place split into its root's name and the names below it.
fn parse(place: &str) -> Option<(&str, Vec<&str>)> {
    if place.is_empty() || place.len() > MAX_FOLDER_PLACE || place.ends_with('/') {
        return None;
    }
    let mut parts = place.split('/');
    let root = parts.next().filter(|r| !r.is_empty())?;
    let names: Vec<&str> = parts.collect();
    (names.len() <= MAX_FOLDER_PLACE_SEGMENTS && names.iter().all(|n| plain_name(n))).then_some((root, names))
}

/// The directory a place names: every step a folder and not a link, and
/// inside its root once resolved. Its canonical path, and how many names
/// lie below the root.
fn resolve(roots: &[Root], place: &str) -> Result<(PathBuf, usize), PlaceError> {
    let (root_name, names) = parse(place).ok_or(PlaceError::Bad)?;
    let root = roots.iter().find(|r| r.name == root_name).ok_or(PlaceError::NotFound)?;
    let base = std::fs::canonicalize(&root.path).map_err(|_| PlaceError::NotFound)?;
    let mut dir = base.clone();
    for name in &names {
        dir.push(name);
        let meta = std::fs::symlink_metadata(&dir).map_err(|_| PlaceError::NotFound)?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(PlaceError::NotFound);
        }
    }
    let real = std::fs::canonicalize(&dir).map_err(|_| PlaceError::NotFound)?;
    if !real.starts_with(&base) {
        return Err(PlaceError::Refused);
    }
    Ok((real, names.len()))
}

/// The roots for an empty place, else the folders in it, sorted without
/// regard to case, [`MAX_FOLDER_PLACE_ENTRIES`] from `offset`. `folders` are the
/// agent folders' canonical paths and ids. Blocking.
pub fn list(
    roots: &[Root],
    refused: &[PathBuf],
    folders: &[(String, String)],
    place: &str,
    offset: u32,
) -> Result<PlacesListResult, PlaceError> {
    if place.is_empty() {
        let entries =
            roots.iter().map(|r| Place { name: r.name.clone(), repo: false, folder_id: None }).collect();
        return Ok(PlacesListResult { place: String::new(), entries, more: false });
    }
    let (dir, _) = resolve(roots, place)?;
    if refused.iter().any(|r| dir.starts_with(r)) {
        return Err(PlaceError::Refused);
    }
    let read = std::fs::read_dir(&dir).map_err(|_| PlaceError::Unreadable)?;
    let mut names: Vec<String> = Vec::new();
    let mut cut = false;
    for (n, entry) in read.enumerate() {
        if n >= MAX_FOLDER_PLACE_READ {
            cut = true;
            break;
        }
        let Ok(entry) = entry else { continue };
        // Links and files are left out: a link could lead out of the root.
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(name) = entry.file_name().into_string() else { continue };
        if plain_name(&name) && !refused.iter().any(|r| *r == dir.join(&name)) {
            names.push(name);
        }
    }
    names.sort_by_cached_key(|n| (n.to_lowercase(), n.clone()));
    let start = (offset as usize).min(MAX_FOLDER_PLACE_READ).min(names.len());
    let end = (start + MAX_FOLDER_PLACE_ENTRIES).min(names.len());
    let by_path: HashMap<&str, &str> =
        folders.iter().map(|(path, id)| (path.as_str(), id.as_str())).collect();
    let entries = names[start..end]
        .iter()
        .map(|name| {
            let path = dir.join(name);
            Place {
                name: name.clone(),
                repo: std::fs::symlink_metadata(path.join(".git")).is_ok(),
                folder_id: path.to_str().and_then(|p| by_path.get(p)).map(|id| (*id).to_owned()),
            }
        })
        .collect();
    Ok(PlacesListResult { place: place.to_owned(), entries, more: cut || end < names.len() })
}

/// The folder a phone may add for `place`: below a root (never a root
/// itself), readable, and neither inside nor holding a refused directory.
/// Its canonical path. Blocking.
pub fn addable(roots: &[Root], refused: &[PathBuf], place: &str) -> Result<PathBuf, PlaceError> {
    let (dir, depth) = resolve(roots, place)?;
    if depth == 0 || refused.iter().any(|r| dir.starts_with(r) || r.starts_with(&dir)) {
        return Err(PlaceError::Refused);
    }
    std::fs::read_dir(&dir).map_err(|_| PlaceError::Unreadable)?;
    Ok(dir)
}

/// `paths` resolved where they exist, so a link to G7's directories is
/// refused like the directory itself.
pub fn canonical_all(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.into_iter().map(|p| std::fs::canonicalize(&p).unwrap_or(p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree {
        _dir: tempfile::TempDir,
        home: PathBuf,
        media: PathBuf,
        outside: PathBuf,
    }

    fn tree() -> Tree {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let media = dir.path().join("media");
        let outside = dir.path().join("outside");
        for d in [
            home.join("code/orbit"),
            home.join("code/plain"),
            home.join(".ssh"),
            home.join("Code-b"),
            media.join("Local Disk/code"),
            media.join("Home"),
            outside.join("secret"),
        ] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::create_dir(home.join("code/orbit/.git")).unwrap();
        std::fs::write(home.join("code/file.txt"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, home.join("code/escape")).unwrap();
        Tree { _dir: dir, home, media, outside }
    }

    fn names(result: &PlacesListResult) -> Vec<&str> {
        result.entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn roots_are_home_then_disks_with_unique_names() {
        let t = tree();
        let roots = roots(Some(&t.home), std::slice::from_ref(&t.media));
        let named: Vec<&str> = roots.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(named, ["Home", "Home (2)", "Local Disk"]);
        assert_eq!(roots[2].path, t.media.join("Local Disk"));
        assert!(super::roots(Some(Path::new("relative")), &[]).is_empty());
        let listed = list(&roots, &[], &[], "", 0).unwrap();
        assert_eq!(names(&listed), ["Home", "Home (2)", "Local Disk"]);
    }

    #[test]
    fn lists_folders_only_without_hidden_ones_or_links() {
        let t = tree();
        let roots = roots(Some(&t.home), &[]);
        let home = list(&roots, &[], &[], "Home", 0).unwrap();
        assert_eq!(names(&home), ["code", "Code-b"], "no .ssh");
        let code_path = std::fs::canonicalize(t.home.join("code/orbit")).unwrap();
        let folders = vec![(code_path.to_string_lossy().into_owned(), "f0123456789ab".to_owned())];
        let code = list(&roots, &[], &folders, "Home/code", 0).unwrap();
        assert_eq!(names(&code), ["orbit", "plain"], "no file, no link");
        assert!(code.entries[0].repo && !code.entries[1].repo);
        assert_eq!(code.entries[0].folder_id.as_deref(), Some("f0123456789ab"));
        assert_eq!(code.entries[1].folder_id, None);
    }

    #[test]
    fn bad_places_are_refused_before_anything_is_read() {
        let t = tree();
        let roots = roots(Some(&t.home), &[]);
        for bad in [
            "/Home",
            "Home/",
            "Home//code",
            "Home/../outside",
            "Home/./code",
            "Home/.ssh",
            "Home/co\\de",
            "Home/co\u{202e}de",
            "Home/co\u{7}de",
        ] {
            assert_eq!(list(&roots, &[], &[], bad, 0).unwrap_err(), PlaceError::Bad, "{bad}");
            assert_eq!(addable(&roots, &[], bad).unwrap_err(), PlaceError::Bad, "{bad}");
        }
        let deep = format!("Home/{}", vec!["a"; MAX_FOLDER_PLACE_SEGMENTS + 1].join("/"));
        assert_eq!(list(&roots, &[], &[], &deep, 0).unwrap_err(), PlaceError::Bad);
        assert_eq!(list(&roots, &[], &[], "Nowhere/code", 0).unwrap_err(), PlaceError::NotFound);
        assert_eq!(list(&roots, &[], &[], "Home/missing", 0).unwrap_err(), PlaceError::NotFound);
    }

    #[test]
    fn a_link_out_of_the_root_is_neither_listed_nor_added() {
        let t = tree();
        let roots = roots(Some(&t.home), &[]);
        assert_eq!(list(&roots, &[], &[], "Home/code/escape", 0).unwrap_err(), PlaceError::NotFound);
        assert_eq!(addable(&roots, &[], "Home/code/escape").unwrap_err(), PlaceError::NotFound);
        assert!(t.outside.join("secret").is_dir());
    }

    #[test]
    fn adds_below_a_root_only_and_never_near_g7s_own_folders() {
        let t = tree();
        let roots = roots(Some(&t.home), std::slice::from_ref(&t.media));
        let orbit = addable(&roots, &[], "Home/code/orbit").unwrap();
        assert_eq!(orbit, std::fs::canonicalize(t.home.join("code/orbit")).unwrap());
        assert!(addable(&roots, &[], "Local Disk/code").is_ok());
        for root in ["Home", "Local Disk", "Home (2)"] {
            assert_eq!(addable(&roots, &[], root).unwrap_err(), PlaceError::Refused, "{root}");
        }
        let data = canonical_all(vec![t.home.join("code/plain")]);
        assert_eq!(addable(&roots, &data, "Home/code/plain").unwrap_err(), PlaceError::Refused, "inside");
        assert_eq!(addable(&roots, &data, "Home/code").unwrap_err(), PlaceError::Refused, "holding");
        assert_eq!(list(&roots, &data, &[], "Home/code/plain", 0).unwrap_err(), PlaceError::Refused);
        assert_eq!(names(&list(&roots, &data, &[], "Home/code", 0).unwrap()), ["orbit"]);
    }

    #[test]
    fn pages_and_clamps_the_offset() {
        let t = tree();
        let many = t.home.join("many");
        for n in 0..(MAX_FOLDER_PLACE_ENTRIES + 5) {
            std::fs::create_dir_all(many.join(format!("d{n:04}"))).unwrap();
        }
        let roots = roots(Some(&t.home), &[]);
        let first = list(&roots, &[], &[], "Home/many", 0).unwrap();
        assert_eq!(first.entries.len(), MAX_FOLDER_PLACE_ENTRIES);
        assert!(first.more);
        let rest = list(&roots, &[], &[], "Home/many", MAX_FOLDER_PLACE_ENTRIES as u32).unwrap();
        assert_eq!(rest.entries.len(), 5);
        assert!(!rest.more);
        assert!(list(&roots, &[], &[], "Home/many", u32::MAX).unwrap().entries.is_empty());
    }
}
