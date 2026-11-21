# Connecting over Tailscale

With Tailscale up on the phone and the PC, the phone reaches the PC away from
home directly over the tailnet, with **Connect over mobile data** (Away mode)
off. Nothing here talks to Tailscale: the apps never run `tailscale`, never
use `tailscaled`'s socket or local API, and never contact Tailscale's servers.
They see Tailscale only as addresses in its ranges.

## Tailnet addresses count as home

`is_lan_ip` includes `is_tailnet_ip` (`100.64.0.0/10`, `fd7a:115c:a1e0::/48`),
so every LAN-only filter keeps tailnet addresses: they're stored, dialed and
allowed with Away mode off. mDNS doesn't cross Tailscale (no multicast), so the
phone needs the PC's tailnet address from somewhere else: the pairing link, a
connect, `device.reach`, or the Address field.

Carrier CGNAT shares `100.64.0.0/10`, so such a direct path is allowed too. It
still needs the paired device's key, and is rarely reachable.

## The PC's port: UDP 7718

The desktop listens on UDP **7718** (`NodeConfig::preferred_port`, set to
`g7link_core::reach::DEFAULT_PORT`). If 7718 is busy (a second copy running),
it takes the port it used last (`~/.local/share/G7Link/port`), then any free
port. The installer's rules and the Connection Doctor are about the port in
use. The phone keeps its own saved port; nothing dials it by a remembered port
across networks, and it has no firewall.

Older versions of the desktop took a free port on first start (for example
`40314`) and kept it; the first start of this one moves to 7718. A rule added by hand for that port is dead now:
`sudo ufw delete allow in on tailscale0 to any port 40314 proto udp`.

## The PC tells its phones its tailnet address

After connecting, and whenever either changes, each device sends `device.reach`
(see [the protocol](../protocol/v1.md)): its own tailnet socket addresses
(`100.78.127.19:7718`) and whether its Away mode is on. The phone stores what a
PC said in its own column (`peers.reach`), so a later connect over the LAN,
which rewrites the remembered addresses, doesn't drop it. Connect once at home
with Tailscale up on the PC, and from then on the phone reaches the PC through
Tailscale away from home, across restarts. No MagicDNS name is sent: a
device's tailnet IP is stable, and finding its name would need Tailscale's
local API.

Both apps need to be new enough. An older peer ignores `device.reach` like any
unknown message, so mixed versions still connect at home; they just don't
learn tailnet addresses.

## Address, on the phone

**Settings › PCs ›** the PC **› Address** takes one address per PC: an IP
(`100.78.127.19`, `[fd7a:115c:a1e0::1]:7718`) or a name (`guchu`,
`guchu:7718`). Without a port, the phone tries 7718 and the ports the PC used
before. It's kept in the phone's trust store and survives restarts. **Clear**
goes back to **Automatic**.

A name is resolved on the phone through Android's resolver, where the Tailscale
app answers MagicDNS names, when it's saved, when the app starts, and at most
once a minute while the PC isn't connected. A dial never waits for it. If the
short name isn't found, use the full MagicDNS name
(`guchu.<tailnet>.ts.net`) or the IP.

Saving says what happened: an address that can be used now closes the dialog;
a name that can't be found right now (Tailscale off) is saved and tried later;
an address outside the home network and Tailscale is saved, but used only with
Connect over mobile data on. Every address still goes through the same filter
as the others; nothing is loosened.

## The hint, on the phone

Under Address, one line says what the phone knows about reaching that PC away
from home: the first line below that fits. It's not a live test; a firewall on
the PC can still be in the way.

| Line | When |
|---|---|
| Away from home: through Tailscale | Tailscale is up on the phone, and a tailnet address of the PC is known |
| Away from home: over mobile data | Connect over mobile data is on here, and the PC hasn't said its own is off |
| Away from home: turn on Tailscale on this phone | A tailnet address of the PC is known, Tailscale is off on the phone |
| Only on your home network | None of these |

"Tailscale is up" on the phone means a VPN network with an address in
Tailscale's ranges; carriers hand out `100.64.0.0/10` on mobile data too, so the
VPN part is required. With Tailscale up, the phone also tells the core about
network changes on mobile data, so leaving Wi-Fi redials at once.

## Firewall, on the PC

The installer opens 7718 (and mDNS, LocalSend) to private ranges only. A rule
for `100.64.0.0/10` would also open the port to carrier and hotspot networks,
so there's none. Instead, with ufw on and `/sys/class/net/tailscale0` present,
it adds one rule for Tailscale's interface and notes it in
`~/.local/share/G7Link/firewall_tailnet` (`tailscale0 7718`):

```sh
sudo ufw allow in on tailscale0 to any port 7718 proto udp
```

`--uninstall` deletes it again. Nothing is added for firewalld.

Whether ufw blocks `tailscale0` at all isn't known: Tailscale adds its own
netfilter rules ahead of ufw's on many setups, and they may let everything in
on its interface already. Without root the app can read neither, so it says
"may".

## The Connection Doctor's Tailscale check

The PC finds Tailscale as an interface that is up with an address in
Tailscale's ranges, named `tailscale…` or a tunnel (`/sys/class/net/<name>/type`
is `65534`). That interface isn't reported as "A VPN is on" any more. With one,
the Doctor shows a line right after the firewall check:

| Firewall | Line | Button |
|---|---|---|
| Off; or `firewall_tailnet` names this interface and port; or firewalld has the interface in zone `trusted`, or its zone lists `<port>/udp` | Away from home, your phone reaches this PC through Tailscale · At 100.78.127.19. | |
| ufw, no note | The firewall may block your phone on Tailscale | **Allow** |
| firewalld, not allowed as above | The same, asking to allow the UDP port on the interface | **Firewall settings** |
| nftables | The same | |

**Allow** asks for the administrator password (Polkit) and runs
`ufw allow in on <interface> to any port <port> proto udp`, which ufw skips if
it's there already, then writes the note. The interface name must be 1 to 15
of `[A-Za-z0-9_.-]`, since it goes into a root shell. A changed port or
interface makes the note stale, and the line warns again. The check is about
being away from home, so it doesn't turn the Doctor's summary to "can't reach
this PC".

The firewall check itself counts only a rule for the port in use: a note or
rule for 7718 no longer passes for another port.
