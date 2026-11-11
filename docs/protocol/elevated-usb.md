# Elevated over USB (and the PC's wireless adb)

> Status: draft (v0). Part of [the protocol](v0.md). Breaking changes are
> allowed until v1. License: CC BY 4.0.

Elevated runs a small helper on the phone as Android's shell user
(`app_process`, from the app's own APK). The phone normally starts it through
its own wireless debugging. A phone plugged into a PC with USB debugging
allowed can instead ask that PC to start it, the way scrcpy starts its server.
So can a phone with no cable, when its Wireless debugging has paired that PC
(the PC is listed under **Paired devices** there) and both are on one network.

## 1. Capability

| Capability | Offered by | Meaning |
|---|---|---|
| `elevated.usb` | PC | Can start a phone's helper over adb |

A PC offers it while it watches adb for USB phones. The name is from when a
cable was the only way; the messages are the same over wireless adb.

## 2. Messages

```
t = "elevated.usb.start"   id = n   b = { apk: text, token: text, grants: [text] }
t = "elevated.usb.ready"   id = n   b = { port: uint }
```

The phone sends `elevated.usb.start`. The PC:

1. Refuses unless the request is valid (§3) and it finds the phone in adb:
   connected over USB, through that phone's own adb serial, or else over the
   phone's Wireless debugging (§4).
2. Runs each grant in the phone's shell, waiting up to 10 seconds for each.
3. Runs `CLASSPATH=<apk> app_process /system/bin
   app.g7link.android.elevated.InputServer <token>` and waits up to 15
   seconds for its first line, `ready <port>`.
4. Answers `elevated.usb.ready` with that port, or an error.

The phone connects to the helper on `127.0.0.1:<port>` with the token, as when
it starts the helper itself. The helper runs while the PC holds its adb shell:
until the phone asks again, the USB tunnel ends (or, without a cable, the
network or Wireless debugging goes), or the PC's node stops.

## 3. What the PC accepts

The PC runs nothing but the app's own helper and grants, and only words the
shell reads as single words (letters, digits and `._/=~+-:`):

- `apk`: a path under `/data/app/` ending in `/base.apk`, containing
  `app.g7link`, without `..`, at most 512 bytes.
- `token`: 32 to 64 hexadecimal characters.
- `grants`: at most 8, each one of:
  - `pm grant app.g7link android.permission.<NAME>`
  - `appops set app.g7link <OP> allow` (`OP` in capitals and `_`)
  - `cmd notification allow_dnd app.g7link`
  - `cmd notification allow_listener app.g7link/<component>`

## 4. Without a cable

A phone that isn't plugged in is looked for over its Wireless debugging:

1. The PC starts its adb server (`adb start-server`) if none answers.
2. A device adb already reaches over the network (`adb connect`) is used if it
   is the phone that asked (below).
3. Otherwise the PC asks the network for `_adb-tls-connect._tcp.local` over
   mDNS for up to 3 seconds, from the address it reaches the phone from, and
   tells adb to connect to what answers (the phone's own address first, 4
   devices at most). adb signs in with the PC's own adb key: a phone that
   hasn't paired the PC refuses it. The PC asks itself because adb only looks
   when it was built with mDNS, and the adb in Debian and Ubuntu isn't.
4. Before anything else runs there, the PC checks the device is the phone that
   asked: `test -f <apk>` must succeed on it. Android makes up the directories
   in an installed APK's path at random for each install, so no other phone
   has that file.

The PC then goes on from step 2 of §2 on that adb connection. The connection
stays in adb afterwards, as after `adb connect`; the USB tunnel never uses it
(a link through adb over Wi-Fi stutters where a direct one doesn't).
