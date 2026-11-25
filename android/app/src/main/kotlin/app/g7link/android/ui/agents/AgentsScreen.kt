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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.Stable
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
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.TextFieldValue
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
import app.g7link.android.ui.theme.G7SectionLabel
import app.g7link.android.ui.theme.LocalReducedMotion
import app.g7link.core.AgentCatalog
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
private enum class AgentsPage { LIST, SEARCH, IMPORT, PULL_REQUESTS }

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
    // Shared from another app to a new thread (contract §V6): the new-thread flow opens with it in the box.
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
    // A shelf (Archived, Snoozed) goes back to the projects first.
    BackHandler(enabled = page == AgentsPage.LIST && filter.shelf != Shelf.MAIN) { filter = ThreadFilter.ALL }
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
        AgentsPage.PULL_REQUESTS -> {
            AgentPrListScreen(
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

    // A folder is chosen in the flow (contract C §5.6); an older PC that can't be browsed still needs one of its own.
    val placesOffered = remember(device.id, device.online) { core.agentPlacesOffered(device.id) }
    val canStart = !offline && catalog != null && agents.readyAgents.isNotEmpty() && (catalog.folders.isNotEmpty() || placesOffered)
    if (newThread && catalog != null && canStart) {
        NewThreadFlow(
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
            onClose = {
                newThread = false
                sharedNew = null
            },
            modifier = modifier,
        )
        return
    }
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
                    onPullRequests = { page = AgentsPage.PULL_REQUESTS },
                )
            }
            if (offline && listShown) OfflineBar(device.name, agents.savedMs ?: agents.listedMs, rememberNow())
            when {
                listShown -> ThreadList(
                    pcId = device.id,
                    pcName = device.name,
                    agents = agents,
                    catalog = catalog,
                    filter = filter,
                    onFilter = { filter = it },
                    canStart = canStart,
                    placesOffered = placesOffered,
                    onNewThread = { newThread = true },
                    rowActions = rowActions,
                    onOpenSession = onOpenSession,
                    askedSessions = askedSessions,
                    onAskSessions = { askedSessions = true },
                    onSearch = if (offline) null else ({ page = AgentsPage.SEARCH }),
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

/** The list's More: bring in earlier sessions, the pull requests (phase 4 §SC9.3), and the agents' settings on this phone. */
@Composable
private fun ListMenu(canImport: Boolean, onImport: () -> Unit, onSettings: () -> Unit, onPullRequests: () -> Unit) {
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
                text = { Text(stringResource(R.string.agents_git_prs)) },
                onClick = {
                    open = false
                    onPullRequests()
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

/**
 * The list: the setup notes, then the projects (what needs you on a pager, a
 * card per folder that opens and closes, the shelves under them) or a shelf's
 * threads; the search button floats at the bottom end.
 */
@Composable
private fun ThreadList(
    pcId: String,
    pcName: String,
    agents: AgentsState,
    catalog: AgentCatalog?,
    filter: ThreadFilter,
    onFilter: (ThreadFilter) -> Unit,
    canStart: Boolean,
    placesOffered: Boolean,
    onNewThread: () -> Unit,
    rowActions: ThreadRowActions,
    onOpenSession: (String) -> Unit,
    askedSessions: Boolean,
    onAskSessions: () -> Unit,
    onSearch: (() -> Unit)?,
) {
    val context = LocalContext.current
    val now = rememberNow()
    val animate = !LocalReducedMotion.current
    val groups = remember(agents.threads, agents.sessions, agents.sessionsAllowed, agents.sessionsUnsupported, agents.gitStrips, agents.scriptRuns) {
        AgentProjects.groups(agents)
    }
    val cards = remember(agents.threads, agents.sessions, agents.sessionsAllowed, agents.sessionsUnsupported, agents.asks, agents.questions) {
        AgentProjects.cards(agents)
    }
    val newest = remember(groups) { AgentProjects.newest(groups) }
    // What the user opened and closed on this PC, kept across visits.
    var chosen by remember(pcId) { mutableStateOf(AgentProjects.decode(Preferences.agentFolders(context, pcId))) }
    var showingAll by remember(pcId) { mutableStateOf(emptySet<String>()) }
    val folders = FolderControl(
        isOpen = { group -> AgentProjects.open(group, newest, chosen) },
        toggle = { group ->
            chosen = AgentProjects.chose(chosen, group.folder, !AgentProjects.open(group, newest, chosen))
            Preferences.setAgentFolders(context, pcId, AgentProjects.encode(chosen))
        },
        showsAll = { group -> group.folder in showingAll },
        showAll = { group -> showingAll = showingAll + group.folder },
    )
    val actions = remember(rowActions, onOpenSession) { ProjectActions(rowActions, onOpenSession) }
    val shelf = filter.shelf != Shelf.MAIN
    Box(Modifier.fillMaxSize()) {
        LazyColumn(
            modifier = Modifier.fillMaxSize(),
            // Clear of the search button at the bottom end.
            contentPadding = PaddingValues(start = 12.dp, end = 12.dp, bottom = if (onSearch != null) 96.dp else 16.dp),
            // A shelf's rows keep their gap; a folder's rows join into one card.
            verticalArrangement = if (shelf) Arrangement.spacedBy(2.dp) else Arrangement.Top,
        ) {
            if (catalog != null && agents.readyAgents.isEmpty()) {
                item(key = "none_ready", contentType = "setup") {
                    Column(Modifier.padding(bottom = 8.dp)) {
                        SetupNote(stringResource(R.string.agents_none_ready, pcName))
                        AgentChoiceRows(catalog.agents, selected = null, onSelect = {})
                    }
                }
            } else if (catalog != null && catalog.folders.isEmpty()) {
                item(key = "no_folders", contentType = "setup") {
                    SetupNote(if (placesOffered) stringResource(R.string.agents_no_folders_start) else stringResource(R.string.agents_no_folders, pcName))
                }
            }
            if (shelf) {
                item(key = "shelf_header", contentType = "shelf_header") { ShelfHeader(filter, onBack = { onFilter(ThreadFilter.ALL) }) }
                threadRows(agents, filter, now, rowActions)
                return@LazyColumn
            }
            if (groups.isEmpty()) {
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
                projects(agents, groups, cards, pcName, now, folders, actions, animate)
            }
            // Sessions started on the PC sit in their folders once the PC lists them; until then, a way to ask (never asked unprompted).
            if (!agents.sessionsUnsupported && agents.sessionsAllowed != true) {
                item(key = "sessions_off", contentType = "sessions_off") {
                    Column(Modifier.padding(top = 12.dp)) {
                        G7SectionLabel(stringResource(R.string.agents_session_section, pcName))
                        SessionsOff(pcName = pcName, asked = askedSessions, onAsk = onAskSessions)
                    }
                }
            }
            item(key = "shelves", contentType = "shelves") { ShelfButtons(agents, onShelf = onFilter) }
        }
        if (onSearch != null) SearchButton(onClick = onSearch, modifier = Modifier.align(Alignment.BottomEnd).padding(16.dp))
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

/**
 * Words to select in a text field once they're in it (the mic's words, phase 5
 * §V8.1). A field given one keeps its own selection, so it can be moved.
 */
@Stable
class TextSelection {
    internal var range by mutableStateOf<TextRange?>(null)
        private set

    /** Selects [start] until [end] of the field's text once it holds them. */
    fun select(start: Int, end: Int) {
        range = TextRange(start, end)
    }

    internal fun done() {
        range = null
    }
}

/**
 * The text box for a task or a reply; [onFocus] hears it gain and lose focus
 * (the typing guard). With [selection], words can be selected in it from
 * outside (the mic's).
 */
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
    selection: TextSelection? = null,
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
        val fieldModifier = Modifier
            .fillMaxWidth()
            .then(if (onFocus != null) Modifier.onFocusChanged { state -> onFocus(state.isFocused) } else Modifier)
            .semantics { contentDescription = hint }
        if (selection == null) {
            BasicTextField(
                value = value,
                onValueChange = onValueChange,
                enabled = enabled,
                textStyle = MaterialTheme.typography.bodyLarge.copy(color = scheme.onSurface),
                cursorBrush = SolidColor(scheme.primary),
                minLines = minLines,
                maxLines = maxLines,
                modifier = fieldModifier,
            )
        } else {
            // As the String field keeps its own selection (Compose's BasicTextField), plus one asked for from outside.
            var field by remember { mutableStateOf(TextFieldValue(value)) }
            val shown = field.copy(text = value)
            SideEffect {
                if (shown.selection != field.selection || shown.composition != field.composition) field = shown
            }
            var lastText by remember(value) { mutableStateOf(value) }
            val asked = selection.range
            LaunchedEffect(asked) {
                val range = asked ?: return@LaunchedEffect
                field = TextFieldValue(value, range)
                selection.done()
            }
            BasicTextField(
                value = shown,
                onValueChange = { next ->
                    field = next
                    val changed = lastText != next.text
                    lastText = next.text
                    if (changed) onValueChange(next.text)
                },
                enabled = enabled,
                textStyle = MaterialTheme.typography.bodyLarge.copy(color = scheme.onSurface),
                cursorBrush = SolidColor(scheme.primary),
                minLines = minLines,
                maxLines = maxLines,
                modifier = fieldModifier,
            )
        }
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

/** After asking the PC to show its sessions: look again this often, this many times, while the list is on screen. */
private const val SESSIONS_ASK_EVERY_MS = 5_000L
private const val SESSIONS_ASK_TRIES = 24
