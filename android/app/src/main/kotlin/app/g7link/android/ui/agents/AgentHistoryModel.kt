// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable
import app.g7link.core.AgentCheckpoint
import app.g7link.core.AgentHeld
import app.g7link.core.AgentHeldState
import app.g7link.core.AgentHeldWhy
import app.g7link.core.AgentNotice
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState
import app.g7link.core.AgentTurn
import java.time.DayOfWeek
import java.time.Instant
import java.time.LocalTime
import java.time.ZoneId
import java.time.ZonedDateTime
import java.time.temporal.TemporalAdjusters

/** Which list a thread is on (contract §U3.5): the main one, or a shelf of its own. */
enum class Shelf { MAIN, ARCHIVED, SNOOZED }

/**
 * The main list's sections, in their order (contract §U9.1): what needs you,
 * what works, the pinned ones, then the rest; the newest first in each.
 */
@Immutable
data class ThreadSections(
    val needsYou: List<AgentThread> = emptyList(),
    val working: List<AgentThread> = emptyList(),
    val pinned: List<AgentThread> = emptyList(),
    val recent: List<AgentThread> = emptyList(),
) {
    val empty: Boolean get() = needsYou.isEmpty() && working.isEmpty() && pinned.isEmpty() && recent.isEmpty()
}

/** The lists and their sections. */
object AgentShelves {
    fun shelfOf(thread: AgentThread): Shelf = when {
        thread.archived -> Shelf.ARCHIVED
        thread.snoozedUntilMs != null -> Shelf.SNOOZED
        else -> Shelf.MAIN
    }

    /** The threads on [shelf]: the newest first; the snoozed ones by when they come back. */
    fun on(threads: List<AgentThread>, shelf: Shelf): List<AgentThread> {
        val listed = threads.filter { shelfOf(it) == shelf }
        return when (shelf) {
            Shelf.SNOOZED -> listed.sortedWith(compareBy<AgentThread> { it.snoozedUntilMs ?: Long.MAX_VALUE }.thenByDescending { it.updatedMs })
            Shelf.MAIN, Shelf.ARCHIVED -> listed.sortedByDescending { it.updatedMs }
        }
    }

    /** A thread needing you or working is in that section even when pinned. */
    fun sections(threads: List<AgentThread>): ThreadSections {
        val needsYou = ArrayList<AgentThread>()
        val working = ArrayList<AgentThread>()
        val pinned = ArrayList<AgentThread>()
        val recent = ArrayList<AgentThread>()
        for (thread in on(threads, Shelf.MAIN)) {
            when {
                AgentsModel.needsYou(thread) -> needsYou += thread
                thread.state == AgentThreadState.WORKING -> working += thread
                thread.pinned -> pinned += thread
                else -> recent += thread
            }
        }
        return ThreadSections(needsYou, working, pinned, recent)
    }

    /** What a list row says under its title instead of the summary, when anything. */
    fun origin(thread: AgentThread): ThreadOrigin? = when {
        thread.forkedFrom != null -> ThreadOrigin.ForkOf(thread.forkedFrom?.title.orEmpty())
        thread.found -> ThreadOrigin.BroughtIn
        else -> null
    }
}

/** Where a thread came from, as its list row says: "Fork of %1", "Brought in". */
@Immutable
sealed interface ThreadOrigin {
    data class ForkOf(val title: String) : ThreadOrigin

    data object BroughtIn : ThreadOrigin
}

/** A change of marks asked for from the phone ([AgentMarks]). */
@Immutable
sealed interface MarkChange {
    data class Pin(val on: Boolean) : MarkChange

    data class Unread(val on: Boolean) : MarkChange

    data class Archive(val on: Boolean) : MarkChange

    /** Out of the main list until [untilMs]; 0 brings it back. */
    data class Snooze(val untilMs: Long) : MarkChange
}

/** What a thread's long-press sheet offers, in its order. */
enum class ThreadAction { PIN, UNPIN, MARK_UNREAD, MARK_READ, SNOOZE, UNSNOOZE, ARCHIVE, UNARCHIVE, RENAME }

/** The times "Snooze" offers (contract §U9.1). */
enum class SnoozeKind { HOUR, EVENING, TOMORROW, NEXT_WEEK, LIMIT }

@Immutable
data class SnoozeChoice(val kind: SnoozeKind, val atMs: Long)

/** Pin, unread, snooze, archive and rename from the phone (contract §U3.5, §U9.1). */
object AgentMarks {
    /** The most threads one mark may name. */
    const val MAX_THREADS = 50

    /** A snooze ends within this. */
    const val MAX_SNOOZE_MS = 30L * 24 * 60 * 60 * 1000

    /** A title is one line of at most this many bytes. */
    const val MAX_TITLE_BYTES = 80

    private val EVENING = LocalTime.of(18, 0)
    private val MORNING = LocalTime.of(9, 0)

    /**
     * What a thread's sheet offers. A snoozed one can be snoozed to another
     * time or brought back now (Unsnooze). Archiving waits while it works or
     * waits (the PC says BUSY then).
     */
    fun actions(thread: AgentThread): List<ThreadAction> = listOfNotNull(
        if (thread.pinned) ThreadAction.UNPIN else ThreadAction.PIN,
        if (thread.unread) ThreadAction.MARK_READ else ThreadAction.MARK_UNREAD,
        ThreadAction.SNOOZE,
        ThreadAction.UNSNOOZE.takeIf { thread.snoozedUntilMs != null },
        if (thread.archived) ThreadAction.UNARCHIVE else ThreadAction.ARCHIVE,
        ThreadAction.RENAME,
    )

    /** Whether [action] can be done now, apart from the PC being there. */
    fun enabled(thread: AgentThread, action: ThreadAction): Boolean = when (action) {
        ThreadAction.ARCHIVE -> !AgentsModel.running(thread)
        else -> true
    }

    /** The change an action makes; null for the ones that ask more first (snooze, rename). */
    fun change(action: ThreadAction): MarkChange? = when (action) {
        ThreadAction.PIN -> MarkChange.Pin(true)
        ThreadAction.UNPIN -> MarkChange.Pin(false)
        ThreadAction.MARK_UNREAD -> MarkChange.Unread(true)
        ThreadAction.MARK_READ -> MarkChange.Unread(false)
        ThreadAction.ARCHIVE -> MarkChange.Archive(true)
        ThreadAction.UNARCHIVE -> MarkChange.Archive(false)
        // 0 ends a snooze (agent_mark's snooze_until_ms).
        ThreadAction.UNSNOOZE -> MarkChange.Snooze(0L)
        ThreadAction.SNOOZE, ThreadAction.RENAME -> null
    }

    /** The thread as it reads once [change] is done: shown at once, the PC's answer replaces it. */
    fun applied(thread: AgentThread, change: MarkChange): AgentThread = when (change) {
        is MarkChange.Pin -> thread.copy(pinned = change.on)
        is MarkChange.Unread -> thread.copy(unread = change.on)
        is MarkChange.Archive -> if (change.on && AgentsModel.running(thread)) thread else thread.copy(archived = change.on)
        is MarkChange.Snooze -> thread.copy(snoozedUntilMs = change.untilMs.takeIf { it > 0L })
    }

    /** Whether the PC takes [change] at [nowMs]: a snooze is ahead and within 30 days. */
    fun valid(change: MarkChange, nowMs: Long): Boolean = when (change) {
        is MarkChange.Snooze -> change.untilMs == 0L || (change.untilMs > nowMs && change.untilMs - nowMs <= MAX_SNOOZE_MS)
        else -> true
    }

    /** A title the PC takes: trimmed, one line, 1 to 80 bytes; null otherwise. */
    fun title(text: String): String? {
        val line = text.trim()
        if (line.isEmpty() || line.any { it == '\n' || it == '\r' }) return null
        return line.takeIf { AgentsModel.utf8Length(it) <= MAX_TITLE_BYTES }
    }

    /**
     * The snooze times at [nowMs] in [zone]: in an hour; this evening at 18:00
     * while it's still ahead; tomorrow at 09:00; next week on Monday at 09:00;
     * and when the agent's limit resets, while it reports one ahead.
     */
    fun snoozeChoices(nowMs: Long, zone: ZoneId, limitResetsMs: Long? = null): List<SnoozeChoice> {
        val now = ZonedDateTime.ofInstant(Instant.ofEpochMilli(nowMs), zone)
        val evening = now.with(EVENING).withSecond(0).withNano(0)
        val tomorrow = now.toLocalDate().plusDays(1).atTime(MORNING).atZone(zone)
        val nextWeek = now.toLocalDate().with(TemporalAdjusters.next(DayOfWeek.MONDAY)).atTime(MORNING).atZone(zone)
        val choices = buildList {
            add(SnoozeChoice(SnoozeKind.HOUR, nowMs + 60L * 60 * 1000))
            if (evening.isAfter(now)) add(SnoozeChoice(SnoozeKind.EVENING, evening.toInstant().toEpochMilli()))
            add(SnoozeChoice(SnoozeKind.TOMORROW, tomorrow.toInstant().toEpochMilli()))
            add(SnoozeChoice(SnoozeKind.NEXT_WEEK, nextWeek.toInstant().toEpochMilli()))
            if (limitResetsMs != null) add(SnoozeChoice(SnoozeKind.LIMIT, limitResetsMs))
        }
        return choices.filter { valid(MarkChange.Snooze(it.atMs), nowMs) }
    }

    /** When the agent's limit resets: the soonest of those nearly or fully used, still ahead. */
    fun limitResets(limits: List<LimitInfo>, nowMs: Long): Long? =
        limits.filter { it.state != LimitState.OK }.mapNotNull { it.resetsMs }.filter { it > nowMs }.minOrNull()
}

/** A turn's rows read again when a restore point is still being recorded (contract §U9.1). */
object AgentTurns {
    /** "Recording changes…" shows once a turn has ended this long ago with its restore point pending. */
    const val RECORDING_AFTER_MS = 1_000L

    /** The most turns whose first files are read for their Changes rows. */
    const val FILES_READ = 20

    /** Turns read a page at a time (the PC's most), and at most this many pages. */
    const val PAGE = 100
    const val PAGES = 5

    /** When the rows must be made again for a pending restore point; null when none waits. */
    fun recheckAtMs(turns: List<AgentTurn>, nowMs: Long): Long? =
        turns.filter { it.checkpoint == AgentCheckpoint.PENDING }
            .mapNotNull { it.endedMs?.plus(RECORDING_AFTER_MS) }
            .filter { it > nowMs }
            .minOrNull()

    /** One turn added or replaced; the list stays in turn order. */
    fun with(turns: List<AgentTurn>, turn: AgentTurn): List<AgentTurn> =
        (turns.filterNot { it.turn == turn.turn } + turn).sortedBy { it.turn }

    /** The turns whose first files the Changes rows want and that weren't read yet: the newest first. */
    fun filesWanted(turns: List<AgentTurn>, read: Set<UInt>): List<UInt> =
        turns.filter { it.checkpoint == AgentCheckpoint.READY && it.files > 0u && it.turn !in read }
            .sortedByDescending { it.turn }
            .take(FILES_READ)
            .map { it.turn }
}

/** Undo on a rewind's notice (integration notes: only these three, only while the thread says it can). */
object UndoOffer {
    enum class Shown { NONE, ON, ON_PC }

    fun shown(notice: AgentNotice?, thread: AgentThread?, nowMs: Long): Shown {
        val undo = thread?.undo ?: return Shown.NONE
        val acts = when (notice) {
            AgentNotice.REWOUND, AgentNotice.EDIT_FROM_HERE, AgentNotice.FILES_RESTORED -> true
            AgentNotice.FORKED, null -> false
        }
        if (!acts || undo.untilMs <= nowMs) return Shown.NONE
        // A rewind that put files back is undone where the files are: "Undo it on %1."
        return if (undo.files) Shown.ON_PC else Shown.ON
    }
}

/** How a held message reads (contract §U8.3, §U9.3). */
@Immutable
sealed interface HeldLine {
    data object Waiting : HeldLine

    data object Sending : HeldLine

    data object AnswerFirst : HeldLine

    data object QueueFull : HeldLine

    /** "Not sent", with Send and Discard; [why] when the PC said. */
    data class NotSent(val why: AgentHeldWhy?) : HeldLine
}

/** The messages that wait on this phone for a PC. */
object AgentOutbox {
    /** A thread's held messages, in the order written. */
    fun of(held: List<AgentHeld>, threadId: String): List<AgentHeld> = held.filter { it.thread == threadId }

    fun line(held: AgentHeld): HeldLine = when (held.state) {
        AgentHeldState.WAITING -> HeldLine.Waiting
        AgentHeldState.SENDING -> HeldLine.Sending
        AgentHeldState.BLOCKED -> when (held.why) {
            AgentHeldWhy.QUEUE_FULL -> HeldLine.QueueFull
            AgentHeldWhy.ANSWER_FIRST, null -> HeldLine.AnswerFirst
            AgentHeldWhy.GONE, AgentHeldWhy.READ_ONLY, AgentHeldWhy.TOO_OLD, AgentHeldWhy.REFUSED -> HeldLine.NotSent(held.why)
        }
        AgentHeldState.NOT_SENT -> HeldLine.NotSent(held.why)
    }

    /** Send and Discard are offered on one that wasn't sent. */
    fun actionable(held: AgentHeld): Boolean = line(held) is HeldLine.NotSent
}

/** What the phone can do while its PC can't be reached (contract §U9.3 "Offline"). */
object AgentOffline {
    /** What "Saved %2 ago" counts in. */
    enum class AgoUnit { MINUTES, HOURS, DAYS }

    /** How long ago [savedMs] was, in whole units, at least a minute. */
    fun ago(savedMs: Long, nowMs: Long): Pair<AgoUnit, Int> {
        val minutes = ((nowMs - savedMs).coerceAtLeast(0L) / 60_000L).coerceAtLeast(1L)
        return when {
            minutes < 60L -> AgoUnit.MINUTES to minutes.toInt()
            minutes < 48L * 60L -> AgoUnit.HOURS to (minutes / 60L).toInt()
            else -> AgoUnit.DAYS to (minutes / (24L * 60L)).coerceAtMost(9_999L).toInt()
        }
    }

    /** A message can wait for the PC: an existing thread that can be continued, text only. */
    fun canHold(thread: AgentThread?, images: Int): Boolean = thread != null && thread.readonly == null && images == 0
}
