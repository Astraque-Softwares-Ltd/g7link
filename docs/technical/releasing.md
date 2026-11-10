# Releasing

How a version of G7 Link gets from `main` to people's devices, and how the
apps update themselves afterwards.

## What a release is

A GitHub release with these files, built by
[`build.yml`](../../.github/workflows/build.yml):

| File | Built by |
|---|---|
| `g7link_<version>_amd64.deb` | `desktop/packaging/linux/build-deb.sh` |
| `g7link-<version>-1.x86_64.rpm` | `desktop/packaging/linux/build-rpm.sh` |
| `G7Link-<version>-x86_64.AppImage` | `desktop/packaging/linux/build-appimage.sh` |
| `G7Link-<version>-linux-x86_64.tar.gz` | `desktop/packaging/linux/build-tarball.sh` |
| `install.sh` | Copied from `desktop/packaging/linux/install.sh` |
| `G7Link-<version>-android.apk` | The Android job: a release build for 64-bit and 32-bit ARM, signed with the release key and checked with `apksigner verify` |
| `SHA256SUMS.txt` | The `publish` job in `release.yml`, over all the files above |

Before uploading, the Linux job checks that each package starts in clean
Ubuntu 24.04 and Fedora 40 containers, and that `install.sh` installs and
removes each one.

Each app's updater needs its own file and `SHA256SUMS.txt`: it only
accepts a file that `SHA256SUMS.txt` lists with a matching hash. The
desktop app picks the file for the way it was installed (AppImage, `.deb`,
`.rpm` or tarball).

## Steps

1. **Version.** `Cargo.toml` (`[workspace.package] version`) is the only
   place it's set: the desktop app, its packages and the Android app
   (`versionName`, and `versionCode` derived from it) all read it. Use
   `MAJOR.MINOR.PATCH`; a pre-release such as `0.2.0-beta.1` is allowed, and
   `[workspace.metadata.release] name` gives the release a name.
2. **Checks.** A green CI run on the commit to release (the commands are in
   [building and testing](building-and-testing.md#what-ci-runs)).
3. **Tag and push.** `git tag v<version>` on that commit and push the tag.
   The `Release` workflow refuses a tag that doesn't match `Cargo.toml`. It
   builds with `build.yml`, the same workflow nightlies use.
4. **Review the draft.** The workflow creates a draft release. Its notes are
   the version's section of `CHANGELOG.md`, or GitHub's generated notes when
   there is none. Edit the notes (what's new, anything users must do), check
   the files are there, then **Publish**. Publishing runs `channels.yml`,
   which points the stable feed at the release; installed copies on Stable
   see it within half a day, sooner when a paired device tells them.

## Channels and nightlies

Installs follow a channel, picked in Settings: **Stable** (releases) or
**Nightly**. Each channel has a feed, a small JSON file on the `channels`
branch (`stable.json`, `nightly.json`), with the newest build's version,
name, notes page, and each file's download address and SHA-256
(`.github/scripts/write-feed.py`; the format is in
`desktop/app/src/feed.rs`). The apps read it with a conditional request, so
an unchanged feed costs a few hundred bytes; until a feed exists they fall
back to GitHub's latest release. A Nightly install reads both feeds, so it
takes a release that's newer than its nightly.

`nightly.yml` runs only when started by hand (Actions, or
`gh workflow run Nightly`). When `main` changed (more than docs) since
the last nightly and CI passed on it, it builds `<next>-nightly.N` with
`build.yml`, publishes it as a pre-release named for a flower
(`nightly-name.py`, "Golden Lavender"), points the nightly feed at it and keeps
the last six nightlies. `<next>` is the version in `Cargo.toml`, or the next
patch when that's already released; `set-version.sh` sets it for the build.
Run it by hand from the Actions tab (with **force** to build even if nothing
changed).

Version order: `1.2.3-nightly.N` comes after `1.2.2` and before `1.2.3`, and
`nightly.10` after `nightly.9`. Android's `versionCode` is
`major·10⁷ + minor·10⁵ + patch·10³ + build`, where `build` is N for a nightly
and 999 for a release. Linux packages use `1.2.3~nightly.N`, which apt and
dnf sort before `1.2.3`. So a nightly install moves to the release when it's out, and
a phone can't go from a nightly back to an older release without
reinstalling.

## Signing

**Android.** Release builds are signed with the key in the
`ANDROID_KEYSTORE` (base64 of the `.jks`) and `ANDROID_KEYSTORE_PASSWORD`
repository secrets; the key's alias is `g7link`. Android only installs an
update signed with the same key as the installed app, so:

- Keep an offline backup of the keystore and its password. If the key is
  lost, no existing install can ever be updated again; everyone would have to
  uninstall and reinstall.
- Builds signed with the debug key (local `assembleRelease` without
  `G7LINK_KEYSTORE`) can't update to or from official releases.
- To sign locally: set `G7LINK_KEYSTORE` to the `.jks` path and
  `G7LINK_KEYSTORE_PASSWORD`, then `./gradlew assembleRelease`.

**Linux.** The packages aren't signed. Their integrity rests on HTTPS from
GitHub and the hashes in `SHA256SUMS.txt`, which `install.sh` and the
desktop updater both check.

## How the apps update

- **Linux desktop** (`desktop/app/src/updater.rs`): an installed copy
  checks its channel's feed 30 seconds after start, then every 12 hours on
  Stable and every 2 hours on Nightly (twice as long on battery), and sooner
  when a phone tells it of a build (`update_relay.rs`, `event.update`); it
  tells its phones when it sees one. **Update** downloads the file for this
  kind of install as the user, resuming after a dropped connection, and
  checks its SHA-256. An AppImage replaces itself. A `.deb`, an `.rpm`, or a
  tarball under `/opt` or `/usr` installs through `pkexec`, which asks for
  the administrator password; the file is copied somewhere only root can
  write and checked again before installing. A tarball in the user's own
  folders installs without it. A small relauncher then opens the new
  version. Details and the security reasoning:
  [security model](security-model.md#automatic-updates-and-release-verification).
- **Android** (`update/AppUpdater.kt`): hears of new builds from its PC
  (`onHint`), and checks on its own daily on Stable or every six hours on
  Nightly, and when the app opens; installs through `PackageInstaller`.
  Android 12 and later skip the confirmation once G7 Link installed the
  running version itself; the first update after a manual install asks once.

## Testing an update before publishing

Updaters only look at the latest published release, so test with two
versions on a test PC and phone: install version N from its draft's files,
publish N+1, and check that both apps update (the desktop log in
`~/.local/share/G7Link/logs`; `adb logcat -s AppUpdater` on the phone).
