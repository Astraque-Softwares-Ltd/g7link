# Changelog

All notable changes to G7 Link. Versions follow
[semantic versioning](https://semver.org); before 1.0, a minor version may
change things that need a step from you, and its notes say so.

## 0.1.21 (unreleased)

Reply from the phone to a Claude or Codex session running in a terminal on
the PC: G7 pastes your reply into that window and sends it, after checking
it's the right window and that nothing else is in the way.

**Update both together**, and update the G7 Linux GNOME extension to 1.5.0:
replies need its quiet primary-selection paste, which leaves your clipboard
alone.

### Added

- **Reply on the PC.** A followed session's screen on the phone has a reply
  box when the PC can reach it: Claude in Ptyxis or GNOME Terminal, and
  Codex when its window was started with `codex resume <id>`. G7 brings the
  window forward, pastes the reply and presses Enter, and only after
  proving the window shows that session. Before Enter it checks again that
  the focus didn't move, the PC isn't locked and the session is still the
  one in front. It's off until you allow it per phone on the PC, and the
  first reply to a window asks you to confirm where it goes. The text is
  never logged or stored.
- **What it won't do, and says so:** sessions in the Claude app ("Reply in
  the Claude app"), Codex sessions not started with `codex resume`, inside
  tmux, SSH or an editor, a locked PC, someone typing at the PC, and
  replies that start with `!` or `/`.

## 0.1.20 (2026-11-21)

Source control comes to the agents: see what a thread changed, commit,
push and open a pull request from the PC, the launcher or the phone,
through your own git and gh. Answer an agent from its notification, and
the phone stays quiet while you read on the PC.

**Update both together.** The phone and the PC exchange new messages for
source control and presence; each needs the other on 0.1.20.

### Added

- **Source control.** A thread's header shows its branch, what changed,
  commits to push and its pull request's checks. Review the changed
  files, tick what to commit, write the message, or "Ask Claude to write
  it" (a visible turn), then commit and push. Hooks, signing and
  credentials are your own git's. Switch or create branches, start a
  thread in its own worktree, and see the Pull Requests list. A pull
  request can be opened, marked ready and merged from the PC, pinned to
  the commit you saw; never auto-merged.
- **From the phone:** review and commit. The phone may commit only files
  whose whole diff it read; untracked folders and very large files stay on
  the PC. **Push and open pull requests** need "Push and open pull
  requests", a per-phone switch on the PC that is off by default. Merging,
  removing a worktree and switching a branch with changes stay on the PC.
- **Quote a diff line** into the reply box, on the PC and the phone.
- **Answer from the notification.** Requests offer Deny, Say why and
  Allow; short questions offer their answers as buttons. Only Deny works
  while the phone is locked. "1 of 3" counts a run of requests, and a
  short guard stops a tap landing on the next one as it slides in.
- **Presence.** While you read a thread on the PC, the phone holds its
  notifications and shows them when you step away or lock the PC. A thread
  you read on the phone counts as read on the PC, and the PC shows
  "· open on Pixel".
- **Limit alarm.** When a usage limit resets, the PC and the phone tell
  you and open the paused thread. Nothing is sent for you.
- **Share to an agent** from any Android app: text and pictures land in a
  thread's reply box, unsent.
- **The launcher:** "Ask about this" attaches your selected text (a copied
  picture, copied files and the browser tab are one key away); "Ask an
  Agent About a Screenshot"; agent threads in the main search; "Answer
  Agent Request", a command you can give a key; and source control.

### Fixed

- **A session stopped by its usage limit said "Working".** It now reads
  "Limit reached · resets 18:00".
- **"Earlier" kept the view on the Earlier button.** The message you were
  reading now stays where it was, with the older ones above it.
- **The plan name** is the CLI's own: "Ultra", not "Max".

## 0.1.19 (2026-11-21)

Agent threads keep their whole history: search them, rewind to any turn
with your files put back, fork from any point, and bring in sessions you
started outside G7. Code and commands are in colour. Messages shows each
conversation once, and the PC's screen on the phone is sharp.

**Update both together.** The phone and the PC exchange new messages for
history, undo and the screen; each needs the other on 0.1.19.

### Added

- **Agent history.** Threads are kept in one database on the PC. The list
  has Needs you, Working, Pinned and Recent, with Archived and Snoozed
  shelves. Pin, mark unread, snooze (until a time, or until the limit
  resets), unsnooze, archive and rename, from the PC, the launcher or the
  phone.
- **Search** across every thread's messages, from 3 letters, opening the
  thread at the match.
- **Rewind and undo.** Before each turn you start, G7 saves a restore point
  of the folder's files in its own store (never your repository's history).
  Rewind puts the files back and cuts the conversation to that turn, after
  showing what will change. Undo lasts 24 hours or until the next turn.
  "Edit from here" puts your message back in the box. From the phone,
  rewind changes the conversation only.
- **Fork** a thread from any turn, with Claude or Codex.
- **What changed.** Every recorded turn has a Changes view: per file, for
  that turn or the whole thread.
- **Bring in sessions** started in a terminal or another app. G7 finds
  them once an hour while a G7 app is connected; you pick what to bring
  in (up to 50 at once) after a read-only preview. Compressed Codex
  history is read too.
- **The phone works offline.** Threads you've opened stay readable, and
  messages you write while the PC is away wait and go out once, in order.
- **Drafts follow you** between the PC, the launcher and the phone.
- **Colour.** Code is highlighted in both themes, and an untagged block's
  language is guessed. Commands are coloured like a shell, paths show the
  folder dimmed and the file bright, and output marks errors, warnings and
  `file:line`. Each status has its own colour and shape: needs you is a
  solid amber chip, done is green everywhere. Every colour is checked for
  contrast in both themes.
- **Pinch zoom on the PC's screen**, with Fit, 100% and 200% and a map
  while zoomed.

### Changed

- **Messages shows each conversation once.** A chat is recognised by the
  phone's own conversation ID, not its title, so a group no longer appears
  once per "(N messages)". Two sections: On your phone now, and
  Conversations (texts and chats you kept). A chat you clear on the phone
  stays open on the PC until you close it, and goes after 24 hours unless
  you keep it; kept chats stay 30 days. Removing a chat on the PC clears it
  on the phone. Existing duplicates are merged on update without losing a
  message.
- **The PC's screen on the phone is sharp.** It's sent at the screen's own
  size, no longer scaled to a multiple of 16, at a rate that holds up on a
  quiet link. A lost frame is noticed and recovered at once, the phone
  shows each picture the moment it's decoded, and Wi-Fi stays in
  low-latency mode while you watch.
- **Touch on the PC's screen works like the touchpad**: tap, double tap,
  hold for right-click, tap-and-drag, two-finger scroll with glide; a
  second finger never clicks. "Task View" is now Overview.
- **Clear states** for sharing turned off, can't reach the PC, and stopped,
  instead of an endless "Connecting".
- **A still screen costs nothing.** The PC encodes a picture only when the
  screen changed, so an idle desktop sends nothing and the laptop stays
  cooler.
- **Agent headers are shorter.** A followed session's header is two rows,
  and "Keep in status bar" is a toggle in it.

### Fixed

- **The PC's hardware encoder is tried in the mode the driver has.** On
  Intel laptops where the driver offers only fixed-quality encoding, G7
  asked for a bitrate, was refused, and always fell back to software. It
  now uses the fixed-quality mode there, picking the quality from the
  target rate.
- **The Linux installer no longer hangs** when GitHub's API is slow: it
  retries, times out, and falls back to the release page.

## 0.1.18 (2026-11-20)

The phone and the PC stay connected, and talk to each other on G7 Link's own
protocol: a slow answer no longer holds up a key press, a call or anything
else.

**Update both together.** This version speaks a new protocol and can't
connect to 0.1.17 or earlier. The one that's behind says "Update G7 Link on
…" instead of staying offline. Pairing, your settings and what's stored
carry over; nothing needs pairing again.

### Changed

- **G7 Link protocol v1.** Every request now travels on its own line instead
  of waiting behind the others. Each feature (typing, the touchpad, calls,
  media, the clipboard) keeps its own order, lists and other slow answers go
  on their own, and a request you no longer need is cancelled on the other
  device. Long lists arrive in parts, so nothing is cut short to fit.
- **Sound from the PC is numbered and timed.** The phone drops late pieces
  and keeps a small cushion sized to the network, so it stutters less on a
  busy Wi-Fi network.
- **The PC does more at once.** Its core runs on four threads instead of two.

### Fixed

- **Far fewer disconnects.** Most drops were G7 Link closing a healthy
  connection itself:
  - a network change on the laptop (a cable, a VPN, a container's network)
    closed the link after one missed 2-second check; now it only makes the
    next check come sooner;
  - one look at the path that seemed "not local" closed it, which happened
    on the phone's own hotspot; now it takes 10 seconds without a local
    path, and the phone's hotspot counts as local;
  - two missed checks (about 11 seconds), not one, now mean the other device
    is gone, and the answer to a check never waits behind other traffic;
  - reconnecting backs off only after a failed attempt, and a working
    connection is never replaced when both devices dial at once.
- **A busy device says so.** "The device is busy. Try again in a moment."
  where it used to say Messages were off, or show nothing.
- **Sending many files at once** retries the ones over the limit instead of
  saying there isn't enough space.

## 0.1.17 (2026-11-20)

You steer the agents now: pick the model, plan before anything changes, send
a message while one works, answer its questions, and show it a picture, from
the PC, the launcher or the phone.

### Added

- **Pick the model and how hard it thinks.** The model and effort lists are
  the ones Claude and Codex themselves report on your PC, short names first
  and the rest under "More models"; G7 ships no list of its own and fetches
  none. Until an agent has said, the choice is "Default", or type a model
  name with "Other…". A choice applies to the thread's next message, never
  to a turn that's running, and is kept per agent and folder. When another
  model answered, the thread says so ("Answered by …").
- **Plan first.** In Plan mode the agent changes nothing: it reads, then
  writes a plan you see as a card. Implement it (on the PC and in the
  launcher you choose how much it asks first; from the phone it asks before
  edits and commands) or say what to change and get a new plan; the old
  one is marked "Replaced by a newer plan". Codex needs a version that offers
  it; otherwise Plan is hidden for Codex with a line saying so. There are
  still only three modes: asks before edits and commands, asks before
  commands (PC and launcher), and Plan.
- **Send while it works.** A message sent while a turn runs either steers it
  or waits its turn, as your setting says ("While it works, a new message:
  steers it / waits its turn"); Alt+Enter does the other one, and on the
  phone a long press. Codex reads a steer at once; Claude at its next step,
  after the tool it's running, never mid-sentence. Up to five messages wait
  above the box, each with Send now, edit and remove. When a turn fails or
  you stop it, they wait for you ("Waiting: the last turn didn't finish.");
  after a usage limit nothing is sent by itself.
- **The agent can ask you.** When Claude or Codex asks a question, it waits
  in the dock like a request: pick an option, several where it allows, or
  "Something else" in your own words, then Send answers; or dismiss it, and
  the agent hears you didn't answer. Answer it on the PC, in the launcher or
  on the phone; the first answer wins. A question Codex marks secret is
  answered on the PC only, and the answer is never kept, logged or sent to a
  phone.
- **Show it a picture.** Add up to four PNG, JPEG, WebP or GIF images to a
  message: pick, paste or drop them on the PC, take one from the gallery or
  the camera on the phone, or attach the clipboard's image in the launcher.
  G7 decodes each one itself and sends the agent a fresh copy, which drops a
  photo's location and camera details. Other files you name with `@`.
- **`@` for files, `/` for commands.** Type `@` for files of the thread's
  folder, best match first, honouring `.gitignore`; G7 inserts the path and
  the agent opens the file itself, under its own rules. Type `/` at the start
  for the agent's own commands and skills, and G7's `/model`, `/effort` and
  `/plan`. Commands that need a terminal or change your account or settings
  aren't offered.
- **Your MCP servers work in G7 threads.** The MCP servers you set up for
  Claude and Codex load in G7's threads too. For Claude there's a switch per
  folder ("Use my MCP servers here"), on by default; Codex loads its servers
  as it always does, since it has no way to leave them out. Each of their tools still asks
  first, and "Always allow" isn't offered for them. G7 never opens the files
  they're set up in: the PC names a folder's servers once a thread has run
  there, the phone only counts them. A server that asks for a sign-in or a
  value mid-turn is turned down with a note; do that in a terminal.
- **Context and limits.** A thread's details show how full its context is
  ("Context 12% of 1M") after each turn, and Agents setup shows each agent's
  usage windows with when they reset, as the agents report them. A thread
  shows one amber line when a limit is nearly used. G7 never asks a vendor
  for these and never shows a cost.
- **Drafts are kept.** What you've typed but not sent in a thread on the PC
  stays there when you leave the page or restart G7 Link, and Up in an empty
  box goes back through what you sent from this PC.
- **The PC stays awake while an agent works.** On by default ("Keep this PC
  awake while an agent works"); it lets the PC sleep again a minute after
  the last turn ends, and never while a thread only waits for you. Closing
  the lid does what your system says, as before.
- **Kotlin, TypeScript and TSX code is coloured** in answers and edits.

### Changed

- **Claude is asked one more question when it starts.** To list its models
  and commands, G7 asks Claude's own CLI on the channel Stop already uses,
  before anything is sent, at no usage cost. Its answer also holds your
  account's email and organisation: G7 drops that part unread. A change of
  model, effort or mode restarts the idle CLI with its documented flags.
- **Codex's experimental switch, for Plan only.** Plan mode in Codex needs
  OpenAI's documented "experimental API" opt-in; G7 turns it on and uses it
  for that one field.
- **Adding a folder says the MCP servers come too,** and that you can turn
  that off for Claude in that folder.
- **Older phones and PCs keep working.** A phone with 0.1.16 or earlier still
  follows threads from an updated PC: a thread in Plan mode shows as asking
  before edits and commands, a plan as an answer and a question as a step to
  answer on the PC. An updated phone works with a PC that isn't updated yet,
  and offers what that PC can do; models, Plan, the queue, questions,
  pictures, `@` and `/` wait for the PC's update.

### Fixed

- **Browsing the PC's agent sessions from the phone no longer holds up the
  link.** Listing, reading and continuing a session used to wait in line with
  everything else the phone sent, so a slow read could delay a key press, a
  call action or the connection check by up to 15 seconds. They now run on
  their own.

## 0.1.16 (2026-11-20)

Agent threads read like a conversation: what each turn did folds away behind
its answer, answers show as formatted text, and you can deny a request and
tell the agent what to do instead.

### Added

- **Each turn folds behind its answer.** Once a turn ends with an answer, you
  see your message, "Worked for 2m 14s", the answer, and the files it changed
  with lines added and removed; open a file to see that turn's edits to it.
  Open the fold to see every step it took. A turn that was stopped or failed
  stays open, so you can see where it got to. On the PC's Agents page, on the
  phone and in the launcher.
- **Steps say what they did.** "Ran cargo test", "Edited src/lib.rs" with
  the lines it added and removed, "Read README.md"; a command that took a
  while says how long. A run of steps shows as one line, "Ran 3 commands and
  changed 2 files", that opens to the steps, and is marked when one of them
  failed. Open a step to see its command, its output, or its edit as a diff
  with the changed words marked.
- **The agent's task list stays in one line.** "Tasks 3/5" and the current
  task sit just above where you type and open to the whole list, instead of
  scrolling away with the thread. On a wide PC window it's in the details
  panel.
- **Answers are formatted.** Headings, lists, checklists, tables, quotes and
  code blocks with colours, a wrap switch and Copy. A link whose text isn't
  its address shows where it goes before it opens. Nothing an agent writes
  can load anything: images and HTML show as text, and math and diagrams show
  as code.
- **Thinking shows as it happens.** While the agent thinks, the last line of
  the thread says "Thinking". Neither Claude nor Codex sends the text of its
  thinking today, so that's all it says for now; when one does, its thoughts
  show there and in the turn.
- **Deny and say why.** When an agent asks to run a command or change a file,
  type what it should do instead and send it: the request is denied and your
  words go to the agent. Claude carries on in the same turn; Codex stops and
  reads your words next. While there's text in the box, Ctrl+Enter doesn't
  allow; click Allow if that's what you want. It needs the PC to be updated
  too; until then the phone offers plain Deny only.
- **A request waits while you type.** One that arrives while you're typing a
  reply shows "Arrived while you were typing" and doesn't take the buttons
  until you pause, so a keystroke never answers it.
- **A drawer for agents on the phone.** Swipe in from the left edge, or tap
  the sidebar button, to switch threads, open a session started on the PC,
  start a new thread or change PC, without going back to the list.
- **The PC's Agents page folds its side panels away.** Hide the thread list
  with Ctrl+[ and the details panel with Ctrl+], or with their buttons; the
  page remembers which you hid.

### Changed

- **A request shows where it belongs.** The step it's about says "Wants to
  run cargo test" and "Waiting for you"; once it's answered on another
  device, "Allowed on Pixel" or "Denied on Pixel", and "Allowed by your rule
  for this thread" when an Always allow rule answered it. Requests come one
  at a time, the oldest first, "1 of 3", and Stop stays in reach while one
  waits.
- **Home keeps Agents to one line.** The phone's Home shows a single Agents
  row saying what needs you or what's running; the start box and the threads
  are on the Agents screen it opens.
- **Thin scroll bars on the Agents page.** They show while you scroll or point
  at a list, and only where something scrolls, instead of the default bars.
- **Older phones and PCs keep working.** A phone with 0.1.15 or earlier still
  follows threads from an updated PC; thinking shows there as a step. An
  updated phone works with a PC that isn't updated yet; "deny and say why"
  waits for the PC's update.
- **If you go back to an older G7 Link on the PC,** threads with thinking in
  them may not open there.

## 0.1.15 (2026-11-20)

Fixes from the first day with Agents and the player.

### Fixed

- **The full player stays open when the track changes.** Next and previous
  closed it and dropped you on Home, because the PC's player can vanish from
  its list for a moment while it changes tracks.
- **Updating G7 Link now updates the agent host too.** The host from before
  an update kept running, so sessions from your terminal, Continue here and
  export didn't work until it stopped. G7 Link now replaces an older host by
  itself when nothing is running in it, and otherwise offers Restart agents.
- **No "needs you" pop-up for the thread you're looking at** on the PC.
- **Agent requests come one at a time,** the oldest first, with how many more
  are waiting, on the PC, the phone and in the launcher.
- **Commands keep their spaces.** The code font drew `--` over the space
  before it, so `tsc --noEmit` read `tsc--noEmit`; code is now shown without
  ligatures.

## 0.1.14 (2026-11-19)

Agents get a page of their own on the PC, the phone follows the sessions
you start in a terminal or the Claude app, and music on the PC gets a real
player on the phone.

### Added

- **An Agents page in G7 Link on the PC.** Agents is now in the sidebar,
  second after Home, with a badge for what needs you. Start a thread, read it,
  reply, stop it and answer what it asks, all on one page, with your folders
  and each phone's switches in its setup area. It works with no phone paired.
  G7 Link connects to an agent host that is already running when it starts,
  so the badge is right from the first moment; it starts one only when you
  open the page, start a thread, or a phone asks.
- **Follow the sessions you start yourself.** Claude and Codex sessions you
  started on the PC in a terminal or in the Claude app show under "On this
  PC", and on the phone under "Started on" and the PC's name: what they're
  doing now, and the whole conversation as it goes, a finished message or
  step at a time. G7 Link reads them from the agents' own session files,
  read-only, and lists those that changed in the last 7 days (the newest 50).
  "Needs you" shows only when Claude itself says it waits for you; when it
  only looks that way (always the case for Codex) it says "May need you",
  which never counts in a badge. You can't type into these sessions or answer
  them from G7 Link: answer them where they run. A switch on the PC, "Follow
  sessions started outside G7", turns this off altogether.
- **Continue a session as a thread.** Once nothing has a session open, Continue
  here makes it an ordinary thread with its last 200 messages and steps,
  waiting for what you send next. Its folder has to be one of your agent
  folders: the PC offers to add it; a phone says to add it on the PC. Started
  from a phone, it always asks before it changes a file or runs a command that
  could change something.
- **A phone sees your sessions only if you say so.** It's a second switch per
  phone, off until you turn it on, and only while that phone can use agents.
  The PC asks once, the first time the phone looks. The phone sees each
  folder's name, never its path.
- **What a thread is doing now.** Threads show their current step ("Now: Run
  command") and how long they've been at it, and the agent's own plan as a
  checklist when it made one. On the phone a thread you started or opened
  shows as a Live Update while it works; a session you follow can too, if you
  ask for it with "Keep in status bar".
- **Agents on the phone's Home.** A card for each PC with agents: what needs
  you (with Review), what's working and its step, or a line to ask an agent
  something.
- **Keep a thread as Markdown.** On the PC, Copy as Markdown and Export… in
  the Agents page's actions copy or save a thread or a followed session, with
  its messages, commands, edits and approvals; the phone's Share sends it to
  any app. The folder shows by its name, never its path, and a long thread
  stops at 2 MiB.
- **A music player on the phone for what plays on the PC.** A mini player sits
  above the tabs while something plays; swipe it up for the full player with
  large artwork, colours taken from the cover, a seek bar that follows the
  PC's real position (at the player's speed, too), shuffle and repeat where
  the player has them, and the PC's volume. Large artwork is fetched only when
  the full player shows it and kept in memory only.
- **Artwork from Spotify and other streaming players** now shows: the PC loads
  the cover the player names by an `https://` address, limited to 4 MiB and
  5 seconds. Before, only covers stored on the PC showed.

### Changed

- **Settings › Agents is one row** that opens the Agents page. Each phone's
  switches are still in that phone's own settings too.
- **A thread isn't resumed while it's open elsewhere.** If you opened a thread
  in a terminal with Open in Terminal and it's still running there, replying
  in G7 Link says so instead of starting a second copy.
- **Music on the phone uses the PC's own volume,** not the player app's.

### Fixed

- **The notification icon is the G7 mark,** centred in the status bar and the
  Live Update chip, instead of the old two-loop mark that read as ∞.
- **Bluetooth to the touchpad says what really happened.** "Out of range" now
  means the phone never saw the PC; any other failure says Couldn't connect,
  and when the PC isn't offering Bluetooth for a few minutes the phone says
  so instead of retrying every 20 seconds. Retries in the background back off,
  and Try again starts over. The PC tells the phone whether it really opened a
  connection window, and a renewal joins the open window instead of using up
  another. Ending G7 Link's Bluetooth link no longer disconnects the phone's
  other Bluetooth connections, such as audio. Both sides now log each step of
  the link, so a failure can be traced.
- **A cover cut short while downloading isn't shown** half grey; the player
  keeps the small picture.

## 0.1.13 (2026-11-19)

Agents: start, follow and answer the Claude and Codex on your PC from your
phone, with the PC's consent and supervision on every thread the phone starts.

### Added

- **Agents: follow and answer coding agents on your PC from your phone.**
  G7 Link now comes with the agent host, which runs the Claude Code and
  Codex you installed on the PC, signed in with your own accounts, in folders
  you list in Settings › Agents. From the phone you can see every thread,
  read what the agent writes as it writes it, start a thread, reply when it
  stops, stop it, and allow or deny what it asks to run or change. A thread
  started from the phone always asks before it changes a file or runs a
  command that could change something, and the PC shows a notification with
  Stop. Each phone needs your OK on the PC first: it asks once, and the
  switch is under that phone in Settings. Your phone sees folder names only,
  never their paths. G7 is not affiliated with Anthropic or OpenAI.

### Fixed

- **An empty chat prompt doesn't show as an empty line** in the typing box
  (Claude, Slack, Gmail): the line break such a prompt keeps for its
  placeholder isn't counted as text.

## 0.1.12 (2026-11-13)

A touchpad you can feel, motion in both apps, a look before the clipboard
goes, and a long pass over security and memory, especially on shared Wi-Fi.

### Added

- **The touchpad vibrates** as you click, drag and scroll, following the
  phone's touch-vibration setting. Settings has a switch to turn it off.
- **Raised keys on the Touchpad tab,** with a help button that shows the
  gestures and fades away, instead of a line of tips.
- **See the clipboard before sending it.** Home's Clipboard shows what is on
  the phone's clipboard first. The tile, notification and widget still send
  at once.
- **Codes from texts are copied for you.** When a text brings a one-time
  code, it is on the PC's clipboard at once and a notification says Ctrl+V
  pastes it, in a browser or anywhere else. The clipboard is wiped after 2
  minutes. This comes on with the update, even where it was off before;
  Settings › General can turn it off again.
- **The phone's typing box follows the laptop.** It shows the text of the
  field you're typing into on the PC, where the app shares it (GTK, Qt,
  Firefox, LibreOffice...), so deleting on the laptop deletes on the phone
  too. A bubble over it names the app and field, and moving to another field
  switches the box; what you typed into a field that doesn't share its text
  comes back when you return to it.
- **Pick how the touchpad reaches the PC.** Tap "Connected · 9 ms" at the top
  of the Touchpad, Air mouse or Presentation tab: Auto (whichever answers
  faster, measured as you go), Wi-Fi or USB, or Bluetooth. Bluetooth carries
  the touchpad, keys and the typing box, so it keeps working with no network,
  and asks for what's missing on the way: the phone's Nearby devices
  permission, and an Allow on the PC. Auto never switches in the middle of a
  drag or a word, and never asks for anything by itself; it offers Set up.
  Each PC keeps its own choice. Bluetooth links one phone to a PC at a time.
- **Pictures and files wait over the typing box** with a small preview each,
  to take out with a tap or send with the message.
- **Bluetooth when there's no network.** Turn it on for a phone and a PC on
  both (the phone's Settings, and the device's settings on the PC). When they
  share no network, the PC can ring the phone and ask for its hotspot, short
  copied text goes both ways, codes from the phone's notifications are copied
  on the PC, and the Touchpad tab and its keyboard keep working. The link is
  encrypted with keys only that pair has, and the PC can be connected to only
  for a few seconds when it needs to be. Android 12 and later.
- **Instant Hotspot offers itself.** When the PC's internet goes, a
  notification offers a linked phone's hotspot (with Elevated): one click
  turns it on and joins it, finding its network the first time. Back in
  range of a known Wi-Fi network, the PC moves back and turns off the hotspot
  it turned on, with how much data it used. Home's Hotspot button shows
  whether the hotspot is really on.
  With Elevated, the phone turns its hotspot on through Android's tethering
  service, which Android 17 needs; without it, the phone asks you with a
  notification.

### Changed

- **Typing from the phone lands as typed.** Each change goes as one edit
  where it happened, instead of deleting and retyping everything after it;
  characters the laptop's layout lacks (’, é, emoji) are typed in place; keys
  go at a pace the desktop keeps up with; Caps Lock and layout switches are
  followed; and the first key after starting isn't lost.
- **Tap then drag drags,** from where you tapped, instead of selecting by
  word or maximising the window.
- **Pictures paste where you're typing on the PC** again: shrunk to a size the
  clipboard takes, and pasted only once the PC has them.
- **The typing box keeps what you typed.** It no longer empties itself while
  you pause, after a tap on the touchpad, or when you switch apps and come
  back. Where the laptop can't read a field, what you typed stays, with a note
  when it may differ there and a button to start afresh.
- **Chrome and Electron apps share their text fields** (Claude, VS Code,
  Slack, web pages) while you type from the phone, so the box shows what's
  already in them, multi-paragraph prompts included. The PC asks each app once,
  only when the phone types into it; Settings › General, "Show browser fields
  on the phone", turns it off.
- **Bluetooth fixes.** A message of exactly 4 KB no longer ends the link,
  more than about 20 keys typed in a burst no longer time out over it, and a
  PC whose Bluetooth is off no longer offers the link.
- **The typing box doesn't freeze or fill in suggestions.** It keeps following
  the laptop after an edit that didn't go through, or a read that failed
  while an app changed its field. A browser's address-bar suggestion stays out
  of the box, and what you type next replaces it on the laptop as typing there
  would. Once you've typed, the keyboard stays open until you close it.
- **The bubble over the typing box shows the app's icon,** with the field's
  name only when it adds something ("Claude · Claude" is gone), and no longer
  crowds the keys above it. It follows the window you're in, not another
  app's focus.
- **Confirmations come as toasts.** Dismissing the phone's notifications on
  the PC says so in a toast with Undo, instead of a row in the feed. On the
  phone, sharing to the PC closes the sheet with a toast when there's nothing
  more to do (the sheet stays to offer Paste on a PC that takes it), as does
  a share that waits for an offline PC.
- **The PC app scrolls with the touchpad** the way GNOME's own apps do: a
  flick keeps a list going and slows it down, instead of stopping the moment
  your fingers lift. Every list and page that scrolls up and down does it,
  from Photos and Messages to Settings and the sheets.
- **Copying a photo on the PC is quicker and says how it's going.** Copy
  spins until the photo is on the clipboard, then says Copied; Save no longer
  spins at the same time. It uses the full photo that clicking the tile
  already started bringing, rather than fetching it a second time, keeps it
  for the next copy and the viewer, and converts it for the clipboard in
  about half the time.

- **The PC's Bluetooth beacon no longer gives it away.** It used to broadcast
  part of the PC's ID all the time, so anyone scanning could recognise and
  follow the PC. Now it changes every 15 minutes, only your paired phones can
  recognise it, and with no phone paired the PC broadcasts nothing. On the
  phone, "Wake when PC is nearby" asks for the Nearby devices permission, and
  keeps working after the PC restarts, which it often didn't. The old beacon
  still goes out while a paired phone has an older G7 Link, so update both.
- **Figtree for text.** Both apps set their text in Figtree, a calmer and
  more readable face. Geist Mono stays for codes, keys, commands and timers.
- **Motion in both apps.** Pages, menus, sheets, banners and notifications
  move with one shared set of timings, and keys come back smoothly when the
  phone's keyboard goes down. Both apps follow the system's reduced-motion
  setting, and nothing animates while it can't be seen.
- **Pairing with a link** shows a code on both screens to compare.
- **Text and links shared from other apps** always ask which PC first.
- **The PC asks before opening a link** from a phone.
- **Phones can't type on a locked PC,** and PCs can't control a locked
  phone; the phone says when a PC controls it.
- **Elevated starts only on trusted Wi-Fi networks.**
- **The phone's name is on the network only while pairing,** and USB
  pairing works only while the pairing screen is open.
- **Copy says "Copied" on its button only,** without a second message.
- A phone's link in the timeline says "Sent from".
- The Screen tab's buttons show their whole names, with room above the
  keyboard.
- **Paste on the Touchpad tab is one quiet line** above the mouse keys, so
  the touchpad keeps its room.
- **Scroll mode shows a small pill** on the touchpad instead of a sentence
  across it.

### Fixed

- mDNS no longer busy-loops after it restarts, and closes the sockets of
  networks the PC has left.
- A phone or app window opens in front of the main window, not behind it.
- The conversation list stays where it was when you open a conversation.
- G7 Island no longer redraws every frame while music plays.
- The phone's touchpad moves the cursor at once, instead of now and then
  seconds later when control moved to the phone.
- Enter on the phone's keyboard loads a link in the same browser tab, instead
  of opening a new window; elsewhere it's still a new line.
- The virtual webcam no longer keeps a CPU core busy while nobody uses it.
- On GNOME, a copied code really reaches the clipboard. With the G7 Linux
  extension 1.5 it stays out of clipboard histories; with older ones, G7
  Linux's history sees it until it's wiped.
- Call audio only goes to your phone. The PC takes a Bluetooth device for
  your phone only when it has exactly the phone's name, never one whose name
  just contains it (someone else's "Pixel 8 Pro" for your "Pixel 8"), and
  remembers it only once you connect call audio to it. "Not this device" on
  the Calls page makes it forget a wrong one. Headsets and speakers are never
  taken for a phone.
- A call that ends while its audio is connecting no longer leaves the PC on
  call audio saying the phone "is still using its own audio".
- The PC's Bluetooth and hotspot settings files are written safely, and a
  damaged one is kept aside instead of being replaced.
- The phone says where to turn on "Your PC in your pocket" when the PC's
  files aren't shared.
- Much less memory for notification pictures, copied pictures, recordings'
  sound, screen sharing and the timeline.
- Received files are saved under names claimed exclusively, without
  text-direction tricks, and synced files stay inside the sync folder.
- Many limits on what other devices on the network and paired devices can
  ask for, and the G7 Link data folder is private to the user.

## 0.1.11 (2026-11-12)

Screenshots and recordings of the PC from the phone, a Paste chip, your own
say over Bluetooth media, and G7's own themes.

### Added

- **Screenshot the PC from the phone.** Home's "Screenshot" takes the PC's
  screen at full resolution: crop it on the phone, then save it, share it or
  copy it back to the PC. It is on the PC's clipboard too.
- **Record the PC's screen from the phone,** with its sound, from Home or
  the remote's Screen tab. The video lands in the PC's Videos and on the
  phone when you stop. Both need "See this screen" on for that phone, never
  work while the PC is locked, and the PC says each time.
- **Paste chip.** When the PC copies something while the phone's trackpad
  is open, a chip with its first line appears: tap to paste. Terminals get
  Ctrl+Shift+V. On GNOME this needs the G7 Linux extension 1.4.
- **Phone media on this PC,** per phone, in Home and Settings. Off by
  default: a phone connected over Bluetooth keeps its music on its own
  speaker or headphones. A change made while one device is away carries
  over when they reconnect, the latest one winning.
- **Type like a chat app.** The phone's keyboard field grows, Enter makes a
  new line, and a round button sends or dictates (hold to talk). Images
  paste where the PC is typing.
- **The phone's keyboard opens by itself** when a text box gets the focus on
  the PC.
- **Middle locks the trackpad to scrolling.**
- **Relays:** add, check and pick relay servers on both apps.

### Changed

- **G7's own themes.** Both apps use G7 Linux's palettes, or follow G7 Linux
  with "Match G7 Linux". The old Hive, Glass, Graphite and Bloom looks are
  gone.
- **What's on is visible.** Active buttons, tabs and rows use the accent in
  both apps.
- **The PC's player** puts its controls under the title, centred.
- **Copy says "Copied" where you clicked.**

### Fixed

- Phone notification pictures show whole instead of squeezed.
- An old chat no longer pops up again when another one gets a message.
- No message box and no Call for senders that take no replies.
- On GNOME, the phone's clipboard reaches the PC's clipboard whether or not
  G7 Link has the focus, and password managers' copies are never recorded.
- Many hardening fixes for pairing, storage paths, USB, Elevated and
  untrusted replies.

## 0.1.10 (2026-11-12)

Faster and more truthful everywhere: mirroring, calls, copying, the trackpad
and Bluetooth, plus a new Home built for a busy phone.

### Changed

- **Home, redesigned.** The actions sit in a fixed bar that never scrolls
  away; notifications are grouped by app, with the app's real icon, and
  scroll on their own; the phone's controls stay on the right.
- **Badges that mean something.** Messages and Calls count only what arrived
  since you last opened them, and clear when you do.
- **Messages has app tabs** (All, SMS, then each chat app) and shows each
  conversation's app icon.
- **The mirror window is the phone's screen and nothing else.** Its controls
  appear on hover. Mirroring converts colors on the GPU for a steadier 60 fps,
  and app windows lay out exactly as on the phone.
- **Real icons.** Both apps draw G7 Linux's glyphs, each matching what its
  row does, and an icon stays on the first line of a wrapped label.
- **Notifications on the PC** read as sender, then message, with the phone
  app's icon.

### Fixed

- Mirroring no longer turns up the phone's volume.
- Answer and Decline react at once, and clicking a phone notification no
  longer leaves a busy pointer.
- Copy shows "Copied" right away and never freezes the window.
- The phone's trackpad scrolls smoothly with two fingers, glides after a
  flick, and no longer right-clicks after a short scroll.
- The phone's trackpad, keys and buttons never wait behind slow PC replies.
- Replies are offered only to SMS senders that can receive them.
- Bluetooth call audio connects with one targeted request, shows
  "Connecting…", and says "On PC" only when the call is really there.
- Elevated starts over Wireless debugging, without a cable.
- A dead link is noticed within about 10 s while the app is open.
- Sharing the PC's screen with the phone recovers after a bad patch.
- The app opens maximized under the top bar, not over it.

## 0.1.9 (2026-11-11)

Both apps are redesigned in G7 Linux's own look. There were no 0.1.7 or 0.1.8
releases; those numbers went to test builds.

### Changed

- **Redesigned desktop app.** A phone picker and search in the header, a
  sidebar of sections with colored glyphs, and a footer with the connection,
  the battery, each screen's main action and Actions (Ctrl B), which lists
  everything else on that screen. Every screen is rebuilt with dense rows and
  Geist Mono, and the phone is drawn large only in the mirror window.
- **One welcome screen** replaces the setup walkthrough: it checks the local
  network, notifications, the webcam module and mouse and keyboard control,
  each with its fix, then pairs a phone.
- **Redesigned phone app.** Home lists what you can send to the PC, how you
  can control it and what the phone can do, as rows with colored icons; Guide
  and Settings sit in a bottom bar. First run is one screen of permissions.
- **Notifications read as sender, then message,** with the phone app's icon.
  The phone's name joins the title only when more than one phone is paired.
- **The window opens maximized**, under the top bar instead of covering it.
  F11 still makes it full screen.
- **The phone asks for the camera only when you use it**, not when pairing.

## 0.1.6 (2026-11-11)

The first release of G7 Link: the phone companion for G7 Linux, on a Linux PC
and an Android phone.

### New

- **Linux only.** The desktop app is built for Linux and installs as a `.deb`,
  `.rpm`, AppImage or tarball, or with `install.sh`. It uses the desktop's own
  notifications, media controls (MPRIS), clipboard, file manager and system
  tray.
- **Device keys sealed with your keyring.** On the PC the device key, clipboard
  history, the LocalSend identity and queued sends are sealed with a key kept
  in the desktop's Secret Service (GNOME Keyring or KWallet). Without a keyring
  they stay in files only your account can read. Existing keys are sealed on
  first start, and a key the keyring can't open is never overwritten.
- **G7's look.** Both apps follow G7's design: dark first, G7's colors and
  glyphs, Geist Mono for code, and no card boxes. The Cobalt theme and a bottom
  dock are new on the desktop; the other themes stay in Settings.
- **The phone app is one scan away.** The installer ends with a QR code to scan
  with the phone's camera for the Android app; `g7link-desktop --phone-app`
  shows it again.
- **Full screen.** The app opens full screen, and phone and app windows can go
  full screen too.
- **Answer a ringing call from Home.**

### Fixed

- Work for the Qt thread is never run after the code that asked for it has
  given up.
- `install.sh --uninstall` runs the app's own cleanup first.
