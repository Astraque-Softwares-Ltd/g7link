# Agents service (protocol v0)

> Status: **draft.** Builds on [protocol v0](v0.md); breaking changes are
> allowed until v1. License: CC BY 4.0.

A phone follows, answers and starts coding-agent threads that run on a paired
PC. The agent always runs on the PC, as the PC's user, in a folder the PC's
user listed there. The phone sends text and decisions and receives events; it
never sends a path, a command, a program name or a flag.

## 1. Capabilities and the toggle

| ID | Offered by | Meaning |
|---|---|---|
| `agent.host` | desktop | The agent host is installed on this PC (`agent.catalog` says which agents can run now) |
| `agent.remote` | phone | Shows a PC's agent threads and answers their requests |
| `agent.sessions` | desktop | Lists sessions started on this PC outside G7 Link (§3) |
| `agent.follow` | phone | Shows a PC's sessions started outside G7 Link (§3) |

Everything here is governed by the PC's per-device toggle `agents`, **off by
default**. While it is off for a phone the PC sends that phone nothing from
this service and answers each of its requests with `DENIED`; the first such
request makes the PC show its user a one-time prompt for that phone (as
`remote_input` does). When the toggle is turned on, the PC sends
`agent.catalog`. The phone's own `agents` toggle is not consulted.

The PC sends `agent.catalog`, `agent.event`, `agent.ask` and `agent.asked`
only to phones that offer `agent.remote`. A phone's requests are answered
whether or not it offers it.

## 2. Messages

All on the control stream, except §4's export. None are carried over Bluetooth.

```
t = "agent.catalog"            b = AgentCatalog                    PC → phone, no reply
t = "agent.threads"  id = n    b = {}                              phone → PC
t = "agent.threads"  re = n    b = { threads: [AgentThread] }      newest first, at most 50
t = "agent.start"    id = n    b = AgentStart                      phone → PC
t = "agent.thread"   re = n    b = AgentThread
t = "agent.send"     id = n    b = { thread: text, text: text }    phone → PC, → ok
t = "agent.cancel"   id = n    b = { thread: text }                phone → PC, → ok
t = "agent.read"     id = n    b = { thread: text, before?: uint, limit?: uint }   phone → PC
t = "agent.items"    re = n    b = AgentItems
t = "agent.watch"    id = n    b = { thread?: text }               phone → PC, → ok
t = "agent.answer"   id = n    b = { token: text, decision: text, scope?: text }   phone → PC, → ok
t = "agent.event"              b = AgentEvent                      PC → phone, no reply
t = "agent.ask"                b = AgentAsk                        PC → phone, no reply
t = "agent.asked"              b = { token: text, thread: text, outcome: text, by_name?: text }   PC → phone, no reply
```

```
AgentCatalog = {
  agents:  [ { id: text, name: text, installed: bool, signed_in: bool, login_hint: text,
               auth?: text } ],                                                               // at most 8
  folders: [ { id: text, label: text } ],                                                     // at most 64
  modes:   [ text ],            // what this phone may start: ["supervised"]
}
AgentStart  = { agent: text, folder: text, prompt: text, mode: text }   // folder: its id
AgentThread = {
  id: text, agent: text, folder: text,        // agent: its id; folder: its label
  title: text, state: text,                   // "working" | "waiting" | "done" | "failed" | "interrupted"
  mode: text, from_phone: bool,               // started by the phone this is sent to
  updated: uint,                              // Unix ms
  asks: uint,                                 // requests waiting for an answer
  summary?: text, error?: text,
  error_code?: text,                          // why it failed: "usage_limit", "login", or the agent's own word
  step?: text,                                // what a running turn is doing now
  since?: uint,                               // Unix ms: when `state` began
  plan?: [ { text: text, status: text } ],    // the agent's own plan; status: "pending" | "in_progress" | "completed"
}
AgentItem = {
  seq: uint, id: text, kind: text,            // "user" | "assistant" | "tool" | "notice"
  when: uint, text?: text, truncated?: bool,
  tool?: { name: text, title: text, detail?: text, status: text, output?: text },
                                              // status: "running" | "ok" | "error" | "denied"
}
AgentItems = { thread: AgentThread, items: [AgentItem], more_before: bool, asks: [AgentAsk] }
AgentAsk = {
  token: text, thread: text, thread_title: text, agent: text,   // agent: its id
  kind: text,                                 // "command" | "edit" | "other"
  title: text, command?: text, diff?: text, path?: text, reason?: text,
  truncated: bool, expires: uint,             // Unix ms
}
AgentEvent = {
  thread: text,
  kind: text,                                 // "state" | "delta" | "item" | "done" | "removed"
  state?: AgentThread,                        // "state", "done"
  seq?: uint, item_id?: text, offset?: uint, text?: text,   // "delta"
  item?: AgentItem,                           // "item"
  outcome?: text, summary?: text,             // "done": "done" | "failed" | "interrupted"
}
```

An agent's `auth` says how it's signed in: `subscription`, `api_key` (show
"Using an API key") or `other`. Nothing else about the account crosses: no
plan, email or account id. `agent.asked`'s `by_name` names the device that
answered an `allowed` or `denied` ask: the phone's name, or the PC's when it
was answered there.

`step` is what a running turn is doing: the tool's own description when the
agent gave one (Claude's `Bash` description), else its title (`Run command`,
`Edit file`); absent while nothing runs. Show it as "Now: <step>" with the time
since `since`, and don't build sentences around it. `plan` is the turn's plan
as the agent wrote it (Claude's todo list, Codex's plan), absent when it made
none. All three are optional; older PCs send none of them.

`seq` is 1-based per thread and never reused. A delta's `offset` is the byte
offset of its `text` in the item's text. `reason` is the agent's own words;
show it as the agent's, apart from the app's own wording. A failed thread's
`error_code` says why: `usage_limit` (show it as paused; its `error` may end
in `|resets_ms=<Unix ms>` when the agent said when the limit resets),
`login` (the agent is signed out on the PC; `error` is the agent's own
words), or another word from the agent.

### 2.1 Rules

- `agent.catalog` is sent when a session starts, when the `agents` toggle is
  turned on, and whenever it changes. A catalog with no agents means the host
  is not running. When the toggle is turned off the PC sends one last, empty
  catalog, then nothing.
- `agent.start`: `mode` must be one of the catalog's `modes`. The PC shows its
  user a notification naming the phone, the agent and the folder, with Stop.
- `agent.watch` names the one thread whose text the phone is showing; without
  `thread`, none. Only a watching phone gets `delta` and `item` events, and
  only for that thread. `state`, `done` and `removed` events and every
  `agent.ask` go to each connected phone with the toggle on, for every thread,
  whoever started it. A watch ends with the session.
- `agent.read` returns the newest `limit` items (default and most: 50), or the
  ones before `before`. A phone reads after it starts watching and whenever a
  `delta`'s `offset` or an `item`'s `seq` shows a gap.
- `agent.answer`: `decision` is `"allow"` or `"deny"`. `scope` is reserved;
  the PC answers `DENIED` to anything but `"once"`. A token is single use;
  the first answer from any device wins and every device then gets
  `agent.asked` with `outcome` `"allowed"`, `"denied"`, `"expired"` or
  `"cancelled"`.
- The phone keeps threads, items and asks in memory only.

### 2.2 Limits

| Field | Limit (bytes of UTF-8 unless noted) |
|---|---|
| ids (`agent`, `folder`, `thread`, `token`) | 1 to 40, `[a-z0-9]` |
| item `id`, delta `item_id` | 1 to 96, no control characters |
| `mode` | 1 to 32, `[a-z0-9_]` |
| `prompt`, `agent.send.text` | 1 to 8192 |
| `label`, `name`, `folder`, `login_hint`, tool `name` | 64 |
| `title`, `thread_title` | 80; ask and tool `title`: 200 |
| `step` | 120 |
| `plan` | 20 steps; a step's `text` 200 |
| `summary` / `error` / `reason` | 280 / 200 / 300 |
| `error_code` | 1 to 40, `[a-z0-9_]` |
| `auth` | 1 to 32, `[a-z0-9_]` |
| `by_name` | 64 |
| item `text` / tool `detail` / tool `output` | 16384 / 4096 / 2048 |
| ask `command` / `diff` / `path` | 2048 / 8192 / 512 |
| delta `text` | 4096; at most 10 `delta` events a second to one phone (later text is joined) |
| `agent.items` | at most 50 items, 196608 bytes of their text (each item counts 64 plus its id and text fields) and 16 asks; the oldest items are left out |
| `agent.catalog` | 8 agents, 64 folders, 8 modes |

A receiver drops a message that breaks a limit (a request is answered
`BAD_MESSAGE`). Text the PC sends carries no control characters other than
newline and tab, and no bidirectional formatting controls; text cut to fit
sets `truncated` where the type has it. Receivers still show all of it as
plain text.

### 2.3 Errors

- `UNSUPPORTED`: the PC doesn't offer `agent.host`, the host can't be
  reached, or the agent isn't installed or signed in.
- `DENIED`: the `agents` toggle is off for the phone; a `mode` outside
  `modes`; `scope` other than `"once"`; the folder was removed.
- `BAD_MESSAGE`: a field is missing, malformed or over its limit.
- `NOT_FOUND`: unknown agent, folder, thread or token, or a token already
  answered.
- `BUSY`: the thread is working, or too many threads are running.
- `INTERNAL`: the host failed.

## 3. Sessions started on the PC

A phone can also follow the Claude and Codex sessions the PC's user started
there without G7 Link: in a terminal, or in the agent's own desktop app. They
are read-only from the phone: there is no message that types into, answers or
stops a session. Once nothing has a session open, the phone may continue it as
a thread of its own.

The PC offers `agent.sessions`; a phone that shows them offers
`agent.follow`. Everything here is governed by a second per-device toggle on
the PC, `agent_sessions`, **off by default**, which counts only while
`agents` is on. While either is off the PC sends that phone nothing from this
section and answers each of its requests `DENIED`; with `agents` off that is
§1's prompt, and with `agents` on and `agent_sessions` off the first such
request makes the PC show its user a second one-time prompt for that phone.
The PC sends `agent.session` and `agent.session.item` only to phones that
offer `agent.follow`. Nothing here travels on Bluetooth.

```
t = "agent.sessions"          id = n  b = {}                                   phone → PC
t = "agent.sessions"          re = n  b = { sessions: [AgentSession] }         at most 50
t = "agent.session"                   b = { session?: AgentSession, removed?: text }   PC → phone, no reply
t = "agent.session.read"      id = n  b = { session: text, before?: uint, limit?: uint }
t = "agent.session.items"     re = n  b = { session: AgentSession, items: [AgentItem], more_before: bool }
t = "agent.session.watch"     id = n  b = { session?: text }                   → ok
t = "agent.session.item"              b = { session: text, item: AgentItem }   PC → phone, no reply
t = "agent.session.continue"  id = n  b = { session: text }                    → agent.thread
```

```
AgentSession = {
  id: text, agent: text, folder: text,       // agent: its id; folder: a name, never a path
  title: text, state: text,                  // "working" | "needs_you" | "may_need_you" | "idle" | "ended"
  guess: bool,                               // the state is inferred, not reported by the agent
  source: text,                              // "terminal" | "desktop_app" | "other"
  updated: uint,                             // Unix ms: when its file last changed
  since?: uint,                              // Unix ms: when `state` began
  step?: text, summary?: text, waiting_for?: text,
  can_continue: bool, blocked?: text,        // "open" | "folder" | "agent"
}
```

- `needs_you` means the agent itself reported that it waits for an answer
  (Claude's own status). `may_need_you` is a guess from the session's file (it
  stopped with a tool call open): it can't tell a permission prompt from a
  long-running command, so show it as "May need you" and never count it in a
  badge, a chip or a summary. `may_need_you` always has `guess` set; a PC never
  sends `needs_you` with `guess`, and a receiver reads one as `may_need_you`.
  `ended`: no process has the session open any more. A receiver reads a
  `source` it doesn't know as `other` and ignores a `blocked` it doesn't know.
- `agent.session` says a session appeared or changed (`session`), or left the
  list (`removed`, its id): it left the PC's window, its file went away, or it
  became a thread. Exactly one of the two. It goes to every connected phone
  with both toggles on, at most once per session every 2 s: a change within
  2 s of the last one waits, and only the newest waiting one is sent. A
  removal goes at once.
- `agent.session.watch` names the one session whose items the phone shows;
  without `session`, none. Only the watching phone gets `agent.session.item`,
  and only for that session. A watch ends with the session (the connection),
  when either toggle goes off, and when the session is removed. A PC may
  refuse a watch with `BUSY` when it already serves as many watching phones as
  it can (G7 Link desktop: two).
- `agent.session.read` returns the newest `limit` items (default and most:
  50) or those before `before`. A session's item `seq` comes from its place in
  the session's file: it grows, may be 0, and isn't consecutive, so a phone
  never looks for gaps; it reads again after it starts watching and after
  reconnecting. Items are added or updated (a tool step finishing) as whole
  blocks, never as streamed text.
- `agent.session.continue` makes the session a supervised thread started by
  that phone and answers it as `agent.start` does. The thread holds the
  session's last 200 items and stays idle until the phone sends something.
  The PC answers `BUSY` while anything has the session open, `DENIED` when its
  folder isn't one of the PC's agent folders (a phone never adds one), and
  `UNSUPPORTED` when the agent can't run. The session is then removed.
- `can_continue` says whether `agent.session.continue` would be taken now;
  when it isn't, `blocked` says why: `open` (still open in a terminal or the
  app), `folder` (not in an agent folder) or `agent` (the agent can't run).

Limits: `step` 120 bytes, `waiting_for` 80, the rest as §2.2; ids 1 to 40
`[a-z0-9]`; `agent.session.items` as `agent.items` (50 items, 196608 bytes,
the oldest left out); `folder` never contains `/` or `\` (a PC cuts a path
to its last component). Errors as §2.3; `NOT_FOUND` for a session that isn't
listed.

## 4. Export

A phone can ask for a thread, or a session it may follow, as Markdown to share
or keep. The PC renders it: the title, the agent, the folder's name (never its
path) and when it started; then each message (the user's quoted, with who sent
it), each tool step with its command and the end of its output, each edit as a
`diff` block, and each approval as "Allowed on <device>". Nothing is exported
unless a phone asks; the PC keeps no copy.

The text can be larger than a control message, so it comes on a bidirectional
stream of its own, one export per stream:

```
stream header  { svc: "agent", op: "export", v: 1 }
t = "agent.export"       id = 1  b = { thread?: text, session?: text }   // phone → PC, exactly one
t = "agent.export.meta"  re = 1  b = { length: uint, truncated: bool, file_name: text }
<length bytes of UTF-8 Markdown>
```

- An export of a thread needs the `agents` toggle; of a session, `agent_sessions`
  as well, with §1's and §3's prompts and errors (`DENIED`, `UNSUPPORTED`,
  `NOT_FOUND`, `BAD_MESSAGE`, `INTERNAL`), sent as an error reply on the stream
  instead of the meta.
- The text is at most 2 MiB (2097152 bytes). When the thread doesn't fit, the
  PC leaves out the items that don't, ends the text saying so, and sets
  `truncated`. It carries no control characters other than newline and tab and
  no bidirectional formatting controls.
- `file_name` is a suggested name to save it as (`claude-<title>-<yyyy-mm-dd>.md`):
  1 to 128 bytes of `[A-Za-z0-9._-]`, not starting with `.`.
- A phone refuses a meta over these limits before reading on, checks that the
  bytes are UTF-8, keeps the text in memory only and treats it as plain text.
  The PC gives up on a stream that doesn't ask within 20 s, and on one whose
  text doesn't cross within 60 s; a phone waits 20 s for the meta.

## 5. Implementation notes

- G7 Link desktop talks to the agent host (`g7-agents`) on a local socket and
  relays its threads; the folder's path never leaves the PC. The PC's
  notifications about agents come from the host, not from G7 Link.
- The core answers `agent.watch` itself and limits deltas per phone; the
  other requests go to the platform (`Platform::agent_request`) on a blocking
  thread.
- On the phone, a `BUSY` reply reaches the app as the generic failure
  (`G7LinkError.Internal`, whose reason starts with `BUSY`), not as `Denied`,
  which means the toggle is off.
- Sessions (§3) come from the agent host, which reads the agents' own session
  files on the PC, read-only. Nothing of a session leaves the PC until a phone
  with both toggles asks; its folder's path never does. The core answers
  `agent.session.watch` for itself as well as telling the platform (G7 Link
  desktop keeps one host connection per watching phone), paces
  `agent.session`, and cuts a folder path to its name.
- The core answers the `agent/export` stream itself, checking the toggles,
  and asks the platform for the text (`AgentRequest::Export`); G7 Link
  desktop gets it from the host's `thread.export`.
- G7 Link desktop's second prompt, for `agent_sessions`: title "Let %1 see
  sessions started on this PC?"; body "It will be able to list and read the
  Claude and Codex sessions you start here in a terminal or the Claude app, in
  any folder, including what you and the agent wrote. It can't type into them.
  Once one ends, it can continue it as a thread in your agent folders. It sees
  each folder's name, never its path."; buttons Not now, Allow.
