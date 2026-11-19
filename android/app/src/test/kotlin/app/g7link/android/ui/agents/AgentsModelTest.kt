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
        mode = AgentMode.SUPERVISED,
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
        modes = listOf(AgentMode.SUPERVISED),
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

        val entries = AgentsModel.transcript(next.state.open!!)
        assertEquals(listOf("seq:1", "seq:2", "seq:3"), entries.map { it.key })
        assertTrue(entries.last() is TranscriptEntry.Streaming)

        val done = reduce(next.state, Event.AgentItemChanged(pc, "t1", item(3u, id = "m3", text = "Hello, wörld!"))).state
        assertTrue(done.open!!.streaming.isEmpty())
        val finished = AgentsModel.transcript(done.open!!)
        assertEquals(listOf("seq:1", "seq:2", "seq:3"), finished.map { it.key })
        assertEquals("Hello, wörld!", (finished.last() as TranscriptEntry.Item).item.text)

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
    fun `a request answered anywhere is removed and shown collapsed`() {
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
        assertEquals(listOf("seq:1", "seq:2", "ask:a1", "ask:a2"), AgentsModel.transcript(state.open!!).map { it.key })
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
}
