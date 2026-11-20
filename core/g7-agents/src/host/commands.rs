// SPDX-License-Identifier: MPL-2.0
//! `/` commands (§Q4.7): what each agent reports it can run, filtered.
//!
//! Claude's list is the catalog probe's (parsed by `catalog.rs`), joined per
//! folder by the names a thread's `system/init` reported; commands that need a
//! terminal or touch the account or configuration are left out. Codex's are
//! the folder's enabled skills from `skills/list`, and `compact`. G7's own
//! `/model`, `/effort` and `/plan` are added by each client, never here.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{Value, json};

use crate::api::{
    AgentId, Command, CommandKind, MAX_COMMAND_HINT, MAX_COMMAND_NAME, MAX_COMMAND_NOTE, MAX_COMMANDS,
    clean_cut, collapse_whitespace,
};

/// Folders whose `system/init` names and skills are kept.
pub const MAX_FOLDERS: usize = 64;
/// Entries of a reported list that are looked at, at most.
const MAX_READ: usize = 2000;
const MAX_SKILL_PATH: usize = 4096;

/// Claude commands G7 never offers: they need a terminal, or touch the
/// account or the configuration.
pub const CLAUDE_DENYLIST: [&str; 50] = [
    "login",
    "logout",
    "exit",
    "quit",
    "config",
    "permissions",
    "mcp",
    "ide",
    "vim",
    "terminal-setup",
    "doctor",
    "upgrade",
    "install-github-app",
    "install-slack-app",
    "bug",
    "feedback",
    "release-notes",
    "status",
    "statusline",
    "theme",
    "keybindings",
    "add-dir",
    "agents",
    "hooks",
    "memory",
    "model",
    "output-style",
    "plugin",
    "privacy-settings",
    "remote-control",
    "resume",
    "rewind",
    "sandbox",
    "bashes",
    "tasks",
    "export",
    "usage",
    "extra-usage",
    "fast",
    "effort",
    "plan",
    "voice",
    "chrome",
    "desktop",
    "mobile",
    "teleport",
    "setup-token",
    "migrate-installer",
    "heapdump",
    "clear",
];

/// G7's own commands: no agent's command may take their names.
pub const G7_OWN: [&str; 3] = ["model", "effort", "plan"];

/// Codex's one built-in.
pub const CODEX_COMPACT: &str = "compact";

/// A command name as typed after `/`: letters, digits and `-_:.`, starting
/// with a letter or digit, at most [`MAX_COMMAND_NAME`] bytes.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_COMMAND_NAME
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.'))
}

/// Whether Claude's command `name` is left out.
pub fn claude_denied(name: &str) -> bool {
    CLAUDE_DENYLIST.contains(&name) || G7_OWN.contains(&name)
}

/// A reported name, without a leading `/`, when it is a valid name.
fn reported_name(name: &str) -> Option<String> {
    let name = name.trim();
    let name = name.strip_prefix('/').unwrap_or(name);
    is_valid_name(name).then(|| name.to_owned())
}

/// Agent-written text for a note or hint: on one line, cleaned, cut.
fn one_line(text: Option<&str>, max: usize) -> Option<String> {
    let (cleaned, _) = clean_cut(&collapse_whitespace(text?), max);
    (!cleaned.is_empty()).then_some(cleaned)
}

/// A command as G7 offers it: a valid name, its note and hint on one line.
fn offered(command: &Command) -> Option<Command> {
    Some(Command {
        name: reported_name(&command.name)?,
        note: one_line(command.note.as_deref(), MAX_COMMAND_NOTE),
        hint: one_line(command.hint.as_deref(), MAX_COMMAND_HINT),
        kind: command.kind,
    })
}

/// Claude's commands from the catalog probe, denied names left out.
pub fn claude_filter(commands: &[Command]) -> Vec<Command> {
    let mut seen = HashSet::new();
    commands
        .iter()
        .take(MAX_READ)
        .filter_map(offered)
        .filter(|c| !claude_denied(&c.name) && seen.insert(c.name.clone()))
        .map(|c| Command { kind: CommandKind::Command, ..c })
        .take(MAX_COMMANDS)
        .collect()
}

/// The command names a Claude `system/init` line reports for its folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeInit {
    /// `slash_commands`: the folder's own commands among them.
    pub names: Vec<String>,
    /// `terminal_slash_commands`: they need a terminal.
    pub terminal: Vec<String>,
}

fn clean_names<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut seen = HashSet::new();
    names.take(MAX_READ).filter_map(reported_name).filter(|n| seen.insert(n.clone())).collect()
}

impl ClaudeInit {
    /// From the lists as reported (names are checked and deduplicated).
    pub fn new(names: &[String], terminal: &[String]) -> ClaudeInit {
        ClaudeInit {
            names: clean_names(names.iter().map(String::as_str)),
            terminal: clean_names(terminal.iter().map(String::as_str)),
        }
    }

    /// From a `system/init` line.
    pub fn from_line(init: &Value) -> ClaudeInit {
        let names = |key: &str| {
            clean_names(
                init.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str),
            )
        };
        ClaudeInit { names: names("slash_commands"), terminal: names("terminal_slash_commands") }
    }
}

/// Claude's list for a folder: the probe's (already filtered), then the
/// names only that folder's `system/init` reported; minus what needs a
/// terminal.
pub fn claude_list(probe: &[Command], init: Option<&ClaudeInit>) -> Vec<Command> {
    let terminal: HashSet<&str> =
        init.map(|i| i.terminal.iter().map(String::as_str).collect()).unwrap_or_default();
    let mut seen = HashSet::new();
    let mut out: Vec<Command> = probe
        .iter()
        .filter(|c| !terminal.contains(c.name.as_str()) && seen.insert(c.name.clone()))
        .take(MAX_COMMANDS)
        .cloned()
        .collect();
    for name in init.map(|i| i.names.as_slice()).unwrap_or_default() {
        if out.len() == MAX_COMMANDS {
            break;
        }
        if claude_denied(name) || terminal.contains(name.as_str()) || !seen.insert(name.clone()) {
            continue;
        }
        out.push(Command { name: name.clone(), note: None, hint: None, kind: CommandKind::Command });
    }
    out
}

/// A Codex skill: what clients see, and the path its input names (never
/// sent to clients; `None` when the list came without paths).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexSkill {
    pub command: Command,
    pub path: Option<String>,
}

fn codex_offered(command: &Command, path: Option<String>, seen: &mut HashSet<String>) -> Option<CodexSkill> {
    let command = offered(command)?;
    if G7_OWN.contains(&command.name.as_str())
        || command.name == CODEX_COMPACT
        || !seen.insert(command.name.clone())
    {
        return None;
    }
    Some(CodexSkill { command: Command { kind: CommandKind::Skill, ..command }, path })
}

/// Codex's enabled skills for `folder` from a `skills/list` result
/// (`{data: [{cwd, skills: [{name, description, shortDescription, path, enabled}]}]}`),
/// with the paths their input needs.
pub fn codex_skills(result: &Value, folder: &Path) -> Vec<CodexSkill> {
    let for_folder =
        |entry: &&Value| entry.get("cwd").and_then(Value::as_str).is_none_or(|cwd| Path::new(cwd) == folder);
    let skills = result
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_READ)
        .filter(for_folder)
        .filter_map(|entry| entry.get("skills").and_then(Value::as_array))
        .flatten()
        .take(MAX_READ);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for skill in skills {
        if skill.get("enabled").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let Some(path) = skill.get("path").and_then(Value::as_str) else { continue };
        if !Path::new(path).is_absolute() || path.len() > MAX_SKILL_PATH || path.contains('\0') {
            continue;
        }
        let text = |key: &str| skill.get(key).and_then(Value::as_str).filter(|t| !t.trim().is_empty());
        let command = Command {
            name: skill.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
            note: text("shortDescription").or_else(|| text("description")).map(str::to_owned),
            hint: None,
            kind: CommandKind::Skill,
        };
        if let Some(skill) = codex_offered(&command, Some(path.to_owned()), &mut seen) {
            out.push(skill);
            if out.len() == MAX_COMMANDS - 1 {
                break;
            }
        }
    }
    out
}

/// Codex's list for a folder: its skills, then `compact`.
pub fn codex_list(skills: &[CodexSkill]) -> Vec<Command> {
    let mut out: Vec<Command> = skills.iter().take(MAX_COMMANDS - 1).map(|s| s.command.clone()).collect();
    out.push(Command {
        name: CODEX_COMPACT.to_owned(),
        note: Some("Compact the conversation".to_owned()),
        hint: None,
        kind: CommandKind::Command,
    });
    out
}

/// Codex's input for a skill, sent before the text input; `None` without
/// its path (send the message as text then).
pub fn codex_skill_input(skill: &CodexSkill) -> Option<Value> {
    let path = skill.path.as_ref()?;
    Some(json!({"type": "skill", "name": skill.command.name, "path": path}))
}

/// A slash command at the start of a message a person sent: the whole first
/// word, `/name`, and the text after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slash<'a> {
    pub name: &'a str,
    pub rest: &'a str,
}

pub fn slash(text: &str) -> Option<Slash<'_>> {
    let body = text.strip_prefix('/')?;
    let end = body.find(char::is_whitespace).unwrap_or(body.len());
    let name = &body[..end];
    is_valid_name(name).then(|| Slash { name, rest: body[end..].trim_start() })
}

/// What the host keeps: Claude's probe list, and per folder Claude's
/// `system/init` names and Codex's skills.
#[derive(Debug, Default)]
pub struct Commands {
    claude: Vec<Command>,
    claude_init: HashMap<String, ClaudeInit>,
    codex: HashMap<String, Vec<CodexSkill>>,
}

impl Commands {
    /// After Claude's catalog probe: its commands, as `catalog.rs` parsed them.
    pub fn set_claude_probe(&mut self, commands: &[Command]) {
        self.claude = claude_filter(commands);
    }

    /// After a Claude thread in `folder_id` reported `system/init`.
    pub fn set_claude_init(&mut self, folder_id: &str, init: ClaudeInit) {
        if !self.claude_init.contains_key(folder_id) && self.claude_init.len() >= MAX_FOLDERS {
            return;
        }
        self.claude_init.insert(folder_id.to_owned(), init);
    }

    /// `system/init.terminal_slash_commands` alone (the adapter's
    /// `Step::TerminalCommands`): the folder's other names are kept.
    pub fn set_claude_terminal(&mut self, folder_id: &str, terminal: &[String]) {
        let names = self.claude_init.get(folder_id).map(|i| i.names.clone()).unwrap_or_default();
        let init = ClaudeInit { names, terminal: ClaudeInit::new(&[], terminal).terminal };
        self.set_claude_init(folder_id, init);
    }

    /// Codex's `skills/list { cwds: [folder_path] }` result, paths kept.
    pub fn set_codex_result(&mut self, folder_id: &str, folder_path: &Path, result: &Value) {
        self.put_codex(folder_id, codex_skills(result, folder_path));
    }

    /// Codex's skills as `catalog.rs` parsed them (no paths: a skill then
    /// goes to Codex as the message's text).
    pub fn set_codex_skills(&mut self, folder_id: &str, skills: &[Command]) {
        let mut seen = HashSet::new();
        let skills = skills
            .iter()
            .take(MAX_READ)
            .filter_map(|c| codex_offered(c, None, &mut seen))
            .take(MAX_COMMANDS - 1)
            .collect();
        self.put_codex(folder_id, skills);
    }

    fn put_codex(&mut self, folder_id: &str, skills: Vec<CodexSkill>) {
        if !self.codex.contains_key(folder_id) && self.codex.len() >= MAX_FOLDERS {
            return;
        }
        self.codex.insert(folder_id.to_owned(), skills);
    }

    /// Whether Codex's skills for the folder are known (else the host asks).
    pub fn has_codex_skills(&self, folder_id: &str) -> bool {
        self.codex.contains_key(folder_id)
    }

    /// `commands.list`: empty while the agent has reported nothing.
    pub fn list(&self, agent: AgentId, folder_id: &str) -> Vec<Command> {
        match agent {
            AgentId::Claude => {
                let init = self.claude_init.get(folder_id);
                if self.claude.is_empty() && init.is_none() {
                    return Vec::new();
                }
                claude_list(&self.claude, init)
            }
            AgentId::Codex => self.codex.get(folder_id).map(|skills| codex_list(skills)).unwrap_or_default(),
        }
    }

    /// The skill a Codex message starting with `/name` names, in that folder.
    pub fn codex_skill(&self, folder_id: &str, name: &str) -> Option<&CodexSkill> {
        self.codex.get(folder_id)?.iter().find(|s| s.command.name == name)
    }

    /// A folder was removed.
    pub fn forget_folder(&mut self, folder_id: &str) {
        self.claude_init.remove(folder_id);
        self.codex.remove(folder_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOLDER: &str = "f000000000001";

    fn command(name: &str, note: Option<&str>) -> Command {
        Command { name: name.into(), note: note.map(Into::into), hint: None, kind: CommandKind::Command }
    }

    fn names(list: &[Command]) -> Vec<&str> {
        list.iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn the_probe_list_leaves_out_denied_and_g7_names() {
        let probe = [
            Command {
                hint: Some("<instructions>".into()),
                ..command("compact", Some("Clear history\nbut keep a summary"))
            },
            command("login", Some("Sign in")),
            command("/review", Some("Review a PR")),
            command("model", None),
            command("clear", None),
            command("bad name", None),
            command("a/b", None),
            command("compact", None),
            command("my-skill:deploy", Some("Ship \u{202e}it")),
        ];
        let list = claude_filter(&probe);
        assert_eq!(names(&list), ["compact", "review", "my-skill:deploy"]);
        assert_eq!(list[0].note.as_deref(), Some("Clear history but keep a summary"));
        assert_eq!(list[0].hint.as_deref(), Some("<instructions>"));
        assert_eq!(list[2].note.as_deref(), Some("Ship it"));
        for denied in CLAUDE_DENYLIST.iter().chain(&G7_OWN) {
            assert!(claude_denied(denied));
            assert!(claude_filter(&[command(denied, None)]).is_empty(), "{denied}");
        }
    }

    #[test]
    fn init_adds_the_folders_own_and_removes_terminal_ones() {
        let probe =
            claude_filter(&[command("compact", None), command("color", None), command("review", None)]);
        let init = ClaudeInit::from_line(&json!({
            "slash_commands": ["review", "deploy-here", "doctor", "focus", 7],
            "terminal_slash_commands": ["color", "focus"]
        }));
        assert_eq!(names(&claude_list(&probe, Some(&init))), ["compact", "review", "deploy-here"]);

        let mut commands = Commands::default();
        commands.set_claude_probe(&[command("compact", None), command("color", None)]);
        commands.set_claude_terminal(FOLDER, &["color".to_owned()]);
        assert_eq!(names(&commands.list(AgentId::Claude, FOLDER)), ["compact"]);
        assert_eq!(commands.list(AgentId::Claude, "f000000000002").len(), 2, "another folder keeps it");
    }

    #[test]
    fn codex_lists_enabled_skills_of_the_folder_then_compact() {
        let result = json!({"data": [
            {"cwd": "/w", "skills": [
                {"name": "lint", "shortDescription": "Lint it", "description": "Long", "path": "/s/lint/SKILL.md", "enabled": true},
                {"name": "off", "path": "/s/off/SKILL.md", "enabled": false},
                {"name": "unsaid", "path": "/s/unsaid/SKILL.md"},
                {"name": "relative", "path": "s/r/SKILL.md", "enabled": true},
                {"name": "plan", "path": "/s/plan/SKILL.md", "enabled": true},
                {"name": "docs", "description": "Write docs", "path": "/s/docs/SKILL.md", "enabled": true}
            ]},
            {"cwd": "/other", "skills": [{"name": "elsewhere", "path": "/s/e/SKILL.md", "enabled": true}]}
        ]});
        let skills = codex_skills(&result, Path::new("/w"));
        let list = codex_list(&skills);
        assert_eq!(names(&list), ["lint", "docs", "compact"]);
        assert_eq!(list[0].note.as_deref(), Some("Lint it"));
        assert_eq!(list[1].note.as_deref(), Some("Write docs"));
        assert_eq!(list[0].kind, CommandKind::Skill);
        assert_eq!(list[2].kind, CommandKind::Command);
        assert_eq!(
            codex_skill_input(&skills[0]),
            Some(json!({"type": "skill", "name": "lint", "path": "/s/lint/SKILL.md"}))
        );

        let mut commands = Commands::default();
        commands.set_codex_result(FOLDER, Path::new("/w"), &result);
        assert!(commands.codex_skill(FOLDER, "docs").is_some_and(|s| s.path.is_some()));
        commands.set_codex_skills(FOLDER, &[command("lint", None), command("compact", None)]);
        let lint = commands.codex_skill(FOLDER, "lint").unwrap();
        assert_eq!((lint.command.kind, codex_skill_input(lint)), (CommandKind::Skill, None));
        assert_eq!(names(&commands.list(AgentId::Codex, FOLDER)), ["lint", "compact"]);
    }

    #[test]
    fn a_slash_command_is_the_whole_first_word_only() {
        assert_eq!(slash("/compact"), Some(Slash { name: "compact", rest: "" }));
        assert_eq!(slash("/review  the PR"), Some(Slash { name: "review", rest: "the PR" }));
        assert_eq!(slash("please /compact"), None);
        assert_eq!(slash(" /compact"), None);
        assert_eq!(slash("/"), None);
        assert_eq!(slash("/ compact"), None);
        assert_eq!(slash("/a/b"), None);
        assert_eq!(slash("/-x"), None);
    }

    #[test]
    fn lists_are_empty_until_the_agent_reports() {
        let mut commands = Commands::default();
        assert!(commands.list(AgentId::Claude, FOLDER).is_empty());
        assert!(commands.list(AgentId::Codex, FOLDER).is_empty());
        assert!(!commands.has_codex_skills(FOLDER));
        commands.set_claude_probe(&[command("compact", None)]);
        assert_eq!(commands.list(AgentId::Claude, FOLDER).len(), 1);
        commands.set_codex_result(FOLDER, Path::new("/w"), &json!({"data": []}));
        assert_eq!(names(&commands.list(AgentId::Codex, FOLDER)), ["compact"]);
        commands.forget_folder(FOLDER);
        assert!(!commands.has_codex_skills(FOLDER));
    }

    #[test]
    fn long_lists_and_many_folders_are_bounded() {
        let many: Vec<Command> = (0..1000).map(|i| command(&format!("c{i}"), None)).collect();
        assert_eq!(claude_filter(&many).len(), MAX_COMMANDS);
        let mut commands = Commands::default();
        for i in 0..(MAX_FOLDERS + 5) {
            commands.set_codex_skills(&format!("f{i:012x}"), &many);
        }
        assert_eq!(commands.list(AgentId::Codex, "f000000000000").len(), MAX_COMMANDS);
        assert!(!commands.has_codex_skills(&format!("f{:012x}", MAX_FOLDERS + 1)));
    }
}
