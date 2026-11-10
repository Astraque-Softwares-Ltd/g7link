# Connecting over Tailscale

## Why it failed
Pairing links already carry the PC's tailnet address (`pairing.rs`, `link_addrs`
keeps `100.x`). But with Connect over mobile data (away mode) off, every address
filter in the core used `session::is_lan_ip`, which rejected `100.64.0.0/10`
as carrier CGNAT. So:

- at node start and when away mode is turned off, stored tailnet addresses were
  purged from each peer's `last_addrs` (`node.rs`, `Node::start`, `set_away_mode`);
- `remember_addrs` never fed them to the dialer, and `direct_addrs` never
  advertised them;
- a session that did arrive over a tailnet path was refused
  (`is_connection_allowed` → "refusing session on non-LAN path").

mDNS doesn't cross Tailscale (no multicast), so with the address dropped there
was nothing to dial. Tailscale's IPv6 range (`fd7a:115c:a1e0::/48`) is already a
unique-local address and passed; most setups only route IPv4 by default.
On Linux, the firewall rules `install.sh` adds (ufw or firewalld) allow only
private ranges (`192.168.0.0/16`, `10.0.0.0/8`, `172.16.0.0/12`,
`fe80::/10`), not the tailnet.

## Change
- `is_lan_ip` also accepts tailnet ranges (`is_tailnet_ip`: `100.64.0.0/10`,
  `fd7a:115c:a1e0::/48`). Every LAN-only filter now keeps tailnet addresses,
  so the PC's tailnet address from pairing (or learned from a session) is
  stored, dialed and allowed, with away mode off.
- No new firewall rule. A rule for `100.64.0.0/10` would also open the port
  to carrier and hotspot networks that use that range. When a firewall is on,
  allow G7 Link's port (UDP `7718` by default) on the Tailscale interface yourself, for
  example `sudo ufw allow in on tailscale0 to any port 7718 proto udp`.

Unchanged: pairing, device keys, QUIC end-to-end encryption, relays (still off
unless away mode is on), mDNS.

## Limits and follow-ups
- Carrier CGNAT shares `100.64.0.0/10`; such a direct path is now allowed too.
  It still needs the paired device's key and is rarely reachable.
- A device paired before its tailnet address existed has no tailnet address
  stored. Re-pair once while Tailscale is up on both, or connect once on the
  LAN with Tailscale up so the session learns the path.
- Follow-up: a "Connect by address" field (IP or MagicDNS name) in the apps,
  calling the existing `Node::add_known_addrs`; MagicDNS names need a resolve
  step before it.
- The PC's QUIC port must stay stable (the node already reuses its last port).
