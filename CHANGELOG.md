# Changelog

All notable changes to G7 Link. Versions follow
[semantic versioning](https://semver.org); before 1.0, a minor version may
change things that need a step from you, and its notes say so.

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
