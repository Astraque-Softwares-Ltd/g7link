// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.pow

/**
 * Maps phone gyroscope angular velocity (`Sensor.TYPE_GYROSCOPE`, in rad/s)
 * to relative PC pointer motion `(dx, dy)` in logical pixels, with a radial
 * dead zone against hand tremor and exponential smoothing.
 */
class AirMouseFilter(
    private val deadZoneRadPerSec: Float = DEFAULT_DEAD_ZONE_RAD_PER_SEC,
    private val smoothingAlpha: Float = DEFAULT_SMOOTHING_ALPHA,
    private val basePixelsPerRad: Float = DEFAULT_PIXELS_PER_RAD,
) {
    var smoothX: Float = 0f
        private set
    var smoothY: Float = 0f
        private set
    private var lastTimestampNs: Long = 0L

    /**
     * Processes a gyroscope sample at [timestampNs] and returns the relative
     * pointer step `(dx, dy)` in logical pixels.
     */
    fun onSample(
        wx: Float,
        wy: Float,
        wz: Float,
        timestampNs: Long,
        sensitivity: Float,
    ): Pair<Float, Float> {
        val dtSec = if (lastTimestampNs == 0L || timestampNs <= lastTimestampNs) {
            DEFAULT_DT_SEC
        } else {
            ((timestampNs - lastTimestampNs) * 1e-9f).coerceIn(MIN_DT_SEC, MAX_DT_SEC)
        }
        lastTimestampNs = timestampNs
        return step(wx, wy, wz, dtSec, sensitivity)
    }

    /**
     * Pure step with an explicit time delta [dtSec] (in seconds).
     *
     * Axis mapping:
     * - Horizontal (`dx` > 0 moves right): panning right rotates around -Z
     *   (when held flat) and -Y (when held upright), so `rawX = -(wz + wy)`.
     * - Vertical (`dy` > 0 moves down): tilting down rotates around -X, so
     *   `rawY = -wx`.
     */
    fun step(
        wx: Float,
        wy: Float,
        wz: Float,
        dtSec: Float,
        sensitivity: Float = 1.0f,
    ): Pair<Float, Float> {
        if (!wx.isFinite() || !wy.isFinite() || !wz.isFinite() || !dtSec.isFinite() || !sensitivity.isFinite()) {
            return 0f to 0f
        }
        val clampedDt = dtSec.coerceIn(MIN_DT_SEC, MAX_DT_SEC)
        val clampedSens = sensitivity.coerceIn(0.1f, 5.0f)

        val rawX = -(wz + wy)
        val rawY = -wx
        val (dzX, dzY) = applyDeadZone(rawX, rawY, deadZoneRadPerSec)

        val alpha = smoothingAlpha.coerceIn(0.05f, 1.0f)
        smoothX += alpha * (dzX - smoothX)
        smoothY += alpha * (dzY - smoothY)

        if (dzX == 0f && dzY == 0f && hypot(smoothX, smoothY) < SNAP_ZERO_EPSILON) {
            smoothX = 0f
            smoothY = 0f
            return 0f to 0f
        }

        val scale = clampedDt * basePixelsPerRad * clampedSens
        val dx = (smoothX * scale).coerceIn(-MAX_MOVE_DELTA, MAX_MOVE_DELTA)
        val dy = (smoothY * scale).coerceIn(-MAX_MOVE_DELTA, MAX_MOVE_DELTA)
        return dx to dy
    }

    /** Clears smoothed velocity and timestamp state when the pad is released. */
    fun reset() {
        smoothX = 0f
        smoothY = 0f
        lastTimestampNs = 0L
    }

    companion object {
        const val DEFAULT_DEAD_ZONE_RAD_PER_SEC = 0.04f
        const val DEFAULT_SMOOTHING_ALPHA = 0.35f
        const val DEFAULT_PIXELS_PER_RAD = 1800f
        const val DEFAULT_DT_SEC = 0.02f
        const val MIN_DT_SEC = 0.001f
        const val MAX_DT_SEC = 0.1f
        const val MAX_MOVE_DELTA = 4000f
        private const val SNAP_ZERO_EPSILON = 0.005f

        /**
         * Applies a continuous radial dead zone of [deadZone] rad/s so hand
         * tremor produces zero output while motion above the threshold ramps
         * smoothly from zero.
         */
        fun applyDeadZone(
            rawX: Float,
            rawY: Float,
            deadZone: Float = DEFAULT_DEAD_ZONE_RAD_PER_SEC,
        ): Pair<Float, Float> {
            val mag = hypot(rawX, rawY)
            if (mag <= deadZone || mag == 0f) return 0f to 0f
            val factor = (mag - deadZone) / mag
            return (rawX * factor) to (rawY * factor)
        }
    }
}

/**
 * Formats consecutive dictated utterances for typing into a PC field:
 * strips control characters, trims surrounding whitespace, and prepends a
 * single space between consecutive non-empty utterances.
 */
class UtteranceJoiner {
    var hasTypedUtterance: Boolean = false
        private set

    fun next(raw: String): String {
        val cleaned = sanitizeUtterance(raw)
        if (cleaned.isEmpty()) return ""
        val out = if (hasTypedUtterance) " $cleaned" else cleaned
        hasTypedUtterance = true
        return out
    }

    fun reset() {
        hasTypedUtterance = false
    }

    companion object {
        const val MAX_REMOTE_TEXT_BYTES = 256

        /**
         * Replaces control characters (newlines, tabs, etc.) with spaces and
         * collapses whitespace runs so the result is valid for `remote.input`
         * (`type = "text"`).
         */
        fun sanitizeUtterance(raw: String): String {
            if (raw.isEmpty()) return ""
            val sb = StringBuilder(raw.length)
            var lastWasSpace = true
            for (ch in raw) {
                if (ch.isISOControl() || ch == '\u0000' || ch.isWhitespace()) {
                    if (!lastWasSpace) {
                        sb.append(' ')
                        lastWasSpace = true
                    }
                } else {
                    sb.append(ch)
                    lastWasSpace = false
                }
            }
            if (sb.isNotEmpty() && sb[sb.length - 1] == ' ') {
                sb.setLength(sb.length - 1)
            }
            return sb.toString()
        }

        /**
         * Splits [text] on Unicode code-point boundaries into chunks of at most
         * [maxBytes] UTF-8 bytes (`docs/protocol/remote.md` §3: 1–256 bytes),
         * dropping control characters.
         */
        fun chunkUtf8(text: String, maxBytes: Int = MAX_REMOTE_TEXT_BYTES): List<String> {
            require(maxBytes >= 4) { "maxBytes must fit at least one UTF-8 code point" }
            val filtered = text.filter { !it.isISOControl() && it != '\u0000' }
            if (filtered.isEmpty()) return emptyList()
            val chunks = mutableListOf<String>()
            var start = 0
            var currentBytes = 0
            var i = 0
            while (i < filtered.length) {
                val codePoint = Character.codePointAt(filtered, i)
                val charCount = Character.charCount(codePoint)
                val utf8Bytes = when {
                    codePoint <= 0x7F -> 1
                    codePoint <= 0x7FF -> 2
                    codePoint <= 0xFFFF -> 3
                    else -> 4
                }
                if (currentBytes + utf8Bytes > maxBytes && i > start) {
                    chunks.add(filtered.substring(start, i))
                    start = i
                    currentBytes = 0
                }
                currentBytes += utf8Bytes
                i += charCount
            }
            if (start < filtered.length) {
                chunks.add(filtered.substring(start, filtered.length))
            }
            return chunks
        }
    }
}

/**
 * How far the pointer goes for how fast the finger moves, like a laptop's
 * touchpad: slow moves land on small targets, quick ones cross the screen.
 * Speed is smoothed so the pointer never lurches when a frame arrives late.
 */
internal class TouchpadBallistics {
    private var speed = -1f

    /** The multiplier for a move of [distance] reference pixels over [dtMs] milliseconds. */
    fun gain(distance: Float, dtMs: Long): Float {
        val instant = distance / dtMs.coerceAtLeast(MIN_FRAME_MS)
        speed = if (speed < 0f) instant else speed + SMOOTHING * (instant - speed)
        val t = ((speed - SLOW_PX_PER_MS) / (FAST_PX_PER_MS - SLOW_PX_PER_MS)).coerceIn(0f, 1f)
        return SLOW_GAIN + (FAST_GAIN - SLOW_GAIN) * (t * t * (3f - 2f * t))
    }

    private companion object {
        const val MIN_FRAME_MS = 4L
        const val SMOOTHING = 0.35f
        const val SLOW_PX_PER_MS = 0.4f
        const val FAST_PX_PER_MS = 6f
        const val SLOW_GAIN = 0.75f
        const val FAST_GAIN = 2.6f
    }
}

/** What lifting two fingers does: a right-click for a quick tap, nothing after a scroll. */
internal enum class TwoFingerRelease { RightClick, None }

/** Wheel clicks to scroll: positive [y] scrolls down, positive [x] scrolls right. */
internal data class ScrollClicks(val x: Float, val y: Float) {
    val isZero: Boolean get() = x == 0f && y == 0f

    companion object {
        val ZERO = ScrollClicks(0f, 0f)
    }
}

/**
 * Two fingers on the touchpad: a quick tap is a right-click, a drag scrolls.
 * Movement is in reference pixels; scrolling follows the fingers like a
 * touchscreen (fingers up scrolls down) and locks to the axis the drag mostly
 * takes, so a vertical scroll doesn't drift sideways.
 */
internal class TwoFingerGesture(private val tapSlopPx: Float) {
    private enum class Axis { Vertical, Horizontal, Both }

    var isScroll: Boolean = false
        private set
    var totalMoved: Float = 0f
        private set
    private var axis = Axis.Both
    private var heldX = 0f
    private var heldY = 0f
    private val recent = ArrayDeque<Sample>()

    private class Sample(val timeMs: Long, val clicks: ScrollClicks)

    /** Moves both fingers by ([dx], [dy]) at [timeMs]; returns what to scroll now. */
    fun onMove(dx: Float, dy: Float, timeMs: Long): ScrollClicks {
        if (!dx.isFinite() || !dy.isFinite()) return ScrollClicks.ZERO
        totalMoved += hypot(dx, dy)
        heldX += dx
        heldY += dy
        if (!isScroll) {
            if (totalMoved <= tapSlopPx) return ScrollClicks.ZERO
            isScroll = true
            axis = when {
                abs(heldY) >= abs(heldX) * AXIS_RATIO -> Axis.Vertical
                abs(heldX) >= abs(heldY) * AXIS_RATIO -> Axis.Horizontal
                else -> Axis.Both
            }
        }
        val x = if (axis == Axis.Vertical) 0f else -heldX / PX_PER_CLICK_X
        val y = if (axis == Axis.Horizontal) 0f else -heldY / PX_PER_CLICK_Y
        heldX = 0f
        heldY = 0f
        val clicks = ScrollClicks(x, y)
        if (!clicks.isZero) {
            recent.addLast(Sample(timeMs, clicks))
            while (recent.size > 1 && timeMs - recent.first().timeMs > VELOCITY_WINDOW_MS) recent.removeFirst()
        }
        return clicks
    }

    /** Fingers lifted [elapsedMs] after touching down. */
    fun onUp(elapsedMs: Long, tapTimeoutMs: Long): TwoFingerRelease =
        if (!isScroll && elapsedMs <= tapTimeoutMs && totalMoved <= tapSlopPx * 1.5f) {
            TwoFingerRelease.RightClick
        } else {
            TwoFingerRelease.None
        }

    /**
     * How fast the scroll was going when the fingers lifted at [upTimeMs], in
     * clicks per millisecond; zero if they had stopped before lifting.
     */
    fun velocity(upTimeMs: Long): ScrollClicks {
        if (!isScroll || recent.size < 2) return ScrollClicks.ZERO
        val last = recent.last()
        if (upTimeMs - last.timeMs > STOPPED_MS) return ScrollClicks.ZERO
        val first = recent.first()
        val spanMs = (last.timeMs - first.timeMs).coerceAtLeast(1L).toFloat()
        // The first sample is the distance covered before the window opened.
        var x = 0f
        var y = 0f
        for (i in 1 until recent.size) {
            x += recent[i].clicks.x
            y += recent[i].clicks.y
        }
        return ScrollClicks(
            (x / spanMs).coerceIn(-MAX_CLICKS_PER_MS, MAX_CLICKS_PER_MS),
            (y / spanMs).coerceIn(-MAX_CLICKS_PER_MS, MAX_CLICKS_PER_MS),
        )
    }

    companion object {
        const val PX_PER_CLICK_X = 48f
        const val PX_PER_CLICK_Y = 36f
        const val AXIS_RATIO = 1.2f
        const val VELOCITY_WINDOW_MS = 100L
        const val STOPPED_MS = 60L
        const val MAX_CLICKS_PER_MS = 0.5f
    }
}

/**
 * The glide after a two-finger flick: starts at a velocity in clicks per
 * millisecond and slows by [DECAY_PER_FRAME] each 16 ms frame.
 */
internal class ScrollFling(velocity: ScrollClicks) {
    private var vx = velocity.x
    private var vy = velocity.y

    /** Whether the flick was fast enough to glide at all. */
    val isMoving: Boolean get() = hypot(vx, vy) * FRAME_MS >= MIN_CLICKS_PER_FRAME

    /** The clicks to scroll over the next [dtMs], or null once the glide has stopped. */
    fun step(dtMs: Float): ScrollClicks? {
        if (!isMoving) return null
        val dt = dtMs.coerceIn(1f, MAX_STEP_MS)
        val out = ScrollClicks(vx * dt, vy * dt)
        val decay = DECAY_PER_FRAME.toDouble().pow((dt / FRAME_MS).toDouble()).toFloat()
        vx *= decay
        vy *= decay
        return out
    }

    companion object {
        /** A glide for a flick at [velocity], or null when the fingers were moving too slowly to throw. */
        fun start(velocity: ScrollClicks): ScrollFling? =
            if (hypot(velocity.x, velocity.y) >= MIN_START_CLICKS_PER_MS) ScrollFling(velocity) else null

        const val MIN_START_CLICKS_PER_MS = 0.01f
        const val FRAME_MS = 16f
        const val DECAY_PER_FRAME = 0.9f
        const val MIN_CLICKS_PER_FRAME = 0.02f
        const val MAX_STEP_MS = 50f
    }
}
