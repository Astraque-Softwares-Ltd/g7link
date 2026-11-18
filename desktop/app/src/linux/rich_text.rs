// SPDX-License-Identifier: GPL-3.0-or-later
//! A rich text field as one plain text. Browsers publish a contenteditable
//! (a chat prompt, a mail body) as a field whose text holds one object
//! replacement character (U+FFFC) per paragraph, each standing for a child
//! object with that paragraph's text. Flattening puts the children's text in
//! their place, one line per paragraph, and keeps where each piece came from,
//! so offsets in the flat text map back to an object and an offset in it.
//! Offsets count Unicode scalar values, as AT-SPI's do.

/// What stands for an embedded object in a field's text.
pub const EMBEDDED: char = '\u{fffc}';
/// Most objects in one flattened field, and how deep they nest.
pub const MAX_OBJECTS: usize = 64;
pub const MAX_DEPTH: usize = 4;
/// Most characters in one flattened field.
pub const MAX_CHARS: usize = 65_536;

/// One object's text as read, with the objects its U+FFFCs stand for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub text: String,
    /// For each U+FFFC, in order: its offset in `text` and the object.
    pub links: Vec<Link>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub at: u32,
    /// A paragraph, heading, list item...: it starts a line of its own.
    pub block: bool,
    pub node: Node,
}

/// A stretch of the flat text that is one object's own text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    /// Which object, in the order [`flatten`] met them (the field is 0).
    pub owner: usize,
    pub local: u32,
    pub flat: u32,
    pub len: u32,
}

/// A flattened field: its text and where each part of it came from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Flat {
    pub text: String,
    pub runs: Vec<Run>,
    /// Where each object starts in the flat text, by owner.
    starts: Vec<u32>,
    /// Each object's own text: how much of it is shown, and how long it is
    /// (a paragraph's last line break is the editor's placeholder, not text).
    lens: Vec<(u32, u32)>,
}

impl Flat {
    /// The object and the offset in it for `flat`. An offset between two
    /// paragraphs is the end of the first.
    pub fn to_local(&self, flat: u32) -> Option<(usize, u32)> {
        self.runs
            .iter()
            .find(|run| flat >= run.flat && flat < run.flat + run.len)
            .or_else(|| self.runs.iter().rev().find(|run| flat == run.flat + run.len))
            .map(|run| (run.owner, run.local + (flat - run.flat)))
            .or_else(|| {
                // An object with no text of its own (an empty paragraph).
                (0..self.starts.len())
                    .rev()
                    .find(|&owner| {
                        self.starts[owner] == flat && self.runs.iter().all(|run| run.owner != owner)
                    })
                    .map(|owner| (owner, 0))
            })
    }

    /// Where `local` in object `owner` is in the flat text.
    pub fn to_flat(&self, owner: usize, local: u32) -> Option<u32> {
        // At or after a paragraph's placeholder break: the end of its text.
        let local = match self.lens.get(owner) {
            Some(&(shown, total)) if local > shown && local <= total => shown,
            _ => local,
        };
        if let Some(run) = self
            .runs
            .iter()
            .find(|run| run.owner == owner && local >= run.local && local < run.local + run.len)
        {
            return Some(run.flat + (local - run.local));
        }
        if let Some(run) =
            self.runs.iter().rev().find(|run| run.owner == owner && local == run.local + run.len)
        {
            return Some(run.flat + run.len);
        }
        // An object with no text of its own (an empty paragraph).
        (local == 0 && self.runs.iter().all(|run| run.owner != owner))
            .then(|| self.starts.get(owner).copied())
            .flatten()
    }

    /// How many objects the text came from.
    pub fn objects(&self) -> usize {
        self.starts.len()
    }
}

/// `root` as one plain text, or `None` when it can't be: an embedded object
/// with no text (an image, a mention chip), or past the limits.
pub fn flatten(root: &Node) -> Option<Flat> {
    let mut flat = Flat::default();
    let mut chars = 0u32;
    walk(root, false, 0, &mut flat, &mut chars)?;
    Some(flat)
}

fn walk(node: &Node, block: bool, depth: usize, flat: &mut Flat, chars: &mut u32) -> Option<()> {
    if depth > MAX_DEPTH || flat.starts.len() >= MAX_OBJECTS {
        return None;
    }
    let owner = flat.starts.len();
    flat.starts.push(*chars);
    // Editors end a paragraph with a line break that holds its line open (an
    // empty one is only that): it isn't text anyone typed.
    let total = u32::try_from(node.text.chars().count()).ok()?;
    let shown = if block && node.text.ends_with('\n') { total - 1 } else { total };
    flat.lens.push((shown, total));
    let mut links = node.links.iter().peekable();
    let mut run_start: Option<(u32, u32)> = None;
    let mut last_block = false;
    for (local, ch) in node.text.chars().take(shown as usize).enumerate() {
        let local = u32::try_from(local).ok()?;
        if ch != EMBEDDED {
            if run_start.is_none() {
                if last_block {
                    push(flat, chars, '\n')?;
                }
                run_start = Some((local, *chars));
            }
            push(flat, chars, ch)?;
            last_block = false;
            continue;
        }
        close_run(flat, owner, &mut run_start, local);
        let link = links.next().filter(|link| link.at == local)?;
        // A paragraph after any text starts a new line; text after a
        // paragraph does too.
        if (link.block && *chars > flat.starts[owner]) || (last_block && !link.block) {
            push(flat, chars, '\n')?;
        }
        walk(&link.node, link.block, depth + 1, flat, chars)?;
        last_block = link.block;
    }
    if links.next().is_some() {
        return None;
    }
    close_run(flat, owner, &mut run_start, shown);
    Some(())
}

fn close_run(flat: &mut Flat, owner: usize, run_start: &mut Option<(u32, u32)>, local_end: u32) {
    if let Some((local, at)) = run_start.take()
        && local_end > local
    {
        flat.runs.push(Run { owner, local, flat: at, len: local_end - local });
    }
}

fn push(flat: &mut Flat, chars: &mut u32, ch: char) -> Option<()> {
    if *chars as usize >= MAX_CHARS {
        return None;
    }
    flat.text.push(ch);
    *chars += 1;
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> Node {
        Node { text: t.into(), links: Vec::new() }
    }

    fn field(paragraphs: &[&str]) -> Node {
        Node {
            text: EMBEDDED.to_string().repeat(paragraphs.len()),
            links: paragraphs
                .iter()
                .enumerate()
                .map(|(i, p)| Link { at: i as u32, block: true, node: text(p) })
                .collect(),
        }
    }

    #[test]
    fn paragraphs_become_lines() {
        let flat = flatten(&field(&["héllo", "", "world"])).unwrap();
        assert_eq!(flat.text, "héllo\n\nworld");
        assert_eq!(flat.objects(), 4);
        // Each paragraph maps both ways, including its end.
        assert_eq!(flat.to_local(0), Some((1, 0)));
        assert_eq!(flat.to_local(4), Some((1, 4)));
        assert_eq!(flat.to_local(5), Some((1, 5)), "the line break is the end of the first");
        assert_eq!(flat.to_local(6), Some((2, 0)), "the empty paragraph's line");
        assert_eq!(flat.to_local(7), Some((3, 0)));
        assert_eq!(flat.to_local(12), Some((3, 5)));
        assert_eq!(flat.to_local(13), None);
        assert_eq!(flat.to_flat(1, 5), Some(5));
        assert_eq!(flat.to_flat(3, 0), Some(7));
        assert_eq!(flat.to_flat(3, 5), Some(12));
        assert_eq!(flat.to_flat(2, 0), Some(6), "an empty paragraph is its line");
        assert_eq!(flat.to_flat(3, 6), None);
    }

    #[test]
    fn plain_text_and_inline_links_stay_on_their_line() {
        let flat = flatten(&text("just text")).unwrap();
        assert_eq!(flat.text, "just text");
        assert_eq!(flat.to_local(4), Some((0, 4)));

        // "see LINK now", the link inline in a paragraph.
        let paragraph = Node {
            text: format!("see {EMBEDDED} now"),
            links: vec![Link { at: 4, block: false, node: text("site") }],
        };
        let root =
            Node { text: EMBEDDED.to_string(), links: vec![Link { at: 0, block: true, node: paragraph }] };
        let flat = flatten(&root).unwrap();
        assert_eq!(flat.text, "see site now");
        assert_eq!(flat.to_local(5), Some((2, 1)));
        assert_eq!(flat.to_local(9), Some((1, 6)));
        assert_eq!(flat.to_flat(1, 6), Some(9));
    }

    #[test]
    fn what_can_t_be_flattened_isn_t() {
        // A U+FFFC with no object behind it (an image the app gave no text).
        assert_eq!(flatten(&Node { text: EMBEDDED.to_string(), links: Vec::new() }), None);
        // An object that doesn't sit where it's said to.
        let wrong =
            Node { text: format!("a{EMBEDDED}"), links: vec![Link { at: 0, block: true, node: text("x") }] };
        assert_eq!(flatten(&wrong), None);
        // Too many paragraphs, too deep, too long.
        let many: Vec<&str> = vec!["p"; MAX_OBJECTS];
        assert_eq!(flatten(&field(&many)), None);
        let mut deep = text("x");
        for _ in 0..=MAX_DEPTH {
            deep = Node { text: EMBEDDED.to_string(), links: vec![Link { at: 0, block: true, node: deep }] };
        }
        assert_eq!(flatten(&deep), None);
        assert_eq!(flatten(&text(&"x".repeat(MAX_CHARS + 1))), None);
    }

    #[test]
    fn a_paragraphs_placeholder_break_isnt_text() {
        // An empty prompt: one paragraph holding only the editor's line break.
        let flat = flatten(&field(&["\n"])).unwrap();
        assert_eq!(flat.text, "");
        assert_eq!(flat.to_local(0), Some((1, 0)));
        assert_eq!(flat.to_flat(1, 0), Some(0));
        assert_eq!(flat.to_flat(1, 1), Some(0), "after the placeholder is where the text ends");
        // A line break typed at the end of a paragraph stays; the placeholder after it goes.
        let flat = flatten(&field(&["hi\n\n", "\n", "there"])).unwrap();
        assert_eq!(flat.text, "hi\n\n\nthere");
        assert_eq!(flat.to_flat(1, 3), Some(3));
        assert_eq!(flat.to_flat(1, 4), Some(3));
        assert_eq!(flat.to_local(5), Some((3, 0)));
        // The field's own text keeps its last line break.
        assert_eq!(flatten(&text("a\n")).unwrap().text, "a\n");
    }

    #[test]
    fn a_picture_in_the_text_takes_no_room() {
        let line =
            Node { text: format!("a{EMBEDDED}b"), links: vec![Link { at: 1, block: false, node: text("") }] };
        let flat = flatten(&line).unwrap();
        assert_eq!(flat.text, "ab");
        assert_eq!(flat.objects(), 2);
        // Between the two letters is after the picture; both sides map back.
        assert_eq!(flat.to_local(1), Some((0, 2)));
        assert_eq!(flat.to_flat(0, 1), Some(1));
        assert_eq!(flat.to_flat(0, 2), Some(1));
        assert_eq!(flat.to_flat(0, 3), Some(2));
    }

    #[test]
    fn text_around_paragraphs_gets_its_own_lines() {
        let root = Node {
            text: format!("top{EMBEDDED}tail"),
            links: vec![Link { at: 3, block: true, node: text("middle") }],
        };
        let flat = flatten(&root).unwrap();
        assert_eq!(flat.text, "top\nmiddle\ntail");
        assert_eq!(flat.to_local(2), Some((0, 2)));
        assert_eq!(flat.to_local(4), Some((1, 0)));
        assert_eq!(flat.to_local(11), Some((0, 4)));
        assert_eq!(flat.to_flat(0, 4), Some(11));
    }
}
