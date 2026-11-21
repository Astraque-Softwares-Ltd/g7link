// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import android.text.format.DateFormat
import androidx.compose.animation.core.Animatable
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalLocale
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.ui.theme.G7SectionLabel
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.mono
import app.g7link.core.AgentAsk
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState
import java.util.Date
import kotlin.math.roundToInt
import kotlinx.coroutines.launch

/** The list's filters (board 16): All shows the sections; the rest one kind, or a shelf. */
enum class ThreadFilter {
    ALL,
    NEEDS_YOU,
    WORKING,
    RECENT,
    ARCHIVED,
    SNOOZED,
    ;

    /** The shelf the filter lists. */
    val shelf: Shelf
        get() = when (this) {
            ARCHIVED -> Shelf.ARCHIVED
            SNOOZED -> Shelf.SNOOZED
            ALL, NEEDS_YOU, WORKING, RECENT -> Shelf.MAIN
        }

    fun matches(thread: AgentThread): Boolean = when (this) {
        ALL -> AgentShelves.shelfOf(thread) == Shelf.MAIN
        NEEDS_YOU -> AgentShelves.shelfOf(thread) == Shelf.MAIN && AgentsModel.needsYou(thread)
        WORKING -> AgentShelves.shelfOf(thread) == Shelf.MAIN && thread.state == AgentThreadState.WORKING && !AgentsModel.needsYou(thread)
        RECENT -> AgentShelves.shelfOf(thread) == Shelf.MAIN && !AgentsModel.running(thread)
        ARCHIVED -> AgentShelves.shelfOf(thread) == Shelf.ARCHIVED
        SNOOZED -> AgentShelves.shelfOf(thread) == Shelf.SNOOZED
    }
}

/** What a list row does when tapped, held, or swiped. */
internal class ThreadRowActions(
    val open: (String) -> Unit,
    val menu: (String) -> Unit,
    /** The start side (archive, or unarchive on the Archived shelf). */
    val archive: (AgentThread) -> Unit,
    /** The end side (pin or unpin). */
    val pin: (AgentThread) -> Unit,
)

/** The filter chips, scrolling sideways at phone width; the counts are of threads that need you and work. */
@Composable
internal fun ShelfChips(selected: ThreadFilter, needsYou: Int, working: Int, onSelect: (ThreadFilter) -> Unit, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    val options = listOf(
        ThreadFilter.ALL to stringResource(R.string.agents_filter_all),
        ThreadFilter.NEEDS_YOU to counted(stringResource(R.string.agents_state_waiting), needsYou),
        ThreadFilter.WORKING to counted(stringResource(R.string.agents_state_working), working),
        ThreadFilter.RECENT to stringResource(R.string.agents_filter_recent),
        ThreadFilter.ARCHIVED to stringResource(R.string.agents_shelf_archived),
        ThreadFilter.SNOOZED to stringResource(R.string.agents_shelf_snoozed),
    )
    Row(
        modifier = modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(top = 4.dp, bottom = 8.dp).selectableGroup(),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        for ((filter, label) in options) {
            val on = filter == selected
            Text(
                label,
                style = MaterialTheme.typography.labelLarge.copy(fontWeight = if (on) FontWeight.SemiBold else FontWeight.Medium),
                color = if (on) scheme.onSurface else scheme.onSurfaceVariant,
                maxLines = 1,
                modifier = Modifier
                    .clip(RoundedCornerShape(8.dp))
                    .background(if (on) scheme.surfaceContainerHigh else scheme.surface)
                    .border(1.dp, if (on) scheme.surfaceContainerHigh else scheme.outlineVariant, RoundedCornerShape(8.dp))
                    .selectable(selected = on, role = Role.RadioButton, onClick = { onSelect(filter) })
                    .heightIn(min = 32.dp)
                    .padding(horizontal = 12.dp, vertical = 7.dp),
            )
        }
    }
}

/** A chip's word and how many it holds, when any. */
@Composable
internal fun counted(word: String, count: Int): String =
    if (count > 0) stringResource(R.string.agents_filter_count, word, count) else word

/**
 * The threads of a filter: with All, the sections in their order (Needs you,
 * Working, Pinned, Recent); otherwise the threads of the filter, the
 * snoozed by when they come back.
 */
internal fun LazyListScope.threadRows(
    agents: AgentsState,
    filter: ThreadFilter,
    now: Long,
    actions: ThreadRowActions,
) {
    if (filter == ThreadFilter.ALL) {
        val sections = AgentShelves.sections(agents.threads)
        section("needs", R.string.agents_state_waiting, sections.needsYou, agents, now, actions)
        section("working", R.string.agents_state_working, sections.working, agents, now, actions)
        section("pinned", R.string.agents_shelf_pinned, sections.pinned, agents, now, actions)
        section("recent", R.string.agents_filter_recent, sections.recent, agents, now, actions)
        return
    }
    val shown = AgentShelves.on(agents.threads, filter.shelf).filter { filter.matches(it) }
    if (shown.isEmpty() && (filter == ThreadFilter.ARCHIVED || filter == ThreadFilter.SNOOZED)) {
        item(key = "shelf_empty", contentType = "setup") {
            Text(
                stringResource(if (filter == ThreadFilter.ARCHIVED) R.string.agents_shelf_archived_empty else R.string.agents_shelf_snoozed_empty),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth().padding(vertical = 24.dp),
            )
        }
    }
    items(shown, key = { it.id }, contentType = { "thread" }) { thread -> ListedThread(thread, agents, now, actions) }
}

private fun LazyListScope.section(
    key: String,
    label: Int,
    threads: List<AgentThread>,
    agents: AgentsState,
    now: Long,
    actions: ThreadRowActions,
) {
    if (threads.isEmpty()) return
    item(key = "section:$key", contentType = "label") { G7SectionLabel(stringResource(label)) }
    items(threads, key = { it.id }, contentType = { "thread" }) { thread -> ListedThread(thread, agents, now, actions) }
}

@Composable
private fun ListedThread(thread: AgentThread, agents: AgentsState, now: Long, actions: ThreadRowActions) {
    val archiveLabel = stringResource(if (thread.archived) R.string.agents_mark_unarchive else R.string.agents_mark_archive)
    val pinLabel = stringResource(if (thread.pinned) R.string.agents_mark_unpin else R.string.agents_mark_pin)
    SwipeRow(
        startLabel = archiveLabel,
        endLabel = pinLabel,
        // Archiving waits while it works or waits (the PC says BUSY then); unarchiving never does.
        startEnabled = thread.archived || !AgentsModel.running(thread),
        onStart = { actions.archive(thread) },
        onEnd = { actions.pin(thread) },
    ) {
        HistoryThreadRow(
            thread = thread,
            agentName = agents.agentName(thread.agent),
            ask = agents.asksFor(thread.id).firstOrNull(),
            questions = maxOf(agents.questionsFor(thread.id).size, thread.asking.toInt()),
            now = now,
            onOpen = { actions.open(thread.id) },
            onMenu = { actions.menu(thread.id) },
        )
    }
}

/**
 * A row that slides sideways (board 16): from the start side it archives,
 * from the end side it pins, once dragged past a threshold; it springs back
 * either way. The same actions are on its long-press sheet and offered to
 * accessibility services.
 */
@Composable
private fun SwipeRow(
    startLabel: String,
    endLabel: String,
    startEnabled: Boolean,
    onStart: () -> Unit,
    onEnd: () -> Unit,
    content: @Composable () -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    val scope = rememberCoroutineScope()
    val density = LocalDensity.current
    val threshold = with(density) { SWIPE_THRESHOLD.toPx() }
    val reach = with(density) { SWIPE_REACH.toPx() }
    val shift = remember { Animatable(0f) }
    val start by rememberUpdatedState(onStart)
    val end by rememberUpdatedState(onEnd)
    val canStart by rememberUpdatedState(startEnabled)
    Box(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .semantics {
                customActions = buildList {
                    if (startEnabled) add(CustomAccessibilityAction(startLabel) { onStart(); true })
                    add(CustomAccessibilityAction(endLabel) { onEnd(); true })
                }
            },
    ) {
        val value = shift.value
        if (value != 0f) {
            val fromStart = value > 0f
            Row(
                modifier = Modifier.matchParentSize().background(scheme.surfaceContainerHigh).padding(horizontal = 20.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = if (fromStart) Arrangement.Start else Arrangement.End,
            ) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Icon(if (fromStart) AgentGlyphs.tray else AgentGlyphs.pin, contentDescription = null, modifier = Modifier.size(18.dp))
                    Text(if (fromStart) startLabel else endLabel, style = MaterialTheme.typography.labelLarge.copy(fontWeight = FontWeight.SemiBold))
                }
            }
        }
        Box(
            Modifier
                .offset { IntOffset(shift.value.roundToInt(), 0) }
                .pointerInput(Unit) {
                    detectHorizontalDragGestures(
                        onDragEnd = {
                            scope.launch {
                                val at = shift.value
                                when {
                                    at >= threshold && canStart -> start()
                                    at <= -threshold -> end()
                                }
                                shift.animateTo(0f)
                            }
                        },
                        onDragCancel = { scope.launch { shift.animateTo(0f) } },
                    ) { change, drag ->
                        change.consume()
                        val low = -reach
                        val high = if (canStart) reach else 0f
                        scope.launch { shift.snapTo((shift.value + drag).coerceIn(low, high)) }
                    }
                },
        ) {
            content()
        }
    }
}

/**
 * A thread in the list: its state, the title (in the primary weight while
 * unread, with a pin when pinned), the agent and folder after where it came
 * from or when it comes back, and what it asks or does now. A tap opens it,
 * a long-press its sheet.
 */
@Composable
private fun HistoryThreadRow(
    thread: AgentThread,
    agentName: String,
    ask: AgentAsk?,
    questions: Int,
    now: Long,
    onOpen: () -> Unit,
    onMenu: () -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    val status = AgentsModel.status(thread)
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .background(scheme.surfaceContainer)
            .combinedClickable(role = Role.Button, onClick = onOpen, onLongClick = onMenu)
            .padding(horizontal = 14.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        StatusLine(
            status = status,
            trailing = relativeTime(thread.updatedMs, now),
            word = if (status is ThreadStatus.Working) workingFor(thread.sinceMs, now) else null,
        )
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(
                thread.title,
                style = MaterialTheme.typography.bodyLarge.copy(fontWeight = if (thread.unread) FontWeight.Bold else FontWeight.Normal),
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f, fill = false),
            )
            if (thread.pinned) {
                Icon(AgentGlyphs.pin, contentDescription = stringResource(R.string.agents_shelf_pinned), tint = scheme.onSurfaceVariant, modifier = Modifier.size(13.dp))
            }
        }
        val snoozed = thread.snoozedUntilMs?.let {
            stringResource(R.string.agents_snoozed_until, DateFormat.format(DateFormat.getBestDateTimePattern(LocalLocale.current.platformLocale, "EEE jm"), Date(it)).toString())
        }
        val origin = when (val from = AgentShelves.origin(thread)) {
            is ThreadOrigin.ForkOf -> stringResource(R.string.agents_fork_of, from.title)
            ThreadOrigin.BroughtIn -> stringResource(R.string.agents_brought_in)
            null -> null
        }
        Text(
            listOfNotNull(snoozed, origin, agentName, thread.folder.ifBlank { null }).joinToString(" · "),
            style = MaterialTheme.typography.bodySmall,
            color = scheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        if (status is ThreadStatus.NeedsYou) {
            // A glance at what it asks; the whole of it is read in the thread before it can be allowed.
            val preview = ask?.let { AgentsModel.askBody(it) ?: it.title }?.lineSequence()?.firstOrNull()
            if (ask == null && questions > 0) {
                Text(
                    pluralStringResource(R.plurals.agents_question_asks_you, questions, questions),
                    style = MaterialTheme.typography.bodySmall,
                    color = scheme.onSurfaceVariant,
                    maxLines = 1,
                )
            } else if (!preview.isNullOrBlank()) {
                Text(
                    preview,
                    style = MaterialTheme.typography.bodySmall.mono(LocalAppFonts.current),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(top = 4.dp)
                        .clip(RoundedCornerShape(8.dp))
                        .background(scheme.surface)
                        .padding(horizontal = 10.dp, vertical = 8.dp),
                )
            }
            Row(Modifier.fillMaxWidth().padding(top = 4.dp), horizontalArrangement = Arrangement.End) {
                G7RowAction(label = stringResource(R.string.agents_review), accent = true, onClick = onOpen)
            }
        } else if (status is ThreadStatus.Working) {
            NowLine(step = thread.step, sinceMs = thread.sinceMs)
        } else {
            val line = statusDetail(status) ?: thread.summary
            if (!line.isNullOrBlank()) {
                Text(
                    line,
                    style = MaterialTheme.typography.bodySmall,
                    color = scheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

/** How far a row moves before a let-go acts, and how far it can go. */
private val SWIPE_THRESHOLD = 96.dp
private val SWIPE_REACH = 128.dp
