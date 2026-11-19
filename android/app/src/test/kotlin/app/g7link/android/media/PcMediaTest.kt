// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.media

import app.g7link.core.MediaAction
import app.g7link.core.MediaPlayer
import app.g7link.core.MediaRepeat
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PcMediaTest {

    private fun player(
        position: ULong? = 10_000u,
        duration: ULong? = 200_000u,
        playing: Boolean = true,
        rateMilli: UInt? = null,
        ageMs: ULong? = null,
        actions: List<MediaAction> = listOf(MediaAction.PLAY, MediaAction.PAUSE, MediaAction.SEEK),
    ) = MediaPlayer(
        id = "org.mpris.MediaPlayer2.spotify",
        app = "Spotify",
        title = "A song",
        artist = "Someone",
        album = "Somewhere",
        playing = playing,
        duration = duration,
        position = position,
        actions = actions,
        artKey = null,
        art = null,
        rateMilli = rateMilli,
        ageMs = ageMs,
        artFull = null,
        icon = null,
        shuffle = null,
        repeat = null,
    )

    @Test
    fun `a position moves on in real time from when it arrived`() {
        val timeline = checkNotNull(PcMedia.timelineOf(player(), arrivedAt = 1_000))
        assertEquals(10_000, timeline.positionAt(1_000))
        assertEquals(12_500, timeline.positionAt(3_500))
    }

    @Test
    fun `the age of a position counts as time already played`() {
        val timeline = checkNotNull(PcMedia.timelineOf(player(ageMs = 300u), arrivedAt = 1_000))
        assertEquals(10_300, timeline.positionAt(1_000))
        assertEquals(11_300, timeline.positionAt(2_000))
    }

    @Test
    fun `the rate scales how fast it moves`() {
        val double = checkNotNull(PcMedia.timelineOf(player(rateMilli = 2_000u, ageMs = 500u), arrivedAt = 0))
        // Sampled 500 ms before it arrived at 2×: 1 s ahead already.
        assertEquals(11_000, double.positionAt(0))
        assertEquals(13_000, double.positionAt(1_000))
        val slow = checkNotNull(PcMedia.timelineOf(player(rateMilli = 250u), arrivedAt = 0))
        assertEquals(10_250, slow.positionAt(1_000))
    }

    @Test
    fun `a rate out of range or missing is normal speed`() {
        assertEquals(PcMedia.NORMAL_RATE, PcMedia.rateOf(player(rateMilli = null)))
        assertEquals(PcMedia.NORMAL_RATE, PcMedia.rateOf(player(rateMilli = 249u)))
        assertEquals(PcMedia.NORMAL_RATE, PcMedia.rateOf(player(rateMilli = 4_001u)))
        assertEquals(PcMedia.NORMAL_RATE, PcMedia.rateOf(player(rateMilli = 0u)))
        assertEquals(4_000, PcMedia.rateOf(player(rateMilli = 4_000u)))
        assertEquals(1_500, PcMedia.rateOf(player(rateMilli = 1_500u)))
    }

    @Test
    fun `a huge age is capped and the position never passes the duration`() {
        assertEquals(PcMedia.MAX_AGE_MS, PcMedia.ageOf(player(ageMs = ULong.MAX_VALUE)))
        assertEquals(0L, PcMedia.ageOf(player(ageMs = null)))
        val timeline = checkNotNull(PcMedia.timelineOf(player(ageMs = ULong.MAX_VALUE, rateMilli = 4_000u), arrivedAt = 0))
        assertEquals(200_000, timeline.positionAt(Long.MAX_VALUE / 8))
    }

    @Test
    fun `a paused player stays where it is`() {
        val timeline = checkNotNull(PcMedia.timelineOf(player(playing = false, ageMs = 900u, rateMilli = 2_000u), arrivedAt = 0))
        assertEquals(10_000, timeline.positionAt(60_000))
    }

    @Test
    fun `no duration means no timeline`() {
        assertNull(PcMedia.timelineOf(player(duration = null), arrivedAt = 0))
        assertNull(PcMedia.timelineOf(player(duration = 0u), arrivedAt = 0))
    }

    @Test
    fun `a position past the duration is held at the end`() {
        val timeline = checkNotNull(PcMedia.timelineOf(player(position = 999_000u), arrivedAt = 0))
        assertEquals(200_000, timeline.positionAt(0))
    }

    private fun held(target: Long, at: Long) = PcMedia.SeekHold(
        PcMedia.Timeline(positionMs = target, durationMs = 200_000, playing = true, canSeek = true, atElapsedMs = at),
        at,
    )

    @Test
    fun `after a seek the PC's stale positions are ignored`() {
        val hold = held(target = 120_000, at = 1_000)
        // The PC's state from before it took the seek still says 10 s in.
        val stale = PcMedia.Timeline(10_400, 200_000, playing = true, canSeek = true, atElapsedMs = 1_400)
        val (shown, keep) = PcMedia.settled(stale, hold, now = 1_400)
        assertTrue(keep)
        assertEquals(120_400, shown.positionAt(1_400))
        assertEquals(121_400, shown.positionAt(2_400))
    }

    @Test
    fun `a position near the seek's confirms it`() {
        val hold = held(target = 120_000, at = 1_000)
        val agreeing = PcMedia.Timeline(119_800, 200_000, playing = true, canSeek = true, atElapsedMs = 1_300)
        val (shown, keep) = PcMedia.settled(agreeing, hold, now = 1_300)
        assertFalse(keep)
        assertEquals(agreeing, shown)
    }

    @Test
    fun `the hold ends after a second whatever the PC says`() {
        val hold = held(target = 120_000, at = 1_000)
        val elsewhere = PcMedia.Timeline(30_000, 200_000, playing = true, canSeek = true, atElapsedMs = 2_000)
        val (shown, keep) = PcMedia.settled(elsewhere, hold, now = 1_000 + PcMedia.SEEK_HOLD_MS)
        assertFalse(keep)
        assertEquals(elsewhere, shown)
    }

    @Test
    fun `a pause during the hold stops the shown position`() {
        val hold = held(target = 120_000, at = 1_000)
        val paused = PcMedia.Timeline(10_000, 200_000, playing = false, canSeek = true, atElapsedMs = 1_500)
        val (shown, keep) = PcMedia.settled(paused, hold, now = 1_500)
        assertTrue(keep)
        assertEquals(120_500, shown.positionAt(1_500))
        assertEquals(120_500, shown.positionAt(9_000))
    }

    @Test
    fun `shuffle and repeat go on the wire as the protocol spells them`() {
        assertEquals("none", PcMedia.repeatValue(MediaRepeat.NONE))
        assertEquals("track", PcMedia.repeatValue(MediaRepeat.TRACK))
        assertEquals("playlist", PcMedia.repeatValue(MediaRepeat.PLAYLIST))
        assertEquals("on", PcMedia.SHUFFLE_ON)
        assertEquals("off", PcMedia.SHUFFLE_OFF)
    }
}
