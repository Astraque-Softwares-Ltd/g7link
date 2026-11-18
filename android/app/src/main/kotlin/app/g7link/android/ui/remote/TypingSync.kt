// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import java.text.BreakIterator

/** What has the keyboard focus on the PC, for the typing box. */
internal enum class FieldKind {
    /** A field whose text the PC shares: the box mirrors it. */
    Mirror,

    /** Takes text, but its app doesn't share it: the box holds what was typed here. */
    Opaque,

    /** A password field: typed straight through, never kept. */
    Secret,

    /** Nothing that takes text, or a PC that doesn't say. */
    None,
}

/** The PC's focused field, as it sent it. Offsets count Unicode code points. */
internal data class PcField(
    val focus: Int,
    val field: Long,
    val epoch: Int,
    val applied: Int,
    val kind: FieldKind,
    val app: String?,
    val label: String?,
    val window: Boolean,
    val multiline: Boolean,
    val start: Int,
    val before: String,
    val after: String,
    val anchor: Int?,
    /** The app's icon, to fetch from the PC by this key. */
    val icon: String?,
)

/** One change for the PC's focused field, sent one at a time, in order. */
internal sealed interface TypingOp {
    val focus: Int
    val seq: Int

    /**
     * Replace code points [rangeStart]..[rangeEnd] of a mirrored field with
     * [text], or (without a range) move the caret [moves] characters
     * (negative: left), delete [backspaces] before it and type [text].
     */
    data class Edit(
        override val focus: Int,
        override val seq: Int,
        val rangeStart: Int?,
        val rangeEnd: Int?,
        val moves: Int,
        val backspaces: Int,
        val text: String,
    ) : TypingOp

    /** Put a mirrored field's caret at [caret], selecting from [anchor]. */
    data class Caret(override val focus: Int, override val seq: Int, val caret: Int, val anchor: Int) : TypingOp
}

/** How an op went on the PC. */
internal enum class TypingResult {
    Done,

    /** The PC's focus moved on before it ran: a new field state is on its way. */
    StaleFocus,
    Failed,
}

/** The typing box's text and selection (UTF-16 indices), as the keyboard left it. */
internal data class BoxText(
    val text: String,
    val selStart: Int = text.length,
    val selEnd: Int = text.length,
    val composing: Boolean = false,
)

/**
 * Keeps the phone's typing box and the PC's focused field the same.
 *
 * What the PC has is [shadow]: in a mirrored field, the text the PC sent
 * (around its caret); elsewhere, what this phone typed there, never erased
 * on its own: when the PC or a click from here gives reason to doubt it, it
 * stays, [unverified]. A change in the box goes out as one [TypingOp]
 * at a time (changes made while one is on its way join the next), worked
 * out from the shadow by common prefix and suffix, so an edit in the middle
 * doesn't retype the rest. The PC's own changes come back as [PcField]s; one
 * that differs from the shadow is taken once both sides have been quiet, so
 * the PC wins without fighting the keyboard. Only text typed into fields
 * whose text isn't shared is kept as a draft, in [drafts], to come back to.
 *
 * Not thread safe: used on the main thread.
 */
internal class TypingSync(
    private val drafts: FieldDrafts,
    private val clock: () -> Long = System::currentTimeMillis,
) {
    /** Called when the box's text is replaced from outside the keyboard. */
    var onReplace: ((BoxText) -> Unit)? = null

    var box = BoxText("")
        private set

    /** The field that has the focus, as the PC last told. */
    var current: PcField? = null
        private set

    val kind: FieldKind get() = current?.kind ?: FieldKind.None

    /**
     * What's in the box may not be what the PC's field has: someone at the PC,
     * or a click or key from here, may have changed a field whose text it
     * doesn't share. Edits then go where the PC's caret is.
     */
    var unverified = false
        private set

    /** Everything in the box has reached the PC (or can't, until it changes). */
    val synced: Boolean get() = inFlight == null && (box.text == shadow || blocked)

    private var focus = 0
    private var shadow = ""
    private var shadowCaret = 0
    private var shadowAnchor = 0
    private var windowStart = 0
    private var sentSeq = 0
    private var inFlight: TypingOp? = null
    private var beforeOp: Triple<String, Int, Int>? = null
    private var blocked = false
    private var pending: PcField? = null
    private var pendingAt = 0L
    private var typedAt = 0L

    /** The keyboard changed the box. */
    fun onLocal(next: BoxText) {
        if (next.text != box.text) {
            typedAt = clock()
            blocked = false
        }
        box = next
    }

    /** The next op to send, when none is on its way and the box differs from the PC. */
    fun nextOp(): TypingOp? {
        if (inFlight != null || blocked) return null
        if (box.text == shadow) return caretOp()
        val text = box.text
        var prefix = commonPrefix(shadow, text)
        val suffix = commonSuffix(shadow, text, prefix)
        val editEnd = shadow.length - suffix
        if (prefix > editEnd) prefix = editEnd
        val removed = shadow.substring(prefix, editEnd)
        var added = text.substring(prefix, text.length - suffix)
        if (utf8Length(added) > MAX_TEXT_BYTES) added = utf8Prefix(added, MAX_TEXT_BYTES)

        val seq = sentSeq + 1
        val op: TypingOp.Edit
        var removedStart = prefix
        if (kind == FieldKind.Mirror) {
            val start = windowStart + shadow.codePointCount(0, prefix)
            op = TypingOp.Edit(focus, seq, start, start + removed.codePointCount(0, removed.length), 0, 0, added)
        } else {
            // Unverified, the PC's caret may be anywhere: keys go where it is, as a keyboard's would.
            val moves = if (unverified) 0 else graphemesBetween(shadow, shadowCaret, editEnd)
            if (kotlin.math.abs(moves) > MAX_STEPS) {
                // Too far back to reach with the arrow keys: start afresh here.
                commit()
                return null
            }
            var backspaces = graphemeCount(removed)
            if (backspaces > MAX_STEPS) {
                // The nearest part first; the rest goes next.
                removedStart = graphemeStart(shadow, editEnd, MAX_STEPS)
                backspaces = MAX_STEPS
                added = ""
            }
            if (moves == 0 && backspaces == 0 && added.isEmpty()) {
                shadow = text
                shadowCaret = box.selEnd
                return null
            }
            op = TypingOp.Edit(focus, seq, null, null, moves, backspaces, added)
        }
        beforeOp = Triple(shadow, shadowCaret, shadowAnchor)
        shadow = shadow.substring(0, removedStart) + added + shadow.substring(editEnd)
        shadowCaret = removedStart + added.length
        shadowAnchor = shadowCaret
        sentSeq = seq
        inFlight = op
        return op
    }

    /** A mirrored field's caret follows the box's, once the text is the same. */
    private fun caretOp(): TypingOp? {
        if (kind != FieldKind.Mirror || box.composing) return null
        if (box.selEnd == shadowCaret && box.selStart == shadowAnchor) return null
        val caret = windowStart + shadow.codePointCount(0, box.selEnd.coerceIn(0, shadow.length))
        val anchor = windowStart + shadow.codePointCount(0, box.selStart.coerceIn(0, shadow.length))
        beforeOp = Triple(shadow, shadowCaret, shadowAnchor)
        shadowCaret = box.selEnd
        shadowAnchor = box.selStart
        sentSeq += 1
        val op = TypingOp.Caret(focus, sentSeq, caret, anchor)
        inFlight = op
        return op
    }

    /** How [op] went. */
    fun onDone(op: TypingOp, result: TypingResult) {
        if (inFlight?.seq == op.seq) inFlight = null
        if (op.focus != focus) return
        if (result == TypingResult.Failed) {
            // Not typed: the box keeps it, and it goes again with the next change.
            beforeOp?.let { (text, caret, anchor) ->
                shadow = text
                shadowCaret = caret
                shadowAnchor = anchor
            }
            blocked = true
        }
        beforeOp = null
    }

    /** The PC told of its focused field. */
    fun onState(state: PcField) {
        if (state.focus != focus) {
            switchTo(state)
            return
        }
        val previous = current
        current = state
        if (previous?.kind != state.kind && state.kind == FieldKind.Mirror) {
            adopt(state)
            return
        }
        when (state.kind) {
            FieldKind.Mirror -> {
                if (inFlight != null || state.applied < sentSeq) return
                val text = state.before + state.after
                if (state.start == windowStart && text == shadow) {
                    // Our own typing, come back.
                    pending = null
                    if (!box.composing && box.text == shadow && state.before.length != shadowCaret) {
                        pending = state
                        pendingAt = clock()
                    }
                    return
                }
                pending = state
                pendingAt = clock()
                tick()
            }
            FieldKind.Opaque ->
                // Someone at the PC typed or clicked: what was typed here may not all be there.
                if (previous != null && state.epoch != previous.epoch) unverified = true
            FieldKind.Secret, FieldKind.None ->
                if (previous != null && state.epoch != previous.epoch) {
                    drafts.remove(state.field)
                    commit()
                }
        }
    }

    /** Takes a waiting field state once both sides are quiet. Returns whether it did. */
    fun tick(): Boolean {
        val state = pending ?: return false
        if (state.applied < sentSeq) {
            pending = null
            return false
        }
        if (inFlight != null) return false
        val now = clock()
        val quiet = now - typedAt >= QUIET_MS && now - pendingAt >= QUIET_MS
        val settled = !box.composing && box.text == shadow && quiet
        if (!settled && now - pendingAt < FORCE_MS) return false
        adopt(state)
        return true
    }

    /** The phone moved the PC's caret or focus (a click, an arrow, a paste): what's typed stays, unverified. */
    fun onPcCaretMoved() {
        if (current != null && kind != FieldKind.Mirror) unverified = true
    }

    /** The user chose to start afresh: what was typed stays on the PC, and the box empties. */
    fun startAfresh() {
        if (kind == FieldKind.Mirror) return
        current?.let { if (it.field != 0L) drafts.remove(it.field) }
        commit()
    }

    /** Send was pressed: the PC took what was typed. */
    fun onSent() = startAfresh()

    /** The typing box closed, or the PC went away: keeps the draft, forgets the PC's text. */
    fun close() {
        saveDraft()
        current = null
        focus = 0
        pending = null
        inFlight = null
        unverified = false
        replace(BoxText(""), shadowToo = true)
    }

    private fun switchTo(state: PcField) {
        saveDraft()
        focus = state.focus
        current = state
        pending = null
        blocked = false
        unverified = false
        when (state.kind) {
            FieldKind.Mirror -> adopt(state)
            FieldKind.Opaque -> {
                // What was typed here comes back, unverified when the PC doubted it meanwhile.
                val draft = if (state.field != 0L) drafts.take(state.field, state.epoch) else null
                windowStart = 0
                unverified = draft?.unverified == true
                replace(BoxText(draft?.text.orEmpty()), shadowToo = true)
            }
            FieldKind.Secret, FieldKind.None -> {
                windowStart = 0
                replace(BoxText(""), shadowToo = true)
            }
        }
    }

    private fun saveDraft() {
        val field = current ?: return
        if (field.kind == FieldKind.Opaque && field.field != 0L && box.text.isNotEmpty()) {
            drafts.put(field.field, field.epoch, box.text, unverified)
        }
    }

    private fun adopt(state: PcField) {
        val text = state.before + state.after
        val caret = state.before.length
        val anchor = state.anchor?.let { utf16Index(text, it - state.start) } ?: caret
        windowStart = state.start
        pending = null
        unverified = false
        replace(BoxText(text, anchor, caret), shadowToo = true)
    }

    /** What's in the box stays on the PC; the box starts empty. */
    private fun commit() {
        unverified = false
        replace(BoxText(""), shadowToo = true)
    }

    private fun replace(next: BoxText, shadowToo: Boolean) {
        if (shadowToo) {
            shadow = next.text
            shadowCaret = next.selEnd
            shadowAnchor = next.selStart
        }
        box = next
        onReplace?.invoke(next)
    }

    internal companion object {
        const val MAX_TEXT_BYTES = 1000
        const val MAX_STEPS = 512
        const val QUIET_MS = 300L
        const val FORCE_MS = 1500L

        private val characters: BreakIterator = BreakIterator.getCharacterInstance()

        fun commonPrefix(a: String, b: String): Int {
            val n = minOf(a.length, b.length)
            var i = 0
            while (i < n && a[i] == b[i]) i++
            // Never split a surrogate pair.
            if (i > 0 && Character.isHighSurrogate(a[i - 1])) i--
            return i
        }

        fun commonSuffix(a: String, b: String, prefix: Int): Int {
            val n = minOf(a.length, b.length) - prefix
            var i = 0
            while (i < n && a[a.length - 1 - i] == b[b.length - 1 - i]) i++
            if (i > 0 && Character.isLowSurrogate(a[a.length - i])) i--
            return i
        }

        /** User-perceived characters in [text], each one Backspace on the PC. */
        fun graphemeCount(text: String): Int {
            if (text.isEmpty()) return 0
            synchronized(characters) {
                characters.setText(text)
                var count = 0
                while (characters.next() != BreakIterator.DONE) count++
                return count
            }
        }

        /** Arrow presses from [from] to [to] in [text]: negative to the left. */
        fun graphemesBetween(text: String, from: Int, to: Int): Int {
            val a = from.coerceIn(0, text.length)
            val b = to.coerceIn(0, text.length)
            return if (a <= b) graphemeCount(text.substring(a, b)) else -graphemeCount(text.substring(b, a))
        }

        /** Where the last [count] user-perceived characters before [end] start. */
        fun graphemeStart(text: String, end: Int, count: Int): Int {
            synchronized(characters) {
                characters.setText(text)
                var at = end
                repeat(count) {
                    val previous = characters.preceding(at)
                    if (previous == BreakIterator.DONE) return 0
                    at = previous
                }
                return at
            }
        }

        fun utf8Length(text: String): Int = text.toByteArray(Charsets.UTF_8).size

        /** The longest start of [text] that's at most [maxBytes] in UTF-8, whole code points. */
        fun utf8Prefix(text: String, maxBytes: Int): String {
            var bytes = 0
            var i = 0
            while (i < text.length) {
                val cp = text.codePointAt(i)
                val size = when {
                    cp < 0x80 -> 1
                    cp < 0x800 -> 2
                    cp < 0x10000 -> 3
                    else -> 4
                }
                if (bytes + size > maxBytes) break
                bytes += size
                i += Character.charCount(cp)
            }
            return text.substring(0, i)
        }

        /** The UTF-16 index of code point [codePoint] in [text], clamped. */
        fun utf16Index(text: String, codePoint: Int): Int {
            val total = text.codePointCount(0, text.length)
            return text.offsetByCodePoints(0, codePoint.coerceIn(0, total))
        }
    }
}

/**
 * What this phone typed into PC fields whose text isn't shared, to come back
 * to when the PC's focus returns to them. In memory only: [MAX_FIELDS] fields,
 * [MAX_CHARS] each, forgotten [TTL_MS] after leaving.
 */
internal class FieldDrafts(private val clock: () -> Long = System::currentTimeMillis) {
    /** A field's draft: [unverified] when the PC may have changed the field since it was typed. */
    data class Draft(val text: String, val unverified: Boolean)

    private data class Kept(val epoch: Int, val draft: Draft, val at: Long)

    private val drafts = LinkedHashMap<Long, Kept>()

    fun put(field: Long, epoch: Int, text: String, unverified: Boolean = false) {
        drafts.remove(field)
        drafts[field] = Kept(epoch, Draft(text.takeLast(MAX_CHARS), unverified), clock())
        while (drafts.size > MAX_FIELDS) drafts.remove(drafts.keys.first())
    }

    /** The draft for [field], unverified when the PC has doubted the field since ([epoch]). */
    fun take(field: Long, epoch: Int): Draft? {
        val kept = drafts.remove(field) ?: return null
        if (clock() - kept.at > TTL_MS) return null
        return if (kept.epoch == epoch) kept.draft else kept.draft.copy(unverified = true)
    }

    fun remove(field: Long) {
        drafts.remove(field)
    }

    fun clear() = drafts.clear()

    companion object {
        const val MAX_FIELDS = 8
        const val MAX_CHARS = 2000
        const val TTL_MS = 10 * 60 * 1000L

        private val byPc = HashMap<String, FieldDrafts>()

        /** The drafts for PC [pcId], kept while the app runs. */
        fun of(pcId: String): FieldDrafts = synchronized(byPc) { byPc.getOrPut(pcId) { FieldDrafts() } }
    }
}
