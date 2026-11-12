# iroh-mdns-address-lookup, patched

This is [iroh-mdns-address-lookup](https://crates.io/crates/iroh-mdns-address-lookup)
0.6.1 by the n0 team (MIT OR Apache-2.0, see `LICENSE-MIT` and
`LICENSE-APACHE`), with two changes, marked "G7 Link patch" in `src/lib.rs`.

## The changes

`MdnsAddressLookupBuilder::multicast_interfaces(include)` makes mDNS send and
receive on every local IPv4 address that `include` accepts, following the
endpoint's addresses as they change (swarm-discovery's per-interface sockets).

Without it, swarm-discovery joins the mDNS group on one interface only, the
one the OS picks for multicast. On a PC with Docker, libvirt or VPN adapters
ranked above Wi-Fi, that's a virtual adapter, so the PC never hears or
answers phones on the LAN.

Publishing endpoint data without user data now removes the `user-data` TXT
attribute (upstream left the old value in place), so G7 Link can stop
announcing a device's name on the LAN without restarting mDNS.

Interfaces that go away are removed with `remove_interface_v4`; the patched
swarm-discovery in `third_party/swarm-discovery` closes their sockets.

The tests in `src/lib.rs` are upstream's and need its dev-dependencies, so
they aren't built here.

## Dropping it

Upstream is working on the same problem
(n0-computer/iroh-address-lookups#7 and #26). Once a release has it, remove
this folder and the `[patch.crates-io]` entry in the workspace `Cargo.toml`,
and switch `install_mdns` in `core/g7link-core/src/node.rs` to the
upstream option.
