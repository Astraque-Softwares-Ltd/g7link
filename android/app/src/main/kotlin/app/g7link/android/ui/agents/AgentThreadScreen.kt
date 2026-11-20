// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import android.text.format.DateFormat
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
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
import androidx.compose.runtime.saveable.listSaver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.repeatOnLifecycle
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7StatusDot
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.g7Collapse
import app.g7link.android.ui.theme.g7Expand
import app.g7link.android.ui.theme.g7Tints
import app.g7link.android.ui.theme.mono
import app.g7link.core.AgentAskKind
import app.g7link.core.AgentLink
import app.g7link.core.AgentPlanStatus
import app.g7link.core.AgentPlanStep
import app.g7link.core.AgentThread
import java.util.Date
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** How far opening the thread got. */
private enum class ThreadLoad { LOADING, READY, NOT_ALLOWED, UNAVAILABLE, UNREACHABLE, GONE, FAILED }

/** The rows the user opened, kept across a rotation. */
private val OPEN_KEYS = listSaver<Set<String>, String>(save = { it.toList() }, restore = { it.toSet() })

/** Diff rows the request sheet shows; a longer change is reviewed full-screen. */
private const val SHEET_DIFF_ROWS = 60

/**
 * One thread on a PC (contract §P6, the phone board): its rows from the FFI,
 * the request it waits on as a sheet over the reply box with Stop beside send,
 * "Deny and send" when the box has words, the plan pinned above, and full-screen
 * edits and output. Nothing is written to disk; leaving drops the transcript.
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
    val context = LocalContext.current
    val all by core.agents.collectAsStateWithLifecycle()
    val agents = all[device.id] ?: AgentsState()
    val thread = agents.thread(threadId)
    val open = agents.open?.takeIf { it.threadId == threadId }
    val hasCatalog = agents.catalog != null
    val canSayWhy = agents.catalog?.canSayWhy == true
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

    // ---- The typing guard (contract §P8.2): a request that comes while you type waits for a pause ----
    val guard = remember(threadId) { FfiTypingGuard() }
    DisposableEffect(guard) { onDispose { guard.close() } }
    var guardTick by remember(threadId) { mutableIntStateOf(0) }
    val pending = remember(agents.asks, threadId) { agents.asksFor(threadId).map { it.token } }
    // Told before anything is drawn for them, so a new request is never shown unheld for a frame.
    val requested = remember(guard, pending) {
        guard.requests(pending, System.currentTimeMillis())
        pending
    }
    val heldTokens = remember(guard, guardTick, requested) { requested.filterTo(HashSet()) { guard.held(it) } }
    var idleAt by remember(threadId) { mutableStateOf<Long?>(null) }
    LaunchedEffect(guard, guardTick, requested) { idleAt = guard.idleAtMs() }
    LaunchedEffect(guard, idleAt) {
        val at = idleAt ?: return@LaunchedEffect
        delay((at - System.currentTimeMillis()).coerceAtLeast(0L))
        guard.idle()
        guardTick++
    }
    val queue = remember(agents.asks, threadId, heldTokens) { agents.askQueue(threadId) { it in heldTokens } }

    // ---- The rows (contract §P6.1): made off the main thread whenever their input changes ----
    var openKeys by rememberSaveable(threadId, stateSaver = OPEN_KEYS) { mutableStateOf(emptySet<String>()) }
    val ffiRows = remember(threadId) { FfiRows() }
    DisposableEffect(ffiRows) { onDispose { ffiRows.close() } }
    val feed = remember(ffiRows) { RowsFeed(threadId, ffiRows::make) }
    LaunchedEffect(feed) { feed.start(this) }
    val input = remember(agents, threadId, openKeys, heldTokens) { AgentsModel.timelineInput(agents, threadId, openKeys) { it in heldTokens } }
    LaunchedEffect(feed, input) { input?.let(feed::submit) }
    val made by feed.rows.collectAsStateWithLifecycle()
    val rows = made?.takeIf { it.input.key == threadId }?.rows.orEmpty()
    val itemsById = remember(open?.items) { open?.items.orEmpty().associateBy { it.id } }
    val folder = thread?.folder.orEmpty()

    val agentName = thread?.let { agents.agentName(it.agent) }.orEmpty()
    val status = thread?.let { AgentsModel.status(it) }
    val running = thread != null && AgentsModel.running(thread)
    var stopping by remember(threadId) { mutableStateOf(false) }
    LaunchedEffect(running) { if (!running) stopping = false }
    var draft by rememberSaveable(threadId) { mutableStateOf("") }
    var draftFor by rememberSaveable(threadId) { mutableStateOf<String?>(null) }
    var sending by remember(threadId) { mutableStateOf(false) }
    var loadingEarlier by remember(threadId) { mutableStateOf(false) }
    var sharing by remember(threadId) { mutableStateOf(false) }
    var full by remember(threadId) { mutableStateOf<FullView?>(null) }
    var link by remember(threadId) { mutableStateOf<PendingLink?>(null) }
    var sheetCollapsed by remember(threadId) { mutableStateOf(false) }
    LaunchedEffect(queue?.shown?.token) { sheetCollapsed = false }
    val retry: () -> Unit = { attempt++ }

    val paused = status as? ThreadStatus.Paused
    val pausedUntil = paused?.resetsMs?.takeIf { it > now }
    val answeringShown = queue != null && queue.shown.token in agents.answering
    val composer = AgentDock.composer(
        ComposerInput(
            running = running,
            hasThread = thread != null,
            queue = queue,
            canSayWhy = canSayWhy,
            draft = draft,
            draftFor = draftFor,
            answering = answeringShown,
            sending = sending,
            paused = paused != null,
            pausedUntilMs = pausedUntil,
            codex = thread?.agent == "codex",
        ),
    )

    val copy = rememberCopy { core.say(resources.getString(R.string.agents_copied)) }
    val guardChanged: () -> Unit = { guardTick++ }
    val stop: () -> Unit = {
        stopping = true
        scope.launch {
            if (core.agentCancel(device.id, threadId) is AgentCall.Failed) {
                stopping = false
                core.say(resources.getString(R.string.agents_failed_generic))
            }
        }
    }
    val answer: (Boolean) -> Unit = answer@{ allow ->
        val ask = queue?.shown ?: return@answer
        scope.launch {
            val result = core.agentAnswer(device.id, ask.token, allow)
            if (result is AgentCall.Failed) {
                // Gone: someone answered first, or it lapsed.
                val message = if (result.failure == AgentFailure.NOT_FOUND) R.string.agents_answered_elsewhere else R.string.agents_failed_generic
                core.say(resources.getString(message))
            }
        }
    }
    val failedSay: (AgentFailure) -> String = { failure ->
        when (failure) {
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
        }
    }
    val send: () -> Unit = send@{
        if (!composer.actionEnabled || sending) return@send
        val text = draft.trim()
        val shown = queue?.shown
        sending = true
        scope.launch {
            if (composer.action == ComposerAction.DENY_AND_SEND && shown != null) {
                // The words go with a denial of the request they were typed under (contract §P8.3).
                when (val result = core.agentAnswer(device.id, shown.token, false, text)) {
                    is AgentCall.Ok -> {
                        draft = ""
                        draftFor = null
                        guard.sent()
                        guardChanged()
                    }
                    is AgentCall.Failed -> core.say(
                        if (result.failure == AgentFailure.NOT_FOUND) {
                            resources.getString(R.string.agents_answered_elsewhere)
                        } else {
                            resources.getString(R.string.agents_failed_generic)
                        },
                    )
                }
            } else {
                when (val result = core.agentSend(device.id, threadId, text)) {
                    is AgentCall.Ok -> {
                        draft = ""
                        draftFor = null
                        guard.sent()
                        guardChanged()
                    }
                    is AgentCall.Failed -> core.say(failedSay(result.failure))
                }
            }
            sending = false
        }
    }
    val typed: (String) -> Unit = { text ->
        draft = text
        draftFor = AgentDock.editedUnder(queue)
        guard.typed(System.currentTimeMillis())
        guardChanged()
    }
    val focused: (Boolean) -> Unit = { on ->
        if (on) guard.focus() else guard.blur()
        guardChanged()
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
    val share: () -> Unit = {
        val title = thread?.title.orEmpty()
        if (!sharing) {
            sharing = true
            scope.launch {
                when (val result = core.agentExport(device.id, threadId, null)) {
                    is AgentCall.Ok -> if (!AgentShare.share(context, result.value, title)) {
                        core.say(resources.getString(R.string.agents_share_failed))
                    }
                    is AgentCall.Failed -> core.say(
                        when (result.failure) {
                            AgentFailure.DENIED -> resources.getString(R.string.agents_access_title, device.name)
                            AgentFailure.UNSUPPORTED -> resources.getString(R.string.agents_unavailable, device.name)
                            AgentFailure.NOT_FOUND -> resources.getString(R.string.agents_thread_gone)
                            AgentFailure.OFFLINE, AgentFailure.TIMEOUT -> resources.getString(R.string.agents_unreachable_title, device.name)
                            AgentFailure.OTHER -> resources.getString(R.string.agents_share_failed)
                        },
                    )
                }
                sharing = false
            }
        }
    }
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
            mark = { sheetCollapsed = false },
        )
    }

    val listState = rememberLazyListState()
    val drawer = rememberAgentsDrawer()
    // A full-screen view closes with Back, and an edit request's when the request is no longer shown.
    BackHandler(enabled = full != null) { full = null }
    val fullEdit = full as? FullView.Edit
    LaunchedEffect(fullEdit, queue?.shown?.token) {
        if (fullEdit != null && queue?.shown?.token != fullEdit.token) full = null
    }
    when (val view = full) {
        is FullView.Edit -> if (queue != null && queue.shown.token == view.token) {
            EditFullScreen(
                queue = queue,
                agentName = agentName,
                folder = folder,
                answering = answeringShown,
                onClose = { full = null },
                onAnswer = { allow ->
                    full = null
                    answer(allow)
                },
            )
            return
        }
        is FullView.Output -> {
            OutputFullScreen(item = itemsById[view.itemId], folder = folder, onClose = { full = null }, onCopy = copy)
            return
        }
        is FullView.Diff -> {
            DiffFullScreen(title = view.title, doc = view.doc, onClose = { full = null })
            return
        }
        null -> Unit
    }

    AgentsDrawer(
        device = device,
        core = core,
        control = drawer,
        selected = DrawerSelection.Thread(threadId),
        // Another thread or session replaces this one, as from its notification; Back still goes to the list.
        onOpenThread = { id -> AgentsNavigation.open(context, device.id, threadId = id) },
        onOpenSession = { id -> AgentsNavigation.open(context, device.id, sessionId = id) },
        modifier = modifier,
    ) {
        Column(Modifier.fillMaxSize().imePadding()) {
            ThreadHeader(
                thread = thread,
                status = status,
                agentName = agentName,
                onBack = onBack,
                canShare = thread != null && load == ThreadLoad.READY && !sharing,
                onShare = share,
                drawer = drawer,
            )
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
                // Loading: one line, no skeleton.
                open == null || !open.loaded || made == null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    Text(stringResource(R.string.agents_loading), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                open != null -> BoxWithConstraints(Modifier.fillMaxWidth().weight(1f)) {
                    // The request sheet rests at most at half the thread's height (reconcile file, the dock, item 4).
                    val sheetMax = maxHeight / 2
                    Column(Modifier.fillMaxSize()) {
                        AgentRowsList(
                            rows = rows,
                            context = remember(itemsById, folder) { RowsContext(itemsById, folder) },
                            actions = actions,
                            earlier = open.moreBefore && open.items.isNotEmpty(),
                            loadingEarlier = loadingEarlier,
                            onLoadEarlier = loadEarlier,
                            listState = listState,
                            modifier = Modifier.weight(1f),
                            footer = threadEnd(status, thread, agents, device.name),
                        )
                        Column(Modifier.fillMaxWidth().padding(horizontal = 10.dp)) {
                            val plan = thread?.plan
                            if (!plan.isNullOrEmpty()) TasksLine(plan, key = threadId)
                            if (queue != null) {
                                RequestSheet(
                                    queue = queue,
                                    agentName = agentName,
                                    folder = folder,
                                    answering = answeringShown,
                                    allowPrimary = composer.allowPrimary,
                                    collapsed = sheetCollapsed,
                                    onToggle = { sheetCollapsed = !sheetCollapsed },
                                    maxHeight = sheetMax,
                                    onAnswer = answer,
                                    onReview = { full = FullView.Edit(queue.shown.token) },
                                )
                            }
                            ReplyBox(
                                view = composer,
                                hint = composerHint(composer.hint, agentName, pausedUntil),
                                draft = draft,
                                onDraft = typed,
                                onFocus = focused,
                                sending = sending,
                                stopping = stopping,
                                onStop = stop,
                                onSend = send,
                                joined = queue != null,
                                place = stringResource(R.string.agents_folder_on, folder, device.name),
                            )
                        }
                    }
                }
            }
        }
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
}

/** What the reply box says while empty. */
@Composable
private fun composerHint(hint: ComposerHint, agentName: String, pausedUntil: Long?): String {
    val context = LocalContext.current
    return when (hint) {
        ComposerHint.MESSAGE -> stringResource(R.string.agents_message_hint, agentName)
        ComposerHint.REPLY_WHEN_STOPS -> stringResource(R.string.agents_reply_when_stops, agentName)
        ComposerHint.SAY_WHY -> stringResource(R.string.agents_say_why_hint, agentName)
        ComposerHint.AFTER_TIME -> stringResource(
            R.string.agents_reply_after,
            pausedUntil?.let { DateFormat.getTimeFormat(context).format(Date(it)) }.orEmpty(),
        )
        ComposerHint.AFTER_RESET -> stringResource(R.string.agents_reply_after_reset)
    }
}

/**
 * The header: Back, the title (two lines at most), its state in words with the
 * agent and folder under it, the drawer of threads, and More (Share). Stop is
 * in the reply box.
 */
@Composable
private fun ThreadHeader(
    thread: AgentThread?,
    status: ThreadStatus?,
    agentName: String,
    onBack: () -> Unit,
    canShare: Boolean,
    onShare: () -> Unit,
    drawer: AgentsDrawerControl,
) {
    val scheme = MaterialTheme.colorScheme
    var menu by remember { mutableStateOf(false) }
    Column {
        Row(
            modifier = Modifier.fillMaxWidth().padding(start = 4.dp, end = 4.dp, top = 4.dp, bottom = 6.dp),
            verticalAlignment = Alignment.Top,
        ) {
            G7IconButton(icon = G7Icons.back, contentDescription = stringResource(R.string.action_back), onClick = onBack)
            Column(Modifier.weight(1f).padding(start = 4.dp, top = 8.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                Text(
                    thread?.title.orEmpty(),
                    style = MaterialTheme.typography.titleMedium.copy(fontWeight = FontWeight.SemiBold),
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                if (thread != null && status != null) ThreadMeta(status, agentName, thread.folder)
            }
            AgentsDrawerButton(drawer)
            Box {
                G7IconButton(
                    icon = AgentGlyphs.more,
                    contentDescription = stringResource(R.string.agents_more),
                    onClick = { menu = true },
                    enabled = canShare,
                )
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.agents_share)) },
                        onClick = {
                            menu = false
                            onShare()
                        },
                    )
                }
            }
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(scheme.outlineVariant))
    }
}

/** "● Needs you · Claude · g7linux-phone": the state in its colour, then the agent and the folder's label. */
@Composable
private fun ThreadMeta(status: ThreadStatus, agentName: String, folder: String) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val (word, color) = when (status) {
        ThreadStatus.NeedsYou -> stringResource(R.string.agents_state_waiting) to tints.amber
        ThreadStatus.Working -> stringResource(R.string.agents_state_working) to scheme.onSurfaceVariant
        ThreadStatus.Done -> stringResource(R.string.agents_state_done) to scheme.onSurfaceVariant
        ThreadStatus.Stopped -> stringResource(R.string.agents_state_interrupted) to scheme.onSurfaceVariant
        is ThreadStatus.Paused -> stringResource(R.string.agents_state_paused) to scheme.onSurfaceVariant
        is ThreadStatus.Failed -> stringResource(R.string.agents_state_failed) to scheme.error
    }
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        G7StatusDot(statusDotColor(status))
        Text(word, style = MaterialTheme.typography.labelMedium.copy(fontWeight = FontWeight.SemiBold), color = color, maxLines = 1)
        Text(
            listOf(agentName, folder).filter { it.isNotBlank() }.joinToString(" · ", prefix = "· "),
            style = MaterialTheme.typography.labelMedium,
            color = scheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/**
 * How the thread ended when it ended badly, after its rows: "Failed" and the
 * CLI's own message as it is (with the sign-in command when it's signed out),
 * or the usage limit and when it resets in amber.
 */
@Composable
private fun threadEnd(status: ThreadStatus?, thread: AgentThread?, agents: AgentsState, pcName: String): (LazyListScope.() -> Unit)? {
    if (thread == null) return null
    val context = LocalContext.current
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val fonts = LocalAppFonts.current
    return when (status) {
        is ThreadStatus.Failed -> {
            val login = if (status.login) {
                val hint = agents.catalog?.agents?.firstOrNull { it.id == thread.agent }?.loginHint?.takeIf { it.isNotBlank() }
                    ?: if (thread.agent == "codex") "codex login" else "claude auth login"
                stringResource(R.string.agents_login_failed, agents.agentName(thread.agent), hint, pcName)
            } else {
                null
            }
            val word = stringResource(R.string.agents_state_failed)
            val message = status.message
            val style = MaterialTheme.typography.bodySmall.mono(fonts).copy(fontSize = 12.5.sp, lineHeight = 19.sp)
            val loginStyle = MaterialTheme.typography.bodySmall
            val end: LazyListScope.() -> Unit = {
                item(key = "end", contentType = "end") {
                    Column(Modifier.fillMaxWidth().padding(top = 12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        EndLine(AgentGlyphs.warning, tints.red, word, scheme.error)
                        if (message != null) Text(message, style = style, color = scheme.onSurfaceVariant, modifier = Modifier.padding(start = 42.dp))
                        if (login != null) Text(login, style = loginStyle, modifier = Modifier.padding(start = 42.dp))
                    }
                }
            }
            end
        }
        is ThreadStatus.Paused -> {
            val text = status.resetsMs?.let {
                stringResource(R.string.agents_paused_resets, DateFormat.getTimeFormat(context).format(Date(it)))
            } ?: stringResource(R.string.agents_usage_limit)
            val end: LazyListScope.() -> Unit = {
                item(key = "end", contentType = "end") {
                    Box(Modifier.padding(top = 12.dp)) { EndLine(G7Icons.history, tints.amber, text, tints.amber) }
                }
            }
            end
        }
        else -> null
    }
}

@Composable
private fun EndLine(icon: ImageVector, tint: Color, text: String, color: Color) {
    Row(Modifier.fillMaxWidth().heightIn(min = 34.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.width(18.dp))
        Icon(icon, contentDescription = null, tint = tint, modifier = Modifier.size(16.dp))
        Text(
            text,
            style = MaterialTheme.typography.bodyMedium.copy(fontWeight = FontWeight.Medium),
            color = color,
            modifier = Modifier.padding(start = 8.dp),
        )
    }
}

/**
 * The plan, pinned above the reply box (reconcile file, item 1): "Tasks 4/5 ·
 * <the step under way>"; it opens upward into the checklist and never scrolls
 * with the thread.
 */
@Composable
private fun TasksLine(plan: List<AgentPlanStep>, key: String) {
    val scheme = MaterialTheme.colorScheme
    var shown by rememberSaveable(key) { mutableStateOf(false) }
    val done = AgentsModel.planDone(plan)
    val current = AgentsModel.planPosition(plan)?.let { plan.getOrNull(it - 1)?.text }.orEmpty()
    Column(Modifier.fillMaxWidth().padding(bottom = 4.dp)) {
        AnimatedVisibility(visible = shown, enter = g7Expand(), exit = g7Collapse()) {
            val line = scheme.outlineVariant
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(max = 240.dp)
                    .padding(start = 7.dp, bottom = 2.dp)
                    .drawBehind { drawLine(line, Offset(0f, 0f), Offset(0f, size.height), 1.dp.toPx()) }
                    .verticalScroll(rememberScrollState())
                    .padding(start = 15.dp),
            ) {
                for (step in plan) PlanStepRow(step)
            }
        }
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = 38.dp)
                .clip(RoundedCornerShape(6.dp))
                .clickable(role = Role.Button) { shown = !shown },
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.width(14.dp), contentAlignment = Alignment.Center) {
                Icon(if (shown) G7Icons.chevronDown else AgentGlyphs.chevronUp, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
            }
            Box(Modifier.width(4.dp))
            Icon(G7Icons.clipboardList, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
            Text(
                stringResource(R.string.agents_tasks, done, plan.size),
                style = MaterialTheme.typography.bodyMedium,
                color = scheme.onSurfaceVariant,
                maxLines = 1,
                modifier = Modifier.padding(start = 8.dp),
            )
            if (current.isNotBlank()) {
                Text(" · ", style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant, maxLines = 1)
                Text(current, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            }
        }
    }
}

@Composable
private fun PlanStepRow(step: AgentPlanStep) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    Row(Modifier.heightIn(min = 30.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Box(Modifier.size(14.dp), contentAlignment = Alignment.Center) {
            when (step.status) {
                AgentPlanStatus.COMPLETED -> Icon(G7Icons.check, contentDescription = null, tint = tints.teal, modifier = Modifier.size(14.dp))
                AgentPlanStatus.IN_PROGRESS -> Box(Modifier.size(7.dp).clip(CircleShape).background(tints.blue))
                AgentPlanStatus.PENDING -> Box(Modifier.size(6.dp).border(1.5.dp, scheme.outline, CircleShape))
            }
        }
        Text(
            step.text,
            style = MaterialTheme.typography.bodyMedium.copy(fontWeight = if (step.status == AgentPlanStatus.IN_PROGRESS) FontWeight.Medium else FontWeight.Normal),
            color = if (step.status == AgentPlanStatus.IN_PROGRESS) scheme.onSurface else scheme.onSurfaceVariant,
        )
    }
}

/**
 * The request the thread waits on, as a sheet over the reply box (the phone
 * board, 4C): who wants what and "1 of 2", the command with its folder or the
 * change, the agent's reason as its own words, Deny and Allow. Allow waits until
 * all of it has been on screen and isn't offered for one the PC cut. A held one
 * shows with its buttons off. It never covers more than half the thread and
 * doesn't block scrolling it; its handle folds it to its first line.
 */
@Composable
private fun RequestSheet(
    queue: AskQueue,
    agentName: String,
    folder: String,
    answering: Boolean,
    allowPrimary: Boolean,
    collapsed: Boolean,
    onToggle: () -> Unit,
    maxHeight: Dp,
    onAnswer: (Boolean) -> Unit,
    onReview: () -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val fonts = LocalAppFonts.current
    val ask = queue.shown
    // A new request is a new sheet: its own "read it all" before Allow.
    key(ask.token) {
        val scroll = rememberScrollState()
        val doc = rememberDiff(if (ask.kind == AgentAskKind.EDIT) ask.diff else null)
        val body = remember(ask) { AgentsModel.askBody(ask) }
        val fitsHere = ask.kind != AgentAskKind.EDIT || ask.diff.isNullOrBlank() || (doc != null && !doc.more && diffRows(doc) <= SHEET_DIFF_ROWS)
        val ready = ask.kind != AgentAskKind.EDIT || ask.diff.isNullOrBlank() || doc != null
        val seenHere = rememberSeenToEnd(ask.token, scroll, ready = ready && fitsHere)
        val seenAll = body == null || (fitsHere && seenHere)
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(max = maxHeight)
                .clip(RoundedCornerShape(topStart = 12.dp, topEnd = 12.dp))
                .background(scheme.surfaceContainer)
                .padding(start = 14.dp, end = 14.dp, bottom = 14.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Box(
                Modifier
                    .fillMaxWidth()
                    .height(18.dp)
                    .clickable(role = Role.Button, onClick = onToggle),
                contentAlignment = Alignment.BottomCenter,
            ) {
                Box(Modifier.width(32.dp).height(4.dp).clip(RoundedCornerShape(2.dp)).background(scheme.outline))
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                val (icon, title) = when (ask.kind) {
                    AgentAskKind.COMMAND -> G7Icons.terminal to R.string.agents_dock_wants_command
                    AgentAskKind.EDIT -> G7Icons.edit to R.string.agents_dock_wants_edit
                    AgentAskKind.OTHER -> G7Icons.help to R.string.agents_dock_wants_other
                }
                Icon(icon, contentDescription = null, tint = tints.amber, modifier = Modifier.size(16.dp))
                Text(
                    stringResource(title, agentName),
                    style = MaterialTheme.typography.titleSmall.copy(fontWeight = FontWeight.SemiBold),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                if (queue.total > 1) {
                    Text(
                        stringResource(R.string.agents_dock_count, queue.position, queue.total),
                        style = MaterialTheme.typography.labelMedium,
                        color = scheme.onSurfaceVariant,
                        maxLines = 1,
                    )
                }
            }
            if (!collapsed) {
                Column(
                    modifier = Modifier.weight(1f, fill = false).verticalScroll(scroll),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    when {
                        ask.kind == AgentAskKind.EDIT && doc != null -> Column(
                            Modifier
                                .fillMaxWidth()
                                .clip(RoundedCornerShape(6.dp))
                                .background(scheme.surface)
                                .clickable(role = Role.Button, onClick = onReview)
                                .padding(vertical = 8.dp),
                            verticalArrangement = Arrangement.spacedBy(6.dp),
                        ) {
                            Row(Modifier.padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                                FolderLabel(listOfNotNull(folder.ifBlank { null }, ask.path?.takeIf { it.isNotBlank() }).joinToString(" · "), Modifier.weight(1f))
                                Counts(doc.adds.toLong(), doc.dels.toLong())
                            }
                            DiffView(doc, wrap = true, maxRows = SHEET_DIFF_ROWS)
                            if (!fitsHere) SmallTextButton(stringResource(R.string.agents_show_all), onClick = onReview)
                        }
                        body != null -> Column(
                            Modifier
                                .fillMaxWidth()
                                .clip(RoundedCornerShape(6.dp))
                                .background(scheme.surface)
                                .padding(horizontal = 12.dp, vertical = 10.dp),
                            verticalArrangement = Arrangement.spacedBy(4.dp),
                        ) {
                            FolderLabel(folder)
                            Text(body, style = MaterialTheme.typography.bodySmall.mono(fonts).copy(fontSize = 13.sp, lineHeight = 19.5.sp))
                        }
                        else -> Text(ask.title, style = MaterialTheme.typography.bodyMedium)
                    }
                    if (ask.truncated) {
                        Text(stringResource(R.string.agents_ask_truncated), style = MaterialTheme.typography.bodySmall, color = tints.amber)
                    }
                    ask.reason?.takeIf { it.isNotBlank() }?.let { AgentSays(agentName, it) }
                }
                AnswerButtons(
                    queue = queue,
                    answering = answering,
                    seenAll = seenAll,
                    allowPrimary = allowPrimary,
                    onAnswer = onAnswer,
                )
            }
        }
    }
}

/**
 * The reply box: the text, Stop beside the button whenever a turn runs or waits
 * (reconcile file, the dock, item 3), and the button: send, or "Deny and send"
 * when it has words and a request is shown; under it, for Codex, that it stops
 * and reads them next; and where it runs.
 */
@Composable
private fun ReplyBox(
    view: ComposerView,
    hint: String,
    draft: String,
    onDraft: (String) -> Unit,
    onFocus: (Boolean) -> Unit,
    sending: Boolean,
    stopping: Boolean,
    onStop: () -> Unit,
    onSend: () -> Unit,
    joined: Boolean,
    place: String,
) {
    val scheme = MaterialTheme.colorScheme
    val shape = if (joined) RoundedCornerShape(bottomStart = 12.dp, bottomEnd = 12.dp) else RoundedCornerShape(12.dp)
    val divider = scheme.outlineVariant
    Column(Modifier.fillMaxWidth().padding(bottom = 8.dp)) {
        if (AgentsModel.tooLong(draft)) {
            Text(
                stringResource(R.string.agents_too_long),
                style = MaterialTheme.typography.bodySmall,
                color = scheme.error,
                modifier = Modifier.padding(start = 4.dp, bottom = 4.dp),
            )
        }
        Column(
            Modifier
                .fillMaxWidth()
                .clip(shape)
                .background(scheme.surfaceContainer)
                .then(if (joined) Modifier.drawBehind { drawLine(divider, Offset(0f, 0f), Offset(size.width, 0f), 1.dp.toPx()) } else Modifier)
                .padding(end = 8.dp, bottom = 8.dp),
        ) {
            AgentTextField(
                value = draft,
                onValueChange = onDraft,
                hint = hint,
                enabled = view.enabled,
                maxLines = 5,
                background = Color.Transparent,
                onFocus = onFocus,
            )
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End),
            ) {
                if (view.showStop) {
                    ComposerPill(G7Icons.stop, stringResource(R.string.agents_stop), accent = false, enabled = !stopping, onClick = onStop)
                }
                if (sending) {
                    Box(Modifier.size(44.dp), contentAlignment = Alignment.Center) {
                        CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                    }
                } else {
                    when (view.action) {
                        ComposerAction.NONE -> Unit
                        ComposerAction.SEND -> G7IconButton(
                            icon = G7Icons.send,
                            contentDescription = stringResource(R.string.agents_send),
                            onClick = onSend,
                            enabled = view.actionEnabled,
                            filled = true,
                            tint = if (view.actionEnabled) scheme.primary else scheme.onSurfaceVariant,
                        )
                        ComposerAction.DENY_AND_SEND -> ComposerPill(
                            G7Icons.send,
                            stringResource(R.string.agents_deny_and_send),
                            accent = true,
                            enabled = view.actionEnabled,
                            onClick = onSend,
                        )
                    }
                }
            }
        }
        if (view.codexNote) {
            Text(
                stringResource(R.string.agents_codex_reads_next),
                style = MaterialTheme.typography.bodySmall,
                color = scheme.onSurfaceVariant,
                textAlign = TextAlign.End,
                modifier = Modifier.fillMaxWidth().padding(top = 4.dp, end = 4.dp),
            )
        }
        FolderLabel(place, Modifier.padding(start = 4.dp, top = 6.dp))
    }
}

/** A button of the reply box with its glyph and word: Stop, or "Deny and send" in the accent. */
@Composable
private fun ComposerPill(icon: ImageVector, label: String, accent: Boolean, enabled: Boolean, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val on = accent && enabled
    val background = if (on) scheme.primary else scheme.surfaceContainerHigh
    val content = when {
        on -> scheme.onPrimary
        enabled -> scheme.onSurface
        else -> scheme.onSurfaceVariant
    }
    Row(
        modifier = Modifier
            .height(44.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(background)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(start = 12.dp, end = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Icon(icon, contentDescription = null, tint = content, modifier = Modifier.size(20.dp))
        Text(label, style = MaterialTheme.typography.labelLarge.copy(fontWeight = if (accent) FontWeight.SemiBold else FontWeight.Medium), color = content, maxLines = 1)
    }
}
