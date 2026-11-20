# Screenshots and recordings of a PC (protocol v1)

> Status: **v1.** Builds on [protocol v1](v1.md). License: CC BY 4.0.

A paired phone takes a screenshot of its PC's screen, or records it, and gets
the file. The PC captures, saves and sends; the phone only asks. The files
travel as ordinary transfers on the [files service](files.md).

## 1. Capability and consent

| ID | Offered by | Meaning |
|---|---|---|
| `pc.capture` | PC | Takes screenshots and records its screen for a paired phone |

A PC offers `pc.capture` when it can capture its screen (on Wayland: the
ScreenCast portal and PipeWire). Phones check it before asking.

Capturing needs the same consent as seeing the screen ([mirroring](mirror.md)):
the PC's `pc_mirror` device toggle for the phone, off by default. A PC also
refuses while its session is locked, and shows a notification every time a
phone takes a screenshot or records, with a way to stop a recording. Where the
system asks before sharing the screen (the portal's dialog, when it has no
earlier choice to restore), someone at the PC has to approve it.

## 2. Messages

The three requests are RPCs (phone → PC, [protocol v1](v1.md) §4.3); the
PC's events go one way on the `pc` lane.

```
RPC pc/capture.shot           t = "pc.capture.shot"           answer: ok { transfer: text, name: text }
RPC pc/capture.record.start   t = "pc.capture.record.start"   answer: ok
RPC pc/capture.record.stop    t = "pc.capture.record.stop"    answer: ok { transfer: text, name: text }

t = "pc.capture.prompt"                                       // pc lane
t = "pc.capture.record.ended"   b = { file?: { transfer: text, name: text } }   // pc lane
```

### 2.1 `pc.capture.shot`

Asks the PC for a screenshot of the screen it shares (one monitor), at its
full resolution, without the pointer, as a PNG. The PC also puts the picture
on its own clipboard. It answers `ok` with the file once it is on its way: `transfer` is the ID of the transfer that brings it (16 to 64 of
`[A-Za-z0-9_-]`) and `name` its file name (a plain name as the files service
allows). The transfer is an ordinary one, so it waits and resumes as any other.

### 2.2 `pc.capture.record.start` and `pc.capture.record.stop`

`start` asks the PC to record its screen (with the pointer) and its sound
into an MP4 file (H.264 and AAC); the PC answers `ok` once it records.
`stop` ends the recording the phone started. The PC saves the video in its
own Videos folder and answers `ok` with the file, naming the transfer that
brings it to the phone.

The PC records one screen for one phone at a time, and for at most 30
minutes. It ends a recording by itself when its user stops it, when the
session locks, when the phone disconnects, or at that limit; then it sends
`pc.capture.record.ended`, with the file when it saved one. A phone that
disconnected gets the file when it connects again (the transfer waits), but
no `pc.capture.record.ended`: a phone treats a recording as over when it loses
the connection.

### 2.3 `pc.capture.prompt`

Sent by the PC while a request waits for someone at the PC to approve sharing
the screen. The phone tells its user. It may come more than once. It travels
on the `pc` lane and the answer on the request's own stream, so a late
prompt can arrive after the answer: a phone ignores one for a request it no
longer waits on.

A request can wait that long: phones allow 150 seconds for `shot` and
`record.start`, and 120 for `record.stop` (saving the video).

### 2.4 Errors

| Code | `msg` | Meaning |
|---|---|---|
| `DENIED` | `off` | The PC's `pc_mirror` toggle for this phone is off |
| `DENIED` | `locked` | The PC's session is locked |
| `DENIED` | `declined` | Someone at the PC dismissed the screen-sharing dialog |
| `DENIED` | `files_off` | The PC's `files` toggle for this phone is off, so the file can't go to it |
| `BUSY` | | A recording is already running (for any phone), or a screenshot is being taken |
| `UNSUPPORTED` | | The PC can't capture its screen, or doesn't offer `pc.capture` |
| `NOT_FOUND` | | `record.stop` with no recording this phone started |
| `INTERNAL` | | Capturing or saving failed |

The `msg` values above are stable; any other `DENIED` reads as `off`.

Stopping is always allowed, also after the toggle was turned off: it ends a
capture the phone was allowed to start. Turning the toggle off on the PC
ends a running recording (the PC sends `pc.capture.record.ended`).
