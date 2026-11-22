// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable
import app.g7link.core.AgentAsk
import app.g7link.core.AgentAskKind
import app.g7link.core.AgentAskOutcome
import app.g7link.core.AgentCachedItems
import app.g7link.core.AgentCachedThreads
import app.g7link.core.AgentCatalog
import app.g7link.core.AgentHeld
import app.g7link.core.AgentImportProgress
import app.g7link.core.AgentInfo
import app.g7link.core.AgentItem
import app.g7link.core.AgentItemKind
import app.g7link.core.AgentMark
import app.g7link.core.AgentMarkKind
import app.g7link.core.AgentPage
import app.g7link.core.AgentPlanState
import app.g7link.core.AgentPlanStatus
import app.g7link.core.AgentPlanStep
import app.g7link.core.AgentSession
import app.g7link.core.AgentSessionPage
import app.g7link.core.AgentSessionState
import app.g7link.core.AgentStreaming
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState
import app.g7link.core.AgentToolStatus
import app.g7link.core.AgentTurn
import app.g7link.core.AgentTurnFile
import app.g7link.core.AgentTurnPaths
import app.g7link.core.Event

/**
 * A PC's agent threads as this phone knows them (docs/protocol/agents.md), in
 * memory: the catalog, the threads (every shelf's: [listed] is the main
 * list), the requests waiting for an answer, the one thread whose transcript
 * is open, and the sessions started on the PC outside G7 (§3) with the one
 * being followed. What the core saved on this phone fills it first while the
 * PC is away (contract §U8.3).
 */
@Immutable
data class AgentsState(
    /** What the PC can run; null until it says, and when nothing can run or access is off. */
    val catalog: AgentCatalog? = null,
    /** Requests waiting first, then the most recently changed. */
    val threads: List<AgentThread> = emptyList(),
    /** Requests waiting for an answer, by token. */
    val asks: Map<String, AgentAsk> = emptyMap(),
    /** Tokens this phone sent an answer for and hasn't heard back about. */
    val answering: Set<String> = emptySet(),
    val open: OpenThread? = null,
    /** Sessions started on the PC outside G7, in the PC's order (exact "needs you" first). */
    val sessions: List<AgentSession> = emptyList(),
    /** True: the PC lists them to this phone. False: its switch for this phone is off. Null: not asked yet, or the PC doesn't list them. */
    val sessionsAllowed: Boolean? = null,
    /** The PC said it doesn't list sessions (an older G7 Link): the section isn't shown. */
    val sessionsUnsupported: Boolean = false,
    val openSession: OpenSession? = null,
    /** Questions waiting for an answer, by token (contract §Q4.4); a secret one without its asks. */
    val questions: Map<String, QuestionInfo> = emptyMap(),
    /** Questions this phone answered or dismissed and hasn't heard back about. */
    val replying: Set<String> = emptySet(),
    /** The list came from what this phone saved then, and the PC hasn't answered since; null once it has. */
    val savedMs: Long? = null,
    /** When the PC last gave the main list. */
    val listedMs: Long? = null,
    /** The shelves read from the PC since it connected (Archived, Snoozed). */
    val shelves: Set<Shelf> = emptySet(),
    /** Messages held on this phone until the PC takes them, in the order written. */
    val held: List<AgentHeld> = emptyList(),
    /** How the last import of sessions goes. */
    val importing: AgentImportProgress? = null,
    /** Drafts changed on another device, by key: their rev. */
    val draftChanges: Map<String, ULong> = emptyMap(),
    /** What the PC last said of each thread's folder (`agent.git`) or this phone read, by thread. */
    val gitStrips: Map<String, GitStrip> = emptyMap(),
    /** Commits, pushes, switches and pull requests on the PC, by id, as they run (contract §SC6.1). */
    val gitJobs: Map<String, GitJob> = emptyMap(),
    /** Runs of the folders' scripts this phone heard of, by id, with their last lines (phase 6 §X5.2); never saved. */
    val scriptRuns: Map<String, ScriptRun> = emptyMap(),
) {
    fun thread(id: String): AgentThread? = threads.firstOrNull { it.id == id }

    /** The strip under a thread's status line: what the PC last said of its folder, else what the thread says. */
    fun gitStrip(threadId: String): GitStrip? = gitStrips[threadId] ?: thread(threadId)?.let { AgentGitFfi.stripOf(null, it) }

    /** A thread's runs: its own list with what this phone heard of each, running first. */
    fun runsOf(threadId: String): List<ScriptRun> =
        AgentScripts.runsOf(threadId, thread(threadId)?.let(AgentGitFfi::briefs).orEmpty(), scriptRuns)

    /** The catalog folder a thread is in (by its label), as the Scripts sheet reads it. */
    fun scriptsFolderOf(threadId: String): ScriptsFolder? {
        val label = thread(threadId)?.folder ?: return null
        return AgentScripts.folderOf(catalog?.folders.orEmpty().map(AgentGitFfi::scriptsFolder), label)
    }

    /** The main list: neither archived nor snoozed. */
    val listed: List<AgentThread>
        get() = threads.filter { AgentShelves.shelfOf(it) == Shelf.MAIN }

    fun session(id: String): AgentSession? = sessions.firstOrNull { it.id == id }

    /** A thread's waiting requests, oldest first. */
    fun asksFor(threadId: String): List<AgentAsk> =
        asks.values.filter { it.thread == threadId }.sortedBy { it.expiresMs }

    /**
     * The one request a thread shows and how many wait with it; null when none
     * waits. Oldest first (they all last as long, so the one expiring first was
     * asked first); the first one the typing guard doesn't hold, or the first
     * when it holds them all (contract §P8.1).
     */
    fun askQueue(threadId: String, held: (String) -> Boolean = { false }): AskQueue? {
        val asks = asksFor(threadId)
        if (asks.isEmpty()) return null
        val index = asks.indexOfFirst { !held(it.token) }.coerceAtLeast(0)
        val shown = asks[index]
        return AskQueue(shown = shown, more = asks.size - 1, position = index + 1, total = asks.size, held = held(shown.token))
    }

    /** A thread's waiting questions, oldest first. */
    fun questionsFor(threadId: String): List<QuestionInfo> =
        questions.values.filter { it.threadId == threadId }.sortedWith(compareBy<QuestionInfo> { it.createdMs }.thenBy { it.token })

    /** The question a thread shows and where it is among them, as [askQueue] does for requests; null when none waits. */
    fun questionQueue(threadId: String, held: (String) -> Boolean = { false }): QuestionQueue? {
        val waiting = questionsFor(threadId)
        if (waiting.isEmpty()) return null
        val index = waiting.indexOfFirst { !held(it.token) }.coerceAtLeast(0)
        val shown = waiting[index]
        return QuestionQueue(shown = shown, position = index + 1, total = waiting.size, held = held(shown.token))
    }

    /**
     * What the dock shows: a request or a question, one at a time, the first
     * one the typing guard doesn't hold, then the one asked first (they all
     * last as long, so the one expiring first was asked first).
     */
    fun dockShown(threadId: String, held: (String) -> Boolean = { false }): DockShown? {
        val asks = askQueue(threadId, held)
        val questions = questionQueue(threadId, held)
        return when {
            asks == null -> questions?.let { DockShown.Question(it) }
            questions == null -> DockShown.Approval(asks)
            asks.held != questions.held -> if (asks.held) DockShown.Question(questions) else DockShown.Approval(asks)
            questions.shown.expiresMs < asks.shown.expiresMs -> DockShown.Question(questions)
            else -> DockShown.Approval(asks)
        }
    }

    /** What an agent can do on this PC (models, plan, steering, images), from the catalog. */
    fun abilities(agentId: String): AgentAbilities =
        AgentControlFfi.abilities(catalog?.agents?.firstOrNull { it.id == agentId }, catalog?.canControl == true)

    /** The agents a thread can start with now. */
    val readyAgents: List<AgentInfo>
        get() = catalog?.agents.orEmpty().filter { it.installed && it.signedIn }

    /** The agent's own name ("Claude", "Codex"), from the catalog when it's known. */
    fun agentName(id: String): String =
        catalog?.agents?.firstOrNull { it.id == id }?.name?.takeIf { it.isNotBlank() }
            ?: id.replaceFirstChar { it.uppercaseChar() }
}

/** The thread on screen: its items in `seq` order and assistant text still streaming in. */
@Immutable
data class OpenThread(
    val threadId: String,
    val items: List<AgentItem> = emptyList(),
    /** Assistant text as it streams, by item id, until the finished item replaces it. */
    val streaming: Map<String, StreamingText> = emptyMap(),
    val moreBefore: Boolean = false,
    /** A page has been read since it opened. */
    val loaded: Boolean = false,
    /** Requests answered while it was open: marks under their steps where they say something. */
    val resolved: List<ResolvedAsk> = emptyList(),
    /** Deleted on the PC while open. */
    val removed: Boolean = false,
    /** Questions that ended while it was open: their rows say how (the FFI reads it from their marks). */
    val resolvedQuestions: List<ResolvedQuestion> = emptyList(),
    /** Its turns, in order (contract §U7.1): each turn's Changes and the messages' actions. */
    val turns: List<AgentTurn> = emptyList(),
    /** The first files of the turns whose files were read, by turn. */
    val turnFiles: Map<UInt, List<AgentTurnFile>> = emptyMap(),
    /** The items shown are what this phone saved then; the PC hasn't sent its page since. */
    val savedMs: Long? = null,
) {
    /** The highest `seq` seen, finished or streaming; 0 before any. */
    val lastSeq: ULong
        get() {
            val item = items.maxOfOrNull { it.seq } ?: 0uL
            val stream = streaming.values.maxOfOrNull { it.seq } ?: 0uL
            return maxOf(item, stream)
        }
}

/**
 * The followed session on screen: its items in `seq` order. A session's `seq`s
 * grow but aren't consecutive, so nothing here looks for gaps; it's read again
 * on coming back and on reconnecting.
 */
@Immutable
data class OpenSession(
    val sessionId: String,
    val items: List<AgentItem> = emptyList(),
    val moreBefore: Boolean = false,
    /** A page has been read since it opened. */
    val loaded: Boolean = false,
    /** It left the PC's list while open (it became a thread, its file went, or it aged out). */
    val removed: Boolean = false,
)

/**
 * A thread's requests are answered one at a time: [shown] in full, "[position]
 * of [total]", and [more] others waiting. [held]: it came while you were
 * typing, so its buttons wait until you pause (contract §P8.2).
 */
@Immutable
data class AskQueue(
    val shown: AgentAsk,
    val more: Int,
    val position: Int = 1,
    val total: Int = more + 1,
    val held: Boolean = false,
)

/**
 * What a thread's rows are made from ([AgentsModel.timelineInput]): handed to
 * the FFI's `AgentTimeline.rows` off the main thread. [key] names the thread or
 * session the rows are for; a result made for another key is never shown.
 */
@Immutable
data class TimelineInput(
    val key: String,
    val items: List<AgentItem>,
    val streaming: List<AgentStreaming>,
    val state: AgentThreadState,
    val stateSinceMs: Long,
    val marks: List<AgentMark>,
    val open: List<String>,
    /** The questions waiting in it, for how many each asks. */
    val questions: List<QuestionInfo> = emptyList(),
    /** Its turns and the first files of those read, for Changes and the messages' actions. */
    val turns: List<AgentTurn> = emptyList(),
    val turnFiles: List<AgentTurnPaths> = emptyList(),
    /** Bumped when time alone changes the rows (a restore point still pending a second after its turn). */
    val tick: Long = 0L,
)

/**
 * Home's one Agents line under the row's title (the user's wish: Home stays
 * brief, the rest is a tap away), by priority: what needs you, what's running,
 * the last one that finished, or a nudge to start.
 */
@Immutable
sealed interface HomeLine {
    /** [count] requests wait (threads' and sessions' that wait on the PC); [agent] is the one asking when there's one. */
    data class NeedsYou(val agent: String, val count: Int) : HomeLine

    /** [count] threads (or, with none, sessions) at work; one alone says its agent and since when. */
    data class Working(val agent: String, val sinceMs: Long?, val count: Int, val sessions: Boolean) : HomeLine

    /** The thread that ran last, how it ended and when. */
    data class Finished(val title: String, val status: ThreadStatus, val updatedMs: Long) : HomeLine

    data object Start : HomeLine
}

/**
 * Text still arriving, an answer's or a thought's ([kind]): [bytes] is its
 * UTF-8 length, which delta offsets count in.
 */
@Immutable
data class StreamingText(
    val seq: ULong,
    val text: String,
    val bytes: Int,
    /** A piece went missing: shown as it is until the finished item arrives. */
    val gap: Boolean = false,
    val kind: AgentItemKind = AgentItemKind.ASSISTANT,
)

/** A question that ended while its thread was open. */
@Immutable
data class ResolvedQuestion(
    val token: String,
    /** The `question` item it belongs to. */
    val itemId: String,
    val outcome: QuestionOutcome,
    /** Answered from this phone. */
    val here: Boolean,
    /** The device that answered, when the PC says. */
    val byName: String? = null,
)

@Immutable
data class ResolvedAsk(
    val token: String,
    val title: String,
    val outcome: AgentAskOutcome,
    /** Answered from this phone. */
    val here: Boolean,
    /** Shown after the item with this `seq`. */
    val afterSeq: ULong,
    /** The device that answered, when the PC says ("Allowed on guchu"). */
    val byName: String? = null,
    /** The step it was about; null from an older PC (its mark goes at the end). */
    val itemId: String? = null,
)

/**
 * What the Agents drawer lists of a PC, grouped like the thread list: what
 * needs you, what works, the rest; then the sessions started on the PC when it
 * lists them to this phone.
 */
@Immutable
data class DrawerGroups(
    val needsYou: List<AgentThread> = emptyList(),
    val working: List<AgentThread> = emptyList(),
    val recent: List<AgentThread> = emptyList(),
    val sessions: List<AgentSession> = emptyList(),
    val pinned: List<AgentThread> = emptyList(),
) {
    val empty: Boolean get() = needsYou.isEmpty() && working.isEmpty() && pinned.isEmpty() && recent.isEmpty() && sessions.isEmpty()
}

/** How a thread reads at a glance (the reconcile file's state words). */
@Immutable
sealed interface ThreadStatus {
    data object Working : ThreadStatus
    data object NeedsYou : ThreadStatus
    data object Done : ThreadStatus
    data object Stopped : ThreadStatus

    /** A plan-mode turn ended: its plan waits for "Implement" (listed with what finished, green). */
    data object PlanReady : ThreadStatus

    /** Stopped by the agent's usage limit; [resetsMs] when the CLI said when it resets. */
    data class Paused(val resetsMs: Long?) : ThreadStatus

    /** [message] is the CLI's own, shown as it is; [login] when the agent is signed out on the PC. */
    data class Failed(val message: String?, val login: Boolean = false) : ThreadStatus
}

/** A state and, when the transcript may be missing something, the thread to read again. */
data class AgentsUpdate(val state: AgentsState, val reread: String? = null)

/** Why a call to the PC's agents didn't go through. */
enum class AgentFailure {
    /** The PC hasn't allowed this phone yet (it now shows its user a prompt). */
    DENIED,

    /** The PC has no agent host, it can't be reached, or the agent can't run. */
    UNSUPPORTED,

    /** The thread or request is gone, or someone answered first. */
    NOT_FOUND,
    OFFLINE,
    TIMEOUT,
    OTHER,
}

/**
 * How a session started on the PC reads: its mark and word. [LIMITED]: it
 * stopped at its agent's usage limit ("Limit reached · resets 18:00"), with the
 * still mark, never the working ring (owner's report, 2026-11-21).
 */
enum class SessionLook { NEEDS_YOU, MAY_NEED_YOU, WORKING, LIMITED, IDLE, ENDED }

/** A call's result. */
sealed interface AgentCall<out T> {
    data class Ok<T>(val value: T) : AgentCall<T>
    data class Failed(val failure: AgentFailure) : AgentCall<Nothing>
}

/**
 * How a question is answered from its notification (contract §V3.2): its one
 * ask, the options as buttons in order, and whether "Type" answers it in the
 * user's own words. Only the option's index travels in the intent.
 */
@Immutable
data class NoticeAnswers(val askId: String, val options: List<String>, val type: Boolean)

/** The pure part of Agents on the phone: folding events and replies into [AgentsState]. */
object AgentsModel {
    /** The longest prompt or reply the phone sends: the wire allows 8192 bytes (§2.2). */
    const val MAX_PROMPT_CHARS = 8000
    const val MAX_PROMPT_BYTES = 8192

    /** The most questions a PC can make the phone keep. */
    private const val MAX_QUESTIONS = 64

    /** Items read per page, and the most the phone keeps for the open thread. */
    const val PAGE = 50
    private const val MAX_ITEMS = 1000
    private const val MAX_RESOLVED = 50
    private const val MAX_THREADS = 200
    private const val MAX_ASKS = 64

    /** The most sessions a PC lists (§3), and the most items kept for the followed one. */
    private const val MAX_SESSIONS = 50
    private const val MAX_SESSION_ITEMS = 400

    /** Streaming text stops growing past this; the finished item replaces it anyway. */
    private const val MAX_STREAM_CHARS = 20_000

    /** How the host appends a usage limit's reset time to the error message. */
    private const val RESETS_MARKER = "|resets_ms="

    /** Error codes the phone tells apart (contract §2.2, reconcile file). */
    private const val USAGE_LIMIT = "usage_limit"
    private const val LOGIN = "login"

    /** The PC an agent event is about, or null for any other event. */
    fun pcOf(event: Event): String? = when (event) {
        is Event.AgentCatalog -> event.id
        is Event.AgentThreadChanged -> event.id
        is Event.AgentThreadRemoved -> event.id
        is Event.AgentDelta -> event.id
        is Event.AgentItemChanged -> event.id
        is Event.AgentDone -> event.id
        is Event.AgentAsk -> event.id
        is Event.AgentAsked -> event.id
        is Event.AgentSessionChanged -> event.id
        is Event.AgentSessionRemoved -> event.id
        is Event.AgentSessionItem -> event.id
        is Event.AgentQuestion -> event.id
        is Event.AgentQuestioned -> event.id
        is Event.AgentDraftChanged -> event.id
        is Event.AgentTurn -> event.id
        is Event.AgentCut -> event.id
        is Event.AgentImport -> event.id
        is Event.AgentOutbox -> event.id
        is Event.AgentPresence -> event.id
        is Event.AgentHandoff -> event.id
        is Event.AgentBeside -> event.id
        is Event.AgentGit -> event.id
        is Event.AgentGitJob -> event.id
        is Event.AgentScriptRun -> event.id
        else -> null
    }

    /** Folds one of a PC's agent events into its state. */
    fun reduce(state: AgentsState, event: Event): AgentsUpdate = when (event) {
        // No catalog: nothing can run there, or this phone's access was turned off.
        is Event.AgentCatalog ->
            if (event.catalog == null) AgentsUpdate(AgentsState()) else AgentsUpdate(state.copy(catalog = event.catalog))
        is Event.AgentThreadChanged -> AgentsUpdate(withGitOf(withThread(state, event.thread), event.thread))
        is Event.AgentThreadRemoved -> AgentsUpdate(removed(state, event.thread))
        is Event.AgentDelta -> delta(state, event)
        is Event.AgentItemChanged -> itemChanged(state, event)
        is Event.AgentDone -> {
            val next = withGitOf(withThread(state, event.thread), event.thread)
            val open = next.open
            // The finished text always comes before the end; text still streaming means some was missed.
            val reread = if (open != null && open.threadId == event.thread.id && open.streaming.isNotEmpty()) open.threadId else null
            AgentsUpdate(next, reread)
        }
        is Event.AgentAsk -> AgentsUpdate(withAsk(state, event.ask))
        is Event.AgentAsked -> AgentsUpdate(asked(state, event.token, event.thread, event.outcome, event.byName))
        // The PC sends these only while it lets this phone see its sessions.
        is Event.AgentSessionChanged -> AgentsUpdate(withSession(state, event.session))
        is Event.AgentSessionRemoved -> AgentsUpdate(sessionRemoved(state, event.session))
        is Event.AgentSessionItem -> AgentsUpdate(withSessionItem(state, event.session, event.item))
        is Event.AgentQuestion -> AgentsUpdate(withQuestion(state, AgentControlFfi.question(event.question)))
        is Event.AgentQuestioned -> AgentsUpdate(
            questioned(state, event.token, event.thread, AgentControlFfi.outcome(event.outcome), event.byName),
        )
        is Event.AgentDraftChanged -> AgentsUpdate(draftChanged(state, event.key, event.rev))
        is Event.AgentTurn -> AgentsUpdate(withTurn(state, event.thread, event.turn))
        is Event.AgentCut -> cut(state, event.thread, event.fromSeq, event.toSeq, event.restored)
        is Event.AgentImport -> AgentsUpdate(state.copy(importing = event.progress))
        is Event.AgentOutbox -> AgentsUpdate(state.copy(held = event.held.take(MAX_HELD)))
        // Source control (contract §SC6.1): the strip's summary, and a job's new lines.
        is Event.AgentGit -> AgentsUpdate(
            state.copy(gitStrips = AgentGitStrips.put(state.gitStrips, event.thread, AgentGitFfi.stripOf(state.gitStrips[event.thread], event))),
        )
        is Event.AgentGitJob -> AgentsUpdate(withJob(state, AgentGitFfi.job(event.job)))
        // A script's run (phase 6 §X5.2): its new lines and its state.
        is Event.AgentScriptRun -> AgentsUpdate(withRun(state, AgentGitFfi.scriptRun(event.run)))
        else -> AgentsUpdate(state)
    }

    // ---- Source control (contract §SC6, §SC9.3) ----

    /** A thread's own git fields update the strip the PC's `agent.git` made, when there is one. */
    private fun withGitOf(state: AgentsState, thread: AgentThread): AgentsState {
        val known = state.gitStrips[thread.id] ?: return state
        return state.copy(gitStrips = AgentGitStrips.put(state.gitStrips, thread.id, AgentGitFfi.stripOf(known, thread)))
    }

    /** A status this phone read: the strip follows it. */
    fun withGitStatus(state: AgentsState, threadId: String, status: GitStatus): AgentsState =
        state.copy(gitStrips = AgentGitStrips.put(state.gitStrips, threadId, AgentGitStrips.fromStatus(state.gitStrips[threadId], status)))

    /** The strip this phone kept, while the PC is away. */
    fun withGitStrip(state: AgentsState, threadId: String, strip: GitStrip): AgentsState =
        state.copy(gitStrips = AgentGitStrips.put(state.gitStrips, threadId, strip))

    /** A job's new lines from `agent.git.progress`. */
    fun withJob(state: AgentsState, job: GitJob): AgentsState = state.copy(gitJobs = AgentJobs.kept(state.gitJobs, job))

    /** A job as the PC answered (started, read again, stopped): it replaces what was kept. */
    fun withJobRead(state: AgentsState, job: GitJob): AgentsState = state.copy(gitJobs = AgentJobs.kept(state.gitJobs, job, read = true))

    // ---- Project scripts (phase 6 §X5.2) ----

    /** A run's new lines from `agent.script`. */
    fun withRun(state: AgentsState, run: ScriptRun): AgentsState = state.copy(scriptRuns = AgentScripts.kept(state.scriptRuns, run))

    /** A run as the PC answered (started, listed, read, stopped): it replaces what was kept. */
    fun withRunRead(state: AgentsState, run: ScriptRun): AgentsState = state.copy(scriptRuns = AgentScripts.kept(state.scriptRuns, run, read = true))

    // ---- History and undo (contract §U7, §U8.3, §U9) ----

    /** The most held messages a PC can make the phone keep (the core keeps 20 a PC). */
    private const val MAX_HELD = 64

    /** The most draft changes remembered (the PC keeps 64 drafts). */
    private const val MAX_DRAFT_CHANGES = 64

    /** The most files kept for one turn's Changes row (the render crate shows 20). */
    private const val MAX_TURN_FILES = 20

    /** A draft changed elsewhere: its newest rev, remembered until the composer reads it. */
    fun draftChanged(state: AgentsState, key: String, rev: ULong): AgentsState {
        if (key.isEmpty() || state.draftChanges[key] == rev) return state
        val changes = state.draftChanges - key + (key to rev)
        return state.copy(draftChanges = if (changes.size > MAX_DRAFT_CHANGES) changes.entries.drop(changes.size - MAX_DRAFT_CHANGES).associate { it.toPair() } else changes)
    }

    /** The open thread's turns as the PC listed them (newest first there; kept in turn order here). */
    fun withTurns(state: AgentsState, threadId: String, turns: List<AgentTurn>): AgentsState {
        val open = state.open?.takeIf { it.threadId == threadId } ?: return state
        val sorted = turns.distinctBy { it.turn }.sortedBy { it.turn }
        val kept = sorted.mapTo(HashSet()) { it.turn }
        return state.copy(open = open.copy(turns = sorted, turnFiles = open.turnFiles.filterKeys { it in kept }))
    }

    /** A turn started, ended, or its restore point settled. */
    fun withTurn(state: AgentsState, threadId: String, turn: AgentTurn): AgentsState {
        val open = state.open?.takeIf { it.threadId == threadId } ?: return state
        val before = open.turns.firstOrNull { it.turn == turn.turn }
        if (before == turn) return state
        // Its files may have changed with its restore point: read again when wanted.
        val files = if (before != null && (before.checkpoint != turn.checkpoint || before.files != turn.files)) open.turnFiles - turn.turn else open.turnFiles
        return state.copy(open = open.copy(turns = AgentTurns.with(open.turns, turn), turnFiles = files))
    }

    /** A turn's first files, for its Changes row. */
    fun withTurnFiles(state: AgentsState, threadId: String, turn: UInt, files: List<AgentTurnFile>): AgentsState {
        val open = state.open?.takeIf { it.threadId == threadId } ?: return state
        return state.copy(open = open.copy(turnFiles = open.turnFiles + (turn to files.take(MAX_TURN_FILES))))
    }

    /**
     * Messages [fromSeq] to [toSeq] left a thread (a rewind): they're dropped
     * from the open transcript with their turns; [restored] (its undo) reads it
     * again.
     */
    fun cut(state: AgentsState, threadId: String, fromSeq: ULong, toSeq: ULong, restored: Boolean): AgentsUpdate {
        val open = state.open?.takeIf { it.threadId == threadId } ?: return AgentsUpdate(state)
        if (restored) return AgentsUpdate(state, threadId)
        if (toSeq < fromSeq) return AgentsUpdate(state)
        val range = fromSeq..toSeq
        val turns = open.turns.filterNot { it.promptSeq in range }
        val kept = turns.mapTo(HashSet()) { it.turn }
        return AgentsUpdate(
            state.copy(
                open = open.copy(
                    items = open.items.filterNot { it.seq in range },
                    streaming = open.streaming.filterValues { it.seq !in range },
                    resolved = open.resolved.filter { it.afterSeq < fromSeq },
                    turns = turns,
                    turnFiles = open.turnFiles.filterKeys { it in kept },
                ),
            ),
        )
    }

    /** The PC's list of one shelf: replaces what was known of that shelf only. */
    fun withShelf(state: AgentsState, shelf: Shelf, threads: List<AgentThread>, nowMs: Long): AgentsState {
        val others = state.threads.filter { AgentShelves.shelfOf(it) != shelf && threads.none { t -> t.id == it.id } }
        val fresh = threads.distinctBy { it.id }
        val next = state.copy(threads = sorted((fresh + others).take(MAX_THREADS)), shelves = state.shelves + shelf)
        return if (shelf == Shelf.MAIN) next.copy(savedMs = null, listedMs = nowMs) else next
    }

    /**
     * What this phone saved of the PC's list, shown at once: only while the PC
     * hasn't given its own since (the live list always wins).
     */
    fun withSaved(state: AgentsState, saved: AgentCachedThreads): AgentsState {
        // The PC gave its list, or its threads are arriving: those win.
        if (state.listedMs != null || (state.threads.isNotEmpty() && state.savedMs == null) || saved.threads.isEmpty()) return state
        val savedMs = saved.savedMs ?: return state
        return state.copy(threads = sorted(saved.threads.distinctBy { it.id }.take(MAX_THREADS)), savedMs = savedMs)
    }

    /** What this phone saved of a thread, shown at once while the PC's page hasn't come. */
    fun withSavedItems(state: AgentsState, threadId: String, saved: AgentCachedItems): AgentsState {
        val known = saved.thread?.takeIf { it.id == threadId && state.thread(threadId) == null }?.let { withThread(state, it) } ?: state
        val open = known.open?.takeIf { it.threadId == threadId } ?: return known
        if (open.loaded && open.savedMs == null) return known
        val savedMs = saved.savedMs ?: return known
        val items = saved.items.distinctBy { it.seq }.sortedBy { it.seq }.let { if (it.size > MAX_ITEMS) it.takeLast(MAX_ITEMS) else it }
        if (items.isEmpty()) return known
        return known.copy(open = open.copy(items = items, loaded = true, savedMs = savedMs, moreBefore = false))
    }

    /** Marks shown at once on [ids] (the PC's answer replaces them, a refusal puts [before] back). */
    fun marked(state: AgentsState, ids: Collection<String>, change: MarkChange): AgentsState {
        val set = ids.toSet()
        if (set.isEmpty()) return state
        return state.copy(threads = sorted(state.threads.map { if (it.id in set) AgentMarks.applied(it, change) else it }))
    }

    /** The threads as they were before a mark the PC refused. */
    fun unmarked(state: AgentsState, before: List<AgentThread>): AgentsState =
        before.fold(state) { acc, thread -> if (acc.thread(thread.id) != null) withThread(acc, thread) else acc }

    /** The PC's answer to a mark, rename, rewind or fork: each thread as it is now. */
    fun withThreadsChanged(state: AgentsState, threads: List<AgentThread>): AgentsState =
        threads.fold(state) { acc, thread -> withThread(acc, thread) }

    /** The PC took a held message or its state changed: the outbox as the core has it. */
    fun withHeld(state: AgentsState, held: List<AgentHeld>): AgentsState =
        if (state.held == held) state else state.copy(held = held.take(MAX_HELD))

    // ---- Questions (contract §Q4.4) ----

    /** A question asked, or asked again: kept by token, the oldest dropped past a bound. */
    fun withQuestion(state: AgentsState, question: QuestionInfo): AgentsState {
        if (question.token.isEmpty() || question.threadId.isEmpty()) return state
        // A question with nothing to answer can't be shown: a secret one has no asks by rule, any other must.
        if (!question.secret && question.asks.isEmpty()) return state
        val questions = state.questions + (question.token to question)
        val kept = if (questions.size > MAX_QUESTIONS) {
            questions.values.sortedBy { it.createdMs }.takeLast(MAX_QUESTIONS).associateBy { it.token }
        } else {
            questions
        }
        return state.copy(questions = kept)
    }

    /**
     * A question ended: answered on some device, dismissed, expired, or its
     * turn ended. While its thread is open, how it ended stays as its row's mark.
     */
    fun questioned(
        state: AgentsState,
        token: String,
        threadId: String,
        outcome: QuestionOutcome,
        byName: String? = null,
    ): AgentsState {
        val question = state.questions[token]
        val here = token in state.replying
        val open = state.open
        val nextOpen = if (open != null && open.threadId == threadId && question != null && question.itemId.isNotEmpty()) {
            val resolved = ResolvedQuestion(
                token = token,
                itemId = question.itemId,
                outcome = outcome,
                here = here,
                byName = byName?.takeIf { it.isNotBlank() },
            )
            open.copy(resolvedQuestions = (open.resolvedQuestions.filterNot { it.token == token } + resolved).takeLast(MAX_RESOLVED))
        } else {
            open
        }
        if (question == null && !here && nextOpen === open) return state
        return state.copy(questions = state.questions - token, replying = state.replying - token, open = nextOpen)
    }

    /** A thread's waiting questions as a page lists them, replacing what was known. */
    fun withQuestionsOf(state: AgentsState, threadId: String, questions: List<QuestionInfo>): AgentsState {
        val others = state.questions.filterValues { it.threadId != threadId }
        val fresh = questions
            .filter { it.threadId == threadId && it.token.isNotEmpty() && (it.secret || it.asks.isNotEmpty()) }
            .associateBy { it.token }
        return state.copy(
            questions = others + fresh,
            replying = state.replying.filter { it in others || it in fresh }.toSet(),
        )
    }

    /** This phone sent an answer to, or dismissed, [token]. */
    fun replying(state: AgentsState, token: String): AgentsState =
        if (token in state.questions) state.copy(replying = state.replying + token) else state

    /** A reply that didn't go through: gone removes the question, anything else lets it be answered again. */
    fun replyFailed(state: AgentsState, token: String, failure: AgentFailure): AgentsState =
        if (failure == AgentFailure.NOT_FOUND) {
            state.copy(questions = state.questions - token, replying = state.replying - token)
        } else {
            state.copy(replying = state.replying - token)
        }

    // ---- Sessions started on the PC (§3) ----

    /** The PC's list, as it sent it: it lets this phone see them. */
    fun withSessions(state: AgentsState, sessions: List<AgentSession>): AgentsState =
        state.copy(sessions = sortedSessions(sessions.distinctBy { it.id }.take(MAX_SESSIONS)), sessionsAllowed = true, sessionsUnsupported = false)

    /** The PC's switch for this phone is off: nothing listed until it's allowed there. */
    fun sessionsOff(state: AgentsState): AgentsState =
        state.copy(sessions = emptyList(), sessionsAllowed = false, sessionsUnsupported = false)

    /** The PC doesn't list sessions (an older G7 Link, or no host). */
    fun sessionsUnsupported(state: AgentsState): AgentsState =
        state.copy(sessions = emptyList(), sessionsAllowed = null, sessionsUnsupported = true)

    /** One session, added or replaced. */
    fun withSession(state: AgentsState, session: AgentSession): AgentsState {
        val others = state.sessions.filterNot { it.id == session.id }
        return state.copy(sessions = sortedSessions(others + session).take(MAX_SESSIONS), sessionsAllowed = true, sessionsUnsupported = false)
    }

    /**
     * The PC's order: what needs you (as the agent reported it) first, then
     * working, what may need you, idle, ended; the newest first within each.
     */
    fun sortedSessions(sessions: List<AgentSession>): List<AgentSession> =
        sessions.sortedWith(compareBy<AgentSession> { sessionRank(it) }.thenByDescending { it.updatedMs })

    private fun sessionRank(session: AgentSession): Int = when (session.state) {
        AgentSessionState.NEEDS_YOU -> if (session.guess) 2 else 0
        AgentSessionState.WORKING -> 1
        AgentSessionState.MAY_NEED_YOU -> 2
        AgentSessionState.IDLE -> 3
        AgentSessionState.ENDED -> 4
    }

    /** Waits for an answer as the agent reported it: counted on Home, unlike a guess ("May need you"). */
    /** How [session] reads in its status line, list row and the drawer: a usage limit over anything else. */
    fun sessionLook(session: AgentSession): SessionLook = when {
        session.limit != null -> SessionLook.LIMITED
        needsYouExactly(session) -> SessionLook.NEEDS_YOU
        mayNeedYou(session) -> SessionLook.MAY_NEED_YOU
        session.state == AgentSessionState.WORKING -> SessionLook.WORKING
        session.state == AgentSessionState.IDLE -> SessionLook.IDLE
        else -> SessionLook.ENDED
    }

    /** When a session's usage limit resets, while that's still ahead; null when it isn't limited, the agent didn't say, or it's past. */
    fun sessionLimitResets(session: AgentSession, nowMs: Long): Long? = session.limit?.resetsMs?.takeIf { it > nowMs }

    /** Whether [ms] falls on the same local day as [nowMs]: the time alone says when, else the weekday too. */
    fun sameDay(ms: Long, nowMs: Long, zone: java.util.TimeZone): Boolean {
        val a = java.util.Calendar.getInstance(zone).apply { timeInMillis = ms }
        val b = java.util.Calendar.getInstance(zone).apply { timeInMillis = nowMs }
        return a.get(java.util.Calendar.YEAR) == b.get(java.util.Calendar.YEAR) &&
            a.get(java.util.Calendar.DAY_OF_YEAR) == b.get(java.util.Calendar.DAY_OF_YEAR)
    }

    fun needsYouExactly(session: AgentSession): Boolean =
        session.state == AgentSessionState.NEEDS_YOU && !session.guess

    /** Looks like it waits, guessed from its file: shown as "May need you", never counted. */
    fun mayNeedYou(session: AgentSession): Boolean =
        session.state == AgentSessionState.MAY_NEED_YOU || (session.state == AgentSessionState.NEEDS_YOU && session.guess)

    /** The session's view is on screen (or about to be). */
    fun sessionOpened(state: AgentsState, sessionId: String): AgentsState =
        if (state.openSession?.sessionId == sessionId) state else state.copy(openSession = OpenSession(sessionId))

    /** Left the session's view: its items are dropped. */
    fun sessionClosed(state: AgentsState, sessionId: String): AgentsState =
        if (state.openSession?.sessionId == sessionId) state.copy(openSession = null) else state

    /** The newest page of a session (on open, on coming back, on reconnecting). */
    fun withSessionPage(state: AgentsState, page: AgentSessionPage): AgentsState {
        val next = withSession(state, page.session)
        val open = next.openSession?.takeIf { it.sessionId == page.session.id } ?: return next
        val items = page.items.distinctBy { it.seq }.sortedBy { it.seq }
        val kept = if (items.size > MAX_SESSION_ITEMS) items.takeLast(MAX_SESSION_ITEMS) else items
        return next.copy(
            openSession = open.copy(items = kept, moreBefore = page.moreBefore || kept.size < items.size, loaded = true, removed = false),
        )
    }

    /** An older page of a session, read with `before`: goes in front of what's shown. */
    fun withOlderSessionPage(state: AgentsState, page: AgentSessionPage): AgentsState {
        val next = withSession(state, page.session)
        val open = next.openSession?.takeIf { it.sessionId == page.session.id } ?: return next
        val first = open.items.firstOrNull()?.seq ?: ULong.MAX_VALUE
        val older = page.items.filter { it.seq < first }.distinctBy { it.seq }.sortedBy { it.seq }
        val items = older + open.items
        // The newest are kept when there are too many; the oldest can be read again.
        val kept = if (items.size > MAX_SESSION_ITEMS) items.takeLast(MAX_SESSION_ITEMS) else items
        return next.copy(openSession = open.copy(items = kept, moreBefore = page.moreBefore || kept.size < items.size))
    }

    private fun sessionRemoved(state: AgentsState, sessionId: String): AgentsState {
        val open = state.openSession
        return state.copy(
            sessions = state.sessions.filterNot { it.id == sessionId },
            openSession = if (open != null && open.sessionId == sessionId) open.copy(removed = true) else open,
        )
    }

    private fun withSessionItem(state: AgentsState, sessionId: String, item: AgentItem): AgentsState {
        val open = state.openSession?.takeIf { it.sessionId == sessionId } ?: return state
        val index = open.items.indexOfFirst { it.seq == item.seq }
        val items = if (index >= 0) {
            open.items.toMutableList().also { it[index] = item }
        } else {
            val at = open.items.indexOfFirst { it.seq > item.seq }.let { if (it < 0) open.items.size else it }
            open.items.toMutableList().also { it.add(at, item) }
        }
        val kept = if (items.size > MAX_SESSION_ITEMS) items.takeLast(MAX_SESSION_ITEMS) else items
        return state.copy(openSession = open.copy(items = kept, moreBefore = open.moreBefore || kept.size < items.size))
    }

    // ---- What a thread is doing now ----

    /** The tool still running last in the transcript: its command or change shows under "Now". */
    fun runningTool(items: List<AgentItem>): AgentItem? =
        items.lastOrNull { it.kind == AgentItemKind.TOOL && it.tool?.status == AgentToolStatus.RUNNING }

    /**
     * The plan "Implement" and "Change the plan" act on: the thread's newest
     * plan, while it's proposed (contract §Q6.1 "Plan"); null otherwise.
     */
    fun newestProposedPlan(items: List<AgentItem>): String? {
        val newest = items.lastOrNull { it.kind == AgentItemKind.PLAN } ?: return null
        // A plan without a state reads as proposed (the FFI's rule).
        val proposed = when (newest.planState) {
            AgentPlanState.PROPOSED, null -> true
            AgentPlanState.IMPLEMENTED, AgentPlanState.REPLACED -> false
        }
        return newest.id.takeIf { proposed }
    }

    /** How many of a plan's steps are done. */
    fun planDone(plan: List<AgentPlanStep>): Int = plan.count { it.status == AgentPlanStatus.COMPLETED }

    /**
     * Which step of the plan is under way, from 1 ("Step 4 of 5"): the first in
     * progress, else the first not done, else the last. Null without a plan.
     */
    fun planPosition(plan: List<AgentPlanStep>?): Int? {
        if (plan.isNullOrEmpty()) return null
        val inProgress = plan.indexOfFirst { it.status == AgentPlanStatus.IN_PROGRESS }
        if (inProgress >= 0) return inProgress + 1
        val pending = plan.indexOfFirst { it.status != AgentPlanStatus.COMPLETED }
        return if (pending >= 0) pending + 1 else plan.size
    }

    /** Time since a state began, as a clock shows it: "0:42", "4:12", "1:05:09". */
    fun elapsed(ms: Long): String {
        val total = (ms.coerceAtLeast(0L) / 1000L)
        val hours = total / 3600L
        val minutes = (total % 3600L) / 60L
        val seconds = total % 60L
        return if (hours > 0L) {
            "$hours:${two(minutes)}:${two(seconds)}"
        } else {
            "$minutes:${two(seconds)}"
        }
    }

    private fun two(value: Long): String = if (value < 10L) "0$value" else value.toString()

    // ---- Home ----

    /**
     * Home's Agents line (see [HomeLine]): a request first, counting every one
     * the phone knows of (the threads' own counts when it hasn't heard of them
     * all) and sessions on the PC that wait for an answer as the agent reported
     * it (a guess never counts); then what works, threads before sessions; then
     * the thread that ran last; else Start.
     */
    fun homeLine(state: AgentsState): HomeLine {
        val threads = state.listed
        val waitingThreads = threads.filter { needsYou(it) }
        val threadRequests = maxOf(
            state.asks.size + state.questions.size,
            threads.sumOf { it.asks.toInt() + it.asking.toInt() },
            waitingThreads.size,
        )
        val waitingSessions = state.sessions.filter { needsYouExactly(it) }
        val needs = threadRequests + waitingSessions.size
        if (needs > 0) {
            val asker = state.asks.values.minByOrNull { it.expiresMs }?.agent
                ?: state.questions.values.minByOrNull { it.expiresMs }?.agent
                ?: waitingThreads.maxByOrNull { it.updatedMs }?.agent
                ?: waitingSessions.maxByOrNull { it.updatedMs }?.agent
                ?: ""
            return HomeLine.NeedsYou(asker, needs)
        }
        val working = threads.filter { it.state == AgentThreadState.WORKING }
        if (working.isNotEmpty()) {
            val one = working.maxBy { it.updatedMs }
            return HomeLine.Working(one.agent, one.sinceMs?.takeIf { it > 0L }, working.size, sessions = false)
        }
        val workingSessions = state.sessions.filter { it.state == AgentSessionState.WORKING }
        if (workingSessions.isNotEmpty()) {
            val one = workingSessions.maxBy { it.updatedMs }
            return HomeLine.Working(one.agent, one.sinceMs?.takeIf { it > 0L }, workingSessions.size, sessions = true)
        }
        val last = threads.filterNot { running(it) }.maxByOrNull { it.updatedMs }
        return if (last != null) HomeLine.Finished(last.title, status(last), last.updatedMs) else HomeLine.Start
    }

    /**
     * The drawer's groups, in the list's order (requests first, then the newest):
     * Needs you (a request waits), Working (no request), Pinned, Recent
     * (everything else that stopped), and the sessions the PC lists. Archived
     * and snoozed threads are on their shelves, not here.
     */
    fun drawerGroups(state: AgentsState): DrawerGroups {
        val needsYou = ArrayList<AgentThread>()
        val working = ArrayList<AgentThread>()
        val pinned = ArrayList<AgentThread>()
        val recent = ArrayList<AgentThread>()
        for (thread in state.listed) {
            when {
                needsYou(thread) -> needsYou += thread
                thread.state == AgentThreadState.WORKING -> working += thread
                thread.pinned -> pinned += thread
                else -> recent += thread
            }
        }
        val sessions = if (state.sessionsAllowed == true && !state.sessionsUnsupported) state.sessions else emptyList()
        return DrawerGroups(needsYou, working, recent, sessions, pinned)
    }

    /**
     * The main list as the PC sent it: replaces the threads known of it, and
     * keeps those of the shelves (archived, snoozed) the PC lists apart.
     */
    fun withThreads(state: AgentsState, threads: List<AgentThread>, nowMs: Long? = null): AgentsState {
        val fresh = threads.distinctBy { it.id }
        val ids = fresh.mapTo(HashSet()) { it.id }
        val shelved = state.threads.filter { it.id !in ids && AgentShelves.shelfOf(it) != Shelf.MAIN }
        val next = state.copy(threads = sorted((fresh + shelved).take(MAX_THREADS)), savedMs = null)
        return if (nowMs != null) next.copy(listedMs = nowMs) else next
    }

    /** One thread, added or replaced. */
    fun withThread(state: AgentsState, thread: AgentThread): AgentsState {
        val others = state.threads.filterNot { it.id == thread.id }
        return state.copy(threads = sorted((others + thread).take(MAX_THREADS)))
    }

    /** Requests waiting first, then the most recently changed. */
    fun sorted(threads: List<AgentThread>): List<AgentThread> =
        threads.sortedWith(
            compareByDescending<AgentThread> { needsYou(it) }
                .thenByDescending { it.updatedMs },
        )

    /** The transcript of [threadId] is on screen (or about to be). */
    fun opened(state: AgentsState, threadId: String): AgentsState =
        if (state.open?.threadId == threadId) state else state.copy(open = OpenThread(threadId))

    /** Left the transcript: its items are dropped. */
    fun closed(state: AgentsState, threadId: String): AgentsState =
        if (state.open?.threadId == threadId) state.copy(open = null) else state

    /** The newest page of a thread (on open, on coming back, and after a gap). */
    fun withPage(state: AgentsState, page: AgentPage): AgentsState {
        val threadId = page.thread.id
        val withAsks = withAsksOf(withThread(state, page.thread), threadId, page.asks)
        val next = withQuestionsOf(withAsks, threadId, page.questions.map(AgentControlFfi::question))
        val open = next.open?.takeIf { it.threadId == threadId } ?: return next
        val items = page.items.distinctBy { it.seq }.sortedBy { it.seq }
        val pageIds = items.mapTo(HashSet()) { it.id }
        val newest = items.lastOrNull()?.seq ?: 0uL
        // Text still streaming past the page's end is an assistant message in progress: keep it.
        val streaming = open.streaming.filter { (id, text) -> id !in pageIds && text.seq > newest }.toMutableMap()
        // A thought still going on is in the page with its text so far, and its deltas carry on from there.
        for (item in items) {
            if (!thinking(item)) continue
            val text = item.text.orEmpty().take(MAX_STREAM_CHARS)
            val kept = open.streaming[item.id]
            streaming[item.id] = if (kept != null && kept.text.length >= text.length) {
                kept
            } else {
                StreamingText(item.seq, text, utf8Length(text), kind = item.kind)
            }
        }
        return next.copy(
            open = open.copy(
                items = items,
                streaming = streaming,
                moreBefore = page.moreBefore,
                loaded = true,
                resolved = open.resolved.filter { it.afterSeq >= (items.firstOrNull()?.seq ?: 0uL) },
                savedMs = null,
            ),
        )
    }

    /** An older page, read with `before`: goes in front of what's shown. */
    fun withOlder(state: AgentsState, page: AgentPage): AgentsState {
        val threadId = page.thread.id
        val next = withThread(state, page.thread)
        val open = next.open?.takeIf { it.threadId == threadId } ?: return next
        val first = open.items.firstOrNull()?.seq ?: ULong.MAX_VALUE
        val older = page.items.filter { it.seq < first }.distinctBy { it.seq }.sortedBy { it.seq }
        val items = older + open.items
        // The newest are kept when there are too many; the oldest can be read again.
        val kept = if (items.size > MAX_ITEMS) items.takeLast(MAX_ITEMS) else items
        return next.copy(
            open = open.copy(items = kept, moreBefore = page.moreBefore || kept.size < items.size),
        )
    }

    /** This phone sent an answer for [token]. */
    fun answering(state: AgentsState, token: String): AgentsState =
        if (token in state.asks) state.copy(answering = state.answering + token) else state

    /** An answer that didn't go through: gone ([AgentFailure.NOT_FOUND]) removes the request, anything else lets it be answered again. */
    fun answerFailed(state: AgentsState, token: String, failure: AgentFailure): AgentsState =
        if (failure == AgentFailure.NOT_FOUND) {
            state.copy(asks = state.asks - token, answering = state.answering - token)
        } else {
            state.copy(answering = state.answering - token)
        }

    /** A request or a question waits for an answer in it. */
    fun needsYou(thread: AgentThread): Boolean =
        thread.state == AgentThreadState.WAITING || thread.asks > 0u || thread.asking > 0u

    /** The thread's state as words: usage limits read as Paused, other failures as Failed, a plan-mode turn that ended as Plan ready. */
    fun status(thread: AgentThread): ThreadStatus = when (thread.state) {
        AgentThreadState.WORKING -> if (thread.asking > 0u) ThreadStatus.NeedsYou else ThreadStatus.Working
        AgentThreadState.WAITING -> ThreadStatus.NeedsYou
        AgentThreadState.DONE -> if (AgentControlFfi.mode(thread.mode) == ThreadMode.PLAN) ThreadStatus.PlanReady else ThreadStatus.Done
        AgentThreadState.INTERRUPTED -> ThreadStatus.Stopped
        AgentThreadState.FAILED -> failure(thread.error, thread.errorCode)
    }

    /**
     * A failed thread's error: a usage limit (code `usage_limit`; the host
     * appends `|resets_ms=<ms>` to the message when the CLI said when it resets)
     * is Paused, anything else Failed with the CLI's message, marked when the
     * agent is signed out (code `login`).
     */
    fun failure(error: String?, code: String? = null): ThreadStatus {
        val text = error?.trim().orEmpty()
        val marker = text.indexOf(RESETS_MARKER)
        val resets = if (marker >= 0) {
            text.substring(marker + RESETS_MARKER.length).takeWhile { it.isDigit() }.take(18).toLongOrNull()?.takeIf { it > 0L }
        } else {
            null
        }
        return when {
            code == USAGE_LIMIT || marker >= 0 -> ThreadStatus.Paused(resets)
            code == LOGIN -> ThreadStatus.Failed(shownError(text), login = true)
            // A host from before the code reached the phone: its strings are English.
            code == null && text.contains("usage limit", ignoreCase = true) -> ThreadStatus.Paused(null)
            else -> ThreadStatus.Failed(shownError(text))
        }
    }

    /** An error message without the host's machine-readable suffix. */
    fun shownError(error: String?): String? =
        error?.substringBefore(RESETS_MARKER)?.trim()?.ifEmpty { null }

    /** Whether a thread is running a turn: it can be stopped, not sent to. */
    fun running(thread: AgentThread): Boolean =
        thread.state == AgentThreadState.WORKING || thread.state == AgentThreadState.WAITING

    /** Whether text can be sent as a prompt or reply: not blank and within both limits. */
    fun fits(text: String): Boolean =
        text.isNotBlank() && text.length <= MAX_PROMPT_CHARS && utf8Length(text) <= MAX_PROMPT_BYTES

    /** Over a limit (as opposed to empty). */
    fun tooLong(text: String): Boolean = text.length > MAX_PROMPT_CHARS || utf8Length(text) > MAX_PROMPT_BYTES

    /**
     * Whether a request can be allowed from a notification: a command shown
     * whole on one line of at most 100 characters. Anything else is reviewed
     * in the app.
     */
    fun allowableInNotification(ask: AgentAsk): Boolean {
        if (ask.kind != AgentAskKind.COMMAND || ask.truncated) return false
        val command = ask.command?.trim() ?: return false
        return command.isNotEmpty() && command.length <= 100 && command.none { it == '\n' || it == '\r' }
    }

    /** Android lets an action require an unlocked phone from 12 (S) on. */
    const val NOTICE_AUTH_SDK = 31

    /** The longest option label a notification button takes; longer ones are answered in the app. */
    const val MAX_NOTICE_OPTION_CHARS = 20

    /** The most a reason given with Deny may be: the wire's `MAX_DENY_MESSAGE`. */
    const val MAX_DENY_BYTES = 2000

    /**
     * Whether [question] can be answered from its notification, and how
     * (contract §V3.2): on Android 12 or later, not secret, exactly one ask
     * that takes one answer, every option one line of at most 20 characters,
     * and two options (with "Type" when the ask takes the user's own words) or
     * three that don't. Anything else is answered in the app.
     */
    fun answerableInNotification(question: QuestionInfo, sdkInt: Int): NoticeAnswers? {
        if (sdkInt < NOTICE_AUTH_SDK || question.secret) return null
        val ask = question.asks.singleOrNull() ?: return null
        if (ask.multi || ask.id.isEmpty()) return null
        val labels = ask.options.map { it.label }
        val fit = labels.all { label ->
            label.isNotBlank() &&
                label.codePointCount(0, label.length) <= MAX_NOTICE_OPTION_CHARS &&
                label.none { it == '\n' || it == '\r' || it == ' ' || it == ' ' }
        }
        if (!fit || labels.toSet().size != labels.size) return null
        return when {
            labels.size == 2 -> NoticeAnswers(ask.id, labels, type = ask.other)
            labels.size == 3 && !ask.other -> NoticeAnswers(ask.id, labels, type = false)
            else -> null
        }
    }

    /**
     * The reason typed under "Say why", as it's sent: trimmed, cut on a
     * character boundary at the wire's 2000 bytes; null when nothing was typed
     * (nothing is sent then).
     */
    fun denyReason(typed: CharSequence?): String? =
        typed?.toString()?.trim()?.ifEmpty { null }?.let { cutUtf8(it, MAX_DENY_BYTES).trimEnd() }

    /** An answer typed under "Type": trimmed, at most the wire's 500 bytes; null when empty. */
    fun typedAnswer(typed: CharSequence?): String? =
        typed?.toString()?.trim()?.ifEmpty { null }?.let { cutUtf8(it, QuestionDock.MAX_OTHER_BYTES).trimEnd() }

    /**
     * The answer an option or typed words from a notification sends, read from
     * the question as it is now; null when it's gone or changed (the question
     * is no longer answerable there, its ask isn't [askId], the option isn't
     * there, or the words don't fit).
     */
    fun noticeAnswer(question: QuestionInfo?, askId: String, option: Int?, typed: String?, sdkInt: Int): QuestionAnswer? {
        val answers = question?.let { answerableInNotification(it, sdkInt) } ?: return null
        if (answers.askId != askId) return null
        return when {
            option != null && typed == null -> answers.options.getOrNull(option)?.let { QuestionAnswer(askId, listOf(it), other = null) }
            option == null && typed != null && answers.type ->
                typed.takeIf { it.isNotBlank() && utf8Length(it) <= QuestionDock.MAX_OTHER_BYTES }?.let { QuestionAnswer(askId, emptyList(), other = it) }
            else -> null
        }
    }

    /** [text] cut to at most [maxBytes] of UTF-8, never inside a character. */
    fun cutUtf8(text: String, maxBytes: Int): String {
        if (maxBytes <= 0) return ""
        var bytes = 0
        var i = 0
        while (i < text.length) {
            val c = text[i]
            val pair = Character.isHighSurrogate(c) && i + 1 < text.length && Character.isLowSurrogate(text[i + 1])
            val size = when {
                c.code < 0x80 -> 1
                c.code < 0x800 -> 2
                pair -> 4
                else -> 3
            }
            if (bytes + size > maxBytes) return text.substring(0, i)
            bytes += size
            i += if (pair) 2 else 1
        }
        return text
    }

    /** The command or change a request shows; null when it shows only its title. */
    fun askBody(ask: AgentAsk): String? = when (ask.kind) {
        AgentAskKind.COMMAND -> ask.command
        AgentAskKind.EDIT -> ask.diff ?: ask.path
        AgentAskKind.OTHER -> ask.command ?: ask.diff
    }?.takeIf { it.isNotBlank() }

    private fun withAsk(state: AgentsState, ask: AgentAsk): AgentsState {
        if (ask.token.isEmpty()) return state
        val asks = state.asks + (ask.token to ask)
        // A bound on what a PC can make the phone keep; the oldest go first.
        val kept = if (asks.size > MAX_ASKS) asks.values.sortedBy { it.expiresMs }.takeLast(MAX_ASKS).associateBy { it.token } else asks
        return state.copy(asks = kept)
    }

    /** A thread's waiting requests as a page lists them, replacing what was known. */
    private fun withAsksOf(state: AgentsState, threadId: String, asks: List<AgentAsk>): AgentsState {
        val others = state.asks.filterValues { it.thread != threadId }
        val fresh = asks.filter { it.thread == threadId && it.token.isNotEmpty() }.associateBy { it.token }
        val tokens = fresh.keys
        return state.copy(
            asks = others + fresh,
            answering = state.answering.filter { it in others || it in tokens }.toSet(),
        )
    }

    private fun asked(
        state: AgentsState,
        token: String,
        threadId: String,
        outcome: AgentAskOutcome,
        byName: String? = null,
    ): AgentsState {
        val ask = state.asks[token]
        val here = token in state.answering
        val open = state.open
        val nextOpen = if (open != null && open.threadId == threadId && ask != null) {
            val resolved = ResolvedAsk(
                token = token,
                title = ask.title,
                outcome = outcome,
                here = here,
                afterSeq = open.lastSeq,
                byName = byName?.takeIf { it.isNotBlank() },
                itemId = ask.item?.takeIf { it.isNotBlank() },
            )
            open.copy(resolved = (open.resolved.filterNot { it.token == token } + resolved).takeLast(MAX_RESOLVED))
        } else {
            open
        }
        return state.copy(asks = state.asks - token, answering = state.answering - token, open = nextOpen)
    }

    private fun removed(state: AgentsState, threadId: String): AgentsState {
        val gone = state.asks.filterValues { it.thread == threadId }.keys
        val goneQuestions = state.questions.filterValues { it.threadId == threadId }.keys
        val open = state.open
        return state.copy(
            threads = state.threads.filterNot { it.id == threadId },
            asks = state.asks - gone,
            answering = state.answering - gone,
            questions = state.questions - goneQuestions,
            replying = state.replying - goneQuestions,
            open = if (open != null && open.threadId == threadId) open.copy(removed = true) else open,
            gitStrips = state.gitStrips - threadId,
            scriptRuns = AgentScripts.withoutThread(state.scriptRuns, threadId),
        )
    }

    private fun delta(state: AgentsState, event: Event.AgentDelta): AgentsUpdate {
        val open = state.open?.takeIf { it.threadId == event.thread } ?: return AgentsUpdate(state)
        // An answer's item comes when it's finished, so the delta is late; a thought's comes
        // when it starts, and its text arrives until it ends.
        val item = open.items.firstOrNull { it.id == event.itemId }
        if (item != null && !thinking(item)) return AgentsUpdate(state)
        val kind = item?.kind ?: AgentItemKind.ASSISTANT
        val current = open.streaming[event.itemId]
        val bytes = utf8Length(event.text)
        val offset = event.offset.toLong()
        val next: StreamingText
        var reread: String? = null
        when {
            current == null -> {
                // A new message: a seq past the next one means items were missed.
                if (open.loaded && event.seq > open.lastSeq + 1uL) reread = open.threadId
                next = if (offset == 0L) {
                    StreamingText(event.seq, event.text, bytes, kind = kind)
                } else {
                    // Joined part way (opened while it streamed): the finished item brings the rest.
                    StreamingText(event.seq, "", 0, gap = true, kind = kind)
                }
            }
            current.gap -> return AgentsUpdate(state)
            offset == current.bytes.toLong() -> {
                next = if (current.text.length + event.text.length > MAX_STREAM_CHARS) {
                    current.copy(gap = true)
                } else {
                    current.copy(text = current.text + event.text, bytes = current.bytes + bytes)
                }
            }
            // Text it already has, sent again.
            offset + bytes <= current.bytes.toLong() -> return AgentsUpdate(state)
            else -> {
                next = current.copy(gap = true)
                reread = open.threadId
            }
        }
        return AgentsUpdate(state.copy(open = open.copy(streaming = open.streaming + (event.itemId to next))), reread)
    }

    private fun itemChanged(state: AgentsState, event: Event.AgentItemChanged): AgentsUpdate {
        val open = state.open?.takeIf { it.threadId == event.thread } ?: return AgentsUpdate(state)
        val item = event.item
        val reread = if (open.loaded && item.seq > open.lastSeq + 1uL) open.threadId else null
        val index = open.items.indexOfFirst { it.seq == item.seq }
        val items = if (index >= 0) {
            open.items.toMutableList().also { it[index] = item }
        } else {
            val at = open.items.indexOfFirst { it.seq > item.seq }.let { if (it < 0) open.items.size else it }
            open.items.toMutableList().also { it.add(at, item) }
        }
        val kept = if (items.size > MAX_ITEMS) items.takeLast(MAX_ITEMS) else items
        return AgentsUpdate(
            state.copy(
                open = open.copy(
                    items = kept,
                    // A thought that hasn't ended keeps the text that arrived for it.
                    streaming = if (thinking(item)) open.streaming else open.streaming - item.id,
                    moreBefore = open.moreBefore || kept.size < items.size,
                ),
            ),
            reread,
        )
    }

    /**
     * An item whose text still arrives as deltas: a thought that hasn't ended,
     * or a plan Codex is still writing (it opens empty, then its text streams).
     */
    private fun thinking(item: AgentItem): Boolean = when (item.kind) {
        AgentItemKind.REASONING -> item.endMs == null
        AgentItemKind.PLAN -> item.endMs == null && item.text.isNullOrEmpty()
        AgentItemKind.USER, AgentItemKind.ASSISTANT, AgentItemKind.TOOL, AgentItemKind.NOTICE, AgentItemKind.QUESTION -> false
    }

    // ---- The timeline's inputs (contract §P6.1) ----

    /**
     * What the open thread's rows are made from, or null before its first page:
     * its items, the text still arriving, its state, the requests at their steps
     * and the rows opened. Equal inputs make equal rows, so the rows are made
     * again only when this changes (an item, a delta, the state, a request, a tap).
     */
    fun timelineInput(state: AgentsState, threadId: String, openKeys: Set<String>, held: (String) -> Boolean = { false }): TimelineInput? {
        val open = state.open?.takeIf { it.threadId == threadId && it.loaded } ?: return null
        val thread = state.thread(threadId) ?: return null
        val streaming = open.streaming
            .filter { (_, text) -> !text.gap || text.text.isNotEmpty() }
            .map { (id, text) -> AgentStreaming(itemId = id, seq = text.seq, kind = text.kind, text = text.text) }
            .sortedBy { it.seq }
        return TimelineInput(
            key = threadId,
            items = open.items,
            streaming = streaming,
            state = thread.state,
            stateSinceMs = thread.sinceMs ?: 0L,
            marks = marks(state, threadId, held),
            open = openKeys.sorted(),
            questions = state.questionsFor(threadId),
            turns = open.turns,
            turnFiles = open.turnFiles.entries.sortedBy { it.key }.map { (turn, files) -> AgentTurnPaths(turn, files) },
        )
    }

    /** A followed session's rows: its items as they are, no requests (they're answered on the PC). */
    fun sessionTimelineInput(open: OpenSession, session: AgentSession, openKeys: Set<String>): TimelineInput? {
        if (!open.loaded || open.sessionId != session.id) return null
        val threadState = when (session.state) {
            AgentSessionState.WORKING -> AgentThreadState.WORKING
            AgentSessionState.NEEDS_YOU, AgentSessionState.MAY_NEED_YOU -> AgentThreadState.WAITING
            AgentSessionState.IDLE, AgentSessionState.ENDED -> AgentThreadState.DONE
        }
        return TimelineInput(
            key = "session:${session.id}",
            items = open.items,
            streaming = emptyList(),
            state = threadState,
            stateSinceMs = session.sinceMs ?: 0L,
            marks = emptyList(),
            open = openKeys.sorted(),
        )
    }

    /**
     * The requests of a thread at their steps: each waiting one ([AgentMarkKind.HELD]
     * while the typing guard holds it), and those answered while it was open when
     * the mark says something the step's own line doesn't (reconcile file, item 7):
     * an answer from another device, one that expired or was cancelled. An answer
     * given here shows only in the step's line; a step the PC's rule allowed
     * (`AgentTool.byRule`) gets its "Allowed by your rule" mark from the FFI.
     */
    fun marks(state: AgentsState, threadId: String, held: (String) -> Boolean = { false }): List<AgentMark> {
        val waiting = state.asksFor(threadId).map { ask ->
            // item_id, kind, by (`by` is a soft keyword in Kotlin, so not named here).
            AgentMark(ask.item.orEmpty(), if (held(ask.token)) AgentMarkKind.HELD else AgentMarkKind.WAITING, "")
        } + state.questionsFor(threadId).map { question ->
            // A question's mark sits under its Question row, as a request's under its step (contract §Q2.3).
            AgentMark(question.itemId, if (held(question.token)) AgentMarkKind.HELD else AgentMarkKind.WAITING, "")
        }
        val open = state.open?.takeIf { it.threadId == threadId } ?: return waiting
        val answered = open.resolved.mapNotNull { ask ->
            val kind = when (ask.outcome) {
                AgentAskOutcome.ALLOWED -> if (ask.here) null else AgentMarkKind.ALLOWED
                AgentAskOutcome.DENIED -> if (ask.here) null else AgentMarkKind.DENIED
                AgentAskOutcome.EXPIRED -> AgentMarkKind.EXPIRED
                AgentAskOutcome.CANCELLED -> AgentMarkKind.CANCELLED
            } ?: return@mapNotNull null
            AgentMark(ask.itemId.orEmpty(), kind, ask.byName.orEmpty())
        }
        // A question that ended says how on its row: answered (here, or on another device), dismissed, not answered.
        val questionsEnded = open.resolvedQuestions.map { question ->
            val kind = when (question.outcome) {
                QuestionOutcome.ANSWERED -> AgentMarkKind.ALLOWED
                QuestionOutcome.DISMISSED -> AgentMarkKind.DENIED
                QuestionOutcome.EXPIRED -> AgentMarkKind.EXPIRED
                QuestionOutcome.CANCELLED -> AgentMarkKind.CANCELLED
            }
            AgentMark(question.itemId, kind, if (question.here) "" else question.byName.orEmpty())
        }
        // One mark per step: a request asked again after an answer shows as waiting.
        val waitingAt = waiting.mapNotNullTo(HashSet()) { it.itemId.ifEmpty { null } }
        return (answered + questionsEnded).filter { it.itemId.isEmpty() || it.itemId !in waitingAt } + waiting
    }

    /** A PC's device id is longer than the wire's ids (z-base-32 of 32 bytes). */
    const val MAX_PC_ID = 128

    /** An id from outside (an intent, a notification): `[a-z0-9]`, 1 to [max] long (§2.2 limits). */
    fun isId(value: String, max: Int = 40): Boolean =
        value.length in 1..max && value.all { it in 'a'..'z' || it in '0'..'9' }

    /** UTF-8 length without encoding, as the wire's byte offsets and limits count. */
    fun utf8Length(text: String): Int {
        var bytes = 0
        var i = 0
        while (i < text.length) {
            val c = text[i]
            when {
                c.code < 0x80 -> bytes += 1
                c.code < 0x800 -> bytes += 2
                Character.isHighSurrogate(c) && i + 1 < text.length && Character.isLowSurrogate(text[i + 1]) -> {
                    bytes += 4
                    i++
                }
                else -> bytes += 3
            }
            i++
        }
        return bytes
    }
}
