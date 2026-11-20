// SPDX-License-Identifier: MPL-2.0
//! `thread.export`: a thread, or a followed session, as Markdown.
//!
//! Every piece of text is cleaned of control and bidi characters again, and
//! every fenced block is fenced with a longer backtick run than any in its
//! content, so nothing an agent wrote can close a block or start a new one.
//! The folder appears by its label, never its path.

use std::collections::HashMap;
use std::sync::Arc;

use crate::api::*;
use crate::host::Host;
use crate::host::sessions::paths::civil_from_days;
use crate::host::store::ApprovalRecord;

/// What the heading says.
#[derive(Debug, Clone)]
pub struct Header {
    pub title: String,
    pub agent: AgentId,
    pub folder_label: String,
    pub started_ms: u64,
}

/// `2026-11-19 09:35 UTC`.
fn when(ms: u64) -> String {
    let days = i64::try_from(ms / 86_400_000).unwrap_or(0);
    let (y, m, d) = civil_from_days(days);
    let minutes = (ms % 86_400_000) / 60_000;
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", minutes / 60, minutes % 60)
}

fn date(ms: u64) -> String {
    let (y, m, d) = civil_from_days(i64::try_from(ms / 86_400_000).unwrap_or(0));
    format!("{y:04}-{m:02}-{d:02}")
}

/// The longest run of backticks in `s`.
fn longest_backticks(s: &str) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for c in s.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest
}

/// `content` in a fenced block that it can't close.
fn fenced(content: &str, info: &str) -> String {
    let fence = "`".repeat(longest_backticks(content).max(2) + 1);
    let body = content.trim_end_matches('\n');
    format!("{fence}{info}\n{body}\n{fence}\n\n")
}

/// One line as inline code that it can't close.
fn inline_code(s: &str) -> String {
    let ticks = "`".repeat(longest_backticks(s) + 1);
    let pad = if s.starts_with('`') || s.ends_with('`') { " " } else { "" };
    format!("{ticks}{pad}{s}{pad}{ticks}")
}

/// `text` with a fence it left open closed at its end, so the rest of the
/// export stays outside it.
fn close_fences(text: &str) -> String {
    let mut open: Option<(char, usize)> = None;
    for line in text.lines() {
        let trimmed = line.trim_start_matches(' ');
        if line.len() - trimmed.len() > 3 {
            continue;
        }
        let Some(c) = trimmed.chars().next().filter(|c| *c == '`' || *c == '~') else { continue };
        let run = trimmed.chars().take_while(|x| *x == c).count();
        if run < 3 {
            continue;
        }
        match open {
            None => open = Some((c, run)),
            Some((oc, on)) if oc == c && run >= on && trimmed[run..].trim().is_empty() => open = None,
            Some(_) => {}
        }
    }
    match open {
        Some((c, n)) => format!("{text}\n{}", c.to_string().repeat(n)),
        None => text.to_owned(),
    }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `claude-fix-the-login-test-2026-11-19.md`.
pub fn file_name(agent: AgentId, title: &str, started_ms: u64) -> String {
    let mut slug = String::new();
    for c in title.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let mut slug = slug.trim_matches('-').to_owned();
    if slug.len() > 48 {
        slug.truncate(48);
        slug = slug.trim_end_matches('-').to_owned();
    }
    if slug.is_empty() {
        slug.push_str("thread");
    }
    format!("{}-{slug}-{}.md", agent.as_str(), date(started_ms))
}

const COMMAND_TOOLS: &[&str] = &["Bash", "shell"];
const EDIT_TOOLS: &[&str] = &["Write", "Edit", "MultiEdit", "NotebookEdit", "apply_patch"];

fn status_note(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Ok => "",
        ToolStatus::Error => " (failed)",
        ToolStatus::Denied => " (denied)",
        ToolStatus::Running => " (not finished)",
    }
}

/// `text` as a block quote under a bold heading.
fn quoted(heading: &str, text: &str) -> String {
    let mut out = format!("> **{heading}**\n>\n");
    for line in text.lines() {
        out.push_str(if line.is_empty() { ">" } else { "> " });
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
    out
}

/// One item as Markdown.
fn item_markdown(
    item: &Item,
    approvals: &[&ApprovalRecord],
    who: &dyn Fn(Option<&Origin>) -> String,
    device: &dyn Fn(&Origin) -> String,
) -> String {
    let text = item.text.as_deref().map(clean).unwrap_or_default();
    let mut out = String::new();
    match item.kind {
        ItemKind::User if item.deny_of.is_some() => out.push_str(&quoted("Denied, and said:", &text)),
        ItemKind::User => out.push_str(&quoted(&clean(&who(item.origin.as_ref())), &text)),
        ItemKind::Reasoning => {
            if !text.trim().is_empty() {
                out.push_str(&quoted("Thinking", text.trim_end()));
            }
        }
        ItemKind::Unknown => {}
        ItemKind::Assistant => {
            if !text.trim().is_empty() {
                out.push_str(&close_fences(text.trim_end()));
                out.push_str("\n\n");
            }
        }
        ItemKind::Notice => {
            if !text.trim().is_empty() {
                out.push_str(&format!("_{}_\n\n", one_line(&text)));
            }
        }
        ItemKind::Tool => {
            let Some(tool) = &item.tool else { return out };
            let name = clean(&tool.name);
            let detail = tool.detail.as_deref().map(clean).unwrap_or_default();
            let note = status_note(tool.status);
            if COMMAND_TOOLS.contains(&name.as_str()) {
                let command = detail.trim();
                if !command.is_empty() && !command.contains('\n') {
                    out.push_str(&format!("**Ran** {}{note}\n\n", inline_code(command)));
                } else {
                    out.push_str(&format!("**Ran**{note}\n\n"));
                    if !command.is_empty() {
                        out.push_str(&fenced(command, "sh"));
                    }
                }
            } else if EDIT_TOOLS.contains(&name.as_str()) {
                out.push_str(&format!("**{}**{note}\n\n", one_line(&clean(&tool.title))));
                if !detail.trim().is_empty() {
                    out.push_str(&fenced(&detail, "diff"));
                }
            } else {
                out.push_str(&format!("**Used** {}{note}\n\n", one_line(&clean(&tool.title))));
                if !detail.trim().is_empty() {
                    out.push_str(&fenced(&detail, ""));
                }
            }
            // Who allowed or denied it, then what it printed.
            for record in approvals {
                let line = match (record.outcome, &record.by) {
                    (ApprovalOutcome::Allowed, Some(by)) => format!("Allowed on {}", clean(&device(by))),
                    (ApprovalOutcome::Denied, Some(by)) => format!("Denied on {}", clean(&device(by))),
                    (ApprovalOutcome::Allowed, None) => "Allowed".to_owned(),
                    (ApprovalOutcome::Denied, None) => "Denied".to_owned(),
                    (ApprovalOutcome::Expired, _) => "Not answered in time, so denied".to_owned(),
                    (ApprovalOutcome::Cancelled, _) => continue,
                };
                out.push_str(&format!("_{line}_\n\n"));
            }
            if let Some(output) = tool.output.as_deref().map(clean).filter(|o| !o.trim().is_empty()) {
                out.push_str(&fenced(&output, ""));
            }
        }
    }
    out
}

/// The whole export: the heading, then each item in order while it fits in
/// [`MAX_EXPORT`]. Returns the text and whether items were left out.
pub fn render(
    header: &Header,
    items: &[Item],
    approvals: &[ApprovalRecord],
    who: &dyn Fn(Option<&Origin>) -> String,
    device: &dyn Fn(&Origin) -> String,
) -> (String, bool) {
    const STOPPED: &str = "_The export stops here: the rest didn't fit in 2 MiB._\n";
    let mut by_seq: HashMap<u64, Vec<&ApprovalRecord>> = HashMap::new();
    for record in approvals {
        if let Some(seq) = record.tool_seq {
            by_seq.entry(seq).or_default().push(record);
        }
    }
    let title = one_line(&clean(&header.title));
    let mut out = format!(
        "# {}\n\n- Agent: {}\n- Folder: {}\n- Started: {}\n\n---\n\n",
        if title.is_empty() { header.agent.display_name().to_owned() } else { title },
        header.agent.display_name(),
        one_line(&clean(&header.folder_label)),
        when(header.started_ms),
    );
    for item in items {
        let none = Vec::new();
        let chunk = item_markdown(item, by_seq.get(&item.seq).unwrap_or(&none), who, device);
        if out.len() + chunk.len() + STOPPED.len() > MAX_EXPORT {
            out.push_str(STOPPED);
            return (out, true);
        }
        out.push_str(&chunk);
    }
    (out, false)
}

impl Host {
    /// `thread.export`.
    pub async fn thread_export(
        self: &Arc<Self>,
        p: ThreadExportParams,
    ) -> Result<ThreadExportResult, RpcError> {
        let ExportFormat::Markdown = p.format;
        let pc = self.hostname.clone();
        let device = move |by: &Origin| match by.kind {
            OriginKind::Phone => by.device_name.clone().unwrap_or_else(|| "your phone".to_owned()),
            OriginKind::Launcher | OriginKind::Cli => pc.clone(),
        };
        let (header, items, approvals) = match (p.thread_id, p.session_id) {
            (Some(id), None) => {
                let info = is_valid_id(&id)
                    .then(|| self.lock_reg().threads.get(&id).map(|e| e.info.clone()))
                    .flatten()
                    .ok_or_else(|| RpcError::new(ErrorCode::NotFound, "No such thread."))?;
                let items = self
                    .store
                    .read_items(&id)
                    .await
                    .map_err(|_| RpcError::new(ErrorCode::Internal, "Couldn't read the thread."))?;
                let approvals = self.store.read_approvals(&id).await.unwrap_or_default();
                let header = Header {
                    title: info.title,
                    agent: info.agent,
                    folder_label: info.folder_label,
                    started_ms: info.created_ms,
                };
                (header, items.into_values().collect::<Vec<_>>(), approvals)
            }
            (None, Some(id)) => {
                let (session, items) = self.session_items_for_export(&id).await?;
                let header = Header {
                    started_ms: items.first().map_or(session.updated_ms, |i| i.ts_ms),
                    title: session.title,
                    agent: session.agent,
                    folder_label: session.folder_label,
                };
                (header, items, Vec::new())
            }
            _ => return Err(RpcError::new(ErrorCode::BadRequest, "Name one thread or one session.")),
        };
        let who = |origin: Option<&Origin>| match origin {
            Some(by) => format!("You on {}", device(by)),
            None => "You".to_owned(),
        };
        let (text, truncated) = render(&header, &items, &approvals, &who, &device);
        Ok(ThreadExportResult {
            text,
            truncated,
            file_name: file_name(header.agent, &header.title, header.started_ms),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(
        seq: u64,
        kind: ItemKind,
        text: Option<&str>,
        tool: Option<Tool>,
        origin: Option<Origin>,
    ) -> Item {
        Item { text: text.map(str::to_owned), origin, tool, ..Item::new(seq, format!("i{seq}"), kind, 1) }
    }

    fn tool(name: &str, title: &str, detail: Option<&str>, status: ToolStatus, output: Option<&str>) -> Tool {
        Tool {
            name: name.into(),
            title: title.into(),
            detail: detail.map(str::to_owned),
            status,
            output: output.map(str::to_owned),
            class: None,
            path: None,
            by_rule: false,
        }
    }

    fn pc(by: &Origin) -> String {
        by.device_name.clone().unwrap_or_else(|| "guchu".to_owned())
    }

    fn who(origin: Option<&Origin>) -> String {
        origin.map_or_else(|| "You".to_owned(), |o| format!("You on {}", pc(o)))
    }

    /// The fixture thread: a prompt from the phone, a command allowed on
    /// the phone, an edit denied on the PC, an answer with a fence in it.
    fn fixture() -> (Header, Vec<Item>, Vec<ApprovalRecord>) {
        let header = Header {
            title: "Fix the login test".into(),
            agent: AgentId::Claude,
            folder_label: "g7linux".into(),
            started_ms: 20_776 * 86_400_000 + 34_500_000,
        };
        let items = vec![
            item(
                1,
                ItemKind::User,
                Some("Fix the login test.\n\nIt fails on CI."),
                None,
                Some(Origin::phone("p", "Pixel")),
            ),
            item(
                2,
                ItemKind::Tool,
                None,
                Some(tool(
                    "Bash",
                    "Run command",
                    Some("cargo test login"),
                    ToolStatus::Ok,
                    Some("test login ... ok\n"),
                )),
                None,
            ),
            item(
                3,
                ItemKind::Tool,
                None,
                Some(tool("Edit", "Edit src/login.rs", Some("-old\n+new"), ToolStatus::Denied, None)),
                None,
            ),
            item(
                4,
                ItemKind::Assistant,
                Some("Done. Watch out:\n```\nnot a fence of ours\n```\u{1b}[31m"),
                None,
                None,
            ),
            item(
                5,
                ItemKind::Tool,
                None,
                Some(tool(
                    "shell",
                    "Run command",
                    Some("echo `date`\nls"),
                    ToolStatus::Error,
                    Some("````\nboom"),
                )),
                None,
            ),
            item(6, ItemKind::Notice, Some("Stopped: the agent host restarted."), None, None),
        ];
        let approvals = vec![
            ApprovalRecord {
                tool_seq: Some(2),
                outcome: ApprovalOutcome::Allowed,
                by: Some(Origin::phone("p", "Pixel")),
                ts_ms: 2,
            },
            ApprovalRecord {
                tool_seq: Some(3),
                outcome: ApprovalOutcome::Denied,
                by: Some(Origin::launcher()),
                ts_ms: 3,
            },
        ];
        (header, items, approvals)
    }

    #[test]
    fn a_thread_as_markdown() {
        let (header, items, approvals) = fixture();
        let (text, truncated) = render(&header, &items, &approvals, &who, &pc);
        assert!(!truncated);
        let expected = "# Fix the login test

- Agent: Claude
- Folder: g7linux
- Started: 2026-11-19 09:35 UTC

---

> **You on Pixel**
>
> Fix the login test.
>
> It fails on CI.

**Ran** `cargo test login`

_Allowed on Pixel_

```
test login ... ok
```

**Edit src/login.rs** (denied)

```diff
-old
+new
```

_Denied on guchu_

Done. Watch out:
```
not a fence of ours
```[31m
```

**Ran** (failed)

```sh
echo `date`
ls
```

`````
````
boom
`````

_Stopped: the agent host restarted._

";
        assert_eq!(text, expected);
        assert!(!text.contains('\u{1b}'));
        assert_eq!(
            file_name(header.agent, &header.title, header.started_ms),
            "claude-fix-the-login-test-2026-11-19.md"
        );
    }

    #[test]
    fn thinking_and_a_denial_with_words() {
        let (header, _, _) = fixture();
        let items = vec![
            Item { text: Some(String::new()), end_ms: Some(2), ..Item::new(1, "r1", ItemKind::Reasoning, 1) },
            Item {
                text: Some("**Plan**\nRead first.\n".into()),
                ..Item::new(2, "r2", ItemKind::Reasoning, 1)
            },
            item(
                3,
                ItemKind::Tool,
                None,
                Some(tool("Bash", "Run command", Some("rm -rf build"), ToolStatus::Denied, None)),
                None,
            ),
            Item {
                text: Some("Use a temp dir\u{202e} instead.".into()),
                origin: Some(Origin::phone("p", "Pixel")),
                deny_of: Some("i3".into()),
                ..Item::new(4, "user-4", ItemKind::User, 1)
            },
            Item::new(5, "x", ItemKind::Unknown, 1),
        ];
        let (text, _) = render(&header, &items, &[], &who, &pc);
        let body = text.split("---\n\n").nth(1).unwrap();
        assert_eq!(
            body,
            "> **Thinking**\n>\n> **Plan**\n> Read first.\n\n\
             **Ran** `rm -rf build` (denied)\n\n\
             > **Denied, and said:**\n>\n> Use a temp dir instead.\n\n"
        );
    }

    #[test]
    fn an_open_fence_in_agent_text_is_closed() {
        assert_eq!(close_fences("a\n```rust\nfn x() {}"), "a\n```rust\nfn x() {}\n```");
        assert_eq!(close_fences("````\n```\nx"), "````\n```\nx\n````");
        assert_eq!(close_fences("```\nx\n```"), "```\nx\n```");
        assert_eq!(close_fences("~~~\nx"), "~~~\nx\n~~~");
        assert_eq!(
            close_fences("    ```\nindented code, not a fence"),
            "    ```\nindented code, not a fence"
        );
    }

    #[test]
    fn fences_and_names() {
        assert_eq!(inline_code("a`b"), "``a`b``");
        assert_eq!(inline_code("`x"), "`` `x ``");
        assert_eq!(fenced("a\n```\nb", ""), "````\na\n```\nb\n````\n\n");
        assert_eq!(
            file_name(AgentId::Codex, "  Ünïcode / & --- stuff!! ", 0),
            "codex-n-code-stuff-1970-01-01.md"
        );
        assert_eq!(file_name(AgentId::Codex, "!!!", 0), "codex-thread-1970-01-01.md");
        assert!(
            file_name(AgentId::Claude, &"word ".repeat(40), 0).len()
                <= "claude-".len() + 48 + "-1970-01-01.md".len()
        );
    }

    #[test]
    fn a_long_thread_stops_at_two_mib_between_items() {
        let (header, _, _) = fixture();
        let big = "x".repeat(16_000);
        let items: Vec<Item> =
            (1..=200).map(|n| item(n, ItemKind::Assistant, Some(&big), None, None)).collect();
        let (text, truncated) = render(&header, &items, &[], &who, &pc);
        assert!(truncated);
        assert!(text.len() <= MAX_EXPORT);
        assert!(text.ends_with("_The export stops here: the rest didn't fit in 2 MiB._\n"));
    }
}
