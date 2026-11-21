// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentCachedItems
import app.g7link.core.AgentCachedThreads
import app.g7link.core.AgentCheckpoint
import app.g7link.core.AgentHeld
import app.g7link.core.AgentHeldState
import app.g7link.core.AgentImportProgress
import app.g7link.core.AgentItem
import app.g7link.core.AgentItemKind
import app.g7link.core.AgentMode
import app.g7link.core.AgentPage
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState
import app.g7link.core.AgentTurn
import app.g7link.core.AgentTurnFile
import app.g7link.core.AgentFileStatus
import app.g7link.core.Event
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The phase 3 parts of the Agents state (contract §U8.3, §U9): turns, cuts,
 * shelves, marks shown at once, what was saved shown first and replaced by
 * the PC's, held messages, imports and drafts changed elsewhere.
 */
class AgentsHistoryStateTest {
    private val pc = "pc1"

    private fun thread(
        id: String = "t1",
        state: AgentThreadState = AgentThreadState.DONE,
        updatedMs: Long = 1_000L,
        archived: Boolean = false,
        snoozedUntilMs: Long? = null,
        pinned: Boolean = false,
        title: String = "Fix the test",
    ) = AgentThread(
        id = id,
        agent = "claude",
        folder = "g7linux",
        title = title,
        state = state,
        mode = AgentMode.SUPERVISED,
        fromPhone = false,
        updatedMs = updatedMs,
        asks = 0u,
        summary = null,
        error = null,
        archived = archived,
        snoozedUntilMs = snoozedUntilMs,
        pinned = pinned,
    )

    private fun item(seq: ULong, turn: UInt = 1u, kind: AgentItemKind = AgentItemKind.ASSISTANT) =
        AgentItem(seq = seq, id = "i$seq", kind = kind, whenMs = seq.toLong(), text = "text $seq", truncated = false, tool = null, turn = turn)

    private fun turn(n: UInt, promptSeq: ULong, checkpoint: AgentCheckpoint = AgentCheckpoint.READY, files: UInt = 1u) = AgentTurn(
        turn = n,
        promptSeq = promptSeq,
        lastSeq = null,
        startedMs = 0L,
        endedMs = 10L,
        ended = null,
        checkpoint = checkpoint,
        why = null,
        files = files,
        adds = 1u,
        dels = 0u,
        moreFiles = false,
        skipped = 0u,
        canFork = true,
        canRewind = true,
    )

    private fun file(path: String) = AgentTurnFile(path = path, status = AgentFileStatus.MODIFIED, from = null, adds = 1u, dels = 0u, binary = false)

    private fun opened(items: List<AgentItem> = (1uL..6uL).map { item(it, turn = if (it <= 3uL) 1u else 2u) }): AgentsState {
        val open = AgentsModel.opened(AgentsState(), "t1")
        return AgentsModel.withPage(open, AgentPage(thread = thread(), items = items, moreBefore = false, asks = emptyList()))
    }

    private fun held(key: String, thread: String = "t1") =
        AgentHeld(key = key, thread = thread, text = "hold", createdMs = 1L, state = AgentHeldState.WAITING, why = null)

    @Test
    fun `the new events are agent events of their PC`() {
        val events = listOf(
            Event.AgentTurn(pc, "t1", turn(1u, 1uL)),
            Event.AgentCut(pc, "t1", 4uL, 6uL, false),
            Event.AgentImport(pc, AgentImportProgress("j", 1u, 0u, 2u, null)),
            Event.AgentOutbox(pc, emptyList()),
            Event.AgentDraftChanged(pc, "t1", 3uL),
        )
        for (event in events) assertEquals(pc, AgentsModel.pcOf(event))
    }

    @Test
    fun `the open thread's turns are kept in order and replaced one at a time`() {
        var state = AgentsModel.withTurns(opened(), "t1", listOf(turn(2u, 4uL), turn(1u, 1uL)))
        assertEquals(listOf(1u, 2u), state.open!!.turns.map { it.turn })
        state = AgentsModel.withTurnFiles(state, "t1", 2u, listOf(file("src/a.rs")))
        val pending = turn(3u, 7uL, AgentCheckpoint.PENDING)
        state = AgentsModel.reduce(state, Event.AgentTurn(pc, "t1", pending)).state
        assertEquals(listOf(1u, 2u, 3u), state.open!!.turns.map { it.turn })
        // Its restore point settled: its files are read again.
        state = AgentsModel.withTurnFiles(state, "t1", 3u, listOf(file("x")))
        state = AgentsModel.reduce(state, Event.AgentTurn(pc, "t1", turn(3u, 7uL, AgentCheckpoint.READY, files = 4u))).state
        assertFalse(3u in state.open!!.turnFiles)
        assertTrue(2u in state.open!!.turnFiles)
        // Another thread's turn isn't this one's.
        assertSame(state, AgentsModel.reduce(state, Event.AgentTurn(pc, "t2", turn(9u, 1uL))).state)
    }

    @Test
    fun `the timeline gets the turns and the files read`() {
        var state = AgentsModel.withTurns(opened(), "t1", listOf(turn(1u, 1uL), turn(2u, 4uL)))
        state = AgentsModel.withTurnFiles(state, "t1", 2u, listOf(file("src/a.rs"), file("src/b.rs")))
        val input = AgentsModel.timelineInput(state, "t1", emptySet())!!
        assertEquals(listOf(1u, 2u), input.turns.map { it.turn })
        assertEquals(listOf(2u), input.turnFiles.map { it.turn })
        assertEquals(listOf("src/a.rs", "src/b.rs"), input.turnFiles.single().files.map { it.path })
        assertEquals(0L, input.tick)
    }

    @Test
    fun `a cut drops its messages and their turns, and its undo reads the thread again`() {
        var state = AgentsModel.withTurns(opened(), "t1", listOf(turn(1u, 1uL), turn(2u, 4uL)))
        val update = AgentsModel.reduce(state, Event.AgentCut(pc, "t1", 4uL, 6uL, false))
        assertNull(update.reread)
        state = update.state
        assertEquals(listOf(1uL, 2uL, 3uL), state.open!!.items.map { it.seq })
        assertEquals(listOf(1u), state.open!!.turns.map { it.turn })
        val undone = AgentsModel.reduce(state, Event.AgentCut(pc, "t1", 4uL, 6uL, true))
        assertEquals("t1", undone.reread)
        assertSame("another thread's cut", state, AgentsModel.reduce(state, Event.AgentCut(pc, "t2", 1uL, 9uL, false)).state)
    }

    @Test
    fun `the main list keeps the shelves, and a shelf's list replaces only that shelf`() {
        var state = AgentsModel.withShelf(AgentsState(), Shelf.ARCHIVED, listOf(thread("a1", archived = true)), 5L)
        state = AgentsModel.withThreads(state, listOf(thread("m1"), thread("m2")), 10L)
        assertEquals(setOf("a1", "m1", "m2"), state.threads.map { it.id }.toSet())
        assertEquals(listOf("m1", "m2").toSet(), state.listed.map { it.id }.toSet())
        assertEquals(10L, state.listedMs)
        assertTrue(Shelf.ARCHIVED in state.shelves)
        // Unarchived elsewhere: the Archived shelf no longer lists it, the main list does.
        state = AgentsModel.withShelf(state, Shelf.ARCHIVED, emptyList(), 20L)
        assertEquals(setOf("m1", "m2"), state.threads.map { it.id }.toSet())
        state = AgentsModel.withShelf(state, Shelf.SNOOZED, listOf(thread("s1", snoozedUntilMs = 99L)), 30L)
        assertEquals(listOf("m1", "m2").toSet(), state.listed.map { it.id }.toSet())
        assertEquals(listOf("s1"), AgentShelves.on(state.threads, Shelf.SNOOZED).map { it.id })
    }

    @Test
    fun `archived and snoozed threads don't count on Home or in the drawer, and the drawer has the pinned`() {
        val state = AgentsModel.withThreads(
            AgentsState(),
            listOf(
                thread("waits_archived", AgentThreadState.WAITING, archived = true),
                thread("pinned", pinned = true, updatedMs = 5L),
                thread("done", updatedMs = 9L),
                thread("snoozed", AgentThreadState.WORKING, snoozedUntilMs = 99L),
            ),
        )
        assertEquals(HomeLine.Finished("Fix the test", ThreadStatus.Done, 9L), AgentsModel.homeLine(state))
        val groups = AgentsModel.drawerGroups(state)
        assertTrue(groups.needsYou.isEmpty())
        assertTrue(groups.working.isEmpty())
        assertEquals(listOf("pinned"), groups.pinned.map { it.id })
        assertEquals(listOf("done"), groups.recent.map { it.id })
    }

    @Test
    fun `what was saved shows at once and the PC's list replaces it`() {
        val saved = AgentCachedThreads(threads = listOf(thread("old")), savedMs = 1_000L)
        var state = AgentsModel.withSaved(AgentsState(), saved)
        assertEquals(listOf("old"), state.threads.map { it.id })
        assertEquals(1_000L, state.savedMs)
        state = AgentsModel.withThreads(state, listOf(thread("live")), 2_000L)
        assertEquals(listOf("live"), state.threads.map { it.id })
        assertNull(state.savedMs)
        assertSame("never over the PC's list", state, AgentsModel.withSaved(state, saved))
        val arriving = AgentsModel.withThread(AgentsState(), thread("event"))
        assertSame("nor over threads the PC is sending", arriving, AgentsModel.withSaved(arriving, saved))
        val empty = AgentsState()
        assertSame("nothing saved", empty, AgentsModel.withSaved(empty, AgentCachedThreads(emptyList(), null)))
    }

    @Test
    fun `a saved thread opens at once and its page from the PC replaces it`() {
        var state = AgentsModel.opened(AgentsState(), "t1")
        val saved = AgentCachedItems(thread = thread(), items = listOf(item(2uL), item(1uL)), savedMs = 500L)
        state = AgentsModel.withSavedItems(state, "t1", saved)
        assertTrue(state.open!!.loaded)
        assertEquals(500L, state.open!!.savedMs)
        assertEquals(listOf(1uL, 2uL), state.open!!.items.map { it.seq })
        assertEquals("the thread itself came with it", "t1", state.thread("t1")?.id)
        assertTrue("its rows can be made", AgentsModel.timelineInput(state, "t1", emptySet()) != null)
        state = AgentsModel.withPage(state, AgentPage(thread = thread(), items = listOf(item(1uL), item(2uL), item(3uL)), moreBefore = false, asks = emptyList()))
        assertNull(state.open!!.savedMs)
        assertEquals(listOf(1uL, 2uL, 3uL), state.open!!.items.map { it.seq })
        assertSame("the PC's page wins", state, AgentsModel.withSavedItems(state, "t1", saved))
    }

    @Test
    fun `a mark shows at once and goes back when the PC refuses it`() {
        val before = AgentsModel.withThreads(AgentsState(), listOf(thread("t1"), thread("t2")))
        val marked = AgentsModel.marked(before, listOf("t1"), MarkChange.Archive(true))
        assertTrue(marked.thread("t1")!!.archived)
        assertEquals(listOf("t2"), marked.listed.map { it.id })
        val back = AgentsModel.unmarked(marked, before.threads.filter { it.id == "t1" })
        assertFalse(back.thread("t1")!!.archived)
        val answered = AgentsModel.withThreadsChanged(marked, listOf(thread("t1", archived = true, title = "Renamed")))
        assertEquals("Renamed", answered.thread("t1")!!.title)
        assertSame(before, AgentsModel.marked(before, emptyList(), MarkChange.Pin(true)))
    }

    @Test
    fun `held messages, imports and drafts changed elsewhere are kept as the PC and the core say`() {
        var state = AgentsModel.reduce(AgentsState(), Event.AgentOutbox(pc, listOf(held("k1"), held("k2", "t2")))).state
        assertEquals(listOf("k1", "k2"), state.held.map { it.key })
        assertSame(state, AgentsModel.withHeld(state, state.held))
        state = AgentsModel.reduce(state, Event.AgentImport(pc, AgentImportProgress("j", 3u, 1u, 4u, "t9"))).state
        assertEquals(3u, state.importing!!.done)
        state = AgentsModel.reduce(state, Event.AgentDraftChanged(pc, "t1", 7uL)).state
        assertEquals(7uL, state.draftChanges["t1"])
        assertSame(state, AgentsModel.draftChanged(state, "t1", 7uL))
        assertSame(state, AgentsModel.draftChanged(state, "", 9uL))
    }
}
