// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.Preferences
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7Row
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.G7SectionLabel
import app.g7link.android.ui.theme.G7StatusDot
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.g7Tints
import app.g7link.core.AgentAsk
import app.g7link.core.AgentAskKind
import app.g7link.core.AgentSession
import app.g7link.core.AgentThread
import kotlinx.coroutines.launch

/**
 * A PC's agents at the top of Home: what needs you (one Review; requests are
 * answered in the thread, where the whole of them shows), what's working and
 * what it does now, or, with neither, a box to start a thread. "All threads"
 * opens the Agents screen in every state. Without anything to show yet (not
 * allowed on the PC, out of reach, nothing can run) it's the one row that
 * opens that screen, which says what's missing.
 */
@Composable
fun AgentsHomeSection(
    pc: Device,
    core: Core,
    onOpenAgents: () -> Unit,
    onOpenThread: (String) -> Unit,
    onOpenSession: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val all by core.agents.collectAsStateWithLifecycle()
    val state = all[pc.id] ?: AgentsState()
    val summary = remember(state) { AgentsModel.homeSummary(state) }
    val tints = g7Tints()
    Column(modifier) {
        if (summary == null || !pc.online) {
            G7SectionLabel(stringResource(R.string.agents_home_title, pc.name))
            G7Row(
                label = stringResource(R.string.action_agents_pc),
                icon = G7Icons.terminal,
                tint = tints.coral,
                enabled = pc.online,
                onClick = onOpenAgents,
            )
        } else {
            HomeAgentsHeader(pcName = pc.name, onAll = onOpenAgents)
            val now = rememberNow()
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                when (val lead = summary.lead) {
                    is HomeLead.NeedsYou -> NeedsYouCard(
                        lead = lead,
                        agentName = state.agentName(lead.thread?.agent ?: lead.ask?.agent.orEmpty()),
                        now = now,
                        onReview = { (lead.thread?.id ?: lead.ask?.thread)?.let(onOpenThread) },
                    )
                    is HomeLead.SessionNeedsYou -> SessionNeedsYouCard(
                        session = lead.session,
                        agentName = state.agentName(lead.session.agent),
                        pcName = pc.name,
                        now = now,
                        onOpen = { onOpenSession(lead.session.id) },
                    )
                    null -> Unit
                }
                for (work in summary.working) {
                    when (work) {
                        is HomeWork.Thread -> WorkingRow(
                            title = work.thread.title,
                            place = "${state.agentName(work.thread.agent)} · ${work.thread.folder}",
                            step = work.thread.step,
                            sinceMs = work.thread.sinceMs,
                            onOpen = { onOpenThread(work.thread.id) },
                        )
                        is HomeWork.Session -> WorkingRow(
                            title = work.session.title,
                            place = sessionPlace(work.session, state.agentName(work.session.agent)),
                            step = work.session.step,
                            sinceMs = work.session.sinceMs,
                            onOpen = { onOpenSession(work.session.id) },
                        )
                    }
                }
                if (summary.start) {
                    StartCard(pc = pc, core = core, state = state, onStarted = onOpenThread)
                    summary.recent?.let { RecentRow(thread = it, now = now, onOpen = { onOpenThread(it.id) }) }
                }
            }
        }
    }
}

/** "Agents on guchu" with "All threads" at its end. */
@Composable
private fun HomeAgentsHeader(pcName: String, onAll: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Bottom) {
        G7SectionLabel(stringResource(R.string.agents_home_title, pcName), Modifier.weight(1f))
        Row(
            modifier = Modifier
                .heightIn(min = 40.dp)
                .clip(RoundedCornerShape(8.dp))
                .clickable(role = Role.Button, onClick = onAll)
                .padding(horizontal = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text(stringResource(R.string.agents_home_all), style = MaterialTheme.typography.labelLarge, color = scheme.onSurface)
            Icon(G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(14.dp))
        }
    }
}

private val CARD = RoundedCornerShape(10.dp)

/**
 * A thread that needs you: who asks, the thread, what it wants to do on one
 * line and the folder beside it, and Review. Allow and Deny are in the thread.
 */
@Composable
private fun NeedsYouCard(lead: HomeLead.NeedsYou, agentName: String, now: Long, onReview: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val ask = lead.ask
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(CARD)
            .background(scheme.surfaceContainer)
            .clickable(role = Role.Button, onClick = onReview)
            .padding(horizontal = 14.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Icon(G7Icons.clock, contentDescription = null, tint = tints.amber, modifier = Modifier.size(18.dp))
            Text(
                stringResource(R.string.agents_home_needs_you, agentName),
                style = MaterialTheme.typography.titleSmall,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            if (ask != null) {
                Text(
                    stringResource(R.string.agents_home_asked, relativeTime(ask.expiresMs - ASK_LIFETIME_MS, now)),
                    style = MaterialTheme.typography.labelMedium,
                    color = scheme.onSurfaceVariant,
                    maxLines = 1,
                )
            }
        }
        val title = lead.thread?.title ?: ask?.threadTitle
        if (!title.isNullOrBlank()) {
            Text(title, style = MaterialTheme.typography.bodyMedium, maxLines = 2, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(start = 28.dp))
        }
        val what = ask?.let { askLine(it) }
        if (what != null) {
            Row(
                modifier = Modifier
                    .padding(start = 28.dp)
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(8.dp))
                    .background(scheme.surface)
                    .padding(horizontal = 10.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Icon(
                    if (ask.kind == AgentAskKind.EDIT) G7Icons.edit else G7Icons.terminal,
                    contentDescription = null,
                    tint = scheme.onSurfaceVariant,
                    modifier = Modifier.size(14.dp),
                )
                Text(
                    what,
                    style = MaterialTheme.typography.bodySmall,
                    fontFamily = if (ask.kind == AgentAskKind.COMMAND) LocalAppFonts.current.mono else null,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                val folder = lead.thread?.folder
                if (!folder.isNullOrBlank()) {
                    Icon(G7Icons.folder, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
                    Text(folder, style = MaterialTheme.typography.labelSmall, color = scheme.onSurfaceVariant, maxLines = 1)
                }
            }
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
            G7RowAction(label = stringResource(R.string.agents_review), accent = true, onClick = onReview)
        }
    }
}

/** A request on one line: the command's first line, "Edit <file>", or its title. */
@Composable
private fun askLine(ask: AgentAsk): String? = when (ask.kind) {
    AgentAskKind.COMMAND -> ask.command?.lineSequence()?.firstOrNull { it.isNotBlank() }?.trim()
    AgentAskKind.EDIT -> ask.path?.substringAfterLast('/')?.takeIf { it.isNotBlank() }?.let { stringResource(R.string.agents_home_edit, it) }
    AgentAskKind.OTHER -> ask.title.takeIf { it.isNotBlank() }
} ?: ask.title.takeIf { it.isNotBlank() }

/** A session started on the PC waits there for an answer, as the agent reported: it's answered on the PC. */
@Composable
private fun SessionNeedsYouCard(session: AgentSession, agentName: String, pcName: String, now: Long, onOpen: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(CARD)
            .background(scheme.surfaceContainer)
            .clickable(role = Role.Button, onClick = onOpen)
            .padding(horizontal = 14.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Icon(G7Icons.clock, contentDescription = null, tint = tints.amber, modifier = Modifier.size(18.dp))
            Text(
                stringResource(R.string.agents_notify_waiting_public, agentName, pcName),
                style = MaterialTheme.typography.titleSmall,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            Text(
                relativeTime(session.sinceMs?.takeIf { it > 0L } ?: session.updatedMs, now),
                style = MaterialTheme.typography.labelMedium,
                color = scheme.onSurfaceVariant,
                maxLines = 1,
            )
        }
        Text(session.title, style = MaterialTheme.typography.bodyMedium, maxLines = 2, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(start = 28.dp))
        Row(
            modifier = Modifier.padding(start = 28.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Icon(G7Icons.terminal, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(14.dp))
            Text(stringResource(R.string.agents_session_answer_on, pcName), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
        }
    }
}

/** A thread or session at work: its title, the agent and folder, and "Now: <step> · 4:12". */
@Composable
private fun WorkingRow(title: String, place: String, step: String?, sinceMs: Long?, onOpen: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(CARD)
            .background(scheme.surfaceContainer)
            .clickable(role = Role.Button, onClick = onOpen)
            .padding(horizontal = 14.dp, vertical = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Box(Modifier.padding(top = 7.dp)) { G7StatusDot(tints.blue) }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(title, style = MaterialTheme.typography.titleSmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
            Text(place, style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
            NowLine(step = step, sinceMs = sinceMs)
        }
    }
}

/**
 * Nothing needs you or works: a box to start a thread with the agent and
 * folder picked last time. It always asks before each change, like every
 * thread started from a phone.
 */
@Composable
private fun StartCard(pc: Device, core: Core, state: AgentsState, onStarted: (String) -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val context = LocalContext.current
    val resources = LocalResources.current
    val scope = rememberCoroutineScope()
    val catalog = state.catalog ?: return
    val ready = state.readyAgents
    val saved = remember(pc.id) { Preferences.agentChoice(context, pc.id) }
    var agentId by rememberSaveable(pc.id) { mutableStateOf(saved.first) }
    var folderId by rememberSaveable(pc.id) { mutableStateOf(saved.second) }
    var prompt by rememberSaveable(pc.id) { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val agent = ready.firstOrNull { it.id == agentId } ?: ready.firstOrNull()
    val folder = catalog.folders.firstOrNull { it.id == folderId } ?: catalog.folders.firstOrNull()
    val canStart = !busy && agent != null && folder != null && AgentsModel.fits(prompt)

    val start: () -> Unit = start@{
        if (!canStart || agent == null || folder == null) return@start
        busy = true
        error = null
        scope.launch {
            when (val result = core.agentStart(pc.id, agent.id, folder.id, prompt.trim())) {
                is AgentCall.Ok -> {
                    Preferences.setAgentChoice(context, pc.id, agent.id, folder.id)
                    prompt = ""
                    onStarted(result.value.id)
                }
                is AgentCall.Failed -> error = when (result.failure) {
                    AgentFailure.DENIED -> resources.getString(R.string.agents_access_title, pc.name)
                    AgentFailure.UNSUPPORTED -> resources.getString(R.string.agents_unavailable, pc.name)
                    AgentFailure.OFFLINE, AgentFailure.TIMEOUT -> resources.getString(R.string.agents_unreachable_title, pc.name)
                    AgentFailure.NOT_FOUND, AgentFailure.OTHER -> resources.getString(R.string.agents_failed_generic)
                }
            }
            busy = false
        }
    }

    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(CARD)
            .background(scheme.surfaceContainer)
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        AgentTextField(
            value = prompt,
            onValueChange = { prompt = it },
            hint = stringResource(R.string.agents_home_prompt_hint, agent?.name.orEmpty(), pc.name),
            minLines = 2,
            maxLines = 5,
            background = scheme.surface,
        )
        if (AgentsModel.tooLong(prompt)) {
            Text(stringResource(R.string.agents_too_long), style = MaterialTheme.typography.bodySmall, color = scheme.error)
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (agent != null) {
                PickChip(
                    label = agent.name,
                    description = stringResource(R.string.agents_pick_agent),
                    options = ready.map { it.id to it.name },
                    onPick = { agentId = it },
                    modifier = Modifier.weight(1f, fill = false),
                )
            }
            if (folder != null) {
                PickChip(
                    label = folder.label,
                    description = stringResource(R.string.agents_pick_folder),
                    options = catalog.folders.map { it.id to it.label },
                    onPick = { folderId = it },
                    leadingFolder = true,
                    modifier = Modifier.weight(1f, fill = false),
                )
            }
            Spacer(Modifier.weight(1f))
            if (busy) {
                CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
            } else {
                G7RowAction(label = stringResource(R.string.agents_start), accent = true, enabled = canStart, onClick = start)
            }
        }
        Text(stringResource(R.string.agents_home_asks_first), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
        error?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = scheme.error) }
    }
}

/** A small outlined choice that opens a menu of the others. */
@Composable
private fun PickChip(
    label: String,
    description: String,
    options: List<Pair<String, String>>,
    onPick: (String) -> Unit,
    modifier: Modifier = Modifier,
    leadingFolder: Boolean = false,
) {
    val scheme = MaterialTheme.colorScheme
    var open by remember { mutableStateOf(false) }
    val many = options.size > 1
    Box(modifier) {
        Row(
            modifier = Modifier
                .heightIn(min = 40.dp)
                .clip(RoundedCornerShape(8.dp))
                .border(1.dp, scheme.outlineVariant, RoundedCornerShape(8.dp))
                .clickable(enabled = many, role = Role.Button, onClickLabel = description) { open = true }
                .padding(horizontal = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            if (leadingFolder) Icon(G7Icons.folder, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(14.dp))
            Text(label, style = MaterialTheme.typography.labelLarge, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
            if (many) Icon(G7Icons.chevronDown, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(14.dp))
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            for ((id, name) in options) {
                DropdownMenuItem(
                    text = { Text(name) },
                    onClick = {
                        open = false
                        onPick(id)
                    },
                )
            }
        }
    }
}

/** The thread that ran last, under the start box: its title and how it ended. */
@Composable
private fun RecentRow(thread: AgentThread, now: Long, onOpen: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val status = AgentsModel.status(thread)
    val (dot, word) = when (status) {
        ThreadStatus.Done -> tints.teal to stringResource(R.string.agents_state_done)
        ThreadStatus.Stopped -> scheme.outline to stringResource(R.string.agents_state_interrupted)
        is ThreadStatus.Paused -> scheme.outline to stringResource(R.string.agents_state_paused)
        is ThreadStatus.Failed -> tints.red to stringResource(R.string.agents_state_failed)
        ThreadStatus.Working -> tints.blue to stringResource(R.string.agents_state_working)
        ThreadStatus.NeedsYou -> tints.amber to stringResource(R.string.agents_state_waiting)
    }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(CARD)
            .background(scheme.surfaceContainer)
            .clickable(role = Role.Button, onClick = onOpen)
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        G7StatusDot(dot)
        Text(thread.title, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        Text(
            stringResource(R.string.agents_home_recent, word, relativeTime(thread.updatedMs, now)),
            style = MaterialTheme.typography.labelMedium,
            color = scheme.onSurfaceVariant,
            maxLines = 1,
        )
    }
}

/** A request lasts two hours on the PC (contract §2.2), so it was asked this long before it expires. */
private const val ASK_LIFETIME_MS = 7_200_000L
