// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.companion

import app.g7link.android.companion.BeaconMatcher.Kind
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class BeaconMatcherTest {

    private fun hex(text: String): ByteArray = text.chunked(2).map { it.toInt(16).toByte() }.toByteArray()

    /** The beacon key of the link secret 00 01 .. 1f, pinned in core/g7link-protocol/src/ble.rs. */
    private val key = hex("e76f53a6349d38f0a86e427569bec16bbfa93aed3edc8ba68685eb0a9f0e5f20")
    private val otherKey = ByteArray(32) { 7 }

    /** Unix seconds in window 1 900 000. */
    private val now = 1_710_000_000L

    private fun beacon(vararg slots: ByteArray, filler: Int = 4 - slots.size): ByteArray {
        var out = BeaconMatcher.HEADER
        for (slot in slots) out += slot
        repeat(filler) { out += ByteArray(6) { 0x55 } }
        return out
    }

    @Test
    fun `tags match the core's published vectors`() {
        assertEquals(1_900_000L, BeaconMatcher.epochAt(now))
        assertArrayEquals(hex("01340facca29"), BeaconMatcher.tag(key, Kind.PRESENCE, 1_900_000))
        assertArrayEquals(hex("f236dc484abb"), BeaconMatcher.tag(key, Kind.PRESENCE, 1_900_001))
        assertArrayEquals(hex("adac05998b8d"), BeaconMatcher.tag(key, Kind.CONNECT, 1_900_000))
        assertArrayEquals(hex("26311d70bd72"), BeaconMatcher.tag(key, Kind.CONNECT, 1_900_001))
    }

    @Test
    fun `a pc is found in any slot and the window either side`() {
        val keys = mapOf("pc" to key)
        val tag = BeaconMatcher.tag(key, Kind.PRESENCE, 1_900_000)
        val first = beacon(tag)
        val last = beacon(ByteArray(6), ByteArray(6), ByteArray(6), tag, filler = 0)
        for (payload in listOf(first, last)) {
            assertEquals(BeaconMatcher.Match("pc", Kind.PRESENCE), BeaconMatcher.match(payload, keys, now))
            assertEquals(Kind.PRESENCE, BeaconMatcher.match(payload, keys, now - 900)?.kind)
            assertEquals(Kind.PRESENCE, BeaconMatcher.match(payload, keys, now + 900)?.kind)
            assertNull(BeaconMatcher.match(payload, keys, now + 1800))
            assertNull(BeaconMatcher.match(payload, keys, now - 1800))
        }
    }

    @Test
    fun `the right pc of two is named, and connect wins`() {
        val keys = mapOf("other" to otherKey, "pc" to key)
        val both = beacon(BeaconMatcher.tag(key, Kind.PRESENCE, 1_900_000), BeaconMatcher.tag(key, Kind.CONNECT, 1_900_000))
        assertEquals(BeaconMatcher.Match("pc", Kind.CONNECT), BeaconMatcher.match(both, keys, now))
        val others = beacon(BeaconMatcher.tag(otherKey, Kind.PRESENCE, 1_900_000))
        assertEquals("other", BeaconMatcher.match(others, keys, now)?.pcId)
    }

    @Test
    fun `only that pc's connect tag asks to connect`() {
        val connect = beacon(BeaconMatcher.tag(key, Kind.CONNECT, 1_900_000))
        assertTrue(BeaconMatcher.asksToConnect(connect, "pc", key, now))
        assertTrue("the window either side", BeaconMatcher.asksToConnect(connect, "pc", key, now + 900))
        val presence = beacon(BeaconMatcher.tag(key, Kind.PRESENCE, 1_900_000))
        assertFalse("there, but not asking", BeaconMatcher.asksToConnect(presence, "pc", key, now))
        val other = beacon(BeaconMatcher.tag(otherKey, Kind.CONNECT, 1_900_000))
        assertFalse("another PC asking", BeaconMatcher.asksToConnect(other, "pc", key, now))
        assertFalse("an old window", BeaconMatcher.asksToConnect(connect, "pc", key, now + 1800))
    }

    @Test
    fun `other data is no beacon`() {
        val keys = mapOf("pc" to key)
        val tag = BeaconMatcher.tag(key, Kind.PRESENCE, 1_900_000)
        // The old beacon, too many slots, an odd length, a wrong version, just the header.
        assertNull(BeaconMatcher.match(PcCompanion.BEACON_MAGIC + ByteArray(8), keys, now))
        assertNull(BeaconMatcher.match(beacon(tag, filler = 4), keys, now))
        assertNull(BeaconMatcher.match(beacon(tag).copyOf(25), keys, now))
        assertNull(BeaconMatcher.match(byteArrayOf(0x47, 0x03) + tag, keys, now))
        assertNull(BeaconMatcher.match(BeaconMatcher.HEADER, keys, now))
        assertNull(BeaconMatcher.match(beacon(tag), emptyMap(), now))
    }
}
