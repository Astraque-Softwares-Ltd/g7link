// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentAsk
import app.g7link.core.AgentAskKind
import app.g7link.core.AgentCheckpoint
import app.g7link.core.AgentItem
import app.g7link.core.AgentItemKind
import app.g7link.core.AgentPane
import app.g7link.core.AgentPlanState
import app.g7link.core.AgentPlanStatus
import app.g7link.core.AgentPlanStep
import app.g7link.core.AgentTool
import app.g7link.core.AgentToolClass
import app.g7link.core.AgentToolStatus
import app.g7link.core.AgentTurn
import app.g7link.core.Event
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/** The phone beside the PC (contract §V9.1): it only changes what this phone shows. */
class AgentBesideTest {
    private val now = 1_000_000L

    private fun presence(atPc: Boolean = true, locked: Boolean = false, viewing: Set<String> = setOf("t1"), untilMs: Long = now + 60_000L) =
        PcPresence(atPc = atPc, locked = locked, dnd = false, viewing = viewing, untilMs = untilMs)

    private fun ask(token: String, expiresMs: Long) = AgentAsk(
        token = token,
        thread = "t1",
        threadTitle = "Fix M-Pesa callback retries",
        agent = "claude",
        kind = AgentAskKind.COMMAND,
        title = "Run command",
        command = "cargo test callbacks::retry",
        diff = null,
        path = null,
        reason = null,
        truncated = false,
        expiresMs = expiresMs,
    )

    private fun question(token: String, expiresMs: Long) = QuestionInfo(
        token = token,
        threadId = "t1",
        agent = "claude",
        itemId = "i$token",
        secret = false,
        asks = listOf(QuestionAsk(id = "0", text = "Which queue?")),
        createdMs = expiresMs - 7_200_000L,
        expiresMs = expiresMs,
    )

    private fun edit(seq: ULong, turn: UInt, path: String, status: AgentToolStatus = AgentToolStatus.OK, detail: String? = "--- a/$path\n+++ b/$path\n@@ -1 +1 @@\n-a\n+b\n") = AgentItem(
        seq = seq,
        id = "i$seq",
        kind = AgentItemKind.TOOL,
        whenMs = seq.toLong(),
        text = null,
        truncated = false,
        tool = AgentTool(name = "Edit", title = "Edit $path", detail = detail, status = status, output = null, toolClass = AgentToolClass.EDIT, path = path),
        turn = turn,
    )

    private fun item(seq: ULong, kind: AgentItemKind, turn: UInt = 1u, planState: AgentPlanState? = null) =
        AgentItem(seq = seq, id = "i$seq", kind = kind, whenMs = seq.toLong(), text = "text $seq", truncated = false, tool = null, turn = turn, planState = planState)

    private fun turn(n: UInt, checkpoint: AgentCheckpoint?, files: UInt = 2u, ended: Boolean = true) = AgentTurn(
        turn = n,
        promptSeq = n.toULong(),
        lastSeq = null,
        startedMs = 0L,
        endedMs = if (ended) 10L else null,
        ended = null,
        checkpoint = checkpoint,
        why = null,
        files = files,
        adds = 3u,
        dels = 1u,
        moreFiles = false,
        skipped = 0u,
        canFork = false,
        canRewind = false,
    )

    @Test
    fun `Show beside is offered while someone at the PC has the thread on screen`() {
        assertTrue(AgentBeside.offered(presence(), "t1", now))
        assertFalse(AgentBeside.offered(presence(), "t2", now))
        assertFalse("nobody at the PC", AgentBeside.offered(presence(atPc = false), "t1", now))
        assertFalse("locked", AgentBeside.offered(presence(locked = true), "t1", now))
        assertFalse("stale", AgentBeside.offered(presence(untilMs = now), "t1", now))
        assertFalse("never said", AgentBeside.offered(null, "t1", now))
    }

    @Test
    fun `it leaves once the PC hasn't shown the thread for more than 60 s`() {
        // Shown: no clock runs.
        assertNull(AgentBeside.stoppedAt(null, shown = true, nowMs = now))
        // Stopped showing: the clock starts then, and keeps its start while it stays stopped.
        val stopped = AgentBeside.stoppedAt(null, shown = false, nowMs = now)
        assertEquals(now, stopped)
        assertEquals(now, AgentBeside.stoppedAt(stopped, shown = false, nowMs = now + 30_000L))
        assertEquals(now + 60_000L, AgentBeside.leaveAtMs(stopped))
        assertFalse(AgentBeside.leaves(stopped, now + 60_000L))
        assertTrue(AgentBeside.leaves(stopped, now + 60_001L))
        // Back on the PC's screen within the minute: it stays, and a later stop starts over.
        assertNull(AgentBeside.stoppedAt(stopped, shown = true, nowMs = now + 59_000L))
        assertNull(AgentBeside.leaveAtMs(null))
        assertFalse(AgentBeside.leaves(null, now + 600_000L))
        // A presence that lapsed counts as not shown.
        assertFalse(AgentBeside.shownOnPc(presence(untilMs = now), "t1", now))
        assertFalse(AgentBeside.shownOnPc(null, "t1", now))
        // Someone stepping away doesn't take the thread off the PC's screen.
        assertTrue(AgentBeside.shownOnPc(presence(atPc = false), "t1", now))
    }

    @Test
    fun `the screen stays awake only while a turn runs, at most 10 minutes`() {
        assertFalse(AgentBeside.keepsAwake(null, now))
        assertNull(AgentBeside.awakeUntilMs(null))
        assertTrue(AgentBeside.keepsAwake(now, now))
        assertTrue(AgentBeside.keepsAwake(now, now + 599_999L))
        assertFalse(AgentBeside.keepsAwake(now, now + 600_000L))
        assertEquals(now + 600_000L, AgentBeside.awakeUntilMs(now))
    }

    @Test
    fun `the pane switches both ways`() {
        // This phone to the PC: each pane in its wire word, the thread when it leaves.
        assertEquals(AgentPane.CHANGES, AgentBeside.wire(BesidePane.CHANGES))
        assertEquals(AgentPane.PLAN, AgentBeside.wire(BesidePane.PLAN))
        assertEquals(AgentPane.REQUESTS, AgentBeside.wire(BesidePane.REQUESTS))
        assertEquals(AgentPane.THREAD, AgentBeside.wire(null))
        // The PC to this phone: followed only beside that thread.
        assertEquals(BesidePane.PLAN, AgentBeside.follow(BesidePane.CHANGES, "t1", "t1", AgentPane.PLAN))
        assertEquals(BesidePane.REQUESTS, AgentBeside.follow(BesidePane.PLAN, "t1", "t1", AgentPane.REQUESTS))
        assertEquals(BesidePane.CHANGES, AgentBeside.follow(BesidePane.REQUESTS, "t1", "t1", AgentPane.CHANGES))
        assertNull("the thread itself takes it back", AgentBeside.follow(BesidePane.PLAN, "t1", "t1", AgentPane.THREAD))
        assertEquals("another thread's", BesidePane.PLAN, AgentBeside.follow(BesidePane.PLAN, "t1", "t2", AgentPane.CHANGES))
        assertNull("the PC never opens beside mode", AgentBeside.follow(null, "t1", "t1", AgentPane.CHANGES))
        // It opens on what waits, else the changes.
        assertEquals(BesidePane.REQUESTS, AgentBeside.firstPane(waiting = true))
        assertEquals(BesidePane.CHANGES, AgentBeside.firstPane(waiting = false))
    }

    @Test
    fun `the PC's switch is that PC's agent event and changes no thread`() {
        val event = Event.AgentBeside("pc1", "t1", AgentPane.PLAN)
        assertEquals("pc1", AgentsModel.pcOf(event))
        val state = AgentsState(asks = mapOf("a1" to ask("a1", 5_000L)))
        val update = AgentsModel.reduce(state, event)
        assertSame(state, update.state)
        assertNull(update.reread)
    }

    @Test
    fun `every waiting request and question, oldest first, and only the first has buttons`() {
        val a1 = ask("a1", expiresMs = 5_000L)
        val a2 = ask("a2", expiresMs = 9_000L)
        val q1 = question("q1", expiresMs = 7_000L)
        val listed = AgentBeside.requests(listOf(a2, a1), listOf(q1))
        assertEquals(listOf("a1", "q1", "a2"), listed.map { it.token })
        // The dock shows the oldest: it alone has buttons.
        val state = AgentsState(asks = mapOf("a1" to a1, "a2" to a2), questions = mapOf("q1" to q1))
        val dock = state.dockShown("t1")
        assertEquals("a1", AgentBeside.withButtons(listed, dock))
        // Answered: the next one has them.
        val next = AgentsState(asks = mapOf("a2" to a2), questions = mapOf("q1" to q1))
        val nextListed = AgentBeside.requests(next.asksFor("t1"), next.questionsFor("t1"))
        assertEquals(listOf("q1", "a2"), nextListed.map { it.token })
        assertEquals("q1", AgentBeside.withButtons(nextListed, next.dockShown("t1")))
        // A request and a question asked together: the request first, as the dock has it.
        val tie = AgentBeside.requests(listOf(ask("a3", 7_000L)), listOf(question("q3", 7_000L)))
        assertEquals(listOf("a3", "q3"), tie.map { it.token })
        // Nothing waits: none.
        assertNull(AgentBeside.withButtons(emptyList(), null))
        // The dock's isn't among them (another thread's): none.
        assertNull(AgentBeside.withButtons(listOf(BesideRequest.Ask(a2)), dock))
    }

    @Test
    fun `while a turn runs, Changes lists its edits so far, newest first`() {
        val items = listOf(
            item(1uL, AgentItemKind.USER, turn = 1u),
            edit(2uL, 1u, "src/old.rs"),
            item(3uL, AgentItemKind.USER, turn = 2u),
            edit(4uL, 2u, "src/mpesa/callback.rs"),
            edit(5uL, 2u, "src/mpesa/denied.rs", status = AgentToolStatus.DENIED),
            edit(6uL, 2u, "src/mpesa/running.rs", status = AgentToolStatus.RUNNING),
            edit(7uL, 2u, "src/mpesa/empty.rs", detail = ""),
            edit(8uL, 2u, "src/mpesa/retry.rs"),
        )
        val shown = AgentBeside.changes(running = true, items = items, turns = listOf(turn(1u, AgentCheckpoint.READY), turn(2u, null, ended = false)))
        assertEquals(listOf("i8", "i4"), (shown as BesideChanges.Edits).items.map { it.id })
        // None yet.
        assertEquals(BesideChanges.Empty, AgentBeside.changes(running = true, items = items.take(3), turns = emptyList()))
        // Items from before turns were kept: what follows the last message.
        val old = items.map { it.copy(turn = 0u) }
        assertEquals(listOf("i8", "i4"), (AgentBeside.changes(true, old, emptyList()) as BesideChanges.Edits).items.map { it.id })
        // At most the newest 30.
        val many = (1..40).map { edit(it.toULong(), 1u, "f$it.rs") }
        val capped = (AgentBeside.changes(true, many, emptyList()) as BesideChanges.Edits).items
        assertEquals(AgentBeside.MAX_EDITS, capped.size)
        assertEquals("i40", capped.first().id)
    }

    @Test
    fun `after the turn, Changes is what the PC recorded for it`() {
        val items = listOf(item(1uL, AgentItemKind.USER), edit(2uL, 1u, "src/a.rs"))
        assertEquals(BesideChanges.Files(2u), AgentBeside.changes(false, items, listOf(turn(1u, AgentCheckpoint.READY), turn(2u, AgentCheckpoint.READY))))
        assertEquals(BesideChanges.Recording, AgentBeside.changes(false, items, listOf(turn(1u, AgentCheckpoint.PENDING))))
        assertEquals(BesideChanges.Empty, AgentBeside.changes(false, items, listOf(turn(1u, AgentCheckpoint.READY, files = 0u))))
        // No record (not a repository, or none kept): its edits instead.
        for (checkpoint in listOf(AgentCheckpoint.NOT_GIT, AgentCheckpoint.MISSING, AgentCheckpoint.EXPIRED, null)) {
            assertEquals(listOf("i2"), (AgentBeside.changes(false, items, listOf(turn(1u, checkpoint))) as BesideChanges.Edits).items.map { it.id })
        }
        assertEquals(BesideChanges.Empty, AgentBeside.changes(false, emptyList(), emptyList()))
    }

    @Test
    fun `Plan shows a plan waiting first, then the task list with its step marked, then the newest plan`() {
        val steps = listOf(
            AgentPlanStep(text = "Read how callbacks are retried today", status = AgentPlanStatus.COMPLETED),
            AgentPlanStep(text = "Retry three times, then dead-letter", status = AgentPlanStatus.IN_PROGRESS),
            AgentPlanStep(text = "Tests for the three outcomes", status = AgentPlanStatus.PENDING),
        )
        assertEquals(BesidePlan.Tasks(steps, 2), AgentBeside.plan(steps, emptyList()))
        val done = steps.map { it.copy(status = AgentPlanStatus.COMPLETED) }
        assertEquals(BesidePlan.Tasks(done, null), AgentBeside.plan(done, emptyList()))
        val proposed = listOf(item(4uL, AgentItemKind.PLAN, planState = AgentPlanState.PROPOSED))
        assertEquals(BesidePlan.Card("i4"), AgentBeside.plan(steps, proposed))
        val implemented = listOf(item(4uL, AgentItemKind.PLAN, planState = AgentPlanState.IMPLEMENTED))
        assertEquals(BesidePlan.Tasks(steps, 2), AgentBeside.plan(steps, implemented))
        assertEquals(BesidePlan.Card("i4"), AgentBeside.plan(null, implemented))
        assertEquals(BesidePlan.None, AgentBeside.plan(emptyList(), listOf(item(1uL, AgentItemKind.USER))))
    }
}
