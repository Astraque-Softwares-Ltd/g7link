# Capability registry

Capability IDs a device may announce in `hello.caps` and `hello.update`
([protocol v1](v1.md) §6 and §7). An ID's prefix is its service's name. A device announces a capability only when it can provide it
**right now**: with its current power level, permissions, add-ons and OS
version.

IDs are lowercase dotted strings (`[a-z0-9._-]`, at most 64 bytes). Receivers
ignore malformed IDs, IDs they don't know, and anything beyond 256 entries.
New IDs may be added in any release; an ID is never reused with a different
meaning.

The capability matrix (`docs/architecture/capabilities.md`) turns these into
feature states. "Unlocked by" is what the matrix offers the user when the
capability is missing.

## Both devices

| ID | Meaning | Unlocked by |
|---|---|---|
| `core.ping` | Answers `ping` | always offered |
| `device.battery` | Sends `device.battery` | always offered |
| `device.ring` | Rings on `device.ring`, even on silent | always offered |
| `update.hint` | Sends and takes `device.update` (new G7 Link builds) | always offered |
| `files.transfer` | Sends and receives files ([files service](files.md)) | app update |
| `link.open` | Opens web links sent to it ([actions](actions.md)) | app update |
| `task.notify` | Sends or shows watched PC task notifications ([actions](actions.md)) | app update |
| `clip.image` | Accepts images on its clipboard ([clipboard service](clipboard.md)) | app update |
| `media.control` | Shares its media players for remote control, and serves their app icons and large artwork on request ([media service](media.md); phone: needs notification access) | phone: notification access · desktop: app update |
| `media.remote` | Shows and controls other devices' media players | app update |
| `ble.link` | Opens the Bluetooth channel with a paired phone or PC, which also carries the typing box and holds (`ble.use`); a phone picks a path for its touchpad ([Bluetooth](bluetooth.md) §4) | phone: Nearby devices permission, Android 12+ · desktop: an adapter that serves GATT |
| `agent.control` | Coding agents' controls ([agents](agents.md) §5): the phone picks the model and effort, plans, steers and queues messages, answers the agent's questions, sends pictures, finds files for `@` and commands for `/`, and keeps drafts; the PC takes all of that. Used only while both offer it; to a phone without it a PC sends what it sent before (plan mode as supervised, a plan as an answer, a question as a tool step) | phone: app update · desktop: an agent host that offers its `control` capability |

> **LocalSend v2.1 LAN interop:** Unpaired LocalSend discovery and file transfer
> (`224.0.0.167:53317` UDP multicast + HTTP/HTTPS `/api/localsend/v2/*` on TCP
> port `53317`, see [protocol §15](v1.md#15-localsend-v21-interoperability-optional-lan-interop))
> is an opt-in local setting (`localsend.enabled`) rather than a paired-session
> capability, because it operates with unpaired third-party LocalSend devices.

## Phone

| ID | Meaning | Unlocked by |
|---|---|---|
| `notify.mirror` | Forwards notifications ([notifications service](notifications.md)) | notification access |
| `notify.live` | Includes live progress, timer, and Android 16 Live Update metadata on mirrored notifications ([notifications service](notifications.md)) | notification access |
| `notify.reply` | Accepts inline replies and actions on notifications | notification access |
| `notify.sensitive` | Forwards notifications Android hides as sensitive (e.g. one-time codes) | Elevated |
| `notify.show` | Shows a paired phone's or tablet's notifications ([notifications service](notifications.md)) | app update |
| `sms.read` | Lists conversations and messages ([messages](sms.md)) | SMS permission |
| `sms.send` | Sends texts when a PC asks ([messages](sms.md)) | SMS permission |
| `call.state` | Reports incoming and ongoing calls ([calls](calls.md)) | phone permission |
| `call.control` | Answers, declines and silences calls when a PC asks ([calls](calls.md)) | phone permission |
| `call.incall` | Mutes, holds and presses keys on the call in progress ([calls](calls.md)) | Elevated, Android 12+ |
| `call.log` | Lists recent calls ([calls](calls.md)) | phone permission |
| `call.dial` | Places a call or opens the dialer when a PC asks ([calls](calls.md)) | phone permission |
| `contacts.read` | Lists and searches contacts ([contacts](contacts.md)) | contacts permission |
| `clip.write` | Accepts clipboard content from the PC ([clipboard service](clipboard.md)) | app update |
| `clip.share` | Sends the clipboard when the user taps "Send" | app update |
| `clip.read.auto` | Sends clipboard changes automatically, in the background | Elevated |
| `photos.read` | Announces new photos and screenshots, lists albums and items, serves thumbnails, and sends files on request ([photos](photos.md)) | photos permission |
| `elevated.usb` | Starts a phone's Elevated helper over adb: a phone on a USB cable, or one whose Wireless debugging has paired the PC ([Elevated over USB](elevated-usb.md)) | adb on the PC |
| `mirror.capture` | Streams its screen (with the system's capture prompt at Basic; [mirroring](mirror.md)) | app update |
| `mirror.decode.hevc` | Decodes H.265 screen-mirroring streams | app update |
| `mirror.input` | Accepts touch, key and scroll input while mirrored | Assist |
| `mirror.virtual_display` | Runs apps on displays of their own, shown in windows on the PC ([mirroring](mirror.md)) | Elevated, Android 11+ |
| `mirror.audio` | Streams all audio while mirrored | Elevated |
| `mirror.audio.playback` | Streams audio from apps that allow playback capture | app update |
| `mirror.sound` | Streams its sound on its own, without the screen ([mirroring](mirror.md) §5) | app update |
| `toggles.read` | Shares quick settings state ([toggles](toggles.md)) | app update |
| `toggles.ringer` | Changes the ringer mode between `ring` and `vibrate` ([toggles](toggles.md)) | app update |
| `toggles.volume` | Changes the media volume ([toggles](toggles.md)) | app update |
| `toggles.flashlight` | Turns the flashlight on or off ([toggles](toggles.md)) | hardware flash unit |
| `toggles.dnd` | Turns Do Not Disturb on or off and sets the ringer to `silent` ([toggles](toggles.md)) | Do Not Disturb access or Elevated |
| `toggles.brightness` | Changes the screen brightness ([toggles](toggles.md)) | Modify system settings or Elevated |
| `toggles.wifi` | Turns Wi-Fi on or off ([toggles](toggles.md)) | Elevated |
| `toggles.bluetooth` | Turns Bluetooth on or off ([toggles](toggles.md)) | Elevated |
| `storage.read` | Lists folders and reads files in shared storage or picked SAF folders ([storage](storage.md)) | All files access or picked SAF folder |
| `storage.write` | Creates folders, writes/uploads files, renames and deletes entries in shared storage or picked SAF folders ([storage](storage.md)) | All files access or picked SAF folder |
| `webcam.stream` | Streams a camera for webcam use ([webcam](webcam.md)) | camera permission |
| `mic.stream` | Streams its microphone when its user agrees ([mic](mic.md)) | app update (asks for the microphone permission when first used) |
| `media.watch` | Shows a PC's playback streamed to it (Play on phone, [media](media.md) §4) | app update |
| `camera.capture` | Captures a photo or document scan on request ([Continuity Camera](v1.md#14-continuity-camera-cameracapture)) | camera permission |
| `ble.beacon2` | Recognises a PC's rotating Bluetooth beacon; while a paired phone lacks it, the PC also broadcasts the old one ([Bluetooth](bluetooth.md) §3) | Nearby devices permission |
| `agent.remote` | Shows a PC's coding-agent threads, starts them and answers their requests, says which one it shows ([agents](agents.md) §7), takes threads handed to it ([agents](agents.md) §10), and shows a thread beside its PC and opens its dev servers ([agents](agents.md) §11) | app update |
| `agent.follow` | Shows a PC's Claude and Codex sessions started outside G7 Link, read-only, and continues one as a thread ([agents](agents.md) §3) | app update |

## Desktop

| ID | Meaning | Unlocked by |
|---|---|---|
| `pc.power` | Locks or sleeps on request ([actions](actions.md)) | app update |
| `pc.wake` | Shares network adapter addresses (`pc.wake.info`) so a paired phone can wake it with Wake-on-LAN ([actions](actions.md)) | app update |
| `pc.capture` | Takes screenshots and records its screen for a paired phone ([screenshots and recordings](pc-capture.md)) | the ScreenCast portal and PipeWire (Wayland) |
| `photos.show` | Shows a phone's new photos and gallery ([photos](photos.md)) | app update |
| `call.show` | Shows a phone's calls, recent calls and dialer ([calls](calls.md)) | app update |
| `contacts.show` | Shows a phone's contacts ([contacts](contacts.md)) | app update |
| `sms.show` | Shows a phone's text messages ([messages](sms.md)) | app update |
| `toggles.show` | Shows a phone's quick settings and sends toggle changes ([toggles](toggles.md)) | app update |
| `mirror.view` | Shows a phone's screen ([mirroring](mirror.md)) | app update |
| `mirror.listen` | Plays a mirrored phone's sound ([mirroring](mirror.md)) | app update |
| `files.browse` | Lets a paired phone browse and fetch its files | app update |
| `storage.mount` | Mounts a paired phone's shared storage in the file manager ([storage](storage.md)) | app update |
| `input.inject` | Accepts pointer, keyboard and presentation input ([input](remote.md)) | app update |
| `input.text_mirror` | Takes `Edit` and `Caret` input and tells watching phones about its focused text field (`input.text_watch`, `input.text_state`; [input](remote.md) §2.4) | app update |
| `input.app_icon` | Names the focused app's icon in its text states and serves it (`input.app_icon`; [input](remote.md) §2.4) | app update |
| `recorder` | Saves and converts voice recordings from a phone ([recorder](recorder.md)) | app update |
| `deck.actions` | Shares its Deck layout and live tile state and runs Deck actions ([deck](deck.md)) | app update |
| `deck.audio` | Reports active audio output devices in `deck.state` and accepts `deck.audio.set` volume, mute and output changes ([deck](deck.md)) | app update |
| `webcam.view` | Receives and decodes a phone's camera stream ([webcam](webcam.md)) | app update |
| `webcam.view.hevc` | Also takes H.265/HEVC webcam video; without it, H.264 only ([webcam](webcam.md)) | a hardware HEVC decoder |
| `mic.listen` | Takes a phone's microphone and plays it into an output ([mic](mic.md)) | app update |
| `media.stream` | Streams a player's picture and sound to a phone that watches (Play on phone, [media](media.md) §4) | app update |
| `media.sound` | Sends its sound to a phone that plays it, in `0x02` datagrams or on a `media/sound` stream ([media](media.md) §6) | app update |
| `webcam.addon` | The virtual camera add-on is installed ([webcam](webcam.md)) | installing the add-on |
| `camera.continuity` | Requests and receives Continuity Camera photos and document scans ([Continuity Camera](v1.md#14-continuity-camera-cameracapture)) | app update |
| `agent.host` | The agent host is installed: paired phones with the `agents` toggle can follow, start and answer coding-agent threads on this PC, hear whether someone is at it ([agents](agents.md) §7), hand threads over both ways ([agents](agents.md) §10), switch what a phone shows beside the PC and see the ports a thread's agent listens on ([agents](agents.md) §11), and see and commit a thread's changes; with the `agent_push` toggle too, push and open pull requests ([agents](agents.md) §8) | installing G7 Link with its agent host |
| `agent.sessions` | Lists Claude and Codex sessions started on this PC outside G7 Link to paired phones with the `agents` and `agent_sessions` toggles ([agents](agents.md) §3) | installing G7 Link with its agent host |
| `agent.say_why` | Takes a denial with the user's words for the agent (`agent.answer`'s `message`, [agents](agents.md) §2.4) | an agent host that can pass the words on |
| `agent.reply` | Enters a reply from a paired phone with the `agents`, `agent_sessions` and `agent_reply` toggles into a Claude or Codex session started on this PC: it brings the session's own window forward, proves it shows that session, pastes and presses Enter ([agents](agents.md) §9). Offered only while it can do it now | GNOME with the G7 Linux Companion 1.4 or later, and the input device |

## Retired in v1

| ID | Why |
|---|---|
| `camera.stream`, `camera.virtual`, `camera.virtual.hevc`, `addon.vcam` | renamed `webcam.stream`, `webcam.view`, `webcam.view.hevc`, `webcam.addon` |
| `pc.audio` | renamed `deck.audio` |
| `agent.items2` | every v1 phone understands reasoning items and their fields ([agents](agents.md) §2.4) |
| `mirror.pace` | every v1 device reports video packets and paces by those reports ([mirroring](mirror.md) §3) |
| `ble.remote` | folded into `ble.link` |

A device stores a paired peer's capabilities and replaces them at the peer's
next `hello`, so old IDs stored for a peer go away when it next connects.

Feature IDs of the capability matrix (`input.deck`, `media.pc_audio`,
`camera.webcam`, …) are not capabilities: they name what the UI offers, and
are built from the capabilities above (`docs/architecture/capabilities.md`).
