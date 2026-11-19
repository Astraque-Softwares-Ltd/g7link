// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.notifications

import android.app.KeyguardManager
import android.app.RemoteInput
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import app.g7link.android.G7LinkApplication
import app.g7link.android.ui.agents.AgentsModel

/**
 * Allow, Deny, Stop and Reply on an agent thread's notification
 * ([AgentNotifications]). Not exported: only this app's own notifications reach
 * it. Allow and Reply are refused while the phone is locked, even where Android
 * already asked to unlock first.
 */
class AgentActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val pcId = intent.getStringExtra(EXTRA_PC)?.takeIf { AgentsModel.isId(it, max = AgentsModel.MAX_PC_ID) } ?: return
        val threadId = intent.getStringExtra(EXTRA_THREAD)?.takeIf { AgentsModel.isId(it) } ?: return
        val core = (context.applicationContext as G7LinkApplication).core
        when (intent.action) {
            ACTION_ALLOW, ACTION_DENY -> {
                val token = intent.getStringExtra(EXTRA_TOKEN)?.takeIf { AgentsModel.isId(it) } ?: return
                val allow = intent.action == ACTION_ALLOW
                if (allow && locked(context)) return
                core.agentAnswerFromNotification(pcId, threadId, token, allow)
            }
            ACTION_STOP -> core.agentStopFromNotification(pcId, threadId)
            ACTION_REPLY -> {
                if (locked(context)) return
                val text = RemoteInput.getResultsFromIntent(intent)
                    ?.getCharSequence(AgentNotifications.REPLY_KEY)
                    ?.toString()
                    ?.takeIf { it.isNotBlank() }
                    ?: return
                core.agentReplyFromNotification(pcId, threadId, text)
            }
        }
    }

    private fun locked(context: Context): Boolean =
        context.getSystemService(KeyguardManager::class.java)?.isDeviceLocked == true

    companion object {
        const val ACTION_ALLOW = "app.g7link.android.agents.ALLOW"
        const val ACTION_DENY = "app.g7link.android.agents.DENY"
        const val ACTION_STOP = "app.g7link.android.agents.STOP"
        const val ACTION_REPLY = "app.g7link.android.agents.REPLY"
        const val EXTRA_PC = "agent_pc"
        const val EXTRA_THREAD = "agent_thread"
        const val EXTRA_TOKEN = "agent_token"
    }
}
