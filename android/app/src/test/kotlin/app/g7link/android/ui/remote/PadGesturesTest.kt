// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** The touchpad's gestures, as the Touchpad tab and the Screen tab's picture share them. */
class PadGesturesTest {
    private val slop = 20f

    /** What the gestures sent: buttons as "left down", moves as "move x,y" (held still or not). */
    private class Recorder : PadPointer, PadOutput {
        val sent = mutableListOf<String>()
        val buttons get() = sent.filter { !it.startsWith("move") && !it.startsWith("start") }
        var release: (() -> Unit)? = null
        var scrolls = 0
        var flings = 0
        var cancelled = 0

        override fun start(x: Float, y: Float, held: Boolean) {
            sent += "start ${x.toInt()},${y.toInt()}${if (held) " held" else ""}"
        }
        override fun move(x: Float, y: Float, dx: Float, dy: Float, dtMs: Long, heldStill: Boolean) {
            sent += "move ${x.toInt()},${y.toInt()}${if (heldStill) " still" else ""}"
        }
        override fun button(button: String, action: String) {
            sent += "$button $action"
        }
        override fun touch(x: Float, y: Float) = Unit
        override fun lift() = Unit
        override fun scroll(clicks: ScrollClicks) {
            scrolls++
        }
        override fun fling(velocity: ScrollClicks) {
            flings++
        }
        override fun dragging(on: Boolean) {
            sent += "dragging $on"
        }
        override fun haptic(kind: TrackpadHaptic) = Unit
        override fun tapped() = Unit
        override fun releaseLater(due: () -> Unit) {
            release = due
        }
        override fun cancelRelease() {
            cancelled++
            // As the coroutine's finally runs once it's cancelled.
            release?.invoke()
            release = null
        }
    }

    private fun pad(): Pair<PadGestures, Recorder> {
        val recorder = Recorder()
        return PadGestures(tapSlop = slop, toReference = 1f, pointer = recorder, out = recorder) to recorder
    }

    /** One finger at ([x], [y]), as a pointer event. */
    private fun PadGestures.one(t: Long, x: Float, y: Float, dx: Float = 0f, dy: Float = 0f) =
        event(t, 1, x, y, dx, dy, 8, dx, dy)

    private fun PadGestures.two(t: Long, x: Float, y: Float, dx: Float = 0f, dy: Float = 0f) =
        event(t, 2, x, y, dx, dy, 8, dx, dy)

    @Test
    fun aTwoFingerTapRightClicksAndNothingElse() {
        val (gestures, sent) = pad()
        gestures.down(0, 100f, 100f, scrollLocked = false)
        gestures.two(16, 100f, 100f)
        gestures.two(60, 101f, 100f, dx = 1f)
        gestures.up(120)
        assertEquals(listOf("right click"), sent.buttons)
    }

    @Test
    fun aTwoFingerScrollNeverClicks() {
        val (gestures, sent) = pad()
        gestures.down(0, 100f, 300f, scrollLocked = false)
        gestures.one(8, 100f, 299f, dy = -1f)
        gestures.two(16, 100f, 299f)
        var y = 299f
        for (t in 24L..200L step 8) {
            y -= 12f
            assertEquals(PadGestures.Consume.All, gestures.two(t, 100f, y, dy = -12f))
        }
        gestures.one(208, 100f, y)
        gestures.up(216)
        assertTrue("it scrolled", sent.scrolls > 0)
        assertEquals("the second finger leaves no click behind", emptyList<String>(), sent.buttons)
    }

    @Test
    fun aSecondFingerAfterATapOnlyLetsTheTapGo() {
        val (gestures, sent) = pad()
        gestures.down(0, 100f, 100f, scrollLocked = false)
        gestures.up(80)
        assertEquals(listOf("left down"), sent.buttons)
        // Back down at once, then a second finger: the tap's click ends, nothing more.
        gestures.down(150, 100f, 100f, scrollLocked = false)
        gestures.two(170, 100f, 100f)
        gestures.up(400)
        assertEquals(listOf("left down", "left up"), sent.buttons)
    }

    @Test
    fun aTapClicksOnLiftAndLetsGoAMomentLater() {
        val (gestures, sent) = pad()
        gestures.down(0, 50f, 60f, scrollLocked = false)
        assertEquals(listOf("start 50,60"), sent.sent)
        gestures.up(90)
        assertEquals(listOf("left down"), sent.buttons)
        sent.release?.invoke()
        assertEquals(listOf("left down", "left up"), sent.buttons)
    }

    @Test
    fun aSlowTouchOrAMoveIsNoClick() {
        val (gestures, sent) = pad()
        gestures.down(0, 50f, 60f, scrollLocked = false)
        gestures.up(600)
        gestures.down(1_000, 50f, 60f, scrollLocked = false)
        gestures.one(1_010, 90f, 60f, dx = 40f)
        gestures.up(1_050)
        assertEquals(emptyList<String>(), sent.buttons)
        assertEquals("move 90,60", sent.sent.last { it.startsWith("move") })
    }

    @Test
    fun tapTwiceDoubleClicksInPlace() {
        val (gestures, sent) = pad()
        gestures.down(0, 50f, 60f, scrollLocked = false)
        gestures.up(80)
        gestures.down(160, 53f, 61f, scrollLocked = false)
        // The finger landed a little off: the pointer holds still for the second click.
        gestures.one(170, 55f, 61f, dx = 2f)
        gestures.up(220)
        assertEquals(1, sent.cancelled)
        assertEquals(listOf("left down", "left up", "left click"), sent.buttons)
        assertTrue(sent.sent.contains("start 53,61 held"))
        assertTrue(sent.sent.contains("move 55,61 still"))
    }

    @Test
    fun tapThenDragHoldsTheButtonUntilLift() {
        val (gestures, sent) = pad()
        gestures.down(0, 50f, 60f, scrollLocked = false)
        gestures.up(80)
        gestures.down(160, 50f, 60f, scrollLocked = false)
        gestures.one(170, 55f, 60f, dx = 5f)
        gestures.one(180, 80f, 60f, dx = 25f)
        gestures.up(400)
        assertEquals(listOf("left down", "dragging true", "left up", "dragging false"), sent.buttons)
        // Held still until it was a drag; then the pointer follows the finger.
        assertTrue(sent.sent.contains("move 55,60 still"))
        assertTrue(sent.sent.contains("move 80,60"))
    }
}
