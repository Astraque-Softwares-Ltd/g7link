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
 * A laptop touchpad over this area: one finger moves the PC's pointer, a tap
 * clicks, tap then drag holds the button, two fingers scroll and a two-finger
 * tap right-clicks. A flicked scroll glides on in [flingScope] until the next
 * touch. [onTouch] follows the finger (null once it lifts), [onDragging] says
 * when the button is held, and [onTap] runs after a click. While
 * [scrollLocked] says so, one finger scrolls like two, in any direction.
 * [onHaptic] is told what to play for each click, drag and scroll notch sent.
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
) {
    val tapSlop = viewConfiguration.touchSlop
    // Movement in reference-density pixels, so the same physical swipe moves
    // the PC pointer and scrolls the same amount on every phone's screen.
    val toReference = TOUCHPAD_REFERENCE_DENSITY / density
    val tapTimeoutMs = 240L
    // Tap and drag, as on a laptop's touchpad: a tap presses the button and lets
    // go a moment later, unless the finger comes back down, which keeps it held:
    // moving drags from where it was tapped, another tap makes a double-click.
    // (Clicking at once, then pressing again to drag, is a double-click and drag:
    // it selects whole words and maximises a window by its title bar.)
    var tapHeld = false
    var keepHeld = false
    var release: Job? = null
    val releaseAfterTap: () -> Unit = {
        keepHeld = false
        release = flingScope.launch {
            try {
                delay(TAP_RELEASE_MS)
            } finally {
                if (tapHeld && !keepHeld) {
                    tapHeld = false
                    core.remoteButton(pcId, "left", "up", onStatus)
                }
            }
        }
    }
    val letGo: () -> Unit = {
        if (tapHeld) {
            tapHeld = false
            core.remoteButton(pcId, "left", "up", onStatus)
        }
    }
    var fling: Job? = null
    val detents = ScrollDetents()
    val scrolled: (ScrollClicks) -> Unit = { clicks ->
        if (detents.onScroll(clicks.x, clicks.y, SystemClock.uptimeMillis())) onHaptic(TrackpadHaptic.ScrollDetent)
    }

    awaitEachGesture {
        val firstDown = awaitFirstDown(requireUnconsumed = false)
        // A touch catches a gliding scroll, like on a touchscreen.
        fling?.cancel()
        fling = null
        detents.reset()
        val downTime = System.currentTimeMillis()
        var lastEventTime = firstDown.uptimeMillis
        var maxPointers = 1
        var totalMoved = 0f
        val twoFinger = TwoFingerGesture(tapSlop * toReference)
        val scrolling = scrollLocked()
        // Back down within the moment after a tap: the button is still held.
        if (tapHeld) {
            keepHeld = true
            release?.cancel()
            if (scrolling) letGo()
        }
        var isDragHold = tapHeld
        var dragStarted = false
        val ballistics = TouchpadBallistics()

        onTouch(firstDown.position)

        while (true) {
            val event = awaitPointerEvent()
            val changes = event.changes
            var pressedCount = 0
            var firstPressed: PointerInputChange? = null
            for (i in 0 until changes.size) {
                val c = changes[i]
                if (c.pressed) {
                    if (firstPressed == null) firstPressed = c
                    pressedCount++
                }
            }
            if (changes.isNotEmpty()) lastEventTime = changes[0].uptimeMillis
            if (pressedCount == 0 || firstPressed == null) break

            if (pressedCount > maxPointers) {
                maxPointers = pressedCount
                if (dragStarted) {
                    dragStarted = false
                    onDragging(false)
                }
                letGo()
                isDragHold = false
            }

            if (pressedCount == 1 && maxPointers == 1 && scrolling) {
                val change = firstPressed
                onTouch(change.position)
                val delta = change.positionChange()
                totalMoved += hypot(delta.x, delta.y)
                val clicks = twoFinger.onMove(delta.x * toReference, delta.y * toReference, change.uptimeMillis)
                if (!clicks.isZero) {
                    core.remoteScroll(pcId, clicks.x, clicks.y, fast = true)
                    scrolled(clicks)
                }
                change.consume()
            } else if (pressedCount == 1 && maxPointers == 1) {
                val change = firstPressed
                onTouch(change.position)
                val delta = change.positionChange()
                val dist = hypot(delta.x, delta.y)
                totalMoved += dist

                if (isDragHold && !dragStarted && totalMoved > tapSlop * 0.6f) {
                    // The button went down with the tap, where the finger was then.
                    dragStarted = true
                    onDragging(true)
                    onHaptic(TrackpadHaptic.DragStart)
                }

                if (dist > 0f) {
                    val dtMs = change.uptimeMillis - change.previousUptimeMillis
                    val gain = ballistics.gain(dist * toReference, dtMs)
                    core.remoteMove(
                        pcId,
                        delta.x * toReference * gain * sensitivity,
                        delta.y * toReference * gain * sensitivity,
                    )
                    change.consume()
                }
            } else if (pressedCount >= 2) {
                onTouch(firstPressed.position)
                var avgDx = 0f
                var avgDy = 0f
                for (i in 0 until changes.size) {
                    val c = changes[i]
                    if (c.pressed) {
                        val d = c.positionChange()
                        avgDx += d.x
                        avgDy += d.y
                        c.consume()
                    }
                }
                avgDx /= pressedCount
                avgDy /= pressedCount
                totalMoved += hypot(avgDx, avgDy)

                val clicks = twoFinger.onMove(
                    avgDx * toReference,
                    avgDy * toReference,
                    firstPressed.uptimeMillis,
                )
                if (!clicks.isZero) {
                    core.remoteScroll(pcId, clicks.x, clicks.y, fast = true)
                    scrolled(clicks)
                }
            }
        }

        onTouch(null)
        val elapsed = System.currentTimeMillis() - downTime
        val tapped = elapsed <= tapTimeoutMs && totalMoved <= tapSlop * 1.5f
        if (dragStarted) {
            letGo()
            onDragging(false)
        } else if (isDragHold && maxPointers == 1) {
            // Back down without dragging: a second tap is a double-click; a
            // press held still just lets go.
            letGo()
            if (tapped) {
                core.remoteButton(pcId, "left", "click", onStatus)
                onHaptic(TrackpadHaptic.Tap)
                onTap()
            }
        } else if (scrolling && maxPointers == 1 && twoFinger.isScroll) {
            fling = ScrollFling.start(twoFinger.velocity(lastEventTime))
                ?.let { flingScope.launchScrollFling(core, pcId, it, onStep = scrolled) }
        } else if (maxPointers >= 2) {
            if (twoFinger.isScroll) {
                fling = ScrollFling.start(twoFinger.velocity(lastEventTime))
                    ?.let { flingScope.launchScrollFling(core, pcId, it, onStep = scrolled) }
            } else if (maxPointers == 2 && totalMoved <= tapSlop * 1.5f &&
                twoFinger.onUp(elapsed, tapTimeoutMs) == TwoFingerRelease.RightClick
            ) {
                core.remoteButton(pcId, "right", "click", onStatus)
                onHaptic(TrackpadHaptic.Tap)
            }
        } else if (tapped && !scrolling) {
            core.remoteButton(pcId, "left", "down", onStatus)
            tapHeld = true
            releaseAfterTap()
            onHaptic(TrackpadHaptic.Tap)
            onTap()
        } else if (tapped) {
            core.remoteButton(pcId, "left", "click", onStatus)
            onHaptic(TrackpadHaptic.Tap)
            onTap()
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
