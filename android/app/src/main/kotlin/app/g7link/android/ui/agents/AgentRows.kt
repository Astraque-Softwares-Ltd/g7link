// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.res.Resources
import android.os.Build
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.InlineTextContent
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.Placeholder
import androidx.compose.ui.text.PlaceholderVerticalAlign
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.currentStateAsState
import app.g7link.android.R
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.LocalReducedMotion
import app.g7link.android.ui.theme.MONO_FEATURES
import app.g7link.android.ui.theme.g7Tints
import app.g7link.android.ui.theme.mono
import app.g7link.core.AgentAlertKind
import app.g7link.core.AgentAlign
import app.g7link.core.AgentBlock
import app.g7link.core.AgentBlockKind
import app.g7link.core.AgentClause
import app.g7link.core.AgentDetailOf
import app.g7link.core.AgentDiffDoc
import app.g7link.core.AgentDiffKind
import app.g7link.core.AgentItem
import app.g7link.core.AgentLink
import app.g7link.core.AgentLiveLabel
import app.g7link.core.AgentMarkKind
import app.g7link.core.AgentMarker
import app.g7link.core.AgentRow
import app.g7link.core.AgentRowKind
import app.g7link.core.AgentRowStatus
import app.g7link.core.AgentSpan
import app.g7link.core.AgentSummary
import app.g7link.core.AgentTable
import app.g7link.core.AgentToolClass
import app.g7link.core.AgentToolLabel
import kotlinx.coroutines.delay

// The gutter of a step's line (the design's GUT and LABEL_X): disclosure, gap, glyph, gap.
private val GUTTER = 14.dp
private val GLYPH = 16.dp
private val LABEL_X = GUTTER + 4.dp + GLYPH + 8.dp

/** How far a nested row (inside an open group, fold or step) moves in, with its hairline. */
private val NEST = 17.dp

/** Lines of output and diff rows a thread shows before "Show all" opens them full-screen. */
private const val INLINE_OUTPUT_LINES = 12
private const val INLINE_DIFF_ROWS = 40

/** What the rows need besides themselves: the items they came from (by id) and the folder's label. */
@Immutable
internal class RowsContext(val items: Map<String, AgentItem>, val folder: String)

/** What a tap on a row does. */
@Immutable
internal class RowActions(
    /** Opens or closes the row with this key (a step, group, thought, fold or changed file). */
    val toggle: (String) -> Unit,
    /** A link was tapped: open it, or show where it goes first. */
    val link: (AgentSpan, String) -> Unit,
    /** Long-press Copy on a step, or Copy on code, a command or output. */
    val copy: (String) -> Unit,
    /** A step's output in full: the tool item's id. */
    val output: (String) -> Unit,
    /** An edit in full, with its heading ("Edit 1 of 3"). */
    val diff: (String, AgentDiffDoc) -> Unit,
    /** The "Waiting for you" line: bring the request into view. */
    val mark: () -> Unit,
)

/**
 * A thread's rows from the FFI (contract §P6.2), keyed by their keys and typed
 * by their kind; an Earlier row on top when more can be read; following the
 * end while it's in view. [footer] adds the thread's end (failed, paused).
 */
@Composable
internal fun AgentRowsList(
    rows: List<AgentRow>,
    context: RowsContext,
    actions: RowActions,
    earlier: Boolean,
    loadingEarlier: Boolean,
    onLoadEarlier: () -> Unit,
    listState: LazyListState,
    modifier: Modifier = Modifier,
    footer: (LazyListScope.() -> Unit)? = null,
) {
    // Follows new rows while the end is in view; scrolling up stops that, scrolling back down resumes it.
    var follow by remember { mutableStateOf(true) }
    LaunchedEffect(listState) {
        snapshotFlow { listState.isScrollInProgress to listState.canScrollForward }
            .collect { scroll -> if (scroll.first) follow = !scroll.second }
    }
    LaunchedEffect(rows) {
        if (follow && rows.isNotEmpty()) {
            val total = listState.layoutInfo.totalItemsCount
            if (total > 0) {
                listState.scrollToItem(total - 1)
                listState.scrollBy(END_SCROLL)
            }
        }
    }
    LazyColumn(
        state = listState,
        modifier = modifier.fillMaxWidth(),
        contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 12.dp),
    ) {
        if (earlier) {
            item(key = "earlier", contentType = "earlier") {
                Box(Modifier.fillMaxWidth().padding(bottom = 12.dp), contentAlignment = Alignment.Center) {
                    if (loadingEarlier) {
                        CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                    } else {
                        G7RowAction(label = stringResource(R.string.agents_load_earlier), onClick = onLoadEarlier)
                    }
                }
            }
        }
        items(rows.size, key = { rows[it].key }, contentType = { kindName(rows[it].kind) }) { index ->
            val row = rows[index]
            val gap = topGap(rows.getOrNull(index - 1), row)
            NestedRow(row.depth.toInt(), gap) { AgentRowView(row, context, actions) }
        }
        footer?.invoke(this)
    }
}

/** A row kind's name, its list content type. */
private fun kindName(kind: AgentRowKind): String = when (kind) {
    is AgentRowKind.User -> "user"
    is AgentRowKind.Block -> "block"
    is AgentRowKind.Tool -> "tool"
    is AgentRowKind.Detail -> "detail"
    is AgentRowKind.Group -> "group"
    is AgentRowKind.Thought -> "thought"
    is AgentRowKind.Fold -> "fold"
    is AgentRowKind.Changes -> "changes"
    is AgentRowKind.Mark -> "mark"
    is AgentRowKind.Notice -> "notice"
    is AgentRowKind.Live -> "live"
}

/**
 * Space above a row: a mark and a step's detail sit under their step, list items
 * close together, a message's blocks a little apart, everything else 12 dp.
 */
private fun topGap(prev: AgentRow?, row: AgentRow): Dp {
    if (prev == null) return 0.dp
    val kind = row.kind
    val prevKind = prev.kind
    return when {
        kind is AgentRowKind.Mark -> 0.dp
        kind is AgentRowKind.Detail -> 2.dp
        row.depth > 0u && prev.depth > 0u -> 2.dp
        kind is AgentRowKind.Block && prevKind is AgentRowKind.Block && prev.itemId == row.itemId ->
            if (kind.block.kind is AgentBlockKind.ListItem && prevKind.block.kind is AgentBlockKind.ListItem) 3.dp else 10.dp
        else -> 12.dp
    }
}

/** A row moved in by its depth, with one hairline per level: the only nesting mark. */
@Composable
private fun NestedRow(depth: Int, gap: Dp, content: @Composable () -> Unit) {
    val line = MaterialTheme.colorScheme.outlineVariant
    val levels = depth.coerceIn(0, 8)
    Box(
        Modifier
            .fillMaxWidth()
            .then(
                if (levels > 0) {
                    Modifier.drawBehind {
                        val stroke = 1.dp.toPx()
                        for (level in 0 until levels) {
                            val x = (GUTTER / 2).toPx() + level * NEST.toPx()
                            drawLine(line, Offset(x, 0f), Offset(x, size.height), stroke)
                        }
                    }
                } else {
                    Modifier
                },
            )
            .padding(start = NEST * levels, top = gap),
    ) {
        content()
    }
}

@Composable
private fun AgentRowView(row: AgentRow, context: RowsContext, actions: RowActions) {
    when (val kind = row.kind) {
        is AgentRowKind.User -> UserRow(kind, context.items[row.itemId])
        is AgentRowKind.Block -> BlockRow(row, kind, context.items[row.itemId], actions)
        is AgentRowKind.Tool -> ToolRow(row, kind, context.items[row.itemId], actions)
        is AgentRowKind.Detail -> DetailRow(row, kind, context, actions)
        is AgentRowKind.Group -> GroupRow(row, kind, actions)
        is AgentRowKind.Thought -> ThoughtRow(row, kind, actions)
        is AgentRowKind.Fold -> FoldRow(row, kind, actions)
        is AgentRowKind.Changes -> ChangesRow(row, kind, actions)
        is AgentRowKind.Mark -> MarkRow(kind, actions)
        is AgentRowKind.Notice -> NoticeRow(kind.text)
        is AgentRowKind.Live -> LiveRow(kind)
    }
}

// ---- Your messages ----

@Composable
private fun UserRow(kind: AgentRowKind.User, item: AgentItem?) {
    val scheme = MaterialTheme.colorScheme
    Column(Modifier.fillMaxWidth().padding(start = 48.dp), horizontalAlignment = Alignment.End) {
        if (kind.denyOf) {
            // The wire doesn't say which device sent it, so it's said without a name.
            Text(
                stringResource(R.string.agents_you_denied_said),
                style = MaterialTheme.typography.labelMedium,
                color = scheme.onSurfaceVariant,
                modifier = Modifier.padding(bottom = 4.dp),
            )
        }
        SelectionContainer {
            Text(
                kind.text,
                style = MaterialTheme.typography.bodyLarge,
                modifier = Modifier
                    .clip(RoundedCornerShape(12.dp))
                    .background(scheme.surfaceContainerHigh)
                    .padding(horizontal = 14.dp, vertical = 10.dp),
            )
        }
        if (item?.truncated == true) {
            Text(
                stringResource(R.string.agents_truncated),
                style = MaterialTheme.typography.labelSmall,
                color = scheme.onSurfaceVariant,
                modifier = Modifier.padding(top = 4.dp),
            )
        }
    }
}

// ---- What the agent said: Markdown blocks ----

/** Agent text's look: the body size of the design (14.5 on 1.5). */
@Composable
private fun bodyStyle(): TextStyle = MaterialTheme.typography.bodyLarge.copy(lineHeight = 22.sp)

@Composable
private fun textLook(): AgentText.Look {
    val scheme = MaterialTheme.colorScheme
    val fonts = LocalAppFonts.current
    return remember(scheme.primary, scheme.surfaceContainerHigh, fonts) {
        AgentText.Look(mono = fonts.mono, link = scheme.primary, codeBackground = scheme.surfaceContainerHigh)
    }
}

/** The image glyph an image's alt text follows: the image itself is never loaded. */
@Composable
private fun imageGlyph(): Map<String, InlineTextContent> {
    val tint = MaterialTheme.colorScheme.primary
    return remember(tint) {
        mapOf(
            AgentText.IMAGE_GLYPH to InlineTextContent(Placeholder(1.1.em, 1.em, PlaceholderVerticalAlign.TextCenter)) {
                Icon(G7Icons.photo, contentDescription = null, tint = tint, modifier = Modifier.size(14.dp))
            },
        )
    }
}

/** Styled text of a block or cell, with its links and image glyphs. */
@Composable
private fun StyledText(
    text: String,
    spans: List<AgentSpan>,
    style: TextStyle,
    actions: RowActions,
    modifier: Modifier = Modifier,
    color: Color = Color.Unspecified,
    softWrap: Boolean = true,
    textAlign: TextAlign? = null,
) {
    val look = textLook()
    val shown = remember(text, spans, look, actions) {
        AgentText.styled(text, spans, look) { span ->
            val start = span.start.toInt().coerceIn(0, text.length)
            val end = (span.start.toLong() + span.len.toLong()).toInt().coerceIn(start, text.length)
            actions.link(span, text.substring(start, end))
        }
    }
    Text(
        shown,
        style = style,
        color = color,
        softWrap = softWrap,
        textAlign = textAlign,
        inlineContent = if (spans.any { it.style.image }) imageGlyph() else emptyMap(),
        modifier = modifier,
    )
}

@Composable
private fun BlockRow(row: AgentRow, kind: AgentRowKind.Block, item: AgentItem?, actions: RowActions) {
    val block = kind.block
    Column(Modifier.fillMaxWidth()) {
        Quoted(block.quote.toInt()) {
            when (val blockKind = block.kind) {
                is AgentBlockKind.Paragraph -> SelectionContainer {
                    StyledText(block.text, block.spans, bodyStyle(), actions, Modifier.fillMaxWidth())
                }
                is AgentBlockKind.Heading -> SelectionContainer {
                    val big = blockKind.level.toInt() <= 2
                    StyledText(
                        block.text,
                        block.spans,
                        MaterialTheme.typography.titleMedium.copy(
                            fontSize = if (big) 16.5.sp else 14.5.sp,
                            fontWeight = FontWeight.SemiBold,
                            lineHeight = if (big) 22.sp else 20.sp,
                        ),
                        actions,
                        Modifier.fillMaxWidth().padding(top = 4.dp),
                    )
                }
                is AgentBlockKind.ListItem -> ListItemBlock(block, actions)
                is AgentBlockKind.Code -> CodeBlock(row.key, block, actions)
                is AgentBlockKind.Table -> block.table?.let { TableBlock(it, actions) }
                is AgentBlockKind.Rule -> Box(
                    Modifier.fillMaxWidth().padding(vertical = 6.dp).height(1.dp).background(MaterialTheme.colorScheme.outlineVariant),
                )
                is AgentBlockKind.Alert -> AlertBlock(blockKind.alert, block, actions)
            }
        }
        if (kind.last && item?.truncated == true) {
            Row(
                modifier = Modifier.padding(top = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                Icon(G7Icons.info, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.size(13.dp))
                Text(stringResource(R.string.agents_cut_short), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
}

/** A block inside [levels] block quotes: one bar per level. */
@Composable
private fun Quoted(levels: Int, content: @Composable () -> Unit) {
    if (levels <= 0) {
        content()
        return
    }
    val bar = MaterialTheme.colorScheme.outline
    Row(Modifier.fillMaxWidth().height(IntrinsicSize.Min)) {
        repeat(levels.coerceAtMost(4)) {
            Box(Modifier.width(2.dp).fillMaxHeight().background(bar))
            Spacer(Modifier.width(10.dp))
        }
        Box(Modifier.weight(1f)) { content() }
    }
}

@Composable
private fun ListItemBlock(block: AgentBlock, actions: RowActions) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val style = bodyStyle()
    Row(Modifier.fillMaxWidth().padding(start = 16.dp * block.depth.toInt().coerceAtMost(8)), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Box(Modifier.width(18.dp).height(22.dp), contentAlignment = Alignment.Center) {
            when (val marker = block.marker) {
                is AgentMarker.Bullet -> Box(Modifier.size(4.dp).clip(CircleShape).background(scheme.onSurfaceVariant))
                is AgentMarker.Number -> Text("${marker.number}.", style = style, color = scheme.onSurfaceVariant, maxLines = 1)
                is AgentMarker.Todo -> Box(Modifier.size(10.dp).border(1.5.dp, scheme.outline, CircleShape))
                is AgentMarker.Done -> Icon(G7Icons.check, contentDescription = null, tint = tints.teal, modifier = Modifier.size(14.dp))
                is AgentMarker.None -> Unit
            }
        }
        SelectionContainer(Modifier.weight(1f)) {
            StyledText(block.text, block.spans, style, actions)
        }
    }
}

@Composable
private fun AlertBlock(alert: AgentAlertKind, block: AgentBlock, actions: RowActions) {
    val tints = g7Tints()
    val (tint, icon, title) = when (alert) {
        AgentAlertKind.NOTE -> Triple(tints.blue, G7Icons.info, R.string.agents_alert_note)
        AgentAlertKind.TIP -> Triple(tints.teal, G7Icons.lightbulb, R.string.agents_alert_tip)
        AgentAlertKind.IMPORTANT -> Triple(tints.pink, G7Icons.info, R.string.agents_alert_important)
        AgentAlertKind.WARNING -> Triple(tints.amber, AgentGlyphs.warning, R.string.agents_alert_warning)
        AgentAlertKind.CAUTION -> Triple(tints.red, AgentGlyphs.warning, R.string.agents_alert_caution)
    }
    Row(Modifier.fillMaxWidth().height(IntrinsicSize.Min)) {
        Box(Modifier.width(2.dp).fillMaxHeight().background(tint))
        Column(Modifier.weight(1f).padding(start = 10.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Icon(icon, contentDescription = null, tint = tint, modifier = Modifier.size(14.dp))
                Text(stringResource(title), style = MaterialTheme.typography.labelLarge, color = tint)
            }
            if (block.text.isNotEmpty()) {
                SelectionContainer { StyledText(block.text, block.spans, bodyStyle(), actions) }
            }
        }
    }
}

/**
 * A code block: its language word, Wrap and Copy (the source as written), and
 * its lines coloured by the render crate's token classes; scrolled sideways
 * unless wrapped. A language we have no colours for is shown plain.
 */
@Composable
private fun CodeBlock(key: String, block: AgentBlock, actions: RowActions) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val fonts = LocalAppFonts.current
    var wrap by rememberSaveable(key) { mutableStateOf(false) }
    val plain = scheme.onSurface
    val muted = scheme.onSurfaceVariant
    val shown = remember(block.lines, block.text, tints, plain, muted) {
        if (block.lines.isEmpty()) {
            AnnotatedString(block.text.trimEnd('\n'))
        } else {
            buildAnnotatedString {
                block.lines.forEachIndexed { index, line ->
                    if (index > 0) append('\n')
                    append(AgentText.codeLine(line) { AgentText.tokColor(it, tints, plain, muted) })
                }
            }
        }
    }
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(scheme.surfaceContainer),
    ) {
        Row(
            modifier = Modifier.fillMaxWidth().height(40.dp).padding(start = 12.dp, end = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(block.lang, style = MaterialTheme.typography.labelSmall, color = scheme.onSurfaceVariant, maxLines = 1, modifier = Modifier.weight(1f))
            SmallTextButton(
                label = stringResource(R.string.agents_wrap),
                selected = wrap,
                onClick = { wrap = !wrap },
            )
            CopyButton { actions.copy(block.text) }
        }
        val codeStyle = MaterialTheme.typography.bodySmall.mono(fonts).copy(fontSize = 12.5.sp, lineHeight = 19.sp)
        SelectionContainer {
            Text(
                shown,
                style = codeStyle,
                color = plain,
                softWrap = wrap,
                modifier = Modifier
                    .fillMaxWidth()
                    .then(if (wrap) Modifier else Modifier.horizontalScroll(rememberScrollState()))
                    .padding(start = 12.dp, end = 12.dp, bottom = 10.dp),
            )
        }
    }
}

/** A small text button in a code block's or a card's header. */
@Composable
internal fun SmallTextButton(label: String, onClick: () -> Unit, selected: Boolean = false, icon: ImageVector? = null) {
    val scheme = MaterialTheme.colorScheme
    val color = if (selected) scheme.primary else scheme.onSurfaceVariant
    Row(
        modifier = Modifier
            .heightIn(min = 36.dp)
            .clip(RoundedCornerShape(6.dp))
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        if (icon != null) Icon(icon, contentDescription = null, tint = color, modifier = Modifier.size(15.dp))
        Text(label, style = MaterialTheme.typography.labelMedium, color = color, maxLines = 1)
    }
}

/** Copy, then "Copied" for a moment. */
@Composable
internal fun CopyButton(onCopy: () -> Unit) {
    var copied by remember { mutableStateOf(false) }
    LaunchedEffect(copied) {
        if (copied) {
            delay(COPIED_MS)
            copied = false
        }
    }
    SmallTextButton(
        label = stringResource(if (copied) R.string.agents_copied else R.string.agents_copy),
        icon = AgentGlyphs.copy,
        onClick = {
            onCopy()
            copied = true
        },
    )
}

/**
 * A table in a grid that scrolls sideways, its header first; rows past the
 * render crate's bound counted under it ("12 more rows").
 */
@Composable
private fun TableBlock(table: AgentTable, actions: RowActions) {
    val scheme = MaterialTheme.colorScheme
    val columns = table.columns.toInt().coerceAtLeast(1)
    val rowCount = (table.cells.size + columns - 1) / columns
    val line = scheme.outlineVariant
    Column(Modifier.fillMaxWidth()) {
        Row(Modifier.horizontalScroll(rememberScrollState())) {
            for (column in 0 until columns) {
                val align = when (table.align.getOrNull(column)) {
                    AgentAlign.RIGHT -> TextAlign.End
                    AgentAlign.CENTER -> TextAlign.Center
                    AgentAlign.LEFT, AgentAlign.NONE, null -> TextAlign.Start
                }
                Column(Modifier.width(IntrinsicSize.Max)) {
                    for (r in 0 until rowCount) {
                        val cell = table.cells.getOrNull(r * columns + column)
                        val header = cell?.header == true
                        Box(
                            Modifier
                                .fillMaxWidth()
                                .drawBehind {
                                    val y = size.height - 0.5.dp.toPx()
                                    drawLine(line, Offset(0f, y), Offset(size.width, y), 1.dp.toPx())
                                }
                                .padding(horizontal = 10.dp, vertical = 6.dp),
                        ) {
                            if (cell != null) {
                                StyledText(
                                    // A cell is one line: a hard break inside it reads as a space.
                                    cell.text.replace('\n', ' '),
                                    cell.spans,
                                    if (header) MaterialTheme.typography.labelMedium else MaterialTheme.typography.bodyMedium,
                                    actions,
                                    Modifier.fillMaxWidth(),
                                    color = if (header) scheme.onSurfaceVariant else Color.Unspecified,
                                    softWrap = false,
                                    textAlign = align,
                                )
                            } else {
                                Text("", style = MaterialTheme.typography.bodyMedium)
                            }
                        }
                    }
                }
            }
        }
        if (table.moreRows > 0u) {
            val more = table.moreRows.toInt()
            Text(
                pluralStringResource(R.plurals.agents_table_more, more, more),
                style = MaterialTheme.typography.bodySmall,
                color = scheme.onSurfaceVariant,
                modifier = Modifier.padding(top = 6.dp),
            )
        }
    }
}

// ---- What the agent did: lines ----

/** A line's gutter: the disclosure when it opens, then its glyph. */
@Composable
private fun Gutter(open: Boolean?, icon: ImageVector?, tint: Color) {
    val muted = MaterialTheme.colorScheme.onSurfaceVariant
    Box(Modifier.width(GUTTER), contentAlignment = Alignment.Center) {
        if (open != null) {
            Icon(if (open) G7Icons.chevronDown else G7Icons.chevronRight, contentDescription = null, tint = muted, modifier = Modifier.size(12.dp))
        }
    }
    Spacer(Modifier.width(4.dp))
    if (icon != null) Icon(icon, contentDescription = null, tint = tint, modifier = Modifier.size(GLYPH)) else Spacer(Modifier.width(GLYPH))
    Spacer(Modifier.width(8.dp))
}

/** The glyph of a kind of step (the desktop's choices, from G7Glyphs). */
internal fun classIcon(cls: AgentToolClass?): ImageVector = when (cls) {
    AgentToolClass.COMMAND -> G7Icons.terminal
    AgentToolClass.EDIT -> G7Icons.edit
    AgentToolClass.READ -> G7Icons.text
    AgentToolClass.SEARCH -> G7Icons.search
    AgentToolClass.WEB -> G7Icons.globe
    AgentToolClass.AGENT -> G7Icons.sparkle
    AgentToolClass.TASKS -> G7Icons.clipboardList
    AgentToolClass.MCP, AgentToolClass.OTHER, null -> G7Icons.apps
}

/** The template a step's line is said with, by its kind and state (contract §P9, reconcile file); null: its name alone. */
private fun toolTemplate(cls: AgentToolClass, status: AgentRowStatus): Int? = when (cls) {
    AgentToolClass.COMMAND -> when (status) {
        AgentRowStatus.RUNNING -> R.string.agents_tool_running
        AgentRowStatus.OK -> R.string.agents_tool_ran
        AgentRowStatus.FAILED -> R.string.agents_tool_failed
        AgentRowStatus.DENIED -> R.string.agents_tool_denied
        AgentRowStatus.STOPPED -> R.string.agents_tool_stopped
        AgentRowStatus.ASKING -> R.string.agents_tool_wants_run
    }
    AgentToolClass.EDIT -> when (status) {
        AgentRowStatus.RUNNING -> R.string.agents_tool_editing
        AgentRowStatus.OK -> R.string.agents_tool_edited
        AgentRowStatus.FAILED -> R.string.agents_tool_edit_failed
        AgentRowStatus.DENIED -> R.string.agents_tool_edit_denied
        AgentRowStatus.STOPPED -> R.string.agents_tool_edit_stopped
        AgentRowStatus.ASKING -> R.string.agents_tool_wants_edit
    }
    AgentToolClass.READ -> when (status) {
        AgentRowStatus.RUNNING -> R.string.agents_tool_reading
        AgentRowStatus.OK -> R.string.agents_tool_read
        AgentRowStatus.FAILED -> R.string.agents_tool_read_failed
        AgentRowStatus.DENIED -> R.string.agents_tool_denied
        AgentRowStatus.STOPPED -> R.string.agents_tool_read_stopped
        AgentRowStatus.ASKING -> R.string.agents_tool_wants_read
    }
    AgentToolClass.SEARCH -> when (status) {
        AgentRowStatus.RUNNING -> R.string.agents_tool_searching
        AgentRowStatus.OK -> R.string.agents_tool_searched
        AgentRowStatus.FAILED -> R.string.agents_tool_search_failed
        AgentRowStatus.DENIED -> R.string.agents_tool_denied
        AgentRowStatus.STOPPED -> R.string.agents_tool_search_stopped
        AgentRowStatus.ASKING -> R.string.agents_tool_wants_use
    }
    AgentToolClass.WEB -> when (status) {
        AgentRowStatus.RUNNING -> R.string.agents_tool_web_searching
        AgentRowStatus.OK -> R.string.agents_tool_web_searched
        AgentRowStatus.FAILED -> R.string.agents_tool_web_failed
        AgentRowStatus.DENIED -> R.string.agents_tool_denied
        AgentRowStatus.STOPPED -> R.string.agents_tool_stopped
        AgentRowStatus.ASKING -> R.string.agents_tool_wants_use
    }
    AgentToolClass.MCP, AgentToolClass.AGENT, AgentToolClass.TASKS, AgentToolClass.OTHER ->
        if (status == AgentRowStatus.ASKING) R.string.agents_tool_wants_use else null
}

/** A step's line in words: "Ran cargo build", the command or file in the code face. */
internal fun toolLabelText(resources: Resources, label: AgentToolLabel, status: AgentRowStatus, monoStyle: SpanStyle): AnnotatedString {
    val code = when (label.toolClass) {
        AgentToolClass.COMMAND, AgentToolClass.EDIT, AgentToolClass.READ, AgentToolClass.SEARCH, AgentToolClass.WEB -> true
        AgentToolClass.MCP, AgentToolClass.AGENT, AgentToolClass.TASKS, AgentToolClass.OTHER -> false
    }
    val target = if (code) AnnotatedString(label.target, monoStyle) else AnnotatedString(label.target)
    val more = label.more.toInt()
    val subject = if (more > 0) {
        AgentText.format(resources.getQuantityString(R.plurals.agents_tool_and_more, more), target, AnnotatedString(more.toString()))
    } else {
        target
    }
    val template = toolTemplate(label.toolClass, status) ?: return subject
    return AgentText.format(resources.getString(template), subject)
}

/** A clause of a group's summary: "Ran 2 commands", or its second form ("ran 2 commands"). */
private fun clauseText(resources: Resources, clause: AgentClause, second: Boolean): String {
    val id = when (clause.toolClass) {
        AgentToolClass.COMMAND -> if (second) R.plurals.agents_clause_commands_lower else R.plurals.agents_clause_commands
        AgentToolClass.EDIT -> if (second) R.plurals.agents_clause_changed_lower else R.plurals.agents_clause_changed
        AgentToolClass.READ -> if (second) R.plurals.agents_clause_read_lower else R.plurals.agents_clause_read
        AgentToolClass.SEARCH -> if (second) R.plurals.agents_clause_searched_lower else R.plurals.agents_clause_searched
        AgentToolClass.WEB -> if (second) R.plurals.agents_clause_web_lower else R.plurals.agents_clause_web
        AgentToolClass.MCP, AgentToolClass.AGENT, AgentToolClass.TASKS, AgentToolClass.OTHER ->
            if (second) R.plurals.agents_clause_tools_lower else R.plurals.agents_clause_tools
    }
    val count = clause.count.toInt()
    return resources.getQuantityString(id, count, count)
}

/** A group's line: one clause, two ("Ran 2 commands and changed 1 file"), or two and the rest counted. */
internal fun summaryText(resources: Resources, summary: AgentSummary): String {
    val first = clauseText(resources, summary.first, second = false)
    val next = summary.second ?: return first
    val second = clauseText(resources, next, second = true)
    val others = summary.others.toInt()
    return if (others > 0) {
        resources.getQuantityString(R.plurals.agents_summary_others, others, first, second, others)
    } else {
        resources.getString(R.string.agents_summary_two, first, second)
    }
}

/** A time as the fold says it: "12s", "2m 14s", "1h 03m". */
internal fun durationText(resources: Resources, ms: Long): String {
    val worked = AgentText.worked(ms)
    return when {
        worked.hours > 0L -> resources.getString(R.string.agents_dur_hours, worked.hours, worked.minutes)
        worked.minutes > 0L -> resources.getString(R.string.agents_dur_minutes, worked.minutes, worked.seconds)
        else -> resources.getString(R.string.agents_dur_seconds, worked.seconds)
    }
}

/** "+11 −2" in the diff's colours. */
@Composable
internal fun Counts(adds: Long, dels: Long, style: TextStyle = MaterialTheme.typography.labelMedium) {
    val tints = g7Tints()
    val fonts = LocalAppFonts.current
    Text(
        buildAnnotatedString {
            withStyle(SpanStyle(color = tints.green)) { append("+$adds") }
            append(' ')
            withStyle(SpanStyle(color = tints.red)) { append("−$dels") }
        },
        style = style.mono(fonts),
        maxLines = 1,
    )
}

/** A step's line. Long-press offers Copy of what it ran or touched. */
@Composable
private fun ToolRow(row: AgentRow, kind: AgentRowKind.Tool, item: AgentItem?, actions: RowActions) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val resources = LocalResources.current
    val fonts = LocalAppFonts.current
    val openable = kind.hasDetail || kind.hasOutput
    var menu by remember { mutableStateOf(false) }
    val label = remember(kind.label, kind.status, resources, fonts) {
        toolLabelText(resources, kind.label, kind.status, SpanStyle(fontFamily = fonts.mono, fontFeatureSettings = MONO_FEATURES))
    }
    val tint = when (kind.status) {
        AgentRowStatus.FAILED -> tints.red
        AgentRowStatus.RUNNING, AgentRowStatus.OK, AgentRowStatus.DENIED, AgentRowStatus.STOPPED, AgentRowStatus.ASKING -> scheme.onSurfaceVariant
    }
    val source = item?.tool?.let { tool -> tool.detail?.takeIf { it.isNotBlank() } ?: tool.path ?: tool.title } ?: kind.label.target
    Box {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = 34.dp)
                .clip(RoundedCornerShape(6.dp))
                .combinedClickable(
                    role = Role.Button,
                    onClick = { if (openable) actions.toggle(row.key) },
                    onLongClick = { menu = true },
                ),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Gutter(open = if (openable) kind.open else null, icon = classIcon(kind.label.toolClass), tint = tint)
            val style = MaterialTheme.typography.bodyMedium
            val color = if (kind.status == AgentRowStatus.FAILED) scheme.onSurface else scheme.onSurfaceVariant
            if (kind.status == AgentRowStatus.RUNNING) {
                BandText(label, style, color, Modifier.weight(1f))
            } else {
                Text(label, style = style, color = color, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            }
            val edit = kind.label.toolClass == AgentToolClass.EDIT && (kind.adds > 0u || kind.dels > 0u)
            if (edit) {
                Spacer(Modifier.width(10.dp))
                Counts(kind.adds.toLong(), kind.dels.toLong())
            } else if (kind.label.toolClass == AgentToolClass.COMMAND && kind.ms >= 1_000L) {
                // A command's time once it ran a second or more (reconcile file, item 3).
                Spacer(Modifier.width(10.dp))
                Text(durationText(resources, kind.ms), style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant, maxLines = 1)
            }
        }
        CopyMenu(expanded = menu, onDismiss = { menu = false }) { actions.copy(source) }
    }
}

/** A one-item menu: Copy. */
@Composable
private fun CopyMenu(expanded: Boolean, onDismiss: () -> Unit, onCopy: () -> Unit) {
    DropdownMenu(expanded = expanded, onDismissRequest = onDismiss) {
        DropdownMenuItem(
            text = { Text(stringResource(R.string.agents_copy)) },
            leadingIcon = { Icon(AgentGlyphs.copy, contentDescription = null, modifier = Modifier.size(16.dp)) },
            onClick = {
                onDismiss()
                onCopy()
            },
        )
    }
}

/** Under an open step: its whole command with the folder, its edit, or the end of its output. */
@Composable
private fun DetailRow(row: AgentRow, kind: AgentRowKind.Detail, context: RowsContext, actions: RowActions) {
    val scheme = MaterialTheme.colorScheme
    val fonts = LocalAppFonts.current
    val card = Modifier
        .fillMaxWidth()
        .padding(start = LABEL_X)
        .clip(RoundedCornerShape(8.dp))
        .background(scheme.surfaceContainer)
    when (val of = kind.of) {
        is AgentDetailOf.Command -> Column(card) {
            Row(
                modifier = Modifier.fillMaxWidth().padding(start = 10.dp, end = 4.dp, top = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                FolderLabel(context.folder, Modifier.weight(1f))
                CopyButton { actions.copy(of.text) }
            }
            SelectionContainer {
                Text(
                    of.text,
                    style = MaterialTheme.typography.bodySmall.mono(fonts).copy(fontSize = 12.5.sp, lineHeight = 19.sp),
                    modifier = Modifier.fillMaxWidth().padding(start = 10.dp, end = 10.dp, bottom = 10.dp),
                )
            }
        }
        is AgentDetailOf.Diff -> {
            val path = context.items[row.itemId]?.tool?.path
            val heading = editHeading(of.edit.toInt(), of.edits.toInt(), path)
            Column(card.clickable(role = Role.Button) { actions.diff(heading.orEmpty(), of.doc) }) {
                if (heading != null) {
                    Text(
                        heading,
                        style = MaterialTheme.typography.labelSmall,
                        color = scheme.onSurfaceVariant,
                        modifier = Modifier.padding(start = 10.dp, end = 10.dp, top = 8.dp),
                    )
                }
                DiffView(of.doc, wrap = true, maxRows = INLINE_DIFF_ROWS, modifier = Modifier.padding(vertical = 4.dp))
                if (diffRows(of.doc) > INLINE_DIFF_ROWS || of.doc.more) {
                    SmallTextButton(stringResource(R.string.agents_show_all), onClick = { actions.diff(heading.orEmpty(), of.doc) })
                }
            }
        }
        is AgentDetailOf.Output -> {
            val (tail, cutHere) = remember(of.text) { AgentText.lastLines(of.text.trimEnd('\n'), INLINE_OUTPUT_LINES) }
            Column(card.clickable(role = Role.Button) { actions.output(row.itemId) }) {
                if (of.cut || cutHere) {
                    Text(
                        stringResource(R.string.agents_output_last_lines),
                        style = MaterialTheme.typography.labelSmall,
                        color = scheme.onSurfaceVariant,
                        modifier = Modifier.padding(start = 10.dp, end = 10.dp, top = 8.dp),
                    )
                }
                Text(
                    tail,
                    style = MaterialTheme.typography.bodySmall.mono(fonts).copy(fontSize = 12.sp, lineHeight = 18.sp),
                    color = scheme.onSurfaceVariant,
                    modifier = Modifier.fillMaxWidth().padding(10.dp),
                )
                if (cutHere) SmallTextButton(stringResource(R.string.agents_show_all), onClick = { actions.output(row.itemId) })
            }
        }
    }
}

/** "Edit 1 of 3 · hold.rs" over a file's edits; the file's name alone over one; nothing over a step's own. */
@Composable
private fun editHeading(edit: Int, edits: Int, path: String?): String? {
    val name = path?.takeIf { it.isNotBlank() }?.let { AgentText.splitPath(it).second }
    val of = if (edits > 1) stringResource(R.string.agents_edit_of, edit, edits) else null
    return listOfNotNull(of, if (edits > 0) name else null).joinToString(" · ").ifEmpty { null }
}

/** The folder's label with its glyph: never a path. */
@Composable
internal fun FolderLabel(folder: String, modifier: Modifier = Modifier) {
    val muted = MaterialTheme.colorScheme.onSurfaceVariant
    Row(modifier, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Icon(G7Icons.folder, contentDescription = null, tint = muted, modifier = Modifier.size(12.dp))
        Text(folder, style = MaterialTheme.typography.labelSmall, color = muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** How many rows a diff has in all. */
internal fun diffRows(doc: AgentDiffDoc): Int = doc.files.sumOf { it.rows.size }

/**
 * A diff: hunk headers, then lines on the add and remove colours with their
 * changed words marked; line numbers only when the diff has them (reconcile
 * file, item 5). [wrap]: long lines wrap (the phone's rule); else it scrolls
 * sideways. At most [maxRows] rows.
 */
@Composable
internal fun DiffView(doc: AgentDiffDoc, wrap: Boolean, modifier: Modifier = Modifier, maxRows: Int = Int.MAX_VALUE) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val fonts = LocalAppFonts.current
    val style = MaterialTheme.typography.bodySmall.mono(fonts).copy(fontSize = 12.5.sp, lineHeight = 20.sp)
    val addBg = tints.green.copy(alpha = 0.12f)
    val delBg = tints.red.copy(alpha = 0.14f)
    val addMark = tints.green.copy(alpha = 0.34f)
    val delMark = tints.red.copy(alpha = 0.36f)
    var left = maxRows
    Column(modifier.fillMaxWidth().then(if (wrap) Modifier else Modifier.horizontalScroll(rememberScrollState()))) {
        for (file in doc.files) {
            if (left <= 0) break
            if (doc.files.size > 1 && file.path.isNotEmpty()) {
                Text(
                    file.path,
                    style = style,
                    color = scheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.StartEllipsis,
                    modifier = Modifier.padding(horizontal = 10.dp, vertical = 4.dp),
                )
            }
            val numbers = file.rows.any { it.oldNo > 0u || it.newNo > 0u }
            for (row in file.rows) {
                if (left-- <= 0) break
                if (row.kind == AgentDiffKind.HUNK) {
                    Text(
                        row.text,
                        style = style.copy(fontSize = 11.5.sp),
                        color = scheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.fillMaxWidth().background(scheme.surfaceContainerHigh).padding(horizontal = 10.dp, vertical = 4.dp),
                    )
                    continue
                }
                val (bg, mark, sign, signColor) = when (row.kind) {
                    AgentDiffKind.ADD -> DiffLook(addBg, addMark, "+", tints.green)
                    AgentDiffKind.DEL -> DiffLook(delBg, delMark, "−", tints.red)
                    AgentDiffKind.CONTEXT, AgentDiffKind.HUNK -> DiffLook(Color.Transparent, Color.Transparent, "", scheme.outline)
                }
                val text = remember(row.text, row.marks, mark) {
                    buildAnnotatedString {
                        append(row.text)
                        for (m in row.marks) {
                            val start = m.start.toInt().coerceIn(0, row.text.length)
                            val end = (m.start.toLong() + m.len.toLong()).toInt().coerceIn(start, row.text.length)
                            if (end > start) addStyle(SpanStyle(background = mark), start, end)
                        }
                    }
                }
                Row(Modifier.fillMaxWidth().background(bg)) {
                    if (numbers) {
                        LineNumber(row.oldNo.toInt(), style)
                        LineNumber(row.newNo.toInt(), style)
                    }
                    Text(sign, style = style, color = signColor, textAlign = TextAlign.Center, modifier = Modifier.width(18.dp))
                    Text(text, style = style, color = scheme.onSurface, softWrap = wrap, modifier = Modifier.padding(end = 10.dp))
                }
            }
        }
    }
}

private data class DiffLook(val background: Color, val mark: Color, val sign: String, val signColor: Color)

@Composable
private fun LineNumber(number: Int, style: TextStyle) {
    Text(
        if (number > 0) number.toString() else "",
        style = style,
        color = MaterialTheme.colorScheme.outline,
        textAlign = TextAlign.End,
        maxLines = 1,
        modifier = Modifier.width(30.dp).padding(end = 4.dp),
    )
}

/** A run of finished steps in one line: what they did, the first clause's glyph, red when one failed. */
@Composable
private fun GroupRow(row: AgentRow, kind: AgentRowKind.Group, actions: RowActions) {
    val scheme = MaterialTheme.colorScheme
    val resources = LocalResources.current
    val text = remember(kind.summary, resources) { summaryText(resources, kind.summary) }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 34.dp)
            .clip(RoundedCornerShape(6.dp))
            .clickable(role = Role.Button) { actions.toggle(row.key) },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Gutter(open = kind.open, icon = classIcon(kind.summary.first.toolClass), tint = if (kind.failed) g7Tints().red else scheme.onSurfaceVariant)
        Text(text, style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** What the agent thought: its first line, or "Thought" when it showed none. */
@Composable
private fun ThoughtRow(row: AgentRow, kind: AgentRowKind.Thought, actions: RowActions) {
    val scheme = MaterialTheme.colorScheme
    val count = kind.count.toInt()
    val text = when {
        kind.preview.isNotBlank() -> kind.preview
        count > 1 -> pluralStringResource(R.plurals.agents_thought_times, count, count)
        else -> stringResource(R.string.agents_thought)
    }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 34.dp)
            .clip(RoundedCornerShape(6.dp))
            .clickable(enabled = kind.hasText, role = Role.Button) { actions.toggle(row.key) },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Gutter(open = if (kind.hasText) kind.open else null, icon = G7Icons.lightbulb, tint = scheme.onSurfaceVariant)
        Text(text, style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant, maxLines = 2, overflow = TextOverflow.Ellipsis)
    }
}

/** A finished turn's fold: "Worked for 2m 14s" and a hairline to the edge. */
@Composable
private fun FoldRow(row: AgentRow, kind: AgentRowKind.Fold, actions: RowActions) {
    val scheme = MaterialTheme.colorScheme
    val resources = LocalResources.current
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 36.dp)
            .clip(RoundedCornerShape(6.dp))
            .clickable(role = Role.Button) { actions.toggle(row.key) },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(GUTTER), contentAlignment = Alignment.Center) {
            Icon(if (kind.open) G7Icons.chevronDown else G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
        }
        Text(
            stringResource(R.string.agents_worked, durationText(resources, kind.workedMs)),
            style = MaterialTheme.typography.labelLarge,
            color = scheme.onSurfaceVariant,
            maxLines = 1,
            modifier = Modifier.padding(start = 4.dp),
        )
        Box(Modifier.weight(1f).padding(start = 10.dp).height(1.dp).background(scheme.outlineVariant))
    }
}

/** The files a turn changed, each with its counts; a file opens the turn's edits to it below. */
@Composable
private fun ChangesRow(row: AgentRow, kind: AgentRowKind.Changes, actions: RowActions) {
    val scheme = MaterialTheme.colorScheme
    val fonts = LocalAppFonts.current
    val total = kind.files.size + kind.more.toInt()
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(scheme.surfaceContainer)
            .padding(4.dp),
    ) {
        Row(
            modifier = Modifier.fillMaxWidth().height(40.dp).padding(horizontal = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Icon(G7Icons.edit, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(14.dp))
            Text(pluralStringResource(R.plurals.agents_files_changed, total, total), style = MaterialTheme.typography.labelLarge)
            Counts(kind.adds.toLong(), kind.dels.toLong())
        }
        kind.files.forEachIndexed { index, file ->
            val (dir, name) = AgentText.splitPath(file.path)
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .height(40.dp)
                    .clip(RoundedCornerShape(6.dp))
                    .background(if (file.open) scheme.surfaceContainerHigh else Color.Transparent)
                    .clickable(role = Role.Button) { actions.toggle("${row.key}:$index") }
                    .padding(horizontal = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Icon(if (file.open) G7Icons.chevronDown else G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
                Text(
                    buildAnnotatedString {
                        withStyle(SpanStyle(color = scheme.onSurfaceVariant)) { append(dir) }
                        append(name)
                    },
                    style = MaterialTheme.typography.bodySmall.mono(fonts).copy(fontSize = 12.5.sp),
                    maxLines = 1,
                    overflow = TextOverflow.StartEllipsis,
                    modifier = Modifier.weight(1f),
                )
                Counts(file.adds.toLong(), file.dels.toLong())
            }
        }
    }
}

/** A request's state under its step: "Waiting for you" in amber (tapping brings the request up), or how it ended. */
@Composable
private fun MarkRow(kind: AgentRowKind.Mark, actions: RowActions) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val by = kind.by.takeIf { it.isNotBlank() }
    val (icon, text) = when (kind.kind) {
        AgentMarkKind.WAITING, AgentMarkKind.HELD -> G7Icons.history to stringResource(R.string.agents_mark_waiting)
        AgentMarkKind.ALLOWED -> G7Icons.check to (by?.let { stringResource(R.string.agents_answered_allowed_on, it) } ?: stringResource(R.string.agents_answered_allowed))
        AgentMarkKind.DENIED -> G7Icons.close to (by?.let { stringResource(R.string.agents_answered_denied_on, it) } ?: stringResource(R.string.agents_answered_denied))
        AgentMarkKind.RULE -> G7Icons.shield to stringResource(R.string.agents_mark_rule)
        AgentMarkKind.EXPIRED -> G7Icons.history to stringResource(R.string.agents_mark_expired)
        AgentMarkKind.CANCELLED -> G7Icons.close to stringResource(R.string.agents_mark_cancelled)
    }
    val waiting = kind.kind == AgentMarkKind.WAITING || kind.kind == AgentMarkKind.HELD
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 28.dp)
            .clip(RoundedCornerShape(6.dp))
            .clickable(enabled = waiting, role = Role.Button, onClick = actions.mark)
            .padding(start = LABEL_X),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Icon(icon, contentDescription = null, tint = if (waiting) tints.amber else scheme.onSurfaceVariant, modifier = Modifier.size(13.dp))
        Text(
            text,
            style = MaterialTheme.typography.labelMedium.copy(fontWeight = if (waiting) FontWeight.SemiBold else FontWeight.Normal),
            color = if (waiting) tints.amber else scheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

@Composable
private fun NoticeRow(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        textAlign = TextAlign.Center,
        modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp),
    )
}

/** The working line, last: "Thinking" in the light band, how long, and the thought so far when there is one. */
@Composable
private fun LiveRow(kind: AgentRowKind.Live) {
    val scheme = MaterialTheme.colorScheme
    val word = when (kind.label) {
        AgentLiveLabel.STARTING -> stringResource(R.string.agents_live_starting)
        AgentLiveLabel.THINKING -> stringResource(R.string.agents_step_thinking)
    }
    Column(Modifier.fillMaxWidth()) {
        Row(Modifier.fillMaxWidth().heightIn(min = 34.dp), verticalAlignment = Alignment.CenterVertically) {
            Gutter(open = null, icon = G7Icons.lightbulb, tint = scheme.onSurfaceVariant)
            BandText(AnnotatedString(word), MaterialTheme.typography.bodyMedium, scheme.onSurfaceVariant, Modifier.weight(1f))
            if (kind.sinceMs > 0L) {
                ElapsedText(kind.sinceMs, style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
            }
        }
        if (kind.thought.isNotBlank()) {
            Text(
                kind.thought,
                style = MaterialTheme.typography.bodySmall.copy(lineHeight = 18.sp),
                color = scheme.onSurfaceVariant,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(start = LABEL_X, bottom = 2.dp),
            )
        }
    }
}

/**
 * Muted text with the light band: a brighter sweep through it every 2.2 s, in
 * 30 steps, drawn without composing again. Still when reduced motion is on and
 * while the screen isn't in front.
 */
@Composable
private fun BandText(text: AnnotatedString, style: TextStyle, color: Color, modifier: Modifier = Modifier) {
    val reduced = LocalReducedMotion.current
    val lifecycle by LocalLifecycleOwner.current.lifecycle.currentStateAsState()
    val moving = !reduced && lifecycle.isAtLeast(Lifecycle.State.RESUMED)
    val shine = MaterialTheme.colorScheme.onSurface
    val textModifier = if (moving) {
        val transition = rememberInfiniteTransition(label = "AgentBand")
        val phase = transition.animateFloat(
            initialValue = 0f,
            targetValue = 1f,
            animationSpec = infiniteRepeatable(tween(BAND_MS, easing = LinearEasing)),
            label = "AgentBandPhase",
        )
        Modifier
            .graphicsLayer { compositingStrategy = CompositingStrategy.Offscreen }
            .drawWithContent {
                drawContent()
                val step = kotlin.math.floor(phase.value * BAND_STEPS) / BAND_STEPS
                val half = size.width * 0.18f + 24.dp.toPx()
                val center = -half + (size.width + 2f * half) * step
                drawRect(
                    brush = Brush.horizontalGradient(listOf(Color.Transparent, shine, Color.Transparent), startX = center - half, endX = center + half),
                    blendMode = BlendMode.SrcAtop,
                )
            }
    } else {
        Modifier
    }
    Text(text, style = style, color = color, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = modifier.then(textModifier))
}

/** Puts [text] on the clipboard; true when Android shows its own confirmation (13 and later). */
internal fun copyToClipboard(context: Context, text: String): Boolean {
    val manager = context.getSystemService(ClipboardManager::class.java) ?: return false
    manager.setPrimaryClip(ClipData.newPlainText("G7 Link", text))
    return Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU
}

/** Copy from a click: [onCopied] says so where Android doesn't (before 13). Stable while the context is. */
@Composable
internal fun rememberCopy(onCopied: () -> Unit): (String) -> Unit {
    val context = LocalContext.current
    val said by rememberUpdatedState(onCopied)
    return remember(context) {
        { text -> if (!copyToClipboard(context, text)) said() }
    }
}

private const val BAND_MS = 2_200
private const val BAND_STEPS = 30f
private const val COPIED_MS = 1_500L

/** More than any row is tall: scrolls to the end of the last one. */
private const val END_SCROLL = 100_000f
