// SPDX-License-Identifier: MPL-2.0
//! G7 Agents rendering: a thread's items into rows, Markdown into blocks,
//! code into classed tokens and diffs into numbered rows with word marks.
//!
//! Pure functions over untrusted text: no file, network, clock, thread or
//! environment access; every function is total and bounded by [`bounds`].
//! Nothing here returns a sentence: clients word labels, summaries and
//! states through their own translations.
//!
//! Every offset and length into text is in UTF-16 code units.

#![forbid(unsafe_code)]

pub mod bounds;
mod diff;
mod guard;
mod highlight;
mod label;
mod markdown;
mod stream;
mod timeline;

pub use diff::{DiffDoc, DiffFile, DiffKind, DiffRow, diff};
pub use guard::{GuardEvent, TypingGuard};
pub use highlight::{ALIASES, CodeLine, Tok, TokClass, highlight};
pub use label::{Clause, Summary, ToolLabel, live_thought_line, summary, tool_label};
pub use markdown::{
    AlertKind, Align, Block, BlockKind, Cell, Doc, Link, Marker, Span, Style, Table, markdown,
};
pub use timeline::{
    Ctx, DetailOf, FileStat, ItemIn, ItemKind, LiveLabel, Mark, MarkKind, Phase, Row, RowKind, RowStatus,
    ThreadState, Timeline, ToolClass, ToolIn, ToolStatus, changed_files,
};
