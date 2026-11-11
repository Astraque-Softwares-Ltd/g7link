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
t = "elevated.usb.start"   id = n   b = { apk: text, token: text, grants: [text], adb_port?: uint }
t = "elevated.usb.ready"   id = n   b = { port: uint }
```

The phone sends `elevated.usb.start`. The PC:

1. Refuses unless the request is valid (§3) and it finds the phone in adb:
   connected over USB, through that phone's own adb serial, or else, when
   the request has `adb_port`, over the phone's Wireless debugging (§4).
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
- `adb_port`: absent, or a port other than 0.
- `grants`: at most 8, each one of:
  - `pm grant app.g7link android.permission.<NAME>`
  - `appops set app.g7link <OP> allow` (`OP` in capitals and `_`)
  - `cmd notification allow_dnd app.g7link`
  - `cmd notification allow_listener app.g7link/<component>`

## 4. Without a cable

A phone that isn't plugged in can be reached through its Wireless debugging,
when its user has paired the PC there. Its port changes each time Wireless
debugging turns on, so the phone names it: `adb_port` in `elevated.usb.start`.
(adb finds it over mDNS only when built to, which the adb in Debian and Ubuntu
isn't, and a sleeping phone doesn't answer mDNS.)

The phone asks without `adb_port` first, which a PC it is plugged into
answers. If none does and its Wireless debugging is on, it finds the port
(among its own listening ports, or over mDNS before Android 17) and asks again
with it. A PC that gets `adb_port` from a phone with no cable:

1. Starts its adb server (`adb start-server`) if none answers.
2. Tells adb to connect to that port at each address the phone's connection
   comes from, IPv4 first, 3 at most (so not over the relay). adb signs in
   with the PC's own adb key: a phone that hasn't paired the PC refuses it.
3. Before anything else runs there, checks the device is the phone that
   asked: `test -f <apk>` must succeed on it. Android makes up the directories
   in an installed APK's path at random for each install, so no other phone
   has that file.

The PC then goes on from step 2 of §2 on that adb connection. The connection
stays in adb afterwards, as after `adb connect`; the USB tunnel never uses it
(a link through adb over Wi-Fi stutters where a direct one doesn't). A PC from
before `adb_port` ignores it and refuses, as it has no cable to the phone.
