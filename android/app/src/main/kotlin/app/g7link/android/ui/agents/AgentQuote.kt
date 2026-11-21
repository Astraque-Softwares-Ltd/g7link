// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.mono

/**
 * Lines of a diff quoted into the reply box (contract §SC9.1 "Comment on a
 * line"): the shared shape every client uses, `Quote { path, first, last, text }`.
 */
@Immutable
data class Quote(val path: String, val first: Int, val last: Int, val text: String)

/** One diff line picked for a quote: its number in the file (the new side, else the old) and its text. */
@Immutable
data class QuotedLine(val number: Int, val text: String)

/**
 * The quote chip's rules. A chip lives in the draft's text: the reply box's
 * text is the chips' text forms, then the words typed, so the draft on the PC
 * keeps them and a message sends them as they read. The text form is the one
 * every client writes (`core/g7-agents/tests/fixtures/scm/quote/`): the line
 * `path:first–last`, then the lines in a fence longer than any run of
 * backticks inside them.
 */
object AgentQuotes {
    /** Chips a message may carry. */
    const val MAX = 5

    /** The most characters of lines one chip quotes. */
    const val MAX_CHARS = 2_000

    /** The longest path a chip names, in bytes. */
    const val MAX_PATH_BYTES = 512

    private const val DASH = '–'
    private const val GAP = "\n\n"
    private val HEAD = Regex("^(.+):(\\d{1,10})$DASH(\\d{1,10})$")

    /** The chip's label, "src/main.rs:12–14". */
    fun label(quote: Quote): String = "${quote.path}:${quote.first}$DASH${quote.last}"

    /** The text form: the label, then the lines in a fence longer than any backtick run in them. */
    fun text(quote: Quote): String {
        val body = quote.text.trimEnd('\n')
        val fence = "`".repeat(maxOf(3, longestRun(body, '`') + 1))
        return "${label(quote)}\n$fence\n$body\n$fence"
    }

    /**
     * A chip for lines picked in a diff of [path]: from the lowest number to
     * the highest, their text joined by line, cut to [MAX_CHARS]; null when
     * nothing numbered was picked or the path can't be named.
     */
    fun of(path: String, lines: List<QuotedLine>): Quote? {
        if (path.isEmpty() || AgentsModel.utf8Length(path) > MAX_PATH_BYTES || path.any { it == '\n' || it == '\r' }) return null
        val numbered = lines.filter { it.number > 0 }
        if (numbered.isEmpty()) return null
        val text = cut(numbered.joinToString("\n") { it.text }, MAX_CHARS)
        return Quote(path, numbered.minOf { it.number }, numbered.maxOf { it.number }, text)
    }

    /** The draft with [quotes] before the words typed; the words alone when there's no chip. */
    fun compose(quotes: List<Quote>, typed: String): String {
        if (quotes.isEmpty()) return typed
        val head = quotes.joinToString(GAP) { text(it) }
        return if (typed.isEmpty()) head else head + GAP + typed
    }

    /** A draft's chips and the words after them (the inverse of [compose]). */
    fun split(draft: String): Pair<List<Quote>, String> {
        val quotes = ArrayList<Quote>()
        var rest = draft
        while (quotes.size < MAX) {
            val (quote, after) = first(rest) ?: break
            quotes += quote
            rest = when {
                after.isEmpty() -> ""
                after.startsWith(GAP) -> after.substring(GAP.length)
                else -> {
                    // Words straight under a quote aren't how a draft is made: leave this one as typed text.
                    quotes.removeAt(quotes.size - 1)
                    break
                }
            }
        }
        if (quotes.isEmpty()) return emptyList<Quote>() to draft
        return quotes to rest
    }

    /** A chip added to a draft: refused (null) when it already has [MAX]. */
    fun add(draft: String, quote: Quote): String? {
        val (quotes, typed) = split(draft)
        if (quotes.size >= MAX) return null
        return compose(quotes + quote, typed)
    }

    /** The chip at [index] taken out of a draft, the rest left as it was. */
    fun remove(draft: String, index: Int): String {
        val (quotes, typed) = split(draft)
        if (index !in quotes.indices) return draft
        return compose(quotes.filterIndexed { i, _ -> i != index }, typed)
    }

    /** The words typed after the chips changed: the chips stay in front. */
    fun typed(draft: String, words: String): String = compose(split(draft).first, words)

    /** One quote at the start of [text] and what follows it, or null. */
    private fun first(text: String): Pair<Quote, String>? {
        val headEnd = text.indexOf('\n').takeIf { it > 0 } ?: return null
        val head = HEAD.matchEntire(text.substring(0, headEnd)) ?: return null
        val fenceEnd = text.indexOf('\n', headEnd + 1).takeIf { it > 0 } ?: return null
        val fence = text.substring(headEnd + 1, fenceEnd)
        if (fence.length < 3 || fence.any { it != '`' }) return null
        val close = "\n$fence"
        var at = fenceEnd
        while (true) {
            val found = text.indexOf(close, at)
            if (found < 0) return null
            val end = found + close.length
            if (end == text.length || text[end] == '\n') {
                val body = if (found >= fenceEnd + 1) text.substring(fenceEnd + 1, found) else ""
                val first = head.groupValues[2].toLongOrNull() ?: return null
                val last = head.groupValues[3].toLongOrNull() ?: return null
                if (first > Int.MAX_VALUE || last > Int.MAX_VALUE) return null
                return Quote(head.groupValues[1], first.toInt(), last.toInt(), body) to text.substring(end)
            }
            at = end
        }
    }

    private fun longestRun(text: String, char: Char): Int {
        var best = 0
        var run = 0
        for (c in text) {
            run = if (c == char) run + 1 else 0
            if (run > best) best = run
        }
        return best
    }

    /** [text] cut to [max] characters, never inside a surrogate pair. */
    private fun cut(text: String, max: Int): String {
        if (text.codePointCount(0, text.length) <= max) return text
        return text.substring(0, text.offsetByCodePoints(0, max))
    }
}

/** The chips over the reply box: the quote glyph, "path:first–last", and Remove. */
@Composable
internal fun QuoteChips(quotes: List<Quote>, onRemove: (Int) -> Unit, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    val fonts = LocalAppFonts.current
    Column(modifier.fillMaxWidth().padding(start = 8.dp, top = 8.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        quotes.forEachIndexed { index, quote ->
            Row(
                Modifier
                    .clip(RoundedCornerShape(8.dp))
                    .background(scheme.surfaceContainerHigh)
                    .heightIn(min = 36.dp)
                    .padding(start = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Icon(AgentGlyphs.comment, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(15.dp))
                Text(
                    AgentQuotes.label(quote),
                    style = MaterialTheme.typography.labelMedium.mono(fonts),
                    maxLines = 1,
                    overflow = TextOverflow.StartEllipsis,
                    modifier = Modifier.weight(1f, fill = false),
                )
                G7IconButton(
                    icon = G7Icons.close,
                    contentDescription = stringResource(R.string.agents_quote_remove),
                    onClick = { onRemove(index) },
                    tint = scheme.onSurfaceVariant,
                )
            }
        }
    }
}
