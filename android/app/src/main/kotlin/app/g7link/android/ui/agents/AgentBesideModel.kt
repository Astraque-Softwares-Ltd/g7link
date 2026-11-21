// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable
import app.g7link.core.AgentAsk
import app.g7link.core.AgentCheckpoint
import app.g7link.core.AgentItem
import app.g7link.core.AgentItemKind
import app.g7link.core.AgentPane
import app.g7link.core.AgentPlanStep
import app.g7link.core.AgentToolClass
import app.g7link.core.AgentToolStatus
import app.g7link.core.AgentTurn

/** The three panes of the phone beside the PC (contract §V9.1), one tap apart. */
enum class BesidePane { CHANGES, PLAN, REQUESTS }

/** The PC asked this phone to show [pane] of [threadId] beside it (`Event.AgentBeside`). */
@Immutable
data class BesideAsk(val pcId: String, val threadId: String, val pane: AgentPane)

/** What the Changes pane shows. */
@Immutable
sealed interface BesideChanges {
    /** The edits so far, newest first, each opening its diff: while a turn runs, or a turn with no record. */
    data class Edits(val items: List<AgentItem>) : BesideChanges

    /** The turn that ended, as the PC recorded it: its files, each opening its diff. */
    data class Files(val turn: UInt) : BesideChanges

    /** The turn ended and its record is being made ("Recording changes…"). */
    data object Recording : BesideChanges

    /** "Nothing changed yet." */
    data object Empty : BesideChanges
}

/** What the Plan pane shows. */
@Immutable
sealed interface BesidePlan {
    /** The task list, [current] marked (from 1); null when every step is done. */
    data class Tasks(val steps: List<AgentPlanStep>, val current: Int?) : BesidePlan

    /** The plan card of item [itemId]. */
    data class Card(val itemId: String) : BesidePlan

    /** "No plan in this thread." */
    data object None : BesidePlan
}

/** Something waiting in the thread, as the Requests pane lists it. */
@Immutable
sealed interface BesideRequest {
    val token: String
    val expiresMs: Long

    data class Ask(val ask: AgentAsk) : BesideRequest {
        override val token: String get() = ask.token
        override val expiresMs: Long get() = ask.expiresMs
    }

    data class Question(val question: QuestionInfo) : BesideRequest {
        override val token: String get() = question.token
        override val expiresMs: Long get() = question.expiresMs
    }
}

/**
 * The rules of the phone beside the PC (contract §V9.1): when it's offered,
 * which pane it opens on and follows, when it leaves, how long the screen
 * stays awake, and what each pane shows. It only ever changes what this
 * phone shows: nothing here sends a message or answers anything.
 */
object AgentBeside {
    /** It leaves once the PC hasn't shown the thread for longer than this. */
    const val LEAVE_AFTER_MS = 60_000L

    /** The screen stays awake while a turn runs, and never longer than this at a time. */
    const val AWAKE_MAX_MS = 10 * 60_000L

    /** The most edits the Changes pane lists while a turn runs (the newest). */
    const val MAX_EDITS = 30

    /**
     * "<PC> is showing this thread" with Show beside: the PC said so lately,
     * someone is at it (not locked), and the thread is on its screen.
     */
    fun offered(presence: PcPresence?, threadId: String, nowMs: Long): Boolean =
        presence != null && presence.fresh(nowMs) && presence.atPc && !presence.locked && threadId in presence.viewing

    /** The PC shows the thread now, as far as this phone knows; stale or unknown is "no". */
    fun shownOnPc(presence: PcPresence?, threadId: String, nowMs: Long): Boolean = presence?.shows(threadId, nowMs) == true

    /**
     * Since when the PC hasn't shown the thread, after it said [shown]: null
     * while it shows; the earlier time while it still doesn't.
     */
    fun stoppedAt(previousMs: Long?, shown: Boolean, nowMs: Long): Long? = if (shown) null else previousMs ?: nowMs

    /** When beside mode ends by itself, 60 s after the PC stopped showing the thread; null while it shows. */
    fun leaveAtMs(stoppedAtMs: Long?): Long? = stoppedAtMs?.plus(LEAVE_AFTER_MS)

    /** It's time to go back to the thread. */
    fun leaves(stoppedAtMs: Long?, nowMs: Long): Boolean = stoppedAtMs != null && nowMs - stoppedAtMs > LEAVE_AFTER_MS

    /**
     * Until when the screen is kept awake: [runningSinceMs] is when this
     * screen first saw the turn running (null: none runs); at most 10 minutes.
     */
    fun awakeUntilMs(runningSinceMs: Long?): Long? = runningSinceMs?.plus(AWAKE_MAX_MS)

    fun keepsAwake(runningSinceMs: Long?, nowMs: Long): Boolean = runningSinceMs != null && nowMs < runningSinceMs + AWAKE_MAX_MS

    /** The pane it opens on: Requests when something waits, else Changes. */
    fun firstPane(waiting: Boolean): BesidePane = if (waiting) BesidePane.REQUESTS else BesidePane.CHANGES

    /** The pane said to the PC (`agent/viewing`): null is the thread itself. */
    fun wire(pane: BesidePane?): AgentPane = when (pane) {
        null -> AgentPane.THREAD
        BesidePane.CHANGES -> AgentPane.CHANGES
        BesidePane.PLAN -> AgentPane.PLAN
        BesidePane.REQUESTS -> AgentPane.REQUESTS
    }

    /**
     * The PC's switch ([asked], for [askedThread]) while this phone shows
     * [current] of [shownThread]: only a phone beside that thread follows it;
     * "the thread" takes it back there. The PC never opens beside mode.
     */
    fun follow(current: BesidePane?, shownThread: String, askedThread: String, asked: AgentPane): BesidePane? {
        if (current == null || askedThread != shownThread) return current
        return when (asked) {
            AgentPane.THREAD -> null
            AgentPane.CHANGES -> BesidePane.CHANGES
            AgentPane.PLAN -> BesidePane.PLAN
            AgentPane.REQUESTS -> BesidePane.REQUESTS
        }
    }

    /**
     * The Changes pane: while a turn runs, its edits so far, newest first;
     * after it, the files the PC recorded for it (or that it's recording
     * them); a turn the PC kept no record of shows its edits instead.
     */
    fun changes(running: Boolean, items: List<AgentItem>, turns: List<AgentTurn>): BesideChanges {
        if (!running) {
            val last = turns.maxByOrNull { it.turn }
            when (last?.checkpoint) {
                AgentCheckpoint.READY -> return if (last.files > 0u) BesideChanges.Files(last.turn) else BesideChanges.Empty
                AgentCheckpoint.PENDING -> return BesideChanges.Recording
                AgentCheckpoint.MISSING, AgentCheckpoint.NOT_GIT, AgentCheckpoint.EXPIRED, null -> Unit
            }
        }
        val edits = edits(items)
        return if (edits.isEmpty()) BesideChanges.Empty else BesideChanges.Edits(edits)
    }

    /**
     * The newest turn's edits that went through, newest first, at most
     * [MAX_EDITS]: its items when they say their turn, else what follows the
     * last message sent.
     */
    fun edits(items: List<AgentItem>): List<AgentItem> {
        val turn = items.maxOfOrNull { it.turn } ?: return emptyList()
        val ofTurn = if (turn > 0u) items.filter { it.turn == turn } else items.takeLastWhile { it.kind != AgentItemKind.USER }
        return ofTurn
            .filter { item ->
                val tool = item.tool
                item.kind == AgentItemKind.TOOL && tool != null && tool.toolClass == AgentToolClass.EDIT &&
                    tool.status == AgentToolStatus.OK && !tool.detail.isNullOrBlank()
            }
            .asReversed()
            .take(MAX_EDITS)
    }

    /**
     * The Plan pane: a plan waiting for Implement first, then the task list
     * with the step under way marked, then the newest plan; else none.
     */
    fun plan(tasks: List<AgentPlanStep>?, items: List<AgentItem>): BesidePlan {
        AgentsModel.newestProposedPlan(items)?.let { return BesidePlan.Card(it) }
        if (!tasks.isNullOrEmpty()) {
            val current = AgentsModel.planPosition(tasks)?.takeIf { AgentsModel.planDone(tasks) < tasks.size }
            return BesidePlan.Tasks(tasks, current)
        }
        val newest = items.lastOrNull { it.kind == AgentItemKind.PLAN } ?: return BesidePlan.None
        return BesidePlan.Card(newest.id)
    }

    /**
     * The Requests pane: every waiting request and question of the thread,
     * oldest first (they all last as long, so the one ending first was asked
     * first; a request before a question asked at the same moment, as the dock
     * orders them).
     */
    fun requests(asks: List<AgentAsk>, questions: List<QuestionInfo>): List<BesideRequest> =
        (asks.map { BesideRequest.Ask(it) } + questions.map { BesideRequest.Question(it) })
            .sortedWith(compareBy<BesideRequest> { it.expiresMs }.thenBy { if (it is BesideRequest.Ask) 0 else 1 })

    /**
     * The one request with buttons: the dock's, so Allow, Deny and the answers
     * are the dock's own with its typing guard. That's the oldest unless the
     * guard holds it; none when nothing waits or the dock shows another thread's.
     */
    fun withButtons(requests: List<BesideRequest>, dock: DockShown?): String? {
        val token = when (dock) {
            is DockShown.Approval -> dock.queue.shown.token
            is DockShown.Question -> dock.queue.shown.token
            null -> return null
        }
        return token.takeIf { shown -> requests.any { it.token == shown } }
    }
}
