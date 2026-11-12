// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.capture

import kotlin.math.abs
import kotlin.math.roundToInt

/** A crop in fractions of the picture: 0 to 1 on each axis, left before right, top above bottom. */
data class CropRect(val left: Float, val top: Float, val right: Float, val bottom: Float) {
    val width: Float get() = right - left
    val height: Float get() = bottom - top

    companion object {
        /** The whole picture. */
        val FULL = CropRect(0f, 0f, 1f, 1f)
    }
}

/** What a drag on the crop moves. */
enum class CropHandle { TopLeft, Top, TopRight, Right, BottomRight, Bottom, BottomLeft, Left, Move }

/** Where a picture shows in its box, in the box's pixels. */
data class FitRect(val left: Float, val top: Float, val width: Float, val height: Float)

/** A crop in a picture's own pixels; right and bottom are exclusive. */
data class PixelRect(val left: Int, val top: Int, val right: Int, val bottom: Int) {
    val width: Int get() = right - left
    val height: Int get() = bottom - top
}

/** Pure math for cropping a screenshot of a PC: what a touch grabs, and how a drag moves it. */
object CropMath {
    /**
     * The handle under a touch at ([x], [y]), in fractions of the picture: a corner,
     * an edge, or the inside (to move the whole crop). [slopX] and [slopY] are how
     * near an edge counts, also in fractions. Null when the touch misses the crop.
     */
    fun handleAt(rect: CropRect, x: Float, y: Float, slopX: Float, slopY: Float): CropHandle? {
        val nearLeft = abs(x - rect.left) <= slopX
        val nearRight = abs(x - rect.right) <= slopX
        val nearTop = abs(y - rect.top) <= slopY
        val nearBottom = abs(y - rect.bottom) <= slopY
        val alongX = x >= rect.left - slopX && x <= rect.right + slopX
        val alongY = y >= rect.top - slopY && y <= rect.bottom + slopY
        return when {
            nearLeft && nearTop -> CropHandle.TopLeft
            nearRight && nearTop -> CropHandle.TopRight
            nearLeft && nearBottom -> CropHandle.BottomLeft
            nearRight && nearBottom -> CropHandle.BottomRight
            nearTop && alongX -> CropHandle.Top
            nearBottom && alongX -> CropHandle.Bottom
            nearLeft && alongY -> CropHandle.Left
            nearRight && alongY -> CropHandle.Right
            x > rect.left && x < rect.right && y > rect.top && y < rect.bottom -> CropHandle.Move
            else -> null
        }
    }

    /**
     * Moves [handle] by ([dx], [dy]) fractions. The crop stays inside the picture and
     * at least [minWidth] by [minHeight] (fractions; never more than it already is).
     */
    fun drag(rect: CropRect, handle: CropHandle, dx: Float, dy: Float, minWidth: Float, minHeight: Float): CropRect {
        if (handle == CropHandle.Move) {
            val mx = clamp(dx, -rect.left, 1f - rect.right)
            val my = clamp(dy, -rect.top, 1f - rect.bottom)
            return CropRect(rect.left + mx, rect.top + my, rect.right + mx, rect.bottom + my)
        }
        val minW = minWidth.coerceIn(0f, rect.width.coerceAtLeast(0f))
        val minH = minHeight.coerceIn(0f, rect.height.coerceAtLeast(0f))
        var left = rect.left
        var top = rect.top
        var right = rect.right
        var bottom = rect.bottom
        if (handle in LEFTS) left = clamp(left + dx, 0f, right - minW)
        if (handle in RIGHTS) right = clamp(right + dx, left + minW, 1f)
        if (handle in TOPS) top = clamp(top + dy, 0f, bottom - minH)
        if (handle in BOTTOMS) bottom = clamp(bottom + dy, top + minH, 1f)
        return CropRect(left, top, right, bottom)
    }

    /** The crop in pixels of a [width] x [height] picture: inside it, and at least one pixel. */
    fun pixels(rect: CropRect, width: Int, height: Int): PixelRect {
        val w = width.coerceAtLeast(1)
        val h = height.coerceAtLeast(1)
        val left = (rect.left * w).roundToInt().coerceIn(0, w - 1)
        val top = (rect.top * h).roundToInt().coerceIn(0, h - 1)
        val right = (rect.right * w).roundToInt().coerceIn(left + 1, w)
        val bottom = (rect.bottom * h).roundToInt().coerceIn(top + 1, h)
        return PixelRect(left, top, right, bottom)
    }

    /** Where a [imageWidth] x [imageHeight] picture shows, whole and centred, in a [boxWidth] x [boxHeight] box. */
    fun fit(boxWidth: Float, boxHeight: Float, imageWidth: Float, imageHeight: Float): FitRect {
        if (boxWidth <= 0f || boxHeight <= 0f || imageWidth <= 0f || imageHeight <= 0f) {
            return FitRect(0f, 0f, boxWidth.coerceAtLeast(0f), boxHeight.coerceAtLeast(0f))
        }
        val scale = minOf(boxWidth / imageWidth, boxHeight / imageHeight)
        val width = imageWidth * scale
        val height = imageHeight * scale
        return FitRect((boxWidth - width) / 2f, (boxHeight - height) / 2f, width, height)
    }

    /** Like coerceIn, without throwing when rounding puts [low] a hair above [high]: [high] wins. */
    private fun clamp(value: Float, low: Float, high: Float): Float = minOf(maxOf(value, low), high)

    private val LEFTS = setOf(CropHandle.TopLeft, CropHandle.Left, CropHandle.BottomLeft)
    private val RIGHTS = setOf(CropHandle.TopRight, CropHandle.Right, CropHandle.BottomRight)
    private val TOPS = setOf(CropHandle.TopLeft, CropHandle.Top, CropHandle.TopRight)
    private val BOTTOMS = setOf(CropHandle.BottomLeft, CropHandle.Bottom, CropHandle.BottomRight)
}
