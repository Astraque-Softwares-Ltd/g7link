// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.player

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class PlayerColorsTest {

    private val width = 390f
    private val statusBar = 24f
    private val rowTop = statusBar + 4f
    private val label = Area(56f, rowTop + 14f, width - 56f, rowTop + 34f)
    private val glyph = Area(20f, rowTop + 12f, 44f, rowTop + 36f)

    /** A square cover, [size] px a side, filled with [rgb], with a [mark] square in the middle. */
    private fun cover(rgb: Int, size: Int = 112, mark: Int? = null): CoverLight {
        val pixels = IntArray(size * size) { i ->
            val x = i % size
            val y = i / size
            val inMark = mark != null && x in size / 3 until size * 2 / 3 && y in size / 3 until size * 2 / 3
            (0xFF shl 24) or (if (inMark) mark!! else rgb)
        }
        return checkNotNull(PlayerColors.measure(pixels, size, size))
    }

    private fun scrimOver(light: CoverLight?, ground: Int, ink: Int) =
        PlayerColors.headerScrim(light, ground, ink, width, width, statusBar, label, glyph)

    /** The contrast [ink] gets over the worst of [under] where [area]'s bottom edge meets the scrim. */
    private fun readsAt(scrim: HeaderScrim, ground: Int, ink: Int, under: Int, area: Area) =
        PlayerColors.contrast(ink, PlayerColors.over(ground, scrim.alphaAt(area.bottom), under))

    @Test
    fun `contrast matches WCAG`() {
        assertEquals(21.0, PlayerColors.contrast(0x000000, 0xFFFFFF), 0.01)
        assertEquals(1.0, PlayerColors.contrast(0x777777, 0x777777), 0.0001)
        // #767676 on white is the classic 4.54:1.
        assertEquals(4.54, PlayerColors.contrast(0x767676, 0xFFFFFF), 0.01)
    }

    @Test
    fun `compositing is per channel`() {
        assertEquals(0x000000, PlayerColors.over(0x000000, 1f, 0xFFFFFF))
        assertEquals(0xFFFFFF, PlayerColors.over(0x000000, 0f, 0xFFFFFF))
        assertEquals(0x808080, PlayerColors.over(0x000000, 0.5f, 0xFFFFFF))
    }

    @Test
    fun `the header on a near-white cover raises its scrim until it reads`() {
        // The "paper" cover: near white, under the extractor's dark roles.
        val ground = 0x1D1B18
        val ink = 0xEFE9E0
        val paper = 0xFBF8F2
        val normal = HeaderScrim(PlayerColors.SCRIM_ALPHA, statusBar, statusBar + PlayerColors.SCRIM_DP)
        assertTrue(readsAt(normal, ground, ink, paper, label) < PlayerColors.TEXT_CONTRAST)
        val scrim = scrimOver(cover(paper), ground, ink)
        assertTrue("held past the header: ${scrim.hold}", scrim.hold >= label.bottom && scrim.hold >= glyph.bottom)
        assertTrue(scrim.alpha >= PlayerColors.SCRIM_ALPHA)
        assertTrue(readsAt(scrim, ground, ink, paper, label) >= PlayerColors.TEXT_CONTRAST)
        assertTrue(readsAt(scrim, ground, ink, paper, glyph) >= PlayerColors.GLYPH_CONTRAST)
    }

    @Test
    fun `a ground too light to read when held is made stronger, and no stronger than it needs`() {
        val ground = 0x3A3A3A
        val ink = 0xDDDDDD
        val scrim = scrimOver(cover(0xFFFFFF), ground, ink)
        assertTrue("alpha ${scrim.alpha}", scrim.alpha > PlayerColors.SCRIM_ALPHA)
        assertTrue(readsAt(scrim, ground, ink, 0xFFFFFF, label) >= PlayerColors.TEXT_CONTRAST)
        val weaker = scrim.copy(alpha = scrim.alpha - 0.02f)
        assertTrue(readsAt(weaker, ground, ink, 0xFFFFFF, label) < PlayerColors.TEXT_CONTRAST)
    }

    @Test
    fun `the header on a dark cover keeps the usual scrim`() {
        val scrim = scrimOver(cover(0x2A0F0B), ground = 0x2A0F0B, ink = 0xFCEAE6)
        assertEquals(HeaderScrim(PlayerColors.SCRIM_ALPHA, statusBar, statusBar + PlayerColors.SCRIM_DP), scrim)
    }

    @Test
    fun `one light spot under the label is enough to raise it`() {
        val ground = 0x1C1924
        val ink = 0xEEE7F8
        val pixels = IntArray(112 * 112) { (0xFF shl 24) or 0x2E2148 }
        // A white dot where "Spotify on guchu" sits (112 px over a 390 dp cover; the label is 42 to 62 dp down).
        for (y in 12..15) for (x in 50..60) pixels[y * 112 + x] = (0xFF shl 24) or 0xFFFFFF
        val scrim = scrimOver(PlayerColors.measure(pixels, 112, 112), ground, ink)
        assertTrue(scrim.hold >= label.bottom)
        assertTrue(readsAt(scrim, ground, ink, 0xFFFFFF, label) >= PlayerColors.TEXT_CONTRAST)
    }

    @Test
    fun `in the light theme dark ink raises the scrim over a black cover`() {
        val ground = 0xFDF7FF
        val ink = 0x1D1A22
        val scrim = scrimOver(cover(0x000000), ground, ink)
        assertTrue(scrim.hold >= label.bottom)
        assertTrue(readsAt(scrim, ground, ink, 0x000000, label) >= PlayerColors.TEXT_CONTRAST)
        assertTrue(readsAt(scrim, ground, ink, 0x000000, glyph) >= PlayerColors.GLYPH_CONTRAST)
    }

    @Test
    fun `without a cover the usual scrim stays`() {
        val scrim = scrimOver(null, 0x18181B, 0xF2F2F3)
        assertEquals(PlayerColors.SCRIM_ALPHA, scrim.alpha, 0f)
    }

    @Test
    fun `a cropped wide cover is measured where it's drawn`() {
        // A 16:9 picture, white on its left quarter and dark elsewhere, cropped into a square: its
        // middle shows, so the white is off the box and nothing light is under the label.
        val w = 112
        val h = 63
        val pixels = IntArray(w * h) { i -> (0xFF shl 24) or (if (i % w < w / 4) 0xFFFFFF else 0x101010) }
        val light = checkNotNull(PlayerColors.measure(pixels, w, h))
        val worst = checkNotNull(PlayerColors.worstUnder(light, width, width, label, lightInk = true))
        assertTrue("got ${Integer.toHexString(worst)}", worst and 0xFF < 0x40)
    }

    @Test
    fun `a scrim's alpha fades from its hold to its end`() {
        val scrim = HeaderScrim(0.8f, 20f, 120f)
        assertEquals(0.8f, scrim.alphaAt(0f), 0f)
        assertEquals(0.8f, scrim.alphaAt(20f), 0f)
        assertEquals(0.4f, scrim.alphaAt(70f), 0.0001f)
        assertEquals(0f, scrim.alphaAt(120f), 0f)
        assertEquals(0f, scrim.alphaAt(500f), 0f)
    }

    @Test
    fun `the mini player dims a near-white cover further so its second line keeps its contrast`() {
        val blocks = IntArray(16 * 16) { (0xFF shl 24) or 0xFBF8F2 }
        val dim = PlayerColors.miniDim(PlayerColors.brightest(blocks))
        assertTrue("dim $dim", dim < PlayerColors.MINI_DIM)
        val v = (PlayerColors.brightest(blocks) * dim * 255f + 0.5f).toInt()
        val played = PlayerColors.over(0xFFFFFF, PlayerColors.MINI_FILL_ALPHA, (v shl 16) or (v shl 8) or v)
        val muted = PlayerColors.over(0xFFFFFF, PlayerColors.MINI_MUTED_ALPHA, played)
        assertTrue(PlayerColors.contrast(muted, played) >= PlayerColors.TEXT_CONTRAST)
    }

    @Test
    fun `the mini player keeps its usual dim on a dark cover`() {
        val blocks = IntArray(16 * 16) { (0xFF shl 24) or 0x56408A }
        assertEquals(PlayerColors.MINI_DIM, PlayerColors.miniDim(PlayerColors.brightest(blocks)), 0f)
    }

    @Test
    fun `the backdrop keeps the cover's shape in a few blocks`() {
        assertEquals(16 to 16, PlayerColors.backdropSize(112, 112))
        assertEquals(16 to 9, PlayerColors.backdropSize(112, 63))
        assertEquals(9 to 16, PlayerColors.backdropSize(63, 112))
        assertEquals(4 to 2, PlayerColors.backdropSize(4, 2))
    }

    @Test
    fun `blocks average their pixels`() {
        val pixels = intArrayOf(0x000000, 0xFFFFFF, 0x000000, 0xFFFFFF).map { (0xFF shl 24) or it }.toIntArray()
        val blocks = PlayerColors.blockAverages(pixels, 2, 2, 1, 1)
        assertEquals(1, blocks.size)
        assertEquals(0xFF7F7F7F.toInt(), blocks[0])
    }

    @Test
    fun `saturating grey changes nothing`() {
        assertEquals(0x808080, PlayerColors.saturate(0x808080, PlayerColors.MINI_SATURATION))
        val red = PlayerColors.saturate(0xC04040, PlayerColors.MINI_SATURATION)
        assertTrue(red shr 16 and 0xFF > 0xC0)
    }
}
