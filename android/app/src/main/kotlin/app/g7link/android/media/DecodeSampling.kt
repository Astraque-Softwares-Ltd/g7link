// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.media

/**
 * The largest power-of-two `inSampleSize` that still decodes a [width]×[height] image at
 * least as large as it is shown when fitted inside [maxWidth]×[maxHeight] (never enlarged),
 * so the decode skips pixels no one sees without losing any that are.
 */
fun sampleSizeToCover(width: Int, height: Int, maxWidth: Int, maxHeight: Int): Int {
    if (width <= 0 || height <= 0 || maxWidth <= 0 || maxHeight <= 0) return 1
    var sample = 1
    while (sample < MAX_SAMPLE) {
        val next = sample * 2L
        if (next * maxWidth > width && next * maxHeight > height) break
        sample *= 2
    }
    return sample
}

private const val MAX_SAMPLE = 1 shl 20
