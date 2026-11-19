// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.notifications

import app.g7link.android.ui.agents.AgentsModel
import app.g7link.android.ui.agents.AgentsState
import app.g7link.core.AgentAsk
import app.g7link.core.AgentAskKind
import app.g7link.core.AgentAskOutcome
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

class AgentNotificationsTest {
    private val pc = "pc1"

    private fun thread(
        id: String = "t1",
        state: AgentThreadState = AgentThreadState.WORKING,
        fromPhone: Boolean = false,
        error: String? = null,
        errorCode: String? = null,
    ) = AgentThread(
        id = id,
        agent = "codex",
        folder = "g7linux",
        title = "Add a dark mode toggle",
        state = state,
        mode = AgentMode.SUPERVISED,
        fromPhone = fromPhone,
        updatedMs = 1_000L,
        asks = 0u,
        summary = null,
        error = error,
        errorCode = errorCode,
    )

    private fun ask(token: String = "a1", threadId: String = "t1", command: String? = "npm run test -- settings", expiresMs: Long = 10_000L) =
        AgentAsk(
            token = token,
            thread = threadId,
            threadTitle = "Add a dark mode toggle",
            agent = "codex",
            kind = AgentAskKind.COMMAND,
            title = "Run command",
            command = command,
            diff = null,
            path = null,
            reason = null,
            truncated = false,
            expiresMs = expiresMs,
        )

    private val catalog = AgentCatalog(
        agents = listOf(AgentInfo(id = "codex", name = "Codex", installed = true, signedIn = true, loginHint = "codex login")),
        folders = emptyList(),
        modes = listOf(AgentMode.SUPERVISED),
    )

    /** The PC's state after [event], as Core hands it to the planner. */
    private fun after(state: AgentsState, event: Event): AgentsState = AgentsModel.reduce(state, event).state

    private fun base(vararg threads: AgentThread) =
        AgentsModel.withThreads(AgentsState(catalog = catalog), threads.toList())

    private fun plan(event: Event, before: AgentsState, shown: String? = null, followed: Set<String> = emptySet()) =
        AgentNotifications.plan(pc, event, after(before, event), shown) { it in followed }

    @Test
    fun `a request notifies for any thread, with the folder and the agent's name`() {
        val event = Event.AgentAsk(pc, ask())
        val notice = plan(event, base(thread(state = AgentThreadState.WAITING))).single() as AgentNotice.Waiting
        assertEquals("Codex", notice.agent)
        assertEquals("g7linux", notice.folder)
        assertEquals("a1", notice.ask.token)
        assertTrue(notice.allowable)

        // A thread the phone doesn't know yet: no folder, so the PC's name stands in.
        val unknown = plan(event, AgentsState(catalog = catalog)).single() as AgentNotice.Waiting
        assertEquals(null, unknown.folder)

        // A long or multi-line command is reviewed in the app.
        val long = plan(Event.AgentAsk(pc, ask(command = "a".repeat(101))), base(thread())).single() as AgentNotice.Waiting
        assertFalse(long.allowable)
    }

    @Test
    fun `nothing posts for the thread on screen`() {
        val waiting = base(thread(state = AgentThreadState.WAITING))
        assertTrue(plan(Event.AgentAsk(pc, ask()), waiting, shown = "t1").isEmpty())
        assertTrue(plan(Event.AgentThreadChanged(pc, thread()), base(thread()), shown = "t1", followed = setOf("t1")).isEmpty())
        val done = plan(Event.AgentDone(pc, thread(state = AgentThreadState.DONE), AgentOutcome.DONE, "All set."), base(thread()), shown = "t1")
        assertEquals(listOf(AgentNotice.Cancel(pc, "t1")), done)
        // Another thread on the same PC still notifies.
        assertTrue(plan(Event.AgentAsk(pc, ask(threadId = "t2")), base(thread("t2", AgentThreadState.WAITING)), shown = "t1").single() is AgentNotice.Waiting)
    }

    @Test
    fun `working shows only for threads this phone started or opened`() {
        val event = Event.AgentThreadChanged(pc, thread())
        assertTrue(plan(event, base(), followed = setOf("t1")).single() is AgentNotice.Working)
        assertEquals(listOf(AgentNotice.Cancel(pc, "t1")), plan(event, base()))
        // The end of a turn is AgentDone's to show.
        assertTrue(plan(Event.AgentThreadChanged(pc, thread(state = AgentThreadState.DONE)), base(), followed = setOf("t1")).isEmpty())
    }

    @Test
    fun `an answered request gives way to the next, then to working`() {
        var state = base(thread())
        state = after(state, Event.AgentAsk(pc, ask("a1")))
        state = after(state, Event.AgentAsk(pc, ask("a2", expiresMs = 20_000L)))

        val next = plan(Event.AgentAsked(pc, "a1", "t1", AgentAskOutcome.ALLOWED), state).single() as AgentNotice.Waiting
        assertEquals("a2", next.ask.token)

        state = after(state, Event.AgentAsked(pc, "a1", "t1", AgentAskOutcome.ALLOWED))
        val asked = Event.AgentAsked(pc, "a2", "t1", AgentAskOutcome.DENIED)
        assertTrue(plan(asked, state, followed = setOf("t1")).single() is AgentNotice.Working)
        assertEquals(listOf(AgentNotice.Cancel(pc, "t1")), plan(asked, state))

        // Cancelled because the turn ended: the finished notification stays.
        val ended = base(thread(state = AgentThreadState.DONE))
        assertTrue(plan(Event.AgentAsked(pc, "zz", "t1", AgentAskOutcome.CANCELLED), ended).isEmpty())
    }

    @Test
    fun `the end of a turn says done, failed or paused, and a stop only clears`() {
        val done = plan(Event.AgentDone(pc, thread(state = AgentThreadState.DONE), AgentOutcome.DONE, "Added it."), base()).single() as AgentNotice.Finished
        assertEquals(FinishKind.DONE, done.kind)
        assertEquals("Added it.", done.text)

        val failed = plan(
            Event.AgentDone(pc, thread(state = AgentThreadState.FAILED, error = "Not logged in", errorCode = "login"), AgentOutcome.FAILED, ""),
            base(),
        ).single() as AgentNotice.Finished
        assertEquals(FinishKind.FAILED, failed.kind)
        assertEquals("Not logged in", failed.text)

        val paused = plan(
            Event.AgentDone(
                pc,
                thread(state = AgentThreadState.FAILED, error = "Usage limit reached|resets_ms=1795050000000", errorCode = "usage_limit"),
                AgentOutcome.FAILED,
                "",
            ),
            base(),
        ).single() as AgentNotice.Finished
        assertEquals(FinishKind.PAUSED, paused.kind)
        assertEquals(1_795_050_000_000L, paused.resetsMs)

        val stopped = plan(Event.AgentDone(pc, thread(state = AgentThreadState.INTERRUPTED), AgentOutcome.INTERRUPTED, ""), base())
        assertEquals(listOf(AgentNotice.Cancel(pc, "t1")), stopped)
    }

    @Test
    fun `a PC with nothing to run, or a deleted thread, clears its notifications`() {
        assertEquals(listOf(AgentNotice.CancelPc(pc)), plan(Event.AgentCatalog(pc, null), base(thread())))
        assertTrue(plan(Event.AgentCatalog(pc, catalog), base()).isEmpty())
        assertEquals(listOf(AgentNotice.Cancel(pc, "t1")), plan(Event.AgentThreadRemoved(pc, "t1"), base(thread())))
    }

    @Test
    fun `deny comes first, and allow and reply need the phone unlocked`() {
        val kinds = { actions: List<NoticeAction> -> actions.map { it.kind } }
        val allow = AgentNotifications.waitingActions(allowable = true, sdkInt = 34)
        assertEquals(listOf(NoticeAction.Kind.DENY, NoticeAction.Kind.ALLOW), kinds(allow))
        assertTrue(allow.single { it.kind == NoticeAction.Kind.ALLOW }.authenticationRequired)
        assertFalse(allow.single { it.kind == NoticeAction.Kind.DENY }.authenticationRequired)

        assertEquals(listOf(NoticeAction.Kind.DENY, NoticeAction.Kind.REVIEW), kinds(AgentNotifications.waitingActions(allowable = false, sdkInt = 34)))
        // Before Android 12 an action can't require unlocking, so Allow isn't offered there.
        assertEquals(listOf(NoticeAction.Kind.DENY, NoticeAction.Kind.REVIEW), kinds(AgentNotifications.waitingActions(allowable = true, sdkInt = 30)))

        val public = AgentNotifications.publicWaitingActions()
        assertEquals(listOf(NoticeAction.Kind.DENY, NoticeAction.Kind.REVIEW), kinds(public))

        val stop = AgentNotifications.workingActions().single()
        assertEquals(NoticeAction.Kind.STOP, stop.kind)
        assertFalse(stop.authenticationRequired)

        val reply = AgentNotifications.finishedActions(FinishKind.DONE, sdkInt = 34).single()
        assertEquals(NoticeAction.Kind.REPLY, reply.kind)
        assertTrue(reply.authenticationRequired)
        assertTrue(AgentNotifications.finishedActions(FinishKind.PAUSED, sdkInt = 34).isEmpty())
        assertTrue(AgentNotifications.finishedActions(FinishKind.DONE, sdkInt = 30).isEmpty())
    }

    @Test
    fun `notifications are tagged per PC and thread`() {
        assertEquals("agent:pc1:t1", AgentNotifications.tag("pc1", "t1"))
        assertTrue(AgentsModel.isId("0123456789abcdef0123456789abcdef"))
        assertFalse(AgentsModel.isId("../etc"))
        assertFalse(AgentsModel.isId(""))
        assertFalse(AgentsModel.isId("A1"))
        assertFalse(AgentsModel.isId("a".repeat(41)))
        assertTrue(AgentsModel.isId("a".repeat(52), max = 128))
    }
}
