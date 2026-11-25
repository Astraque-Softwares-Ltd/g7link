// SPDX-License-Identifier: MPL-2.0
//! Bodies of a new thread in any folder (`docs/protocol/agents.md` §15): a
//! phone browses the PC's folders by name and adds one as an agent folder.
//! A place is a root's name ("Home", a mounted disk's) and `/`-separated
//! folder names below it; a phone never sees or sends an absolute path, a
//! hidden folder or a link. The PC offers it as `agent.places`; the
//! `agents` toggle governs it, and adding a folder also the PC's
//! `agent_folders` toggle: while that is off, someone at the PC is asked
//! (`agent.folder.prompt` tells the phone) and the add waits for them.
//!
//! A `Debug` prints no folder name.

use serde::{Deserialize, Serialize};

use super::{AgentCatalogFolder, is_bidi_control, is_valid_agent_id};

/// The capability, toggle and limits of §15.
pub mod agent_places {
    /// Desktop: lists places and adds folders for a phone.
    pub const CAP: &str = "agent.places";
    /// Per phone, on the PC (off): it adds folders without the PC asking.
    pub const TOGGLE: &str = "agent_folders";
    /// The root for the PC user's home folder; other roots are disks.
    pub const HOME: &str = "Home";
    /// A place: bytes in all, names below its root, bytes per name.
    pub const MAX_PLACE_BYTES: usize = 1024;
    pub const MAX_PLACE_NAMES: usize = 16;
    pub const MAX_NAME_BYTES: usize = 255;
    /// Folders in one `agent.places` answer; `offset` at most.
    pub const MAX_ENTRIES: usize = 100;
    pub const MAX_OFFSET: u32 = 4000;
    /// How long a phone waits for `agent.folder.add`: someone at the PC may
    /// be asked first.
    pub const ADD_WAIT_SECS: u64 = 160;
    /// How long the PC waits for its user before it answers `DENIED`.
    pub const ASK_SECS: u64 = 150;
    /// `msg` of a `DENIED` when someone at the PC said no or didn't answer.
    pub const DECLINED: &str = "declined";
}

/// Whether `name` can be a folder's name in a place: 1 to 255 bytes, not
/// hidden, no separator, control or direction character.
pub fn is_place_name(name: &str) -> bool {
    (1..=agent_places::MAX_NAME_BYTES).contains(&name.len())
        && !name.starts_with('.')
        && !name.contains(['/', '\\'])
        && !name.chars().any(|c| c.is_control() || is_bidi_control(c))
}

/// Whether `place` is "" (the roots) or a root's name and up to 16 names:
/// no empty, `.` or `..` part, no leading or trailing `/`.
pub fn is_place(place: &str) -> bool {
    if place.is_empty() {
        return true;
    }
    if place.len() > agent_places::MAX_PLACE_BYTES {
        return false;
    }
    let mut parts = place.split('/');
    let root_ok = parts
        .next()
        .is_some_and(|root| !root.is_empty() && !root.chars().any(|c| c.is_control() || is_bidi_control(c)));
    let names: Vec<&str> = parts.collect();
    root_ok && names.len() <= agent_places::MAX_PLACE_NAMES && names.iter().all(|n| is_place_name(n))
}

/// Body of `agent.places` (phone → PC, RPC `agent/places`): the roots for
/// an empty `place`, else the folders in it, from `offset`.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentPlacesRequest {
    #[serde(default)]
    pub place: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
}

impl std::fmt::Debug for AgentPlacesRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPlacesRequest")
            .field("depth", &self.place.split('/').filter(|p| !p.is_empty()).count())
            .field("offset", &self.offset)
            .finish()
    }
}

impl AgentPlacesRequest {
    pub fn is_valid(&self) -> bool {
        is_place(&self.place) && self.offset.is_none_or(|o| o <= agent_places::MAX_OFFSET)
    }
}

/// One folder in a place: its name, whether it's a repository, and its
/// agent folder's id when it already is one.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentPlace {
    pub name: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub repo: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

/// Never prints the name.
impl std::fmt::Debug for AgentPlace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPlace").field("repo", &self.repo).field("folder", &self.folder).finish()
    }
}

/// The `ok` answer of `agent.places`: the place asked about, its folders
/// sorted by name (the roots for ""), and whether more follow from
/// `offset + entries`.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentPlaces {
    #[serde(default)]
    pub place: String,
    #[serde(default)]
    pub entries: Vec<AgentPlace>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub more: bool,
}

impl std::fmt::Debug for AgentPlaces {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPlaces").field("entries", &self.entries.len()).field("more", &self.more).finish()
    }
}

impl AgentPlaces {
    /// Within its limits, on either side: entries with a name that isn't one
    /// (or a root's that isn't whole) left out, a bad folder id dropped, at
    /// most [`agent_places::MAX_ENTRIES`] (then `more`); `None` when the
    /// place isn't one.
    pub fn sanitized(mut self) -> Option<Self> {
        if !is_place(&self.place) {
            return None;
        }
        let roots = self.place.is_empty();
        self.entries.retain(|e| {
            if roots { is_place(&e.name) && !e.name.contains('/') } else { is_place_name(&e.name) }
        });
        for entry in &mut self.entries {
            if !entry.folder.as_deref().is_none_or(is_valid_agent_id) {
                entry.folder = None;
            }
            if roots {
                entry.repo = false;
            }
        }
        if self.entries.len() > agent_places::MAX_ENTRIES {
            self.entries.truncate(agent_places::MAX_ENTRIES);
            self.more = true;
        }
        Some(self)
    }
}

/// Body of `agent.folder.add` (phone → PC, RPC `agent/folder.add`): make
/// the folder at `place` an agent folder. Never a root itself.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentFolderAdd {
    pub place: String,
}

impl std::fmt::Debug for AgentFolderAdd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentFolderAdd").field("depth", &self.place.split('/').count()).finish()
    }
}

impl AgentFolderAdd {
    pub fn is_valid(&self) -> bool {
        is_place(&self.place) && self.place.contains('/')
    }
}

/// The `ok` answer of `agent.folder.add`: the folder as the catalog lists
/// it; the catalog that follows has it too.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct AgentFolderAdded {
    pub folder: AgentCatalogFolder,
}

impl AgentFolderAdded {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.folder.id)
    }
}

/// Body of `agent.folder.prompt` (PC → phone, `agent` lane): someone at
/// the PC is being asked about the add that `place` names. It may come
/// more than once, and after the answer: a phone ignores one for an add it
/// no longer waits on.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentFolderPrompt {
    pub place: String,
}

impl std::fmt::Debug for AgentFolderPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentFolderPrompt").finish_non_exhaustive()
    }
}

impl AgentFolderPrompt {
    pub fn is_valid(&self) -> bool {
        is_place(&self.place) && !self.place.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Envelope,
        lanes::{Lane, lane_of, rpc_of, svc},
        messages::types,
    };

    fn round_trip<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
        t: &str,
        body: &T,
    ) {
        let env = Envelope::new(t, body).unwrap();
        let back: T = env.body().unwrap();
        assert_eq!(&back, body);
    }

    #[test]
    fn places_are_names_below_a_root() {
        for good in ["", "Home", "Local Disk", "Home/code", "Local Disk/code/orbit-api", "Home/a b/ç"] {
            assert!(is_place(good), "{good}");
        }
        let deep = format!("Home/{}", vec!["a"; agent_places::MAX_PLACE_NAMES + 1].join("/"));
        let long = format!("Home/{}", "a".repeat(agent_places::MAX_NAME_BYTES + 1));
        for bad in [
            "/Home",
            "Home/",
            "Home//code",
            "Home/..",
            "Home/.",
            "Home/.ssh",
            "Home/a\\b",
            "Home/a\u{202e}b",
            "Home/a\nb",
            "\u{7}",
            deep.as_str(),
            long.as_str(),
        ] {
            assert!(!is_place(bad), "{bad:?}");
        }
    }

    #[test]
    fn requests_and_answers_round_trip_and_check() {
        let request = AgentPlacesRequest { place: "Home/code".into(), offset: Some(100) };
        assert!(request.is_valid());
        round_trip(types::AGENT_PLACES, &request);
        assert!(
            !AgentPlacesRequest { place: "Home".into(), offset: Some(agent_places::MAX_OFFSET + 1) }
                .is_valid()
        );
        let add = AgentFolderAdd { place: "Home/code/orbit".into() };
        assert!(add.is_valid());
        round_trip(types::AGENT_FOLDER_ADD, &add);
        assert!(!AgentFolderAdd { place: "Home".into() }.is_valid(), "never a root itself");
        assert!(!AgentFolderAdd { place: String::new() }.is_valid());
        let prompt = AgentFolderPrompt { place: "Home/code/orbit".into() };
        assert!(prompt.is_valid());
        round_trip(types::AGENT_FOLDER_PROMPT, &prompt);
        let added = AgentFolderAdded {
            folder: AgentCatalogFolder {
                id: "f0123456789ab".into(),
                label: "orbit".into(),
                ..Default::default()
            },
        };
        assert!(added.is_valid());
        round_trip(types::OK, &added);
    }

    #[test]
    fn an_answer_is_made_to_fit() {
        let mut entries: Vec<AgentPlace> = (0..agent_places::MAX_ENTRIES + 3)
            .map(|n| AgentPlace { name: format!("d{n}"), repo: n == 0, folder: None })
            .collect();
        entries.push(AgentPlace { name: ".hidden".into(), ..Default::default() });
        entries.push(AgentPlace { name: "a/b".into(), ..Default::default() });
        entries[1].folder = Some("NOT-AN-ID".into());
        entries[2].folder = Some("f0123456789ab".into());
        let places = AgentPlaces { place: "Home/code".into(), entries, more: false }.sanitized().unwrap();
        assert_eq!(places.entries.len(), agent_places::MAX_ENTRIES);
        assert!(places.more && places.entries[0].repo);
        assert_eq!(places.entries[1].folder, None);
        assert_eq!(places.entries[2].folder.as_deref(), Some("f0123456789ab"));
        assert!(places.entries.iter().all(|e| is_place_name(&e.name)));
        let roots = AgentPlaces {
            place: String::new(),
            entries: vec![
                AgentPlace { name: "Home".into(), repo: true, folder: None },
                AgentPlace { name: "Local Disk".into(), ..Default::default() },
                AgentPlace { name: "a/b".into(), ..Default::default() },
            ],
            more: false,
        }
        .sanitized()
        .unwrap();
        let names: Vec<&str> = roots.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Home", "Local Disk"]);
        assert!(!roots.entries[0].repo, "a root is never a repository");
        assert!(AgentPlaces { place: "/x".into(), ..Default::default() }.sanitized().is_none());
    }

    #[test]
    fn nothing_prints_a_folder_name() {
        let shown = format!(
            "{:?} {:?} {:?} {:?}",
            AgentPlacesRequest { place: "Home/secret-project".into(), offset: None },
            AgentFolderAdd { place: "Home/secret-project".into() },
            AgentFolderPrompt { place: "Home/secret-project".into() },
            AgentPlaces {
                place: "Home".into(),
                entries: vec![AgentPlace { name: "secret-project".into(), ..Default::default() }],
                more: false
            },
        );
        assert!(!shown.contains("secret"), "{shown}");
    }

    #[test]
    fn lanes() {
        assert_eq!(rpc_of(types::AGENT_PLACES).map(|op| op.op), Some("places"));
        assert_eq!(rpc_of(types::AGENT_FOLDER_ADD).map(|op| op.op), Some("folder.add"));
        assert_eq!(lane_of(types::AGENT_FOLDER_PROMPT), Some(Lane::Service(svc::AGENT)));
    }
}
