// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import java.util.concurrent.ConcurrentHashMap

/**
 * When the phone may ask a PC for its sessions started outside G7 on its own.
 * The first refused request makes the PC show its user a prompt, so the phone
 * never asks unprompted unless that PC has listed them to it before (kept per
 * PC, until unpaired or refused) or the user tapped "Ask on <PC>" in this run.
 */
class SessionsConsent(private val store: Store) {
    /** Where "this PC listed its sessions before" is kept across runs. */
    interface Store {
        fun allowedBefore(pcId: String): Boolean

        fun setAllowedBefore(pcId: String, allowed: Boolean)
    }

    /** How a sessions request ended, as far as consent goes. */
    enum class Answer { LISTED, REFUSED, OTHER }

    private val askedThisRun: MutableSet<String> = ConcurrentHashMap.newKeySet()

    /** A request now can't make the PC prompt without the user having asked for it. */
    fun mayAskAutomatically(pcId: String): Boolean = pcId in askedThisRun || store.allowedBefore(pcId)

    /** The user tapped "Ask on <PC>": requests may go out (and prompt) for the rest of this run. */
    fun userAsked(pcId: String) {
        askedThisRun += pcId
    }

    fun answered(pcId: String, answer: Answer) {
        when (answer) {
            Answer.LISTED -> if (!store.allowedBefore(pcId)) store.setAllowedBefore(pcId, true)
            // Turned off on the PC since: back to asking first.
            Answer.REFUSED -> if (store.allowedBefore(pcId)) store.setAllowedBefore(pcId, false)
            Answer.OTHER -> Unit
        }
    }

    /** The PC was unpaired. */
    fun forget(pcId: String) {
        askedThisRun -= pcId
        store.setAllowedBefore(pcId, false)
    }
}
