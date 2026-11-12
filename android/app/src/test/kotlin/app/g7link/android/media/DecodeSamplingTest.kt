// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.media

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class DecodeSamplingTest {

    @Test
    fun `an image no larger than the box decodes at full size`() {
        assertEquals(1, sampleSizeToCover(200, 100, 1080, 360))
        assertEquals(1, sampleSizeToCover(4000, 3000, 2400, 2400))
    }

    @Test
    fun `a large photo halves until one more halving would go below the long side`() {
        assertEquals(2, sampleSizeToCover(8160, 6120, 2400, 2400))
        assertEquals(2, sampleSizeToCover(4800, 100, 2400, 2400))
        assertEquals(1, sampleSizeToCover(4799, 100, 2400, 2400))
    }

    @Test
    fun `the decoded image still covers the size it is shown at`() {
        val cases = listOf(
            intArrayOf(1080, 2400, 1080, 360),
            intArrayOf(4000, 3000, 1080, 360),
            intArrayOf(8000, 2000, 1080, 360),
            intArrayOf(12_000, 9_000, 2400, 2400),
            intArrayOf(3, 50_000, 1440, 420),
        )
        for ((w, h, boxW, boxH) in cases) {
            val sample = sampleSizeToCover(w, h, boxW, boxH)
            val scale = minOf(1.0, boxW.toDouble() / w, boxH.toDouble() / h)
            val shownW = w * scale
            val shownH = h * scale
            assertTrue("$w×$h in $boxW×$boxH", w / sample >= shownW.toInt() && h / sample >= shownH.toInt())
            val coarser = sample * 2
            assertTrue(
                "$w×$h in $boxW×$boxH could sample $coarser",
                w / coarser < shownW.toInt() || h / coarser < shownH.toInt(),
            )
        }
    }

    @Test
    fun `a square box keeps the long-side rule it replaces`() {
        for (w in listOf(1, 719, 720, 1439, 1440, 2880, 4000, 9000)) {
            for (h in listOf(1, 720, 1440, 3000, 12_000)) {
                var legacy = 1
                while (maxOf(w, h) / (legacy * 2) >= 720) legacy *= 2
                assertEquals("$w×$h", legacy, sampleSizeToCover(w, h, 720, 720))
            }
        }
    }

    @Test
    fun `missing or empty sizes decode at full size`() {
        assertEquals(1, sampleSizeToCover(0, 0, 1080, 360))
        assertEquals(1, sampleSizeToCover(-1, 500, 1080, 360))
        assertEquals(1, sampleSizeToCover(4000, 3000, 0, 360))
    }
}
