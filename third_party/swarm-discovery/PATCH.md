# swarm-discovery, patched

This is [swarm-discovery](https://crates.io/crates/swarm-discovery) 0.6.3 by
Roland Kuhn (Apache-2.0, see `LICENSE.Apache_2.0`), with two changes, marked
"G7 Link patch" in the source. Upstream 0.6.6 still has both problems.

## The changes

- `src/updater.rs`: the garbage-collection timer retried a failed send every
  10 ms. Once a `Discoverer` was dropped its updater was gone, every send
  failed, and the timer kept retrying forever: a busy loop per restart of
  mDNS (G7 Link restarts it when a pairing or Nearby screen opens or closes).
  It now stops once the updater is gone, and still retries a full mailbox.
- `src/receiver.rs`, `src/guardian.rs`: removing an interface dropped its
  socket from the map but left the interface's receiver running, holding the
  socket, so a UDP socket stayed bound to every address the device had left.
  A receiver now ends when the guardian lets go of it, which it does when its
  interface is removed; the guardian keeps going when such a receiver ends,
  and keeps the main receivers' references so they never end that way.

The tests and examples are upstream's and need its dev-dependencies, so they
aren't built here.
