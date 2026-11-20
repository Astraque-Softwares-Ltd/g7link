// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.audio

import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SoundJitterBufferTest {
    private var now = 1_000L

    private fun buffer(maxBufferedUs: Long = 1_000_000L) =
        SoundJitterBuffer(maxBufferedUs = maxBufferedUs, nowMs = { now })

    /** 960 bytes of 48 kHz stereo: 5 ms. */
    private fun piece(seq: Int, tsMs: Long = seq * 5L, rate: Int = 48_000) =
        SoundPiece(seq and 0xFFFF, tsMs, rate, 2, ByteArray(960))

    private fun SoundJitterBuffer.next(): Int? = poll(0)?.piece?.seq

    @Test
    fun afterIsLessThanHalfTheRangeAheadModulo65536() {
        assertTrue(SoundJitterBuffer.isAfter(1, 0))
        assertTrue(SoundJitterBuffer.isAfter(0, 65_535))
        assertTrue(SoundJitterBuffer.isAfter(0x7FFF, 0))
        assertFalse(SoundJitterBuffer.isAfter(0x8000, 0))
        assertFalse(SoundJitterBuffer.isAfter(0, 0))
        assertFalse(SoundJitterBuffer.isAfter(65_535, 0))
    }

    @Test
    fun piecesPlayInOrderEvenWhenTheyArriveOutOfOrder() {
        val b = buffer()
        assertTrue(b.offer(piece(1)))
        assertTrue(b.offer(piece(3)))
        assertTrue(b.offer(piece(2)))
        assertEquals(listOf(1, 2, 3), listOf(b.next(), b.next(), b.next()))
        assertNull(b.next())
    }

    @Test
    fun aPieceOlderThanTheLastPlayedIsDropped() {
        val b = buffer()
        b.offer(piece(5))
        b.offer(piece(6))
        assertEquals(5, b.next())
        assertFalse("older than what played", b.offer(piece(4)))
        assertFalse("the one that played", b.offer(piece(5)))
        assertFalse("already waiting", b.offer(piece(6)))
        assertEquals(3L, b.late)
        assertEquals(6, b.next())
    }

    @Test
    fun numbersWrapAround() {
        val b = buffer()
        b.offer(piece(65_535, tsMs = 0))
        b.offer(piece(1, tsMs = 10))
        b.offer(piece(0, tsMs = 5))
        assertEquals(listOf(65_535, 0, 1), listOf(b.next(), b.next(), b.next()))
        assertFalse("from before the wrap", b.offer(piece(65_534, tsMs = 0)))
        assertTrue(b.offer(piece(2, tsMs = 15)))
        assertEquals(2, b.next())
        assertEquals(0L, b.lost)
    }

    @Test
    fun farBehindWhatPlayedIsANewStartNotLate() {
        val b = buffer()
        b.offer(piece(5_000))
        assertEquals(5_000, b.next())
        assertTrue("the PC's sound started again", b.offer(piece(0)))
        val out = b.poll(0)
        assertEquals(0, out?.piece?.seq)
        assertTrue(out!!.fresh)
    }

    @Test
    fun afterASilenceTheNumbersStartAfresh() {
        val b = buffer()
        b.offer(piece(10))
        assertEquals(10, b.next())
        now += SoundJitterBuffer.RESTART_AFTER_IDLE_MS + 1
        assertTrue(b.offer(piece(3)))
        assertEquals(3, b.next())
    }

    @Test
    fun anotherPcStartsOver() {
        val b = buffer()
        b.offer(piece(10))
        assertEquals(10, b.next())
        b.restart()
        assertTrue(b.offer(piece(3)))
        assertEquals(3, b.next())
    }

    @Test
    fun theBacklogIsCappedAndWhatWasLetGoCountsAsPlayed() {
        val b = buffer(maxBufferedUs = 10_000L)
        b.offer(piece(1))
        b.offer(piece(2))
        b.offer(piece(3))
        assertEquals("two 5 ms pieces wait", 10_000L, b.bufferedUs())
        assertFalse("let go to keep the delay down", b.offer(piece(1)))
        val out = b.poll(0)
        assertEquals(2, out?.piece?.seq)
        assertFalse("it follows what was let go", out!!.fresh)
    }

    @Test
    fun captureTimesSayWhenTheSoundStoppedForAWhile() {
        val b = buffer()
        b.offer(piece(1, tsMs = 0))
        b.offer(piece(2, tsMs = 5))
        b.offer(piece(3, tsMs = 500))
        b.offer(piece(4, tsMs = 505, rate = 44_100))
        assertTrue("the first", b.poll(0)!!.fresh)
        assertFalse("right after it", b.poll(0)!!.fresh)
        assertTrue("half a second on", b.poll(0)!!.fresh)
        assertTrue("another rate", b.poll(0)!!.fresh)
    }

    @Test
    fun aFewLostPiecesAreCountedButDontRestartTheCushion() {
        val b = buffer()
        b.offer(piece(1))
        b.offer(piece(4))
        assertEquals(1, b.next())
        assertFalse(b.poll(0)!!.fresh)
        assertEquals(2L, b.lost)
    }

    @Test
    fun jitterSetsTheCushion() {
        val steady = buffer()
        for (i in 0 until 50) {
            steady.offer(piece(i))
            now += 5
        }
        assertEquals("on time: the least", SoundJitterBuffer.MIN_CUSHION_US, steady.cushionUs())

        // Four pieces at once every 20 ms.
        val bursty = buffer()
        for (i in 50 until 150) {
            bursty.offer(piece(i))
            if (i % 4 == 3) now += 20
        }
        val cushion = bursty.cushionUs()
        assertTrue("$cushion", cushion > SoundJitterBuffer.MIN_CUSHION_US && cushion < SoundJitterBuffer.MAX_CUSHION_US)

        // Pairs 70 ms apart: as much as it gathers.
        val bad = buffer()
        for (i in 150 until 350) {
            bad.offer(piece(i))
            if (i % 2 == 1) now += 70
        }
        assertEquals(SoundJitterBuffer.MAX_CUSHION_US, bad.cushionUs())
        bad.restart()
        assertEquals("another PC is measured afresh", SoundJitterBuffer.MIN_CUSHION_US, bad.cushionUs())
    }

    @Test
    fun aPieceWakesAWaitingPoll() {
        val b = buffer()
        val waiting = CompletableFuture.supplyAsync { b.poll(10_000) }
        b.offer(piece(7))
        assertEquals(7, waiting.get(5, TimeUnit.SECONDS)?.piece?.seq)
    }

    @Test
    fun stopEmptiesTheBufferAndEndsAWaitingPoll() {
        val b = buffer()
        val waiting = CompletableFuture.supplyAsync { b.poll(10_000) }
        b.stop()
        assertNull(waiting.get(5, TimeUnit.SECONDS))

        b.offer(piece(1))
        b.stop()
        assertTrue(b.isEmpty())
        assertNull("the worker stops", b.poll(0))
        assertTrue("then sound starts over", b.offer(piece(0)))
        assertEquals(0, b.next())
    }
}
