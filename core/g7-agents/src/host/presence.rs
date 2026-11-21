// SPDX-License-Identifier: MPL-2.0
//! Presence (phase 5, §V4.2): G7 Link says who is at this PC and what its
//! phones show (`ui.presence`); the host keeps the newest for
//! [`PRESENCE_LIFE_MS`] and tells launchers and G7 Link (the `ui.presence`
//! event, at most every [`PRESENCE_EVENT_MS`]). A thread a phone shows counts
//! as viewed: the PC posts no notification for it and it is read.
//!
//! Presence is used for nothing else. Whatever is unknown, stale or gone
//! reads as nobody there.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::api::*;
use crate::host::Host;

const LIFE: Duration = Duration::from_millis(PRESENCE_LIFE_MS);
const EVENT_GAP: Duration = Duration::from_millis(PRESENCE_EVENT_MS);

/// A `ui.presence` within the limits, its device names cleaned.
pub fn check(mut p: UiPresenceParams) -> Result<UiPresenceParams, RpcError> {
    let bad = |message: &str| RpcError::new(ErrorCode::BadRequest, message);
    if p.remote.len() > MAX_REMOTE_VIEWS {
        return Err(bad("At most 8 phone views."));
    }
    for (n, view) in p.remote.iter().enumerate() {
        if view.device_id.is_empty() || view.device_id.len() > MAX_DEVICE_ID {
            return Err(bad("Bad device id."));
        }
        if view.device_name.len() > MAX_DEVICE_NAME {
            return Err(bad("The device name is too long."));
        }
        if view.thread_id.as_deref().is_some_and(|id| !is_valid_thread_id(id))
            || view.session_id.as_deref().is_some_and(|id| !is_valid_session_id(id))
        {
            return Err(bad("Bad thread or session id."));
        }
        if p.remote[..n].iter().any(|v| v.device_id == view.device_id) {
            return Err(bad("One view per device."));
        }
    }
    for view in &mut p.remote {
        view.device_name = clean(&view.device_name);
    }
    Ok(p)
}

/// The newest `ui.presence` and the Link connection it came on.
#[derive(Debug)]
struct Reported {
    client: u64,
    at: Instant,
    value: UiPresenceParams,
}

/// When the `ui.presence` event goes out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Send {
    /// Nothing changed since clients last heard, or a send is scheduled.
    No,
    Now,
    /// At the end of the gap since the last one.
    After(Duration),
}

/// What G7 Link last said, and what presence clients last heard.
#[derive(Debug, Default)]
pub struct Presence {
    last: Option<Reported>,
    heard: Option<(Instant, PresenceEvent)>,
    /// A send is scheduled for the end of the gap.
    scheduled: bool,
}

impl Presence {
    /// Keeps `value` (checked) from Link connection `client`.
    pub fn set(&mut self, client: u64, value: UiPresenceParams, now: Instant) {
        self.last = Some(Reported { client, at: now, value });
    }

    /// The newest `ui.presence` while it is fresh.
    pub fn fresh(&self, now: Instant) -> Option<&UiPresenceParams> {
        self.last.as_ref().filter(|r| now.saturating_duration_since(r.at) < LIFE).map(|r| &r.value)
    }

    /// A phone shows `thread_id` now.
    pub fn views(&self, thread_id: &str, now: Instant) -> bool {
        self.fresh(now).is_some_and(|p| p.remote.iter().any(|v| v.thread_id.as_deref() == Some(thread_id)))
    }

    /// Connection `client` closed: what it said goes. `true` when it had said something.
    pub fn closed(&mut self, client: u64) -> bool {
        if self.last.as_ref().is_some_and(|r| r.client == client) {
            self.last = None;
            return true;
        }
        false
    }

    /// Drops a `ui.presence` not refreshed in time. `true` when one went.
    pub fn expire(&mut self, now: Instant) -> bool {
        if self.last.is_some() && self.fresh(now).is_none() {
            self.last = None;
            return true;
        }
        false
    }

    /// The event for what launchers and G7 Link show (newest first) and the fresh presence.
    pub fn event(&self, viewing: Vec<String>, viewing_sessions: Vec<String>, now: Instant) -> PresenceEvent {
        let fresh = self.fresh(now);
        PresenceEvent {
            at_pc: fresh.is_some_and(|p| p.at_pc),
            locked: fresh.is_some_and(|p| p.locked),
            dnd: fresh.is_some_and(|p| p.dnd),
            viewing,
            viewing_sessions,
            remote: fresh.map(|p| p.remote.clone()).unwrap_or_default(),
        }
    }

    /// Whether `event` goes out now: at most one per [`PRESENCE_EVENT_MS`],
    /// and only when it differs from what clients last heard.
    pub fn offer(&mut self, event: &PresenceEvent, now: Instant) -> Send {
        match &self.heard {
            Some((_, heard)) if heard == event => Send::No,
            Some((at, _)) if now.saturating_duration_since(*at) < EVENT_GAP => {
                if self.scheduled {
                    Send::No
                } else {
                    self.scheduled = true;
                    Send::After(EVENT_GAP - now.saturating_duration_since(*at))
                }
            }
            _ => {
                self.heard = Some((now, event.clone()));
                Send::Now
            }
        }
    }

    /// The scheduled send is due with `event`, the state now: it goes when it differs.
    pub fn due(&mut self, event: PresenceEvent, now: Instant) -> Option<PresenceEvent> {
        self.scheduled = false;
        if self.heard.as_ref().is_some_and(|(_, heard)| *heard == event) {
            return None;
        }
        self.heard = Some((now, event.clone()));
        Some(event)
    }
}

impl Host {
    /// `ui.presence`: from G7 Link only. The threads its phones show are read.
    pub(crate) fn ui_presence(
        self: &Arc<Self>,
        from_link: bool,
        client: u64,
        params: Value,
    ) -> Result<Value, RpcError> {
        if !from_link {
            return Err(RpcError::new(ErrorCode::Denied, "Only G7 Link says who is at this PC."));
        }
        let params = serde_json::from_value::<UiPresenceParams>(params)
            .map_err(|e| RpcError::new(ErrorCode::BadRequest, e.to_string()))?;
        let params = check(params)?;
        let shown: Vec<String> = params.remote.iter().filter_map(|v| v.thread_id.clone()).collect();
        self.hub.lock_presence().set(client, params, Instant::now());
        for thread_id in &shown {
            self.thread_viewed(thread_id);
        }
        self.presence_changed();
        Ok(json!({}))
    }

    /// Drops a `ui.presence` not refreshed in time (the host's tick).
    pub(crate) fn expire_presence(self: &Arc<Self>) {
        let expired = self.hub.lock_presence().expire(Instant::now());
        if expired {
            self.presence_changed();
        }
    }

    /// Something presence clients hear of may have changed: the event goes
    /// now, or at the end of the gap with the state then.
    pub(crate) fn presence_changed(self: &Arc<Self>) {
        let now = Instant::now();
        let event = self.hub.presence_event(now);
        let send = self.hub.lock_presence().offer(&event, now);
        match send {
            Send::No => {}
            Send::Now => self.hub.send_presence(&event, None),
            Send::After(wait) => {
                let host = Arc::downgrade(self);
                tokio::spawn(async move {
                    tokio::time::sleep(wait).await;
                    let Some(host) = host.upgrade() else { return };
                    let now = Instant::now();
                    let event = host.hub.presence_event(now);
                    let due = host.hub.lock_presence().due(event, now);
                    if let Some(event) = due {
                        host.hub.send_presence(&event, None);
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(device: &str, thread: Option<&str>) -> RemoteView {
        RemoteView {
            device_id: device.into(),
            device_name: format!("Pixel {device}"),
            thread_id: thread.map(str::to_owned),
            session_id: None,
            pane: Some(Pane::Thread),
            dnd: false,
        }
    }

    fn at_pc(remote: Vec<RemoteView>) -> UiPresenceParams {
        UiPresenceParams { at_pc: true, locked: false, dnd: false, remote }
    }

    #[test]
    fn a_presence_is_checked() {
        let t = "a".repeat(32);
        assert!(check(at_pc((0..8).map(|n| view(&n.to_string(), Some(&t))).collect())).is_ok());
        let too_many = at_pc((0..9).map(|n| view(&n.to_string(), None)).collect());
        assert_eq!(check(too_many).unwrap_err().code, ErrorCode::BadRequest);
        let twice = at_pc(vec![view("p", None), view("p", Some(&t))]);
        assert_eq!(check(twice).unwrap_err().code, ErrorCode::BadRequest, "one view per device");
        for bad in [
            RemoteView { device_id: String::new(), ..view("p", None) },
            RemoteView { device_id: "d".repeat(MAX_DEVICE_ID + 1), ..view("p", None) },
            RemoteView { device_name: "n".repeat(MAX_DEVICE_NAME + 1), ..view("p", None) },
            view("p", Some("../x")),
            RemoteView { session_id: Some(t.clone()), ..view("p", None) },
        ] {
            assert_eq!(check(at_pc(vec![bad])).unwrap_err().code, ErrorCode::BadRequest);
        }
        let named = RemoteView { device_name: "Pixel\u{1b}[31m 8".into(), ..view("p", None) };
        assert_eq!(check(at_pc(vec![named])).unwrap().remote[0].device_name, "Pixel[31m 8");
    }

    #[test]
    fn a_presence_lives_a_minute_and_goes_with_its_connection() {
        let t = "a".repeat(32);
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut presence = Presence::default();
        assert!(!presence.views(&t, t0), "nothing said: nobody there");
        presence.set(7, at_pc(vec![view("p", Some(&t))]), t0);
        assert!(presence.views(&t, at(59)));
        assert!(!presence.views(&"b".repeat(32), at(1)));
        assert!(!presence.views(&t, at(61)), "stale after 60 s");
        assert_eq!(presence.event(Vec::new(), Vec::new(), at(61)), PresenceEvent::default());
        assert!(!presence.expire(at(30)));
        assert!(presence.expire(at(61)));
        assert!(!presence.expire(at(62)), "gone once");

        presence.set(7, at_pc(vec![view("p", Some(&t))]), at(70));
        assert!(!presence.closed(8), "another connection closing changes nothing");
        assert!(presence.views(&t, at(71)));
        assert!(presence.closed(7));
        assert!(!presence.views(&t, at(71)));
        assert!(!presence.event(vec![t.clone()], Vec::new(), at(71)).at_pc);
    }

    #[test]
    fn the_event_goes_at_most_twice_a_second_with_the_newest_state() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut presence = Presence::default();
        let one = PresenceEvent { at_pc: true, ..PresenceEvent::default() };
        let two = PresenceEvent { viewing: vec!["a".repeat(32)], ..one.clone() };
        assert_eq!(presence.offer(&one, ms(0)), Send::Now);
        assert_eq!(presence.offer(&one, ms(10)), Send::No, "nothing changed");
        assert_eq!(presence.offer(&two, ms(100)), Send::After(Duration::from_millis(400)));
        assert_eq!(presence.offer(&PresenceEvent::default(), ms(200)), Send::No, "one is scheduled");
        assert_eq!(presence.due(two.clone(), ms(500)), Some(two.clone()), "the state at the end");
        assert_eq!(presence.offer(&one, ms(1100)), Send::Now);
        assert_eq!(presence.offer(&two, ms(1200)), Send::After(Duration::from_millis(400)));
        assert_eq!(presence.due(one, ms(1600)), None, "back to what clients heard");
        assert_eq!(presence.offer(&two, ms(1700)), Send::Now);
    }
}
