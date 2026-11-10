# Changelog

All notable changes to G7 Link. Versions follow
[semantic versioning](https://semver.org); before 1.0, a minor version may
change things that need a step from you, and its notes say so.

## 0.1.6 (unreleased)

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
- **Full screen.** The app opens full screen, and phone and app windows can go
  full screen too.
- **Answer a ringing call from Home.**

### Fixed

- Work for the Qt thread is never run after the code that asked for it has
  given up.
- `install.sh --uninstall` runs the app's own cleanup first.
