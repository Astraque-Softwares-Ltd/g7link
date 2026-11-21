// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable

/**
 * What a PC says of itself (contract §V4.3, `agent.presence`): someone is at
 * it, it's locked, it's in Do Not Disturb, and which of this phone's threads
 * are on its screen. Never a window, an app or an idle time. [untilMs] is this
 * phone's clock when it stops counting; after that the PC is "not there".
 */
@Immutable
data class PcPresence(
    val atPc: Boolean,
    val locked: Boolean,
    val dnd: Boolean,
    val viewing: Set<String> = emptySet(),
    val sessions: Set<String> = emptySet(),
    val untilMs: Long,
) {
    fun fresh(nowMs: Long): Boolean = nowMs < untilMs

    /** [threadId] is on the PC's screen now ("open on <PC>"). */
    fun shows(threadId: String, nowMs: Long): Boolean = fresh(nowMs) && threadId in viewing

    /**
     * Whether a notice for [threadId] waits (contract §V4.4): fresh, someone
     * is at the PC, it isn't locked or in Do Not Disturb (a PC in DND shows no
     * banner, so the phone must), and the thread is on its screen. Anything
     * unknown notifies.
     */
    fun quiets(threadId: String, nowMs: Long): Boolean = fresh(nowMs) && atPc && !locked && !dnd && threadId in viewing

    companion object {
        /** The most threads and sessions a PC names (§V4.5). */
        const val MAX_VIEWING = 4
    }
}

/** The FFI's presence as the phone keeps it. */
object AgentPresenceFfi {
    fun of(presence: app.g7link.core.AgentPresence): PcPresence = PcPresence(
        atPc = presence.atPc && !presence.locked,
        locked = presence.locked,
        dnd = presence.dnd,
        viewing = presence.viewing.take(PcPresence.MAX_VIEWING).toSet(),
        sessions = presence.sessions.take(PcPresence.MAX_VIEWING).toSet(),
        untilMs = presence.untilMs,
    )
}
