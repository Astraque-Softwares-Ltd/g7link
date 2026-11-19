// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SessionsConsentTest {
    private class MemoryStore : SessionsConsent.Store {
        val allowed = mutableSetOf<String>()

        override fun allowedBefore(pcId: String): Boolean = pcId in allowed

        override fun setAllowedBefore(pcId: String, allowed: Boolean) {
            if (allowed) this.allowed += pcId else this.allowed -= pcId
        }
    }

    @Test
    fun `nothing is asked unprompted until the user asks or the PC listed them before`() {
        val store = MemoryStore()
        val consent = SessionsConsent(store)
        assertFalse(consent.mayAskAutomatically("pc1"))

        consent.userAsked("pc1")
        assertTrue(consent.mayAskAutomatically("pc1"))
        assertFalse(consent.mayAskAutomatically("pc2"))

        // A new run: only what the PC answered is kept.
        val nextRun = SessionsConsent(store)
        assertFalse(nextRun.mayAskAutomatically("pc1"))
        consent.answered("pc1", SessionsConsent.Answer.LISTED)
        assertTrue(SessionsConsent(store).mayAskAutomatically("pc1"))
    }

    @Test
    fun `a refusal or an unpairing forgets that the PC listed them`() {
        val store = MemoryStore()
        val consent = SessionsConsent(store)
        consent.answered("pc1", SessionsConsent.Answer.LISTED)
        consent.answered("pc1", SessionsConsent.Answer.OTHER)
        assertTrue(SessionsConsent(store).mayAskAutomatically("pc1"))

        consent.answered("pc1", SessionsConsent.Answer.REFUSED)
        assertFalse(SessionsConsent(store).mayAskAutomatically("pc1"))

        consent.answered("pc2", SessionsConsent.Answer.LISTED)
        consent.userAsked("pc2")
        consent.forget("pc2")
        assertFalse(consent.mayAskAutomatically("pc2"))
        assertFalse(SessionsConsent(store).mayAskAutomatically("pc2"))
    }

    @Test
    fun `asked in this run, a refusal still lets the user's request be checked again`() {
        val consent = SessionsConsent(MemoryStore())
        consent.userAsked("pc1")
        consent.answered("pc1", SessionsConsent.Answer.REFUSED)
        assertTrue(consent.mayAskAutomatically("pc1"))
    }
}
