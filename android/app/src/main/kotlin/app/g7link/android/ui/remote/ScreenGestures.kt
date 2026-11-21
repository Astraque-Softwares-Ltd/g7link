// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import android.os.SystemClock
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.ui.input.pointer.PointerInputChange
import androidx.compose.ui.input.pointer.PointerInputScope
import androidx.compose.ui.input.pointer.positionChange
import app.g7link.android.core.Core
import app.g7link.core.MirrorInputEvent
import app.g7link.core.TouchPhase
import kotlin.math.abs
import kotlin.math.hypot
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** What the Screen tab's gestures do besides pointing and what the touchpad does. */
internal interface ScreenOutput : PadOutput {
    /**
     * Two fingers scaled the picture by [factor] about ([x], [y]) and moved it by
     * ([dx], [dy]), in the picture box's pixels. Nothing goes to the PC.
     */
    fun zoom(factor: Float, x: Float, y: Float, dx: Float, dy: Float)

    /** A pinch ([pinched]) or a move of the zoomed picture ended. */
    fun zoomEnded(pinched: Boolean)

    /** Runs [due] once the finger has been held still for [ScreenGestures.HOLD_MS], unless [cancelHold] comes first. */
    fun holdLater(due: () -> Unit)
    fun cancelHold()

    /** The armed right click went to the PC. */
    fun rightClicked()
}

/**
 * The Screen tab's direct touch: a finger on the picture is where the PC's
 * pointer goes, as on a touchscreen, and nothing presses until the gesture is
 * known (`plans/screen-share-review.md` §2, the gesture table):
 *
 * - one finger down and moving: the pointer follows, no button;
 * - a tap clicks there as the finger lifts; tap, tap double-clicks;
 * - tap, then down again and move: the left button is held, to drag;
 * - touch and hold [HOLD_MS]: a right-click as it lifts, or the left button held
 *   to drag if the finger then moves;
 * - two fingers: a tap right-clicks at the first; moving together they scroll
 *   with a glide, or move the picture when it's zoomed; a pinch zooms it. A
 *   second finger never leaves a click behind.
 *
 * The timings, the scroll and the haptics are the touchpad's ([PadGestures]).
 * [tapSlop] is in the picture's pixels; [toReference] turns those into
 * reference-density pixels, so scrolling goes the same on every phone.
 * [rightArmed] turns the next tap into a right-click.
 */
internal class ScreenGestures(
    private val tapSlop: Float,
    private val toReference: Float,
    private val pointer: PadPointer,
    private val out: ScreenOutput,
) {
    /** What two fingers turned out to be doing. */
    private enum class Two { Undecided, Pinch, Together }

    var rightArmed = false

    // Tap and drag as on the touchpad (PadGestures): a tap presses the button and
    // lets go a moment later, unless the finger comes back down to drag or click again.
    private var tapHeld = false
    private var keepHeld = false
    private var isDragHold = false
    private var dragStarted = false

    private var fingerDown = false
    private var onPicture = true
    private var zoomed = false
    private var downTime = 0L
    private var maxPointers = 1
    private var totalMoved = 0f

    // Touch and hold.
    private var held = false
    private var holdDrag = false
    private var movedAtHold = 0f

    // Two fingers.
    private var two = Two.Undecided
    private var twoFinger = TwoFingerGesture(tapSlop * toReference)
    private var startSpan = 0f
    private var lastSpan = 0f
    private var pendingDx = 0f
    private var pendingDy = 0f

    /**
     * The first finger came down at [timeMs] at ([x], [y]). [zoomed]: the picture is
     * zoomed in. [onPicture]: it landed on the picture, not the space beside it,
     * where only zooming and moving the picture work.
     */
    fun down(timeMs: Long, x: Float, y: Float, zoomed: Boolean, onPicture: Boolean = true) {
        fingerDown = true
        downTime = timeMs
        maxPointers = 1
        totalMoved = 0f
        held = false
        holdDrag = false
        movedAtHold = 0f
        two = Two.Undecided
        twoFinger = TwoFingerGesture(tapSlop * toReference)
        startSpan = 0f
        lastSpan = 0f
        pendingDx = 0f
        pendingDy = 0f
        this.zoomed = zoomed
        this.onPicture = onPicture
        out.cancelHold()
        out.touch(x, y)
        if (!onPicture) return
        // Back down within the moment after a tap: the button is still held.
        if (tapHeld) {
            keepHeld = true
            out.cancelRelease()
        }
        isDragHold = tapHeld
        dragStarted = false
        pointer.start(x, y, held = tapHeld)
        if (!tapHeld) out.holdLater(::holdDue)
    }

    /**
     * [pressed] fingers are down at [timeMs]. The first of them moved by ([dx],
     * [dy]) in [dtMs] to ([x], [y]). Their middle is at ([centerX], [centerY]) and
     * moved by ([centerDx], [centerDy]); the first two are [span] apart.
     */
    fun event(
        timeMs: Long,
        pressed: Int,
        x: Float,
        y: Float,
        dx: Float,
        dy: Float,
        dtMs: Long,
        centerX: Float = x,
        centerY: Float = y,
        centerDx: Float = dx,
        centerDy: Float = dy,
        span: Float = 0f,
    ): PadGestures.Consume {
        if (pressed > maxPointers) {
            if (maxPointers == 1) {
                out.cancelHold()
                startSpan = span
                lastSpan = span
            }
            maxPointers = pressed
            // A second finger ends whatever one finger held.
            if (dragStarted || holdDrag) out.dragging(false)
            if (holdDrag) pointer.button("left", "up")
            dragStarted = false
            holdDrag = false
            held = false
            letGo()
            isDragHold = false
        }
        return when {
            pressed == 1 && maxPointers == 1 -> {
                out.touch(x, y)
                val dist = hypot(dx, dy)
                totalMoved += dist
                if (!onPicture) return PadGestures.Consume.None
                if (!held && totalMoved > tapSlop) out.cancelHold()
                if (held && !holdDrag && totalMoved - movedAtHold > tapSlop * DRAG_START) {
                    // Held, then moved: the left button goes down where it was held.
                    holdDrag = true
                    pointer.button("left", "down")
                    out.dragging(true)
                    out.haptic(TrackpadHaptic.DragStart)
                }
                if (isDragHold && !dragStarted && totalMoved > tapSlop * DRAG_START) {
                    // The button went down with the tap, where the finger was then.
                    dragStarted = true
                    out.dragging(true)
                    out.haptic(TrackpadHaptic.DragStart)
                }
                if (dist > 0f) {
                    val still = (isDragHold && !dragStarted) || (held && !holdDrag)
                    pointer.move(x, y, dx, dy, dtMs, heldStill = still)
                    PadGestures.Consume.First
                } else {
                    PadGestures.Consume.None
                }
            }
            pressed >= 2 -> {
                out.touch(x, y)
                twoFingers(timeMs, centerX, centerY, centerDx, centerDy, span)
                PadGestures.Consume.All
            }
            else -> PadGestures.Consume.None
        }
    }

    private fun twoFingers(timeMs: Long, cx: Float, cy: Float, cdx: Float, cdy: Float, span: Float) {
        when (two) {
            Two.Undecided -> {
                pendingDx += cdx
                pendingDy += cdy
                totalMoved += hypot(cdx, cdy)
                if (startSpan > 0f && abs(span - startSpan) > tapSlop * PINCH_SLOP) {
                    // The fingers spread or closed: a pinch, from where they started.
                    two = Two.Pinch
                    out.zoom(span / startSpan, cx, cy, pendingDx, pendingDy)
                    lastSpan = span
                    return
                }
                val clicks = twoFinger.onMove(cdx * toReference, cdy * toReference, timeMs)
                if (!twoFinger.isScroll) return
                two = Two.Together
                if (zoomed) {
                    out.zoom(1f, cx, cy, pendingDx, pendingDy)
                } else if (onPicture) {
                    scroll(clicks)
                }
            }
            Two.Pinch -> {
                val factor = if (lastSpan > 0f && span > 0f) span / lastSpan else 1f
                lastSpan = span
                out.zoom(factor, cx, cy, cdx, cdy)
            }
            Two.Together -> {
                if (zoomed) {
                    out.zoom(1f, cx, cy, cdx, cdy)
                } else {
                    val clicks = twoFinger.onMove(cdx * toReference, cdy * toReference, timeMs)
                    if (onPicture) scroll(clicks)
                }
            }
        }
    }

    /** The last finger lifted at [timeMs]. */
    fun up(timeMs: Long) {
        fingerDown = false
        out.lift()
        out.cancelHold()
        val elapsed = timeMs - downTime
        val tapped = elapsed <= PadGestures.TAP_TIMEOUT_MS && totalMoved <= tapSlop * 1.5f
        if (maxPointers >= 2 && (two == Two.Pinch || (two == Two.Together && zoomed))) {
            out.zoomEnded(pinched = two == Two.Pinch)
        }
        when {
            !onPicture -> if (maxPointers == 1 && tapped) out.tapped()
            holdDrag -> {
                pointer.button("left", "up")
                out.dragging(false)
            }
            held -> {
                // Touched and held, and let go where it was: a right-click there.
                pointer.button("right", "click")
                out.haptic(TrackpadHaptic.Click)
                if (rightArmed) {
                    rightArmed = false
                    out.rightClicked()
                }
            }
            dragStarted -> {
                letGo()
                out.dragging(false)
            }
            isDragHold && maxPointers == 1 -> {
                // Back down without dragging: a second tap is a double-click; a
                // press held still just lets go.
                letGo()
                if (tapped) click("left")
            }
            maxPointers >= 2 -> when (two) {
                Two.Together -> if (!zoomed && twoFinger.isScroll) out.fling(twoFinger.velocity(timeMs))
                Two.Pinch -> Unit
                Two.Undecided -> if (maxPointers == 2 && totalMoved <= tapSlop * 1.5f &&
                    twoFinger.onUp(elapsed, PadGestures.TAP_TIMEOUT_MS) == TwoFingerRelease.RightClick
                ) {
                    pointer.button("right", "click")
                    out.haptic(TrackpadHaptic.Tap)
                }
            }
            tapped && rightArmed -> {
                rightArmed = false
                pointer.button("right", "click")
                out.haptic(TrackpadHaptic.Click)
                out.rightClicked()
                out.tapped()
            }
            tapped -> {
                pointer.button("left", "down")
                tapHeld = true
                keepHeld = false
                out.releaseLater(::releaseDue)
                out.haptic(TrackpadHaptic.Tap)
                out.tapped()
            }
        }
    }

    /** The finger has been down, still, for [HOLD_MS]. */
    private fun holdDue() {
        if (!fingerDown || !onPicture || maxPointers != 1 || isDragHold || held || totalMoved > tapSlop) return
        held = true
        movedAtHold = totalMoved
        out.haptic(TrackpadHaptic.LongPress)
    }

    /** The moment after a tap is over: its button lets go unless the finger came back. */
    private fun releaseDue() {
        if (tapHeld && !keepHeld) {
            tapHeld = false
            pointer.button("left", "up")
        }
    }

    private fun click(button: String) {
        pointer.button(button, "click")
        out.haptic(TrackpadHaptic.Tap)
        out.tapped()
    }

    private fun scroll(clicks: ScrollClicks) {
        if (!clicks.isZero) out.scroll(clicks)
    }

    private fun letGo() {
        if (tapHeld) {
            tapHeld = false
            pointer.button("left", "up")
        }
    }

    companion object {
        /** How long a still finger is held before it's a right-click or a drag. */
        const val HOLD_MS = 400L

        /** How far, in tap slops, a held finger moves before it drags. */
        const val DRAG_START = 0.6f

        /** How far, in tap slops, two fingers spread or close before it's a pinch. */
        const val PINCH_SLOP = 2f
    }
}

/**
 * The Screen tab's direct touch: the PC's pointer goes where the finger is on
 * the picture ([toPc] gives that place, `0..1` across and down), and the left
 * button presses there. [onClick] hears each left press, which may move the
 * PC's caret.
 */
internal class DirectPadPointer(
    private val core: Core,
    private val pcId: String,
    private val onStatus: (Core.RemoteAccess) -> Unit,
    private val toPc: (x: Float, y: Float) -> Pair<Float, Float>,
    private val onClick: () -> Unit,
) : PadPointer {
    private var lastX = 0.5f
    private var lastY = 0.5f

    private fun send(phase: TouchPhase) {
        core.sendPcMirrorInput(pcId, MirrorInputEvent.Touch(phase, lastX, lastY))
    }

    override fun start(x: Float, y: Float, held: Boolean) {
        // Still held from a tap: the pointer stays, so a second tap double-clicks there.
        if (held) return
        val (nx, ny) = toPc(x, y)
        lastX = nx
        lastY = ny
        send(TouchPhase.MOVE)
    }

    override fun move(x: Float, y: Float, dx: Float, dy: Float, dtMs: Long, heldStill: Boolean) {
        if (heldStill) return
        val (nx, ny) = toPc(x, y)
        if (abs(nx - lastX) > DIRECT_MOVE_STEP || abs(ny - lastY) > DIRECT_MOVE_STEP) {
            lastX = nx
            lastY = ny
            send(TouchPhase.MOVE)
        }
    }

    override fun button(button: String, action: String) {
        if (button != "left") {
            core.remoteButton(pcId, button, action, onStatus)
            return
        }
        when (action) {
            "down" -> {
                send(TouchPhase.DOWN)
                onClick()
            }
            "up" -> send(TouchPhase.UP)
            else -> {
                send(TouchPhase.DOWN)
                send(TouchPhase.UP)
                onClick()
            }
        }
    }
}

/** The least move, as a share of the PC's screen, that moves its pointer. */
private const val DIRECT_MOVE_STEP = 0.002f

/** What the picture does with two fingers: zoom it, move it, and settle it after. */
internal interface ScreenZoom {
    /** Whether it's zoomed in now. */
    fun zoomed(): Boolean

    /** Whether ([x], [y]) is on the picture, not the space beside it. */
    fun onPicture(x: Float, y: Float): Boolean

    /** Scales it by [factor] about ([x], [y]) and moves it by ([dx], [dy]). */
    fun move(factor: Float, x: Float, y: Float, dx: Float, dy: Float)
    fun ended(pinched: Boolean)
}

/**
 * [ScreenGestures] over the picture, pointing with [pointer] and zooming with
 * [zoom]. A flicked scroll glides on in [scope]; [onDragging] says when a button
 * is held, [onTap] runs after a tap (on the picture or beside it), [rightArmed]
 * says whether the next tap right-clicks and [onRightClicked] that it did.
 * [onHaptic] plays each click, hold, drag and scroll notch, as on the touchpad.
 */
internal suspend fun PointerInputScope.screenGestures(
    pcId: String,
    core: Core,
    pointer: PadPointer,
    zoom: ScreenZoom,
    scope: CoroutineScope,
    onDragging: (Boolean) -> Unit = {},
    onTap: () -> Unit = {},
    rightArmed: () -> Boolean = { false },
    onRightClicked: () -> Unit = {},
    onHaptic: (TrackpadHaptic) -> Unit = {},
) {
    val tapSlop = viewConfiguration.touchSlop
    val toReference = TOUCHPAD_REFERENCE_DENSITY / density
    var release: Job? = null
    var hold: Job? = null
    var gliding: Job? = null
    val detents = ScrollDetents()
    val scrolled: (ScrollClicks) -> Unit = { clicks ->
        if (detents.onScroll(clicks.x, clicks.y, SystemClock.uptimeMillis())) onHaptic(TrackpadHaptic.ScrollDetent)
    }
    val out = object : ScreenOutput {
        override fun touch(x: Float, y: Float) = Unit
        override fun lift() = Unit
        override fun scroll(clicks: ScrollClicks) {
            core.remoteScroll(pcId, clicks.x, clicks.y, fast = true)
            scrolled(clicks)
        }
        override fun fling(velocity: ScrollClicks) {
            gliding = ScrollFling.start(velocity)?.let { scope.launchScrollFling(core, pcId, it, onStep = scrolled) }
        }
        override fun dragging(on: Boolean) = onDragging(on)
        override fun haptic(kind: TrackpadHaptic) = onHaptic(kind)
        override fun tapped() = onTap()
        override fun releaseLater(due: () -> Unit) {
            release = scope.launch {
                try {
                    delay(TAP_RELEASE_MS)
                } finally {
                    due()
                }
            }
        }
        override fun cancelRelease() {
            release?.cancel()
        }
        override fun zoom(factor: Float, x: Float, y: Float, dx: Float, dy: Float) = zoom.move(factor, x, y, dx, dy)
        override fun zoomEnded(pinched: Boolean) = zoom.ended(pinched)
        override fun holdLater(due: () -> Unit) {
            hold = scope.launch {
                delay(ScreenGestures.HOLD_MS)
                due()
            }
        }
        override fun cancelHold() {
            hold?.cancel()
            hold = null
        }
        override fun rightClicked() = onRightClicked()
    }
    val gestures = ScreenGestures(tapSlop, toReference, pointer, out)

    awaitEachGesture {
        val firstDown = awaitFirstDown(requireUnconsumed = false)
        // A touch catches a gliding scroll, like on a touchscreen.
        gliding?.cancel()
        gliding = null
        detents.reset()
        gestures.rightArmed = rightArmed()
        val at = firstDown.position
        gestures.down(firstDown.uptimeMillis, at.x, at.y, zoom.zoomed(), zoom.onPicture(at.x, at.y))
        while (true) {
            val changes = awaitPointerEvent().changes
            var pressedCount = 0
            var firstPressed: PointerInputChange? = null
            var secondPressed: PointerInputChange? = null
            var sumX = 0f
            var sumY = 0f
            var sumDx = 0f
            var sumDy = 0f
            for (i in 0 until changes.size) {
                val c = changes[i]
                if (c.pressed) {
                    if (firstPressed == null) firstPressed = c else if (secondPressed == null) secondPressed = c
                    pressedCount++
                    sumX += c.position.x
                    sumY += c.position.y
                    val d = c.positionChange()
                    sumDx += d.x
                    sumDy += d.y
                }
            }
            val first = firstPressed
            if (pressedCount == 0 || first == null) {
                gestures.up(if (changes.isNotEmpty()) changes[0].uptimeMillis else SystemClock.uptimeMillis())
                break
            }
            val delta = first.positionChange()
            val second = secondPressed
            val span = if (second != null) (second.position - first.position).getDistance() else 0f
            val consume = gestures.event(
                timeMs = first.uptimeMillis,
                pressed = pressedCount,
                x = first.position.x,
                y = first.position.y,
                dx = delta.x,
                dy = delta.y,
                dtMs = first.uptimeMillis - first.previousUptimeMillis,
                centerX = sumX / pressedCount,
                centerY = sumY / pressedCount,
                centerDx = sumDx / pressedCount,
                centerDy = sumDy / pressedCount,
                span = span,
            )
            when (consume) {
                PadGestures.Consume.First -> first.consume()
                PadGestures.Consume.All -> {
                    for (i in 0 until changes.size) {
                        if (changes[i].pressed) changes[i].consume()
                    }
                }
                PadGestures.Consume.None -> Unit
            }
        }
    }
}
