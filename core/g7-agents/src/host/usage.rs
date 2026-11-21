// SPDX-License-Identifier: MPL-2.0
//! Usage limits (§Q4.8): the windows each CLI reports during its turns,
//! kept per agent and shown in `AgentInfo.limits`. `agents.changed` for a
//! change of limits alone goes out at most every [`LIMITS_EVENT_MS`] per
//! agent, at once when a window's state changes. G7 never asks a vendor.
//!
//! The limit alarm (phase 5, §V5): a PC notification [`LIMIT_ALARM_DELAY_MS`]
//! after the reset time the CLI reported for a limit it reached. It only
//! tells: no thread changes and nothing is sent to an agent.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::api::*;
use crate::host::Host;
use crate::host::agents::code;
use crate::host::notify::Note;
use crate::host::store::now_ms;
use crate::host::threads::notify;

/// Limits as kept: cleaned, cut, at most [`MAX_LIMITS`], one per window.
pub fn sanitize(limits: Vec<Limit>) -> Vec<Limit> {
    let mut out: Vec<Limit> = Vec::with_capacity(limits.len().min(MAX_LIMITS));
    for limit in limits {
        let window = clean_cut(limit.window.trim(), MAX_LIMIT_WINDOW).0;
        if window.is_empty() || out.iter().any(|l| l.window == window) {
            continue;
        }
        out.push(Limit {
            window,
            used_pct: limit.used_pct.map(|p| p.min(100)),
            resets_ms: limit.resets_ms,
            state: limit.state,
        });
        if out.len() == MAX_LIMITS {
            break;
        }
    }
    out
}

/// What to do about an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Announce {
    /// Nothing changed, or a later announcement already covers it.
    No,
    Now,
    /// After this long (the agent's last announcement was too recent).
    After(Duration),
}

#[derive(Debug, Default)]
struct AgentUsage {
    limits: Vec<Limit>,
    /// The windows' states clients last heard, and when.
    announced: Option<(Instant, Vec<(String, LimitState)>)>,
    /// An announcement is scheduled.
    pending: bool,
    /// The reset time the limit alarm waits for.
    alarm: Option<u64>,
}

/// Every agent's limits.
#[derive(Debug, Default)]
pub struct Usage {
    agents: HashMap<AgentId, AgentUsage>,
    every: Duration,
}

fn states(limits: &[Limit]) -> Vec<(String, LimitState)> {
    limits.iter().map(|l| (l.window.clone(), l.state)).collect()
}

impl Usage {
    pub fn new(every: Duration) -> Self {
        Self { agents: HashMap::new(), every }
    }

    pub fn limits(&self, agent: AgentId) -> Vec<Limit> {
        self.agents.get(&agent).map(|a| a.limits.clone()).unwrap_or_default()
    }

    /// Keeps `limits` (sanitized) and says when clients should hear of them.
    pub fn update(&mut self, agent: AgentId, limits: Vec<Limit>, now: Instant) -> Announce {
        let limits = sanitize(limits);
        let every = self.every;
        let usage = self.agents.entry(agent).or_default();
        if usage.limits == limits {
            return Announce::No;
        }
        usage.limits = limits;
        let now_states = states(&usage.limits);
        match &usage.announced {
            Some((_, heard)) if *heard != now_states => {
                usage.announced = Some((now, now_states));
                Announce::Now
            }
            Some((at, _)) if now.duration_since(*at) < every => {
                if usage.pending {
                    Announce::No
                } else {
                    usage.pending = true;
                    Announce::After(every - now.duration_since(*at))
                }
            }
            _ => {
                usage.announced = Some((now, now_states));
                Announce::Now
            }
        }
    }

    /// A scheduled announcement went out.
    pub fn announced(&mut self, agent: AgentId, now: Instant) {
        if let Some(usage) = self.agents.get_mut(&agent) {
            usage.pending = false;
            usage.announced = Some((now, states(&usage.limits)));
        }
    }

    /// The reset time `agent`'s alarm waits for.
    pub fn alarm(&self, agent: AgentId) -> Option<u64> {
        self.agents.get(&agent).and_then(|a| a.alarm)
    }

    /// Arms `agent`'s alarm for a limit that resets at `resets_ms`, or moves
    /// it later. A time that has passed, or is more than
    /// [`LIMIT_ALARM_AHEAD_MS`] ahead, arms nothing.
    pub fn arm(&mut self, agent: AgentId, resets_ms: u64, now_ms: u64) {
        if resets_ms.saturating_add(LIMIT_ALARM_DELAY_MS) <= now_ms
            || resets_ms > now_ms.saturating_add(LIMIT_ALARM_AHEAD_MS)
        {
            return;
        }
        let usage = self.agents.entry(agent).or_default();
        usage.alarm = Some(usage.alarm.map_or(resets_ms, |at| at.max(resets_ms)));
    }

    /// The alarm follows the windows `agent` reported last: the latest reset
    /// of those reached; none reached, the limit has cleared and the alarm
    /// goes. Windows reached with no reset time leave it as it is.
    pub fn arm_from_limits(&mut self, agent: AgentId, now_ms: u64) {
        let Some(usage) = self.agents.get_mut(&agent) else { return };
        if usage.limits.is_empty() {
            return;
        }
        let reached: Vec<&Limit> = usage.limits.iter().filter(|l| l.state == LimitState::Reached).collect();
        if reached.is_empty() {
            usage.alarm = None;
            return;
        }
        let latest = reached.iter().filter_map(|l| l.resets_ms).max();
        if let Some(resets) = latest {
            self.arm(agent, resets, now_ms);
        }
    }

    /// The agents whose alarm is due at `now_ms`; each rings once.
    pub fn ring(&mut self, now_ms: u64) -> Vec<AgentId> {
        let mut due: Vec<AgentId> = self
            .agents
            .iter_mut()
            .filter(|(_, a)| a.alarm.is_some_and(|at| at.saturating_add(LIMIT_ALARM_DELAY_MS) <= now_ms))
            .map(|(agent, a)| {
                a.alarm = None;
                *agent
            })
            .collect();
        due.sort();
        due
    }
}

/// The reset time in a usage-limit failure's message (`…|resets_ms=<ms>`).
fn failed_resets(error: &ThreadError) -> Option<u64> {
    if error.code != code::USAGE_LIMIT {
        return None;
    }
    error.message.split_once("|resets_ms=").and_then(|(_, ms)| ms.trim().parse().ok())
}

impl Host {
    fn lock_usage(&self) -> std::sync::MutexGuard<'_, Usage> {
        self.usage.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The limits kept for `agent`.
    pub fn limits_of(&self, agent: AgentId) -> Vec<Limit> {
        self.lock_usage().limits(agent)
    }

    /// Keeps limits a probe read; clients hear of them with the probe's result.
    pub(crate) fn keep_limits(&self, agent: AgentId, limits: Vec<Limit>) {
        let now = Instant::now();
        let mut usage = self.lock_usage();
        if usage.update(agent, limits, now) != Announce::No {
            usage.announced(agent, now);
        }
        usage.arm_from_limits(agent, now_ms());
    }

    /// An agent reported its usage windows (from any thread).
    pub(crate) fn on_limits(self: &Arc<Self>, agent: AgentId, limits: Vec<Limit>) {
        let announce = {
            let mut usage = self.lock_usage();
            let announce = usage.update(agent, limits, Instant::now());
            usage.arm_from_limits(agent, now_ms());
            announce
        };
        let wait = match announce {
            Announce::No => return,
            Announce::Now => Duration::ZERO,
            Announce::After(wait) => wait,
        };
        let host = Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            let Some(host) = host.upgrade() else { return };
            if !wait.is_zero() {
                host.lock_usage().announced(agent, Instant::now());
            }
            host.announce_agents().await;
        });
    }

    /// A turn of `agent` failed: a usage limit with its reset time arms the alarm.
    pub(crate) fn limit_failed(&self, agent: AgentId, error: &ThreadError) {
        if let Some(resets) = failed_resets(error) {
            self.lock_usage().arm(agent, resets, now_ms());
        }
    }

    /// Threads stopped by a usage limit arm their agent's alarm (at start).
    pub(crate) fn arm_limit_alarms(&self) {
        let failed: Vec<(AgentId, ThreadError)> = self
            .lock_reg()
            .threads
            .values()
            .filter(|e| e.info.state == State::Failed)
            .filter_map(|e| Some((e.info.agent, e.info.error.clone()?)))
            .collect();
        for (agent, error) in &failed {
            self.limit_failed(*agent, error);
        }
    }

    /// Alarms due at `now_ms` ring: "Claude's limit has reset", with the
    /// newest thread it stopped, while the setting is on. Nothing else.
    pub(crate) fn ring_limit_alarms(&self, now_ms: u64) {
        let due = self.lock_usage().ring(now_ms);
        if due.is_empty() || !self.limit_alarm_on() {
            return;
        }
        for agent in due {
            let thread = self.paused_by_limit(agent);
            notify(self, Note::LimitReset { agent, thread });
        }
    }

    /// The newest thread of `agent` a usage limit stopped, not archived.
    fn paused_by_limit(&self, agent: AgentId) -> Option<ThreadInfo> {
        self.lock_reg()
            .threads
            .values()
            .map(|e| &e.info)
            .filter(|t| t.agent == agent && t.state == State::Failed && !t.archived)
            .filter(|t| t.error.as_ref().is_some_and(|e| e.code == code::USAGE_LIMIT))
            .max_by_key(|t| (t.updated_ms, t.id.clone()))
            .cloned()
    }

    /// Sends `agents.changed` when what clients were shown differs from the
    /// probes with the limits kept now. Never probes.
    pub(crate) async fn announce_agents(&self) {
        let mut cache = self.probes.lock().await;
        let Some(shown) = cache.shown.clone() else { return };
        let now: Vec<AgentInfo> =
            shown.iter().map(|a| AgentInfo { limits: self.limits_of(a.id), ..a.clone() }).collect();
        if now != shown {
            cache.shown = Some(now.clone());
            self.hub.broadcast(&Event::AgentsChanged(AgentsListResult { agents: now }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit(window: &str, used: u8, state: LimitState) -> Limit {
        Limit { window: window.into(), used_pct: Some(used), resets_ms: Some(1), state }
    }

    #[test]
    fn limit_changes_are_announced_at_most_every_period_unless_a_state_changes() {
        let mut usage = Usage::new(Duration::from_secs(30));
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        assert_eq!(
            usage.update(AgentId::Claude, vec![limit("five_hour", 10, LimitState::Ok)], at(0)),
            Announce::Now
        );
        assert_eq!(
            usage.update(AgentId::Claude, vec![limit("five_hour", 10, LimitState::Ok)], at(1)),
            Announce::No
        );
        assert_eq!(
            usage.update(AgentId::Claude, vec![limit("five_hour", 11, LimitState::Ok)], at(5)),
            Announce::After(Duration::from_secs(25))
        );
        assert_eq!(
            usage.update(AgentId::Claude, vec![limit("five_hour", 12, LimitState::Ok)], at(6)),
            Announce::No,
            "one announcement is already scheduled"
        );
        assert_eq!(usage.limits(AgentId::Claude)[0].used_pct, Some(12), "it carries the newest");
        // A state change goes at once.
        assert_eq!(
            usage.update(AgentId::Claude, vec![limit("five_hour", 81, LimitState::Warn)], at(7)),
            Announce::Now
        );
        // Another agent has its own clock.
        assert_eq!(usage.update(AgentId::Codex, vec![limit("300", 1, LimitState::Ok)], at(8)), Announce::Now);
        usage.announced(AgentId::Claude, at(30));
        assert_eq!(
            usage.update(AgentId::Claude, vec![limit("five_hour", 82, LimitState::Warn)], at(61)),
            Announce::Now
        );
    }

    fn at(window: &str, state: LimitState, resets_ms: Option<u64>) -> Limit {
        Limit { window: window.into(), used_pct: Some(100), resets_ms, state }
    }

    #[test]
    fn the_limit_alarm_arms_moves_clears_and_rings_once() {
        let now = 1_800_000_000_000;
        let hour = 3_600_000;
        let mut usage = Usage::new(Duration::from_secs(30));
        let t = Instant::now();
        usage.arm(AgentId::Claude, now - LIMIT_ALARM_DELAY_MS, now);
        usage.arm(AgentId::Claude, now + LIMIT_ALARM_AHEAD_MS + 1, now);
        assert_eq!(usage.alarm(AgentId::Claude), None, "passed, or more than 8 days ahead");
        usage.arm(AgentId::Claude, now + LIMIT_ALARM_AHEAD_MS, now);
        assert_eq!(usage.alarm(AgentId::Claude), Some(now + LIMIT_ALARM_AHEAD_MS), "8 days");

        // From the windows reported: the latest reset of those reached.
        let mut usage = Usage::new(Duration::from_secs(30));
        usage.update(
            AgentId::Claude,
            vec![
                at("five_hour", LimitState::Reached, Some(now + hour)),
                at("seven_day", LimitState::Reached, Some(now + 2 * hour)),
                at("opus", LimitState::Ok, Some(now + 9 * hour)),
            ],
            t,
        );
        usage.arm_from_limits(AgentId::Claude, now);
        assert_eq!(usage.alarm(AgentId::Claude), Some(now + 2 * hour));
        usage.arm(AgentId::Claude, now + hour, now);
        assert_eq!(usage.alarm(AgentId::Claude), Some(now + 2 * hour), "never earlier");
        usage.arm(AgentId::Claude, now + 3 * hour, now);
        assert_eq!(usage.alarm(AgentId::Claude), Some(now + 3 * hour), "moved later");
        usage.update(AgentId::Claude, vec![at("five_hour", LimitState::Reached, None)], t);
        usage.arm_from_limits(AgentId::Claude, now);
        assert_eq!(usage.alarm(AgentId::Claude), Some(now + 3 * hour), "no time: left as it is");
        assert!(usage.ring(now + 3 * hour + LIMIT_ALARM_DELAY_MS - 1).is_empty());
        assert_eq!(usage.ring(now + 3 * hour + LIMIT_ALARM_DELAY_MS), [AgentId::Claude]);
        assert!(usage.ring(now + 4 * hour).is_empty(), "once");

        usage.arm(AgentId::Codex, now + hour, now);
        usage.update(AgentId::Codex, vec![at("300", LimitState::Warn, Some(now + hour))], t);
        usage.arm_from_limits(AgentId::Codex, now);
        assert_eq!(usage.alarm(AgentId::Codex), None, "the limit cleared");
        usage.arm(AgentId::Codex, now + hour, now);
        usage.update(AgentId::Codex, Vec::new(), t);
        usage.arm_from_limits(AgentId::Codex, now);
        assert_eq!(usage.alarm(AgentId::Codex), Some(now + hour), "no windows say nothing");
    }

    #[test]
    fn a_usage_limit_failure_carries_its_reset_time() {
        let error = |code: &str, message: &str| ThreadError { code: code.into(), message: message.into() };
        assert_eq!(
            failed_resets(&error("usage_limit", "Usage limit reached|resets_ms=1795050000000")),
            Some(1_795_050_000_000)
        );
        assert_eq!(failed_resets(&error("usage_limit", "Usage limit reached")), None);
        assert_eq!(failed_resets(&error("failed", "x|resets_ms=1795050000000")), None);
    }

    mod alarm {
        use super::super::*;
        use tokio::sync::mpsc::UnboundedReceiver;

        use crate::host::agents::{Step, TurnEnd};

        fn rung(rx: &mut UnboundedReceiver<Note>) -> Vec<(AgentId, Option<String>)> {
            let mut seen = Vec::new();
            while let Ok(note) = rx.try_recv() {
                if let Note::LimitReset { agent, thread } = note {
                    seen.push((agent, thread.map(|t| t.id)));
                }
            }
            seen
        }

        fn reached(resets_ms: u64) -> Step {
            Step::Limits(vec![Limit {
                window: "five_hour".into(),
                used_pct: Some(100),
                resets_ms: Some(resets_ms),
                state: LimitState::Reached,
            }])
        }

        #[tokio::test]
        async fn the_alarm_tells_once_and_changes_nothing() {
            let dir = tempfile::tempdir().unwrap();
            let (host, mut rx) = crate::host::test_host_noting(dir.path(), APPROVAL_LIFETIME_MS);
            let (older, _, _older_cmds) = host.test_thread(AgentId::Claude);
            let (id, _, mut cmds) = host.test_thread(AgentId::Claude);
            let resets = now_ms() + 60_000;
            let limited = format!("Usage limit reached|resets_ms={resets}");
            host.test_step(&older, reached(resets));
            host.test_finish(&older, TurnEnd::failed("usage_limit", &limited));
            host.test_entry(&older, |e| e.info.updated_ms -= 1000);
            host.test_finish(&id, TurnEnd::failed("usage_limit", &limited));
            assert_eq!(host.lock_usage().alarm(AgentId::Claude), Some(resets));
            let before = host.test_state(&id);

            host.ring_limit_alarms(resets + LIMIT_ALARM_DELAY_MS - 1);
            assert!(rung(&mut rx).is_empty(), "not yet");
            host.ring_limit_alarms(resets + LIMIT_ALARM_DELAY_MS);
            assert_eq!(rung(&mut rx), [(AgentId::Claude, Some(id.clone()))], "the newest it stopped");
            host.ring_limit_alarms(resets + LIMIT_ALARM_DELAY_MS + 5_000);
            assert!(rung(&mut rx).is_empty(), "once");
            assert_eq!(host.test_state(&id), before, "the thread is as it was");
            assert_eq!(host.test_state(&older).state, State::Failed);
            assert!(cmds.try_recv().is_err(), "nothing was sent to the agent");
        }

        #[tokio::test]
        async fn no_alarm_when_turned_off_or_after_the_limit_cleared() {
            let dir = tempfile::tempdir().unwrap();
            let (host, mut rx) = crate::host::test_host_noting(dir.path(), APPROVAL_LIFETIME_MS);
            let (id, _, _cmds) = host.test_thread(AgentId::Codex);
            let resets = now_ms() + 60_000;
            host.test_step(&id, reached(resets));
            host.limit_alarm.store(false, std::sync::atomic::Ordering::SeqCst);
            host.ring_limit_alarms(resets + LIMIT_ALARM_DELAY_MS);
            host.limit_alarm.store(true, std::sync::atomic::Ordering::SeqCst);
            host.ring_limit_alarms(resets + LIMIT_ALARM_DELAY_MS + 5_000);
            assert!(rung(&mut rx).is_empty(), "off when it was due");

            host.test_step(&id, reached(resets));
            host.test_step(
                &id,
                Step::Limits(vec![Limit {
                    window: "five_hour".into(),
                    used_pct: Some(3),
                    resets_ms: Some(resets + 18_000_000),
                    state: LimitState::Ok,
                }]),
            );
            host.ring_limit_alarms(resets + LIMIT_ALARM_DELAY_MS);
            assert!(rung(&mut rx).is_empty(), "the limit cleared");

            // No thread was stopped by it: nothing to open.
            host.test_step(&id, reached(resets));
            host.ring_limit_alarms(resets + LIMIT_ALARM_DELAY_MS);
            assert_eq!(rung(&mut rx), [(AgentId::Codex, None)]);
        }
    }

    #[test]
    fn limits_are_cleaned_bounded_and_one_per_window() {
        let many: Vec<Limit> = (0..8).map(|n| limit(&format!("w{n}"), 200, LimitState::Ok)).collect();
        let kept = sanitize(many);
        assert_eq!(kept.len(), MAX_LIMITS);
        assert_eq!(kept[0].used_pct, Some(100));
        let odd = sanitize(vec![
            limit(&format!("\u{1b}{}", "x".repeat(40)), 1, LimitState::Ok),
            limit("  ", 1, LimitState::Ok),
            limit("seven_day", 1, LimitState::Ok),
            limit("seven_day", 2, LimitState::Reached),
        ]);
        assert_eq!(odd.len(), 2);
        assert_eq!(odd[0].window.len(), MAX_LIMIT_WINDOW);
        assert_eq!(odd[1].state, LimitState::Ok, "the first of a window is kept");
    }
}
