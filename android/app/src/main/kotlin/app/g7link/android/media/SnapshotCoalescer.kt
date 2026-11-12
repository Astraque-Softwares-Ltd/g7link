// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.media

/**
 * PC media snapshots waiting to be applied, at most one per PC: a snapshot
 * replaces its PC's whole list, so a newer one stands in for the one still
 * waiting. Artwork arrives only once per key, so its bytes are kept (the
 * newest [maxArt], by "pc/key") until a snapshot that shows it is taken.
 *
 * Taking everything waiting and applying it in order leaves the same players,
 * in the same order, as applying each snapshot as it came.
 */
internal class SnapshotCoalescer<P>(
    private val artKeyOf: (P) -> String?,
    private val artOf: (P) -> ByteArray?,
    private val maxArt: Int = 8,
) {
    /**
     * A PC's newest players (empty: it went). [reattach]: the PC went and came
     * back while this waited, so it's removed before it's put back.
     */
    class Update<P>(val pc: String, val name: String, val players: List<P>, val reattach: Boolean)

    /** What to apply, in order, and the artwork bytes it shows by "pc/key". */
    class Batch<P>(val updates: List<Update<P>>, val art: Map<String, ByteArray>)

    private val waiting = LinkedHashMap<String, Update<P>>()
    private val art = object : LinkedHashMap<String, ByteArray>() {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, ByteArray>) = size > maxArt
    }

    @Synchronized
    fun offer(pc: String, name: String, players: List<P>) {
        for (player in players) {
            val key = artKeyOf(player) ?: continue
            val bytes = artOf(player) ?: continue
            art.remove("$pc/$key")
            art["$pc/$key"] = bytes
        }
        val before = waiting[pc]
        // A PC that went and comes back is added after the PCs that came in between.
        val returns = before != null && before.players.isEmpty() && players.isNotEmpty()
        if (returns) waiting.remove(pc)
        waiting[pc] = Update(pc, name, players, reattach = returns || before?.reattach == true)
    }

    /** Everything waiting, or null when nothing is. */
    @Synchronized
    fun take(): Batch<P>? {
        if (waiting.isEmpty()) return null
        val updates = waiting.values.toList()
        waiting.clear()
        val shown = LinkedHashMap<String, ByteArray>()
        for (update in updates) {
            for (player in update.players) {
                val key = "${update.pc}/${artKeyOf(player) ?: continue}"
                art.remove(key)?.let { shown[key] = it }
            }
        }
        return Batch(updates, shown)
    }
}
