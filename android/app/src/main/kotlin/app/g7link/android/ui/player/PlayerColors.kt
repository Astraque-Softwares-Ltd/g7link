// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.player

import kotlin.math.ceil
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow

/**
 * How light a cover is where the player puts things over it, measured once per cover from the
 * small picture: for each cell of a coarse grid, the lightest and the darkest of each channel
 * (0xRRGGBB). Text over a cell is checked against these, so no pixel in it can read worse.
 */
class CoverLight internal constructor(
    val cols: Int,
    val rows: Int,
    /** The picture's width over its height. */
    val aspect: Float,
    internal val lightest: IntArray,
    internal val darkest: IntArray,
)

/**
 * The full player's header scrim: the ground at [alpha] from the art's top edge, held to [hold]
 * and gone at [end] (dp from the art's top).
 */
data class HeaderScrim(val alpha: Float, val hold: Float, val end: Float) {
    fun alphaAt(y: Float): Float = when {
        y <= hold -> alpha
        y >= end -> 0f
        else -> alpha * (1f - (y - hold) / (end - hold))
    }
}

/** A box over the art, in dp from its top-left corner. */
data class Area(val left: Float, val top: Float, val right: Float, val bottom: Float)

/**
 * The player's colour arithmetic, on 0xRRGGBB ints so it runs in JVM tests: WCAG contrast,
 * the header scrim over the cover, and how far the mini player dims its blurred cover.
 */
object PlayerColors {
    /** Text over the cover (the header's "Spotify on guchu", the mini player's lines). */
    const val TEXT_CONTRAST = 4.5

    /** A glyph over the cover (the header's chevron). */
    const val GLYPH_CONTRAST = 3.0

    /** The header scrim at the art's top edge, before it's raised. */
    const val SCRIM_ALPHA = 0.72f

    /** How far below the status bar the header scrim fades out. */
    const val SCRIM_DP = 96f

    /** How far past the header a raised scrim takes to fade out. */
    private const val RAISED_FADE_DP = 32f

    /** The mini player's cover: blurred, saturated a little and dimmed this far. */
    const val MINI_DIM = 0.4f
    const val MINI_SATURATION = 1.2f

    /** The mini player's played part: white at this alpha over the cover. */
    const val MINI_FILL_ALPHA = 0.14f

    /** The mini player's second line: white at this alpha. */
    const val MINI_MUTED_ALPHA = 0.72f

    /** The mini player's backdrop: about this many blocks along the cover's longer side. */
    const val BACKDROP_BLOCKS = 16

    const val WHITE = 0xFFFFFF

    private const val GRID_COLS = 16
    private const val GRID_ROWS = 32

    /** WCAG relative luminance of an sRGB colour. */
    fun luminance(rgb: Int): Double {
        fun linear(channel: Int): Double {
            val s = channel / 255.0
            return if (s <= 0.04045) s / 12.92 else ((s + 0.055) / 1.055).pow(2.4)
        }
        return 0.2126 * linear(rgb shr 16 and 0xFF) + 0.7152 * linear(rgb shr 8 and 0xFF) + 0.0722 * linear(rgb and 0xFF)
    }

    /** WCAG contrast ratio, 1 to 21. */
    fun contrast(a: Int, b: Int): Double {
        val la = luminance(a)
        val lb = luminance(b)
        return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)
    }

    /** [top] at [alpha] over [under], per sRGB channel, as the compositor draws it. */
    fun over(top: Int, alpha: Float, under: Int): Int {
        val a = alpha.coerceIn(0f, 1f)
        fun mix(shift: Int): Int {
            val t = top shr shift and 0xFF
            val u = under shr shift and 0xFF
            return (t * a + u * (1f - a) + 0.5f).toInt().coerceIn(0, 255)
        }
        return (mix(16) shl 16) or (mix(8) shl 8) or mix(0)
    }

    private fun channelMax(a: Int, b: Int): Int =
        (max(a shr 16 and 0xFF, b shr 16 and 0xFF) shl 16) or
            (max(a shr 8 and 0xFF, b shr 8 and 0xFF) shl 8) or
            max(a and 0xFF, b and 0xFF)

    private fun channelMin(a: Int, b: Int): Int =
        (min(a shr 16 and 0xFF, b shr 16 and 0xFF) shl 16) or
            (min(a shr 8 and 0xFF, b shr 8 and 0xFF) shl 8) or
            min(a and 0xFF, b and 0xFF)

    /** [pixels] (ARGB, [width] × [height], row by row) as a [CoverLight]; null for an empty picture. */
    fun measure(pixels: IntArray, width: Int, height: Int): CoverLight? {
        if (width <= 0 || height <= 0 || pixels.size < width * height) return null
        val cols = min(GRID_COLS, width)
        val rows = min(GRID_ROWS, height)
        val lightest = IntArray(cols * rows)
        val darkest = IntArray(cols * rows) { WHITE }
        for (y in 0 until height) {
            val row = y * rows / height
            for (x in 0 until width) {
                val cell = row * cols + x * cols / width
                val px = pixels[y * width + x] and WHITE
                lightest[cell] = channelMax(lightest[cell], px)
                darkest[cell] = channelMin(darkest[cell], px)
            }
        }
        return CoverLight(cols, rows, width.toFloat() / height, lightest, darkest)
    }

    /**
     * The colour that reads worst against light ink ([lightInk]: the lightest) or dark ink (the
     * darkest) anywhere under [area], with the cover drawn cropped and centred in a [boxWidth] ×
     * [boxHeight] box. Null when the area is off the picture.
     */
    internal fun worstUnder(light: CoverLight, boxWidth: Float, boxHeight: Float, area: Area, lightInk: Boolean): Int? {
        if (boxWidth <= 0f || boxHeight <= 0f || light.aspect <= 0f) return null
        // ContentScale.Crop: the picture (aspect × 1) scaled until it covers the box.
        val scale = max(boxWidth / light.aspect, boxHeight)
        val drawnWidth = light.aspect * scale
        val drawnHeight = scale
        val offsetX = (drawnWidth - boxWidth) / 2f
        val offsetY = (drawnHeight - boxHeight) / 2f
        val u0 = ((area.left + offsetX) / drawnWidth).coerceIn(0f, 1f)
        val u1 = ((area.right + offsetX) / drawnWidth).coerceIn(0f, 1f)
        val v0 = ((area.top + offsetY) / drawnHeight).coerceIn(0f, 1f)
        val v1 = ((area.bottom + offsetY) / drawnHeight).coerceIn(0f, 1f)
        if (u1 <= u0 || v1 <= v0) return null
        val c0 = (u0 * light.cols).toInt().coerceIn(0, light.cols - 1)
        val c1 = (ceil(u1 * light.cols).toInt() - 1).coerceIn(c0, light.cols - 1)
        val r0 = (v0 * light.rows).toInt().coerceIn(0, light.rows - 1)
        val r1 = (ceil(v1 * light.rows).toInt() - 1).coerceIn(r0, light.rows - 1)
        var worst = if (lightInk) 0 else WHITE
        for (r in r0..r1) {
            for (c in c0..c1) {
                val cell = r * light.cols + c
                worst = if (lightInk) channelMax(worst, light.lightest[cell]) else channelMin(worst, light.darkest[cell])
            }
        }
        return worst
    }

    private class Check(val under: Int, val y: Float, val need: Double)

    /**
     * The scrim behind the full player's header: [ground] at [SCRIM_ALPHA] from the top edge,
     * fading out [SCRIM_DP] below the status bar, so [ink] keeps [TEXT_CONTRAST] over the
     * [label] and [GLYPH_CONTRAST] over the [glyph]. On a cover too light (or too dark, for
     * dark ink) for that, the scrim is raised: held at full strength past the header before it
     * fades, and made stronger than [SCRIM_ALPHA] only when holding isn't enough. Each check uses
     * the scrim where it is weakest under the piece, its bottom edge.
     */
    fun headerScrim(
        light: CoverLight?,
        ground: Int,
        ink: Int,
        boxWidth: Float,
        boxHeight: Float,
        statusBar: Float,
        label: Area,
        glyph: Area,
    ): HeaderScrim {
        val normal = HeaderScrim(SCRIM_ALPHA, statusBar, statusBar + SCRIM_DP)
        if (light == null) return normal
        val lightInk = luminance(ink) > luminance(ground)
        val checks = listOfNotNull(
            worstUnder(light, boxWidth, boxHeight, label, lightInk)?.let { Check(it, label.bottom, TEXT_CONTRAST) },
            worstUnder(light, boxWidth, boxHeight, glyph, lightInk)?.let { Check(it, glyph.bottom, GLYPH_CONTRAST) },
        )
        fun reads(scrim: HeaderScrim) = checks.all { contrast(ink, over(ground, scrim.alphaAt(it.y), it.under)) >= it.need }
        if (reads(normal)) return normal
        val hold = max(label.bottom, glyph.bottom)
        val end = max(normal.end, hold + RAISED_FADE_DP)
        // Raised: first held at full strength past the header, then stronger only if that isn't enough.
        val held = HeaderScrim(SCRIM_ALPHA, hold, end)
        if (reads(held)) return held
        if (!reads(HeaderScrim(1f, hold, end))) return HeaderScrim(1f, hold, end)
        var low = SCRIM_ALPHA
        var high = 1f
        repeat(16) {
            val mid = (low + high) / 2f
            if (reads(HeaderScrim(mid, hold, end))) high = mid else low = mid
        }
        return HeaderScrim(high, hold, end)
    }

    /** [rgb] saturated by [amount], as Compose's `ColorMatrix.setToSaturation` does it. */
    fun saturate(rgb: Int, amount: Float): Int {
        val rest = 1f - amount
        val wr = 0.213f * rest
        val wg = 0.715f * rest
        val wb = 0.072f * rest
        val r = (rgb shr 16 and 0xFF).toFloat()
        val g = (rgb shr 8 and 0xFF).toFloat()
        val b = (rgb and 0xFF).toFloat()
        fun clamp(v: Float) = (v + 0.5f).toInt().coerceIn(0, 255)
        return (clamp((wr + amount) * r + wg * g + wb * b) shl 16) or
            (clamp(wr * r + (wg + amount) * g + wb * b) shl 8) or
            clamp(wr * r + wg * g + (wb + amount) * b)
    }

    /** The backdrop's blocks for a [width] × [height] cover: [BACKDROP_BLOCKS] along its longer side. */
    fun backdropSize(width: Int, height: Int): Pair<Int, Int> {
        if (width <= 0 || height <= 0) return 1 to 1
        val cols = min(BACKDROP_BLOCKS, width)
        val rows = min(BACKDROP_BLOCKS, height)
        return if (width >= height) {
            cols to (cols * height / width).coerceIn(1, rows)
        } else {
            (rows * width / height).coerceIn(1, cols) to rows
        }
    }

    /** The average of each of [cols] × [rows] blocks of [pixels], opaque ARGB, row by row. */
    fun blockAverages(pixels: IntArray, width: Int, height: Int, cols: Int, rows: Int): IntArray {
        if (width <= 0 || height <= 0 || cols <= 0 || rows <= 0 || pixels.size < width * height) return IntArray(0)
        val sums = LongArray(cols * rows * 3)
        val counts = IntArray(cols * rows)
        for (y in 0 until height) {
            val row = y * rows / height
            for (x in 0 until width) {
                val block = row * cols + x * cols / width
                val px = pixels[y * width + x]
                sums[block * 3] += (px shr 16 and 0xFF).toLong()
                sums[block * 3 + 1] += (px shr 8 and 0xFF).toLong()
                sums[block * 3 + 2] += (px and 0xFF).toLong()
                counts[block]++
            }
        }
        return IntArray(cols * rows) { block ->
            val n = counts[block].coerceAtLeast(1)
            val r = (sums[block * 3] / n).toInt()
            val g = (sums[block * 3 + 1] / n).toInt()
            val b = (sums[block * 3 + 2] / n).toInt()
            (0xFF shl 24) or (r shl 16) or (g shl 8) or b
        }
    }

    /** The lightest channel among [blocks] once saturated as the mini player draws them, 0 to 1. */
    fun brightest(blocks: IntArray): Float {
        var top = 0
        for (block in blocks) {
            val px = saturate(block and WHITE, MINI_SATURATION)
            top = max(top, max(px shr 16 and 0xFF, max(px shr 8 and 0xFF, px and 0xFF)))
        }
        return top / 255f
    }

    /**
     * How far the mini player dims its cover: [MINI_DIM], or further when the cover is so light
     * that its second line (white at [MINI_MUTED_ALPHA]) wouldn't keep [TEXT_CONTRAST] over the
     * played part, the lightest ground under it. [brightest] is from [brightest]; the check
     * treats it as grey in every channel, which reads no better than the colour it stands for.
     */
    fun miniDim(brightest: Float): Float {
        fun reads(dim: Float): Boolean {
            val v = (brightest.coerceIn(0f, 1f) * dim * 255f + 0.5f).toInt().coerceIn(0, 255)
            val ground = over(WHITE, MINI_FILL_ALPHA, (v shl 16) or (v shl 8) or v)
            return contrast(over(WHITE, MINI_MUTED_ALPHA, ground), ground) >= TEXT_CONTRAST
        }
        if (reads(MINI_DIM)) return MINI_DIM
        var low = 0f
        var high = MINI_DIM
        repeat(16) {
            val mid = (low + high) / 2f
            if (reads(mid)) low = mid else high = mid
        }
        return low
    }
}
