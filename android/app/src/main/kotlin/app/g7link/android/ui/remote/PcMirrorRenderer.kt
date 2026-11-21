// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import android.graphics.Matrix
import android.graphics.Paint
import android.view.TextureView
import kotlin.math.abs
import kotlin.math.floor
import kotlin.math.roundToInt

/**
 * Where the PC's picture shows in its box ([boxWidth] x [boxHeight] pixels): fitted,
 * or zoomed in up to [MAX_SCALE] times that and moved. [scale] is how many times the
 * fitted size it shows at; ([centerX], [centerY]) is the point of the PC's screen
 * (`0..1` across and down) at the middle of the box. The PC's screen is [pcWidth] x
 * [pcHeight] pixels.
 *
 * Besides fit it settles on exact pixels: one, two or more phone pixels to each of
 * the PC's ([snaps]), where text is drawn without blur.
 */
internal data class PcMirrorZoom(
    val boxWidth: Float = 0f,
    val boxHeight: Float = 0f,
    val pcWidth: Int = 16,
    val pcHeight: Int = 9,
    val scale: Float = 1f,
    val centerX: Float = 0.5f,
    val centerY: Float = 0.5f,
) {
    private val aspect: Float get() = if (pcWidth > 0 && pcHeight > 0) pcWidth.toFloat() / pcHeight else 16f / 9f

    /** The picture fitted in the box, as it shows at [scale] 1. */
    val fit: VideoViewportRect get() = PcMirrorMath.computeVideoViewport(boxWidth, boxHeight, aspect)

    /** Where the picture shows now, in the box's pixels; it may reach past the box. */
    val rect: VideoViewportRect
        get() {
            val fitted = fit
            val w = fitted.width * scale
            val h = fitted.height * scale
            return VideoViewportRect(
                left = place(boxWidth.coerceAtLeast(1f), w, centerX),
                top = place(boxHeight.coerceAtLeast(1f), h, centerY),
                width = w,
                height = h,
            )
        }

    /** Whether it shows larger than fitted. */
    val isZoomed: Boolean get() = scale > 1f + EPSILON

    /** Phone pixels to each of the PC's, as it shows now. */
    val pixelRatio: Float get() = fit.width * scale / pcWidth.coerceAtLeast(1)

    /**
     * The scales it settles on: fit, then each whole number of phone pixels to
     * each of the PC's that's larger than fitted, up to [MAX_SCALE].
     */
    val snaps: List<Float>
        get() {
            val fitRatio = fit.width / pcWidth.coerceAtLeast(1)
            if (!fitRatio.isFinite() || fitRatio <= 0f) return listOf(1f)
            val out = mutableListOf(1f)
            var k = floor(fitRatio).toInt().coerceAtLeast(1)
            while (true) {
                val s = k / fitRatio
                if (s > MAX_SCALE + EPSILON) break
                if (s > 1f + SNAP_APART) out += s
                k++
            }
            return out
        }

    /** Whether it shows at exact pixels: a whole number of phone pixels to each of the PC's. */
    val isExact: Boolean
        get() {
            val ratio = pixelRatio
            return ratio.isFinite() && ratio >= 1f - EPSILON && abs(ratio - ratio.roundToInt()) <= EXACT_RATIO
        }

    /** What the zoom chip says: null for fit, else the phone pixels to each of the PC's, as a percentage. */
    val percent: Int? get() = if (isZoomed) (pixelRatio * 100f).roundToInt() else null

    /** The same picture in a box of [width] x [height] pixels. */
    fun inBox(width: Float, height: Float): PcMirrorZoom = copy(boxWidth = width, boxHeight = height).clamped()

    /** The same zoom on a PC screen of [width] x [height] pixels. */
    fun ofScreen(width: Int, height: Int): PcMirrorZoom =
        if (width <= 0 || height <= 0) this else copy(pcWidth = width, pcHeight = height).clamped()

    /**
     * Scaled by [factor] about ([focusX], [focusY]) in the box, so the part of the
     * PC's screen under the fingers stays under them, then moved by ([dx], [dy]).
     */
    fun zoomBy(factor: Float, focusX: Float, focusY: Float, dx: Float = 0f, dy: Float = 0f): PcMirrorZoom {
        if (!factor.isFinite() || factor <= 0f || !dx.isFinite() || !dy.isFinite()) return this
        val before = rect
        if (before.width <= 0f || before.height <= 0f) return this
        val px = (focusX - before.left) / before.width
        val py = (focusY - before.top) / before.height
        val next = (scale * factor).coerceIn(1f, MAX_SCALE)
        val fitted = fit
        val w = fitted.width * next
        val h = fitted.height * next
        val left = focusX + dx - px * w
        val top = focusY + dy - py * h
        return copy(
            scale = next,
            centerX = (boxWidth / 2f - left) / w,
            centerY = (boxHeight / 2f - top) / h,
        ).clamped()
    }

    /** Moved by ([dx], [dy]) box pixels. */
    fun panBy(dx: Float, dy: Float): PcMirrorZoom = zoomBy(1f, boxWidth / 2f, boxHeight / 2f, dx, dy)

    /** At [target] times fitted, about the middle of the box. */
    fun at(target: Float): PcMirrorZoom = zoomBy(target / scale, boxWidth / 2f, boxHeight / 2f)

    /**
     * After a pinch: onto the snap nearest [scale] when it's within [SNAP_TOLERANCE]
     * of one. Returns the zoom and the snap it settled on, if it did.
     */
    fun settle(): Pair<PcMirrorZoom, Float?> {
        val nearest = snaps.minByOrNull { abs(it - scale) / it } ?: return this to null
        if (abs(nearest - scale) / nearest > SNAP_TOLERANCE) return this to null
        return at(nearest) to nearest
    }

    /** The zoom chip's tap: fit, then the first [CYCLE_STEPS] exact-pixel scales, then fit again. */
    fun cycle(): PcMirrorZoom {
        val steps = snaps.drop(1).take(CYCLE_STEPS)
        val next = steps.firstOrNull { it > scale + EPSILON } ?: 1f
        return at(next)
    }

    /** Back to fit. */
    fun fitted(): PcMirrorZoom = copy(scale = 1f, centerX = 0.5f, centerY = 0.5f)

    /**
     * The point of the PC's screen (`0..1` across and down) under ([x], [y]) in the
     * box, or null off the picture unless [clampOutside].
     */
    fun toPc(x: Float, y: Float, clampOutside: Boolean = true): Pair<Float, Float>? =
        PcMirrorMath.mapTouchToNormalized(x, y, rect, clampOutside)

    /** The part of the PC's screen in sight (`0..1` across and down), for the mini-map. */
    val visible: VideoViewportRect
        get() {
            val r = rect
            if (r.width <= 0f || r.height <= 0f) return VideoViewportRect(0f, 0f, 1f, 1f)
            val left = ((0f - r.left) / r.width).coerceIn(0f, 1f)
            val top = ((0f - r.top) / r.height).coerceIn(0f, 1f)
            val right = ((boxWidth - r.left) / r.width).coerceIn(0f, 1f)
            val bottom = ((boxHeight - r.top) / r.height).coerceIn(0f, 1f)
            return VideoViewportRect(left, top, (right - left).coerceAtLeast(0f), (bottom - top).coerceAtLeast(0f))
        }

    /** Kept in reach: a picture larger than the box covers it, a smaller one sits in its middle. */
    private fun clamped(): PcMirrorZoom {
        val s = if (scale.isFinite()) scale.coerceIn(1f, MAX_SCALE) else 1f
        val fitted = fit
        return copy(
            scale = s,
            centerX = clampCenter(boxWidth, fitted.width * s, centerX),
            centerY = clampCenter(boxHeight, fitted.height * s, centerY),
        )
    }

    companion object {
        /** The most it zooms in: four times fitted. */
        const val MAX_SCALE = 4f

        /** How near a pinch has to end to a snap to land on it, as a share of the snap. */
        const val SNAP_TOLERANCE = 0.06f

        /** Exact-pixel steps the chip goes through after fit. */
        const val CYCLE_STEPS = 2

        /** An exact scale this close above fit is fit, for the chip and for snapping. */
        private const val SNAP_APART = 0.04f
        private const val EPSILON = 0.002f

        /** Phone pixels to each of the PC's this near a whole number count as exact. */
        private const val EXACT_RATIO = 0.005f

        /** The box's edge for a picture [size] wide at [center], kept so it covers the box or sits centred in it. */
        private fun place(box: Float, size: Float, center: Float): Float =
            if (size <= box) (box - size) / 2f else (box / 2f - center * size).coerceIn(box - size, 0f)

        private fun clampCenter(box: Float, size: Float, center: Float): Float {
            if (size <= 0f || size <= box || !center.isFinite()) return 0.5f
            val half = box / 2f / size
            return center.coerceIn(half, 1f - half)
        }
    }
}

/**
 * Shows the picture on [view] (laid out over the whole box) where [zoom] puts it,
 * drawn pixel for pixel at exact scales and smoothed at any other.
 */
internal fun TextureView.showZoomed(zoom: PcMirrorZoom) {
    val w = width.toFloat()
    val h = height.toFloat()
    if (w <= 0f || h <= 0f) return
    val r = zoom.rect
    setTransform(
        Matrix().apply {
            setScale(r.width / w, r.height / h)
            postTranslate(r.left, r.top)
        },
    )
    // Whether the layer is drawn nearest-pixel follows its paint's filter flag where
    // the system honours it; elsewhere this changes nothing. The view has no getter
    // for its layer paint: its tag says which it has.
    val sharp = zoom.isExact && zoom.isZoomed
    if (tag != sharp) {
        tag = sharp
        setLayerPaint(Paint().apply { isFilterBitmap = !sharp })
    }
}
