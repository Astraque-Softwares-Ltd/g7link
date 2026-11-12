// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import android.annotation.SuppressLint
import android.os.Build
import android.view.HapticFeedbackConstants
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.platform.LocalView
import kotlin.math.floor
import kotlin.math.hypot

/** What the remote's input feels like in the hand, one kind per thing sent to the PC. */
internal enum class TrackpadHaptic {
    /** A tap on the touchpad clicked. */
    Tap,

    /** Left, Middle or Right clicked. */
    Click,

    /** A scroll went past another wheel notch. */
    ScrollDetent,

    /** Double-tap and hold took hold of the button to drag. */
    DragStart,

    ScrollLockOn,
    ScrollLockOff,

    /** A special key or a quick action. */
    Key,

    /** Middle held down to middle-click. */
    LongPress,
}

/** The [HapticFeedbackConstants] for [haptic] on Android [sdk]: the closest one older versions have. */
// Each newer constant is chosen only when sdk says the phone has it.
@SuppressLint("InlinedApi")
internal fun hapticConstant(haptic: TrackpadHaptic, sdk: Int): Int = when (haptic) {
    TrackpadHaptic.Tap -> HapticFeedbackConstants.CLOCK_TICK
    TrackpadHaptic.Click -> HapticFeedbackConstants.CONTEXT_CLICK
    TrackpadHaptic.ScrollDetent -> if (sdk >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
        HapticFeedbackConstants.SEGMENT_FREQUENT_TICK
    } else {
        HapticFeedbackConstants.CLOCK_TICK
    }
    TrackpadHaptic.DragStart -> if (sdk >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
        HapticFeedbackConstants.DRAG_START
    } else {
        HapticFeedbackConstants.LONG_PRESS
    }
    TrackpadHaptic.ScrollLockOn -> when {
        sdk >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE -> HapticFeedbackConstants.TOGGLE_ON
        sdk >= Build.VERSION_CODES.R -> HapticFeedbackConstants.CONFIRM
        else -> HapticFeedbackConstants.VIRTUAL_KEY
    }
    TrackpadHaptic.ScrollLockOff -> when {
        sdk >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE -> HapticFeedbackConstants.TOGGLE_OFF
        sdk >= Build.VERSION_CODES.R -> HapticFeedbackConstants.CONFIRM
        else -> HapticFeedbackConstants.VIRTUAL_KEY
    }
    TrackpadHaptic.Key -> HapticFeedbackConstants.KEYBOARD_TAP
    TrackpadHaptic.LongPress -> HapticFeedbackConstants.LONG_PRESS
}

/** Lets one event through per [minIntervalMs]; the rest in between are dropped. */
internal class HapticRateLimiter(private val minIntervalMs: Long) {
    private var last: Long? = null

    /** Whether an event at [nowMs] may play. */
    fun tryAcquire(nowMs: Long): Boolean {
        val previous = last
        if (previous != null && nowMs >= previous && nowMs - previous < minIntervalMs) return false
        last = nowMs
        return true
    }
}

/**
 * A wheel's detents under a smooth scroll: scrolls arrive a frame at a time in
 * fractions of a notch, and a tick plays each time another whole notch has
 * gone by, no closer together than [SCROLL_TICK_MIN_MS].
 */
internal class ScrollDetents(private val limiter: HapticRateLimiter = HapticRateLimiter(SCROLL_TICK_MIN_MS)) {
    private var travel = 0f

    /** [x], [y] notches were scrolled at [nowMs]; true when a tick should play. */
    fun onScroll(x: Float, y: Float, nowMs: Long): Boolean {
        val step = hypot(x, y)
        if (!step.isFinite() || step <= 0f) return false
        travel += step
        if (travel < 1f) return false
        travel -= floor(travel)
        return limiter.tryAcquire(nowMs)
    }

    /** A new touch starts counting from nothing. */
    fun reset() {
        travel = 0f
    }

    companion object {
        const val SCROLL_TICK_MIN_MS = 30L
    }
}

/**
 * Plays [TrackpadHaptic]s through this window's view, which follows the phone's
 * touch-vibration setting. Nothing plays while [live] is false: input doesn't
 * reach a PC that's offline or hasn't allowed this phone.
 */
@Composable
internal fun rememberTrackpadHaptics(live: Boolean): (TrackpadHaptic) -> Unit {
    val view = LocalView.current
    val currentLive by rememberUpdatedState(live)
    return remember(view) {
        { haptic: TrackpadHaptic ->
            if (currentLive) view.performHapticFeedback(hapticConstant(haptic, Build.VERSION.SDK_INT))
        }
    }
}
