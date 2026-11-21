// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import android.text.format.DateFormat
import android.text.format.DateUtils
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
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
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
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
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLocale
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.Preferences
import app.g7link.android.ui.theme.G7Header
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.G7Row
import app.g7link.android.ui.theme.G7SectionLabel
import app.g7link.android.ui.theme.g7Tints
import app.g7link.core.AgentCatalog
import app.g7link.core.AgentInfo
import app.g7link.core.AgentSession
import app.g7link.core.AgentSessionSource
import app.g7link.core.AgentThread
import java.util.Date
import java.util.TimeZone
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** How far the thread list got. */
private enum class ListLoad { LOADING, READY, NOT_ALLOWED, UNAVAILABLE, UNREACHABLE, FAILED }

/** What fills the Agents screen: the list, or a screen opened from its top bar. */
private enum class AgentsPage { LIST, SEARCH, IMPORT }

/**
 * A PC's agent threads (board 16): the sections, the filters with the
 * Archived and Snoozed shelves, a swipe to archive or pin and a long-press
 * sheet for the rest; search and import from the top bar. What this phone
 * saved shows at once, and is all there is while the PC can't be reached
 * (contract §U9.3 "Offline"); before any thread shows, the states the phone
 * can be in (not allowed yet, the PC out of reach, nothing signed in, no
 * folders).
 */
@Composable
fun AgentsScreen(
    device: Device,
    core: Core,
    onOpenThread: (String) -> Unit,
    onOpenSession: (String) -> Unit,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val resources = LocalResources.current
    val all by core.agents.collectAsStateWithLifecycle()
    val agents = all[device.id] ?: AgentsState()
    val catalog = agents.catalog
    var load by remember(device.id) { mutableStateOf(ListLoad.LOADING) }
    var attempt by remember(device.id) { mutableIntStateOf(0) }
    var filter by rememberSaveable { mutableStateOf(ThreadFilter.ALL) }
    var page by rememberSaveable { mutableStateOf(AgentsPage.LIST) }
    // Shared from another app to a new thread (contract §V6): the new-thread sheet opens with it in the box.
    var sharedNew by remember(device.id) { mutableStateOf(AgentShared.take(device.id, null)) }
    var newThread by rememberSaveable { mutableStateOf(sharedNew != null) }
    var settings by rememberSaveable { mutableStateOf(false) }
    var menuFor by rememberSaveable { mutableStateOf<String?>(null) }
    var snoozeFor by rememberSaveable { mutableStateOf<String?>(null) }
    var renameFor by rememberSaveable { mutableStateOf<String?>(null) }
    val hasCatalog = catalog != null
    val sessionsAllowed by rememberUpdatedState(agents.sessionsAllowed)
    val scope = rememberCoroutineScope()
    val offline = !device.online

    // What this phone saved, at once: shown until the PC answers, and all there is while it can't.
    LaunchedEffect(device.id) { core.agentSaved(device.id) }
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
        // Sessions started on the PC, only when asking can't make the PC prompt unasked (it listed them before, or the user asked).
        if (load == ListLoad.READY && sessionsAllowed != false) core.agentSessionsIfAllowed(device.id)
    }
    // A shelf is read from the PC when it's shown.
    LaunchedEffect(device.id, device.online, filter, load) {
        if (device.online && load == ListLoad.READY && filter.shelf != Shelf.MAIN) core.agentShelf(device.id, filter.shelf)
    }
    // The user asked on the PC: ask now (the PC may show its prompt), then look again now and then while this is on screen.
    var askedSessions by remember(device.id) { mutableStateOf(false) }
    val sessionsListed = agents.sessionsAllowed == true
    val sessionsUnsupported = agents.sessionsUnsupported
    LaunchedEffect(device.id, askedSessions, sessionsListed, sessionsUnsupported, device.online) {
        if (!askedSessions || sessionsListed || sessionsUnsupported || !device.online) return@LaunchedEffect
        repeat(SESSIONS_ASK_TRIES) {
            core.agentSessionsAsk(device.id)
            delay(SESSIONS_ASK_EVERY_MS)
        }
    }

    BackHandler(enabled = page != AgentsPage.LIST) { page = AgentsPage.LIST }
    when (page) {
        AgentsPage.SEARCH -> {
            AgentSearchScreen(
                device = device,
                core = core,
                agents = agents,
                onOpenThread = onOpenThread,
                onImport = { page = AgentsPage.IMPORT },
                onBack = { page = AgentsPage.LIST },
                modifier = modifier,
            )
            return
        }
        AgentsPage.IMPORT -> {
            AgentImportScreen(
                device = device,
                core = core,
                agents = agents,
                onOpenThread = onOpenThread,
                onBack = { page = AgentsPage.LIST },
                modifier = modifier,
            )
            return
        }
        AgentsPage.LIST -> Unit
    }

    val canStart = !offline && catalog != null && agents.readyAgents.isNotEmpty() && catalog.folders.isNotEmpty()
    val retry: () -> Unit = { attempt++ }
    // The saved list stands in while the PC can't be reached or hasn't answered yet.
    val savedShown = load != ListLoad.READY && agents.savedMs != null && agents.threads.isNotEmpty() &&
        (load == ListLoad.LOADING || load == ListLoad.UNREACHABLE)
    val listShown = (load == ListLoad.READY && catalog != null) || savedShown
    val failedSay: (AgentFailure) -> String = { failure ->
        when (failure) {
            AgentFailure.DENIED -> resources.getString(R.string.agents_access_title, device.name)
            AgentFailure.UNSUPPORTED -> resources.getString(R.string.agents_unavailable, device.name)
            AgentFailure.NOT_FOUND -> resources.getString(R.string.agents_thread_gone)
            AgentFailure.OFFLINE, AgentFailure.TIMEOUT -> resources.getString(R.string.agents_unreachable_title, device.name)
            // The PC's BUSY: archiving one that works or waits.
            AgentFailure.OTHER -> resources.getString(R.string.agents_wait_until_finishes)
        }
    }
    val mark: (AgentThread, MarkChange) -> Unit = { thread, change ->
        if (offline) {
            core.say(resources.getString(R.string.agents_needs_pc, device.name))
        } else {
            scope.launch {
                val result = core.agentMark(device.id, listOf(thread.id), change)
                if (result is AgentCall.Failed) core.say(failedSay(result.failure))
            }
        }
    }
    val rowActions = remember(device.id, offline) {
        ThreadRowActions(
            open = onOpenThread,
            menu = { id -> menuFor = id },
            archive = { thread -> mark(thread, MarkChange.Archive(!thread.archived)) },
            pin = { thread -> mark(thread, MarkChange.Pin(!thread.pinned)) },
        )
    }
    val drawer = rememberAgentsDrawer()
    AgentsDrawer(
        device = device,
        core = core,
        control = drawer,
        selected = null,
        onOpenThread = onOpenThread,
        onOpenSession = onOpenSession,
        modifier = modifier,
    ) {
        Column(Modifier.fillMaxSize()) {
            G7Header(
                title = stringResource(R.string.agents_title),
                onBack = onBack,
                note = device.name,
                noteColor = if (device.online) MaterialTheme.colorScheme.onSurfaceVariant else MaterialTheme.colorScheme.error,
            ) {
                AgentsDrawerButton(drawer)
                G7IconButton(
                    icon = G7Icons.search,
                    contentDescription = stringResource(R.string.agents_search),
                    onClick = { page = AgentsPage.SEARCH },
                    enabled = !offline && listShown,
                )
                G7IconButton(
                    icon = G7Icons.plus,
                    contentDescription = stringResource(R.string.agents_new_thread),
                    onClick = { newThread = true },
                    enabled = canStart && load == ListLoad.READY,
                    tint = if (canStart && load == ListLoad.READY) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
                )
                ListMenu(
                    canImport = !offline && load == ListLoad.READY,
                    onImport = { page = AgentsPage.IMPORT },
                    onSettings = { settings = true },
                )
            }
            if (offline && listShown) OfflineBar(device.name, agents.savedMs ?: agents.listedMs, rememberNow())
            when {
                listShown -> ThreadList(
                    pcName = device.name,
                    agents = agents,
                    catalog = catalog,
                    filter = filter,
                    onFilter = { filter = it },
                    canStart = canStart,
                    onNewThread = { newThread = true },
                    rowActions = rowActions,
                    onOpenSession = onOpenSession,
                    askedSessions = askedSessions,
                    onAskSessions = { askedSessions = true },
                )
                load == ListLoad.LOADING -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator(Modifier.size(28.dp), strokeWidth = 2.dp)
                }
                load == ListLoad.UNREACHABLE -> AgentsEmptyState(
                    title = stringResource(R.string.agents_unreachable_title, device.name),
                    body = stringResource(R.string.agents_unreachable_body),
                    action = stringResource(R.string.agents_try_again),
                    onAction = retry,
                )
                load == ListLoad.NOT_ALLOWED -> AgentsEmptyState(
                    title = stringResource(R.string.agents_access_title, device.name),
                    body = stringResource(R.string.agents_access_body),
                    action = stringResource(R.string.agents_try_again),
                    onAction = retry,
                )
                load == ListLoad.UNAVAILABLE || load == ListLoad.READY -> AgentsEmptyState(
                    title = stringResource(R.string.agents_unavailable, device.name),
                    action = stringResource(R.string.agents_try_again),
                    onAction = retry,
                )
                else -> AgentsEmptyState(
                    title = stringResource(R.string.agents_failed_generic),
                    action = stringResource(R.string.agents_try_again),
                    onAction = retry,
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
            shared = sharedNew,
            onStarted = { id ->
                newThread = false
                sharedNew = null
                onOpenThread(id)
            },
            onDismiss = {
                newThread = false
                sharedNew = null
            },
        )
    }
    if (settings) AgentCacheSheet(core = core, pcId = device.id, pcName = device.name, onDismiss = { settings = false })
    menuFor?.let { id -> agents.thread(id) }?.let { thread ->
        ThreadActionsSheet(
            thread = thread,
            place = listOf(agents.agentName(thread.agent), thread.folder).filter { it.isNotBlank() }.joinToString(" · "),
            pcName = device.name,
            offline = offline,
            onAction = { action ->
                menuFor = null
                when (action) {
                    ThreadAction.SNOOZE -> snoozeFor = thread.id
                    ThreadAction.RENAME -> renameFor = thread.id
                    else -> AgentMarks.change(action)?.let { mark(thread, it) }
                }
            },
            onDismiss = { menuFor = null },
        )
    }
    snoozeFor?.let { id -> agents.thread(id) }?.let { thread ->
        val now = System.currentTimeMillis()
        SnoozeSheet(
            thread = thread,
            agentName = agents.agentName(thread.agent),
            limitResetsMs = AgentMarks.limitResets(agents.abilities(thread.agent).limits, now),
            nowMs = now,
            onPick = { at ->
                snoozeFor = null
                mark(thread, MarkChange.Snooze(at))
            },
            onDismiss = { snoozeFor = null },
        )
    }
    renameFor?.let { id -> agents.thread(id) }?.let { thread ->
        RenameSheet(
            thread = thread,
            onRename = { title ->
                renameFor = null
                scope.launch {
                    val result = core.agentRename(device.id, thread.id, title)
                    if (result is AgentCall.Failed) core.say(failedSay(result.failure))
                }
            },
            onDismiss = { renameFor = null },
        )
    }
}

/** The list's More: bring in earlier sessions, and the agents' settings on this phone. */
@Composable
private fun ListMenu(canImport: Boolean, onImport: () -> Unit, onSettings: () -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        G7IconButton(icon = AgentGlyphs.more, contentDescription = stringResource(R.string.agents_more), onClick = { open = true })
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            DropdownMenuItem(
                text = { Text(stringResource(R.string.agents_import_title)) },
                enabled = canImport,
                onClick = {
                    open = false
                    onImport()
                },
            )
            DropdownMenuItem(
                text = { Text(stringResource(R.string.agents_phone_settings)) },
                onClick = {
                    open = false
                    onSettings()
                },
            )
        }
    }
}

@Composable
private fun ThreadList(
    pcName: String,
    agents: AgentsState,
    catalog: AgentCatalog?,
    filter: ThreadFilter,
    onFilter: (ThreadFilter) -> Unit,
    canStart: Boolean,
    onNewThread: () -> Unit,
    rowActions: ThreadRowActions,
    onOpenSession: (String) -> Unit,
    askedSessions: Boolean,
    onAskSessions: () -> Unit,
) {
    val now = rememberNow()
    val listed = agents.listed
    // The chips count threads only: sessions have their own section.
    val needsYou = remember(listed) { listed.count { ThreadFilter.NEEDS_YOU.matches(it) } }
    val working = remember(listed) { listed.count { ThreadFilter.WORKING.matches(it) } }
    LazyColumn(
        modifier = Modifier.fillMaxSize(),
        contentPadding = PaddingValues(start = 12.dp, end = 12.dp, bottom = 16.dp),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        if (catalog != null && agents.readyAgents.isEmpty()) {
            item(key = "none_ready", contentType = "setup") {
                Column(Modifier.padding(bottom = 8.dp)) {
                    SetupNote(stringResource(R.string.agents_none_ready, pcName))
                    AgentRows(catalog.agents, selected = null, onSelect = {})
                }
            }
        } else if (catalog != null && catalog.folders.isEmpty()) {
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
                ShelfChips(selected = filter, needsYou = needsYou, working = working, onSelect = onFilter)
            }
            threadRows(agents, filter, now, rowActions)
        }
        // Sessions started on the PC outside G7: listed once the PC allows it, else a way to ask (never asked unprompted).
        if (!agents.sessionsUnsupported && filter.shelf == Shelf.MAIN) {
            item(key = "sessions_label", contentType = "label") {
                G7SectionLabel(stringResource(R.string.agents_session_section, pcName), Modifier.padding(top = 8.dp))
            }
            when {
                agents.sessionsAllowed != true -> item(key = "sessions_off", contentType = "sessions_off") {
                    SessionsOff(pcName = pcName, asked = askedSessions, onAsk = onAskSessions)
                }
                agents.sessions.isEmpty() -> item(key = "sessions_none", contentType = "setup") {
                    SetupNote(stringResource(R.string.agents_session_none, pcName))
                }
                else -> items(agents.sessions, key = { "session:${it.id}" }, contentType = { "session" }) { session ->
                    SessionRow(
                        session = session,
                        agentName = agents.agentName(session.agent),
                        pcName = pcName,
                        now = now,
                        onOpen = { onOpenSession(session.id) },
                    )
                }
            }
        }
    }
}

/**
 * The PC hasn't let this phone see the sessions started on it: what they
 * are, and a way to ask (the PC then shows its user a prompt).
 */
@Composable
internal fun SessionsOff(pcName: String, asked: Boolean, onAsk: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .border(1.dp, scheme.outlineVariant, RoundedCornerShape(8.dp))
            .padding(horizontal = 14.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Icon(G7Icons.terminal, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(18.dp))
            Text(stringResource(R.string.agents_session_off_body, pcName), style = MaterialTheme.typography.bodyMedium)
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            G7RaisedButton(label = stringResource(R.string.agents_session_ask, pcName), onClick = onAsk, modifier = Modifier.padding(horizontal = 4.dp))
            Text(
                if (asked) stringResource(R.string.agents_session_allow_on, pcName) else stringResource(R.string.agents_session_off_note),
                style = MaterialTheme.typography.bodySmall,
                color = scheme.onSurfaceVariant,
                modifier = Modifier.weight(1f),
            )
        }
    }
}

/** A session started on the PC: its state, title, agent and folder and where it runs, and what to do about it. */
@Composable
private fun SessionRow(session: AgentSession, agentName: String, pcName: String, now: Long, onOpen: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(scheme.surfaceContainer)
            .clickable(role = Role.Button, onClick = onOpen)
            .padding(horizontal = 14.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        SessionStatusLine(session = session, now = now, trailing = relativeTime(session.updatedMs, now))
        Text(session.title, style = MaterialTheme.typography.bodyLarge, maxLines = 2, overflow = TextOverflow.Ellipsis)
        Text(
            sessionPlace(session, agentName),
            style = MaterialTheme.typography.bodySmall,
            color = scheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        val line = when {
            AgentsModel.needsYouExactly(session) -> stringResource(R.string.agents_session_answer_on, pcName)
            AgentsModel.mayNeedYou(session) -> mayWait(session, agentName, pcName)
            else -> null
        }
        if (line != null) {
            Text(line, style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
        } else if (AgentsModel.sessionLook(session) == SessionLook.WORKING) {
            NowLine(step = session.step, sinceMs = session.sinceMs)
        }
    }
}

/** "Claude · g7linux · in a terminal": the agent, the folder's name and where it runs. */
@Composable
internal fun sessionPlace(session: AgentSession, agentName: String): String {
    val where = when (session.source) {
        AgentSessionSource.TERMINAL -> stringResource(R.string.agents_session_in_terminal)
        AgentSessionSource.DESKTOP_APP -> stringResource(R.string.agents_session_in_app, agentName)
        AgentSessionSource.OTHER -> null
    }
    return listOfNotNull(agentName, session.folder.ifBlank { null }, where).joinToString(" · ")
}

/** A guessed wait: it may be a prompt or a long command, so it's checked on the PC. */
@Composable
internal fun mayWait(session: AgentSession, agentName: String, pcName: String): String =
    if (session.source == AgentSessionSource.TERMINAL) {
        stringResource(R.string.agents_session_may_wait_terminal, agentName, pcName)
    } else {
        stringResource(R.string.agents_session_may_wait, agentName, pcName)
    }

/**
 * A session's state as a dot and a word: Needs you (amber), May need you (an
 * amber ring: a guess), Working for 6 min, Idle, Ended.
 */
@Composable
internal fun SessionStatusLine(session: AgentSession, now: Long, trailing: String?, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    Row(
        modifier = modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        val (word, color) = sessionWord(session, now)
        SessionLookMark(AgentsModel.sessionLook(session))
        Text(word, style = MaterialTheme.typography.labelMedium, color = color, maxLines = 1)
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

/**
 * A session's state in words and their colour: Needs you, May need you,
 * Working for 6 min, Limit reached · resets 18:00, Idle, Ended.
 */
@Composable
internal fun sessionWord(session: AgentSession, now: Long): Pair<String, Color> {
    val scheme = MaterialTheme.colorScheme
    val p = agentPalette()
    return when (AgentsModel.sessionLook(session)) {
        SessionLook.NEEDS_YOU -> stringResource(R.string.agents_state_waiting) to p.statusNeedsYou
        SessionLook.MAY_NEED_YOU -> stringResource(R.string.agents_session_state_may_need_you) to p.statusMayNeedYou
        SessionLook.WORKING -> workingFor(session.sinceMs, now) to p.statusWorking
        SessionLook.LIMITED -> limitReached(AgentsModel.sessionLimitResets(session, now), now) to p.statusPaused
        SessionLook.IDLE -> stringResource(R.string.agents_session_state_idle) to scheme.onSurfaceVariant
        SessionLook.ENDED -> stringResource(R.string.agents_session_state_ended) to scheme.onSurfaceVariant
    }
}

/** "Limit reached · resets 18:00" (or "· resets Mon 18:00" another day); "Limit reached" when it's not said or past. */
@Composable
internal fun limitReached(resetsMs: Long?, now: Long): String {
    if (resetsMs == null) return stringResource(R.string.agents_session_limit)
    val context = LocalContext.current
    val locale = LocalLocale.current.platformLocale
    val at = if (AgentsModel.sameDay(resetsMs, now, TimeZone.getDefault())) {
        DateFormat.getTimeFormat(context).format(Date(resetsMs))
    } else {
        DateFormat.format(DateFormat.getBestDateTimePattern(locale, "EEE jm"), resetsMs).toString()
    }
    return stringResource(R.string.agents_session_limit_resets, at)
}

/** A session's mark: needs you, may need you (a hollow amber ring), at work, or still (a usage limit, idle, ended). */
@Composable
internal fun SessionLookMark(look: SessionLook) {
    SessionStatusMark(
        needsYou = look == SessionLook.NEEDS_YOU,
        mayNeedYou = look == SessionLook.MAY_NEED_YOU,
        working = look == SessionLook.WORKING,
    )
}

/** "Working for 4 min" from when it began working; "Working" when the PC didn't say. */
@Composable
internal fun workingFor(sinceMs: Long?, now: Long): String {
    if (sinceMs == null || sinceMs <= 0L) return stringResource(R.string.agents_state_working)
    val minutes = ((now - sinceMs).coerceAtLeast(0L) / 60_000L).coerceAtLeast(1L)
    return if (minutes < 60L) {
        pluralStringResource(R.plurals.agents_step_working_for_minutes, minutes.toInt(), minutes.toInt())
    } else {
        val hours = (minutes / 60L).toInt()
        pluralStringResource(R.plurals.agents_step_working_for_hours, hours, hours)
    }
}

/** "Now: Run command · 4:12" on one line: the step is cut short before the time is. */
@Composable
internal fun NowLine(step: String?, sinceMs: Long?, modifier: Modifier = Modifier) {
    val style = MaterialTheme.typography.bodySmall
    val color = MaterialTheme.colorScheme.onSurface
    Row(modifier, verticalAlignment = Alignment.CenterVertically) {
        Text(
            step?.takeIf { it.isNotBlank() }?.let { stringResource(R.string.agents_step_now, it) } ?: stringResource(R.string.agents_state_working),
            style = style,
            color = color,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f, fill = false),
        )
        if (sinceMs != null && sinceMs > 0L) {
            Text(" · ", style = style, color = color, maxLines = 1)
            ElapsedText(sinceMs, style = style, color = color)
        }
    }
}

/** The time since [sinceMs] as a clock ("4:12"), ticking each second; only this text redraws. */
@Composable
internal fun ElapsedText(sinceMs: Long, style: TextStyle, color: Color, modifier: Modifier = Modifier) {
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(sinceMs) {
        while (true) {
            now = System.currentTimeMillis()
            delay(1_000L - (now - sinceMs).mod(1_000L))
        }
    }
    Text(AgentsModel.elapsed(now - sinceMs), style = style, color = color, maxLines = 1, modifier = modifier)
}

/** The new-thread sheet: the task, the agent (when there's a choice), the folder. Always asks before each change. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun NewThreadSheet(
    device: Device,
    core: Core,
    catalog: AgentCatalog,
    threads: List<AgentThread>,
    onStarted: (String) -> Unit,
    onDismiss: () -> Unit,
    shared: SharedDraft? = null,
) {
    val resources = LocalResources.current
    val context = LocalContext.current
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val ready = catalog.agents.filter { it.installed && it.signedIn }
    // The agent and folder picked last time on this PC, while they can still be picked.
    val saved = remember(device.id) { Preferences.agentChoice(context, device.id) }
    var agentId by rememberSaveable { mutableStateOf(saved.first?.takeIf { id -> ready.any { it.id == id } } ?: ready.firstOrNull()?.id) }
    var folderId by rememberSaveable {
        mutableStateOf(saved.second?.takeIf { id -> catalog.folders.any { it.id == id } } ?: catalog.folders.firstOrNull()?.id)
    }
    var prompt by rememberSaveable { mutableStateOf(shared?.text.orEmpty()) }
    var busy by remember { mutableStateOf(false) }
    // A share's pictures, uploaded as the sheet opens and sent with Start; never anything sent before it.
    var images by remember { mutableStateOf(emptyList<PendingImage>()) }
    val thumbs = remember(device.id) { ImageThumbs { id -> (core.agentImageThumb(device.id, id) as? AgentCall.Ok)?.value } }
    LaunchedEffect(shared) {
        if (shared == null) return@LaunchedEffect
        if (shared.shortened) core.say(resources.getString(R.string.agents_share_shortened))
        for (path in shared.images) {
            val key = path
            images = AgentImages.adding(images, key)
            if (images.none { it.key == key }) {
                AgentImageIntake.deleteCameraFile(path)
                continue
            }
            val problem = when (val read = AgentImageIntake.read(context, android.net.Uri.fromFile(java.io.File(path)))) {
                is ImageRead.Problem -> read.problem
                is ImageRead.Bytes -> when (val sent = core.agentImageSend(device.id, read.name, read.bytes)) {
                    is AgentCall.Ok -> {
                        images = AgentImages.uploaded(images, key, sent.value)
                        null
                    }
                    is AgentCall.Failed -> ImageProblem.FAILED
                }
            }
            // The copy lives only until it's on the PC.
            AgentImageIntake.deleteCameraFile(path)
            if (problem != null) {
                images = AgentImages.removed(images, key)
                core.say(resources.getString(R.string.agents_image_failed))
            }
        }
    }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val agent = ready.firstOrNull { it.id == agentId }
    val folder = catalog.folders.firstOrNull { it.id == folderId }
    // The model, effort and mode it starts with (contract §Q6.3): the last ones for this agent and folder, Plan chosen per task.
    val abilities = remember(agent, catalog.canControl) { AgentControlFfi.abilities(agent, catalog.canControl) }
    var chosen by remember(agentId, folderId) {
        mutableStateOf(if (agent != null && folder != null) AgentChoices.get(device.id, agent.id, folder.label) else ThreadSettings())
    }
    var settingsOpen by remember { mutableStateOf(false) }
    val tooLong = AgentsModel.tooLong(prompt)
    val imagesTaken = images.isEmpty() || AgentSettings.takesImages(abilities, chosen)
    val canStart = !busy && agent != null && folder != null && AgentsModel.fits(prompt) && !AgentImages.uploading(images) && imagesTaken

    fun start() {
        if (!canStart || agent == null || folder == null) return
        busy = true
        error = null
        scope.launch {
            when (val result = core.agentStart(device.id, agent.id, folder.id, prompt.trim(), chosen, AgentImages.readyIds(images))) {
                is AgentCall.Ok -> {
                    Preferences.setAgentChoice(context, device.id, agent.id, folder.id)
                    AgentChoices.set(device.id, agent.id, folder.label, chosen)
                    onStarted(result.value.id)
                }
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
            if (images.isNotEmpty()) {
                ImageStrip(images = images, thumbs = thumbs, onRemove = { key -> images = AgentImages.removed(images, key) })
                if (!imagesTaken && agent != null) {
                    Text(
                        stringResource(R.string.agents_image_model, agent.name),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                }
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
            if (agent != null && abilities.control) {
                val model = AgentSettings.modelName(abilities.models, chosen.model) ?: stringResource(R.string.agents_model_default)
                val effort = AgentSettings.effectiveEffort(abilities.models, chosen)?.let(AgentSettings::effortLabel)
                Column {
                    G7SectionLabel(stringResource(R.string.agents_settings_model))
                    G7Row(
                        label = listOfNotNull(model, effort).joinToString(" · "),
                        sublabel = modeLabel(chosen.mode),
                        icon = G7Icons.sparkle,
                        onClick = { settingsOpen = true },
                    ) {
                        Icon(G7Icons.chevronRight, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
                    }
                }
            }
            Text(
                stringResource(if (chosen.mode == ThreadMode.PLAN) R.string.agents_mode_plan else R.string.agents_supervised_note),
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
    if (settingsOpen && agent != null) {
        AgentSettingsSheet(
            agentName = agent.name,
            codex = agent.id == "codex",
            abilities = abilities,
            initial = chosen,
            appliesLater = false,
            onDone = { picked ->
                chosen = picked
                settingsOpen = false
            },
            onDismiss = { settingsOpen = false },
        )
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
internal fun StatusLine(status: ThreadStatus, trailing: String?, modifier: Modifier = Modifier, word: String? = null) {
    val scheme = MaterialTheme.colorScheme
    val stateWord = when (status) {
        ThreadStatus.NeedsYou -> stringResource(R.string.agents_state_waiting)
        ThreadStatus.Working -> stringResource(R.string.agents_state_working)
        ThreadStatus.Done -> stringResource(R.string.agents_state_done)
        ThreadStatus.PlanReady -> stringResource(R.string.agents_state_plan_ready)
        ThreadStatus.Stopped -> stringResource(R.string.agents_state_interrupted)
        is ThreadStatus.Paused -> stringResource(R.string.agents_state_paused)
        is ThreadStatus.Failed -> stringResource(R.string.agents_state_failed)
    }
    Row(
        modifier = modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        // A mark of its own shape and the word in its colour (§2.9): never a bare dot.
        ThreadStatusMark(status)
        StatusWord(status, word ?: stateWord)
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

/** The text box for a task or a reply; [onFocus] hears it gain and lose focus (the typing guard). */
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
    onFocus: ((Boolean) -> Unit)? = null,
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
                .then(if (onFocus != null) Modifier.onFocusChanged { state -> onFocus(state.isFocused) } else Modifier)
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

/** After asking the PC to show its sessions: look again this often, this many times, while the list is on screen. */
private const val SESSIONS_ASK_EVERY_MS = 5_000L
private const val SESSIONS_ASK_TRIES = 24
