// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.repeatOnLifecycle
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.theme.G7Header
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.G7StatusDot
import app.g7link.android.ui.theme.G7SwitchRow
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.g7Tints
import app.g7link.android.ui.theme.mono
import app.g7link.core.AgentContinueBlock
import app.g7link.core.AgentLink
import app.g7link.core.AgentSession
import app.g7link.core.AgentSessionSource
import app.g7link.core.AgentSessionState
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.launch

/** How far opening the session got. */
private enum class SessionLoad { LOADING, READY, NOT_ALLOWED, UNAVAILABLE, UNREACHABLE, GONE, FAILED }

/**
 * A session started on a PC outside G7 (in a terminal or the agent's own app),
 * followed read-only: its transcript with what it does now, where it runs, and
 * Keep in status bar. Nothing here types into it or answers it; once it's no
 * longer open anywhere it can be continued here as a thread. Nothing is
 * written to disk; leaving drops the transcript.
 */
@Composable
fun AgentSessionScreen(
    device: Device,
    core: Core,
    sessionId: String,
    onOpenThread: (String) -> Unit,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val resources = LocalResources.current
    val context = LocalContext.current
    val all by core.agents.collectAsStateWithLifecycle()
    val keptSessions by core.agentKeptSessions.collectAsStateWithLifecycle()
    val agents = all[device.id] ?: AgentsState()
    val session = agents.session(sessionId)
    val open = agents.openSession?.takeIf { it.sessionId == sessionId }
    var load by remember(sessionId) { mutableStateOf(SessionLoad.LOADING) }
    var attempt by remember(sessionId) { mutableIntStateOf(0) }
    var loadingEarlier by remember(sessionId) { mutableStateOf(false) }
    var continuing by remember(sessionId) { mutableStateOf(false) }
    var sharing by remember(sessionId) { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val now = rememberNow()
    val retry: () -> Unit = { attempt++ }

    // Leaving the view drops the transcript.
    DisposableEffect(device.id, sessionId) {
        onDispose { core.agentSessionClosed(device.id, sessionId) }
    }
    // In front, the PC sends its items; on coming back or reconnecting, its newest page is read again.
    LaunchedEffect(device.id, sessionId, device.online, attempt, lifecycle) {
        if (!device.online) {
            load = SessionLoad.UNREACHABLE
            return@LaunchedEffect
        }
        lifecycle.repeatOnLifecycle(Lifecycle.State.RESUMED) {
            try {
                load = when (val result = core.agentSessionShown(device.id, sessionId)) {
                    is AgentCall.Ok -> SessionLoad.READY
                    is AgentCall.Failed -> when (result.failure) {
                        AgentFailure.DENIED -> SessionLoad.NOT_ALLOWED
                        AgentFailure.UNSUPPORTED -> SessionLoad.UNAVAILABLE
                        AgentFailure.NOT_FOUND -> SessionLoad.GONE
                        AgentFailure.OFFLINE, AgentFailure.TIMEOUT -> SessionLoad.UNREACHABLE
                        AgentFailure.OTHER -> SessionLoad.FAILED
                    }
                }
                awaitCancellation()
            } finally {
                core.agentSessionHidden(device.id, sessionId)
            }
        }
    }

    val agentName = session?.let { agents.agentName(it.agent) }.orEmpty()
    val kept = "${device.id}/$sessionId" in keptSessions
    val loadEarlier: () -> Unit = {
        val first = open?.items?.firstOrNull()?.seq
        if (first != null && !loadingEarlier) {
            loadingEarlier = true
            scope.launch {
                core.agentSessionRead(device.id, sessionId, first)
                loadingEarlier = false
            }
        }
    }
    val continueHere: () -> Unit = {
        val current = session
        if (current != null && !continuing) {
            continuing = true
            scope.launch {
                when (val result = core.agentSessionContinue(device.id, sessionId)) {
                    is AgentCall.Ok -> onOpenThread(result.value.id)
                    is AgentCall.Failed -> core.say(
                        when (result.failure) {
                            AgentFailure.DENIED -> resources.getString(R.string.agents_session_add_folder, current.folder, device.name)
                            AgentFailure.UNSUPPORTED -> resources.getString(R.string.agents_session_agent_unavailable, agentName, device.name)
                            AgentFailure.NOT_FOUND -> resources.getString(R.string.agents_session_gone, device.name)
                            AgentFailure.OFFLINE, AgentFailure.TIMEOUT -> resources.getString(R.string.agents_unreachable_title, device.name)
                            // The PC's BUSY: something opened it again since the list said it could continue.
                            AgentFailure.OTHER -> when (current.source) {
                                AgentSessionSource.TERMINAL -> resources.getString(R.string.agents_session_open_terminal)
                                AgentSessionSource.DESKTOP_APP -> resources.getString(R.string.agents_session_open_app, agentName)
                                AgentSessionSource.OTHER -> resources.getString(R.string.agents_session_open_other, device.name)
                            }
                        },
                    )
                }
                continuing = false
            }
        }
    }

    val share: () -> Unit = {
        val title = session?.title.orEmpty()
        if (!sharing) {
            sharing = true
            scope.launch {
                when (val result = core.agentExport(device.id, null, sessionId)) {
                    is AgentCall.Ok -> if (!AgentShare.share(context, result.value, title)) {
                        core.say(resources.getString(R.string.agents_share_failed))
                    }
                    is AgentCall.Failed -> core.say(
                        when (result.failure) {
                            AgentFailure.DENIED -> resources.getString(R.string.agents_session_allow_on, device.name)
                            AgentFailure.UNSUPPORTED -> resources.getString(R.string.agents_unavailable, device.name)
                            AgentFailure.NOT_FOUND -> resources.getString(R.string.agents_session_gone, device.name)
                            AgentFailure.OFFLINE, AgentFailure.TIMEOUT -> resources.getString(R.string.agents_unreachable_title, device.name)
                            AgentFailure.OTHER -> resources.getString(R.string.agents_share_failed)
                        },
                    )
                }
                sharing = false
            }
        }
    }

    // The same rows as a thread's (contract §P6.1): no turns or phases from a session, so a turn reads as answered once it settles.
    var openKeys by remember(sessionId) { mutableStateOf(emptySet<String>()) }
    val ffiRows = remember(sessionId) { FfiRows() }
    DisposableEffect(ffiRows) { onDispose { ffiRows.close() } }
    val feedKey = "session:$sessionId"
    val feed = remember(ffiRows) { RowsFeed(feedKey, ffiRows::make) }
    LaunchedEffect(feed) { feed.start(this) }
    val input = remember(open, session, openKeys) {
        if (open != null && session != null) AgentsModel.sessionTimelineInput(open, session, openKeys) else null
    }
    LaunchedEffect(feed, input) { input?.let(feed::submit) }
    val made by feed.rows.collectAsStateWithLifecycle()
    val rows = made?.takeIf { it.input.key == feedKey }?.rows.orEmpty()
    val itemsById = remember(open?.items) { open?.items.orEmpty().associateBy { it.id } }
    var full by remember(sessionId) { mutableStateOf<FullView?>(null) }
    var link by remember(sessionId) { mutableStateOf<PendingLink?>(null) }
    val copy = rememberCopy { core.say(resources.getString(R.string.agents_copied)) }
    val actions = remember(copy) {
        RowActions(
            toggle = { key -> openKeys = if (key in openKeys) openKeys - key else openKeys + key },
            link = { span, text ->
                // A link whose text is its target opens at once; any other shows where it goes first.
                val shown = span.style.link == AgentLink.SHOWN
                if (shown) openLink(context, span.href)
                if (!shown) link = PendingLink(span.href, text)
            },
            copy = copy,
            output = { itemId -> full = FullView.Output(itemId) },
            diff = { title, doc -> full = FullView.Diff(title, doc) },
            mark = {},
        )
    }
    link?.let { shown ->
        LinkSheet(
            link = shown,
            onOpen = {
                link = null
                openLink(context, shown.href)
            },
            onDismiss = { link = null },
        )
    }

    val listState = rememberLazyListState()
    val drawer = rememberAgentsDrawer()
    BackHandler(enabled = full != null) { full = null }
    when (val view = full) {
        is FullView.Output -> {
            OutputFullScreen(item = itemsById[view.itemId], folder = session?.folder.orEmpty(), onClose = { full = null }, onCopy = copy)
            return
        }
        is FullView.Diff -> {
            DiffFullScreen(title = view.title, doc = view.doc, onClose = { full = null })
            return
        }
        is FullView.Edit, null -> Unit
    }

    AgentsDrawer(
        device = device,
        core = core,
        control = drawer,
        selected = DrawerSelection.Session(sessionId),
        // A thread replaces this view as Continue here does; another session as from its notification.
        onOpenThread = onOpenThread,
        onOpenSession = { id -> AgentsNavigation.open(context, device.id, sessionId = id) },
        modifier = modifier,
    ) {
        Column(Modifier.fillMaxSize()) {
            G7Header(title = session?.title.orEmpty(), onBack = onBack) {
                AgentsDrawerButton(drawer)
                if (session != null && load == SessionLoad.READY) {
                    G7RowAction(label = stringResource(R.string.agents_share), enabled = !sharing, onClick = share)
                }
            }
            if (session != null) {
                SessionStatusLine(
                    session = session,
                    now = now,
                    trailing = listOfNotNull(agentName, session.folder.ifBlank { null }).joinToString(" · "),
                    modifier = Modifier.padding(start = 16.dp, end = 16.dp, bottom = 8.dp),
                )
            }
            when {
                load == SessionLoad.GONE || open?.removed == true || (load == SessionLoad.READY && session == null) ->
                    AgentsEmptyState(title = stringResource(R.string.agents_session_gone, device.name))
                load == SessionLoad.UNREACHABLE -> AgentsEmptyState(
                    title = stringResource(R.string.agents_unreachable_title, device.name),
                    body = stringResource(R.string.agents_unreachable_body),
                    action = stringResource(R.string.agents_try_again),
                    onAction = retry,
                )
                load == SessionLoad.NOT_ALLOWED -> AgentsEmptyState(
                    title = stringResource(R.string.agents_session_allow_on, device.name),
                    action = stringResource(R.string.agents_try_again),
                    onAction = retry,
                )
                load == SessionLoad.UNAVAILABLE -> AgentsEmptyState(
                    title = stringResource(R.string.agents_unavailable, device.name),
                    action = stringResource(R.string.agents_try_again),
                    onAction = retry,
                )
                load == SessionLoad.FAILED -> AgentsEmptyState(
                    title = stringResource(R.string.agents_failed_generic),
                    action = stringResource(R.string.agents_try_again),
                    onAction = retry,
                )
                open == null || !open.loaded || session == null || made == null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    Text(stringResource(R.string.agents_loading), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                else -> {
                    StartedBanner(session = session, agentName = agentName, pcName = device.name)
                    if (session.state != AgentSessionState.ENDED) {
                        G7SwitchRow(
                            label = stringResource(R.string.agents_session_keep),
                            sublabel = stringResource(R.string.agents_session_keep_note),
                            checked = kept,
                            onCheckedChange = { on -> core.agentSessionKeep(device.id, sessionId, on) },
                            modifier = Modifier.padding(horizontal = 4.dp),
                        )
                    }
                    AgentRowsList(
                        rows = rows,
                        context = remember(itemsById, session.folder) { RowsContext(itemsById, session.folder) },
                        actions = actions,
                        earlier = open.moreBefore && open.items.isNotEmpty(),
                        loadingEarlier = loadingEarlier,
                        onLoadEarlier = loadEarlier,
                        listState = listState,
                        modifier = Modifier.weight(1f),
                    )
                    SessionFooter(
                        session = session,
                        agents = agents,
                        agentName = agentName,
                        pcName = device.name,
                        now = now,
                        running = remember(open.items) { AgentsModel.runningTool(open.items) },
                        continuing = continuing,
                        onContinue = continueHere,
                    )
                }
            }
        }
    }
}

/** "Started in a terminal on guchu. Read only." */
@Composable
private fun StartedBanner(session: AgentSession, agentName: String, pcName: String) {
    val scheme = MaterialTheme.colorScheme
    val where = when (session.source) {
        AgentSessionSource.TERMINAL -> stringResource(R.string.agents_session_started_terminal, pcName)
        AgentSessionSource.DESKTOP_APP -> stringResource(R.string.agents_session_started_app, agentName, pcName)
        AgentSessionSource.OTHER -> stringResource(R.string.agents_session_started_other, pcName)
    }
    // An ended one can be continued here, so it's no longer only read.
    val text = if (session.state == AgentSessionState.ENDED) where else "$where ${stringResource(R.string.agents_session_read_only)}"
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp, vertical = 4.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(scheme.surfaceContainer)
            .padding(horizontal = 12.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(G7Icons.terminal, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
        Text(text, style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
    }
}

/**
 * Under the transcript, by state: where it waits for you (answered on the PC),
 * what it does now, or how to continue it here (and why not yet).
 */
@Composable
private fun SessionFooter(
    session: AgentSession,
    agents: AgentsState,
    agentName: String,
    pcName: String,
    now: Long,
    running: app.g7link.core.AgentItem?,
    continuing: Boolean,
    onContinue: () -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val card = Modifier
        .fillMaxWidth()
        .padding(horizontal = 12.dp, vertical = 8.dp)
        .clip(RoundedCornerShape(12.dp))
        .background(scheme.surfaceContainer)
        .padding(14.dp)
    when {
        AgentsModel.needsYouExactly(session) -> Column(card, verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Icon(G7Icons.clock, contentDescription = null, tint = tints.amber, modifier = Modifier.size(18.dp))
                Text(
                    when (session.source) {
                        AgentSessionSource.TERMINAL -> stringResource(R.string.agents_session_waiting_terminal, agentName)
                        AgentSessionSource.DESKTOP_APP -> stringResource(R.string.agents_session_waiting_app, agentName)
                        AgentSessionSource.OTHER -> stringResource(R.string.agents_session_waiting_other, agentName, pcName)
                    },
                    style = MaterialTheme.typography.titleSmall,
                    modifier = Modifier.weight(1f),
                )
            }
            RunningCommand(running = running, folder = session.folder)
            val asked = session.sinceMs?.takeIf { it > 0L }?.let { stringResource(R.string.agents_ask_when, relativeTime(it, now)) }
            Text(
                listOfNotNull(asked, stringResource(R.string.agents_session_answer_on, pcName)).joinToString(" "),
                style = MaterialTheme.typography.bodySmall,
                color = scheme.onSurfaceVariant,
            )
        }
        AgentsModel.mayNeedYou(session) -> Column(card, verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Icon(G7Icons.clock, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(18.dp))
                Text(mayWait(session, agentName, pcName), style = MaterialTheme.typography.bodyMedium, modifier = Modifier.weight(1f))
            }
            RunningCommand(running = running, folder = session.folder)
        }
        session.state == AgentSessionState.WORKING -> Column(card, verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                G7StatusDot(tints.blue)
                Text(
                    session.step?.takeIf { it.isNotBlank() }?.let { stringResource(R.string.agents_step_now, it) }
                        ?: stringResource(R.string.agents_state_working),
                    style = MaterialTheme.typography.titleSmall,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                val since = session.sinceMs
                if (since != null && since > 0L) {
                    ElapsedText(since, style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
                }
            }
            RunningCommand(running = running, folder = session.folder)
        }
        else -> ContinueCard(
            session = session,
            agents = agents,
            agentName = agentName,
            pcName = pcName,
            now = now,
            continuing = continuing,
            onContinue = onContinue,
            modifier = card,
        )
    }
}

/** The command or change a session runs now, with its folder's name. */
@Composable
private fun RunningCommand(running: app.g7link.core.AgentItem?, folder: String) {
    val detail = running?.tool?.detail?.takeIf { it.isNotBlank() } ?: return
    val scheme = MaterialTheme.colorScheme
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(scheme.surface)
            .padding(horizontal = 10.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        if (folder.isNotBlank()) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Icon(G7Icons.folder, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(14.dp))
                Text(folder, style = MaterialTheme.typography.labelSmall, color = scheme.onSurfaceVariant, maxLines = 1)
            }
        }
        Text(
            detail,
            style = MaterialTheme.typography.bodySmall.mono(LocalAppFonts.current),
            maxLines = 6,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/**
 * Idle or ended: Continue here when it can be (it becomes a thread on the PC,
 * started from this phone, which asks before each change; nothing runs until
 * something is sent), else why not: still open where it runs, a folder that
 * isn't an agent folder (added on the PC), or the agent can't run there now.
 */
@Composable
private fun ContinueCard(
    session: AgentSession,
    agents: AgentsState,
    agentName: String,
    pcName: String,
    now: Long,
    continuing: Boolean,
    onContinue: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val scheme = MaterialTheme.colorScheme
    val blocked = session.blocked ?: if (session.canContinue) null else AgentContinueBlock.OPEN
    Column(modifier, verticalArrangement = Arrangement.spacedBy(10.dp)) {
        if (session.state == AgentSessionState.ENDED) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Icon(G7Icons.stop, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(18.dp))
                Text(
                    stringResource(R.string.agents_session_ended, relativeTime(session.sinceMs?.takeIf { it > 0L } ?: session.updatedMs, now)),
                    style = MaterialTheme.typography.titleSmall,
                    modifier = Modifier.weight(1f),
                )
            }
        }
        val reason = when (blocked) {
            null -> null
            AgentContinueBlock.OPEN -> when (session.source) {
                AgentSessionSource.TERMINAL -> stringResource(R.string.agents_session_open_terminal)
                AgentSessionSource.DESKTOP_APP -> stringResource(R.string.agents_session_open_app, agentName)
                AgentSessionSource.OTHER -> stringResource(R.string.agents_session_open_other, pcName)
            }
            AgentContinueBlock.FOLDER -> stringResource(R.string.agents_session_add_folder, session.folder, pcName)
            AgentContinueBlock.AGENT -> {
                val info = agents.catalog?.agents?.firstOrNull { it.id == session.agent }
                val hint = info?.loginHint?.takeIf { it.isNotBlank() }
                if (info != null && info.installed && !info.signedIn && hint != null) {
                    stringResource(R.string.agents_login_failed, agentName, hint, pcName)
                } else {
                    stringResource(R.string.agents_session_agent_unavailable, agentName, pcName)
                }
            }
        }
        Text(
            reason ?: stringResource(R.string.agents_session_continue_body, pcName),
            style = MaterialTheme.typography.bodyMedium,
            color = scheme.onSurfaceVariant,
        )
        if (continuing) {
            Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
            }
        } else {
            G7PrimaryButton(
                label = stringResource(R.string.agents_session_continue),
                onClick = onContinue,
                enabled = blocked == null && session.canContinue,
            )
        }
        if (blocked == null) {
            Text(stringResource(R.string.agents_home_asks_first), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
        }
    }
}
