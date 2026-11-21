// SPDX-License-Identifier: MPL-2.0
//! The timeline on item lists decoded from arbitrary bytes: each item takes
//! a few bytes for its kind, turn, phase, group and tool, and a slice of the
//! input as its text; the host's turns take a few more bytes each, and
//! slices of the input as their files' paths.

#![no_main]

use std::collections::HashSet;

use g7_agents_render::{
    Checkpoint, CheckpointWhy, Ctx, ItemIn, ItemKind, Mark, MarkKind, NoticeKind, Phase, PlanState,
    QuestionIn, QuestionState, RowKind, ThreadState, Timeline, ToolClass, ToolIn, ToolStatus, TurnFileIn,
    TurnFileStatus, TurnIn, Via,
};
use libfuzzer_sys::fuzz_target;

const IDS: [&str; 8] = ["a", "b", "c", "d", "a:0", "b:1", "1", "live"];
const KINDS: [ItemKind; 7] = [
    ItemKind::User,
    ItemKind::Assistant,
    ItemKind::Tool,
    ItemKind::Notice,
    ItemKind::Reasoning,
    ItemKind::Plan,
    ItemKind::Question,
];
const VIAS: [Via; 3] = [Via::Turn, Via::Steer, Via::Queue];
const PLAN_STATES: [Option<PlanState>; 4] =
    [None, Some(PlanState::Proposed), Some(PlanState::Implemented), Some(PlanState::Replaced)];
const QUESTION_STATES: [QuestionState; 4] =
    [QuestionState::Asked, QuestionState::Answered, QuestionState::Dismissed, QuestionState::Expired];
const CLASSES: [ToolClass; 10] = [
    ToolClass::Command,
    ToolClass::Edit,
    ToolClass::Read,
    ToolClass::Search,
    ToolClass::Web,
    ToolClass::Mcp,
    ToolClass::Agent,
    ToolClass::Tasks,
    ToolClass::Other,
    ToolClass::Plan,
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

const NOTICES: [Option<NoticeKind>; 5] = [
    None,
    Some(NoticeKind::Rewound),
    Some(NoticeKind::EditFromHere),
    Some(NoticeKind::FilesRestored),
    Some(NoticeKind::Forked),
];
const CHECKPOINTS: [Checkpoint; 5] =
    [Checkpoint::Pending, Checkpoint::Ready, Checkpoint::Missing, Checkpoint::NotGit, Checkpoint::Expired];
const WHYS: [Option<CheckpointWhy>; 7] = [
    None,
    Some(CheckpointWhy::Busy),
    Some(CheckpointWhy::Slow),
    Some(CheckpointWhy::Large),
    Some(CheckpointWhy::Refused),
    Some(CheckpointWhy::Off),
    Some(CheckpointWhy::Before),
];
const FILE_STATUSES: [TurnFileStatus; 6] = [
    TurnFileStatus::Added,
    TurnFileStatus::Modified,
    TurnFileStatus::Deleted,
    TurnFileStatus::Renamed,
    TurnFileStatus::Type,
    TurnFileStatus::Unrecorded,
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
            name: if h & 128 == 0 { "t" } else { "ToolSearch" },
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
            via: VIAS[(h >> 6) % VIAS.len()],
            plan_state: PLAN_STATES[(h >> 8) % PLAN_STATES.len()],
            question: (h & 256 == 0).then(|| QuestionIn {
                state: QUESTION_STATES[(h >> 10) % QUESTION_STATES.len()],
                count: (h >> 13) as u8 % 6,
                by: if h & 512 == 0 { piece } else { "" },
            }),
            notice: NOTICES[(h >> 15) % NOTICES.len()],
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
        .flat_map(|id| {
            [format!("i:{id}"), format!("g:{id}"), format!("t:{id}"), format!("p:{id}"), format!("q:{id}")]
        })
        .chain(["f:1".into(), "c:1:0".into()])
        .collect();
    let open = &open[..b(0) % open.len()];
    let paths: Vec<TurnFileIn<'_>> = pieces
        .iter()
        .take(24)
        .enumerate()
        .map(|(k, p)| TurnFileIn {
            path: p,
            status: FILE_STATUSES[b(k + 7) % FILE_STATUSES.len()],
            adds: b(k + 8) as u32,
            dels: b(k + 9) as u32,
        })
        .collect();
    let turns: Vec<TurnIn<'_>> = (0..b(6) % 6)
        .map(|k| {
            let h = b(k * 3 + 10) ^ (b(k * 3 + 11) << 8) ^ (b(k * 3 + 12) << 16);
            TurnIn {
                turn: (h % 5) as u32,
                checkpoint: CHECKPOINTS[(h >> 3) % CHECKPOINTS.len()],
                why: WHYS[(h >> 6) % WHYS.len()],
                ended_ms: (h & 512 == 0).then_some((h as u64 >> 10) * 50),
                files: if h & 1024 == 0 { (h >> 11) as u32 % 30 } else { u32::MAX },
                adds: h as u32,
                dels: (h >> 4) as u32,
                skipped: (h >> 13) as u32 % 3,
                paths: &paths[..(h >> 2) % (paths.len() + 1)],
                can_fork: h & 2 == 0,
                can_rewind: h & 4 == 0,
                can_restore: h & 8 == 0,
            }
        })
        .collect();
    let ctx = Ctx {
        state: STATES[b(5) % STATES.len()],
        state_since_ms: 0,
        now_ms: b(4) as u64 * 40,
        marks: &marks,
        open,
        turns: &turns,
    };
    let mut timeline = Timeline::new();
    let rows = timeline.rows(&items, &ctx);
    let keys: HashSet<&str> = rows.iter().map(|r| r.key.as_str()).collect();
    assert_eq!(keys.len(), rows.len());
    for row in rows.iter().filter(|r| !r.acts.is_hidden()) {
        assert_eq!(row.depth, 0);
        assert!(matches!(row.kind, RowKind::User { .. } | RowKind::Block { last: true, .. }));
    }
    assert_eq!(timeline.rows(&items, &ctx), rows);
});
