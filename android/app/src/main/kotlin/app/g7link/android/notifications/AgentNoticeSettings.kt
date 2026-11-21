// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.notifications

import android.content.Context
import androidx.core.content.edit

/**
 * A PC's agent notification settings on this phone (contract §V4.4, §V5,
 * §V8.5, §V8.6): stay quiet while someone is at that PC looking at the
 * thread, say when an agent's usage limit resets, and offer the thread here
 * when you leave the PC, all on until turned off; and let needs-you break
 * through Do Not Disturb, off until turned on.
 */
object AgentNoticeSettings {
    private const val PREFS = "agent_notices"
    private const val QUIET_AT_PC = "quiet_at_pc."
    private const val LIMIT_ALARM = "limit_alarm."
    private const val HANDOFF_OFFER = "handoff_offer."
    private const val BREAK_DND = "break_dnd."
    private val KEYS = listOf(QUIET_AT_PC, LIMIT_ALARM, HANDOFF_OFFER, BREAK_DND)

    fun quietAtPc(context: Context, pcId: String): Boolean = prefs(context).getBoolean(QUIET_AT_PC + pcId, true)

    fun setQuietAtPc(context: Context, pcId: String, on: Boolean) {
        prefs(context).edit { if (on) remove(QUIET_AT_PC + pcId) else putBoolean(QUIET_AT_PC + pcId, false) }
    }

    fun limitAlarm(context: Context, pcId: String): Boolean = prefs(context).getBoolean(LIMIT_ALARM + pcId, true)

    fun setLimitAlarm(context: Context, pcId: String, on: Boolean) {
        prefs(context).edit { if (on) remove(LIMIT_ALARM + pcId) else putBoolean(LIMIT_ALARM + pcId, false) }
    }

    /** "Offer the thread here when I leave <PC>" (on). */
    fun handoffOffer(context: Context, pcId: String): Boolean = prefs(context).getBoolean(HANDOFF_OFFER + pcId, true)

    fun setHandoffOffer(context: Context, pcId: String, on: Boolean) {
        prefs(context).edit { if (on) remove(HANDOFF_OFFER + pcId) else putBoolean(HANDOFF_OFFER + pcId, false) }
    }

    /** "Needs-you breaks through Do Not Disturb" (off). */
    fun breakDnd(context: Context, pcId: String): Boolean = prefs(context).getBoolean(BREAK_DND + pcId, false)

    fun setBreakDnd(context: Context, pcId: String, on: Boolean) {
        prefs(context).edit { if (on) putBoolean(BREAK_DND + pcId, true) else remove(BREAK_DND + pcId) }
    }

    /** The PC was unpaired: paired again, it starts from the defaults. */
    fun forgetPc(context: Context, pcId: String) {
        prefs(context).edit { KEYS.forEach { remove(it + pcId) } }
    }

    /** Forgets the PCs that aren't among [paired]. */
    fun keepPcs(context: Context, paired: Collection<String>) {
        val prefs = prefs(context)
        val gone = prefs.all.keys.filter { key ->
            val prefix = KEYS.firstOrNull { key.startsWith(it) } ?: return@filter false
            key.removePrefix(prefix) !in paired
        }
        if (gone.isNotEmpty()) prefs.edit { gone.forEach { remove(it) } }
    }

    private fun prefs(context: Context) = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
}
