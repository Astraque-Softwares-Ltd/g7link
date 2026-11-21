// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The Screen tab's direct touch: the gesture table in plans/screen-share-review.md §2. */
class ScreenGesturesTest {
    private val slop = 20f

    /** What the gestures sent: buttons as "left down", moves as "move x,y" (held still or not). */
    private class Recorder : PadPointer, ScreenOutput {
        val sent = mutableListOf<String>()
        val buttons get() = sent.filter { it.startsWith("left") || it.startsWith("right") || it.startsWith("dragging") }
        var release: (() -> Unit)? = null
        var hold: (() -> Unit)? = null
        var scrolls = 0
        var flings = 0
        var taps = 0
        var rightClicked = 0
        val zooms = mutableListOf<Float>()
        val pans = mutableListOf<Pair<Float, Float>>()
        var zoomEnded: Boolean? = null
        val haptics = mutableListOf<TrackpadHaptic>()

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
        override fun haptic(kind: TrackpadHaptic) {
            haptics += kind
        }
        override fun tapped() {
            taps++
        }
        override fun releaseLater(due: () -> Unit) {
            release = due
        }
        override fun cancelRelease() {
            // As the coroutine's finally runs once it's cancelled.
            release?.invoke()
            release = null
        }
        override fun zoom(factor: Float, x: Float, y: Float, dx: Float, dy: Float) {
            if (factor != 1f) zooms += factor
            if (dx != 0f || dy != 0f) pans += dx to dy
        }
        override fun zoomEnded(pinched: Boolean) {
            zoomEnded = pinched
        }
        override fun holdLater(due: () -> Unit) {
            hold = due
        }
        override fun cancelHold() {
            hold = null
        }
        override fun rightClicked() {
            rightClicked++
        }

        /** The finger has been held still for [ScreenGestures.HOLD_MS]. */
        fun holdPasses() {
            val due = hold
            hold = null
            due?.invoke()
        }
    }

    private fun screen(): Pair<ScreenGestures, Recorder> {
        val recorder = Recorder()
        return ScreenGestures(tapSlop = slop, toReference = 1f, pointer = recorder, out = recorder) to recorder
    }

    private fun ScreenGestures.one(t: Long, x: Float, y: Float, dx: Float = 0f, dy: Float = 0f) =
        event(t, 1, x, y, dx, dy, 8)

    /** Two fingers [span] apart, their middle at ([x], [y]) and moved by ([dx], [dy]). */
    private fun ScreenGestures.two(t: Long, x: Float, y: Float, dx: Float = 0f, dy: Float = 0f, span: Float = 100f) =
        event(t, 2, x, y, dx, dy, 8, centerX = x, centerY = y, centerDx = dx, centerDy = dy, span = span)

    @Test
    fun aFingerDownMovesThePointerAndPressesNothing() {
        val (gestures, sent) = screen()
        gestures.down(0, 50f, 60f, zoomed = false)
        gestures.one(10, 90f, 60f, dx = 40f)
        gestures.one(20, 140f, 60f, dx = 50f)
        assertEquals(listOf("start 50,60", "move 90,60", "move 140,60"), sent.sent)
        gestures.up(500)
        assertEquals(emptyList<String>(), sent.buttons)
    }

    @Test
    fun aTapClicksThereAsTheFingerLifts() {
        val (gestures, sent) = screen()
        gestures.down(0, 50f, 60f, zoomed = false)
        assertEquals(emptyList<String>(), sent.buttons)
        gestures.up(90)
        assertEquals(listOf("left down"), sent.buttons)
        sent.release?.invoke()
        assertEquals(listOf("left down", "left up"), sent.buttons)
        assertEquals(1, sent.taps)
        assertNull("a tap doesn't wait to be a hold", sent.hold)
    }

    @Test
    fun tapTapIsADoubleClickWhereTheFirstTapWas() {
        val (gestures, sent) = screen()
        gestures.down(0, 50f, 60f, zoomed = false)
        gestures.up(80)
        gestures.down(160, 53f, 61f, zoomed = false)
        // The second touch lands a little off: the pointer stays for the second click.
        gestures.one(170, 55f, 61f, dx = 2f)
        gestures.up(220)
        assertEquals(listOf("left down", "left up", "left click"), sent.buttons)
        assertTrue(sent.sent.contains("start 53,61 held"))
        assertTrue(sent.sent.contains("move 55,61 still"))
    }

    @Test
    fun touchAndHoldIsARightClickWhereItWasHeld() {
        val (gestures, sent) = screen()
        gestures.down(0, 50f, 60f, zoomed = false)
        assertNotNull("a still finger waits to be a hold", sent.hold)
        sent.holdPasses()
        assertTrue(TrackpadHaptic.LongPress in sent.haptics)
        gestures.up(700)
        assertEquals(listOf("right click"), sent.buttons)
    }

    @Test
    fun touchHoldThenMoveDragsWithTheLeftButton() {
        val (gestures, sent) = screen()
        gestures.down(0, 50f, 60f, zoomed = false)
        sent.holdPasses()
        // A tremble doesn't drag yet, and the pointer stays where it was held.
        gestures.one(450, 52f, 60f, dx = 2f)
        assertEquals(emptyList<String>(), sent.buttons)
        gestures.one(460, 80f, 60f, dx = 28f)
        gestures.one(470, 120f, 60f, dx = 40f)
        gestures.up(700)
        assertEquals(listOf("left down", "dragging true", "left up", "dragging false"), sent.buttons)
        // Pressed where it was held, before the pointer moved on.
        assertTrue(sent.sent.indexOf("left down") < sent.sent.indexOf("move 80,60"))
        assertTrue(sent.sent.contains("move 52,60 still"))
        assertTrue(TrackpadHaptic.DragStart in sent.haptics)
    }

    @Test
    fun aMovingFingerIsNeverAHold() {
        val (gestures, sent) = screen()
        gestures.down(0, 50f, 60f, zoomed = false)
        gestures.one(100, 90f, 60f, dx = 40f)
        assertNull("moving cancels the hold", sent.hold)
        gestures.up(900)
        assertEquals(emptyList<String>(), sent.buttons)
    }

    @Test
    fun tapThenDragHoldsTheLeftButtonUntilLift() {
        val (gestures, sent) = screen()
        gestures.down(0, 50f, 60f, zoomed = false)
        gestures.up(80)
        gestures.down(160, 50f, 60f, zoomed = false)
        assertNull("the second touch of tap-then-drag is no hold", sent.hold)
        gestures.one(170, 55f, 60f, dx = 5f)
        gestures.one(180, 80f, 60f, dx = 25f)
        gestures.up(400)
        assertEquals(listOf("left down", "dragging true", "left up", "dragging false"), sent.buttons)
        assertTrue(sent.sent.contains("move 55,60 still"))
        assertTrue(sent.sent.contains("move 80,60"))
    }

    @Test
    fun aTwoFingerTapRightClicksAndNothingElse() {
        val (gestures, sent) = screen()
        gestures.down(0, 100f, 100f, zoomed = false)
        gestures.two(16, 150f, 100f)
        gestures.two(60, 151f, 100f, dx = 1f)
        gestures.up(120)
        assertEquals(listOf("right click"), sent.buttons)
    }

    @Test
    fun aSecondFingerNeverLeavesAClickBehind() {
        // Scrolling at fit.
        val (scrolling, scrolled) = screen()
        scrolling.down(0, 100f, 300f, zoomed = false)
        scrolling.one(8, 100f, 299f, dy = -1f)
        scrolling.two(16, 100f, 299f)
        var y = 299f
        for (t in 24L..200L step 8) {
            y -= 12f
            assertEquals(PadGestures.Consume.All, scrolling.two(t, 100f, y, dy = -12f))
        }
        scrolling.up(216)
        assertTrue("it scrolled", scrolled.scrolls > 0)
        assertEquals(emptyList<String>(), scrolled.buttons)

        // A second finger on a touch being held: no right-click as they lift.
        val (holding, held) = screen()
        holding.down(0, 100f, 100f, zoomed = false)
        held.holdPasses()
        holding.two(450, 150f, 100f)
        holding.up(900)
        assertEquals(emptyList<String>(), held.buttons)

        // A second finger on a tap's held button only lets it go.
        val (tapping, tapped) = screen()
        tapping.down(0, 100f, 100f, zoomed = false)
        tapping.up(80)
        tapping.down(150, 100f, 100f, zoomed = false)
        tapping.two(170, 150f, 100f)
        tapping.up(400)
        assertEquals(listOf("left down", "left up"), tapped.buttons)

        // A second finger while dragging ends the drag, and clicks nothing.
        val (dragging, dragged) = screen()
        dragging.down(0, 50f, 60f, zoomed = false)
        dragged.holdPasses()
        dragging.one(460, 90f, 60f, dx = 40f)
        dragging.two(470, 120f, 60f)
        dragging.up(700)
        assertEquals(listOf("left down", "dragging true", "dragging false", "left up"), dragged.buttons)
    }

    @Test
    fun aPinchZoomsAndSendsNothing() {
        val (gestures, sent) = screen()
        gestures.down(0, 100f, 300f, zoomed = false)
        val before = sent.sent.size
        gestures.two(16, 150f, 300f, span = 100f)
        gestures.two(24, 150f, 300f, span = 120f)
        gestures.two(32, 150f, 300f, span = 160f)
        gestures.two(40, 150f, 300f, span = 200f)
        gestures.up(300)
        assertEquals("nothing went to the PC once the second finger landed", before, sent.sent.size)
        assertEquals(emptyList<String>(), sent.buttons)
        assertEquals(0, sent.scrolls)
        assertEquals(0, sent.flings)
        // Spread from 100 to 200 apart: twice the size, in steps.
        assertEquals(2f, sent.zooms.fold(1f) { acc, f -> acc * f }, 0.001f)
        assertEquals(true, sent.zoomEnded)
    }

    @Test
    fun twoFingersTogetherMoveTheZoomedPictureAndSendNothing() {
        val (gestures, sent) = screen()
        gestures.down(0, 100f, 300f, zoomed = true)
        gestures.two(16, 150f, 300f)
        var y = 300f
        for (t in 24L..120L step 8) {
            y -= 12f
            gestures.two(t, 150f, y, dy = -12f)
        }
        gestures.up(130)
        assertEquals(emptyList<String>(), sent.buttons)
        assertEquals(0, sent.scrolls)
        assertEquals(0, sent.flings)
        assertEquals(0, sent.zooms.size)
        // Every bit of the move, including what it took to tell it from a tap.
        assertEquals(-12f * 13, sent.pans.sumOf { it.second.toDouble() }.toFloat(), 0.01f)
        assertEquals(false, sent.zoomEnded)
    }

    @Test
    fun theArmedRightClickTakesTheNextTap() {
        val (gestures, sent) = screen()
        gestures.rightArmed = true
        gestures.down(0, 50f, 60f, zoomed = false)
        gestures.up(90)
        assertEquals(listOf("right click"), sent.buttons)
        assertEquals(1, sent.rightClicked)
        assertFalse(gestures.rightArmed)
        // The tap after is a left click again.
        gestures.down(600, 50f, 60f, zoomed = false)
        gestures.up(680)
        assertEquals(listOf("right click", "left down"), sent.buttons)
    }

    @Test
    fun besideThePictureATapGoesNowhere() {
        val (gestures, sent) = screen()
        gestures.down(0, 5f, 60f, zoomed = false, onPicture = false)
        assertNull(sent.hold)
        gestures.one(10, 6f, 60f, dx = 1f)
        gestures.up(80)
        assertEquals(emptyList<String>(), sent.sent)
        assertEquals("it still shows the controls", 1, sent.taps)
    }
}
