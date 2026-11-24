// SPDX-License-Identifier: GPL-3.0-or-later
//! A phone's reply entered into a session started on this PC
//! (`plans/reply-on-pc.md`, `plans/reply-tabs-idle.md`, docs/protocol/agents.md
//! §9): the PC finds the session's terminal, brings its window and tab
//! forward, proves that the terminal in front is the session's, pastes the
//! text as one bracketed paste (PRIMARY and Shift+Insert) and presses Enter.
//!
//! Refusing is the default: each step is a check that must pass, and what
//! isn't known is a refusal with a reason the phone can show. Nothing is
//! typed before the proof; nothing but the paste chord and Enter is ever
//! pressed, and nothing is clicked or typed as a probe. When the terminal
//! may show another tab or window, G7 writes a one-time title to the
//! session's own terminal device (the output side: the agent never reads
//! it), finds the window or tab showing that title, and selects that tab
//! through the accessibility bus. The title is put back on every way out.
//! Someone at the PC is `in_use` with how long to wait, so a phone can send
//! the reply again once the PC is free. Once the reply is in, the focus goes
//! back to the window that had it, when nobody touched the PC meanwhile
//! (`plans/move-and-focus.md` §3). Moving a session to G7 ([`move_here`])
//! types nothing at all: the agent host ends the session's proven process.
//!
//! The sequence runs over four small traits ([`Host`], [`Windows`],
//! [`Keys`], [`Desk`]) so every refusal is tested on fakes; the live ones
//! are at the end. The text is never logged or stored: it lives in memory
//! for one request, and on PRIMARY until Enter.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use g7_agents::{
    api::{self, ErrorCode, PlaceRefusal, SessionPlaceResult, SessionSource, SessionState, cap},
    client::{Client, ClientError},
};
use g7link_core::{AgentFailure, AgentReply, AgentReplyTarget, AgentSessionReplied, DeviceId};

use crate::sys::text_focus::{MAX_TABS, TabError, TabRoute, TabSpot};

/// Most bytes in a reply, after it's cleaned.
pub(crate) const MAX_TEXT: usize = 8192;
/// How long the window has to come forward.
const FOCUS_WAIT: Duration = Duration::from_secs(1);
const FOCUS_POLL: Duration = Duration::from_millis(50);
/// Between the paste and the checks before Enter (S5: Claude's status file
/// says `waiting` within about 50 ms of a question).
const SETTLE: Duration = Duration::from_millis(250);
/// A title G7 wrote has this long to show on a window or a tab (S6).
const TITLE_WAIT: Duration = Duration::from_millis(600);
/// The window in front has this long to show the title once its tab is selected.
const TAB_WAIT: Duration = Duration::from_millis(800);
/// From the checks before bringing anything forward to the proof's end.
const BEFORE_PASTE: Duration = Duration::from_secs(6);
/// Bytes of a window title the window backend keeps.
const WINDOW_TITLE_BYTES: usize = 256;
/// Input at the PC this recent is someone there (§3.2).
pub(crate) const OWNER_AWAY: Duration = Duration::from_secs(5);
/// `retry_ms` with `in_use`: past the owner's [`OWNER_AWAY`], while a
/// phone's input is typed, and behind a fullscreen window; within these.
const IN_USE_MARGIN: Duration = Duration::from_millis(100);
const PHONE_TYPING_WAIT: Duration = Duration::from_secs(1);
const FULLSCREEN_WAIT: Duration = Duration::from_secs(5);
const MIN_IN_USE_MS: u128 = 250;
const MAX_IN_USE_MS: u128 = 5000;
/// A confirmation nonce works once, this long.
const NONCE_LIFE: Duration = Duration::from_secs(120);
/// A confirmation is remembered this long.
const CONFIRMED_FOR: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_NONCES: usize = 32;
const MAX_CONFIRMED: usize = 256;
const MAX_PHONES: usize = 16;
/// Rate (§3.5): checks a second, one looked-at attempt a second, one entered
/// reply per session in [`SESSION_GAP`], and entered replies a minute and an hour.
const CHECKS_PER_SECOND: usize = 2;
const SESSION_GAP: Duration = Duration::from_secs(3);
const PER_MINUTE: usize = 6;
const PER_HOUR: usize = 60;
const SECOND: Duration = Duration::from_secs(1);
const MINUTE: Duration = Duration::from_secs(60);
const HOUR: Duration = Duration::from_secs(60 * 60);
const MAX_RETRY_MS: u64 = 60_000;
/// A mark shorter than this, without whitespace, proves nothing.
const MIN_MARK_CHARS: usize = 16;
/// Bytes in a target's `app`, `agent` and `folder`.
const MAX_TARGET_BYTES: usize = 64;
/// After Enter, before the focus goes back: the key reaches the terminal first.
const GIVE_BACK_AFTER: Duration = Duration::from_millis(300);
/// The window that had the focus has this long to take it back.
const BACK_WAIT: Duration = Duration::from_millis(500);

// ---- What the request and the answer are ----

/// A phone's `agent.session.reply`.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Ask {
    pub session: String,
    pub text: Option<String>,
    pub check: bool,
    pub confirm: Option<String>,
}

impl std::fmt::Debug for Ask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ask")
            .field("session", &self.session)
            .field("text_len", &self.text.as_ref().map(String::len))
            .field("check", &self.check)
            .field("confirm", &self.confirm.is_some())
            .finish()
    }
}

/// Where a reply goes, as the phone shows it: "Terminal · Claude · g7linux".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub app: String,
    pub agent: String,
    pub folder: String,
    pub kind: &'static str,
}

/// Nothing was typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refused {
    Locked,
    InUse,
    Confirm,
    NeedsYou,
    Busy,
    Ended,
    NoProcess,
    NoWindow,
    Ambiguous,
    /// In a background tab G7 couldn't bring forward.
    Tab,
    /// G7 couldn't make sure which terminal is the session's.
    Unproven,
    Place,
    Text,
    Rate,
    Setup,
    /// A move's: the session's folder isn't one of the agent folders.
    Folder,
    /// A move's: the agent can't run on this PC now.
    Agent,
}

impl Refused {
    pub(crate) fn word(self) -> &'static str {
        match self {
            Refused::Locked => "locked",
            Refused::InUse => "in_use",
            Refused::Confirm => "confirm",
            Refused::NeedsYou => "needs_you",
            Refused::Busy => "busy",
            Refused::Ended => "ended",
            Refused::NoProcess => "no_process",
            Refused::NoWindow => "no_window",
            Refused::Ambiguous => "ambiguous",
            Refused::Tab => "tab",
            Refused::Unproven => "unproven",
            Refused::Place => "place",
            Refused::Text => "text",
            Refused::Rate => "rate",
            Refused::Setup => "setup",
            Refused::Folder => "folder",
            Refused::Agent => "agent",
        }
    }
}

/// The reply didn't go through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Failed {
    /// The window didn't come forward: nothing was typed.
    Focus,
    /// The text was pasted and not sent.
    Interrupted,
    /// A key couldn't be pressed; the paste may have landed.
    Input,
}

impl Failed {
    pub(crate) fn word(self) -> &'static str {
        match self {
            Failed::Focus => "focus",
            Failed::Interrupted => "interrupted",
            Failed::Input => "input",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// A check found nothing in the way.
    Ready,
    /// Enter was pressed in the proven window.
    Sent,
    Refused(Refused),
    Failed(Failed),
}

impl Outcome {
    pub(crate) fn result(self) -> &'static str {
        match self {
            Outcome::Ready => "ready",
            Outcome::Sent => "sent",
            Outcome::Refused(_) => "refused",
            Outcome::Failed(_) => "failed",
        }
    }

    pub(crate) fn reason(self) -> Option<&'static str> {
        match self {
            Outcome::Ready | Outcome::Sent => None,
            Outcome::Refused(r) => Some(r.word()),
            Outcome::Failed(f) => Some(f.word()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Answer {
    pub outcome: Outcome,
    pub target: Option<Target>,
    pub nonce: Option<String>,
    pub retry_ms: Option<u32>,
    /// The terminal's executable, for the log line only.
    pub emulator: Option<String>,
    pub trail: Trail,
}

/// How a reply was proven and brought forward, for the log line only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Trail {
    /// `structural`, `title`, `title_text`, or empty before a proof began.
    pub proof: &'static str,
    /// `front` (its tab was in front), `switched`, or empty.
    pub tab: &'static str,
    /// Tabs moved past when switching.
    pub steps: usize,
    /// The focus afterwards: `given` back to the window that had it, `kept`
    /// on the terminal (a rule said no), `gone` (that window isn't there),
    /// `failed` (it didn't take the focus), or empty: nothing was brought
    /// forward, or nothing else had the focus.
    pub back: &'static str,
}

impl Answer {
    fn of(outcome: Outcome) -> Self {
        Answer { outcome, target: None, nonce: None, retry_ms: None, emulator: None, trail: Trail::default() }
    }

    fn refused(reason: Refused) -> Self {
        Answer::of(Outcome::Refused(reason))
    }

    fn failed(reason: Failed) -> Self {
        Answer::of(Outcome::Failed(reason))
    }

    /// Where it was for: the target, and the terminal for the log.
    fn at(mut self, resolved: &Resolved) -> Self {
        self.target = Some(resolved.target.clone());
        self.emulator = Some(resolved.emulator.exe_name.clone());
        self
    }
}

/// The request itself can't be answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Error {
    /// The text breaks §2.5, or both or neither of `text` and `check`.
    BadMessage,
    /// No such session.
    NotFound,
    /// Another reply is being entered on this PC.
    Busy,
    /// The agent host couldn't be asked.
    Internal,
}

// ---- What the sequence asks of the PC ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Agent {
    Claude,
    Codex,
}

impl Agent {
    fn name(self) -> &'static str {
        match self {
            Agent::Claude => "Claude",
            Agent::Codex => "Codex",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    Terminal,
    DesktopApp,
    Other,
}

/// The session's state as followed (§2.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Now {
    Idle,
    Working,
    /// The agent reported it waits for the user.
    NeedsYou,
    /// Guessed from the file.
    MayNeedYou,
    Ended,
}

/// A process above the agent: its parent, grandparent, and so on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ancestor {
    pub pid: u32,
    pub start_ticks: u64,
    pub comm: String,
    /// The file name of its executable, never a path.
    pub exe_name: String,
}

/// Where a session's process runs, from the host's `session.place`.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Place {
    pub agent: Agent,
    pub source: Source,
    pub state: Now,
    /// The state came from Claude's status file, not a guess.
    pub state_exact: bool,
    pub pid: u32,
    pub start_ticks: u64,
    /// The agent's terminal (`tty_nr`); `None`: it has none.
    pub tty: Option<u32>,
    /// The agent is its terminal's foreground job.
    pub foreground: bool,
    /// Parent first, up to 32.
    pub chain: Vec<Ancestor>,
    /// Per ancestor pid: how many distinct terminals its descendants hold.
    pub ptys_under: Vec<(u32, u32)>,
    /// Runs of the session's newest text, whitespace removed. Never logged.
    pub marks: Vec<String>,
    /// The folder's label, never a path.
    pub folder: String,
    /// What would still stop continuing it once this process is gone, from
    /// a host that says (`cap::MOVE`).
    pub continue_block: Option<Blocked>,
}

/// Why a session couldn't be continued as a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Blocked {
    /// Something keeps it open with no trace (a session of the Codex app).
    Open,
    /// Its folder isn't one of the agent folders.
    Folder,
    /// The agent isn't installed or signed in.
    Agent,
}

impl std::fmt::Debug for Place {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Place")
            .field("agent", &self.agent)
            .field("source", &self.source)
            .field("state", &self.state)
            .field("state_exact", &self.state_exact)
            .field("pid", &self.pid)
            .field("tty", &self.tty)
            .field("foreground", &self.foreground)
            .field("chain", &self.chain.len())
            .field("marks", &self.marks.len())
            .field("continue_block", &self.continue_block)
            .finish_non_exhaustive()
    }
}

/// What the host says about a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Found {
    Place(Place),
    /// No session with this id.
    Unknown,
    /// No process has it.
    Ended,
    /// Two live processes have it.
    Ambiguous,
    /// It runs somewhere G7 can't reply into: the Claude or Codex app, or a
    /// Codex that doesn't name its session.
    Elsewhere,
}

/// The agent host couldn't be asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HostDown;

pub(crate) trait Host {
    fn place(&self, session: &str) -> Result<Found, HostDown>;
}

/// A window as the window backend lists it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Win {
    pub id: u64,
    pub pid: u32,
    pub class: String,
    /// Compared in memory to find a window, never logged.
    pub title: String,
    pub focused: bool,
    pub fullscreen: bool,
    pub minimized: bool,
    /// A normal window, not a dialog or a menu.
    pub normal: bool,
    /// Its workspace's index, -1 on every workspace; `None` when not said.
    pub workspace: Option<i64>,
}

impl std::fmt::Debug for Win {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Win")
            .field("id", &self.id)
            .field("pid", &self.pid)
            .field("class", &self.class)
            .field("focused", &self.focused)
            .field("fullscreen", &self.fullscreen)
            .field("minimized", &self.minimized)
            .field("normal", &self.normal)
            .field("workspace", &self.workspace)
            .finish_non_exhaustive()
    }
}

/// The window backend: windows, focus, PRIMARY, a terminal's title, its
/// tabs and its text.
pub(crate) trait Windows {
    /// Lists windows with `fullscreen` and `minimized`, and writes PRIMARY
    /// alone (the companion extension's additions).
    fn ready(&self) -> bool;
    /// `None` when the backend didn't answer.
    fn list(&self) -> Option<Vec<Win>>;
    fn activate(&self, id: u64) -> bool;
    fn minimize(&self, id: u64) -> bool;
    fn set_primary(&self, text: &str) -> bool;
    /// Clears PRIMARY if it still holds `expected`.
    fn clear_primary(&self, expected: &str);
    /// The text of the focused terminal in the active window of the app
    /// with `pid`; `None` when there's no such terminal or it can't be read.
    fn terminal_text(&self, pid: u32) -> Option<String>;
    /// The app's name from its desktop entry.
    fn app_name(&self, class: &str, pid: u32) -> Option<String>;
    /// Sets `title` on the terminal `tty` (written to its device, the output
    /// side), pushing the old one; false when it couldn't.
    fn title_mark(&self, tty: u32, title: &str) -> bool;
    /// Puts back the title [`Windows::title_mark`] pushed.
    fn title_unmark(&self, tty: u32);
    /// Turns off the mouse reporting an agent that exited may have left on
    /// the terminal `tty`, and shows its cursor (written to its device, the
    /// output side); false when it couldn't.
    fn terminal_reset(&self, tty: u32) -> bool;
    /// Where tabs titled with `mark` are among the windows of the app with
    /// `pid`, read without changing anything; `None` when it can't be read.
    fn tab_titled(&self, pid: u32, mark: &str, route: TabRoute) -> Option<Vec<TabSpot>>;
    /// In the app's active window, selects the tab titled with `mark`;
    /// `Ok` holds the tab selected before.
    fn select_tab(&self, pid: u32, mark: &str, route: TabRoute) -> Result<usize, TabError>;
    /// Selects tab `at` again in the window whose tab is titled with `mark`.
    fn select_tab_at(&self, pid: u32, mark: &str, at: usize, route: TabRoute) -> bool;
}

/// The keys a reply presses.
pub(crate) trait Keys {
    /// The virtual keyboard exists (made now when it doesn't).
    fn ready(&self) -> bool;
    /// Shift+Insert: pastes PRIMARY in the terminal.
    fn paste(&self) -> bool;
    fn enter(&self) -> bool;
}

/// The PC around the window: the lock, the owner, the clock.
pub(crate) trait Desk {
    /// `None` when logind didn't say.
    fn locked(&self) -> Option<bool>;
    /// How long until nobody will have used this PC's keyboard or mouse for
    /// [`OWNER_AWAY`], if nobody touches them: zero when nobody has (a
    /// phone's own input doesn't count); `None` when the desktop didn't say.
    fn free_in(&self) -> Option<Duration>;
    /// A phone's input is being typed now.
    fn phone_typing(&self) -> bool;
    fn wait(&self, how_long: Duration);
    /// The clock a reply's deadline runs on.
    fn now(&self) -> Instant;
}

/// The four, together.
#[derive(Debug)]
pub(crate) struct Pc<'a, H, W, K, D> {
    pub host: &'a H,
    pub windows: &'a W,
    pub keys: &'a K,
    pub desk: &'a D,
}

// ---- The text (§2.5) ----

/// The text cleaned for a terminal: CRLF and CR to LF, a tab to four
/// spaces, trailing whitespace trimmed. `Err(BadMessage)` for an empty or
/// too long text, a control character other than LF (no `ESC` can end the
/// paste early) or a bidirectional control; `Ok(Err(Text))` when it starts
/// with `!` or `/`, which the agent reads as a command (§9.1).
pub(crate) fn clean(text: &str) -> Result<Result<String, Refused>, Error> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n").replace('\t', "    ");
    let text = text.trim_end();
    if text.trim_start().is_empty() || text.len() > MAX_TEXT {
        return Err(Error::BadMessage);
    }
    if text.chars().any(|c| (c.is_control() && c != '\n') || is_bidi_control(c)) {
        return Err(Error::BadMessage);
    }
    if text.trim_start().starts_with(['!', '/']) {
        return Ok(Err(Refused::Text));
    }
    Ok(Ok(text.to_owned()))
}

fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

// ---- Rate and confirmations (§3.3, §3.5) ----

/// What a confirmation holds for: this phone, this session, this run of
/// the agent in this run of this terminal. Not the window: none is chosen
/// before the phone's user confirms, and a tab moved to another window of
/// the same terminal is still the session's.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Bound {
    phone: String,
    session: String,
    agent: u32,
    agent_start: u64,
    emulator: u32,
    emulator_start: u64,
}

impl Bound {
    /// The same phone, session and run of the agent: a move ends that
    /// process, whichever window shows it.
    fn same_run(&self, other: &Bound) -> bool {
        self.phone == other.phone
            && self.session == other.session
            && self.agent == other.agent
            && self.agent_start == other.agent_start
    }
}

#[derive(Debug)]
struct Nonce {
    value: String,
    bound: Bound,
    at: Instant,
}

#[derive(Debug, Default)]
struct Pace {
    checks: VecDeque<Instant>,
    attempt: Option<Instant>,
    /// Replies entered in the last hour, and their session.
    entered: VecDeque<(Instant, String)>,
}

/// The PC's memory of replies: nonces, confirmations and the pace of each
/// phone. In memory only: a restart asks again.
#[derive(Debug, Default)]
pub(crate) struct Ledger {
    nonces: VecDeque<Nonce>,
    confirmed: VecDeque<(Bound, Instant)>,
    phones: VecDeque<(String, Pace)>,
}

fn wait_ms(wait: Duration) -> u32 {
    u32::try_from(wait.as_millis().clamp(1, u128::from(MAX_RETRY_MS))).unwrap_or(1)
}

impl Ledger {
    fn pace(&mut self, phone: &str) -> &mut Pace {
        if let Some(at) = self.phones.iter().position(|(p, _)| p == phone) {
            let entry = self.phones.remove(at).expect("an index just found");
            self.phones.push_back(entry);
        } else {
            if self.phones.len() >= MAX_PHONES {
                self.phones.pop_front();
            }
            self.phones.push_back((phone.to_owned(), Pace::default()));
        }
        &mut self.phones.back_mut().expect("just pushed").1
    }

    /// At most [`CHECKS_PER_SECOND`] checks a second; `Err` holds how long to wait.
    fn check(&mut self, phone: &str, now: Instant) -> Result<(), Duration> {
        let pace = self.pace(phone);
        pace.checks.retain(|at| now.saturating_duration_since(*at) < SECOND);
        if pace.checks.len() >= CHECKS_PER_SECOND {
            let oldest = pace.checks.front().copied().unwrap_or(now);
            return Err(SECOND.saturating_sub(now.saturating_duration_since(oldest)));
        }
        pace.checks.push_back(now);
        Ok(())
    }

    /// Whether an attempt to enter a reply is looked at now: one a second
    /// (but `exempt`, a send with a live confirmation), and within what was
    /// entered lately.
    fn attempt(&mut self, phone: &str, session: &str, now: Instant, exempt: bool) -> Result<(), Duration> {
        let pace = self.pace(phone);
        if !exempt
            && let Some(last) = pace.attempt
            && now.saturating_duration_since(last) < SECOND
        {
            return Err(SECOND - now.saturating_duration_since(last));
        }
        pace.attempt = Some(now);
        pace.entered.retain(|(at, _)| now.saturating_duration_since(*at) < HOUR);
        let since = |at: &Instant| now.saturating_duration_since(*at);
        if let Some((at, _)) = pace.entered.iter().rev().find(|(_, s)| s == session)
            && since(at) < SESSION_GAP
        {
            return Err(SESSION_GAP - since(at));
        }
        let last_minute: Vec<&Instant> =
            pace.entered.iter().map(|(at, _)| at).filter(|at| since(at) < MINUTE).collect();
        if last_minute.len() >= PER_MINUTE {
            return Err(MINUTE.saturating_sub(since(last_minute[last_minute.len() - PER_MINUTE])));
        }
        if pace.entered.len() >= PER_HOUR {
            return Err(HOUR.saturating_sub(since(&pace.entered[pace.entered.len() - PER_HOUR].0)));
        }
        Ok(())
    }

    /// A reply was entered: keys were pressed.
    fn entered(&mut self, phone: &str, session: &str, now: Instant) {
        let pace = self.pace(phone);
        pace.entered.push_back((now, session.to_owned()));
        while pace.entered.len() > PER_HOUR {
            pace.entered.pop_front();
        }
    }

    fn prune(&mut self, now: Instant) {
        self.nonces.retain(|n| now.saturating_duration_since(n.at) < NONCE_LIFE);
        self.confirmed.retain(|(_, at)| now.saturating_duration_since(*at) < CONFIRMED_FOR);
    }

    fn is_confirmed(&mut self, bound: &Bound, now: Instant) -> bool {
        self.prune(now);
        self.confirmed.iter().any(|(b, _)| b == bound)
    }

    /// A new one-time nonce for `bound`.
    fn mint(&mut self, bound: Bound, now: Instant) -> String {
        self.prune(now);
        let value = data_encoding::HEXLOWER.encode(&rand::random::<[u8; 16]>());
        if self.nonces.len() >= MAX_NONCES {
            self.nonces.pop_front();
        }
        self.nonces.push_back(Nonce { value: value.clone(), bound, at: now });
        value
    }

    /// Whether `nonce` is live for this phone and session (it may not match
    /// the process: that's for [`Ledger::redeem`] to say).
    fn holds(&mut self, phone: &str, session: &str, nonce: &str, now: Instant) -> bool {
        self.prune(now);
        self.nonces.iter().any(|n| n.value == nonce && n.bound.phone == phone && n.bound.session == session)
    }

    /// Spends `nonce` (once, matching or not); true when it was for
    /// `bound`, which is then confirmed.
    fn redeem(&mut self, nonce: &str, bound: &Bound, now: Instant) -> bool {
        let matched = self.spend(nonce, now).is_some_and(|spent| spent == *bound);
        if matched {
            self.confirm(bound, now);
        }
        matched
    }

    /// Whether this phone confirmed this run of the agent in this session,
    /// in any window: what a move asks.
    fn run_confirmed(&mut self, bound: &Bound, now: Instant) -> bool {
        self.prune(now);
        self.confirmed.iter().any(|(b, _)| b.same_run(bound))
    }

    /// [`Ledger::redeem`] for a move: the nonce must be for the same run of
    /// the agent, for this phone and session.
    fn redeem_run(&mut self, nonce: &str, bound: &Bound, now: Instant) -> bool {
        let matched = self.spend(nonce, now).is_some_and(|spent| spent.same_run(bound));
        if matched {
            self.confirm(bound, now);
        }
        matched
    }

    /// Removes `nonce`, giving back what it was for.
    fn spend(&mut self, nonce: &str, now: Instant) -> Option<Bound> {
        self.prune(now);
        let at = self.nonces.iter().position(|n| n.value == nonce)?;
        self.nonces.remove(at).map(|spent| spent.bound)
    }

    fn confirm(&mut self, bound: &Bound, now: Instant) {
        self.confirmed.retain(|(b, _)| !(b.phone == bound.phone && b.session == bound.session));
        if self.confirmed.len() >= MAX_CONFIRMED {
            self.confirmed.pop_front();
        }
        self.confirmed.push_back((bound.clone(), now));
    }
}

/// The PC's replies: the ledger, and the one reply being entered.
#[derive(Debug, Default)]
pub(crate) struct Replies {
    ledger: Mutex<Ledger>,
    entering: Mutex<()>,
}

impl Replies {
    fn ledger(&self) -> std::sync::MutexGuard<'_, Ledger> {
        self.ledger.lock().unwrap_or_else(|e| e.into_inner())
    }
}

// ---- Finding the window (§1.3, §1.4, §1.5, §2.2) ----

/// A terminal G7 knows how to paste into: its executable, its window
/// classes, the name shown when its desktop entry can't be read, and how
/// its tabs are switched.
#[derive(Debug)]
struct Terminal {
    exe: &'static str,
    classes: &'static [&'static str],
    name: &'static str,
    tabs: TabRoute,
}

/// v1 (§9.1): the VTE terminals, where Shift+Insert pastes PRIMARY.
const TERMINALS: [Terminal; 2] = [
    Terminal {
        exe: "ptyxis",
        classes: &["org.gnome.Ptyxis", "org.gnome.Ptyxis.Devel", "ptyxis"],
        name: "Ptyxis",
        tabs: TabRoute::AdwPages,
    },
    Terminal {
        exe: "gnome-terminal-server",
        classes: &["org.gnome.Terminal", "gnome-terminal", "gnome-terminal-server"],
        name: "Terminal",
        tabs: TabRoute::Notebook,
    },
];

/// Above the agent, these keep G7 from the window: a multiplexer, a remote
/// shell or an editor's terminal panel (§1.3).
const IN_BETWEEN: [&str; 11] = [
    "tmux",
    "zellij",
    "screen",
    "sshd",
    "sshd-session",
    "mosh-server",
    "code",
    "code-insiders",
    "codium",
    "cursor",
    "windsurf",
];

/// The program of [`IN_BETWEEN`] an ancestor is.
fn in_between(ancestor: &Ancestor) -> Option<&'static str> {
    let comm = ancestor.comm.split(':').next().unwrap_or_default().trim();
    IN_BETWEEN.iter().copied().find(|name| ancestor.exe_name == *name || comm == *name)
}

/// The editors of [`IN_BETWEEN`]: their terminal is a panel of an app.
const EDITORS: [&str; 5] = ["code", "code-insiders", "codium", "cursor", "windsurf"];

/// How the phone names a program of [`IN_BETWEEN`] that has no window of
/// its own to name it by.
fn program_name(program: &str) -> &'static str {
    match program {
        "tmux" => "tmux",
        "zellij" => "Zellij",
        "screen" => "Screen",
        "sshd" | "sshd-session" => "SSH",
        "mosh-server" => "Mosh",
        "code" => "VS Code",
        "code-insiders" => "VS Code Insiders",
        "codium" => "VSCodium",
        "cursor" => "Cursor",
        _ => "Windsurf",
    }
}

fn known_terminal(exe_name: &str, class: &str) -> Option<&'static Terminal> {
    TERMINALS.iter().find(|t| t.exe == exe_name && t.classes.iter().any(|c| c.eq_ignore_ascii_case(class)))
}

/// Whether the terminal's text shows one of the session's marks. Both are
/// compared without whitespace, so line wrapping can't break a match.
pub(crate) fn shows_a_mark(screen: &str, marks: &[String]) -> bool {
    let flat: String = screen.chars().filter(|c| !c.is_whitespace()).collect();
    marks.iter().any(|mark| {
        let mark: String = mark.chars().filter(|c| !c.is_whitespace()).collect();
        mark.chars().count() >= MIN_MARK_CHARS && flat.contains(&mark)
    })
}

/// The terminal that holds the session, and when it can only be showing
/// the session's terminal (§1.4), its one window.
#[derive(Debug, Clone)]
struct Resolved {
    /// `Some` only for the structural proof; else the title finds it.
    window: Option<u64>,
    /// Its first window's class.
    class: String,
    emulator: Ancestor,
    terminal: &'static Terminal,
    target: Target,
}

/// Claude with a live status file: the host found the session exactly,
/// not by a command line that may have switched sessions inside.
fn by_status_file(place: &Place) -> bool {
    place.agent == Agent::Claude && place.state_exact
}

fn cut(text: &str, max: usize) -> String {
    let end = (0..=max.min(text.len())).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
    text[..end].to_owned()
}

/// A folder's label for the phone: never a path.
fn folder_label(folder: &str) -> String {
    cut(folder.rsplit('/').find(|part| !part.is_empty()).unwrap_or_default(), MAX_TARGET_BYTES)
}

/// Steps 2 and 4's checks of the session itself: open, one process, in
/// a terminal, at a prompt.
fn session_allows(place: &Place) -> Result<(), Refused> {
    if place.source != Source::Terminal {
        return Err(Refused::Place);
    }
    match place.state {
        Now::Ended => return Err(Refused::Ended),
        Now::NeedsYou | Now::MayNeedYou => return Err(Refused::NeedsYou),
        Now::Idle => {}
        // Claude queues a message sent mid-turn (O2), when its status file
        // says it's working; a guessed state is idle only (§9.1, S5). Codex
        // reports nothing a file reader can trust mid-turn.
        Now::Working if place.agent == Agent::Claude && place.state_exact => {}
        Now::Working => return Err(Refused::Busy),
    }
    if place.tty.is_none() {
        return Err(Refused::Place);
    }
    if !place.foreground {
        return Err(Refused::NoProcess);
    }
    Ok(())
}

/// Step 2: the session's one process, or why there's none.
fn placed<H: Host>(host: &H, session: &str) -> Result<Result<Place, Answer>, Error> {
    Ok(match host.place(session).map_err(|HostDown| Error::Internal)? {
        Found::Place(place) => Ok(place),
        Found::Unknown => return Err(Error::NotFound),
        Found::Ended => Err(Answer::refused(Refused::Ended)),
        Found::Ambiguous => Err(Answer::refused(Refused::Ambiguous)),
        Found::Elsewhere => Err(Answer::refused(Refused::Place)),
    })
}

/// Nobody at the PC, nothing in front that mustn't be covered. `Err` is
/// `in_use` with how long until waiting may help; `None`: it won't.
fn pc_allows<D: Desk>(desk: &D, windows: &[Win], target_pid: Option<u32>) -> Result<(), Option<Duration>> {
    if desk.phone_typing() {
        return Err(Some(PHONE_TYPING_WAIT));
    }
    match desk.free_in() {
        None => return Err(None),
        Some(free) if !free.is_zero() => return Err(Some(free + IN_USE_MARGIN)),
        Some(_) => {}
    }
    // A film, a presentation or a call; a fullscreen terminal holding the
    // session is fine.
    if windows.iter().any(|w| w.focused && w.fullscreen && Some(w.pid) != target_pid) {
        return Err(Some(FULLSCREEN_WAIT));
    }
    Ok(())
}

/// `retry_ms` for an `in_use` that waiting may end.
fn in_use_ms(wait: Duration) -> u32 {
    u32::try_from(wait.as_millis().clamp(MIN_IN_USE_MS, MAX_IN_USE_MS)).unwrap_or(u32::MAX)
}

/// The emulator above the agent, a terminal G7 knows, and its one window
/// when that can only be showing the session. Reads nothing from the screen.
fn resolve<W: Windows>(windows: &W, place: &Place, listed: &[Win]) -> Result<Resolved, Refused> {
    let mut emulator = None;
    for ancestor in &place.chain {
        if in_between(ancestor).is_some() {
            return Err(Refused::Place);
        }
        if listed.iter().any(|w| w.pid == ancestor.pid) {
            emulator = Some(ancestor.clone());
            break;
        }
    }
    let emulator = emulator.ok_or(Refused::NoWindow)?;
    let own: Vec<&Win> = listed.iter().filter(|w| w.pid == emulator.pid && w.normal).collect();
    let first = own.first().ok_or(Refused::NoWindow)?;
    let terminal = known_terminal(&emulator.exe_name, &first.class).ok_or(Refused::Place)?;
    if own.iter().any(|w| known_terminal(&emulator.exe_name, &w.class).is_none()) {
        return Err(Refused::Place);
    }
    let ptys = place.ptys_under.iter().find(|(pid, _)| *pid == emulator.pid).map(|(_, n)| *n);
    // A process found by its command line may have switched sessions inside
    // (Codex's `/resume`, a Claude without its status file): the title says
    // which terminal, and only the screen which session it shows.
    let structural = by_status_file(place) && own.len() == 1 && ptys == Some(1);
    let class = first.class.clone();
    let app = windows
        .app_name(&class, emulator.pid)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| terminal.name.to_owned());
    let target = target(place, &app, "terminal");
    Ok(Resolved { window: structural.then_some(first.id), class, emulator, terminal, target })
}

fn target(place: &Place, app: &str, kind: &'static str) -> Target {
    Target {
        app: cut(app, MAX_TARGET_BYTES),
        agent: place.agent.name().to_owned(),
        folder: folder_label(&place.folder),
        kind,
    }
}

/// A `place` refusal, naming where the session runs when something was
/// found above it: the app whose window that is (VS Code, a terminal G7
/// doesn't type into), or the program in between that has none (tmux,
/// SSH). The phone words its answer by it.
fn refused_place<W: Windows>(windows: &W, place: &Place, listed: Option<&[Win]>) -> Answer {
    let listed = match listed {
        Some(listed) => listed.to_vec(),
        None => windows.list().unwrap_or_default(),
    };
    Answer { target: place_target(windows, place, &listed), ..Answer::refused(Refused::Place) }
}

fn place_target<W: Windows>(windows: &W, place: &Place, listed: &[Win]) -> Option<Target> {
    let mut between = None;
    for ancestor in &place.chain {
        between = between.or(in_between(ancestor));
        let Some(window) = listed.iter().find(|w| w.pid == ancestor.pid && w.normal) else { continue };
        let app = windows
            .app_name(&window.class, window.pid)
            .filter(|name| !name.trim().is_empty())
            .or_else(|| between.map(|program| program_name(program).to_owned()))
            .unwrap_or_else(|| ancestor.exe_name.clone());
        let editor = between.is_some_and(|program| EDITORS.contains(&program))
            || EDITORS.contains(&ancestor.exe_name.as_str());
        return (!app.trim().is_empty())
            .then(|| target(place, &app, if editor { "app" } else { "terminal" }));
    }
    let program = between?;
    let kind = if EDITORS.contains(&program) { "app" } else { "terminal" };
    Some(target(place, program_name(program), kind))
}

/// The window a reply goes into.
#[derive(Debug, Clone)]
struct Front {
    id: u64,
    pid: u32,
    class: String,
}

impl Front {
    fn of(window: &Win) -> Self {
        Front { id: window.id, pid: window.pid, class: window.class.clone() }
    }

    /// The same window: its id, and the same pid and class (an id reused
    /// by another window isn't it).
    fn is(&self, window: &Win) -> bool {
        window.id == self.id && window.pid == self.pid && window.class == self.class
    }
}

/// What was in front before a reply, and what it brought forward: for
/// giving the focus back (`plans/move-and-focus.md` §3).
#[derive(Debug, Default)]
struct Shown {
    /// The window that had the focus before anything was brought forward.
    prior: Option<Front>,
    /// The windows that were minimized then.
    minimized: Vec<u64>,
    /// The window G7 brought forward, once it had the focus.
    front: Option<Front>,
    /// Someone used the PC between the first look and G7's keys.
    touched: bool,
}

/// Whether `front` has the focus, and, when G7 wrote a title, shows it.
fn focused_on<W: Windows>(windows: &W, front: &Front, mark: Option<&str>) -> bool {
    windows.list().is_some_and(|list| {
        list.iter().any(|w| {
            w.id == front.id
                && w.focused
                && w.pid == front.pid
                && w.class == front.class
                && mark.is_none_or(|mark| w.title.contains(mark))
        })
    })
}

/// Puts the terminal's title back on the way out, whichever way that is.
struct Titled<'a, W: Windows> {
    windows: &'a W,
    tty: u32,
}

impl<W: Windows> Drop for Titled<'_, W> {
    fn drop(&mut self) {
        self.windows.title_unmark(self.tty);
    }
}

impl<W: Windows> std::fmt::Debug for Titled<'_, W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Titled").finish_non_exhaustive()
    }
}

/// After G7 switched tabs, selects the tab that was selected before on the
/// way out, unless the reply was pasted: then the pasted text stays in view
/// (§3.3). Once, best effort.
struct Back<'a, W: Windows> {
    windows: &'a W,
    pid: u32,
    mark: String,
    at: usize,
    route: TabRoute,
    armed: std::cell::Cell<bool>,
}

impl<W: Windows> Back<'_, W> {
    /// The tab stays: the reply is being pasted into it.
    fn keep(&self) {
        self.armed.set(false);
    }
}

impl<W: Windows> Drop for Back<'_, W> {
    fn drop(&mut self) {
        if self.armed.get() {
            self.windows.select_tab_at(self.pid, &self.mark, self.at, self.route);
        }
    }
}

impl<W: Windows> std::fmt::Debug for Back<'_, W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Back").field("at", &self.at).field("armed", &self.armed.get()).finish_non_exhaustive()
    }
}

/// What the title proof leaves until the reply is entered. Fields drop in
/// order: the old tab is selected again while its title still names it,
/// then the title is put back.
struct Proven<'a, W: Windows> {
    front: Front,
    mark: String,
    back: Option<Back<'a, W>>,
    _titled: Titled<'a, W>,
}

impl<W: Windows> std::fmt::Debug for Proven<'_, W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Proven")
            .field("front", &self.front)
            .field("back", &self.back)
            .field("titled", &self._titled)
            .finish_non_exhaustive()
    }
}

/// Where a title G7 wrote showed up.
#[derive(Debug)]
enum Seen {
    /// On this window: its selected tab is the session's.
    Window(Win),
    /// On a tab of a window, maybe in the background.
    Tab(TabSpot),
    /// On more than one.
    Several,
    Nowhere,
}

/// A one-time title: "G7 " and 16 hex digits, never logged.
fn new_mark() -> String {
    format!("G7 {}", data_encoding::HEXLOWER.encode(&rand::random::<[u8; 8]>()))
}

/// Clears PRIMARY on the way out, if it still holds the reply.
struct Primary<'a, W: Windows> {
    windows: &'a W,
    text: &'a str,
}

impl<W: Windows> Drop for Primary<'_, W> {
    fn drop(&mut self) {
        self.windows.clear_primary(self.text);
    }
}

impl<W: Windows> std::fmt::Debug for Primary<'_, W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Primary").finish_non_exhaustive()
    }
}

// ---- The sequence (§2.4, `plans/reply-tabs-idle.md` §5) ----

/// Answers a phone's reply or check. `phone` is the asking phone's id.
pub(crate) fn answer<H: Host, W: Windows, K: Keys, D: Desk>(
    pc: &Pc<'_, H, W, K, D>,
    replies: &Replies,
    phone: &str,
    ask: &Ask,
) -> Result<Answer, Error> {
    let answer = match (&ask.text, ask.check) {
        (None, true) => check(pc, replies, phone, &ask.session)?,
        (Some(text), false) => send(pc, replies, phone, ask, text)?,
        _ => return Err(Error::BadMessage),
    };
    tracing::info!(
        session = %ask.session,
        check = ask.check,
        result = answer.outcome.result(),
        reason = answer.outcome.reason().unwrap_or_default(),
        emulator = answer.emulator.as_deref().unwrap_or_default(),
        proof = answer.trail.proof,
        tab = answer.trail.tab,
        steps = answer.trail.steps,
        back = answer.trail.back,
        "a phone's reply into a session"
    );
    Ok(answer)
}

fn rate(wait: Duration) -> Answer {
    Answer { retry_ms: Some(wait_ms(wait)), ..Answer::refused(Refused::Rate) }
}

/// The host, the session and its terminal, without bringing anything
/// forward, writing a title or reading the screen: `ready`, or why not.
/// Someone at the PC isn't a reason: a reply waits for them.
fn check<H: Host, W: Windows, K: Keys, D: Desk>(
    pc: &Pc<'_, H, W, K, D>,
    replies: &Replies,
    phone: &str,
    session: &str,
) -> Result<Answer, Error> {
    if let Err(wait) = replies.ledger().check(phone, Instant::now()) {
        return Ok(rate(wait));
    }
    let place = match placed(pc.host, session)? {
        Ok(place) => place,
        Err(answer) => return Ok(answer),
    };
    Ok(match ready(pc, &place) {
        Ok(resolved) => Answer::of(Outcome::Ready).at(&resolved),
        Err(answer) => *answer,
    })
}

/// The session's state, the lock, the window backend and the terminal.
fn ready<H: Host, W: Windows, K: Keys, D: Desk>(
    pc: &Pc<'_, H, W, K, D>,
    place: &Place,
) -> Result<Resolved, Box<Answer>> {
    let refused = |reason| Box::new(Answer::refused(reason));
    session_allows(place).map_err(|reason| match reason {
        Refused::Place => Box::new(refused_place(pc.windows, place, None)),
        reason => refused(reason),
    })?;
    // Before the extension: GNOME turns it off while the screen is locked.
    if pc.desk.locked() != Some(false) {
        return Err(refused(Refused::Locked));
    }
    if !pc.windows.ready() {
        return Err(refused(Refused::Setup));
    }
    let listed = pc.windows.list().ok_or_else(|| refused(Refused::Setup))?;
    resolve(pc.windows, place, &listed).map_err(|reason| match reason {
        Refused::Place => Box::new(refused_place(pc.windows, place, Some(&listed))),
        reason => refused(reason),
    })
}

fn send<H: Host, W: Windows, K: Keys, D: Desk>(
    pc: &Pc<'_, H, W, K, D>,
    replies: &Replies,
    phone: &str,
    ask: &Ask,
    text: &str,
) -> Result<Answer, Error> {
    // Step 1: the text, then the pace.
    let text = match clean(text)? {
        Ok(text) => text,
        Err(reason) => return Ok(Answer::refused(reason)),
    };
    let now = Instant::now();
    {
        let mut ledger = replies.ledger();
        let exempt = ask.confirm.as_deref().is_some_and(|n| ledger.holds(phone, &ask.session, n, now));
        if let Err(wait) = ledger.attempt(phone, &ask.session, now, exempt) {
            return Ok(rate(wait));
        }
    }
    let Ok(_entering) = replies.entering.try_lock() else { return Err(Error::Busy) };

    // Step 2: the host, the session and its terminal.
    let place = match placed(pc.host, &ask.session)? {
        Ok(place) => place,
        Err(answer) => return Ok(answer),
    };
    let resolved = match ready(pc, &place) {
        Ok(resolved) => resolved,
        Err(answer) => return Ok(*answer),
    };
    // The confirmation, before anything waits for someone at the PC.
    let bound = Bound {
        phone: phone.to_owned(),
        session: ask.session.clone(),
        agent: place.pid,
        agent_start: place.start_ticks,
        emulator: resolved.emulator.pid,
        emulator_start: resolved.emulator.start_ticks,
    };
    {
        let mut ledger = replies.ledger();
        let confirmed = ledger.is_confirmed(&bound, now)
            || ask.confirm.as_deref().is_some_and(|nonce| ledger.redeem(nonce, &bound, now));
        if !confirmed {
            let nonce = ledger.mint(bound, now);
            return Ok(Answer { nonce: Some(nonce), ..Answer::refused(Refused::Confirm).at(&resolved) });
        }
    }
    if !pc.keys.ready() {
        return Ok(Answer::refused(Refused::Setup).at(&resolved));
    }
    let entry =
        Entry { replies, phone, session: &ask.session, text: &text, place: &place, resolved: &resolved };
    let mut trail = Trail::default();
    let mut shown = Shown::default();
    let answer = enter(pc, &entry, &mut trail, &mut shown);
    // The title is back and PRIMARY cleared: the answer is decided.
    trail.back = give_back(pc, &shown, answer.outcome);
    Ok(Answer { trail, ..answer.at(&resolved) })
}

/// One reply being entered: who for, what, and where.
struct Entry<'r> {
    replies: &'r Replies,
    phone: &'r str,
    session: &'r str,
    text: &'r str,
    place: &'r Place,
    resolved: &'r Resolved,
}

impl std::fmt::Debug for Entry<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry").field("session", &self.session).finish_non_exhaustive()
    }
}

/// After the confirmation: nobody at the PC, the window and its tab in
/// front and proven, the same process again, then the paste and Enter.
fn enter<H: Host, W: Windows, K: Keys, D: Desk>(
    pc: &Pc<'_, H, W, K, D>,
    entry: &Entry<'_>,
    trail: &mut Trail,
    shown: &mut Shown,
) -> Answer {
    let place = entry.place;
    let resolved = entry.resolved;
    // Nobody at the PC: nothing has been brought forward, titled or read yet.
    let Some(listed) = pc.windows.list() else { return Answer::refused(Refused::Setup) };
    let in_chain = |pid: u32| place.chain.iter().any(|a| a.pid == pid);
    let front_pid = listed.iter().find(|w| w.focused).map(|w| w.pid).filter(|pid| in_chain(*pid));
    if let Err(wait) = pc_allows(pc.desk, &listed, front_pid) {
        return Answer { retry_ms: wait.map(in_use_ms), ..Answer::refused(Refused::InUse) };
    }
    // Never brought forward on the lock screen.
    if pc.desk.locked() != Some(false) {
        return Answer::refused(Refused::Locked);
    }
    let Some(tty) = place.tty else { return Answer::refused(Refused::Place) };
    let deadline = pc.desk.now() + BEFORE_PASTE;
    shown.prior = listed.iter().find(|w| w.focused).map(Front::of);
    shown.minimized = listed.iter().filter(|w| w.minimized).map(|w| w.id).collect();

    // The window, and its tab, in front and proven.
    let (front, proven) = match resolved.window {
        Some(id) => {
            trail.proof = "structural";
            let front = Front { id, pid: resolved.emulator.pid, class: resolved.class.clone() };
            if let Err(outcome) = bring_forward(pc, &front, &mut shown.front) {
                return Answer::of(outcome);
            }
            (front, None)
        }
        None => {
            trail.proof = if by_status_file(place) { "title" } else { "title_text" };
            match title_proof(pc, place, resolved, tty, deadline, trail, &mut shown.front) {
                Ok(proven) => (proven.front.clone(), Some(proven)),
                Err(outcome) => return Answer::of(outcome),
            }
        }
    };
    let mark = proven.as_ref().map(|p| p.mark.as_str());

    // The same run of the agent, on the same terminal, still at a prompt:
    // the terminal shown is the one the agent reads.
    match pc.host.place(entry.session) {
        Ok(Found::Place(again)) if same_run(&again, place) => {
            if let Err(reason) = session_allows(&again) {
                return Answer::refused(reason);
            }
        }
        _ => return Answer::refused(Refused::NoProcess),
    }

    // PRIMARY, still in front, the chord.
    if !pc.windows.set_primary(entry.text) {
        return Answer::refused(Refused::Setup);
    }
    let _primary = Primary { windows: pc.windows, text: entry.text };
    if !focused_on(pc.windows, &front, mark) {
        return Answer::failed(Failed::Focus);
    }
    if let Some(back) = proven.as_ref().and_then(|p| p.back.as_ref()) {
        back.keep();
    }
    // Before G7's own keys, which the desk counts as nobody's.
    shown.touched = pc.desk.phone_typing() || pc.desk.free_in() != Some(Duration::ZERO);
    entry.replies.ledger().entered(entry.phone, entry.session, Instant::now());
    if !pc.keys.paste() {
        return Answer::failed(Failed::Input);
    }

    // Settle, then everything that can have changed.
    pc.desk.wait(SETTLE);
    let interrupted = Answer::failed(Failed::Interrupted);
    if !focused_on(pc.windows, &front, mark) || pc.desk.locked() != Some(false) {
        return interrupted;
    }
    match pc.host.place(entry.session) {
        Ok(Found::Place(again)) if same_run(&again, place) && session_allows(&again).is_ok() => {}
        _ => return interrupted,
    }

    // Enter.
    if !pc.keys.enter() {
        return Answer::failed(Failed::Input);
    }
    Answer::of(Outcome::Sent)
}

/// The same run of the agent on the same terminal: a pty freed and reused
/// in between would have another process on it.
fn same_run(again: &Place, place: &Place) -> bool {
    again.pid == place.pid && again.start_ticks == place.start_ticks && again.tty == place.tty
}

/// Brings `front` forward and waits for it to have the focus; then it's
/// `brought`, whatever happens after.
fn bring_forward<H: Host, W: Windows, K: Keys, D: Desk>(
    pc: &Pc<'_, H, W, K, D>,
    front: &Front,
    brought: &mut Option<Front>,
) -> Result<(), Outcome> {
    if !pc.windows.activate(front.id) {
        return Err(Outcome::Failed(Failed::Focus));
    }
    let mut waited = Duration::ZERO;
    while !focused_on(pc.windows, front, None) {
        if waited >= FOCUS_WAIT {
            return Err(Outcome::Failed(Failed::Focus));
        }
        pc.desk.wait(FOCUS_POLL);
        waited += FOCUS_POLL;
    }
    *brought = Some(front.clone());
    Ok(())
}

/// Gives the focus back to the window that had it before G7 brought the
/// terminal forward (`plans/move-and-focus.md` §3): after a reply once
/// Enter had time to arrive, at once when nothing was typed, never when
/// text may sit in the agent's box. Only while the PC is unlocked, nobody
/// touched it since before the reply, the terminal still has the focus, and
/// that window is still there, on the terminal's workspace. Once, never
/// retried. The word for the log.
fn give_back<H: Host, W: Windows, K: Keys, D: Desk>(
    pc: &Pc<'_, H, W, K, D>,
    shown: &Shown,
    outcome: Outcome,
) -> &'static str {
    let Some(front) = &shown.front else { return "" };
    // The terminal had the focus itself, or nothing listed did.
    let Some(prior) = shown.prior.as_ref().filter(|prior| prior.id != front.id) else { return "" };
    match outcome {
        Outcome::Sent => pc.desk.wait(GIVE_BACK_AFTER),
        // The text may be in the agent's box: the PC's user must see it.
        Outcome::Failed(Failed::Interrupted | Failed::Input) => return "kept",
        Outcome::Ready | Outcome::Refused(_) | Outcome::Failed(Failed::Focus) => {}
    }
    if pc.desk.locked() != Some(false) {
        return "kept";
    }
    // Someone back at the PC may be typing into the terminal now.
    if shown.touched || pc.desk.phone_typing() || pc.desk.free_in() != Some(Duration::ZERO) {
        return "kept";
    }
    let Some(listed) = pc.windows.list() else { return "kept" };
    // Moved by someone or something else: left where it is.
    let Some(front_now) = listed.iter().find(|w| front.is(w) && w.focused) else { return "kept" };
    let Some(prior_now) = listed.iter().find(|w| prior.is(w)) else { return "gone" };
    // Giving it back would switch workspaces (not tried with the owner, S10).
    if prior_now.workspace.is_none() || prior_now.workspace != front_now.workspace {
        return "kept";
    }
    if !pc.windows.activate(prior.id) {
        return "failed";
    }
    let mut waited = Duration::ZERO;
    while !focused_on(pc.windows, prior, None) {
        if waited >= BACK_WAIT {
            return "failed";
        }
        pc.desk.wait(FOCUS_POLL);
        waited += FOCUS_POLL;
    }
    if shown.minimized.contains(&front.id) {
        pc.windows.minimize(front.id);
    }
    "given"
}

/// The title proof: a one-time title on the session's terminal, the window
/// or tab showing it brought forward, and, for a session found by its
/// command line, its newest text on the screen. Past `deadline`, the
/// refusal of the step it's in.
fn title_proof<'a, H: Host, W: Windows, K: Keys, D: Desk>(
    pc: &Pc<'a, H, W, K, D>,
    place: &Place,
    resolved: &Resolved,
    tty: u32,
    deadline: Instant,
    trail: &mut Trail,
    brought: &mut Option<Front>,
) -> Result<Proven<'a, W>, Outcome> {
    let refused = Outcome::Refused;
    let pid = resolved.emulator.pid;
    let route = resolved.terminal.tabs;

    // The title, and where it shows. Once more with a new one when it
    // showed nowhere: the agent may have set its own title just then.
    let mut second = false;
    let (titled, mark, seen) = loop {
        let mark = new_mark();
        if !pc.windows.title_mark(tty, &mark) {
            return Err(refused(Refused::Unproven));
        }
        let titled = Titled { windows: pc.windows, tty };
        match seen_where(pc, pid, &mark, route, deadline) {
            Seen::Nowhere if !second && pc.desk.now() <= deadline => second = true,
            Seen::Nowhere => return Err(refused(Refused::Unproven)),
            Seen::Several => return Err(refused(Refused::Ambiguous)),
            seen => break (titled, mark, seen),
        }
    };

    // Its window forward, and its tab selected. G7 moves only to the tab it
    // knows has the title; it never steps through tabs to look.
    let mut back = None;
    let front = match seen {
        Seen::Window(window) => {
            trail.tab = "front";
            let front = Front { id: window.id, pid, class: window.class };
            bring_forward(pc, &front, brought)?;
            front
        }
        Seen::Tab(spot) => {
            if spot.tabs > MAX_TABS {
                return Err(refused(Refused::Tab));
            }
            let window = spot_window(pc.windows, pid, &spot)?;
            let front = Front { id: window.id, pid, class: window.class };
            bring_forward(pc, &front, brought)?;
            if spot.at == spot.selected {
                trail.tab = "front";
            } else {
                // Armed first: a switch that stopped half-way goes back too.
                let going_back = back.insert(Back {
                    windows: pc.windows,
                    pid,
                    mark: mark.clone(),
                    at: spot.selected,
                    route,
                    armed: std::cell::Cell::new(true),
                });
                let before = pc.windows.select_tab(pid, &mark, route).map_err(|_| refused(Refused::Tab))?;
                going_back.at = before;
                trail.tab = "switched";
                trail.steps = spot.at.abs_diff(before);
                if pc.desk.now() > deadline {
                    return Err(refused(Refused::Tab));
                }
            }
            front
        }
        Seen::Several | Seen::Nowhere => return Err(refused(Refused::Unproven)),
    };

    // The gate: the window in front shows the title, so its selected tab is
    // the session's terminal.
    if pc.desk.locked() != Some(false) {
        return Err(refused(Refused::Locked));
    }
    let not_shown = refused(if back.is_some() { Refused::Tab } else { Refused::Unproven });
    let mut waited = Duration::ZERO;
    while !focused_on(pc.windows, &front, Some(&mark)) {
        if waited >= TAB_WAIT || pc.desk.now() > deadline {
            return Err(not_shown);
        }
        pc.desk.wait(FOCUS_POLL);
        waited += FOCUS_POLL;
    }

    // A process found by its command line may have switched sessions inside
    // (Codex's `/resume`): the terminal must show the session's newest text.
    if !by_status_file(place) {
        if pc.desk.now() > deadline {
            return Err(refused(Refused::Unproven));
        }
        match pc.windows.terminal_text(pid) {
            Some(screen) if shows_a_mark(&screen, &place.marks) => {}
            Some(_) => return Err(refused(Refused::Unproven)),
            None => return Err(refused(Refused::Ambiguous)),
        }
    }
    Ok(Proven { front, mark, back, _titled: titled })
}

/// Where the title `mark` shows: on the terminal's windows, looked at for
/// [`TITLE_WAIT`] (a window's title is its selected tab's), else on one of
/// their tabs.
fn seen_where<H: Host, W: Windows, K: Keys, D: Desk>(
    pc: &Pc<'_, H, W, K, D>,
    pid: u32,
    mark: &str,
    route: TabRoute,
    deadline: Instant,
) -> Seen {
    let mut waited = Duration::ZERO;
    loop {
        let mut showing: Vec<Win> = pc
            .windows
            .list()
            .unwrap_or_default()
            .into_iter()
            .filter(|w| w.pid == pid && w.normal && w.title.contains(mark))
            .collect();
        match showing.len() {
            0 => {}
            1 => return Seen::Window(showing.remove(0)),
            _ => return Seen::Several,
        }
        if pc.desk.now() > deadline {
            return Seen::Nowhere;
        }
        if waited >= TITLE_WAIT {
            break;
        }
        pc.desk.wait(FOCUS_POLL);
        waited += FOCUS_POLL;
    }
    match pc.windows.tab_titled(pid, mark, route) {
        Some(mut spots) if spots.len() == 1 => Seen::Tab(spots.remove(0)),
        Some(spots) if spots.len() > 1 => Seen::Several,
        _ => Seen::Nowhere,
    }
}

/// The window a tab is in: the terminal's only window, else the one whose
/// title is that of the tab's window.
fn spot_window<W: Windows>(windows: &W, pid: u32, spot: &TabSpot) -> Result<Win, Outcome> {
    let listed = windows.list().ok_or(Outcome::Refused(Refused::Setup))?;
    let mut own: Vec<Win> = listed.into_iter().filter(|w| w.pid == pid && w.normal).collect();
    if own.is_empty() {
        return Err(Outcome::Refused(Refused::NoWindow));
    }
    if own.len() > 1 {
        let title = cut(&spot.frame_title, WINDOW_TITLE_BYTES);
        own.retain(|w| w.title == title);
    }
    match own.len() {
        1 => Ok(own.remove(0)),
        _ => Err(Outcome::Refused(Refused::Ambiguous)),
    }
}

// ---- This PC ----

/// A session's place as the host's `session.place` answers it.
fn found(result: SessionPlaceResult, folder: &str) -> Found {
    let place = match result {
        SessionPlaceResult::Place(place) => place,
        SessionPlaceResult::Refused(refused) => {
            return match refused.reason {
                PlaceRefusal::Ended => Found::Ended,
                PlaceRefusal::Ambiguous => Found::Ambiguous,
                // A word a newer host sends is a refusal too.
                PlaceRefusal::Place | PlaceRefusal::Unknown => Found::Elsewhere,
            };
        }
    };
    let state = match place.state {
        SessionState::Idle => Now::Idle,
        SessionState::Working => Now::Working,
        SessionState::NeedsYou if place.state_exact => Now::NeedsYou,
        SessionState::NeedsYou | SessionState::NeedsYouGuess => Now::MayNeedYou,
        SessionState::Ended => Now::Ended,
    };
    Found::Place(Place {
        agent: match place.agent {
            api::AgentId::Claude => Agent::Claude,
            api::AgentId::Codex => Agent::Codex,
        },
        source: match place.source {
            SessionSource::Terminal => Source::Terminal,
            SessionSource::DesktopApp => Source::DesktopApp,
            SessionSource::Other => Source::Other,
        },
        state,
        state_exact: place.state_exact,
        pid: place.pid,
        start_ticks: place.start_ticks,
        tty: place.tty,
        foreground: place.foreground,
        ptys_under: place.chain.iter().filter_map(|a| Some((a.pid, a.ptys_under?))).collect(),
        chain: place
            .chain
            .into_iter()
            .map(|a| Ancestor {
                pid: a.pid,
                start_ticks: a.start_ticks,
                comm: a.comm,
                exe_name: a.exe_name.unwrap_or_default(),
            })
            .collect(),
        marks: place.marks,
        folder: folder.to_owned(),
        continue_block: place.continue_block.map(|block| match block {
            api::ContinueBlock::Open => Blocked::Open,
            api::ContinueBlock::Folder => Blocked::Folder,
            api::ContinueBlock::Agent => Blocked::Agent,
        }),
    })
}

/// The agent host, asked from the reply's own thread.
#[derive(Debug)]
struct LiveHost {
    client: Client,
    runtime: tokio::runtime::Handle,
    folder: String,
}

impl LiveHost {
    /// For `session`, asked from a thread of its own. `folder` is the
    /// session's folder label when the page's list has it.
    async fn new(client: &Client, folder: Option<String>, session: &str) -> Self {
        let folder = match folder {
            Some(folder) => folder,
            None => client
                .sessions_list(Some(g7link_core::agent::MAX_SESSIONS as u32))
                .await
                .ok()
                .and_then(|list| list.into_iter().find(|s| s.id == session).map(|s| s.folder_label))
                .unwrap_or_default(),
        };
        LiveHost { client: client.clone(), runtime: tokio::runtime::Handle::current(), folder }
    }
}

impl Host for LiveHost {
    fn place(&self, session: &str) -> Result<Found, HostDown> {
        match self.runtime.block_on(self.client.session_place(session.to_owned())) {
            Ok(result) => Ok(found(result, &self.folder)),
            Err(ClientError::Remote(e)) if e.code == ErrorCode::NotFound => Ok(Found::Unknown),
            Err(error) => {
                tracing::info!(%error, "the agent host didn't place a session");
                Err(HostDown)
            }
        }
    }
}

/// The answer as the core passes it to the phone.
fn wire(answer: Answer) -> AgentSessionReplied {
    AgentSessionReplied {
        result: answer.outcome.result().to_owned(),
        reason: answer.outcome.reason().map(str::to_owned),
        target: answer.target.map(wire_target),
        nonce: answer.nonce,
        retry_ms: answer.retry_ms.map(u64::from),
    }
}

fn wire_target(target: Target) -> AgentReplyTarget {
    AgentReplyTarget {
        app: target.app,
        agent: target.agent,
        folder: target.folder,
        kind: target.kind.to_owned(),
    }
}

fn wire_error(error: Error) -> AgentFailure {
    match error {
        Error::BadMessage => AgentFailure::BadMessage,
        Error::NotFound => AgentFailure::NotFound,
        Error::Busy => AgentFailure::Busy,
        Error::Internal => AgentFailure::Internal,
    }
}

/// A phone's `agent.session.reply` (`text: None` is a check), after the
/// core checked the capability, the toggles and the limits. `folder` is
/// the session's folder label when the page's list has it. Runs on a
/// thread of its own: it waits on windows, the accessibility bus and the
/// keyboard, and asks the host from there.
pub(crate) async fn phone(
    from: DeviceId,
    client: &Client,
    folder: Option<String>,
    session: String,
    text: Option<String>,
    confirm: Option<String>,
) -> Result<AgentReply, AgentFailure> {
    // A host older than `session.place` can't say where a session runs.
    if !client.host().has(cap::PLACE) {
        return Ok(AgentReply::Replied(wire(Answer::refused(Refused::Setup))));
    }
    let host = LiveHost::new(client, folder, &session).await;
    let ask = Ask { session, check: text.is_none(), text, confirm };
    let (tx, rx) = tokio::sync::oneshot::channel();
    let spawned = std::thread::Builder::new().name("agent-reply".into()).spawn(move || {
        let pc = Pc { host: &host, windows: &LiveWindows, keys: &LiveKeys, desk: &LiveDesk };
        let _ = tx.send(answer(&pc, replies(), &from.to_string(), &ask));
    });
    if let Err(error) = spawned {
        tracing::warn!(%error, "can't enter a phone's reply");
        return Err(AgentFailure::Internal);
    }
    match rx.await {
        Ok(Ok(answer)) => Ok(AgentReply::Replied(wire(answer))),
        Ok(Err(error)) => Err(wire_error(error)),
        Err(_) => Err(AgentFailure::Internal),
    }
}

/// Offers `agent.reply`, or says what it needs, again when that comes or
/// goes (the extension enabled after login, or updated). One thread for the
/// app's lifetime.
pub(crate) fn watch_setup() {
    const EVERY: Duration = Duration::from_secs(60);
    let spawned = std::thread::Builder::new().name("agent-reply-setup".into()).spawn(|| {
        loop {
            std::thread::sleep(EVERY);
            // Locked, the extension is off: the phone shows "unlock", not "set up".
            if !super::host_installed() || crate::sys::input::lock_state() != Some(false) {
                continue;
            }
            let before = *SETUP.lock().unwrap_or_else(|e| e.into_inner());
            if before != Some(offered()) {
                crate::core_host::refresh_capabilities();
            }
        }
    });
    if let Err(error) = spawned {
        tracing::warn!(%error, "can't follow what replies need");
    }
}

static REPLIES: OnceLock<Replies> = OnceLock::new();

/// The PC's one ledger.
pub(crate) fn replies() -> &'static Replies {
    REPLIES.get_or_init(Replies::default)
}

/// GNOME Shell's companion extension, the accessibility bus, the session's
/// terminal device and the desktop entries.
#[derive(Debug)]
pub(crate) struct LiveWindows;

impl Windows for LiveWindows {
    fn ready(&self) -> bool {
        crate::sys::shell_clipboard::has_primary() && crate::sys::shell_windows::has_states()
    }

    fn list(&self) -> Option<Vec<Win>> {
        let listed = crate::sys::shell_windows::list()?;
        Some(
            listed
                .into_iter()
                .map(|w| Win {
                    id: w.id,
                    pid: w.pid,
                    class: w.class,
                    title: w.title,
                    focused: w.focused,
                    fullscreen: w.fullscreen,
                    minimized: w.minimized,
                    normal: w.normal,
                    workspace: w.workspace,
                })
                .collect(),
        )
    }

    fn activate(&self, id: u64) -> bool {
        crate::sys::shell_windows::activate(id)
    }

    fn minimize(&self, id: u64) -> bool {
        crate::sys::shell_windows::minimize(id)
    }

    fn set_primary(&self, text: &str) -> bool {
        crate::sys::shell_clipboard::set_primary(text)
    }

    fn clear_primary(&self, expected: &str) {
        crate::sys::shell_clipboard::clear_primary(expected);
    }

    fn terminal_text(&self, pid: u32) -> Option<String> {
        crate::sys::text_focus::focused_terminal_text(pid)
    }

    fn app_name(&self, class: &str, pid: u32) -> Option<String> {
        crate::sys::app_entry::identify(class, pid).map(|app| app.name.clone())
    }

    fn title_mark(&self, tty: u32, title: &str) -> bool {
        crate::sys::tty_title::mark(tty, title)
    }

    fn title_unmark(&self, tty: u32) {
        if !crate::sys::tty_title::unmark(tty) {
            tracing::info!("a terminal's title couldn't be put back after a phone's reply");
        }
    }

    fn terminal_reset(&self, tty: u32) -> bool {
        crate::sys::tty_title::restore(tty)
    }

    fn tab_titled(&self, pid: u32, mark: &str, route: TabRoute) -> Option<Vec<TabSpot>> {
        crate::sys::text_focus::tab_titled(pid, mark, route)
    }

    fn select_tab(&self, pid: u32, mark: &str, route: TabRoute) -> Result<usize, TabError> {
        crate::sys::text_focus::select_tab(pid, mark, route)
    }

    fn select_tab_at(&self, pid: u32, mark: &str, at: usize, route: TabRoute) -> bool {
        crate::sys::text_focus::select_tab_at(pid, mark, at, route)
    }
}

/// The virtual keyboard only: never the portal or a helper program.
#[derive(Debug)]
pub(crate) struct LiveKeys;

impl Keys for LiveKeys {
    fn ready(&self) -> bool {
        crate::sys::input::reply_keyboard()
    }

    fn paste(&self) -> bool {
        // Counted as a phone's input, so the idle check of the next reply
        // doesn't take it for someone at the PC.
        let _ours = crate::sys::text_focus::injecting();
        crate::sys::input::press_reply(crate::sys::input::ReplyKey::Paste)
    }

    fn enter(&self) -> bool {
        let _ours = crate::sys::text_focus::injecting();
        crate::sys::input::press_reply(crate::sys::input::ReplyKey::Enter)
    }
}

#[derive(Debug)]
pub(crate) struct LiveDesk;

impl Desk for LiveDesk {
    fn locked(&self) -> Option<bool> {
        crate::sys::input::lock_state()
    }

    fn free_in(&self) -> Option<Duration> {
        crate::sys::text_focus::free_in(OWNER_AWAY)
    }

    fn phone_typing(&self) -> bool {
        crate::sys::text_focus::phone_typing()
    }

    fn wait(&self, how_long: Duration) {
        std::thread::sleep(how_long);
    }

    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// What this PC can do about replies now (`plans/move-and-focus.md` §7.1),
/// each found by asking for it, never by the desktop's name: GNOME Shell on
/// the session bus, the companion extension's window states and PRIMARY,
/// and a virtual keyboard. Asks the desktop (blocking); the answer is kept
/// for [`setup`]. While the PC is locked GNOME turns the extension off, so
/// what was found unlocked stands.
pub(crate) fn offered() -> Setup {
    let kept = *SETUP.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(setup) = kept
        && crate::sys::input::lock_state() == Some(true)
    {
        return setup;
    }
    let shell = crate::sys::shell_windows::has_shell();
    let extension =
        shell && crate::sys::shell_clipboard::has_primary() && crate::sys::shell_windows::has_states();
    let setup = Setup::of(shell, extension, crate::sys::input::reply_keyboard_possible());
    *SETUP.lock().unwrap_or_else(|e| e.into_inner()) = Some(setup);
    setup
}

/// Whether replies can be typed on this PC, and if not, what they need first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Setup {
    Ready,
    /// No window backend for this desktop: GNOME Shell isn't on the bus.
    Desktop,
    /// The companion extension is missing or too old.
    Extension,
    /// No virtual keyboard can be made.
    Keyboard,
}

impl Setup {
    /// In this order: the desktop, its extension, the keyboard.
    fn of(shell: bool, extension: bool, keyboard: bool) -> Setup {
        match (shell, extension, keyboard) {
            (false, ..) => Setup::Desktop,
            (true, false, _) => Setup::Extension,
            (true, true, false) => Setup::Keyboard,
            (true, true, true) => Setup::Ready,
        }
    }
}

/// The capabilities a PC in `setup` offers: `agent.reply`, or the one word
/// that says what it needs (docs/protocol/agents.md §9); and `agent.move`
/// beside `agent.reply` while the agent host can close sessions.
pub(crate) fn capabilities(setup: Setup, host_moves: bool) -> Vec<&'static str> {
    use g7link_core::{
        session_move,
        session_reply::{self, needs},
    };
    match setup {
        Setup::Ready if host_moves => vec![session_reply::CAP, session_move::CAP],
        Setup::Ready => vec![session_reply::CAP],
        Setup::Desktop => vec![needs::DESKTOP],
        Setup::Extension => vec![needs::EXTENSION],
        Setup::Keyboard => vec![needs::KEYBOARD],
    }
}

static SETUP: Mutex<Option<Setup>> = Mutex::new(None);

/// What [`offered`] found last, for Settings: `ready`, `desktop`,
/// `extension`, `keyboard`, or empty when it hasn't been asked.
pub(crate) fn setup() -> &'static str {
    match *SETUP.lock().unwrap_or_else(|e| e.into_inner()) {
        Some(Setup::Ready) => "ready",
        Some(Setup::Desktop) => "desktop",
        Some(Setup::Extension) => "extension",
        Some(Setup::Keyboard) => "keyboard",
        None => "",
    }
}

#[path = "move_here.rs"]
pub(crate) mod move_here;

#[cfg(test)]
#[path = "reply_tests.rs"]
mod tests;
