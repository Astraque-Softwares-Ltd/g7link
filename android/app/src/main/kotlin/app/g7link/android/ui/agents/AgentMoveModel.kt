// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable
import app.g7link.core.AgentContinueBlock
import app.g7link.core.AgentSession
import app.g7link.core.AgentSessionSource
import app.g7link.core.AgentSessionState

/** What became of a move (docs/protocol/agents.md §9). A result this phone doesn't know comes as [FAILED]. */
enum class MoveResult {
    /** The PC continued it as a thread: open it. */
    MOVED,

    /** The agent is gone on the PC; the thread isn't made yet. The phone continues it once the PC's list says it can. */
    CLOSED,

    /** Nothing was done on the PC. */
    REFUSED,
    FAILED,
}

/** Why, in the PC's words: every wire word, so this model alone decides what each means here. */
enum class MoveReason {
    LOCKED,
    IN_USE,
    CONFIRM,
    NEEDS_YOU,
    BUSY,
    ENDED,
    NO_PROCESS,
    NO_WINDOW,
    AMBIGUOUS,
    TAB,
    UNPROVEN,
    PLACE,
    RATE,
    SETUP,
    FOLDER,
    AGENT,
    FOCUS,
    INTERRUPTED,
    INPUT,
    DRAFT,
    OPEN,
}

/** The PC's answer to a move. [threadId]: the thread it became, with [MoveResult.MOVED]. */
@Immutable
data class MoveAnswer(
    val result: MoveResult,
    val reason: MoveReason? = null,
    val target: ReplyTarget? = null,
    val nonce: String? = null,
    val threadId: String? = null,
)

/**
 * The line a move's answer leaves. [amber]: something to do on the PC.
 * [sticky]: a state change doesn't clear it, only its close or the next move.
 *
 * The PC ends the agent with a signal (the owner's choice, plans/move-and-focus.md
 * §2 option b), so nothing here asks to unlock the PC, to stop using it, or
 * to check a window: those lines belong to a typed exit only.
 */
enum class MoveNote(val amber: Boolean = false, val sticky: Boolean = false) {
    /** "Claude is mid-turn. Move it when it's done." */
    BUSY,

    /** "It's waiting for an answer on guchu." */
    NEEDS_YOU(amber = true),

    /** "Too many at once. Try again in a moment." */
    RATE,

    /** "Add g7linux to your agent folders on guchu to continue it here." */
    FOLDER(amber = true),

    /** "Claude can't run on guchu right now." */
    AGENT,

    /** "G7 can't close it where it runs. Close it there to continue here." */
    CANT,

    /** "Claude didn't close on guchu. Close it there to continue here." */
    OPEN(amber = true, sticky = true),

    /** "Not known if it was moved. Check the session." */
    NOT_KNOWN(sticky = true),

    /** "Couldn't move it." */
    FAILED,

    /** "Can't reach guchu.": it never left this phone. */
    UNREACHABLE,
}

/** The first move into this session's terminal asks first: where it runs, and the PC's one-time word for it. */
@Immutable
data class MoveConfirm(val target: ReplyTarget?, val nonce: String)

/** A followed session's move on this screen. Held by the screen only: leaving it drops it. */
@Immutable
data class MoveState(
    /** A move is on its way: "Closing Claude on guchu…". */
    val asking: Boolean = false,
    val confirm: MoveConfirm? = null,
    /** When the PC said the agent closed without continuing it: "Closed on guchu. Continuing here…" until then plus [AgentMoveModel.CLOSED_WAIT_MS]. */
    val closedAtMs: Long? = null,
    val note: MoveNote? = null,
    /** Bumped when a move starts: an answer to an older one is dropped. */
    val epoch: Int = 0,
)

/** Where "Move to G7" stands on the screen. */
enum class MoveShow {
    HIDDEN,

    /** "Move to G7" beside the reply box's target, or as the session card's button. */
    OFFER,

    /** "Closing Claude on guchu…" with a spinner. */
    CLOSING,

    /** "Closed on guchu. Continuing here…" with a spinner. */
    CLOSED,
}

/**
 * "Move to G7" (plans/move-and-focus.md §10), the pure part: a followed
 * session idle in a terminal on the PC, ended there and continued here as a
 * thread in one tap. The PC proves the process and checks every rule; the
 * phone shows what it said and continues the session itself only when the
 * PC closed the agent but couldn't make the thread yet.
 */
object AgentMoveModel {
    /** How long after `closed` this screen continues the session by itself once the PC's list allows it. */
    const val CLOSED_WAIT_MS = 120_000L

    /** A nonce is 32 hex digits (§4 limits); anything else isn't sent back. */
    private val NONCE = Regex("[0-9a-f]{32}")

    /** Whether to offer "Move to G7": the PC offers `agent.move`, and the session is idle in a terminal that still has it open. */
    fun offered(pcOffersMove: Boolean, session: AgentSession?): Boolean =
        pcOffersMove &&
            session != null &&
            session.source == AgentSessionSource.TERMINAL &&
            session.state == AgentSessionState.IDLE &&
            !AgentsModel.mayNeedYou(session) &&
            session.blocked == AgentContinueBlock.OPEN

    /** What the screen shows for the move now. */
    fun shown(state: MoveState, offered: Boolean): MoveShow = when {
        state.asking -> MoveShow.CLOSING
        state.closedAtMs != null -> MoveShow.CLOSED
        offered -> MoveShow.OFFER
        else -> MoveShow.HIDDEN
    }

    /** A move goes now (the action, or the sheet's Move). */
    fun started(state: MoveState): MoveState =
        state.copy(asking = true, confirm = null, closedAtMs = null, note = null, epoch = state.epoch + 1)

    /** A move's answer, for the move that started at [epoch]. */
    fun answered(state: MoveState, epoch: Int, call: AgentCall<MoveAnswer>, nowMs: Long): MoveState {
        if (epoch != state.epoch || !state.asking) return state
        val done = state.copy(asking = false)
        val answer = when (call) {
            is AgentCall.Ok -> call.value
            is AgentCall.Failed -> return done.copy(
                note = when (call.failure) {
                    // It went and no answer came: the agent may have been closed, or the thread made.
                    AgentFailure.TIMEOUT -> MoveNote.NOT_KNOWN
                    AgentFailure.OFFLINE -> MoveNote.UNREACHABLE
                    AgentFailure.DENIED, AgentFailure.UNSUPPORTED, AgentFailure.NOT_FOUND -> MoveNote.CANT
                    AgentFailure.OTHER -> MoveNote.FAILED
                },
            )
        }
        return when (answer.result) {
            // Without the thread there is nothing to open: wait for the list, as after `closed`.
            MoveResult.MOVED -> if (answer.threadId != null) done else done.copy(closedAtMs = nowMs)
            MoveResult.CLOSED -> done.copy(closedAtMs = nowMs)
            MoveResult.REFUSED -> when (answer.reason) {
                MoveReason.CONFIRM -> {
                    val nonce = answer.nonce?.takeIf { NONCE.matches(it) }
                    if (nonce == null) done.copy(note = MoveNote.FAILED) else done.copy(confirm = MoveConfirm(answer.target, nonce))
                }
                // Already gone where it ran: what the user asked for is the thread, so continue it as after `closed`.
                MoveReason.ENDED -> done.copy(closedAtMs = nowMs)
                else -> done.copy(note = noteOf(answer.reason))
            }
            MoveResult.FAILED -> done.copy(note = noteOf(answer.reason))
        }
    }

    /** The thread to open after [call]: only a `moved` with its thread. */
    fun opens(call: AgentCall<MoveAnswer>): String? =
        (call as? AgentCall.Ok)?.value?.takeIf { it.result == MoveResult.MOVED }?.threadId

    /** Whether to continue the session here now: the PC closed it within [CLOSED_WAIT_MS] and its list says it can be continued. */
    fun shouldContinue(state: MoveState, session: AgentSession?, nowMs: Long): Boolean {
        val at = state.closedAtMs ?: return false
        return session != null && session.canContinue && nowMs < at + CLOSED_WAIT_MS
    }

    /** The continue went (it shows its own spinner and its own failure). */
    fun continued(state: MoveState): MoveState = state.copy(closedAtMs = null)

    /** Past [CLOSED_WAIT_MS] after `closed`: the session card's own Continue here is what's left. */
    fun expired(state: MoveState, nowMs: Long): MoveState {
        val at = state.closedAtMs ?: return state
        return if (nowMs >= at + CLOSED_WAIT_MS) state.copy(closedAtMs = null) else state
    }

    /** Cancel on the sheet: nothing was done on the PC. */
    fun confirmCancelled(state: MoveState): MoveState = state.copy(confirm = null)

    /** The close beside a sticky line. */
    fun dismissed(state: MoveState): MoveState = if (state.note?.sticky == true) state.copy(note = null) else state

    /** The session's state changed: a passing line no longer says how it is (a sticky one stays). */
    fun sessionChanged(state: MoveState): MoveState = if (state.note?.sticky == false) state.copy(note = null) else state

    /** The line for a refusal or failure. */
    fun noteOf(reason: MoveReason?): MoveNote = when (reason) {
        MoveReason.BUSY -> MoveNote.BUSY
        MoveReason.NEEDS_YOU -> MoveNote.NEEDS_YOU
        MoveReason.RATE -> MoveNote.RATE
        MoveReason.FOLDER -> MoveNote.FOLDER
        MoveReason.AGENT -> MoveNote.AGENT
        MoveReason.PLACE, MoveReason.NO_WINDOW, MoveReason.NO_PROCESS, MoveReason.SETUP, MoveReason.AMBIGUOUS, MoveReason.ENDED -> MoveNote.CANT
        MoveReason.OPEN -> MoveNote.OPEN
        // Only a typed exit answers these, and the PC ends the agent with a signal: read as an unknown word.
        MoveReason.LOCKED, MoveReason.IN_USE, MoveReason.TAB, MoveReason.UNPROVEN, MoveReason.FOCUS,
        MoveReason.INTERRUPTED, MoveReason.INPUT, MoveReason.DRAFT,
        // Not a line: the sheet, or (without a usable nonce) a plain failure.
        MoveReason.CONFIRM,
        null,
        -> MoveNote.FAILED
    }
}
