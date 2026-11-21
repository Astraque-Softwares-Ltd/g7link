// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import android.os.SystemClock
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.pointer.PointerInputChange
import androidx.compose.ui.input.pointer.PointerInputScope
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.runtime.withFrameNanos
import app.g7link.android.core.Core
import kotlin.math.hypot
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** Movement in reference-density pixels, so a swipe moves the pointer the same on every phone. */
internal const val TOUCHPAD_REFERENCE_DENSITY = 2.625f

/**
 * Where a pad's gestures point: the touchpad moves the PC's pointer by how far
 * the finger goes ([RelativePadPointer]); the Screen tab puts it where the
 * finger is on the picture.
 */
internal interface PadPointer {
    /** A gesture's first finger came down at ([x], [y]), in the pad's pixels; [held] while a tap's button is down. */
    fun start(x: Float, y: Float, held: Boolean)

    /**
     * One finger moved by ([dx], [dy]) pixels in [dtMs] to ([x], [y]). [heldStill]:
     * a tap's button is down and the finger hasn't gone far enough to drag yet.
     */
    fun move(x: Float, y: Float, dx: Float, dy: Float, dtMs: Long, heldStill: Boolean)

    /** Presses, releases or clicks ([action] `down`, `up`, `click`) [button] (`left`, `right`). */
    fun button(button: String, action: String)
}

/** The touchpad's pointer: moves by how far and how fast the finger goes, as on a laptop. */
internal class RelativePadPointer(
    private val core: Core,
    private val pcId: String,
    private val sensitivity: Float,
    private val toReference: Float,
    private val onStatus: (Core.RemoteAccess) -> Unit,
) : PadPointer {
    private var ballistics = TouchpadBallistics()

    override fun start(x: Float, y: Float, held: Boolean) {
        ballistics = TouchpadBallistics()
    }

    override fun move(x: Float, y: Float, dx: Float, dy: Float, dtMs: Long, heldStill: Boolean) {
        val gain = ballistics.gain(hypot(dx, dy) * toReference, dtMs)
        core.remoteMove(pcId, dx * toReference * gain * sensitivity, dy * toReference * gain * sensitivity)
    }

    override fun button(button: String, action: String) = core.remoteButton(pcId, button, action, onStatus)
}

/** What a pad's gestures do besides pointing. */
internal interface PadOutput {
    /** The first finger is at ([x], [y]). */
    fun touch(x: Float, y: Float)

    /** Every finger lifted. */
    fun lift()
    fun scroll(clicks: ScrollClicks)

    /** The fingers lifted while scrolling at [velocity]: the scroll glides on. */
    fun fling(velocity: ScrollClicks)
    fun dragging(on: Boolean)
    fun haptic(kind: TrackpadHaptic)

    /** A click went to the PC from a tap. */
    fun tapped()

    /** Runs [due] a moment after a tap ([TAP_RELEASE_MS]), or when that wait is cut short. */
    fun releaseLater(due: () -> Unit)
    fun cancelRelease()
}

/**
 * A laptop touchpad's gestures, as the touchpad and the Screen tab share them:
 * a tap clicks, tap then drag holds the button, two fingers scroll and a
 * two-finger tap right-clicks. Nothing is pressed until the gesture is known,
 * so a second finger never leaves a click behind. While the gesture is
 * scroll-locked, one finger scrolls like two, in any direction.
 *
 * [tapSlop] is in the pad's pixels; [toReference] turns those into
 * reference-density pixels, so scrolling goes the same on every phone.
 */
internal class PadGestures(
    private val tapSlop: Float,
    private val toReference: Float,
    private val pointer: PadPointer,
    private val out: PadOutput,
) {
    /** Which of an event's changes [event] used up. */
    enum class Consume { None, First, All }

    // Tap and drag, as on a laptop's touchpad: a tap presses the button and lets
    // go a moment later, unless the finger comes back down, which keeps it held:
    // moving drags from where it was tapped, another tap makes a double-click.
    // (Clicking at once, then pressing again to drag, is a double-click and drag:
    // it selects whole words and maximises a window by its title bar.)
    private var tapHeld = false
    private var keepHeld = false
    private var downTime = 0L
    private var lastEventTime = 0L
    private var maxPointers = 1
    private var totalMoved = 0f
    private var twoFinger = TwoFingerGesture(tapSlop * toReference)
    private var scrolling = false
    private var isDragHold = false
    private var dragStarted = false

    /** The first finger came down at [timeMs] at ([x], [y]). */
    fun down(timeMs: Long, x: Float, y: Float, scrollLocked: Boolean) {
        downTime = timeMs
        lastEventTime = timeMs
        maxPointers = 1
        totalMoved = 0f
        twoFinger = TwoFingerGesture(tapSlop * toReference)
        scrolling = scrollLocked
        // Back down within the moment after a tap: the button is still held.
        if (tapHeld) {
            keepHeld = true
            out.cancelRelease()
            if (scrolling) letGo()
        }
        isDragHold = tapHeld
        dragStarted = false
        pointer.start(x, y, held = tapHeld)
        out.touch(x, y)
    }

    /**
     * [pressed] fingers are down at [timeMs]: the first of them moved by
     * ([dx], [dy]) in [dtMs] to ([x], [y]), and they moved by ([avgDx],
     * [avgDy]) on average.
     */
    fun event(
        timeMs: Long,
        pressed: Int,
        x: Float,
        y: Float,
        dx: Float,
        dy: Float,
        dtMs: Long,
        avgDx: Float,
        avgDy: Float,
    ): Consume {
        lastEventTime = timeMs
        if (pressed > maxPointers) {
            maxPointers = pressed
            if (dragStarted) {
                dragStarted = false
                out.dragging(false)
            }
            letGo()
            isDragHold = false
        }
        return when {
            pressed == 1 && maxPointers == 1 && scrolling -> {
                out.touch(x, y)
                totalMoved += hypot(dx, dy)
                scroll(twoFinger.onMove(dx * toReference, dy * toReference, timeMs))
                Consume.First
            }
            pressed == 1 && maxPointers == 1 -> {
                out.touch(x, y)
                val dist = hypot(dx, dy)
                totalMoved += dist
                if (isDragHold && !dragStarted && totalMoved > tapSlop * 0.6f) {
                    // The button went down with the tap, where the finger was then.
                    dragStarted = true
                    out.dragging(true)
                    out.haptic(TrackpadHaptic.DragStart)
                }
                if (dist > 0f) {
                    pointer.move(x, y, dx, dy, dtMs, heldStill = isDragHold && !dragStarted)
                    Consume.First
                } else {
                    Consume.None
                }
            }
            pressed >= 2 -> {
                out.touch(x, y)
                totalMoved += hypot(avgDx, avgDy)
                scroll(twoFinger.onMove(avgDx * toReference, avgDy * toReference, timeMs))
                Consume.All
            }
            else -> Consume.None
        }
    }

    /** The last finger lifted at [timeMs]. */
    fun up(timeMs: Long) {
        lastEventTime = timeMs
        out.lift()
        val elapsed = lastEventTime - downTime
        val tapped = elapsed <= TAP_TIMEOUT_MS && totalMoved <= tapSlop * 1.5f
        if (dragStarted) {
            letGo()
            out.dragging(false)
        } else if (isDragHold && maxPointers == 1) {
            // Back down without dragging: a second tap is a double-click; a
            // press held still just lets go.
            letGo()
            if (tapped) click("left")
        } else if (scrolling && maxPointers == 1 && twoFinger.isScroll) {
            out.fling(twoFinger.velocity(lastEventTime))
        } else if (maxPointers >= 2) {
            if (twoFinger.isScroll) {
                out.fling(twoFinger.velocity(lastEventTime))
            } else if (maxPointers == 2 && totalMoved <= tapSlop * 1.5f &&
                twoFinger.onUp(elapsed, TAP_TIMEOUT_MS) == TwoFingerRelease.RightClick
            ) {
                pointer.button("right", "click")
                out.haptic(TrackpadHaptic.Tap)
            }
        } else if (tapped && !scrolling) {
            pointer.button("left", "down")
            tapHeld = true
            keepHeld = false
            out.releaseLater(::releaseDue)
            out.haptic(TrackpadHaptic.Tap)
            out.tapped()
        } else if (tapped) {
            click("left")
        }
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
        /** A touch this short, that moved this little, is a tap. */
        const val TAP_TIMEOUT_MS = 240L
    }
}

/**
 * A laptop touchpad over this area ([PadGestures]): one finger moves the PC's
 * pointer, a tap clicks, tap then drag holds the button, two fingers scroll
 * and a two-finger tap right-clicks. A flicked scroll glides on in
 * [flingScope] until the next touch. [onTouch] follows the finger (null once
 * it lifts), [onDragging] says when the button is held, and [onTap] runs after
 * a click. While [scrollLocked] says so, one finger scrolls like two, in any
 * direction. [onHaptic] is told what to play for each click, drag and scroll
 * notch sent.
 */
internal suspend fun PointerInputScope.touchpadGestures(
    pcId: String,
    core: Core,
    sensitivity: Float,
    flingScope: CoroutineScope,
    onStatus: (Core.RemoteAccess) -> Unit,
    onTouch: (Offset?) -> Unit = {},
    onDragging: (Boolean) -> Unit = {},
    onTap: () -> Unit = {},
    scrollLocked: () -> Boolean = { false },
    onHaptic: (TrackpadHaptic) -> Unit = {},
) = padGestures(
    pcId = pcId,
    core = core,
    pointer = RelativePadPointer(core, pcId, sensitivity, TOUCHPAD_REFERENCE_DENSITY / density, onStatus),
    flingScope = flingScope,
    onTouch = onTouch,
    onDragging = onDragging,
    onTap = onTap,
    scrollLocked = scrollLocked,
    onHaptic = onHaptic,
)

/**
 * The touchpad's gestures ([PadGestures]) over this area, pointing with
 * [pointer]; the rest as [touchpadGestures]. A gesture that starts where
 * [startsHere] says no (beside the picture) sends the PC nothing, and if it's
 * a tap, [onTap] runs.
 */
internal suspend fun PointerInputScope.padGestures(
    pcId: String,
    core: Core,
    pointer: PadPointer,
    flingScope: CoroutineScope,
    onTouch: (Offset?) -> Unit = {},
    onDragging: (Boolean) -> Unit = {},
    onTap: () -> Unit = {},
    scrollLocked: () -> Boolean = { false },
    onHaptic: (TrackpadHaptic) -> Unit = {},
    startsHere: (Offset) -> Boolean = { true },
) {
    val tapSlop = viewConfiguration.touchSlop
    // Movement in reference-density pixels, so the same physical swipe moves
    // the PC pointer and scrolls the same amount on every phone's screen.
    val toReference = TOUCHPAD_REFERENCE_DENSITY / density
    var release: Job? = null
    var gliding: Job? = null
    val detents = ScrollDetents()
    val scrolled: (ScrollClicks) -> Unit = { clicks ->
        if (detents.onScroll(clicks.x, clicks.y, SystemClock.uptimeMillis())) onHaptic(TrackpadHaptic.ScrollDetent)
    }
    val out = object : PadOutput {
        override fun touch(x: Float, y: Float) = onTouch(Offset(x, y))
        override fun lift() = onTouch(null)
        override fun scroll(clicks: ScrollClicks) {
            core.remoteScroll(pcId, clicks.x, clicks.y, fast = true)
            scrolled(clicks)
        }
        override fun fling(velocity: ScrollClicks) {
            gliding = ScrollFling.start(velocity)?.let { flingScope.launchScrollFling(core, pcId, it, onStep = scrolled) }
        }
        override fun dragging(on: Boolean) = onDragging(on)
        override fun haptic(kind: TrackpadHaptic) = onHaptic(kind)
        override fun tapped() = onTap()
        override fun releaseLater(due: () -> Unit) {
            release = flingScope.launch {
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
    }
    val gestures = PadGestures(tapSlop, toReference, pointer, out)

    awaitEachGesture {
        val firstDown = awaitFirstDown(requireUnconsumed = false)
        // A touch catches a gliding scroll, like on a touchscreen.
        gliding?.cancel()
        gliding = null
        detents.reset()

        if (!startsHere(firstDown.position)) {
            var moved = 0f
            var pointers = 1
            var lastTime = firstDown.uptimeMillis
            while (true) {
                val changes = awaitPointerEvent().changes
                val pressed = changes.count { it.pressed }
                if (pressed > pointers) pointers = pressed
                if (changes.isNotEmpty()) lastTime = changes[0].uptimeMillis
                if (pressed == 0) break
                changes.firstOrNull { it.pressed }?.positionChange()?.let { moved += hypot(it.x, it.y) }
            }
            val tapped = pointers == 1 &&
                lastTime - firstDown.uptimeMillis <= PadGestures.TAP_TIMEOUT_MS &&
                moved <= tapSlop * 1.5f
            if (tapped) onTap()
            return@awaitEachGesture
        }

        gestures.down(firstDown.uptimeMillis, firstDown.position.x, firstDown.position.y, scrollLocked())
        while (true) {
            val changes = awaitPointerEvent().changes
            var pressedCount = 0
            var firstPressed: PointerInputChange? = null
            var sumDx = 0f
            var sumDy = 0f
            for (i in 0 until changes.size) {
                val c = changes[i]
                if (c.pressed) {
                    if (firstPressed == null) firstPressed = c
                    pressedCount++
                    val d = c.positionChange()
                    sumDx += d.x
                    sumDy += d.y
                }
            }
            if (pressedCount == 0 || firstPressed == null) {
                gestures.up(if (changes.isNotEmpty()) changes[0].uptimeMillis else SystemClock.uptimeMillis())
                break
            }
            val delta = firstPressed.positionChange()
            val consume = gestures.event(
                timeMs = firstPressed.uptimeMillis,
                pressed = pressedCount,
                x = firstPressed.position.x,
                y = firstPressed.position.y,
                dx = delta.x,
                dy = delta.y,
                dtMs = firstPressed.uptimeMillis - firstPressed.previousUptimeMillis,
                avgDx = sumDx / pressedCount,
                avgDy = sumDy / pressedCount,
            )
            when (consume) {
                PadGestures.Consume.First -> firstPressed.consume()
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

/** How long a tap holds the button, waiting to see whether the finger comes back to drag. */
internal const val TAP_RELEASE_MS = 200L

/**
 * Glides a flicked scroll on after the fingers lift, one display frame at a
 * time; [onStep] hears each step sent.
 */
internal fun CoroutineScope.launchScrollFling(
    core: Core,
    pcId: String,
    fling: ScrollFling,
    onStep: (ScrollClicks) -> Unit = {},
): Job = launch {
    var last = withFrameNanos { it }
    while (true) {
        val now = withFrameNanos { it }
        val clicks = fling.step((now - last) / 1_000_000f) ?: break
        last = now
        if (!clicks.isZero) {
            core.remoteScroll(pcId, clicks.x, clicks.y, fast = true)
            onStep(clicks)
        }
    }
}
