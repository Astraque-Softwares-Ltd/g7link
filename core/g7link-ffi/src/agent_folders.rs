// SPDX-License-Identifier: MPL-2.0
//! A new thread in any folder (`docs/protocol/agents.md` §15): browse a
//! PC's folders by name and add one as an agent folder.
//!
//! The PC offers it as `agent.places` (`Unsupported`, without asking, while
//! it doesn't); its `agents` toggle for this phone governs it (`Denied`).
//! A place is a root's name ("Home", a disk's) and folder names below it,
//! `/`-separated: never an absolute path, a hidden folder or a link. No
//! `Debug` here prints a folder's name.

use g7link_core as core;

use crate::{AgentFolder, G7LinkError, G7LinkNode, Result, parse_id};

/// One folder in a place.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentPlace {
    pub name: String,
    /// It holds a git repository of its own: show the branch glyph and a
    /// direct "Use".
    pub repo: bool,
    /// It is already one of the PC's agent folders: using it adds nothing.
    pub folder: Option<String>,
}

impl std::fmt::Debug for AgentPlace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPlace").field("repo", &self.repo).field("folder", &self.folder).finish()
    }
}

/// A PC's roots (for an empty `place`: Home, then each mounted disk), or
/// one page of the folders in `place`, sorted by name. `more`: ask again
/// with `offset` = the entries had so far.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentPlaces {
    pub place: String,
    pub entries: Vec<AgentPlace>,
    pub more: bool,
}

impl std::fmt::Debug for AgentPlaces {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPlaces").field("entries", &self.entries.len()).field("more", &self.more).finish()
    }
}

impl From<core::AgentPlaces> for AgentPlaces {
    fn from(p: core::AgentPlaces) -> Self {
        AgentPlaces {
            place: p.place,
            entries: p
                .entries
                .into_iter()
                .map(|e| AgentPlace { name: e.name, repo: e.repo, folder: e.folder })
                .collect(),
            more: p.more,
        }
    }
}

#[uniffi::export]
impl G7LinkNode {
    /// Whether PC `id` offered browsing its folders (`agent.places`) when it
    /// last connected. Offer "Browse <PC>…" only while it did.
    pub fn agent_places_offered(&self, id: String) -> bool {
        parse_id(&id).is_ok_and(|id| self.node.agent_places_offered(id))
    }

    /// PC `id`'s roots for an empty `place` (Home first, then its disks by
    /// name; show Home as the user's home and the others as disks), else one
    /// page of the folders in `place` (`"Home/code"`) from `offset`.
    /// Errors: `Unsupported` while the PC doesn't offer it; `Denied` while
    /// its `agents` switch is off for this phone; `NotFound`: the place is
    /// gone; `Internal` whose reason starts with `BUSY`: a disk was slow to
    /// answer, try again; `Offline`.
    pub async fn agent_places(&self, id: String, place: String, offset: Option<u32>) -> Result<AgentPlaces> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        Ok(self.run(async move { node.agent_places(id, place, offset).await }).await?.into())
    }

    /// Makes the folder at `place` one of PC `id`'s agent folders and
    /// answers it as the catalog lists it (select it; the next catalog has
    /// it too). While the catalog's `add_folders` is off, someone at the PC
    /// approves it first (`Event.AgentFolderPrompt`: "Approve on <PC>"), and
    /// this waits up to 160 s. Sent once, never retried. Errors: `Denied`
    /// when they said no, didn't answer, or the place may not be added from
    /// a phone (a disk's top level, G7's own folders); `NotFound`;
    /// `Internal` starting `LIMIT` at the PC's folder cap; `Unsupported`;
    /// `Timeout`: it went out and no usable answer came; `Offline`.
    pub async fn agent_folder_add(&self, id: String, place: String) -> Result<AgentFolder> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let folder = self.run(async move { node.agent_folder_add(id, place).await }).await?;
        if folder.id.is_empty() {
            return Err(G7LinkError::Timeout);
        }
        Ok(folder.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_convert_and_never_print_a_name() {
        let places = AgentPlaces::from(core::AgentPlaces {
            place: "Home/code".into(),
            entries: vec![core::AgentPlace {
                name: "secret-project".into(),
                repo: true,
                folder: Some("f0123456789ab".into()),
            }],
            more: true,
        });
        assert_eq!(places.entries[0].name, "secret-project");
        assert!(places.entries[0].repo && places.more);
        assert!(!format!("{places:?} {:?}", places.entries[0]).contains("secret"));
    }
}
