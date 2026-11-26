// SPDX-License-Identifier: MPL-2.0
//! A new thread in any folder (`docs/protocol/agents.md` §15): a phone
//! browses the PC's folders by name and adds one as an agent folder.
//!
//! PC side: the core checks that this PC offers `agent.places`, then the
//! `agents` toggle (with its one-time prompt) and the request's limits, and
//! only then asks the platform ([`AgentRequest::Places`],
//! [`AgentRequest::FolderAdd`]). For an add while the phone's
//! `agent_folders` toggle is off, it first tells the phone that someone at
//! the PC is being asked (`agent.folder.prompt`) and gives the platform
//! [`agent_places::ASK_SECS`] to ask them. The host's own checks (a root,
//! a hidden folder, a link, G7's own directories) stay the judge.
//! Phone side: it sends each request once and never retries an add.

use std::sync::Arc;
use std::time::Duration;

pub use g7link_protocol::messages::{
    AgentCatalog, AgentCatalogFolder, AgentFolderAdd, AgentFolderAdded, AgentFolderLast, AgentFolderMode,
    AgentFolderPrompt, AgentPlace, AgentPlaces, AgentPlacesRequest, agent_places,
};
use g7link_protocol::{
    DeviceId, Envelope, ErrorCode,
    messages::{is_valid_agent_id, types},
};

use crate::{
    Error, Node, NodeEvent, Result,
    agents::{self, AgentFailure, AgentReply, AgentRequest},
    dispatch::Responder,
    node::Shared,
};

/// Whether `t` is one of §15's requests.
pub(crate) fn is_request(t: &str) -> bool {
    t == types::AGENT_PLACES || t == types::AGENT_FOLDER_ADD
}

/// Whether `peer` adds folders without the PC asking: its `agent_folders`
/// toggle, read for each request.
pub(crate) fn may_add(shared: &Shared, peer: &DeviceId) -> bool {
    shared.toggle_on(peer, agent_places::TOGGLE)
}

/// PC: answers `agent.places` or `agent.folder.add` from `peer`.
pub(crate) async fn handle_request(
    shared: &Arc<Shared>,
    peer: DeviceId,
    env: &Envelope,
    rsp: &Responder<'_>,
) -> Result<()> {
    rsp.send(answer(shared, peer, env).await).await
}

async fn answer(shared: &Arc<Shared>, peer: DeviceId, env: &Envelope) -> Envelope {
    if !shared.we_offer(agent_places::CAP) {
        return Envelope::error(ErrorCode::Unsupported, "folders can't be browsed on this PC now");
    }
    if let Some(refused) = agents::refusal(shared, peer, false) {
        return refused;
    }
    if env.t == types::AGENT_PLACES { places(shared, peer, env).await } else { add(shared, peer, env).await }
}

async fn places(shared: &Arc<Shared>, peer: DeviceId, env: &Envelope) -> Envelope {
    let Some(body) = agents::decode::<AgentPlacesRequest>(env, AgentPlacesRequest::is_valid) else {
        return Envelope::error(ErrorCode::BadMessage, "invalid agent.places");
    };
    let request = AgentRequest::Places { place: body.place.clone(), offset: body.offset };
    match agents::ask_platform(shared, peer, request).await {
        Ok(AgentReply::Places(places)) => match places.sanitized().filter(|p| p.place == body.place) {
            Some(places) => {
                Envelope::new(types::OK, &places).unwrap_or_else(|_| AgentFailure::Internal.reply("failed"))
            }
            None => AgentFailure::Internal.reply("the host's answer was malformed"),
        },
        Ok(_) => AgentFailure::Internal.reply("the host answered something else"),
        Err(failure) => failure.reply(agents::failure_message(failure)),
    }
}

async fn add(shared: &Arc<Shared>, peer: DeviceId, env: &Envelope) -> Envelope {
    let Some(body) = agents::decode::<AgentFolderAdd>(env, AgentFolderAdd::is_valid) else {
        return Envelope::error(ErrorCode::BadMessage, "invalid agent.folder.add");
    };
    let ask = !may_add(shared, &peer);
    let limit = if ask {
        if let Some(session) = shared.session(&peer)
            && let Ok(prompt) =
                Envelope::new(types::AGENT_FOLDER_PROMPT, &AgentFolderPrompt { place: body.place.clone() })
        {
            let _ = session.send(prompt).await;
        }
        // A little longer than the platform gives its user, so its own
        // refusal arrives first.
        Duration::from_secs(agent_places::ASK_SECS + 5)
    } else {
        agents::PLATFORM_LIMIT
    };
    let request = AgentRequest::FolderAdd { place: body.place, ask };
    match agents::ask_platform_within(shared, peer, request, limit).await {
        Ok(AgentReply::FolderAdded(folder)) => match shown(folder) {
            Some(folder) => Envelope::new(types::OK, &AgentFolderAdded { folder })
                .unwrap_or_else(|_| AgentFailure::Internal.reply("failed")),
            None => AgentFailure::Internal.reply("the host's answer was malformed"),
        },
        Ok(_) => AgentFailure::Internal.reply("the host answered something else"),
        Err(AgentFailure::Denied) if ask => Envelope::error(ErrorCode::Denied, agent_places::DECLINED),
        Err(failure) => failure.reply(agents::failure_message(failure)),
    }
}

/// The folder as a catalog carries it: cleaned and cut; `None` without an id.
fn shown(folder: AgentCatalogFolder) -> Option<AgentCatalogFolder> {
    if !is_valid_agent_id(&folder.id) {
        return None;
    }
    AgentCatalog { folders: vec![folder], ..Default::default() }.sanitized().folders.pop()
}

/// Phone: a PC asks its user about an add; the app tells its own.
pub(crate) fn prompt_arrived(shared: &Arc<Shared>, peer: DeviceId, env: &Envelope) {
    if let Ok(prompt) = env.body::<AgentFolderPrompt>()
        && prompt.is_valid()
    {
        shared.emit(NodeEvent::AgentFolderPrompt { device: peer, place: prompt.place });
    }
}

/// The node's §15 methods (phone).
impl Node {
    /// Phone: whether paired PC `peer` offers `agent.places`, as it said
    /// when it last connected.
    pub fn agent_places_offered(&self, peer: DeviceId) -> bool {
        self.shared().peer_offers_cap(&peer, agent_places::CAP)
    }

    /// Phone: the roots of PC `peer` (Home, its mounted disks) for an empty
    /// `place`, else one page of the folders in it from `offset`.
    /// `Unsupported` while the PC doesn't offer `agent.places`; `Denied`
    /// while its `agents` toggle is off; `NotFound` for a place that isn't
    /// there; a `Protocol` error starting `BUSY` for a disk slow to answer.
    pub async fn agent_places(
        &self,
        peer: DeviceId,
        place: String,
        offset: Option<u32>,
    ) -> Result<AgentPlaces> {
        let body = AgentPlacesRequest { place, offset };
        if !body.is_valid() {
            return Err(agents::invalid(types::AGENT_PLACES));
        }
        let env = Envelope::new(types::AGENT_PLACES, &body)?;
        let places: AgentPlaces = agents::request_of(self.shared(), peer, env, agent_places::CAP).await?;
        places
            .sanitized()
            .filter(|p| p.place == body.place)
            .ok_or_else(|| agents::invalid("agent.places reply"))
    }

    /// Phone: makes the folder at `place` an agent folder on PC `peer`, and
    /// answers it as the catalog lists it (the catalog that follows has it
    /// too). While the phone's `agent_folders` toggle is off there, someone
    /// at the PC approves it first ([`NodeEvent::AgentFolderPrompt`] says
    /// so); this waits up to [`agent_places::ADD_WAIT_SECS`]. Sent once.
    /// `Denied` (`declined`) when they said no or didn't answer, or for a
    /// place a phone may not add (a root itself, G7's own directories);
    /// `NotFound`; `Protocol` `LIMIT` at the folder cap.
    pub async fn agent_folder_add(&self, peer: DeviceId, place: String) -> Result<AgentCatalogFolder> {
        let body = AgentFolderAdd { place };
        if !body.is_valid() {
            return Err(agents::invalid(types::AGENT_FOLDER_ADD));
        }
        let shared = self.shared();
        let session = agents::session_offering(shared, &peer, agent_places::CAP)?;
        let env = Envelope::new(types::AGENT_FOLDER_ADD, &body)?;
        let reply = session.request(env, Duration::from_secs(agent_places::ADD_WAIT_SECS)).await?;
        let added = reply.expect_body::<AgentFolderAdded>(types::OK).map_err(agents::reply_error)?;
        if !added.is_valid() {
            return Err(Error::Timeout);
        }
        shown(added.folder).ok_or(Error::Timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_requests_are_this_modules() {
        assert!(is_request(types::AGENT_PLACES) && is_request(types::AGENT_FOLDER_ADD));
        assert!(!is_request(types::AGENT_FOLDER_PROMPT) && !is_request(types::AGENT_FILES));
    }

    #[test]
    fn nothing_prints_a_place() {
        let request = AgentRequest::Places { place: "Home/secret-project".into(), offset: Some(3) };
        let add = AgentRequest::FolderAdd { place: "Home/secret-project".into(), ask: true };
        let shown = format!("{request:?} {add:?}");
        assert!(!shown.contains("secret"), "{shown}");
        assert!(shown.contains("ask: true"), "{shown}");
    }

    #[test]
    fn a_folder_is_cleaned_before_it_goes() {
        let folder = AgentCatalogFolder {
            id: "f0123456789ab".into(),
            label: "a\u{202e}b".into(),
            ..Default::default()
        };
        assert_eq!(shown(folder).unwrap().label, "ab");
        assert!(shown(AgentCatalogFolder { id: "BAD ID".into(), ..Default::default() }).is_none());
    }
}
