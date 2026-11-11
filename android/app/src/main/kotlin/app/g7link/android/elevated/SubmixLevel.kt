// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.elevated

import kotlin.math.abs
import kotlin.math.pow
import kotlin.math.roundToInt
import kotlin.math.tanh

/**
 * Makes up, in the captured sound itself, for the volume Android applies to
 * the remote submix. The submix has a media volume of its own, low by
 * default, and what the helper captures comes out that much quieter. The
 * phone's volume is the user's: it isn't touched to fix that. The samples
 * are turned up by as much as Android turned them down instead.
 */
internal object SubmixLevel {
    /** The most the sound is turned up: beyond it, it is mostly the noise of 16 bits. */
    const val MAX_GAIN_DB = 36f

    /** Where loud samples start to be rounded off instead of cut, as a part of full scale. */
    private const val KNEE = 0.8f
    private const val FULL_SCALE = 32768f

    /**
     * How much to multiply samples by when Android plays them [volumeDb]
     * below full volume. 1 when it doesn't turn them down, or can't say.
     */
    fun gainFor(volumeDb: Float): Float {
        if (volumeDb.isNaN() || volumeDb >= 0f) return 1f
        return 10f.pow(minOf(-volumeDb, MAX_GAIN_DB) / 20f)
    }

    /**
     * Turns up the first [length] bytes of [pcm] (16-bit little-endian
     * samples) by [gain], in place. A sample that would pass full scale is
     * bent towards it rather than cut off flat, which would crackle.
     */
    fun amplify(pcm: ByteArray, length: Int, gain: Float) {
        if (gain <= 1f) return
        var at = 0
        val end = minOf(length, pcm.size) and 1.inv()
        while (at < end) {
            val sample = (pcm[at].toInt() and 0xff) or (pcm[at + 1].toInt() shl 8)
            val out = soften(sample * gain / FULL_SCALE)
            val value = (out * FULL_SCALE).roundToInt().coerceIn(-32768, 32767)
            pcm[at] = value.toByte()
            pcm[at + 1] = (value shr 8).toByte()
            at += 2
        }
    }

    /** [x] as it is up to the knee, and curved towards 1 (never past it) beyond. */
    private fun soften(x: Float): Float {
        val size = abs(x)
        if (size <= KNEE) return x
        val bent = KNEE + (1f - KNEE) * tanh((size - KNEE) / (1f - KNEE))
        return if (x < 0f) -bent else bent
    }
}
