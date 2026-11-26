# Agents service (protocol v1)

> Status: **v1.** Builds on [protocol v1](v1.md). License: CC BY 4.0.

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
| `agent.say_why` | desktop | `agent.answer` may carry `message`: deny and say why (§2.4) |
| `agent.control` | both | Models and effort, plan mode, steering and the queue, the agent's questions, images, `@` files, `/` commands and drafts (§5) |
| `agent.reply` | desktop | Can enter a reply into a session started on this PC now (§9) |
| `agent.move` | desktop | Can move a session started on this PC to G7 now (§9.1); offered only with `agent.reply` |
| `agent.reply.needs.desktop` | desktop | Offered instead of `agent.reply`: G7 can't type replies on this desktop (§9) |
| `agent.reply.needs.extension` | desktop | Offered instead of `agent.reply`: the desktop's extension is missing or too old (§9) |
| `agent.reply.needs.keyboard` | desktop | Offered instead of `agent.reply`: no virtual keyboard can be made (§9) |

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

The PC's events (`agent.catalog`, `agent.thread`, `agent.event`,
`agent.ask`, `agent.asked`, and §3's and §5's) go one way on the `agent`
lane, in order ([protocol v1](v1.md) §4.2). Every phone request is an RPC
`agent/<op>` (§4.3 there), answered `ok` with the body shown, or a plain
`ok`; the lists are paged (§4.4 there). A phone waits up to 20 s for an
answer, longer than the PC's own 15 s limit on the host. §4's export and
§5.3's images have streams of their own. None of this is carried over
Bluetooth.

The agent host keeps the order of `agent/send`, `agent/queue` and
`agent/cancel` for one thread; a phone doesn't rely on their arrival order.

```
t = "agent.catalog"   b = AgentCatalog                                     PC → phone, agent lane
t = "agent.event"     b = AgentEvent                                       PC → phone, agent lane
t = "agent.ask"       b = AgentAsk                                         PC → phone, agent lane
t = "agent.asked"     b = { token: text, thread: text, outcome: text, by_name?: text, message?: text }   PC → phone, agent lane

RPC agent/threads   t = "agent.threads"   b = {}                                    → ok { threads: [AgentThread] }   paged; newest first, at most 50
RPC agent/start     t = "agent.start"     b = AgentStart                            → ok AgentThread
RPC agent/send      t = "agent.send"      b = { thread: text, text: text }          → ok
RPC agent/cancel    t = "agent.cancel"    b = { thread: text }                      → ok
RPC agent/read      t = "agent.read"      b = { thread: text, before?: uint, limit?: uint }   → ok AgentItems   paged
RPC agent/watch     t = "agent.watch"     b = { thread?: text }                     → ok
RPC agent/answer    t = "agent.answer"    b = { token: text, decision: text, scope?: text, message?: text }   → ok
```

```
AgentCatalog = {
  agents:  [ { id: text, name: text, installed: bool, signed_in: bool, login_hint: text,
               auth?: text } ],                                                               // at most 8
  folders: [ { id: text, label: text } ],                                                     // at most 64
}                               // a mode list: each agent's own (§5.1)
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
  seq: uint, id: text, kind: text,            // "user" | "assistant" | "tool" | "notice" | "reasoning" (§2.4)
  when: uint, text?: text, truncated?: bool,
  tool?: { name: text, title: text, detail?: text, status: text, output?: text,
                                              // status: "running" | "ok" | "error" | "denied"
           class?: text, path?: text, by_rule?: bool },   // §2.4
  turn?: uint, phase?: text, end?: uint,      // §2.4
  group?: text, deny_of?: text,               // §2.4
}
AgentItems = { thread: AgentThread, items: [AgentItem], more_before: bool, asks: [AgentAsk] }
                                              // paged: `thread` and `more_before` in every part,
                                              // `asks` (and §5's `questions`) in the first only
AgentAsk = {
  token: text, thread: text, thread_title: text, agent: text,   // agent: its id
  kind: text,                                 // "command" | "edit" | "other"
  title: text, command?: text, diff?: text, path?: text, reason?: text,
  truncated: bool, expires: uint,             // Unix ms
  item?: text,                                // the tool item it belongs to (§2.4)
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
none. All three are optional.

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
- `agent.start`: `mode` is one of the agent's catalog `modes` (§5.1); a phone
  without `agent.control` starts in `ask` only. The PC shows its user a
  notification naming the phone, the agent and the folder, with Stop.
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
- The rules of §2.4 apply to the fields added there.

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
| tool `path` | 512; relative, never absolute (§2.4) |
| `group`, `deny_of`, ask `item` | 1 to 96, no control characters (item ids) |
| `phase`, tool `class` | 1 to 32 |
| `agent.answer` / `agent.asked` `message` | 1 to 2000 / 2000 |
| delta `text` | 4096; at most 10 `delta` events a second to one phone (later text is joined) |
| `agent.read` answer | at most 50 items and 16 asks; the oldest items past 50 are left out. However large, it comes in parts ([protocol v1](v1.md) §4.4) |
| `agent.catalog` | 8 agents, 64 folders; 8 modes an agent, one mode per agent a folder |

A receiver drops a message that breaks a limit (a request is answered
`BAD_MESSAGE`). Text the PC sends carries no control characters other than
newline and tab, and no bidirectional formatting controls; text cut to fit
sets `truncated` where the type has it. Receivers still show all of it as
plain text.

### 2.3 Errors

- `UNSUPPORTED`: the PC doesn't offer `agent.host`, the host can't be
  reached, or the agent isn't installed or signed in; `message` to a PC that
  doesn't offer `agent.say_why`.
- `DENIED`: the `agents` toggle is off for the phone; `scope` other than
  `"once"`; the folder was removed.
- `UNAVAILABLE`: a `mode` the agent doesn't offer (§5.1).
- `BAD_MESSAGE`: a field is missing, malformed or over its limit; `message`
  with a decision other than `"deny"`.
- `NOT_FOUND`: unknown agent, folder, thread or token, or a token already
  answered.
- `BUSY`: the thread is working, or too many threads are running.
- `INTERNAL`: the host failed.

### 2.4 How a turn reads (`agent.say_why`)

Every field below is optional: a receiver reads the rest of the message
without it.

```
AgentItem += turn?: uint,                      // 1-based per thread; absent on items from before turns were kept
             phase?: text,                     // kind "assistant": "interim" | "final"
             end?: uint,                       // Unix ms: when it finished (answer done, tool ended, thinking stopped)
             group?: text,                     // the id of the tool item it was produced inside (a sub-agent's work)
             deny_of?: text                    // kind "user": the id of the tool step this message denied
AgentTool += class?: text,                     // "command" | "edit" | "read" | "search" | "web" | "mcp" | "agent" | "tasks" | "other"
             path?: text,                      // the file an edit or read is about
             by_rule?: bool,                   // allowed by the thread's "Always allow" rule, without asking
             review?: { status: text,          // "approved" | "denied" | "timed_out" | "aborted"
                        risk?: text,           // the reviewer's word: "low", "medium", "high"
                        rationale?: text }     // its own words, at most 600 bytes
AgentAsk  += item?: text,                      // the id of the tool item the request belongs to
             deny_stops?: bool                 // denying it stops the agent's turn
agent.answer += message?: text                 // 1 to 2000 bytes, with decision "deny" only
agent.asked  += message?: text                 // with outcome "denied": what the user said
```

- **Turns.** A turn starts with a `user` item and holds what follows until
  the next one. `turn` numbers them from 1; a `user` item with `deny_of` stays
  in the turn it answers (Claude) or starts the next one (Codex: the turn was
  stopped and the message sent as a follow-up). A receiver that gets items
  without `turn` counts turns itself, advancing at every `user` item without
  `deny_of`.
- **Phase.** `final` marks the turn's answer, `interim` what the agent said on
  the way. A turn whose items have no phase (a turn that was
  stopped) has no known answer; a receiver may take its last `assistant` item
  once the turn has ended.
- **Reasoning.** An item of kind `reasoning` is the agent thinking. Its `text`
  is what the agent showed of its thought, often empty (the agents don't send
  it today); `end` is set when the thinking stopped. Its text streams as
  `delta` events like an answer's.
- **Words a receiver doesn't know.** A `phase` or `class` that isn't one of
  the words above is read as absent, not refused, so a later word doesn't
  break a receiver. A `phase` on an item that isn't `assistant`, a
  `deny_of` on one that isn't `user`, and `turn` 0 are read as absent too.
- **Paths.** A tool's `path` and an ask's `path` are relative to the
  thread's folder, or, for a file outside it, the file's name alone: the
  phone never receives an absolute path. A receiver cuts one that looks
  absolute (`/`, `\`, `~`, a drive letter), starts with `…` (cut from the
  left) or climbs out with `..` to its last component.
- **Rules.** A tool's `by_rule` says the PC allowed the step from an "Always
  allow" rule its user set for the thread, without asking anyone; show
  "Allowed by your rule for this thread" with it, in place of "Allowed on
  <device>". It's absent otherwise.
- **Reviews.** A tool's `review` says the agent's own reviewer decided about
  the step without asking anyone (Codex's "Approve for me", Claude's Auto
  classifier, which gives no risk or rationale). With `approved`, show
  "Approved by <agent>'s reviewer" with the step while no answer or rule
  says more; a denial or a timeout also arrives as a notice. A `status` a
  receiver doesn't know leaves the step without its review; a `risk` that
  isn't a word of at most 32 bytes is left out; the `rationale` is cleaned
  and cut like an item's text.
- **Deny that stops.** An ask with `deny_stops` can't be refused on its own:
  Codex offered no "decline" for it (an escalation in "Ask for approval"),
  so Deny stops the agent's turn. Say so on Deny ("Deny stops this turn").
- **Ids.** `group`, `deny_of` and an ask's `item` are item ids (1 to 96 bytes,
  no control characters). One that isn't is left out; it's never cut.
- **Deny and say why.** A phone sends `message` only to a PC that offers
  `agent.say_why`, and only with `"deny"`: the PC answers `BAD_MESSAGE` to a
  `message` with any other decision, and `UNSUPPORTED` to one it can't pass
  on. The message is the user's own words to the agent about what to do
  instead; the PC strips control characters other than newline and tab, and
  bidi controls, before passing it on. The request is denied either way. Every
  device then gets `agent.asked` with `outcome: "denied"`, `by_name` and the
  same `message`, and the thread gets a `user` item with the message and
  `deny_of` naming the denied tool step.

## 3. Sessions started on the PC

A phone can also follow the Claude and Codex sessions the PC's user started
there without G7 Link: in a terminal, or in the agent's own desktop app. They
are read-only from the phone: no message answers or stops a session, and only
§9's reply and its move to G7 (§9.1), behind a toggle of their own, type into
one. Once nothing has a session open, the phone may continue it as a thread of
its own.

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
t = "agent.session"        b = { session?: AgentSession, removed?: text }   PC → phone, agent lane
t = "agent.session.item"   b = { session: text, item: AgentItem }           PC → phone, agent lane

RPC agent/sessions           t = "agent.sessions"           b = {}                     → ok { sessions: [AgentSession] }   paged; at most 50
RPC agent/session.read       t = "agent.session.read"       b = { session: text, before?: uint, limit?: uint }
                                                                                       → ok { session: AgentSession, items: [AgentItem], more_before: bool }   paged
RPC agent/session.watch      t = "agent.session.watch"      b = { session?: text }     → ok
RPC agent/session.continue   t = "agent.session.continue"   b = { session: text }      → ok AgentThread
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
  limit?: { resets_ms?: uint },              // stopped at the agent's usage limit (0.1.20)
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
- `limit` says the session stopped at its agent's usage limit; its `state`
  is then `idle`, never `working`. `resets_ms` (Unix ms) is when the limit
  resets, when the agent said. Show "Limit reached · resets …", or "Limit
  reached" without a time, with the stopped mark. A receiver that doesn't know
  the field shows the state alone. It goes once the session moves on.
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
- `agent.session.continue` makes the session a thread started by that phone,
  in the mode the folder remembers for the agent (else `ask`), and answers it as `agent.start` does. The thread holds the
  session's last 200 items and stays idle until the phone sends something.
  The PC answers `BUSY` while anything has the session open, `DENIED` when its
  folder isn't one of the PC's agent folders (a phone adds one with
  `agent.folder.add`, §15), and
  `UNSUPPORTED` when the agent can't run. The session is then removed.
- `can_continue` says whether `agent.session.continue` would be taken now;
  when it isn't, `blocked` says why: `open` (still open in a terminal or the
  app), `folder` (not in an agent folder) or `agent` (the agent can't run).

Limits: `step` 120 bytes, `waiting_for` 80, the rest as §2.2; ids 1 to 40
`[a-z0-9]`; the `agent.session.read` answer as `agent.read`'s (50 items, the
oldest left out, in parts when large); `folder` never contains `/` or `\` (a PC cuts a path
to its last component). Errors as §2.3; `NOT_FOUND` for a session that isn't
listed.

## 4. Export

A phone can ask for a thread, or a session it may follow, as Markdown to share
or keep. The PC renders it: the title, the agent, the folder's name (never its
path) and when it started; then each message (the user's quoted, with who sent
it), each tool step with its command and the end of its output, each edit as a
`diff` block, and each approval as "Allowed on <device>". Nothing is exported
unless a phone asks; the PC keeps no copy.

The text can be larger than a frame, so it comes on a bidirectional bulk
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

## 5. Control (`agent.control`)

Added after G7 Link 0.1.16. A phone that offers `agent.control` picks the
model and effort, plans before it acts, steers a running turn or queues a
message behind it, answers the agent's questions, sends pictures, finds files
for `@` and commands for `/`, and carries on with a message started on
another device. A PC that offers it takes all of that. **Only while both
offer it** does either side send or answer anything of this section: to any
other phone the PC sends what §2 says and nothing more (§5.7), and a phone
sends a PC without it nothing of this section (it gives up with
`UNSUPPORTED` without sending). Everything here follows the `agents` toggle
as §2 does, with its prompt and errors. Nothing here travels on Bluetooth.

The models, effort levels, commands and limits are the ones the agents' own
CLIs report on the PC. The PC asks no vendor for them and keeps no list of
its own.

### 5.1 Messages

```
t = "agent.question"     b = AgentQuestion                                PC → phone, agent lane
t = "agent.questioned"   b = { token, thread, outcome, by_name? }         PC → phone, agent lane
t = "agent.draft"        b = { key, rev }                                 PC → phone, agent lane

RPC agent/configure   t = "agent.configure"   b = { thread, model?, effort?, mode?, custom_model? }    → ok AgentThread
RPC agent/queue       t = "agent.queue"       b = { thread, id, op, text? }                             → ok
RPC agent/reply       t = "agent.reply"       b = { token, answers: [AgentAnswer] } | { token, dismiss: true }   → ok
RPC agent/implement   t = "agent.implement"   b = { thread, item, mode? }                               → ok
RPC agent/files       t = "agent.files"       b = { thread? | folder?, query, limit? }                 → ok { files: [{ path, dir? }], more }   paged
RPC agent/commands    t = "agent.commands"    b = { agent, folder }                                     → ok { commands: [AgentCommand] }   paged
RPC agent/draft.get   t = "agent.draft.get"   b = { key }                                               → ok { draft?: AgentDraft }
RPC agent/draft.set   t = "agent.draft.set"   b = { key, text, images?: [text], rev }                   → ok { rev, conflict?: AgentDraft }
```

What §2's bodies gain:

```
AgentCatalog.agents[] += models?: [AgentModel],   // at most 40; absent: the CLI hasn't said yet
                         can?: [text],            // "plan" | "questions" | "steer" | "images" | "mcp_switch" | "commands"
                         limits?: [AgentLimit],   // at most 4
                         modes?: [text]           // at most 8, the CLI's order: what a thread of it may use
AgentCatalog.folders[] += modes?: [{ agent: text, mode: text }]   // one per agent: its last mode there, never "plan"
AgentStart  += model?: text, effort?: text, images?: [text], custom_model?: bool;  mode any of the agent's modes
agent.send  += dispatch?: text,                   // "auto" | "steer" | "queue"
               images?: [text]                    // `text` may then be empty
AgentThread += model?: text, effort?: text,       // the next turn's; absent: the CLI's default
               answered_model?: text,             // what the CLI says answered last
               queued?: [AgentQueued],            // at most 5, oldest first
               queue_held?: bool,                 // the last turn didn't finish: nothing is sent by itself
               context?: { used: uint, max: uint },
               mcp?: { on: bool, servers: uint, connected: uint, need_login: uint },
               asking?: uint                      // questions waiting (not counted in `asks`)
AgentItem   += via?: text,                        // kind "user": "steer" | "queue"
               images?: [AgentImage],             // kind "user"
               plan_state?: text,                 // kind "plan": "proposed" | "implemented" | "replaced"
               question?: { count: uint, secret?: bool, outcome?: text, by_name?: text }
                                                  // kind "question": how many it asked, how it ended (§5.2)
agent::item += "plan", "question"
AgentTool.class += "plan"                         // the plan file's edit (below)
AgentItems  += questions?: [AgentQuestion]        // waiting in the thread; at most 4

AgentModel    = { id: text, name: text, note?: text, default?: bool, efforts?: [text],
                  default_effort?: text, images?: bool, short?: bool }
AgentLimit    = { window: text, used_pct?: uint, resets?: uint, state: text }   // "ok" | "warn" | "reached"
AgentQueued   = { id: text, text: text, truncated?: bool, by_name?: text, images?: [AgentImage], created: uint }
AgentImage    = { id: text, name: text, bytes: uint, w: uint, h: uint }
AgentQuestion = { token: text, thread: text, thread_title: text, agent: text, item: text,
                  secret?: bool, asks: [AgentAsk2], created: uint, expires: uint }
AgentAsk2     = { id: text, header: text, text: text, multi?: bool, other?: bool, secret?: bool,
                  options?: [{ label: text, note?: text }] }
AgentAnswer   = { id: text, labels?: [text], other?: text }
AgentCommand  = { name: text, note?: text, hint?: text, kind: text }            // "command" | "skill"
AgentDraft    = { key: text, text: text, images?: [AgentImage], rev: uint, by_name?: text }
```

(`AgentAsk2` is a question's ask, so as not to be confused with §2's
`AgentAsk`, a request for approval. All times are Unix ms.)

- **Models and effort.** A model's `id` is what the CLI takes; `short` ones
  (`opus`) follow the newest model and are listed first, the rest under "More
  models". An agent without `models` hasn't said yet: offer "Default" and a
  typed model. A typed model is sent with `custom_model` and needn't be
  listed. `effort` must be one the model lists. A model id is one word: no
  white space or control characters, not starting with `-`, at most 64 bytes;
  the PC passes it to the CLI as one argument, never through a shell, and
  maps effort and mode to fixed words of its own.
- **Modes.** A thread's `mode` is one of `ask`, `accept_edits`, `auto`,
  `plan` and `full`; each agent's `modes` list the ones it offers, in its
  CLI's order, and a phone may choose any of them. Names and descriptions
  never cross: each client words them per agent, by the CLI's own names
  (Claude: Manual, Accept edits, Auto, Plan, Bypass permissions; Codex: Ask
  for approval, Approve for me, Full access, Plan), and shows a word it
  doesn't know as it is. Claude offers `auto` only with a model that can run
  it; Codex has no `accept_edits`; `plan` needs the agent's `plan`. A mode
  the agent doesn't offer is `UNAVAILABLE`. Each folder's `modes` give the
  mode each agent last started or ran with there (never `plan`): a new
  thread there starts in it while the agent still offers it, else in `ask`.
  Both sides read the words before 0.1.31 (`supervised`, `auto_edit`) as
  `ask` and `accept_edits`; a phone without `agent.control` is sent them (§5.7).
- **`agent.configure`** changes a thread for its next turn: `model`,
  `effort`, `mode`, each optional and at least one; an empty `model` or
  `effort` goes back to the default. Any mode the agent offers, from a phone
  too. The PC answers `BUSY` while the thread works or waits. Nothing
  changes a running turn.
- **Plan mode.** A thread in mode `plan` changes nothing; the agent writes a
  plan, which arrives as an item of kind `plan` with `plan_state:
  proposed`, and the thread ends its turn. An earlier proposed plan of the
  thread becomes `replaced`. "Change the plan" is an ordinary `agent.send`.
  `agent.implement` implements the thread's newest proposed plan (its item
  id) in `mode`, any of the agent's modes but `plan` (absent: the one the
  folder remembers for the agent, else `ask`): the plan becomes
  `implemented`, and the PC sends the agent "Implement the plan." from that
  phone. `NOT_FOUND` for any other plan, `BUSY`
  while the thread runs. Claude writes its plan to a file outside the folder
  first: that edit is a tool item of class `plan`, which a phone gets as a
  step named `plan`, titled "Plan", with no detail, output, path or text.
- **Sending while a turn runs.** `dispatch: "steer"` gives the message to
  the running turn: Codex reads it at once, Claude at its next step (after
  the tool it is running). `"queue"` puts it in the thread's queue, sent when
  the turn ends; `"auto"`, or `"steer"` to an agent that can't be steered
  (no `steer` in `can`), queues it too. Without `dispatch` a running thread
  answers `BUSY`, as before. A message the agent has taken in shows as a
  `user` item with `via`. While the thread waits for an answer (a request or
  a question) every send is `BUSY`. A sixth queued message is `BUSY`.
- **The queue.** `queued` lists the messages waiting, oldest first. When a
  turn ends done the oldest is sent (its item has `via: "queue"`), one at a
  time. When it fails or is stopped, `queue_held` is set and nothing is sent
  until the user sends something, sends one now, or removes them. A usage
  limit holds it too, never resumed by itself. `agent.queue` changes one:
  `op` `edit` (with `text`, 1 to 16384 bytes), `remove`, or `send_now` (steer
  with it while the thread works and the agent can, else start a turn with
  it; `BUSY` while it waits for an answer). `NOT_FOUND` when it was sent
  already. Only messages a person wrote and sent enter the queue.
- **Context and limits** are as the agent's CLI last reported them; absent
  until it has. `used_pct` 0 to 100; `resets` when the window resets;
  `state` `warn` when it's nearly used, `reached` when it is. A client words
  `window` itself (`"300"` → "5-hour", `"10080"` → "weekly", `"five_hour"`,
  `"seven_day"`, anything else as given). Nothing else about the account
  crosses.
- **MCP.** `mcp` counts the PC user's own MCP servers in the thread, as the
  agent reported them: `servers` in all, `connected`, and `need_login` (signed
  in in a terminal on the PC). Their names, commands and settings never
  reach a phone; a phone can't change the PC's per-folder switch.
- **Files for `@`.** `agent.files` searches a thread's folder (`thread`) or
  a folder (`folder`), exactly one; `query` is what follows the `@` (empty:
  the folder's top level and its recently changed files), `limit` 1 to 50
  (default 20). Each hit's `path` is relative to the folder with `/` between
  components, a directory may end with `/`: never absolute, never with `..`,
  `.` or an empty component (a receiver leaves out a hit that is). The PC
  reads nothing for it: the message carries the path as text and the agent
  opens the file itself, under its own permission rules.
- **Commands for `/`.** `agent.commands` lists what `agent` offers in
  `folder`: its CLI's commands, less those that need a terminal or touch the
  account or its settings, and its skills (`kind: "skill"`). `name` is one
  word, without the `/`; `hint` is its arguments. A command is sent as the
  first word of a message the user typed. Clients add their own `/model`,
  `/effort` and `/plan`, which are never sent.
- **Drafts.** The PC keeps the message being written per thread (`key`: its
  id) or per folder for a new thread (`new:<folder id>`), so another device
  can carry on with it. `agent.draft.set` stores it when `rev` is the draft's
  `rev` as the phone last saw it (0 for a new one) and answers its new `rev`;
  otherwise it stores nothing and answers `conflict`, the draft as it is. An
  empty text with no images clears it. After a change the PC sends
  `agent.draft` (`key`, `rev`) to every other phone that offers
  `agent.control`, which reads it with `agent.draft.get` when it matters. A
  draft is removed when its text is sent from any device. The PC keeps at
  most 64.

### 5.2 Questions

An agent may ask the user something mid-turn: `agent.question` goes to every
phone that offers `agent.control` with the toggle on, and the thread is
`waiting` with `asking` counting them. A question has 1 to 4 asks; each has
up to 6 options, may allow more than one (`multi`) and the user's own words
(`other`, "Something else"). It lives two hours, as a request does; at most
two wait per thread.

- `agent.reply` answers it, one answer per ask, by the options' own labels
  and/or `other` (the PC answers `BAD_MESSAGE` to a label that isn't the
  ask's own without `other`); or dismisses it (`dismiss: true`: the agent is
  told the user didn't answer). The first answer from any device wins; every
  phone then gets `agent.questioned` with `outcome` `answered`, `dismissed`,
  `expired` or `cancelled`, and `by_name`. A later answer is `NOT_FOUND`.
- **Secret questions** are answered on the PC only. A phone gets one with
  `secret: true` and no asks, never their words or options, and the PC
  answers `DENIED` to a phone's `agent.reply` for it. A secret answer is
  never stored, logged or sent to a phone.
- The `question` item's text lists the questions and their answers once
  answered, and is absent when any ask was secret. Its `question` says how
  many it asked, whether one was secret, and once it ended its `outcome`
  (as `agent.questioned`'s) and `by_name`; without `outcome` it waits. A
  receiver reads an `outcome` it doesn't know as absent.
- A request for a value or a sign-in from an MCP server is declined by the
  PC with a notice item; it never reaches a phone.

### 5.3 Images

Up to 4 images go with a message (`agent.start`, `agent.send`, a draft),
by id. A phone sends each first on a bidirectional stream of its own:

```
stream header  { svc: "agent", op: "image", v: 1 }
t = "agent.image.meta"   id = 1  b = { name: text, bytes: uint }    // phone → PC
t = "ok"                 re = 1                                     // PC → phone: send the bytes (or an error)
<bytes bytes>, then the stream's end                                 // phone → PC
t = "agent.image"        re = 1  b = AgentImage                     // PC → phone (or an error)

stream header  { svc: "agent", op: "thumb", v: 1 }
t = "agent.image.thumb"       id = 1  b = { id: text, max?: uint }  // phone → PC
t = "agent.image.thumb.meta"  re = 1  b = { length: uint }          // a JPEG, at most 153600 bytes
<length bytes>
```

- `bytes` is 1 to 10485760 (10 MiB). The PC refuses a meta over it before
  any byte comes, answers `BUSY` while that phone has another upload running
  (one at a time per phone), and drops an upload whose bytes don't end
  within 60 s. When more bytes come than `bytes` said, it answers
  `BAD_MESSAGE` and keeps none of them.
- The PC decodes the picture with its own bounded decoder (PNG, JPEG, WebP or
  GIF, by its first bytes; at most 12000 px a side), scales it to 2048 px at
  most and keeps a re-encoded copy without its metadata (location, camera,
  colour profile); the bytes as sent are never stored. `BAD_MESSAGE` for
  anything else ("That isn't an image G7 can send.") or too large ("That
  image is too large."). `AgentImage.name` is a file name, never a path.
- `agent.image.thumb` returns a JPEG of an image of a thread the phone may
  read, no larger than `max` px a side (64 to 1024; default 512) and 150
  KiB. A phone refuses a meta over that before reading on and checks it's a
  JPEG.
- Images, like everything here, follow the `agents` toggle (`DENIED`), and
  need `agent.control` on both sides (`UNSUPPORTED`).

### 5.4 Rules

- A phone never receives: an MCP server's name or settings; an absolute path
  (file hits are relative; the plan file's edit comes without its path or
  content; an image's name is a file name); a secret question's asks; any
  usage limit field but `window`, `used_pct`, `resets` and `state`; a
  model's `note` longer than 120 bytes; the PC's prompt history; a sign-in
  token, an account detail (email, organisation, plan, credits) or any other
  credential.
- A phone may: start and configure threads in any mode the agent offers;
  implement a plan in any of them but `plan`; steer, queue, edit, remove and send now
  queued messages; answer or dismiss a question that isn't secret; send up to
  4 images per message, one upload at a time; search files and list commands
  of folders the catalog lists.
- A phone may not: change a folder's MCP switch, read or change the PC's
  "keep awake" setting, add an image by a path, or read another device's
  draft images other than as thumbnails.
- Every message, queue change, plan implementation and answer a phone sends
  is shown on the PC with the phone's name, as messages are.
- A receiver reads a `can`, `via`, `plan_state`, limit `state` or command
  `kind` it doesn't know as absent (a limit as `ok`, a command as
  `command`), and doesn't refuse the message for it. A model, option or
  command whose id, label or name isn't whole is left out, never cut.

### 5.5 Limits

| Field | Limit (bytes of UTF-8 unless noted) |
|---|---|
| model `id` | 1 to 64; no white space or control characters; not starting with `-` |
| model `name` / `note` | 64 / 120 |
| `models` | 40 per agent |
| effort | 1 to 16, `[a-z0-9_]`; 8 per model |
| `can` | 16 words of 1 to 32 |
| `limits` | 4; `window` 1 to 32 |
| `queued` | 5 per thread; `text` 16384; `id` 16 lowercase hex digits |
| `agent.threads` answer | queued messages whole, as in a thread's page and its events; a list larger than a frame comes in parts |
| `images` | 4 per message; `id` 32 lowercase hex digits; `name` 128 |
| `agent.image.meta` `bytes` | 1 to 10485760 |
| thumbnail | 153600; `max` 64 to 1024 px |
| question | 1 to 4 asks; `item` an item id (1 to 96) |
| ask `id` / `header` / `text` | 1 to 64 / 40 / 500 |
| options | 6 per ask; `label` 1 to 80; `note` 200 |
| answer | `labels` 6 of 1 to 80; `other` 1 to 500 |
| `agent.read` answer `questions` | 4 |
| `agent.files` | `query` 128; `limit` 1 to 50; `path` 1 to 512 |
| `agent.commands` | 300; `name` 1 to 64, one word; `note` 200; `hint` 80 |
| draft `key` | a thread id, or `new:` and a folder id (at most 64) |
| draft `text` | 16384 |

### 5.6 Errors

As §2.3, and: `UNSUPPORTED` for anything of this section when the PC or the
phone doesn't offer `agent.control`; `UNAVAILABLE` for a mode the agent
doesn't offer; `DENIED` for a phone's reply to a secret question;
`BAD_MESSAGE` for a model id that isn't one word, more than 4 images, an
image's bytes that don't match its meta, and an answer that isn't one;
`NOT_FOUND` for a queued message already sent, a question already answered,
a plan that isn't the newest proposed one; `BUSY` while a thread waits for
an answer, for a sixth queued message, for configuring a running thread, and
for a second upload at once. A draft conflict is not an error: it's the
`conflict` reply.

### 5.7 Older devices

A phone without `agent.control` gets what it got before, whatever the PC
does: a catalog without `models`, `can`, `limits` and `modes`, and folders
without `modes`; a thread's mode in the old words (`accept_edits` as
`auto_edit`, any other as `supervised`), without `model`,
`effort`, `answered_model`, `queued`, `queue_held`, `context`, `mcp` and
`asking`; a `plan` item as an `assistant` item with the plan's text; a
`question` item as a tool step named `question`, titled "Question",
`running` until it ends (`end` or an `outcome`) and then `ok`, with no text
and no `question`; a user item
without `via` and `images`; no `agent.question`, `agent.questioned`,
`agent.draft` or `questions`. Its `seq`s are as they are for everyone, so it
sees no gap. Its requests are answered as before, except a mode other than
`ask` (`supervised`), which is `UNSUPPORTED` like the rest of this section. A phone that offers `agent.control`
talking to a PC that doesn't sends it nothing of this section and offers
what that PC's catalog says.

## 6. History and undo

Added in G7 Link 0.1.19. The PC keeps every thread in one store, so a phone
can read a thread's turns and what each changed, search everything the PC
holds, pin, archive, mark, snooze and rename threads, fork a thread, rewind
its conversation (and undo that), and bring in the Claude and Codex sessions
found on the PC. It's plain protocol v1: no capability, both apps ship
together. Everything follows the `agents` toggle (§1); `agent/found`,
`agent/found.preview` and `agent/import` need `agent_sessions` as well (§3),
with the same prompts and errors. Nothing here travels on Bluetooth.

```
t = "agent.turn"              b = { thread, turn: AgentTurn }                       PC → phone, agent lane
t = "agent.cut"               b = { thread, from_seq, to_seq, restored? }           PC → phone, agent lane
t = "agent.import.progress"   b = { job, done, failed, total, thread? }             PC → phone, agent lane

RPC agent/threads          t = "agent.threads"          b = { show?, before_ms? }                         → ok { threads: [AgentThread] }   paged
RPC agent/turns            t = "agent.turns"            b = { thread, before?, limit? }                   → ok { turns: [AgentTurn] }   paged
RPC agent/diff.files       t = "agent.diff.files"       b = { thread, turn? | from? + to? }               → ok { files: [AgentTurnFile], more, adds, dels }   paged
RPC agent/diff.patch       t = "agent.diff.patch"       b = { thread, turn? | from? + to?, path }         → ok { patch, truncated, binary }   paged
RPC agent/search           t = "agent.search"           b = { query, limit?, cursor?, archived? }         → ok { hits: [AgentHit], next?, titles_only? }   paged
RPC agent/mark             t = "agent.mark"             b = { threads: [id], pinned?, archived?, unread?, snooze? }   → ok { threads: [AgentThread] }   paged
RPC agent/rename           t = "agent.rename"           b = { thread, title }                             → ok AgentThread
RPC agent/fork             t = "agent.fork"             b = { thread, turn? }                             → ok AgentThread
RPC agent/rewind.preview   t = "agent.rewind.preview"   b = { thread, turn }                              → ok { token, expires_ms, turns, messages, blocked? }
RPC agent/rewind.apply     t = "agent.rewind.apply"     b = { token, refill? }                            → ok AgentThread
RPC agent/rewind.undo      t = "agent.rewind.undo"      b = { thread }                                    → ok AgentThread
RPC agent/found            t = "agent.found"            b = { query?, agent?, show?, before_ms?, limit? } → ok { found: [AgentFound], next_ms?, total, scanning, partial }   paged
RPC agent/found.preview    t = "agent.found.preview"    b = { id, limit? }                                → ok { items: [AgentItem] }   paged
RPC agent/import           t = "agent.import"           b = { ids: [id] } | { cancel: job }               → ok { job, total } | ok
```

The event is `agent.import.progress`, not `agent.import`: that `t` is the
`agent/import` RPC's request.

```
AgentThread += pinned?: bool, archived?: bool, unread?: bool,
               snoozed_until_ms?: uint,           // hidden from the main list until then
               turns?: uint,                      // the newest turn's number
               forked_from?: { thread, turn?, title },
               found?: bool,                      // brought in from a session on the PC
               readonly?: text,                   // "outside_folders" | "folder_missing"
               undo?: { until_ms: uint, files: bool }   // files: only the PC can undo it
AgentCatalog.folders[] += checkpoints: bool       // restore points are kept for it
AgentCatalog.agents[].can += "fork", "fork_at"    // a fork from the latest message; from any
agent.start, agent.send += key?: text             // 16 lowercase hex digits (below)
agent.send's answer     += duplicate?: bool
AgentItem   += notice?: text                      // kind "notice": "rewound" | "edit_from_here" |
                                                  // "files_restored" | "forked"

AgentTurn     = { turn, prompt_seq, last_seq?, started_ms, ended_ms?,
                  ended?: text,                   // "done" | "failed" | "stopped"
                  checkpoint?: text,              // "pending" | "ready" | "missing" | "none" | "expired"
                  why?: text,                     // with "missing": "busy" | "slow" | "large" | "refused" | "off" | "before"
                  files, adds, dels, more_files?, skipped,
                  can_fork?: bool, can_rewind?: bool }
AgentTurnFile = { path, status?: text,            // "added" | "modified" | "deleted" | "renamed" | "type" | "unrecorded"
                  from?, adds, dels, binary? }
AgentHit      = { thread, title, agent, folder, state, archived?, seq,   // seq 0: the title matched
                  kind?, when, snippet, marks?: [[start, length]], more_in_thread }
AgentFound    = { id, agent, title, folder, folder_id?, first_ms?, updated_ms, bytes,
                  packed?, source, open?, thread?, damaged? }
```

- **Shelves.** `agent.threads` without `show` lists the main shelf (neither
  archived nor snoozed), `"archived"` and `"snoozed"` the others; `before_ms`
  pages back by last change. `agent.mark` changes up to 50 threads at once and
  answers the threads it changed: an archive of a working or waiting thread is
  `BUSY` and left out, the others still apply. `snooze` is Unix ms in the
  future and at most 30 days ahead, `0` brings it back; a snoozed thread
  comes back by itself the moment it needs you, fails or is sent to, and
  sends nothing when it does. Sending to an archived thread unarchives it.
- **Turns.** A turn starts with a message you sent. `agent.turn` comes when
  one starts, ends, and when its restore point settles. `checkpoint` says
  whether the PC recorded the folder's files before and after it; `files`,
  `adds` and `dels` count what changed, `skipped` the large files left out
  (`status: "unrecorded"`: no diff, and no rewind touches them).
  `can_fork` and `can_rewind` are the PC's verdicts for that turn; a client
  doesn't work them out itself.
- **Diffs.** `agent.diff.files` lists what a turn (`turn`) or a range of
  turns (`from` to `to`) changed, at most 500 files (`more` past that; `adds`
  and `dels` count them all), each path relative to the thread's folder with
  `/` between components. `agent.diff.patch` sends one listed file's patch,
  at most 1 MiB (`truncated` past that); its parts each carry the next piece
  of `patch` and the receiver joins them. A path the diff doesn't list is
  `BAD_MESSAGE`; a turn whose restore point isn't `ready` is `UNSUPPORTED`.
- **Search** covers every message the PC holds (yours and the agent's
  answers, not tool output) and thread titles. A query of words of one or two
  letters searches titles only (`titles_only`: show "Type 3 letters or more
  to search messages."). Hits are newest first, at most 3 a thread a page
  (`more_in_thread`); `next` is the `cursor` for the next page. A `snippet`
  is one line of at most 240 characters; `marks` are the matches in it as
  start and length in UTF-16 units. Archived threads only with `archived`.
- **Fork** makes a new thread from a copy of the conversation up to `turn`
  (the latest without it), in the same folder; the files are as they are.
  It keeps the source's mode, from a phone too. The PC shows its user the same
  notice as for a thread a phone started.
- **Rewind** is two steps. `agent.rewind.preview` says how many turns and
  messages would leave the thread and hands out a `token` for 120 s; nothing
  changes yet. `agent.rewind.apply` with that token does it, once; `refill`
  puts the turn's message back as the thread's draft (edit from here). The
  thread's `undo` is then set: `agent.rewind.undo` brings the messages back
  until the next turn starts or 24 h pass. Every device gets `agent.cut` with
  the range of `seq`s that left (`restored` when an undo brought them back):
  drop them, or read again. **A phone rewinds the conversation only**: the PC
  answers `DENIED` to a preview with `files` or an apply with
  `include_later`, never sends a phone a list of files to restore, and a
  rewind that put files back (`undo.files`) can be undone on the PC only.
- **Sessions found on the PC.** `agent.found` lists every Claude and Codex
  session on the PC, including compressed ones, newest first; `query`
  matches title and folder, `show: "new"` leaves out those already brought
  in. While the PC is still looking it answers what it has with `scanning`;
  ask again. `partial`: it stopped at one of its bounds. `folder` is a label
  or the folder's name, never a path. `agent.found.preview` reads a session's
  newest messages without keeping anything. `agent.import` brings up to 50 in
  as threads, one job at a time (`BUSY`), reporting `agent.import.progress`
  at most 4 times a second, the last with `done + failed == total`. A thread
  brought in is read-only (`readonly`) until its folder is an agent folder;
  sending to it continues a copy of the session, never the original.
- **Held messages.** A phone may send `key` with `agent.start` and
  `agent.send`: 16 random lowercase hex digits. A second message with a key
  the thread already took in the last 24 h is answered `ok` with
  `duplicate: true` and nothing is sent, so a phone can resend after a lost
  answer.
- **Notices.** A rewind and a fork write an ordinary `notice` item ("Rewound
  to here", "Forked from here…"); its `notice` says which, so a receiver
  puts Undo on a `rewound` or `edit_from_here` one while the thread's `undo`
  is set. A receiver reads a `notice` it doesn't know as absent, and one on
  an item that isn't a notice is left out.
- Every mark, rename, fork, rewind and import a phone makes is shown on the
  PC with the phone's name, as its messages are.
- **Never to a phone**: a folder's path or any absolute path (diff paths are
  relative to the folder; a found session's `folder` is a label), the files a
  rewind would restore, the PC's store size, a found session's working
  directory.
- A receiver reads a `checkpoint`, `why`, `ended`, `status`, `blocked`,
  `readonly` or `kind` it doesn't know as absent, and a `source` as `other`.
  A diff path that isn't relative, or climbs out with `..`, is left out (never
  cut); a hit's mark that falls outside its snippet is left out.

Limits (bytes of UTF-8 unless noted):

| Field | Limit |
|---|---|
| `agent.turns` | `limit` 1 to 100 (default 30) |
| `agent.diff.files` answer | 500 files; `path`, `from` 1 to 512, relative |
| `agent.diff.patch` | `path` 1 to 512; the answer's `patch` 1 MiB |
| `agent.search` | `query` 1 to 200, one line; `limit` 1 to 50 (default 20); `cursor` 1 to 256 |
| hit | `snippet` 240 characters, one line; 8 `marks`; `title` 80; `folder` 64 |
| `agent.mark` | 1 to 50 distinct thread ids; at least one change; `snooze` at most 30 days ahead |
| `agent.rename` | `title` 1 to 80, one line |
| `token` | an id (1 to 40, `[a-z0-9]`) |
| `agent.found` | `query` 128, one line; `limit` 1 to 100 (default 50); `show` `"all"` or `"new"` |
| `agent.found.preview` | `limit` 1 to 50 (default 50) |
| `agent.import` | 1 to 50 distinct ids; `job` 16 lowercase hex digits |
| `key` | 16 lowercase hex digits |

Errors as §2.3, and: `UNSUPPORTED` for a diff of a turn without a ready
restore point, or a fork the agent can't make (only from the latest message,
say); `DENIED` for a rewind from a phone that names files, and for anything
of `agent/found*` and `agent/import` with `agent_sessions` off;
`BAD_MESSAGE` for a path the diff doesn't list, more than 50 ids, a snooze in
the past or beyond 30 days; `NOT_FOUND` for an unknown or used token, or
nothing to undo; `BUSY` while the thread works or waits (rewind, fork,
archive), and for a second import job.

## 7. Presence

Added in G7 Link 0.1.20. A PC tells its phones three facts about itself and
which of their threads are on its screen; a phone tells its PC which thread or
session it shows. A phone uses it for one thing: holding a notification for a
thread you're reading on the PC, and showing it the moment you stop. A PC uses
a phone's view not to notify about a thread you're reading on the phone, and to
mark it read. It's plain protocol v1: no capability. Everything follows the
`agents` toggle (§1); sessions also `agent_sessions` (§3). Nothing here travels
on Bluetooth, and nothing is retried or queued while offline.

```
t = "agent.presence"   b = { at_pc, locked, dnd, viewing?: [id], sessions?: [id], ttl_ms }   PC → phone, agent lane

RPC agent/viewing      t = "agent.viewing"    b = { thread?, session?, visible, pane? }       → ok
```

- **`agent.presence`.** `at_pc`: someone used the PC in the last 30 s and it
  isn't locked (an idle time the PC can't read counts as away). `locked`: its
  session is locked. `dnd`: it shows no notifications. `viewing` and
  `sessions` are the threads and sessions on the PC's screen, in G7 Link or the
  launcher, newest first; `sessions` is empty for a phone without
  `agent_sessions`. The PC sends it to each phone with the toggle on, on every
  change and at least every 20 s, with `ttl_ms` 60000; and once to a phone that
  connects or is let in, with what's left of that time. A phone trusts it for
  `ttl_ms` from when it arrives. Older, or with the link down, it says nothing:
  nobody is there, and the phone notifies. `at_pc` with `locked` reads as not
  at the PC.
- **`agent.viewing`.** A phone sends it when a thread or session screen comes
  to the front or leaves, when its screen turns on or off, and every 30 s while
  it shows. A visible view names exactly one thread or session; `pane` is
  `"thread"`, `"changes"`, `"plan"` or `"requests"`. `visible: false` ends the
  view it names, or with neither a thread nor a session, whatever the phone
  showed; one that names another than the view the PC has changes nothing. A phone
  sends them in order, one at a time, at most one every 2 s: a repeat inside
  that is dropped, a change waits for its turn and the newest wins,
  `visible: false` doesn't wait. The PC trusts a view for 60 s, drops it when
  the phone's session ends or a toggle goes off, and never asks its user about
  one.
- **Never to a phone**: an idle time, a window title, an app name, another
  device's view. A PC learns no more of a phone than the thread or session it
  shows and the pane.
- A receiver reads a `pane` it doesn't know as absent.

Limits:

| Field | Limit |
|---|---|
| `viewing`, `sessions` | 4 each, distinct ids |
| `ttl_ms` | 1000 to 120000 |
| `thread`, `session` | an id (1 to 40, `[a-z0-9]`); never both |
| `pane` | 1 to 32 |

Errors as §2.3: `DENIED` with `agents` off, and for a session's view with
`agent_sessions` off (neither prompts the PC's user); `BAD_MESSAGE` for a
visible view of nothing, a view of both, or one out of its limits. A phone
drops an `agent.presence` out of its limits.

## 8. Source control

Added in G7 Link 0.1.20. A phone sees a thread's folder as git sees it: its
branch, what changed and the uncommitted diff, the branch's pull request and
its checks, and the lists of open pull requests. It can commit files it read
whole, fetch, switch or create a branch while nothing is changed, ask the
thread's agent to write a commit message or a pull request, and, only with
the PC's `agent_push` toggle for it, push and open a pull request. Git and
the GitHub CLI run on the PC, as the PC's user, with the user's own hooks,
signing and sign-in. It's plain protocol v1: no capability. Everything
follows the `agents` toggle (§1). Nothing here travels on Bluetooth.

```
t = "agent.git"            b = { thread, branch?, detached?, changed, ahead?, behind?, pr?: AgentPrBrief }   PC → phone, agent lane
t = "agent.git.progress"   b = { job: AgentJob }                                     PC → phone, agent lane

RPC agent/git.status     t = "agent.git.status"     b = { thread }                               → ok AgentGit   paged
RPC agent/git.patch      t = "agent.git.patch"      b = { thread, path, review? }                → ok { patch, truncated, binary, names?, more_names? }   paged
RPC agent/git.review     t = "agent.git.review"     b = { thread, paths: [text] }                → ok { review, files: [{ path, readable, why? }] }   paged
RPC agent/git.commit     t = "agent.git.commit"     b = { thread, review, message }              → ok AgentJob
RPC agent/git.push       t = "agent.git.push"       b = { thread, remote? }                      → ok AgentJob
RPC agent/git.fetch      t = "agent.git.fetch"      b = { thread }                               → ok AgentJob
RPC agent/git.branches   t = "agent.git.branches"   b = { thread, query? }                       → ok { branches: [AgentBranch], more, current? }   paged
RPC agent/git.switch     t = "agent.git.switch"     b = { thread, branch, create?, track? }      → ok AgentJob
RPC agent/git.job        t = "agent.git.job"        b = { job, stop? }                           → ok AgentJob
RPC agent/pr.view        t = "agent.pr.view"        b = { thread, refresh? }                     → ok { pr?: AgentPrInfo, gh: AgentGhState, github?, why? }
RPC agent/pr.create      t = "agent.pr.create"      b = { thread, title, body, base?, draft? }   → ok AgentJob
RPC agent/pr.list        t = "agent.pr.list"        b = { show, folder?, refresh? }              → ok { prs: [AgentPrRow], more, gh: AgentGhState }   paged
RPC agent/write          t = "agent.write"          b = { thread, purpose, pr? }                 → ok AgentThread
```

The job event is `agent.git.progress`, and "ask the agent to write it" is
`agent/write`: `agent.git.job` is the RPC's request, and `agent.ask` is §2's
request for permission.

```
AgentThread += branch?: text,                   // absent: not a repository, or not read yet
               worktree?: { label, branch },    // it works in a worktree of its own; never its path
               changed?: uint,                  // files with changes, as last read
               wrote?: { purpose, turn, text, at_ms }   // what the agent wrote when asked (below)
AgentCatalog += push?: bool                     // this phone's `agent_push`
AgentCatalog.folders[] += follow_remote?: bool  // "Keep in step with the remote", set on the PC

AgentGit     = { repo: text,                    // "ok" | "none" | "refused" | "no_git"
                 branch?, detached?: text,      // detached: the commit's short id
                 unborn?: bool, upstream?, ahead?: uint, behind?: uint,   // as of the last fetch
                 remote?, remotes?: [{ name, label?, local? }],           // label: host and path, never a URL
                 default_branch?, on_default?: bool,
                 doing?: text,                  // "merge" | "rebase" | "cherry_pick" | "revert" | "bisect"
                 locked?: bool,                 // git is busy in the folder
                 files: [AgentGitFile], more?: bool, total, adds, dels, conflicts,
                 last?: { oid7, subject, ms }, worktree?: { label, branch },
                 fetched_ms?, checked_ms, github?: { repo, host }, fingerprint }
AgentGitFile = { path, from?, status?: text,    // "added" | "modified" | "deleted" | "renamed" | "type" | "untracked" | "conflict"
                 staged?: text,                 // "all" | "part" | "none": what the user staged already
                 folder?: bool, adds?, dels?, binary? }
AgentJob     = { id, thread, kind: text,        // "commit" | "push" | "fetch" | "switch" | "worktree" | "pr_create" | "pr_merge"
                 phase: text,                   // "running" | "done" | "failed" | "stopped"
                 started_ms, ended_ms?, mine?: bool, lines?: [text], from, dropped,
                 result?: { oid7?, subject?, files?, branch?, remote?, up_to_date?, created?, pr?: AgentPrInfo, number? },
                 error?: { reason, message, names?: [text] } }
AgentBranch  = { name, remote?, current?, upstream?, ahead?, behind?, worktree?, last_ms }
AgentPrInfo  = { number, title, state: text,    // "open" | "merged" | "closed"
                 draft?, url, base, head,
                 review?: text,                 // "approved" | "changes_requested" | "review_required"
                 mergeable?: text,              // "yes" | "conflicts" | "unknown"
                 blocked?, updated_ms, checks?: [{ name, state, url? }], pass, fail, pending, read_ms }
AgentPrBrief = { number, state, draft?, pass, fail, pending }
AgentGhState = { gh: text,                      // "ok" | "missing" | "signed_out" | "old"
                 version?, hosts?: [text], checked_ms }
AgentPrRow   = { number, title, repo, state, draft?, updated_ms, url, branch?, checks?: { pass, fail, pending }, thread? }
```

- **Status.** `agent.git.status` reads the thread's folder now: at most 500
  files to a phone (`more` past that; `total`, `adds` and `dels` count them
  all), each path relative to the folder; an untracked folder is one row
  ending in `/` (`folder`). `fingerprint` changes when anything listed
  changes. `agent.git` comes when the folder's branch, changes, ahead or
  behind, or pull request change: at most one a second per thread; read the
  status again for the rest. A phone keeps each thread's last `agent.git`
  (and a status's strip) for reading offline.
- **The uncommitted diff.** `agent.git.patch` sends one listed file's change
  against the last commit, at most 1 MiB (`truncated` past that), in pieces
  the receiver joins as §6's patch; an untracked folder has no patch but the
  first 50 `names` inside it (`more_names` past them). A path the status
  doesn't list is `BAD_MESSAGE`.
- **Review, then commit.** A phone commits only files it was given whole.
  `agent.git.review` pins 1 to 200 listed files as they are now and answers
  a `review` token of the PC's own: a file whose size passes 1 MiB is
  `readable: false, why: "large"`, and an untracked folder is always
  `readable: false, why: "folder"`: a phone gets its names, never its
  files' bytes, so it's committed on the PC. `agent.git.patch` with that
  `review` counts a file read once the last part of its patch was written to
  the phone, whole: never a cut patch. `agent.git.commit` with the review commits exactly its
  files, never paths the phone names, and only when every one is readable
  and was read; otherwise `DENIED`. The PC keeps 4 reviews a phone, each
  for 10 minutes, and a review commits once; a review it no longer keeps is
  `NOT_FOUND` (read the files again). If a file changed after it was read,
  the job fails with `changed`. Another phone can't use a phone's review.
- **Jobs.** A commit, push, fetch, switch or pull request opened answers at
  once with its job; `agent.git.progress` brings its new lines (to every
  phone with the toggle on) and the last one its final `phase`. A phone gets
  a job's newest 40 lines and 8 KiB (`from` numbers the first one, `dropped`
  counts those not kept), each line at most 512 characters, with control
  characters, URLs' user parts and the PC's home folder cleaned out.
  `agent.git.job` reads one again for 10 minutes after it ends, `stop` stops
  one the phone started (`mine`). A request the PC refuses before anything
  runs answers with a job already `failed`, its `error.reason` saying why;
  `agent.git.job` doesn't know that one.
- **Reasons** a phone words itself: `git_busy`, `doing`, `detached`,
  `conflicts`, `nothing`, `identity`, `thread_busy`, `shared_busy`,
  `changed`, `hook`, `failed`, `stopped`, `behind`, `remote_refused`,
  `sign_in`, `network`, `pick_remote` (with `names`), `no_remote`,
  `no_commits`, `dirty`, `would_overwrite`, `in_worktree`, `exists`,
  `bad_name`, `not_pushed`, `not_github`, `no_gh`, `signed_out`, `old_gh`,
  `pr_exists`. One it doesn't know reads as `failed`, with `message`.
- **Push and pull requests need `agent_push`**, a per-device toggle on the
  PC, **off by default**, that a phone can't turn on. The PC reads it for
  every `agent.git.push` and `agent.pr.create` and answers `DENIED` while
  it's off, naming it; it sends `agent.catalog` again when it changes, with
  `push`. A push sends the current branch to one remote, never forced;
  `remote` only after a `pick_remote`. `agent.pr.create` opens a pull
  request from the pushed branch into `base` (a remote branch
  `agent.git.branches` listed; absent: the repository's default).
- **Branches.** `agent.git.branches` lists local branches, then those only
  on a remote (`remote`, named `origin/feat`), each newest first, at most
  200; `query` matches part of the name. `agent.git.switch` switches to a
  listed branch, one only on a remote (`track`), or a new one (`create`).
  A phone switches only while nothing is changed: the job fails with
  `dirty` otherwise.
- **Ask the agent to write it.** `agent/write` sends the thread, as an
  ordinary message from the user, the PC's fixed request for a commit
  message (`purpose: "commit"`) or a pull request's title and description
  (`"pr"`); when that turn ends the thread's `wrote` holds the answer until
  the next commit or pull request, the next ask, or a day. `BUSY` unless the
  thread is idle with nothing queued. `purpose: "review"` with `pr` asks the
  agent to review a pull request instead (§13); `pr` comes with that
  purpose only.
- **Pull requests** are read through the GitHub CLI on the PC.
  `agent.pr.view` answers the branch's pull request, or `why` there's none:
  `"not_github"`, `"no_upstream"`, `"detached"`, `"not_repo"`; with `"network"`, `pr` is the
  last one read. A check is `pass`, `fail`, `pending`, `skipped` or
  `cancelled`; skipped and cancelled ones count in neither `pass`, `fail`
  nor `pending`. `agent.pr.list` lists at most 50 open ones: `"mine"`,
  waiting for `"review"`, or of one `"folder"` (with its id); a row's
  `thread` is a thread on the PC on that branch. A phone opens a `url` only
  when it's `https` on one of `gh.hosts`.
- **The PC only.** A merge, marking a pull request ready, a switch that takes
  changes along, creating or removing a worktree, and "keep in step with the
  remote" have no message on the wire. `agent.git.switch` with `carry` is
  `DENIED`. Each commit, push, pull request and switch a phone makes is
  shown on the PC with the phone's name.
- **Never to a phone**: a remote's URL (a `label` is host and path; a
  remote that is a folder on the PC has none, `local`), an absolute path, a
  worktree's path, an account name. A receiver leaves out a path that isn't
  relative or climbs out with `..`, a label that could be a URL or a path,
  and a URL that isn't `https`.
- A receiver reads a `status`, `staged`, `doing`, `why`, `review` or
  `mergeable` it doesn't know as absent, a check's `state` as `pending` and
  never `pass`, and drops a job whose `kind` or `phase` it doesn't know.

Limits (bytes of UTF-8 unless noted):

| Field | Limit |
|---|---|
| `agent.git.status` answer | 500 files; `path`, `from` 1 to 512, relative; 8 remotes |
| `agent.git.patch` | `path` 1 to 512; the answer's `patch` 1 MiB; 50 `names` |
| `agent.git.review` | 1 to 200 distinct paths |
| `review`, `job` | 16 lowercase hex digits |
| `message` | 1 to 8192, not blank, no NUL |
| `branch`, `base`, `upstream` | 1 to 255, no white space, no `:` or `\`, not starting with `-` or `/` |
| `remote` | 1 to 64, no white space, `:`, `@` or `\`, not starting with `-`, `/`, `.` or `~` |
| `query` | 100, one line |
| `title` | 1 to 256, one line |
| `body` | 32768, no NUL |
| job | 40 lines and 8 KiB to a phone, each line 512 characters; `reason` 24; `message` 200 |
| `agent.git.branches` answer | 200 branches |
| `agent.pr.list` answer | 50 rows; a pull request's checks 100 |
| `url` | `https`, 512 |

Errors as §2.3, and: `DENIED` for a push or a pull request with `agent_push`
off, a commit of a review not read whole here, and a switch with `carry`;
`BAD_MESSAGE` for a request out of its limits, and a path a status or a
review doesn't list; `NOT_FOUND` for an unknown thread or job, and a review
the PC no longer keeps; `BUSY` while git is busy in the folder or the thread
works.

## 9. Reply on the PC

Added in G7 Link 0.1.21. A phone types a reply into a session it follows
(§3), where the session runs: the PC brings the session's own window and its
tab forward, proves that what has the keyboard is that session, pastes the
text as one bracketed paste and presses Enter. When the terminal may have
more than one tab or window, the PC sets a one-time title on the session's
terminal, selects the tab that shows it, and restores the terminal's title
before it answers. The agent's answer arrives as the session's items, as
everything else does. Nothing is written to the agent's files and nothing
goes to a vendor's host. Each check that fails is a refusal with a reason the
phone can show; a reply is never a best guess.

The PC offers `agent.reply` only while it can do it now: it offers
`agent.sessions`, its window backend can write the PRIMARY selection, and its
input device can be made. While it can't, it may offer instead one of three
words that say why, so a phone can show "Can't reply to it from here" with a
reason rather than nothing: `agent.reply.needs.desktop` (G7 has no window
backend for this desktop), `agent.reply.needs.extension` (the desktop's
extension is missing or too old) or `agent.reply.needs.keyboard` (no virtual
keyboard can be made). A PC offers at most one of the four at a time; a phone
that sees more than one reads only the first in that order, and a phone sees
none from a PC before 0.1.29. Everything here is governed by a third per-device
toggle on the PC, `agent_reply`, **off by default**, which counts only while
`agents` and `agent_sessions` are on. While it is off the PC answers `DENIED`,
and the first such request makes the PC show its user a one-time prompt for
that phone, as §1's and §3's do. Nothing here travels on Bluetooth.

```
RPC agent/session.reply   t = "agent.session.reply"
    b = { session: text, text?: text, check?: bool, confirm?: text }
    → ok { result: text, reason?: text, target?: AgentReplyTarget, nonce?: text, retry_ms?: uint }

AgentReplyTarget = { app: text, agent: text, folder: text, kind: text }   // kind: "terminal" | "app"
```

This isn't `agent/reply`, §5's answer to an agent's question.

- **A check** (`check: true`, no `text`) enters nothing. The PC runs its
  checks without bringing anything forward, setting a title or reading the
  screen, and answers `ready` or `refused`, never `confirm`: a check that
  lacks only the confirmation is `ready`. It is never refused `in_use`,
  `tab` or `unproven`: only a reply finds those out (a PC before 0.1.27
  may still answer a check `in_use` or `tab`). It may answer `ambiguous`
  when the session's process itself can't be told apart.
  A phone checks when the session's screen opens, when the session's state
  changes and when the reply box gains focus, and shows a box only while the
  last check was `ready`.
- **A reply** (`text`) answers `sent`, `refused` or `failed`. `sent`: Enter was
  pressed in the proven window; it doesn't say the agent read it, which the
  next `agent.session.item` does. `refused`: nothing was typed. `failed`:
  `focus` (the window didn't come forward; nothing was typed),
  `interrupted` (the text was pasted and **not** sent; a phone never retries
  it, which would paste it twice) or `input` (a key couldn't be pressed; the
  paste may have landed, so a phone shows it as `interrupted`).
- **The focus.** After a reply or a move, the PC gives the focus back to the
  window that had it before, when nobody touched the PC meanwhile, the PC
  isn't locked, the terminal still has the focus and that window still
  exists. After `interrupted` and `input` (and a move's `draft` and `open`)
  it leaves the terminal in front: the PC's user must see what's in it.
  Nothing of this is on the wire.
- **Refusal reasons:** `locked`, `in_use` (someone used the PC in the last
  5 s, or a window is full screen; with `retry_ms` a phone may send the same
  reply again after that long, for up to a minute, while its user sees it
  waiting; without it, waiting won't help), `confirm`, `needs_you` (the
  agent waits for an answer on the PC), `busy` (Codex is mid-turn), `ended`,
  `no_process`, `no_window`, `ambiguous` (it can't be told which window shows
  the session), `tab` (it's in a background tab the PC couldn't bring
  forward), `unproven` (the PC couldn't make sure which terminal is the
  session's), `place` (it runs where G7 doesn't type: the agent's app,
  VS Code, ssh, a terminal G7 doesn't know), `clipboard`, `text` (it starts
  with `!` or `/`, which the agent reads as a command), `rate`, `setup` (the
  PC needs an update to its desktop's extension).
- **Confirming.** The first reply from a phone to a session is refused with
  `confirm`, the `target` and a `nonce`; the phone shows where it will type
  and, when its user says Send, sends the same text again with `confirm` set
  to the nonce. A nonce works once, for 2 minutes, for that phone and
  session. The PC remembers the confirmation in memory, bound to the phone,
  the session and the process it runs in, not the window: a restarted agent
  or terminal, a restarted G7 Link or 24 h asks again; the session's tab
  moved to another window of the same terminal doesn't.
- **`target`** is on every `ready`, `sent` and `confirm` answer: the app's
  name from its desktop entry (never a window title), the agent's name and
  the folder's name (never a path). A phone shows "Terminal · Claude ·
  g7linux".
- **Rate.** One reply is entered at a time on a PC (another one meanwhile is
  `BUSY`). Each phone gets at most 1 reply entered into a session every 3 s,
  6 a minute and 60 an hour, and 2 checks a second; refusals that typed
  nothing don't count, but at most 1 attempt a second is looked at.
  `retry_ms` comes with `rate` and `in_use` when the PC knows it.
- **The text.** The PC makes line ends one `\n`, a tab four spaces and the
  text NFC, and trims trailing white space and new lines, before anything
  else. It never logs the text, writes it to disk or puts it in an error, a
  notification or a crash report.
- **Sending again.** A phone never holds a reply while offline. A `refused`
  answer is definite (nothing was typed), so the same reply may be sent
  again. An answer that never came may not: it is shown as "Not known if it
  was sent" until the session's next item settles it.
- A receiver reads a `result` it doesn't know as `failed`, a `reason` it
  doesn't know as a plain "Couldn't reply", and a `kind` it doesn't know with
  a plain icon.

Limits (bytes of UTF-8 unless noted):

| Field | Limit |
|---|---|
| `session` | an id (1 to 40, `[a-z0-9]`) |
| `text` | 1 to 8192, not blank; no control character but newline, carriage return and tab; no bidirectional formatting control |
| `check`, `text` | exactly one of the two |
| `confirm`, `nonce` | 32 lowercase hex digits; `confirm` only with `text` |
| `result`, `reason` | 1 to 32, `[a-z_]` |
| `app`, `agent`, `folder` | 64; `folder` never contains `/` or `\` |
| `kind` | 1 to 32, `[a-z_]` |
| `retry_ms` | at most 60000 |

Errors as §2.3: `UNSUPPORTED` while the PC doesn't offer `agent.reply`;
`DENIED` while a toggle is off; `NOT_FOUND` for a session the PC doesn't list;
`BAD_MESSAGE` for a request out of its limits; `BUSY` while another reply is
being entered on the PC; `INTERNAL` when the PC's answer didn't come in time
(a phone treats it as not known whether it was typed).

### 9.1 Moving a session

Added in G7 Link 0.1.29. "Move to G7" ends a session where it runs and
continues it as a thread started by the phone, in one tap: the PC ends the
session's agent where it runs (typing its own exit command into the proven
terminal, or ending its proven process) and, only once nothing has the
session open, makes the thread exactly as `agent.session.continue` does (§3),
whose checks on the PC stay the judge. A phone offers it for a session whose
`source` is `terminal`, whose `state` is `idle` and which is `blocked: open`.

The PC offers `agent.move` only beside `agent.reply`, while its agent host can
say, before anything is typed, what would still stop the continue. §9's three
toggles govern it, with §9's one-time prompt; it shares §9's confirmation,
rate and one-at-a-time lock. Nothing here travels on Bluetooth, and a phone
never holds a move while offline or sends it again by itself.

```
RPC agent/session.move   t = "agent.session.move"
    b = { session: text, confirm?: text }
    → ok { result: text, reason?: text, target?: AgentReplyTarget, nonce?: text,
           retry_ms?: uint, thread?: AgentThread }
```

- **`result`:** `moved` (with `thread`: the agent closed and the session is
  that thread now; it left the PC's list, and the phone opens the thread),
  `closed` (the agent closed, and the PC won't continue it yet), `refused`
  (nothing was typed) or `failed`.
- **`closed`.** The agent exited but the PC still sees the session open (a
  Codex session within 90 s of its last write), or the continue was refused.
  The session is `ended` in the PC's list; once the PC's list says
  `can_continue`, the phone sends `agent.session.continue` by itself, once,
  while that session's screen is open, for up to 2 minutes. After that the
  session offers Continue here as any ended session does. Nothing is lost
  either way.
- **Refusal reasons** (nothing was typed): §9's `locked`, `in_use`,
  `confirm`, `needs_you`, `busy` (the agent is mid-turn, Claude's included),
  `ended`, `no_process`, `no_window`, `ambiguous`, `tab`, `unproven`,
  `place`, `rate` and `setup`, and `folder` (the session's folder isn't one of
  the PC's agent folders) and `agent` (the agent can't run on the PC now),
  both found before anything is typed.
- **Failure reasons:** `focus` (nothing was typed), `interrupted` and `input`
  (as a reply's: the exit command may sit in the agent's box, not entered),
  `draft` (the exit command was pasted and not entered: the agent's box held
  something else, or couldn't be read) and `open` (it was entered and the
  agent didn't close in time). The PC never tries a second time to end it.
- **Confirming** is §9's, and shared: a phone that confirmed replies into
  this session where it runs now isn't asked again; a first move is refused
  with `confirm`, the `target` and a `nonce`, and sent again with `confirm`
  set to it. `retry_ms` comes with `rate` and `in_use` as in §9.
- **The thread** is in the form the asking phone reads (§5.7 without
  `agent.control`), with `from_phone` set.
- **The focus** goes back as §9 says, once the agent closed, when a window
  was brought forward at all.
- A receiver reads a `result` it doesn't know as `failed` and a `reason` it
  doesn't know as a plain "Couldn't move it". A `moved` without a thread the
  phone can read is shown as not known whether it was moved.

Limits as §9's: `session` an id; `confirm`, `nonce` 32 lowercase hex digits;
`result`, `reason` 1 to 32 `[a-z_]`; `target` as §9's; `retry_ms` at most
60000; `thread` as §2.2's.

Errors as §9's: `UNSUPPORTED` while the PC doesn't offer `agent.move`;
`DENIED` while a toggle is off; `NOT_FOUND` for a session the PC doesn't list;
`BAD_MESSAGE` for a request out of its limits; `BUSY` while a reply or a move
is being entered on the PC; `INTERNAL` when the PC's answer didn't come in
time (a phone treats it as not known whether it was moved).

## 10. Hand-off

Added in G7 Link 0.1.22. A PC hands a thread to a phone ("Continue on
<phone>"), or offers it when the PC locks with the thread on screen; a phone
hands a thread to its PC ("Continue on <PC>"). Either side only **opens** the
thread where it was handed, at the item that was first on the sender's screen:
nothing is sent, typed, answered or unlocked. The sender saves its draft first
(`agent.draft.set`, §5), and the receiver reads it as always. A question's
typed answer is not a draft and doesn't travel. It's plain protocol v1: no
capability, governed by the `agents` toggle (§1). Nothing here travels on
Bluetooth, and nothing is retried or queued while offline.

```
t = "agent.handoff"         b = { thread, seq?: uint, offer: bool, ttl_ms: uint }   PC → phone, agent lane

RPC agent/handoff.open      t = "agent.handoff.open"   b = { thread, seq?: uint }
    → ok { result: text }                                  // "opened" | "locked" | "away"
```

- **`agent.handoff`.** Open `thread` here, at item `seq` when it's given. A
  phone with G7 Link in front opens it; otherwise it posts "Continue on this
  phone", which does the same when tapped. With `offer` the PC was locked
  (not merely idle) while the thread was on its screen in the last 60 s and
  working, waiting or with a draft: the phone shows it quietly, and takes it
  away when `agent.presence` says someone is at the PC again or it lapses.
  A PC sends an explicit one to the phone named, an offer to each phone with
  the toggle on; it holds for `ttl_ms` (600000, 10 minutes) from when it
  arrives. A phone drops one out of its limits.
- **`agent/handoff.open`.** The PC opens the thread on its screen at `seq`,
  as its own "open this thread" does, and says it came from the phone.
  `opened`: it's on screen, and someone is at the PC. `locked`: the PC is
  locked; it keeps this one hand-off (the newest) for 10 minutes and opens it
  when it's unlocked, once. `away`: it's open there, but nobody has used the
  PC in the last 30 s. A phone reads a `result` it doesn't know as `away`.
- Neither message carries a draft, a path or anything typed.

Limits:

| Field | Limit |
|---|---|
| `thread` | an id (1 to 40, `[a-z0-9]`) |
| `seq` | any `uint` |
| `ttl_ms` | 1000 to 600000 |
| `result` | 1 to 32, `[a-z0-9_]` |

Errors as §2.3: `UNSUPPORTED` without the agent host; `DENIED` with `agents`
off (the first time, the PC asks its user, as for any request of this
service); `NOT_FOUND` for a thread the PC doesn't have; `BAD_MESSAGE` for a
request out of its limits.

## 11. The phone beside the PC

Added in G7 Link 0.1.23. Two things for a phone used next to its PC. A phone
that shows a thread the PC also shows can show that thread's changes, plan or
requests beside the PC's screen, and the PC can switch which; and a thread
lists the TCP ports its agent's processes listen on, so a phone can open a dev
server that already answers on the network. It's plain protocol v1: no
capability, governed by the `agents` toggle (§1). Nothing here opens,
forwards or tunnels a port, nothing is probed or connected to, and nothing
travels on Bluetooth or is retried or queued while offline.

```
t = "agent.beside"     b = { thread, pane }                                  PC → phone, agent lane

AgentThread += ports?: [{ port: uint, reach: text }]                         // "pc" | "lan"
```

### 11.1 Second screen

- A phone says what it shows with `agent.viewing` and its `pane` (§7):
  `"thread"`, or beside the PC `"changes"`, `"plan"` or `"requests"`.
- **`agent.beside`.** The PC asks the phone to show `pane` beside `thread`: a
  phone acts on it only while it shows that thread beside the PC, and then
  reports the new pane with `agent.viewing`; otherwise it ignores it.
  `"thread"` asks for the thread itself. A PC sends only a pane this
  version knows, and only to a phone with the toggle on; a phone drops one
  out of its limits or with a pane it doesn't know.
- On the phone, requests and questions beside the PC are answered as in the
  thread (the first waiting one, with the same guards); nothing else there
  changes anything on the PC.

### 11.2 Dev servers

- **`ports`.** While a thread's agent runs, the PC lists the TCP ports in
  `LISTEN` held by the agent's process and the processes it started, at
  most 4, oldest first, none under 1024. `reach` is `"pc"` when every socket
  on that port is bound to loopback, so it answers on the PC alone; `"lan"`
  when one is bound to every address or to a network address. A port goes
  when its socket does, and all go when the agent's process ends. The PC
  finds them by reading its own process table; it never connects to one.
- A phone offers to open only a `"lan"` port, and only while its link to the
  PC is a direct path on the local network or the tailnet (not the relay,
  not USB):
  `http://<the PC's address on that link>:<port>/`, in the phone's own
  browser, after the same confirmation as an opened link. For a `"pc"` port
  it says how to make the server answer on the network instead. A phone
  leaves out a port whose `reach` it doesn't know.

Limits:

| Field | Limit |
|---|---|
| `thread` | an id (1 to 40, `[a-z0-9]`) |
| `pane` | 1 to 32, `[a-z0-9_]` |
| `ports` | 4, distinct ports |
| `port` | 1024 to 65535 |
| `reach` | 1 to 32, `[a-z0-9_]` |

There are no requests here, so no errors of their own.

## 12. Project scripts

Added in G7 Link 0.1.25. A folder's scripts are commands the PC's user typed
for it in G7 Link on the PC (`dev` / `npm run dev`); G7 never reads one from
a file in the folder. A script runs from a thread, in the thread's folder
(its worktree, for a thread in one), through the PC user's shell, as that
user, until it ends or is stopped; it isn't an agent's turn and the agent
doesn't see it. A phone sees every run of a thread it can read, with the
last lines. It can start and stop a folder's scripts only while that
folder's **"Phones can run these"** switch is on, which is set on the PC and
**off by default**; the PC's agent host checks it on every request. It's
plain protocol v1: no capability, governed by the `agents` toggle (§1).
Nothing here travels on Bluetooth.

```
t = "agent.script"          b = { run: AgentScriptRun }                    PC → phone, agent lane

RPC agent/script.run     t = "agent.script.run"     b = { thread, script }   → ok AgentScriptRun
RPC agent/script.stop    t = "agent.script.stop"    b = { run }              → ok AgentScriptRun
RPC agent/script.runs    t = "agent.script.runs"    b = { thread }           → ok { runs: [AgentScriptRun] }
RPC agent/script.output  t = "agent.script.output"  b = { run }              → ok AgentScriptRun   paged

AgentCatalog.folders[] += phone_scripts?: bool,   // "Phones can run these", set on the PC
                          script_count?: uint,    // how many scripts the folder has
                          scripts?: [AgentScript] // only with phone_scripts
AgentThread += runs?: [{ id, name, phase, exit? }]  // running first, then ended ones still kept

AgentScript    = { id, name, command }
AgentScriptRun = { id, thread, script, name,
                   phase: text,                   // "running" | "done" | "failed" | "stopped"
                   exit?: int, started_ms, ended_ms?,
                   by?: text,                     // the phone that started it, by its name; absent: the PC
                   mine?: bool,                   // the phone this goes to started it
                   lines?: [text], from, dropped,
                   reason?: text }                // "spawn" | "signal": why a failed run failed
```

- **Scripts.** A catalog folder says how many scripts it has
  (`script_count`) and whether phones may run them (`phone_scripts`). Only
  then does it list them, commands included, so a phone can show the line it
  would run. With the switch off a phone gets no script and no command, and
  says to turn the switch on in G7 Link on the PC. The PC sends
  `agent.catalog` again when a folder's scripts or switch change. A phone
  can't add, edit or remove a script, or turn the switch on: no message
  says so.
- **Running one.** `agent.script.run` runs one of the thread's folder's
  scripts, by its id, as this phone's request. A phone asks its user first,
  each time, showing the command; the PC shows its user a notice naming the
  phone, the script and the folder. One run of a script per place at a time
  (`BUSY`, `script_running`), four running in a thread and eight on the PC
  (`BUSY`, `runs`). A run that couldn't start answers `failed` with
  `reason: "spawn"`. A thread that can only be read is `DENIED`.
- **Stopping.** `agent.script.stop` stops a run this phone started, or any
  run of a folder whose switch is on; otherwise `DENIED`. A run also stops
  when the thread is deleted, the folder removed or the agent host stops;
  nothing restarts it, and nothing starts one by itself.
- **Output.** `agent.script` brings a run's new lines (to every phone with
  the toggle on), at most the newest 40 and 8 KiB, and the last one its
  final `phase`. `agent.script.output` answers a run with its newest 200
  lines and 32 KiB. `from` numbers the first line sent, counting from the
  run's first; `dropped` counts lines not kept or left out to fit. Lines are
  what the script printed, cleaned on the PC as a job's are (§8): control
  characters, URLs' user parts and the PC's home folder taken out, each at
  most 512 characters. A phone shows them as plain text, never as Markdown
  or links, and keeps nothing of a run.
- **`agent.script.runs`** lists a thread's runs, newest first, at most 12,
  without lines. The PC keeps an ended run for 30 minutes; a run it no
  longer keeps is `NOT_FOUND`.
- **Never to a phone**: a run's command, a folder's path, where a run runs,
  or a script's command while the folder's switch is off. A receiver drops
  a run with a `phase` it doesn't know, and reads a `reason` it doesn't
  know as absent.

Limits (bytes of UTF-8 unless noted):

| Field | Limit |
|---|---|
| `scripts` | 12 a folder; `script_count` 0 to 12 |
| `AgentScript.id`, `script` | `s` and 8 lowercase hex digits |
| `name` | 1 to 40, one line |
| `command` | 1 to 1024, one line (no control character but tab, no bidi control) |
| `run`, `AgentScriptRun.id` | 16 lowercase hex digits |
| `runs` | 8 in a thread; 12 in an `agent.script.runs` answer |
| `by` | 64, one line |
| `lines` | 40 and 8 KiB in `agent.script`; 200 and 32 KiB in an answer; each 512 characters |

Errors as §2.3, and: `DENIED` while the folder's switch is off (message
`phone_scripts_off`), for a thread that can only be read, and for a stop of
another device's run of a folder whose switch is off; `BUSY` with
`script_running` or `runs`; `NOT_FOUND` for an unknown thread or run, and a
script no longer in the folder's list (`no_script`); `BAD_MESSAGE` for a
request out of its limits, and a folder missing on the PC
(`folder_missing`).

## 13. Pull request detail

Added in G7 Link 0.1.25. A phone reads a pull request's description,
changed files and their diff, checks and conversation, and can ask the
thread's agent to review it. It's **read-only**: no message comments on,
reviews, approves, edits, closes or merges a pull request, and G7 posts
nothing to GitHub. It's plain protocol v1: no capability, governed by the
`agents` toggle (§1). Nothing here travels on Bluetooth.

```
RPC agent/pr.detail   t = "agent.pr.detail"   b = { thread?, repo?, number?, refresh? }   → ok AgentPrDetail   paged
RPC agent/pr.diff     t = "agent.pr.diff"     b = { repo, number, path }                  → ok { patch, truncated, binary }   paged
RPC agent/write       t = "agent.write"       b = { thread, purpose: "review", pr }      → ok AgentThread

AgentPrDetail  = { pr?: AgentPrInfo, gh: AgentGhState, why?: text,   // as §8's, and "too_large" | "no_pr"
                   repo, author, body, body_cut?: bool, adds, dels, changed,
                   files: [{ path, change, adds, dels }], files_more?: bool,
                   comments: [AgentPrComment], comments_older,
                   thread?, read_ms }
AgentPrComment = { author, at_ms, body, cut?: bool,
                   review?: text,                // "approved" | "changes_requested" | "commented" | "dismissed"
                   url? }
```

- **`agent.pr.detail`** answers the thread's branch's pull request
  (`thread`), or one `agent.pr.list` listed in the last 10 minutes (`repo`
  and `number`, both or neither; otherwise `BAD_MESSAGE`). `why` as
  `agent.pr.view`, `"no_pr"` when the branch has none, and `"too_large"`;
  with `"network"` the detail is the last one read, if any. `body` is
  Markdown, at most 32 KiB (`body_cut`); files at most 300 (`files_more`),
  each relative to the repository, its `change` one of `added`, `deleted`,
  `renamed`, `copied`, `modified` or `changed` (a word a receiver doesn't
  know reads as `changed`); the conversation's newest 50 comments and
  reviews' summaries, oldest first, each at most 4,096 bytes (`cut`),
  `comments_older` counting the rest. A review's summary carries its
  `review`. Comments made on lines of code aren't among them: a phone says
  so and links to GitHub. `thread` is a thread on the PC on that repository
  and branch.
- **`agent.pr.diff`** sends one file the detail listed, at most 1 MiB
  (`truncated` past that), in pieces the receiver joins as §8's patch. A
  path the detail doesn't list is `BAD_MESSAGE`.
- **Ask to review it.** `agent/write` with `purpose: "review"` and the pull
  request's `pr` sends the thread, as an ordinary message from the user, the
  PC's fixed request to review that pull request, naming it by number and a
  link the PC builds, never by its title or text. The thread's own mode
  applies: the agent's reads of the pull request ask as any tool would.
  `BUSY` unless the thread is idle with nothing queued; `BAD_MESSAGE` when
  the thread's folder isn't on that repository or the pull request wasn't
  listed. Nothing comes back in `wrote`. No message starts a thread for a
  review from a phone.
- A receiver renders `body` and comments as Markdown under its own bounds:
  raw HTML as text, no remote image, a link opened only after asking. It
  opens a `url` only when it's `https` on one of `gh.hosts`.
- A receiver reads a `why`, `change` or `review` it doesn't know as above,
  and leaves out a file whose path isn't relative or climbs out with `..`.

Limits (bytes of UTF-8 unless noted):

| Field | Limit |
|---|---|
| `repo` | `owner/name` or `host/owner/name`, each part 1 to 100 of `[A-Za-z0-9._-]`, not starting with `-` or `.`; 140 in all |
| `number`, `pr` | 1 or more |
| `path` | 1 to 512, relative |
| `body` | 32768 |
| `files` | 300 |
| `comments` | 50 to a phone; `body` 4096; `author` 64 |
| `agent.pr.diff` answer | `patch` 1 MiB |

Errors as §2.3, and: `BAD_MESSAGE` for a request out of its limits, a
pull request not listed, and a path the detail doesn't list; `BUSY` for a
review while the thread works.

## 14. Implementation notes

- G7 Link desktop talks to the agent host (`g7-agents`) on a local socket and
  relays its threads; the folder's path never leaves the PC. The PC's
  notifications about agents come from the host, not from G7 Link.
- The core answers `agent.watch` itself and limits deltas per phone; the
  other requests go to the platform (`Platform::agent_request`) on a blocking
  thread.
- On a phone the core refuses to send a `message` to a PC that doesn't offer
  `agent.say_why`.
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
- The core answers `agent/viewing` itself and keeps each phone's view
  (`Node::agent_views`); G7 Link desktop puts them in the host's `ui.presence`
  as `remote`, and passes the host's `ui.presence` event on to the phones as
  `agent.presence` (`Node::agent_presence`). On the phone the FFI's
  `agent_presence` is null once a presence lapses or the link drops; no event
  says so.
- On a PC the platform hands a thread over with `Node::agent_handoff` (one
  phone, or every phone that gets agent pushes for an offer); the core
  answers `agent/handoff.open` after checking the toggle and the limits, and
  asks the platform (`AgentRequest::Handoff`), which opens G7 Link's Agents
  page and remembers one hand-off while the PC is locked. On the phone the
  core turns `agent.handoff` into an event with its expiry on the phone's
  clock (`expires_ms`); the PC's clock never matters.
- The agent host lists a thread's ports in `ThreadInfo.ports` (never
  stored): while the agent's process runs it reads `/proc` every 5 s for a
  minute after a command starts, else every 30 s, looking at at most 64 of
  this user's processes in the agent's tree (256 descriptors each, each
  process's start time read with its pid and again after) and at
  `net/tcp` and `net/tcp6` (1 MiB each); never a process's environment or
  command line. A server in another network namespace (a container) isn't
  seen. G7 Link desktop passes the ports on in `AgentThread`. The host's
  `ui.beside` (from G7 Link only, for a phone that shows that thread) comes
  back to G7 Link as an event, which sends `agent.beside`
  (`Node::agent_beside`). On the phone the core drops an `agent.beside` with
  a pane it doesn't know, and `Node::agent_port_url` gives the address in
  use on a LAN path, IPv4 first, never loopback or a link-local IPv6 one.
  On the PC, `Node::agent_port_url_for_phone` gives this PC's own address on
  its LAN path to that phone by the same rules, for G7 Link desktop's "Open
  on <phone>" (`link.open`).
- The core answers §5's requests after checking `agent.control` on both
  sides, the toggle, the limits, the mode and secret questions (it remembers
  the last 64 secret tokens it relayed), and makes every thread, item, page
  and catalog fit the phone it goes to (`for_phone`, `for_legacy`). It runs
  the image streams itself, one upload per phone, and hands the platform the
  whole bytes only when exactly as many came as announced
  (`AgentRequest::ImageAdd`); G7 Link desktop passes them to the host's
  `image.begin`/`image.chunk`/`image.end`. The G7 Link FFI announces
  `agent.control` for the app.
- The core answers §8's requests after checking the toggles and the
  limits, and keeps each phone's reviews itself: it hands the platform a
  commit's paths from the review, never the phone's, with the host's review
  id (`AgentRequest::GitCommit`), and lets a review commit once. It follows
  the host's pages of a patch up to 1 MiB, counts a file read only when all
  of it was written to the phone, and makes every status, job and pull
  request fit a phone. A refusal from the host before anything ran comes
  back as `AgentReply::Refused`; the core answers a request that starts a job
  with a job that already failed. G7 Link desktop passes the host's
  `git.changed` and `git.progress` on as `agent.git` and `agent.git.progress`
  (`Node::agent_git_changed`, `Node::agent_git_progress`).
- The core answers §9's request after checking that the PC offers
  `agent.reply`, the three toggles (with their prompts:
  `NodeEvent::AgentReplyRequested` for the third) and the limits, then asks
  the platform (`AgentRequest::SessionReply`, answered with
  `AgentReply::Replied`). G7 Link desktop does the rest: the text's rules,
  the rate, the one-at-a-time lock, the confirmation, finding and proving
  the window, and the keys. The core passes an answer on only when it's a
  whole answer to that request (a check is answered `ready` or `refused`; a
  `confirm` carries its target and nonce), made to fit; anything else is
  `INTERNAL`. On the phone, an error after the request went out without a
  usable answer, the PC's `INTERNAL` included, reaches the app as
  `G7LinkError.Timeout`: not known whether it was typed. The FFI's
  `agent_session_reply_check` and `agent_session_reply` clean the text
  first (line ends made `\n`, other control characters dropped).
- The core answers §9.1's request after the same checks, with `agent.move`
  in place of `agent.reply`, then asks the platform
  (`AgentRequest::SessionMove`, answered with `AgentReply::Moved`). It passes
  an answer on only when it's whole (`moved` with its thread and only then,
  a `confirm` with its target and nonce), made to fit, the thread in the
  asking phone's form; anything else is `INTERNAL`. On the phone,
  `Node::agent_session_move` sends it once, and errors as §9's;
  `Node::agent_move_offered` and `Node::agent_reply_needs` read what the PC
  offered when it last connected.
- The core answers §12's requests after checking the toggle and the limits
  (an id that isn't its shape, or an op it doesn't know, never reaches the
  platform), then asks the platform (`AgentRequest::ScriptRun`,
  `ScriptStop`, `ScriptRuns`, `ScriptOutput`), always as that phone's
  request. The folder's switch is the agent host's to check, on every
  request; G7 Link desktop passes the phone on as the request's origin. A
  host's refusal comes back as `AgentReply::Refused` with its reason word,
  which becomes the error's message. The core cuts a run's lines to fit
  and sets `mine`; G7 Link desktop passes the host's `script.progress` on
  as `agent.script` (`Node::agent_script_progress`). On the phone the FFI's
  errors are as for §8: `Denied` for `phone_scripts_off` (and the toggle),
  `NotFound`, and `Internal` with a reason starting `BUSY` (`script_running`
  or `runs` follows).
- The core answers §13's requests after checking the toggle and the
  limits, follows the host's pages of a diff up to 1 MiB as for §8's
  patch, and makes a detail fit a phone (50 comments, cleaned text,
  relative paths). `agent/write` with `review` reaches the platform as
  `AgentRequest::AskReview`; the host checks the pull request was listed
  and builds the message itself.
- G7 Link desktop's second prompt, for `agent_sessions`: title "Let %1 see
  sessions started on this PC?"; body "It will be able to list and read the
  Claude and Codex sessions you start here in a terminal or the Claude app, in
  any folder, including what you and the agent wrote. It can't type into them.
  Once one ends, it can continue it as a thread in your agent folders. It sees
  each folder's name, never its path."; buttons Not now, Allow.

## 15. New thread in any folder

Added in G7 Link 0.1.30. A phone starts a thread in any folder on the PC,
not only the ones already added: it browses the PC's folders by name and
adds one as an agent folder in the same flow. A PC offers it as
`agent.places`; the `agents` toggle governs it (§1), and adding a folder also
the PC's per-phone `agent_folders` toggle (off). Nothing here travels on
Bluetooth.

```
RPC agent/places       t = "agent.places"        b = { place, offset? }   → ok { place, entries: [{ name, repo?, folder? }], more? }
RPC agent/folder.add   t = "agent.folder.add"    b = { place }            → ok { folder: AgentCatalogFolder }
t = "agent.folder.prompt"   b = { place }        // PC → phone, `agent` lane, while someone at the PC is asked
```

- **Places.** A place is a root's name and `/`-separated folder names below
  it: `"Home/code/orbit-api"`, `"Local Disk/code"`. The roots are fixed: `Home`
  (the PC user's home folder), then each disk mounted under
  `/run/media/<user>` or `/media/<user>` by its folder's name (a name already
  taken gets " (2)"). An empty `place` lists the roots. A phone receives
  folder **names** under those roots and nothing else: never an absolute
  path, a hidden folder, a link or a file. A name is 1 to 255 bytes, not
  starting with `.`, without `/`, `\`, control or direction characters; a
  place has at most 16 names below its root and 1024 bytes in all; anything
  else is refused before the PC reads a disk.
- **Listing.** Folders only, sorted by name without regard to case, at most
  100 per answer from `offset` (at most 4000); `more` when there are more:
  ask again with `offset` = the entries had so far. `repo`: the folder holds
  a git repository of its own. `folder`: it is already one of the PC's agent
  folders, with that id; using it adds nothing. A disk slow to answer gets
  `BUSY` rather than holding the request.
- **Adding.** `agent.folder.add` makes the folder at `place` an agent folder
  and answers it as the catalog lists it; the next catalog has it too. Never
  a root itself, never inside or holding one of G7's own directories or the
  agents' configuration (`DENIED`). A folder that already is one answers
  that folder. The PC posts a notification naming the phone and the folder.
- **Asking the PC.** While the phone's `agent_folders` toggle is off, the PC
  first asks its user ("Let Pixel start agents in barber?", with "Don't ask
  again for this phone", which turns the toggle on) and sends
  `agent.folder.prompt` so the phone can say "Approve on <PC>". The PC waits
  up to 150 seconds; no answer, or "Not now", is `DENIED` with `msg`
  `declined`. Phones allow 160 seconds for `agent.folder.add`. A prompt may
  arrive after the answer: a phone ignores one for an add it no longer waits
  on. The catalog's `add_folders` tells the phone whether its adds go
  through without asking.
- **Catalog.** Each `AgentCatalogFolder` also carries `used` (when a thread
  there was last started or moved, else when it was added, Unix ms), `repo`,
  and `last`: what the folder's newest thread ran with and was about
  (`{ agent, model?, effort?, title, branch? }`). A phone lists recent folders
  by `used` and offers `last` as the first choice of agent, model and effort.
  A PC lists its 64 most recently used folders. Each `AgentModel` may carry
  `exact`: the exact model an alias stands for ("Opus 5"), when the agent's
  CLI says; a phone shows it in place of `name` and never guesses a version.
- **What a folder exposes.** Running an agent in a folder runs that folder's
  own agent settings (hooks, MCP servers) and reads its instructions
  (`CLAUDE.md`, `AGENTS.md`) from the first turn, before anything is asked;
  that is why the PC asks the first time.
