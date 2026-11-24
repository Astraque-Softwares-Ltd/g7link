// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentMoveAnswer
import app.g7link.core.AgentMoveReason
import app.g7link.core.AgentMoveResult
import app.g7link.core.AgentReplyNeeds

/** A PC's answer to a move (docs/protocol/agents.md §9), as [AgentMoveModel] reads it, and why it can't type replies. */
object AgentMoveFfi {
    fun of(answer: AgentMoveAnswer): MoveAnswer = MoveAnswer(
        result = when (answer.result) {
            AgentMoveResult.MOVED -> MoveResult.MOVED
            AgentMoveResult.CLOSED -> MoveResult.CLOSED
            AgentMoveResult.REFUSED -> MoveResult.REFUSED
            AgentMoveResult.FAILED -> MoveResult.FAILED
        },
        reason = answer.reason?.let(::reasonOf),
        target = answer.target?.let(AgentReplyFfi::targetOf),
        nonce = answer.nonce,
        threadId = answer.thread?.id,
    )

    fun reasonOf(reason: AgentMoveReason): MoveReason = when (reason) {
        AgentMoveReason.LOCKED -> MoveReason.LOCKED
        AgentMoveReason.IN_USE -> MoveReason.IN_USE
        AgentMoveReason.CONFIRM -> MoveReason.CONFIRM
        AgentMoveReason.NEEDS_YOU -> MoveReason.NEEDS_YOU
        AgentMoveReason.BUSY -> MoveReason.BUSY
        AgentMoveReason.ENDED -> MoveReason.ENDED
        AgentMoveReason.NO_PROCESS -> MoveReason.NO_PROCESS
        AgentMoveReason.NO_WINDOW -> MoveReason.NO_WINDOW
        AgentMoveReason.AMBIGUOUS -> MoveReason.AMBIGUOUS
        AgentMoveReason.TAB -> MoveReason.TAB
        AgentMoveReason.UNPROVEN -> MoveReason.UNPROVEN
        AgentMoveReason.PLACE -> MoveReason.PLACE
        AgentMoveReason.RATE -> MoveReason.RATE
        AgentMoveReason.SETUP -> MoveReason.SETUP
        AgentMoveReason.FOLDER -> MoveReason.FOLDER
        AgentMoveReason.AGENT -> MoveReason.AGENT
        AgentMoveReason.FOCUS -> MoveReason.FOCUS
        AgentMoveReason.INTERRUPTED -> MoveReason.INTERRUPTED
        AgentMoveReason.INPUT -> MoveReason.INPUT
        AgentMoveReason.DRAFT -> MoveReason.DRAFT
        AgentMoveReason.OPEN -> MoveReason.OPEN
    }

    /** What "Why?" says when the PC offers `agent.reply.needs.*` instead of replies. */
    fun needsOf(needs: AgentReplyNeeds): CantReply = when (needs) {
        AgentReplyNeeds.EXTENSION -> CantReply.SETUP
        AgentReplyNeeds.DESKTOP -> CantReply.DESKTOP
        AgentReplyNeeds.KEYBOARD -> CantReply.KEYBOARD
    }
}
