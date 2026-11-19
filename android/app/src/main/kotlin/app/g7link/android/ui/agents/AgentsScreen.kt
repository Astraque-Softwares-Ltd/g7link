// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import android.text.format.DateUtils
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.theme.G7Header
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.G7Row
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.G7SectionLabel
import app.g7link.android.ui.theme.G7StatusDot
import app.g7link.android.ui.theme.G7Tabs
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.g7Tints
import app.g7link.core.AgentAsk
import app.g7link.core.AgentCatalog
import app.g7link.core.AgentInfo
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** How far the thread list got. */
private enum class ListLoad { LOADING, READY, NOT_ALLOWED, UNAVAILABLE, UNREACHABLE, FAILED }

/** The list's filters (the reconcile file): Recent holds done, stopped, paused and failed threads. */
private enum class ThreadFilter {
    ALL,
    NEEDS_YOU,
    WORKING,
    RECENT,
    ;

    fun matches(thread: AgentThread): Boolean = when (this) {
        ALL -> true
        NEEDS_YOU -> thread.state == AgentThreadState.WAITING || thread.asks > 0u
        WORKING -> thread.state == AgentThreadState.WORKING && thread.asks == 0u
        RECENT -> !AgentsModel.running(thread)
    }
}

/**
 * A PC's agent threads: requests waiting first, then the newest, with the
 * states the phone can be in before any thread shows (not allowed yet, the PC
 * out of reach, nothing signed in, no folders).
 */
@Composable
fun AgentsScreen(
    device: Device,
    core: Core,
    onOpenThread: (String) -> Unit,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val all by core.agents.collectAsStateWithLifecycle()
    val agents = all[device.id] ?: AgentsState()
    val catalog = agents.catalog
    var load by remember(device.id) { mutableStateOf(ListLoad.LOADING) }
    var attempt by remember(device.id) { mutableIntStateOf(0) }
    var filter by rememberSaveable { mutableStateOf(ThreadFilter.ALL) }
    var newThread by rememberSaveable { mutableStateOf(false) }
    val hasCatalog = catalog != null

    // Again when the PC allows this phone (its catalog arrives) or comes back.
    LaunchedEffect(device.id, device.online, hasCatalog, attempt) {
        if (!device.online) {
            load = ListLoad.UNREACHABLE
            return@LaunchedEffect
        }
        if (load != ListLoad.READY) load = ListLoad.LOADING
        load = when (val result = core.agentThreads(device.id)) {
            is AgentCall.Ok -> ListLoad.READY
            is AgentCall.Failed -> when (result.failure) {
                AgentFailure.DENIED -> ListLoad.NOT_ALLOWED
                AgentFailure.UNSUPPORTED -> ListLoad.UNAVAILABLE
                AgentFailure.OFFLINE, AgentFailure.TIMEOUT -> ListLoad.UNREACHABLE
                AgentFailure.NOT_FOUND, AgentFailure.OTHER -> ListLoad.FAILED
            }
        }
    }

    val canStart = catalog != null && agents.readyAgents.isNotEmpty() && catalog.folders.isNotEmpty()
    val retry: () -> Unit = { attempt++ }
    Column(modifier.fillMaxSize()) {
        G7Header(
            title = stringResource(R.string.agents_title),
            onBack = onBack,
            note = device.name,
            noteColor = if (device.online) MaterialTheme.colorScheme.onSurfaceVariant else MaterialTheme.colorScheme.error,
        ) {
            if (load == ListLoad.READY && canStart) {
                G7RowAction(
                    label = stringResource(R.string.agents_new_thread),
                    accent = true,
                    onClick = { newThread = true },
                )
            }
        }
        when (load) {
            ListLoad.LOADING -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                CircularProgressIndicator(Modifier.size(28.dp), strokeWidth = 2.dp)
            }
            ListLoad.UNREACHABLE -> AgentsEmptyState(
                title = stringResource(R.string.agents_unreachable_title, device.name),
                body = stringResource(R.string.agents_unreachable_body),
                action = stringResource(R.string.agents_try_again),
                onAction = retry,
            )
            ListLoad.NOT_ALLOWED -> AgentsEmptyState(
                title = stringResource(R.string.agents_access_title, device.name),
                body = stringResource(R.string.agents_access_body),
                action = stringResource(R.string.agents_try_again),
                onAction = retry,
            )
            ListLoad.UNAVAILABLE -> AgentsEmptyState(
                title = stringResource(R.string.agents_unavailable, device.name),
                action = stringResource(R.string.agents_try_again),
                onAction = retry,
            )
            ListLoad.FAILED -> AgentsEmptyState(
                title = stringResource(R.string.agents_failed_generic),
                action = stringResource(R.string.agents_try_again),
                onAction = retry,
            )
            ListLoad.READY -> if (catalog == null) {
                AgentsEmptyState(
                    title = stringResource(R.string.agents_unavailable, device.name),
                    action = stringResource(R.string.agents_try_again),
                    onAction = retry,
                )
            } else {
                ThreadList(
                    pcName = device.name,
                    agents = agents,
                    catalog = catalog,
                    filter = filter,
                    onFilter = { filter = it },
                    canStart = canStart,
                    onNewThread = { newThread = true },
                    onOpenThread = onOpenThread,
                )
            }
        }
    }

    if (newThread && catalog != null && canStart) {
        NewThreadSheet(
            device = device,
            core = core,
            catalog = catalog,
            threads = agents.threads,
            onStarted = { id ->
                newThread = false
                onOpenThread(id)
            },
            onDismiss = { newThread = false },
        )
    }
}

@Composable
private fun ThreadList(
    pcName: String,
    agents: AgentsState,
    catalog: AgentCatalog,
    filter: ThreadFilter,
    onFilter: (ThreadFilter) -> Unit,
    canStart: Boolean,
    onNewThread: () -> Unit,
    onOpenThread: (String) -> Unit,
) {
    val now = rememberNow()
    val shown = remember(agents.threads, filter) { agents.threads.filter { filter.matches(it) } }
    LazyColumn(
        modifier = Modifier.fillMaxSize(),
        contentPadding = PaddingValues(start = 12.dp, end = 12.dp, bottom = 16.dp),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        if (agents.readyAgents.isEmpty()) {
            item(key = "none_ready", contentType = "setup") {
                Column(Modifier.padding(bottom = 8.dp)) {
                    SetupNote(stringResource(R.string.agents_none_ready, pcName))
                    AgentRows(catalog.agents, selected = null, onSelect = {})
                }
            }
        } else if (catalog.folders.isEmpty()) {
            item(key = "no_folders", contentType = "setup") {
                SetupNote(stringResource(R.string.agents_no_folders, pcName))
            }
        }
        if (agents.threads.isEmpty()) {
            item(key = "empty", contentType = "empty") {
                Column(
                    Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 24.dp),
                    verticalArrangement = Arrangement.spacedBy(16.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Text(
                        stringResource(R.string.agents_empty),
                        style = MaterialTheme.typography.bodyLarge,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        textAlign = TextAlign.Center,
                    )
                    if (canStart) G7PrimaryButton(label = stringResource(R.string.agents_new_thread), onClick = onNewThread)
                }
            }
        } else {
            item(key = "filters", contentType = "filters") {
                G7Tabs(
                    options = listOf(
                        ThreadFilter.ALL to stringResource(R.string.agents_filter_all),
                        ThreadFilter.NEEDS_YOU to stringResource(R.string.agents_state_waiting),
                        ThreadFilter.WORKING to stringResource(R.string.agents_state_working),
                        ThreadFilter.RECENT to stringResource(R.string.agents_filter_recent),
                    ),
                    selected = filter,
                    onSelect = onFilter,
                    modifier = Modifier.fillMaxWidth().padding(bottom = 8.dp),
                )
            }
            items(shown, key = { it.id }, contentType = { "thread" }) { thread ->
                ThreadRow(
                    thread = thread,
                    agentName = agents.agentName(thread.agent),
                    ask = agents.asksFor(thread.id).firstOrNull(),
                    now = now,
                    onOpen = { onOpenThread(thread.id) },
                )
            }
        }
    }
}

@Composable
private fun ThreadRow(thread: AgentThread, agentName: String, ask: AgentAsk?, now: Long, onOpen: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val status = AgentsModel.status(thread)
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(scheme.surfaceContainer)
            .clickable(role = Role.Button, onClick = onOpen)
            .padding(horizontal = 14.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        StatusLine(status = status, trailing = relativeTime(thread.updatedMs, now))
        Text(
            thread.title,
            style = MaterialTheme.typography.bodyLarge,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
        )
        Text(
            "$agentName · ${thread.folder}",
            style = MaterialTheme.typography.bodySmall,
            color = scheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        if (status is ThreadStatus.NeedsYou) {
            // A glance at what it asks; the whole of it is read in the thread before it can be allowed.
            val preview = ask?.let { AgentsModel.askBody(it) ?: it.title }?.lineSequence()?.firstOrNull()
            if (!preview.isNullOrBlank()) {
                Text(
                    preview,
                    style = MaterialTheme.typography.bodySmall,
                    fontFamily = LocalAppFonts.current.mono,
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

/** The new-thread sheet: the task, the agent (when there's a choice), the folder. Always asks before each change. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun NewThreadSheet(
    device: Device,
    core: Core,
    catalog: AgentCatalog,
    threads: List<AgentThread>,
    onStarted: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    val resources = LocalResources.current
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val ready = catalog.agents.filter { it.installed && it.signedIn }
    var agentId by rememberSaveable { mutableStateOf(ready.firstOrNull()?.id) }
    var folderId by rememberSaveable { mutableStateOf(catalog.folders.firstOrNull()?.id) }
    var prompt by rememberSaveable { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val agent = ready.firstOrNull { it.id == agentId }
    val folder = catalog.folders.firstOrNull { it.id == folderId }
    val tooLong = AgentsModel.tooLong(prompt)
    val canStart = !busy && agent != null && folder != null && AgentsModel.fits(prompt)

    fun start() {
        if (!canStart || agent == null || folder == null) return
        busy = true
        error = null
        scope.launch {
            when (val result = core.agentStart(device.id, agent.id, folder.id, prompt.trim())) {
                is AgentCall.Ok -> onStarted(result.value.id)
                is AgentCall.Failed -> error = when (result.failure) {
                    AgentFailure.DENIED -> resources.getString(R.string.agents_access_title, device.name)
                    AgentFailure.UNSUPPORTED -> resources.getString(R.string.agents_unavailable, device.name)
                    AgentFailure.OFFLINE, AgentFailure.TIMEOUT -> resources.getString(R.string.agents_unreachable_title, device.name)
                    AgentFailure.NOT_FOUND, AgentFailure.OTHER -> {
                        // The PC's BUSY reaches the phone as a plain failure: the caps (4 in all, 3 per agent) say which.
                        val running = threads.filter { AgentsModel.running(it) }
                        when {
                            running.size >= MAX_RUNNING -> resources.getString(R.string.agents_start_busy, device.name)
                            running.count { it.agent == agent.id } >= MAX_RUNNING_PER_AGENT ->
                                resources.getString(R.string.agents_start_busy_agent, device.name, agent.name)
                            else -> resources.getString(R.string.agents_failed_generic)
                        }
                    }
                }
            }
            busy = false
        }
    }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        containerColor = MaterialTheme.colorScheme.surfaceContainerHigh,
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 20.dp)
                .padding(bottom = 20.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(stringResource(R.string.agents_new_thread_on, device.name), style = MaterialTheme.typography.titleLarge)
            Text(
                stringResource(R.string.agents_new_thread_note),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            AgentTextField(
                value = prompt,
                onValueChange = { prompt = it },
                hint = stringResource(R.string.agents_prompt_hint),
                minLines = 3,
                maxLines = 8,
                background = MaterialTheme.colorScheme.surfaceContainer,
            )
            if (tooLong) {
                Text(stringResource(R.string.agents_too_long), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
            }
            if (catalog.agents.size > 1) {
                Column {
                    G7SectionLabel(stringResource(R.string.agents_pick_agent))
                    AgentRows(catalog.agents, selected = agentId, onSelect = { agentId = it })
                }
            }
            Column {
                G7SectionLabel(stringResource(R.string.agents_pick_folder))
                val tints = g7Tints()
                for (each in catalog.folders) {
                    G7Row(
                        label = each.label,
                        icon = G7Icons.folder,
                        tint = tints.amber,
                        role = Role.RadioButton,
                        active = each.id == folderId,
                        onClick = { folderId = each.id },
                    )
                }
            }
            Text(
                stringResource(R.string.agents_supervised_note),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            error?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.error) }
            if (busy) {
                Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
                }
            } else {
                G7PrimaryButton(label = stringResource(R.string.agents_start), enabled = canStart, onClick = ::start)
            }
        }
    }
}

/** The PC's agents, each with whether it can run: signed in, signed out (and the command for it), not installed. */
@Composable
private fun AgentRows(agents: List<AgentInfo>, selected: String?, onSelect: (String) -> Unit) {
    for (agent in agents) {
        val ready = agent.installed && agent.signedIn
        G7Row(
            label = agent.name,
            sublabel = when {
                !agent.installed -> stringResource(R.string.agents_not_installed)
                !agent.signedIn -> stringResource(R.string.agents_signed_out) + "\n" + stringResource(R.string.agents_login_hint, agent.loginHint)
                agent.auth == "api_key" -> stringResource(R.string.agents_api_key)
                else -> stringResource(R.string.agents_signed_in)
            },
            role = Role.RadioButton,
            enabled = ready,
            active = ready && agent.id == selected,
            onClick = if (ready) ({ onSelect(agent.id) }) else null,
        )
    }
}

@Composable
private fun SetupNote(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(horizontal = 12.dp, vertical = 8.dp),
    )
}

/** A centred state in place of the list: what's wrong, what to do, and a way to try again. */
@Composable
internal fun AgentsEmptyState(title: String, body: String? = null, action: String? = null, onAction: () -> Unit = {}) {
    Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(title, style = MaterialTheme.typography.titleMedium, textAlign = TextAlign.Center)
            if (body != null) {
                Text(
                    body,
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    textAlign = TextAlign.Center,
                )
            }
            if (action != null) {
                Spacer(Modifier.size(4.dp))
                G7RaisedButton(label = action, onClick = onAction, modifier = Modifier.padding(horizontal = 24.dp))
            }
        }
    }
}

/** A dot in the state's colour, the state in words, and something after it (a time, the agent and folder). */
@Composable
internal fun StatusLine(status: ThreadStatus, trailing: String?, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val (dot, word, wordColor) = when (status) {
        ThreadStatus.NeedsYou -> Triple(tints.amber, stringResource(R.string.agents_state_waiting), tints.amber)
        ThreadStatus.Working -> Triple(tints.blue, stringResource(R.string.agents_state_working), scheme.onSurfaceVariant)
        ThreadStatus.Done -> Triple(tints.teal, stringResource(R.string.agents_state_done), scheme.onSurfaceVariant)
        ThreadStatus.Stopped -> Triple(scheme.outline, stringResource(R.string.agents_state_interrupted), scheme.onSurfaceVariant)
        is ThreadStatus.Paused -> Triple(scheme.outline, stringResource(R.string.agents_state_paused), scheme.onSurfaceVariant)
        is ThreadStatus.Failed -> Triple(tints.red, stringResource(R.string.agents_state_failed), scheme.error)
    }
    Row(
        modifier = modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        G7StatusDot(dot)
        Text(word, style = MaterialTheme.typography.labelMedium, color = wordColor, maxLines = 1)
        if (!trailing.isNullOrBlank()) {
            Text(
                trailing,
                style = MaterialTheme.typography.labelMedium,
                color = scheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
                textAlign = TextAlign.End,
            )
        }
    }
}

/** A paused or failed thread's line: the usage limit and when it resets, or the CLI's message. */
@Composable
internal fun statusDetail(status: ThreadStatus): String? {
    val context = LocalContext.current
    return when (status) {
        is ThreadStatus.Paused -> status.resetsMs?.let {
            stringResource(R.string.agents_usage_resets, android.text.format.DateFormat.getTimeFormat(context).format(java.util.Date(it)))
        } ?: stringResource(R.string.agents_usage_limit)
        is ThreadStatus.Failed -> status.message
        else -> null
    }
}

/** The text box for a task or a reply. */
@Composable
internal fun AgentTextField(
    value: String,
    onValueChange: (String) -> Unit,
    hint: String,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    minLines: Int = 1,
    maxLines: Int = 6,
    background: Color = MaterialTheme.colorScheme.surfaceContainer,
) {
    val scheme = MaterialTheme.colorScheme
    Box(
        modifier = modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(background)
            .padding(horizontal = 12.dp, vertical = 12.dp),
    ) {
        if (value.isEmpty()) {
            Text(hint, style = MaterialTheme.typography.bodyLarge, color = scheme.onSurfaceVariant, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
        BasicTextField(
            value = value,
            onValueChange = onValueChange,
            enabled = enabled,
            textStyle = MaterialTheme.typography.bodyLarge.copy(color = scheme.onSurface),
            cursorBrush = SolidColor(scheme.primary),
            minLines = minLines,
            maxLines = maxLines,
            modifier = Modifier
                .fillMaxWidth()
                .semantics { contentDescription = hint },
        )
    }
}

/** The time now, a little stale: enough for "2 min ago". */
@Composable
internal fun rememberNow(stepMs: Long = 30_000L): Long {
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(stepMs) {
        while (true) {
            delay(stepMs)
            now = System.currentTimeMillis()
        }
    }
    return now
}

internal fun relativeTime(ms: Long, now: Long): String =
    DateUtils.getRelativeTimeSpanString(ms, now, DateUtils.MINUTE_IN_MILLIS, DateUtils.FORMAT_ABBREV_RELATIVE).toString()

/** The PC's limits on running threads (the reconcile file): 4 in all, 3 per agent. */
private const val MAX_RUNNING = 4
private const val MAX_RUNNING_PER_AGENT = 3
