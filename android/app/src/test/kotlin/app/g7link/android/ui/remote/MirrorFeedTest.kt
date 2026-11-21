// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * The packets on their way to the decoder: a gap in the PC's numbering or a
 * decoder that fell behind drops to the next keyframe and asks for one, once.
 */
class MirrorFeedTest {
    private class Feed(capacity: Int = 4) {
        var asks = 0
        val feed = MirrorFeed<String>(capacity) { asks++ }

        fun key(seq: Long) = feed.offer(seq, keyframe = true, item = "K$seq")
        fun delta(seq: Long) = feed.offer(seq, keyframe = false, item = "P$seq")

        /** Everything the decoder would take now. */
        fun drain(): List<String> = generateSequence { feed.poll() }.toList()
    }

    @Test
    fun aSteadyStreamGoesThroughInOrderWithoutAsking() {
        val f = Feed()
        f.key(0)
        f.delta(1)
        assertEquals(listOf("K0", "P1"), f.drain())
        f.delta(2)
        f.delta(3)
        assertEquals(listOf("P2", "P3"), f.drain())
        assertEquals(0, f.asks)
        assertEquals(0L, f.feed.missed)
    }

    @Test
    fun anOverflowAsksForOneKeyframeAndShowsNothingBeforeIt() {
        val f = Feed(capacity = 4)
        f.key(0)
        f.delta(1)
        f.delta(2)
        f.delta(3)
        // The decoder took none of them: the fifth finds the queue full.
        f.delta(4)
        f.delta(5)
        f.delta(6)
        assertEquals("one keyframe asked for, however many follow", 1, f.asks)
        assertEquals("nothing waits for the decoder until the keyframe", emptyList<String>(), f.drain())
        f.key(7)
        f.delta(8)
        assertEquals(listOf("K7", "P8"), f.drain())
        assertEquals(1, f.asks)
        assertEquals(1L, f.feed.asks)
    }

    @Test
    fun aGapAsksForOneKeyframeAndDropsWhatFollowsUntilIt() {
        val f = Feed()
        f.key(0)
        f.delta(1)
        // Packet 2 never came (the phone's flow dropped it).
        f.delta(3)
        f.delta(4)
        f.delta(5)
        assertEquals(1, f.asks)
        // What came before the gap still decodes; nothing after it shows.
        assertEquals(listOf("K0", "P1"), f.drain())
        f.key(6)
        f.delta(7)
        assertEquals(listOf("K6", "P7"), f.drain())
        assertEquals(1L, f.feed.missed)
        assertEquals(3L, f.feed.dropped)
    }

    @Test
    fun eachLossAsksOnce() {
        val f = Feed()
        f.key(0)
        f.delta(3)
        f.delta(4)
        f.key(5)
        f.delta(6)
        f.delta(9)
        f.delta(10)
        assertEquals(2, f.asks)
        assertEquals(4L, f.feed.missed)
        assertEquals(listOf("K0", "K5", "P6"), f.drain())
    }

    @Test
    fun aKeyframeAfterAGapNeedsNoAsking() {
        val f = Feed()
        f.key(0)
        f.key(4)
        assertEquals(0, f.asks)
        assertEquals(listOf("K0", "K4"), f.drain())
    }

    @Test
    fun aNewStreamStartsAgainFromItsKeyframe() {
        val f = Feed()
        f.key(0)
        f.delta(1)
        f.delta(2)
        f.drain()
        // A new config: numbered from 0 again, starting with a keyframe.
        f.key(0)
        f.delta(1)
        assertEquals(listOf("K0", "P1"), f.drain())
        assertEquals(0, f.asks)
    }

    @Test
    fun joiningMidStreamAsksOnceAndWaitsForTheKeyframe() {
        val f = Feed()
        f.delta(40)
        f.delta(41)
        f.delta(42)
        assertEquals(1, f.asks)
        assertNull(f.feed.poll())
        f.key(43)
        assertEquals(listOf("K43"), f.drain())
    }

    @Test
    fun aPacketTheDecoderCouldNotTakeDropsWhatWaitsAndAsksOnce() {
        val f = Feed()
        f.key(0)
        f.delta(1)
        f.delta(2)
        assertEquals("K0", f.feed.poll())
        f.feed.lost()
        f.delta(3)
        assertEquals(1, f.asks)
        assertEquals(emptyList<String>(), f.drain())
        f.key(4)
        assertEquals(listOf("K4"), f.drain())
    }

    @Test
    fun percentilesAreNearestRank() {
        val values = (1L..100L).toList().shuffled()
        assertEquals(50L, MirrorStats.percentile(values, 50))
        assertEquals(95L, MirrorStats.percentile(values, 95))
        assertEquals(7L, MirrorStats.percentile(listOf(7L), 95))
        assertNull(MirrorStats.percentile(emptyList(), 50))
    }

    @Test
    fun eachSecondSaysWhatShowedAndHowLate() {
        val stats = MirrorStats()
        assertNull("the first call starts the count", stats.window(1_000))
        repeat(30) { i ->
            stats.arrived(10_000)
            stats.shown(captureToShownUs = 50_000L + i * 1_000L, arrivedToShownUs = 20_000L)
        }
        // A clock that's far off isn't a latency.
        stats.shown(captureToShownUs = -5_000L, arrivedToShownUs = null)
        stats.counts(missed = 2, dropped = 3, asks = 1)
        val window = stats.window(2_000)!!
        assertEquals(31, window.frames)
        assertEquals(31f, window.fps, 0.01f)
        assertEquals(2.4f, window.megabits, 0.01f)
        assertEquals(64L, window.latencyP50)
        assertEquals(78L, window.latencyP95)
        assertEquals(20L, window.localP50)
        assertEquals(2L, window.missed)
        assertEquals(1L, window.asks)
        // The next second counts from there.
        stats.counts(missed = 2, dropped = 3, asks = 1)
        val quiet = stats.window(3_000)!!
        assertEquals(0, quiet.frames)
        assertEquals(0L, quiet.missed)
        assertNull(quiet.latencyP50)
    }
}
