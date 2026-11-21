// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentCheckpoint
import app.g7link.core.AgentForkedFrom
import app.g7link.core.AgentHeld
import app.g7link.core.AgentHeldState
import app.g7link.core.AgentHeldWhy
import app.g7link.core.AgentMode
import app.g7link.core.AgentNotice
import app.g7link.core.AgentReadonly
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState
import app.g7link.core.AgentTurn
import app.g7link.core.AgentUndo
import java.time.ZoneId
import java.time.ZonedDateTime
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Shelves, marks, snooze times, turns, undo, held messages and the offline rules (contract §U9). */
class AgentHistoryModelTest {
    private fun thread(
        id: String,
        state: AgentThreadState = AgentThreadState.DONE,
        updatedMs: Long = 1_000L,
        pinned: Boolean = false,
        archived: Boolean = false,
        unread: Boolean = false,
        snoozedUntilMs: Long? = null,
        asks: UInt = 0u,
        undo: AgentUndo? = null,
        readonly: AgentReadonly? = null,
        forkedFrom: AgentForkedFrom? = null,
        found: Boolean = false,
    ) = AgentThread(
        id = id,
        agent = "claude",
        folder = "duka-pos",
        title = "Thread $id",
        state = state,
        mode = AgentMode.SUPERVISED,
        fromPhone = false,
        updatedMs = updatedMs,
        asks = asks,
        summary = null,
        error = null,
        pinned = pinned,
        archived = archived,
        unread = unread,
        snoozedUntilMs = snoozedUntilMs,
        undo = undo,
        readonly = readonly,
        forkedFrom = forkedFrom,
        found = found,
    )

    private fun turn(n: UInt, checkpoint: AgentCheckpoint? = AgentCheckpoint.READY, endedMs: Long? = 5_000L, files: UInt = 2u) = AgentTurn(
        turn = n,
        promptSeq = n.toULong() * 10uL,
        lastSeq = null,
        startedMs = 1_000L,
        endedMs = endedMs,
        ended = null,
        checkpoint = checkpoint,
        why = null,
        files = files,
        adds = 3u,
        dels = 1u,
        moreFiles = false,
        skipped = 0u,
        canFork = true,
        canRewind = true,
    )

    private fun held(key: String, state: AgentHeldState, why: AgentHeldWhy? = null, thread: String = "t1") =
        AgentHeld(key = key, thread = thread, text = "Na usibadilishe Cargo.lock", createdMs = 1_000L, state = state, why = why)

    // ---- Shelves and sections ----

    @Test
    fun `sections come in their order and a pinned thread that needs you or works stays in that section`() {
        val threads = listOf(
            thread("recent", updatedMs = 1L),
            thread("pinned_done", pinned = true, updatedMs = 2L),
            thread("pinned_working", state = AgentThreadState.WORKING, pinned = true, updatedMs = 3L),
            thread("waits", state = AgentThreadState.WAITING, pinned = true, updatedMs = 4L),
            thread("archived", archived = true),
            thread("snoozed", snoozedUntilMs = 99_000L),
            thread("recent2", updatedMs = 9L),
        )
        val sections = AgentShelves.sections(threads)
        assertEquals(listOf("waits"), sections.needsYou.map { it.id })
        assertEquals(listOf("pinned_working"), sections.working.map { it.id })
        assertEquals(listOf("pinned_done"), sections.pinned.map { it.id })
        assertEquals("newest first", listOf("recent2", "recent"), sections.recent.map { it.id })
        assertTrue(AgentShelves.sections(emptyList()).empty)
    }

    @Test
    fun `an archived or snoozed thread is on its own shelf, the snoozed by when they come back`() {
        val threads = listOf(
            thread("a", archived = true, updatedMs = 5L),
            thread("b", archived = true, updatedMs = 9L),
            thread("late", snoozedUntilMs = 9_000L),
            thread("soon", snoozedUntilMs = 2_000L),
            thread("main"),
            // Archived wins over a snooze.
            thread("both", archived = true, snoozedUntilMs = 1L, updatedMs = 1L),
        )
        assertEquals(listOf("b", "a", "both"), AgentShelves.on(threads, Shelf.ARCHIVED).map { it.id })
        assertEquals(listOf("soon", "late"), AgentShelves.on(threads, Shelf.SNOOZED).map { it.id })
        assertEquals(listOf("main"), AgentShelves.on(threads, Shelf.MAIN).map { it.id })
    }

    @Test
    fun `a fork's row says what it forked from and a brought-in one says so`() {
        assertEquals(ThreadOrigin.ForkOf("Sync"), AgentShelves.origin(thread("f", forkedFrom = AgentForkedFrom("t0", 2u, "Sync"))))
        assertEquals(ThreadOrigin.BroughtIn, AgentShelves.origin(thread("i", found = true)))
        assertNull(AgentShelves.origin(thread("plain")))
    }

    @Test
    fun `filters list their kind of thread on the main shelf, and the shelves`() {
        val waits = thread("w", state = AgentThreadState.WAITING)
        val archivedWaits = thread("aw", state = AgentThreadState.WAITING, archived = true)
        assertTrue(ThreadFilter.NEEDS_YOU.matches(waits))
        assertFalse(ThreadFilter.NEEDS_YOU.matches(archivedWaits))
        assertTrue(ThreadFilter.ARCHIVED.matches(archivedWaits))
        assertFalse(ThreadFilter.ALL.matches(archivedWaits))
        assertTrue(ThreadFilter.SNOOZED.matches(thread("s", snoozedUntilMs = 5L)))
        assertEquals(Shelf.ARCHIVED, ThreadFilter.ARCHIVED.shelf)
        assertEquals(Shelf.MAIN, ThreadFilter.RECENT.shelf)
    }

    // ---- Marks ----

    @Test
    fun `the sheet offers the opposite of each mark the thread has`() {
        assertEquals(
            listOf(ThreadAction.PIN, ThreadAction.MARK_UNREAD, ThreadAction.SNOOZE, ThreadAction.ARCHIVE, ThreadAction.RENAME),
            AgentMarks.actions(thread("t")),
        )
        assertEquals(
            listOf(ThreadAction.UNPIN, ThreadAction.MARK_READ, ThreadAction.SNOOZE, ThreadAction.UNARCHIVE, ThreadAction.RENAME),
            AgentMarks.actions(thread("t", pinned = true, unread = true, archived = true)),
        )
    }

    @Test
    fun `a snoozed thread's sheet offers Unsnooze, which ends the snooze, and an unsnoozed one's doesn't`() {
        val snoozed = thread("s", snoozedUntilMs = 99_000L)
        assertEquals(
            listOf(ThreadAction.PIN, ThreadAction.MARK_UNREAD, ThreadAction.SNOOZE, ThreadAction.UNSNOOZE, ThreadAction.ARCHIVE, ThreadAction.RENAME),
            AgentMarks.actions(snoozed),
        )
        assertFalse(ThreadAction.UNSNOOZE in AgentMarks.actions(thread("t")))
        val change = AgentMarks.change(ThreadAction.UNSNOOZE)
        assertEquals(MarkChange.Snooze(0L), change)
        assertTrue(AgentMarks.valid(change!!, 1_000L))
        assertTrue(AgentMarks.enabled(snoozed, ThreadAction.UNSNOOZE))
        assertNull("back on the main list at once", AgentMarks.applied(snoozed, change).snoozedUntilMs)
    }

    @Test
    fun `archiving waits while the thread works or waits, and is shown at once otherwise`() {
        val working = thread("t", state = AgentThreadState.WORKING)
        assertFalse(AgentMarks.enabled(working, ThreadAction.ARCHIVE))
        assertTrue(AgentMarks.enabled(working, ThreadAction.PIN))
        assertEquals("not archived while it works", working, AgentMarks.applied(working, MarkChange.Archive(true)))
        assertTrue(AgentMarks.applied(thread("t"), MarkChange.Archive(true)).archived)
        assertTrue(AgentMarks.applied(thread("t"), MarkChange.Pin(true)).pinned)
        assertTrue(AgentMarks.applied(thread("t"), MarkChange.Unread(true)).unread)
        assertEquals(5_000L, AgentMarks.applied(thread("t"), MarkChange.Snooze(5_000L)).snoozedUntilMs)
        assertNull("0 brings it back", AgentMarks.applied(thread("t", snoozedUntilMs = 9L), MarkChange.Snooze(0L)).snoozedUntilMs)
        assertEquals(MarkChange.Archive(false), AgentMarks.change(ThreadAction.UNARCHIVE))
        assertNull("snooze asks when first", AgentMarks.change(ThreadAction.SNOOZE))
        assertNull(AgentMarks.change(ThreadAction.RENAME))
    }

    @Test
    fun `a snooze is ahead and within 30 days`() {
        val now = 1_000_000L
        assertTrue(AgentMarks.valid(MarkChange.Snooze(now + 1), now))
        assertTrue(AgentMarks.valid(MarkChange.Snooze(now + AgentMarks.MAX_SNOOZE_MS), now))
        assertFalse(AgentMarks.valid(MarkChange.Snooze(now + AgentMarks.MAX_SNOOZE_MS + 1), now))
        assertFalse(AgentMarks.valid(MarkChange.Snooze(now), now))
        assertTrue("0 clears it", AgentMarks.valid(MarkChange.Snooze(0L), now))
        assertTrue(AgentMarks.valid(MarkChange.Pin(true), now))
    }

    @Test
    fun `a title is one trimmed line of 1 to 80 bytes`() {
        assertEquals("Receipt printer", AgentMarks.title("  Receipt printer \t"))
        assertNull(AgentMarks.title("   "))
        assertNull(AgentMarks.title("two\nlines"))
        assertEquals("a".repeat(80), AgentMarks.title("a".repeat(80)))
        assertNull(AgentMarks.title("a".repeat(81)))
        // 40 two-byte characters and one more are 81 bytes.
        assertNull(AgentMarks.title("ž".repeat(40) + "a"))
    }

    // ---- Snooze times ----

    private val zone: ZoneId = ZoneId.of("Africa/Nairobi")

    private fun at(text: String): Long = ZonedDateTime.parse("${text}+03:00[Africa/Nairobi]").toInstant().toEpochMilli()

    @Test
    fun `snooze times on a Wednesday afternoon are an hour, this evening, tomorrow and next Monday`() {
        val now = at("2026-11-18T14:32:00")
        val choices = AgentMarks.snoozeChoices(now, zone)
        assertEquals(listOf(SnoozeKind.HOUR, SnoozeKind.EVENING, SnoozeKind.TOMORROW, SnoozeKind.NEXT_WEEK), choices.map { it.kind })
        assertEquals(at("2026-11-18T15:32:00"), choices[0].atMs)
        assertEquals(at("2026-11-18T18:00:00"), choices[1].atMs)
        assertEquals(at("2026-11-19T09:00:00"), choices[2].atMs)
        assertEquals(at("2026-11-23T09:00:00"), choices[3].atMs)
    }

    @Test
    fun `after six the evening is gone, and on a Monday next week is the Monday after`() {
        val late = AgentMarks.snoozeChoices(at("2026-11-23T19:10:00"), zone)
        assertEquals(listOf(SnoozeKind.HOUR, SnoozeKind.TOMORROW, SnoozeKind.NEXT_WEEK), late.map { it.kind })
        assertEquals(at("2026-11-30T09:00:00"), late.last().atMs)
    }

    @Test
    fun `when the limit resets is offered only while the agent reports one ahead`() {
        val now = at("2026-11-18T14:32:00")
        val limits = listOf(
            LimitInfo("300", 99, at("2026-11-18T18:00:00"), LimitState.REACHED),
            LimitInfo("10080", 40, at("2026-11-20T09:00:00"), LimitState.OK),
        )
        val resets = AgentMarks.limitResets(limits, now)
        assertEquals(at("2026-11-18T18:00:00"), resets)
        assertEquals(SnoozeKind.LIMIT, AgentMarks.snoozeChoices(now, zone, resets).last().kind)
        assertNull("only nearly or fully used windows", AgentMarks.limitResets(listOf(limits[1]), now))
        assertNull("a reset in the past", AgentMarks.limitResets(listOf(LimitInfo("300", 99, now - 1, LimitState.WARN)), now))
        assertNull(AgentMarks.limitResets(listOf(LimitInfo("300", 99, null, LimitState.REACHED)), now))
    }

    // ---- Turns ----

    @Test
    fun `a pending restore point makes the rows again a second after its turn ended`() {
        val turns = listOf(turn(1u), turn(2u, AgentCheckpoint.PENDING, endedMs = 10_000L), turn(3u, AgentCheckpoint.PENDING, endedMs = null))
        assertEquals(11_000L, AgentTurns.recheckAtMs(turns, 10_200L))
        assertNull("already past: the rows made now say it", AgentTurns.recheckAtMs(turns, 11_000L))
        assertNull(AgentTurns.recheckAtMs(listOf(turn(1u)), 0L))
    }

    @Test
    fun `turns stay in order, replaced by number`() {
        val turns = AgentTurns.with(AgentTurns.with(listOf(turn(1u), turn(3u)), turn(2u)), turn(3u, AgentCheckpoint.EXPIRED))
        assertEquals(listOf(1u, 2u, 3u), turns.map { it.turn })
        assertEquals(AgentCheckpoint.EXPIRED, turns.last().checkpoint)
    }

    @Test
    fun `the files read for Changes rows are the recorded turns that changed something, newest first, once`() {
        val turns = listOf(turn(1u), turn(2u, files = 0u), turn(3u, AgentCheckpoint.MISSING), turn(4u))
        assertEquals(listOf(4u, 1u), AgentTurns.filesWanted(turns, emptySet()))
        assertEquals(listOf(1u), AgentTurns.filesWanted(turns, setOf(4u)))
        val many = (1u..30u).map { turn(it) }
        assertEquals(AgentTurns.FILES_READ, AgentTurns.filesWanted(many, emptySet()).size)
    }

    // ---- Undo ----

    @Test
    fun `undo shows only on a rewind's notices and only while the thread can be put back`() {
        val undoable = thread("t", undo = AgentUndo(untilMs = 10_000L, files = false))
        assertEquals(UndoOffer.Shown.ON, UndoOffer.shown(AgentNotice.REWOUND, undoable, 5_000L))
        assertEquals(UndoOffer.Shown.ON, UndoOffer.shown(AgentNotice.EDIT_FROM_HERE, undoable, 5_000L))
        assertEquals(UndoOffer.Shown.ON, UndoOffer.shown(AgentNotice.FILES_RESTORED, undoable, 5_000L))
        assertEquals(UndoOffer.Shown.NONE, UndoOffer.shown(AgentNotice.FORKED, undoable, 5_000L))
        assertEquals(UndoOffer.Shown.NONE, UndoOffer.shown(null, undoable, 5_000L))
        assertEquals("no longer", UndoOffer.Shown.NONE, UndoOffer.shown(AgentNotice.REWOUND, undoable, 10_000L))
        assertEquals(UndoOffer.Shown.NONE, UndoOffer.shown(AgentNotice.REWOUND, thread("t"), 5_000L))
        assertEquals(UndoOffer.Shown.NONE, UndoOffer.shown(AgentNotice.REWOUND, null, 5_000L))
        val files = thread("t", undo = AgentUndo(untilMs = 10_000L, files = true))
        assertEquals("files are put back on the PC", UndoOffer.Shown.ON_PC, UndoOffer.shown(AgentNotice.REWOUND, files, 5_000L))
    }

    // ---- Held messages (contract §U8.3's flush table, as the phone shows it) ----

    @Test
    fun `each held message reads as its state says`() {
        assertEquals(HeldLine.Waiting, AgentOutbox.line(held("a", AgentHeldState.WAITING)))
        assertEquals(HeldLine.Sending, AgentOutbox.line(held("a", AgentHeldState.SENDING)))
        // A thread waiting on a request or question.
        assertEquals(HeldLine.AnswerFirst, AgentOutbox.line(held("a", AgentHeldState.BLOCKED, AgentHeldWhy.ANSWER_FIRST)))
        // A working thread whose queue on the PC is full.
        assertEquals(HeldLine.QueueFull, AgentOutbox.line(held("a", AgentHeldState.BLOCKED, AgentHeldWhy.QUEUE_FULL)))
        // Gone, read only, more than a day old, refused: not sent, with Send and Discard.
        for (why in listOf(AgentHeldWhy.GONE, AgentHeldWhy.READ_ONLY, AgentHeldWhy.TOO_OLD, AgentHeldWhy.REFUSED)) {
            val line = AgentOutbox.line(held("a", AgentHeldState.NOT_SENT, why))
            assertEquals(HeldLine.NotSent(why), line)
            assertTrue(AgentOutbox.actionable(held("a", AgentHeldState.NOT_SENT, why)))
        }
        assertFalse(AgentOutbox.actionable(held("a", AgentHeldState.WAITING)))
        assertFalse(AgentOutbox.actionable(held("a", AgentHeldState.BLOCKED, AgentHeldWhy.ANSWER_FIRST)))
    }

    @Test
    fun `a thread's held messages are its own, in the order written`() {
        val all = listOf(held("1", AgentHeldState.WAITING), held("2", AgentHeldState.WAITING, thread = "t2"), held("3", AgentHeldState.SENDING))
        assertEquals(listOf("1", "3"), AgentOutbox.of(all, "t1").map { it.key })
    }

    // ---- Offline ----

    @Test
    fun `saved ago counts minutes, then hours, then days`() {
        assertEquals(AgentOffline.AgoUnit.MINUTES to 1, AgentOffline.ago(10_000L, 20_000L))
        assertEquals(AgentOffline.AgoUnit.MINUTES to 12, AgentOffline.ago(0L, 12 * 60_000L + 30_000L))
        assertEquals(AgentOffline.AgoUnit.HOURS to 3, AgentOffline.ago(0L, 3 * 3_600_000L + 5L))
        assertEquals(AgentOffline.AgoUnit.DAYS to 2, AgentOffline.ago(0L, 2 * 86_400_000L))
        assertEquals("a clock that went back", AgentOffline.AgoUnit.MINUTES to 1, AgentOffline.ago(50_000L, 0L))
    }

    @Test
    fun `a message waits for the PC only for a thread that can be continued, and never with pictures`() {
        assertTrue(AgentOffline.canHold(thread("t"), images = 0))
        assertFalse("pictures can't wait", AgentOffline.canHold(thread("t"), images = 1))
        assertFalse("a new thread is never started from the outbox", AgentOffline.canHold(null, images = 0))
        assertFalse(AgentOffline.canHold(thread("t", readonly = AgentReadonly.OUTSIDE_FOLDERS), images = 0))
    }
}
