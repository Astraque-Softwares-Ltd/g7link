// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.android.notifications.AgentNotice
import app.g7link.android.notifications.AgentNotifications
import app.g7link.core.AgentAsk
import app.g7link.core.AgentAskKind
import app.g7link.core.AgentCan
import app.g7link.core.AgentCatalog
import app.g7link.core.AgentFolder
import app.g7link.core.AgentInfo
import app.g7link.core.AgentItem
import app.g7link.core.AgentItemKind
import app.g7link.core.AgentLimit
import app.g7link.core.AgentLimitState
import app.g7link.core.AgentMarkKind
import app.g7link.core.AgentMode
import app.g7link.core.AgentModel
import app.g7link.core.AgentPage
import app.g7link.core.AgentPlanState
import app.g7link.core.AgentQuestion
import app.g7link.core.AgentQuestionAsk
import app.g7link.core.AgentQuestionOption
import app.g7link.core.AgentQuestionOutcome
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState
import app.g7link.core.Event
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Questions, plans and control in the phone's state (contract §Q4.4, §Q6; reconcile file "Lists"). */
class AgentQuestionsModelTest {
    private val pc = "pc1"

    private fun thread(
        id: String = "t1",
        state: AgentThreadState = AgentThreadState.WAITING,
        mode: AgentMode = AgentMode.ASK,
        asking: UInt = 0u,
        updatedMs: Long = 1_000L,
    ) = AgentThread(
        id = id,
        agent = "codex",
        folder = "g7linux",
        title = "Draft the release notes",
        state = state,
        mode = mode,
        fromPhone = true,
        updatedMs = updatedMs,
        asks = 0u,
        summary = null,
        error = null,
        asking = asking,
    )

    private fun ffiQuestion(token: String = "q1", threadId: String = "t1", secret: Boolean = false, item: String = "i3", createdMs: Long = 2_000L) =
        AgentQuestion(
            token = token,
            thread = threadId,
            threadTitle = "Draft the release notes",
            agent = "codex",
            item = item,
            secret = secret,
            asks = if (secret) {
                emptyList()
            } else {
                listOf(
                    AgentQuestionAsk(
                        id = "runner",
                        header = "Test runner",
                        text = "Which setup?",
                        multi = false,
                        other = true,
                        secret = false,
                        options = listOf(AgentQuestionOption(label = "cargo test", note = "Runs on CI.")),
                    ),
                )
            },
            createdMs = createdMs,
            expiresMs = createdMs + 7_200_000L,
        )

    private fun ask(token: String = "a1", expiresMs: Long = 10_000L) = AgentAsk(
        token = token,
        thread = "t1",
        threadTitle = "Draft the release notes",
        agent = "codex",
        kind = AgentAskKind.COMMAND,
        title = "Run command",
        command = "cargo test",
        diff = null,
        path = null,
        reason = null,
        truncated = false,
        expiresMs = expiresMs,
    )

    private fun item(seq: ULong, kind: AgentItemKind, planState: AgentPlanState? = null, id: String = "i$seq") =
        AgentItem(seq = seq, id = id, kind = kind, whenMs = seq.toLong(), text = "text", truncated = false, tool = null, planState = planState)

    private fun reduce(state: AgentsState, event: Event) = AgentsModel.reduce(state, event).state

    private fun opened(): AgentsState {
        val open = AgentsModel.opened(AgentsModel.withThread(AgentsState(), thread()), "t1")
        return AgentsModel.withPage(open, AgentPage(thread = thread(), items = listOf(item(1u, AgentItemKind.USER), item(3u, AgentItemKind.QUESTION, id = "i3")), moreBefore = false, asks = emptyList()))
    }

    @Test
    fun `a question is kept until it ends, a secret one without what it asks`() {
        var state = reduce(opened(), Event.AgentQuestion(pc, ffiQuestion()))
        assertEquals(listOf("q1"), state.questionsFor("t1").map { it.token })
        assertEquals("Which setup?", state.questionsFor("t1").single().asks.single().text)

        val secret = reduce(state, Event.AgentQuestion(pc, ffiQuestion(token = "q2", secret = true, item = "i4", createdMs = 3_000L)))
        val shown = secret.questionsFor("t1").last()
        assertTrue(shown.secret)
        assertTrue(shown.asks.isEmpty())

        state = reduce(secret, Event.AgentQuestioned(pc, "q1", "t1", AgentQuestionOutcome.ANSWERED, "Pixel 8 Pro"))
        assertEquals(listOf("q2"), state.questionsFor("t1").map { it.token })
        // How it ended stays on its row while the thread is open: answered on another device.
        val mark = AgentsModel.marks(state, "t1").single { it.itemId == "i3" }
        assertEquals(AgentMarkKind.ALLOWED, mark.kind)
        assertEquals("Pixel 8 Pro", mark.by)
        // The other one still waits at its item.
        assertEquals(AgentMarkKind.WAITING, AgentsModel.marks(state, "t1").single { it.itemId == "i4" }.kind)
    }

    @Test
    fun `an answer from here reads as yours, a dismissal and a lapse as not answered`() {
        var state = reduce(opened(), Event.AgentQuestion(pc, ffiQuestion()))
        state = AgentsModel.replying(state, "q1")
        assertTrue("q1" in state.replying)
        state = reduce(state, Event.AgentQuestioned(pc, "q1", "t1", AgentQuestionOutcome.ANSWERED, "This phone"))
        assertEquals("", AgentsModel.marks(state, "t1").single().by)
        assertTrue(state.replying.isEmpty())

        val dismissed = reduce(reduce(opened(), Event.AgentQuestion(pc, ffiQuestion())), Event.AgentQuestioned(pc, "q1", "t1", AgentQuestionOutcome.DISMISSED, null))
        assertEquals(AgentMarkKind.DENIED, AgentsModel.marks(dismissed, "t1").single().kind)
        val lapsed = reduce(reduce(opened(), Event.AgentQuestion(pc, ffiQuestion())), Event.AgentQuestioned(pc, "q1", "t1", AgentQuestionOutcome.EXPIRED, null))
        assertEquals(AgentMarkKind.EXPIRED, AgentsModel.marks(lapsed, "t1").single().kind)
    }

    @Test
    fun `a reply that didn't go through can be sent again, unless the question is gone`() {
        val state = AgentsModel.replying(reduce(opened(), Event.AgentQuestion(pc, ffiQuestion())), "q1")
        val again = AgentsModel.replyFailed(state, "q1", AgentFailure.TIMEOUT)
        assertEquals(1, again.questionsFor("t1").size)
        assertTrue(again.replying.isEmpty())
        val gone = AgentsModel.replyFailed(state, "q1", AgentFailure.NOT_FOUND)
        assertTrue(gone.questionsFor("t1").isEmpty())
    }

    @Test
    fun `a page lists the questions waiting, replacing what was known of its thread`() {
        val known = reduce(opened(), Event.AgentQuestion(pc, ffiQuestion(token = "old")))
        val page = AgentPage(thread = thread(), items = emptyList(), moreBefore = false, asks = emptyList(), questions = listOf(ffiQuestion(token = "new")))
        val next = AgentsModel.withPage(known, page)
        assertEquals(listOf("new"), next.questionsFor("t1").map { it.token })
    }

    @Test
    fun `a deleted thread takes its questions with it`() {
        val state = reduce(opened(), Event.AgentQuestion(pc, ffiQuestion()))
        assertTrue(reduce(state, Event.AgentThreadRemoved(pc, "t1")).questions.isEmpty())
    }

    @Test
    fun `the dock shows a request or a question, the one asked first, a held one last`() {
        var state = reduce(opened(), Event.AgentQuestion(pc, ffiQuestion(createdMs = 2_000L)))
        state = reduce(state, Event.AgentAsk(pc, ask(expiresMs = 2_000L + 7_300_000L)))
        assertTrue(state.dockShown("t1") is DockShown.Question)
        assertTrue("the question is held, the request isn't", state.dockShown("t1") { it == "q1" } is DockShown.Approval)
        val older = reduce(state, Event.AgentAsk(pc, ask(token = "a0", expiresMs = 1_000L)))
        assertEquals("a0", (older.dockShown("t1") as DockShown.Approval).queue.shown.token)
        assertNull(AgentsState().dockShown("t1"))
    }

    @Test
    fun `a question counts as needing you, in the list, the drawer and Home`() {
        val asking = thread(state = AgentThreadState.WAITING, asking = 1u)
        val state = reduce(AgentsModel.withThread(AgentsState(), asking), Event.AgentQuestion(pc, ffiQuestion()))
        assertTrue(AgentsModel.needsYou(asking))
        assertEquals(listOf("t1"), AgentsModel.drawerGroups(state).needsYou.map { it.id })
        assertEquals(HomeLine.NeedsYou("codex", 1), AgentsModel.homeLine(state))
        assertEquals(ThreadStatus.NeedsYou, AgentsModel.status(asking))
    }

    @Test
    fun `a plan-mode turn that ended reads as Plan ready, listed with what finished`() {
        val ready = thread(state = AgentThreadState.DONE, mode = AgentMode.PLAN)
        assertEquals(ThreadStatus.PlanReady, AgentsModel.status(ready))
        val state = AgentsModel.withThread(AgentsState(), ready)
        assertEquals(listOf("t1"), AgentsModel.drawerGroups(state).recent.map { it.id })
        assertTrue(AgentsModel.drawerGroups(state).needsYou.isEmpty())
        assertEquals(ThreadStatus.Done, AgentsModel.status(thread(state = AgentThreadState.DONE)))
    }

    @Test
    fun `Implement acts on the newest plan only while it's proposed`() {
        val items = listOf(item(1u, AgentItemKind.PLAN, AgentPlanState.REPLACED), item(2u, AgentItemKind.USER), item(3u, AgentItemKind.PLAN, AgentPlanState.PROPOSED))
        assertEquals("i3", AgentsModel.newestProposedPlan(items))
        assertEquals("i3", AgentsModel.newestProposedPlan(items.dropLast(1) + item(3u, AgentItemKind.PLAN, null)))
        assertNull(AgentsModel.newestProposedPlan(items.dropLast(1) + item(3u, AgentItemKind.PLAN, AgentPlanState.IMPLEMENTED)))
        assertNull(AgentsModel.newestProposedPlan(items.take(1)))
        assertNull(AgentsModel.newestProposedPlan(emptyList()))
    }

    @Test
    fun `a plan Codex is still writing streams in, and its finished text replaces it`() {
        val open = AgentsModel.opened(AgentsModel.withThread(AgentsState(), thread(state = AgentThreadState.WORKING)), "t1")
        val started = AgentItem(seq = 2u, id = "p1", kind = AgentItemKind.PLAN, whenMs = 2L, text = null, truncated = false, tool = null)
        var state = AgentsModel.withPage(open, AgentPage(thread = thread(state = AgentThreadState.WORKING), items = listOf(item(1u, AgentItemKind.USER), started), moreBefore = false, asks = emptyList()))
        state = reduce(state, Event.AgentDelta(pc, "t1", "p1", 2u, 0u, "1. Add DropReason"))
        assertEquals("1. Add DropReason", state.open!!.streaming["p1"]?.text)
        assertEquals(AgentItemKind.PLAN, state.open!!.streaming["p1"]?.kind)
        val done = started.copy(text = "1. Add DropReason\n2. Send it", endMs = 9L)
        state = reduce(state, Event.AgentItemChanged(pc, "t1", done))
        assertNull(state.open!!.streaming["p1"])
    }

    @Test
    fun `the timeline gets the questions waiting, for how many each asks`() {
        val state = reduce(opened(), Event.AgentQuestion(pc, ffiQuestion()))
        val input = AgentsModel.timelineInput(state, "t1", emptySet())!!
        assertEquals(listOf("q1"), input.questions.map { it.token })
        assertEquals(1, AgentControlFfi.timelineQuestions(input.questions).single().asks.size)
    }

    @Test
    fun `an agent's abilities come only from a PC that takes control`() {
        val codex = AgentInfo(
            id = "codex",
            name = "Codex",
            installed = true,
            signedIn = true,
            loginHint = "codex login",
            models = listOf(AgentModel(id = "gpt-5", name = "GPT-5", note = null, isDefault = true, efforts = listOf("low", "high"), defaultEffort = "low", images = true, short = false)),
            can = listOf(AgentCan.STEER, AgentCan.IMAGES),
            limits = listOf(AgentLimit(window = "300", usedPct = 85u, resetsMs = 5L, state = AgentLimitState.WARN)),
        )
        val catalog = AgentCatalog(agents = listOf(codex), folders = listOf(AgentFolder(id = "f1", label = "g7linux")), canControl = true)
        val abilities = AgentsState(catalog = catalog).abilities("codex")
        assertTrue(abilities.control)
        assertEquals(setOf(AgentAbilities.STEER, AgentAbilities.IMAGES), abilities.can)
        assertEquals(FollowUp.STEER, AgentFollowUp.primary(abilities))
        assertEquals(LimitState.WARN, AgentUsage.pinned(abilities.limits)?.state)
        assertEquals(listOf("gpt-5"), abilities.models.map { it.id })
        assertFalse(AgentsState(catalog = catalog.copy(canControl = false)).abilities("codex").control)
    }

    @Test
    fun `a question notifies as needing you, opened in the app only`() {
        val state = reduce(AgentsModel.withThread(AgentsState(), thread(asking = 1u)), Event.AgentQuestion(pc, ffiQuestion()))
        val notice = AgentNotifications.plan(pc, Event.AgentQuestion(pc, ffiQuestion()), state, shown = null) { true }.single()
        assertTrue(notice is AgentNotice.Asking)
        assertEquals(1, (notice as AgentNotice.Asking).count)
        assertEquals(listOf(app.g7link.android.notifications.NoticeAction.Kind.REVIEW), AgentNotifications.askingActions().map { it.kind })
        // Not while its thread is on screen.
        assertTrue(AgentNotifications.plan(pc, Event.AgentQuestion(pc, ffiQuestion()), state, shown = "t1") { true }.isEmpty())
    }

    @Test
    fun `a plan-mode turn that ends notifies that a plan is ready, opened in the app only`() {
        val ready = thread(state = AgentThreadState.DONE, mode = AgentMode.PLAN)
        val state = AgentsModel.withThread(AgentsState(), ready)
        val event = Event.AgentDone(pc, ready, app.g7link.core.AgentOutcome.DONE, "Plan ready")
        val notice = AgentNotifications.plan(pc, event, state, shown = null) { true }.single() as AgentNotice.Finished
        assertEquals(app.g7link.android.notifications.FinishKind.PLAN, notice.kind)
        assertEquals("g7linux", notice.folder)
        assertTrue(AgentNotifications.finishedActions(app.g7link.android.notifications.FinishKind.PLAN, sdkInt = 34).isEmpty())
    }
}
