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
import app.g7link.core.AgentPlanStatus
import app.g7link.core.AgentPlanStep
import app.g7link.core.AgentSession
import app.g7link.core.AgentSessionSource
import app.g7link.core.AgentSessionState
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

    // ---- 0.1.14: the Live Update, followed sessions ----

    private val sessionId = "sc0123456789abcdef0123456789abcdef"

    private fun session(state: AgentSessionState, guess: Boolean = false, step: String? = null) = AgentSession(
        id = sessionId,
        agent = "codex",
        folder = "g7linux",
        title = "Bump the Qt version",
        state = state,
        guess = guess,
        source = AgentSessionSource.TERMINAL,
        updatedMs = 1_000L,
        step = step,
        summary = null,
        waitingFor = null,
        canContinue = false,
        blocked = null,
        sinceMs = 500L,
    )

    @Test
    fun `the working Live Update carries the step, when it began and where the plan is`() {
        val running = thread().copy(
            step = "Run BLE tests 10 times",
            sinceMs = 1_795_000_000_000L,
            plan = listOf(
                AgentPlanStep("Reproduce", AgentPlanStatus.COMPLETED),
                AgentPlanStep("Run the tests", AgentPlanStatus.IN_PROGRESS),
                AgentPlanStep("Say why", AgentPlanStatus.PENDING),
            ),
        )
        val notice = plan(Event.AgentThreadChanged(pc, running), base(running), followed = setOf("t1")).single() as AgentNotice.Working
        assertEquals("Run BLE tests 10 times", notice.step)
        assertEquals(1_795_000_000_000L, notice.sinceMs)
        assertEquals(2, notice.planStep)
        assertEquals(3, notice.planSize)

        val lines = AgentNotifications.workingLines(notice, now = { "Now: $it" }, stepOf = { at, of -> "Step $at of $of" })
        assertEquals(listOf("Add a dark mode toggle", "Now: Run BLE tests 10 times", "Step 2 of 3"), lines)

        // No plan, no step: only the thread's title; a blank step isn't shown.
        val plain = plan(Event.AgentThreadChanged(pc, thread().copy(step = " ")), base(thread()), followed = setOf("t1")).single() as AgentNotice.Working
        assertEquals(null, plain.step)
        assertEquals(listOf("Add a dark mode toggle"), AgentNotifications.workingLines(plain, { "Now: $it" }, { a, b -> "$a/$b" }))
    }

    @Test
    fun `a step change updates the Live Update at most once every 3 s`() {
        val gate = LiveUpdateGate(intervalMs = 3_000L)
        val tag = AgentNotifications.tag(pc, "t1")
        assertEquals(LiveUpdateGate.Admit.Now, gate.admit(tag, 10_000L))
        assertEquals(LiveUpdateGate.Admit.Later(2_000L), gate.admit(tag, 11_000L))
        assertEquals(LiveUpdateGate.Admit.Later(500L), gate.admit(tag, 12_500L))
        assertEquals(LiveUpdateGate.Admit.Now, gate.admit(tag, 13_000L))
        // Another thread has its own pace.
        assertEquals(LiveUpdateGate.Admit.Now, gate.admit(AgentNotifications.tag(pc, "t2"), 13_100L))
        // A request in between: working again shows at once.
        gate.replaced(tag)
        assertEquals(LiveUpdateGate.Admit.Now, gate.admit(tag, 13_200L))
    }

    @Test
    fun `a dismissed Live Update isn't posted again until its turn ends`() {
        val gate = LiveUpdateGate()
        val tag = AgentNotifications.tag(pc, "t1")
        assertEquals(LiveUpdateGate.Admit.Now, gate.admit(tag, 0L))
        gate.dismissed(tag)
        assertEquals(LiveUpdateGate.Admit.Drop, gate.admit(tag, 10_000L))
        // A request doesn't bring it back for the same turn.
        gate.replaced(tag)
        assertEquals(LiveUpdateGate.Admit.Drop, gate.admit(tag, 20_000L))
        gate.ended(tag)
        assertEquals(LiveUpdateGate.Admit.Now, gate.admit(tag, 30_000L))
        // A PC going away ends all of its own.
        gate.dismissed(tag)
        gate.endedAll("agent:$pc:")
        assertFalse(gate.isDismissed(tag))
    }

    @Test
    fun `a session shows in the status bar only once the user keeps it there`() {
        val state = AgentsModel.withSessions(AgentsState(catalog = catalog), listOf(session(AgentSessionState.WORKING)))
        val event = Event.AgentSessionChanged(pc, session(AgentSessionState.WORKING, step = "Run command"))
        assertTrue(AgentNotifications.plan(pc, event, state, shown = null) { false }.isEmpty())

        val kept = AgentNotifications.plan(pc, event, state, shown = null, kept = { it == sessionId }) { false }
        val working = kept.single() as AgentNotice.SessionWorking
        assertEquals("Codex", working.agent)
        assertEquals("Run command", working.step)
        assertEquals(500L, working.sinceMs)
        assertEquals(listOf(NoticeAction.Kind.UNFOLLOW), AgentNotifications.sessionWorkingActions().map { it.kind })

        // A guess still reads as working; idle and ended take it away.
        val guessed = Event.AgentSessionChanged(pc, session(AgentSessionState.MAY_NEED_YOU, guess = true))
        assertTrue(AgentNotifications.plan(pc, guessed, state, shown = null, kept = { true }) { false }.single() is AgentNotice.SessionWorking)
        for (gone in listOf(AgentSessionState.IDLE, AgentSessionState.ENDED)) {
            val notices = AgentNotifications.plan(pc, Event.AgentSessionChanged(pc, session(gone)), state, shown = null, kept = { true }) { false }
            assertEquals(listOf(AgentNotice.CancelSession(pc, sessionId)), notices)
        }
        // Removed from the PC's list: cancelled whether kept or not.
        assertEquals(
            listOf(AgentNotice.CancelSession(pc, sessionId)),
            AgentNotifications.plan(pc, Event.AgentSessionRemoved(pc, sessionId), state, shown = null) { false },
        )
        // On screen: nothing in the status bar for it.
        assertEquals(
            listOf(AgentNotice.CancelSession(pc, sessionId)),
            AgentNotifications.plan(pc, event, state, shown = null, shownSession = sessionId, kept = { true }) { false },
        )
    }

    @Test
    fun `a kept session that needs you says so on the waiting channel, with nothing to press`() {
        val state = AgentsModel.withSessions(AgentsState(catalog = catalog), listOf(session(AgentSessionState.NEEDS_YOU)))
        val notice = AgentNotifications.plan(
            pc,
            Event.AgentSessionChanged(pc, session(AgentSessionState.NEEDS_YOU)),
            state,
            shown = null,
            kept = { true },
        ) { false }.single() as AgentNotice.SessionWaiting
        assertEquals("Codex", notice.agent)
        assertEquals("Bump the Qt version", notice.title)
        assertTrue(AgentNotifications.sessionWaitingActions().isEmpty())
        assertEquals("agent-session:pc1:$sessionId", AgentNotifications.sessionTag(pc, sessionId))
    }
}
