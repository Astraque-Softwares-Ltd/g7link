// SPDX-License-Identifier: MPL-2.0
//! G7 Agents: a host that runs the user's own coding-agent CLIs (Claude and
//! Codex) in folders the user listed, and serves their threads to the
//! launcher, G7 Link and the phone over a private local socket.
//!
//! [`api`] holds the wire types and [`client`] connects to the host; both
//! build without the `host` feature. The host itself is [`host`].

pub mod api;
pub mod client;
#[cfg(feature = "host")]
pub mod host;
mod lines;
