// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentReplyAnswer
import app.g7link.core.AgentReplyKind
import app.g7link.core.AgentReplyReason
import app.g7link.core.AgentReplyResult
import app.g7link.core.AgentReplyTarget

/** A PC's answer to a check or a reply (docs/protocol/agents.md §9), as [AgentReplyModel] reads it. */
object AgentReplyFfi {
    fun of(answer: AgentReplyAnswer): ReplyAnswer = ReplyAnswer(
        result = when (answer.result) {
            AgentReplyResult.READY -> ReplyResult.READY
            AgentReplyResult.SENT -> ReplyResult.SENT
            AgentReplyResult.REFUSED -> ReplyResult.REFUSED
            AgentReplyResult.FAILED -> ReplyResult.FAILED
        },
        reason = answer.reason?.let(::reasonOf),
        target = answer.target?.let(::targetOf),
        nonce = answer.nonce,
        retryMs = answer.retryMs?.toLong(),
    )

    fun targetOf(target: AgentReplyTarget): ReplyTarget =
        ReplyTarget(app = target.app, agent = target.agent, folder = target.folder, inApp = target.kind == AgentReplyKind.APP)

    fun reasonOf(reason: AgentReplyReason): ReplyReason = when (reason) {
        AgentReplyReason.LOCKED -> ReplyReason.LOCKED
        AgentReplyReason.IN_USE -> ReplyReason.IN_USE
        AgentReplyReason.CONFIRM -> ReplyReason.CONFIRM
        AgentReplyReason.NEEDS_YOU -> ReplyReason.NEEDS_YOU
        AgentReplyReason.BUSY -> ReplyReason.BUSY
        AgentReplyReason.ENDED -> ReplyReason.ENDED
        AgentReplyReason.NO_PROCESS -> ReplyReason.NO_PROCESS
        AgentReplyReason.NO_WINDOW -> ReplyReason.NO_WINDOW
        AgentReplyReason.AMBIGUOUS -> ReplyReason.AMBIGUOUS
        AgentReplyReason.TAB -> ReplyReason.TAB
        AgentReplyReason.PLACE -> ReplyReason.PLACE
        AgentReplyReason.CLIPBOARD -> ReplyReason.CLIPBOARD
        AgentReplyReason.TEXT -> ReplyReason.TEXT
        AgentReplyReason.RATE -> ReplyReason.RATE
        AgentReplyReason.SETUP -> ReplyReason.SETUP
        AgentReplyReason.FOCUS -> ReplyReason.FOCUS
        AgentReplyReason.INTERRUPTED -> ReplyReason.INTERRUPTED
        AgentReplyReason.INPUT -> ReplyReason.INPUT
    }
}
