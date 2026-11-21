// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable

/**
 * Handing a thread between the PC and this phone (contract §V8.5): what an
 * arriving hand-off does, how long it lasts, when a walk-away offer goes, and
 * what the PC said when this phone handed one over. A hand-off only ever
 * opens a view: it never sends, types, unlocks or answers anything.
 */
object AgentHandoffs {
    /** A hand-off lasts this long at most, whatever the PC says. */
    const val LIFE_MS = 10 * 60_000L

    /** What a hand-off from the PC does on arrival. */
    enum class Arrival {
        /** G7 Link is in front: the thread opens at once. */
        OPEN,

        /** "Continue on this phone", which opens it when tapped. */
        NOTIFY,

        /** The walk-away offer, on the quiet channel. */
        OFFER,

        /** Nothing: it lapsed, the offer is turned off, or someone is in Do Not Disturb. */
        DROP,
    }

    /** Everything that decides what an arriving hand-off does. */
    data class Incoming(
        /** The PC locked with this thread on screen (not one the user handed over). */
        val offer: Boolean,
        /** When it lapses, by this phone's clock. */
        val expiresMs: Long,
        /** G7 Link is on screen. */
        val inFront: Boolean,
        /** This thread is on the phone's screen already. */
        val showing: Boolean,
        /** "Offer the thread here when I leave <PC>" (on). */
        val offerSetting: Boolean,
        val phoneDnd: Boolean,
        /** The PC said it's in Do Not Disturb, and that's still fresh. */
        val pcDnd: Boolean,
    )

    fun arrival(incoming: Incoming, nowMs: Long): Arrival = when {
        incoming.expiresMs <= nowMs -> Arrival.DROP
        incoming.showing -> Arrival.DROP
        incoming.offer -> if (!incoming.offerSetting || incoming.phoneDnd || incoming.pcDnd) Arrival.DROP else Arrival.OFFER
        incoming.inFront -> Arrival.OPEN
        else -> Arrival.NOTIFY
    }

    /** How long its notification stays: until it lapses, and never more than [LIFE_MS]. */
    fun lastsMs(expiresMs: Long, nowMs: Long): Long = (expiresMs - nowMs).coerceIn(0L, LIFE_MS)

    /** The walk-away offer goes once the PC says someone is at it again. */
    fun offerWithdrawn(presence: PcPresence?, nowMs: Long): Boolean = presence != null && presence.fresh(nowMs) && presence.atPc

    /** What the PC said when this phone handed a thread over. */
    enum class Sent {
        /** Open on the PC now. */
        OPENED,

        /** It opens when the PC is unlocked ("Opens when <PC> is unlocked."). */
        LOCKED,

        /** Nobody's there to open it (G7 Link isn't open on the PC). */
        AWAY,

        /** It didn't reach the PC, or the PC refused it. */
        FAILED,
    }

    /** A hand-off from the PC that opens a thread: where it starts, and that its draft came from the PC. */
    @Immutable
    data class Opened(val pcId: String, val threadId: String, val seq: ULong?, val fromPc: Boolean)

    @Volatile private var pending: Opened? = null

    /** The thread [threadId] opens from a hand-off ([seq]: the first message on the PC's screen). */
    fun request(pcId: String, threadId: String, seq: ULong?) {
        pending = Opened(pcId, threadId, seq?.takeIf { it > 0uL }, fromPc = true)
    }

    /** The hand-off [threadId] is opened for, once. */
    @Synchronized
    fun take(pcId: String, threadId: String): Opened? {
        val opened = pending?.takeIf { it.pcId == pcId && it.threadId == threadId } ?: return null
        pending = null
        return opened
    }

    /** "Draft from <PC>" shows over the box while it still holds what the PC handed over. */
    fun draftLine(handedOver: Boolean, loaded: String?, box: String): Boolean =
        handedOver && !loaded.isNullOrBlank() && box == loaded
}
