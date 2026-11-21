// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import android.text.format.DateUtils
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.theme.G7Header
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.g7Tints
import app.g7link.android.ui.theme.mono
import app.g7link.core.AgentFound
import app.g7link.core.AgentItem
import app.g7link.core.AgentThreadState
import app.g7link.core.AgentTimeline
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Bring in earlier sessions (board 18, contract §U9.1 "Import"): every
 * Claude and Codex session on the PC by folder, an agent filter and search,
 * several chosen and brought in as threads with how it goes, and a read-only
 * look at one first. Behind the sessions switch the PC keeps for this phone.
 */
@Composable
internal fun AgentImportScreen(
    device: Device,
    core: Core,
    agents: AgentsState,
    onOpenThread: (String) -> Unit,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val resources = LocalResources.current
    var state by remember(device.id) { mutableStateOf(ImportState()) }
    var query by rememberSaveable(device.id) { mutableStateOf("") }
    var agent by rememberSaveable(device.id) { mutableStateOf<String?>(null) }
    var asked by remember(device.id) { mutableStateOf(false) }
    var attempt by remember(device.id) { mutableIntStateOf(0) }
    var preview by remember(device.id) { mutableStateOf<AgentFound?>(null) }
    val scope = rememberCoroutineScope()
    val offline = !device.online

    suspend fun read(more: Boolean) {
        val before = if (more) state.nextMs else null
        when (val result = core.agentFound(device.id, query, agent, before, asked)) {
            is AgentCall.Ok -> state = AgentImport.settled(AgentImport.listed(state, result.value, more))
            is AgentCall.Failed -> state = when (result.failure) {
                // The PC's sessions switch for this phone is off, or it was never asked: ask first.
                AgentFailure.DENIED -> state.copy(load = ImportLoad.CONSENT)
                else -> state.copy(load = if (state.found.isEmpty()) ImportLoad.FAILED else state.load, failure = result.failure)
            }
        }
    }
    // The list, again when the filter changes (after a pause in typing), and while the PC is still looking.
    LaunchedEffect(device.id, query, agent, asked, attempt, offline) {
        if (offline) return@LaunchedEffect
        state = AgentImport.filtered(state, agent, query)
        if (query.isNotEmpty()) delay(AgentSearch.DEBOUNCE_MS)
        read(more = false)
        // Asked on the PC: look again now and then until it's allowed there.
        var tries = 0
        while (asked && state.load == ImportLoad.CONSENT && tries++ < CONSENT_TRIES) {
            delay(CONSENT_EVERY_MS)
            read(more = false)
        }
        while (state.scanning && state.load == ImportLoad.READY) {
            delay(AgentImport.SCAN_POLL_MS)
            read(more = false)
        }
    }
    // How the import goes: the PC says it as events.
    val progress = agents.importing
    LaunchedEffect(progress) {
        if (progress == null) return@LaunchedEffect
        val before = state.run?.finished == true
        state = AgentImport.progressed(state, progress)
        if (!before && state.run?.finished == true) {
            // Done: the list again, to name the ones that couldn't be read.
            read(more = false)
        }
    }
    val start: () -> Unit = start@{
        val ids = state.selected.toList()
        if (ids.isEmpty() || state.busy) return@start
        scope.launch {
            when (val result = core.agentImport(device.id, ids)) {
                is AgentCall.Ok -> state = AgentImport.started(state, ids, result.value)
                is AgentCall.Failed -> core.say(
                    if (result.failure == AgentFailure.OTHER) resources.getString(R.string.agents_wait_until_finishes) else resources.getString(R.string.agents_failed_generic),
                )
            }
        }
    }
    val cancel: () -> Unit = cancel@{
        val run = state.run ?: return@cancel
        scope.launch {
            core.agentImportCancel(device.id, run.job)
            state = AgentImport.cancelled(state)
            read(more = false)
        }
    }

    val scheme = MaterialTheme.colorScheme
    Column(modifier.fillMaxSize().imePadding()) {
        G7Header(
            title = stringResource(R.string.agents_import_title),
            onBack = onBack,
            note = stringResource(R.string.agents_import_on, device.name),
        )
        when {
            offline -> AgentsEmptyState(title = stringResource(R.string.agents_needs_pc, device.name))
            state.load == ImportLoad.CONSENT -> Box(Modifier.padding(horizontal = 12.dp, vertical = 8.dp)) {
                SessionsOff(pcName = device.name, asked = asked, onAsk = {
                    asked = true
                    attempt++
                })
            }
            state.load == ImportLoad.LOADING && state.found.isEmpty() -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                CircularProgressIndicator(Modifier.size(28.dp), strokeWidth = 2.dp)
            }
            state.load == ImportLoad.FAILED -> AgentsEmptyState(
                title = stringResource(R.string.agents_failed_generic),
                action = stringResource(R.string.agents_try_again),
                onAction = { attempt++ },
            )
            else -> {
                Column(Modifier.padding(horizontal = 14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    AgentTextField(
                        value = query,
                        onValueChange = { query = it.replace('\n', ' ').take(MAX_QUERY_CHARS) },
                        hint = stringResource(R.string.agents_import_search),
                        maxLines = 1,
                    )
                    AgentFilter(agents = agents, selected = agent, onSelect = { agent = it })
                }
                val run = state.run
                LazyColumn(
                    modifier = Modifier.weight(1f).fillMaxWidth(),
                    state = rememberLazyListState(),
                    contentPadding = PaddingValues(start = 12.dp, end = 12.dp, top = 6.dp, bottom = 16.dp),
                ) {
                    if (run != null && run.finished) {
                        item(key = "result", contentType = "result") { ImportResult(run, agents) }
                    }
                    if (state.scanning) {
                        item(key = "scanning", contentType = "note") {
                            Row(
                                modifier = Modifier.fillMaxWidth().heightIn(min = 40.dp).padding(horizontal = 4.dp),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(10.dp),
                            ) {
                                CircularProgressIndicator(Modifier.size(15.dp), strokeWidth = 2.dp)
                                Text(
                                    stringResource(R.string.agents_import_scanning, java.text.NumberFormat.getIntegerInstance().format(state.total)),
                                    style = MaterialTheme.typography.bodyMedium,
                                    color = scheme.onSurfaceVariant,
                                )
                            }
                        }
                    }
                    if (state.found.isEmpty() && !state.scanning && state.load == ImportLoad.READY) {
                        item(key = "none", contentType = "note") {
                            Text(
                                stringResource(R.string.agents_import_none),
                                style = MaterialTheme.typography.bodyLarge,
                                color = scheme.onSurfaceVariant,
                                textAlign = TextAlign.Center,
                                modifier = Modifier.fillMaxWidth().padding(vertical = 48.dp),
                            )
                        }
                    }
                    for (group in AgentImport.groups(state.found)) {
                        item(key = "folder:${group.folder}", contentType = "folder") {
                            FolderHead(group, onToggle = { state = AgentImport.toggledFolder(state, group.folder) }, enabled = !state.busy)
                        }
                        items(group.rows, key = { "found:${it.id}" }, contentType = { "found" }) { found ->
                            FoundRow(
                                found = found,
                                agentName = agents.agentName(found.agent),
                                selected = found.id in state.selected,
                                enabled = !state.busy,
                                onToggle = { state = AgentImport.toggled(state, found.id) },
                                onOpen = {
                                    val thread = found.thread
                                    if (thread != null) onOpenThread(thread) else preview = found
                                },
                            )
                        }
                    }
                    if (state.nextMs != null) {
                        item(key = "next", contentType = "next") {
                            Box(Modifier.fillMaxWidth().padding(8.dp), contentAlignment = Alignment.Center) {
                                G7RowAction(label = stringResource(R.string.agents_show_all), onClick = { scope.launch { read(more = true) } })
                            }
                        }
                    }
                }
                ImportBar(state = state, onStart = start, onCancel = cancel)
            }
        }
    }
    preview?.let { found -> FoundPreviewSheet(device = device, core = core, found = found, agentName = agents.agentName(found.agent), onDismiss = { preview = null }) }
}

/** All, Claude, Codex: the agents the PC has, by their own names. */
@Composable
private fun AgentFilter(agents: AgentsState, selected: String?, onSelect: (String?) -> Unit) {
    val ids = agents.catalog?.agents?.map { it.id }?.ifEmpty { null } ?: listOf("claude", "codex")
    val options = listOf<String?>(null) + ids
    val scheme = MaterialTheme.colorScheme
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        for (id in options) {
            val on = id == selected
            Text(
                id?.let { agents.agentName(it) } ?: stringResource(R.string.agents_filter_all),
                style = MaterialTheme.typography.labelLarge,
                color = if (on) scheme.onSurface else scheme.onSurfaceVariant,
                maxLines = 1,
                modifier = Modifier
                    .clip(RoundedCornerShape(8.dp))
                    .background(if (on) scheme.surfaceContainerHigh else scheme.surface)
                    .border(1.dp, if (on) scheme.surfaceContainerHigh else scheme.outlineVariant, RoundedCornerShape(8.dp))
                    .clickable(role = Role.RadioButton) { onSelect(id) }
                    .heightIn(min = 34.dp)
                    .padding(horizontal = 14.dp, vertical = 8.dp),
            )
        }
    }
}

/** A folder's label and how many sessions it has; a tap chooses those that can be. */
@Composable
private fun FolderHead(group: FoundGroup, onToggle: () -> Unit, enabled: Boolean) {
    val scheme = MaterialTheme.colorScheme
    val count = group.rows.size
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 36.dp)
            .clip(RoundedCornerShape(6.dp))
            .clickable(enabled = enabled, role = Role.Button, onClick = onToggle)
            .padding(horizontal = 4.dp)
            .padding(top = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(G7Icons.folder, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(14.dp))
        Text(group.folder, style = MaterialTheme.typography.labelLarge.mono(LocalAppFonts.current), maxLines = 1, overflow = TextOverflow.StartEllipsis, modifier = Modifier.weight(1f))
        Text(pluralStringResource(R.plurals.agents_import_sessions, count, count), style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
    }
}

/** A session: its check, title, agent and date, and its badge. A tap looks at it; the check chooses it. */
@Composable
private fun FoundRow(found: AgentFound, agentName: String, selected: Boolean, enabled: Boolean, onToggle: () -> Unit, onOpen: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val palette = agentPalette()
    val context = LocalContext.current
    val selectable = AgentImport.selectable(found) && enabled
    val label = stringResource(R.string.agents_import_select, found.title)
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(if (selected) scheme.surfaceContainerHigh else scheme.surface)
            .clickable(role = Role.Button, onClick = onOpen)
            .padding(horizontal = 6.dp, vertical = 4.dp),
        verticalAlignment = Alignment.Top,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Box(
            Modifier
                .size(44.dp)
                .toggleable(value = selected, enabled = selectable, role = Role.Checkbox, onValueChange = { onToggle() }),
            contentAlignment = Alignment.Center,
        ) {
            Box(
                Modifier
                    .size(18.dp)
                    .clip(RoundedCornerShape(4.dp))
                    .background(if (selected) scheme.primary else scheme.surface)
                    .border(1.5.dp, if (selected) scheme.primary else if (selectable) scheme.outline else scheme.outlineVariant, RoundedCornerShape(4.dp)),
                contentAlignment = Alignment.Center,
            ) {
                if (selected) Icon(G7Icons.check, contentDescription = label, tint = scheme.onPrimary, modifier = Modifier.size(12.dp))
            }
        }
        Column(Modifier.weight(1f).padding(top = 10.dp, bottom = 8.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            val dim = found.thread != null || found.damaged
            Text(found.title, style = MaterialTheme.typography.bodyLarge, color = if (dim) scheme.onSurfaceVariant else scheme.onSurface, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(
                listOf(agentName, DateUtils.formatDateTime(context, found.updatedMs, DateUtils.FORMAT_SHOW_DATE or DateUtils.FORMAT_ABBREV_MONTH)).joinToString(" · "),
                style = MaterialTheme.typography.labelMedium,
                color = scheme.onSurfaceVariant,
                maxLines = 1,
            )
            val badge = AgentImport.badge(found)
            if (badge != null) {
                val (icon, text, tint) = when (badge) {
                    FoundBadge.BROUGHT_IN -> Triple(G7Icons.check, stringResource(R.string.agents_brought_in), scheme.onSurfaceVariant)
                    FoundBadge.OPEN -> Triple(G7Icons.terminal, stringResource(R.string.agents_import_open), palette.toolStatusAsking)
                    FoundBadge.OUTSIDE -> Triple(G7Icons.folder, stringResource(R.string.agents_import_outside), scheme.onSurfaceVariant)
                    FoundBadge.DAMAGED -> Triple(AgentGlyphs.warning, stringResource(R.string.agents_import_damaged), palette.statusFailed)
                }
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Icon(icon, contentDescription = null, tint = tint, modifier = Modifier.size(13.dp))
                    Text(text, style = MaterialTheme.typography.labelMedium, color = if (badge == FoundBadge.DAMAGED) scheme.error else scheme.onSurfaceVariant)
                }
            }
        }
    }
}

/** The bar under the list: "12 selected · Bring in 12", or "Bringing in 12 of 40" with Cancel. */
@Composable
private fun ImportBar(state: ImportState, onStart: () -> Unit, onCancel: () -> Unit) {
    val run = state.run
    val scheme = MaterialTheme.colorScheme
    if (run == null || run.finished) {
        if (state.selected.isEmpty()) return
        val count = state.selected.size
        Column {
            Box(Modifier.fillMaxWidth().height(1.dp).background(scheme.outlineVariant))
            Row(
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Text(pluralStringResource(R.plurals.agents_import_selected, count, count), style = MaterialTheme.typography.titleSmall, modifier = Modifier.weight(1f))
                G7PrimaryButton(label = pluralStringResource(R.plurals.agents_import_bring, count, count), onClick = onStart, modifier = Modifier.weight(1f))
            }
        }
        return
    }
    val handled = run.done + run.failed
    Column {
        Box(Modifier.fillMaxWidth().height(1.dp).background(scheme.outlineVariant))
        Row(
            modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(R.string.agents_import_progress, handled, run.total), style = MaterialTheme.typography.titleSmall)
                LinearProgressIndicator(
                    progress = { if (run.total > 0) handled.toFloat() / run.total.toFloat() else 0f },
                    modifier = Modifier.fillMaxWidth(),
                )
            }
            G7RaisedButton(label = stringResource(R.string.action_cancel), onClick = onCancel)
        }
    }
}

/** After a run: how many came in, and those that couldn't be read. */
@Composable
private fun ImportResult(run: ImportRun, agents: AgentsState) {
    val scheme = MaterialTheme.colorScheme
    Column(
        Modifier
            .fillMaxWidth()
            .padding(bottom = 8.dp)
            .clip(RoundedCornerShape(8.dp))
            .border(1.dp, scheme.outlineVariant, RoundedCornerShape(8.dp))
            .padding(vertical = 6.dp),
    ) {
        Row(
            modifier = Modifier.fillMaxWidth().heightIn(min = 36.dp).padding(horizontal = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            val failed = run.failed > 0
            Icon(if (failed) AgentGlyphs.warning else G7Icons.check, contentDescription = null, tint = if (failed) g7Tints().amber else scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
            Text(
                if (failed) {
                    stringResource(R.string.agents_import_done_failed, run.done, run.failed)
                } else {
                    pluralStringResource(R.plurals.agents_import_done, run.done, run.done)
                },
                style = MaterialTheme.typography.titleSmall,
            )
        }
        for (found in run.failures) {
            Column(Modifier.padding(start = 38.dp, end = 12.dp, top = 6.dp, bottom = 6.dp), verticalArrangement = Arrangement.spacedBy(1.dp)) {
                Text(found.title, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(
                    listOf(agents.agentName(found.agent), found.folder).joinToString(" · "),
                    style = MaterialTheme.typography.labelMedium,
                    color = scheme.onSurfaceVariant,
                    maxLines = 1,
                )
            }
        }
    }
}

/** A session looked at before it's brought in: its newest messages, read now and kept nowhere. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun FoundPreviewSheet(device: Device, core: Core, found: AgentFound, agentName: String, onDismiss: () -> Unit) {
    val state = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val items by produceState<List<AgentItem>?>(null, found.id) {
        value = (core.agentFoundPreview(device.id, found.id) as? AgentCall.Ok)?.value ?: emptyList()
    }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = state, containerColor = MaterialTheme.colorScheme.surface) {
        Column(Modifier.fillMaxWidth().padding(bottom = 16.dp)) {
            Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 8.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(found.title, style = MaterialTheme.typography.titleMedium, maxLines = 2, overflow = TextOverflow.Ellipsis)
                Text(
                    listOf(agentName, found.folder).joinToString(" · "),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            val shown = items
            when {
                shown == null -> Box(Modifier.fillMaxWidth().padding(24.dp), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
                }
                shown.isEmpty() -> Text(
                    stringResource(R.string.agents_failed_generic),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(20.dp),
                )
                else -> PreviewRows(shown, found.folder)
            }
        }
    }
}

/** Read-only rows of a session's messages, made off the main thread as a thread's are. */
@Composable
private fun PreviewRows(items: List<AgentItem>, folder: String) {
    val rows by produceState<List<app.g7link.core.AgentRow>?>(null, items) {
        value = withContext(Dispatchers.Default) {
            runCatching {
                AgentTimeline().use { timeline ->
                    timeline.rows(items, emptyList(), AgentThreadState.DONE, 0L, System.currentTimeMillis(), emptyList(), emptyList(), emptyList(), emptyList())
                }
            }.getOrNull()
        }
    }
    val shown = rows ?: return
    val byId = remember(items) { items.associateBy { it.id } }
    Box(Modifier.fillMaxWidth().heightIn(max = 520.dp)) {
        AgentRowsList(
            rows = shown,
            context = remember(byId, folder) { RowsContext(byId, folder) },
            actions = remember { RowActions(toggle = {}, link = { _, _ -> }, copy = {}, output = {}, diff = { _, _ -> }, mark = {}) },
            earlier = false,
            loadingEarlier = false,
            onLoadEarlier = {},
            listState = rememberLazyListState(),
        )
    }
}

/** The longest search typed here (the PC takes 128). */
private const val MAX_QUERY_CHARS = 128

/** After asking on the PC: look again this often, this many times, while the screen is open. */
private const val CONSENT_EVERY_MS = 5_000L
private const val CONSENT_TRIES = 24
