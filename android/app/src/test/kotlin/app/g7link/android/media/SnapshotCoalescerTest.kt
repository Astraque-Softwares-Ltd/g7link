// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.media

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SnapshotCoalescerTest {
    private class Player(val id: String, val artKey: String? = null, val art: ByteArray? = null)

    private fun coalescer(maxArt: Int = 8) =
        SnapshotCoalescer<Player>(artKeyOf = { it.artKey }, artOf = { it.art }, maxArt = maxArt)

    /** Applies a batch the way PcMedia does. */
    private fun apply(into: LinkedHashMap<String, List<Player>>, batch: SnapshotCoalescer.Batch<Player>) {
        for (update in batch.updates) {
            if (update.reattach) into.remove(update.pc)
            if (update.players.isEmpty()) into.remove(update.pc) else into[update.pc] = update.players
        }
    }

    @Test
    fun `nothing waiting takes nothing`() {
        val waiting = coalescer()
        assertNull(waiting.take())
        waiting.offer("pc", "PC", listOf(Player("a")))
        assertTrue(waiting.take() != null)
        assertNull(waiting.take())
    }

    @Test
    fun `only the newest snapshot per PC waits`() {
        val waiting = coalescer()
        repeat(1000) { waiting.offer("pc", "PC $it", listOf(Player("p$it"))) }
        waiting.offer("other", "Other", listOf(Player("o")))
        val batch = waiting.take()!!
        assertEquals(listOf("pc", "other"), batch.updates.map { it.pc })
        assertEquals("PC 999", batch.updates[0].name)
        assertEquals(listOf("p999"), batch.updates[0].players.map { it.id })
    }

    @Test
    fun `a disconnect after a snapshot wins`() {
        val waiting = coalescer()
        waiting.offer("pc", "PC", listOf(Player("a")))
        waiting.offer("pc", "", emptyList())
        val players = linkedMapOf("pc" to listOf(Player("old")))
        apply(players, waiting.take()!!)
        assertTrue(players.isEmpty())
    }

    @Test
    fun `artwork sent once survives a superseded snapshot`() {
        val waiting = coalescer()
        val bytes = byteArrayOf(1, 2, 3)
        waiting.offer("pc", "PC", listOf(Player("a", "k1", bytes)))
        waiting.offer("pc", "PC", listOf(Player("a", "k1")))
        val batch = waiting.take()!!
        assertEquals(setOf("pc/k1"), batch.art.keys)
        assertArrayEquals(bytes, batch.art["pc/k1"])
    }

    @Test
    fun `only artwork the newest snapshots show is taken`() {
        val waiting = coalescer()
        waiting.offer("pc", "PC", listOf(Player("a", "k1", byteArrayOf(1))))
        waiting.offer("pc", "PC", listOf(Player("a", "k2", byteArrayOf(2))))
        assertEquals(setOf("pc/k2"), waiting.take()!!.art.keys)
        // The skipped track's artwork is still there if it comes back.
        waiting.offer("pc", "PC", listOf(Player("a", "k1")))
        assertEquals(setOf("pc/k1"), waiting.take()!!.art.keys)
        waiting.offer("pc", "PC", listOf(Player("a", "k1")))
        assertTrue(waiting.take()!!.art.isEmpty())
    }

    @Test
    fun `waiting artwork is bounded`() {
        val waiting = coalescer(maxArt = 2)
        for (i in 0 until 100) waiting.offer("pc", "PC", listOf(Player("a", "k$i", byteArrayOf(i.toByte()))))
        waiting.take()
        waiting.offer("pc", "PC", listOf(Player("a", "k98"), Player("b", "k0")))
        assertEquals(setOf("pc/k98"), waiting.take()!!.art.keys)
    }

    @Test
    fun `players end in the order applying every snapshot would leave them`() {
        val offers = listOf(
            "a" to listOf(Player("a1")),
            "b" to listOf(Player("b1")),
            "a" to emptyList(),
            "c" to listOf(Player("c1")),
            "a" to listOf(Player("a2")),
            "b" to listOf(Player("b2")),
            "c" to emptyList(),
            "c" to listOf(Player("c2")),
        )
        for (cut in 0..offers.size) {
            val start = linkedMapOf("b" to listOf(Player("b0")), "a" to listOf(Player("a0")))
            val oneByOne = LinkedHashMap(start)
            val coalesced = LinkedHashMap(start)
            val waiting = coalescer()
            for ((index, offer) in offers.withIndex()) {
                val (pc, list) = offer
                if (list.isEmpty()) oneByOne.remove(pc) else oneByOne[pc] = list
                waiting.offer(pc, pc, list)
                if (index + 1 == cut) waiting.take()?.let { apply(coalesced, it) }
            }
            waiting.take()?.let { apply(coalesced, it) }
            assertEquals("cut $cut", oneByOne.keys.toList(), coalesced.keys.toList())
            assertEquals(
                "cut $cut",
                oneByOne.values.map { list -> list.map { it.id } },
                coalesced.values.map { list -> list.map { it.id } },
            )
        }
    }

    @Test
    fun `a PC that stays doesn't move`() {
        val waiting = coalescer()
        waiting.offer("a", "A", listOf(Player("a1")))
        waiting.offer("b", "B", listOf(Player("b1")))
        waiting.offer("a", "A", listOf(Player("a2")))
        val batch = waiting.take()!!
        assertEquals(listOf("a", "b"), batch.updates.map { it.pc })
        assertFalse(batch.updates[0].reattach)
    }
}
