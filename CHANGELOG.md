# Changelog

All notable changes to G7 Link. Versions follow
[semantic versioning](https://semver.org); before 1.0, a minor version may
change things that need a step from you, and its notes say so.

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
