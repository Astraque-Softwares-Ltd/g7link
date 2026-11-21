// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentHit
import app.g7link.core.AgentHits
import app.g7link.core.AgentItemKind
import app.g7link.core.AgentSnippetMark
import app.g7link.core.AgentThreadState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/** Search (contract §U9.1 "Search", reconcile §8): debounce and supersede, titles only, grouping. */
class AgentSearchModelTest {
    private fun hit(thread: String, seq: ULong, more: UInt = 0u, archived: Boolean = false, whenMs: Long = 1_000L) = AgentHit(
        thread = thread,
        title = "Thread $thread",
        agent = "claude",
        folder = "duka-pos",
        state = AgentThreadState.DONE,
        archived = archived,
        seq = seq,
        kind = if (seq == 0uL) null else AgentItemKind.USER,
        whenMs = whenMs,
        snippet = "the retry now waits",
        marks = listOf(AgentSnippetMark(4u, 5u)),
        moreInThread = more,
    )

    private fun page(vararg hits: AgentHit, next: String? = null, titlesOnly: Boolean = false) =
        AgentHits(hits = hits.toList(), next = next, titlesOnly = titlesOnly)

    @Test
    fun `words under 3 letters search titles only, CJK pairs included`() {
        assertTrue(AgentSearch.short("re"))
        assertTrue(AgentSearch.short("a bc"))
        assertTrue(AgentSearch.short("同步"))
        assertFalse(AgentSearch.short("retry"))
        assertFalse(AgentSearch.short("re try"))
        assertFalse(AgentSearch.short("   "))
    }

    @Test
    fun `typing makes a new request once the typing pauses, and nothing for an empty box`() {
        var state = AgentSearch.typed(SearchState(), "ret")
        assertEquals(SearchPhase.WAITING, state.phase)
        val first = AgentSearch.request(state)
        assertNotNull(first)
        assertEquals("ret", first!!.query)
        state = AgentSearch.typed(state, "retry ")
        val second = AgentSearch.request(state)!!
        assertTrue("a newer generation", second.generation > first.generation)
        assertEquals("trimmed", "retry", second.query)
        assertSame("the same text again changes nothing", state, AgentSearch.typed(state, "retry "))
        val cleared = AgentSearch.typed(AgentSearch.answered(state, second.generation, page(hit("t1", 4u))), "")
        assertEquals(SearchPhase.IDLE, cleared.phase)
        assertTrue(cleared.hits.isEmpty())
        assertNull(AgentSearch.request(cleared))
        assertEquals("debounce", 150L, AgentSearch.DEBOUNCE_MS)
    }

    @Test
    fun `an answer to a superseded request is dropped`() {
        var state = AgentSearch.typed(SearchState(), "ret")
        val old = AgentSearch.request(state)!!
        state = AgentSearch.started(state, old.generation)
        assertEquals(SearchPhase.SEARCHING, state.phase)
        state = AgentSearch.typed(state, "retry")
        val stale = AgentSearch.answered(state, old.generation, page(hit("old", 1u)))
        assertSame(state, stale)
        assertSame(state, AgentSearch.failed(state, old.generation, AgentFailure.TIMEOUT))
        val fresh = AgentSearch.answered(state, state.generation, page(hit("t1", 4u), titlesOnly = false))
        assertEquals(SearchPhase.READY, fresh.phase)
        assertEquals(listOf("t1"), fresh.hits.map { it.thread })
    }

    @Test
    fun `including archived threads asks again`() {
        var state = AgentSearch.answered(AgentSearch.typed(SearchState(), "retry"), 1L, page(hit("t1", 4u)))
        state = AgentSearch.archivedChanged(state, true)
        val request = AgentSearch.request(state)!!
        assertTrue(request.archived)
        assertEquals(SearchPhase.WAITING, state.phase)
        assertSame(state, AgentSearch.archivedChanged(state, true))
    }

    @Test
    fun `a query past the PC's bound isn't sent`() {
        val state = AgentSearch.typed(SearchState(), "x".repeat(AgentSearch.MAX_QUERY_BYTES + 1))
        assertNull(AgentSearch.request(state))
    }

    @Test
    fun `the PC's titles-only answer says so`() {
        val state = AgentSearch.typed(SearchState(), "re")
        val answered = AgentSearch.answered(state, state.generation, page(hit("t1", 0u), titlesOnly = true))
        assertTrue(answered.titlesOnly)
    }

    @Test
    fun `hits group by thread in the PC's order, three a thread and how many more`() {
        val hits = listOf(
            hit("sync", 1u, whenMs = 9L),
            hit("sync", 2u),
            hit("mpesa", 7u),
            hit("sync", 3u),
            hit("sync", 4u, more = 2u),
            hit("rate", 0u, archived = true),
        )
        val groups = AgentSearch.groups(hits)
        assertEquals(listOf("sync", "mpesa", "rate"), groups.map { it.threadId })
        assertEquals(listOf(1uL, 2uL, 3uL), groups[0].hits.map { it.seq })
        assertEquals("the PC's count wins when it's larger", 2, groups[0].more)
        assertEquals(1_000L, groups[0].whenMs)
        assertEquals(0, groups[1].more)
        assertTrue("a title's own hit makes no message row", groups[2].hits.isEmpty())
        assertNotNull(groups[2].titleHit)
        assertTrue(groups[2].archived)
    }

    @Test
    fun `marks are clamped to the snippet`() {
        val ranges = AgentSearch.markRanges("retry", listOf(AgentSnippetMark(0u, 5u), AgentSnippetMark(3u, 50u), AgentSnippetMark(9u, 2u), AgentSnippetMark(2u, 0u)))
        assertEquals(listOf(0 until 5, 3 until 5), ranges)
    }

    @Test
    fun `a next page adds hits it hasn't shown, for the request still on screen`() {
        var state = AgentSearch.typed(SearchState(), "retry")
        state = AgentSearch.answered(state, state.generation, page(hit("t1", 1u), next = "c1"))
        val more = AgentSearch.more(state)!!
        assertEquals("c1", more.cursor)
        state = AgentSearch.loadingMore(state, more.generation)
        assertNull("one at a time", AgentSearch.more(state))
        state = AgentSearch.moreAnswered(state, more.generation, page(hit("t1", 1u), hit("t2", 5u)))
        assertEquals(listOf("t1", "t2"), state.hits.map { it.thread })
        assertNull(state.next)
        assertFalse(state.loadingMore)
        val typedOver = AgentSearch.typed(state, "retries")
        assertSame(typedOver, AgentSearch.moreAnswered(typedOver, more.generation, page(hit("t3", 9u))))
    }

    @Test
    fun `a hit opens its thread at its message once`() {
        AgentJump.request("pc", "t1", 42uL)
        assertNull("another thread doesn't take it", AgentJump.take("pc", "t2"))
        assertEquals(42uL, AgentJump.take("pc", "t1"))
        assertNull(AgentJump.take("pc", "t1"))
        AgentJump.request("pc", "t1", 0uL)
        assertNull("the title opens the thread as usual", AgentJump.take("pc", "t1"))
    }
}
