// SPDX-License-Identifier: GPL-3.0-or-later
//! The phone beside this PC (phase 5 §V9.1): a phone that shows the open
//! thread's Changes, Plan or Requests beside the PC says so in the thread's
//! header ("Pixel ya Amani shows Changes"), with a menu to switch it. A
//! switch goes through the host (`ui.beside`) to that phone
//! (`agent.beside`).
//!
//! Nothing here sends a message or answers anything: it only changes what
//! the phone shows, and only on a phone that shows this thread beside the PC
//! now.

use g7_agents::{
    api::{self, BesideEvent, Origin, Pane, RemoteView, UiBesideParams, cap},
    client::Client,
};
use g7link_core::{AgentBeside, DeviceId, Node, presence as wire};
use serde_json::{Value, json};

use super::{OpenKind, lock, presence};
use crate::{core_host, i18n::tr};

/// The panes a phone beside the PC shows, in the menu's order.
pub(crate) const PANES: [Pane; 3] = [Pane::Changes, Pane::Plan, Pane::Requests];

// ---- The rules (pure) ----

/// A pane's word on the wire and to the page.
pub(crate) fn word(pane: Pane) -> &'static str {
    match pane {
        Pane::Thread => wire::pane::THREAD,
        Pane::Changes => wire::pane::CHANGES,
        Pane::Plan => wire::pane::PLAN,
        Pane::Requests => wire::pane::REQUESTS,
    }
}

/// The phone showing `thread` beside the PC: the newest view of it in one of
/// [`PANES`]. A phone on the thread itself isn't beside it (its header says
/// "open on").
pub(crate) fn shown<'a>(views: &'a [RemoteView], thread: &str) -> Option<&'a RemoteView> {
    views
        .iter()
        .find(|v| v.thread_id.as_deref() == Some(thread) && v.pane.is_some_and(|p| PANES.contains(&p)))
}

/// The header's line for `thread`: `{ device, name, pane }`, or `{}` while no
/// phone shows it beside the PC.
pub(crate) fn header(views: &[RemoteView], thread: &str) -> Value {
    match shown(views, thread) {
        Some(v) => json!({
            "device": v.device_id,
            "name": v.device_name,
            "pane": v.pane.map(word).unwrap_or_default(),
        }),
        None => json!({}),
    }
}

/// The pane to ask `device` for: `pane` when that phone shows `thread`
/// beside the PC now in another pane, and `pane` is one it can show there.
pub(crate) fn asked(views: &[RemoteView], thread: &str, device: &str, pane: Pane) -> Option<Pane> {
    if !PANES.contains(&pane) {
        return None;
    }
    views
        .iter()
        .find(|v| v.device_id == device && v.thread_id.as_deref() == Some(thread))
        .filter(|v| v.pane.is_some_and(|p| PANES.contains(&p) && p != pane))
        .map(|_| pane)
}

/// The host's `ui.beside` event goes to the phone only while that phone
/// shows the thread (in any pane): a phone that went elsewhere is left
/// alone.
pub(crate) fn still_there(views: &[RemoteView], thread: &str, device: &str) -> bool {
    views.iter().any(|v| v.device_id == device && v.thread_id.as_deref() == Some(thread))
}

// ---- The page's menu ----

/// The header menu's pick: `device` (its id, as the header names it) shows
/// `pane` ("changes", "plan" or "requests") of the open thread. The host
/// hears it (`ui.beside`) and answers with its event, which sends it to the
/// phone ([`from_host`]).
pub fn switch(device: String, pane: String) {
    let Some(pane) = presence::pane(&pane) else { return };
    // Presence's lock is never taken while the state's is held.
    let views = presence::shown();
    let (thread, client) = {
        let s = lock();
        let thread = s.open.as_ref().filter(|o| o.kind == OpenKind::Thread).map(|o| o.id.clone());
        (thread, s.client.clone())
    };
    let Some(thread) = thread else { return };
    let Some(pane) = asked(&views, &thread, &device, pane) else { return };
    let Ok(phone) = device.parse::<DeviceId>() else { return };
    core_host::spawn(async move {
        let params = UiBesideParams { thread_id: thread, device_id: device, pane, by: Origin::launcher() };
        if !ask_host(client, params).await {
            not_reached(&phone);
        }
    });
}

/// `ui.beside`, to a host that has it. False when it wasn't taken.
async fn ask_host(client: Option<Client>, params: UiBesideParams) -> bool {
    let Some(client) = client.filter(|c| c.host().has(cap::BESIDE)) else { return false };
    match client.ui_beside(params).await {
        Ok(()) => true,
        Err(e) => {
            tracing::debug!(error = %e, "the host didn't take the pane");
            false
        }
    }
}

/// The host's `ui.beside` event: the phone shows that pane of the thread
/// (`agent.beside`), when it still shows that thread.
pub(crate) fn from_host(event: BesideEvent) {
    if !api::is_valid_thread_id(&event.thread_id) || !PANES.contains(&event.pane) {
        return;
    }
    let Ok(phone) = event.device_id.parse::<DeviceId>() else { return };
    if !still_there(&presence::shown(), &event.thread_id, &event.device_id) {
        return;
    }
    let Some(node) = core_host::node() else { return };
    core_host::spawn(async move {
        if !tell_phone(&node, phone, event.thread_id, event.pane).await {
            not_reached(&phone);
        }
    });
}

fn not_reached(phone: &DeviceId) {
    crate::bridge::app::show_message(tr!("Agents", "%1 isn't reachable.", super::device_name(phone)));
}

/// `agent.beside` to `phone`. False when it didn't reach it.
async fn tell_phone(node: &Node, phone: DeviceId, thread: String, pane: Pane) -> bool {
    match node.agent_beside(phone, AgentBeside { thread, pane: word(pane).to_owned() }).await {
        Ok(()) => true,
        Err(e) => {
            tracing::debug!(error = %e, "the pane didn't reach the phone");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(c: char) -> String {
        c.to_string().repeat(32)
    }

    fn view(device: &str, thread_id: &str, pane: Option<Pane>) -> RemoteView {
        RemoteView {
            device_id: device.into(),
            device_name: format!("Phone {device}"),
            thread_id: Some(thread_id.into()),
            session_id: None,
            pane,
            dnd: false,
        }
    }

    #[test]
    fn the_header_names_the_phone_showing_this_thread_beside_the_pc() {
        let a = thread('a');
        let views = vec![
            view("1", &a, Some(Pane::Thread)),
            view("2", &thread('b'), Some(Pane::Plan)),
            view("3", &a, Some(Pane::Changes)),
            view("4", &a, Some(Pane::Requests)),
        ];
        assert_eq!(
            header(&views, &a),
            json!({ "device": "3", "name": "Phone 3", "pane": "changes" }),
            "the newest"
        );
        assert_eq!(header(&views, &thread('b')), json!({ "device": "2", "name": "Phone 2", "pane": "plan" }));
        // On the thread itself, or no pane said: "open on" only.
        assert_eq!(header(&views[..1], &a), json!({}));
        assert_eq!(header(&[view("5", &a, None)], &a), json!({}));
        assert_eq!(header(&views, &thread('c')), json!({}));
    }

    #[test]
    fn a_session_is_never_beside() {
        let mut v = view("1", &thread('a'), Some(Pane::Changes));
        v.session_id = v.thread_id.take();
        assert_eq!(header(std::slice::from_ref(&v), &thread('a')), json!({}));
    }

    #[test]
    fn the_menu_switches_a_phone_beside_this_thread_to_another_pane() {
        let a = thread('a');
        let views = vec![view("1", &a, Some(Pane::Changes)), view("2", &a, Some(Pane::Thread))];
        assert_eq!(asked(&views, &a, "1", Pane::Plan), Some(Pane::Plan));
        assert_eq!(asked(&views, &a, "1", Pane::Requests), Some(Pane::Requests));
        assert_eq!(asked(&views, &a, "1", Pane::Changes), None, "it shows that already");
        assert_eq!(asked(&views, &a, "1", Pane::Thread), None, "only the three panes");
        assert_eq!(asked(&views, &a, "2", Pane::Plan), None, "a phone on the thread isn't beside it");
        assert_eq!(asked(&views, &thread('b'), "1", Pane::Plan), None, "another thread");
        assert_eq!(asked(&views, &a, "3", Pane::Plan), None, "a phone that shows nothing");
        assert_eq!(asked(&[], &a, "1", Pane::Plan), None);
    }

    #[test]
    fn the_hosts_event_reaches_a_phone_only_while_it_shows_the_thread() {
        let a = thread('a');
        let views = vec![view("1", &a, Some(Pane::Thread)), view("2", &thread('b'), Some(Pane::Plan))];
        assert!(still_there(&views, &a, "1"), "back on the thread: the pick still goes");
        assert!(!still_there(&views, &a, "2"), "it went to another thread");
        assert!(!still_there(&views, &a, "3"));
        assert!(!still_there(&[], &a, "1"));
    }

    #[test]
    fn pane_words_go_both_ways() {
        for pane in [Pane::Thread, Pane::Changes, Pane::Plan, Pane::Requests] {
            assert_eq!(presence::pane(word(pane)), Some(pane));
        }
    }
}
