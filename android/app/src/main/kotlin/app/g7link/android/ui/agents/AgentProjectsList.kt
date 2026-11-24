// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyItemScope
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.collapse
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.expand
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.G7SectionLabel
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.g7Tints
import app.g7link.android.ui.theme.mono
import app.g7link.core.AgentSession
import app.g7link.core.AgentSessionSource
import app.g7link.core.AgentThread

/** What a projects row does: a thread's tap, hold and swipes, and a session's tap. */
internal class ProjectActions(
    val thread: ThreadRowActions,
    val session: (String) -> Unit,
)

/** Whether folders open and close, and how many rows each shows. */
internal class FolderControl(
    val isOpen: (ProjectGroup) -> Boolean,
    val toggle: (ProjectGroup) -> Unit,
    val showsAll: (ProjectGroup) -> Boolean,
    val showAll: (ProjectGroup) -> Unit,
)

private val CARD = RoundedCornerShape(12.dp)
private val TOP = RoundedCornerShape(topStart = 12.dp, topEnd = 12.dp)
private val BOTTOM = RoundedCornerShape(bottomStart = 12.dp, bottomEnd = 12.dp)

/**
 * The Agents list as projects (the owner's pick of board B, 2026-11-24): the
 * requests waiting on a pager, then a card per folder that opens and closes,
 * the busiest first, with the folder's branch and running script under its
 * name and its threads and sessions inside. [animate] is off with the phone's
 * animations.
 */
internal fun LazyListScope.projects(
    agents: AgentsState,
    groups: List<ProjectGroup>,
    cards: List<NeedsYouCard>,
    pcName: String,
    now: Long,
    folders: FolderControl,
    actions: ProjectActions,
    animate: Boolean,
) {
    if (cards.isNotEmpty()) {
        item(key = "needs_you", contentType = "needs_you") { NeedsYouPager(cards, agents, pcName, now, actions) }
    }
    if (groups.isEmpty()) return
    item(key = "projects_label", contentType = "label") {
        G7SectionLabel(stringResource(R.string.agents_projects), Modifier.padding(top = 12.dp).then(moved(animate)))
    }
    for (group in groups) {
        val open = folders.isOpen(group)
        val rows = if (open) AgentProjects.shown(group, folders.showsAll(group)) else emptyList()
        val hidden = group.items.size - rows.size
        item(key = "folder:${group.folder}", contentType = "folder") {
            FolderHeader(group, open, rows.isNotEmpty(), now, onToggle = { folders.toggle(group) }, modifier = moved(animate))
        }
        itemsIndexed(rows, key = { _, row -> "row:${row.key}" }, contentType = { _, row -> if (row is ProjectItem.Thread) "project_thread" else "project_session" }) { index, row ->
            val shape = if (index == rows.lastIndex && hidden == 0) BOTTOM else RectangleShape
            when (row) {
                is ProjectItem.Thread -> ProjectThreadRow(row.thread, agents, now, actions.thread, shape, moved(animate))
                is ProjectItem.Session -> ProjectSessionRow(row.session, agents, pcName, now, { actions.session(row.session.id) }, shape, moved(animate))
            }
        }
        if (open && hidden > 0) {
            item(key = "more:${group.folder}", contentType = "more") {
                MoreRow(hidden, onClick = { folders.showAll(group) }, modifier = moved(animate))
            }
        }
    }
}

/** Rows slide into place as folders open and close; at once with animations off. */
private fun LazyItemScope.moved(animate: Boolean): Modifier = if (animate) Modifier.animateItem() else Modifier

/** The requests waiting, one card at a time, the next one peeking in from the side. */
@Composable
private fun NeedsYouPager(cards: List<NeedsYouCard>, agents: AgentsState, pcName: String, now: Long, actions: ProjectActions) {
    val scheme = MaterialTheme.colorScheme
    val p = agentPalette()
    val count by rememberUpdatedState(cards.size)
    val pager = rememberPagerState { count }
    Column(Modifier.fillMaxWidth().padding(top = 4.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(Modifier.fillMaxWidth().padding(start = 4.dp, end = 4.dp, top = 4.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(stringResource(R.string.agents_state_waiting), style = MaterialTheme.typography.labelLarge.copy(fontWeight = FontWeight.SemiBold), color = p.statusNeedsYou)
            Text(cards.size.toString(), style = MaterialTheme.typography.labelLarge, color = scheme.onSurfaceVariant)
            Spacer(Modifier.weight(1f))
            if (cards.size > 1) {
                val shown = pager.currentPage.coerceIn(0, cards.lastIndex)
                Text(stringResource(R.string.agents_needs_you_position, shown + 1, cards.size), style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
                if (cards.size <= MAX_DOTS) {
                    Row(horizontalArrangement = Arrangement.spacedBy(4.dp), verticalAlignment = Alignment.CenterVertically) {
                        for (i in cards.indices) {
                            Box(
                                Modifier
                                    .width(if (i == shown) 14.dp else 6.dp)
                                    .height(6.dp)
                                    .clip(RoundedCornerShape(3.dp))
                                    .background(if (i == shown) p.statusNeedsYou else scheme.outlineVariant),
                            )
                        }
                    }
                }
            }
        }
        HorizontalPager(
            state = pager,
            contentPadding = PaddingValues(end = if (cards.size > 1) PEEK else 0.dp),
            pageSpacing = 8.dp,
            verticalAlignment = Alignment.Top,
            key = { index -> cards.getOrNull(index)?.key ?: index },
        ) { page ->
            cards.getOrNull(page)?.let { card -> NeedsYouCardView(card, agents, pcName, now, actions) }
        }
    }
}

/** One request: where it is, its title, what it asks, and the button that opens it. */
@Composable
private fun NeedsYouCardView(card: NeedsYouCard, agents: AgentsState, pcName: String, now: Long, actions: ProjectActions) {
    val scheme = MaterialTheme.colorScheme
    val fonts = LocalAppFonts.current
    val open: () -> Unit = when (card) {
        is NeedsYouCard.Thread -> ({ actions.thread.open(card.thread.id) })
        is NeedsYouCard.Session -> ({ actions.session(card.session.id) })
    }
    Column(
        Modifier
            .fillMaxWidth()
            .clip(CARD)
            .background(scheme.surfaceContainer)
            .clickable(role = Role.Button, onClick = open)
            .padding(horizontal = 14.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        val (title, unread) = when (card) {
            is NeedsYouCard.Thread -> card.thread.title to card.thread.unread
            is NeedsYouCard.Session -> card.session.title to false
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            ThreadStatusMark(ThreadStatus.NeedsYou)
            val place = when (card) {
                is NeedsYouCard.Thread -> listOf(agents.agentName(card.thread.agent), card.thread.folder).filter { it.isNotBlank() }.joinToString(" · ")
                is NeedsYouCard.Session -> sessionPlace(card.session, agents.agentName(card.session.agent))
            }
            if (card is NeedsYouCard.Session) sourceIcon(card.session.source)?.let { Icon(it, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(13.dp)) }
            Text(place, style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            Text(relativeTime(card.updatedMs, now), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant, maxLines = 1)
        }
        Text(
            title,
            style = MaterialTheme.typography.bodyLarge.copy(fontWeight = if (unread) FontWeight.Bold else FontWeight.SemiBold),
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
        )
        Row(Modifier.fillMaxWidth().padding(top = 6.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            when (card) {
                is NeedsYouCard.Thread -> {
                    val ask = card.ask
                    if (ask != null) {
                        val body = AgentsModel.askBody(ask)?.lineSequence()?.firstOrNull { it.isNotBlank() }
                        Column(
                            Modifier
                                .weight(1f)
                                .clip(RoundedCornerShape(8.dp))
                                .background(scheme.surface)
                                .heightIn(min = 44.dp)
                                .padding(horizontal = 10.dp, vertical = 6.dp),
                            verticalArrangement = Arrangement.Center,
                        ) {
                            Text(ask.title, style = MaterialTheme.typography.labelSmall, color = scheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            if (body != null) {
                                Text(body, style = MaterialTheme.typography.bodySmall.mono(fonts), maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                        }
                    } else {
                        val words = if (card.questions > 0) pluralStringResource(R.plurals.agents_question_asks_you, card.questions, card.questions) else ""
                        Text(words, style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant, maxLines = 2, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                    }
                    G7RowAction(label = stringResource(R.string.agents_review), accent = true, onClick = open)
                }
                is NeedsYouCard.Session -> {
                    val waits = card.session.waitingFor?.takeIf { it.isNotBlank() }
                    Text(
                        waits ?: stringResource(R.string.agents_session_answer_on, pcName),
                        style = MaterialTheme.typography.bodyMedium,
                        color = scheme.onSurfaceVariant,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f),
                    )
                    G7RowAction(label = stringResource(R.string.agents_open), accent = true, onClick = open)
                }
            }
        }
    }
}

/**
 * A folder's name with the agents' folder glyph; open, its counts, branch and
 * running script; closed, one line of what's in it and when it last moved.
 */
@Composable
private fun FolderHeader(group: ProjectGroup, open: Boolean, hasRows: Boolean, now: Long, onToggle: () -> Unit, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val name = group.folder.ifBlank { stringResource(R.string.agents_no_folder) }
    val actionLabel = stringResource(if (open) R.string.agents_folder_collapse else R.string.agents_folder_expand)
    Column(
        modifier
            .fillMaxWidth()
            .padding(top = 8.dp)
            .clip(if (open && hasRows) TOP else CARD)
            .background(scheme.surfaceContainer)
            .clickable(role = Role.Button, onClickLabel = actionLabel, onClick = onToggle)
            .semantics {
                if (open) {
                    collapse { onToggle(); true }
                } else {
                    expand { onToggle(); true }
                }
            },
    ) {
        Column(Modifier.padding(start = 14.dp, end = 2.dp, top = 6.dp, bottom = 6.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(G7Icons.folder, contentDescription = null, tint = tints.amber, modifier = Modifier.size(18.dp))
                Column(Modifier.weight(1f).padding(start = 10.dp)) {
                    Text(name, style = MaterialTheme.typography.bodyLarge.copy(fontWeight = FontWeight.SemiBold), maxLines = 1, overflow = TextOverflow.Ellipsis)
                    if (!open) FolderSummary(group, now)
                }
                if (open) {
                    Text(folderCounts(group), style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant, maxLines = 1)
                }
                Box(Modifier.size(44.dp), contentAlignment = Alignment.Center) {
                    Icon(if (open) AgentGlyphs.chevronUp else G7Icons.chevronDown, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
                }
            }
            if (open) FolderStrip(group, Modifier.padding(start = 28.dp, bottom = 4.dp))
        }
        if (open && hasRows) Box(Modifier.fillMaxWidth().height(1.dp).background(scheme.surfaceContainerHigh))
    }
}

/** A closed folder's line: what needs you and runs in it, else what it holds; then when it last moved. */
@Composable
private fun FolderSummary(group: ProjectGroup, now: Long) {
    val scheme = MaterialTheme.colorScheme
    val p = agentPalette()
    val parts = buildList {
        if (group.needsYou > 0) add(pluralStringResource(R.plurals.agents_folder_needs_you, group.needsYou, group.needsYou) to p.statusNeedsYou)
        if (group.running > 0) add(pluralStringResource(R.plurals.agents_folder_running, group.running, group.running) to p.statusWorking)
        if (isEmpty()) add(folderCounts(group) to scheme.onSurfaceVariant)
        add(relativeTime(group.lastMs, now) to scheme.onSurfaceVariant)
    }
    Text(
        joined(parts),
        style = MaterialTheme.typography.bodySmall,
        color = scheme.onSurfaceVariant,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
    )
}

/** "7 threads · 2 sessions". */
@Composable
private fun folderCounts(group: ProjectGroup): String = listOfNotNull(
    group.threads.takeIf { it > 0 }?.let { pluralStringResource(R.plurals.agents_folder_threads, it, it) },
    group.sessions.takeIf { it > 0 }?.let { pluralStringResource(R.plurals.agents_folder_sessions, it, it) },
).joinToString(" · ")

/** The folder's branch, changes and pull request, and a script running in it. */
@Composable
private fun FolderStrip(group: ProjectGroup, modifier: Modifier = Modifier) {
    val strip = group.strip?.takeIf { AgentGit.stripShown(it) }
    val script = group.script
    if (strip == null && script == null) return
    val scheme = MaterialTheme.colorScheme
    val fonts = LocalAppFonts.current
    val style = MaterialTheme.typography.labelMedium
    Column(modifier, verticalArrangement = Arrangement.spacedBy(2.dp)) {
        if (strip != null) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Icon(AgentGlyphs.branch, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(13.dp))
                val branch = strip.branch ?: strip.detached?.let { stringResource(R.string.agents_git_detached, it) }.orEmpty()
                Text(branch, style = style.mono(fonts), color = scheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                if (strip.changed > 0) {
                    Text(pluralStringResource(R.plurals.agents_git_changed, strip.changed, strip.changed), style = style, color = scheme.onSurfaceVariant, maxLines = 1)
                }
                strip.pr?.let { pr ->
                    Icon(AgentGlyphs.pullRequest, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(13.dp))
                    Text("#${pr.number}", style = style, color = scheme.onSurfaceVariant, maxLines = 1)
                }
            }
        }
        if (script != null) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Icon(G7Icons.play, contentDescription = null, tint = g7Tints().teal, modifier = Modifier.size(13.dp))
                Text(stringResource(R.string.agents_script_running_line, script), style = style, color = scheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
    }
}

/** A thread in its folder: its mark, title, then its state, agent and what it does; it swipes and holds as the shelves' rows do. */
@Composable
private fun ProjectThreadRow(thread: AgentThread, agents: AgentsState, now: Long, actions: ThreadRowActions, shape: Shape, modifier: Modifier = Modifier) {
    val p = agentPalette()
    val status = AgentsModel.status(thread)
    val archiveLabel = stringResource(if (thread.archived) R.string.agents_mark_unarchive else R.string.agents_mark_archive)
    val pinLabel = stringResource(if (thread.pinned) R.string.agents_mark_unpin else R.string.agents_mark_pin)
    Box(modifier.fillMaxWidth()) {
        SwipeRow(
            startLabel = archiveLabel,
            endLabel = pinLabel,
            // Archiving waits while it works or waits (the PC says BUSY then).
            startEnabled = !AgentsModel.running(thread),
            onStart = { actions.archive(thread) },
            onEnd = { actions.pin(thread) },
            shape = shape,
        ) {
            ProjectRowBody(
                mark = { ThreadStatusMark(status) },
                title = thread.title,
                unread = thread.unread,
                pinned = thread.pinned,
                trailing = {
                    when (status) {
                        ThreadStatus.NeedsYou -> Trailing(stringResource(R.string.agents_state_waiting), p.statusNeedsYou, strong = true)
                        ThreadStatus.Working -> thread.sinceMs?.takeIf { it > 0L }?.let {
                            ElapsedText(it, style = MaterialTheme.typography.labelMedium, color = p.statusWorking)
                        }
                        else -> Trailing(relativeTime(thread.updatedMs, now), MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                },
                meta = threadMeta(thread, agents, status),
                metaIcon = null,
                onClick = { actions.open(thread.id) },
                onLongClick = { actions.menu(thread.id) },
            )
        }
    }
}

/** What a thread's row says under its title: the state when it rests, the agent, then what it asks, does or said. */
@Composable
private fun threadMeta(thread: AgentThread, agents: AgentsState, status: ThreadStatus): AnnotatedString {
    val p = agentPalette()
    val muted = MaterialTheme.colorScheme.onSurfaceVariant
    val agent = agents.agentName(thread.agent)
    val origin = when (val from = AgentShelves.origin(thread)) {
        is ThreadOrigin.ForkOf -> stringResource(R.string.agents_fork_of, from.title)
        ThreadOrigin.BroughtIn -> stringResource(R.string.agents_brought_in)
        null -> null
    }
    return when (status) {
        ThreadStatus.NeedsYou -> {
            val ask = agents.asksFor(thread.id).firstOrNull()
            val questions = maxOf(agents.questionsFor(thread.id).size, thread.asking.toInt())
            val what = ask?.let { AgentsModel.askBody(it) ?: it.title }?.lineSequence()?.firstOrNull { it.isNotBlank() }
                ?: if (questions > 0) pluralStringResource(R.plurals.agents_question_asks_you, questions, questions) else null
            joined(listOfNotNull(agent, what).map { it to muted })
        }
        ThreadStatus.Working -> joined(listOfNotNull(agent, thread.step?.takeIf { it.isNotBlank() }?.let { stringResource(R.string.agents_step_now, it) }, origin).map { it to muted })
        else -> {
            val (word, color) = restingWord(status)
            val detail = statusDetail(status) ?: thread.summary?.takeIf { it.isNotBlank() }
            joined(listOf(word to color) + listOfNotNull(agent, origin, detail).map { it to muted })
        }
    }
}

/** A thread at rest in words and their colour, as its status line says it. */
@Composable
private fun restingWord(status: ThreadStatus): Pair<String, Color> {
    val p = agentPalette()
    val muted = MaterialTheme.colorScheme.onSurfaceVariant
    return when (status) {
        ThreadStatus.Done -> stringResource(R.string.agents_state_done) to p.statusDone
        ThreadStatus.PlanReady -> stringResource(R.string.agents_state_plan_ready) to p.statusDone
        ThreadStatus.Stopped -> stringResource(R.string.agents_state_interrupted) to muted
        is ThreadStatus.Paused -> stringResource(R.string.agents_state_paused) to muted
        is ThreadStatus.Failed -> stringResource(R.string.agents_state_failed) to p.statusFailedText
        ThreadStatus.NeedsYou -> stringResource(R.string.agents_state_waiting) to p.statusNeedsYou
        ThreadStatus.Working -> stringResource(R.string.agents_state_working) to p.statusWorking
    }
}

/** A session started on the PC, in its folder: where it runs, and its state in the session's own words. */
@Composable
private fun ProjectSessionRow(session: AgentSession, agents: AgentsState, pcName: String, now: Long, onOpen: () -> Unit, shape: Shape, modifier: Modifier = Modifier) {
    val p = agentPalette()
    val muted = MaterialTheme.colorScheme.onSurfaceVariant
    val look = AgentsModel.sessionLook(session)
    val agent = agents.agentName(session.agent)
    val where = when (session.source) {
        AgentSessionSource.TERMINAL -> stringResource(R.string.agents_session_in_terminal)
        AgentSessionSource.DESKTOP_APP -> stringResource(R.string.agents_session_in_app, agent)
        AgentSessionSource.OTHER -> null
    }
    val (word, color) = sessionWord(session, now)
    val meta = when (look) {
        SessionLook.NEEDS_YOU -> joined(
            listOfNotNull(agent, where, session.waitingFor?.takeIf { it.isNotBlank() } ?: stringResource(R.string.agents_session_answer_on, pcName)).map { it to muted },
        )
        SessionLook.WORKING -> joined(listOfNotNull(agent, where, session.step?.takeIf { it.isNotBlank() }?.let { stringResource(R.string.agents_step_now, it) }).map { it to muted })
        SessionLook.MAY_NEED_YOU, SessionLook.LIMITED, SessionLook.IDLE, SessionLook.ENDED -> joined(listOf(word to color) + listOfNotNull(agent, where).map { it to muted })
    }
    Box(modifier.fillMaxWidth().clip(shape)) {
        ProjectRowBody(
            mark = { SessionLookMark(look) },
            title = session.title,
            unread = false,
            pinned = false,
            trailing = {
                when (look) {
                    SessionLook.NEEDS_YOU -> Trailing(stringResource(R.string.agents_state_waiting), p.statusNeedsYou, strong = true)
                    SessionLook.WORKING -> session.sinceMs?.takeIf { it > 0L }?.let {
                        ElapsedText(it, style = MaterialTheme.typography.labelMedium, color = p.statusWorking)
                    }
                    else -> Trailing(relativeTime(session.updatedMs, now), muted)
                }
            },
            meta = meta,
            metaIcon = sourceIcon(session.source),
            onClick = onOpen,
            onLongClick = null,
        )
    }
}

/** The glyph for where a session runs: a terminal, or the agent's own app. */
private fun sourceIcon(source: AgentSessionSource): ImageVector? = when (source) {
    AgentSessionSource.TERMINAL -> G7Icons.terminal
    AgentSessionSource.DESKTOP_APP -> G7Icons.window
    AgentSessionSource.OTHER -> null
}

@Composable
private fun ProjectRowBody(
    mark: @Composable () -> Unit,
    title: String,
    unread: Boolean,
    pinned: Boolean,
    trailing: @Composable () -> Unit,
    meta: AnnotatedString,
    metaIcon: ImageVector?,
    onClick: () -> Unit,
    onLongClick: (() -> Unit)?,
) {
    val scheme = MaterialTheme.colorScheme
    Row(
        Modifier
            .fillMaxWidth()
            .background(scheme.surfaceContainer)
            .then(
                if (onLongClick != null) {
                    Modifier.combinedClickable(role = Role.Button, onClick = onClick, onLongClick = onLongClick)
                } else {
                    Modifier.clickable(role = Role.Button, onClick = onClick)
                },
            )
            .heightIn(min = 56.dp)
            .padding(horizontal = 14.dp, vertical = 10.dp),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Box(Modifier.height(20.dp), contentAlignment = Alignment.Center) { mark() }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(
                    title,
                    style = MaterialTheme.typography.bodyLarge.copy(fontWeight = if (unread) FontWeight.Bold else FontWeight.Normal),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false),
                )
                if (pinned) {
                    Icon(AgentGlyphs.pin, contentDescription = stringResource(R.string.agents_shelf_pinned), tint = scheme.onSurfaceVariant, modifier = Modifier.size(13.dp))
                }
                Spacer(Modifier.weight(1f))
                trailing()
            }
            if (meta.isNotEmpty()) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp)) {
                    metaIcon?.let { Icon(it, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp)) }
                    Text(meta, style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
        }
    }
}

@Composable
private fun Trailing(text: String, color: Color, strong: Boolean = false) {
    Text(
        text,
        style = MaterialTheme.typography.labelMedium.copy(fontWeight = if (strong) FontWeight.SemiBold else FontWeight.Normal),
        color = color,
        maxLines = 1,
    )
}

/** "3 more" at the foot of a folder that shows its first rows. */
@Composable
private fun MoreRow(count: Int, onClick: () -> Unit, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    Column(modifier.fillMaxWidth().clip(BOTTOM).background(scheme.surfaceContainer)) {
        Box(Modifier.fillMaxWidth().height(1.dp).background(scheme.surfaceContainerHigh))
        Row(
            Modifier
                .fillMaxWidth()
                .clickable(role = Role.Button, onClick = onClick)
                .heightIn(min = 44.dp)
                .padding(horizontal = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Text(pluralStringResource(R.plurals.agents_folder_more, count, count), style = MaterialTheme.typography.labelLarge, color = scheme.onSurfaceVariant)
            Icon(G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
        }
    }
}

/** The shelves under the folders: Snoozed and Archived, with how many when the PC has listed them. */
@Composable
internal fun ShelfButtons(agents: AgentsState, onShelf: (ThreadFilter) -> Unit, modifier: Modifier = Modifier) {
    Row(modifier.fillMaxWidth().padding(top = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        for ((filter, icon) in listOf(ThreadFilter.SNOOZED to G7Icons.clock, ThreadFilter.ARCHIVED to AgentGlyphs.tray)) {
            val word = stringResource(if (filter == ThreadFilter.SNOOZED) R.string.agents_shelf_snoozed else R.string.agents_shelf_archived)
            val count = if (filter.shelf in agents.shelves) AgentShelves.on(agents.threads, filter.shelf).size else null
            ShelfButton(icon = icon, word = word, count = count, onClick = { onShelf(filter) }, modifier = Modifier.weight(1f))
        }
    }
}

@Composable
private fun ShelfButton(icon: ImageVector, word: String, count: Int?, onClick: () -> Unit, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    Row(
        modifier
            .clip(CARD)
            .border(1.dp, scheme.outlineVariant, CARD)
            .clickable(role = Role.Button, onClick = onClick)
            .heightIn(min = 48.dp)
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(icon, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
        Text(word, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        if (count != null) Text(count.toString(), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
    }
}

/** A shelf's own heading, with the way back to the projects. */
@Composable
internal fun ShelfHeader(filter: ThreadFilter, onBack: () -> Unit) {
    Row(Modifier.fillMaxWidth().padding(bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        G7IconButton(icon = G7Icons.back, contentDescription = stringResource(R.string.agents_shelf_back), onClick = onBack)
        Text(
            stringResource(if (filter == ThreadFilter.SNOOZED) R.string.agents_shelf_snoozed else R.string.agents_shelf_archived),
            style = MaterialTheme.typography.titleMedium,
        )
    }
}

/** The search button at the screen's bottom end, in reach of the thumb. */
@Composable
internal fun SearchButton(onClick: () -> Unit, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    val label = stringResource(R.string.agents_search)
    Box(
        modifier
            .size(56.dp)
            .shadow(6.dp, RoundedCornerShape(16.dp))
            .clip(RoundedCornerShape(16.dp))
            .background(scheme.primary)
            .clickable(role = Role.Button, onClick = onClick)
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) {
        Icon(G7Icons.search, contentDescription = null, tint = scheme.onPrimary, modifier = Modifier.size(22.dp))
    }
}

/** Parts joined with " · ", each in its colour. */
private fun joined(parts: List<Pair<String, Color>>): AnnotatedString = buildAnnotatedString {
    parts.forEachIndexed { index, (text, color) ->
        if (index > 0) append(" · ")
        withStyle(SpanStyle(color = color)) { append(text) }
    }
}

/** How far the next request peeks in from the side. */
private val PEEK = 28.dp

/** Dots under the pager up to this many requests; past it, only "2 of 9". */
private const val MAX_DOTS = 6
