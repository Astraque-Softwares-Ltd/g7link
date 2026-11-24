// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentItem
import app.g7link.core.AgentItemKind
import app.g7link.core.AgentReplyAnswer
import app.g7link.core.AgentReplyKind
import app.g7link.core.AgentReplyReason
import app.g7link.core.AgentReplyResult
import app.g7link.core.AgentReplyTarget
import app.g7link.core.AgentSession
import app.g7link.core.AgentSessionSource
import app.g7link.core.AgentSessionState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** "Reply on the PC": the footer's state table (plans/reply-on-pc.md §6.3, §6.4, §9.1). */
class AgentReplyModelTest {
    private val now = 1_795_260_720_000L
    private val terminal = ReplyTarget(app = "Terminal", agent = "Claude", folder = "g7linux", inApp = false)
    private val nonce = "0123456789abcdef0123456789abcdef"

    private fun session(
        state: AgentSessionState = AgentSessionState.WORKING,
        agent: String = "claude",
        source: AgentSessionSource = AgentSessionSource.TERMINAL,
    ) = AgentSession(
        id = "sc0123456789abcdef0123456789abcdef",
        agent = agent,
        folder = "g7linux",
        title = "Ongeza ukurasa wa malipo",
        state = state,
        guess = false,
        source = source,
        updatedMs = now,
        step = null,
        summary = null,
        waitingFor = null,
        canContinue = false,
        blocked = null,
        sinceMs = now - 60_000L,
        limit = null,
    )

    private fun user(seq: ULong, text: String) =
        AgentItem(seq = seq, id = "u$seq", kind = AgentItemKind.USER, whenMs = seq.toLong(), text = text, truncated = false, tool = null)

    private fun assistant(seq: ULong, text: String) =
        AgentItem(seq = seq, id = "a$seq", kind = AgentItemKind.ASSISTANT, whenMs = seq.toLong(), text = text, truncated = false, tool = null)

    private fun ok(result: ReplyResult, reason: ReplyReason? = null, target: ReplyTarget? = terminal, nonce: String? = null, retryMs: Long? = null) =
        AgentCall.Ok(ReplyAnswer(result, reason, target, nonce, retryMs))

    private fun refused(reason: ReplyReason, target: ReplyTarget? = null, retryMs: Long? = null) =
        ok(ReplyResult.REFUSED, reason, target, retryMs = retryMs)

    private fun failed(reason: ReplyReason?) = ok(ReplyResult.FAILED, reason, target = null)

    /** Checked ready, with a draft typed. */
    private fun ready(draft: String = "Run the tests again", s: AgentSession = session()): ReplyState {
        val checked = AgentReplyModel.checked(ReplyState(), 0, ok(ReplyResult.READY), s)
        return AgentReplyModel.typed(checked, draft)
    }

    /** Sent [state]'s draft and got [call] back. */
    private fun send(state: ReplyState, call: AgentCall<ReplyAnswer>, s: AgentSession = session(), lastSeq: ULong = 10u): ReplyState {
        val (text, _) = AgentReplyModel.outgoing(state, s) ?: error("nothing can be sent")
        return AgentReplyModel.replied(AgentReplyModel.sendStarted(state), call, text, lastSeq, s)
    }

    private fun box(state: ReplyState, s: AgentSession = session()): ReplyFooter.Box =
        AgentReplyModel.footer(state, s) as? ReplyFooter.Box ?: error("no box: ${AgentReplyModel.footer(state, s)}")

    private fun cant(state: ReplyState, s: AgentSession = session()): ReplyFooter.Cant =
        AgentReplyModel.footer(state, s) as? ReplyFooter.Cant ?: error("not the quiet line: ${AgentReplyModel.footer(state, s)}")

    /**
     * The screen's send (AgentSessionScreen's sendReply) on a clock that moves
     * only by the waits: [answer] gives the PC's answer to each try in turn;
     * Cancel is tapped in the wait after try number [cancelAfter]. The state
     * at the end, and how many tries went.
     */
    private fun sendWaiting(state: ReplyState, cancelAfter: Int? = null, answer: (Int) -> AgentCall<ReplyAnswer>): Pair<ReplyState, Int> {
        val (text, _) = AgentReplyModel.outgoing(state, session()) ?: error("nothing can be sent")
        var reply = AgentReplyModel.sendStarted(state)
        val epoch = reply.epoch
        var clock = now
        var tries = 1
        var call = answer(0)
        while (true) {
            val gap = AgentReplyModel.waitFor(reply, epoch, call, now, clock) ?: break
            reply = AgentReplyModel.waiting(reply, clock + gap)
            assertTrue(box(reply).waiting)
            if (tries == cancelAfter) reply = AgentReplyModel.waitCancelled(reply)
            clock += gap
            if (!AgentReplyModel.stillWaiting(reply, epoch)) return reply to tries
            reply = AgentReplyModel.resending(reply)
            call = answer(tries++)
        }
        return AgentReplyModel.replied(reply, call, text, 10u, session()) to tries
    }

    @Test
    fun `before any answer, and for an ended session, nothing is added`() {
        assertEquals(ReplyFooter.None, AgentReplyModel.footer(ReplyState(), session()))
        assertFalse(AgentReplyModel.replies(ReplyState(), session()))
        // Ended keeps today's Continue here, whatever was said before.
        assertEquals(ReplyFooter.None, AgentReplyModel.footer(ready(), session(AgentSessionState.ENDED)))
        assertFalse(AgentReplyModel.shouldCheck(ReplyState(), session(AgentSessionState.ENDED)))
        assertTrue(AgentReplyModel.shouldCheck(ReplyState(), session()))
        // The PC said it ended before this phone heard.
        val ended = AgentReplyModel.checked(ReplyState(), 0, refused(ReplyReason.ENDED), session())
        assertEquals(ReplyFooter.None, AgentReplyModel.footer(ended, session()))
    }

    @Test
    fun `a PC that doesn't offer replies leaves the screen exactly as it was`() {
        val state = AgentReplyModel.checked(ReplyState(), 0, AgentCall.Failed(AgentFailure.UNSUPPORTED), session())
        assertEquals(ReplyOffer.NOT_OFFERED, state.offer)
        assertEquals(ReplyFooter.None, AgentReplyModel.footer(state, session()))
        assertFalse(AgentReplyModel.shouldCheck(state, session()))
        // A session gone from the PC: nothing either.
        val gone = AgentReplyModel.checked(ready(), 0, AgentCall.Failed(AgentFailure.NOT_FOUND), session())
        assertEquals(ReplyFooter.None, AgentReplyModel.footer(gone, session()))
    }

    @Test
    fun `ready shows the box under its target line, and the header says so`() {
        val state = ready()
        val footer = box(state)
        assertEquals(terminal, footer.target)
        assertEquals("Terminal · Claude · g7linux", footer.target?.line)
        assertFalse(footer.locked)
        assertFalse(footer.sending)
        assertNull(footer.note)
        assertTrue(footer.canSend)
        assertTrue(AgentReplyModel.replies(state, session()))
        // In every state but ended, including while it works (O2: Claude queues it).
        for (s in listOf(AgentSessionState.IDLE, AgentSessionState.NEEDS_YOU, AgentSessionState.MAY_NEED_YOU)) {
            assertTrue(box(state, session(s)).canSend)
        }
        // A blank draft can't go.
        assertFalse(box(AgentReplyModel.typed(state, "  \n ")).canSend)
        assertNull(AgentReplyModel.outgoing(AgentReplyModel.typed(state, " "), session()))
    }

    @Test
    fun `a draft over the wire's limit can't go and says so`() {
        val state = ready("ü".repeat(4097))
        assertTrue(box(state).tooLong)
        assertFalse(box(state).canSend)
        assertNull(AgentReplyModel.outgoing(state, session()))
    }

    @Test
    fun `sending turns the button into a spinner and makes the field read only`() {
        val going = AgentReplyModel.sendStarted(ready())
        val footer = box(going)
        assertTrue(footer.sending)
        assertFalse(footer.canSend)
        assertNull(AgentReplyModel.outgoing(going, session()))
        assertEquals("Run the tests again", AgentReplyModel.typed(going, "changed").draft)
        assertFalse(AgentReplyModel.shouldCheck(going, session()))
    }

    @Test
    fun `sent clears the field and shows a pending row until its own row arrives`() {
        val sent = send(ready(), ok(ReplyResult.SENT), lastSeq = 10u)
        assertEquals("", sent.draft)
        assertNull(box(sent).note)
        assertFalse(box(sent).sending)
        // While it works it waits for the agent to pick it up; otherwise it was entered.
        assertEquals(PendingRow("Run the tests again", waiting = true), AgentReplyModel.pendingRow(sent, session(AgentSessionState.WORKING)))
        assertEquals(PendingRow("Run the tests again", waiting = false), AgentReplyModel.pendingRow(sent, session(AgentSessionState.IDLE)))
        // Its answer or an older user item doesn't settle it; the next user item does.
        val stillPending = AgentReplyModel.itemsSeen(sent, listOf(user(9u, "older"), assistant(11u, "On it")))
        assertEquals(sent.pending, stillPending.pending)
        val settled = AgentReplyModel.itemsSeen(stillPending, listOf(user(9u, "older"), assistant(11u, "On it"), user(12u, "Run the tests again")))
        assertNull(AgentReplyModel.pendingRow(settled, session()))
    }

    @Test
    fun `the first reply asks first, then goes with the PC's one-time word`() {
        val asked = send(ready(), ok(ReplyResult.REFUSED, ReplyReason.CONFIRM, terminal, nonce = nonce))
        assertEquals(ReplyConfirm(terminal, nonce, "Run the tests again"), asked.confirm)
        // The draft stays behind the sheet, and the box can't send over it.
        assertEquals("Run the tests again", asked.draft)
        assertFalse(box(asked).canSend)
        // Send on the sheet: the same text, with the nonce.
        assertEquals("Run the tests again" to nonce, AgentReplyModel.outgoing(asked, session()))
        val sent = AgentReplyModel.replied(AgentReplyModel.sendStarted(asked), ok(ReplyResult.SENT), "Run the tests again", 10u, session())
        assertNull(sent.confirm)
        assertEquals("", sent.draft)
        // Cancel: nothing typed, the draft kept, the box free again.
        val cancelled = AgentReplyModel.confirmCancelled(asked)
        assertNull(cancelled.confirm)
        assertEquals("Run the tests again", cancelled.draft)
        assertTrue(box(cancelled).canSend)
        // A check meanwhile doesn't drop the sheet.
        assertEquals(asked.confirm, AgentReplyModel.checked(asked, asked.epoch, ok(ReplyResult.READY), session()).confirm)
    }

    @Test
    fun `a confirmation without a proper nonce is not sent back`() {
        for (bad in listOf(null, "", "XYZ", nonce.uppercase(), nonce + "0")) {
            val state = send(ready(), ok(ReplyResult.REFUSED, ReplyReason.CONFIRM, terminal, nonce = bad))
            assertNull(state.confirm)
            assertEquals(ReplyNote.FAILED, box(state).note)
        }
    }

    @Test
    fun `a check that needs a confirmation still shows the box`() {
        val state = AgentReplyModel.checked(ReplyState(), 0, ok(ReplyResult.REFUSED, ReplyReason.CONFIRM, terminal, nonce = nonce), session())
        assertEquals(terminal, box(AgentReplyModel.typed(state, "hi")).target)
        assertTrue(box(AgentReplyModel.typed(state, "hi")).canSend)
        assertNull(state.confirm)
    }

    @Test
    fun `locked turns the box off with the lock and keeps the draft until it's unlocked`() {
        val locked = AgentReplyModel.checked(ready(), 0, refused(ReplyReason.LOCKED), session())
        val footer = box(locked)
        assertTrue(footer.locked)
        assertFalse(footer.canSend)
        assertEquals("Run the tests again", locked.draft)
        // The last target stays known.
        assertEquals(terminal, footer.target)
        // A send that finds it locked does the same.
        assertTrue(box(send(ready(), refused(ReplyReason.LOCKED))).locked)
        // Checked again once unlocked.
        assertTrue(box(AgentReplyModel.checked(locked, locked.epoch, ok(ReplyResult.READY), session())).canSend)
    }

    @Test
    fun `refused for now says why in the neutral colour, keeps the draft and the button`() {
        for ((reason, note) in listOf(ReplyReason.IN_USE to ReplyNote.IN_USE, ReplyReason.BUSY to ReplyNote.BUSY, ReplyReason.RATE to ReplyNote.RATE)) {
            val state = send(ready(), refused(reason, retryMs = 4_000L))
            val footer = box(state)
            assertEquals(note, footer.note)
            assertFalse(note.amber)
            assertTrue(footer.canSend)
            assertEquals("Run the tests again", state.draft)
            assertEquals(4_000L, state.retryMs)
        }
        // A wait past the wire's bound isn't trusted.
        assertNull(send(ready(), refused(ReplyReason.IN_USE, retryMs = 600_000L)).retryMs)
        // A check that finds the owner at the PC says the same; the next one that doesn't clears it.
        val inUse = AgentReplyModel.checked(ready(), 0, refused(ReplyReason.IN_USE), session())
        assertEquals(ReplyNote.IN_USE, box(inUse).note)
        assertNull(box(AgentReplyModel.checked(inUse, inUse.epoch, ok(ReplyResult.READY), session())).note)
        // A check asked too fast is about checks, not replies: nothing changes.
        assertEquals(inUse, AgentReplyModel.checked(inUse, inUse.epoch, refused(ReplyReason.RATE), session()))
    }

    @Test
    fun `refused until something is done on the PC says what in amber`() {
        val cases = listOf(
            ReplyReason.NEEDS_YOU to ReplyNote.NEEDS_YOU,
            ReplyReason.TAB to ReplyNote.TAB,
            ReplyReason.UNPROVEN to ReplyNote.UNPROVEN,
            ReplyReason.AMBIGUOUS to ReplyNote.AMBIGUOUS,
            ReplyReason.CLIPBOARD to ReplyNote.CLIPBOARD,
        )
        for ((reason, note) in cases) {
            val state = send(ready(), refused(reason))
            assertEquals(note, box(state).note)
            assertTrue(note.amber)
            assertTrue(box(state).canSend)
            assertEquals("Run the tests again", state.draft)
        }
    }

    @Test
    fun `a reply that starts with a command character is refused with the PC's reason`() {
        val state = send(ready("/compact"), refused(ReplyReason.TEXT))
        assertEquals(ReplyNote.TEXT, box(state).note)
        assertEquals("/compact", state.draft)
        // Editing it clears the line.
        assertNull(box(AgentReplyModel.typed(state, "compact the context")).note)
    }

    @Test
    fun `a window that didn't come forward offers Retry, since nothing was typed`() {
        val state = send(ready(), failed(ReplyReason.FOCUS))
        val footer = box(state)
        assertEquals(ReplyNote.FOCUS, footer.note)
        assertTrue(footer.canRetry)
        assertTrue(footer.canSend)
        assertEquals("Run the tests again", state.draft)
        assertEquals("Run the tests again" to null, AgentReplyModel.outgoing(state, session()))
    }

    @Test
    fun `typed but not sent stays until dismissed, with no Retry and no second paste`() {
        for (reason in listOf(ReplyReason.INTERRUPTED, ReplyReason.INPUT)) {
            val state = send(ready(), failed(reason))
            val footer = box(state)
            assertEquals(ReplyNote.INTERRUPTED, footer.note)
            assertTrue(ReplyNote.INTERRUPTED.amber)
            assertFalse(footer.canRetry)
            assertFalse(footer.canSend)
            assertTrue(footer.canDismiss)
            assertEquals("Run the tests again", state.draft)
            assertNull(AgentReplyModel.outgoing(state, session()))
            // Checks don't clear it, whatever they find; a lock keeps it in view.
            val checked = AgentReplyModel.checked(state, state.epoch, ok(ReplyResult.READY), session())
            assertEquals(ReplyNote.INTERRUPTED, box(checked).note)
            val locked = AgentReplyModel.checked(state, state.epoch, refused(ReplyReason.LOCKED), session())
            assertEquals(ReplyNote.INTERRUPTED, box(locked).note)
            val inUse = AgentReplyModel.checked(state, state.epoch, refused(ReplyReason.IN_USE), session())
            assertEquals(ReplyNote.INTERRUPTED, box(inUse).note)
            assertEquals(ReplyNote.INTERRUPTED, cant(AgentReplyModel.checked(state, state.epoch, refused(ReplyReason.NO_PROCESS), session())).note)
            // Dismissed: the box can send again.
            val dismissed = AgentReplyModel.dismissed(state)
            assertNull(box(dismissed).note)
            assertTrue(box(dismissed).canSend)
        }
    }

    @Test
    fun `a reply that never left this phone says the PC can't be reached`() {
        val state = send(ready(), AgentCall.Failed(AgentFailure.OFFLINE))
        assertEquals(ReplyNote.UNREACHABLE, box(state).note)
        assertTrue(box(state).canSend)
        assertEquals("Run the tests again", state.draft)
        assertNull(state.unsure)
    }

    @Test
    fun `an answer that never came is not known until a user item settles it`() {
        val state = send(ready(), AgentCall.Failed(AgentFailure.TIMEOUT), lastSeq = 10u)
        assertEquals(ReplyNote.NOT_KNOWN, box(state).note)
        assertEquals("Run the tests again", state.draft)
        assertNull(state.pending)
        // A check doesn't settle it.
        assertEquals(ReplyNote.NOT_KNOWN, box(AgentReplyModel.checked(state, state.epoch, ok(ReplyResult.READY), session())).note)
        // Its text arrived (wrapped and with the owner's half-written words before it): it was sent.
        val entered = AgentReplyModel.itemsSeen(state, listOf(user(11u, "also: Run the\ntests  again")))
        assertNull(box(entered).note)
        assertEquals("", entered.draft)
        // Something else arrived: settled, but the draft stays to send.
        val other = AgentReplyModel.itemsSeen(state, listOf(user(11u, "Something else")))
        assertNull(box(other).note)
        assertEquals("Run the tests again", other.draft)
        // An older item settles nothing.
        assertEquals(state, AgentReplyModel.itemsSeen(state, listOf(user(10u, "Run the tests again"))))
    }

    @Test
    fun `sessions the PC can't place get the quiet line with their reason`() {
        // The Claude app (spike S2 failed): reply in the app.
        val app = session(source = AgentSessionSource.DESKTOP_APP)
        assertEquals(CantReply.AGENT_APP, cant(AgentReplyModel.checked(ReplyState(), 0, refused(ReplyReason.PLACE), app), app).why)
        // Codex not opened with `codex resume` (spike S4 failed): its window can't be told.
        val codex = session(agent = "codex")
        assertEquals(CantReply.CODEX_WINDOW, cant(AgentReplyModel.checked(ReplyState(), 0, refused(ReplyReason.PLACE), codex), codex).why)
        // Found, in something G7 doesn't type into, named when the PC says.
        val vsCode = ReplyTarget("Visual Studio Code", "Codex", "g7linux", inApp = false)
        val named = cant(AgentReplyModel.checked(ReplyState(), 0, refused(ReplyReason.PLACE, target = vsCode), codex), codex)
        assertEquals(CantReply.TERMINAL, named.why)
        assertEquals("Visual Studio Code", named.app)
        val unnamed = cant(AgentReplyModel.checked(ReplyState(), 0, refused(ReplyReason.PLACE), session()))
        assertEquals(CantReply.TERMINAL, unnamed.why)
        assertNull(unnamed.app)
        // No window, no process, no companion.
        assertEquals(CantReply.NO_WINDOW, cant(AgentReplyModel.checked(ReplyState(), 0, refused(ReplyReason.NO_WINDOW), session())).why)
        assertEquals(CantReply.NO_WINDOW, cant(AgentReplyModel.checked(ReplyState(), 0, refused(ReplyReason.NO_PROCESS), session())).why)
        assertEquals(CantReply.SETUP, cant(AgentReplyModel.checked(ReplyState(), 0, refused(ReplyReason.SETUP), session())).why)
        // Off for this phone on the PC.
        val off = AgentReplyModel.checked(ReplyState(), 0, AgentCall.Failed(AgentFailure.DENIED), session())
        assertEquals(CantReply.OFF, cant(off).why)
        assertFalse(AgentReplyModel.replies(off, session()))
        // A send that finds it can't: the quiet line, the draft kept for when it can.
        val sent = send(ready(), refused(ReplyReason.NO_WINDOW))
        assertEquals(CantReply.NO_WINDOW, cant(sent).why)
        assertEquals("Run the tests again", sent.draft)
        assertEquals(CantReply.OFF, cant(send(ready(), AgentCall.Failed(AgentFailure.DENIED))).why)
        // Once it can, the box is back.
        val back = AgentReplyModel.checked(sent, sent.epoch, ok(ReplyResult.READY), session())
        assertEquals("Run the tests again", back.draft)
        assertTrue(box(back).canSend)
    }

    @Test
    fun `words this phone doesn't know read as a plain couldn't reply`() {
        assertEquals(ReplyResult.FAILED, ReplyResult.of("queued"))
        assertEquals(ReplyResult.FAILED, ReplyResult.of(null))
        assertEquals(ReplyResult.SENT, ReplyResult.of("sent"))
        assertEquals(ReplyReason.OTHER, ReplyReason.of("moon_phase"))
        assertEquals(ReplyReason.NO_PROCESS, ReplyReason.of("no_process"))
        assertEquals(ReplyReason.UNPROVEN, ReplyReason.of("unproven"))
        assertNull(ReplyReason.of(null))
        assertNull(ReplyReason.of(""))
        assertEquals(ReplyNote.FAILED, box(send(ready(), refused(ReplyReason.OTHER))).note)
        assertEquals(ReplyNote.FAILED, box(send(ready(), failed(ReplyReason.OTHER))).note)
        assertEquals(ReplyNote.FAILED, box(send(ready(), failed(null))).note)
        // "ready" isn't an answer to a reply.
        assertEquals(ReplyNote.FAILED, box(send(ready(), ok(ReplyResult.READY))).note)
        // The PC's BUSY or BAD_MESSAGE.
        val other = send(ready(), AgentCall.Failed(AgentFailure.OTHER))
        assertEquals(ReplyNote.FAILED, box(other).note)
        assertEquals("Run the tests again", other.draft)
        // An unknown reason on a check: the box, saying it couldn't.
        assertEquals(ReplyNote.FAILED, box(AgentReplyModel.checked(ReplyState(), 0, refused(ReplyReason.OTHER), session())).note)
    }

    @Test
    fun `a check answered after a reply started or ended is dropped`() {
        val state = ready()
        val asked = state.epoch
        val going = AgentReplyModel.sendStarted(state)
        assertEquals(going, AgentReplyModel.checked(going, asked, refused(ReplyReason.LOCKED), session()))
        val sent = AgentReplyModel.replied(going, ok(ReplyResult.SENT), "Run the tests again", 10u, session())
        assertEquals(sent, AgentReplyModel.checked(sent, asked, refused(ReplyReason.LOCKED), session()))
        // One asked after it applies.
        assertTrue(box(AgentReplyModel.checked(sent, sent.epoch, refused(ReplyReason.LOCKED), session())).locked)
    }

    @Test
    fun `a check that isn't answered keeps what was said last`() {
        val state = ready()
        for (failure in listOf(AgentFailure.OFFLINE, AgentFailure.TIMEOUT, AgentFailure.OTHER)) {
            assertEquals(state, AgentReplyModel.checked(state, state.epoch, AgentCall.Failed(failure), session()))
        }
        // A check never enters anything, so "failed" from one changes nothing.
        assertEquals(state, AgentReplyModel.checked(state, state.epoch, failed(ReplyReason.FOCUS), session()))
        assertEquals(ReplyFooter.None, AgentReplyModel.footer(AgentReplyModel.checked(ReplyState(), 0, AgentCall.Failed(AgentFailure.TIMEOUT), session()), session()))
    }

    @Test
    fun `the FFI's answer reads into the same words`() {
        for (reason in AgentReplyReason.entries) assertEquals(reason.name, AgentReplyFfi.reasonOf(reason).name)
        assertEquals(ReplyReason.UNPROVEN, AgentReplyFfi.reasonOf(AgentReplyReason.UNPROVEN))
        val confirm = AgentReplyFfi.of(
            AgentReplyAnswer(
                result = AgentReplyResult.REFUSED,
                reason = AgentReplyReason.CONFIRM,
                target = AgentReplyTarget("Terminal", "Claude", "g7linux", AgentReplyKind.TERMINAL),
                nonce = nonce,
                retryMs = null,
            ),
        )
        assertEquals(ReplyAnswer(ReplyResult.REFUSED, ReplyReason.CONFIRM, terminal, nonce, null), confirm)
        val busy = AgentReplyFfi.of(AgentReplyAnswer(AgentReplyResult.REFUSED, AgentReplyReason.IN_USE, null, null, 4_000u))
        assertEquals(4_000L, busy.retryMs)
        // A reason the core didn't know comes as none: "Couldn't reply".
        val unknown = AgentReplyFfi.of(AgentReplyAnswer(AgentReplyResult.FAILED, null, null, null, null))
        assertNull(unknown.reason)
        assertEquals(ReplyNote.FAILED, box(send(ready(), AgentCall.Ok(unknown))).note)
        // The agent's app, and a kind this build doesn't know (a plain terminal icon).
        assertTrue(AgentReplyFfi.targetOf(AgentReplyTarget("Claude", "Claude", "g7linux", AgentReplyKind.APP)).inApp)
        assertFalse(AgentReplyFfi.targetOf(AgentReplyTarget("Terminal", "Codex", "g7linux", null)).inApp)
    }

    @Test
    fun `G7 couldn't make sure of the terminal, so it says so in amber, nothing typed, the draft kept`() {
        val state = send(ready(), refused(ReplyReason.UNPROVEN))
        val footer = box(state)
        assertEquals(ReplyNote.UNPROVEN, footer.note)
        assertTrue(ReplyNote.UNPROVEN.amber)
        assertFalse(ReplyNote.UNPROVEN.sticky)
        assertTrue(footer.canSend)
        assertFalse(footer.canRetry)
        assertEquals("Run the tests again", state.draft)
        assertNull(state.pending)
        // Not a reason to wait: it goes once.
        val going = AgentReplyModel.sendStarted(ready())
        assertNull(AgentReplyModel.waitFor(going, going.epoch, refused(ReplyReason.UNPROVEN, retryMs = 4_000L), now, now))
    }

    @Test
    fun `in use with the PC's wait is sent again after it, within the minute`() {
        val going = AgentReplyModel.sendStarted(ready())
        val epoch = going.epoch
        val inUse = refused(ReplyReason.IN_USE, retryMs = 4_000L)
        assertEquals(4_000L, AgentReplyModel.waitFor(going, epoch, inUse, now, now + 100L))
        // Never sooner than the PC looks at attempts.
        assertEquals(AgentReplyModel.WAIT_GAP_MS, AgentReplyModel.waitFor(going, epoch, refused(ReplyReason.IN_USE, retryMs = 250L), now, now))
        // No wait from the PC (an older one, or it can't tell): waiting won't help.
        assertNull(AgentReplyModel.waitFor(going, epoch, refused(ReplyReason.IN_USE), now, now))
        // A wait outside the wire's bounds isn't trusted.
        for (bad in listOf(0L, -1L, AgentReplyModel.WAIT_MS + 1)) {
            assertNull(AgentReplyModel.waitFor(going, epoch, refused(ReplyReason.IN_USE, retryMs = bad), now, now))
        }
        // Only while the next try still goes inside the minute.
        assertEquals(4_000L, AgentReplyModel.waitFor(going, epoch, inUse, now, now + 55_999L))
        assertNull(AgentReplyModel.waitFor(going, epoch, inUse, now, now + 56_000L))
        // Anything else is answered as before, and an answer that never came is never sent again.
        val others = listOf(
            ok(ReplyResult.SENT),
            ok(ReplyResult.READY, retryMs = 4_000L),
            refused(ReplyReason.LOCKED, retryMs = 4_000L),
            refused(ReplyReason.RATE, retryMs = 4_000L),
            refused(ReplyReason.BUSY, retryMs = 4_000L),
            ok(ReplyResult.FAILED, ReplyReason.IN_USE, retryMs = 4_000L),
            failed(ReplyReason.FOCUS),
            AgentCall.Failed(AgentFailure.TIMEOUT),
            AgentCall.Failed(AgentFailure.OFFLINE),
        )
        for (call in others) assertNull(AgentReplyModel.waitFor(going, epoch, call, now, now))
        // Not for a reply that isn't going, or not this one.
        assertNull(AgentReplyModel.waitFor(ready(), ready().epoch, inUse, now, now))
        assertNull(AgentReplyModel.waitFor(going, epoch - 1, inUse, now, now))
    }

    @Test
    fun `a waiting reply says so with Cancel, the field read only and checks held`() {
        val going = AgentReplyModel.sendStarted(ready())
        assertFalse(box(going).waiting)
        val waiting = AgentReplyModel.waiting(going, now + 4_000L)
        assertEquals(ReplyWait(now + 4_000L), waiting.wait)
        val footer = box(waiting)
        assertTrue(footer.waiting)
        assertTrue(footer.sending)
        assertFalse(footer.canSend)
        assertNull(footer.note)
        assertNull(AgentReplyModel.outgoing(waiting, session()))
        assertEquals("Run the tests again", AgentReplyModel.typed(waiting, "changed").draft)
        assertFalse(AgentReplyModel.shouldCheck(waiting, session()))
        assertTrue(AgentReplyModel.stillWaiting(waiting, going.epoch))
        // A check answered meanwhile is dropped.
        assertEquals(waiting, AgentReplyModel.checked(waiting, going.epoch, ok(ReplyResult.READY), session()))
        // Going again, it still reads as waiting.
        val again = AgentReplyModel.resending(waiting)
        assertTrue(box(again).waiting)
        assertTrue(AgentReplyModel.stillWaiting(again, going.epoch))
    }

    @Test
    fun `a reply that waits goes by itself once the PC is free`() {
        val (state, tries) = sendWaiting(ready()) { n -> if (n < 2) refused(ReplyReason.IN_USE, retryMs = 2_300L) else ok(ReplyResult.SENT) }
        assertEquals(3, tries)
        assertNull(state.wait)
        assertFalse(box(state).sending)
        assertFalse(box(state).waiting)
        assertNull(box(state).note)
        assertEquals("", state.draft)
        assertEquals(PendingReply("Run the tests again", 10u), state.pending)
        // The first reply to a window still asks first, before any waiting.
        val (asked, askedTries) = sendWaiting(ready()) { ok(ReplyResult.REFUSED, ReplyReason.CONFIRM, terminal, nonce = nonce) }
        assertEquals(1, askedTries)
        assertEquals(ReplyConfirm(terminal, nonce, "Run the tests again"), asked.confirm)
    }

    @Test
    fun `still in use after a minute gives up, the text kept in the box`() {
        val (state, tries) = sendWaiting(ready()) { refused(ReplyReason.IN_USE, retryMs = 4_000L) }
        // Tried at 0, 4 s, … 56 s: the next would be past the minute.
        assertEquals(15, tries)
        val footer = box(state)
        assertEquals(ReplyNote.GAVE_UP, footer.note)
        assertFalse(ReplyNote.GAVE_UP.amber)
        assertFalse(footer.sending)
        assertFalse(footer.waiting)
        assertTrue(footer.canSend)
        assertEquals("Run the tests again", state.draft)
        assertNull(state.wait)
        // Said once: no check is asked for straight after it.
        assertNull(state.retryMs)
        // Never more than a try per gap.
        assertEquals(50, sendWaiting(ready()) { refused(ReplyReason.IN_USE, retryMs = 1L) }.second)
        // Without waiting first, or once the PC no longer says waiting helps: today's line.
        assertEquals(ReplyNote.IN_USE, box(send(ready(), refused(ReplyReason.IN_USE))).note)
        val (stopped, _) = sendWaiting(ready()) { n -> if (n == 0) refused(ReplyReason.IN_USE, retryMs = 4_000L) else refused(ReplyReason.IN_USE) }
        assertEquals(ReplyNote.IN_USE, box(stopped).note)
        // Something else after a wait: its own line.
        val (tab, _) = sendWaiting(ready()) { n -> if (n == 0) refused(ReplyReason.IN_USE, retryMs = 4_000L) else refused(ReplyReason.TAB) }
        assertEquals(ReplyNote.TAB, box(tab).note)
        val (locked, _) = sendWaiting(ready()) { n -> if (n == 0) refused(ReplyReason.IN_USE, retryMs = 4_000L) else refused(ReplyReason.LOCKED) }
        assertTrue(box(locked).locked)
    }

    @Test
    fun `Cancel ends the waiting at once, the draft kept, no line and nothing more sent`() {
        val going = AgentReplyModel.sendStarted(ready())
        val epoch = going.epoch
        val cancelled = AgentReplyModel.waitCancelled(AgentReplyModel.waiting(going, now + 4_000L))
        assertNull(cancelled.wait)
        assertFalse(cancelled.sending)
        assertFalse(AgentReplyModel.stillWaiting(cancelled, epoch))
        val footer = box(cancelled)
        assertFalse(footer.waiting)
        assertNull(footer.note)
        assertTrue(footer.canSend)
        assertEquals("Run the tests again", cancelled.draft)
        assertTrue(AgentReplyModel.shouldCheck(cancelled, session()))
        assertNull(AgentReplyModel.waitFor(cancelled, epoch, refused(ReplyReason.IN_USE, retryMs = 4_000L), now, now))
        // A reply sent after it waits on its own; the cancelled one doesn't take it for its wait.
        val newer = AgentReplyModel.waiting(AgentReplyModel.sendStarted(cancelled), now + 8_000L)
        assertFalse(AgentReplyModel.stillWaiting(newer, epoch))
        assertNull(AgentReplyModel.waitFor(newer, epoch, refused(ReplyReason.IN_USE, retryMs = 4_000L), now, now))
        // Nothing waiting: Cancel changes nothing.
        assertEquals(going, AgentReplyModel.waitCancelled(going))
        assertEquals(ready(), AgentReplyModel.waitCancelled(ready()))
        // In the screen's loop: no try after the Cancel, and the box is free with its draft.
        val (end, tries) = sendWaiting(ready(), cancelAfter = 3) { refused(ReplyReason.IN_USE, retryMs = 4_000L) }
        assertEquals(3, tries)
        assertNull(end.wait)
        assertNull(box(end).note)
        assertTrue(box(end).canSend)
        assertEquals("Run the tests again", end.draft)
    }

    @Test
    fun `Cancel while a try is on its way still reads what it answers`() {
        val going = AgentReplyModel.sendStarted(ready())
        val epoch = going.epoch
        val asking = AgentReplyModel.resending(AgentReplyModel.waiting(going, now + 4_000L))
        val cancelled = AgentReplyModel.waitCancelled(asking)
        assertNull(cancelled.wait)
        // Busy until it answers, so no second send can go over it.
        assertTrue(box(cancelled).sending)
        assertFalse(box(cancelled).waiting)
        assertFalse(box(cancelled).canSend)
        assertNull(AgentReplyModel.outgoing(cancelled, session()))
        assertFalse(AgentReplyModel.stillWaiting(cancelled, epoch))
        // It went in: shown as sent.
        val sent = AgentReplyModel.replied(cancelled, ok(ReplyResult.SENT), "Run the tests again", 10u, session())
        assertEquals("", sent.draft)
        assertEquals(PendingReply("Run the tests again", 10u), sent.pending)
        // Still in use: not sent again, and today's line rather than "gave up".
        val inUse = refused(ReplyReason.IN_USE, retryMs = 4_000L)
        assertNull(AgentReplyModel.waitFor(cancelled, epoch, inUse, now, now))
        val notSent = AgentReplyModel.replied(cancelled, inUse, "Run the tests again", 10u, session())
        assertEquals(ReplyNote.IN_USE, box(notSent).note)
        assertTrue(box(notSent).canSend)
        assertEquals("Run the tests again", notSent.draft)
    }

    @Test
    fun `a check's in use, from an older PC, is still a line and never a wait`() {
        val state = AgentReplyModel.checked(ready(), 0, refused(ReplyReason.IN_USE, retryMs = 3_000L), session())
        assertEquals(ReplyNote.IN_USE, box(state).note)
        assertEquals(3_000L, state.retryMs)
        assertNull(state.wait)
        assertFalse(box(state).waiting)
        assertFalse(box(state).sending)
        assertTrue(box(state).canSend)
    }
}
