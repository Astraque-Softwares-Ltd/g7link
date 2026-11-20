// SPDX-License-Identifier: MPL-2.0
//! Usage limits (§Q4.8): the windows each CLI reports during its turns,
//! kept per agent and shown in `AgentInfo.limits`. `agents.changed` for a
//! change of limits alone goes out at most every [`LIMITS_EVENT_MS`] per
//! agent, at once when a window's state changes. G7 never asks a vendor.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::api::*;
use crate::host::Host;

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
    }

    /// An agent reported its usage windows (from any thread).
    pub(crate) fn on_limits(self: &Arc<Self>, agent: AgentId, limits: Vec<Limit>) {
        let announce = self.lock_usage().update(agent, limits, Instant::now());
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
