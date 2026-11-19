// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.core

import app.g7link.android.ble.LinkFailure
import app.g7link.core.BleUseState

// The touchpad's Bluetooth hold with a PC (docs/protocol/bluetooth.md §4.5):
// what the PC's answers and this phone's link tries mean for the connection
// sheet, and how often background renewals look for the PC's window.

/**
 * A PC's answer to `ble.use`. With `open`, a PC from 0.1.14 says how long its
 * connect window for this phone stays open ([windowSecs], 0 when there's none
 * now) and, with none, in how long a renewal can open one ([retrySecs]). An
 * older PC says neither.
 */
internal data class BleUseAnswer(
    val state: BleUseState,
    val windowSecs: Int? = null,
    val retrySecs: Int? = null,
) {
    /** Whether there may be a window to look for: not when the PC says there's none. */
    val windowOpen: Boolean
        get() = windowSecs != 0
}

/**
 * Where a try at linking leaves the bring-up: out of range only when the PC's
 * window was looked for and its tag never seen; seen but no link is a failure
 * to connect.
 */
internal fun bleSetupAfterLink(failure: LinkFailure?): BleSetup = when (failure) {
    null -> BleSetup.Linked
    LinkFailure.NotSeen -> BleSetup.OutOfRange
    else -> BleSetup.CouldntConnect
}

/**
 * How long background renewals wait before they look for the PC's window again,
 * after [failures] tries in a row came to nothing: 20 s, 40 s, 80 s, then every
 * five minutes. The hold itself is still renewed every 20 s meanwhile.
 */
internal fun bleLookAgainAfterMs(failures: Int): Long = when {
    failures <= 0 -> 0L
    failures == 1 -> 20_000L
    failures == 2 -> 40_000L
    failures == 3 -> 80_000L
    else -> 300_000L
}

/**
 * Background looks for one PC's window, backing off after tries that came to
 * nothing ([bleLookAgainAfterMs]). A link, or the user trying again, starts it
 * over. Times are `SystemClock.elapsedRealtime()`.
 */
internal class BleBackoff {
    private var failures = 0
    private var nextAt = 0L

    /** Whether a background renewal may look for the window at [now]. */
    @Synchronized
    fun due(now: Long): Boolean = failures == 0 || now >= nextAt

    /** A try ended without a link at [now]: how long until the next look. */
    @Synchronized
    fun failed(now: Long): Long {
        failures++
        val wait = bleLookAgainAfterMs(failures)
        nextAt = now + wait
        return wait
    }

    @Synchronized
    fun reset() {
        failures = 0
        nextAt = 0L
    }
}
