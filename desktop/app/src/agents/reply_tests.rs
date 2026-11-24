// SPDX-License-Identifier: GPL-3.0-or-later
//! The reply sequence on a fake PC: one world plays the host, the windows
//! and their tabs, the terminal's title, the keyboard and the desk, and
//! records every call, so each test says which keys were pressed and which
//! titles were written and put back. No real window, pty, session,
//! accessibility bus, clipboard or keyboard. A move to G7 runs on the same
//! world (`move_tests.rs`), which also plays the host's `session.close` and
//! `session.continue`.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::*;

const PHONE: &str = "phone-a";
const SESSION: &str = "sc0123";
const MARK: &str = "Thequickbrownfoxjumpsoverthelazydog";
/// Ptyxis (pid 100) with one window (7); the agent (pid 300) under bash
/// (200) and ptyxis-agent (150), on pts/0; the owner's file manager (pid 50,
/// window 1) in front.
const EMULATOR: u32 = 100;
const WINDOW: u64 = 7;
/// The session's terminal (`/dev/pts/0`), and other tabs' (pts/1, pts/2).
const TTY: u32 = 34816;
const OTHER_TTY: u32 = 34817;
const THIRD_TTY: u32 = 34818;
/// The session's tab as Ptyxis titles it.
const CLAUDE_TITLE: &str = "✳ Claude Code — claude ";

type Hook = Box<dyn Fn(&World)>;

/// A terminal tab: its pty, its title, and the titles pushed under it.
#[derive(Debug, Clone)]
struct Tab {
    tty: u32,
    title: String,
    pushed: Vec<String>,
}

struct World {
    log: RefCell<Vec<String>>,
    places: RefCell<VecDeque<Result<Found, HostDown>>>,
    /// What each `place` answered, in order: `place`, `ended`, `elsewhere`,
    /// `ambiguous`, `unknown` or `down`.
    found: RefCell<Vec<&'static str>>,
    /// The host closes sessions (`cap::MOVE`).
    host_moves: Cell<bool>,
    closes: RefCell<VecDeque<Result<move_here::Closing, HostDown>>>,
    continues: RefCell<VecDeque<Result<move_here::Continued, HostDown>>>,
    windows_ready: Cell<bool>,
    wins: RefCell<Option<Vec<Win>>>,
    /// Per window: its tabs and the selected one; the window's title is
    /// the selected tab's.
    tabs: RefCell<Vec<(u64, Vec<Tab>, usize)>>,
    activate_ok: Cell<bool>,
    activate_focuses: Cell<bool>,
    /// Windows that don't take the focus when activated.
    stuck: RefCell<Vec<u64>>,
    screen: RefCell<Option<String>>,
    primary: RefCell<Option<String>>,
    primary_ok: Cell<bool>,
    app: RefCell<Option<String>>,
    /// The pty can be written to.
    title_ok: Cell<bool>,
    /// The terminal shows a title written to it (false: a fixed title).
    title_shows: Cell<bool>,
    /// The titles G7 wrote, to look for them where they mustn't be.
    written: RefCell<Vec<String>>,
    tabs_readable: Cell<bool>,
    select_ok: Cell<bool>,
    keys_ready: Cell<bool>,
    paste_ok: Cell<bool>,
    enter_ok: Cell<bool>,
    locked: RefCell<VecDeque<Option<bool>>>,
    free: Cell<Option<Duration>>,
    typing: Cell<bool>,
    /// The window comes forward after this many looks.
    focus_after_waits: Cell<Option<usize>>,
    after_primary: RefCell<Option<Hook>>,
    after_paste: RefCell<Option<Hook>>,
    after_enter: RefCell<Option<Hook>>,
    after_select: RefCell<Option<Hook>>,
    after_mark: RefCell<Option<Hook>>,
    start: Instant,
    /// Time passed on the fake clock: every wait, and what hooks add.
    clock: Cell<Duration>,
}

fn ancestor(pid: u32, name: &str) -> Ancestor {
    Ancestor { pid, start_ticks: u64::from(pid) * 10, comm: name.into(), exe_name: name.into() }
}

fn place() -> Place {
    Place {
        agent: Agent::Claude,
        source: Source::Terminal,
        state: Now::Idle,
        state_exact: true,
        pid: 300,
        start_ticks: 3000,
        tty: Some(TTY),
        foreground: true,
        chain: vec![
            ancestor(200, "bash"),
            ancestor(150, "ptyxis-agent"),
            ancestor(EMULATOR, "ptyxis"),
            ancestor(2, "systemd"),
        ],
        ptys_under: vec![(200, 1), (150, 1), (EMULATOR, 1), (2, 4)],
        marks: vec![MARK.into()],
        folder: "g7linux".into(),
        continue_block: None,
    }
}

fn win(id: u64, pid: u32, class: &str, title: &str) -> Win {
    Win {
        id,
        pid,
        class: class.into(),
        title: title.into(),
        focused: false,
        fullscreen: false,
        minimized: false,
        normal: true,
        workspace: Some(0),
    }
}

fn files_in_front() -> Win {
    Win { focused: true, ..win(1, 50, "org.gnome.Nautilus", "Home") }
}

fn tab(tty: u32, title: &str) -> Tab {
    Tab { tty, title: title.into(), pushed: Vec::new() }
}

impl World {
    fn new() -> Self {
        World {
            log: RefCell::new(Vec::new()),
            places: RefCell::new(VecDeque::from([Ok(Found::Place(place()))])),
            found: RefCell::new(Vec::new()),
            host_moves: Cell::new(true),
            closes: RefCell::new(VecDeque::from([Ok(move_here::Closing::Closed)])),
            continues: RefCell::new(VecDeque::from([Ok(move_here::Continued::Refused)])),
            windows_ready: Cell::new(true),
            wins: RefCell::new(Some(vec![
                files_in_front(),
                win(WINDOW, EMULATOR, "org.gnome.Ptyxis", CLAUDE_TITLE),
            ])),
            tabs: RefCell::new(vec![(WINDOW, vec![tab(TTY, CLAUDE_TITLE)], 0)]),
            activate_ok: Cell::new(true),
            activate_focuses: Cell::new(true),
            stuck: RefCell::new(Vec::new()),
            screen: RefCell::new(None),
            primary: RefCell::new(Some("the owner's selection".into())),
            primary_ok: Cell::new(true),
            app: RefCell::new(Some("Terminal".into())),
            title_ok: Cell::new(true),
            title_shows: Cell::new(true),
            written: RefCell::new(Vec::new()),
            tabs_readable: Cell::new(true),
            select_ok: Cell::new(true),
            keys_ready: Cell::new(true),
            paste_ok: Cell::new(true),
            enter_ok: Cell::new(true),
            locked: RefCell::new(VecDeque::from([Some(false)])),
            free: Cell::new(Some(Duration::ZERO)),
            typing: Cell::new(false),
            focus_after_waits: Cell::new(None),
            after_primary: RefCell::new(None),
            after_paste: RefCell::new(None),
            after_enter: RefCell::new(None),
            after_select: RefCell::new(None),
            after_mark: RefCell::new(None),
            start: Instant::now(),
            clock: Cell::new(Duration::ZERO),
        }
    }

    /// The host answers these in turn, the last one from then on.
    fn places(&self, answers: Vec<Result<Found, HostDown>>) {
        *self.places.borrow_mut() = answers.into();
    }

    fn place_is(&self, place: Place) {
        self.places(vec![Ok(Found::Place(place))]);
    }

    fn locked_is(&self, answers: Vec<Option<bool>>) {
        *self.locked.borrow_mut() = answers.into();
    }

    /// The terminal's windows: each with its tabs and the selected one.
    fn tabbed(&self, layout: Vec<(u64, Vec<Tab>, usize)>) {
        *self.tabs.borrow_mut() = layout;
        self.titles_follow();
    }

    /// Each window's title is its selected tab's.
    fn titles_follow(&self) {
        let tabs = self.tabs.borrow();
        if let Some(wins) = self.wins.borrow_mut().as_mut() {
            for w in wins.iter_mut() {
                if let Some((_, tabs, selected)) = tabs.iter().find(|(id, ..)| *id == w.id) {
                    w.title = tabs[*selected].title.clone();
                }
            }
        }
    }

    /// The tab selected in `window`.
    fn selected(&self, window: u64) -> usize {
        self.tabs.borrow().iter().find(|(id, ..)| *id == window).map(|(_, _, s)| *s).expect("a window")
    }

    /// The title of the tab on `tty`.
    fn title_of(&self, tty: u32) -> String {
        let tabs = self.tabs.borrow();
        let tab = tabs.iter().flat_map(|(_, tabs, _)| tabs).find(|t| t.tty == tty).expect("a tab");
        tab.title.clone()
    }

    fn focus(&self, id: u64) {
        if let Some(wins) = self.wins.borrow_mut().as_mut() {
            for w in wins {
                w.focused = w.id == id;
            }
        }
    }

    fn note(&self, what: impl Into<String>) {
        self.log.borrow_mut().push(what.into());
    }

    fn log(&self) -> Vec<String> {
        self.log.borrow().clone()
    }

    /// The log entries whose first word is one of `names`, in order.
    fn events(&self, names: &[&str]) -> Vec<String> {
        self.log().into_iter().filter(|e| names.contains(&e.split(' ').next().unwrap_or_default())).collect()
    }

    /// The keys pressed, in order.
    fn keys(&self) -> Vec<String> {
        self.events(&["paste", "enter"])
    }

    fn activated(&self) -> bool {
        self.log().iter().any(|e| e.starts_with("activate"))
    }

    fn count(&self, what: &str) -> usize {
        self.log().iter().filter(|e| *e == what).count()
    }

    fn pass(&self, how_long: Duration) {
        self.clock.set(self.clock.get() + how_long);
    }

    fn pc(&self) -> Pc<'_, World, World, World, World> {
        Pc { host: self, windows: self, keys: self, desk: self }
    }
}

fn next<T: Clone>(queue: &RefCell<VecDeque<T>>) -> T {
    let mut queue = queue.borrow_mut();
    if queue.len() > 1 {
        queue.pop_front().expect("not empty")
    } else {
        queue.front().cloned().expect("an answer")
    }
}

impl Host for World {
    fn place(&self, _session: &str) -> Result<Found, HostDown> {
        self.note("place");
        let answer = next(&self.places);
        self.found.borrow_mut().push(match &answer {
            Ok(Found::Place(_)) => "place",
            Ok(Found::Ended) => "ended",
            Ok(Found::Elsewhere) => "elsewhere",
            Ok(Found::Ambiguous) => "ambiguous",
            Ok(Found::Unknown) => "unknown",
            Err(HostDown) => "down",
        });
        answer
    }
}

impl Windows for World {
    fn ready(&self) -> bool {
        self.windows_ready.get()
    }

    fn list(&self) -> Option<Vec<Win>> {
        self.wins.borrow().clone()
    }

    fn activate(&self, id: u64) -> bool {
        self.note(format!("activate {id}"));
        if self.activate_ok.get() && self.activate_focuses.get() && !self.stuck.borrow().contains(&id) {
            self.focus(id);
        }
        self.activate_ok.get()
    }

    fn minimize(&self, id: u64) -> bool {
        self.note(format!("minimize {id}"));
        if let Some(wins) = self.wins.borrow_mut().as_mut() {
            for w in wins.iter_mut().filter(|w| w.id == id) {
                w.minimized = true;
                w.focused = false;
            }
        }
        true
    }

    fn set_primary(&self, text: &str) -> bool {
        self.note("primary");
        if self.primary_ok.get() {
            *self.primary.borrow_mut() = Some(text.to_owned());
        }
        if let Some(hook) = self.after_primary.borrow().as_ref() {
            hook(self);
        }
        self.primary_ok.get()
    }

    fn clear_primary(&self, expected: &str) {
        self.note("clear");
        let mut primary = self.primary.borrow_mut();
        if primary.as_deref() == Some(expected) {
            *primary = None;
        }
    }

    fn terminal_text(&self, pid: u32) -> Option<String> {
        self.note("read");
        (pid == EMULATOR).then(|| self.screen.borrow().clone()).flatten()
    }

    fn app_name(&self, class: &str, _pid: u32) -> Option<String> {
        match class {
            "code" => Some("Visual Studio Code".into()),
            "kitty" => Some("kitty".into()),
            _ => self.app.borrow().clone(),
        }
    }

    fn title_mark(&self, tty: u32, title: &str) -> bool {
        self.note("mark");
        if let Some(hook) = self.after_mark.borrow().as_ref() {
            hook(self);
        }
        if !self.title_ok.get() {
            return false;
        }
        self.written.borrow_mut().push(title.to_owned());
        for (_, tabs, _) in self.tabs.borrow_mut().iter_mut() {
            for tab in tabs.iter_mut().filter(|t| t.tty == tty) {
                tab.pushed.push(tab.title.clone());
                if self.title_shows.get() {
                    // Ptyxis adds the foreground command.
                    tab.title = format!("{title} — claude ");
                }
            }
        }
        self.titles_follow();
        true
    }

    fn title_unmark(&self, tty: u32) {
        self.note("unmark");
        for (_, tabs, _) in self.tabs.borrow_mut().iter_mut() {
            for tab in tabs.iter_mut().filter(|t| t.tty == tty) {
                if let Some(old) = tab.pushed.pop() {
                    tab.title = old;
                }
            }
        }
        self.titles_follow();
    }

    fn terminal_reset(&self, tty: u32) -> bool {
        self.note(format!("reset {tty}"));
        true
    }

    fn tab_titled(&self, pid: u32, mark: &str, _route: TabRoute) -> Option<Vec<TabSpot>> {
        self.note("tabs");
        if pid != EMULATOR || !self.tabs_readable.get() {
            return None;
        }
        let wins = self.wins.borrow().clone().unwrap_or_default();
        let mut spots = Vec::new();
        for (id, tabs, selected) in self.tabs.borrow().iter() {
            let frame_title = wins.iter().find(|w| w.id == *id).map(|w| w.title.clone()).unwrap_or_default();
            for (at, _) in tabs.iter().enumerate().filter(|(_, t)| t.title.contains(mark)) {
                spots.push(TabSpot {
                    frame_title: frame_title.clone(),
                    at,
                    selected: *selected,
                    tabs: tabs.len(),
                });
            }
        }
        Some(spots)
    }

    fn select_tab(&self, pid: u32, mark: &str, _route: TabRoute) -> Result<usize, TabError> {
        self.note("select");
        if !self.select_ok.get() {
            return Err(TabError::CantSwitch);
        }
        let front =
            self.wins.borrow().clone().unwrap_or_default().into_iter().find(|w| w.focused && w.pid == pid);
        let front = front.ok_or(TabError::NotFound)?;
        let before = {
            let mut all = self.tabs.borrow_mut();
            let (_, tabs, selected) =
                all.iter_mut().find(|(id, ..)| *id == front.id).ok_or(TabError::NotFound)?;
            let at = tabs.iter().position(|t| t.title.contains(mark)).ok_or(TabError::NotFound)?;
            std::mem::replace(selected, at)
        };
        self.titles_follow();
        if let Some(hook) = self.after_select.borrow().as_ref() {
            hook(self);
        }
        Ok(before)
    }

    fn select_tab_at(&self, _pid: u32, mark: &str, at: usize, _route: TabRoute) -> bool {
        self.note(format!("back {at}"));
        let done = {
            let mut all = self.tabs.borrow_mut();
            match all.iter_mut().find(|(_, tabs, _)| tabs.iter().any(|t| t.title.contains(mark))) {
                Some((_, tabs, selected)) if at < tabs.len() => {
                    *selected = at;
                    true
                }
                _ => false,
            }
        };
        self.titles_follow();
        done
    }
}

impl Keys for World {
    fn ready(&self) -> bool {
        self.keys_ready.get()
    }

    fn paste(&self) -> bool {
        self.note("paste");
        if let Some(hook) = self.after_paste.borrow().as_ref() {
            hook(self);
        }
        self.paste_ok.get()
    }

    fn enter(&self) -> bool {
        self.note("enter");
        if let Some(hook) = self.after_enter.borrow().as_ref() {
            hook(self);
        }
        self.enter_ok.get()
    }
}

impl Desk for World {
    fn locked(&self) -> Option<bool> {
        next(&self.locked)
    }

    fn free_in(&self) -> Option<Duration> {
        self.free.get()
    }

    fn phone_typing(&self) -> bool {
        self.typing.get()
    }

    fn wait(&self, how_long: Duration) {
        self.note(format!("wait {}", how_long.as_millis()));
        self.pass(how_long);
        let looks = self.log().iter().filter(|e| *e == "wait 50").count();
        if self.focus_after_waits.get() == Some(looks) {
            self.focus(WINDOW);
        }
    }

    fn now(&self) -> Instant {
        self.start + self.clock.get()
    }
}

/// The session in a Ptyxis with two terminals.
fn tabs_place() -> Place {
    Place { ptys_under: vec![(200, 1), (150, 2), (EMULATOR, 2)], ..place() }
}

/// Ptyxis window 7 with two tabs, a shell on pts/1 and the session on
/// pts/0, `selected` in front: only the title can tell them apart.
fn two_tabs(selected: usize) -> World {
    let world = World::new();
    world.place_is(tabs_place());
    world.tabbed(vec![(WINDOW, vec![tab(OTHER_TTY, "~/src — bash "), tab(TTY, CLAUDE_TITLE)], selected)]);
    world
}

fn reply(text: &str) -> Ask {
    Ask { session: SESSION.into(), text: Some(text.into()), check: false, confirm: None }
}

fn check_ask() -> Ask {
    Ask { session: SESSION.into(), text: None, check: true, confirm: None }
}

/// A ledger that already confirmed this run of the agent in this Ptyxis,
/// for this phone and session.
fn confirmed() -> Replies {
    let replies = Replies::default();
    let bound = Bound {
        phone: PHONE.into(),
        session: SESSION.into(),
        agent: 300,
        agent_start: 3000,
        emulator: EMULATOR,
        emulator_start: u64::from(EMULATOR) * 10,
    };
    let now = Instant::now();
    let nonce = replies.ledger().mint(bound.clone(), now);
    assert!(replies.ledger().redeem(&nonce, &bound, now));
    replies
}

fn send(world: &World, replies: &Replies, text: &str) -> Result<Answer, Error> {
    answer(&world.pc(), replies, PHONE, &reply(text))
}

fn sent(world: &World) -> Answer {
    send(world, &confirmed(), "Looks good, ship it").expect("an answer")
}

fn outcome(world: &World) -> Outcome {
    sent(world).outcome
}

const NOTHING: [&str; 0] = [];

// ---- The way through ----

#[test]
fn a_reply_is_pasted_from_primary_and_entered() {
    let world = World::new();
    let answer = sent(&world);
    assert_eq!(answer.outcome, Outcome::Sent);
    assert_eq!(
        answer.target,
        Some(Target {
            app: "Terminal".into(),
            agent: "Claude".into(),
            folder: "g7linux".into(),
            kind: "terminal"
        })
    );
    assert_eq!(answer.emulator.as_deref(), Some("ptyxis"));
    // One window, one terminal: no title is written and no screen is read.
    assert_eq!(
        world.log(),
        [
            "place",
            "activate 7",
            "place",
            "primary",
            "paste",
            "wait 250",
            "place",
            "enter",
            "clear",
            "wait 300",
            "activate 1"
        ],
        "the same process before the paste; PRIMARY set before the chord and cleared after Enter; \
         then the focus back to the file manager"
    );
    assert_eq!(*world.primary.borrow(), None);
    assert_eq!(answer.trail, Trail { proof: "structural", tab: "", steps: 0, back: "given" });
}

#[test]
fn the_desktop_entry_names_the_app_or_the_table_does() {
    let world = World::new();
    *world.app.borrow_mut() = None;
    assert_eq!(sent(&world).target.map(|t| t.app), Some("Ptyxis".into()));
    let world = World::new();
    let mut p = place();
    p.folder = "/home/me/src/g7linux/".into();
    world.place_is(p);
    assert_eq!(sent(&world).target.map(|t| t.folder), Some("g7linux".into()), "never a path");
}

#[test]
fn waits_for_the_window_to_come_forward() {
    let world = World::new();
    world.activate_focuses.set(false);
    world.focus_after_waits.set(Some(3));
    assert_eq!(outcome(&world), Outcome::Sent);
    let log = world.log();
    let before_paste = log.iter().take_while(|e| *e != "paste").filter(|e| *e == "wait 50").count();
    assert_eq!(before_paste, 3);
}

#[test]
fn a_window_that_doesnt_come_forward_types_nothing() {
    let world = World::new();
    world.activate_focuses.set(false);
    assert_eq!(outcome(&world), Outcome::Failed(Failed::Focus));
    assert_eq!(world.keys(), NOTHING);
    let waits = world.log().iter().filter(|e| *e == "wait 50").count();
    assert_eq!(waits, 20, "a second, looked at every 50 ms");
    assert!(!world.log().contains(&"primary".to_owned()));

    let world = World::new();
    world.activate_ok.set(false);
    assert_eq!(outcome(&world), Outcome::Failed(Failed::Focus));
    assert_eq!(world.keys(), NOTHING);
}

// ---- Checks ----

#[test]
fn a_check_enters_nothing_and_says_where() {
    let world = World::new();
    let answer = answer(&world.pc(), &Replies::default(), PHONE, &check_ask()).expect("an answer");
    assert_eq!(answer.outcome, Outcome::Ready);
    assert_eq!(answer.target.map(|t| t.agent), Some("Claude".into()));
    assert_eq!(answer.nonce, None, "a check asks for no confirmation");
    assert!(!world.activated());
    assert_eq!(world.keys(), NOTHING);
    assert!(!world.log().contains(&"primary".to_owned()));

    let world = World::new();
    world.locked_is(vec![Some(true)]);
    let answer = super::answer(&world.pc(), &Replies::default(), PHONE, &check_ask()).expect("an answer");
    assert_eq!(answer.outcome, Outcome::Refused(Refused::Locked));
}

#[test]
fn checks_are_held_to_two_a_second() {
    let world = World::new();
    let replies = Replies::default();
    for _ in 0..2 {
        let ok = answer(&world.pc(), &replies, PHONE, &check_ask()).expect("an answer");
        assert_eq!(ok.outcome, Outcome::Ready);
    }
    let third = answer(&world.pc(), &replies, PHONE, &check_ask()).expect("an answer");
    assert_eq!(third.outcome, Outcome::Refused(Refused::Rate));
    assert!(third.retry_ms.is_some_and(|ms| (1..=1000).contains(&ms)));
    // Another phone has its own.
    let other = answer(&world.pc(), &replies, "phone-b", &check_ask()).expect("an answer");
    assert_eq!(other.outcome, Outcome::Ready);
}

#[test]
fn text_and_check_go_one_at_a_time() {
    let world = World::new();
    let both = Ask { check: true, ..reply("hi") };
    assert_eq!(answer(&world.pc(), &Replies::default(), PHONE, &both), Err(Error::BadMessage));
    let neither = Ask { text: None, ..reply("hi") };
    assert_eq!(answer(&world.pc(), &Replies::default(), PHONE, &neither), Err(Error::BadMessage));
    assert!(world.log().is_empty());
}

// ---- The confirmation (§3.3) ----

#[test]
fn the_first_reply_asks_and_the_nonce_sends_it() {
    let world = World::new();
    let replies = Replies::default();
    let first = send(&world, &replies, "hello").expect("an answer");
    assert_eq!(first.outcome, Outcome::Refused(Refused::Confirm));
    assert!(first.target.is_some());
    let nonce = first.nonce.expect("a nonce");
    assert_eq!(nonce.len(), 32);
    assert!(nonce.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    assert!(!world.activated(), "nothing is brought forward before the phone confirms");
    assert_eq!(world.keys(), NOTHING);

    // Sent again with the nonce, at once: the rate doesn't stand in the way.
    let again = Ask { confirm: Some(nonce.clone()), ..reply("hello") };
    let second = answer(&world.pc(), &replies, PHONE, &again).expect("an answer");
    assert_eq!(second.outcome, Outcome::Sent);
    assert_eq!(world.keys(), ["paste", "enter"]);
}

#[test]
fn a_wrong_nonce_asks_again() {
    let world = World::new();
    let replies = Replies::default();
    let wrong = Ask { confirm: Some("0".repeat(32)), ..reply("hello") };
    let answer = answer(&world.pc(), &replies, PHONE, &wrong).expect("an answer");
    assert_eq!(answer.outcome, Outcome::Refused(Refused::Confirm));
    assert!(answer.nonce.is_some());
    assert_eq!(world.keys(), NOTHING);
}

#[test]
fn a_restarted_terminal_asks_again() {
    let world = World::new();
    let replies = confirmed();
    // The terminal was restarted: another window and a new process.
    *world.wins.borrow_mut() = Some(vec![files_in_front(), win(8, 101, "org.gnome.Ptyxis", "claude")]);
    let mut p = place();
    p.chain[2] = ancestor(101, "ptyxis");
    p.ptys_under = vec![(200, 1), (150, 1), (101, 1)];
    world.place_is(p);
    let answer = send(&world, &replies, "hello").expect("an answer");
    assert_eq!(answer.outcome, Outcome::Refused(Refused::Confirm));
    assert_eq!(world.keys(), NOTHING);
}

#[test]
fn the_confirmation_holds_for_the_agent_not_the_window() {
    // The session's tab dragged to another window of the same Ptyxis.
    let world = World::new();
    *world.wins.borrow_mut() = Some(vec![files_in_front(), win(9, EMULATOR, "org.gnome.Ptyxis", "claude")]);
    world.tabbed(vec![(9, vec![tab(TTY, CLAUDE_TITLE)], 0)]);
    let replies = confirmed();
    assert_eq!(send(&world, &replies, "hello").map(|a| a.outcome), Ok(Outcome::Sent));
    assert!(world.log().contains(&"activate 9".to_owned()));

    // The agent restarted in the same terminal (`claude --resume`).
    let world = World::new();
    world.place_is(Place { pid: 301, start_ticks: 3010, ..place() });
    let answer = send(&world, &confirmed(), "hello").expect("an answer");
    assert_eq!(answer.outcome, Outcome::Refused(Refused::Confirm));
    assert!(!world.activated());
    assert_eq!(world.keys(), NOTHING);
}

#[test]
fn a_nonce_works_once_for_its_window_and_expires() {
    let bound = Bound {
        phone: PHONE.into(),
        session: SESSION.into(),
        agent: 300,
        agent_start: 3000,
        emulator: EMULATOR,
        emulator_start: 1000,
    };
    let t0 = Instant::now();
    let mut ledger = Ledger::default();
    let nonce = ledger.mint(bound.clone(), t0);
    assert!(ledger.holds(PHONE, SESSION, &nonce, t0));
    assert!(!ledger.holds("phone-b", SESSION, &nonce, t0), "another phone's");
    assert!(ledger.redeem(&nonce, &bound, t0));
    assert!(!ledger.redeem(&nonce, &bound, t0), "once");
    assert!(ledger.is_confirmed(&bound, t0));
    assert!(ledger.is_confirmed(&bound, t0 + Duration::from_secs(23 * 3600)));
    assert!(!ledger.is_confirmed(&bound, t0 + Duration::from_secs(24 * 3600)), "a day");

    let mut ledger = Ledger::default();
    let nonce = ledger.mint(bound.clone(), t0);
    assert!(!ledger.redeem(&nonce, &bound, t0 + Duration::from_secs(121)), "two minutes");

    for other in [
        Bound { phone: "phone-b".into(), ..bound.clone() },
        Bound { session: "sc9".into(), ..bound.clone() },
        Bound { agent: 301, ..bound.clone() },
        Bound { agent_start: 3001, ..bound.clone() },
        Bound { emulator_start: 1001, ..bound.clone() },
    ] {
        let mut ledger = Ledger::default();
        let nonce = ledger.mint(bound.clone(), t0);
        assert!(!ledger.redeem(&nonce, &other, t0), "{other:?}");
        assert!(!ledger.redeem(&nonce, &bound, t0), "spent even when it didn't match");
    }

    let mut ledger = Ledger::default();
    let nonces: Vec<String> = (0..40).map(|_| ledger.mint(bound.clone(), t0)).collect();
    assert!(!ledger.holds(PHONE, SESSION, &nonces[0], t0), "the oldest go first");
    assert!(ledger.holds(PHONE, SESSION, &nonces[39], t0));
}

// ---- Rate (§3.5) ----

#[test]
fn one_attempt_a_second_is_looked_at() {
    let t0 = Instant::now();
    let mut ledger = Ledger::default();
    assert_eq!(ledger.attempt(PHONE, SESSION, t0, false), Ok(()));
    let at = t0 + Duration::from_millis(400);
    assert_eq!(ledger.attempt(PHONE, SESSION, at, false), Err(Duration::from_millis(600)));
    assert_eq!(ledger.attempt(PHONE, SESSION, at, true), Ok(()), "a confirmed send");
    assert_eq!(ledger.attempt("phone-b", SESSION, at, false), Ok(()), "each phone its own");
    assert_eq!(ledger.attempt(PHONE, SESSION, t0 + Duration::from_millis(1500), false), Ok(()));
}

#[test]
fn entered_replies_are_paced_per_session_minute_and_hour() {
    let t0 = Instant::now();
    let ms = |n: u64| t0 + Duration::from_millis(n);
    let mut ledger = Ledger::default();
    ledger.entered(PHONE, SESSION, t0);
    assert_eq!(ledger.attempt(PHONE, SESSION, ms(1200), false), Err(Duration::from_millis(1800)));
    assert_eq!(ledger.attempt(PHONE, "sc9", ms(2400), false), Ok(()), "another session");
    assert_eq!(ledger.attempt(PHONE, SESSION, ms(3600), false), Ok(()));

    // Six a minute, whatever the session.
    let mut ledger = Ledger::default();
    for i in 0..6 {
        ledger.entered(PHONE, &format!("sc{i}"), ms(i * 4000));
    }
    let at = ms(30_000);
    assert_eq!(ledger.attempt(PHONE, "sc7", at, false), Err(Duration::from_secs(30)));
    assert_eq!(ledger.attempt(PHONE, "sc7", ms(60_000), false), Ok(()));

    // Sixty an hour.
    let mut ledger = Ledger::default();
    for i in 0..60 {
        ledger.entered(PHONE, &format!("sc{i}"), ms(i * 50_000));
    }
    let wait = ledger.attempt(PHONE, "sc99", ms(59 * 50_000 + 20_000), false).expect_err("the hour");
    assert_eq!(wait, Duration::from_secs(3600) - Duration::from_millis(59 * 50_000 + 20_000));
    assert_eq!(wait_ms(wait), 60_000, "retry_ms is at most a minute");
}

#[test]
fn refusals_dont_count_but_entered_replies_do() {
    let world = World::new();
    let replies = confirmed();
    world.free.set(Some(Duration::from_secs(3)));
    assert_eq!(send(&world, &replies, "hi").map(|a| a.outcome), Ok(Outcome::Refused(Refused::InUse)));
    assert!(replies.ledger().phones.iter().all(|(_, pace)| pace.entered.is_empty()));

    let world = World::new();
    world.locked_is(vec![Some(false), Some(false), Some(true)]);
    assert_eq!(outcome(&world), Outcome::Failed(Failed::Interrupted));
    let world = World::new();
    let replies = confirmed();
    assert_eq!(send(&world, &replies, "hi").map(|a| a.outcome), Ok(Outcome::Sent));
    let entered = replies.ledger().phones.iter().map(|(_, pace)| pace.entered.len()).sum::<usize>();
    assert_eq!(entered, 1);
}

#[test]
fn a_second_reply_at_once_is_refused_rate() {
    let world = World::new();
    let replies = confirmed();
    assert_eq!(send(&world, &replies, "one").map(|a| a.outcome), Ok(Outcome::Sent));
    let second = send(&world, &replies, "two").expect("an answer");
    assert_eq!(second.outcome, Outcome::Refused(Refused::Rate));
    assert!(second.retry_ms.is_some());
    assert_eq!(world.keys(), ["paste", "enter"], "only the first");
}

#[test]
fn one_reply_at_a_time_on_the_pc() {
    let world = World::new();
    let replies = confirmed();
    let _entering = replies.entering.lock().expect("the lock");
    assert_eq!(send(&world, &replies, "hi"), Err(Error::Busy));
    assert!(world.log().is_empty());
}

// ---- The text (§2.5) ----

#[test]
fn the_text_is_cleaned_for_a_terminal() {
    assert_eq!(clean("one\r\ntwo\rthree\tfour  \n\n"), Ok(Ok("one\ntwo\nthree    four".into())));
    assert_eq!(clean("  indented"), Ok(Ok("  indented".into())));
    assert_eq!(clean("é".repeat(4096).as_str()), Ok(Ok("é".repeat(4096))));
    assert_eq!(clean(""), Err(Error::BadMessage));
    assert_eq!(clean(" \n\t "), Err(Error::BadMessage));
    assert_eq!(clean(&"a".repeat(MAX_TEXT + 1)), Err(Error::BadMessage));
    assert_eq!(
        clean(&"\t".repeat(MAX_TEXT / 4 + 1).replace('\t', "a\t")),
        Err(Error::BadMessage),
        "after tabs"
    );
}

#[test]
fn nothing_can_end_the_paste_or_reorder_the_text() {
    for bad in [
        "stop\x1b[201~rm -rf ~",
        "bell\x07",
        "nul\0",
        "del\x7f",
        "c1\u{9b}201~",
        "rtl\u{202E}txt",
        "iso\u{2067}",
    ] {
        assert_eq!(clean(bad), Err(Error::BadMessage), "{bad:?}");
    }
    let world = World::new();
    assert_eq!(send(&world, &confirmed(), "x\x1b[201~\ny"), Err(Error::BadMessage));
    assert!(world.log().is_empty());
}

#[test]
fn commands_are_refused_text() {
    for command in ["!rm -rf ~", "/clear", "  !ls", "\n/model"] {
        assert_eq!(clean(command), Ok(Err(Refused::Text)), "{command:?}");
        let world = World::new();
        assert_eq!(
            send(&world, &confirmed(), command).map(|a| a.outcome),
            Ok(Outcome::Refused(Refused::Text))
        );
        assert!(world.log().is_empty(), "nothing is asked of the PC");
    }
    assert_eq!(clean("what does /clear do?"), Ok(Ok("what does /clear do?".into())));
}

#[test]
fn the_requests_debug_shows_no_text() {
    let shown = format!("{:?}", reply("my secret plan"));
    assert!(!shown.contains("secret"), "{shown}");
    assert!(shown.contains("14"));
    let shown = format!("{:?}", place());
    assert!(!shown.contains(MARK), "{shown}");

    // The title G7 wrote is in no answer.
    let world = two_tabs(0);
    let answer = sent(&world);
    assert_eq!(answer.outcome, Outcome::Sent);
    let written = world.written.borrow().clone();
    assert_eq!(written.len(), 1);
    assert!(written[0].starts_with("G7 ") && written[0].len() == 19, "G7 and 16 hex digits");
    let shown = format!("{answer:?}");
    assert!(!shown.contains(&written[0][3..]), "{shown}");
}

// ---- The host's answers (§1.1, §2.6) ----

#[test]
fn the_sessions_state_decides() {
    let cases = [
        (Agent::Claude, Now::Idle, true, Outcome::Sent),
        (Agent::Claude, Now::Idle, false, Outcome::Sent),
        (Agent::Claude, Now::Working, true, Outcome::Sent),
        (Agent::Claude, Now::Working, false, Outcome::Refused(Refused::Busy)),
        (Agent::Claude, Now::NeedsYou, true, Outcome::Refused(Refused::NeedsYou)),
        (Agent::Claude, Now::MayNeedYou, false, Outcome::Refused(Refused::NeedsYou)),
        (Agent::Claude, Now::Ended, true, Outcome::Refused(Refused::Ended)),
        (Agent::Codex, Now::Idle, false, Outcome::Sent),
        (Agent::Codex, Now::Working, false, Outcome::Refused(Refused::Busy)),
        (Agent::Codex, Now::MayNeedYou, false, Outcome::Refused(Refused::NeedsYou)),
    ];
    for (agent, state, exact, expected) in cases {
        let world = World::new();
        world.place_is(Place { agent, state, state_exact: exact, ..place() });
        // Codex, and Claude without its status file, are proven by the
        // title and the screen.
        *world.screen.borrow_mut() = Some(MARK.into());
        assert_eq!(outcome(&world), expected, "{agent:?} {state:?} exact {exact}");
        if expected != Outcome::Sent {
            assert_eq!(world.keys(), NOTHING);
            assert!(!world.activated());
        }
    }
}

#[test]
fn what_the_host_couldnt_place_is_refused() {
    let cases = [
        (Found::Ended, Refused::Ended),
        (Found::Ambiguous, Refused::Ambiguous),
        (Found::Elsewhere, Refused::Place),
    ];
    for (found, reason) in cases {
        let world = World::new();
        world.places(vec![Ok(found.clone())]);
        assert_eq!(outcome(&world), Outcome::Refused(reason), "{found:?}");
        assert_eq!(world.keys(), NOTHING);
    }
    let world = World::new();
    world.places(vec![Ok(Found::Unknown)]);
    assert_eq!(send(&world, &confirmed(), "hi"), Err(Error::NotFound));
    let world = World::new();
    world.places(vec![Err(HostDown)]);
    assert_eq!(send(&world, &confirmed(), "hi"), Err(Error::Internal));
    assert_eq!(answer(&world.pc(), &Replies::default(), PHONE, &check_ask()), Err(Error::Internal));
}

#[test]
fn the_shell_never_gets_it() {
    // The CLI exited or was suspended: a shell is the foreground job.
    let world = World::new();
    world.place_is(Place { foreground: false, ..place() });
    assert_eq!(outcome(&world), Outcome::Refused(Refused::NoProcess));
    assert_eq!(world.keys(), NOTHING);
    // No terminal at all.
    let world = World::new();
    world.place_is(Place { tty: None, ..place() });
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Place));
}

#[test]
fn the_claude_app_is_refused_place() {
    let world = World::new();
    world.place_is(Place { source: Source::DesktopApp, ..place() });
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Place));
    let world = World::new();
    world.place_is(Place { source: Source::Other, ..place() });
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Place));
    assert_eq!(world.keys(), NOTHING);
}

// ---- The PC (§3.1, §3.2) ----

#[test]
fn locked_is_refused_and_unknown_is_locked() {
    for lock in [Some(true), None] {
        let world = World::new();
        world.locked_is(vec![lock]);
        assert_eq!(outcome(&world), Outcome::Refused(Refused::Locked), "{lock:?}");
        assert!(!world.activated(), "never brought forward on the lock screen");
        assert_eq!(world.keys(), NOTHING);
    }
    // Locked between the checks and bringing the window forward.
    let world = World::new();
    world.locked_is(vec![Some(false), Some(true)]);
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Locked));
    assert!(!world.activated());
}

#[test]
fn locked_after_the_paste_is_interrupted() {
    let world = World::new();
    world.locked_is(vec![Some(false), Some(false), Some(true)]);
    assert_eq!(outcome(&world), Outcome::Failed(Failed::Interrupted));
    assert_eq!(world.keys(), ["paste"], "the chord and no Enter");
    assert_eq!(*world.primary.borrow(), None, "PRIMARY cleared");
}

#[test]
fn someone_at_the_pc_is_in_use() {
    let s = Duration::from_secs;
    for (free, typing) in [(Some(s(3)), false), (None, false), (Some(Duration::ZERO), true)] {
        let world = two_tabs(0);
        world.free.set(free);
        world.typing.set(typing);
        assert_eq!(outcome(&world), Outcome::Refused(Refused::InUse), "{free:?} {typing}");
        assert!(!world.activated());
        assert_eq!(world.keys(), NOTHING);
        assert!(
            world.events(&["mark", "tabs", "read"]).is_empty(),
            "nothing titled or read while someone's there"
        );
    }
}

#[test]
fn in_use_says_how_long_to_wait() {
    let ms = Duration::from_millis;
    let retry = |world: &World| {
        let answer = sent(world);
        assert_eq!(answer.outcome, Outcome::Refused(Refused::InUse));
        assert!(answer.target.is_some());
        answer.retry_ms
    };
    // The rest of the owner's 5 s, and a little more.
    let world = World::new();
    world.free.set(Some(ms(2000)));
    assert_eq!(retry(&world), Some(2100));
    let world = World::new();
    world.free.set(Some(ms(50)));
    assert_eq!(retry(&world), Some(250), "at least 250 ms");
    let world = World::new();
    world.free.set(Some(ms(9000)));
    assert_eq!(retry(&world), Some(5000), "at most 5 s");
    // A phone's input being typed.
    let world = World::new();
    world.typing.set(true);
    assert_eq!(retry(&world), Some(1000));
    // A film in front.
    let world = World::new();
    if let Some(wins) = world.wins.borrow_mut().as_mut() {
        wins[0].fullscreen = true;
    }
    assert_eq!(retry(&world), Some(5000));
    // The desktop doesn't say: waiting won't help.
    let world = World::new();
    world.free.set(None);
    assert_eq!(retry(&world), None);
}

#[test]
fn a_check_with_someone_at_the_pc_is_ready() {
    let busy: [Hook; 4] = [
        Box::new(|w: &World| w.free.set(Some(Duration::from_secs(4)))),
        Box::new(|w: &World| w.free.set(None)),
        Box::new(|w: &World| w.typing.set(true)),
        Box::new(|w: &World| {
            if let Some(wins) = w.wins.borrow_mut().as_mut() {
                wins[0].fullscreen = true;
            }
        }),
    ];
    for (i, make_busy) in busy.iter().enumerate() {
        let world = World::new();
        make_busy(&world);
        let answer = answer(&world.pc(), &Replies::default(), PHONE, &check_ask()).expect("an answer");
        assert_eq!(answer.outcome, Outcome::Ready, "case {i}: a reply would wait");
        assert_eq!(answer.retry_ms, None);
    }
}

#[test]
fn the_confirmation_comes_before_in_use() {
    let world = World::new();
    world.free.set(Some(Duration::from_secs(4)));
    let answer = send(&world, &Replies::default(), "hello").expect("an answer");
    assert_eq!(answer.outcome, Outcome::Refused(Refused::Confirm), "not after a wait");
    assert!(answer.nonce.is_some());
}

#[test]
fn a_fullscreen_window_in_front_is_in_use() {
    let world = World::new();
    if let Some(wins) = world.wins.borrow_mut().as_mut() {
        wins[0].fullscreen = true;
    }
    assert_eq!(outcome(&world), Outcome::Refused(Refused::InUse));
    assert_eq!(world.keys(), NOTHING);
    assert!(!world.activated());

    // The session's own terminal, fullscreen in front, is fine.
    let world = World::new();
    *world.wins.borrow_mut() = Some(vec![Win {
        focused: true,
        fullscreen: true,
        ..win(WINDOW, EMULATOR, "org.gnome.Ptyxis", "claude")
    }]);
    assert_eq!(outcome(&world), Outcome::Sent);
}

#[test]
fn without_the_extension_or_the_keyboard_it_says_setup() {
    let world = World::new();
    world.windows_ready.set(false);
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Setup));
    let world = World::new();
    *world.wins.borrow_mut() = None;
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Setup));
    let world = World::new();
    world.keys_ready.set(false);
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Setup));
    assert!(!world.activated());
    let world = World::new();
    world.primary_ok.set(false);
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Setup));
    assert_eq!(world.keys(), NOTHING);
}

// ---- Finding the window (§1.3, §2.2) ----

#[test]
fn whats_between_the_agent_and_a_window_is_refused_place() {
    for (name, comm) in [
        ("tmux", "tmux: server"),
        ("zellij", "zellij"),
        ("screen", "screen"),
        ("sshd", "sshd"),
        ("code", "code"),
    ] {
        let world = World::new();
        let mut p = place();
        p.chain.insert(1, Ancestor { pid: 180, start_ticks: 1, comm: comm.into(), exe_name: name.into() });
        world.place_is(p);
        assert_eq!(outcome(&world), Outcome::Refused(Refused::Place), "{name}");
        assert_eq!(world.keys(), NOTHING);
    }
}

#[test]
fn a_chain_that_reaches_no_window_is_no_window() {
    let world = World::new();
    let mut p = place();
    p.chain = vec![ancestor(200, "bash"), ancestor(1, "systemd")];
    world.place_is(p);
    assert_eq!(outcome(&world), Outcome::Refused(Refused::NoWindow));
    assert_eq!(world.keys(), NOTHING);
}

#[test]
fn only_known_terminals_are_pasted_into() {
    let world = World::new();
    *world.wins.borrow_mut() = Some(vec![files_in_front(), win(WINDOW, EMULATOR, "kitty", "claude")]);
    let mut p = place();
    p.chain[2] = ancestor(EMULATOR, "kitty");
    world.place_is(p);
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Place));

    // A known executable with another program's window class.
    let world = World::new();
    *world.wins.borrow_mut() =
        Some(vec![files_in_front(), win(WINDOW, EMULATOR, "org.example.Thing", "claude")]);
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Place));
    assert_eq!(world.keys(), NOTHING);

    // GNOME Terminal.
    let world = World::new();
    *world.wins.borrow_mut() =
        Some(vec![files_in_front(), win(WINDOW, EMULATOR, "org.gnome.Terminal", "claude")]);
    let mut p = place();
    p.chain = vec![ancestor(200, "bash"), ancestor(EMULATOR, "gnome-terminal-server")];
    p.ptys_under = vec![(200, 1), (EMULATOR, 1)];
    world.place_is(p);
    assert_eq!(outcome(&world), Outcome::Sent);
}

// ---- Proofs (§1.4, §1.5) ----

#[test]
fn structural_proof_needs_one_window_and_one_terminal() {
    // One window, three tabs: the title proves which.
    let world = World::new();
    let mut p = place();
    p.ptys_under = vec![(200, 1), (150, 3), (EMULATOR, 3)];
    world.place_is(p);
    world.tabbed(vec![(
        WINDOW,
        vec![tab(TTY, CLAUDE_TITLE), tab(OTHER_TTY, "bash"), tab(THIRD_TTY, "vim")],
        0,
    )]);
    let answer = sent(&world);
    assert_eq!(answer.outcome, Outcome::Sent);
    assert_eq!(world.count("mark"), 1);
    assert_eq!(answer.trail, Trail { proof: "title", tab: "front", steps: 0, back: "given" });

    // Two windows, one terminal each.
    let world = World::new();
    *world.wins.borrow_mut() = Some(vec![
        files_in_front(),
        win(WINDOW, EMULATOR, "org.gnome.Ptyxis", "claude"),
        win(8, EMULATOR, "org.gnome.Ptyxis", "notes"),
    ]);
    world.tabbed(vec![(WINDOW, vec![tab(TTY, CLAUDE_TITLE)], 0), (8, vec![tab(OTHER_TTY, "vim")], 0)]);
    let mut p = place();
    p.ptys_under = vec![(200, 1), (150, 2), (EMULATOR, 2)];
    world.place_is(p);
    assert_eq!(outcome(&world), Outcome::Sent);
    assert!(world.log().contains(&"activate 7".to_owned()));
    assert_eq!(world.count("mark"), 1);

    // A count the host couldn't make: the title proves it too.
    let world = World::new();
    let mut p = place();
    p.ptys_under.retain(|(pid, _)| *pid != EMULATOR);
    world.place_is(p);
    assert_eq!(outcome(&world), Outcome::Sent);
    assert_eq!(world.count("mark"), 1);
}

#[test]
fn codex_and_a_claude_without_its_status_file_need_the_title_and_the_screen() {
    // `codex resume <uuid>` keeps the id on its command line after `/resume`
    // inside it: the title says which terminal, the screen which session.
    for (agent, exact) in [(Agent::Codex, false), (Agent::Claude, false)] {
        let world = two_tabs(1);
        world.place_is(Place { agent, state_exact: exact, ..tabs_place() });
        assert_eq!(outcome(&world), Outcome::Refused(Refused::Ambiguous), "{agent:?} unread");
        assert_eq!(world.keys(), NOTHING);
        assert_eq!(world.events(&["mark", "unmark"]), ["mark", "unmark"]);

        let world = two_tabs(1);
        world.place_is(Place { agent, state_exact: exact, ..tabs_place() });
        *world.screen.borrow_mut() = Some("› /resume\n  another session".into());
        assert_eq!(outcome(&world), Outcome::Refused(Refused::Unproven), "{agent:?} switched");
        assert_eq!(world.keys(), NOTHING);

        // No marks yet: nothing to prove the session with.
        let world = two_tabs(1);
        world.place_is(Place { agent, state_exact: exact, marks: Vec::new(), ..tabs_place() });
        *world.screen.borrow_mut() = Some(MARK.into());
        assert_eq!(outcome(&world), Outcome::Refused(Refused::Unproven), "{agent:?} no marks");

        let world = two_tabs(1);
        world.place_is(Place { agent, state_exact: exact, ..tabs_place() });
        *world.screen.borrow_mut() = Some(MARK.into());
        let answer = sent(&world);
        assert_eq!(answer.outcome, Outcome::Sent, "{agent:?} shown");
        assert_eq!(answer.trail.proof, "title_text");
        assert_eq!(
            world.events(&["mark", "read", "paste", "enter", "unmark"]),
            ["mark", "read", "paste", "enter", "unmark"]
        );
    }
}

#[test]
fn a_place_refusal_names_where_it_runs() {
    let place_target = |world: &World| {
        let answer = sent(world);
        assert_eq!(answer.outcome, Outcome::Refused(Refused::Place));
        assert_eq!(world.keys(), NOTHING);
        answer.target.map(|t| (t.app, t.kind))
    };

    // VS Code's terminal panel: its window's app.
    let world = World::new();
    *world.wins.borrow_mut() = Some(vec![files_in_front(), win(9, 170, "code", "g7linux")]);
    let mut p = place();
    p.chain = vec![ancestor(200, "bash"), ancestor(180, "code"), ancestor(170, "code")];
    world.place_is(p);
    assert_eq!(place_target(&world), Some(("Visual Studio Code".into(), "app")));

    // tmux: its server has no window.
    let world = World::new();
    let mut p = place();
    p.chain = vec![
        ancestor(200, "bash"),
        Ancestor { pid: 180, start_ticks: 1, comm: "tmux: server".into(), exe_name: "tmux".into() },
    ];
    world.place_is(p);
    assert_eq!(place_target(&world), Some(("tmux".into(), "terminal")));

    // SSH.
    let world = World::new();
    let mut p = place();
    p.chain = vec![ancestor(200, "bash"), ancestor(190, "sshd-session")];
    world.place_is(p);
    assert_eq!(place_target(&world), Some(("SSH".into(), "terminal")));

    // A terminal G7 doesn't type into.
    let world = World::new();
    *world.wins.borrow_mut() = Some(vec![files_in_front(), win(WINDOW, EMULATOR, "kitty", "claude")]);
    let mut p = place();
    p.chain[2] = ancestor(EMULATOR, "kitty");
    world.place_is(p);
    assert_eq!(place_target(&world), Some(("kitty".into(), "terminal")));

    // No terminal, but the window it's under still names it.
    let world = World::new();
    world.place_is(Place { tty: None, ..place() });
    assert_eq!(place_target(&world), Some(("Terminal".into(), "terminal")));

    // The host found no process: nothing to name.
    let world = World::new();
    world.places(vec![Ok(Found::Elsewhere)]);
    assert_eq!(place_target(&world), None);
    // Nor when nothing above it is known.
    let world = World::new();
    world.place_is(Place { tty: None, chain: vec![ancestor(200, "bash")], ..place() });
    assert_eq!(place_target(&world), None);
}

#[test]
fn marks_match_across_wrapped_lines() {
    assert!(shows_a_mark("⏺ The quick brown\n  fox jumps over the\n  lazy dog.", &[MARK.into()]));
    assert!(!shows_a_mark("The quick brown cat", &[MARK.into()]));
    assert!(!shows_a_mark("anything at all", &["any".into()]), "a short mark proves nothing");
    assert!(!shows_a_mark("anything at all", &[]));
}

// ---- The title and the tabs (`plans/reply-tabs-idle.md` §2, §3) ----

#[test]
fn a_new_session_in_the_tab_in_front_is_proven_by_its_title() {
    // Brand new: no text to prove it with, Claude by its status file.
    let world = two_tabs(1);
    world.place_is(Place { marks: Vec::new(), ptys_under: vec![(EMULATOR, 2)], ..place() });
    let answer = sent(&world);
    assert_eq!(answer.outcome, Outcome::Sent);
    assert_eq!(
        world.events(&["mark", "unmark", "select", "back", "paste", "enter"]),
        ["mark", "paste", "enter", "unmark"]
    );
    assert_eq!(answer.trail, Trail { proof: "title", tab: "front", steps: 0, back: "given" });
    assert!(!world.log().contains(&"read".to_owned()), "no screen is read");
    assert_eq!(world.title_of(TTY), CLAUDE_TITLE, "the title is put back");
}

#[test]
fn a_session_in_a_background_tab_is_switched_to() {
    let world = two_tabs(0);
    let answer = sent(&world);
    assert_eq!(answer.outcome, Outcome::Sent);
    assert_eq!(
        world.events(&["mark", "tabs", "activate", "select", "back", "paste", "enter", "unmark"]),
        ["mark", "tabs", "activate 7", "select", "paste", "enter", "unmark", "activate 1"],
        "found by its title, its window forward, its tab selected, then the keys, then the focus back"
    );
    assert_eq!(answer.trail, Trail { proof: "title", tab: "switched", steps: 1, back: "given" });
    assert_eq!(world.selected(WINDOW), 1, "the terminal stays on the session's tab");
    assert_eq!(world.title_of(TTY), CLAUDE_TITLE);
}

#[test]
fn an_ended_sessions_old_tab_never_gets_the_reply() {
    // §1.4: the tab in front shows the session's text from before it ended
    // there; the session runs on in the tab behind it, on another pty.
    let world = two_tabs(0);
    world.tabbed(vec![(WINDOW, vec![tab(OTHER_TTY, CLAUDE_TITLE), tab(TTY, CLAUDE_TITLE)], 0)]);
    *world.screen.borrow_mut() = Some(MARK.into());
    assert_eq!(outcome(&world), Outcome::Sent);
    assert_eq!(world.events(&["select", "paste", "enter"]), ["select", "paste", "enter"]);
    assert_eq!(world.selected(WINDOW), 1);

    // The tab can't be selected: nothing typed anywhere.
    let world = two_tabs(0);
    world.tabbed(vec![(WINDOW, vec![tab(OTHER_TTY, CLAUDE_TITLE), tab(TTY, CLAUDE_TITLE)], 0)]);
    *world.screen.borrow_mut() = Some(MARK.into());
    world.select_ok.set(false);
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Tab));
    assert_eq!(world.keys(), NOTHING);
    assert!(!world.log().contains(&"primary".to_owned()));
}

#[test]
fn a_tab_that_cant_be_brought_forward_is_tab() {
    let world = two_tabs(0);
    world.select_ok.set(false);
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Tab));
    assert!(world.activated(), "the window stays in front");
    assert_eq!(world.keys(), NOTHING);
    assert_eq!(world.selected(WINDOW), 0);

    // More tabs than G7 steps through: nothing is selected.
    let world = two_tabs(0);
    let mut tabs: Vec<Tab> = (0..17).map(|i| tab(OTHER_TTY + 1 + i, "bash")).collect();
    tabs[16] = tab(TTY, CLAUDE_TITLE);
    world.tabbed(vec![(WINDOW, tabs, 0)]);
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Tab));
    assert!(!world.log().contains(&"select".to_owned()));
    assert_eq!(world.keys(), NOTHING);

    // Selected, but the window never shows the title.
    let world = two_tabs(0);
    *world.after_select.borrow_mut() = Some(Box::new(|w: &World| w.focus(1)));
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Tab));
    assert_eq!(world.keys(), NOTHING);
    assert_eq!(world.count("back 0"), 1);
}

#[test]
fn a_title_that_never_shows_is_unproven() {
    // A tab with a fixed title ("Set Title"): a second title, then no.
    let world = two_tabs(0);
    world.title_shows.set(false);
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Unproven));
    assert_eq!(world.events(&["mark", "unmark"]), ["mark", "unmark", "mark", "unmark"]);
    assert_eq!(world.keys(), NOTHING);
    assert!(!world.activated());
    let written = world.written.borrow().clone();
    assert_ne!(written[0], written[1], "a new title the second time");

    // Tabs that can't be read are no proof either.
    let world = two_tabs(0);
    world.tabs_readable.set(false);
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Unproven));
    assert_eq!(world.count("unmark"), 2);
    assert_eq!(world.keys(), NOTHING);
}

#[test]
fn a_terminal_that_cant_be_titled_is_unproven() {
    let world = two_tabs(1);
    world.title_ok.set(false);
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Unproven));
    assert_eq!(world.events(&["mark", "unmark"]), ["mark"], "nothing to put back");
    assert_eq!(world.keys(), NOTHING);
    assert!(!world.activated());
}

#[test]
fn of_several_windows_the_one_whose_tab_has_the_title() {
    let two = |first: &str, second: &str| {
        let world = World::new();
        *world.wins.borrow_mut() = Some(vec![
            files_in_front(),
            win(WINDOW, EMULATOR, "org.gnome.Ptyxis", ""),
            win(8, EMULATOR, "org.gnome.Ptyxis", ""),
        ]);
        world.tabbed(vec![
            (WINDOW, vec![tab(THIRD_TTY, first)], 0),
            (8, vec![tab(OTHER_TTY, second), tab(TTY, CLAUDE_TITLE)], 0),
        ]);
        let mut p = place();
        p.ptys_under = vec![(EMULATOR, 3)];
        world.place_is(p);
        world
    };

    // In a background tab of the second window: that window, then the tab.
    let world = two("build", "notes");
    assert_eq!(outcome(&world), Outcome::Sent);
    assert!(world.log().contains(&"activate 8".to_owned()));
    assert!(!world.log().contains(&"activate 7".to_owned()));
    assert_eq!(world.selected(8), 1);

    // Two windows with the same title: G7 can't tell which holds the tab.
    let world = two("notes", "notes");
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Ambiguous));
    assert!(!world.activated(), "G7 never cycles through windows to look");
    assert_eq!(world.keys(), NOTHING);
    assert_eq!(world.count("unmark"), 1);

    // The title on two windows at once: ambiguous.
    let world = two("build", "notes");
    world.tabbed(vec![(WINDOW, vec![tab(TTY, CLAUDE_TITLE)], 0), (8, vec![tab(TTY, CLAUDE_TITLE)], 0)]);
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Ambiguous));
    assert!(!world.activated());
    assert_eq!(world.keys(), NOTHING);
}

#[test]
fn a_switch_then_a_refusal_goes_back_once() {
    let cases: [(&str, Hook, Outcome); 3] = [
        (
            "locked at the gate",
            Box::new(|w: &World| w.locked_is(vec![Some(false), Some(false), Some(true)])),
            Outcome::Refused(Refused::Locked),
        ),
        ("PRIMARY refused", Box::new(|w: &World| w.primary_ok.set(false)), Outcome::Refused(Refused::Setup)),
        (
            "focus lost before the paste",
            Box::new(|w: &World| *w.after_primary.borrow_mut() = Some(Box::new(|w: &World| w.focus(1)))),
            Outcome::Failed(Failed::Focus),
        ),
    ];
    for (case, setup, expected) in cases {
        let world = two_tabs(0);
        setup(&world);
        assert_eq!(outcome(&world), expected, "{case}");
        assert_eq!(world.keys(), NOTHING, "{case}");
        assert_eq!(world.events(&["select", "back"]), ["select", "back 0"], "{case}");
        assert_eq!(world.selected(WINDOW), 0, "{case}: the tab that was in front");
        assert_eq!(world.events(&["back", "unmark"]), ["back 0", "unmark"], "{case}: back while titled");
    }

    // After the paste, the pasted text stays in view.
    let world = two_tabs(0);
    *world.after_paste.borrow_mut() = Some(Box::new(|w: &World| w.focus(1)));
    assert_eq!(outcome(&world), Outcome::Failed(Failed::Interrupted));
    assert_eq!(world.keys(), ["paste"]);
    assert!(world.events(&["back"]).is_empty());
    assert_eq!(world.selected(WINDOW), 1);
}

#[test]
fn another_process_on_the_terminal_before_the_paste_is_no_process() {
    for again in [
        Place { tty: Some(OTHER_TTY), ..tabs_place() },
        Place { pid: 301, ..tabs_place() },
        Place { start_ticks: 9999, ..tabs_place() },
    ] {
        let world = two_tabs(0);
        world.places(vec![Ok(Found::Place(tabs_place())), Ok(Found::Place(again.clone()))]);
        assert_eq!(outcome(&world), Outcome::Refused(Refused::NoProcess), "{again:?}");
        assert_eq!(world.keys(), NOTHING);
        assert!(!world.log().contains(&"primary".to_owned()));
        assert_eq!(world.count("back 0"), 1);
    }
    // The structural proof looks too.
    let world = World::new();
    world.places(vec![Ok(Found::Place(place())), Ok(Found::Ended)]);
    assert_eq!(outcome(&world), Outcome::Refused(Refused::NoProcess));
    assert_eq!(world.keys(), NOTHING);
    // The same process, now asking a question: its own reason.
    let world = two_tabs(1);
    world.places(vec![
        Ok(Found::Place(tabs_place())),
        Ok(Found::Place(Place { state: Now::NeedsYou, ..tabs_place() })),
    ]);
    assert_eq!(outcome(&world), Outcome::Refused(Refused::NeedsYou));
    assert_eq!(world.keys(), NOTHING);
}

#[test]
fn the_title_is_put_back_on_every_way_out() {
    let cases: [(&str, Hook); 15] = [
        ("sent", Box::new(|_: &World| {})),
        ("tab not selected", Box::new(|w: &World| w.select_ok.set(false))),
        ("window doesn't come forward", Box::new(|w: &World| w.activate_focuses.set(false))),
        ("window can't be activated", Box::new(|w: &World| w.activate_ok.set(false))),
        ("locked at the gate", Box::new(|w: &World| w.locked_is(vec![Some(false), Some(false), Some(true)]))),
        ("PRIMARY refused", Box::new(|w: &World| w.primary_ok.set(false))),
        (
            "focus lost before the paste",
            Box::new(|w: &World| *w.after_primary.borrow_mut() = Some(Box::new(|w: &World| w.focus(1)))),
        ),
        (
            "focus lost after the paste",
            Box::new(|w: &World| *w.after_paste.borrow_mut() = Some(Box::new(|w: &World| w.focus(1)))),
        ),
        ("paste failed", Box::new(|w: &World| w.paste_ok.set(false))),
        ("enter failed", Box::new(|w: &World| w.enter_ok.set(false))),
        (
            "locked after the paste",
            Box::new(|w: &World| w.locked_is(vec![Some(false), Some(false), Some(false), Some(true)])),
        ),
        (
            "process changed",
            Box::new(|w: &World| w.places(vec![Ok(Found::Place(tabs_place())), Ok(Found::Ended)])),
        ),
        (
            "codex without its text",
            Box::new(|w: &World| {
                w.place_is(Place { agent: Agent::Codex, state_exact: false, ..tabs_place() })
            }),
        ),
        ("fixed title", Box::new(|w: &World| w.title_shows.set(false))),
        (
            "too slow",
            Box::new(|w: &World| {
                *w.after_select.borrow_mut() = Some(Box::new(|w: &World| w.pass(Duration::from_secs(7))))
            }),
        ),
    ];
    for (case, setup) in cases {
        for selected in [0, 1] {
            let world = two_tabs(selected);
            setup(&world);
            let _ = sent(&world);
            let titles = world.events(&["mark", "unmark"]);
            assert!(!titles.is_empty(), "{case}");
            assert_eq!(titles.last().map(String::as_str), Some("unmark"), "{case} {selected}");
            assert_eq!(world.count("mark"), world.count("unmark"), "{case} {selected}");
            assert_eq!(world.title_of(TTY), CLAUDE_TITLE, "{case} {selected}");
        }
    }
}

#[test]
fn past_the_deadline_it_refuses_the_step_its_in() {
    // Selecting the tab took too long.
    let world = two_tabs(0);
    *world.after_select.borrow_mut() = Some(Box::new(|w: &World| w.pass(Duration::from_secs(7))));
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Tab));
    assert_eq!(world.keys(), NOTHING);
    assert_eq!(world.count("back 0"), 1);

    // The title step ran past it: no second title is tried.
    let world = two_tabs(0);
    world.title_shows.set(false);
    *world.after_mark.borrow_mut() = Some(Box::new(|w: &World| w.pass(Duration::from_secs(7))));
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Unproven));
    assert_eq!(world.events(&["mark", "unmark"]), ["mark", "unmark"]);
    assert!(!world.log().contains(&"tabs".to_owned()), "no tabs are looked at past it");
    assert_eq!(world.keys(), NOTHING);
}

#[test]
fn a_check_titles_reads_and_selects_nothing() {
    let worlds: [(&str, World); 7] = [
        ("one window", World::new()),
        ("its tab in front", two_tabs(1)),
        ("its tab behind", two_tabs(0)),
        ("a fixed title", {
            let w = two_tabs(0);
            w.title_shows.set(false);
            w
        }),
        ("codex", {
            let w = two_tabs(1);
            w.place_is(Place { agent: Agent::Codex, state_exact: false, ..tabs_place() });
            w
        }),
        ("no pty to title", {
            let w = two_tabs(0);
            w.title_ok.set(false);
            w
        }),
        ("someone at the PC", {
            let w = two_tabs(0);
            w.free.set(Some(Duration::from_secs(2)));
            w
        }),
    ];
    for (case, world) in worlds {
        let answer = answer(&world.pc(), &Replies::default(), PHONE, &check_ask()).expect("an answer");
        assert_eq!(answer.outcome, Outcome::Ready, "{case}");
        assert!(
            world
                .events(&["mark", "unmark", "tabs", "select", "back", "read", "activate", "primary"])
                .is_empty(),
            "{case}: {:?}",
            world.log()
        );
        assert_eq!(world.keys(), NOTHING);
    }
}

// ---- During the entry (§2.4 steps 7 to 9) ----

#[test]
fn focus_lost_before_the_paste_types_nothing() {
    let world = World::new();
    *world.after_primary.borrow_mut() = Some(Box::new(|w: &World| w.focus(1)));
    assert_eq!(outcome(&world), Outcome::Failed(Failed::Focus));
    assert_eq!(world.keys(), NOTHING);
    assert_eq!(*world.primary.borrow(), None, "PRIMARY cleared");
}

#[test]
fn focus_lost_between_the_paste_and_enter_is_interrupted() {
    let world = World::new();
    *world.after_paste.borrow_mut() = Some(Box::new(|w: &World| w.focus(1)));
    assert_eq!(outcome(&world), Outcome::Failed(Failed::Interrupted));
    assert_eq!(world.keys(), ["paste"]);
    assert_eq!(*world.primary.borrow(), None);
}

#[test]
fn a_question_after_the_paste_never_gets_enter() {
    let world = World::new();
    world.places(vec![
        Ok(Found::Place(place())),
        Ok(Found::Place(place())),
        Ok(Found::Place(Place { state: Now::NeedsYou, ..place() })),
    ]);
    assert_eq!(outcome(&world), Outcome::Failed(Failed::Interrupted));
    assert_eq!(world.keys(), ["paste"]);
}

#[test]
fn the_process_gone_after_the_paste_is_interrupted() {
    for after in [
        Ok(Found::Ended),
        Ok(Found::Place(Place { start_ticks: 9999, ..place() })),
        Ok(Found::Place(Place { tty: Some(OTHER_TTY), ..place() })),
        Ok(Found::Place(Place { foreground: false, ..place() })),
        Err(HostDown),
    ] {
        let world = World::new();
        world.places(vec![Ok(Found::Place(place())), Ok(Found::Place(place())), after.clone()]);
        assert_eq!(outcome(&world), Outcome::Failed(Failed::Interrupted), "{after:?}");
        assert_eq!(world.keys(), ["paste"]);
    }
}

#[test]
fn a_key_that_cant_be_pressed_is_failed_input() {
    let world = World::new();
    world.paste_ok.set(false);
    assert_eq!(outcome(&world), Outcome::Failed(Failed::Input));
    assert_eq!(world.keys(), ["paste"]);
    assert_eq!(*world.primary.borrow(), None);

    let world = World::new();
    world.enter_ok.set(false);
    assert_eq!(outcome(&world), Outcome::Failed(Failed::Input));
    assert_eq!(world.keys(), ["paste", "enter"]);
}

#[test]
fn primary_is_left_alone_once_its_not_ours() {
    let world = World::new();
    // The owner selected something while it was pasted.
    *world.after_paste.borrow_mut() =
        Some(Box::new(|w: &World| *w.primary.borrow_mut() = Some("selected by hand".into())));
    assert_eq!(outcome(&world), Outcome::Sent);
    assert_eq!(world.primary.borrow().as_deref(), Some("selected by hand"));
}

// ---- Giving the focus back (`plans/move-and-focus.md` §3) ----

/// The file manager, which had the focus before the reply.
const BEFORE: u64 = 1;

/// What happened after Enter.
fn after_enter(world: &World) -> Vec<String> {
    let log = world.log();
    let at = log.iter().rposition(|e| e == "enter").expect("Enter was pressed");
    log[at + 1..].to_vec()
}

fn focused(world: &World) -> Option<u64> {
    world.list().unwrap_or_default().iter().find(|w| w.focused).map(|w| w.id)
}

#[test]
fn after_a_reply_the_focus_goes_back_to_the_window_that_had_it() {
    let world = World::new();
    let answer = sent(&world);
    assert_eq!(answer.outcome, Outcome::Sent);
    assert_eq!(after_enter(&world), ["clear", "wait 300", "activate 1"], "300 ms after Enter");
    assert_eq!(answer.trail.back, "given");
    assert_eq!(focused(&world), Some(BEFORE));

    // With a title: put back first, then the focus.
    let world = two_tabs(0);
    let answer = sent(&world);
    assert_eq!(answer.trail.back, "given");
    assert_eq!(after_enter(&world), ["clear", "unmark", "wait 300", "activate 1"]);
    assert_eq!(world.selected(WINDOW), 1, "the terminal stays on the session's tab behind it");
}

#[test]
fn nothing_is_given_back_when_nothing_else_had_the_focus() {
    // Nothing focused before (the desktop, the overview, the launcher).
    let world = World::new();
    world.focus(0);
    let answer = sent(&world);
    assert_eq!(answer.outcome, Outcome::Sent);
    assert_eq!(world.events(&["activate", "wait"]), ["activate 7", "wait 250"]);
    assert_eq!(answer.trail.back, "");
    // The terminal itself had it.
    let world = World::new();
    world.focus(WINDOW);
    let answer = sent(&world);
    assert_eq!(world.events(&["activate", "wait"]), ["activate 7", "wait 250"]);
    assert_eq!(answer.trail.back, "");
}

#[test]
fn a_window_that_went_meanwhile_is_never_activated() {
    let replaced: [(&str, Hook); 3] = [
        (
            "closed",
            Box::new(|w: &World| w.wins.borrow_mut().as_mut().expect("windows").retain(|w| w.id != BEFORE)),
        ),
        (
            "its id another app's window",
            Box::new(|w: &World| {
                for win in
                    w.wins.borrow_mut().as_mut().expect("windows").iter_mut().filter(|w| w.id == BEFORE)
                {
                    win.pid = 51;
                }
            }),
        ),
        (
            "another class",
            Box::new(|w: &World| {
                for win in
                    w.wins.borrow_mut().as_mut().expect("windows").iter_mut().filter(|w| w.id == BEFORE)
                {
                    win.class = "org.example.Other".into();
                }
            }),
        ),
    ];
    for (case, change) in replaced {
        let world = World::new();
        *world.after_paste.borrow_mut() = Some(change);
        let answer = sent(&world);
        assert_eq!(answer.outcome, Outcome::Sent, "{case}");
        assert_eq!(answer.trail.back, "gone", "{case}");
        assert_eq!(world.events(&["activate"]), ["activate 7"], "{case}");
    }
}

#[test]
fn locked_or_logind_silent_after_enter_gives_nothing_back() {
    for lock in [Some(true), None] {
        let world = World::new();
        world.locked_is(vec![Some(false), Some(false), Some(false), lock]);
        let answer = sent(&world);
        assert_eq!(answer.outcome, Outcome::Sent, "{lock:?}");
        assert_eq!(answer.trail.back, "kept", "{lock:?}");
        assert_eq!(world.events(&["activate"]), ["activate 7"], "{lock:?}");
    }
}

#[test]
fn someone_at_the_pc_since_the_reply_began_keeps_the_terminal() {
    // Each case's hooks: before the keys, and after the paste.
    type Hooks = (Option<Hook>, Option<Hook>);
    type Case = (&'static str, fn() -> Hooks);
    let cases: [Case; 4] = [
        ("touched after the paste", || {
            (None, Some(Box::new(|w: &World| w.free.set(Some(Duration::from_secs(4))))))
        }),
        ("a phone typing", || (None, Some(Box::new(|w: &World| w.typing.set(true))))),
        ("the desktop stopped saying", || (None, Some(Box::new(|w: &World| w.free.set(None))))),
        ("touched before the keys, then only G7's own", || {
            (
                Some(Box::new(|w: &World| w.free.set(Some(Duration::from_secs(4))))),
                Some(Box::new(|w: &World| w.free.set(Some(Duration::ZERO)))),
            )
        }),
    ];
    for (case, hooks) in cases {
        let world = World::new();
        let (before_keys, after_paste) = hooks();
        *world.after_primary.borrow_mut() = before_keys;
        *world.after_paste.borrow_mut() = after_paste;
        let answer = sent(&world);
        assert_eq!(answer.outcome, Outcome::Sent, "{case}");
        assert_eq!(answer.trail.back, "kept", "{case}");
        assert_eq!(world.events(&["activate"]), ["activate 7"], "{case}");
        assert_eq!(focused(&world), Some(WINDOW), "{case}");
    }
}

#[test]
fn a_focus_moved_meanwhile_stays_where_it_went() {
    let world = World::new();
    if let Some(wins) = world.wins.borrow_mut().as_mut() {
        wins.push(win(3, 60, "org.gnome.TextEditor", "notes"));
    }
    *world.after_enter.borrow_mut() = Some(Box::new(|w: &World| w.focus(3)));
    let answer = sent(&world);
    assert_eq!(answer.outcome, Outcome::Sent);
    assert_eq!(answer.trail.back, "kept");
    assert_eq!(world.events(&["activate"]), ["activate 7"]);
    assert_eq!(focused(&world), Some(3));
}

#[test]
fn text_that_may_sit_in_the_box_keeps_the_terminal_in_front() {
    // A question came between the paste and Enter.
    let world = World::new();
    world.places(vec![
        Ok(Found::Place(place())),
        Ok(Found::Place(place())),
        Ok(Found::Place(Place { state: Now::NeedsYou, ..place() })),
    ]);
    let answer = sent(&world);
    assert_eq!(answer.outcome, Outcome::Failed(Failed::Interrupted));
    assert_eq!(answer.trail.back, "kept");
    assert_eq!(world.events(&["activate", "wait"]), ["activate 7", "wait 250"], "no wait, no activate");
    // Enter couldn't be pressed.
    let world = World::new();
    world.enter_ok.set(false);
    let answer = sent(&world);
    assert_eq!(answer.outcome, Outcome::Failed(Failed::Input));
    assert_eq!(answer.trail.back, "kept");
    assert_eq!(world.events(&["activate"]), ["activate 7"]);
}

#[test]
fn a_refusal_after_the_terminal_came_forward_gives_the_focus_back_at_once() {
    let cases: [(&str, World, Outcome); 3] = [
        (
            "locked at the gate, unlocked again",
            {
                let w = two_tabs(0);
                w.locked_is(vec![Some(false), Some(false), Some(true), Some(false)]);
                w
            },
            Outcome::Refused(Refused::Locked),
        ),
        (
            "PRIMARY refused",
            {
                let w = World::new();
                w.primary_ok.set(false);
                w
            },
            Outcome::Refused(Refused::Setup),
        ),
        (
            "another process before the paste",
            {
                let w = World::new();
                w.places(vec![Ok(Found::Place(place())), Ok(Found::Ended)]);
                w
            },
            Outcome::Refused(Refused::NoProcess),
        ),
    ];
    for (case, world, expected) in cases {
        let answer = sent(&world);
        assert_eq!(answer.outcome, expected, "{case}");
        assert_eq!(answer.trail.back, "given", "{case}");
        assert_eq!(world.keys(), NOTHING, "{case}");
        assert!(!world.log().contains(&"wait 300".to_owned()), "{case}: nothing was typed to wait for");
        assert_eq!(world.log().last().map(String::as_str), Some("activate 1"), "{case}");
    }
}

#[test]
fn a_terminal_that_was_minimized_is_minimized_again_once_the_focus_is_back() {
    let minimized = || {
        let world = World::new();
        if let Some(wins) = world.wins.borrow_mut().as_mut() {
            wins[1].minimized = true;
        }
        world
    };
    let world = minimized();
    assert_eq!(sent(&world).trail.back, "given");
    assert_eq!(after_enter(&world), ["clear", "wait 300", "activate 1", "minimize 7"]);

    // The focus didn't arrive: the terminal stays as it is, nothing is tried again.
    let world = minimized();
    world.stuck.borrow_mut().push(BEFORE);
    assert_eq!(sent(&world).trail.back, "failed");
    let after = after_enter(&world);
    assert_eq!(after.iter().filter(|e| e.starts_with("activate")).count(), 1);
    assert_eq!(after.iter().filter(|e| *e == "wait 50").count(), 10, "half a second");
    assert!(!after.iter().any(|e| e.starts_with("minimize")));

    // A terminal that wasn't minimized isn't minimized.
    let world = World::new();
    sent(&world);
    assert!(!world.log().iter().any(|e| e.starts_with("minimize")));
}

#[test]
fn a_window_on_another_workspace_is_left_alone() {
    for workspace in [Some(1), Some(-1), None] {
        let world = World::new();
        if let Some(wins) = world.wins.borrow_mut().as_mut() {
            wins[0].workspace = workspace;
        }
        let answer = sent(&world);
        assert_eq!(answer.outcome, Outcome::Sent, "{workspace:?}");
        assert_eq!(answer.trail.back, "kept", "{workspace:?}");
        assert_eq!(world.events(&["activate"]), ["activate 7"], "{workspace:?}");
    }
}

// ---- What the PC offers (`plans/move-and-focus.md` §7.1) ----

#[test]
fn what_a_pc_offers_is_found_by_capability() {
    use g7link_core::{
        agent_move::reply_needs,
        session_move,
        session_reply::{self, needs},
    };
    assert_eq!(Setup::of(false, false, false), Setup::Desktop);
    assert_eq!(Setup::of(false, true, true), Setup::Desktop, "no shell is no window backend");
    assert_eq!(Setup::of(true, false, true), Setup::Extension);
    assert_eq!(Setup::of(true, false, false), Setup::Extension);
    assert_eq!(Setup::of(true, true, false), Setup::Keyboard);
    assert_eq!(Setup::of(true, true, true), Setup::Ready);

    let four = [session_reply::CAP, needs::DESKTOP, needs::EXTENSION, needs::KEYBOARD];
    for (setup, word, need) in [
        (Setup::Ready, session_reply::CAP, None),
        (Setup::Desktop, needs::DESKTOP, Some(needs::DESKTOP)),
        (Setup::Extension, needs::EXTENSION, Some(needs::EXTENSION)),
        (Setup::Keyboard, needs::KEYBOARD, Some(needs::KEYBOARD)),
    ] {
        for host_moves in [false, true] {
            let caps = capabilities(setup, host_moves);
            assert_eq!(caps.iter().filter(|c| four.contains(c)).count(), 1, "{setup:?}: one of the four");
            assert!(caps.contains(&word), "{setup:?}");
            assert_eq!(reply_needs(|c| caps.contains(&c)), need, "{setup:?}: as a phone reads it");
            assert_eq!(
                caps.contains(&session_move::CAP),
                setup == Setup::Ready && host_moves,
                "{setup:?} {host_moves}: a move only beside a reply"
            );
        }
    }
}

#[path = "move_tests.rs"]
mod moves;
