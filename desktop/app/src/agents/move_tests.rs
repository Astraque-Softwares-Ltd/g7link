// SPDX-License-Identifier: GPL-3.0-or-later
//! Move to G7 on the fake PC (`plans/move-and-focus.md` §11, by a signal as
//! the owner chose in §2): the reply tests' world also plays the host's
//! `session.close` and `session.continue`, and records every call. Nothing
//! real is signalled, closed, continued or written to.

use g7_agents::api::{CloseOutcome, CloseRefusal, SessionCloseResult};
use g7link_core::{AgentThread, session_move};

use super::*;
use crate::agents::reply::move_here::{
    Closing, Continued, MoveAsk, MoveOutcome, Moved, Mover, answer, closing, wire,
};

impl Mover for World {
    fn closes(&self) -> bool {
        self.host_moves.get()
    }

    fn close(&self, _session: &str, pid: u32, start_ticks: u64) -> Result<Closing, HostDown> {
        self.note(format!("close {pid} {start_ticks}"));
        next(&self.closes)
    }

    fn continue_here(&self, _session: &str) -> Result<Continued, HostDown> {
        self.note("continue");
        next(&self.continues)
    }
}

fn thread() -> AgentThread {
    AgentThread {
        id: "th0123".into(),
        agent: "claude".into(),
        folder: "g7linux".into(),
        title: "Fix the build".into(),
        state: "done".into(),
        mode: "supervised".into(),
        from_phone: true,
        updated: 1_700_000_000_000,
        ..Default::default()
    }
}

fn move_ask(confirm: Option<String>) -> MoveAsk {
    MoveAsk { session: SESSION.into(), confirm }
}

/// An idle Claude, by its status file, that exits when the host closes it
/// and is continued as a thread.
fn moving() -> World {
    let world = World::new();
    world.places(vec![Ok(Found::Place(place())), Ok(Found::Ended)]);
    *world.continues.borrow_mut() = VecDeque::from([Ok(Continued::Thread(Box::new(thread())))]);
    world
}

fn move_as(world: &World, replies: &Replies, phone: &str, confirm: Option<String>) -> Result<Moved, Error> {
    let moved = answer(world, world, replies, phone, &move_ask(confirm));
    continued_only_once_gone(world);
    nothing_on_screen(world);
    moved
}

fn move_with(world: &World, replies: &Replies) -> Result<Moved, Error> {
    move_as(world, replies, PHONE, None)
}

/// Moved by a phone that already confirmed replies into this run.
fn moved(world: &World) -> Moved {
    move_with(world, &confirmed()).expect("an answer")
}

fn outcome_of(world: &World) -> MoveOutcome {
    moved(world).outcome
}

/// `continue` is asked only after the close, and only once a `place` since
/// the close said nothing runs the session.
fn continued_only_once_gone(world: &World) {
    let log = world.log();
    let Some(at) = log.iter().position(|e| e == "continue") else { return };
    assert_eq!(world.count("continue"), 1, "asked once");
    let closed = log.iter().position(|e| e.starts_with("close")).expect("closed first");
    assert!(closed < at, "{log:?}");
    assert!(log[closed..at].contains(&"place".to_owned()), "placed again after the close: {log:?}");
    let places = log[..at].iter().filter(|e| *e == "place").count();
    let last = world.found.borrow()[places - 1];
    assert!(matches!(last, "ended" | "elsewhere"), "the last place said {last}");
}

/// A move brings nothing forward, titles nothing, reads no screen, touches
/// no selection and presses no key.
fn nothing_on_screen(world: &World) {
    let shown = world.events(&[
        "activate", "minimize", "mark", "unmark", "tabs", "select", "back", "read", "primary", "clear",
        "paste", "enter",
    ]);
    assert!(shown.is_empty(), "{:?}", world.log());
}

fn closes(world: &World) -> usize {
    world.log().iter().filter(|e| e.starts_with("close")).count()
}

// ---- The way through ----

#[test]
fn an_idle_claude_is_closed_by_the_host_then_continued_as_a_thread() {
    let world = moving();
    let moved = moved(&world);
    assert_eq!(moved.outcome, MoveOutcome::Moved(Box::new(thread())));
    assert_eq!(
        world.log(),
        ["place", "close 300 3000", "reset 34816", "place", "continue"],
        "the process placed is the one closed; its terminal reset once it's gone; placed again; continued"
    );
    let wired = wire(moved);
    assert!(wired.answers(), "{wired:?}");
    assert_eq!(wired.result, session_move::result::MOVED);
    assert_eq!(wired.thread.map(|t| t.id), Some("th0123".into()));
}

#[test]
fn a_move_works_while_the_pc_is_locked_or_in_use() {
    let world = moving();
    world.locked_is(vec![Some(true)]);
    world.free.set(Some(Duration::from_secs(2)));
    world.typing.set(true);
    // GNOME turns the extension off on the lock screen.
    *world.wins.borrow_mut() = None;
    world.windows_ready.set(false);
    world.keys_ready.set(false);
    assert!(matches!(outcome_of(&world), MoveOutcome::Moved(_)));

    // Behind a fullscreen window, or with a tmux in between: nothing is on screen anyway.
    let world = moving();
    if let Some(wins) = world.wins.borrow_mut().as_mut() {
        wins[0].fullscreen = true;
    }
    let mut p = place();
    p.chain.insert(
        1,
        Ancestor { pid: 180, start_ticks: 1, comm: "tmux: server".into(), exe_name: "tmux".into() },
    );
    world.places(vec![Ok(Found::Place(p)), Ok(Found::Ended)]);
    assert!(matches!(outcome_of(&world), MoveOutcome::Moved(_)));
}

#[test]
fn the_terminal_is_reset_only_after_its_agent_closed() {
    // No terminal to reset: still moved.
    let world = moving();
    world.places(vec![Ok(Found::Place(Place { tty: None, ..place() })), Ok(Found::Ended)]);
    assert!(matches!(outcome_of(&world), MoveOutcome::Moved(_)));
    assert!(world.events(&["reset"]).is_empty());
    // Still running, or nothing sent: nothing written.
    for closing in [Closing::Open, Closing::Refused(Refused::Busy)] {
        let world = moving();
        *world.closes.borrow_mut() = VecDeque::from([Ok(closing)]);
        moved(&world);
        assert!(world.events(&["reset"]).is_empty(), "{closing:?}");
    }
    let world = moving();
    *world.closes.borrow_mut() = VecDeque::from([Err(HostDown)]);
    assert_eq!(move_with(&world, &confirmed()), Err(Error::Internal));
    assert!(world.events(&["reset"]).is_empty());
}

// ---- What may be moved (§4.1) ----

#[test]
fn only_an_idle_session_is_closed() {
    let cases = [
        (Agent::Claude, Now::Working, true, Refused::Busy),
        (Agent::Claude, Now::Working, false, Refused::Busy),
        (Agent::Codex, Now::Working, false, Refused::Busy),
        (Agent::Claude, Now::NeedsYou, true, Refused::NeedsYou),
        (Agent::Claude, Now::MayNeedYou, false, Refused::NeedsYou),
        (Agent::Codex, Now::MayNeedYou, false, Refused::NeedsYou),
        (Agent::Claude, Now::Ended, true, Refused::Ended),
        // Claude idle by a guess from its file: it may be mid-turn.
        (Agent::Claude, Now::Idle, false, Refused::Place),
    ];
    for (agent, state, exact, reason) in cases {
        let world = moving();
        world.places(vec![Ok(Found::Place(Place { agent, state, state_exact: exact, ..place() }))]);
        let moved = move_with(&world, &Replies::default()).expect("an answer");
        assert_eq!(moved.outcome, MoveOutcome::Refused(reason), "{agent:?} {state:?} {exact}");
        assert_eq!(moved.nonce, None, "refused before the phone's user is asked");
        assert_eq!(world.log(), ["place"], "{agent:?} {state:?}");
    }
    // Codex says nothing of its state: its idle is a guess, and enough.
    let world = moving();
    let codex = Place { agent: Agent::Codex, state_exact: false, ..place() };
    world.places(vec![Ok(Found::Place(codex)), Ok(Found::Elsewhere)]);
    assert!(matches!(outcome_of(&world), MoveOutcome::Moved(_)));
}

#[test]
fn what_would_stop_the_continue_is_refused_before_anything_is_asked() {
    for (block, reason) in [
        (Blocked::Folder, Refused::Folder),
        (Blocked::Agent, Refused::Agent),
        (Blocked::Open, Refused::Place),
    ] {
        let world = moving();
        world.places(vec![Ok(Found::Place(Place { continue_block: Some(block), ..place() }))]);
        let moved = move_with(&world, &Replies::default()).expect("an answer");
        assert_eq!(moved.outcome, MoveOutcome::Refused(reason), "{block:?}");
        assert_eq!(moved.nonce, None, "{block:?}");
        assert_eq!(world.log(), ["place"], "{block:?}: nothing closed");
    }
}

#[test]
fn what_doesnt_run_in_a_terminal_is_refused() {
    for source in [Source::DesktopApp, Source::Other] {
        let world = moving();
        world.places(vec![Ok(Found::Place(Place { source, ..place() }))]);
        assert_eq!(outcome_of(&world), MoveOutcome::Refused(Refused::Place), "{source:?}");
        assert_eq!(closes(&world), 0);
    }
    for (found, reason) in [
        (Found::Ended, Refused::Ended),
        (Found::Ambiguous, Refused::Ambiguous),
        (Found::Elsewhere, Refused::Place),
    ] {
        let world = moving();
        world.places(vec![Ok(found.clone())]);
        assert_eq!(outcome_of(&world), MoveOutcome::Refused(reason), "{found:?}");
        assert_eq!(closes(&world), 0);
    }
    let world = moving();
    world.places(vec![Ok(Found::Unknown)]);
    assert_eq!(move_with(&world, &confirmed()), Err(Error::NotFound));
    let world = moving();
    world.places(vec![Err(HostDown)]);
    assert_eq!(move_with(&world, &confirmed()), Err(Error::Internal));
    assert_eq!(closes(&world), 0);
}

#[test]
fn a_host_that_cant_close_sessions_is_setup() {
    let world = moving();
    world.host_moves.set(false);
    assert_eq!(outcome_of(&world), MoveOutcome::Refused(Refused::Setup));
    assert!(world.log().is_empty(), "nothing asked of the host");
}

// ---- The close and the continue (§4.4) ----

#[test]
fn an_agent_still_running_after_its_signal_is_failed_open() {
    let world = moving();
    *world.closes.borrow_mut() = VecDeque::from([Ok(Closing::Open)]);
    let moved = moved(&world);
    assert_eq!(moved.outcome, MoveOutcome::Open);
    assert_eq!(world.log(), ["place", "close 300 3000"], "nothing more: no second close, no continue");
    let wired = wire(moved);
    assert!(wired.answers(), "{wired:?}");
    assert_eq!(
        (wired.result.as_str(), wired.reason.as_deref()),
        (session_move::result::FAILED, Some(session_move::reason::OPEN))
    );
}

#[test]
fn another_process_on_the_session_after_the_close_is_failed_open() {
    for after in [Found::Place(Place { pid: 301, start_ticks: 3010, ..place() }), Found::Ambiguous] {
        let world = moving();
        world.places(vec![Ok(Found::Place(place())), Ok(after.clone())]);
        assert_eq!(outcome_of(&world), MoveOutcome::Open, "{after:?}");
        assert_eq!(world.count("continue"), 0, "{after:?}");
    }
}

#[test]
fn a_codex_the_host_still_sees_open_is_closed_for_the_phone_to_continue() {
    for (continued, why) in
        [(Continued::StillOpen, "BUSY within its 90 s"), (Continued::Refused, "any other refusal")]
    {
        let world = moving();
        let codex = Place { agent: Agent::Codex, state_exact: false, ..place() };
        // Gone, but the host says `place` while Codex may still write it.
        world.places(vec![Ok(Found::Place(codex)), Ok(Found::Elsewhere)]);
        *world.continues.borrow_mut() = VecDeque::from([Ok(continued)]);
        let moved = moved(&world);
        assert_eq!(moved.outcome, MoveOutcome::Closed, "{why}");
        assert_eq!(world.log(), ["place", "close 300 3000", "reset 34816", "place", "continue"], "{why}");
        let wired = wire(moved);
        assert!(wired.answers(), "{why}: {wired:?}");
        assert_eq!(wired.result, session_move::result::CLOSED);
    }
}

#[test]
fn a_host_that_stops_answering_is_not_known() {
    // At the close: whether it was sent isn't known.
    let world = moving();
    *world.closes.borrow_mut() = VecDeque::from([Err(HostDown)]);
    assert_eq!(move_with(&world, &confirmed()), Err(Error::Internal));
    assert_eq!(world.count("continue"), 0);
    // Placing it again, or a session the host no longer lists.
    for after in [Err(HostDown), Ok(Found::Unknown)] {
        let world = moving();
        world.places(vec![Ok(Found::Place(place())), after.clone()]);
        assert_eq!(move_with(&world, &confirmed()), Err(Error::Internal), "{after:?}");
        assert_eq!(world.count("continue"), 0, "{after:?}");
    }
    // At the continue: whether the thread was made isn't known.
    let world = moving();
    *world.continues.borrow_mut() = VecDeque::from([Err(HostDown)]);
    assert_eq!(move_with(&world, &confirmed()), Err(Error::Internal));
}

#[test]
fn the_hosts_refusals_keep_their_words() {
    for reason in [
        Refused::Ended,
        Refused::Ambiguous,
        Refused::Place,
        Refused::NoProcess,
        Refused::Busy,
        Refused::NeedsYou,
        Refused::Folder,
        Refused::Agent,
        Refused::Setup,
    ] {
        let world = moving();
        *world.closes.borrow_mut() = VecDeque::from([Ok(Closing::Refused(reason))]);
        assert_eq!(outcome_of(&world), MoveOutcome::Refused(reason), "{reason:?}");
        assert_eq!(world.log(), ["place", "close 300 3000"], "{reason:?}");
    }
    let world = moving();
    *world.closes.borrow_mut() = VecDeque::from([Ok(Closing::Unknown)]);
    assert_eq!(move_with(&world, &confirmed()), Err(Error::NotFound));
}

#[test]
fn session_close_answers_are_read_as_the_move_words() {
    let answer = |result, reason| closing(SessionCloseResult { result, reason });
    assert_eq!(answer(CloseOutcome::Closed, None), Ok(Closing::Closed));
    assert_eq!(answer(CloseOutcome::Open, None), Ok(Closing::Open));
    for (reason, word) in [
        (Some(CloseRefusal::Ended), Refused::Ended),
        (Some(CloseRefusal::Ambiguous), Refused::Ambiguous),
        (Some(CloseRefusal::Place), Refused::Place),
        (Some(CloseRefusal::NoProcess), Refused::NoProcess),
        (Some(CloseRefusal::Busy), Refused::Busy),
        (Some(CloseRefusal::NeedsYou), Refused::NeedsYou),
        (Some(CloseRefusal::Folder), Refused::Folder),
        (Some(CloseRefusal::Agent), Refused::Agent),
        (Some(CloseRefusal::Unknown), Refused::Place),
        (None, Refused::Place),
    ] {
        assert_eq!(answer(CloseOutcome::Refused, reason), Ok(Closing::Refused(word)), "{reason:?}");
    }
    // A word for what it did that this build doesn't know: whether it closed isn't known.
    assert_eq!(answer(CloseOutcome::Unknown, None), Err(HostDown));
}

// ---- The confirmation, the pace and the lock (§4.3) ----

#[test]
fn a_phone_that_confirmed_replies_into_this_run_isnt_asked_again() {
    let world = moving();
    assert!(matches!(outcome_of(&world), MoveOutcome::Moved(_)));
    // Confirmed in another window of the same terminal, or with none listed now.
    let world = moving();
    *world.wins.borrow_mut() = None;
    assert!(matches!(outcome_of(&world), MoveOutcome::Moved(_)));
    // Another run of the agent (`claude --resume`) is asked about.
    let world = moving();
    world.places(vec![Ok(Found::Place(Place { pid: 301, start_ticks: 3010, ..place() }))]);
    let moved = moved(&world);
    assert_eq!(moved.outcome, MoveOutcome::Refused(Refused::Confirm));
    assert_eq!(closes(&world), 0);
}

#[test]
fn a_new_phone_is_asked_first_and_its_nonce_works_once() {
    let world = moving();
    let replies = Replies::default();
    let first = move_with(&world, &replies).expect("an answer");
    assert_eq!(first.outcome, MoveOutcome::Refused(Refused::Confirm));
    assert_eq!(
        first.target,
        Some(Target {
            app: "Terminal".into(),
            agent: "Claude".into(),
            folder: "g7linux".into(),
            kind: "terminal"
        })
    );
    let nonce = first.nonce.clone().expect("a nonce");
    assert_eq!(nonce.len(), 32);
    assert_eq!(world.log(), ["place"], "nothing closed before the phone's user says so");
    let wired = wire(first);
    assert!(wired.answers(), "{wired:?}");

    // The agent restarted in between: the nonce is spent, and the phone asked again.
    let restarted = || Place { pid: 301, start_ticks: 3010, ..place() };
    let world = moving();
    world.places(vec![Ok(Found::Place(restarted())), Ok(Found::Ended)]);
    let other = move_as(&world, &replies, PHONE, Some(nonce.clone())).expect("an answer");
    assert_eq!(other.outcome, MoveOutcome::Refused(Refused::Confirm));
    let fresh = other.nonce.expect("a nonce");
    assert_ne!(fresh, nonce);
    assert!(!replies.ledger().holds(PHONE, SESSION, &nonce, Instant::now()), "spent");
    assert_eq!(closes(&world), 0);

    // Sent back at once with the new one: closed and continued.
    let world = moving();
    world.places(vec![Ok(Found::Place(restarted())), Ok(Found::Ended)]);
    let moved = move_as(&world, &replies, PHONE, Some(fresh.clone())).expect("an answer");
    assert!(matches!(moved.outcome, MoveOutcome::Moved(_)));
    assert_eq!(world.events(&["close"]), ["close 301 3010"]);
    assert!(!replies.ledger().holds(PHONE, SESSION, &fresh, Instant::now()), "once");
}

#[test]
fn a_nonce_is_this_phones_alone() {
    let world = moving();
    let replies = Replies::default();
    let nonce = move_with(&world, &replies).expect("an answer").nonce.expect("a nonce");
    let world = moving();
    let other = move_as(&world, &replies, "phone-b", Some(nonce)).expect("an answer");
    assert_eq!(other.outcome, MoveOutcome::Refused(Refused::Confirm));
    assert_eq!(closes(&world), 0);
}

#[test]
fn asked_while_locked_it_names_the_terminal_by_its_program() {
    let world = moving();
    *world.wins.borrow_mut() = None;
    let replies = Replies::default();
    let first = move_with(&world, &replies).expect("an answer");
    assert_eq!(first.target.map(|t| t.app), Some("Ptyxis".into()));
    // Confirmed once the PC is unlocked again: the same run of the agent.
    let world = moving();
    let moved = move_as(&world, &replies, PHONE, first.nonce).expect("an answer");
    assert!(matches!(moved.outcome, MoveOutcome::Moved(_)));

    // A terminal G7 doesn't know: a plain name.
    let world = moving();
    *world.wins.borrow_mut() = None;
    let mut p = place();
    p.chain[2] = ancestor(EMULATOR, "kitty");
    world.places(vec![Ok(Found::Place(p))]);
    let first = move_with(&world, &Replies::default()).expect("an answer");
    assert_eq!(first.target.map(|t| (t.app, t.kind)), Some(("Terminal".into(), "terminal")));
}

#[test]
fn moves_are_paced_as_replies() {
    let replies = confirmed();
    let world = moving();
    assert!(matches!(move_with(&world, &replies).map(|m| m.outcome), Ok(MoveOutcome::Moved(_))));
    let world = moving();
    let second = move_with(&world, &replies).expect("an answer");
    assert_eq!(second.outcome, MoveOutcome::Refused(Refused::Rate));
    assert!(second.retry_ms.is_some_and(|ms| (1..=3000).contains(&ms)));
    assert!(world.log().is_empty());
    let wired = wire(second);
    assert!(wired.answers() && wired.retry_ms.is_some(), "{wired:?}");

    // A move counts as one entered reply for the session.
    let entered = replies.ledger().phones.iter().map(|(_, pace)| pace.entered.len()).sum::<usize>();
    assert_eq!(entered, 1);
}

#[test]
fn one_reply_or_move_at_a_time_on_the_pc() {
    let world = moving();
    let replies = confirmed();
    let _entering = replies.entering.lock().expect("the lock");
    assert_eq!(move_with(&world, &replies), Err(Error::Busy));
    assert!(world.log().is_empty());
}

// ---- What the phone and the log see ----

#[test]
fn every_answer_is_a_wire_word() {
    let refusals = [
        Refused::Confirm,
        Refused::NeedsYou,
        Refused::Busy,
        Refused::Ended,
        Refused::NoProcess,
        Refused::Ambiguous,
        Refused::Place,
        Refused::Rate,
        Refused::Setup,
        Refused::Folder,
        Refused::Agent,
    ];
    for reason in refusals {
        assert!(session_move::reason::REFUSED.contains(&reason.word()), "{reason:?}");
    }
    assert!(session_move::reason::FAILED.contains(&MoveOutcome::Open.reason().expect("a reason")));
    for outcome in [
        MoveOutcome::Moved(Box::new(thread())),
        MoveOutcome::Closed,
        MoveOutcome::Refused(Refused::Busy),
        MoveOutcome::Open,
    ] {
        assert!(session_move::result::ALL.contains(&outcome.result()), "{outcome:?}");
    }
}

#[test]
fn no_nonce_or_title_reaches_a_debug() {
    let nonce = "fedcba9876543210fedcba9876543210";
    let shown = format!("{:?}", move_ask(Some(nonce.into())));
    assert!(!shown.contains("fedcba") && shown.contains("confirm: true"), "{shown}");

    let world = moving();
    let asked = move_with(&world, &Replies::default()).expect("an answer");
    let minted = asked.nonce.clone().expect("a nonce");
    let shown = format!("{asked:?}");
    assert!(!shown.contains(&minted), "{shown}");

    let world = moving();
    let shown = format!("{:?}", moved(&world));
    assert!(!shown.contains("Fix the build"), "{shown}");
}
