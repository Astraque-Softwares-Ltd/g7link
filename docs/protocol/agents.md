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

All on the control stream. None are carried over Bluetooth.

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

## 3. Implementation notes

- G7 Link desktop talks to the agent host (`g7-agents`) on a local socket and
  relays its threads; the folder's path never leaves the PC. The PC's
  notifications about agents come from the host, not from G7 Link.
- The core answers `agent.watch` itself and limits deltas per phone; the
  other requests go to the platform (`Platform::agent_request`) on a blocking
  thread.
- On the phone, a `BUSY` reply reaches the app as the generic failure
  (`G7LinkError.Internal`, whose reason starts with `BUSY`), not as `Denied`,
  which means the toggle is off.
