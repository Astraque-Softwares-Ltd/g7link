// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.foundation.text.appendInlineContent
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import app.g7link.android.ui.theme.G7Tints
import app.g7link.android.ui.theme.MONO_FEATURES
import app.g7link.core.AgentCodeLine
import app.g7link.core.AgentLink
import app.g7link.core.AgentSpan
import app.g7link.core.AgentTokClass

/**
 * Agent text as Compose draws it: Markdown blocks' text and spans from the FFI
 * as [AnnotatedString]s, code lines coloured by token class, and our own
 * translated templates with styled arguments. Nothing here parses Markdown, loads
 * anything or makes a link of text the render crate didn't (contract §P6.2).
 */
internal object AgentText {
    /** The inline content id of the image glyph before an image's alt text. */
    const val IMAGE_GLYPH = "agent-image"

    /** A piece of a translated template: its own text, or argument [arg] (from 0). */
    sealed interface Piece {
        data class Literal(val text: String) : Piece

        data class Arg(val arg: Int) : Piece
    }

    private val PLACEHOLDER = Regex("%(?:(\\d+)\\$)?([sd%])")

    /**
     * A template from the resources ("Ran %1\$s", "%1\$s and %2\$d more") in
     * pieces; `%%` is a percent sign, and a placeholder without a position takes
     * the next argument.
     */
    fun pieces(template: String): List<Piece> {
        val out = ArrayList<Piece>()
        var at = 0
        var next = 0
        val literal = StringBuilder()
        for (match in PLACEHOLDER.findAll(template)) {
            literal.append(template, at, match.range.first)
            at = match.range.last + 1
            if (match.groupValues[2] == "%") {
                literal.append('%')
                continue
            }
            if (literal.isNotEmpty()) {
                out += Piece.Literal(literal.toString())
                literal.setLength(0)
            }
            val position = match.groupValues[1].toIntOrNull()
            out += Piece.Arg(if (position != null) position - 1 else next)
            next = (position ?: (next + 1))
        }
        literal.append(template, at, template.length)
        if (literal.isNotEmpty()) out += Piece.Literal(literal.toString())
        return out
    }

    /** A translated template with styled arguments; a missing argument is left out. */
    fun format(template: String, vararg args: AnnotatedString): AnnotatedString = buildAnnotatedString {
        for (piece in pieces(template)) {
            when (piece) {
                is Piece.Literal -> append(piece.text)
                is Piece.Arg -> args.getOrNull(piece.arg)?.let { append(it) }
            }
        }
    }

    /**
     * [spans] made safe for [length]: in order, inside the text, not overlapping
     * and not empty. The render crate already keeps them so (UTF-16 offsets,
     * Kotlin's string indices); this only makes a wrong one harmless.
     */
    fun safeSpans(length: Int, spans: List<AgentSpan>): List<Pair<IntRange, AgentSpan>> {
        val out = ArrayList<Pair<IntRange, AgentSpan>>(spans.size)
        var floor = 0
        for (span in spans.sortedBy { it.start }) {
            val start = span.start.toLong().coerceIn(floor.toLong(), length.toLong()).toInt()
            val end = (span.start.toLong() + span.len.toLong()).coerceIn(start.toLong(), length.toLong()).toInt()
            if (end <= start) continue
            out.add((start until end) to span)
            floor = end
        }
        return out
    }

    /** The colours and font of agent text. */
    data class Look(
        val mono: FontFamily,
        val link: Color,
        val codeBackground: Color,
    )

    /**
     * A block's [text] with its [spans]: strong, emphasis, struck and inline code
     * as styles; a link as a link annotation that only calls [onLink] with the
     * span (the target is shown before anything opens, unless it's the text
     * itself); an image's alt text after the image glyph.
     */
    fun styled(text: String, spans: List<AgentSpan>, look: Look, onLink: ((AgentSpan) -> Unit)?): AnnotatedString = buildAnnotatedString {
        var at = 0
        for ((range, span) in safeSpans(text.length, spans)) {
            if (range.first > at) append(text, at, range.first)
            val part = text.substring(range.first, range.last + 1)
            val style = span.style
            val spanStyle = SpanStyle(
                fontWeight = if (style.strong) FontWeight.SemiBold else null,
                fontStyle = if (style.em) FontStyle.Italic else null,
                textDecoration = if (style.strike) TextDecoration.LineThrough else null,
                fontFamily = if (style.code) look.mono else null,
                fontFeatureSettings = if (style.code) MONO_FEATURES else null,
                background = if (style.code) look.codeBackground else Color.Unspecified,
            )
            val tapped = onLink?.takeIf { style.link != AgentLink.NONE && span.href.isNotEmpty() }
            if (style.image) appendInlineContent(IMAGE_GLYPH)
            if (tapped != null) {
                val link = LinkAnnotation.Clickable(
                    tag = "agent-link",
                    styles = TextLinkStyles(SpanStyle(color = look.link, textDecoration = TextDecoration.Underline)),
                ) { tapped(span) }
                withLink(link) { withStyle(spanStyle) { append(part) } }
            } else {
                withStyle(spanStyle) { append(part) }
            }
            at = range.last + 1
        }
        if (at < text.length) append(text, at, text.length)
    }

    /** A token class's colour: the glyph tints, never the diff's green and red, the amber or the lavender. */
    fun tokColor(cls: AgentTokClass, tints: G7Tints, plain: Color, muted: Color): Color = when (cls) {
        AgentTokClass.COMMENT -> muted
        AgentTokClass.KEYWORD, AgentTokClass.TAG -> tints.pink
        AgentTokClass.STRING -> tints.teal
        AgentTokClass.NUMBER, AgentTokClass.CONSTANT -> tints.coral
        AgentTokClass.TYPE, AgentTokClass.ATTRIBUTE -> tints.blue
        AgentTokClass.PLAIN,
        AgentTokClass.FUNCTION,
        AgentTokClass.VARIABLE,
        AgentTokClass.OPERATOR,
        AgentTokClass.PUNCTUATION,
        AgentTokClass.INSERTED,
        AgentTokClass.DELETED,
        -> plain
    }

    /** A code line coloured by its tokens; what the tokens don't cover stays plain. */
    fun codeLine(line: AgentCodeLine, color: (AgentTokClass) -> Color): AnnotatedString = buildAnnotatedString {
        val text = line.text
        var at = 0
        for (tok in line.toks) {
            if (at >= text.length) break
            val end = (at.toLong() + tok.len.toLong()).coerceAtMost(text.length.toLong()).toInt()
            if (end <= at) continue
            if (tok.tokClass == AgentTokClass.PLAIN) {
                append(text, at, end)
            } else {
                withStyle(SpanStyle(color = color(tok.tokClass))) { append(text, at, end) }
            }
            at = end
        }
        if (at < text.length) append(text, at, text.length)
    }

    /** A duration as the fold says it: "12s", "2m 14s", "1h 03m" (t3code's `AgentElapsed`). */
    data class Worked(val hours: Long, val minutes: Long, val seconds: Long)

    fun worked(ms: Long): Worked {
        val total = ms.coerceAtLeast(0L) / 1000L
        return Worked(total / 3600L, (total % 3600L) / 60L, total % 60L)
    }

    /** A link a phone may open: the render crate allows only these; checked again here. */
    fun openable(href: String): Boolean {
        val scheme = href.substringBefore(':', "").trim().lowercase()
        return scheme == "http" || scheme == "https" || scheme == "mailto"
    }

    /**
     * A link's target as the sheet shows it before it opens: as the render crate
     * let it through, with an internationalised host in its ASCII (punycode) form,
     * so a look-alike host can't pass for another.
     */
    fun shownTarget(href: String): String {
        val schemeEnd = href.indexOf("://")
        if (schemeEnd < 0) {
            val at = href.lastIndexOf('@')
            if (!href.startsWith("mailto:", ignoreCase = true) || at < 0) return href
            val end = href.indexOfAny(charArrayOf('?', '#'), at).let { if (it < 0) href.length else it }
            return href.substring(0, at + 1) + asciiHost(href.substring(at + 1, end)) + href.substring(end)
        }
        val start = schemeEnd + 3
        val end = href.indexOfAny(charArrayOf('/', '?', '#'), start).let { if (it < 0) href.length else it }
        val authority = href.substring(start, end)
        val hostStart = authority.lastIndexOf('@') + 1
        val bracketed = authority.startsWith("[", hostStart)
        val hostEnd = if (bracketed) authority.length else authority.indexOf(':', hostStart).let { if (it < 0) authority.length else it }
        val host = authority.substring(hostStart, hostEnd)
        return href.substring(0, start) + authority.substring(0, hostStart) + asciiHost(host) + authority.substring(hostEnd) + href.substring(end)
    }

    private fun asciiHost(host: String): String =
        if (host.all { it.code < 0x80 }) host else runCatching { java.net.IDN.toASCII(host, java.net.IDN.ALLOW_UNASSIGNED) }.getOrDefault(host)

    /** "core/ble/src/" and "hold.rs" of a relative path, for a muted folder and the name in full. */
    fun splitPath(path: String): Pair<String, String> {
        val cut = path.lastIndexOf('/')
        return if (cut < 0) "" to path else path.substring(0, cut + 1) to path.substring(cut + 1)
    }

    /** The last [max] lines of [text], and whether any were left out. */
    fun lastLines(text: String, max: Int): Pair<String, Boolean> {
        var count = 0
        var at = text.length
        while (at > 0) {
            val nl = text.lastIndexOf('\n', at - 1)
            count++
            if (count >= max) {
                return if (nl < 0) text to false else text.substring(nl + 1) to true
            }
            if (nl < 0) break
            at = nl
        }
        return text to false
    }
}
