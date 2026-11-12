// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

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
) {
    val tapSlop = viewConfiguration.touchSlop
    // Movement in reference-density pixels, so the same physical swipe moves
    // the PC pointer and scrolls the same amount on every phone's screen.
    val toReference = TOUCHPAD_REFERENCE_DENSITY / density
    val tapTimeoutMs = 240L
    val doubleTapWindowMs = 260L
    var lastSingleTapTime = 0L
    var fling: Job? = null

    awaitEachGesture {
        val firstDown = awaitFirstDown(requireUnconsumed = false)
        // A touch catches a gliding scroll, like on a touchscreen.
        fling?.cancel()
        fling = null
        val downTime = System.currentTimeMillis()
        var lastEventTime = firstDown.uptimeMillis
        var maxPointers = 1
        var totalMoved = 0f
        val twoFinger = TwoFingerGesture(tapSlop * toReference)
        val scrolling = scrollLocked()
        var isDragHold = !scrolling && (downTime - lastSingleTapTime) <= doubleTapWindowMs
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
                    core.remoteButton(pcId, "left", "up", onStatus)
                    dragStarted = false
                    onDragging(false)
                }
                isDragHold = false
            }

            if (pressedCount == 1 && maxPointers == 1 && scrolling) {
                val change = firstPressed
                onTouch(change.position)
                val delta = change.positionChange()
                totalMoved += hypot(delta.x, delta.y)
                val clicks = twoFinger.onMove(delta.x * toReference, delta.y * toReference, change.uptimeMillis)
                if (!clicks.isZero) core.remoteScroll(pcId, clicks.x, clicks.y, fast = true)
                change.consume()
            } else if (pressedCount == 1 && maxPointers == 1) {
                val change = firstPressed
                onTouch(change.position)
                val delta = change.positionChange()
                val dist = hypot(delta.x, delta.y)
                totalMoved += dist

                if (isDragHold && !dragStarted && totalMoved > tapSlop * 0.6f) {
                    dragStarted = true
                    onDragging(true)
                    core.remoteButton(pcId, "left", "down", onStatus)
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
                if (!clicks.isZero) core.remoteScroll(pcId, clicks.x, clicks.y, fast = true)
            }
        }

        onTouch(null)
        val elapsed = System.currentTimeMillis() - downTime
        if (dragStarted) {
            core.remoteButton(pcId, "left", "up", onStatus)
            onDragging(false)
            lastSingleTapTime = 0L
        } else if (scrolling && maxPointers == 1 && twoFinger.isScroll) {
            lastSingleTapTime = 0L
            fling = ScrollFling.start(twoFinger.velocity(lastEventTime))
                ?.let { flingScope.launchScrollFling(core, pcId, it) }
        } else if (maxPointers >= 2) {
            lastSingleTapTime = 0L
            if (twoFinger.isScroll) {
                fling = ScrollFling.start(twoFinger.velocity(lastEventTime))
                    ?.let { flingScope.launchScrollFling(core, pcId, it) }
            } else if (maxPointers == 2 && totalMoved <= tapSlop * 1.5f &&
                twoFinger.onUp(elapsed, tapTimeoutMs) == TwoFingerRelease.RightClick
            ) {
                core.remoteButton(pcId, "right", "click", onStatus)
            }
        } else if (elapsed <= tapTimeoutMs && totalMoved <= tapSlop * 1.5f) {
            lastSingleTapTime = System.currentTimeMillis()
            core.remoteButton(pcId, "left", "click", onStatus)
            onTap()
        } else {
            lastSingleTapTime = 0L
        }
    }
}

/** Glides a flicked scroll on after the fingers lift, one display frame at a time. */
internal fun CoroutineScope.launchScrollFling(core: Core, pcId: String, fling: ScrollFling): Job = launch {
    var last = withFrameNanos { it }
    while (true) {
        val now = withFrameNanos { it }
        val clicks = fling.step((now - last) / 1_000_000f) ?: break
        last = now
        if (!clicks.isZero) core.remoteScroll(pcId, clicks.x, clicks.y, fast = true)
    }
}
