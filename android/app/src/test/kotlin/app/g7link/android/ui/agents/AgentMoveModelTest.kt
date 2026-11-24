// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentContinueBlock
import app.g7link.core.AgentMoveAnswer
import app.g7link.core.AgentMoveReason
import app.g7link.core.AgentMoveResult
import app.g7link.core.AgentReplyKind
import app.g7link.core.AgentReplyNeeds
import app.g7link.core.AgentReplyTarget
import app.g7link.core.AgentSession
import app.g7link.core.AgentSessionSource
import app.g7link.core.AgentSessionState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** "Move to G7": the phone's half (plans/move-and-focus.md §10), with the PC ending the agent by a signal (§2 option b). */
class AgentMoveModelTest {
    private val now = 1_795_260_720_000L
    private val terminal = ReplyTarget(app = "Terminal", agent = "Claude", folder = "g7linux", inApp = false)
    private val nonce = "0123456789abcdef0123456789abcdef"
    private val thread = "t0123456789abcdef"

    private fun session(
        state: AgentSessionState = AgentSessionState.IDLE,
        guess: Boolean = false,
        source: AgentSessionSource = AgentSessionSource.TERMINAL,
        blocked: AgentContinueBlock? = AgentContinueBlock.OPEN,
        canContinue: Boolean = false,
    ) = AgentSession(
        id = "sc0123456789abcdef0123456789abcdef",
        agent = "claude",
        folder = "g7linux",
        title = "Ongeza ukurasa wa malipo",
        state = state,
        guess = guess,
        source = source,
        updatedMs = now,
        step = null,
        summary = null,
        waitingFor = null,
        canContinue = canContinue,
        blocked = blocked,
        sinceMs = now - 60_000L,
        limit = null,
    )

    private fun ok(result: MoveResult, reason: MoveReason? = null, target: ReplyTarget? = null, nonce: String? = null, threadId: String? = null) =
        AgentCall.Ok(MoveAnswer(result, reason, target, nonce, threadId))

    /** A move started from [state] that got [call] back at [atMs]. */
    private fun move(call: AgentCall<MoveAnswer>, state: MoveState = MoveState(), atMs: Long = now): MoveState {
        val started = AgentMoveModel.started(state)
        assertTrue(started.asking)
        return AgentMoveModel.answered(started, started.epoch, call, atMs)
    }

    @Test
    fun `offered only for an idle terminal session still open, on a PC that offers moves`() {
        assertTrue(AgentMoveModel.offered(true, session()))
        // Each condition off in turn.
        assertFalse(AgentMoveModel.offered(false, session()))
        assertFalse(AgentMoveModel.offered(true, null))
        assertFalse(AgentMoveModel.offered(true, session(source = AgentSessionSource.DESKTOP_APP)))
        assertFalse(AgentMoveModel.offered(true, session(source = AgentSessionSource.OTHER)))
        for (state in listOf(AgentSessionState.WORKING, AgentSessionState.NEEDS_YOU, AgentSessionState.MAY_NEED_YOU, AgentSessionState.ENDED)) {
            assertFalse("$state", AgentMoveModel.offered(true, session(state = state)))
        }
        // A guess that says it may wait for an answer isn't idle.
        assertFalse(AgentMoveModel.offered(true, session(state = AgentSessionState.NEEDS_YOU, guess = true)))
        // An idle guessed from its file is still idle.
        assertTrue(AgentMoveModel.offered(true, session(guess = true)))
        // Not open any more (Continue here does it), or blocked by something a move can't change.
        assertFalse(AgentMoveModel.offered(true, session(blocked = null, canContinue = true)))
        assertFalse(AgentMoveModel.offered(true, session(blocked = AgentContinueBlock.FOLDER)))
        assertFalse(AgentMoveModel.offered(true, session(blocked = AgentContinueBlock.AGENT)))
    }

    @Test
    fun `what shows is the action, then closing while it goes, then closed while it waits for the list`() {
        assertEquals(MoveShow.HIDDEN, AgentMoveModel.shown(MoveState(), offered = false))
        assertEquals(MoveShow.OFFER, AgentMoveModel.shown(MoveState(), offered = true))
        val asking = AgentMoveModel.started(MoveState())
        assertEquals(MoveShow.CLOSING, AgentMoveModel.shown(asking, offered = true))
        // Once the agent is gone the session isn't offered any more; closed still shows.
        assertEquals(MoveShow.CLOSED, AgentMoveModel.shown(move(ok(MoveResult.CLOSED)), offered = false))
    }

    @Test
    fun `moved opens the thread and leaves no line`() {
        val call = ok(MoveResult.MOVED, threadId = thread)
        val state = move(call)
        assertFalse(state.asking)
        assertNull(state.note)
        assertNull(state.closedAtMs)
        assertEquals(thread, AgentMoveModel.opens(call))
        // Only a move opens a thread.
        assertNull(AgentMoveModel.opens(ok(MoveResult.CLOSED, threadId = thread)))
        assertNull(AgentMoveModel.opens(AgentCall.Failed(AgentFailure.TIMEOUT)))
    }

    @Test
    fun `moved without its thread waits for the list as closed does`() {
        val call = ok(MoveResult.MOVED)
        assertNull(AgentMoveModel.opens(call))
        assertEquals(now, move(call).closedAtMs)
    }

    @Test
    fun `closed continues here once the list says it can, within two minutes`() {
        val closed = move(ok(MoveResult.CLOSED))
        assertEquals(now, closed.closedAtMs)
        assertNull(closed.note)
        // Not yet: something still has it (Codex's last write was recent).
        assertFalse(AgentMoveModel.shouldContinue(closed, session(state = AgentSessionState.ENDED), now + 5_000L))
        assertFalse(AgentMoveModel.shouldContinue(closed, null, now + 5_000L))
        // Now it can.
        val free = session(state = AgentSessionState.ENDED, blocked = null, canContinue = true)
        assertTrue(AgentMoveModel.shouldContinue(closed, free, now + 5_000L))
        assertTrue(AgentMoveModel.shouldContinue(closed, free, now + AgentMoveModel.CLOSED_WAIT_MS - 1L))
        // Past the two minutes: the card's own Continue here is what's left.
        assertFalse(AgentMoveModel.shouldContinue(closed, free, now + AgentMoveModel.CLOSED_WAIT_MS))
        // Once, then nothing more by itself.
        val continued = AgentMoveModel.continued(closed)
        assertNull(continued.closedAtMs)
        assertFalse(AgentMoveModel.shouldContinue(continued, free, now + 5_000L))
        assertEquals(MoveShow.HIDDEN, AgentMoveModel.shown(continued, offered = false))
        // Nothing before a closed.
        assertFalse(AgentMoveModel.shouldContinue(MoveState(), free, now))
    }

    @Test
    fun `closed expires after two minutes, not before`() {
        val closed = move(ok(MoveResult.CLOSED))
        assertEquals(closed, AgentMoveModel.expired(closed, now + AgentMoveModel.CLOSED_WAIT_MS - 1L))
        assertNull(AgentMoveModel.expired(closed, now + AgentMoveModel.CLOSED_WAIT_MS).closedAtMs)
        assertEquals(MoveState(), AgentMoveModel.expired(MoveState(), now))
    }

    @Test
    fun `already ended where it ran reads as closed, so it is continued here`() {
        val state = move(ok(MoveResult.REFUSED, MoveReason.ENDED))
        assertEquals(now, state.closedAtMs)
        assertNull(state.note)
    }

    @Test
    fun `confirm with a good nonce asks first, a bad one fails, and Move sends it`() {
        val asked = move(ok(MoveResult.REFUSED, MoveReason.CONFIRM, terminal, nonce))
        assertEquals(MoveConfirm(terminal, nonce), asked.confirm)
        assertNull(asked.note)
        assertFalse(asked.asking)
        // Move on the sheet: the sheet goes, the move with the nonce goes.
        val again = AgentMoveModel.started(asked)
        assertNull(again.confirm)
        assertTrue(again.asking)
        // Cancel: nothing was done on the PC.
        assertEquals(asked.copy(confirm = null), AgentMoveModel.confirmCancelled(asked))
        for (bad in listOf(null, "", "0123456789ABCDEF0123456789ABCDEF", "0123", nonce + "0")) {
            val failed = move(ok(MoveResult.REFUSED, MoveReason.CONFIRM, terminal, bad))
            assertNull("$bad", failed.confirm)
            assertEquals("$bad", MoveNote.FAILED, failed.note)
        }
    }

    @Test
    fun `every reason to its line, and nothing asks to unlock, wait or check a window`() {
        val expected = mapOf(
            MoveReason.BUSY to MoveNote.BUSY,
            MoveReason.NEEDS_YOU to MoveNote.NEEDS_YOU,
            MoveReason.RATE to MoveNote.RATE,
            MoveReason.FOLDER to MoveNote.FOLDER,
            MoveReason.AGENT to MoveNote.AGENT,
            MoveReason.PLACE to MoveNote.CANT,
            MoveReason.NO_WINDOW to MoveNote.CANT,
            MoveReason.NO_PROCESS to MoveNote.CANT,
            MoveReason.SETUP to MoveNote.CANT,
            MoveReason.AMBIGUOUS to MoveNote.CANT,
            MoveReason.OPEN to MoveNote.OPEN,
            // A typed exit's words: the PC ends the agent with a signal, so they read as an unknown one.
            MoveReason.LOCKED to MoveNote.FAILED,
            MoveReason.IN_USE to MoveNote.FAILED,
            MoveReason.TAB to MoveNote.FAILED,
            MoveReason.UNPROVEN to MoveNote.FAILED,
            MoveReason.FOCUS to MoveNote.FAILED,
            MoveReason.INTERRUPTED to MoveNote.FAILED,
            MoveReason.INPUT to MoveNote.FAILED,
            MoveReason.DRAFT to MoveNote.FAILED,
        )
        for ((reason, note) in expected) {
            assertEquals("refused $reason", note, move(ok(MoveResult.REFUSED, reason)).note)
            assertEquals("failed $reason", note, move(ok(MoveResult.FAILED, reason)).note)
        }
        // Every word is decided here, and the two that aren't lines lead elsewhere.
        assertEquals(MoveReason.entries.toSet() - MoveReason.CONFIRM - MoveReason.ENDED, expected.keys)
        assertEquals(MoveNote.FAILED, AgentMoveModel.noteOf(null))
        // A word this phone doesn't know: "Couldn't move it".
        assertEquals(MoveNote.FAILED, move(ok(MoveResult.REFUSED)).note)
        assertEquals(MoveNote.FAILED, move(ok(MoveResult.FAILED)).note)
        // Amber is for something to do on the PC; only what a later look can't settle is sticky.
        assertEquals(setOf(MoveNote.NEEDS_YOU, MoveNote.FOLDER, MoveNote.OPEN), MoveNote.entries.filter { it.amber }.toSet())
        assertEquals(setOf(MoveNote.OPEN, MoveNote.NOT_KNOWN), MoveNote.entries.filter { it.sticky }.toSet())
    }

    @Test
    fun `no answer is not known, unreachable never left, the PC's no is can't`() {
        assertEquals(MoveNote.NOT_KNOWN, move(AgentCall.Failed(AgentFailure.TIMEOUT)).note)
        assertEquals(MoveNote.UNREACHABLE, move(AgentCall.Failed(AgentFailure.OFFLINE)).note)
        for (failure in listOf(AgentFailure.DENIED, AgentFailure.UNSUPPORTED, AgentFailure.NOT_FOUND)) {
            assertEquals("$failure", MoveNote.CANT, move(AgentCall.Failed(failure)).note)
        }
        // The PC's BUSY (another reply or move being entered), or anything else.
        assertEquals(MoveNote.FAILED, move(AgentCall.Failed(AgentFailure.OTHER)).note)
        for (failure in AgentFailure.entries) {
            val state = move(AgentCall.Failed(failure))
            assertFalse(state.asking)
            assertNull(state.closedAtMs)
            assertNull(state.confirm)
        }
    }

    @Test
    fun `an answer to an older move is dropped`() {
        val first = AgentMoveModel.started(MoveState())
        val second = AgentMoveModel.started(first)
        assertEquals(second, AgentMoveModel.answered(second, first.epoch, ok(MoveResult.CLOSED), now))
        // Nor is an answer read when nothing was asked.
        assertEquals(MoveState(), AgentMoveModel.answered(MoveState(), 0, ok(MoveResult.CLOSED), now))
    }

    @Test
    fun `a new move clears the last line, and the close takes a sticky one only`() {
        val open = move(ok(MoveResult.FAILED, MoveReason.OPEN))
        assertEquals(MoveNote.OPEN, open.note)
        assertNull(AgentMoveModel.started(open).note)
        assertNull(AgentMoveModel.dismissed(open).note)
        val busy = move(ok(MoveResult.REFUSED, MoveReason.BUSY))
        assertEquals(busy, AgentMoveModel.dismissed(busy))
        val notKnown = move(AgentCall.Failed(AgentFailure.TIMEOUT))
        assertNull(AgentMoveModel.dismissed(notKnown).note)
        // A new move also forgets an earlier closed.
        assertNull(AgentMoveModel.started(move(ok(MoveResult.CLOSED))).closedAtMs)
    }

    @Test
    fun `a state change clears a passing line and keeps a sticky one`() {
        val busy = move(ok(MoveResult.REFUSED, MoveReason.BUSY))
        assertNull(AgentMoveModel.sessionChanged(busy).note)
        val open = move(ok(MoveResult.FAILED, MoveReason.OPEN))
        assertEquals(open, AgentMoveModel.sessionChanged(open))
        val notKnown = move(AgentCall.Failed(AgentFailure.TIMEOUT))
        assertEquals(notKnown, AgentMoveModel.sessionChanged(notKnown))
        // The wait after closed isn't a line.
        val closed = move(ok(MoveResult.CLOSED))
        assertEquals(closed, AgentMoveModel.sessionChanged(closed))
    }

    @Test
    fun `the FFI's answer reads into the same words`() {
        for (reason in AgentMoveReason.entries) assertEquals(reason.name, AgentMoveFfi.reasonOf(reason).name)
        assertEquals(AgentMoveReason.entries.size, MoveReason.entries.size)
        val confirm = AgentMoveFfi.of(
            AgentMoveAnswer(
                result = AgentMoveResult.REFUSED,
                reason = AgentMoveReason.CONFIRM,
                target = AgentReplyTarget("Terminal", "Claude", "g7linux", AgentReplyKind.TERMINAL),
                nonce = nonce,
                retryMs = null,
                thread = null,
            ),
        )
        assertEquals(MoveAnswer(MoveResult.REFUSED, MoveReason.CONFIRM, terminal, nonce, null), confirm)
        for ((ffi, kotlin) in listOf(
            AgentMoveResult.MOVED to MoveResult.MOVED,
            AgentMoveResult.CLOSED to MoveResult.CLOSED,
            AgentMoveResult.FAILED to MoveResult.FAILED,
        )) {
            assertEquals(kotlin, AgentMoveFfi.of(AgentMoveAnswer(ffi, null, null, null, null, null)).result)
        }
        // A reason the core didn't know comes as none: "Couldn't move it".
        val unknown = AgentMoveFfi.of(AgentMoveAnswer(AgentMoveResult.FAILED, null, null, null, null, null))
        assertNull(unknown.reason)
        assertEquals(MoveNote.FAILED, move(AgentCall.Ok(unknown)).note)
    }

    @Test
    fun `the three needs read as their why`() {
        assertEquals(CantReply.SETUP, AgentMoveFfi.needsOf(AgentReplyNeeds.EXTENSION))
        assertEquals(CantReply.DESKTOP, AgentMoveFfi.needsOf(AgentReplyNeeds.DESKTOP))
        assertEquals(CantReply.KEYBOARD, AgentMoveFfi.needsOf(AgentReplyNeeds.KEYBOARD))
    }

    @Test
    fun `a PC that says why it can't type replies shows the quiet line for a terminal session`() {
        val working = session(state = AgentSessionState.WORKING)
        for (needs in listOf(CantReply.SETUP, CantReply.DESKTOP, CantReply.KEYBOARD)) {
            // Before the check, and once it said it doesn't offer replies.
            assertEquals(ReplyFooter.Cant(needs, null, null), AgentReplyModel.footer(ReplyState(), working, needs))
            assertEquals(ReplyFooter.Cant(needs, null, null), AgentReplyModel.footer(ReplyState(offer = ReplyOffer.NOT_OFFERED), session(), needs))
        }
        // Not for a session in the agent's app or elsewhere, nor an ended one.
        assertEquals(ReplyFooter.None, AgentReplyModel.footer(ReplyState(), session(source = AgentSessionSource.DESKTOP_APP), CantReply.DESKTOP))
        assertEquals(ReplyFooter.None, AgentReplyModel.footer(ReplyState(), session(source = AgentSessionSource.OTHER), CantReply.DESKTOP))
        assertEquals(ReplyFooter.None, AgentReplyModel.footer(ReplyState(), session(state = AgentSessionState.ENDED), CantReply.DESKTOP))
        // Without a word the screen is as before; and what the PC answered wins over its capabilities.
        assertEquals(ReplyFooter.None, AgentReplyModel.footer(ReplyState(offer = ReplyOffer.NOT_OFFERED), session()))
        assertTrue(AgentReplyModel.footer(ReplyState(offer = ReplyOffer.BOX), session(), CantReply.DESKTOP) is ReplyFooter.Box)
        assertEquals(ReplyFooter.None, AgentReplyModel.footer(ReplyState(offer = ReplyOffer.ENDED), session(), CantReply.DESKTOP))
    }
}
