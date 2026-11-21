# Media service (protocol v1)

> Status: **v1.** Builds on [protocol v1](v1.md). License: CC BY 4.0.

What plays on one device shows on the other, which can control it: a
phone's music in the PC's app and in the desktop's media controls (MPRIS),
a PC's in the phone's media controls. Both directions use the same messages.

## 1. Capabilities

| ID | Offered by | Meaning |
|---|---|---|
| `media.control` | both | Shares its players and takes commands for them (a phone needs notification access to read other apps' players) |
| `media.remote` | both | Shows other devices' players and sends commands to them |
| `media.watch` | phone | Shows a PC's playback streamed to it (Play on phone, §4) |
| `media.stream` | PC | Streams a player's picture and sound on request (§4) |
| `media.sound` | PC | Sends its sound to a phone that plays it (§6) |

A device sends its players only to devices that offer `media.remote`, and
only while the user allows media for that device (the `media` device
toggle, on by default). It takes commands only while it offers
`media.control` and allows the sender.

## 2. Messages

| Message | Lane ([protocol v1](v1.md) §4) | Dir |
|---|---|---|
| `media.state`, `media.sync`, `media.bt` | `media` lane, one way | either |
| `media.command` → `ok` | `media` lane, with `id` | either |
| `media.watch` → `ok` | `media` lane, with `id` | PC → phone |
| `media.icon` | RPC `media/icon` | either |
| `media.art`, `media.file` | their own streams (§2.3, §4) | either |
| PC sound | datagram `0x02`, else the `media/sound` stream (§6) | PC → phone |

The `media` lane keeps a device's states and commands in order, and nothing
else waits behind them.

```
t = "media.state"    b = { players: [Player] }          // no answer
t = "media.sync"     b = {}                             // no answer
t = "media.command"  id = n  b = { player: text, action: text, position?: uint, value?: text }
t = "ok"             re = n

RPC media/icon   t = "media.icon"   b = { key: text }   // §2.2
  answer: ok { png: bytes }
```

**`media.state`** carries every player the sender shares, the most
relevant first (what's playing first). It's sent when a session starts, after
every change, and again when the user changes what the other device may
see. An empty list means nothing plays (or the user stopped sharing); the
receiver clears what it showed. A receiver also clears a device's players
when the connection ends.
If the artwork would push the snapshot over the frame limit, the sender
keeps all player metadata and defers excess artwork until another snapshot.

**`media.sync`** asks for a `media.state` now (the user allowed media again
on the receiving side).

**`media.command`** asks a player to `play`, `pause`, go to the `next` or
`previous` item, `seek` to `position` (milliseconds), turn `shuffle` `on` or
`off` (`value`), or set `loop` to `none`, `track` or `playlist` (`value`).
`value` is ignored for the other actions. Errors: `NOT_FOUND` when the player
is gone, `UNSUPPORTED` when it can't do that, `DENIED` when the user turned
media off for the sender, `BAD_MESSAGE` for a seek without a position, or a
`shuffle` or `loop` without a `value` it takes.

```
Player = {
  id:        text,       // 1 to 256 bytes, stable while the player exists
  app:       text,       // user-visible app name
  title?:    text,       // each text up to 256 characters
  artist?:   text,
  album?:    text,
  playing:   bool,
  duration?: uint,       // milliseconds
  position?: uint,       // milliseconds, when sent
  actions:   [text],     // what it takes: play, pause, next, previous, seek, shuffle, loop
  art_key?:  text,       // 1 to 64 bytes; the same key means the same picture
  art?:      bytes,      // JPEG or PNG, up to 256 KiB
  rate?:     float,      // playback rate, 0.25 to 4; absent means 1 (§2.1)
  age?:      uint,       // ms between sampling `position` and sending this state; absent means 0
  art_full?: text,       // key of a larger picture, 1 to 64 bytes (§2.3)
  icon?:     text,       // key of the app's icon, 16 lowercase hex digits (§2.2)
  shuffle?:  bool,       // whether it shuffles
  loop?:     text,       // "none" | "track" | "playlist"
}
```

Artwork goes once per `art_key` and session: later states carry the key
alone, and receivers keep pictures by key. A device sends at most 8
players. Receivers drop actions they don't know.

A player offers `shuffle` and `loop` in `actions` only when it can do them,
and then says where it stands in `shuffle` and `loop`. Receivers show those
controls only for players that offer them. Every field after `art` is
optional and ignored by older versions; a receiver drops a `rate`, `loop`,
`icon` or `art_full` it can't use (out of range, unknown or malformed) and
keeps the rest of the player.

### 2.1 Position, rate and age

While `playing`, the position moves on from when the state arrived. The
receiver's position now is

```
position + (age + time since the state arrived) × rate
```

capped at `duration`; while paused it stays at `position`. `age` is the
sender's own time between reading the position from its player and sending
the state (a state sent later, to a device that connects, counts the wait
too), so the two devices' clocks never need to agree. Senders still send only
on a change, never because time passes; a change of rate is a change. A
`rate` outside 0.25 to 4, or one that isn't a finite number, is treated as 1.

### 2.2 App icons

`icon` names the player app's icon. A receiver that shows it asks once per
key with `media.icon` and keeps it by key; the answer is a PNG of at most
16 KiB. The sender answers only for a key one of its current players names:
`NOT_FOUND` for any other (or when it no longer has that icon), `DENIED` when
media is off for the asking device, `UNSUPPORTED` when it doesn't offer
`media.control`, `BAD_MESSAGE` for a key that isn't 16 lowercase hex digits.

### 2.3 Large artwork

`art` stays small, for lists, the mini player and the media notification. When
a sender has a larger picture of the same artwork it names it in `art_full`,
and a receiver that shows a large player fetches it then, on a bidirectional
stream of its own, one picture per stream:

```
stream header  { svc: "media", op: "art", v: 1 }
t = "media.art"       id = 1  b = { key: text }                     // receiver to sender
t = "media.art.meta"  re = 1  b = { length: uint, format: text }    // "jpeg" | "png"
<length bytes>
```

The picture is JPEG or PNG, at most 2 MiB (2097152 bytes), and at most 1200
pixels on its longer side. A receiver refuses a `length` over 2 MiB, or a
format it doesn't know, before reading on, checks that the bytes are the
format given, and keeps large pictures in memory only (a few at a time). The
sender answers only for a key one of its current players names, with the same
errors as `media.icon`, and gives up on a stream that doesn't ask or read
within 20 s. Pictures from either field are never logged.

## 3. Rules

- **No echoes.** A device leaves its own stand-in for another device's
  player (its MPRIS player on Linux, its media session on Android) out of
  what it shares.
- Implementations **MUST NOT** log titles, artists or albums ([protocol v1](v1.md) §11).

## 4. Play on phone

A PC can move what one of its players plays to a phone. How depends on
what plays, and the PC decides:

- **Streaming services that protect their video** (Netflix, Prime Video,
  JioHotstar, Disney+ and the like) come out black when captured. The PC
  sends the page's address with `link.open` ([actions.md](actions.md)),
  which Android hands to the service's app, and pauses its own player.
- **A video file** a desktop player plays (VLC, Media Player and the like)
  is offered as a file: the phone reads it from the PC and plays it
  itself, from where the PC had got to, at its full quality.
- **Everything else** is streamed from the PC as it plays, picture and
  sound, so whatever the PC does to it carries over (a browser's ad
  blocker on YouTube, for one). The PC keeps playing.

Streaming starts with an offer from the PC:

```
t = "media.watch"  id = n  b = { player: text, title?: text, file?: WatchFile }   // PC to phone
t = "ok"           re = n

WatchFile = { name: text, size: uint, position: uint }
```

`player` is the PC's ID for the player, as in `media.state`; `title` is
what it plays, up to 256 characters, to show with the offer. The phone
shows the offer (Android doesn't let an app open a screen from the
background, so usually as a notification) and answers `ok`. Errors:
`UNSUPPORTED` when the phone doesn't offer `media.watch`, `DENIED` when
media is off for the PC, `BAD_MESSAGE` for an empty or too long `player`
or `title`.

To watch, the phone sends `mirror.start` with `watch` set to that
player, `session` 0 and `audio` true ([mirror.md](mirror.md) §2). The PC
allows it only for a player it offered that phone, while the offer is
open: for 10 minutes after the offer or after watching last stopped, so
the phone can come back to it, and for as long as the connection lasts.
It then streams the part of its screen the player's picture shows in (a
browser's video, else its page, else its window) on the mirroring video
stream, and its sound as Phone speaker does. Input from the phone is
ignored while it watches. Anything else in that `mirror.start` is as for
the PC's screen; without an offer the answer is `DENIED`, and the PC
doesn't ask its user to share the screen.

### Playing a file

With `file`, the phone plays the file itself instead of watching a
stream. `name` is the file's name (1 to 256 characters, no `/` or `\`),
for the phone's player to tell the format by; `size` its size in bytes
(more than 0); `position` where the PC had got to, in milliseconds. A
PC offers a file only in a format phones play (MP4, MKV, WebM, MOV, 3GP,
MPEG-TS), and streams anything else. Once the phone plays it, it pauses
the PC's player with `media.command`.

The phone reads the file on bidirectional streams of their own, one read
each:

```
stream header  { svc: "media", op: "file", v: 1 }
t = "media.file"       id = 1  b = { offset: uint, length: uint }   // phone to PC
t = "media.file.meta"  re = 1  b = { size: uint, length: uint }     // PC to phone
<length bytes>
```

The PC answers with at most 4 MiB (`length` is cut to that, and to the
end of the file; past the end it is 0), then the bytes, and finishes the
stream. It serves only the file of its open offer to that phone (the
same 10 minutes as for watching, renewed by every read), so a phone can't
read anything else on the PC; without one the answer is `DENIED`.
A newer offer, streamed or not, replaces it.

## 5. A phone's media over Bluetooth

A phone connected to a PC over Bluetooth (for calls, or by hand) also
opens a media link, and Android then plays everything on the PC instead of
the headphones it was using. Each device has a `bt_media` device toggle for
the other, off by default: on a PC, whether that phone may play its media
there (off, the PC closes the phone's media link as it opens; calls aren't
touched); on a phone, the same for that PC. Unlike other toggles, both
devices keep it the same:

```
t = "media.bt"  b = { on_pc: bool, changed_ms?: u64 }   // either way, media lane, no answer
```

`changed_ms` is when the toggle was changed, in Unix milliseconds. Each
device keeps it with its own toggle: a change the user makes there is
stamped with the device's clock, or 1 ms after the stamp it already has if
that's later (its clock was set back, or the other device's is ahead), so a
change made after one the device knows of always wins.

A device sends `media.bt`:

- when its user changes `bt_media` for a connected device, in the order of
  the changes;
- when a session with a phone (if it's a PC) or a PC (if it's a phone)
  starts, with its current toggle for that device and its stamp, so a
  change made on either while they were apart reaches the other. A device
  whose toggle was never changed (no stamp) sends nothing, so its default
  never replaces a choice made on the other one.

The receiver stores `on_pc` as its own `bt_media` toggle for the sender,
with `changed_ms` as its stamp, only if that's later than the stamp it has
(or it has none); at the same time or earlier, it keeps its own. It never
sends it back, so a change doesn't bounce between the two. When both were
changed while apart, both end up with the later change: last writer wins by
each device's clock, so with clocks far apart the change made on the device
whose clock is ahead can win though it was made first.

Without `changed_ms`, the change counts as made when it arrives. A body that
isn't `{ on_pc: bool, changed_ms?: u64 }`, or a `changed_ms` above 2^63 − 1,
is ignored.

## 6. PC sound

A phone can be the PC's speaker ("Use phone as speaker": the PC starts it
itself, or the phone asks with `deck.audio.set` volume `201`, [deck](deck.md)
§2.4). The PC sends what it plays as 16-bit PCM in QUIC datagrams tagged
`0x02` ([protocol v1](v1.md) §5):

```
0x02 · seq: u16 LE · ts_ms: u32 LE · codec: u8 · channels: u8 · rate: u32 LE · payload
```

`seq` counts up by one per datagram and wraps; `ts_ms` is when the first
sample was captured, in ms since the PC started sending sound to this phone.
`codec` is `0` (16-bit signed little-endian PCM, channels interleaved); a
phone drops any other. The payload is whole frames, at most 960 bytes (5 ms
of 48 kHz stereo).

When a datagram can't carry a piece (the path's datagram size is too small,
or sending fails), the PC opens a media stream with header
`{ svc: "media", op: "sound", v: 1 }` the first time it needs one, and sends
each such piece there as a frame: the datagram without its tag byte. It keeps
at most 24 pieces waiting for that stream and drops the oldest past that.
Sound never goes on control or a lane.

The phone plays sound only while its `media` toggle is on for the PC. It
drops a piece whose `seq` is not after the last it played (counting on
modulo 2^16, less than 2^15 ahead), and uses `ts_ms` to keep its jitter
buffer short.

`ts_ms` is on a clock of its own. The picture Play on phone sends with the
sound is stamped on the PC's monotonic clock in microseconds
([mirror.md](mirror.md) §3), which the phone times with `mirror.clock`; nothing
yet ties the two, so the sound plays as it comes. Putting the sound on the
same clock, for the phone to hold it back to the picture's delay, is planned.

