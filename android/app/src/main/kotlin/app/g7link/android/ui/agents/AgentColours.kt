// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.g7link.android.ui.theme.AgentPalette
import app.g7link.android.ui.theme.AgentWash
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7TintTokens
import app.g7link.android.ui.theme.G7Tints
import app.g7link.android.ui.theme.agentSyntaxStyles
import app.g7link.android.ui.theme.g7Tints
import app.g7link.core.AgentRowStatus
import app.g7link.core.AgentTokClass
import app.g7link.core.AgentToolClass

/** What a theme gives the agent timeline to colour with (Material's roles and the glyph tints). */
@Immutable
data class ThemeInks(
    val surface: Color,
    val code: Color,
    val chip: Color,
    val ink: Color,
    val muted: Color,
    val outline: Color,
    val link: Color,
    val tints: G7Tints,
)

/** The agent timeline's colours for the theme on screen (plans/agent-colour.md). */
@Composable
internal fun agentPalette(): AgentPalette {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val inks = ThemeInks(
        surface = scheme.surface,
        code = scheme.surfaceContainer,
        chip = scheme.surfaceContainerHigh,
        ink = scheme.onSurface,
        muted = scheme.onSurfaceVariant,
        outline = scheme.outline,
        link = scheme.primary,
        tints = tints,
    )
    return remember(inks) { AgentColours.palette(inks) }
}

/**
 * The colour system of plans/agent-colour.md on the phone: G7's own palette
 * as generated from tokens.json, or, on another theme, the same roles taken
 * from its colours and made legible (§2.1.6). Every mapping is an exhaustive
 * `when`, so a new class or kind can't go uncoloured.
 */
object AgentColours {
    /** Text passes this on the worst ground it can meet (§2.1.4). */
    const val TEXT_RATIO = 4.5f

    /** G7's palette for G7's theme; another theme's colours on the same roles otherwise. */
    fun palette(inks: ThemeInks): AgentPalette {
        if (inks.tints == G7TintTokens.dark && inks.surface == AgentPalette.dark.groundSurface) return AgentPalette.dark
        if (inks.tints == G7TintTokens.light && inks.surface == AgentPalette.light.groundSurface) return AgentPalette.light
        return derived(inks)
    }

    /** A token class's colour (§2.4). */
    fun tok(cls: AgentTokClass, p: AgentPalette): Color = when (cls) {
        AgentTokClass.PLAIN -> p.syntaxPlain
        AgentTokClass.COMMENT -> p.syntaxComment
        AgentTokClass.KEYWORD -> p.syntaxKeyword
        AgentTokClass.STRING -> p.syntaxString
        AgentTokClass.NUMBER -> p.syntaxNumber
        AgentTokClass.CONSTANT -> p.syntaxConstant
        AgentTokClass.TYPE -> p.syntaxType
        AgentTokClass.FUNCTION -> p.syntaxFunction
        AgentTokClass.VARIABLE -> p.syntaxVariable
        AgentTokClass.OPERATOR -> p.syntaxOperator
        AgentTokClass.PUNCTUATION -> p.syntaxPunctuation
        AgentTokClass.TAG -> p.syntaxTag
        AgentTokClass.ATTRIBUTE -> p.syntaxAttribute
        AgentTokClass.INSERTED -> p.syntaxInserted
        AgentTokClass.DELETED -> p.syntaxDeleted
        AgentTokClass.DIR -> p.syntaxDir
        AgentTokClass.FILE -> p.syntaxFile
        AgentTokClass.POS -> p.syntaxPos
        AgentTokClass.ERROR -> p.syntaxError
        AgentTokClass.WARN -> p.syntaxWarn
        AgentTokClass.OK -> p.syntaxOk
    }

    /** A token class's word in tokens.json (`agentSyntaxStyles`' keys). */
    fun word(cls: AgentTokClass): String = when (cls) {
        AgentTokClass.PLAIN -> "plain"
        AgentTokClass.COMMENT -> "comment"
        AgentTokClass.KEYWORD -> "keyword"
        AgentTokClass.STRING -> "string"
        AgentTokClass.NUMBER -> "number"
        AgentTokClass.CONSTANT -> "constant"
        AgentTokClass.TYPE -> "type"
        AgentTokClass.FUNCTION -> "function"
        AgentTokClass.VARIABLE -> "variable"
        AgentTokClass.OPERATOR -> "operator"
        AgentTokClass.PUNCTUATION -> "punctuation"
        AgentTokClass.TAG -> "tag"
        AgentTokClass.ATTRIBUTE -> "attribute"
        AgentTokClass.INSERTED -> "inserted"
        AgentTokClass.DELETED -> "deleted"
        AgentTokClass.DIR -> "dir"
        AgentTokClass.FILE -> "file"
        AgentTokClass.POS -> "pos"
        AgentTokClass.ERROR -> "error"
        AgentTokClass.WARN -> "warn"
        AgentTokClass.OK -> "ok"
    }

    /** A token's colour and font style: comments italic, keywords and tags semibold. */
    fun tokStyle(cls: AgentTokClass, p: AgentPalette): SpanStyle {
        val style = agentSyntaxStyles[word(cls)]
        return SpanStyle(
            color = tok(cls, p),
            fontStyle = if (style == "italic") FontStyle.Italic else null,
            fontWeight = if (style == "semibold") FontWeight.SemiBold else null,
        )
    }

    /** A kind of step's glyph colour, the way G7 draws every feature glyph (§2.6). */
    fun toolGlyph(cls: AgentToolClass?, p: AgentPalette): Color = when (cls) {
        AgentToolClass.COMMAND -> p.toolCommand
        AgentToolClass.EDIT -> p.toolEdit
        AgentToolClass.READ -> p.toolRead
        AgentToolClass.SEARCH -> p.toolSearch
        AgentToolClass.WEB -> p.toolWeb
        AgentToolClass.AGENT -> p.toolAgent
        AgentToolClass.MCP -> p.toolMcp
        AgentToolClass.TASKS -> p.toolTasks
        AgentToolClass.PLAN -> p.toolPlan
        AgentToolClass.OTHER, null -> p.toolOther
    }

    /** A step's status colour (§2.6). */
    fun toolStatus(status: AgentRowStatus, p: AgentPalette): Color = when (status) {
        AgentRowStatus.RUNNING -> p.toolStatusRunning
        AgentRowStatus.OK -> p.toolStatusOk
        AgentRowStatus.FAILED -> p.toolStatusFailed
        AgentRowStatus.DENIED -> p.toolStatusDenied
        AgentRowStatus.STOPPED -> p.toolStatusStopped
        AgentRowStatus.ASKING -> p.toolStatusAsking
    }

    /** The ok check shows on commands only: forty reads aren't forty checks (decision 4). */
    fun showsOk(cls: AgentToolClass): Boolean = cls == AgentToolClass.COMMAND

    /** A thread's state colour (§2.9): done and plan ready are green. */
    fun status(status: ThreadStatus, p: AgentPalette): Color = when (status) {
        ThreadStatus.Working -> p.statusWorking
        ThreadStatus.NeedsYou -> p.statusNeedsYou
        ThreadStatus.Done, ThreadStatus.PlanReady -> p.statusDone
        ThreadStatus.Stopped, is ThreadStatus.Paused -> p.statusPaused
        is ThreadStatus.Failed -> p.statusFailed
    }

    /** WCAG 2's contrast ratio of two opaque colours. */
    fun contrast(a: Color, b: Color): Float {
        val la = a.luminance()
        val lb = b.luminance()
        return (maxOf(la, lb) + 0.05f) / (minOf(la, lb) + 0.05f)
    }

    /** [color] moved towards the ink of [ground] until it passes [ratio] on it (§2.1.6). */
    fun legible(color: Color, ground: Color, ratio: Float = TEXT_RATIO): Color {
        val opaque = color.copy(alpha = 1f)
        if (contrast(opaque, ground) >= ratio) return opaque
        val ink = if (ground.luminance() < 0.5f) Color.White else Color.Black
        for (step in 1..LEGIBLE_STEPS) {
            val moved = mix(opaque, ink, step.toFloat() / LEGIBLE_STEPS)
            if (contrast(moved, ground) >= ratio) return moved
        }
        return ink
    }

    /** [wash] over [ground]: what it looks like on screen. */
    fun over(wash: AgentWash, ground: Color): Color = mix(ground, wash.color.copy(alpha = 1f), wash.alpha)

    private const val LEGIBLE_STEPS = 40

    private fun mix(a: Color, b: Color, t: Float) = Color(
        red = a.red + (b.red - a.red) * t,
        green = a.green + (b.green - a.green) * t,
        blue = a.blue + (b.blue - a.blue) * t,
    )

    /** The roles of §2 from another theme's colours, each made legible on the ground it meets. */
    private fun derived(inks: ThemeInks): AgentPalette {
        val dark = inks.surface.luminance() < 0.5f
        val base = if (dark) AgentPalette.dark else AgentPalette.light
        val t = inks.tints
        val chip = inks.chip
        val surface = inks.surface
        // Text on code meets the chip ground at worst; text in lines meets the surface and the code ground.
        fun code(c: Color) = legible(c, chip)
        fun line(c: Color) = legible(legible(c, surface), inks.code)
        val ink = code(inks.ink)
        val muted = code(inks.muted)
        val dim = code(mix(inks.muted, inks.ink, 0.5f))
        val lavenderInk = code(mix(t.lavender, inks.ink, 0.5f))
        val addInk = code(mix(t.green, inks.ink, 0.35f))
        val delInk = code(mix(t.red, inks.ink, 0.35f))
        return base.copy(
            groundChip = chip,
            groundCode = inks.code,
            groundSurface = surface,
            stepsAddInk = addInk,
            stepsDelInk = delInk,
            stepsDim = dim,
            stepsLavenderInk = lavenderInk,
            syntaxAttribute = code(t.amber),
            syntaxComment = muted,
            syntaxConstant = code(t.coral),
            syntaxDeleted = if (dark) code(t.red) else delInk,
            syntaxDir = muted,
            syntaxError = delInk,
            syntaxFile = ink,
            syntaxFunction = code(t.blue),
            syntaxInserted = code(t.green),
            syntaxKeyword = code(t.pink),
            syntaxNumber = code(t.coral),
            syntaxOk = code(t.green),
            syntaxOperator = dim,
            syntaxPlain = ink,
            syntaxPos = muted,
            syntaxPunctuation = muted,
            syntaxString = code(t.teal),
            syntaxTag = code(t.pink),
            syntaxType = code(t.amber),
            syntaxVariable = lavenderInk,
            syntaxWarn = code(t.amber),
            diffAddRow = AgentWash(t.green, base.diffAddRow.alpha),
            diffAddSign = addInk,
            diffAddWord = AgentWash(t.green, base.diffAddWord.alpha),
            diffDelRow = AgentWash(t.red, base.diffDelRow.alpha),
            diffDelSign = delInk,
            diffDelWord = AgentWash(t.red, base.diffDelWord.alpha),
            diffHunk = muted,
            diffHunkGround = chip,
            diffLineNo = dim,
            diffText = ink,
            statusDone = line(t.green),
            statusFailed = line(t.red),
            statusFailedText = line(delInk),
            statusFailedWash = AgentWash(t.red, base.statusFailedWash.alpha),
            statusMayNeedYou = line(t.amber),
            statusNeedsYou = line(t.amber),
            statusOnNeedsYou = surface,
            statusPaused = inks.outline,
            statusWorking = line(t.blue),
            toolStatusAsking = line(t.amber),
            toolStatusDenied = line(inks.muted),
            toolStatusFailed = line(t.red),
            toolStatusOk = line(t.green),
            toolStatusRunning = line(t.blue),
            toolStatusStopped = line(inks.muted),
            toolAgent = line(t.lavender),
            toolCommand = line(t.teal),
            toolEdit = line(t.coral),
            toolMcp = line(inks.muted),
            toolOther = line(inks.muted),
            toolPlan = line(inks.muted),
            toolRead = line(t.blue),
            toolSearch = line(t.pink),
            toolTasks = line(inks.muted),
            toolWeb = line(t.blue),
            proseAlertCaution = line(t.red),
            proseAlertImportant = line(t.lavender),
            proseAlertNote = line(t.blue),
            proseAlertTip = line(t.teal),
            proseAlertWarning = line(t.amber),
            proseDoneBox = line(t.green),
            proseHeadingRule = inks.link,
            proseInlineCode = code(t.coral),
            proseLink = line(inks.link),
            proseListMarker = line(inks.muted),
            proseTableHeader = line(inks.muted),
            proseThinking = line(inks.muted),
        )
    }
}

/**
 * A thread's state as a mark (§2.9): working a ring, needs you a filled dot
 * with a bang, failed an x, done a check, paused or stopped a hollow ring. Six
 * states can't be told apart by colour alone, so each has its own shape.
 */
@Composable
internal fun ThreadStatusMark(status: ThreadStatus, modifier: Modifier = Modifier, size: Dp = 10.dp) {
    val p = agentPalette()
    val color = AgentColours.status(status, p)
    when (status) {
        ThreadStatus.Working -> RingMark(color, open = true, modifier = modifier, size = size)
        ThreadStatus.NeedsYou -> BangMark(color, p.statusOnNeedsYou, modifier, size)
        is ThreadStatus.Failed -> Icon(G7Icons.close, contentDescription = null, tint = color, modifier = modifier.size(size + 2.dp))
        ThreadStatus.Done, ThreadStatus.PlanReady -> Icon(G7Icons.check, contentDescription = null, tint = color, modifier = modifier.size(size + 2.dp))
        ThreadStatus.Stopped, is ThreadStatus.Paused -> RingMark(color, open = false, modifier = modifier, size = size)
    }
}

/**
 * A thread's state in words as §2.9 draws it: needs you a solid amber chip,
 * failed a red wash, working blue, done and plan ready green, the rest muted.
 */
@Composable
internal fun StatusWord(status: ThreadStatus, word: String, style: TextStyle = MaterialTheme.typography.labelMedium) {
    val p = agentPalette()
    when (status) {
        ThreadStatus.NeedsYou -> Text(
            word,
            style = style.copy(fontWeight = FontWeight.SemiBold),
            color = p.statusOnNeedsYou,
            maxLines = 1,
            modifier = Modifier.clip(RoundedCornerShape(4.dp)).background(p.statusNeedsYou).padding(horizontal = 6.dp, vertical = 1.dp),
        )
        is ThreadStatus.Failed -> Text(
            word,
            style = style.copy(fontWeight = FontWeight.SemiBold),
            color = p.statusFailedText,
            maxLines = 1,
            modifier = Modifier
                .clip(RoundedCornerShape(4.dp))
                .background(p.statusFailedWash.color.copy(alpha = p.statusFailedWash.alpha))
                .padding(horizontal = 6.dp, vertical = 1.dp),
        )
        ThreadStatus.Working -> Text(word, style = style, color = p.statusWorking, maxLines = 1)
        ThreadStatus.Done, ThreadStatus.PlanReady -> Text(word, style = style, color = p.statusDone, maxLines = 1)
        ThreadStatus.Stopped, is ThreadStatus.Paused -> Text(word, style = style, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
    }
}

/** A followed session's state as a mark: needs you, may need you (a hollow amber ring: a guess), working, else a hollow ring. */
@Composable
internal fun SessionStatusMark(needsYou: Boolean, mayNeedYou: Boolean, working: Boolean, modifier: Modifier = Modifier, size: Dp = 10.dp) {
    val p = agentPalette()
    when {
        mayNeedYou -> RingMark(p.statusMayNeedYou, open = false, modifier = modifier, size = size)
        needsYou -> BangMark(p.statusNeedsYou, p.statusOnNeedsYou, modifier, size)
        working -> RingMark(p.statusWorking, open = true, modifier = modifier, size = size)
        else -> RingMark(p.statusPaused, open = false, modifier = modifier, size = size)
    }
}

/** A ring; [open] leaves a quarter out, as a still spinner. */
@Composable
private fun RingMark(color: Color, open: Boolean, modifier: Modifier, size: Dp) {
    Canvas(modifier.size(size)) {
        val stroke = 1.5.dp.toPx()
        val inset = stroke / 2f
        val arc = Size(this.size.width - stroke, this.size.height - stroke)
        drawArc(
            color = color,
            startAngle = -90f,
            sweepAngle = if (open) 270f else 360f,
            useCenter = false,
            topLeft = Offset(inset, inset),
            size = arc,
            style = Stroke(width = stroke, cap = StrokeCap.Round),
        )
    }
}

/** A filled dot with a bang in the ground's colour. */
@Composable
private fun BangMark(color: Color, on: Color, modifier: Modifier, size: Dp) {
    Canvas(modifier.size(size)) {
        drawCircle(color)
        val w = this.size.width
        val bar = w * 0.14f
        val x = (w - bar) / 2f
        drawRoundRect(on, topLeft = Offset(x, w * 0.2f), size = Size(bar, w * 0.38f), cornerRadius = CornerRadius(bar / 2f))
        drawCircle(on, radius = bar * 0.62f, center = Offset(w / 2f, w * 0.75f))
    }
}
