# ADR 0008: Platforms and distribution

- **Status:** Accepted
- **Date:** 2026-10-05
- **Updated:** 2026-11-10, for the Linux-only desktop

## Context

Focus beats breadth. G7 Link is the phone companion for G7 Linux, and store policies (Play) restrict SMS, accessibility and file access.

## Decision

The Linux desktop (x86_64) and Android 8+. Distribute GitHub-first: GitHub Releases on Linux (`.deb`, `.rpm`, AppImage and a tarball, with `install.sh` to pick one), GitHub Releases + Obtainium (later IzzyOnDroid) on Android.

## Consequences

- The desktop is built on Linux interfaces: D-Bus, XDG portals, PipeWire and PulseAudio, BlueZ, `v4l2loopback` and `/dev/uinput`. Desktops and compositors differ, so features check what's available instead of assuming it.
- No Play policy constraints in the `full` flavor; a `play` flavor seam is kept.
- Android developer verification must be completed before global enforcement (2027).
