// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.elevated

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class SubmixLevelTest {
    private fun pcm(vararg samples: Int): ByteArray {
        val bytes = ByteArray(samples.size * 2)
        samples.forEachIndexed { i, s ->
            bytes[2 * i] = s.toByte()
            bytes[2 * i + 1] = (s shr 8).toByte()
        }
        return bytes
    }

    private fun samples(bytes: ByteArray): List<Int> =
        (0 until bytes.size / 2).map { (bytes[2 * it].toInt() and 0xff) or (bytes[2 * it + 1].toInt() shl 8) }

    @Test
    fun theGainUndoesWhatAndroidTurnedDownUpToACap() {
        assertEquals(1f, SubmixLevel.gainFor(0f), 0f)
        assertEquals(10f, SubmixLevel.gainFor(-20f), 0.001f)
        assertEquals(31.62f, SubmixLevel.gainFor(-30f), 0.01f)
        // A volume of nothing at all isn't turned up without end.
        val most = SubmixLevel.gainFor(-SubmixLevel.MAX_GAIN_DB)
        assertEquals(most, SubmixLevel.gainFor(-96f), 0f)
        assertEquals(most, SubmixLevel.gainFor(Float.NEGATIVE_INFINITY), 0f)
        // Android can't say, or says louder than full: left as it is.
        assertEquals(1f, SubmixLevel.gainFor(Float.NaN), 0f)
        assertEquals(1f, SubmixLevel.gainFor(3f), 0f)
    }

    @Test
    fun quietSamplesAreMultipliedExactly() {
        val bytes = pcm(100, -100, 0, 2000, -2000)
        SubmixLevel.amplify(bytes, bytes.size, 10f)
        assertEquals(listOf(1000, -1000, 0, 20000, -20000), samples(bytes))
    }

    @Test
    fun loudSamplesBendTowardsFullScaleAndNeverWrapAround() {
        val quiet = intArrayOf(2700, 3000, 5000, 20000, 32767)
        val bytes = pcm(*quiet, -3000, -32768)
        SubmixLevel.amplify(bytes, bytes.size, 10f)
        val out = samples(bytes)
        // Still in order, none quieter than the one before, and inside 16 bits.
        for (i in 1 until quiet.size) assertTrue("$out", out[i] >= out[i - 1])
        assertTrue("$out", out[0] < out[1] && out[1] < out[2])
        assertTrue("$out", out[quiet.size - 1] <= 32767)
        assertTrue("$out", out[1] in 26215..30000)
        // The same curve below zero.
        assertEquals(-out[1], out[quiet.size])
        assertTrue("$out", out.last() >= -32768 && out.last() < -32000)
    }

    @Test
    fun onlyWholeSamplesWithinTheLengthAreTouched() {
        val bytes = pcm(100, 100, 100)
        SubmixLevel.amplify(bytes, 3, 2f)
        assertEquals(listOf(200, 100, 100), samples(bytes))
        val same = pcm(100, 100)
        SubmixLevel.amplify(same, same.size, 1f)
        assertArrayEquals(pcm(100, 100), same)
    }
}
