// SPDX-License-Identifier: MPL-2.0
//! G7 Agents rendering: a thread's items into rows, Markdown into blocks,
//! code into classed tokens, diffs into numbered rows with word marks,
//! search snippets into parts with the matches marked, and a message into
//! words to read aloud.
//!
//! Pure functions over untrusted text: no file, network, clock, thread or
//! environment access; every function is total and bounded by [`bounds`].
//! Nothing here returns a sentence: clients word labels, summaries and
//! states through their own translations.
//!
//! Every offset and length into text is in UTF-16 code units.

#![forbid(unsafe_code)]

pub mod bounds;
mod composer;
mod detect;
mod diff;
mod guard;
mod highlight;
mod label;
mod markdown;
mod output;
mod snippet;
mod speak;
mod stream;
mod timeline;

pub use composer::{Trigger, TriggerKind, apply, mentions, trigger};
pub use detect::guess;
pub use diff::{DiffDoc, DiffFile, DiffKind, DiffNote, DiffRow, diff};
pub use guard::{GuardEvent, TypingGuard};
pub use highlight::{ALIASES, CodeLine, Tok, TokClass, highlight, languages};
pub use label::{Clause, Summary, ToolLabel, live_thought_line, summary, tool_label};
pub use markdown::{
    AlertKind, Align, Block, BlockKind, Cell, Doc, Hints, Link, Marker, Span, Style, Table, markdown,
    markdown_with,
};
pub use output::output_lines;
pub use snippet::{SnippetPart, snippet};
pub use speak::speakable;
pub use timeline::{
    Act, Acts, Checkpoint, CheckpointWhy, Ctx, DetailOf, FileStat, ItemIn, ItemKind, LiveLabel, Mark,
    MarkKind, NoteKind, NoticeKind, Phase, PlanState, QuestionIn, QuestionState, Row, RowKind, RowStatus,
    ThreadState, Timeline, ToolClass, ToolIn, ToolStatus, TurnFileIn, TurnFileStatus, TurnIn, Via,
    changed_files,
};
