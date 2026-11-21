// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable
import app.g7link.core.AgentFound
import app.g7link.core.AgentFoundList
import app.g7link.core.AgentImportJob
import app.g7link.core.AgentImportProgress

/** How a found session's row is badged (contract §U9.1 "Import"). */
enum class FoundBadge { BROUGHT_IN, OPEN, OUTSIDE, DAMAGED }

/** The sessions of one folder, newest first. */
@Immutable
data class FoundGroup(val folder: String, val rows: List<AgentFound>)

/** An import on its way, or done. */
@Immutable
data class ImportRun(
    val job: String,
    val ids: List<String>,
    val total: Int,
    val done: Int = 0,
    val failed: Int = 0,
    /** The sessions that couldn't be read, once it's done and the list was read again. */
    val failures: List<AgentFound> = emptyList(),
) {
    val finished: Boolean get() = done + failed >= total
}

/** Where the import screen stands. */
enum class ImportLoad { CONSENT, LOADING, READY, FAILED }

/**
 * The import screen (contract §U9.1 "Import", board 18): the agent filter and
 * search, the found sessions grouped by folder, the ones chosen, and the run.
 */
@Immutable
data class ImportState(
    val load: ImportLoad = ImportLoad.LOADING,
    /** "claude", "codex", or null for both. */
    val agent: String? = null,
    val query: String = "",
    val found: List<AgentFound> = emptyList(),
    val total: Int = 0,
    val scanning: Boolean = false,
    val nextMs: Long? = null,
    val selected: Set<String> = emptySet(),
    val run: ImportRun? = null,
    val failure: AgentFailure? = null,
) {
    val busy: Boolean get() = run != null && !run.finished
}

object AgentImport {
    /** The most sessions one import takes. */
    const val MAX_PER_JOB = 50

    /** A page of found sessions. */
    const val PAGE = 100

    /** The newest messages a session's preview shows. */
    const val PREVIEW = 30

    /** While the PC looks, the list is read again this often. */
    const val SCAN_POLL_MS = 1_500L

    fun badge(found: AgentFound): FoundBadge? = when {
        found.thread != null -> FoundBadge.BROUGHT_IN
        found.damaged -> FoundBadge.DAMAGED
        found.open -> FoundBadge.OPEN
        found.folderId == null -> FoundBadge.OUTSIDE
        else -> null
    }

    /** One brought in or damaged can't be chosen; one open in a terminal or outside the folders can (it reads, read only for the latter). */
    fun selectable(found: AgentFound): Boolean = found.thread == null && !found.damaged

    /** Groups by folder, the folder with the newest session first, each newest first. */
    fun groups(found: List<AgentFound>): List<FoundGroup> =
        found.groupBy { it.folder }
            .map { (folder, rows) -> FoundGroup(folder, rows.sortedByDescending { it.updatedMs }) }
            .sortedByDescending { group -> group.rows.firstOrNull()?.updatedMs ?: 0L }

    /** The PC's page: the first replaces the list, a later one adds to it. */
    fun listed(state: ImportState, page: AgentFoundList, more: Boolean): ImportState {
        val found = if (more) {
            val seen = state.found.mapTo(HashSet()) { it.id }
            state.found + page.found.filter { it.id !in seen }
        } else {
            page.found.distinctBy { it.id }
        }
        val byId = found.associateBy { it.id }
        return state.copy(
            load = ImportLoad.READY,
            found = found,
            total = page.total.toInt(),
            scanning = page.scanning,
            nextMs = page.nextMs,
            // A choice that can't be made any more (brought in meanwhile) goes.
            selected = state.selected.filterTo(LinkedHashSet()) { id -> byId[id]?.let(::selectable) == true },
            failure = null,
        )
    }

    fun filtered(state: ImportState, agent: String?, query: String): ImportState =
        state.copy(agent = agent, query = query, nextMs = null)

    /** Chooses or unchooses one; never past [MAX_PER_JOB], never one that can't be. */
    fun toggled(state: ImportState, id: String): ImportState {
        if (state.busy) return state
        val found = state.found.firstOrNull { it.id == id } ?: return state
        return when {
            id in state.selected -> state.copy(selected = state.selected - id)
            !selectable(found) || state.selected.size >= MAX_PER_JOB -> state
            else -> state.copy(selected = state.selected + id)
        }
    }

    /** Chooses every one of a folder that can be, while there's room. */
    fun toggledFolder(state: ImportState, folder: String): ImportState {
        if (state.busy) return state
        val ids = state.found.filter { it.folder == folder && selectable(it) }.map { it.id }
        if (ids.isEmpty()) return state
        if (ids.all { it in state.selected }) return state.copy(selected = state.selected - ids.toSet())
        val room = MAX_PER_JOB - state.selected.size
        return state.copy(selected = state.selected + ids.filterNot { it in state.selected }.take(room.coerceAtLeast(0)))
    }

    /** "Bring in N" was pressed and the PC took the job. */
    fun started(state: ImportState, ids: List<String>, job: AgentImportJob): ImportState =
        state.copy(run = ImportRun(job = job.job, ids = ids, total = job.total.toInt()), selected = emptySet())

    /** How the PC says it goes; another job's is ignored. */
    fun progressed(state: ImportState, progress: AgentImportProgress): ImportState {
        val run = state.run ?: return state
        if (run.job != progress.job) return state
        return state.copy(
            run = run.copy(
                done = progress.done.toInt().coerceAtLeast(run.done),
                failed = progress.failed.toInt().coerceAtLeast(run.failed),
                total = progress.total.toInt(),
            ),
        )
    }

    /**
     * After it's done and the list was read again: the sessions it was given
     * that didn't become threads are the ones that couldn't be read.
     */
    fun settled(state: ImportState): ImportState {
        val run = state.run?.takeIf { it.finished } ?: return state
        if (run.failed == 0) return state.copy(run = run.copy(failures = emptyList()))
        val byId = state.found.associateBy { it.id }
        val failures = run.ids.mapNotNull { byId[it] }.filter { it.thread == null }.take(run.failed)
        return state.copy(run = run.copy(failures = failures))
    }

    /** Cancel was pressed: what was brought in stays. */
    fun cancelled(state: ImportState): ImportState {
        val run = state.run ?: return state
        return state.copy(run = run.copy(total = run.done + run.failed))
    }
}
