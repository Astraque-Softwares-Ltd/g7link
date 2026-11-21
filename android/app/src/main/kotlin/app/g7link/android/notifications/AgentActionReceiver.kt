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
 * Allow, Deny, Say why, Stop and Reply on an agent thread's notification, a
 * question's options and Type, Stop following on a followed session's, and a
 * Live Update swiped away ([AgentNotifications]). Not exported: only this
 * app's own notifications reach it. Everything but Deny and Stop is refused
 * while the phone is locked, even where Android already asked to unlock first.
 * A tap on Allow, an option or Type within 1.5 s of the notification changing
 * to that request is ignored (contract §V3.1): it may have been meant for the
 * one before.
 */
class AgentActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val pcId = intent.getStringExtra(EXTRA_PC)?.takeIf { AgentsModel.isId(it, max = AgentsModel.MAX_PC_ID) } ?: return
        val core = (context.applicationContext as G7LinkApplication).core
        when (intent.action) {
            ACTION_UNFOLLOW -> {
                val sessionId = intent.getStringExtra(EXTRA_SESSION)?.takeIf { AgentsModel.isId(it) } ?: return
                core.agentSessionKeep(pcId, sessionId, false)
                return
            }
            ACTION_DISMISSED -> {
                val threadId = intent.getStringExtra(EXTRA_THREAD)?.takeIf { AgentsModel.isId(it) }
                val sessionId = intent.getStringExtra(EXTRA_SESSION)?.takeIf { AgentsModel.isId(it) }
                when {
                    threadId != null -> AgentNotifications.dismissed(AgentNotifications.tag(pcId, threadId))
                    sessionId != null -> AgentNotifications.dismissed(AgentNotifications.sessionTag(pcId, sessionId))
                }
                return
            }
        }
        val threadId = intent.getStringExtra(EXTRA_THREAD)?.takeIf { AgentsModel.isId(it) } ?: return
        val restore = { AgentNotifications.restore(context, pcId, threadId) }
        val tooSoon = { token: String -> kindOf(intent.action)?.let(AgentNotifications::guards) == true && AgentNotifications.guarded(token) }
        when (intent.action) {
            ACTION_ALLOW, ACTION_DENY -> {
                val token = intent.getStringExtra(EXTRA_TOKEN)?.takeIf { AgentsModel.isId(it) } ?: return
                val allow = intent.action == ACTION_ALLOW
                if (allow && locked(context)) return
                if (tooSoon(token)) return
                AgentNotifications.answered(pcId, threadId)
                core.agentAnswerFromNotification(pcId, threadId, token, allow)
            }
            ACTION_DENY_WHY -> {
                val token = intent.getStringExtra(EXTRA_TOKEN)?.takeIf { AgentsModel.isId(it) } ?: return
                // The reason reaches the agent as an instruction: never from a locked phone.
                if (locked(context)) return restore()
                val reason = AgentsModel.denyReason(typed(intent, AgentNotifications.DENY_WHY_KEY)) ?: return restore()
                AgentNotifications.answered(pcId, threadId)
                core.agentAnswerFromNotification(pcId, threadId, token, allow = false, message = reason)
            }
            ACTION_OPTION -> {
                val token = intent.getStringExtra(EXTRA_TOKEN)?.takeIf { AgentsModel.isId(it) } ?: return
                val ask = intent.getStringExtra(EXTRA_ASK)?.takeIf { it.isNotEmpty() && it.length <= MAX_ASK_ID } ?: return
                val option = intent.getIntExtra(EXTRA_OPTION, -1).takeIf { it >= 0 } ?: return
                if (locked(context) || tooSoon(token)) return
                AgentNotifications.answered(pcId, threadId)
                core.agentQuestionFromNotification(pcId, threadId, token, ask, option = option, typed = null)
            }
            ACTION_ANSWER -> {
                val token = intent.getStringExtra(EXTRA_TOKEN)?.takeIf { AgentsModel.isId(it) } ?: return
                val ask = intent.getStringExtra(EXTRA_ASK)?.takeIf { it.isNotEmpty() && it.length <= MAX_ASK_ID } ?: return
                if (locked(context) || tooSoon(token)) return restore()
                val text = AgentsModel.typedAnswer(typed(intent, AgentNotifications.ANSWER_KEY)) ?: return restore()
                AgentNotifications.answered(pcId, threadId)
                core.agentQuestionFromNotification(pcId, threadId, token, ask, option = null, typed = text)
            }
            ACTION_STOP -> core.agentStopFromNotification(pcId, threadId)
            ACTION_REPLY -> {
                if (locked(context)) return
                val text = typed(intent, AgentNotifications.REPLY_KEY)
                    ?.toString()
                    ?.takeIf { it.isNotBlank() }
                    ?: return
                core.agentReplyFromNotification(pcId, threadId, text)
            }
        }
    }

    private fun kindOf(action: String?): NoticeAction.Kind? = when (action) {
        ACTION_ALLOW -> NoticeAction.Kind.ALLOW
        ACTION_DENY -> NoticeAction.Kind.DENY
        ACTION_DENY_WHY -> NoticeAction.Kind.DENY_WHY
        ACTION_OPTION -> NoticeAction.Kind.OPTION
        ACTION_ANSWER -> NoticeAction.Kind.ANSWER
        ACTION_STOP -> NoticeAction.Kind.STOP
        ACTION_REPLY -> NoticeAction.Kind.REPLY
        else -> null
    }

    private fun typed(intent: Intent, key: String): CharSequence? = RemoteInput.getResultsFromIntent(intent)?.getCharSequence(key)

    private fun locked(context: Context): Boolean =
        context.getSystemService(KeyguardManager::class.java)?.isDeviceLocked == true

    companion object {
        const val ACTION_ALLOW = "app.g7link.android.agents.ALLOW"
        const val ACTION_DENY = "app.g7link.android.agents.DENY"
        const val ACTION_DENY_WHY = "app.g7link.android.agents.DENY_WHY"
        const val ACTION_OPTION = "app.g7link.android.agents.OPTION"
        const val ACTION_ANSWER = "app.g7link.android.agents.ANSWER"
        const val ACTION_STOP = "app.g7link.android.agents.STOP"
        const val ACTION_REPLY = "app.g7link.android.agents.REPLY"
        const val ACTION_UNFOLLOW = "app.g7link.android.agents.UNFOLLOW"
        const val ACTION_DISMISSED = "app.g7link.android.agents.DISMISSED"
        const val EXTRA_PC = "agent_pc"
        const val EXTRA_THREAD = "agent_thread"
        const val EXTRA_SESSION = "agent_session"
        const val EXTRA_TOKEN = "agent_token"
        const val EXTRA_ASK = "agent_ask"
        const val EXTRA_OPTION = "agent_option"

        /** An ask's id is the agent's, not the wire's `[a-z0-9]`: only its length is bounded here. */
        private const val MAX_ASK_ID = 128
    }
}
