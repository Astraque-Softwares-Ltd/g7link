// SPDX-License-Identifier: MPL-2.0
//! The timeline on item lists decoded from arbitrary bytes: each item takes
//! a few bytes for its kind, turn, phase, group and tool, and a slice of the
//! input as its text.

#![no_main]

use std::collections::HashSet;

use g7_agents_render::{
    Ctx, ItemIn, ItemKind, Mark, MarkKind, Phase, ThreadState, Timeline, ToolClass, ToolIn, ToolStatus,
};
use libfuzzer_sys::fuzz_target;

const IDS: [&str; 8] = ["a", "b", "c", "d", "a:0", "b:1", "1", "live"];
const KINDS: [ItemKind; 5] =
    [ItemKind::User, ItemKind::Assistant, ItemKind::Tool, ItemKind::Notice, ItemKind::Reasoning];
const CLASSES: [ToolClass; 9] = [
    ToolClass::Command,
    ToolClass::Edit,
    ToolClass::Read,
    ToolClass::Search,
    ToolClass::Web,
    ToolClass::Mcp,
    ToolClass::Agent,
    ToolClass::Tasks,
    ToolClass::Other,
];
const STATUSES: [ToolStatus; 4] =
    [ToolStatus::Running, ToolStatus::Ok, ToolStatus::Error, ToolStatus::Denied];
const STATES: [ThreadState; 5] = [
    ThreadState::Working,
    ThreadState::Waiting,
    ThreadState::Done,
    ThreadState::Failed,
    ThreadState::Interrupted,
];
const MARKS: [MarkKind; 7] = [
    MarkKind::Waiting,
    MarkKind::Held,
    MarkKind::Allowed,
    MarkKind::Denied,
    MarkKind::Expired,
    MarkKind::Cancelled,
    MarkKind::Rule,
];

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let pieces: Vec<&str> = text.split('\u{1e}').collect();
    let b = |i: usize| data.get(i).copied().unwrap_or(0) as usize;
    let mut items = Vec::new();
    for (k, piece) in pieces.iter().enumerate().take(64) {
        let h = b(k * 3) ^ (b(k * 3 + 1) << 8) ^ (b(k * 3 + 2) << 16);
        let kind = KINDS[h % KINDS.len()];
        let tool = (kind == ItemKind::Tool).then(|| ToolIn {
            name: "t",
            title: piece,
            class: CLASSES[(h >> 3) % CLASSES.len()],
            detail: piece,
            path: if h & 1 == 0 { "src/a.rs" } else { "" },
            status: STATUSES[(h >> 7) % STATUSES.len()],
            output: piece,
            by_rule: h & 2 == 0,
        });
        items.push(ItemIn {
            seq: k as u64 + 1,
            id: IDS[(h >> 9) % IDS.len()],
            kind,
            ts_ms: (h as u64) * 7,
            end_ms: (h & 4 == 0).then_some(h as u64 * 9),
            rev: (h >> 12) as u32 % 3,
            text: piece,
            streaming: h & 8 == 0,
            turn: ((h >> 14) % 4) as u32,
            group: (h & 16 == 0).then(|| IDS[(h >> 2) % IDS.len()]),
            phase: [None, Some(Phase::Interim), Some(Phase::Final)][(h >> 5) % 3],
            from: "",
            deny_of: (h & 32 == 0).then(|| IDS[(h >> 4) % IDS.len()]),
            tool,
            truncated: h & 64 == 0,
        });
    }
    let marks: Vec<Mark> = (0..b(1) % 4)
        .map(|k| Mark {
            item_id: IDS[b(k + 2) % IDS.len()].to_owned(),
            kind: MARKS[b(k + 3) % MARKS.len()],
            by: String::new(),
        })
        .collect();
    let open: Vec<String> = IDS
        .iter()
        .flat_map(|id| [format!("i:{id}"), format!("g:{id}"), format!("t:{id}")])
        .chain(["f:1".into(), "c:1:0".into()])
        .collect();
    let open = &open[..b(0) % open.len()];
    let ctx = Ctx { state: STATES[b(5) % STATES.len()], state_since_ms: 0, now_ms: 0, marks: &marks, open };
    let mut timeline = Timeline::new();
    let rows = timeline.rows(&items, &ctx);
    let keys: HashSet<&str> = rows.iter().map(|r| r.key.as_str()).collect();
    assert_eq!(keys.len(), rows.len());
    assert_eq!(timeline.rows(&items, &ctx), rows);
});
