// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable

// The phone's part of project scripts (phase 6 contract §X5.2): a folder's
// scripts as the PC lets this phone see them, a thread's runs, and the rules of
// the Scripts sheet, its confirmation and the output screen. Plain Kotlin
// types, filled from the FFI in AgentGitFfi.kt only, so the rules run in JVM
// tests. Whether a phone may run a folder's scripts is the PC's check; these
// rules only decide what the phone offers.

enum class RunPhase { RUNNING, DONE, FAILED, STOPPED }

/** A script of a folder, typed in G7 Link on the PC. */
@Immutable
data class Script(val id: String, val name: String, val command: String)

/**
 * A thread's folder as the catalog shows it: whether phones can run its
 * scripts ("Phones can run these" on the PC), how many it has, and the
 * scripts themselves (the PC sends them only while phones can run them).
 */
@Immutable
data class ScriptsFolder(val id: String, val label: String, val phoneScripts: Boolean, val count: Int, val scripts: List<Script>)

/** A run of a script on the PC, and the last lines it printed. */
@Immutable
data class ScriptRun(
    val id: String,
    val threadId: String,
    /** Empty when only the thread's brief of it is known. */
    val scriptId: String,
    val name: String,
    val phase: RunPhase,
    val exit: Int? = null,
    val startedMs: Long = 0L,
    val endedMs: Long? = null,
    /** The phone that started it, by its name; null: started on the PC. */
    val by: String? = null,
    /** This phone started it. */
    val mine: Boolean = false,
    val lines: List<String> = emptyList(),
    /** The number of `lines[0]` among every line the run printed, counting from 0. */
    val from: Long = 0L,
    /** Lines the PC didn't keep. */
    val dropped: Int = 0,
    /** Why it failed without an exit code: "spawn" (the shell didn't start) or "signal". */
    val reason: String? = null,
    /** Lines this phone didn't keep. */
    val cut: Int = 0,
    /** Lines went missing between two updates: the run is read again. */
    val gap: Boolean = false,
    /** Only the thread's brief of it is known: no lines, no `by` or `mine` yet. */
    val brief: Boolean = false,
)

/** A script's row in the sheet: Run, or the run of it under way and whether this phone may stop it. */
@Immutable
data class ScriptRow(val script: Script, val running: ScriptRun?, val canRun: Boolean, val canStop: Boolean)

/** The line the sheet shows instead of Run buttons. */
enum class ScriptsNote {
    /** "Needs %1": the PC can't be reached. */
    NEEDS_PC,

    /** "To run this folder's scripts here, turn on “Phones can run these”…". */
    SWITCH_OFF,
}

/** What the Scripts sheet shows: the folder's scripts, a line, then the thread's runs. */
@Immutable
data class ScriptsSheet(
    val scripts: List<ScriptRow>,
    val note: ScriptsNote?,
    val runs: List<ScriptRun>,
    /** The runs this phone may stop, by id. */
    val stoppable: Set<String>,
)

/** "%1 is running" under the thread's status line, with Stop when this phone may stop it. */
@Immutable
data class RunningLine(val run: ScriptRun, val canStop: Boolean)

/** A run's state in words. */
@Immutable
sealed interface RunWord {
    data object Running : RunWord

    data object Done : RunWord

    data class FailedExit(val exit: Int) : RunWord

    data object Failed : RunWord

    data object Stopped : RunWord

    /** "Couldn't start it.": the shell didn't start. */
    data object NotStarted : RunWord
}

/** Why a run wasn't started, as the copy words it. */
enum class ScriptRefusal {
    NEEDS_PC,

    /** The folder's switch is off on the PC (or the thread is read-only there). */
    SWITCH_OFF,

    /** "“%1” is already running." */
    BUSY,

    /** "Too many scripts are running on %1. Stop one first." */
    LIMIT,

    /** "That script isn't in this folder's list any more." */
    GONE,

    FAILED,
}

/** Starting a run: the run (failed at once when its shell couldn't start), or why the PC refused. */
@Immutable
sealed interface ScriptStart {
    data class Started(val run: ScriptRun) : ScriptStart

    data class Refused(val why: ScriptRefusal) : ScriptStart
}

/** Run on a script: a confirmation each time, and nothing is sent before its Run. */
@Immutable
sealed interface RunAsk {
    data object None : RunAsk

    /** "Run “%1” on %2?" with the command, Cancel and Run. */
    data class Confirm(val script: Script) : RunAsk

    /** Run was pressed on the confirmation; the PC hasn't answered yet. */
    data class Sending(val script: Script) : RunAsk
}

/** The rules of the Scripts sheet, the line under the status line and the output screen. */
object AgentScripts {
    /** Lines a phone keeps of one run (the PC sends it the newest 200 when asked, 40 at a time live). */
    const val MAX_LINES = 400

    /** Runs kept per PC. */
    const val MAX_RUNS = 16

    /** A thread's runs the PC lists (`AgentThread.runs`). */
    const val MAX_BRIEFS = 8

    /** The reason word of a run whose shell couldn't start. */
    const val SPAWN = "spawn"

    /**
     * The catalog folder a thread is in, by the folder's label (a thread names
     * its folder by label); none when no folder has it, or more than one does.
     */
    fun folderOf(folders: List<ScriptsFolder>, label: String): ScriptsFolder? =
        folders.filter { it.label == label }.singleOrNull()

    /** The thread's menu offers Scripts once its folder has any, or the thread has runs. */
    fun menuShown(folder: ScriptsFolder?, runs: List<ScriptRun>): Boolean = (folder?.count ?: 0) > 0 || runs.isNotEmpty()

    /**
     * A thread's runs: the thread's own list (running, then ended ones the PC
     * still keeps) with what this phone heard of each, then runs heard of
     * that the list doesn't name yet; running ones first. A run that ended
     * doesn't go back to running.
     */
    fun runsOf(threadId: String, briefs: List<ScriptRun>, kept: Map<String, ScriptRun>): List<ScriptRun> {
        val listed = briefs.take(MAX_BRIEFS).map { brief ->
            val known = kept[brief.id] ?: return@map brief
            if (known.phase == RunPhase.RUNNING && brief.phase != RunPhase.RUNNING) {
                known.copy(phase = brief.phase, exit = brief.exit ?: known.exit)
            } else {
                known
            }
        }
        val named = listed.mapTo(HashSet()) { it.id }
        val more = kept.values.filter { it.threadId == threadId && it.id !in named }.sortedByDescending { it.startedMs }
        return (listed + more).sortedBy { if (it.phase == RunPhase.RUNNING) 0 else 1 }
    }

    /** This phone may stop a run while it runs: one it started, or any of a folder whose switch is on. */
    fun canStop(run: ScriptRun, folder: ScriptsFolder?, offline: Boolean): Boolean =
        !offline && run.phase == RunPhase.RUNNING && (run.mine || folder?.phoneScripts == true)

    /**
     * The sheet: with the folder's switch on, each of its scripts with Run (or
     * its run under way); with it off, no script and no command, and the line
     * saying where to turn it on. A read-only thread runs nothing. The runs
     * stay readable whatever the switch.
     */
    fun sheet(folder: ScriptsFolder?, runs: List<ScriptRun>, offline: Boolean, readonly: Boolean): ScriptsSheet {
        val scripts = if (folder == null || !folder.phoneScripts || readonly) {
            emptyList<ScriptRow>()
        } else {
            folder.scripts.map { script ->
                val running = runs.firstOrNull { it.phase == RunPhase.RUNNING && runOf(it, script) }
                ScriptRow(
                    script = script,
                    running = running,
                    canRun = !offline && running == null,
                    canStop = running != null && canStop(running, folder, offline),
                )
            }
        }
        val note = when {
            offline -> ScriptsNote.NEEDS_PC
            folder != null && !folder.phoneScripts -> ScriptsNote.SWITCH_OFF
            else -> null
        }
        val stoppable = runs.filter { canStop(it, folder, offline) }.mapTo(HashSet()) { it.id }
        return ScriptsSheet(scripts, note, runs, stoppable)
    }

    /** A run is of [script]: by its id, or by name for a brief (names are unique in a folder). */
    private fun runOf(run: ScriptRun, script: Script): Boolean =
        if (run.scriptId.isNotEmpty()) run.scriptId == script.id else run.name.equals(script.name, ignoreCase = true)

    /** The line under the status line while one of the thread's runs runs. */
    fun runningLine(runs: List<ScriptRun>, folder: ScriptsFolder?, offline: Boolean): RunningLine? {
        val run = runs.firstOrNull { it.phase == RunPhase.RUNNING } ?: return null
        return RunningLine(run, canStop(run, folder, offline))
    }

    /** A run's state in words. */
    fun word(run: ScriptRun): RunWord = when (run.phase) {
        RunPhase.RUNNING -> RunWord.Running
        RunPhase.DONE -> RunWord.Done
        RunPhase.STOPPED -> RunWord.Stopped
        RunPhase.FAILED -> {
            val exit = run.exit
            when {
                run.reason == SPAWN -> RunWord.NotStarted
                exit != null -> RunWord.FailedExit(exit)
                else -> RunWord.Failed
            }
        }
    }

    /** "from %1": a run another phone started. */
    fun fromPhone(run: ScriptRun): String? = run.by?.takeIf { it.isNotBlank() && !run.mine }

    /** "Showing the last lines. The rest is on %1.": the output doesn't start at the run's first line. */
    fun tailShown(run: ScriptRun): Boolean = run.from > 0L || run.dropped > 0 || run.cut > 0

    /**
     * Why the PC refused a run: the host's reason word when it gave one, else
     * the failure (a denial is the folder's switch, or a read-only thread).
     */
    fun refusal(failure: AgentFailure, word: String?): ScriptRefusal = when (word) {
        "script_running" -> ScriptRefusal.BUSY
        "runs" -> ScriptRefusal.LIMIT
        "no_script" -> ScriptRefusal.GONE
        "phone_scripts_off" -> ScriptRefusal.SWITCH_OFF
        else -> when (failure) {
            AgentFailure.DENIED -> ScriptRefusal.SWITCH_OFF
            AgentFailure.NOT_FOUND -> ScriptRefusal.GONE
            AgentFailure.OFFLINE -> ScriptRefusal.NEEDS_PC
            AgentFailure.UNSUPPORTED, AgentFailure.TIMEOUT, AgentFailure.OTHER -> ScriptRefusal.FAILED
        }
    }

    // ---- Lines as they arrive (`Event.AgentScriptRun`) ----

    /**
     * A live update: its lines are the new ones, numbered from [ScriptRun.from];
     * they're added when they follow those kept. A gap keeps what was there and
     * marks the run to be read again. A run that ended doesn't go back to running.
     */
    fun update(old: ScriptRun?, update: ScriptRun): ScriptRun {
        if (old == null || old.id != update.id) return capped(update.copy(brief = false), 0)
        val ended = old.phase != RunPhase.RUNNING
        val phase = if (ended && update.phase == RunPhase.RUNNING) old.phase else update.phase
        val expected = old.from + old.lines.size
        val gap = update.from > expected
        val skip = (expected - update.from).coerceIn(0L, update.lines.size.toLong()).toInt()
        val fresh = if (gap) emptyList() else update.lines.drop(skip)
        return capped(
            update.copy(
                phase = phase,
                exit = update.exit ?: old.exit,
                lines = old.lines + fresh,
                from = old.from,
                dropped = maxOf(old.dropped, update.dropped),
                endedMs = update.endedMs ?: old.endedMs,
                startedMs = if (old.startedMs != 0L) old.startedMs else update.startedMs,
                by = update.by ?: old.by,
                mine = old.mine || update.mine,
                scriptId = update.scriptId.ifEmpty { old.scriptId },
                reason = update.reason ?: old.reason,
                gap = old.gap || gap,
                brief = false,
            ),
            old.cut,
        )
    }

    /** The run as the PC answered (started, read, stopped): it replaces what was kept, without going back to running. */
    fun read(old: ScriptRun?, read: ScriptRun): ScriptRun {
        var fresh = read.copy(gap = false, brief = false)
        if (old == null || old.id != read.id) return capped(fresh, 0)
        if (old.phase != RunPhase.RUNNING && read.phase == RunPhase.RUNNING) {
            fresh = fresh.copy(phase = old.phase, exit = old.exit, endedMs = old.endedMs, reason = old.reason)
        }
        // A list without lines (`agent.script.runs`) keeps the lines already here, and a gap still open.
        if (read.lines.isEmpty() && old.lines.isNotEmpty()) {
            return capped(fresh.copy(lines = old.lines, from = old.from, gap = old.gap), old.cut)
        }
        return capped(fresh, 0)
    }

    private fun capped(run: ScriptRun, cut: Int): ScriptRun {
        val over = run.lines.size - MAX_LINES
        return if (over <= 0) run.copy(cut = cut) else run.copy(lines = run.lines.drop(over), from = run.from + over, cut = cut + over)
    }

    /** The runs a PC's state keeps: this one in ([read]: the PC's answer, else a live update), the oldest out past [MAX_RUNS]. */
    fun kept(runs: Map<String, ScriptRun>, run: ScriptRun, read: Boolean = false): Map<String, ScriptRun> {
        val next = LinkedHashMap(runs)
        next.remove(run.id)
        next[run.id] = if (read) read(runs[run.id], run) else update(runs[run.id], run)
        while (next.size > MAX_RUNS) next.remove(next.keys.first())
        return next
    }

    /** A thread's runs go with it. */
    fun withoutThread(runs: Map<String, ScriptRun>, threadId: String): Map<String, ScriptRun> =
        if (runs.values.none { it.threadId == threadId }) runs else runs.filterValues { it.threadId != threadId }
}

/** The confirmation each Run goes through (§X5.2): Run on a row only opens it; its own Run sends, once. */
object AgentScriptAsk {
    /** Run on a script's row: the confirmation, when the row can run and nothing is being sent. */
    fun tap(ask: RunAsk, row: ScriptRow): RunAsk = if (row.canRun && ask !is RunAsk.Sending) RunAsk.Confirm(row.script) else ask

    /** Run on the confirmation: the script to send (null when no confirmation shows). */
    fun confirm(ask: RunAsk): Pair<RunAsk, Script?> = if (ask is RunAsk.Confirm) RunAsk.Sending(ask.script) to ask.script else ask to null

    /** Cancel, or the sheet dismissed: nothing is sent. A run already on its way isn't called back. */
    fun cancel(ask: RunAsk): RunAsk = if (ask is RunAsk.Sending) ask else RunAsk.None

    /** The PC answered. */
    fun done(): RunAsk = RunAsk.None
}
