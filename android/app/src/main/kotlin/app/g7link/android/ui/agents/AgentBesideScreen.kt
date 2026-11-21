// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.g7Tints
import app.g7link.android.ui.theme.mono
import app.g7link.core.AgentAskKind
import app.g7link.core.AgentDiffDoc
import app.g7link.core.AgentFileStatus
import app.g7link.core.AgentItem
import app.g7link.core.AgentPlanStatus
import app.g7link.core.AgentTurnFile
import app.g7link.core.AgentTurnFiles
import app.g7link.core.AgentTurnRange
import app.g7link.core.agentDiff
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext

/** Diff rows each edit shows under its file in Changes (board 10b); the rest is a tap away. */
private const val EDIT_ROWS = 4

/** The request with buttons scrolls within this share of the pane, so all of it can be read before Allow. */
private const val DOCK_CARD_SHARE = 0.7f

/**
 * The phone beside the PC (contract §V9.1, boards 10b–10d): the thread's
 * title and "Beside Kilele · Working", one switch of three panes, and the
 * pane. Changes: while a turn runs, its edits so far, newest first, each
 * opening its diff; after it, the files the PC recorded. Plan: the plan card
 * or the task list with the step under way marked. Requests: everything
 * waiting, oldest first, and only the one the dock shows has buttons (the
 * dock's own, through [dockCard]). The screen stays awake while a turn runs,
 * at most 10 minutes at a time. Nothing is restored from here.
 */
@Composable
internal fun AgentBesideScreen(
    core: Core,
    pcId: String,
    pcName: String,
    threadId: String,
    title: String,
    status: ThreadStatus?,
    agentName: String,
    running: Boolean,
    pane: BesidePane,
    onPane: (BesidePane) -> Unit,
    changes: BesideChanges,
    plan: BesidePlan,
    requests: List<BesideRequest>,
    buttonsFor: String?,
    onBack: () -> Unit,
    onOpenEdit: (String, AgentDiffDoc) -> Unit,
    onOpenFile: (UInt, String) -> Unit,
    planCard: @Composable (String) -> Unit,
    dockCard: @Composable (BesideRequest, Int, Int, Dp) -> Unit,
    modifier: Modifier = Modifier,
) {
    // Awake while a turn runs, from when this screen saw it run, and never more than 10 minutes.
    var runningSince by remember(threadId) { mutableStateOf<Long?>(null) }
    LaunchedEffect(running) { runningSince = if (running) System.currentTimeMillis() else null }
    var awake by remember(threadId) { mutableStateOf(false) }
    LaunchedEffect(runningSince) {
        val since = runningSince
        awake = AgentBeside.keepsAwake(since, System.currentTimeMillis())
        val until = AgentBeside.awakeUntilMs(since) ?: return@LaunchedEffect
        if (!awake) return@LaunchedEffect
        delay((until - System.currentTimeMillis()).coerceAtLeast(0L))
        awake = false
    }
    if (awake) {
        val view = LocalView.current
        DisposableEffect(view) {
            val before = view.keepScreenOn
            view.keepScreenOn = true
            onDispose { view.keepScreenOn = before }
        }
    }
    val scheme = MaterialTheme.colorScheme
    Column(modifier.fillMaxSize().background(scheme.surface)) {
        BesideHeader(title = title, status = status, pcName = pcName, onBack = onBack)
        PaneSwitch(pane = pane, onPane = onPane)
        Box(Modifier.weight(1f).fillMaxWidth()) {
            when (pane) {
                BesidePane.CHANGES -> ChangesPane(core, pcId, threadId, changes, onOpenEdit, onOpenFile)
                BesidePane.PLAN -> PlanPane(plan, planCard)
                BesidePane.REQUESTS -> RequestsPane(requests, buttonsFor, agentName, pcName, dockCard)
            }
        }
    }
}

/** Under the thread's header (board 10a): "Kilele is showing this thread" and Show beside. */
@Composable
internal fun BesideOfferLine(pcName: String, onShow: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Row(
        modifier = Modifier.fillMaxWidth().background(scheme.surfaceContainer).padding(start = 16.dp, end = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(G7Icons.monitor, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(15.dp))
        Text(
            stringResource(R.string.agents_beside_offer, pcName),
            style = MaterialTheme.typography.bodySmall,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        AccentTextButton(stringResource(R.string.agents_beside_show), onClick = onShow)
    }
}

/** Back (to the thread), the title on one line, and "● Beside Kilele · Working" under it. */
@Composable
private fun BesideHeader(title: String, status: ThreadStatus?, pcName: String, onBack: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Row(
        modifier = Modifier.fillMaxWidth().padding(start = 4.dp, end = 4.dp, top = 4.dp, bottom = 6.dp),
        verticalAlignment = Alignment.Top,
    ) {
        G7IconButton(icon = G7Icons.back, contentDescription = stringResource(R.string.action_back), onClick = onBack)
        Column(Modifier.weight(1f).padding(start = 4.dp, end = 12.dp, top = 8.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Text(
                title,
                style = MaterialTheme.typography.titleMedium.copy(fontWeight = FontWeight.SemiBold),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                if (status != null) ThreadStatusMark(status)
                val beside = stringResource(R.string.agents_beside_of, pcName)
                val word = status?.let { threadStateWord(it) }
                Text(
                    listOfNotNull(beside, word).joinToString(" · "),
                    style = MaterialTheme.typography.labelMedium,
                    color = scheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

/** Changes · Plan · Requests: one tap to switch. */
@Composable
private fun PaneSwitch(pane: BesidePane, onPane: (BesidePane) -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val panes = listOf(
        BesidePane.CHANGES to R.string.agents_beside_changes,
        BesidePane.PLAN to R.string.agents_beside_plan,
        BesidePane.REQUESTS to R.string.agents_beside_requests,
    )
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(start = 12.dp, end = 12.dp, top = 4.dp, bottom = 8.dp)
            .clip(RoundedCornerShape(10.dp))
            .background(scheme.surfaceContainer)
            .padding(4.dp)
            .selectableGroup(),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        for ((value, label) in panes) {
            val on = value == pane
            Box(
                modifier = Modifier
                    .weight(1f)
                    .heightIn(min = 36.dp)
                    .clip(RoundedCornerShape(7.dp))
                    .background(if (on) scheme.surfaceContainerHigh else Color.Transparent)
                    .selectable(selected = on, role = Role.Tab, onClick = { onPane(value) }),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    stringResource(label),
                    style = MaterialTheme.typography.labelLarge.copy(fontWeight = if (on) FontWeight.SemiBold else FontWeight.Medium),
                    color = if (on) scheme.onSurface else scheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

/** A pane with nothing to show: one muted line. */
@Composable
private fun PaneNote(text: String) {
    Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
        Text(text, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center)
    }
}

// ---- Changes ----

@Composable
private fun ChangesPane(
    core: Core,
    pcId: String,
    threadId: String,
    changes: BesideChanges,
    onOpenEdit: (String, AgentDiffDoc) -> Unit,
    onOpenFile: (UInt, String) -> Unit,
) {
    when (changes) {
        is BesideChanges.Edits -> EditsList(changes.items, onOpenEdit)
        is BesideChanges.Files -> TurnFiles(core, pcId, threadId, changes.turn, onOpenFile)
        BesideChanges.Recording -> PaneNote(stringResource(R.string.agents_changes_recording))
        BesideChanges.Empty -> PaneNote(stringResource(R.string.agents_beside_nothing_changed))
    }
}

/** The turn's edits so far, newest first: each its file, counts and first lines, opening its diff. */
@Composable
private fun EditsList(edits: List<AgentItem>, onOpen: (String, AgentDiffDoc) -> Unit) {
    // Each edit's diff is read once, off the main thread, and again only when it changes.
    val cache = remember { HashMap<String, Pair<String, AgentDiffDoc?>>() }
    val docs by produceState(emptyMap<String, AgentDiffDoc>(), edits) {
        val wanted = edits.mapNotNull { item -> item.tool?.detail?.takeIf { it.isNotBlank() }?.let { item.id to it } }
        val missing = wanted.filter { (id, detail) -> cache[id]?.first != detail }
        if (missing.isNotEmpty()) {
            val parsed = withContext(Dispatchers.Default) {
                missing.map { (id, detail) -> Triple(id, detail, runCatching { agentDiff(detail) }.getOrNull()) }
            }
            parsed.forEach { (id, detail, doc) -> cache[id] = detail to doc }
        }
        val ids = wanted.mapTo(HashSet()) { it.first }
        cache.keys.retainAll(ids)
        value = wanted.mapNotNull { (id, _) -> cache[id]?.second?.let { id to it } }.toMap()
    }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 16.dp)) {
        items(edits, key = { it.id }, contentType = { "edit" }) { item ->
            val doc = docs[item.id]
            val path = item.tool?.path?.takeIf { it.isNotBlank() } ?: doc?.files?.firstOrNull()?.path.orEmpty()
            EditEntry(path = path, doc = doc, onOpen = { if (doc != null) onOpen(path, doc) })
        }
    }
}

@Composable
private fun EditEntry(path: String, doc: AgentDiffDoc?, onOpen: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val fonts = LocalAppFonts.current
    Column(Modifier.fillMaxWidth().clickable(enabled = doc != null, role = Role.Button, onClick = onOpen)) {
        Row(
            modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).padding(horizontal = 16.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                path,
                style = MaterialTheme.typography.bodySmall.mono(fonts),
                maxLines = 1,
                overflow = TextOverflow.StartEllipsis,
                modifier = Modifier.weight(1f),
            )
            if (doc != null) Counts(doc.adds.toLong(), doc.dels.toLong())
            Icon(G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
        }
        if (doc != null) DiffView(doc, wrap = false, maxRows = EDIT_ROWS, modifier = Modifier.padding(bottom = 8.dp))
        Box(Modifier.fillMaxWidth().height(1.dp).background(scheme.outlineVariant))
    }
}

/** The files the PC recorded for the turn that ended, read when shown; each opens its diff. */
@Composable
private fun TurnFiles(core: Core, pcId: String, threadId: String, turn: UInt, onOpen: (UInt, String) -> Unit) {
    var files by remember(threadId, turn) { mutableStateOf<AgentCall<AgentTurnFiles>?>(null) }
    LaunchedEffect(pcId, threadId, turn) { files = core.agentDiffFiles(pcId, threadId, AgentTurnRange.Turn(turn)) }
    when (val result = files) {
        null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp) }
        is AgentCall.Failed -> PaneNote(
            stringResource(if (result.failure == AgentFailure.UNSUPPORTED) R.string.agents_changes_missing else R.string.agents_failed_generic),
        )
        is AgentCall.Ok -> {
            val (withDiff, unrecorded) = remember(result.value) { AgentDiffModel.split(result.value.files) }
            if (withDiff.isEmpty() && unrecorded.isEmpty()) {
                PaneNote(stringResource(R.string.agents_beside_nothing_changed))
            } else {
                LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 16.dp)) {
                    items(withDiff, key = { "file:${it.path}" }, contentType = { "file" }) { file -> FileEntry(file, onOpen = { onOpen(turn, file.path) }) }
                    items(unrecorded, key = { "unrecorded:${it.path}" }, contentType = { "file" }) { file -> FileEntry(file, onOpen = null) }
                }
            }
        }
    }
}

@Composable
private fun FileEntry(file: AgentTurnFile, onOpen: (() -> Unit)?) {
    val scheme = MaterialTheme.colorScheme
    val fonts = LocalAppFonts.current
    Column(Modifier.fillMaxWidth().then(if (onOpen != null) Modifier.clickable(role = Role.Button, onClick = onOpen) else Modifier)) {
        Row(
            modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).padding(horizontal = 16.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                file.path,
                style = MaterialTheme.typography.bodySmall.mono(fonts),
                color = if (onOpen != null) scheme.onSurface else scheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.StartEllipsis,
                modifier = Modifier.weight(1f),
            )
            if (file.status != AgentFileStatus.UNRECORDED && !file.binary) Counts(file.adds.toLong(), file.dels.toLong())
            if (onOpen != null) Icon(G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(scheme.outlineVariant))
    }
}

// ---- Plan ----

@Composable
private fun PlanPane(plan: BesidePlan, planCard: @Composable (String) -> Unit) {
    when (plan) {
        is BesidePlan.Tasks -> TaskList(plan)
        is BesidePlan.Card -> Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 12.dp, vertical = 8.dp)) {
            planCard(plan.itemId)
        }
        BesidePlan.None -> PaneNote(stringResource(R.string.agents_beside_no_plan))
    }
}

/** "Tasks 2/5", then each step: done a check, the one under way a dot, the rest a ring. */
@Composable
private fun TaskList(tasks: BesidePlan.Tasks) {
    val scheme = MaterialTheme.colorScheme
    val done = AgentsModel.planDone(tasks.steps)
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 16.dp)) {
        item(key = "count", contentType = "label") {
            Text(
                stringResource(R.string.agents_tasks, done, tasks.steps.size),
                style = MaterialTheme.typography.labelLarge,
                color = scheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 20.dp, vertical = 6.dp),
            )
        }
        itemsIndexed(tasks.steps, key = { index, _ -> "step:$index" }, contentType = { _, _ -> "step" }) { index, step ->
            val current = tasks.current == index + 1
            val completed = step.status == AgentPlanStatus.COMPLETED
            Row(
                modifier = Modifier.fillMaxWidth().heightIn(min = 44.dp).padding(horizontal = 20.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Box(Modifier.size(20.dp), contentAlignment = Alignment.Center) {
                    when {
                        completed -> ThreadStatusMark(ThreadStatus.Done, size = 14.dp)
                        current -> ThreadStatusMark(ThreadStatus.Working, size = 10.dp)
                        else -> Box(Modifier.size(14.dp).border(1.5.dp, scheme.outline, CircleShape))
                    }
                }
                Text(
                    step.text,
                    style = MaterialTheme.typography.bodyMedium.copy(fontWeight = if (current) FontWeight.SemiBold else FontWeight.Normal),
                    color = if (completed) scheme.onSurfaceVariant else scheme.onSurface,
                )
            }
        }
    }
}

// ---- Requests ----

/** Everything waiting, oldest first; the dock's one with its buttons, the rest saying to answer that first. */
@Composable
private fun RequestsPane(
    requests: List<BesideRequest>,
    buttonsFor: String?,
    agentName: String,
    pcName: String,
    dockCard: @Composable (BesideRequest, Int, Int, Dp) -> Unit,
) {
    if (requests.isEmpty()) {
        PaneNote(stringResource(R.string.agents_beside_nothing_waiting))
        return
    }
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val cardMax = maxHeight * DOCK_CARD_SHARE
        val buttonsAt = requests.indexOfFirst { it.token == buttonsFor }
        Column(
            modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(start = 12.dp, end = 12.dp, top = 4.dp, bottom = 16.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            requests.forEachIndexed { index, request ->
                key(request.token) {
                    if (index == buttonsAt) {
                        dockCard(request, index + 1, requests.size, cardMax)
                    } else {
                        WaitingCard(request, index + 1, requests.size, agentName, pcName, afterButtons = buttonsAt in 0 until index)
                    }
                }
            }
        }
    }
}

/** A request or question waiting behind the one with buttons: what it is and "2 of 3", no buttons. */
@Composable
private fun WaitingCard(request: BesideRequest, position: Int, total: Int, agentName: String, pcName: String, afterButtons: Boolean) {
    val scheme = MaterialTheme.colorScheme
    val fonts = LocalAppFonts.current
    val tints = g7Tints()
    val shape = RoundedCornerShape(12.dp)
    Column(
        modifier = Modifier.fillMaxWidth().clip(shape).border(1.dp, scheme.outlineVariant, shape).padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        val icon = when (request) {
            is BesideRequest.Ask -> when (request.ask.kind) {
                AgentAskKind.COMMAND -> G7Icons.terminal
                AgentAskKind.EDIT -> G7Icons.edit
                AgentAskKind.OTHER -> G7Icons.help
            }
            is BesideRequest.Question -> G7Icons.help
        }
        val title = when (request) {
            is BesideRequest.Ask -> stringResource(
                when (request.ask.kind) {
                    AgentAskKind.COMMAND -> R.string.agents_dock_wants_command
                    AgentAskKind.EDIT -> R.string.agents_dock_wants_edit
                    AgentAskKind.OTHER -> R.string.agents_dock_wants_other
                },
                agentName,
            )
            is BesideRequest.Question -> stringResource(R.string.agents_question_asks, agentName)
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Icon(icon, contentDescription = null, tint = tints.amber, modifier = Modifier.size(16.dp))
            Text(
                title,
                style = MaterialTheme.typography.titleSmall.copy(fontWeight = FontWeight.SemiBold),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            Text(stringResource(R.string.agents_dock_count, position, total), style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant, maxLines = 1)
        }
        val box = Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(scheme.surface).padding(horizontal = 10.dp, vertical = 8.dp)
        when (request) {
            is BesideRequest.Ask -> {
                val ask = request.ask
                // An edit names its file here; its change is read in full once it's the one with buttons.
                val body = when (ask.kind) {
                    AgentAskKind.EDIT -> ask.path?.takeIf { it.isNotBlank() } ?: ask.title
                    AgentAskKind.COMMAND, AgentAskKind.OTHER -> ask.command?.takeIf { it.isNotBlank() } ?: ask.title
                }
                Text(body, style = MaterialTheme.typography.bodySmall.mono(fonts), maxLines = 4, overflow = TextOverflow.Ellipsis, modifier = box)
            }
            is BesideRequest.Question -> {
                val question = request.question
                // A secret question's words never reach the phone.
                val text = if (question.secret) stringResource(R.string.agents_question_answer_on, pcName) else question.asks.firstOrNull()?.text.orEmpty()
                if (text.isNotBlank()) Text(text, style = MaterialTheme.typography.bodyMedium, maxLines = 4, overflow = TextOverflow.Ellipsis, modifier = box)
            }
        }
        if (afterButtons) {
            Text(stringResource(R.string.agents_beside_answer_above), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
        }
    }
}
