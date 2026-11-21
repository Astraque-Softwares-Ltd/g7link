// SPDX-License-Identifier: MPL-2.0
//! Markdown into a flat list of blocks with inline spans.
//!
//! The parser is pulldown-cmark with tables, strikethrough, task lists and
//! GFM alerts on, and nothing else. Nothing in the output can load or run
//! anything: HTML is shown as text, images become their alt text, and links
//! survive only with an `http`, `https` or `mailto` target.
//!
//! Reference-style links (`[text][label]`, `[text]`) are shown as written,
//! never resolved: a definition later in a message would otherwise change a
//! block that has already been shown as final while the message streams.

use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd,
};
use serde::{Deserialize, Serialize};

use std::collections::HashSet;

use crate::bounds::{
    MAX_BLOCKS, MAX_DEPTH, MAX_HREF_BYTES, MAX_PATH_HINTS, MAX_SPANS_PER_BLOCK, MAX_SRC_BYTES,
    MAX_TABLE_COLS, MAX_TABLE_ROWS, clean, cut_bytes, sat32, units32,
};
use crate::detect::guess;
use crate::highlight::{CodeLine, highlight};

/// A parsed Markdown text.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Doc {
    pub blocks: Vec<Block>,
    /// The text was too complex (more than `MAX_BLOCKS` blocks) and is the
    /// single paragraph in `blocks`, with no spans.
    #[serde(default, skip_serializing_if = "is_false")]
    pub plain: bool,
}

/// One paragraph, heading, list item, code block, table or rule.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Block {
    pub kind: BlockKind,
    /// List nesting, `0..=MAX_DEPTH`.
    #[serde(skip_serializing_if = "is_zero")]
    pub depth: u8,
    /// Block-quote nesting, `0..=MAX_DEPTH`.
    #[serde(skip_serializing_if = "is_zero")]
    pub quote: u8,
    /// For `ListItem`: the bullet, number or check box; `None` on an item's
    /// later paragraphs.
    #[serde(skip_serializing_if = "Marker::is_none")]
    pub marker: Marker,
    /// Paragraph, heading, list item, alert: the visible text. Code: the source.
    pub text: String,
    /// Over `text`, in UTF-16 units: sorted, non-overlapping; gaps are plain.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<Span>,
    /// Code: the fence's language word, lowercased, at most 32 bytes; or,
    /// for a block that names none, the word [`crate::guess`] found.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub lang: String,
    /// Code: `lang` is a guess, not the fence's word.
    #[serde(skip_serializing_if = "is_false")]
    pub lang_guessed: bool,
    /// Code: the coloured lines; empty when not highlighted.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<CodeLine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table: Option<Table>,
    /// Part of a text that is still arriving: it may still change.
    #[serde(skip_serializing_if = "is_false")]
    pub open: bool,
}

impl std::fmt::Debug for Block {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Block")
            .field("kind", &self.kind)
            .field("depth", &self.depth)
            .field("quote", &self.quote)
            .field("marker", &self.marker)
            .field("text_bytes", &self.text.len())
            .field("spans", &self.spans.len())
            .field("lines", &self.lines.len())
            .field("open", &self.open)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    #[default]
    Paragraph,
    /// Level 1 to 6.
    Heading(u8),
    ListItem,
    Code,
    Table,
    Rule,
    Alert(AlertKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Marker {
    #[default]
    None,
    Bullet,
    Number(u32),
    Todo,
    Done,
}

impl Marker {
    pub fn is_none(&self) -> bool {
        matches!(self, Marker::None)
    }
}

/// A styled stretch of a block's text, in UTF-16 units.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Span {
    pub start: u32,
    pub len: u32,
    pub style: Style,
    /// The link's target; empty unless `style.link` is not `None`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub href: String,
}

impl std::fmt::Debug for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Span")
            .field("start", &self.start)
            .field("len", &self.len)
            .field("style", &self.style)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
    #[serde(skip_serializing_if = "is_false")]
    pub strong: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub em: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub strike: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub code: bool,
    #[serde(skip_serializing_if = "Link::is_none")]
    pub link: Link,
    /// The alt text of an image that is a link (never loaded).
    #[serde(skip_serializing_if = "is_false")]
    pub image: bool,
    /// Inline code that names a file the turn changed (its whole path, or
    /// the end of it after a `/`): the client draws it as a path.
    #[serde(skip_serializing_if = "is_false")]
    pub path: bool,
}

impl Style {
    fn is_plain(&self) -> bool {
        *self == Style::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Link {
    #[default]
    None,
    /// `http` or `https`; the client shows the target before opening it.
    Web,
    /// `mailto`; the client shows the target before opening it.
    Mail,
    /// The visible text is the target itself.
    Shown,
}

impl Link {
    pub fn is_none(&self) -> bool {
        matches!(self, Link::None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Table {
    pub columns: u8,
    pub align: Vec<Align>,
    /// Row-major; the header row first. Every row has `columns` cells.
    pub cells: Vec<Cell>,
    /// Body rows past `MAX_TABLE_ROWS`, not included in `cells`.
    #[serde(skip_serializing_if = "is_zero")]
    pub more_rows: u32,
}

#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Cell {
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<Span>,
    #[serde(skip_serializing_if = "is_false")]
    pub header: bool,
}

impl std::fmt::Debug for Cell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cell")
            .field("text_bytes", &self.text.len())
            .field("spans", &self.spans)
            .field("header", &self.header)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    #[default]
    None,
    Left,
    Center,
    Right,
}

pub(crate) fn is_false(b: &bool) -> bool {
    !*b
}

pub(crate) fn is_zero<T: Default + PartialEq>(n: &T) -> bool {
    *n == T::default()
}

/// Parses `src` (cut to `MAX_SRC_BYTES`, cleaned) into blocks, with code
/// blocks highlighted.
pub fn markdown(src: &str) -> Doc {
    markdown_with(src, Hints::default())
}

/// What a text's surroundings say about it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Hints<'a> {
    /// The extension of a file the same turn edited or read last: a code
    /// block that names no language and could be two is taken as this one's.
    pub ext: Option<&'a str>,
    /// The files the turn changed: inline code naming one is marked `path`.
    pub paths: &'a [&'a str],
}

/// [`markdown`] with what the text's surroundings say.
pub fn markdown_with(src: &str, hints: Hints<'_>) -> Doc {
    let src = clean(cut_bytes(src, MAX_SRC_BYTES));
    let mut parsed = parse(&src);
    finish(&mut parsed.blocks, hints);
    Doc { blocks: parsed.blocks, plain: parsed.plain }
}

/// What a parse leaves to the hints: colours code ([`fill_code`]) and
/// marks inline code that names a file ([`mark_paths`]).
pub(crate) fn finish(blocks: &mut [Block], hints: Hints<'_>) {
    fill_code(blocks, hints);
    mark_paths(blocks, hints.paths);
}

/// Marks inline code whose text is one of `paths` or ends one after a `/`.
pub(crate) fn mark_paths(blocks: &mut [Block], paths: &[&str]) {
    if paths.is_empty() {
        return;
    }
    let mut names: HashSet<&str> = HashSet::new();
    for path in paths.iter().take(MAX_PATH_HINTS) {
        names.insert(path);
        names.extend(path.match_indices('/').map(|(i, _)| &path[i + 1..]).filter(|s| !s.is_empty()));
    }
    let mark = |text: &str, spans: &mut [Span]| {
        if !spans.iter().any(|s| s.style.code) {
            return;
        }
        let units: Vec<u16> = text.encode_utf16().collect();
        for span in spans.iter_mut().filter(|s| s.style.code) {
            let (start, end) = (span.start as usize, span.start.saturating_add(span.len) as usize);
            if let Some(piece) = units.get(start..end) {
                span.style.path = names.contains(String::from_utf16_lossy(piece).as_str());
            }
        }
    };
    for block in blocks.iter_mut() {
        mark(&block.text, &mut block.spans);
        if let Some(table) = block.table.as_mut() {
            for cell in &mut table.cells {
                mark(&cell.text, &mut cell.spans);
            }
        }
    }
}

/// Colours the code blocks that are not open and not coloured yet, first
/// guessing the language of one that names none.
pub(crate) fn fill_code(blocks: &mut [Block], hints: Hints<'_>) {
    for b in blocks.iter_mut().filter(|b| b.kind == BlockKind::Code && !b.open && b.lines.is_empty()) {
        if b.lang.is_empty()
            && let Some(word) = guess(&b.text, hints.ext)
        {
            word.clone_into(&mut b.lang);
            b.lang_guessed = true;
        }
        b.lines = highlight(&b.lang, &b.text);
    }
}

/// The blocks of `src`, which must already be cut and cleaned, with the
/// source offsets each block starts and ends at (the end of a list item's
/// marker block is the end of the whole item).
#[derive(Debug, Default)]
pub(crate) struct Parsed {
    pub blocks: Vec<Block>,
    pub starts: Vec<usize>,
    pub ends: Vec<usize>,
    pub plain: bool,
}

/// `src` as one plain paragraph: the fallback past `MAX_BLOCKS`.
pub(crate) fn plain_doc(src: &str) -> Parsed {
    Parsed {
        blocks: vec![Block { text: src.to_owned(), ..Block::default() }],
        starts: vec![0],
        ends: vec![src.len()],
        plain: true,
    }
}

pub(crate) fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_GFM
}

/// Parses already cut and cleaned text; code is not coloured ([`fill_code`]
/// does that).
pub(crate) fn parse(src: &str) -> Parsed {
    let mut b = Builder::new(src);
    for (event, range) in Parser::new_ext(src, options()).into_offset_iter() {
        b.event(event, range.start, range.end);
        if b.blocks.len() > MAX_BLOCKS {
            return plain_doc(src);
        }
    }
    b.finish_inline();
    if b.blocks.len() > MAX_BLOCKS {
        return plain_doc(src);
    }
    Parsed { blocks: b.blocks, starts: b.starts, ends: b.ends, plain: false }
}

/// Text with spans, being built for a block or a table cell.
#[derive(Default)]
struct Inline {
    text: String,
    units: u32,
    spans: Vec<Span>,
}

impl Inline {
    fn push(&mut self, s: &str, style: Style, href: &str) {
        if s.is_empty() {
            return;
        }
        let len = units32(s);
        if !style.is_plain() {
            match self.spans.last_mut() {
                Some(last)
                    if last.style == style
                        && last.href == href
                        && last.start.saturating_add(last.len) == self.units =>
                {
                    last.len = last.len.saturating_add(len);
                }
                _ => self.spans.push(Span { start: self.units, len, style, href: href.to_owned() }),
            }
        }
        self.text.push_str(s);
        self.units = self.units.saturating_add(len);
    }

    fn into_parts(self) -> (String, Vec<Span>) {
        let spans = if self.spans.len() > MAX_SPANS_PER_BLOCK { Vec::new() } else { self.spans };
        (self.text, spans)
    }
}

/// A link or image being read.
struct LinkFrame {
    /// The link this frame applies; `None` when it is not a link we keep.
    kind: Link,
    href: String,
    /// Whether this frame decides the link (false: it inherits an outer one).
    own: bool,
    /// Where its visible text starts in the inline text, and its first span.
    text_start: usize,
    span_start: usize,
    /// Reference-style links are shown as written: text appended at the end.
    suffix: Option<String>,
    /// Inside an image: its alt text, marked so a client can show a glyph.
    image: bool,
}

struct ListFrame {
    next: Option<u64>,
}

struct ItemFrame {
    marker: Marker,
    /// Whether the item has produced a block yet.
    started: bool,
    start: usize,
    end: usize,
}

struct QuoteFrame {
    alert: Option<AlertKind>,
    /// Whether a block has been produced inside it yet.
    started: bool,
    start: usize,
    end: usize,
}

enum InlineKind {
    Paragraph,
    Heading(u8),
    ListItem,
}

struct OpenInline {
    kind: InlineKind,
    marker: Marker,
    start: usize,
    inline: Inline,
}

struct CodeBuild {
    lang: String,
    text: String,
    start: usize,
}

struct TableBuild {
    columns: usize,
    align: Vec<Align>,
    cells: Vec<Cell>,
    row: Vec<Cell>,
    header: bool,
    body_rows: usize,
    more_rows: usize,
    cell: Option<Inline>,
    start: usize,
}

struct Builder<'s> {
    src: &'s str,
    blocks: Vec<Block>,
    starts: Vec<usize>,
    ends: Vec<usize>,
    /// Where a block finished now ends: the current event's end, or its
    /// start when it is a new element that closes the one before.
    cur_end: usize,
    quotes: Vec<QuoteFrame>,
    lists: Vec<ListFrame>,
    items: Vec<ItemFrame>,
    strong: u32,
    em: u32,
    strike: u32,
    links: Vec<LinkFrame>,
    open: Option<OpenInline>,
    code: Option<CodeBuild>,
    table: Option<TableBuild>,
    /// Inside a table too wide to keep: events are skipped until its end.
    skip_table: Option<usize>,
    html_block: bool,
}

impl<'s> Builder<'s> {
    fn new(src: &'s str) -> Self {
        Builder {
            src,
            blocks: Vec::new(),
            starts: Vec::new(),
            ends: Vec::new(),
            cur_end: 0,
            quotes: Vec::new(),
            lists: Vec::new(),
            items: Vec::new(),
            strong: 0,
            em: 0,
            strike: 0,
            links: Vec::new(),
            open: None,
            code: None,
            table: None,
            skip_table: None,
            html_block: false,
        }
    }

    fn depth(&self) -> u8 {
        u8::try_from(self.lists.len().saturating_sub(1)).unwrap_or(u8::MAX).min(MAX_DEPTH)
    }

    fn quote(&self) -> u8 {
        u8::try_from(self.quotes.len()).unwrap_or(u8::MAX).min(MAX_DEPTH)
    }

    fn style(&self, code: bool) -> (Style, String) {
        let (link, href, image) = match self.links.last() {
            Some(f) => (f.kind, f.href.clone(), f.image),
            None => (Link::None, String::new(), false),
        };
        let image = image && !link.is_none();
        let style = Style {
            strong: self.strong > 0,
            em: self.em > 0,
            strike: self.strike > 0,
            code,
            link,
            image,
            path: false,
        };
        (style, href)
    }

    /// Adds a finished block, applying a pending alert and list marker.
    fn push_block(&mut self, mut block: Block, start: usize) {
        block.depth = if matches!(block.kind, BlockKind::ListItem) || !self.items.is_empty() {
            self.depth()
        } else {
            0
        };
        block.quote = self.quote();
        let pending: Vec<(Option<AlertKind>, (usize, usize), u8)> = self
            .quotes
            .iter_mut()
            .enumerate()
            .filter(|(_, q)| !q.started)
            .map(|(i, q)| {
                q.started = true;
                (q.alert, (q.start, q.end), u8::try_from(i + 1).unwrap_or(u8::MAX).min(MAX_DEPTH))
            })
            .collect();
        let alerts: Vec<_> = pending.into_iter().filter(|(a, _, _)| a.is_some()).collect();
        for (n, (alert, (qstart, qend), quote)) in alerts.iter().enumerate() {
            let Some(kind) = alert else { continue };
            let last = n + 1 == alerts.len();
            if last && matches!(block.kind, BlockKind::Paragraph) && block.marker.is_none() {
                block.kind = BlockKind::Alert(*kind);
            } else {
                self.blocks.push(Block {
                    kind: BlockKind::Alert(*kind),
                    depth: block.depth,
                    quote: *quote,
                    ..Block::default()
                });
                self.starts.push(*qstart);
                self.ends.push((*qend).max(self.cur_end));
            }
        }
        if let Some(item) = self.items.last_mut()
            && !item.started
        {
            item.started = true;
            if matches!(block.kind, BlockKind::ListItem) {
                block.marker = item.marker;
            } else {
                let marker = item.marker;
                let (start, end) = (item.start, item.end.max(self.cur_end));
                self.blocks.push(Block {
                    kind: BlockKind::ListItem,
                    depth: block.depth,
                    quote: block.quote,
                    marker,
                    ..Block::default()
                });
                self.starts.push(start);
                self.ends.push(end);
            }
        }
        self.blocks.push(block);
        self.starts.push(start);
        self.ends.push(self.cur_end.max(start));
    }

    fn open_inline(&mut self, kind: InlineKind, start: usize) {
        self.finish_inline();
        self.open = Some(OpenInline { kind, marker: Marker::None, start, inline: Inline::default() });
    }

    /// Where inline content goes: the open table cell, else the open block,
    /// opened on demand (tight list items have no paragraph events).
    fn inline_target(&mut self, at: usize) -> &mut Inline {
        let Builder { table, open, items, .. } = self;
        if let Some(TableBuild { cell: Some(cell), .. }) = table {
            return cell;
        }
        let open = open.get_or_insert_with(|| {
            let kind = if items.is_empty() { InlineKind::Paragraph } else { InlineKind::ListItem };
            let start = match items.last() {
                Some(item) if !item.started => item.start,
                _ => at,
            };
            OpenInline { kind, marker: Marker::None, start, inline: Inline::default() }
        });
        &mut open.inline
    }

    /// Character references (`&#x202e;`) decode after the input was
    /// cleaned, so what the parser hands back is cleaned again.
    fn push_text(&mut self, s: &str, code: bool, at: usize) {
        let (style, href) = self.style(code);
        let s = clean(s);
        self.inline_target(at).push(&s, style, &href);
    }

    fn finish_inline(&mut self) {
        let Some(open) = self.open.take() else { return };
        // A link left open (cannot happen with a well-formed event stream).
        self.links.clear();
        let (mut text, spans) = open.inline.into_parts();
        let kind = match open.kind {
            InlineKind::Paragraph => BlockKind::Paragraph,
            InlineKind::Heading(level) => BlockKind::Heading(level),
            InlineKind::ListItem => BlockKind::ListItem,
        };
        if self.html_block {
            while text.ends_with('\n') {
                text.pop();
            }
        }
        let spans = clip_spans(spans, &text);
        self.push_block(Block { kind, marker: open.marker, text, spans, ..Block::default() }, open.start);
    }

    fn event(&mut self, event: Event<'_>, start: usize, end: usize) {
        self.cur_end = if matches!(event, Event::Start(_)) { start } else { end };
        if let Some(table_start) = self.skip_table {
            if matches!(event, Event::End(TagEnd::Table)) {
                self.skip_table = None;
                let mut text = self.src.get(table_start..end).unwrap_or_default().to_owned();
                while text.ends_with('\n') {
                    text.pop();
                }
                self.push_block(Block { kind: BlockKind::Code, text, ..Block::default() }, table_start);
            }
            return;
        }
        if let Some(code) = self.code.as_mut() {
            match event {
                Event::Text(t) => code.text.push_str(&clean(&t)),
                Event::End(TagEnd::CodeBlock) => self.finish_code(),
                _ => {}
            }
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag, start, end),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.push_text(&t, false, start),
            Event::Code(t) => self.push_text(&t, true, start),
            Event::InlineMath(t) | Event::DisplayMath(t) => self.push_text(&t, false, start),
            Event::Html(t) | Event::InlineHtml(t) => self.push_text(&t, false, start),
            Event::FootnoteReference(t) => {
                let s = format!("[^{t}]");
                self.push_text(&s, false, start)
            }
            Event::SoftBreak => self.push_text(" ", false, start),
            Event::HardBreak => self.push_text("\n", false, start),
            Event::Rule => {
                self.finish_inline();
                self.push_block(Block { kind: BlockKind::Rule, ..Block::default() }, start);
            }
            Event::TaskListMarker(done) => {
                let marker = if done { Marker::Done } else { Marker::Todo };
                if let Some(item) = self.items.last_mut() {
                    item.marker = marker;
                }
            }
        }
    }

    /// `start..end` is the whole element's source.
    fn start(&mut self, tag: Tag<'_>, start: usize, end: usize) {
        match tag {
            Tag::Paragraph => {
                let kind = if self.items.is_empty() { InlineKind::Paragraph } else { InlineKind::ListItem };
                let start = match self.items.last() {
                    Some(item) if !item.started => item.start,
                    _ => start,
                };
                self.open_inline(kind, start);
            }
            Tag::Heading { level, .. } => self.open_inline(InlineKind::Heading(heading_level(level)), start),
            Tag::BlockQuote(kind) => {
                self.finish_inline();
                self.quotes.push(QuoteFrame { alert: kind.map(alert_kind), started: false, start, end });
            }
            Tag::CodeBlock(kind) => {
                self.finish_inline();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => lang_word(&info),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some(CodeBuild { lang, text: String::new(), start });
            }
            Tag::HtmlBlock => {
                self.open_inline(InlineKind::Paragraph, start);
                self.html_block = true;
            }
            Tag::List(first) => {
                self.finish_inline();
                self.lists.push(ListFrame { next: first });
            }
            Tag::Item => {
                self.finish_inline();
                let marker = match self.lists.last_mut() {
                    Some(ListFrame { next: Some(n) }) => {
                        let m = Marker::Number(u32::try_from(*n).unwrap_or(u32::MAX));
                        *n = n.saturating_add(1);
                        m
                    }
                    _ => Marker::Bullet,
                };
                self.items.push(ItemFrame { marker, started: false, start, end });
            }
            Tag::Table(aligns) => {
                self.finish_inline();
                if aligns.len() > MAX_TABLE_COLS {
                    self.skip_table = Some(start);
                    return;
                }
                self.table = Some(TableBuild {
                    columns: aligns.len(),
                    align: aligns.iter().map(|a| align(*a)).collect(),
                    cells: Vec::new(),
                    row: Vec::new(),
                    header: false,
                    body_rows: 0,
                    more_rows: 0,
                    cell: None,
                    start,
                });
            }
            Tag::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.header = true;
                    t.row.clear();
                }
            }
            Tag::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    t.header = false;
                    t.row.clear();
                }
            }
            Tag::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    t.cell = Some(Inline::default());
                }
            }
            Tag::Emphasis => self.em += 1,
            Tag::Strong => self.strong += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { link_type, dest_url, id, .. } => {
                self.start_link(link_type, &dest_url, &id, false, start)
            }
            Tag::Image { link_type, dest_url, id, .. } => {
                self.start_link(link_type, &dest_url, &id, true, start)
            }
            Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_) => self.finish_inline(),
            Tag::Superscript | Tag::Subscript => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) => self.finish_inline(),
            TagEnd::HtmlBlock => {
                self.finish_inline();
                self.html_block = false;
            }
            TagEnd::BlockQuote(_) => {
                self.finish_inline();
                if let Some(q) = self.quotes.last()
                    && !q.started
                    && q.alert.is_some()
                {
                    let start = q.start;
                    self.push_block(Block::default(), start);
                }
                self.quotes.pop();
            }
            TagEnd::CodeBlock => self.finish_code(),
            TagEnd::List(_) => {
                self.finish_inline();
                self.lists.pop();
            }
            TagEnd::Item => {
                self.finish_inline();
                if let Some(item) = self.items.last()
                    && !item.started
                {
                    let start = item.start;
                    self.push_block(Block { kind: BlockKind::ListItem, ..Block::default() }, start);
                }
                self.items.pop();
            }
            TagEnd::Table => self.finish_table(),
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    let mut row = std::mem::take(&mut t.row);
                    row.resize_with(t.columns, Cell::default);
                    for cell in &mut row {
                        cell.header = t.header;
                    }
                    if t.header {
                        t.cells.extend(row);
                    } else if t.body_rows < MAX_TABLE_ROWS {
                        t.body_rows += 1;
                        t.cells.extend(row);
                    } else {
                        t.more_rows += 1;
                    }
                    t.header = false;
                }
            }
            TagEnd::TableCell => {
                if let Some(t) = self.table.as_mut()
                    && let Some(inline) = t.cell.take()
                {
                    let (text, spans) = inline.into_parts();
                    let spans = clip_spans(spans, &text);
                    t.row.push(Cell { text, spans, header: false });
                }
            }
            TagEnd::Emphasis => self.em = self.em.saturating_sub(1),
            TagEnd::Strong => self.strong = self.strong.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link | TagEnd::Image => self.end_link(),
            TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::MetadataBlock(_) => self.finish_inline(),
            TagEnd::Superscript | TagEnd::Subscript => {}
        }
    }

    fn start_link(&mut self, link_type: LinkType, dest: &str, id: &str, image: bool, at: usize) {
        let (text_start, span_start) = {
            let target = self.inline_target(at);
            (target.text.len(), target.spans.len())
        };
        let reference = matches!(
            link_type,
            LinkType::Reference
                | LinkType::ReferenceUnknown
                | LinkType::Collapsed
                | LinkType::CollapsedUnknown
                | LinkType::Shortcut
                | LinkType::ShortcutUnknown
        );
        if reference {
            let suffix = match link_type {
                LinkType::Reference | LinkType::ReferenceUnknown => format!("][{id}]"),
                LinkType::Collapsed | LinkType::CollapsedUnknown => "][]".to_owned(),
                _ => "]".to_owned(),
            };
            let prefix = if image { "![" } else { "[" };
            self.push_text(prefix, false, at);
            let (kind, href) = match self.links.last() {
                Some(f) => (f.kind, f.href.clone()),
                None => (Link::None, String::new()),
            };
            self.links.push(LinkFrame {
                kind,
                href,
                own: false,
                text_start,
                span_start,
                suffix: Some(suffix),
                image: false,
            });
            return;
        }
        let outer =
            self.links.last().filter(|f| !f.kind.is_none()).map(|f| (f.kind, f.href.clone(), f.image));
        let frame = match outer {
            // An image inside a link keeps the outer link.
            Some((kind, href, outer_image)) => LinkFrame {
                kind,
                href,
                own: false,
                text_start,
                span_start,
                suffix: None,
                image: image || outer_image,
            },
            None => {
                let (kind, href) = link_target(link_type, dest);
                LinkFrame { kind, href, own: true, text_start, span_start, suffix: None, image }
            }
        };
        self.links.push(frame);
    }

    fn end_link(&mut self) {
        let Some(frame) = self.links.pop() else { return };
        if let Some(suffix) = frame.suffix {
            self.push_text(&suffix, false, 0);
            return;
        }
        if !frame.own || frame.kind.is_none() {
            return;
        }
        let target = self.inline_target(0);
        let visible = target.text.get(frame.text_start..).unwrap_or_default().trim();
        if shows_target(visible, &frame.href) {
            for span in target.spans.iter_mut().skip(frame.span_start) {
                if span.href == frame.href {
                    span.style.link = Link::Shown;
                }
            }
        }
    }

    fn finish_code(&mut self) {
        let Some(code) = self.code.take() else { return };
        let mut text = code.text;
        if text.ends_with('\n') {
            text.pop();
        }
        self.push_block(
            Block { kind: BlockKind::Code, text, lang: code.lang, ..Block::default() },
            code.start,
        );
    }

    fn finish_table(&mut self) {
        let Some(t) = self.table.take() else { return };
        let table = Table {
            columns: u8::try_from(t.columns).unwrap_or(u8::MAX),
            align: t.align,
            cells: t.cells,
            more_rows: sat32(t.more_rows),
        };
        self.push_block(Block { kind: BlockKind::Table, table: Some(table), ..Block::default() }, t.start);
    }
}

/// Drops spans that would reach past the text (cannot happen for spans
/// built here; kept as a guard for the bound the clients rely on).
fn clip_spans(spans: Vec<Span>, text: &str) -> Vec<Span> {
    let total = units32(text);
    spans.into_iter().filter(|s| s.len > 0 && s.start.saturating_add(s.len) <= total).collect()
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn alert_kind(kind: BlockQuoteKind) -> AlertKind {
    match kind {
        BlockQuoteKind::Note => AlertKind::Note,
        BlockQuoteKind::Tip => AlertKind::Tip,
        BlockQuoteKind::Important => AlertKind::Important,
        BlockQuoteKind::Warning => AlertKind::Warning,
        BlockQuoteKind::Caution => AlertKind::Caution,
    }
}

fn align(a: Alignment) -> Align {
    match a {
        Alignment::None => Align::None,
        Alignment::Left => Align::Left,
        Alignment::Center => Align::Center,
        Alignment::Right => Align::Right,
    }
}

/// The language word of a fence's info string: its first word, lowercased,
/// at most 32 bytes. The rest (often a file name) is dropped.
pub(crate) fn lang_word(info: &str) -> String {
    let info = clean(info);
    let word = info.split_whitespace().next().unwrap_or_default();
    cut_bytes(&word.to_lowercase(), 32).to_owned()
}

/// The link a destination is allowed to be, and its target.
pub(crate) fn link_target(link_type: LinkType, dest: &str) -> (Link, String) {
    let dest = clean(dest);
    let dest = dest.trim();
    let href = if matches!(link_type, LinkType::Email) { format!("mailto:{dest}") } else { dest.to_owned() };
    if href.len() > MAX_HREF_BYTES {
        return (Link::None, String::new());
    }
    match scheme(&href) {
        Some("http" | "https") => (Link::Web, href),
        Some("mailto") => (Link::Mail, href),
        _ => (Link::None, String::new()),
    }
}

/// The scheme of `href`, lowercased, when it is one we know.
fn scheme(href: &str) -> Option<&'static str> {
    let colon = href.find(':')?;
    let s = href.get(..colon)?;
    ["https", "http", "mailto"].into_iter().find(|k| s.eq_ignore_ascii_case(k))
}

/// Whether a link's visible text is its target, with or without the scheme.
pub(crate) fn shows_target(visible: &str, href: &str) -> bool {
    if visible.is_empty() {
        return false;
    }
    if visible == href {
        return true;
    }
    let Some(colon) = href.find(':') else { return false };
    let rest = &href[colon + 1..];
    visible == rest || visible == rest.trim_start_matches('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(doc: &Doc) -> Vec<BlockKind> {
        doc.blocks.iter().map(|b| b.kind).collect()
    }

    #[test]
    fn paragraphs_headings_and_breaks() {
        let doc = markdown("# Title\n\nOne\ntwo  \nthree");
        assert_eq!(kinds(&doc), [BlockKind::Heading(1), BlockKind::Paragraph]);
        assert_eq!(doc.blocks[0].text, "Title");
        assert_eq!(doc.blocks[1].text, "One two\nthree");
    }

    #[test]
    fn inline_styles_become_spans_in_utf16_units() {
        let doc = markdown("😀 **bold** and `code`");
        let b = &doc.blocks[0];
        assert_eq!(b.text, "😀 bold and code");
        assert_eq!(b.spans.len(), 2);
        assert_eq!((b.spans[0].start, b.spans[0].len), (3, 4));
        assert!(b.spans[0].style.strong);
        assert_eq!((b.spans[1].start, b.spans[1].len), (12, 4));
        assert!(b.spans[1].style.code);
    }

    #[test]
    fn lists_nest_and_carry_markers() {
        let doc = markdown("1. one\n2. two\n   - [x] done\n   - [ ] todo\n\n   more");
        let got: Vec<_> = doc.blocks.iter().map(|b| (b.kind, b.depth, b.marker, b.text.as_str())).collect();
        assert_eq!(
            got,
            [
                (BlockKind::ListItem, 0, Marker::Number(1), "one"),
                (BlockKind::ListItem, 0, Marker::Number(2), "two"),
                (BlockKind::ListItem, 1, Marker::Done, "done"),
                (BlockKind::ListItem, 1, Marker::Todo, "todo"),
                (BlockKind::ListItem, 0, Marker::None, "more"),
            ]
        );
    }

    #[test]
    fn quotes_and_alerts() {
        let doc = markdown("> [!WARNING]\n> Careful\n>\n> Second\n\n> plain");
        let got: Vec<_> = doc.blocks.iter().map(|b| (b.kind, b.quote, b.text.as_str())).collect();
        assert_eq!(
            got,
            [
                (BlockKind::Alert(AlertKind::Warning), 1, "Careful"),
                (BlockKind::Paragraph, 1, "Second"),
                (BlockKind::Paragraph, 1, "plain"),
            ]
        );
    }

    #[test]
    fn code_blocks_keep_source_and_language() {
        let doc = markdown("```Rust ignore\nfn main() {}\n```\n\n    indented");
        assert_eq!(doc.blocks[0].kind, BlockKind::Code);
        assert_eq!(doc.blocks[0].lang, "rust");
        assert_eq!(doc.blocks[0].text, "fn main() {}");
        assert!(!doc.blocks[0].lines.is_empty());
        assert_eq!(doc.blocks[1].text, "indented");
        assert_eq!(doc.blocks[1].lang, "");
    }

    #[test]
    fn tables_pad_rows_and_count_extra_ones() {
        let doc = markdown("| a | b |\n|:-|-:|\n| 1 |\n| 2 | 3 | 4 |");
        let t = doc.blocks[0].table.as_ref().expect("a table");
        assert_eq!(t.columns, 2);
        assert_eq!(t.align, [Align::Left, Align::Right]);
        let texts: Vec<_> = t.cells.iter().map(|c| (c.text.as_str(), c.header)).collect();
        assert_eq!(texts, [("a", true), ("b", true), ("1", false), ("", false), ("2", false), ("3", false)]);
    }

    #[test]
    fn links_keep_only_safe_schemes() {
        let doc = markdown(
            "[a](https://x.example) [b](javascript:alert(1)) [c](mailto:me@x.example) <https://y.example> [d](/rel)",
        );
        let b = &doc.blocks[0];
        let links: Vec<_> = b.spans.iter().map(|s| (s.style.link, s.href.as_str())).collect();
        assert_eq!(
            links,
            [
                (Link::Web, "https://x.example"),
                (Link::Mail, "mailto:me@x.example"),
                (Link::Shown, "https://y.example"),
            ]
        );
        assert_eq!(b.text, "a b c https://y.example d");
    }

    #[test]
    fn a_link_whose_text_is_another_address_is_not_shown() {
        let doc = markdown("[https://good.example](https://evil.example) [x.example/a](https://x.example/a)");
        let links: Vec<_> = doc.blocks[0].spans.iter().map(|s| s.style.link).collect();
        assert_eq!(links, [Link::Web, Link::Shown]);
    }

    #[test]
    fn images_become_their_alt_text() {
        let doc = markdown("![a cat](https://x.example/cat.png) ![b](file:///etc/passwd)");
        let b = &doc.blocks[0];
        assert_eq!(b.text, "a cat b");
        assert_eq!(b.spans.len(), 1);
        assert_eq!(b.spans[0].style.link, Link::Web);
        assert!(b.spans[0].style.image);
    }

    #[test]
    fn html_is_text() {
        let doc = markdown("<script>alert(1)</script>\n\nhi <b>there</b>");
        assert_eq!(doc.blocks[0].text, "<script>alert(1)</script>");
        assert_eq!(doc.blocks[1].text, "hi <b>there</b>");
        assert!(doc.blocks.iter().all(|b| b.spans.is_empty()));
    }

    #[test]
    fn reference_links_are_shown_as_written() {
        let doc = markdown("see [docs][1] and [here]\n\n[1]: https://x.example\n[here]: https://y.example");
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].text, "see [docs][1] and [here]");
        assert!(doc.blocks[0].spans.is_empty());
    }

    #[test]
    fn empty_items_and_alerts_still_make_blocks() {
        let doc = markdown("- \n- ```\n  x\n  ```\n\n> [!NOTE]");
        let got: Vec<_> = doc.blocks.iter().map(|b| (b.kind, b.marker)).collect();
        assert_eq!(
            got,
            [
                (BlockKind::ListItem, Marker::Bullet),
                (BlockKind::ListItem, Marker::Bullet),
                (BlockKind::Code, Marker::None),
                (BlockKind::Alert(AlertKind::Note), Marker::None),
            ]
        );
    }
}
