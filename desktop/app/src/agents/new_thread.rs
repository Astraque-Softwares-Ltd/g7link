// SPDX-License-Identifier: GPL-3.0-or-later
//! The new-thread form, direction C ("where first, then quick picks"): the
//! folders by last use with what their newest thread ran, the quick picks
//! and which one the form opens on, the folder tree read from the host's
//! `places.list`, and adding a folder from the form (at once, with a one-line
//! note; Setup's "Add folder…" keeps its sheet).

use std::collections::{BTreeMap, HashMap};

use g7_agents::api::{self, AgentId, Folder};
use serde_json::{Value, json};

use super::{
    BridgeState, NewForm, changed, connected_client,
    control::{self, FolderLast, FolderUse, PickFrom, QuickPick, Setting},
    folder_error, lock,
};
use crate::{core_host, i18n::tr};

// ---- Folders and picks ----

/// The folders a pick can come from: those on this PC now, with what their
/// newest thread ran (`lasts`, from [`control::lasts`]).
pub(super) fn folder_uses<'a>(s: &'a BridgeState, lasts: &HashMap<String, FolderLast>) -> Vec<FolderUse<'a>> {
    s.folders
        .iter()
        .filter(|f| f.exists)
        .map(|f| FolderUse {
            id: &f.id,
            label: &f.label,
            used_ms: f.used_ms,
            last: lasts.get(&f.id).cloned(),
        })
        .collect()
}

/// The quick picks for `folder`.
pub(super) fn picks(s: &BridgeState, folder: &str) -> Vec<QuickPick> {
    let lasts = control::lasts(s.threads.values());
    control::quick_picks(&folder_uses(s, &lasts), &s.agents, folder)
}

fn setting_of(pick: &QuickPick) -> Setting {
    Setting { model: pick.model.clone(), effort: pick.effort.clone(), mode: None }
}

/// What the form opens on for `folder`: what its newest thread ran, else
/// what was last chosen there (its thread was deleted), else the first
/// quick pick. `None` when no agent can start a thread.
pub(super) fn preselect(s: &BridgeState, folder: &str) -> Option<(AgentId, Setting)> {
    let picks = picks(s, folder);
    if let Some(here) = picks.first().filter(|p| p.from == PickFrom::Here) {
        return Some((here.agent, setting_of(here)));
    }
    let usable = |id: AgentId| s.agents.iter().any(|a| a.id == id && control::usable(a));
    let remembered = s.prefs.choices.iter().rev().find(|c| c.folder == folder && usable(c.agent));
    if let Some(c) = remembered {
        return Some((c.agent, Setting { model: c.model.clone(), effort: c.effort.clone(), mode: None }));
    }
    picks.first().map(|p| (p.agent, setting_of(p)))
}

/// The setting when the user chooses `agent` under "Other…": its first quick
/// pick here, else what was last chosen for it here, else its default.
pub(super) fn agent_setting(s: &BridgeState, agent: AgentId, folder: &str) -> Setting {
    if let Some(p) = picks(s, folder).iter().find(|p| p.agent == agent) {
        return setting_of(p);
    }
    s.prefs
        .choice(agent, folder)
        .map(|c| Setting { model: c.model.clone(), effort: c.effort.clone(), mode: None })
        .unwrap_or_default()
}

/// The new-thread form's model control and Run with: the chosen agent's
/// picker (none while no agent can start a thread), and always the folder,
/// the quick picks, every agent with its state, the agents whose picks were
/// left out because they can't start now, and the folder just added from
/// the form (for its note).
pub(super) fn picker_json(s: &BridgeState) -> Value {
    let form = &s.new_form;
    let agent = form.agent.and_then(|id| s.agents.iter().find(|a| a.id == id));
    let mut picker = agent.map_or_else(
        || json!({}),
        |a| {
            let setting = Setting { mode: Some(form_mode(s, a)), ..form.setting.clone() };
            control::picker_json(a, &setting, None)
        },
    );
    let lasts = control::lasts(s.threads.values());
    let uses = folder_uses(s, &lasts);
    let picks = control::quick_picks(&uses, &s.agents, &form.folder);
    picker["control"] = json!(s.control);
    picker["folder"] = json!(form.folder);
    picker["picks"] = control::picks_json(&picks, &s.agents, form.agent.map(|id| (id, &form.setting)));
    picker["agents"] = control::agents_json(&s.agents);
    picker["leftOut"] =
        json!(control::left_out(&uses, &s.agents).iter().map(|a| a.as_str()).collect::<Vec<_>>());
    picker["added"] = json!(form.added.as_deref().filter(|id| *id == form.folder).unwrap_or_default());
    picker
}

/// The form's mode for `agent`: the one the user picked in the form while
/// the agent offers it, else the one the folder remembers for the agent
/// (plan §2.2: never `plan`; `ask` in a folder without one).
pub(super) fn form_mode(s: &BridgeState, agent: &api::AgentInfo) -> api::Mode {
    let form = &s.new_form;
    form.setting
        .mode
        .filter(|m| agent.offers(*m))
        .unwrap_or_else(|| super::folder_mode(s, Some(&form.folder), agent.id, form.setting.model.as_deref()))
}

/// A folder's newest thread as its card says it: the agent, the model and
/// effort it ran ("" when unknown), its title and the folder's branch then.
pub(super) fn last_json(s: &BridgeState, last: &FolderLast) -> Value {
    let agent = s.agents.iter().find(|a| a.id == last.agent);
    let model = agent
        .and_then(|a| control::setting_name(a, last.model.as_deref(), last.answered.as_deref()))
        .unwrap_or_default();
    let effort = agent
        .map(|a| control::shown_effort(a, last.model.as_deref(), last.effort.as_deref()))
        .unwrap_or_default();
    json!({
        "agent": last.agent.as_str(),
        "agentName": agent.map_or_else(|| last.agent.display_name().to_owned(), |a| a.name.clone()),
        "model": model,
        "effort": effort,
        "title": last.title,
        "branch": last.branch.clone().unwrap_or_default(),
    })
}

/// Folders as Setup and the form list them: the most recently used first.
pub(super) fn by_use(folders: &[Folder]) -> Vec<&Folder> {
    let mut sorted: Vec<&Folder> = folders.iter().collect();
    sorted.sort_by_key(|f| std::cmp::Reverse(f.used_ms));
    sorted
}

/// The form shows a folder just added from it: its agent and setting are
/// what it opens on, and the note shows under Where.
fn select_added(folder: Folder) {
    {
        let mut s = lock();
        if !s.folders.iter().any(|f| f.id == folder.id) {
            s.folders.push(folder.clone());
        }
        let id = folder.id;
        let (agent, setting) =
            preselect(&s, &id).map_or((None, Setting::default()), |(a, set)| (Some(a), set));
        s.new_form = NewForm { agent, folder: id.clone(), setting, added: Some(id) };
    }
    changed();
}

/// "Open the folder dialog…" in the form: adds the folder and selects it.
pub(crate) fn add_folder(path: String) {
    lock().user_started = true;
    core_host::spawn(async move {
        if let Some(folder) = super::added_folder(path).await {
            select_added(folder);
        }
    });
}

// ---- The folder tree ----

/// Places the tree keeps read; past that, branches away from the one being
/// opened are forgotten (and show closed).
const PLACES_KEPT: usize = 48;
/// Folders kept per place ("Show more" stops there).
const PLACE_ENTRIES_KEPT: usize = 1000;

/// What `places.list` answered for one place, page by page.
#[derive(Debug, Default)]
struct PlaceList {
    entries: Vec<api::Place>,
    more: bool,
    loading: bool,
    failed: bool,
}

/// The tree under "All folders on <PC>": the roots (`""`) and every place
/// read since the form opened, by place.
#[derive(Debug, Default)]
pub(crate) struct Places {
    lists: BTreeMap<String, PlaceList>,
}

/// `place` lies on the way to `to` (or is it).
fn on_the_way(place: &str, to: &str) -> bool {
    place.is_empty() || to == place || to.strip_prefix(place).is_some_and(|rest| rest.starts_with('/'))
}

impl Places {
    /// Marks `place` as being read from `offset`; false when that read
    /// isn't needed (it runs, or the page is already there).
    fn begin(&mut self, place: &str, offset: usize) -> bool {
        if self.lists.get(place).is_some_and(|l| l.loading) {
            return false;
        }
        if place.is_empty() && offset == 0 {
            // The roots: a fresh tree each time the form opens.
            self.lists.clear();
        }
        if !self.lists.contains_key(place) && self.lists.len() >= PLACES_KEPT {
            self.lists.retain(|p, _| on_the_way(p, place));
        }
        let list = self.lists.entry(place.to_owned()).or_default();
        if offset > 0 && (offset != list.entries.len() || offset >= PLACE_ENTRIES_KEPT) {
            return false;
        }
        if offset == 0 {
            *list = PlaceList::default();
        }
        list.loading = true;
        list.failed = false;
        true
    }

    /// The answer for `place` from `offset`; one for a list that changed
    /// meanwhile is dropped.
    fn answer(&mut self, place: &str, offset: usize, result: Option<api::PlacesListResult>) {
        let Some(list) = self.lists.get_mut(place).filter(|l| l.loading && l.entries.len() == offset) else {
            return;
        };
        list.loading = false;
        match result {
            Some(r) => {
                let room = PLACE_ENTRIES_KEPT.saturating_sub(list.entries.len());
                let more = r.more || r.entries.len() > room;
                list.entries.extend(r.entries.into_iter().take(room));
                list.more = more && list.entries.len() < PLACE_ENTRIES_KEPT;
            }
            None => list.failed = true,
        }
    }

    /// A place was added as folder `id`: its row says so.
    fn added(&mut self, place: &str, id: &str) {
        let (parent, name) = place.rsplit_once('/').unwrap_or(("", place));
        if let Some(entry) =
            self.lists.get_mut(parent).and_then(|l| l.entries.iter_mut().find(|e| e.name == name))
        {
            entry.folder_id = Some(id.to_owned());
        }
    }

    /// `{ "<place>": { entries: [{ name, repo, folderId }], more, loading, failed } }`.
    pub(crate) fn json(&self) -> Value {
        let lists = self.lists.iter().map(|(place, l)| {
            let entries: Vec<Value> = l
                .entries
                .iter()
                .map(|e| json!({ "name": e.name, "repo": e.repo, "folderId": e.folder_id.clone().unwrap_or_default() }))
                .collect();
            (place.clone(), json!({ "entries": entries, "more": l.more, "loading": l.loading, "failed": l.failed }))
        });
        Value::Object(lists.collect())
    }
}

/// A place the host can be asked about: as long as its limit allows.
fn place_ok(place: &str) -> bool {
    place.len() <= api::MAX_FOLDER_PLACE && !place.starts_with('/') && !place.ends_with('/')
}

/// Reads the folders in `place` (`""`: the roots, which starts the tree
/// afresh) from `offset` ("Show more").
pub(crate) fn places(place: String, offset: usize) {
    if !place_ok(&place) || !lock().places.begin(&place, offset) {
        return;
    }
    changed();
    core_host::spawn(async move {
        let result = match connected_client().await {
            Some(client) => {
                client.places_list(place.clone(), Some(offset.min(u32::MAX as usize) as u32)).await.ok()
            }
            None => None,
        };
        lock().places.answer(&place, offset, result);
        changed();
    });
}

/// A folder in the tree chosen in the form: added at once and selected.
pub(crate) fn add_place(place: String) {
    if place.is_empty() || !place_ok(&place) {
        return;
    }
    lock().user_started = true;
    core_host::spawn(async move {
        let Some(client) = connected_client().await else {
            crate::bridge::app::show_message(tr!("Agents", "Couldn't reach the agent host"));
            return;
        };
        match client.folders_add_place(place.clone(), None).await {
            Ok(folder) => {
                lock().places.added(&place, &folder.id);
                select_added(folder);
            }
            Err(e) => crate::bridge::app::show_message(folder_error(&e)),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(name: &str) -> api::Place {
        api::Place { name: name.into(), repo: false, folder_id: None }
    }

    fn answer(names: &[&str], more: bool) -> Option<api::PlacesListResult> {
        Some(api::PlacesListResult {
            place: String::new(),
            entries: names.iter().map(|n| place(n)).collect(),
            more,
        })
    }

    #[test]
    fn the_tree_reads_page_by_page_and_drops_stale_answers() {
        let mut places = Places::default();
        assert!(places.begin("", 0));
        assert!(!places.begin("", 0), "already being read");
        places.answer("", 0, answer(&["Home", "Local Disk"], false));
        assert!(places.begin("Home", 0));
        places.answer("Home", 0, answer(&["Code", "Music"], true));
        assert!(!places.begin("Home", 5), "not where the list ends");
        assert!(places.begin("Home", 2));
        places.answer("Home", 0, answer(&["Old"], false));
        places.answer("Home", 2, answer(&["Notes"], false));
        let json = places.json();
        let names: Vec<&str> =
            json["Home"]["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["Code", "Music", "Notes"], "an answer for another page is dropped");
        assert_eq!(json["Home"]["more"], false);
        places.added("Home/Code", "f0123456789ab");
        assert_eq!(places.json()["Home"]["entries"][0]["folderId"], "f0123456789ab");

        assert!(places.begin("Home/Music", 0));
        places.answer("Home/Music", 0, None);
        assert_eq!(places.json()["Home/Music"]["failed"], true);

        // Opening the form again reads the roots afresh.
        assert!(places.begin("", 0));
        assert_eq!(places.json().as_object().unwrap().len(), 1);
    }

    #[test]
    fn a_long_listing_stops_at_its_limit_and_far_branches_are_forgotten() {
        let mut places = Places::default();
        let many: Vec<String> = (0..PLACE_ENTRIES_KEPT + 10).map(|i| format!("d{i}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        assert!(places.begin("Home", 0));
        places.answer("Home", 0, answer(&refs, true));
        assert_eq!(places.json()["Home"]["entries"].as_array().unwrap().len(), PLACE_ENTRIES_KEPT);
        assert_eq!(places.json()["Home"]["more"], false);
        assert!(!places.begin("Home", PLACE_ENTRIES_KEPT));

        for i in 0..PLACES_KEPT - 1 {
            assert!(places.begin(&format!("Home/d{i}"), 0));
        }
        assert!(places.begin("Home/d1/deeper", 0));
        let kept = places.json();
        let kept: Vec<&String> = kept.as_object().unwrap().keys().collect();
        assert_eq!(kept, ["Home", "Home/d1", "Home/d1/deeper"], "only the way to the new one stays");
    }

    fn ready(id: AgentId) -> api::AgentInfo {
        let mut a = api::AgentInfo::missing(id);
        a.installed = true;
        a.signed_in = true;
        a
    }

    fn folder(id: &str, used_ms: u64) -> Folder {
        Folder {
            id: id.into(),
            label: id.into(),
            path: format!("/home/me/{id}"),
            exists: true,
            added_ms: 1,
            used_ms,
            repo: false,
            mcp: true,
            mcp_seen: Vec::new(),
            checkpoints: true,
            follow_remote: false,
            scripts: Vec::new(),
            phone_scripts: false,
            last_modes: Vec::new(),
        }
    }

    fn thread(folder: &str, agent: AgentId, model: Option<&str>) -> api::ThreadInfo {
        let json = json!({
            "id": format!("{folder:0>32}"), "agent": agent.as_str(), "folder_id": folder, "folder_label": folder,
            "title": "t", "state": "done", "mode": "supervised", "origin": { "kind": "launcher" },
            "created_ms": 1, "updated_ms": 2, "last_seq": 0, "pending": 0, "model": model,
        });
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_folder_opens_on_its_newest_thread_else_what_was_chosen_there_else_the_first_pick() {
        let mut s = BridgeState {
            agents: vec![ready(AgentId::Claude), ready(AgentId::Codex)],
            folders: vec![folder("f000000000001", 5), folder("f000000000002", 9), folder("f000000000003", 1)],
            ..BridgeState::default()
        };
        let t = thread("f000000000001", AgentId::Codex, Some("gpt-y"));
        s.threads.insert(t.id.clone(), t);
        s.prefs.remember(control::Choice {
            agent: AgentId::Claude,
            folder: "f000000000001".into(),
            model: Some("sonnet".into()),
            effort: None,
            custom: false,
        });
        let (agent, setting) = preselect(&s, "f000000000001").unwrap();
        assert_eq!(
            (agent, setting.model.as_deref()),
            (AgentId::Codex, Some("gpt-y")),
            "its newest thread, not the last click"
        );

        s.prefs.remember(control::Choice {
            agent: AgentId::Claude,
            folder: "f000000000002".into(),
            model: Some("opus".into()),
            effort: Some("high".into()),
            custom: false,
        });
        let (agent, setting) = preselect(&s, "f000000000002").unwrap();
        assert_eq!(
            (agent, setting.model.as_deref(), setting.effort.as_deref()),
            (AgentId::Claude, Some("opus"), Some("high")),
            "no thread there: what was chosen there"
        );
        let (agent, setting) = preselect(&s, "f000000000003").unwrap();
        assert_eq!(
            (agent, setting.model.as_deref()),
            (AgentId::Codex, Some("gpt-y")),
            "else the first pick: used elsewhere"
        );
        assert_eq!(
            agent_setting(&s, AgentId::Claude, "f000000000003"),
            Setting::default(),
            "Claude's own default"
        );

        s.agents.iter_mut().for_each(|a| a.signed_in = false);
        assert_eq!(preselect(&s, "f000000000001"), None, "no agent can start");
        s.folders.reverse();
        let order: Vec<&str> = by_use(&s.folders).iter().map(|f| f.id.as_str()).collect();
        assert_eq!(order, ["f000000000002", "f000000000001", "f000000000003"]);
    }

    #[test]
    fn the_form_starts_in_the_folders_mode_for_its_agent_until_another_is_picked() {
        use api::{FolderMode, Mode};
        let mut claude = ready(AgentId::Claude);
        claude.modes = vec![Mode::Ask, Mode::AcceptEdits, Mode::Plan, Mode::Full];
        let mut codex = ready(AgentId::Codex);
        codex.modes = vec![Mode::Ask, Mode::Auto, Mode::Full, Mode::Plan];
        let mut used = folder("f000000000001", 5);
        used.last_modes = vec![FolderMode { agent: AgentId::Claude, mode: Mode::Full }];
        let mut s = BridgeState {
            agents: vec![claude, codex],
            folders: vec![used, folder("f000000000002", 1)],
            ..BridgeState::default()
        };
        let form = |agent, folder: &str| NewForm {
            agent: Some(agent),
            folder: folder.into(),
            setting: Setting::default(),
            added: None,
        };
        let mode = |s: &BridgeState| {
            let p = picker_json(s);
            (p["mode"].as_str().unwrap().to_owned(), p["modeName"].as_str().unwrap().to_owned())
        };
        s.new_form = form(AgentId::Claude, "f000000000001");
        assert_eq!(mode(&s), ("full".into(), "Bypass permissions".into()), "what Claude last ran there");
        s.new_form = form(AgentId::Codex, "f000000000001");
        assert_eq!(mode(&s), ("ask".into(), "Ask for approval".into()), "Claude's mode isn't Codex's");
        s.new_form = form(AgentId::Claude, "f000000000002");
        assert_eq!(mode(&s).0, "ask", "a folder without one starts ask-first");
        s.new_form.setting.mode = Some(Mode::Plan);
        assert_eq!(mode(&s).0, "plan", "what the user picked in the form");
        s.new_form = form(AgentId::Codex, "f000000000001");
        s.new_form.setting.mode = Some(Mode::AcceptEdits);
        assert_eq!(mode(&s).0, "ask", "a pick the agent doesn't offer gives way");
        let ids: Vec<String> = picker_json(&s)["modes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(ids, ["ask", "auto", "full", "plan"], "Codex's own order");
    }

    #[test]
    fn only_a_place_within_its_limit_is_asked_about() {
        assert!(place_ok("Home/Code"));
        assert!(!place_ok("/home/me"));
        assert!(!place_ok("Home/"));
        assert!(!place_ok(&"a".repeat(api::MAX_FOLDER_PLACE + 1)));
        assert!(on_the_way("Home", "Home/Code"));
        assert!(!on_the_way("Home/Co", "Home/Code"));
    }
}
