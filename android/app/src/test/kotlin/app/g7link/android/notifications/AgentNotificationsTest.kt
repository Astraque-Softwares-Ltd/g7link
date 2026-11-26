// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.notifications

import app.g7link.android.ui.agents.AgentsModel
import app.g7link.android.ui.agents.AgentsState
import app.g7link.android.ui.agents.NoticeAnswers
import app.g7link.android.ui.agents.PcPresence
import app.g7link.android.ui.agents.QuestionAnswer
import app.g7link.android.ui.agents.QuestionAsk
import app.g7link.android.ui.agents.QuestionInfo
import app.g7link.android.ui.agents.QuestionOption
import app.g7link.core.AgentAsk
import app.g7link.core.AgentAskKind
import app.g7link.core.AgentAskOutcome
import app.g7link.core.AgentCatalog
import app.g7link.core.AgentInfo
import app.g7link.core.AgentMode
import app.g7link.core.AgentOutcome
import app.g7link.core.AgentPlanStatus
import app.g7link.core.AgentPlanStep
import app.g7link.core.AgentQuestion
import app.g7link.core.AgentQuestionAsk
import app.g7link.core.AgentQuestionOption
import app.g7link.core.AgentSession
import app.g7link.core.AgentSessionSource
import app.g7link.core.AgentSessionState
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState
import app.g7link.core.Event
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
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
        mode = AgentMode.ASK,
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

    // ---- 0.1.20: answering from a notification (contract §V3), presence (§V4.4) ----

    private fun info(
        options: List<String> = listOf("Back off", "Fixed 30 s"),
        other: Boolean = true,
        multi: Boolean = false,
        secret: Boolean = false,
        asks: Int = 1,
        token: String = "q1",
    ) = QuestionInfo(
        token = token,
        threadId = "t1",
        agent = "claude",
        itemId = "i1",
        secret = secret,
        asks = (1..asks).map { n ->
            QuestionAsk(id = "a$n", text = "Should the callback back off?", multi = multi, other = other, options = options.map { QuestionOption(it) })
        },
        createdMs = 1_000L,
        expiresMs = 60_000L,
    )

    @Test
    fun `a question is answered from its notification only in the shapes that fit`() {
        val two = AgentsModel.answerableInNotification(info(), sdkInt = 34)
        assertEquals(NoticeAnswers("a1", listOf("Back off", "Fixed 30 s"), type = true), two)
        assertEquals(
            listOf(NoticeAction.Kind.OPTION, NoticeAction.Kind.OPTION, NoticeAction.Kind.ANSWER),
            AgentNotifications.askingActions(two).map { it.kind },
        )
        assertEquals(listOf(0, 1, null), AgentNotifications.askingActions(two).map { it.option })

        val twoClosed = AgentsModel.answerableInNotification(info(other = false), sdkInt = 34)
        assertEquals(listOf(NoticeAction.Kind.OPTION, NoticeAction.Kind.OPTION), AgentNotifications.askingActions(twoClosed).map { it.kind })

        val three = AgentsModel.answerableInNotification(info(options = listOf("SQLite", "Postgres", "Both"), other = false), sdkInt = 34)
        assertEquals(listOf(0, 1, 2), AgentNotifications.askingActions(three).map { it.option })
        // Every way of answering here needs the phone unlocked.
        assertTrue(AgentNotifications.askingActions(two).all { it.authenticationRequired })
        assertTrue(AgentNotifications.askingActions(three).all { it.authenticationRequired })

        // Anything else is opened in the app, as today.
        val review = listOf(NoticeAction.Kind.REVIEW)
        for (refused in listOf(
            info(options = listOf("SQLite", "Postgres", "Both")), // three and "Something else": four buttons
            info(options = listOf("Yes")),
            info(options = listOf("a", "b", "c", "d"), other = false),
            info(secret = true),
            info(multi = true),
            info(asks = 2),
            info(options = listOf("Back off", "x".repeat(21))),
            info(options = listOf("Back off", "Fixed\n30 s")),
        )) {
            val answers = AgentsModel.answerableInNotification(refused, sdkInt = 34)
            assertNull(answers)
            assertEquals(review, AgentNotifications.askingActions(answers).map { it.kind })
        }
        // A label of exactly 20 characters fits; Android 11 can't ask to unlock for a button.
        assertTrue(AgentsModel.answerableInNotification(info(options = listOf("Back off", "x".repeat(20))), sdkInt = 34) != null)
        assertNull(AgentsModel.answerableInNotification(info(), sdkInt = 30))
        // The lock screen is unchanged: who asks, nothing to press.
        assertTrue(AgentNotifications.publicAskingActions().isEmpty())
    }

    @Test
    fun `a request offers Say why while the PC takes a reason, between Deny and Allow`() {
        val kinds = { actions: List<NoticeAction> -> actions.map { it.kind } }
        val why = AgentNotifications.waitingActions(allowable = true, sdkInt = 34, sayWhy = true)
        assertEquals(listOf(NoticeAction.Kind.DENY, NoticeAction.Kind.DENY_WHY, NoticeAction.Kind.ALLOW), kinds(why))
        assertTrue(why.single { it.kind == NoticeAction.Kind.DENY_WHY }.authenticationRequired)
        assertFalse(why.single { it.kind == NoticeAction.Kind.DENY }.authenticationRequired)
        assertEquals(
            listOf(NoticeAction.Kind.DENY, NoticeAction.Kind.DENY_WHY, NoticeAction.Kind.REVIEW),
            kinds(AgentNotifications.waitingActions(allowable = false, sdkInt = 34, sayWhy = true)),
        )
        // A PC that doesn't take one: as before.
        assertEquals(listOf(NoticeAction.Kind.DENY, NoticeAction.Kind.ALLOW), kinds(AgentNotifications.waitingActions(allowable = true, sdkInt = 34)))
        // Before Android 12 nothing that needs unlocking is offered.
        assertEquals(
            listOf(NoticeAction.Kind.DENY, NoticeAction.Kind.REVIEW),
            kinds(AgentNotifications.waitingActions(allowable = true, sdkInt = 30, sayWhy = true)),
        )
        // The lock screen's version is unchanged.
        assertEquals(listOf(NoticeAction.Kind.DENY, NoticeAction.Kind.REVIEW), kinds(AgentNotifications.publicWaitingActions()))

        // The plan says whether the PC takes a reason, and what else waits in the thread.
        val withWhy = AgentCatalog(agents = catalog.agents, folders = emptyList(), canSayWhy = true)
        var state = AgentsModel.withThreads(AgentsState(catalog = withWhy), listOf(thread(state = AgentThreadState.WAITING)))
        state = after(state, Event.AgentAsk(pc, ask("a1")))
        val notice = plan(Event.AgentAsk(pc, ask("a2", expiresMs = 20_000L)), state).single() as AgentNotice.Waiting
        assertTrue(notice.sayWhy)
        assertEquals("a1", notice.ask.token)
        assertEquals(listOf("a1", "a2"), notice.waiting)
        assertFalse((plan(Event.AgentAsk(pc, ask()), base(thread())).single() as AgentNotice.Waiting).sayWhy)
    }

    private fun ffiQuestion(options: List<String>, other: Boolean = false) = AgentQuestion(
        token = "q1",
        thread = "t1",
        threadTitle = "Add a dark mode toggle",
        agent = "codex",
        item = "i1",
        secret = false,
        asks = listOf(
            AgentQuestionAsk(
                id = "a1",
                header = "Storage",
                text = "Where does the report read from?",
                multi = false,
                other = other,
                secret = false,
                options = options.map { AgentQuestionOption(it, null) },
            ),
        ),
        createdMs = 1_000L,
        expiresMs = 60_000L,
    )

    @Test
    fun `a question's notice carries its answers and words on Android 12 and later`() {
        val event = Event.AgentQuestion(pc, ffiQuestion(listOf("SQLite", "Postgres", "Both")))
        val state = after(base(thread()), event)
        val asking = AgentNotifications.plan(pc, event, state, shown = null, sdkInt = 34) { false }.single() as AgentNotice.Asking
        assertEquals(NoticeAnswers("a1", listOf("SQLite", "Postgres", "Both"), type = false), asking.answers)
        assertEquals("Where does the report read from?", asking.text)
        assertEquals(listOf("q1"), asking.waiting)
        val old = AgentNotifications.plan(pc, event, state, shown = null, sdkInt = 30) { false }.single() as AgentNotice.Asking
        assertNull(old.answers)
    }

    @Test
    fun `the receiver reads an option by its index from the question as it is now`() {
        val question = info()
        assertEquals(QuestionAnswer("a1", listOf("Fixed 30 s"), null), AgentsModel.noticeAnswer(question, "a1", option = 1, typed = null, sdkInt = 34))
        assertEquals(QuestionAnswer("a1", emptyList(), "Back off, 3 tries"), AgentsModel.noticeAnswer(question, "a1", null, "Back off, 3 tries", 34))
        // Out of range, another ask, a question gone or changed shape: nothing is sent.
        assertNull(AgentsModel.noticeAnswer(question, "a1", option = 2, typed = null, sdkInt = 34))
        assertNull(AgentsModel.noticeAnswer(question, "a1", option = -1, typed = null, sdkInt = 34))
        assertNull(AgentsModel.noticeAnswer(question, "a9", option = 0, typed = null, sdkInt = 34))
        assertNull(AgentsModel.noticeAnswer(null, "a1", option = 0, typed = null, sdkInt = 34))
        assertNull(AgentsModel.noticeAnswer(info(asks = 2), "a1", option = 0, typed = null, sdkInt = 34))
        assertNull(AgentsModel.noticeAnswer(info(secret = true), "a1", option = 0, typed = null, sdkInt = 34))
        // Typed words only where the ask takes them, and within the wire's 500 bytes.
        assertNull(AgentsModel.noticeAnswer(info(other = false), "a1", null, "mine", 34))
        assertNull(AgentsModel.noticeAnswer(question, "a1", null, "x".repeat(501), 34))
        assertEquals("x".repeat(500), AgentsModel.typedAnswer("  " + "x".repeat(600) + " "))
        assertNull(AgentsModel.typedAnswer("   "))
    }

    @Test
    fun `a reason is trimmed, cut on a character boundary, and an empty one sends nothing`() {
        assertNull(AgentsModel.denyReason(null))
        assertNull(AgentsModel.denyReason("  \n "))
        assertEquals("Run only the retry tests", AgentsModel.denyReason("  Run only the retry tests\n"))
        assertEquals("x".repeat(2000), AgentsModel.denyReason("x".repeat(2001)))
        // 1999 bytes, then a two-byte letter that would end at 2001: it goes whole or not at all.
        val cut = AgentsModel.denyReason("x".repeat(1999) + "é" + "y")!!
        assertEquals("x".repeat(1999), cut)
        assertTrue(AgentsModel.utf8Length(cut) <= AgentsModel.MAX_DENY_BYTES)
        // A four-byte character is never split.
        val emoji = AgentsModel.cutUtf8("ab\uD83D\uDE00", 5)
        assertEquals("ab", emoji)
        assertEquals("ab\uD83D\uDE00", AgentsModel.cutUtf8("ab\uD83D\uDE00", 6))
    }

    @Test
    fun `taps that let something happen are ignored for 1500 ms after the notification changes`() {
        assertTrue(AgentNotifications.guards(NoticeAction.Kind.ALLOW))
        assertTrue(AgentNotifications.guards(NoticeAction.Kind.OPTION))
        assertTrue(AgentNotifications.guards(NoticeAction.Kind.ANSWER))
        assertFalse(AgentNotifications.guards(NoticeAction.Kind.DENY))
        assertFalse(AgentNotifications.guards(NoticeAction.Kind.DENY_WHY))
        assertFalse(AgentNotifications.guards(NoticeAction.Kind.STOP))

        val memory = NoticeMemory()
        val tag = AgentNotifications.tag(pc, "t1")
        memory.showing(tag, "a1", 10_000L)
        assertTrue(memory.guarded("a1", 10_000L))
        assertTrue(memory.guarded("a1", 11_499L))
        assertFalse(memory.guarded("a1", 11_500L))
        // The next one slides in: guarded from then.
        memory.showing(tag, "a2", 20_000L)
        assertTrue(memory.guarded("a2", 21_000L))
        // Shown again unchanged (a restore, a repost): the guard isn't renewed.
        assertEquals(NoticeMemory.Showing(again = true, quiet = false), memory.showing(tag, "a2", 21_400L))
        assertFalse(memory.guarded("a2", 21_600L))
        // A token never shown here (the app restarted) counts as old.
        assertFalse(memory.guarded("zz", 21_000L))
    }

    @Test
    fun `the next one replaces your own answer quietly only within 10 s`() {
        val memory = NoticeMemory()
        val tag = AgentNotifications.tag(pc, "t1")
        assertEquals(NoticeMemory.Showing(again = false, quiet = false), memory.showing(tag, "a1", 0L))
        memory.answered(tag, 1_000L)
        assertEquals(NoticeMemory.Showing(again = false, quiet = true), memory.showing(tag, "a2", 3_000L))
        // Used once: the one after that sounds again.
        assertEquals(NoticeMemory.Showing(again = false, quiet = false), memory.showing(tag, "a3", 4_000L))
        // Too late after the answer: it sounds.
        memory.answered(tag, 5_000L)
        assertEquals(NoticeMemory.Showing(again = false, quiet = false), memory.showing(tag, "a4", 15_001L))
        // Answered on another thread: this one sounds.
        memory.answered(AgentNotifications.tag(pc, "t2"), 20_000L)
        assertEquals(NoticeMemory.Showing(again = false, quiet = false), memory.showing(tag, "a5", 20_500L))
    }

    @Test
    fun `the count follows the run of requests and questions in a thread`() {
        val memory = NoticeMemory()
        val tag = AgentNotifications.tag(pc, "t1")
        assertNull(memory.count(tag, listOf("a1")))
        assertEquals(1 to 3, memory.count(tag, listOf("a1", "a2", "q1")))
        // The first was answered: the second of three.
        assertEquals(2 to 3, memory.count(tag, listOf("a2", "q1")))
        // One more arrives meanwhile.
        assertEquals(2 to 4, memory.count(tag, listOf("a2", "q1", "a3")))
        assertEquals(4 to 4, memory.count(tag, listOf("a3")))
        // Nothing waits: the run ends, and the next one is alone again.
        assertNull(memory.count(tag, emptyList()))
        assertNull(memory.count(tag, listOf("a4")))
        memory.ended(tag)
        assertEquals(1 to 2, memory.count(tag, listOf("a5", "a6")))
    }

    private fun presence(
        atPc: Boolean = true,
        locked: Boolean = false,
        dnd: Boolean = false,
        viewing: Set<String> = setOf("t1"),
        untilMs: Long = 100_000L,
    ) = PcPresence(atPc = atPc, locked = locked, dnd = dnd, viewing = viewing, untilMs = untilMs)

    @Test
    fun `a notice waits while someone at the PC looks at its thread, and every unknown notifies`() {
        val waiting = plan(Event.AgentAsk(pc, ask()), base(thread(state = AgentThreadState.WAITING))).single()
        val now = 50_000L
        val held = { notice: AgentNotice, p: PcPresence?, quiet: Boolean -> AgentNotifications.held(notice, p, quiet, now) is AgentNotice.Held }
        assertTrue(held(waiting, presence(), true))
        // The setting off, nobody at the PC, another thread on screen, the PC locked or in DND, stale or no presence: it notifies.
        assertFalse(held(waiting, presence(), false))
        assertFalse(held(waiting, presence(atPc = false), true))
        assertFalse(held(waiting, presence(viewing = setOf("t2")), true))
        assertFalse(held(waiting, presence(locked = true), true))
        assertFalse(held(waiting, presence(dnd = true), true))
        assertFalse(held(waiting, presence(untilMs = now), true))
        assertFalse(held(waiting, null, true))

        // Questions and finished turns wait too; the working Live Update never does.
        val done = plan(Event.AgentDone(pc, thread(state = AgentThreadState.DONE), AgentOutcome.DONE, "Added it."), base()).single()
        assertTrue(held(done, presence(), true))
        val working = plan(Event.AgentThreadChanged(pc, thread()), base(), followed = setOf("t1")).single()
        assertFalse(held(working, presence(), true))

        // Through the plan.
        val planned = AgentNotifications.plan(
            pc,
            Event.AgentAsk(pc, ask()),
            after(base(thread(state = AgentThreadState.WAITING)), Event.AgentAsk(pc, ask())),
            shown = null,
            presence = presence(),
            quietAtPc = true,
            nowMs = now,
        ) { false }.single()
        assertEquals(AgentNotice.Held(waiting), planned)
    }

    @Test
    fun `a held notice is let go when it stops holding, if it's still true`() {
        val store = HeldNotices()
        var state = base(thread(state = AgentThreadState.WAITING))
        state = after(state, Event.AgentAsk(pc, ask("a1")))
        val waiting = AgentNotifications.threadNotice(pc, state, "t1", null, 34) { false }!!
        val tag = AgentNotifications.tag(pc, "t1")
        store.hold(tag, waiting)
        val now = 50_000L
        val holds = { p: PcPresence? -> { n: AgentNotice -> AgentNotifications.held(n, p, true, now) is AgentNotice.Held } }

        // Still looking: nothing goes.
        assertTrue(store.release("agent:$pc:", holds(presence())).isEmpty())
        assertTrue(store.has(tag))
        // Stepped away: it goes, as what the thread shows now.
        val out = store.release("agent:$pc:", holds(presence(atPc = false)))
        assertEquals(listOf(waiting), out)
        assertFalse(store.has(tag))
        assertEquals(waiting, AgentNotifications.released(pc, out.single(), state, null, 34) { false })
        // Answered on the PC meanwhile: nothing waits, nothing is posted.
        val answered = after(state, Event.AgentAsked(pc, "a1", "t1", AgentAskOutcome.ALLOWED))
        assertNull(AgentNotifications.released(pc, waiting, answered, null, 34) { false })

        // A finished turn goes only while the thread is still unread.
        val unread = thread(state = AgentThreadState.DONE).copy(unread = true)
        val done = AgentNotice.Finished(pc, "t1", "Codex", "Add a dark mode toggle", FinishKind.DONE, "Added it.")
        assertEquals(done, AgentNotifications.released(pc, done, base(unread), null, 34) { false })
        assertNull(AgentNotifications.released(pc, done, base(unread.copy(unread = false)), null, 34) { false })
        // Open on this phone: nothing to post.
        assertNull(AgentNotifications.released(pc, done, base(unread), "t1", 34) { false })

        // Only this PC's are let go.
        store.hold("agent:pc2:t9", waiting)
        assertTrue(store.release("agent:$pc:", holds(null)).isEmpty())
        store.dropAll("agent:pc2:")
        assertFalse(store.has("agent:pc2:t9"))
    }
}
