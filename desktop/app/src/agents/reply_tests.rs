// SPDX-License-Identifier: GPL-3.0-or-later
//! The reply sequence on a fake PC: one world plays the host, the windows,
//! the keyboard and the desk, and records every call, so each test says
//! which keys were pressed. No real window, session, clipboard or keyboard.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::*;

const PHONE: &str = "phone-a";
const SESSION: &str = "sc0123";
const MARK: &str = "Thequickbrownfoxjumpsoverthelazydog";
/// Ptyxis (pid 100) with one window (7); the agent (pid 300) under bash
/// (200) and ptyxis-agent (150); the owner's file manager (pid 50, window 1)
/// in front.
const EMULATOR: u32 = 100;
const WINDOW: u64 = 7;

type Hook = Box<dyn Fn(&World)>;

struct World {
    log: RefCell<Vec<String>>,
    places: RefCell<VecDeque<Result<Found, HostDown>>>,
    windows_ready: Cell<bool>,
    wins: RefCell<Option<Vec<Win>>>,
    activate_ok: Cell<bool>,
    activate_focuses: Cell<bool>,
    frames: RefCell<Option<Vec<Frame>>>,
    screen: RefCell<Option<String>>,
    primary: RefCell<Option<String>>,
    primary_ok: Cell<bool>,
    app: RefCell<Option<String>>,
    keys_ready: Cell<bool>,
    paste_ok: Cell<bool>,
    enter_ok: Cell<bool>,
    locked: RefCell<VecDeque<Option<bool>>>,
    owner: Cell<Option<bool>>,
    typing: Cell<bool>,
    /// The window comes forward after this many looks.
    focus_after_waits: Cell<Option<usize>>,
    after_primary: RefCell<Option<Hook>>,
    after_paste: RefCell<Option<Hook>>,
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
        tty: Some(34816),
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
    }
}

fn files_in_front() -> Win {
    Win { focused: true, ..win(1, 50, "org.gnome.Nautilus", "Home") }
}

impl World {
    fn new() -> Self {
        World {
            log: RefCell::new(Vec::new()),
            places: RefCell::new(VecDeque::from([Ok(Found::Place(place()))])),
            windows_ready: Cell::new(true),
            wins: RefCell::new(Some(vec![
                files_in_front(),
                win(WINDOW, EMULATOR, "org.gnome.Ptyxis", "claude"),
            ])),
            activate_ok: Cell::new(true),
            activate_focuses: Cell::new(true),
            frames: RefCell::new(None),
            screen: RefCell::new(None),
            primary: RefCell::new(Some("the owner's selection".into())),
            primary_ok: Cell::new(true),
            app: RefCell::new(Some("Terminal".into())),
            keys_ready: Cell::new(true),
            paste_ok: Cell::new(true),
            enter_ok: Cell::new(true),
            locked: RefCell::new(VecDeque::from([Some(false)])),
            owner: Cell::new(Some(false)),
            typing: Cell::new(false),
            focus_after_waits: Cell::new(None),
            after_primary: RefCell::new(None),
            after_paste: RefCell::new(None),
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

    /// Several tabs: the content proof reads the window's terminal.
    fn tabs(&self, screen: &str) {
        let mut p = place();
        p.ptys_under = vec![(200, 1), (150, 3), (EMULATOR, 3)];
        self.place_is(p);
        *self.frames.borrow_mut() =
            Some(vec![Frame { title: "claude".into(), active: false, text: screen.into() }]);
        *self.screen.borrow_mut() = Some(screen.into());
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

    /// The keys pressed, in order.
    fn keys(&self) -> Vec<String> {
        self.log().into_iter().filter(|e| e == "paste" || e == "enter").collect()
    }

    fn activated(&self) -> bool {
        self.log().iter().any(|e| e.starts_with("activate"))
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
        next(&self.places)
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
        if self.activate_ok.get() && self.activate_focuses.get() {
            self.focus(id);
        }
        self.activate_ok.get()
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

    fn terminal_frames(&self, pid: u32) -> Option<Vec<Frame>> {
        (pid == EMULATOR).then(|| self.frames.borrow().clone()).flatten()
    }

    fn app_name(&self, class: &str, _pid: u32) -> Option<String> {
        match class {
            "code" => Some("Visual Studio Code".into()),
            "kitty" => Some("kitty".into()),
            _ => self.app.borrow().clone(),
        }
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
        self.enter_ok.get()
    }
}

impl Desk for World {
    fn locked(&self) -> Option<bool> {
        next(&self.locked)
    }

    fn owner_active(&self) -> Option<bool> {
        self.owner.get()
    }

    fn phone_typing(&self) -> bool {
        self.typing.get()
    }

    fn wait(&self, how_long: Duration) {
        self.note(format!("wait {}", how_long.as_millis()));
        let looks = self.log().iter().filter(|e| *e == "wait 50").count();
        if self.focus_after_waits.get() == Some(looks) {
            self.focus(WINDOW);
        }
    }
}

fn reply(text: &str) -> Ask {
    Ask { session: SESSION.into(), text: Some(text.into()), check: false, confirm: None }
}

fn check_ask() -> Ask {
    Ask { session: SESSION.into(), text: None, check: true, confirm: None }
}

/// A ledger that already confirmed window 7 of this Ptyxis for this phone
/// and session.
fn confirmed() -> Replies {
    confirmed_window(WINDOW)
}

fn confirmed_window(window: u64) -> Replies {
    let replies = Replies::default();
    let bound = Bound {
        phone: PHONE.into(),
        session: SESSION.into(),
        window,
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
    // One window, one terminal: no screen is read.
    assert_eq!(
        world.log(),
        ["place", "activate 7", "primary", "paste", "wait 250", "place", "enter", "clear"],
        "PRIMARY is set before the chord and cleared after Enter"
    );
    assert_eq!(*world.primary.borrow(), None);
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
    let waits = world.log().iter().filter(|e| *e == "wait 50").count();
    assert_eq!(waits, 3);
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
fn a_changed_window_asks_again() {
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
fn a_nonce_works_once_for_its_window_and_expires() {
    let bound = Bound {
        phone: PHONE.into(),
        session: SESSION.into(),
        window: WINDOW,
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
        Bound { window: 8, ..bound.clone() },
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
    world.owner.set(Some(true));
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
    let frame = Frame { title: "claude".into(), active: true, text: MARK.into() };
    assert!(!format!("{frame:?}").contains(MARK));
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
        // Codex, and Claude without its status file, are proven by the screen.
        *world.frames.borrow_mut() =
            Some(vec![Frame { title: "claude".into(), active: false, text: MARK.into() }]);
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
    for (owner, typing) in [(Some(true), false), (None, false), (Some(false), true)] {
        let world = World::new();
        world.owner.set(owner);
        world.typing.set(typing);
        assert_eq!(outcome(&world), Outcome::Refused(Refused::InUse), "{owner:?} {typing}");
        assert!(!world.activated());
        assert_eq!(world.keys(), NOTHING);
    }
}

#[test]
fn a_fullscreen_window_in_front_is_in_use() {
    let world = World::new();
    if let Some(wins) = world.wins.borrow_mut().as_mut() {
        wins[0].fullscreen = true;
    }
    assert_eq!(outcome(&world), Outcome::Refused(Refused::InUse));
    assert_eq!(world.keys(), NOTHING);

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
    // One window, three tabs: the screen is read.
    let world = World::new();
    world.tabs("⏺ The quick brown fox\n  jumps over the lazy dog.\n> ");
    assert_eq!(outcome(&world), Outcome::Sent);
    assert!(world.log().contains(&"read".to_owned()));

    // Two windows, one terminal each.
    let world = World::new();
    *world.wins.borrow_mut() = Some(vec![
        files_in_front(),
        win(WINDOW, EMULATOR, "org.gnome.Ptyxis", "claude"),
        win(8, EMULATOR, "org.gnome.Ptyxis", "notes"),
    ]);
    *world.frames.borrow_mut() = Some(vec![
        Frame { title: "claude".into(), active: false, text: MARK.into() },
        Frame { title: "notes".into(), active: false, text: "vim".into() },
    ]);
    *world.screen.borrow_mut() = Some(MARK.into());
    let mut p = place();
    p.ptys_under = vec![(200, 1), (150, 2), (EMULATOR, 2)];
    world.place_is(p);
    assert_eq!(outcome(&world), Outcome::Sent);
    assert!(world.log().contains(&"activate 7".to_owned()));
}

#[test]
fn codex_and_a_claude_without_its_status_file_are_proven_by_the_screen() {
    // `codex resume <uuid>` keeps the id on its command line after `/resume`
    // inside it: one window and one terminal don't say which session shows.
    for (agent, exact) in [(Agent::Codex, false), (Agent::Claude, false)] {
        let world = World::new();
        world.place_is(Place { agent, state_exact: exact, ..place() });
        assert_eq!(outcome(&world), Outcome::Refused(Refused::Ambiguous), "{agent:?} unread");
        assert_eq!(world.keys(), NOTHING);

        let world = World::new();
        world.place_is(Place { agent, state_exact: exact, ..place() });
        *world.frames.borrow_mut() =
            Some(vec![Frame { title: "claude".into(), active: false, text: MARK.into() }]);
        *world.screen.borrow_mut() = Some("› /resume\n  another session".into());
        assert_eq!(outcome(&world), Outcome::Refused(Refused::Tab), "{agent:?} switched");
        assert_eq!(world.keys(), NOTHING);

        let world = World::new();
        world.place_is(Place { agent, state_exact: exact, ..place() });
        *world.frames.borrow_mut() =
            Some(vec![Frame { title: "claude".into(), active: false, text: MARK.into() }]);
        *world.screen.borrow_mut() = Some(MARK.into());
        assert_eq!(outcome(&world), Outcome::Sent, "{agent:?} shown");
        assert!(world.log().contains(&"read".to_owned()));
    }
    // A count the host couldn't make proves nothing either.
    let world = World::new();
    let mut p = place();
    p.ptys_under.retain(|(pid, _)| *pid != EMULATOR);
    world.place_is(p);
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Ambiguous));
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

#[test]
fn a_background_tab_is_refused_tab() {
    // Seen before bringing it forward: nothing activated.
    let world = World::new();
    world.tabs("~/src $ ls\nCargo.toml");
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Tab));
    assert!(!world.activated());
    assert_eq!(world.keys(), NOTHING);

    // Changed after: the window stays in front, nothing typed.
    let world = World::new();
    world.tabs(MARK);
    *world.screen.borrow_mut() = Some("~/src $ ".into());
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Tab));
    assert!(world.activated());
    assert_eq!(world.keys(), NOTHING);

    // No marks yet: nothing to prove with.
    let world = World::new();
    world.tabs(MARK);
    let mut p = place();
    p.ptys_under = vec![(EMULATOR, 3)];
    p.marks.clear();
    world.place_is(p);
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Tab));
}

#[test]
fn a_screen_that_cant_be_read_is_ambiguous() {
    // No accessible terminal before (another app's, or none).
    let world = World::new();
    world.tabs(MARK);
    *world.frames.borrow_mut() = None;
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Ambiguous));
    let world = World::new();
    world.tabs(MARK);
    *world.frames.borrow_mut() = Some(Vec::new());
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Ambiguous));
    assert!(!world.activated());

    // After: the focused object isn't a terminal of this app.
    let world = World::new();
    world.tabs(MARK);
    *world.screen.borrow_mut() = None;
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Ambiguous));
    assert_eq!(world.keys(), NOTHING);
}

#[test]
fn of_several_windows_the_one_in_front_or_the_only_one_showing_it() {
    let two = || {
        let world = World::new();
        *world.wins.borrow_mut() = Some(vec![
            win(WINDOW, EMULATOR, "org.gnome.Ptyxis", "claude"),
            win(8, EMULATOR, "org.gnome.Ptyxis", "build"),
        ]);
        let mut p = place();
        p.ptys_under = vec![(EMULATOR, 2)];
        world.place_is(p);
        *world.screen.borrow_mut() = Some(MARK.into());
        world
    };

    // The one in front shows it: no other is looked for.
    let world = two();
    world.focus(8);
    *world.frames.borrow_mut() = Some(vec![
        Frame { title: "claude".into(), active: false, text: MARK.into() },
        Frame { title: "build".into(), active: true, text: MARK.into() },
    ]);
    let answer = send(&world, &confirmed_window(8), "hello").expect("an answer");
    assert_eq!(answer.outcome, Outcome::Sent);
    assert!(world.log().contains(&"activate 8".to_owned()));

    // Behind: the only window showing it, by its title.
    let world = two();
    *world.frames.borrow_mut() = Some(vec![
        Frame { title: "claude".into(), active: false, text: MARK.into() },
        Frame { title: "build".into(), active: false, text: "cargo".into() },
    ]);
    assert_eq!(outcome(&world), Outcome::Sent);
    assert!(world.log().contains(&"activate 7".to_owned()));

    // Two showing it, or none.
    for texts in [[MARK, MARK], ["cargo", "make"]] {
        let world = two();
        *world.frames.borrow_mut() = Some(vec![
            Frame { title: "claude".into(), active: false, text: texts[0].into() },
            Frame { title: "build".into(), active: false, text: texts[1].into() },
        ]);
        assert_eq!(outcome(&world), Outcome::Refused(Refused::Ambiguous));
        assert!(!world.activated(), "G7 never cycles through windows to look");
    }

    // Two windows with the same title.
    let world = two();
    if let Some(wins) = world.wins.borrow_mut().as_mut() {
        wins[1].title = "claude".into();
    }
    *world.frames.borrow_mut() =
        Some(vec![Frame { title: "claude".into(), active: false, text: MARK.into() }]);
    assert_eq!(outcome(&world), Outcome::Refused(Refused::Ambiguous));
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
    world
        .places(vec![Ok(Found::Place(place())), Ok(Found::Place(Place { state: Now::NeedsYou, ..place() }))]);
    assert_eq!(outcome(&world), Outcome::Failed(Failed::Interrupted));
    assert_eq!(world.keys(), ["paste"]);
}

#[test]
fn the_process_gone_after_the_paste_is_interrupted() {
    for after in [
        Ok(Found::Ended),
        Ok(Found::Place(Place { start_ticks: 9999, ..place() })),
        Ok(Found::Place(Place { foreground: false, ..place() })),
        Err(HostDown),
    ] {
        let world = World::new();
        world.places(vec![Ok(Found::Place(place())), after.clone()]);
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
