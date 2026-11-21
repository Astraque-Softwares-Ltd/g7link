// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable
import app.g7link.core.AgentItem
import app.g7link.core.AgentItemKind
import app.g7link.core.AgentSession
import app.g7link.core.AgentSessionSource
import app.g7link.core.AgentSessionState
import java.text.Normalizer

/** What the PC did with a check or a reply (docs/protocol/agents.md §8). A word this phone doesn't know reads as [FAILED]. */
enum class ReplyResult {
    READY,
    SENT,
    REFUSED,
    FAILED,
    ;

    companion object {
        fun of(word: String?): ReplyResult = when (word) {
            "ready" -> READY
            "sent" -> SENT
            "refused" -> REFUSED
            else -> FAILED
        }
    }
}

/** Why, in the PC's words. One this phone doesn't know reads as [OTHER]: a plain "Couldn't reply". */
enum class ReplyReason(val word: String) {
    LOCKED("locked"),
    IN_USE("in_use"),
    CONFIRM("confirm"),
    NEEDS_YOU("needs_you"),
    BUSY("busy"),
    ENDED("ended"),
    NO_PROCESS("no_process"),
    NO_WINDOW("no_window"),
    AMBIGUOUS("ambiguous"),
    TAB("tab"),
    PLACE("place"),
    CLIPBOARD("clipboard"),
    TEXT("text"),
    RATE("rate"),
    SETUP("setup"),
    FOCUS("focus"),
    INTERRUPTED("interrupted"),
    INPUT("input"),
    OTHER(""),
    ;

    companion object {
        fun of(word: String?): ReplyReason? {
            if (word.isNullOrEmpty()) return null
            return entries.firstOrNull { it != OTHER && it.word == word } ?: OTHER
        }
    }
}

/** Where the PC types: "Terminal · Claude · g7linux". [inApp]: the agent's own app rather than a terminal. */
@Immutable
data class ReplyTarget(val app: String, val agent: String, val folder: String, val inApp: Boolean) {
    val line: String
        get() = listOf(app, agent, folder).filter { it.isNotBlank() }.joinToString(" · ")
}

/** The PC's answer to a check or a reply. [retryMs]: how long until trying again makes sense, when it knows. */
@Immutable
data class ReplyAnswer(
    val result: ReplyResult,
    val reason: ReplyReason? = null,
    val target: ReplyTarget? = null,
    val nonce: String? = null,
    val retryMs: Long? = null,
)

/** What the footer offers for replies, from the last answer. */
enum class ReplyOffer {
    /** Not asked yet, or the session is gone: nothing is added. */
    UNKNOWN,

    /** The PC doesn't offer replies (another desktop, an older G7 Link): the screen is exactly as before. */
    NOT_OFFERED,

    /** The reply box. */
    BOX,

    /** The box, off with the lock until the PC is unlocked. */
    LOCKED,

    /** No box: the quiet line and its "Why?". */
    CANT,

    /** The session ended: Continue here, as before. */
    ENDED,
}

/** Why replies can't be sent from here (plans/reply-on-pc.md §6.4 and §9.1). */
enum class CantReply {
    /** "Reply in the Claude app on guchu.": the app shows no session G7 can prove. */
    AGENT_APP,

    /** "Can't tell which window has this Codex session.": it wasn't opened with `codex resume`. */
    CODEX_WINDOW,

    /** A terminal or editor G7 doesn't type into ("This one is in VS Code" when the PC names it). */
    TERMINAL,

    /** "G7 can't find this session's window." */
    NO_WINDOW,

    /** "Replying needs the G7 Linux Companion 1.4 on guchu." */
    SETUP,

    /** "Replies are off for this phone on guchu." */
    OFF,
}

/** The line under the box. [amber]: something to do on the PC. [sticky]: a later check doesn't clear it. */
enum class ReplyNote(val amber: Boolean = false, val sticky: Boolean = false) {
    /** "guchu is in use. Try again in a moment." */
    IN_USE,

    /** "Codex is mid-turn. Send when it's done." */
    BUSY,

    /** "Too many replies at once. Try again in a moment." */
    RATE,

    /** "It's waiting for an answer on guchu." */
    NEEDS_YOU(amber = true),

    /** "It's in a background tab. Bring it forward on guchu." */
    TAB(amber = true),

    /** "G7 can't tell which window it's in." */
    AMBIGUOUS(amber = true),

    /** "Something is copied on guchu that G7 can't put back." */
    CLIPBOARD(amber = true),

    /** "Starts with ! or /, which Claude reads as a command." */
    TEXT,

    /** "The window didn't come forward. Nothing was typed.", with Retry. */
    FOCUS,

    /** "Your message was typed on guchu but not sent. Check the window.": until dismissed, and no Retry (it would paste twice). */
    INTERRUPTED(amber = true, sticky = true),

    /** "Not known if it was sent. Check the session.": until an item settles it. */
    NOT_KNOWN(sticky = true),

    /** "Couldn't reply." */
    FAILED,

    /** "Can't reach guchu.": it never left this phone. */
    UNREACHABLE,
}

/** A reply's text and the newest item seen when it went: a user item after that settles it. */
@Immutable
data class PendingReply(val text: String, val afterSeq: ULong)

/** The first reply to this window asks first (§6.2): where it will type, and the PC's one-time word for it. */
@Immutable
data class ReplyConfirm(val target: ReplyTarget?, val nonce: String, val text: String)

/** A followed session's replies on this screen. Held by the screen only: leaving it drops them. */
@Immutable
data class ReplyState(
    val offer: ReplyOffer = ReplyOffer.UNKNOWN,
    val cant: CantReply? = null,
    /** The app the PC named for a [CantReply.TERMINAL] ("VS Code"), when it did. */
    val cantApp: String? = null,
    /** Where it types, as the PC last said. */
    val target: ReplyTarget? = null,
    val draft: String = "",
    val sending: Boolean = false,
    val note: ReplyNote? = null,
    val retryMs: Long? = null,
    val confirm: ReplyConfirm? = null,
    /** Entered on the PC: shown at the end of the transcript until its own row arrives. */
    val pending: PendingReply? = null,
    /** No answer came: settled by the next user item. */
    val unsure: PendingReply? = null,
    /** Bumped when a reply starts and when it ends: a check asked before then is stale. */
    val epoch: Int = 0,
)

/** What the footer shows for replies. */
@Immutable
sealed interface ReplyFooter {
    data object None : ReplyFooter

    /** The quiet line, "Why?" opening [why]; [note] a sticky line still owed. */
    data class Cant(val why: CantReply, val app: String?, val note: ReplyNote?) : ReplyFooter

    data class Box(
        val target: ReplyTarget?,
        /** Off with the lock: "Unlock guchu to reply". */
        val locked: Boolean,
        /** The spinner for the button, the field read-only, "Typing on guchu…" for the target line. */
        val sending: Boolean,
        val note: ReplyNote?,
        val canSend: Boolean,
        /** Over the wire's limit (as opposed to empty). */
        val tooLong: Boolean,
        /** Retry beside the note ([ReplyNote.FOCUS]). */
        val canRetry: Boolean,
        /** A close beside the note ([ReplyNote.INTERRUPTED]). */
        val canDismiss: Boolean,
    ) : ReplyFooter
}

/** The pending row at the end of the transcript: "Entered on guchu", or "Waiting for Claude to pick it up" while it works. */
@Immutable
data class PendingRow(val text: String, val waiting: Boolean)

/**
 * "Reply on the PC" (plans/reply-on-pc.md §6), the pure part: the PC's
 * answers folded into what the followed session's footer shows. The PC
 * enforces every rule; the phone shows what it said, keeps the draft, and
 * sends only when the user does.
 */
object AgentReplyModel {
    /** The least time between two checks: the PC looks at no more than 2 a second. */
    const val CHECK_GAP_MS = 600L

    /** A nonce is 32 hex digits (§4 limits); anything else isn't sent back. */
    private val NONCE = Regex("[0-9a-f]{32}")

    fun typed(state: ReplyState, text: String): ReplyState =
        if (state.sending) state else state.copy(draft = text, note = if (state.note == ReplyNote.TEXT) null else state.note)

    /** Whether to ask the PC now: not while a reply goes, for an ended session, or once it said it doesn't offer them. */
    fun shouldCheck(state: ReplyState, session: AgentSession?): Boolean =
        session != null && session.state != AgentSessionState.ENDED && !state.sending && state.offer != ReplyOffer.NOT_OFFERED

    /** A check's answer, asked at [epoch]: dropped when a reply started or ended since. */
    fun checked(state: ReplyState, epoch: Int, call: AgentCall<ReplyAnswer>, session: AgentSession): ReplyState {
        if (epoch != state.epoch || state.sending) return state
        val answer = when (call) {
            is AgentCall.Ok -> call.value
            is AgentCall.Failed -> return failedCall(state, call.failure)
        }
        val target = answer.target ?: state.target
        return when (answer.result) {
            ReplyResult.READY, ReplyResult.SENT -> box(state, target, null)
            // A check enters nothing, so it can't fail; whatever it means, nothing changes.
            ReplyResult.FAILED -> state
            ReplyResult.REFUSED -> when (val reason = answer.reason ?: ReplyReason.OTHER) {
                // The first reply asks first; the PC answers that when one is sent.
                ReplyReason.CONFIRM -> box(state, target, null)
                // About checks asked too fast, not about replies.
                ReplyReason.RATE -> state
                else -> refused(state, reason, answer, target, session)
            }
        }
    }

    /** A reply goes now (from the box, Retry, or the sheet's Send). */
    fun sendStarted(state: ReplyState): ReplyState = state.copy(
        sending = true,
        epoch = state.epoch + 1,
        note = null,
        retryMs = null,
        unsure = null,
        confirm = null,
    )

    /** The text a reply sends now, and the nonce once the sheet confirmed it; null when nothing can go. */
    fun outgoing(state: ReplyState, session: AgentSession?): Pair<String, String?>? {
        val confirm = state.confirm
        if (confirm != null) return if (state.sending) null else confirm.text to confirm.nonce
        val footer = footer(state, session) as? ReplyFooter.Box ?: return null
        return if (footer.canSend) state.draft to null else null
    }

    /** A reply's answer: [text] is what went, [lastSeq] the newest item seen when it went. */
    fun replied(state: ReplyState, call: AgentCall<ReplyAnswer>, text: String, lastSeq: ULong, session: AgentSession?): ReplyState {
        val done = state.copy(sending = false, epoch = state.epoch + 1)
        val answer = when (call) {
            is AgentCall.Ok -> call.value
            is AgentCall.Failed -> return when (call.failure) {
                // It went and no answer came: it may have been entered (the core doesn't send it again).
                AgentFailure.TIMEOUT ->
                    done.copy(offer = boxOffer(done.offer), note = ReplyNote.NOT_KNOWN, unsure = PendingReply(text, lastSeq))
                // It never went.
                AgentFailure.OFFLINE -> done.copy(offer = boxOffer(done.offer), note = ReplyNote.UNREACHABLE)
                AgentFailure.OTHER -> done.copy(offer = boxOffer(done.offer), note = ReplyNote.FAILED)
                AgentFailure.DENIED, AgentFailure.UNSUPPORTED, AgentFailure.NOT_FOUND -> failedCall(done, call.failure)
            }
        }
        val target = answer.target ?: done.target
        return when (answer.result) {
            ReplyResult.SENT -> done.copy(
                offer = ReplyOffer.BOX,
                target = target,
                draft = if (done.draft == text) "" else done.draft,
                note = null,
                pending = PendingReply(text.trim(), lastSeq),
            )
            // Not a reply's answer.
            ReplyResult.READY -> done.copy(offer = boxOffer(done.offer), target = target, note = ReplyNote.FAILED)
            ReplyResult.FAILED -> done.copy(
                offer = boxOffer(done.offer),
                target = target,
                note = when (answer.reason) {
                    ReplyReason.FOCUS -> ReplyNote.FOCUS
                    // The text may be in the box: no Retry, it would paste twice.
                    ReplyReason.INTERRUPTED, ReplyReason.INPUT -> ReplyNote.INTERRUPTED
                    else -> ReplyNote.FAILED
                },
            )
            ReplyResult.REFUSED -> when (val reason = answer.reason ?: ReplyReason.OTHER) {
                ReplyReason.CONFIRM -> {
                    val nonce = answer.nonce?.takeIf { NONCE.matches(it) }
                    if (nonce == null) {
                        done.copy(offer = boxOffer(done.offer), target = target, note = ReplyNote.FAILED)
                    } else {
                        done.copy(offer = ReplyOffer.BOX, target = target, confirm = ReplyConfirm(target, nonce, text))
                    }
                }
                else -> refused(done, reason, answer, target, session)
            }
        }
    }

    /** Cancel on the sheet: nothing was typed; the draft stays. */
    fun confirmCancelled(state: ReplyState): ReplyState = state.copy(confirm = null)

    /** The close beside an interrupted reply's line. */
    fun dismissed(state: ReplyState): ReplyState = if (state.note == ReplyNote.INTERRUPTED) state.copy(note = null) else state

    /**
     * The session's items as they are now: a user item after a reply makes
     * its pending row the real one, and settles one whose answer never came
     * (its draft goes when the item holds its text).
     */
    fun itemsSeen(state: ReplyState, items: List<AgentItem>): ReplyState {
        val pending = state.pending
        val unsure = state.unsure
        if (pending == null && unsure == null) return state
        val users = items.filter { it.kind == AgentItemKind.USER }
        var next = state
        if (pending != null && users.any { it.seq > pending.afterSeq }) next = next.copy(pending = null)
        if (unsure != null) {
            val after = users.filter { it.seq > unsure.afterSeq }
            if (after.isNotEmpty()) {
                val wanted = squash(unsure.text)
                val entered = wanted.isNotEmpty() && after.any { squash(it.text.orEmpty()).contains(wanted) }
                next = next.copy(
                    unsure = null,
                    note = if (next.note == ReplyNote.NOT_KNOWN) null else next.note,
                    draft = if (entered && next.draft == unsure.text) "" else next.draft,
                )
            }
        }
        return next
    }

    /** What the footer shows for replies; [session] null or ended: nothing (Continue here, as before). */
    fun footer(state: ReplyState, session: AgentSession?): ReplyFooter {
        if (session == null || session.state == AgentSessionState.ENDED) return ReplyFooter.None
        return when (state.offer) {
            ReplyOffer.UNKNOWN, ReplyOffer.NOT_OFFERED, ReplyOffer.ENDED -> ReplyFooter.None
            ReplyOffer.CANT -> ReplyFooter.Cant(state.cant ?: CantReply.NO_WINDOW, state.cantApp, state.note?.takeIf { it.sticky })
            ReplyOffer.BOX, ReplyOffer.LOCKED -> {
                val locked = state.offer == ReplyOffer.LOCKED
                val note = if (locked) state.note?.takeIf { it.sticky } else state.note
                val fits = AgentsModel.fits(state.draft)
                val free = !locked && !state.sending && state.confirm == null && note != ReplyNote.INTERRUPTED
                ReplyFooter.Box(
                    target = state.target,
                    locked = locked,
                    sending = state.sending,
                    note = note,
                    canSend = free && fits,
                    tooLong = AgentsModel.tooLong(state.draft),
                    canRetry = free && fits && note == ReplyNote.FOCUS,
                    canDismiss = note == ReplyNote.INTERRUPTED,
                )
            }
        }
    }

    /** Whether the header says "reply on the PC" instead of "read only": the box shows. */
    fun replies(state: ReplyState, session: AgentSession?): Boolean = footer(state, session) is ReplyFooter.Box

    /** The pending row, while there is one. */
    fun pendingRow(state: ReplyState, session: AgentSession?): PendingRow? =
        state.pending?.let { PendingRow(it.text, waiting = session?.state == AgentSessionState.WORKING) }

    /** The newest item's `seq` on screen; 0 before any. */
    fun lastSeq(items: List<AgentItem>): ULong = items.maxOfOrNull { it.seq } ?: 0uL

    /** Why [reason] keeps a reply from here, or null when it's a line under the box. */
    fun cantOf(reason: ReplyReason, session: AgentSession?, target: ReplyTarget?): CantReply? = when (reason) {
        ReplyReason.PLACE -> when {
            session?.source == AgentSessionSource.DESKTOP_APP -> CantReply.AGENT_APP
            // Named: it was found, in something G7 doesn't type into. Not found: not opened with `codex resume`.
            session?.agent == "codex" && target == null -> CantReply.CODEX_WINDOW
            else -> CantReply.TERMINAL
        }
        ReplyReason.NO_PROCESS, ReplyReason.NO_WINDOW -> CantReply.NO_WINDOW
        ReplyReason.SETUP -> CantReply.SETUP
        else -> null
    }

    private fun refused(state: ReplyState, reason: ReplyReason, answer: ReplyAnswer, target: ReplyTarget?, session: AgentSession?): ReplyState {
        cantOf(reason, session, answer.target)?.let { why ->
            return state.copy(
                offer = ReplyOffer.CANT,
                cant = why,
                cantApp = answer.target?.app?.takeIf { why == CantReply.TERMINAL && it.isNotBlank() },
                note = state.note?.takeIf { it.sticky },
                retryMs = null,
            )
        }
        val note = when (reason) {
            ReplyReason.LOCKED -> return state.copy(offer = ReplyOffer.LOCKED, target = target, note = state.note?.takeIf { it.sticky }, retryMs = null)
            ReplyReason.ENDED -> return state.copy(offer = ReplyOffer.ENDED, note = state.note?.takeIf { it.sticky }, retryMs = null)
            ReplyReason.IN_USE -> ReplyNote.IN_USE
            ReplyReason.BUSY -> ReplyNote.BUSY
            ReplyReason.RATE -> ReplyNote.RATE
            ReplyReason.NEEDS_YOU -> ReplyNote.NEEDS_YOU
            ReplyReason.TAB -> ReplyNote.TAB
            ReplyReason.AMBIGUOUS -> ReplyNote.AMBIGUOUS
            ReplyReason.CLIPBOARD -> ReplyNote.CLIPBOARD
            ReplyReason.TEXT -> ReplyNote.TEXT
            // Refused: nothing was typed, whatever the word.
            else -> ReplyNote.FAILED
        }
        // A sticky line stays over a passing one.
        val kept = state.note?.takeIf { it.sticky } ?: note
        return state.copy(offer = ReplyOffer.BOX, target = target, note = kept, retryMs = answer.retryMs?.takeIf { it in 1L..60_000L })
    }

    /** The box, with no passing line (a sticky one stays). */
    private fun box(state: ReplyState, target: ReplyTarget?, note: ReplyNote?): ReplyState =
        state.copy(offer = ReplyOffer.BOX, target = target, note = state.note?.takeIf { it.sticky } ?: note, retryMs = null, cant = null, cantApp = null)

    private fun failedCall(state: ReplyState, failure: AgentFailure): ReplyState = when (failure) {
        AgentFailure.DENIED -> state.copy(offer = ReplyOffer.CANT, cant = CantReply.OFF, cantApp = null, note = state.note?.takeIf { it.sticky })
        AgentFailure.UNSUPPORTED -> state.copy(offer = ReplyOffer.NOT_OFFERED, note = null, confirm = null)
        AgentFailure.NOT_FOUND -> state.copy(offer = ReplyOffer.UNKNOWN, note = null, confirm = null)
        // Not reached, or no answer: what it said last stands.
        AgentFailure.OFFLINE, AgentFailure.TIMEOUT, AgentFailure.OTHER -> state
    }

    /** After a reply that didn't change what's offered: the box stays (a locked one too). */
    private fun boxOffer(offer: ReplyOffer): ReplyOffer = if (offer == ReplyOffer.LOCKED) offer else ReplyOffer.BOX

    /** Text compared as the PC enters it: composed, without any whitespace (so wrapping and trimming can't break a match). */
    private fun squash(text: String): String =
        Normalizer.normalize(text, Normalizer.Form.NFC).filterNot { it.isWhitespace() }
}
