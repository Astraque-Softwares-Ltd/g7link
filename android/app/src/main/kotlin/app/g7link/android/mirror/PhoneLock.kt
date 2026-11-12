// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.mirror

import android.app.KeyguardManager
import android.content.Context

/**
 * No control from a PC while the phone is locked: its taps and keys are
 * dropped until the phone is unlocked, app windows don't open, and
 * mirroring through Elevated asks on the phone as it does without it.
 */
object PhoneLock {
    /** Whether the lock screen is up. */
    fun locked(context: Context): Boolean =
        runCatching { context.getSystemService(KeyguardManager::class.java)?.isKeyguardLocked == true }.getOrDefault(true)

    /** Whether a PC's request for the screen may start through Elevated, without a tap on the phone. */
    fun startsWithoutTap(elevatedRunning: Boolean, locked: Boolean, soundOnly: Boolean): Boolean =
        elevatedRunning && (soundOnly || !locked)

    /**
     * Which mirroring sessions were told their taps and keys are dropped, so
     * each is told once per lock.
     */
    class Gate {
        private val told = mutableSetOf<Pair<String, UInt>>()

        /** What to do with input from [pc] on [session]. */
        @Synchronized
        fun check(locked: Boolean, pc: String, session: UInt): Input {
            if (!locked) {
                told.clear()
                return Input.Pass
            }
            return if (told.add(pc to session)) Input.DropAndTell else Input.Drop
        }
    }

    enum class Input { Pass, Drop, DropAndTell }
}
