// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentAsk
import app.g7link.core.AgentAskKind
import app.g7link.core.AgentAskOutcome
import app.g7link.core.AgentCatalog
import app.g7link.core.AgentFolder
import app.g7link.core.AgentInfo
import app.g7link.core.AgentItem
import app.g7link.core.AgentItemKind
import app.g7link.core.AgentMode
import app.g7link.core.AgentOutcome
import app.g7link.core.AgentPage
import app.g7link.core.AgentPlanStatus
import app.g7link.core.AgentPlanStep
import app.g7link.core.AgentSession
import app.g7link.core.AgentSessionPage
import app.g7link.core.AgentSessionSource
import app.g7link.core.AgentSessionState
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState
import app.g7link.core.AgentTool
import app.g7link.core.AgentToolStatus
import app.g7link.core.Event
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class AgentsModelTest {
    private val pc = "pc1"

    private fun thread(
        id: String = "t1",
        state: AgentThreadState = AgentThreadState.WORKING,
        updatedMs: Long = 1_000L,
        asks: UInt = 0u,
        error: String? = null,
        fromPhone: Boolean = false,
    ) = AgentThread(
        id = id,
        agent = "claude",
        folder = "g7linux",
        title = "Fix the test",
        state = state,
        mode = AgentMode.ASK,
        fromPhone = fromPhone,
        updatedMs = updatedMs,
        asks = asks,
        summary = null,
        error = error,
    )

    private fun item(
        seq: ULong,
        id: String = "i$seq",
        kind: AgentItemKind = AgentItemKind.ASSISTANT,
        text: String? = "text $seq",
        tool: AgentTool? = null,
    ) = AgentItem(seq = seq, id = id, kind = kind, whenMs = seq.toLong(), text = text, truncated = false, tool = tool)

    private fun ask(
        token: String = "a1",
        threadId: String = "t1",
        kind: AgentAskKind = AgentAskKind.COMMAND,
        command: String? = "cargo test",
        truncated: Boolean = false,
        expiresMs: Long = 10_000L,
    ) = AgentAsk(
        token = token,
        thread = threadId,
        threadTitle = "Fix the test",
        agent = "claude",
        kind = kind,
        title = "Run command",
        command = command,
        diff = null,
        path = null,
        reason = null,
        truncated = truncated,
        expiresMs = expiresMs,
    )

    private fun catalog() = AgentCatalog(
        agents = listOf(
            AgentInfo(id = "claude", name = "Claude", installed = true, signedIn = true, loginHint = "claude auth login"),
            AgentInfo(id = "codex", name = "Codex", installed = true, signedIn = false, loginHint = "codex login"),
        ),
        folders = listOf(AgentFolder(id = "f0123456789ab", label = "g7linux")),
    )

    private fun page(thread: AgentThread, items: List<AgentItem>, moreBefore: Boolean = false, asks: List<AgentAsk> = emptyList()) =
        AgentPage(thread = thread, items = items, moreBefore = moreBefore, asks = asks)

    /** A state with t1 open and a first page read. */
    private fun opened(items: List<AgentItem> = listOf(item(1u), item(2u))): AgentsState {
        val open = AgentsModel.opened(AgentsState(catalog = catalog()), "t1")
        return AgentsModel.withPage(open, page(thread(), items))
    }

    private fun reduce(state: AgentsState, event: Event): AgentsUpdate = AgentsModel.reduce(state, event)

    @Test
    fun `the catalog is kept and a missing one clears everything`() {
        var state = reduce(AgentsState(), Event.AgentCatalog(pc, catalog())).state
        assertEquals(listOf("Claude"), state.readyAgents.map { it.name })
        assertEquals("Codex", state.agentName("codex"))
        assertEquals("Other", state.agentName("other"))
        state = reduce(state, Event.AgentThreadChanged(pc, thread())).state
        state = reduce(state, Event.AgentAsk(pc, ask())).state
        state = AgentsModel.opened(state, "t1")

        val cleared = reduce(state, Event.AgentCatalog(pc, null)).state
        assertNull(cleared.catalog)
        assertTrue(cleared.threads.isEmpty())
        assertTrue(cleared.asks.isEmpty())
        assertNull(cleared.open)
    }

    @Test
    fun `threads needing an answer sort first, then the newest`() {
        val list = AgentsModel.withThreads(
            AgentsState(),
            listOf(
                thread("old", AgentThreadState.DONE, updatedMs = 1L),
                thread("new", AgentThreadState.DONE, updatedMs = 9L),
                thread("asks", AgentThreadState.WAITING, updatedMs = 2L, asks = 1u),
                thread("mid", AgentThreadState.WORKING, updatedMs = 5L),
            ),
        )
        assertEquals(listOf("asks", "new", "mid", "old"), list.threads.map { it.id })

        // A change moves a thread and replaces it in place.
        val changed = reduce(list, Event.AgentThreadChanged(pc, thread("old", AgentThreadState.WAITING, updatedMs = 1L, asks = 1u))).state
        assertEquals(listOf("asks", "old", "new", "mid"), changed.threads.map { it.id })
        assertEquals(4, changed.threads.size)
    }

    @Test
    fun `a removed thread goes with its requests and marks the open transcript`() {
        var state = opened()
        state = reduce(state, Event.AgentAsk(pc, ask("a1"))).state
        state = reduce(state, Event.AgentAsk(pc, ask("a2", threadId = "t2"))).state
        state = reduce(state, Event.AgentThreadRemoved(pc, "t1")).state
        assertNull(state.thread("t1"))
        assertEquals(setOf("a2"), state.asks.keys)
        assertTrue(state.open!!.removed)
    }

    @Test
    fun `deltas append in order and the finished item replaces them`() {
        var state = opened()
        state = reduce(state, Event.AgentDelta(pc, "t1", "m3", 3u, 0u, "Hello")).state
        state = reduce(state, Event.AgentDelta(pc, "t1", "m3", 3u, 5u, ", wörld")).state
        val streaming = state.open!!.streaming["m3"]!!
        assertEquals("Hello, wörld", streaming.text)
        // "ö" is two bytes: offsets count UTF-8 bytes.
        assertEquals(13, streaming.bytes)
        val next = reduce(state, Event.AgentDelta(pc, "t1", "m3", 3u, 13u, "!"))
        assertNull(next.reread)
        assertEquals("Hello, wörld!", next.state.open!!.streaming["m3"]!!.text)

        // The timeline gets the text so far as an answer still arriving.
        val arriving = AgentsModel.timelineInput(next.state, "t1", emptySet())!!.streaming.single()
        assertEquals(Triple("m3", 3uL, "Hello, wörld!"), Triple(arriving.itemId, arriving.seq, arriving.text))
        assertEquals(AgentItemKind.ASSISTANT, arriving.kind)

        val done = reduce(next.state, Event.AgentItemChanged(pc, "t1", item(3u, id = "m3", text = "Hello, wörld!"))).state
        assertTrue(done.open!!.streaming.isEmpty())
        assertEquals(listOf(1uL, 2uL, 3uL), done.open!!.items.map { it.seq })
        assertEquals("Hello, wörld!", done.open!!.items.last().text)
        assertTrue(AgentsModel.timelineInput(done, "t1", emptySet())!!.streaming.isEmpty())

        // A delta after the finished item changes nothing.
        val late = reduce(done, Event.AgentDelta(pc, "t1", "m3", 3u, 14u, "?"))
        assertSame(done, late.state)
    }

    @Test
    fun `a delta gap stops the text and asks to read again, once`() {
        var state = opened()
        state = reduce(state, Event.AgentDelta(pc, "t1", "m3", 3u, 0u, "Hello")).state
        val gap = reduce(state, Event.AgentDelta(pc, "t1", "m3", 3u, 9u, "lost"))
        assertEquals("t1", gap.reread)
        val text = gap.state.open!!.streaming["m3"]!!
        assertTrue(text.gap)
        assertEquals("Hello", text.text)
        // Later pieces of the same message are ignored, and don't ask again.
        val after = reduce(gap.state, Event.AgentDelta(pc, "t1", "m3", 3u, 13u, "more"))
        assertNull(after.reread)
        assertSame(gap.state, after.state)
    }

    @Test
    fun `a repeated delta is ignored`() {
        var state = opened()
        state = reduce(state, Event.AgentDelta(pc, "t1", "m3", 3u, 0u, "Hello")).state
        val again = reduce(state, Event.AgentDelta(pc, "t1", "m3", 3u, 0u, "Hello"))
        assertNull(again.reread)
        assertSame(state, again.state)
    }

    @Test
    fun `joining a message part way waits for the finished item`() {
        val state = opened()
        val joined = reduce(state, Event.AgentDelta(pc, "t1", "m3", 3u, 40u, "middle"))
        assertNull("the seq follows on, so nothing was missed", joined.reread)
        val text = joined.state.open!!.streaming["m3"]!!
        assertTrue(text.gap)
        assertEquals("", text.text)
    }

    @Test
    fun `a seq past the next one asks to read again`() {
        val state = opened()
        val skipped = reduce(state, Event.AgentItemChanged(pc, "t1", item(5u)))
        assertEquals("t1", skipped.reread)
        assertEquals(listOf(1uL, 2uL, 5uL), skipped.state.open!!.items.map { it.seq })

        val deltaSkipped = reduce(state, Event.AgentDelta(pc, "t1", "m9", 9u, 0u, "Hi"))
        assertEquals("t1", deltaSkipped.reread)

        // The next seq, or an update of one it has, is no gap.
        assertNull(reduce(state, Event.AgentItemChanged(pc, "t1", item(3u))).reread)
        val tool = AgentTool(name = "Bash", title = "Run command", detail = "ls", status = AgentToolStatus.OK, output = "a b")
        val updated = reduce(state, Event.AgentItemChanged(pc, "t1", item(2u, kind = AgentItemKind.TOOL, text = null, tool = tool)))
        assertNull(updated.reread)
        assertEquals(AgentToolStatus.OK, updated.state.open!!.items[1].tool!!.status)
        assertEquals(2, updated.state.open!!.items.size)
    }

    @Test
    fun `events for a thread that isn't open don't touch the transcript`() {
        val state = opened()
        assertSame(state, reduce(state, Event.AgentDelta(pc, "t2", "m3", 3u, 0u, "Hi")).state)
        assertSame(state, reduce(state, Event.AgentItemChanged(pc, "t2", item(3u))).state)
    }

    @Test
    fun `a request answered anywhere is removed and kept for its mark`() {
        var state = opened()
        state = reduce(state, Event.AgentAsk(pc, ask("a1"))).state
        state = reduce(state, Event.AgentAsk(pc, ask("a2", expiresMs = 20_000L))).state
        assertEquals(listOf("a1", "a2"), state.asksFor("t1").map { it.token })

        // Answered here.
        state = AgentsModel.answering(state, "a1")
        assertEquals(setOf("a1"), state.answering)
        state = reduce(state, Event.AgentAsked(pc, "a1", "t1", AgentAskOutcome.ALLOWED)).state
        assertEquals(listOf("a2"), state.asksFor("t1").map { it.token })
        assertTrue(state.answering.isEmpty())
        val here = state.open!!.resolved.single()
        assertTrue(here.here)
        assertEquals(AgentAskOutcome.ALLOWED, here.outcome)

        // Answered on the PC.
        state = reduce(state, Event.AgentAsked(pc, "a2", "t1", AgentAskOutcome.DENIED)).state
        assertTrue(state.asks.isEmpty())
        val elsewhere = state.open!!.resolved.last()
        assertFalse(elsewhere.here)
        assertEquals(AgentAskOutcome.DENIED, elsewhere.outcome)
        assertEquals(listOf("a1", "a2"), state.open!!.resolved.map { it.token })
    }

    @Test
    fun `an answer that failed can be given again, and one that's gone is removed`() {
        var state = reduce(opened(), Event.AgentAsk(pc, ask("a1"))).state
        state = AgentsModel.answering(state, "a1")
        val retry = AgentsModel.answerFailed(state, "a1", AgentFailure.OFFLINE)
        assertTrue(retry.answering.isEmpty())
        assertNotNull(retry.asks["a1"])
        val gone = AgentsModel.answerFailed(state, "a1", AgentFailure.NOT_FOUND)
        assertTrue(gone.asks.isEmpty())
        assertTrue(gone.answering.isEmpty())
        // Unknown tokens aren't marked.
        assertTrue(AgentsModel.answering(opened(), "zz").answering.isEmpty())
    }

    @Test
    fun `a page replaces the transcript and the thread's requests`() {
        var state = AgentsModel.opened(AgentsState(), "t1")
        state = reduce(state, Event.AgentAsk(pc, ask("stale"))).state
        state = reduce(state, Event.AgentAsk(pc, ask("other", threadId = "t2"))).state
        state = AgentsModel.withPage(
            state,
            page(thread(state = AgentThreadState.WAITING, asks = 1u), listOf(item(3u), item(2u), item(2u)), moreBefore = true, asks = listOf(ask("fresh"))),
        )
        val open = state.open!!
        assertTrue(open.loaded)
        assertTrue(open.moreBefore)
        assertEquals(listOf(2uL, 3uL), open.items.map { it.seq })
        assertEquals(setOf("fresh", "other"), state.asks.keys)
        assertEquals(AgentThreadState.WAITING, state.thread("t1")!!.state)

        // An older page goes in front.
        state = AgentsModel.withOlder(state, page(thread(state = AgentThreadState.WAITING), listOf(item(1u), item(2u))))
        assertEquals(listOf(1uL, 2uL, 3uL), state.open!!.items.map { it.seq })
        assertFalse(state.open!!.moreBefore)
    }

    @Test
    fun `a page keeps text that is still streaming past its end`() {
        var state = opened()
        state = reduce(state, Event.AgentDelta(pc, "t1", "m3", 3u, 0u, "Hel")).state
        state = AgentsModel.withPage(state, page(thread(), listOf(item(1u), item(2u))))
        assertEquals("Hel", state.open!!.streaming["m3"]!!.text)
    }

    @Test
    fun `the end of a turn with text still streaming asks to read again`() {
        var state = opened()
        state = reduce(state, Event.AgentDelta(pc, "t1", "m3", 3u, 0u, "Hel")).state
        val done = reduce(state, Event.AgentDone(pc, thread(state = AgentThreadState.DONE), AgentOutcome.DONE, "Hel"))
        assertEquals("t1", done.reread)
        assertEquals(AgentThreadState.DONE, done.state.thread("t1")!!.state)
        assertNull(reduce(opened(), Event.AgentDone(pc, thread(state = AgentThreadState.DONE), AgentOutcome.DONE, "")).reread)
    }

    @Test
    fun `leaving the transcript drops its items`() {
        val state = opened()
        assertNull(AgentsModel.closed(state, "t1").open)
        assertSame(state, AgentsModel.closed(state, "t2"))
        assertSame(state, AgentsModel.opened(state, "t1"))
    }

    @Test
    fun `failures read as paused for a usage limit and failed otherwise`() {
        assertEquals(ThreadStatus.Paused(1_795_050_000_000L), AgentsModel.failure("Usage limit reached|resets_ms=1795050000000"))
        assertEquals(ThreadStatus.Paused(null), AgentsModel.failure("Claude usage limit reached"))
        assertEquals(ThreadStatus.Failed("Not logged in"), AgentsModel.failure("Not logged in"))
        assertEquals(ThreadStatus.Failed(null), AgentsModel.failure(null))
        // The host's code wins over the message.
        assertEquals(ThreadStatus.Paused(null), AgentsModel.failure("Rate limited", "usage_limit"))
        assertEquals(ThreadStatus.Failed("Not logged in · Please run /login", login = true), AgentsModel.failure("Not logged in · Please run /login", "login"))
        assertEquals(ThreadStatus.Failed("usage limit of the API key", login = false), AgentsModel.failure("usage limit of the API key", "api_error"))
        assertEquals("Usage limit reached", AgentsModel.shownError("Usage limit reached|resets_ms=1795050000000"))
        assertEquals(ThreadStatus.NeedsYou, AgentsModel.status(thread(state = AgentThreadState.WAITING)))
        assertEquals(ThreadStatus.Stopped, AgentsModel.status(thread(state = AgentThreadState.INTERRUPTED)))
        assertEquals(ThreadStatus.Failed("boom"), AgentsModel.status(thread(state = AgentThreadState.FAILED, error = "boom")))
    }

    @Test
    fun `prompts fit in 8000 characters and 8192 bytes`() {
        assertFalse(AgentsModel.fits("   "))
        assertTrue(AgentsModel.fits("a".repeat(8000)))
        assertFalse(AgentsModel.fits("a".repeat(8001)))
        // 3 bytes each: under 8000 characters, over 8192 bytes.
        assertFalse(AgentsModel.fits("€".repeat(2731)))
        assertTrue(AgentsModel.fits("€".repeat(2730)))
        assertTrue(AgentsModel.tooLong("€".repeat(2731)))
        assertEquals(4, AgentsModel.utf8Length("😀"))
        assertEquals("€".toByteArray(Charsets.UTF_8).size, AgentsModel.utf8Length("€"))
    }

    @Test
    fun `only a whole one-line command can be allowed from a notification`() {
        assertTrue(AgentsModel.allowableInNotification(ask(command = "npm run test -- settings")))
        assertFalse(AgentsModel.allowableInNotification(ask(command = "a".repeat(101))))
        assertFalse(AgentsModel.allowableInNotification(ask(command = "cd x\nrm -rf y")))
        assertFalse(AgentsModel.allowableInNotification(ask(command = "ls", truncated = true)))
        assertFalse(AgentsModel.allowableInNotification(ask(kind = AgentAskKind.EDIT, command = null)))
        assertFalse(AgentsModel.allowableInNotification(ask(command = null)))
    }

    @Test
    fun `agent events name their PC and others don't`() {
        assertEquals(pc, AgentsModel.pcOf(Event.AgentAsked(pc, "a", "t", AgentAskOutcome.EXPIRED)))
        assertEquals(pc, AgentsModel.pcOf(Event.AgentCatalog(pc, null)))
        assertNull(AgentsModel.pcOf(Event.ReconnectChanged(pc, true)))
    }

    // ---- 0.1.14: sessions started on the PC, steps, plan, Home ----

    private fun session(
        id: String = "sc0123456789abcdef0123456789abcdef",
        state: AgentSessionState = AgentSessionState.WORKING,
        guess: Boolean = false,
        updatedMs: Long = 1_000L,
        canContinue: Boolean = false,
    ) = AgentSession(
        id = id,
        agent = "claude",
        folder = "g7linux",
        title = "Fix the settings import",
        state = state,
        guess = guess,
        source = AgentSessionSource.TERMINAL,
        updatedMs = updatedMs,
        step = null,
        summary = null,
        waitingFor = null,
        canContinue = canContinue,
        blocked = null,
    )

    private fun tool(seq: ULong, status: AgentToolStatus, title: String = "Run command") =
        item(seq, kind = AgentItemKind.TOOL, text = null, tool = AgentTool(name = "Bash", title = title, detail = "cargo test", status = status, output = null))

    private val idA = "sc0123456789abcdef0123456789abcdef"
    private val idB = "sxfedcba9876543210fedcba9876543210"

    @Test
    fun `session events add, replace and remove sessions, and mean the PC lists them`() {
        var state = AgentsState(catalog = catalog())
        assertNull(state.sessionsAllowed)
        state = reduce(state, Event.AgentSessionChanged(pc, session(idA))).state
        assertEquals(true, state.sessionsAllowed)
        assertEquals(listOf(idA), state.sessions.map { it.id })

        state = reduce(state, Event.AgentSessionChanged(pc, session(idA, state = AgentSessionState.IDLE))).state
        assertEquals(AgentSessionState.IDLE, state.sessions.single().state)

        state = reduce(state, Event.AgentSessionChanged(pc, session(idB, state = AgentSessionState.ENDED))).state
        state = reduce(state, Event.AgentSessionRemoved(pc, idA)).state
        assertEquals(listOf(idB), state.sessions.map { it.id })
        assertEquals(pc, AgentsModel.pcOf(Event.AgentSessionRemoved(pc, idA)))
        assertEquals(pc, AgentsModel.pcOf(Event.AgentSessionItem(pc, idA, item(1u))))
    }

    @Test
    fun `sessions sort like the PC's list, and a guess sorts with may need you`() {
        val sorted = AgentsModel.sortedSessions(
            listOf(
                session("sc01", AgentSessionState.ENDED, updatedMs = 9_000L),
                session("sc02", AgentSessionState.IDLE, updatedMs = 8_000L),
                session("sc03", AgentSessionState.NEEDS_YOU, guess = true, updatedMs = 7_000L),
                session("sc04", AgentSessionState.WORKING, updatedMs = 6_000L),
                session("sc05", AgentSessionState.NEEDS_YOU, updatedMs = 1_000L),
                session("sc06", AgentSessionState.WORKING, updatedMs = 6_500L),
            ),
        )
        assertEquals(listOf("sc05", "sc06", "sc04", "sc03", "sc02", "sc01"), sorted.map { it.id })
        assertTrue(AgentsModel.mayNeedYou(session(state = AgentSessionState.MAY_NEED_YOU)))
        assertTrue(AgentsModel.mayNeedYou(session(state = AgentSessionState.NEEDS_YOU, guess = true)))
        assertFalse(AgentsModel.needsYouExactly(session(state = AgentSessionState.NEEDS_YOU, guess = true)))
        assertTrue(AgentsModel.needsYouExactly(session(state = AgentSessionState.NEEDS_YOU)))
    }

    @Test
    fun `a switch that's off, or a PC that doesn't list sessions, is remembered`() {
        val listed = AgentsModel.withSessions(AgentsState(catalog = catalog()), listOf(session(idA)))
        val off = AgentsModel.sessionsOff(listed)
        assertEquals(false, off.sessionsAllowed)
        assertTrue(off.sessions.isEmpty())
        val unsupported = AgentsModel.sessionsUnsupported(listed)
        assertNull(unsupported.sessionsAllowed)
        assertTrue(unsupported.sessionsUnsupported)
        // Not asked yet: neither listed nor unsupported, so the list offers to ask.
        assertNull(AgentsState().sessionsAllowed)
        assertFalse(AgentsState().sessionsUnsupported)
        assertFalse(AgentsModel.withSession(unsupported, session(idA)).sessionsUnsupported)
    }

    @Test
    fun `the followed session takes its page and then its items, and others are ignored`() {
        var state = AgentsModel.sessionOpened(AgentsState(catalog = catalog()), idA)
        state = AgentsModel.withSessionPage(state, AgentSessionPage(session(idA), listOf(item(64u), item(128u)), moreBefore = true))
        val open = state.openSession!!
        assertTrue(open.loaded)
        assertTrue(open.moreBefore)
        assertEquals(listOf(64uL, 128uL), open.items.map { it.seq })

        // Seqs aren't consecutive: a far one is simply added, an update replaces in place.
        state = reduce(state, Event.AgentSessionItem(pc, idA, item(4096u))).state
        state = reduce(state, Event.AgentSessionItem(pc, idA, item(64u, text = "changed"))).state
        assertEquals(listOf(64uL, 128uL, 4096uL), state.openSession!!.items.map { it.seq })
        assertEquals("changed", state.openSession!!.items.first().text)

        val other = reduce(state, Event.AgentSessionItem(pc, idB, item(9u))).state
        assertSame(state, other)

        val removed = reduce(state, Event.AgentSessionRemoved(pc, idA)).state
        assertTrue(removed.openSession!!.removed)
        assertNull(AgentsModel.sessionClosed(removed, idA).openSession)
    }

    @Test
    fun `an older session page goes in front`() {
        var state = AgentsModel.sessionOpened(AgentsState(), idA)
        state = AgentsModel.withSessionPage(state, AgentSessionPage(session(idA), listOf(item(640u)), moreBefore = true))
        state = AgentsModel.withOlderSessionPage(state, AgentSessionPage(session(idA), listOf(item(64u), item(640u)), moreBefore = false))
        assertEquals(listOf(64uL, 640uL), state.openSession!!.items.map { it.seq })
        assertFalse(state.openSession!!.moreBefore)
    }

    @Test
    fun `sessions are cleared with the catalog`() {
        var state = reduce(AgentsState(), Event.AgentCatalog(pc, catalog())).state
        state = reduce(state, Event.AgentSessionChanged(pc, session(idA))).state
        state = AgentsModel.sessionOpened(state, idA)
        val cleared = reduce(state, Event.AgentCatalog(pc, null)).state
        assertTrue(cleared.sessions.isEmpty())
        assertNull(cleared.sessionsAllowed)
        assertNull(cleared.openSession)
    }

    @Test
    fun `the tool still running is the last one that runs`() {
        val running = AgentsModel.withPage(
            AgentsModel.opened(AgentsState(), "t1"),
            page(thread(), listOf(tool(1u, AgentToolStatus.OK), tool(2u, AgentToolStatus.RUNNING), item(3u))),
        ).open!!
        assertEquals(2uL, AgentsModel.runningTool(running.items)?.seq)
        assertNull(AgentsModel.runningTool(listOf(tool(1u, AgentToolStatus.OK))))
    }

    @Test
    fun `a plan says how much is done and which step is under way`() {
        val plan = listOf(
            AgentPlanStep("Reproduce", AgentPlanStatus.COMPLETED),
            AgentPlanStep("Find it", AgentPlanStatus.COMPLETED),
            AgentPlanStep("Fix it", AgentPlanStatus.COMPLETED),
            AgentPlanStep("Run the tests", AgentPlanStatus.IN_PROGRESS),
            AgentPlanStep("Say why", AgentPlanStatus.PENDING),
        )
        assertEquals(3, AgentsModel.planDone(plan))
        assertEquals(4, AgentsModel.planPosition(plan))
        assertEquals(1, AgentsModel.planPosition(listOf(AgentPlanStep("a", AgentPlanStatus.PENDING))))
        assertEquals(2, AgentsModel.planPosition(listOf(AgentPlanStep("a", AgentPlanStatus.COMPLETED), AgentPlanStep("b", AgentPlanStatus.COMPLETED))))
        assertNull(AgentsModel.planPosition(null))
        assertNull(AgentsModel.planPosition(emptyList()))
    }

    @Test
    fun `elapsed time reads like a clock`() {
        assertEquals("0:00", AgentsModel.elapsed(-5_000L))
        assertEquals("0:42", AgentsModel.elapsed(42_000L))
        assertEquals("4:12", AgentsModel.elapsed(252_999L))
        assertEquals("1:05:09", AgentsModel.elapsed(3_909_000L))
    }

    @Test
    fun `home's one line says a request first, then a session that needs you, then what works, then the last one, then start`() {
        val ready = AgentsState(catalog = catalog())
        val working = thread("t2", AgentThreadState.WORKING, updatedMs = 5_000L).copy(sinceMs = 4_000L)
        val waiting = thread("t1", AgentThreadState.WAITING, updatedMs = 2_000L)

        // A request, with a thread working beside it: the request, and who asks.
        var state = AgentsModel.withThreads(ready, listOf(waiting, working))
        state = reduce(state, Event.AgentAsk(pc, ask())).state
        assertEquals(HomeLine.NeedsYou("claude", 1), AgentsModel.homeLine(state))

        // A session that needs you, as the agent reported, when no thread does.
        state = AgentsModel.withThreads(ready, listOf(working))
        state = AgentsModel.withSessions(state, listOf(session(idA, AgentSessionState.NEEDS_YOU)))
        assertEquals(HomeLine.NeedsYou("claude", 1), AgentsModel.homeLine(state))

        // Working: one says its agent and since when; more say how many.
        assertEquals(HomeLine.Working("claude", 4_000L, 1, sessions = false), AgentsModel.homeLine(AgentsModel.withThreads(ready, listOf(working))))
        val three = listOf(working, thread("t3", updatedMs = 4_000L), thread("t4", updatedMs = 3_000L))
        assertEquals(3, (AgentsModel.homeLine(AgentsModel.withThreads(ready, three)) as HomeLine.Working).count)

        // A session at work, when no thread works.
        val sessions = AgentsModel.homeLine(AgentsModel.withSessions(ready, listOf(session(idB, AgentSessionState.WORKING))))
        assertEquals(HomeLine.Working("claude", null, 1, sessions = true), sessions)

        // Nothing needs you or works: the thread that ran last, and how it ended.
        val done = thread("t5", AgentThreadState.DONE, updatedMs = 9_000L)
        val failed = thread("t6", AgentThreadState.FAILED, updatedMs = 1_000L, error = "boom")
        assertEquals(HomeLine.Finished("Fix the test", ThreadStatus.Done, 9_000L), AgentsModel.homeLine(AgentsModel.withThreads(ready, listOf(failed, done))))

        // Nothing at all yet.
        assertEquals(HomeLine.Start, AgentsModel.homeLine(ready))
        assertEquals(HomeLine.Start, AgentsModel.homeLine(AgentsState()))
    }

    @Test
    fun `a thread shows one request at a time, the oldest, and how many more wait`() {
        var state = AgentsModel.withThreads(AgentsState(catalog = catalog()), listOf(thread("t1", AgentThreadState.WAITING, asks = 3u)))
        assertNull(state.askQueue("t1"))
        state = reduce(state, Event.AgentAsk(pc, ask("a2", command = "rg --files", expiresMs = 20_000L))).state
        state = reduce(state, Event.AgentAsk(pc, ask("a1", command = "pnpm exec tsc --noEmit", expiresMs = 10_000L))).state
        state = reduce(state, Event.AgentAsk(pc, ask("a3", command = "cargo test", expiresMs = 30_000L))).state
        var queue = state.askQueue("t1")!!
        assertEquals("a1", queue.shown.token)
        assertEquals("pnpm exec tsc --noEmit", queue.shown.command)
        assertEquals(2, queue.more)
        // "1 of 3".
        assertEquals(1 to 3, queue.position to queue.total)
        assertFalse(queue.held)

        // Answering it shows the next.
        state = reduce(state, Event.AgentAsked(pc, "a1", "t1", AgentAskOutcome.DENIED)).state
        queue = state.askQueue("t1")!!
        assertEquals("a2", queue.shown.token)
        assertEquals(1, queue.more)
        assertEquals(1 to 2, queue.position to queue.total)
        assertNull(state.askQueue("other"))
    }

    @Test
    fun `home counts every request waiting, the threads' own counts when the phone hasn't heard of them all`() {
        val ready = AgentsState(catalog = catalog())
        var state = AgentsModel.withThreads(
            ready,
            listOf(thread("t1", AgentThreadState.WAITING, asks = 2u), thread("t2", AgentThreadState.WAITING, asks = 1u)),
        )
        assertEquals(3, (AgentsModel.homeLine(state) as HomeLine.NeedsYou).count)

        state = reduce(state, Event.AgentAsk(pc, ask("a1", threadId = "t1", expiresMs = 10_000L))).state
        assertEquals(HomeLine.NeedsYou("claude", 3), AgentsModel.homeLine(state))

        // A session waiting on the PC counts too.
        state = AgentsModel.withSessions(state, listOf(session(idA, AgentSessionState.NEEDS_YOU)))
        assertEquals(4, (AgentsModel.homeLine(state) as HomeLine.NeedsYou).count)

        // One waiting request alone.
        state = AgentsModel.withThreads(ready, listOf(thread("t1", AgentThreadState.WAITING, asks = 1u)))
        state = reduce(state, Event.AgentAsk(pc, ask("a1", threadId = "t1"))).state
        assertEquals(1, (AgentsModel.homeLine(state) as HomeLine.NeedsYou).count)
    }

    @Test
    fun `a guessed needs-you is never home's line`() {
        val ready = AgentsState(catalog = catalog())
        for (guessed in listOf(session(idA, AgentSessionState.NEEDS_YOU, guess = true), session(idA, AgentSessionState.MAY_NEED_YOU))) {
            assertEquals(HomeLine.Start, AgentsModel.homeLine(AgentsModel.withSessions(ready, listOf(guessed))))
        }
    }
}
