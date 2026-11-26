// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.notifications

import app.g7link.android.ui.agents.AgentsModel
import app.g7link.android.ui.agents.AgentsState
import app.g7link.android.ui.agents.PcPresence
import app.g7link.core.AgentAsk
import app.g7link.core.AgentAskKind
import app.g7link.core.AgentCatalog
import app.g7link.core.AgentInfo
import app.g7link.core.AgentMode
import app.g7link.core.AgentOutcome
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState
import app.g7link.core.Event
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Do Not Disturb (contract §V8.6) and Stop speaking (§V8.2) on a PC's agent notices. */
class AgentDndTest {
    private val pc = "pc1"

    private val catalog = AgentCatalog(
        agents = listOf(AgentInfo(id = "claude", name = "Claude", installed = true, signedIn = true, loginHint = "claude login")),
        folders = emptyList(),
    )

    private fun thread(state: AgentThreadState) = AgentThread(
        id = "t1",
        agent = "claude",
        folder = "duka-pos",
        title = "Stock sync drops rows on reconnect",
        state = state,
        mode = AgentMode.ASK,
        fromPhone = true,
        updatedMs = 1_000L,
        asks = 0u,
        summary = "A row now leaves the queue only after the server acknowledges it.",
        error = null,
    )

    private val ask = AgentAsk(
        token = "a1",
        thread = "t1",
        threadTitle = "Stock sync drops rows on reconnect",
        agent = "claude",
        kind = AgentAskKind.COMMAND,
        title = "Run command",
        command = "cargo test sync::",
        diff = null,
        path = null,
        reason = null,
        truncated = false,
        expiresMs = 10_000L,
    )

    private fun state(thread: AgentThread): AgentsState = AgentsModel.withThreads(AgentsState(catalog = catalog), listOf(thread))

    private fun waiting(): AgentNotice.Waiting {
        val before = state(thread(AgentThreadState.WAITING))
        val event = Event.AgentAsk(pc, ask)
        val after = AgentsModel.reduce(before, event).state
        return AgentNotifications.plan(pc, event, after, shown = null) { false }.single() as AgentNotice.Waiting
    }

    private fun finished(): AgentNotice.Finished {
        val done = thread(AgentThreadState.DONE)
        val event = Event.AgentDone(pc, done, AgentOutcome.DONE, "All set.")
        val after = AgentsModel.reduce(state(thread(AgentThreadState.WORKING)), event).state
        return AgentNotifications.plan(pc, event, after, shown = null) { true }.single() as AgentNotice.Finished
    }

    @Test
    fun `needs-you goes on the channel that breaks through only when that's on`() {
        val notice = waiting()
        assertFalse(notice.through)
        val through = AgentNotifications.dnd(notice, NoticeDnd(through = true)) as AgentNotice.Waiting
        assertTrue(through.through)
        assertEquals(AgentNotifications.CHANNEL_WAITING_THROUGH, AgentNotifications.waitingChannel(through.through))
        assertEquals("agents_waiting", AgentNotifications.waitingChannel(false))
        // Off, it stays on the usual channel; the PC's Do Not Disturb doesn't touch a request.
        assertEquals(notice, AgentNotifications.dnd(notice, NoticeDnd(pcDnd = true)))
    }

    @Test
    fun `a finished turn posts silently while the PC is in Do Not Disturb, and only then`() {
        val notice = finished()
        assertFalse(notice.silent)
        assertTrue((AgentNotifications.dnd(notice, NoticeDnd(pcDnd = true)) as AgentNotice.Finished).silent)
        assertFalse((AgentNotifications.dnd(notice, NoticeDnd(through = true)) as AgentNotice.Finished).silent)
    }

    @Test
    fun `a held notice keeps its shape for when it's let go`() {
        val held = AgentNotice.Held(waiting())
        val shaped = AgentNotifications.dnd(held, NoticeDnd(through = true)) as AgentNotice.Held
        assertTrue((shaped.notice as AgentNotice.Waiting).through)
        val cancel = AgentNotice.Cancel(pc, "t1")
        assertEquals(cancel, AgentNotifications.dnd(cancel, NoticeDnd(pcDnd = true, through = true)))
    }

    @Test
    fun `the PC's Do Not Disturb never holds a notice`() {
        val dnd = PcPresence(atPc = true, locked = false, dnd = true, viewing = setOf("t1"), untilMs = Long.MAX_VALUE)
        val notice = waiting()
        assertEquals(notice, AgentNotifications.held(notice, dnd, quietAtPc = true, nowMs = 0L))
    }

    @Test
    fun `Stop speaking comes first while a result is read aloud, without unlocking`() {
        val kinds = { actions: List<NoticeAction> -> actions.map { it.kind } }
        val speaking = AgentNotifications.finishedActions(FinishKind.DONE, sdkInt = 34, speaking = true)
        assertEquals(listOf(NoticeAction.Kind.STOP_SPEAKING, NoticeAction.Kind.REPLY), kinds(speaking))
        assertFalse(speaking.first().authenticationRequired)
        assertEquals(listOf(NoticeAction.Kind.STOP_SPEAKING), kinds(AgentNotifications.finishedActions(FinishKind.PLAN, sdkInt = 34, speaking = true)))
        assertEquals(listOf(NoticeAction.Kind.REPLY), kinds(AgentNotifications.finishedActions(FinishKind.DONE, sdkInt = 34)))
        // Stop speaking only ever stops the phone's own voice: the tap guard doesn't hold it back.
        assertFalse(AgentNotifications.guards(NoticeAction.Kind.STOP_SPEAKING))
    }
}
