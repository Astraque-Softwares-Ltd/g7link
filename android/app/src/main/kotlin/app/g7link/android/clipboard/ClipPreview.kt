// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.clipboard

/** How the Send clipboard sheet shows what was copied. */
internal object ClipPreview {
    /** The lines of copied text the sheet's card shows. */
    const val LINES = 6

    /** The most characters the card lays out, however few lines they make. */
    const val CHARS = 600

    /** The confirmation's one line. */
    const val ONE_LINE_CHARS = 120

    private const val MINUTE_MS = 60_000L

    /** When something was copied, as the sheet says it. */
    sealed interface Copied {
        /** Under a minute ago. */
        data object JustNow : Copied

        /** Longer ago: said relative to now, from [atMillis]. */
        data class At(val atMillis: Long) : Copied
    }

    /**
     * When [copiedAtMillis] was, seen at [nowMillis]; null when Android gave no time (0)
     * or one well in the future (the clock moved), so the sheet says nothing rather
     * than something wrong. A little ahead of now still reads as just now.
     */
    fun copied(copiedAtMillis: Long?, nowMillis: Long): Copied? {
        if (copiedAtMillis == null || copiedAtMillis <= 0L) return null
        val age = nowMillis - copiedAtMillis
        return when {
            age < -MINUTE_MS -> null
            age < MINUTE_MS -> Copied.JustNow
            else -> Copied.At(copiedAtMillis)
        }
    }

    /**
     * The first [maxLines] lines of [text] (blank lines before and after dropped, each
     * line's trailing spaces too), at most [maxChars] long, ending in "…" when there's more.
     */
    fun lines(text: String, maxLines: Int = LINES, maxChars: Int = CHARS): String {
        val all = text.lineSequence().dropWhile { it.isBlank() }
        val head = all.take(maxLines).toList().dropLastWhile { it.isBlank() }.map { it.trimEnd() }
        var cut = all.drop(maxLines).any { it.isNotBlank() }
        var shown = head.joinToString("\n")
        if (shown.length > maxChars) {
            shown = shown.cutAt(maxChars).trimEnd()
            cut = true
        }
        return if (cut) "$shown…" else shown
    }

    /** [text] on one line: every run of spaces and line breaks made one space, at most [maxChars] long. */
    fun oneLine(text: String, maxChars: Int = ONE_LINE_CHARS): String {
        val out = StringBuilder()
        var space = false
        for (ch in text) {
            if (ch.isWhitespace()) {
                space = out.isNotEmpty()
                continue
            }
            if (space) {
                out.append(' ')
                space = false
            }
            out.append(ch)
            if (out.length > maxChars) break
        }
        return if (out.length > maxChars) out.toString().cutAt(maxChars).trimEnd() + "…" else out.toString()
    }

    /** How many characters [text] has, as people count them: an emoji or accented letter is one. */
    fun characters(text: String): Int = text.codePointCount(0, text.length)

    /** An image type's short name ("PNG", "JPEG"), or null when it isn't an image type. */
    fun imageFormat(mime: String?): String? {
        val subtype = mime?.lowercase()?.takeIf { it.startsWith("image/") }?.substringAfter('/') ?: return null
        return when (subtype) {
            "jpeg", "jpg", "pjpeg" -> "JPEG"
            else -> subtype.substringBefore('+').substringBefore(';').removePrefix("x-").uppercase()
                .takeIf { it.isNotBlank() && it.length <= 8 }
        }
    }

    /** An image's type, pixel size and file size, those that are known, joined as the sheet shows them. */
    fun imageDetails(format: String?, width: Int, height: Int, size: String?): String =
        listOfNotNull(
            format,
            if (width > 0 && height > 0) "$width × $height" else null,
            size,
        ).joinToString(" · ")

    /** The first [count] characters, one fewer when that would split an emoji in two. */
    private fun String.cutAt(count: Int): String {
        if (length <= count) return this
        val end = if (count > 0 && this[count - 1].isHighSurrogate()) count - 1 else count
        return substring(0, end)
    }
}
