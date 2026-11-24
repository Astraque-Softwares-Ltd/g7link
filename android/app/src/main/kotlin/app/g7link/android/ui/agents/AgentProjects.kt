// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable
import app.g7link.core.AgentAsk
import app.g7link.core.AgentSession
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState

/** A row in a folder: a thread G7 runs, or a session started on the PC that this phone follows. */
@Immutable
sealed interface ProjectItem {
    val key: String
    val folder: String
    val updatedMs: Long

    data class Thread(val thread: AgentThread) : ProjectItem {
        override val key: String get() = "thread:${thread.id}"
        override val folder: String get() = thread.folder
        override val updatedMs: Long get() = thread.updatedMs
    }

    data class Session(val session: AgentSession) : ProjectItem {
        override val key: String get() = "session:${session.id}"
        override val folder: String get() = session.folder
        override val updatedMs: Long get() = session.updatedMs
    }
}

/** Where a row sits in its folder, first to last. */
enum class ProjectRank { NEEDS_YOU, MAY_NEED_YOU, RUNNING, PINNED, REST }

/**
 * One folder of a PC (the Agents list's projects): its threads and the
 * sessions started in it, what needs you and runs first, then the newest;
 * the counts its header shows; and its branch and running script, read from
 * its threads.
 */
@Immutable
data class ProjectGroup(
    /** The folder's label (a thread's) or name (a session's); empty when neither said. */
    val folder: String,
    val items: List<ProjectItem>,
    /** Requests and questions waiting, and sessions that need you as their agent reported it (never a guess). */
    val needsYou: Int,
    /** Sessions that look like they wait: never counted, but they keep the folder open. */
    val mayNeedYou: Int,
    val running: Int,
    val threads: Int,
    val sessions: Int,
    val lastMs: Long,
    /** The branch strip of its newest thread that has one, preferring one outside a worktree. */
    val strip: GitStrip? = null,
    /** A script running for one of its threads, by name. */
    val script: String? = null,
) {
    /** Rows shown before "n more": every one that needs you, may, or runs, then [AgentProjects.REST_SHOWN] more. */
    val firstShown: Int
        get() = (items.count { AgentProjects.rank(it) < ProjectRank.PINNED } + AgentProjects.REST_SHOWN).coerceAtMost(items.size)
}

/** A request on the Needs you pager: a thread's ask or question, or a session waiting on the PC. */
@Immutable
sealed interface NeedsYouCard {
    val key: String
    val updatedMs: Long

    /** [ask] is the first request the phone holds of it; [questions] how many questions it asks. */
    data class Thread(val thread: AgentThread, val ask: AgentAsk?, val questions: Int) : NeedsYouCard {
        override val key: String get() = "thread:${thread.id}"
        override val updatedMs: Long get() = thread.updatedMs
    }

    data class Session(val session: AgentSession) : NeedsYouCard {
        override val key: String get() = "session:${session.id}"
        override val updatedMs: Long get() = session.updatedMs
    }
}

/**
 * The Agents list as projects (the owner's pick, 2026-11-24): what needs you
 * on a pager, then one card per folder that opens and closes, the busiest
 * first. Sessions started on the PC sit in their folder beside its threads,
 * so a session today is never below last month's threads.
 */
object AgentProjects {
    /** Rows a folder shows past the ones that need you or run, before "n more". */
    const val REST_SHOWN = 3

    /** Folders whose open or closed state is remembered per PC. */
    const val MAX_REMEMBERED = 64

    fun rank(item: ProjectItem): ProjectRank = when (item) {
        is ProjectItem.Thread -> when {
            AgentsModel.needsYou(item.thread) -> ProjectRank.NEEDS_YOU
            item.thread.state == AgentThreadState.WORKING -> ProjectRank.RUNNING
            item.thread.pinned -> ProjectRank.PINNED
            else -> ProjectRank.REST
        }
        is ProjectItem.Session -> when (AgentsModel.sessionLook(item.session)) {
            SessionLook.NEEDS_YOU -> ProjectRank.NEEDS_YOU
            SessionLook.MAY_NEED_YOU -> ProjectRank.MAY_NEED_YOU
            SessionLook.WORKING -> ProjectRank.RUNNING
            SessionLook.LIMITED, SessionLook.IDLE, SessionLook.ENDED -> ProjectRank.REST
        }
    }

    /** The sessions the list shows: only those the PC lists to this phone. */
    fun sessionsShown(state: AgentsState): List<AgentSession> =
        if (state.sessionsAllowed == true && !state.sessionsUnsupported) state.sessions else emptyList()

    /**
     * The main list's threads (neither archived nor snoozed) and the listed
     * sessions, by folder: folders with something that needs you first, then
     * those with something running, then the most recently changed.
     */
    fun groups(state: AgentsState): List<ProjectGroup> {
        val items = state.listed.map { ProjectItem.Thread(it) } + sessionsShown(state).map { ProjectItem.Session(it) }
        return items
            .groupBy { it.folder.trim() }
            .map { (folder, rows) -> group(state, folder, rows) }
            .sortedWith(
                compareByDescending<ProjectGroup> { it.needsYou > 0 }
                    .thenByDescending { it.running > 0 }
                    .thenByDescending { it.lastMs }
                    .thenBy { it.folder },
            )
    }

    private fun group(state: AgentsState, folder: String, rows: List<ProjectItem>): ProjectGroup {
        val ordered = rows.sortedWith(compareBy<ProjectItem> { rank(it) }.thenByDescending { it.updatedMs }.thenBy { it.key })
        val threads = ordered.mapNotNull { (it as? ProjectItem.Thread)?.thread }
        val withStrip = threads.filter { AgentGit.stripShown(state.gitStrip(it.id)) }.sortedByDescending { it.updatedMs }
        val stripOf = withStrip.firstOrNull { state.gitStrip(it.id)?.worktree == null } ?: withStrip.firstOrNull()
        val script = threads.firstNotNullOfOrNull { thread -> state.runsOf(thread.id).firstOrNull { it.phase == RunPhase.RUNNING }?.name }
        return ProjectGroup(
            folder = folder,
            items = ordered,
            needsYou = ordered.count { rank(it) == ProjectRank.NEEDS_YOU },
            mayNeedYou = ordered.count { rank(it) == ProjectRank.MAY_NEED_YOU },
            running = ordered.count { rank(it) == ProjectRank.RUNNING },
            threads = threads.size,
            sessions = ordered.size - threads.size,
            lastMs = ordered.maxOfOrNull { it.updatedMs } ?: 0L,
            strip = stripOf?.let { state.gitStrip(it.id) },
            script = script,
        )
    }

    /** The pager: threads with a request or question, and sessions that need you as their agent said; the newest first. */
    fun cards(state: AgentsState): List<NeedsYouCard> {
        val threads = state.listed.filter { AgentsModel.needsYou(it) }.map { thread ->
            NeedsYouCard.Thread(
                thread = thread,
                ask = state.asksFor(thread.id).firstOrNull(),
                questions = maxOf(state.questionsFor(thread.id).size, thread.asking.toInt()),
            )
        }
        val sessions = sessionsShown(state).filter { AgentsModel.needsYouExactly(it) }.map { NeedsYouCard.Session(it) }
        return (threads + sessions).sortedWith(compareByDescending<NeedsYouCard> { it.updatedMs }.thenBy { it.key })
    }

    /**
     * Whether a folder is open: as the user left it on this PC, else open when
     * something in it needs you, may, or runs, or it's [newest] (the folder
     * that changed last).
     */
    fun open(group: ProjectGroup, newest: String?, chosen: Map<String, Boolean>): Boolean =
        chosen[group.folder] ?: (group.needsYou > 0 || group.mayNeedYou > 0 || group.running > 0 || group.folder == newest)

    /** The folder that changed last, which starts open. */
    fun newest(groups: List<ProjectGroup>): String? = groups.maxByOrNull { it.lastMs }?.folder

    /** The rows a folder shows: all of them, or the first ones and a "n more". */
    fun shown(group: ProjectGroup, all: Boolean): List<ProjectItem> =
        if (all) group.items else group.items.take(group.firstShown)

    /** The user opened or closed [folder]: remembered last, the oldest choices dropped past [MAX_REMEMBERED]. */
    fun chose(chosen: Map<String, Boolean>, folder: String, open: Boolean): Map<String, Boolean> {
        val next = LinkedHashMap<String, Boolean>(chosen)
        next.remove(folder)
        next[folder] = open
        while (next.size > MAX_REMEMBERED) next.remove(next.keys.first())
        return next
    }

    /** The choices as one stored string: a line per folder, `1` or `0` and a tab before its name. */
    fun encode(chosen: Map<String, Boolean>): String =
        chosen.entries
            .filter { (folder, _) -> storable(folder) }
            .takeLast(MAX_REMEMBERED)
            .joinToString("\n") { (folder, open) -> (if (open) "1\t" else "0\t") + folder }

    /** Reads [encode]'s string back; anything else is skipped. */
    fun decode(stored: String?): Map<String, Boolean> {
        if (stored.isNullOrEmpty()) return emptyMap()
        val out = LinkedHashMap<String, Boolean>()
        for (line in stored.lineSequence().take(MAX_REMEMBERED * 2)) {
            if (line.length < 2 || line[1] != '\t') continue
            val open = when (line[0]) {
                '1' -> true
                '0' -> false
                else -> continue
            }
            val folder = line.substring(2)
            if (!storable(folder)) continue
            out.remove(folder)
            out[folder] = open
        }
        while (out.size > MAX_REMEMBERED) out.remove(out.keys.first())
        return out
    }

    private fun storable(folder: String): Boolean = folder.length <= MAX_FOLDER_CHARS && '\n' !in folder && '\r' !in folder

    /** The PC sends a folder's name in at most 64 characters (§2.2); a longer one isn't remembered. */
    private const val MAX_FOLDER_CHARS = 64
}
