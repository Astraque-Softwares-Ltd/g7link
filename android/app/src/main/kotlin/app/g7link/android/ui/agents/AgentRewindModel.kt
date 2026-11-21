// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable
import app.g7link.core.AgentAct
import app.g7link.core.AgentActs
import app.g7link.core.AgentDraft
import app.g7link.core.AgentDraftSaved
import app.g7link.core.AgentRewindBlocked
import app.g7link.core.AgentRewindPlan

/** What a message's long-press sheet offers (contract §U9.1 "Per-message actions", board 20). */
enum class MessageAction { COPY, EDIT, REWIND, FORK }

/** Why an action can't be done now, said under it. */
@Immutable
sealed interface ActBlock {
    /** "%1 can only be forked from its latest message." */
    data object LatestOnly : ActBlock

    /** "No restore point for this turn." */
    data object NoRestorePoint : ActBlock

    /** "This isn't a git repository, so G7 can't undo file changes here." */
    data object NotGit : ActBlock

    /** "Needs %1": the PC can't be reached. */
    data object NeedsPc : ActBlock
}

/** One row of the sheet: shown on, or off with its reason. */
@Immutable
data class MessageActionRow(val action: MessageAction, val block: ActBlock? = null)

object AgentMessageActions {
    /**
     * The sheet of a row: Copy always; then Edit from here, Rewind to here and
     * Fork from here as the row's `acts` allow (all hidden while the thread
     * works or waits: the render crate leaves them out then); each disabled
     * with its reason, or with "Needs <PC>" while the PC can't be reached.
     */
    fun rows(acts: AgentActs?, user: Boolean, offline: Boolean): List<MessageActionRow> = buildList {
        add(MessageActionRow(MessageAction.COPY))
        if (acts == null) return@buildList
        if (user) {
            row(MessageAction.EDIT, acts.edit, offline)?.let(::add)
            row(MessageAction.REWIND, acts.rewind, offline)?.let(::add)
        }
        row(MessageAction.FORK, acts.fork, offline)?.let(::add)
    }

    private fun row(action: MessageAction, act: AgentAct, offline: Boolean): MessageActionRow? {
        val block = when (act) {
            AgentAct.HIDDEN -> return null
            AgentAct.ON -> null
            AgentAct.LATEST_ONLY -> ActBlock.LatestOnly
            AgentAct.NO_RESTORE_POINT -> ActBlock.NoRestorePoint
            AgentAct.NOT_GIT -> ActBlock.NotGit
        }
        return MessageActionRow(action, block ?: if (offline) ActBlock.NeedsPc else null)
    }
}

/** The rewind sheet (board 21): conversation only from the phone. */
@Immutable
sealed interface RewindSheet {
    /** The turn whose message goes back; [refill]: Edit from here. */
    val turn: UInt
    val refill: Boolean

    data class Loading(override val turn: UInt, override val refill: Boolean) : RewindSheet

    data class Ready(override val turn: UInt, override val refill: Boolean, val plan: AgentRewindPlan) : RewindSheet

    data class Applying(override val turn: UInt, override val refill: Boolean, val plan: AgentRewindPlan) : RewindSheet

    data class Failed(override val turn: UInt, override val refill: Boolean, val failure: AgentFailure) : RewindSheet
}

object AgentRewind {
    fun opened(turn: UInt, refill: Boolean): RewindSheet = RewindSheet.Loading(turn, refill)

    fun previewed(sheet: RewindSheet?, turn: UInt, plan: AgentRewindPlan): RewindSheet? =
        if (sheet is RewindSheet.Loading && sheet.turn == turn) RewindSheet.Ready(turn, sheet.refill, plan) else sheet

    fun previewFailed(sheet: RewindSheet?, turn: UInt, failure: AgentFailure): RewindSheet? =
        if (sheet != null && sheet.turn == turn && sheet !is RewindSheet.Applying) RewindSheet.Failed(turn, sheet.refill, failure) else sheet

    /** Rewind can be pressed: a plan nothing blocks, not yet applied, not expired. */
    fun canApply(sheet: RewindSheet?, nowMs: Long): Boolean =
        sheet is RewindSheet.Ready && sheet.plan.blocked == null && sheet.plan.expiresMs > nowMs

    fun applying(sheet: RewindSheet?): RewindSheet? =
        if (sheet is RewindSheet.Ready && sheet.plan.blocked == null) RewindSheet.Applying(sheet.turn, sheet.refill, sheet.plan) else sheet

    /** The PC refused: an expired or used plan is asked for again (Loading), anything else says why. */
    fun applyFailed(sheet: RewindSheet?, failure: AgentFailure): RewindSheet? = when {
        sheet !is RewindSheet.Applying -> sheet
        failure == AgentFailure.NOT_FOUND -> RewindSheet.Loading(sheet.turn, sheet.refill)
        else -> RewindSheet.Failed(sheet.turn, sheet.refill, failure)
    }

    /** Why a plan can't be applied, as the copy words it. */
    enum class Reason { SHARED, BUSY, NO_RESTORE_POINT, NOT_GIT, EXPIRED, OFF, LATEST_ONLY }

    fun reason(blocked: AgentRewindBlocked?): Reason? = when (blocked) {
        null -> null
        AgentRewindBlocked.SHARED -> Reason.SHARED
        AgentRewindBlocked.BUSY -> Reason.BUSY
        AgentRewindBlocked.NO_CHECKPOINT -> Reason.NO_RESTORE_POINT
        AgentRewindBlocked.NOT_GIT -> Reason.NOT_GIT
        AgentRewindBlocked.EXPIRED -> Reason.EXPIRED
        AgentRewindBlocked.OFF -> Reason.OFF
        AgentRewindBlocked.AGENT -> Reason.LATEST_ONLY
    }
}

/**
 * The composer's draft on the PC (phase 2 §Q6.1, phase 3 §U9.3): read when
 * the composer opens, written 500 ms after the last key and on leaving,
 * never while a question waits; a newer one from another device replaces
 * the text while the composer isn't focused, and is offered while it is.
 */
@Immutable
data class DraftState(
    /** The stored draft's rev this composer knows (0: none). */
    val rev: ULong = 0uL,
    /** The draft was read since the composer opened. */
    val loaded: Boolean = false,
    /** Typed since it was last written. */
    val dirty: Boolean = false,
    /** A newer draft from another device, offered: "A newer draft from %1 · Use it". */
    val offer: AgentDraft? = null,
)

/** What a draft decision asks the screen to do with the composer's text. */
@Immutable
data class DraftStep(val state: DraftState, val text: String? = null, val images: List<String>? = null)

object AgentDrafts {
    /** A draft is written this long after the last key. */
    const val SAVE_AFTER_MS = 500L

    /** The longest draft the PC keeps, in bytes. */
    const val MAX_BYTES = 16_384

    /** A thread's draft key is its id. */
    fun key(threadId: String): String = threadId

    /** Whether the draft may be written now: never while a question waits (phase 2 reconcile). */
    fun mayWrite(state: DraftState, questionWaiting: Boolean): Boolean =
        state.loaded && state.dirty && !questionWaiting

    /** Read when the composer opened: an empty composer takes it; one with words keeps them and is offered it. */
    fun loaded(state: DraftState, draft: AgentDraft?, local: String): DraftStep {
        if (draft == null) return DraftStep(state.copy(loaded = true, rev = 0uL, offer = null))
        val next = state.copy(loaded = true, rev = draft.rev)
        if (draft.text == local && draft.images.isEmpty()) return DraftStep(next.copy(offer = null))
        return if (local.isBlank()) {
            DraftStep(next.copy(offer = null, dirty = false), draft.text, draft.images.map { it.id })
        } else {
            DraftStep(next.copy(offer = draft))
        }
    }

    fun edited(state: DraftState): DraftState = if (state.dirty) state else state.copy(dirty = true)

    /** What writing it answered: its new rev, or another device's newer one (offered). */
    fun saved(state: DraftState, saved: AgentDraftSaved, local: String): DraftState {
        val conflict = saved.conflict
        return when {
            conflict == null -> state.copy(rev = saved.rev, dirty = false)
            conflict.text == local -> state.copy(rev = conflict.rev, dirty = false, offer = null)
            else -> state.copy(rev = conflict.rev, offer = conflict)
        }
    }

    /** Another device changed it ([rev]): read it unless it's the one this composer wrote. */
    fun stale(state: DraftState, rev: ULong): Boolean = state.loaded && rev > state.rev

    /** The newer draft arrived: it replaces the text while the composer isn't focused, else it's offered. */
    fun arrived(state: DraftState, draft: AgentDraft?, focused: Boolean, local: String): DraftStep {
        if (draft == null) return DraftStep(state.copy(offer = null))
        if (draft.rev <= state.rev && state.offer == null) return DraftStep(state)
        val next = state.copy(rev = maxOf(state.rev, draft.rev))
        return when {
            draft.text == local && draft.images.isEmpty() -> DraftStep(next.copy(offer = null, dirty = false))
            focused -> DraftStep(next.copy(offer = draft))
            else -> DraftStep(next.copy(offer = null, dirty = false), draft.text, draft.images.map { it.id })
        }
    }

    /** "Use it". */
    fun used(state: DraftState): DraftStep {
        val offer = state.offer ?: return DraftStep(state)
        return DraftStep(state.copy(offer = null, rev = maxOf(state.rev, offer.rev), dirty = false), offer.text, offer.images.map { it.id })
    }

    /** Sent: the PC drops the draft with the message. */
    fun sent(state: DraftState): DraftState = state.copy(rev = 0uL, dirty = false, offer = null)
}
