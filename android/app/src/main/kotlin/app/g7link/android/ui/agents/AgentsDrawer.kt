// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import androidx.activity.compose.BackHandler
import androidx.annotation.StringRes
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemGestures
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.DrawerState
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.State
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.MainActivity
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7SectionLabel
import app.g7link.android.ui.theme.LocalReducedMotion
import app.g7link.android.ui.theme.g7Tints
import app.g7link.core.AgentSession
import app.g7link.core.AgentThread
import kotlin.math.abs
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch

/** What the drawer marks as the one on screen. */
@Immutable
sealed interface DrawerSelection {
    data class Thread(val id: String) : DrawerSelection

    data class Session(val id: String) : DrawerSelection
}

/** Opens and closes the Agents drawer: at once, with no slide, when reduced motion is on. */
@Stable
internal class AgentsDrawerControl(
    val state: DrawerState,
    private val scope: CoroutineScope,
    private val reduced: State<Boolean>,
) {
    fun open() {
        scope.launch { if (reduced.value) state.snapTo(DrawerValue.Open) else state.open() }
    }

    fun close() {
        scope.launch { if (reduced.value) state.snapTo(DrawerValue.Closed) else state.close() }
    }
}

@Composable
internal fun rememberAgentsDrawer(): AgentsDrawerControl {
    val state = rememberDrawerState(DrawerValue.Closed)
    val scope = rememberCoroutineScope()
    val reduced = rememberUpdatedState(LocalReducedMotion.current)
    return remember(state, scope) { AgentsDrawerControl(state, scope, reduced) }
}

/** The header's button that opens the drawer (G7 Linux's sidebar glyph). */
@Composable
internal fun AgentsDrawerButton(control: AgentsDrawerControl) {
    G7IconButton(icon = AgentGlyphs.sidebar, contentDescription = stringResource(R.string.agents_drawer_show), onClick = control::open)
}

/**
 * Moves between a PC's agents the way a notification does: the app's own
 * "open this PC's agents (and this thread or session)" request, so a switch
 * from the drawer replaces what's shown, and Back still leads to the list.
 */
internal object AgentsNavigation {
    fun open(context: Context, pcId: String, threadId: String? = null, sessionId: String? = null): Boolean {
        val intent = Intent(context, MainActivity::class.java)
            .putExtra(MainActivity.EXTRA_OPEN_AGENT_PC, pcId)
            .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        if (threadId != null) intent.putExtra(MainActivity.EXTRA_OPEN_AGENT_THREAD, threadId)
        if (sessionId != null) intent.putExtra(MainActivity.EXTRA_OPEN_AGENT_SESSION, sessionId)
        return try {
            context.startActivity(intent)
            true
        } catch (_: ActivityNotFoundException) {
            false
        }
    }
}

/**
 * The Agents side drawer around a screen (t3code's floating thread sidebar, at
 * phone size): the PCs to switch between when more than one is paired, New
 * thread, then Needs you, Working, Pinned and Recent, each with the threads
 * and the sessions started on the PC together, newest first, with their state
 * dots. A tap switches to what was tapped and closes it.
 *
 * It opens from the header's button or a swipe that starts in a strip just
 * inside the left edge, past the system's back-gesture area, and only on a
 * clearly sideways swipe: the transcript's scrolling and the sideways
 * scrolling of code keep their touches. Closed, it takes no other gesture;
 * open, a swipe or a tap outside closes it, and Back closes it first.
 */
@Composable
internal fun AgentsDrawer(
    device: Device,
    core: Core,
    control: AgentsDrawerControl,
    selected: DrawerSelection?,
    onOpenThread: (String) -> Unit,
    onOpenSession: (String) -> Unit,
    modifier: Modifier = Modifier,
    content: @Composable () -> Unit,
) {
    val context = LocalContext.current
    val all by core.agents.collectAsStateWithLifecycle()
    val coreState by core.state.collectAsStateWithLifecycle()
    val agents = all[device.id] ?: AgentsState()
    val groups = remember(agents.threads, agents.sessions, agents.sessionsAllowed, agents.sessionsUnsupported) { AgentsModel.drawerGroups(agents) }
    val pcs = remember(coreState.devices) { coreState.devices.filter { !it.isAndroid && it.offersAgents } }
    val catalog = agents.catalog
    // A folder is chosen in the flow (contract C §5.6); an older PC that can't be browsed still needs one of its own.
    val placesOffered = remember(device.id, device.online) { core.agentPlacesOffered(device.id) }
    val canStart = catalog != null && agents.readyAgents.isNotEmpty() && (catalog.folders.isNotEmpty() || placesOffered) && device.online
    var newThread by remember(device.id) { mutableStateOf(false) }
    val state = control.state
    val showing = state.isOpen || state.isAnimationRunning

    // The edge strip: as wide as a thumb's start, beyond the system's own back-gesture inset.
    val density = LocalDensity.current
    val direction = LocalLayoutDirection.current
    val gestureInset = WindowInsets.systemGestures.let {
        if (direction == LayoutDirection.Ltr) it.getLeft(density, direction) else it.getRight(density, direction)
    }.toFloat()
    val stripWidth = with(density) { EDGE_STRIP.toPx() }

    ModalNavigationDrawer(
        drawerState = state,
        // Closed, the screen keeps every gesture; the edge strip below opens it.
        gesturesEnabled = showing,
        modifier = modifier,
        drawerContent = {
            ModalDrawerSheet(
                modifier = Modifier.widthIn(max = 320.dp),
                drawerShape = RoundedCornerShape(topEnd = 8.dp, bottomEnd = 8.dp),
                drawerContainerColor = MaterialTheme.colorScheme.surface,
                drawerContentColor = MaterialTheme.colorScheme.onSurface,
                drawerTonalElevation = 0.dp,
            ) {
                DrawerContent(
                    device = device,
                    agents = agents,
                    groups = groups,
                    pcs = pcs,
                    selected = selected,
                    canStart = canStart,
                    onNewThread = {
                        control.close()
                        newThread = true
                    },
                    onPc = { pcId ->
                        control.close()
                        if (pcId != device.id) AgentsNavigation.open(context, pcId)
                    },
                    onThread = { id ->
                        control.close()
                        if (selected != DrawerSelection.Thread(id)) onOpenThread(id)
                    },
                    onSession = { id ->
                        control.close()
                        if (selected != DrawerSelection.Session(id)) onOpenSession(id)
                    },
                )
            }
        },
    ) {
        Box(
            Modifier
                .fillMaxSize()
                .openFromEdge(enabled = !showing && !newThread, startPx = gestureInset, widthPx = stripWidth, rtl = direction == LayoutDirection.Rtl, onOpen = control::open),
        ) {
            content()
            // A new thread from a thread's screen: the flow covers it, which stays as it was underneath.
            if (newThread && catalog != null && canStart) {
                Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
                    NewThreadFlow(
                        device = device,
                        core = core,
                        catalog = catalog,
                        threads = agents.threads,
                        onStarted = { id ->
                            newThread = false
                            onOpenThread(id)
                        },
                        onClose = { newThread = false },
                    )
                }
            }
        }
    }
    // Composed after the screen, so it hears Back before the screen's own handlers do.
    BackHandler(enabled = state.isOpen) { control.close() }

}

/**
 * Opens the drawer on a swipe that starts in the strip [startPx]..[startPx] +
 * [widthPx] from the leading edge and goes clearly sideways, inward. It watches
 * before the screen's own gestures (the initial pass) and takes the touch only
 * once it's sure; a vertical move, or one outward, is left to the screen.
 */
private fun Modifier.openFromEdge(enabled: Boolean, startPx: Float, widthPx: Float, rtl: Boolean, onOpen: () -> Unit): Modifier =
    if (!enabled) {
        this
    } else {
        pointerInput(startPx, widthPx, rtl, onOpen) {
            awaitEachGesture {
                val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
                val fromEdge = if (rtl) size.width - down.position.x else down.position.x
                if (fromEdge < startPx || fromEdge > startPx + widthPx) return@awaitEachGesture
                val slop = viewConfiguration.touchSlop
                var dx = 0f
                var dy = 0f
                while (true) {
                    val event = awaitPointerEvent(PointerEventPass.Initial)
                    val change = event.changes.firstOrNull { it.id == down.id } ?: return@awaitEachGesture
                    if (!change.pressed) return@awaitEachGesture
                    val delta = change.positionChange()
                    dx += if (rtl) -delta.x else delta.x
                    dy += delta.y
                    when {
                        abs(dy) > slop && abs(dy) >= abs(dx) -> return@awaitEachGesture
                        dx < -slop -> return@awaitEachGesture
                        dx > slop * 2 && dx > abs(dy) * 2 -> {
                            change.consume()
                            onOpen()
                            // The rest of this touch belongs to the drawer, not the screen under it.
                            while (true) {
                                val rest = awaitPointerEvent(PointerEventPass.Initial)
                                rest.changes.forEach { it.consume() }
                                if (rest.changes.none { it.pressed }) return@awaitEachGesture
                            }
                        }
                    }
                }
            }
        }
    }

@Composable
private fun DrawerContent(
    device: Device,
    agents: AgentsState,
    groups: DrawerGroups,
    pcs: List<Device>,
    selected: DrawerSelection?,
    canStart: Boolean,
    onNewThread: () -> Unit,
    onPc: (String) -> Unit,
    onThread: (String) -> Unit,
    onSession: (String) -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val now = rememberNow()
    val manyPcs = pcs.size > 1
    LazyColumn(
        modifier = Modifier.fillMaxSize(),
        contentPadding = PaddingValues(start = 8.dp, end = 8.dp, top = 12.dp, bottom = 16.dp),
    ) {
        item(key = "title", contentType = "title") {
            Text(
                if (manyPcs) stringResource(R.string.agents_title) else stringResource(R.string.agents_home_title, device.name),
                style = MaterialTheme.typography.titleMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(start = 12.dp, end = 12.dp, top = 4.dp, bottom = 8.dp),
            )
        }
        if (manyPcs) {
            item(key = "pcs", contentType = "label") { G7SectionLabel(stringResource(R.string.settings_devices)) }
            items(pcs, key = { "pc:${it.id}" }, contentType = { "pc" }) { pc ->
                DrawerRow(
                    label = pc.name,
                    selected = pc.id == device.id,
                    note = if (pc.online) null else stringResource(R.string.not_connected),
                    onClick = { onPc(pc.id) },
                ) {
                    Icon(G7Icons.desktop, contentDescription = null, tint = tints.coral, modifier = Modifier.size(18.dp))
                }
            }
        }
        if (canStart) {
            item(key = "new", contentType = "new") {
                Box(Modifier.padding(top = if (manyPcs) 8.dp else 0.dp)) {
                    DrawerRow(label = stringResource(R.string.agents_new_thread), labelColor = scheme.primary, onClick = onNewThread) {
                        Icon(G7Icons.plus, contentDescription = null, tint = scheme.primary, modifier = Modifier.size(18.dp))
                    }
                }
            }
        }
        section("needs", R.string.agents_state_waiting, groups.needsYou, agents, selected, now, onThread, onSession)
        section("working", R.string.agents_state_working, groups.working, agents, selected, now, onThread, onSession)
        section("pinned", R.string.agents_shelf_pinned, groups.pinned, agents, selected, now, onThread, onSession)
        section("recent", R.string.agents_filter_recent, groups.recent, agents, selected, now, onThread, onSession)
        if (groups.empty) {
            item(key = "empty", contentType = "empty") {
                Text(
                    stringResource(R.string.agents_empty),
                    style = MaterialTheme.typography.bodyMedium,
                    color = scheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 12.dp, vertical = 16.dp),
                )
            }
        }
    }
}

/** A group under its label, its threads and sessions together; nothing when it's empty. */
private fun LazyListScope.section(
    key: String,
    @StringRes label: Int,
    rows: List<ProjectItem>,
    agents: AgentsState,
    selected: DrawerSelection?,
    now: Long,
    onThread: (String) -> Unit,
    onSession: (String) -> Unit,
) {
    if (rows.isEmpty()) return
    item(key = "label:$key", contentType = "label") {
        G7SectionLabel(stringResource(label), Modifier.padding(top = 8.dp))
    }
    items(rows, key = { it.key }, contentType = { if (it is ProjectItem.Thread) "thread" else "session" }) { row ->
        when (row) {
            is ProjectItem.Thread -> ThreadRow(row.thread, agents, selected, now, onThread)
            is ProjectItem.Session -> SessionRow(row.session, agents, selected, now, onSession)
        }
    }
}

@Composable
private fun ThreadRow(thread: AgentThread, agents: AgentsState, selected: DrawerSelection?, now: Long, onThread: (String) -> Unit) {
    val status = AgentsModel.status(thread)
    // A question waiting, or a plan ready, says so where the folder would (reconcile file, "Lists").
    val asking = maxOf(agents.questionsFor(thread.id).size, thread.asking.toInt())
    val what = when {
        asking > 0 && thread.asks == 0u -> pluralStringResource(R.plurals.agents_question_asks_you, asking, asking)
        status == ThreadStatus.PlanReady -> stringResource(R.string.agents_plan_ready_sub)
        else -> thread.folder
    }
    DrawerRow(
        label = thread.title,
        sublabel = listOf(agents.agentName(thread.agent), what).filter { it.isNotBlank() }.joinToString(" · "),
        mark = AgentMark.of(thread.agent),
        note = if (AgentsModel.running(thread)) null else relativeTime(thread.updatedMs, now),
        selected = selected == DrawerSelection.Thread(thread.id),
        onClick = { onThread(thread.id) },
    ) {
        ThreadStatusMark(status)
    }
}

/** A session started on the PC: where it runs (in a terminal, in the Claude app), and when it last moved unless it works. */
@Composable
private fun SessionRow(session: AgentSession, agents: AgentsState, selected: DrawerSelection?, now: Long, onSession: (String) -> Unit) {
    DrawerRow(
        label = session.title,
        sublabel = sessionPlace(session, agents.agentName(session.agent)),
        mark = AgentMark.of(session.agent),
        note = if (AgentsModel.sessionLook(session) == SessionLook.WORKING) null else relativeTime(session.updatedMs, now),
        selected = selected == DrawerSelection.Session(session.id),
        onClick = { onSession(session.id) },
    ) {
        SessionDot(session)
    }
}

/**
 * A row of the drawer: something small before it (a state dot, a glyph), a
 * label and maybe a line under it, a muted note after it. 48 dp tall at least,
 * with an 8 dp corner; the one on screen sits on the accent's plate. A line
 * that starts with the agent's name has its [mark] before it.
 */
@Composable
private fun DrawerRow(
    label: String,
    onClick: () -> Unit,
    selected: Boolean = false,
    sublabel: String? = null,
    mark: AgentMark? = null,
    note: String? = null,
    labelColor: Color = Color.Unspecified,
    leading: @Composable () -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(if (selected) scheme.primary.copy(alpha = 0.14f) else Color.Transparent)
            .clickable(role = Role.Button, onClick = onClick)
            .semantics { this.selected = selected }
            .padding(horizontal = 12.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Box(Modifier.size(18.dp), contentAlignment = Alignment.Center) { leading() }
        Column(Modifier.weight(1f)) {
            Text(
                label,
                style = MaterialTheme.typography.bodyLarge,
                color = when {
                    selected -> scheme.primary
                    labelColor != Color.Unspecified -> labelColor
                    else -> scheme.onSurface
                },
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (!sublabel.isNullOrEmpty()) {
                Text(markedText(sublabel, mark), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis, inlineContent = agentMarkContent(mark))
            }
        }
        if (!note.isNullOrEmpty()) {
            Text(note, style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant, maxLines = 1)
        }
    }
}

/** A session's mark, as its status line draws it: needs you, may need you (a hollow amber ring), at work, or still. */
@Composable
private fun SessionDot(session: AgentSession) {
    SessionLookMark(AgentsModel.sessionLook(session))
}

/** How wide the strip is that a swipe to open starts in, past the system's back-gesture area. */
private val EDGE_STRIP = 24.dp
