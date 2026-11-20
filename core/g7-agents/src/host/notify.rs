// SPDX-License-Identifier: MPL-2.0
//! PC notifications (§7.3): the host posts them, so the launcher and G7 Link
//! never post a second one. One notification per thread, replaced in place.
//! Host strings are English only in v1.

use std::collections::HashMap;
use std::pin::Pin;

use tokio::sync::mpsc;
use zbus::export::futures_core::Stream;
use zbus::zvariant::Value as DbusValue;

use crate::api::{Approval, ApprovalKind, OriginKind, ThreadInfo};

const SERVICE: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const APP_NAME: &str = "G7 Agents";
const ICON: &str = "g7link";
/// A command short enough to allow from the notification.
const MAX_ALLOW_COMMAND: usize = 100;
const MAX_BODY_LINE: usize = 160;

/// Something to show or take down.
#[derive(Debug, Clone)]
pub enum Note {
    Approval {
        thread: ThreadInfo,
        approval: Approval,
    },
    /// The approval was answered elsewhere, expired or cancelled.
    ApprovalGone {
        thread_id: String,
        token: String,
    },
    Done {
        thread: ThreadInfo,
        summary: String,
    },
    Failed {
        thread: ThreadInfo,
        message: String,
    },
    PhoneStarted {
        thread: ThreadInfo,
        /// Made by continuing a session started elsewhere.
        continued: bool,
    },
    Removed {
        thread_id: String,
    },
    /// The agent asks the user something; answered in G7, never from the
    /// notification. Taken down by an `ApprovalGone` with its token.
    Question {
        thread: ThreadInfo,
        token: String,
    },
    /// A plan is ready to review.
    PlanReady {
        thread: ThreadInfo,
    },
}

/// What the user did on a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteAction {
    Answer { token: String, allow: bool },
    Open { thread_id: String },
    Stop { thread_id: String },
}

/// The text and buttons of one notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub summary: String,
    pub body: String,
    /// `(key, label)`; the default action (`"default"`) opens the thread.
    pub actions: Vec<(&'static str, &'static str)>,
    pub critical: bool,
    pub token: Option<String>,
}

fn agent_name(thread: &ThreadInfo) -> &'static str {
    thread.agent.display_name()
}

fn first_line(text: &str, max_chars: usize) -> String {
    let line = text.lines().next().unwrap_or_default();
    if line.chars().count() <= max_chars {
        line.to_owned()
    } else {
        let mut cut: String = line.chars().take(max_chars.saturating_sub(1)).collect();
        cut.push('…');
        cut
    }
}

/// A usage-limit message without its machine part.
fn readable_error(message: &str) -> String {
    match message.split_once("|resets_ms=") {
        Some((text, _)) => format!("{text}."),
        None => message.to_owned(),
    }
}

/// What a note looks like. `None` for notes that only close something.
pub fn render(note: &Note) -> Option<Rendered> {
    match note {
        Note::Approval { thread, approval } => {
            let line = match approval.kind {
                ApprovalKind::Command => {
                    approval.command.as_deref().map(|c| first_line(c, MAX_BODY_LINE)).unwrap_or_default()
                }
                ApprovalKind::Edit => match &approval.path {
                    Some(path) => format!("Edit {path}"),
                    None => approval.title.clone(),
                },
                ApprovalKind::Other => first_line(&approval.title, MAX_BODY_LINE),
            };
            let mut body = line;
            if thread.origin.kind == OriginKind::Phone {
                let device = thread.origin.device_name.as_deref().unwrap_or("your phone");
                body.push_str(&format!("\nStarted from {device}."));
            }
            let one_line_command = approval.kind == ApprovalKind::Command
                && !approval.truncated
                && approval.command.as_deref().is_some_and(|c| {
                    !c.trim().is_empty()
                        && !c.trim_end().contains('\n')
                        && c.chars().count() <= MAX_ALLOW_COMMAND
                });
            let actions = if one_line_command {
                vec![("default", "Open"), ("deny", "Deny"), ("allow", "Allow")]
            } else {
                vec![("default", "Open"), ("deny", "Deny"), ("open", "Open")]
            };
            Some(Rendered {
                summary: format!("{} needs you in {}", agent_name(thread), thread.folder_label),
                body,
                actions,
                critical: true,
                token: Some(approval.token.clone()),
            })
        }
        Note::Done { thread, summary } => Some(Rendered {
            summary: format!("{} finished", agent_name(thread)),
            body: summary.clone(),
            actions: vec![("default", "Open")],
            critical: false,
            token: None,
        }),
        Note::Failed { thread, message } => Some(Rendered {
            summary: format!("{} stopped", agent_name(thread)),
            body: readable_error(message),
            actions: vec![("default", "Open")],
            critical: false,
            token: None,
        }),
        Note::PhoneStarted { thread, continued } => {
            let device = thread.origin.device_name.as_deref().unwrap_or("A phone");
            let body = if *continued {
                format!("continued a session in {}\n{}", thread.folder_label, thread.title)
            } else {
                format!("in {}\n{}", thread.folder_label, thread.title)
            };
            Some(Rendered {
                summary: format!("{device} started a {} thread", agent_name(thread)),
                body,
                actions: vec![("default", "Open"), ("stop", "Stop")],
                critical: false,
                token: None,
            })
        }
        Note::Question { thread, token } => Some(Rendered {
            summary: format!("{} has a question in {}", agent_name(thread), thread.folder_label),
            body: first_line(&thread.title, MAX_BODY_LINE),
            actions: vec![("default", "Open")],
            critical: true,
            token: Some(token.clone()),
        }),
        Note::PlanReady { thread } => Some(Rendered {
            summary: format!("{} has a plan in {}", agent_name(thread), thread.folder_label),
            body: first_line(&thread.title, MAX_BODY_LINE),
            actions: vec![("default", "Open")],
            critical: false,
            token: None,
        }),
        Note::ApprovalGone { .. } | Note::Removed { .. } => None,
    }
}

/// Posts notes on the session bus. Without the service, or with
/// `G7_AGENTS_NO_NOTIFY=1`, nothing is posted.
#[derive(Debug, Clone)]
pub struct Notifier {
    tx: mpsc::UnboundedSender<Note>,
}

impl Notifier {
    pub fn spawn(actions: mpsc::UnboundedSender<NoteAction>) -> Notifier {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(run(rx, actions));
        Notifier { tx }
    }

    pub fn send(&self, note: Note) {
        let _ = self.tx.send(note);
    }

    /// A notifier that posts nothing: its notes land in the receiver.
    #[cfg(test)]
    pub(crate) fn recording() -> (Notifier, mpsc::UnboundedReceiver<Note>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Notifier { tx }, rx)
    }
}

#[derive(Debug)]
struct Shown {
    id: u32,
    token: Option<String>,
}

async fn drain(mut rx: mpsc::UnboundedReceiver<Note>) {
    while rx.recv().await.is_some() {}
}

async fn run(mut rx: mpsc::UnboundedReceiver<Note>, actions: mpsc::UnboundedSender<NoteAction>) {
    let Ok(conn) = zbus::Connection::session().await else { return drain(rx).await };
    let owner = match conn
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetNameOwner",
            &(SERVICE,),
        )
        .await
        .and_then(|reply| reply.body().deserialize::<String>())
    {
        Ok(owner) => owner,
        Err(_) => {
            tracing::debug!("no notification service: notifications are off");
            return drain(rx).await;
        }
    };
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(owner.as_str())
        .and_then(|b| b.interface(SERVICE))
        .and_then(|b| b.path(PATH))
        .map(|b| b.build());
    let Ok(rule) = rule else { return drain(rx).await };
    let Ok(stream) = zbus::MessageStream::for_match_rule(rule, &conn, Some(64)).await else {
        return drain(rx).await;
    };
    let mut stream = Box::pin(stream);
    let mut by_thread: HashMap<String, Shown> = HashMap::new();
    let mut by_id: HashMap<u32, String> = HashMap::new();
    loop {
        tokio::select! {
            note = rx.recv() => {
                let Some(note) = note else { break };
                handle_note(&conn, note, &mut by_thread, &mut by_id).await;
            }
            msg = next_message(&mut stream) => {
                let Some(Ok(msg)) = msg else { break };
                if let Some(action) = signal_action(&msg, &mut by_thread, &mut by_id) {
                    let _ = actions.send(action);
                }
            }
        }
    }
    drain(rx).await;
}

async fn next_message(stream: &mut Pin<Box<zbus::MessageStream>>) -> Option<zbus::Result<zbus::Message>> {
    std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await
}

async fn close(conn: &zbus::Connection, id: u32) {
    tracing::debug!(id, "closing a notification");
    let _ = conn.call_method(Some(SERVICE), PATH, Some(SERVICE), "CloseNotification", &(id,)).await;
}

async fn handle_note(
    conn: &zbus::Connection,
    note: Note,
    by_thread: &mut HashMap<String, Shown>,
    by_id: &mut HashMap<u32, String>,
) {
    let thread_id = match &note {
        Note::Approval { thread, .. }
        | Note::Done { thread, .. }
        | Note::Failed { thread, .. }
        | Note::PhoneStarted { thread, .. }
        | Note::Question { thread, .. }
        | Note::PlanReady { thread } => thread.id.clone(),
        Note::ApprovalGone { thread_id, .. } | Note::Removed { thread_id } => thread_id.clone(),
    };
    let Some(rendered) = render(&note) else {
        let matches = match &note {
            Note::ApprovalGone { token, .. } => {
                by_thread.get(&thread_id).is_some_and(|s| s.token.as_deref() == Some(token.as_str()))
            }
            _ => true,
        };
        if matches && let Some(shown) = by_thread.remove(&thread_id) {
            by_id.remove(&shown.id);
            close(conn, shown.id).await;
        }
        return;
    };
    let replaces = by_thread.get(&thread_id).map_or(0, |s| s.id);
    let actions: Vec<&str> = rendered.actions.iter().flat_map(|(k, l)| [*k, *l]).collect();
    let mut hints: HashMap<&str, DbusValue<'_>> = HashMap::new();
    hints.insert("urgency", DbusValue::U8(if rendered.critical { 2 } else { 1 }));
    hints.insert("desktop-entry", DbusValue::from("g7link"));
    let timeout: i32 = if rendered.critical { 0 } else { -1 };
    let reply = conn
        .call_method(
            Some(SERVICE),
            PATH,
            Some(SERVICE),
            "Notify",
            &(
                APP_NAME,
                replaces,
                ICON,
                rendered.summary.as_str(),
                rendered.body.as_str(),
                actions,
                hints,
                timeout,
            ),
        )
        .await;
    let reply = match reply {
        Ok(reply) => reply,
        Err(e) => {
            tracing::debug!("the notification service refused a notification: {e}");
            return;
        }
    };
    let Ok(id) = reply.body().deserialize::<u32>() else { return };
    tracing::debug!(id, "posted a notification");
    if replaces != 0 && replaces != id {
        by_id.remove(&replaces);
    }
    by_id.insert(id, thread_id.clone());
    by_thread.insert(thread_id, Shown { id, token: rendered.token });
}

fn signal_action(
    msg: &zbus::Message,
    by_thread: &mut HashMap<String, Shown>,
    by_id: &mut HashMap<u32, String>,
) -> Option<NoteAction> {
    let header = msg.header();
    match header.member()?.as_str() {
        "ActionInvoked" => {
            let (id, key) = msg.body().deserialize::<(u32, String)>().ok()?;
            let thread_id = by_id.get(&id)?.clone();
            let token = by_thread.get(&thread_id).and_then(|s| s.token.clone());
            match key.as_str() {
                "allow" => Some(NoteAction::Answer { token: token?, allow: true }),
                "deny" => Some(NoteAction::Answer { token: token?, allow: false }),
                "stop" => Some(NoteAction::Stop { thread_id }),
                "default" | "open" => Some(NoteAction::Open { thread_id }),
                _ => None,
            }
        }
        "NotificationClosed" => {
            let (id, _reason) = msg.body().deserialize::<(u32, u32)>().ok()?;
            if let Some(thread_id) = by_id.remove(&id)
                && by_thread.get(&thread_id).is_some_and(|s| s.id == id)
            {
                by_thread.remove(&thread_id);
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::*;

    fn thread(origin: Origin) -> ThreadInfo {
        ThreadInfo {
            id: "a".repeat(32),
            agent: AgentId::Codex,
            folder_id: "f000000000001".into(),
            folder_label: "g7linux".into(),
            title: "Fix the build".into(),
            state: State::Waiting,
            mode: Mode::Supervised,
            origin,
            created_ms: 0,
            updated_ms: 0,
            last_seq: 0,
            pending: 1,
            summary: None,
            error: None,
            native_id: None,
            step: None,
            plan: None,
            state_since_ms: 0,
            subdir: None,
            continued: false,
            model: None,
            effort: None,
            answered_model: None,
            queued: Vec::new(),
            queue_held: false,
            context: None,
            mcp: None,
            asking: 0,
        }
    }

    fn approval(kind: ApprovalKind, command: Option<&str>, path: Option<&str>) -> Approval {
        Approval {
            token: "b".repeat(32),
            thread_id: "a".repeat(32),
            kind,
            title: "Run command".into(),
            command: command.map(Into::into),
            diff: None,
            path: path.map(Into::into),
            reason: None,
            truncated: false,
            can_allow_thread: true,
            created_ms: 0,
            expires_ms: 0,
            item_id: None,
        }
    }

    #[test]
    fn a_short_command_can_be_allowed_deny_first() {
        let note = Note::Approval {
            thread: thread(Origin::phone("p", "Pixel")),
            approval: approval(ApprovalKind::Command, Some("cargo test"), None),
        };
        let r = render(&note).unwrap();
        assert_eq!(r.summary, "Codex needs you in g7linux");
        assert_eq!(r.body, "cargo test\nStarted from Pixel.");
        assert_eq!(r.actions, [("default", "Open"), ("deny", "Deny"), ("allow", "Allow")]);
        assert!(r.critical);
    }

    #[test]
    fn long_commands_and_edits_must_be_opened() {
        let long = "x".repeat(101);
        for a in [
            approval(ApprovalKind::Command, Some(&long), None),
            approval(ApprovalKind::Command, Some("a\nb"), None),
            approval(ApprovalKind::Edit, None, Some("src/main.rs")),
        ] {
            let r = render(&Note::Approval { thread: thread(Origin::launcher()), approval: a }).unwrap();
            assert!(!r.actions.iter().any(|(k, _)| *k == "allow"));
            assert!(r.actions.iter().any(|(k, _)| *k == "open"));
        }
        let edit = render(&Note::Approval {
            thread: thread(Origin::launcher()),
            approval: approval(ApprovalKind::Edit, None, Some("src/main.rs")),
        })
        .unwrap();
        assert_eq!(edit.body, "Edit src/main.rs");
    }

    #[test]
    fn done_failed_and_phone_started() {
        let r =
            render(&Note::Done { thread: thread(Origin::launcher()), summary: "All good".into() }).unwrap();
        assert_eq!((r.summary.as_str(), r.body.as_str()), ("Codex finished", "All good"));
        let r = render(&Note::Failed {
            thread: thread(Origin::launcher()),
            message: "Usage limit reached|resets_ms=1795050000000".into(),
        })
        .unwrap();
        assert_eq!((r.summary.as_str(), r.body.as_str()), ("Codex stopped", "Usage limit reached."));
        let r = render(&Note::PhoneStarted { thread: thread(Origin::phone("p", "Pixel")), continued: false })
            .unwrap();
        assert_eq!(r.summary, "Pixel started a Codex thread");
        assert_eq!(r.body, "in g7linux\nFix the build");
        assert!(r.actions.contains(&("stop", "Stop")));
        let r = render(&Note::PhoneStarted { thread: thread(Origin::phone("p", "Pixel")), continued: true })
            .unwrap();
        assert_eq!(r.body, "continued a session in g7linux\nFix the build");
    }
}
