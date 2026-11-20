// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable

/** What the reply box says while it's empty. */
enum class ComposerHint {
    /** "Message Claude". */
    MESSAGE,

    /** "You can reply when Claude stops." */
    REPLY_WHEN_STOPS,

    /** "Say why, to deny it and tell Claude what to do instead" (a request is shown). */
    SAY_WHY,

    /** "You can reply after 18:00." */
    AFTER_TIME,

    /** "You can reply when the limit resets." */
    AFTER_RESET,
}

/** The button at the end of the reply box. */
enum class ComposerAction {
    /** None: only Stop while the agent works. */
    NONE,
    SEND,

    /** "Deny and send": denies the shown request with the words typed (reconcile file, the dock). */
    DENY_AND_SEND,
}

/**
 * The reply box and the request above it, as one decision: what can be typed,
 * what the button does and whether it can, whether Stop is beside it, and
 * whether Allow is the primary button.
 */
@Immutable
data class ComposerView(
    val enabled: Boolean,
    val hint: ComposerHint,
    val action: ComposerAction,
    val actionEnabled: Boolean,
    /** Stop beside the button whenever a turn runs or waits (reconcile file, the dock, item 3). */
    val showStop: Boolean,
    /** Allow is the primary button; with words in the box "Deny and send" is, and Allow an outline. */
    val allowPrimary: Boolean,
    /** "Codex stops and reads this next." under the box: Codex threads, while it would deny with the words. */
    val codexNote: Boolean,
)

/** What the composer is made from: the thread, its shown request, and the draft. */
@Immutable
data class ComposerInput(
    /** A turn is running or waiting for an answer. */
    val running: Boolean,
    /** The thread is known (it can be sent to once it stops). */
    val hasThread: Boolean = true,
    /** The request shown in the dock, if any. */
    val queue: AskQueue? = null,
    /** The PC takes words with a denial (`AgentCatalog.canSayWhy`). */
    val canSayWhy: Boolean = false,
    val draft: String = "",
    /** The request the draft was last edited under; null when it was edited with none shown. */
    val draftFor: String? = null,
    /** This phone sent an answer to the shown request and hasn't heard back. */
    val answering: Boolean = false,
    /** A send or a denial with words is on its way. */
    val sending: Boolean = false,
    /** Paused by a usage limit; [pausedUntilMs] when it resets, if the CLI said and it's still ahead. */
    val paused: Boolean = false,
    val pausedUntilMs: Long? = null,
    /** The thread's agent is Codex: it stops the turn and reads the words next (contract §P8.3). */
    val codex: Boolean = false,
)

/** The pure rules of the reply box and the dock (contract §P8, reconcile file "Approvals dock"). */
object AgentDock {
    /**
     * With a request shown on a PC that takes words with a denial, the box is open
     * and its text denies the shown request with the words: never when empty or too
     * long, never while the request is held, and not after the shown request changed
     * under the words until they're edited again (words are for the request they
     * were typed under). Otherwise the box is closed while the agent works, closed
     * until a usage limit's reset time, and sends a message when the thread stops.
     */
    fun composer(input: ComposerInput): ComposerView {
        val hasText = input.draft.isNotBlank()
        val queue = input.queue
        val allowPrimary = !hasText
        if (queue != null && input.canSayWhy) {
            val busy = input.sending || input.answering
            val words = hasText && AgentsModel.fits(input.draft) && input.draftFor == queue.shown.token
            return ComposerView(
                enabled = !busy,
                hint = ComposerHint.SAY_WHY,
                action = if (hasText) ComposerAction.DENY_AND_SEND else ComposerAction.SEND,
                actionEnabled = hasText && words && !queue.held && !busy,
                showStop = input.running,
                allowPrimary = allowPrimary,
                codexNote = input.codex && hasText,
            )
        }
        if (input.running) {
            return ComposerView(
                enabled = false,
                hint = ComposerHint.REPLY_WHEN_STOPS,
                action = ComposerAction.NONE,
                actionEnabled = false,
                showStop = true,
                allowPrimary = true,
                codexNote = false,
            )
        }
        val until = input.pausedUntilMs
        val hint = when {
            input.paused && until != null -> ComposerHint.AFTER_TIME
            input.paused -> ComposerHint.AFTER_RESET
            else -> ComposerHint.MESSAGE
        }
        val enabled = input.hasThread && until == null && !input.sending
        return ComposerView(
            enabled = enabled,
            hint = hint,
            action = ComposerAction.SEND,
            actionEnabled = enabled && AgentsModel.fits(input.draft),
            showStop = false,
            allowPrimary = true,
            codexNote = false,
        )
    }

    /**
     * The request the draft is now for, after the user edited it: the one shown,
     * so "Deny and send" answers what the words were typed under.
     */
    fun editedUnder(queue: AskQueue?): String? = queue?.shown?.token

    /**
     * Whether Allow can be pressed: never while held or while an answer is on its
     * way, and only once the whole command or change has been on screen and the
     * PC didn't cut it (the phone's rule in agents-reconcile.md).
     */
    fun canAllow(queue: AskQueue, answering: Boolean, seenAll: Boolean): Boolean =
        !queue.held && !answering && seenAll && !queue.shown.truncated

    /** Whether Deny can be pressed. */
    fun canDeny(queue: AskQueue, answering: Boolean): Boolean = !queue.held && !answering
}
