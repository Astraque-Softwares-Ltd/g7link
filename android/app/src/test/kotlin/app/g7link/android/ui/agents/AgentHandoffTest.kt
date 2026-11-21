// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Handing a thread between the PC and the phone (contract §V8.5); it only ever opens a view. */
class AgentHandoffTest {
    private val now = 1_000_000L

    private val handed = AgentHandoffs.Incoming(
        offer = false,
        expiresMs = now + 600_000L,
        inFront = false,
        showing = false,
        offerSetting = true,
        phoneDnd = false,
        pcDnd = false,
    )

    @Test
    fun `a thread handed over opens at once in front, else says Continue on this phone`() {
        assertEquals(AgentHandoffs.Arrival.NOTIFY, AgentHandoffs.arrival(handed, now))
        assertEquals(AgentHandoffs.Arrival.OPEN, AgentHandoffs.arrival(handed.copy(inFront = true), now))
        // Already on the phone's screen: nothing to do.
        assertEquals(AgentHandoffs.Arrival.DROP, AgentHandoffs.arrival(handed.copy(inFront = true, showing = true), now))
        // Asked for explicitly, Do Not Disturb and the offer setting don't stop it.
        assertEquals(AgentHandoffs.Arrival.NOTIFY, AgentHandoffs.arrival(handed.copy(phoneDnd = true, pcDnd = true, offerSetting = false), now))
    }

    @Test
    fun `a lapsed hand-off does nothing, and none lasts more than 10 minutes`() {
        assertEquals(AgentHandoffs.Arrival.DROP, AgentHandoffs.arrival(handed.copy(expiresMs = now), now))
        assertEquals(AgentHandoffs.Arrival.DROP, AgentHandoffs.arrival(handed.copy(expiresMs = now - 1), now))
        assertEquals(60_000L, AgentHandoffs.lastsMs(now + 60_000L, now))
        assertEquals(AgentHandoffs.LIFE_MS, AgentHandoffs.lastsMs(now + 86_400_000L, now))
        assertEquals(0L, AgentHandoffs.lastsMs(now - 5, now))
    }

    @Test
    fun `the walk-away offer shows only when turned on and nobody is in Do Not Disturb`() {
        val offer = handed.copy(offer = true)
        assertEquals(AgentHandoffs.Arrival.OFFER, AgentHandoffs.arrival(offer, now))
        // Offered, not asked for: it never opens by itself, even in front.
        assertEquals(AgentHandoffs.Arrival.OFFER, AgentHandoffs.arrival(offer.copy(inFront = true), now))
        assertEquals(AgentHandoffs.Arrival.DROP, AgentHandoffs.arrival(offer.copy(offerSetting = false), now))
        assertEquals(AgentHandoffs.Arrival.DROP, AgentHandoffs.arrival(offer.copy(phoneDnd = true), now))
        assertEquals(AgentHandoffs.Arrival.DROP, AgentHandoffs.arrival(offer.copy(pcDnd = true), now))
        assertEquals(AgentHandoffs.Arrival.DROP, AgentHandoffs.arrival(offer.copy(expiresMs = now - 1), now))
    }

    @Test
    fun `the offer goes when someone is at the PC again, and stays while that's unknown`() {
        fun presence(atPc: Boolean, untilMs: Long = now + 60_000L) =
            PcPresence(atPc = atPc, locked = !atPc, dnd = false, untilMs = untilMs)
        assertTrue(AgentHandoffs.offerWithdrawn(presence(atPc = true), now))
        assertFalse(AgentHandoffs.offerWithdrawn(presence(atPc = false), now))
        assertFalse(AgentHandoffs.offerWithdrawn(presence(atPc = true, untilMs = now), now))
        assertFalse(AgentHandoffs.offerWithdrawn(null, now))
    }

    @Test
    fun `the thread screen takes a hand-off once, for its own thread`() {
        AgentHandoffs.request("pc1", "t1", 41uL)
        assertNull(AgentHandoffs.take("pc1", "t2"))
        assertNull(AgentHandoffs.take("pc2", "t1"))
        assertEquals(AgentHandoffs.Opened("pc1", "t1", 41uL, fromPc = true), AgentHandoffs.take("pc1", "t1"))
        assertNull(AgentHandoffs.take("pc1", "t1"))
        // No place on the PC's screen: it opens at the end.
        AgentHandoffs.request("pc1", "t1", 0uL)
        assertNull(AgentHandoffs.take("pc1", "t1")!!.seq)
    }

    @Test
    fun `Draft from the PC shows while the box still holds what came`() {
        assertTrue(AgentHandoffs.draftLine(handedOver = true, loaded = "Pia log kila retry", box = "Pia log kila retry"))
        assertFalse(AgentHandoffs.draftLine(handedOver = true, loaded = "Pia log kila retry", box = "Pia log kila retry na reference"))
        assertFalse(AgentHandoffs.draftLine(handedOver = true, loaded = null, box = ""))
        assertFalse(AgentHandoffs.draftLine(handedOver = false, loaded = "words", box = "words"))
    }
}
