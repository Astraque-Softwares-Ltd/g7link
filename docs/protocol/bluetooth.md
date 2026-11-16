# Bluetooth (protocol v0)

> Status: **draft.** Builds on [protocol v0](v0.md); breaking changes are
> allowed until v1. License: CC BY 4.0.

A paired phone and PC use Bluetooth LE for two things: the phone recognises
the PC's beacon nearby (to reconnect at once), and, when the user turns it on
for that pair, the two open a small encrypted channel when they share no
network. Sessions themselves never run over Bluetooth.

## 1. Capabilities and toggles

| ID | Offered by | Meaning |
|---|---|---|
| `ble.beacon2` | phone | Recognises the rotating beacon (§3). While a paired phone lacks it, the PC also broadcasts the old one (§3.4). |
| `ble.link` | phone, PC | Can open the channel (§4): the phone has the Nearby devices permission, the PC an adapter that serves GATT. |

The device toggle `ble_link` (off by default) lets a paired device use the
channel. Each device checks its own toggle for the other.

## 2. The link secret (`ble.key`)

On every session with a paired phone, the PC sends:

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
for the phones wanted; a phone that sees its tag connects. The link ends 10 s
after the phone's session is back.

### 4.2 Framing

Each value is one fragment: a header byte (bit 7 first, bit 6 last, bits 0-5
a sequence number per direction), on the first fragment the record's length
(2 bytes, big endian), then the record's bytes. Records are at most 4112
bytes, 256 before the handshake ends. Anything out of sequence, too long or
short ends the link.

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

Envelopes are as on a session, at most 4096 bytes, 20 records then 2 a
second from the other side (`BUSY` past that). Only these are carried, each
also needing the receiver's `ble_link` toggle:

| Type | Direction |
|---|---|
| `ping` / `pong` | both |
| `device.ring` → `ok`, `device.ring_stopped` | PC → phone, phone → PC |

Anything else is answered `UNSUPPORTED`.
