# Bluetooth (protocol v1)

> Status: **v1.** Builds on [protocol v1](v1.md). License: CC BY 4.0.

A paired phone and PC use Bluetooth LE for two things: the phone recognises
the PC's beacon nearby (to reconnect at once), and, when the user turns it on
for that pair, the two open a small encrypted channel when they share no
network, or while the phone's touchpad uses it (§4.5). Sessions themselves
never run over Bluetooth.

## 1. Capabilities and toggles

| ID | Offered by | Meaning |
|---|---|---|
| `ble.beacon2` | phone | Recognises the rotating beacon (§3). While a paired phone lacks it, the PC also broadcasts the old one (§3.4). |
| `ble.link` | phone, PC | Can open the channel (§4): the phone has the Nearby devices permission, the PC an adapter that serves GATT. The channel carries the typing box (§4.4) and holds (§4.5), and a phone picks a path for its touchpad (§4.6). |

The device toggle `ble_link` (off by default) lets a paired device use the
channel. Each device checks its own toggle for the other.

## 2. The link secret (`ble.key`)

On every session with a paired phone, the PC sends on control
([protocol v1](v1.md) §4.1):

```
ble.key { secret: bytes(32), bt_addr?: "AA:BB:CC:DD:EE:FF" }
```

The PC makes the secret once per phone (random) and both keep it sealed with
the pairing; it goes when either unpairs. Sending it again is harmless and
gives it back to a phone that lost it. Only a PC sends it, only to a phone.

## 3. The rotating beacon

### 3.1 Tags

```
beacon_key   = HMAC-SHA256(secret, "g7link ble beacon v2")
epoch        = floor(unix_seconds / 900)
tag(kind, e) = HMAC-SHA256(beacon_key, [kind] || be_u64(e))[0..6]
kind         = 'P' (0x50) presence, 'C' (0x43) connect (§4)
```

Test vectors, for `secret = 00 01 .. 1f`: `beacon_key = e76f53a6…0e5f20`,
`tag('P', 1900000) = 01340facca29`, `tag('P', 1900001) = f236dc484abb`,
`tag('C', 1900000) = adac05998b8d`, `tag('C', 1900001) = 26311d70bd72`
(pinned in `core/g7link-protocol/src/ble.rs` and the Android tests).

### 3.2 Payload

Manufacturer data, company `0xFFFF`: `47 02` then six-byte slots, four in a
non-connectable beacon and three in a connectable advertisement (BlueZ adds
flags). One slot per phone, the rest random, in random order. More phones than
slots take turns of 20 s. With no phone to broadcast for, there is no beacon.

### 3.3 Rotation and matching

The PC registers the beacon again for each window, up to a minute late (a
random delay per run), which gives it a new payload and a new random address
together. A phone accepts the window before and after its own (clock skew)
and tries each paired PC's key. A tag proves who made it, not that the PC is
near: a recorded one can be replayed elsewhere for up to 45 minutes, so a
phone reconnects at most once per PC every 30 s.

### 3.4 The old beacon

`NCTR\x01` and the first 8 bytes of the PC's device ID, at the controller's
default interval, only while a paired phone lacks `ble.beacon2`.

## 4. The channel

### 4.1 GATT

```
Service  bcac09a4-522f-43c6-9533-b30bd7428154
  RX     7f2964e4-c2df-4b2e-8425-68d2be4c85ab   write (with response)   phone → PC
  TX     ea1df14d-6860-4376-a3b0-64fdcb79d7f2   notify                  PC → phone
```

The PC serves it while a phone may use the channel, but can be connected to
only in windows: a connectable advertisement shows the PC's public address. A
window (30 s when the PC has something for a phone, 45 s after a phone's
session ends or at start, those two at most once a minute) carries `C` tags
for the phones wanted; a phone that sees its tag connects. A window asked for
while another is open joins it (at most three phones). A window closes when a
link comes up. The PC serves one link at a time. The link ends 10 s after the
phone's session is back, unless the phone holds it (§4.5).

Ending a link ends only its LE connection. Where the phone's device object in
BlueZ is its own (an LE address BlueZ can't resolve), the PC disconnects the
device; where it also carries other connections (a paired phone, known over
BR/EDR, whose classic audio may be up), it uses `Bearer.LE1.Disconnect` where
BlueZ offers it, and otherwise sends the empty value (§4.2), never
`Device1.Disconnect`.

### 4.2 Framing

Each value is one fragment: a header byte (bit 7 first, bit 6 last, bits 0-5
a sequence number per direction), on the first fragment the record's length
(2 bytes, big endian), then the record's bytes. Records are at most 4112
bytes, 256 before the handshake ends. Anything out of sequence, too long or
short ends the link. An empty value ends it too, which a PC uses to have the
phone hang up when it can't end only the LE connection itself (§4.1); a phone
then disconnects. A message whose record wouldn't fit, or for which the
sender has no room, is refused before it's sealed: a sealed record that isn't
sent would leave the counters apart.

### 4.3 Handshake ("G7BLE/1")

```
msg1  phone→PC  01 || e_I(32) || HMAC-SHA256(secret, "g7ble hint" || e_I)[0..8]
th  = SHA-256("G7BLE/1" || e_I || hint || e_R || id_phone || id_pc)
keys = HKDF-SHA256(salt = secret, ikm = X25519(e_I, e_R), info = "G7BLE/1 keys" || th, 64)
       phone→PC = keys[0..32], PC→phone = keys[32..64], ChaCha20-Poly1305
msg2  PC→phone  02 || e_R(32) || seal(PC→phone, n = 0, sign_pc("G7BLE/1 responder" || th))
msg3  phone→PC  03 || seal(phone→PC, n = 0, sign_phone("G7BLE/1 initiator" || th))
record          10 || seal(direction key, n = counter from 1, CBOR envelope)
```

Signatures are the devices' Ed25519 identity keys. The PC finds the phone by
the hint, without a stable identifier on the air. Counters must follow each
other exactly; nonces are four zero bytes and the counter, big endian.

### 4.4 Messages

Envelopes are as on a session's control stream, at most 4095 bytes: there
are no lanes or RPC streams here, so every request carries an `id` and every
answer a `re`, whatever lane the message has in a session. Only these are
carried, each also needing the receiver's `ble_link` toggle:

| Type | Direction |
|---|---|
| `ping` / `pong` | both |
| `device.ring` → `ok`, `device.ring_stopped` | PC → phone, phone → PC |
| `clip.set` → `ok` (plain text, at most 2048 bytes, needs `clipboard`) | both |
| `ble.otp { code, source? }` (needs `notifications`) | phone → PC |
| `toggles.set` → `ok` (`hotspot` only, needs `toggles`), `toggles.state` | PC → phone, phone → PC |
| `input.check` → `ok` (an RPC in a session), `input.event` (needs `remote_input`) | phone → PC |
| `input.text_watch` (needs `remote_input`) | phone → PC |
| `input.text_state`, `input.text_focus` (needs `remote_input`) | PC → phone |
| `ble.use` → `ble.use` (§4.5) | phone → PC |

Anything else is answered `UNSUPPORTED`; `input.app_icon` isn't carried (the
phone shows the icon it has, or the app's name). Each side holds what the
other sends to three budgets, each a burst and then a rate:

| Budget | Messages | Burst, then a second |
|---|---|---|
| records | everything not below | 20, 2 |
| input | `input.event` with an `id`, `input.text_watch`, `ble.use`, `ping` | 60, 25 |
| states | `input.text_state`, `input.text_focus` | 20, 12 |

Past a budget a request is answered `BUSY` and a one-way message dropped.
Replies aren't counted: they come as fast as the side that asked sends. A
`input.event` without an `id` is pointer motion, a scroll or the laser: it
has no reply, is held to the PC's datagram limit instead, and the phone adds
motion up while the link is busy rather than queueing it. Copied text that
doesn't go over the link waits in the send queue for the network.

Text states over the link are cut to 2048 bytes before the caret and 512
after it (512 and 128 while the link's ATT MTU is under 185), with `start`
and `more_before`/`more_after` saying so (remote.md §2.4). The newest waiting
state replaces an older one: a slow link carries fewer, newer states, after
any replies queued. The PC sends each state on the path the phone's last
`input.text_watch` with `on` came on, or on the other while that one is
down; a phone sends `text_watch` with `on` again on its new path whenever its
touchpad's path changes.

### 4.5 Holding the link (`ble.use`)

```
t = "ble.use"  id = n  b = { on: bool, ask?: bool }        phone → PC, session control or link
t = "ble.use"  re = n  b = { state: text, lease?: u32, window?: u32, retry?: u32 }
state = "linked" | "open" | "asking" | "off" | "unavailable" | "busy"
```

`lease` (with `linked` and `open`) is in seconds. With `open`, a PC also says
for how many seconds its connect window for the phone stays open (`window`,
0 when there's none now) and, with none, in how many seconds a renewal can
open one again (`retry`).

While its touchpad shows and its route is Auto or Bluetooth, a phone asks the
PC to keep a link with it, and renews that every 20 s; a hold that isn't
renewed ends 45 s after the last renewal (the `lease`). Unknown `state` values
read as `off`. Only a phone sends it, only to a PC that offers `ble.link`;
a PC answers:

- `off` to `on: false` (the hold ends), and while its `ble_link` toggle is off
  for the phone and it isn't asking its user.
- `asking` while its toggle is off and it asks its user: a phone's `ask: true`
  raises the PC's prompt at most once a minute. The phone asks again with
  `ask: false` every 1.5 s, for a minute at most, until it's `open`,
  `linked` or `off`. "Not now" isn't kept: the next answer is `off`.
- `unavailable` when it has no adapter that serves GATT (or Bluetooth is off
  there), or the phone hasn't announced `ble.link` or has no link secret.
- `busy` while another phone's link is up.
- `linked` (held) when the phone's link is up, `open` (held) otherwise: the
  PC opens a window (§4.1) for the phone, 30 s long, unless the one its hold
  opened last is still open (the request joins it, and `window` says how long
  it has left) or the hold has opened its four (`window: 0`, below).

While a phone holds the link, it isn't closed when the phone's session comes
back; the 10 s rule applies again once the hold ends. When a held link drops,
the PC opens a window again. A hold opens at most four windows in any five
minutes per phone (two minutes of advertising in five at worst): renewing
every 20 s with no link, that is a window every 40 s for two and a half
minutes. Past that, only a `ble.use` with `ask: true` (the phone's user
asked) opens one; the others are answered `open` with `window: 0` and
`retry`, the seconds until the oldest of the four is five minutes old.

A phone looks for the window only when there is one: not after `window: 0`.
It says "out of range" only when the window was open (or the PC didn't say)
and no advertisement with its tag was seen; when the PC was seen but the link
didn't come up, it says it couldn't connect. While its touchpad shows it
renews the hold every 20 s, but after link tries that came to nothing it
looks for the window again only after 20 s, 40 s, 80 s and then every five
minutes, until a link comes up or its user tries again.

### 4.6 The touchpad's path (phone)

A phone keeps a route per PC: Auto (the default), Network or Bluetooth.
Pinned to one, input takes that path while it's up and the other while it
isn't. Auto measures both every 2 s while a touchpad shows (a ping each, and
the round trip of each click or key) and moves to the other path when it
scores better three times in a row (score = 0.7 × median + 0.3 × max of the
last eight; better is at most 0.7 of the one in use, and 15 ms less), at most
once in 20 s. A path changes only when nothing would notice: no input waiting
for its answer, no button held, no motion for 150 ms (and for Auto, no key or
click for 300 ms); a path that goes down is left at once. A key or click on
the link with no answer for 1.5 s, or two pings lost, counts the link as down
for 10 s; the input isn't sent again on the session, as it may have run.
Discrete input goes one at a time whatever path each takes, so the PC runs it
in order.
