// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.roundToInt

/** Zooming the PC's picture: fit, exact pixels, the chip and the mini-map. */
class PcMirrorZoomTest {
    /** The laptop's 1366 x 768 screen in the Pixel's portrait card: 1234 px across. */
    private val portrait = PcMirrorZoom(pcWidth = 1366, pcHeight = 768).inBox(1234f, 1100f)

    /** The same in landscape, between the rails: 1344 px tall, so 1.75 phone pixels to each. */
    private val landscape = PcMirrorZoom(pcWidth = 1366, pcHeight = 768).inBox(2390.5f, 1344f)

    @Test
    fun itStartsFitted() {
        assertFalse(portrait.isZoomed)
        assertNull("the chip says Fit", portrait.percent)
        val r = portrait.rect
        assertEquals(1234f, r.width, 0.01f)
        assertEquals(0f, r.left, 0.01f)
        // Letterboxed, in the middle of the card.
        assertEquals((1100f - r.height) / 2f, r.top, 0.01f)
    }

    @Test
    fun theSnapsAreFitThenWholePhonePixelsToEachOfThePcs() {
        val portraitRatios = portrait.snaps.map { (portrait.at(it).pixelRatio * 100f).roundToInt() }
        assertEquals(listOf(90, 100, 200, 300), portraitRatios)
        // Landscape fits at 1.75: one to one is smaller than fit, so the first exact step is 200%.
        val landscapeRatios = landscape.snaps.drop(1).map { (landscape.at(it).pixelRatio * 100f).roundToInt() }
        assertEquals(listOf(200, 300, 400, 500, 600, 700), landscapeRatios)
        assertTrue(landscape.snaps.all { it <= PcMirrorZoom.MAX_SCALE + 0.01f })
    }

    @Test
    fun theChipCyclesFitThenTwoExactSteps() {
        var zoom = portrait
        val seen = mutableListOf<Int?>()
        repeat(4) {
            seen += zoom.percent
            zoom = zoom.cycle()
        }
        assertEquals(listOf(null, 100, 200, null), seen)

        var wide = landscape
        val seenWide = mutableListOf<Int?>()
        repeat(4) {
            seenWide += wide.percent
            wide = wide.cycle()
        }
        assertEquals(listOf(null, 200, 300, null), seenWide)
    }

    @Test
    fun aPinchThatEndsNearASnapLandsOnIt() {
        val mid = portrait.fit
        val near100 = portrait.zoomBy(1.13f, mid.width / 2f, 550f)
        val (settled, snap) = near100.settle()
        assertNotNull(snap)
        assertEquals(100, settled.percent)
        assertTrue("exact pixels", settled.isExact)

        val nearFit = portrait.zoomBy(1.03f, 600f, 550f)
        val (back, fitSnap) = nearFit.settle()
        assertEquals(1f, fitSnap!!, 0.0001f)
        assertFalse(back.isZoomed)
    }

    @Test
    fun aPinchBetweenSnapsStaysWhereItIs() {
        val between = portrait.zoomBy(1.6f, 600f, 550f)
        val (settled, snap) = between.settle()
        assertNull(snap)
        assertEquals(between, settled)
        assertFalse(settled.isExact)
        // 1.6 times fit, at 0.903 phone pixels to the PC's when fitted.
        assertEquals(145, settled.percent)
    }

    @Test
    fun theFingersKeepTheirPlaceOnThePicture() {
        // Two fingers left of the middle: the PC's point under them stays under them.
        val before = portrait.toPc(300f, 550f)!!
        val zoomed = portrait.zoomBy(2f, 300f, 550f)
        val after = zoomed.toPc(300f, 550f)!!
        assertEquals(0.243f, before.first, 0.001f)
        assertEquals(before.first, after.first, 0.001f)
        assertEquals(before.second, after.second, 0.001f)
        assertEquals(2f, zoomed.scale, 0.0001f)
    }

    @Test
    fun itStaysBetweenFitAndFourTimes() {
        assertEquals(1f, portrait.zoomBy(0.3f, 600f, 550f).scale, 0.0001f)
        assertEquals(PcMirrorZoom.MAX_SCALE, portrait.zoomBy(50f, 600f, 550f).scale, 0.0001f)
        assertEquals(portrait, portrait.zoomBy(Float.NaN, 600f, 550f))
    }

    @Test
    fun aZoomedPictureCoversItsBoxHoweverFarItsMoved() {
        val zoomed = portrait.zoomBy(3f, 617f, 550f)
        for ((dx, dy) in listOf(5000f to 0f, -5000f to 0f, 0f to 5000f, 0f to -5000f)) {
            val r = zoomed.panBy(dx, dy).rect
            assertTrue("left ${r.left}", r.left <= 0.01f)
            assertTrue("top ${r.top}", r.top <= 0.01f)
            assertTrue("right ${r.right}", r.right >= 1234f - 0.01f)
            assertTrue("bottom ${r.bottom}", r.bottom >= 1100f - 0.01f)
        }
        // Moved to the far left, the PC's left edge is at the box's.
        assertEquals(0f, zoomed.panBy(5000f, 0f).toPc(0f, 550f)!!.first, 0.001f)
    }

    @Test
    fun theMiniMapShowsThePartInSight() {
        val whole = portrait.visible
        assertEquals(0f, whole.left, 0.001f)
        assertEquals(1f, whole.width, 0.001f)
        val zoomed = landscape.zoomBy(2f, 2390.5f / 2f, 672f)
        val part = zoomed.visible
        assertEquals(0.5f, part.width, 0.001f)
        assertEquals(0.5f, part.height, 0.001f)
        assertEquals(0.25f, part.left, 0.001f)
        assertEquals(0.25f, part.top, 0.001f)
    }

    @Test
    fun aNewBoxOrScreenKeepsTheZoomInReach() {
        val zoomed = portrait.zoomBy(2f, 1000f, 550f)
        // The keyboard came up: a shorter box, the same zoom, still covering it.
        val shorter = zoomed.inBox(1234f, 500f)
        assertEquals(zoomed.scale, shorter.scale, 0.0001f)
        assertTrue(shorter.rect.left <= 0f && shorter.rect.right >= 1234f)
        // Back to fit, centred.
        val fitted = zoomed.fitted()
        assertEquals(portrait, fitted)
        // A screen of another size keeps the zoom relative to fit.
        assertEquals(zoomed.scale, zoomed.ofScreen(1920, 1080).scale, 0.0001f)
        assertEquals(zoomed, zoomed.ofScreen(0, 0))
    }
}
