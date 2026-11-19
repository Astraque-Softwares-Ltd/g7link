// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import android.text.format.DateFormat
import androidx.compose.foundation.ScrollState
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
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
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.repeatOnLifecycle
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.theme.G7Header
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.g7Tints
import app.g7link.core.AgentAsk
import app.g7link.core.AgentAskKind
import app.g7link.core.AgentAskOutcome
import app.g7link.core.AgentItem
import app.g7link.core.AgentItemKind
import app.g7link.core.AgentThread
import app.g7link.core.AgentTool
import app.g7link.core.AgentToolStatus
import java.util.Date
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch

/** How far opening the thread got. */
private enum class ThreadLoad { LOADING, READY, NOT_ALLOWED, UNAVAILABLE, UNREACHABLE, GONE, FAILED }

/**
 * One thread on a PC: its transcript, the requests waiting for an answer
 * pinned above the reply box, and Stop while it works. Nothing is written to
 * disk; leaving drops the transcript.
 */
@Composable
fun AgentThreadScreen(
    device: Device,
    core: Core,
    threadId: String,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val resources = LocalResources.current
    val all by core.agents.collectAsStateWithLifecycle()
    val agents = all[device.id] ?: AgentsState()
    val thread = agents.thread(threadId)
    val open = agents.open?.takeIf { it.threadId == threadId }
    val asks = remember(agents.asks, threadId) { agents.asksFor(threadId) }
    val hasCatalog = agents.catalog != null
    var load by remember(threadId) { mutableStateOf(ThreadLoad.LOADING) }
    var attempt by remember(threadId) { mutableIntStateOf(0) }
    val scope = rememberCoroutineScope()
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val now = rememberNow()

    // Leaving the screen drops the transcript.
    DisposableEffect(device.id, threadId) {
        onDispose { core.agentClosed(device.id, threadId) }
    }
    // While it's in front the PC sends its text and its notifications wait; read it again on coming back.
    LaunchedEffect(device.id, threadId, device.online, hasCatalog, attempt, lifecycle) {
        if (!device.online) {
            load = ThreadLoad.UNREACHABLE
            return@LaunchedEffect
        }
        lifecycle.repeatOnLifecycle(Lifecycle.State.RESUMED) {
            try {
                load = when (val result = core.agentShown(device.id, threadId)) {
                    is AgentCall.Ok -> ThreadLoad.READY
                    is AgentCall.Failed -> when (result.failure) {
                        AgentFailure.DENIED -> ThreadLoad.NOT_ALLOWED
                        AgentFailure.UNSUPPORTED -> ThreadLoad.UNAVAILABLE
                        AgentFailure.NOT_FOUND -> ThreadLoad.GONE
                        AgentFailure.OFFLINE, AgentFailure.TIMEOUT -> ThreadLoad.UNREACHABLE
                        AgentFailure.OTHER -> ThreadLoad.FAILED
                    }
                }
                awaitCancellation()
            } finally {
                core.agentHidden(device.id, threadId)
            }
        }
    }

    val agentName = thread?.let { agents.agentName(it.agent) }.orEmpty()
    val status = thread?.let { AgentsModel.status(it) }
    val running = thread != null && AgentsModel.running(thread)
    var stopping by remember(threadId) { mutableStateOf(false) }
    LaunchedEffect(running) { if (!running) stopping = false }
    var draft by rememberSaveable(threadId) { mutableStateOf("") }
    var sending by remember(threadId) { mutableStateOf(false) }
    var loadingEarlier by remember(threadId) { mutableStateOf(false) }
    val retry: () -> Unit = { attempt++ }

    val stop: () -> Unit = {
        stopping = true
        scope.launch {
            if (core.agentCancel(device.id, threadId) is AgentCall.Failed) {
                stopping = false
                core.say(resources.getString(R.string.agents_failed_generic))
            }
        }
    }
    val answer: (AgentAsk, Boolean) -> Unit = { ask, allow ->
        scope.launch {
            val result = core.agentAnswer(device.id, ask.token, allow)
            if (result is AgentCall.Failed) {
                // Gone: someone answered first, or it lapsed.
                val message = if (result.failure == AgentFailure.NOT_FOUND) R.string.agents_answered_elsewhere else R.string.agents_failed_generic
                core.say(resources.getString(message))
            }
        }
    }
    val send: () -> Unit = {
        val text = draft.trim()
        if (AgentsModel.fits(text) && !sending) {
            sending = true
            scope.launch {
                when (val result = core.agentSend(device.id, threadId, text)) {
                    is AgentCall.Ok -> draft = ""
                    is AgentCall.Failed -> core.say(
                        when (result.failure) {
                            AgentFailure.DENIED -> resources.getString(R.string.agents_access_title, device.name)
                            AgentFailure.UNSUPPORTED -> resources.getString(R.string.agents_unavailable, device.name)
                            AgentFailure.NOT_FOUND -> resources.getString(R.string.agents_thread_gone)
                            AgentFailure.OFFLINE, AgentFailure.TIMEOUT -> resources.getString(R.string.agents_unreachable_title, device.name)
                            // The PC's BUSY: it started working again (from the PC) before this reached it.
                            AgentFailure.OTHER -> if (running) {
                                resources.getString(R.string.agents_reply_when_stops, agentName)
                            } else {
                                resources.getString(R.string.agents_failed_generic)
                            }
                        },
                    )
                }
                sending = false
            }
        }
    }
    val loadEarlier: () -> Unit = {
        val first = open?.items?.firstOrNull()?.seq
        if (first != null && !loadingEarlier) {
            loadingEarlier = true
            scope.launch {
                core.agentRead(device.id, threadId, first)
                loadingEarlier = false
            }
        }
    }

    Column(modifier.fillMaxSize().imePadding()) {
        G7Header(title = thread?.title.orEmpty(), onBack = onBack) {
            if (running) {
                G7RowAction(label = stringResource(R.string.agents_stop), enabled = !stopping, onClick = stop)
            }
        }
        if (thread != null && status != null) {
            StatusLine(
                status = status,
                trailing = "$agentName · ${thread.folder}",
                modifier = Modifier.padding(start = 16.dp, end = 16.dp, bottom = 8.dp),
            )
        }
        when {
            load == ThreadLoad.GONE || open?.removed == true -> AgentsEmptyState(title = stringResource(R.string.agents_thread_gone))
            load == ThreadLoad.UNREACHABLE -> AgentsEmptyState(
                title = stringResource(R.string.agents_unreachable_title, device.name),
                body = stringResource(R.string.agents_unreachable_body),
                action = stringResource(R.string.agents_try_again),
                onAction = retry,
            )
            load == ThreadLoad.NOT_ALLOWED -> AgentsEmptyState(
                title = stringResource(R.string.agents_access_title, device.name),
                body = stringResource(R.string.agents_access_body),
                action = stringResource(R.string.agents_try_again),
                onAction = retry,
            )
            load == ThreadLoad.UNAVAILABLE -> AgentsEmptyState(
                title = stringResource(R.string.agents_unavailable, device.name),
                action = stringResource(R.string.agents_try_again),
                onAction = retry,
            )
            load == ThreadLoad.FAILED -> AgentsEmptyState(
                title = stringResource(R.string.agents_failed_generic),
                action = stringResource(R.string.agents_try_again),
                onAction = retry,
            )
            open == null || !open.loaded -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                CircularProgressIndicator(Modifier.size(28.dp), strokeWidth = 2.dp)
            }
            open != null -> {
                Transcript(
                    open = open,
                    loadingEarlier = loadingEarlier,
                    onLoadEarlier = loadEarlier,
                    modifier = Modifier.weight(1f),
                )
                if (thread != null && status != null) {
                    EndNote(status, thread, agents, device.name)
                }
                if (asks.isNotEmpty()) {
                    AskCards(
                        asks = asks,
                        answering = agents.answering,
                        agentName = agentName,
                        folder = thread?.folder,
                        pcName = device.name,
                        now = now,
                        onAnswer = answer,
                    )
                }
                Composer(
                    thread = thread,
                    status = status,
                    agentName = agentName,
                    now = now,
                    draft = draft,
                    onDraft = { draft = it },
                    sending = sending,
                    onSend = send,
                )
            }
        }
    }
}

@Composable
private fun Transcript(
    open: OpenThread,
    loadingEarlier: Boolean,
    onLoadEarlier: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val entries = remember(open) { AgentsModel.transcript(open) }
    val listState = rememberLazyListState()
    // Follows new text while the end is in view; scrolling up stops that, scrolling back down resumes it.
    var follow by remember { mutableStateOf(true) }
    LaunchedEffect(listState) {
        snapshotFlow { listState.isScrollInProgress to listState.canScrollForward }
            .collect { scroll -> if (scroll.first) follow = !scroll.second }
    }
    val earlier = open.moreBefore && open.items.isNotEmpty()
    val last = entries.lastOrNull()
    val lastLength = (last as? TranscriptEntry.Streaming)?.text?.length ?: 0
    LaunchedEffect(entries.size, last?.key, lastLength) {
        if (follow && entries.isNotEmpty()) {
            listState.scrollToItem(entries.size - 1 + if (earlier) 1 else 0)
            listState.scrollBy(END_SCROLL)
        }
    }
    LazyColumn(
        state = listState,
        modifier = modifier.fillMaxWidth(),
        contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        if (earlier) {
            item(key = "earlier", contentType = "earlier") {
                Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                    if (loadingEarlier) {
                        CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                    } else {
                        G7RowAction(label = stringResource(R.string.agents_load_earlier), onClick = onLoadEarlier)
                    }
                }
            }
        }
        items(entries, key = { it.key }, contentType = { contentTypeOf(it) }) { entry ->
            Column(Modifier.fillMaxWidth()) {
                when (entry) {
                    is TranscriptEntry.Item -> ItemView(entry.item)
                    is TranscriptEntry.Streaming -> AssistantText(entry.text, streaming = true)
                    is TranscriptEntry.Resolved -> ResolvedRow(entry.ask)
                }
            }
        }
    }
}

private fun contentTypeOf(entry: TranscriptEntry): String = when (entry) {
    is TranscriptEntry.Item -> "item_${entry.item.kind.name}"
    // The same as the finished item that replaces it.
    is TranscriptEntry.Streaming -> "item_${AgentItemKind.ASSISTANT.name}"
    is TranscriptEntry.Resolved -> "resolved"
}

@Composable
private fun ColumnScope.ItemView(item: AgentItem) {
    val scheme = MaterialTheme.colorScheme
    when (item.kind) {
        AgentItemKind.USER -> Row(
            modifier = Modifier.fillMaxWidth().padding(start = 48.dp),
            horizontalArrangement = Arrangement.End,
        ) {
            SelectionContainer {
                Text(
                    item.text.orEmpty(),
                    style = MaterialTheme.typography.bodyLarge,
                    modifier = Modifier
                        .clip(RoundedCornerShape(12.dp))
                        .background(scheme.surfaceContainerHigh)
                        .padding(horizontal = 12.dp, vertical = 8.dp),
                )
            }
        }
        AgentItemKind.ASSISTANT -> AssistantText(item.text.orEmpty(), streaming = false)
        AgentItemKind.TOOL -> {
            val tool = item.tool
            if (tool != null) ToolRow(tool) else NoticeText(item.text.orEmpty())
        }
        AgentItemKind.NOTICE -> NoticeText(item.text.orEmpty())
    }
    if (item.truncated) {
        Text(
            stringResource(R.string.agents_truncated),
            style = MaterialTheme.typography.labelSmall,
            color = scheme.onSurfaceVariant,
            modifier = Modifier.align(if (item.kind == AgentItemKind.USER) Alignment.End else Alignment.Start),
        )
    }
}

@Composable
private fun AssistantText(text: String, streaming: Boolean) {
    SelectionContainer {
        Text(
            // A caret while it's still coming.
            if (streaming) "$text▍" else text,
            style = MaterialTheme.typography.bodyLarge,
            modifier = Modifier.fillMaxWidth(),
        )
    }
}

@Composable
private fun NoticeText(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        textAlign = TextAlign.Center,
        modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp),
    )
}

/** A tool step: its state as a glyph, its title, and, opened, what it ran or changed and the end of its output. */
@Composable
private fun ToolRow(tool: AgentTool) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    var expanded by rememberSaveable { mutableStateOf(false) }
    val detail = tool.detail?.takeIf { it.isNotBlank() }
    val output = tool.output?.takeIf { it.isNotBlank() }
    val hasMore = detail != null || output != null
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .clickable(enabled = hasMore, role = Role.Button) { expanded = !expanded }
            .padding(horizontal = 4.dp, vertical = 6.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Box(Modifier.size(18.dp), contentAlignment = Alignment.Center) {
                when (tool.status) {
                    AgentToolStatus.RUNNING -> CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp, color = tints.blue)
                    AgentToolStatus.OK -> Icon(G7Icons.check, contentDescription = null, tint = tints.teal, modifier = Modifier.size(16.dp))
                    AgentToolStatus.ERROR -> Icon(G7Icons.close, contentDescription = null, tint = tints.red, modifier = Modifier.size(16.dp))
                    AgentToolStatus.DENIED -> Icon(G7Icons.close, contentDescription = null, tint = scheme.outline, modifier = Modifier.size(16.dp))
                }
            }
            Column(Modifier.weight(1f)) {
                Text(tool.title, style = MaterialTheme.typography.bodyMedium, maxLines = 2, overflow = TextOverflow.Ellipsis)
                if (detail != null && !expanded) {
                    Text(
                        detail.lineSequence().first(),
                        style = MaterialTheme.typography.bodySmall,
                        fontFamily = LocalAppFonts.current.mono,
                        color = scheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
            val word = when (tool.status) {
                AgentToolStatus.RUNNING -> stringResource(R.string.agents_step_running)
                AgentToolStatus.DENIED -> stringResource(R.string.agents_step_denied)
                AgentToolStatus.ERROR -> stringResource(R.string.agents_step_failed)
                AgentToolStatus.OK -> null
            }
            if (word != null) {
                Text(
                    word,
                    style = MaterialTheme.typography.labelMedium,
                    color = if (tool.status == AgentToolStatus.ERROR) scheme.error else scheme.onSurfaceVariant,
                    maxLines = 1,
                )
            }
            if (hasMore) {
                Icon(
                    if (expanded) G7Icons.chevronDown else G7Icons.chevronRight,
                    contentDescription = null,
                    tint = scheme.onSurfaceVariant,
                    modifier = Modifier.size(16.dp),
                )
            }
        }
        if (expanded) {
            if (detail != null) CodeBlock(detail, diff = tool.name in EDIT_TOOLS)
            if (output != null) CodeBlock(output, diff = false)
        }
    }
}

/** A request answered here or elsewhere, collapsed into the transcript. */
@Composable
private fun ResolvedRow(ask: ResolvedAsk) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val word = when (ask.outcome) {
        AgentAskOutcome.ALLOWED -> ask.byName?.let { stringResource(R.string.agents_answered_allowed_on, it) }
            ?: stringResource(R.string.agents_answered_allowed)
        AgentAskOutcome.DENIED -> ask.byName?.let { stringResource(R.string.agents_answered_denied_on, it) }
            ?: stringResource(R.string.agents_answered_denied)
        AgentAskOutcome.EXPIRED -> stringResource(R.string.agents_answered_expired)
        AgentAskOutcome.CANCELLED -> stringResource(R.string.agents_answered_cancelled)
    }
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(
            if (ask.outcome == AgentAskOutcome.ALLOWED) G7Icons.check else G7Icons.close,
            contentDescription = null,
            tint = if (ask.outcome == AgentAskOutcome.ALLOWED) tints.teal else scheme.outline,
            modifier = Modifier.size(16.dp),
        )
        Text(
            "$word · ${ask.title}",
            style = MaterialTheme.typography.bodySmall,
            color = scheme.onSurfaceVariant,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** Why a turn ended badly: the usage limit, or the CLI's own message (and the sign-in command when it's signed out). */
@Composable
private fun EndNote(status: ThreadStatus, thread: AgentThread, agents: AgentsState, pcName: String) {
    val scheme = MaterialTheme.colorScheme
    when (status) {
        is ThreadStatus.Paused -> Text(
            statusDetail(status).orEmpty(),
            style = MaterialTheme.typography.bodyMedium,
            modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
        )
        is ThreadStatus.Failed -> Column(
            modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            if (status.login) {
                val hint = agents.catalog?.agents?.firstOrNull { it.id == thread.agent }?.loginHint?.takeIf { it.isNotBlank() }
                    ?: if (thread.agent == "codex") "codex login" else "claude auth login"
                Text(
                    stringResource(R.string.agents_login_failed, agents.agentName(thread.agent), hint, pcName),
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
            status.message?.let { CodeBlock(it, diff = false) }
            Text(
                stringResource(R.string.agents_failed_fix, pcName),
                style = MaterialTheme.typography.bodySmall,
                color = scheme.onSurfaceVariant,
            )
        }
        else -> Unit
    }
}

/** The requests waiting in this thread, oldest first, above the reply box. */
@Composable
private fun AskCards(
    asks: List<AgentAsk>,
    answering: Set<String>,
    agentName: String,
    folder: String?,
    pcName: String,
    now: Long,
    onAnswer: (AgentAsk, Boolean) -> Unit,
) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(max = 460.dp)
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 12.dp, vertical = 6.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        for (ask in asks) {
            key(ask.token) {
                AskCard(
                    ask = ask,
                    answering = ask.token in answering,
                    agentName = agentName,
                    folder = folder,
                    pcName = pcName,
                    now = now,
                    onAnswer = { allow -> onAnswer(ask, allow) },
                )
            }
        }
    }
}

/**
 * A request: what it is, the whole command or change (scrolling, never cut
 * short), the agent's own reason set apart as its words, where it runs, and
 * Deny then Allow. Allow waits until the command or change has been scrolled
 * to its end, and is never offered for one too long to show in full.
 */
@Composable
private fun AskCard(
    ask: AgentAsk,
    answering: Boolean,
    agentName: String,
    folder: String?,
    pcName: String,
    now: Long,
    onAnswer: (Boolean) -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val body = remember(ask) { AgentsModel.askBody(ask) }
    val scroll = rememberScrollState()
    var seenAll by remember(ask.token) { mutableStateOf(body == null) }
    LaunchedEffect(ask.token, body) {
        if (body == null) return@LaunchedEffect
        // Laid out (no longer MAX_VALUE) and at the end: everything has been on screen.
        snapshotFlow { scroll.maxValue != Int.MAX_VALUE && scroll.value >= scroll.maxValue }.first { it }
        seenAll = true
    }
    val canAllow = seenAll && !ask.truncated
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(scheme.surfaceContainer)
            .padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Icon(
                when (ask.kind) {
                    AgentAskKind.COMMAND -> G7Icons.terminal
                    AgentAskKind.EDIT -> G7Icons.edit
                    AgentAskKind.OTHER -> G7Icons.help
                },
                contentDescription = null,
                tint = tints.amber,
                modifier = Modifier.size(20.dp),
            )
            Text(
                stringResource(
                    when (ask.kind) {
                        AgentAskKind.COMMAND -> R.string.agents_ask_command
                        AgentAskKind.EDIT -> R.string.agents_ask_edit
                        AgentAskKind.OTHER -> R.string.agents_ask_other
                    },
                ),
                style = MaterialTheme.typography.titleSmall,
                modifier = Modifier.weight(1f),
            )
        }
        if (ask.kind != AgentAskKind.COMMAND || body == null) {
            Text(ask.title, style = MaterialTheme.typography.bodyMedium)
        }
        if (body != null) {
            CodeBlock(
                body,
                diff = ask.kind == AgentAskKind.EDIT,
                background = scheme.surface,
                scroll = scroll,
                modifier = Modifier.heightIn(max = 220.dp),
            )
        }
        if (ask.truncated) {
            Text(stringResource(R.string.agents_ask_truncated), style = MaterialTheme.typography.bodySmall, color = tints.amber)
        }
        val reason = ask.reason?.takeIf { it.isNotBlank() }
        if (reason != null) {
            // The agent's own words, set apart from G7's.
            Row(Modifier.height(IntrinsicSize.Min)) {
                Box(Modifier.width(2.dp).fillMaxHeight().background(scheme.outlineVariant))
                Text(
                    stringResource(R.string.agents_ask_says, agentName, reason),
                    style = MaterialTheme.typography.bodySmall,
                    color = scheme.onSurfaceVariant,
                    modifier = Modifier.padding(start = 8.dp),
                )
            }
        }
        val asked = relativeTime(ask.expiresMs - APPROVAL_LIFETIME_MS, now)
        Text(
            listOfNotNull(
                folder?.takeIf { it.isNotBlank() }?.let { stringResource(R.string.agents_ask_where, it, pcName) },
                stringResource(R.string.agents_ask_when, asked),
                stringResource(R.string.agents_ask_also_pc),
            ).joinToString(" "),
            style = MaterialTheme.typography.bodySmall,
            color = scheme.onSurfaceVariant,
        )
        if (!seenAll && !ask.truncated) {
            Text(stringResource(R.string.agents_ask_read_all), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
        }
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            G7RaisedButton(
                label = stringResource(R.string.agents_deny),
                onClick = { onAnswer(false) },
                enabled = !answering,
                modifier = Modifier.weight(1f),
            )
            G7PrimaryButton(
                label = stringResource(R.string.agents_allow),
                onClick = { onAnswer(true) },
                enabled = !answering && canAllow,
                modifier = Modifier.weight(1f),
            )
        }
        if (answering) {
            Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
            }
        }
        Text(stringResource(R.string.agents_ask_deny_note), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
    }
}

/** The reply box: off while the agent works (Stop is in the header) and while a usage limit hasn't reset. */
@Composable
private fun Composer(
    thread: AgentThread?,
    status: ThreadStatus?,
    agentName: String,
    now: Long,
    draft: String,
    onDraft: (String) -> Unit,
    sending: Boolean,
    onSend: () -> Unit,
) {
    val context = LocalContext.current
    val running = thread != null && AgentsModel.running(thread)
    val paused = status as? ThreadStatus.Paused
    val pausedUntil = paused?.resetsMs?.takeIf { it > now }
    val hint = when {
        running -> stringResource(R.string.agents_reply_when_stops, agentName)
        pausedUntil != null -> stringResource(R.string.agents_reply_after, DateFormat.getTimeFormat(context).format(Date(pausedUntil)))
        paused != null -> stringResource(R.string.agents_reply_after_reset)
        else -> stringResource(R.string.agents_message_hint, agentName)
    }
    val enabled = thread != null && !running && pausedUntil == null && !sending
    Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp)) {
        if (AgentsModel.tooLong(draft)) {
            Text(
                stringResource(R.string.agents_too_long),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error,
                modifier = Modifier.padding(start = 4.dp, bottom = 4.dp),
            )
        }
        Row(verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            AgentTextField(
                value = draft,
                onValueChange = onDraft,
                hint = hint,
                enabled = enabled,
                maxLines = 5,
                modifier = Modifier.weight(1f),
            )
            if (sending) {
                Box(Modifier.size(48.dp), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                }
            } else {
                G7IconButton(
                    icon = G7Icons.arrowUp,
                    contentDescription = stringResource(R.string.agents_send),
                    onClick = onSend,
                    enabled = enabled && AgentsModel.fits(draft),
                    filled = true,
                    tint = MaterialTheme.colorScheme.primary,
                )
            }
        }
    }
}

/** Monospace text that wraps instead of hiding anything; a diff's added and removed lines in their colours. */
@Composable
private fun CodeBlock(
    text: String,
    diff: Boolean,
    modifier: Modifier = Modifier,
    background: Color = MaterialTheme.colorScheme.surfaceContainer,
    scroll: ScrollState? = null,
) {
    val tints = g7Tints()
    val shown = remember(text, diff, tints) { if (diff) diffText(text, tints.green, tints.red) else AnnotatedString(text) }
    Box(
        modifier = modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(background),
    ) {
        Text(
            shown,
            style = MaterialTheme.typography.bodySmall,
            fontFamily = LocalAppFonts.current.mono,
            color = MaterialTheme.colorScheme.onSurface,
            softWrap = true,
            modifier = Modifier
                .fillMaxWidth()
                .then(if (scroll != null) Modifier.verticalScroll(scroll) else Modifier)
                .padding(10.dp),
        )
    }
}

private fun diffText(text: String, added: Color, removed: Color): AnnotatedString = buildAnnotatedString {
    text.lines().forEachIndexed { index, line ->
        if (index > 0) append('\n')
        when {
            line.startsWith("+") && !line.startsWith("+++") -> withStyle(SpanStyle(color = added)) { append(line) }
            line.startsWith("-") && !line.startsWith("---") -> withStyle(SpanStyle(color = removed)) { append(line) }
            else -> append(line)
        }
    }
}

/** Tools whose detail is a diff (contract §4.4 and §5.4). */
private val EDIT_TOOLS = setOf("Write", "Edit", "MultiEdit", "NotebookEdit", "apply_patch")

/** A request lasts two hours on the PC (contract §2.2), so it was asked this long before it expires. */
private const val APPROVAL_LIFETIME_MS = 7_200_000L

/** More than any item is tall: scrolls to the end of the last one. */
private const val END_SCROLL = 100_000f
