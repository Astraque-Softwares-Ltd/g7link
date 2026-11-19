// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.player

import app.g7link.android.media.PcMedia
import app.g7link.core.FfiRolePalette
import app.g7link.core.FfiThemeScheme
import app.g7link.core.MediaRepeat
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class PlayerModelTest {

    private fun track(
        pc: String = "pc1",
        id: String = "spotify",
        playing: Boolean = false,
        activeAt: Long = 0,
        canPlay: Boolean = true,
        canPause: Boolean = true,
        canNext: Boolean = true,
        canPrevious: Boolean = true,
        canSeek: Boolean = true,
        canShuffle: Boolean = false,
        canRepeat: Boolean = false,
    ) = PcMedia.Summary(
        playerId = id,
        title = "Title of $id",
        subtitle = "Artist · Spotify on guchu",
        playing = playing,
        artwork = null,
        pc = pc,
        pcName = "guchu",
        appName = "Spotify",
        source = "Spotify on guchu",
        canPlay = canPlay,
        canPause = canPause,
        canNext = canNext,
        canPrevious = canPrevious,
        canSeek = canSeek,
        canShuffle = canShuffle,
        canRepeat = canRepeat,
        activeAt = activeAt,
    )

    private val timeline = PcMedia.Timeline(positionMs = 30_000, durationMs = 120_000, playing = true, canSeek = true, atElapsedMs = 0)

    @Test
    fun `nothing shows without a player`() {
        assertNull(PlayerModel.shown(emptyList(), null))
    }

    @Test
    fun `the one that plays shows, the most recently started of several`() {
        val paused = track(id = "vlc", activeAt = 900)
        val older = track(pc = "pc1", id = "spotify", playing = true, activeAt = 100)
        val newer = track(pc = "pc2", id = "firefox", playing = true, activeAt = 500)
        assertSame(newer, PlayerModel.shown(listOf(paused, older, newer), null))
        assertSame(older, PlayerModel.shown(listOf(paused, older), null))
    }

    @Test
    fun `with nothing playing the latest shows, ties going to the PCs' order`() {
        val first = track(pc = "pc1", id = "a", activeAt = 300)
        val second = track(pc = "pc2", id = "b", activeAt = 300)
        val older = track(pc = "pc2", id = "c", activeAt = 100)
        assertSame(first, PlayerModel.shown(listOf(older, first, second), null))
    }

    @Test
    fun `a player picked in the sheet stays until another starts playing after the pick`() {
        val playing = track(pc = "pc1", id = "spotify", playing = true, activeAt = 100)
        val picked = track(pc = "pc2", id = "vlc", activeAt = 50)
        val pick = PlayerModel.Pick("pc2", "vlc", at = 200)
        assertSame(picked, PlayerModel.shown(listOf(playing, picked), pick))
        val restarted = playing.copy(activeAt = 300)
        assertSame(restarted, PlayerModel.shown(listOf(restarted, picked), pick))
    }

    @Test
    fun `a pick that's gone falls back to the usual choice`() {
        val playing = track(id = "spotify", playing = true, activeAt = 100)
        assertSame(playing, PlayerModel.shown(listOf(playing), PlayerModel.Pick("pc9", "gone", at = 500)))
    }

    @Test
    fun `the sheet opens only with something to choose`() {
        val one = listOf(track())
        assertFalse(PlayerModel.hasChoices(one, outputs = 0, phoneSpeaker = false))
        assertTrue(PlayerModel.hasChoices(one + track(id = "vlc"), outputs = 0, phoneSpeaker = false))
        assertTrue(PlayerModel.hasChoices(one, outputs = 1, phoneSpeaker = false))
        assertTrue(PlayerModel.hasChoices(one, outputs = 0, phoneSpeaker = true))
    }

    @Test
    fun `shuffle and repeat show only when offered`() {
        val plain = PlayerModel.controls(track(), timeline)
        assertFalse(plain.shuffle)
        assertFalse(plain.repeat)
        val full = PlayerModel.controls(track(canShuffle = true, canRepeat = true), timeline)
        assertTrue(full.shuffle)
        assertTrue(full.repeat)
    }

    @Test
    fun `seeking needs the action and a duration`() {
        assertTrue(PlayerModel.controls(track(), timeline).seek)
        assertFalse(PlayerModel.controls(track(canSeek = false), timeline).seek)
        assertFalse(PlayerModel.controls(track(), null).seek)
        assertFalse(PlayerModel.controls(track(), timeline.copy(canSeek = false)).seek)
    }

    @Test
    fun `play or pause works only when the player offers the one it would send`() {
        assertTrue(PlayerModel.controls(track(playing = true), timeline).playPause)
        assertFalse(PlayerModel.controls(track(playing = true, canPause = false), timeline).playPause)
        assertFalse(PlayerModel.controls(track(playing = false, canPlay = false), timeline).playPause)
        val noSkips = PlayerModel.controls(track(canNext = false, canPrevious = false), timeline)
        assertFalse(noSkips.next)
        assertFalse(noSkips.previous)
    }

    @Test
    fun `repeat goes off, the playlist, this track, off`() {
        assertEquals(MediaRepeat.PLAYLIST, PlayerModel.nextRepeat(null))
        assertEquals(MediaRepeat.PLAYLIST, PlayerModel.nextRepeat(MediaRepeat.NONE))
        assertEquals(MediaRepeat.TRACK, PlayerModel.nextRepeat(MediaRepeat.PLAYLIST))
        assertEquals(MediaRepeat.NONE, PlayerModel.nextRepeat(MediaRepeat.TRACK))
    }

    @Test
    fun `progress follows the timeline and stays in range`() {
        assertEquals(0.25f, PlayerModel.progress(timeline, 0), 0.0001f)
        assertEquals(0.5f, PlayerModel.progress(timeline, 30_000), 0.0001f)
        assertEquals(1f, PlayerModel.progress(timeline, 10_000_000), 0.0001f)
        assertEquals(0f, PlayerModel.progress(null, 0), 0f)
    }

    @Test
    fun `times read as the seek bar shows them`() {
        assertEquals("0:00", PlayerModel.clock(0))
        assertEquals("1:04", PlayerModel.clock(64_999))
        assertEquals("59:59", PlayerModel.clock(3_599_000))
        assertEquals("1:02:03", PlayerModel.clock(3_723_000))
        assertEquals("0:00", PlayerModel.clock(-5_000))
    }

    @Test
    fun `the volume bar moves in the PC's 4 percent steps`() {
        assertEquals(0, PlayerModel.volumeStep(0f))
        assertEquals(PcMedia.VOLUME_STEPS, PlayerModel.volumeStep(1f))
        assertEquals(PcMedia.VOLUME_STEPS, PlayerModel.volumeStep(7f))
        assertEquals(13, PlayerModel.volumeStep(0.5f))
        assertEquals(52, PlayerModel.volumeOf(13))
        for (step in 0..PcMedia.VOLUME_STEPS) {
            assertEquals(step, PcMedia.toSteps(PlayerModel.volumeOf(step)))
        }
    }

    @Test
    fun `a sideways swipe past the threshold skips, left to the next`() {
        assertEquals(1, PlayerModel.skip(-60f, 56f))
        assertEquals(-1, PlayerModel.skip(60f, 56f))
        assertEquals(0, PlayerModel.skip(30f, 56f))
        assertEquals(0, PlayerModel.skip(-55.9f, 56f))
    }

    private fun roles(surface: UInt) = FfiRolePalette(
        primary = 0xD4BCFFu,
        onPrimary = 0x3B2569u,
        primaryContainer = 0x52407Eu,
        onPrimaryContainer = 0xEBDDFFu,
        secondaryContainer = 0x4A4458u,
        onSecondaryContainer = 0xE8DEF8u,
        surface = surface,
        surfaceContainerLow = surface,
        surfaceContainer = 0x2B2933u,
        surfaceContainerHigh = 0x383050u,
        surfaceContainerHighest = 0x413B4Au,
        onSurface = 0xEEE7F8u,
        onSurfaceVariant = 0xCDC3DEu,
        outline = 0x958DA6u,
        outlineVariant = 0x4A4458u,
    )

    @Test
    fun `the cover's dark roles in the dark theme, light in the light, the app's for a grey cover`() {
        val dark = roles(0x1C1924u)
        val light = roles(0xFDF7FFu)
        val scheme = FfiThemeScheme(seedRgb = 0x8E6CC8u, achromatic = false, light = light, dark = dark)
        assertSame(dark, PlayerModel.roles(scheme, dark = true))
        assertSame(light, PlayerModel.roles(scheme, dark = false))
        assertNull(PlayerModel.roles(scheme.copy(achromatic = true), dark = true))
        assertNull(PlayerModel.roles(null, dark = true))
    }
}
