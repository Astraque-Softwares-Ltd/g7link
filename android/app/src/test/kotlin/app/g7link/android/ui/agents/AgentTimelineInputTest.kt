// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentAsk
import app.g7link.core.AgentAskKind
import app.g7link.core.AgentAskOutcome
import app.g7link.core.AgentCatalog
import app.g7link.core.AgentItem
import app.g7link.core.AgentItemKind
import app.g7link.core.AgentMarkKind
import app.g7link.core.AgentMode
import app.g7link.core.AgentPage
import app.g7link.core.AgentRow
import app.g7link.core.AgentRowKind
import app.g7link.core.AgentSession
import app.g7link.core.AgentSessionPage
import app.g7link.core.AgentSessionSource
import app.g7link.core.AgentSessionState
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState
import app.g7link.core.Event
import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/** What the phone hands the FFI's timeline, when it hands it again, and which rows it keeps (contract §P6.1, §P10.5). */
class AgentTimelineInputTest {
    private val pc = "pc1"

    private fun thread(id: String = "t1", state: AgentThreadState = AgentThreadState.WORKING, updatedMs: Long = 1_000L, sinceMs: Long? = 500L) = AgentThread(
        id = id,
        agent = "claude",
        folder = "g7linux-phone",
        title = "Fix the test",
        state = state,
        mode = AgentMode.ASK,
        fromPhone = true,
        updatedMs = updatedMs,
        asks = 0u,
        summary = null,
        error = null,
        sinceMs = sinceMs,
    )

    private fun item(seq: ULong, id: String = "i$seq", kind: AgentItemKind = AgentItemKind.ASSISTANT, text: String? = "text $seq", endMs: Long? = null) =
        AgentItem(seq = seq, id = id, kind = kind, whenMs = seq.toLong(), text = text, truncated = false, tool = null, turn = 1u, endMs = endMs)

    private fun ask(token: String, item: String?, threadId: String = "t1", expiresMs: Long = 10_000L) = AgentAsk(
        token = token,
        thread = threadId,
        threadTitle = "Fix the test",
        agent = "claude",
        kind = AgentAskKind.COMMAND,
        title = "Run command",
        command = "cargo test",
        diff = null,
        path = null,
        reason = null,
        truncated = false,
        expiresMs = expiresMs,
        item = item,
    )

    private fun opened(items: List<AgentItem> = listOf(item(1u, kind = AgentItemKind.USER), item(2u))): AgentsState {
        val catalog = AgentCatalog(agents = emptyList(), folders = emptyList())
        val open = AgentsModel.opened(AgentsState(catalog = catalog), "t1")
        return AgentsModel.withPage(open, AgentPage(thread = thread(), items = items, moreBefore = false, asks = emptyList()))
    }

    private fun input(state: AgentsState, open: Set<String> = emptySet(), held: (String) -> Boolean = { false }) =
        AgentsModel.timelineInput(state, "t1", open, held)!!

    // ---- The inputs and their triggers ----

    @Test
    fun `no rows before the first page`() {
        assertNull(AgentsModel.timelineInput(AgentsModel.opened(AgentsState(), "t1"), "t1", emptySet()))
        assertNull(AgentsModel.timelineInput(opened(), "t2", emptySet()))
    }

    @Test
    fun `the input carries the items, the state and since when, and the rows opened`() {
        val state = opened()
        val input = input(state, open = setOf("g:i3", "f:1"))
        assertEquals("t1", input.key)
        assertSame(state.open!!.items, input.items)
        assertEquals(AgentThreadState.WORKING, input.state)
        assertEquals(500L, input.stateSinceMs)
        assertEquals(listOf("f:1", "g:i3"), input.open)
    }

    @Test
    fun `rows are made again on an item, a delta, the state, a request, a tap or a hold, and not otherwise`() {
        val state = opened()
        val before = input(state)

        // Nothing the rows show: another thread, the catalog, the thread's own time.
        val other = AgentsModel.reduce(state, Event.AgentThreadChanged(pc, thread("t2", updatedMs = 9_000L))).state
        assertEquals(before, input(other))
        val catalog = AgentsModel.reduce(state, Event.AgentCatalog(pc, AgentCatalog(agents = emptyList(), folders = emptyList(), canSayWhy = true))).state
        assertEquals(before, input(catalog))
        val touched = AgentsModel.withThread(state, thread(updatedMs = 5_000L))
        assertEquals(before, input(touched))

        // An item.
        assertNotEquals(before, input(AgentsModel.reduce(state, Event.AgentItemChanged(pc, "t1", item(3u))).state))
        // A delta.
        val delta = AgentsModel.reduce(state, Event.AgentDelta(pc, "t1", "m3", 3u, 0u, "Hel")).state
        assertNotEquals(before, input(delta))
        assertTrue(RowsSchedule.onlyStreamingChanged(before, input(delta)))
        // The state.
        assertNotEquals(before, input(AgentsModel.withThread(state, thread(state = AgentThreadState.DONE))))
        // A request.
        val asked = AgentsModel.reduce(state, Event.AgentAsk(pc, ask("a1", item = "i2"))).state
        assertNotEquals(before, input(asked))
        // A tap.
        assertNotEquals(before, input(state, open = setOf("f:1")))
        // A hold.
        assertNotEquals(input(asked), input(asked) { it == "a1" })
    }

    // ---- Thinking ----

    @Test
    fun `a thought arrives as deltas after its item, and its end replaces them`() {
        val thinking = item(3u, id = "rs_1", kind = AgentItemKind.REASONING, text = "")
        var state = AgentsModel.reduce(opened(), Event.AgentItemChanged(pc, "t1", thinking)).state
        state = AgentsModel.reduce(state, Event.AgentDelta(pc, "t1", "rs_1", 3u, 0u, "**Reading** the worker")).state
        val text = state.open!!.streaming["rs_1"]!!
        assertEquals("**Reading** the worker", text.text)
        assertEquals(AgentItemKind.REASONING, text.kind)
        val streaming = input(state).streaming.single()
        assertEquals(AgentItemKind.REASONING, streaming.kind)

        // A late update of the open thought keeps what arrived.
        state = AgentsModel.reduce(state, Event.AgentItemChanged(pc, "t1", thinking)).state
        assertEquals("**Reading** the worker", state.open!!.streaming["rs_1"]!!.text)

        // Its end: the item has the text, nothing streams.
        val done = item(3u, id = "rs_1", kind = AgentItemKind.REASONING, text = "**Reading** the worker", endMs = 9L)
        state = AgentsModel.reduce(state, Event.AgentItemChanged(pc, "t1", done)).state
        assertTrue(state.open!!.streaming.isEmpty())
        // A delta after it is late.
        assertSame(state, AgentsModel.reduce(state, Event.AgentDelta(pc, "t1", "rs_1", 3u, 22u, "!")).state)
    }

    @Test
    fun `a page with a thought still going on carries on from its text`() {
        val thinking = item(3u, id = "rs_1", kind = AgentItemKind.REASONING, text = "Look")
        var state = opened(listOf(item(1u, kind = AgentItemKind.USER), thinking))
        val seeded = state.open!!.streaming["rs_1"]!!
        assertEquals("Look" to 4, seeded.text to seeded.bytes)
        state = AgentsModel.reduce(state, Event.AgentDelta(pc, "t1", "rs_1", 3u, 4u, " first.")).state
        assertEquals("Look first.", state.open!!.streaming["rs_1"]!!.text)
    }

    @Test
    fun `an empty piece missing its start isn't handed to the timeline`() {
        val joined = AgentsModel.reduce(opened(), Event.AgentDelta(pc, "t1", "m3", 3u, 40u, "middle")).state
        assertTrue(joined.open!!.streaming["m3"]!!.gap)
        assertTrue(input(joined).streaming.isEmpty())
    }

    // ---- Marks ----

    @Test
    fun `requests mark their steps while they wait, held ones as held`() {
        var state = opened()
        state = AgentsModel.reduce(state, Event.AgentAsk(pc, ask("a1", item = "tool-1"))).state
        state = AgentsModel.reduce(state, Event.AgentAsk(pc, ask("a2", item = null, expiresMs = 20_000L))).state
        val marks = AgentsModel.marks(state, "t1") { it == "a2" }
        assertEquals(listOf("tool-1" to AgentMarkKind.WAITING, "" to AgentMarkKind.HELD), marks.map { it.itemId to it.kind })
    }

    @Test
    fun `answers mark their steps only when the mark says something`() {
        var state = opened()
        for ((token, step) in listOf("here" to "s1", "there" to "s2", "late" to "s3", "gone" to "s4", "deniedThere" to "s5", "deniedHere" to "s6")) {
            state = AgentsModel.reduce(state, Event.AgentAsk(pc, ask(token, item = step))).state
        }
        state = AgentsModel.answering(state, "here")
        state = AgentsModel.answering(state, "deniedHere")
        state = AgentsModel.reduce(state, Event.AgentAsked(pc, "here", "t1", AgentAskOutcome.ALLOWED)).state
        state = AgentsModel.reduce(state, Event.AgentAsked(pc, "there", "t1", AgentAskOutcome.ALLOWED, "Pixel 8 Pro")).state
        state = AgentsModel.reduce(state, Event.AgentAsked(pc, "late", "t1", AgentAskOutcome.EXPIRED)).state
        state = AgentsModel.reduce(state, Event.AgentAsked(pc, "gone", "t1", AgentAskOutcome.CANCELLED)).state
        state = AgentsModel.reduce(state, Event.AgentAsked(pc, "deniedThere", "t1", AgentAskOutcome.DENIED, "guchu")).state
        state = AgentsModel.reduce(state, Event.AgentAsked(pc, "deniedHere", "t1", AgentAskOutcome.DENIED)).state
        val marks = AgentsModel.marks(state, "t1").map { Triple(it.itemId, it.kind, it.by) }
        assertEquals(
            listOf(
                Triple("s2", AgentMarkKind.ALLOWED, "Pixel 8 Pro"),
                Triple("s3", AgentMarkKind.EXPIRED, ""),
                Triple("s4", AgentMarkKind.CANCELLED, ""),
                Triple("s5", AgentMarkKind.DENIED, "guchu"),
            ),
            marks,
        )
    }

    @Test
    fun `a followed session's rows come from its items alone`() {
        val session = AgentSession(
            id = "sc01",
            agent = "claude",
            folder = "g7linux",
            title = "Fix it",
            state = AgentSessionState.NEEDS_YOU,
            guess = false,
            source = AgentSessionSource.TERMINAL,
            updatedMs = 1L,
            step = null,
            summary = null,
            waitingFor = null,
            canContinue = false,
            blocked = null,
        )
        var state = AgentsModel.sessionOpened(AgentsState(), "sc01")
        state = AgentsModel.withSessionPage(state, AgentSessionPage(session, listOf(item(64u)), moreBefore = false))
        val input = AgentsModel.sessionTimelineInput(state.openSession!!, session, emptySet())!!
        assertEquals("session:sc01", input.key)
        assertEquals(AgentThreadState.WAITING, input.state)
        assertTrue(input.marks.isEmpty() && input.streaming.isEmpty())
        assertNull(AgentsModel.sessionTimelineInput(OpenSession("sc01"), session, emptySet()))
    }

    // ---- When rows are made, and which are kept ----

    @Test
    fun `text arriving is gathered to one recompute per 100 ms, anything else is made at once`() {
        val state = opened()
        val before = input(state)
        val delta = input(AgentsModel.reduce(state, Event.AgentDelta(pc, "t1", "m3", 3u, 0u, "Hel")).state)
        assertEquals(60L, RowsSchedule.waitMs(before, delta, lastAtMs = 1_000L, nowMs = 1_040L))
        assertEquals(0L, RowsSchedule.waitMs(before, delta, lastAtMs = 1_000L, nowMs = 1_200L))
        // Not only text: at once.
        val tapped = input(state, open = setOf("f:1"))
        assertEquals(0L, RowsSchedule.waitMs(before, tapped, lastAtMs = 1_000L, nowMs = 1_010L))
        // The first rows: at once.
        assertEquals(0L, RowsSchedule.waitMs(null, delta, lastAtMs = 0L, nowMs = 10L))
    }

    @Test
    fun `a result for an input already superseded, or for another thread, is dropped`() {
        val gate = RowsGate("t1")
        val input = input(opened())
        val first = gate.submit()
        val second = gate.submit()
        assertTrue(gate.accept(TimelineRows(input, emptyList(), second)))
        assertFalse("older than the rows shown", gate.accept(TimelineRows(input, emptyList(), first)))
        val third = gate.submit()
        assertFalse("made for another thread", gate.accept(TimelineRows(input.copy(key = "t2"), emptyList(), third)))
        assertTrue(gate.accept(TimelineRows(input, emptyList(), third)))
    }

    @Test
    fun `the feed makes rows for new inputs only and shows the newest`() = runBlocking {
        val made = CopyOnWriteArrayList<TimelineInput>()
        val rows = listOf(AgentRow(key = "live", kind = AgentRowKind.Notice(text = "x"), turn = 1u, depth = 0u, itemId = "", seq = 0uL))
        val feed = RowsFeed("t1", make = { input, _ -> made += input; rows }, worker = Dispatchers.Unconfined)
        val job = feed.start(this)
        val state = opened()
        val first = input(state)
        feed.submit(first)
        withTimeout(2_000L) { feed.rows.filterNotNull().first { it.input == first } }
        feed.submit(first)
        val tapped = input(state, open = setOf("f:1"))
        feed.submit(tapped)
        withTimeout(2_000L) { feed.rows.filterNotNull().first { it.input == tapped } }
        // Two deltas close together: at most one more recompute for them, and it shows the newest.
        val one = input(AgentsModel.reduce(state, Event.AgentDelta(pc, "t1", "m3", 3u, 0u, "Hel")).state, open = setOf("f:1"))
        val two = input(AgentsModel.reduce(state, Event.AgentDelta(pc, "t1", "m3", 3u, 0u, "Hello")).state, open = setOf("f:1"))
        feed.submit(one)
        feed.submit(two)
        withTimeout(2_000L) { feed.rows.filterNotNull().first { it.input == two } }
        delay(50L)
        job.cancelAndJoin()
        assertEquals(listOf(first, tapped, two), made)
        assertEquals(rows, feed.rows.value!!.rows)
    }
}
