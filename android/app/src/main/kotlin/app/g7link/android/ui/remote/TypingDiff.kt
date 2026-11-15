// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

/**
 * What changed in the phone's typing field, as keys for the PC: delete
 * [backspaces] characters at the cursor, then type [added]. The field keeps
 * what was typed so the keyboard can autocorrect and stay on its symbols
 * page; a word it corrects ("teh" to "the") comes out as backspaces and the
 * new word.
 */
internal data class TypingDiff(val backspaces: Int, val added: String)

internal fun typingDiff(old: String, new: String): TypingDiff {
    var same = 0
    val shorter = minOf(old.length, new.length)
    while (same < shorter && old[same] == new[same]) same++
    // Never split an emoji (or any character outside the BMP) in two.
    if (same > 0 && Character.isHighSurrogate(old[same - 1]) && same < old.length) same--
    val removed = old.substring(same)
    return TypingDiff(
        backspaces = removed.codePointCount(0, removed.length),
        added = new.substring(same),
    )
}

/**
 * Whether the phone's Enter, pressed with [field] in the typing field, reaches
 * the PC as a plain Enter instead of Shift+Enter (a new line): when there's no
 * line of text to break, because nothing was typed here (a link pasted on the
 * PC, say), or the field holds a single link. A browser's address bar opens a
 * new window on Shift+Enter.
 */
internal fun enterGoesAsIs(field: String): Boolean {
    val text = field.trim()
    return text.isEmpty() || LINK.matches(text)
}

/** A web address on its own: with a scheme, starting with `www.`, or a bare domain and path. */
private val LINK = Regex("""(?i)(https?://\S+|www\.\S+|[a-z0-9-]+(\.[a-z0-9-]+)*\.[a-z]{2,}(:\d+)?([/?#]\S*)?)""")
