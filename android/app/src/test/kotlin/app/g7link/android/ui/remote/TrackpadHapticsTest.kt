// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import android.view.HapticFeedbackConstants
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class TrackpadHapticsTest {
    @Test
    fun androidFourteenGetsItsOwnTicksAndToggles() {
        val sdk = 34
        assertEquals(HapticFeedbackConstants.SEGMENT_FREQUENT_TICK, hapticConstant(TrackpadHaptic.ScrollDetent, sdk))
        assertEquals(HapticFeedbackConstants.DRAG_START, hapticConstant(TrackpadHaptic.DragStart, sdk))
        assertEquals(HapticFeedbackConstants.TOGGLE_ON, hapticConstant(TrackpadHaptic.ScrollLockOn, sdk))
        assertEquals(HapticFeedbackConstants.TOGGLE_OFF, hapticConstant(TrackpadHaptic.ScrollLockOff, sdk))
    }

    @Test
    fun androidElevenConfirmsTheScrollLock() {
        val sdk = 30
        assertEquals(HapticFeedbackConstants.CONFIRM, hapticConstant(TrackpadHaptic.ScrollLockOn, sdk))
        assertEquals(HapticFeedbackConstants.CONFIRM, hapticConstant(TrackpadHaptic.ScrollLockOff, sdk))
        assertEquals(HapticFeedbackConstants.CLOCK_TICK, hapticConstant(TrackpadHaptic.ScrollDetent, sdk))
        assertEquals(HapticFeedbackConstants.LONG_PRESS, hapticConstant(TrackpadHaptic.DragStart, sdk))
    }

    @Test
    fun theOldestSupportedAndroidFallsBackToKeysItHas() {
        val sdk = 26
        assertEquals(HapticFeedbackConstants.VIRTUAL_KEY, hapticConstant(TrackpadHaptic.ScrollLockOn, sdk))
        assertEquals(HapticFeedbackConstants.VIRTUAL_KEY, hapticConstant(TrackpadHaptic.ScrollLockOff, sdk))
        assertEquals(HapticFeedbackConstants.CLOCK_TICK, hapticConstant(TrackpadHaptic.ScrollDetent, sdk))
        assertEquals(HapticFeedbackConstants.LONG_PRESS, hapticConstant(TrackpadHaptic.DragStart, sdk))
    }

    @Test
    fun theSameOnEveryVersion() {
        for (sdk in listOf(26, 30, 34, 37)) {
            assertEquals(HapticFeedbackConstants.CLOCK_TICK, hapticConstant(TrackpadHaptic.Tap, sdk))
            assertEquals(HapticFeedbackConstants.CONTEXT_CLICK, hapticConstant(TrackpadHaptic.Click, sdk))
            assertEquals(HapticFeedbackConstants.KEYBOARD_TAP, hapticConstant(TrackpadHaptic.Key, sdk))
            assertEquals(HapticFeedbackConstants.LONG_PRESS, hapticConstant(TrackpadHaptic.LongPress, sdk))
        }
    }

    @Test
    fun theLimiterLetsOneThroughPerInterval() {
        val limiter = HapticRateLimiter(30)
        assertTrue(limiter.tryAcquire(1_000))
        assertFalse(limiter.tryAcquire(1_010))
        assertFalse(limiter.tryAcquire(1_029))
        assertTrue(limiter.tryAcquire(1_030))
        assertFalse(limiter.tryAcquire(1_031))
        assertTrue(limiter.tryAcquire(1_100))
    }

    @Test
    fun aClockThatGoesBackDoesNotSilenceTheLimiter() {
        val limiter = HapticRateLimiter(30)
        assertTrue(limiter.tryAcquire(5_000))
        assertTrue(limiter.tryAcquire(100))
        assertFalse(limiter.tryAcquire(110))
    }

    @Test
    fun aDetentTicksForEachWholeNotch() {
        val detents = ScrollDetents()
        var now = 0L
        var ticks = 0
        // Frames of a quarter notch each, far enough apart to never be limited.
        repeat(12) {
            now += 100
            if (detents.onScroll(0f, 0.25f, now)) ticks++
        }
        assertEquals(3, ticks)
    }

    @Test
    fun detentsCountEitherDirection() {
        val detents = ScrollDetents()
        assertFalse(detents.onScroll(0f, 0.6f, 0))
        assertTrue(detents.onScroll(0f, -0.6f, 100))
        assertFalse(detents.onScroll(0.3f, 0f, 200))
    }

    @Test
    fun aFastScrollTicksNoFasterThanTheLimit() {
        val detents = ScrollDetents()
        var ticks = 0
        // Two notches a millisecond for a tenth of a second.
        for (ms in 0L until 100L) {
            if (detents.onScroll(0f, 2f, ms)) ticks++
        }
        assertEquals(4, ticks)
    }

    @Test
    fun nothingOrNonsenseNeverTicks() {
        val detents = ScrollDetents()
        assertFalse(detents.onScroll(0f, 0f, 0))
        assertFalse(detents.onScroll(Float.NaN, 0f, 100))
        assertFalse(detents.onScroll(0f, Float.POSITIVE_INFINITY, 200))
        assertFalse(detents.onScroll(0f, 0.9f, 300))
        assertTrue(detents.onScroll(0f, 0.2f, 400))
    }

    @Test
    fun resetDropsAPartNotch() {
        val detents = ScrollDetents()
        assertFalse(detents.onScroll(0f, 0.9f, 0))
        detents.reset()
        assertFalse(detents.onScroll(0f, 0.2f, 100))
    }
}
