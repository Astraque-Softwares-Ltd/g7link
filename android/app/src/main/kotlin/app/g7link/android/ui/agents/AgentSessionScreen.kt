// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.toggleableState
import androidx.compose.ui.state.ToggleableState
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
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.g7Tints
import app.g7link.android.ui.theme.mono
import app.g7link.core.AgentContinueBlock
import app.g7link.core.AgentLink
import app.g7link.core.AgentSession
import app.g7link.core.AgentSessionSource
import app.g7link.core.AgentSessionState
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** How far opening the session got. */
private enum class SessionLoad { LOADING, READY, NOT_ALLOWED, UNAVAILABLE, UNREACHABLE, GONE, FAILED }

/**
 * A session started on a PC outside G7 (in a terminal or the agent's own app),
 * followed: its transcript with what it does now, where it runs, and Keep in
 * status bar. Where the PC offers it, a reply typed here goes into the
 * session's own window on the PC (plans/reply-on-pc.md); the PC decides each
 * time, and nothing is sent unless the user sends it. Once it's no longer open
 * anywhere it can be continued here as a thread. Nothing is written to disk;
 * leaving drops the transcript and the draft.
 *
 * Its header is two rows (owner's feedback, 2026-11-21): Back, the title and
 * its actions (Keep in status bar as a toggle, More), then one line with its
 * state, agent, folder and where it runs; tapping the line shows the whole
 * sentence. The footer under the transcript is one component, [SessionFooter].
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

    // Reply on the PC (plans/reply-on-pc.md §6): the PC says whether it can, and does it; this screen keeps the draft.
    var reply by remember(sessionId) { mutableStateOf(ReplyState()) }
    var checkTick by remember(sessionId) { mutableIntStateOf(0) }
    var lastCheckMs by remember(sessionId) { mutableLongStateOf(0L) }
    var why by remember(sessionId) { mutableStateOf(false) }
    val presence by core.agentPresence.collectAsStateWithLifecycle()
    val pcLocked = presence[device.id]?.locked
    val pcInUse = presence[device.id]?.atPc
    val currentSession by rememberUpdatedState(session)
    // Asked when the view comes into view, when the session's state or the PC's lock changes, and when the box gains focus.
    LaunchedEffect(device.id, sessionId, load, device.online, session?.state, session?.guess, pcLocked, pcInUse, checkTick, lifecycle) {
        if (load != SessionLoad.READY || !device.online) return@LaunchedEffect
        lifecycle.repeatOnLifecycle(Lifecycle.State.RESUMED) {
            if (!AgentReplyModel.shouldCheck(reply, currentSession)) return@repeatOnLifecycle
            delay((lastCheckMs + AgentReplyModel.CHECK_GAP_MS - System.currentTimeMillis()).coerceIn(CHECK_SETTLE_MS, AgentReplyModel.CHECK_GAP_MS))
            val epoch = reply.epoch
            lastCheckMs = System.currentTimeMillis()
            val answer = core.agentSessionReplyCheck(device.id, sessionId)
            currentSession?.let { reply = AgentReplyModel.checked(reply, epoch, answer, it) }
        }
    }
    // In use or too fast: asked again once the PC says it may be worth it, while it keeps saying so.
    LaunchedEffect(reply.retryMs, reply.note, lastCheckMs) {
        val wait = reply.retryMs ?: return@LaunchedEffect
        delay(wait.coerceAtLeast(RETRY_LEAST_MS))
        checkTick++
    }
    LaunchedEffect(open?.items) { open?.items?.let { reply = AgentReplyModel.itemsSeen(reply, it) } }
    // Each reply sent takes the transcript back to its end (AgentRowsList's jump).
    var jumps by remember(sessionId) { mutableIntStateOf(0) }
    val sendReply: () -> Unit = {
        val out = AgentReplyModel.outgoing(reply, session)
        if (out != null) {
            val (text, confirm) = out
            // Back to the end, where the reply and what answers it come in.
            jumps++
            val lastSeq = AgentReplyModel.lastSeq(open?.items.orEmpty())
            reply = AgentReplyModel.sendStarted(reply)
            val epoch = reply.epoch
            scope.launch {
                // One act of the user's. Only "in use" with the PC's wait is sent again, for up to a
                // minute while "Waiting until guchu is free…" shows (plans/reply-tabs-idle.md §4);
                // an answer that never came is never sent again. Leaving the screen ends it.
                val startedMs = System.currentTimeMillis()
                var answer = core.agentSessionReply(device.id, sessionId, text, confirm)
                while (true) {
                    val nowMs = System.currentTimeMillis()
                    val gap = AgentReplyModel.waitFor(reply, epoch, answer, startedMs, nowMs) ?: break
                    reply = AgentReplyModel.waiting(reply, nowMs + gap)
                    delay(gap)
                    // Cancelled while it waited: the box is free again and nothing more goes.
                    if (!AgentReplyModel.stillWaiting(reply, epoch)) return@launch
                    reply = AgentReplyModel.resending(reply)
                    answer = core.agentSessionReply(device.id, sessionId, text, confirm)
                }
                reply = AgentReplyModel.replied(reply, answer, text, lastSeq, currentSession)
            }
        }
    }
    val replyFooter = AgentReplyModel.footer(reply, session)
    reply.confirm?.let { confirm ->
        ReplyConfirmSheet(confirm = confirm, pcName = device.name, onSend = sendReply, onCancel = { reply = AgentReplyModel.confirmCancelled(reply) })
    }
    val cantReply = replyFooter as? ReplyFooter.Cant
    LaunchedEffect(cantReply == null) { if (cantReply == null) why = false }
    if (why && cantReply != null) {
        ReplyWhySheet(cant = cantReply, agentName = agentName, pcName = device.name, onDismiss = { why = false })
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
    // A reply entered on the PC shows at the end, as a new message would.
    LaunchedEffect(reply.pending) { if (reply.pending != null) jumps++ }
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
        // A followed session has no requests, turns, source control or scripts of its own.
        is FullView.Edit, is FullView.Changes, is FullView.Review, is FullView.Pr, is FullView.Push, is FullView.RunOutput, null -> Unit
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
        // The reply box rides on the keyboard, as the thread's does.
        Column(Modifier.fillMaxSize().imePadding()) {
            G7Header(title = session?.title.orEmpty(), onBack = onBack) {
                AgentsDrawerButton(drawer)
                if (session != null && session.state != AgentSessionState.ENDED) {
                    KeepToggle(kept = kept, onChange = { on -> core.agentSessionKeep(device.id, sessionId, on) })
                }
                SessionMenu(canShare = session != null && load == SessionLoad.READY && !sharing, onShare = share)
            }
            if (session != null) {
                SessionLine(
                    session = session,
                    agentName = agentName,
                    pcName = device.name,
                    now = now,
                    replies = replyFooter is ReplyFooter.Box,
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
                    val pending = AgentReplyModel.pendingRow(reply, session)
                    val pendingRows: (LazyListScope.() -> Unit)? = if (pending == null) {
                        null
                    } else {
                        {
                            item(key = PENDING_KEY, contentType = "pending") { PendingReplyRow(pending, agentName = agentName, pcName = device.name) }
                        }
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
                        footer = pendingRows,
                        jump = jumps,
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
                        reply = replyFooter,
                        draft = reply.draft,
                        onReply = ReplyActions(
                            text = { reply = AgentReplyModel.typed(reply, it) },
                            focus = { focused -> if (focused) checkTick++ },
                            send = sendReply,
                            dismiss = { reply = AgentReplyModel.dismissed(reply) },
                            why = { why = true },
                            cancelWait = { reply = AgentReplyModel.waitCancelled(reply) },
                        ),
                    )
                }
            }
        }
    }
}

/**
 * "Started in a terminal on guchu. Read only.": where it runs, whole; an
 * ended one can be continued, and one the PC takes replies for can be
 * answered, so neither is only read.
 */
@Composable
private fun startedSentence(session: AgentSession, agentName: String, pcName: String, replies: Boolean): String {
    val where = when (session.source) {
        AgentSessionSource.TERMINAL -> stringResource(R.string.agents_session_started_terminal, pcName)
        AgentSessionSource.DESKTOP_APP -> stringResource(R.string.agents_session_started_app, agentName, pcName)
        AgentSessionSource.OTHER -> stringResource(R.string.agents_session_started_other, pcName)
    }
    return if (session.state == AgentSessionState.ENDED || replies) where else "$where ${stringResource(R.string.agents_session_read_only)}"
}

/**
 * The header's second row: "● Working for 1 h · Claude · g7linux · in the
 * Claude app, read only" on one line ("reply on the PC" while the PC takes
 * replies for it); a tap shows where it was started in a whole sentence
 * under it.
 */
@Composable
private fun SessionLine(session: AgentSession, agentName: String, pcName: String, now: Long, replies: Boolean, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    var whole by remember(session.id) { mutableStateOf(false) }
    // A usage limit reads as such, with the still mark (owner's report, 2026-11-21).
    val (word, color) = sessionWord(session, now)
    val where = when (session.source) {
        AgentSessionSource.TERMINAL -> stringResource(R.string.agents_session_in_terminal)
        AgentSessionSource.DESKTOP_APP -> stringResource(R.string.agents_session_in_app, agentName)
        AgentSessionSource.OTHER -> null
    }
    val place = where?.let {
        when {
            session.state == AgentSessionState.ENDED -> it
            replies -> stringResource(R.string.agents_pc_reply_line, it)
            else -> stringResource(R.string.agents_session_line_read_only, it)
        }
    }
    val rest = listOfNotNull(agentName.ifBlank { null }, session.folder.ifBlank { null }, place).joinToString(" · ", prefix = "· ")
    val sentence = startedSentence(session, agentName, pcName, replies)
    Column(
        modifier = modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(6.dp))
            .clickable(role = Role.Button, onClickLabel = sentence) { whole = !whole },
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            SessionLookMark(AgentsModel.sessionLook(session))
            Text(word, style = MaterialTheme.typography.labelMedium, color = color, maxLines = 1)
            Text(rest, style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (whole) Text(sentence, style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
    }
}

/** "Keep in status bar" as a toggle in the header: a bell, on while it's kept. */
@Composable
private fun KeepToggle(kept: Boolean, onChange: (Boolean) -> Unit) {
    val label = stringResource(R.string.agents_session_keep)
    G7IconButton(
        icon = G7Icons.bell,
        contentDescription = label,
        onClick = { onChange(!kept) },
        selected = kept,
        tint = if (kept) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.semantics { toggleableState = if (kept) ToggleableState.On else ToggleableState.Off },
    )
}

/** The header's More: Share. */
@Composable
private fun SessionMenu(canShare: Boolean, onShare: () -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        G7IconButton(icon = AgentGlyphs.more, contentDescription = stringResource(R.string.agents_more), onClick = { open = true })
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            DropdownMenuItem(
                text = { Text(stringResource(R.string.agents_share)) },
                enabled = canShare,
                onClick = {
                    open = false
                    onShare()
                },
            )
        }
    }
}

/** What the reply box and its lines do. */
private class ReplyActions(
    val text: (String) -> Unit,
    val focus: (Boolean) -> Unit,
    val send: () -> Unit,
    val dismiss: () -> Unit,
    val why: () -> Unit,
    val cancelWait: () -> Unit,
)

/**
 * Under the transcript, by state: where it waits for you (answered on the PC),
 * what it does now, or how to continue it here (and why not yet). One
 * component for every state, so a reply box can join it in one place
 * ("Reply on the PC", plans/reply-on-pc.md): with the box, "Working · 12 min"
 * is one line above it and an idle session needs no card; without it, the
 * quiet line says why when the PC can't take replies for it.
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
    reply: ReplyFooter,
    draft: String,
    onReply: ReplyActions,
) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val box = reply as? ReplyFooter.Box
    val card = Modifier
        .fillMaxWidth()
        .padding(horizontal = 12.dp, vertical = 8.dp)
        .clip(RoundedCornerShape(12.dp))
        .background(scheme.surfaceContainer)
        .padding(14.dp)
    Column(Modifier.fillMaxWidth()) {
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
            // With the box: "Working · 12 min" as one line above it (§6.1).
            session.state == AgentSessionState.WORKING && box != null -> Row(
                modifier = Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                SessionStatusMark(needsYou = false, mayNeedYou = false, working = true)
                NowLine(step = session.step, sinceMs = session.sinceMs, modifier = Modifier.weight(1f))
            }
            session.state == AgentSessionState.WORKING -> Column(card, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    SessionStatusMark(needsYou = false, mayNeedYou = false, working = true)
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
            // Idle and open where it runs: the box is how to answer it, so no card says it can't be continued yet.
            box != null -> Unit
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
        when (reply) {
            ReplyFooter.None -> Unit
            is ReplyFooter.Cant -> CantReplyLine(reply, agentName = agentName, pcName = pcName, onWhy = onReply.why, onDismiss = onReply.dismiss)
            is ReplyFooter.Box -> ReplyBox(reply, draft = draft, agentName = agentName, pcName = pcName, actions = onReply)
        }
    }
}

/**
 * The reply box (§6.1): above it the target line ("Terminal · Claude ·
 * g7linux", a tap tells what happens), "Typing on guchu…" while it goes, or
 * the lock while the PC is locked; then the thread's text field and send
 * button, without the model, mode, image, `@` or `/` controls; under it the
 * PC's line for the last answer.
 */
@Composable
private fun ReplyBox(box: ReplyFooter.Box, draft: String, agentName: String, pcName: String, actions: ReplyActions) {
    val scheme = MaterialTheme.colorScheme
    var whole by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxWidth().padding(start = 10.dp, end = 10.dp, top = 6.dp, bottom = 8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        val muted = scheme.onSurfaceVariant
        when {
            // Cancel ends only the waiting: a try already on its way is still read.
            box.waiting -> ReplyTargetRow(G7Icons.clock, stringResource(R.string.agents_pc_reply_waiting, pcName), muted) {
                G7RowAction(label = stringResource(R.string.action_cancel), onClick = actions.cancelWait)
            }
            box.sending -> ReplyTargetRow(G7Icons.keyboard, stringResource(R.string.agents_pc_reply_typing, pcName), muted)
            box.locked -> ReplyTargetRow(G7Icons.lock, stringResource(R.string.agents_pc_reply_unlock, pcName), scheme.onSurface)
            else -> box.target?.let { target ->
                val details = stringResource(R.string.agents_pc_reply_where, pcName)
                Column(
                    Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(6.dp))
                        .clickable(role = Role.Button, onClickLabel = details) { whole = !whole },
                ) {
                    ReplyTargetRow(if (target.inApp) G7Icons.window else G7Icons.terminal, target.line, muted)
                    if (whole) Text(details, style = MaterialTheme.typography.bodySmall, color = muted, modifier = Modifier.padding(start = 4.dp, bottom = 2.dp))
                }
            }
        }
        if (box.tooLong) {
            Text(stringResource(R.string.agents_too_long), style = MaterialTheme.typography.bodySmall, color = scheme.error, modifier = Modifier.padding(start = 4.dp))
        }
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(12.dp))
                .background(scheme.surfaceContainer)
                .padding(end = 8.dp)
                .dimmed(box.locked),
            verticalAlignment = Alignment.Bottom,
        ) {
            AgentTextField(
                value = draft,
                onValueChange = actions.text,
                hint = stringResource(R.string.agents_pc_reply_hint, pcName),
                enabled = !box.locked && !box.sending,
                maxLines = 5,
                background = Color.Transparent,
                onFocus = actions.focus,
                modifier = Modifier.weight(1f),
            )
            Box(Modifier.padding(bottom = 4.dp)) {
                if (box.sending) {
                    Box(Modifier.size(44.dp), contentAlignment = Alignment.Center) {
                        CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                    }
                } else {
                    G7IconButton(
                        icon = G7Icons.send,
                        contentDescription = stringResource(R.string.agents_send),
                        onClick = actions.send,
                        enabled = box.canSend,
                        filled = true,
                        tint = if (box.canSend) scheme.primary else scheme.onSurfaceVariant,
                    )
                }
            }
        }
        box.note?.let { note ->
            ReplyNoteLine(
                note = note,
                agentName = agentName,
                pcName = pcName,
                onRetry = if (box.canRetry) actions.send else null,
                onDismiss = if (box.canDismiss) actions.dismiss else null,
            )
        }
    }
}

/** The line above the box; [action] at its end ("Cancel" while a reply waits). */
@Composable
private fun ReplyTargetRow(icon: ImageVector, text: String, color: Color, action: @Composable (() -> Unit)? = null) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 4.dp, vertical = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Icon(icon, contentDescription = null, tint = color, modifier = Modifier.size(14.dp))
        Text(
            text,
            style = MaterialTheme.typography.labelMedium,
            color = color,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = if (action != null) Modifier.weight(1f) else Modifier,
        )
        action?.invoke()
    }
}

/** The PC's line for the last answer: neutral for "try later", amber for what to do on the PC; Retry or a close where they apply. */
@Composable
private fun ReplyNoteLine(note: ReplyNote, agentName: String, pcName: String, onRetry: (() -> Unit)?, onDismiss: (() -> Unit)?) {
    val scheme = MaterialTheme.colorScheme
    Row(
        modifier = Modifier.fillMaxWidth().padding(start = 4.dp, top = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(
            if (note.amber) AgentGlyphs.warning else G7Icons.info,
            contentDescription = null,
            tint = if (note.amber) g7Tints().amber else scheme.onSurfaceVariant,
            modifier = Modifier.size(15.dp),
        )
        Text(
            replyNoteText(note, agentName, pcName),
            style = MaterialTheme.typography.bodySmall,
            color = if (note.amber) scheme.onSurface else scheme.onSurfaceVariant,
            modifier = Modifier.weight(1f),
        )
        if (onRetry != null) G7RowAction(label = stringResource(R.string.agents_try_again), onClick = onRetry)
        if (onDismiss != null) {
            G7IconButton(icon = G7Icons.close, contentDescription = stringResource(R.string.action_dismiss), onClick = onDismiss, tint = scheme.onSurfaceVariant)
        }
    }
}

@Composable
private fun replyNoteText(note: ReplyNote, agentName: String, pcName: String): String = when (note) {
    ReplyNote.IN_USE -> stringResource(R.string.agents_pc_reply_in_use, pcName)
    ReplyNote.GAVE_UP -> stringResource(R.string.agents_pc_reply_gave_up, pcName)
    ReplyNote.BUSY -> stringResource(R.string.agents_pc_reply_busy, agentName)
    ReplyNote.RATE -> stringResource(R.string.agents_pc_reply_rate)
    ReplyNote.NEEDS_YOU -> stringResource(R.string.agents_pc_reply_needs_you, pcName)
    ReplyNote.TAB -> stringResource(R.string.agents_pc_reply_tab, pcName)
    ReplyNote.UNPROVEN -> stringResource(R.string.agents_pc_reply_unproven)
    ReplyNote.AMBIGUOUS -> stringResource(R.string.agents_pc_reply_ambiguous)
    ReplyNote.CLIPBOARD -> stringResource(R.string.agents_pc_reply_clipboard, pcName)
    ReplyNote.TEXT -> stringResource(R.string.agents_pc_reply_text, agentName)
    ReplyNote.FOCUS -> stringResource(R.string.agents_pc_reply_focus)
    ReplyNote.INTERRUPTED -> stringResource(R.string.agents_pc_reply_interrupted, pcName)
    ReplyNote.NOT_KNOWN -> stringResource(R.string.agents_pc_reply_not_known)
    ReplyNote.FAILED -> stringResource(R.string.agents_pc_reply_failed)
    ReplyNote.UNREACHABLE -> stringResource(R.string.agents_unreachable_title, pcName)
}

/** No box (§6.4): one quiet line, "Why?" opening the reason; a line still owed for an earlier reply under it. */
@Composable
private fun CantReplyLine(cant: ReplyFooter.Cant, agentName: String, pcName: String, onWhy: () -> Unit, onDismiss: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Column(Modifier.fillMaxWidth().padding(start = 16.dp, end = 12.dp, bottom = 8.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Icon(G7Icons.terminal, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(14.dp))
            Text(
                stringResource(R.string.agents_pc_reply_cant),
                style = MaterialTheme.typography.bodySmall,
                color = scheme.onSurfaceVariant,
                modifier = Modifier.weight(1f, fill = false),
            )
            Text(
                stringResource(R.string.agents_pc_reply_why),
                style = MaterialTheme.typography.labelLarge,
                color = scheme.primary,
                modifier = Modifier
                    .clip(RoundedCornerShape(6.dp))
                    .clickable(role = Role.Button, onClick = onWhy)
                    .padding(horizontal = 8.dp, vertical = 10.dp),
            )
        }
        cant.note?.let { note ->
            ReplyNoteLine(note, agentName = agentName, pcName = pcName, onRetry = null, onDismiss = if (note == ReplyNote.INTERRUPTED) onDismiss else null)
        }
    }
}

/** What "Why?" opens: the reason in a sentence or two. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ReplyWhySheet(cant: ReplyFooter.Cant, agentName: String, pcName: String, onDismiss: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val state = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val reason = when (cant.why) {
        CantReply.AGENT_APP -> listOf(stringResource(R.string.agents_pc_reply_cant_app, agentName, pcName))
        CantReply.CODEX_WINDOW -> listOf(stringResource(R.string.agents_pc_reply_cant_codex), stringResource(R.string.agents_pc_reply_cant_codex_tip))
        CantReply.TERMINAL -> listOfNotNull(
            stringResource(R.string.agents_pc_reply_cant_terminal),
            cant.app?.let { stringResource(R.string.agents_pc_reply_cant_in, it) },
        )
        CantReply.NO_WINDOW -> listOf(stringResource(R.string.agents_pc_reply_cant_window))
        CantReply.SETUP -> listOf(stringResource(R.string.agents_pc_reply_cant_setup, pcName))
        CantReply.OFF -> listOf(stringResource(R.string.agents_pc_reply_cant_off, pcName), stringResource(R.string.agents_pc_reply_cant_off_tip))
    }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = state, containerColor = scheme.surfaceContainerHigh) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(horizontal = 20.dp).padding(bottom = 20.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(stringResource(R.string.agents_pc_reply_cant), style = MaterialTheme.typography.titleMedium)
            reason.forEachIndexed { index, line ->
                Text(line, style = if (index == 0) MaterialTheme.typography.bodyLarge else MaterialTheme.typography.bodyMedium, color = if (index == 0) scheme.onSurface else scheme.onSurfaceVariant)
            }
            G7RaisedButton(label = stringResource(R.string.agents_close), onClick = onDismiss, modifier = Modifier.fillMaxWidth())
        }
    }
}

/**
 * The first reply to this window (§6.2): where it will type, large, what G7
 * does, and Send or Cancel. The draft stays in the box behind it.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ReplyConfirmSheet(confirm: ReplyConfirm, pcName: String, onSend: () -> Unit, onCancel: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val state = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(onDismissRequest = onCancel, sheetState = state, containerColor = scheme.surfaceContainerHigh) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(horizontal = 20.dp).padding(bottom = 20.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(stringResource(R.string.agents_pc_reply_confirm_title, pcName), style = MaterialTheme.typography.titleMedium)
            confirm.target?.let { target ->
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(10.dp))
                        .background(scheme.surface)
                        .padding(horizontal = 14.dp, vertical = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Icon(if (target.inApp) G7Icons.window else G7Icons.terminal, contentDescription = null, tint = scheme.onSurface, modifier = Modifier.size(22.dp))
                    Text(target.line, style = MaterialTheme.typography.titleSmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
                }
            }
            Text(stringResource(R.string.agents_pc_reply_confirm_body), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
            ButtonPair(
                secondary = stringResource(R.string.action_cancel),
                onSecondary = onCancel,
                secondaryEnabled = true,
                primary = stringResource(R.string.agents_send),
                onPrimary = onSend,
                primaryEnabled = true,
            )
        }
    }
}

/**
 * A reply entered on the PC, at the end of the transcript until its own row
 * arrives (§6.3): the text, and "Entered on guchu", or "Waiting for Claude
 * to pick it up" while the session works.
 */
@Composable
private fun PendingReplyRow(row: PendingRow, agentName: String, pcName: String) {
    val scheme = MaterialTheme.colorScheme
    Column(Modifier.fillMaxWidth().padding(start = 48.dp, top = 10.dp), horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text(
            row.text,
            style = MaterialTheme.typography.bodyLarge,
            maxLines = 12,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier
                .clip(RoundedCornerShape(12.dp))
                .border(1.dp, scheme.outline, RoundedCornerShape(12.dp))
                .background(scheme.surfaceContainer)
                .padding(horizontal = 14.dp, vertical = 10.dp),
        )
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Icon(if (row.waiting) G7Icons.history else G7Icons.check, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(13.dp))
            Text(
                if (row.waiting) stringResource(R.string.agents_pc_reply_picking_up, agentName) else stringResource(R.string.agents_pc_reply_entered, pcName),
                style = MaterialTheme.typography.labelMedium,
                color = scheme.onSurfaceVariant,
            )
        }
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

/** The pending reply's place at the end of the transcript. */
private const val PENDING_KEY = "reply-pending"

/** A check waits this long after what asked for it, so a burst of changes asks once. */
private const val CHECK_SETTLE_MS = 300L

/** The least wait before asking again after "in use" or "too fast". */
private const val RETRY_LEAST_MS = 1_000L
