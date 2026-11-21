// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.notifications

import android.content.Context
import androidx.core.content.edit

/**
 * A PC's agent notification settings on this phone (contract §V4.4, §V5),
 * both on until turned off: stay quiet while someone is at that PC looking
 * at the thread, and say when an agent's usage limit resets.
 */
object AgentNoticeSettings {
    private const val PREFS = "agent_notices"
    private const val QUIET_AT_PC = "quiet_at_pc."
    private const val LIMIT_ALARM = "limit_alarm."

    fun quietAtPc(context: Context, pcId: String): Boolean = prefs(context).getBoolean(QUIET_AT_PC + pcId, true)

    fun setQuietAtPc(context: Context, pcId: String, on: Boolean) {
        prefs(context).edit { if (on) remove(QUIET_AT_PC + pcId) else putBoolean(QUIET_AT_PC + pcId, false) }
    }

    fun limitAlarm(context: Context, pcId: String): Boolean = prefs(context).getBoolean(LIMIT_ALARM + pcId, true)

    fun setLimitAlarm(context: Context, pcId: String, on: Boolean) {
        prefs(context).edit { if (on) remove(LIMIT_ALARM + pcId) else putBoolean(LIMIT_ALARM + pcId, false) }
    }

    /** The PC was unpaired: paired again, it starts from the defaults. */
    fun forgetPc(context: Context, pcId: String) {
        prefs(context).edit {
            remove(QUIET_AT_PC + pcId)
            remove(LIMIT_ALARM + pcId)
        }
    }

    /** Forgets the PCs that aren't among [paired]. */
    fun keepPcs(context: Context, paired: Collection<String>) {
        val prefs = prefs(context)
        val gone = prefs.all.keys.filter { key ->
            val pc = when {
                key.startsWith(QUIET_AT_PC) -> key.removePrefix(QUIET_AT_PC)
                key.startsWith(LIMIT_ALARM) -> key.removePrefix(LIMIT_ALARM)
                else -> return@filter false
            }
            pc !in paired
        }
        if (gone.isNotEmpty()) prefs.edit { gone.forEach { remove(it) } }
    }

    private fun prefs(context: Context) = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
}
