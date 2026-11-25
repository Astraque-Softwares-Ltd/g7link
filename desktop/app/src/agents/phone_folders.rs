// SPDX-License-Identifier: GPL-3.0-or-later
//! A phone's new thread in any folder (`docs/protocol/agents.md` §15), PC
//! side: the phone browses this PC's folders by name (the host's
//! `places.list`) and adds one (`folders.add` by place, the phone named as
//! who chose it). While the phone's `agent_folders` toggle is off, someone at
//! this PC approves each add first: a sheet ("Let Pixel start agents in
//! barber?") with "Don't ask again for this phone", which turns the toggle
//! on. No answer, "Not now", or a second ask while one is open is a refusal.
//! The host tells the PC about every add a phone makes.

use std::time::Duration;

use g7_agents::{
    api::{self, Folder, Origin},
    client::Client,
};
use g7link_core::{AgentFailure, AgentPlace, AgentPlaces, AgentReply, DeviceId, agent_places};
use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::{catalog_folder, changed, device_name, failure, lock, wire_last_of};

/// How long someone at the PC has to answer: a little less than the core
/// gives the platform, so this refusal arrives first.
pub(crate) const ASK_WAIT: Duration = Duration::from_secs(agent_places::ASK_SECS - 5);

/// The add being asked about at this PC.
pub(crate) struct Prompt {
    id: u64,
    device: DeviceId,
    device_name: String,
    place: String,
    answer: Option<oneshot::Sender<bool>>,
}

/// Never prints the place.
impl std::fmt::Debug for Prompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prompt")
            .field("id", &self.id)
            .field("device", &self.device.short())
            .finish_non_exhaustive()
    }
}

/// A host's `places.list` answer as a phone gets it: at most
/// [`agent_places::MAX_ENTRIES`] folders, `more` when there are others
/// (the phone asks again from the number it has; the host counts the same).
pub(crate) fn wire_places(result: api::PlacesListResult) -> AgentPlaces {
    let more = result.more || result.entries.len() > agent_places::MAX_ENTRIES;
    let entries = result
        .entries
        .into_iter()
        .take(agent_places::MAX_ENTRIES)
        .map(|p| AgentPlace { name: p.name, repo: p.repo, folder: p.folder_id })
        .collect();
    AgentPlaces { place: result.place, entries, more }
}

/// `agent.places`: the roots for "", else the folders in `place` from `offset`.
pub(crate) async fn places(
    client: &Client,
    place: String,
    offset: Option<u32>,
) -> Result<AgentReply, AgentFailure> {
    let result = client.places_list(place, offset).await.map_err(|e| failure(&e))?;
    Ok(AgentReply::Places(wire_places(result)))
}

/// `agent.folder.add`: asks at this PC first when `ask` (the phone's
/// `agent_folders` toggle is off), then adds the folder as the phone's
/// choice and answers with it as the catalog lists it.
pub(crate) async fn add(
    client: &Client,
    from: DeviceId,
    place: String,
    ask: bool,
) -> Result<AgentReply, AgentFailure> {
    if ask && !asked(from, &place).await {
        return Err(AgentFailure::Denied);
    }
    let by = Origin::phone(from.to_string(), device_name(&from));
    let folder = client.folders_add_place(place, Some(by)).await.map_err(|e| failure(&e))?;
    // A folder that was one already comes back as it was, threads and all.
    let wire = catalog_folder(&folder, wire_last_of(&folder.id));
    remember(folder);
    Ok(AgentReply::FolderAdded(wire))
}

/// The folder is one of this PC's now (the host's `folders.changed` follows).
fn remember(folder: Folder) {
    {
        let mut s = lock();
        if !s.folders.iter().any(|f| f.id == folder.id) {
            s.folders.push(folder);
        }
    }
    changed();
}

/// Shows the sheet and waits for its answer: true when someone at this PC
/// allowed the add. A second ask while one is open is refused at once.
async fn asked(from: DeviceId, place: &str) -> bool {
    let (tx, rx) = oneshot::channel();
    let id = {
        let mut s = lock();
        if s.folder_prompt.is_some() {
            return false;
        }
        let id = s.next_gen();
        s.folder_prompt = Some(Prompt {
            id,
            device: from,
            device_name: device_name(&from),
            place: place.to_owned(),
            answer: Some(tx),
        });
        id
    };
    changed();
    let allowed = matches!(tokio::time::timeout(ASK_WAIT, rx).await, Ok(Ok(true)));
    let closed = {
        let mut s = lock();
        let ours = s.folder_prompt.as_ref().is_some_and(|p| p.id == id);
        if ours {
            s.folder_prompt = None;
        }
        ours
    };
    if closed {
        changed();
    }
    allowed
}

/// The sheet's answer: "Allow" or "Not now" (or closed).
pub(crate) fn answer(allow: bool) {
    let prompt = lock().folder_prompt.take();
    let Some(mut prompt) = prompt else { return };
    if let Some(tx) = prompt.answer.take() {
        let _ = tx.send(allow);
    }
    changed();
}

/// The sheet: `{ id, device, deviceName, place, name }` (`{}` while none is open).
pub(crate) fn prompt_json(prompt: Option<&Prompt>) -> Value {
    let Some(p) = prompt else { return json!({}) };
    json!({
        "id": p.id,
        "device": p.device.to_string(),
        "deviceName": p.device_name,
        "place": p.place,
        "name": p.place.rsplit('/').next().unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(name: &str, folder: Option<&str>) -> api::Place {
        api::Place { name: name.into(), repo: name.starts_with('r'), folder_id: folder.map(str::to_owned) }
    }

    #[test]
    fn a_long_place_reaches_the_phone_a_hundred_at_a_time() {
        let entries: Vec<api::Place> = (0..150).map(|i| place(&format!("d{i:03}"), None)).collect();
        let wire = wire_places(api::PlacesListResult { place: "Home/Code".into(), entries, more: false });
        assert_eq!(wire.entries.len(), agent_places::MAX_ENTRIES);
        assert!(wire.more, "cut: the phone asks again from 100");
        assert_eq!(wire.place, "Home/Code");
        assert_eq!(wire.entries[99].name, "d099");

        let wire = wire_places(api::PlacesListResult {
            place: "Home".into(),
            entries: vec![place("repo", Some("f0123456789ab")), place("notes", None)],
            more: false,
        });
        assert!(!wire.more);
        assert_eq!((wire.entries[0].repo, wire.entries[0].folder.as_deref()), (true, Some("f0123456789ab")));
        assert_eq!((wire.entries[1].repo, wire.entries[1].folder.as_deref()), (false, None));
        let wire =
            wire_places(api::PlacesListResult { place: "Home".into(), entries: Vec::new(), more: true });
        assert!(wire.more, "the host's own 'more' stays");
    }

    #[test]
    fn the_sheet_shows_the_phone_and_the_folder_and_nothing_while_closed() {
        assert_eq!(prompt_json(None), json!({}));
        let device = DeviceId([7; 32]);
        let prompt = Prompt {
            id: 3,
            device,
            device_name: "Pixel 8 Pro".into(),
            place: "Local Disk/code/ml-notebooks".into(),
            answer: None,
        };
        let shown = prompt_json(Some(&prompt));
        assert_eq!(shown["deviceName"], "Pixel 8 Pro");
        assert_eq!(shown["name"], "ml-notebooks");
        assert_eq!(shown["place"], "Local Disk/code/ml-notebooks");
        assert_eq!(shown["device"], device.to_string());
        assert!(!format!("{prompt:?}").contains("ml-notebooks"), "never printed");
    }

    type Log = std::sync::Arc<std::sync::Mutex<Vec<(String, Value)>>>;

    /// A host that adds any place as folder `f0123456789ab` and logs each request.
    async fn folders_host(path: std::path::PathBuf, log: Log) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let request: Value = serde_json::from_str(&line).unwrap();
                let method = request["method"].as_str().unwrap_or_default().to_owned();
                log.lock().unwrap().push((method.clone(), request["params"].clone()));
                let result = match method.as_str() {
                    "hello" => json!({ "version": 1, "host_version": "test", "pid": 1 }),
                    "folders.add" => json!({ "folder": {
                        "id": "f0123456789ab", "label": "ml-notebooks", "path": "/home/me/code/ml-notebooks",
                        "exists": true, "added_ms": 5, "used_ms": 5, "repo": true,
                    } }),
                    "places.list" => {
                        json!({ "place": "Home", "entries": [{ "name": "code" }], "more": true })
                    }
                    _ => json!({}),
                };
                let reply = json!({ "id": request["id"], "result": result });
                write.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
            }
        }
    }

    async fn connected(log: &Log) -> (tempfile::TempDir, Client) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join("host.sock");
        tokio::spawn(folders_host(path.clone(), log.clone()));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !path.exists() {
            assert!(tokio::time::Instant::now() < deadline, "the host never listened");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let (client, _events) = Client::connect(&path, super::super::hello()).await.unwrap();
        (dir, client)
    }

    fn params_of(log: &Log, method: &str) -> Option<Value> {
        log.lock().unwrap().iter().rev().find(|(m, _)| m == method).map(|(_, p)| p.clone())
    }

    #[tokio::test]
    async fn a_phone_browses_and_adds_and_the_pc_asks_only_when_its_switch_is_off() {
        let log = Log::default();
        let (_dir, client) = connected(&log).await;
        let phone = DeviceId([9; 32]);

        let Ok(AgentReply::Places(places)) = places(&client, "Home".into(), Some(100)).await else {
            panic!()
        };
        assert_eq!((places.entries[0].name.as_str(), places.more), ("code", true));
        assert_eq!(params_of(&log, "places.list").unwrap(), json!({ "place": "Home", "offset": 100 }));

        // The switch is on: straight through, as the phone's choice.
        let reply = add(&client, phone, "Local Disk/code/ml-notebooks".into(), false).await;
        let Ok(AgentReply::FolderAdded(folder)) = reply else { panic!("{reply:?}") };
        assert_eq!(
            (folder.id.as_str(), folder.label.as_str(), folder.repo),
            ("f0123456789ab", "ml-notebooks", true)
        );
        let params = params_of(&log, "folders.add").unwrap();
        assert_eq!(params["place"], "Local Disk/code/ml-notebooks");
        assert_eq!(params["by"]["kind"], "phone");
        assert!(params.get("path").is_none(), "a phone never names a path");
        assert!(lock().folder_prompt.is_none(), "nobody was asked");
        assert!(lock().folders.iter().any(|f| f.id == "f0123456789ab"));

        // Off: the sheet asks; Allow adds it.
        let adds = || log.lock().unwrap().iter().filter(|(m, _)| m == "folders.add").count();
        let asking = tokio::spawn({
            let client = client.clone();
            async move { add(&client, phone, "Home/notes".into(), true).await }
        });
        let shown = loop {
            if let Some(json) = lock().folder_prompt.as_ref().map(|p| prompt_json(Some(p))) {
                break json;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert_eq!(shown["name"], "notes");
        assert!(
            matches!(add(&client, phone, "Home/other".into(), true).await, Err(AgentFailure::Denied)),
            "one sheet at a time: a second ask is refused"
        );
        assert_eq!(adds(), 1, "nothing added before the answer");
        answer(true);
        assert!(matches!(asking.await.unwrap(), Ok(AgentReply::FolderAdded(_))));
        assert_eq!(adds(), 2);
        assert!(lock().folder_prompt.is_none());

        // Not now: refused, and nothing is added.
        let asking = tokio::spawn({
            let client = client.clone();
            async move { add(&client, phone, "Home/notes".into(), true).await }
        });
        while lock().folder_prompt.is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        answer(false);
        assert!(matches!(asking.await.unwrap(), Err(AgentFailure::Denied)));
        assert_eq!(adds(), 2);
    }
}
