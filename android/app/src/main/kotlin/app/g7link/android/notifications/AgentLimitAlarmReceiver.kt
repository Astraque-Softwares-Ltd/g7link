// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.notifications

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import app.g7link.android.ui.agents.AgentsModel

/**
 * A limit-reset alarm went off ([AgentLimitAlarm]). Not exported: only this
 * app's own alarms reach it. It may post a notification; it never sends
 * anything to the PC.
 */
class AgentLimitAlarmReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != AgentLimitAlarm.ACTION_FIRE) return
        val pcId = intent.getStringExtra(AgentLimitAlarm.EXTRA_PC)?.takeIf { AgentsModel.isId(it, max = AgentsModel.MAX_PC_ID) } ?: return
        val agent = intent.getStringExtra(AgentLimitAlarm.EXTRA_AGENT)?.takeIf { AgentsModel.isId(it) } ?: return
        AgentLimitAlarm.fire(context, pcId, agent)
    }
}
