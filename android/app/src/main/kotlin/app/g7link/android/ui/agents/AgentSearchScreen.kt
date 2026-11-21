// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.G7SwitchRow
import app.g7link.core.AgentHit
import app.g7link.core.AgentItemKind
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * Search (board 17, contract §U9.1 "Search"): asked 150 ms after the last key;
 * words under 3 letters search titles only; hits grouped by thread with "N
 * more in this thread"; archived threads by a switch. A hit opens its thread
 * at that message.
 */
@Composable
internal fun AgentSearchScreen(
    device: Device,
    core: Core,
    agents: AgentsState,
    onOpenThread: (String) -> Unit,
    onImport: () -> Unit,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var state by remember(device.id) { mutableStateOf(SearchState()) }
    var typed by rememberSaveable(device.id) { mutableStateOf("") }
    var archived by rememberSaveable(device.id) { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val focus = remember { FocusRequester() }
    LaunchedEffect(focus) { runCatching { focus.requestFocus() } }
    LaunchedEffect(typed, archived) { state = AgentSearch.archivedChanged(AgentSearch.typed(state, typed), archived) }
    // The newest request goes once the typing pauses; an answer to an older one is dropped by the model.
    LaunchedEffect(state.generation) {
        val request = AgentSearch.request(state) ?: return@LaunchedEffect
        delay(AgentSearch.DEBOUNCE_MS)
        state = AgentSearch.started(state, request.generation)
        state = when (val result = core.agentSearch(device.id, request)) {
            is AgentCall.Ok -> AgentSearch.answered(state, request.generation, result.value)
            is AgentCall.Failed -> AgentSearch.failed(state, request.generation, result.failure)
        }
    }
    val more: () -> Unit = more@{
        val request = AgentSearch.more(state) ?: return@more
        state = AgentSearch.loadingMore(state, request.generation)
        scope.launch {
            state = when (val result = core.agentSearch(device.id, request)) {
                is AgentCall.Ok -> AgentSearch.moreAnswered(state, request.generation, result.value)
                is AgentCall.Failed -> AgentSearch.failed(state, request.generation, result.failure)
            }
        }
    }
    val open: (String, ULong) -> Unit = { thread, seq ->
        AgentJump.request(device.id, thread, seq)
        onOpenThread(thread)
    }
    val groups = remember(state.hits) { AgentSearch.groups(state.hits) }
    val scheme = MaterialTheme.colorScheme
    Column(modifier.fillMaxSize().imePadding()) {
        Row(
            modifier = Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(start = 4.dp, end = 12.dp, top = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            G7IconButton(icon = G7Icons.back, contentDescription = stringResource(R.string.action_back), onClick = onBack)
            Box(Modifier.weight(1f), contentAlignment = Alignment.CenterEnd) {
                AgentTextField(
                    value = typed,
                    onValueChange = { typed = it.replace('\n', ' ') },
                    hint = stringResource(R.string.agents_search),
                    maxLines = 1,
                    modifier = Modifier.focusRequester(focus).padding(end = 40.dp),
                )
                when {
                    state.phase == SearchPhase.SEARCHING || state.phase == SearchPhase.WAITING ->
                        CircularProgressIndicator(Modifier.padding(end = 12.dp).size(16.dp), strokeWidth = 2.dp)
                    typed.isNotEmpty() -> G7IconButton(
                        icon = G7Icons.close,
                        contentDescription = stringResource(R.string.agents_search_clear),
                        onClick = { typed = "" },
                    )
                }
            }
        }
        LazyColumn(
            modifier = Modifier.weight(1f).fillMaxWidth(),
            contentPadding = PaddingValues(start = 12.dp, end = 12.dp, bottom = 16.dp),
        ) {
            item(key = "archived", contentType = "switch") {
                G7SwitchRow(label = stringResource(R.string.agents_search_archived), checked = archived, onCheckedChange = { archived = it })
            }
            val short = state.titlesOnly || AgentSearch.short(typed)
            if (short && typed.isNotBlank()) {
                item(key = "short", contentType = "note") { SearchNote(stringResource(R.string.agents_search_short)) }
            }
            when {
                state.phase == SearchPhase.FAILED -> item(key = "failed", contentType = "note") {
                    SearchNote(stringResource(R.string.agents_failed_generic))
                }
                state.phase == SearchPhase.READY && groups.isEmpty() -> item(key = "none", contentType = "note") {
                    SearchNote(stringResource(R.string.agents_search_none))
                }
            }
            for (group in groups) {
                item(key = "group:${group.threadId}", contentType = "group") {
                    HitGroupHead(group, agents, onOpen = { open(group.threadId, 0uL) })
                }
                items(group.hits, key = { "hit:${it.thread}:${it.seq}" }, contentType = { "hit" }) { hit ->
                    HitRow(hit, agents.agentName(hit.agent), onOpen = { open(hit.thread, hit.seq) })
                }
                if (group.more > 0) {
                    item(key = "more:${group.threadId}", contentType = "more") {
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .heightIn(min = 40.dp)
                                .clip(RoundedCornerShape(8.dp))
                                .clickable(role = Role.Button) { open(group.threadId, group.hits.lastOrNull()?.seq ?: 0uL) }
                                .padding(start = 34.dp, end = 14.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(4.dp),
                        ) {
                            Text(pluralStringResource(R.plurals.agents_search_more, group.more, group.more), style = MaterialTheme.typography.labelLarge)
                            Icon(G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
                        }
                    }
                }
            }
            if (state.next != null) {
                item(key = "next", contentType = "next") {
                    Box(Modifier.fillMaxWidth().padding(8.dp), contentAlignment = Alignment.Center) {
                        if (state.loadingMore) {
                            CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                        } else {
                            G7RowAction(label = stringResource(R.string.agents_show_all), onClick = more)
                        }
                    }
                }
            }
        }
        if (typed.isNotBlank()) {
            Text(
                stringResource(R.string.agents_search_foot),
                style = MaterialTheme.typography.bodySmall,
                color = scheme.onSurfaceVariant,
                modifier = Modifier
                    .fillMaxWidth()
                    .background(scheme.surfaceContainer)
                    .clickable(role = Role.Button, onClickLabel = stringResource(R.string.agents_import_title), onClick = onImport)
                    .padding(horizontal = 26.dp, vertical = 10.dp),
            )
        }
    }
}

@Composable
private fun SearchNote(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 12.dp),
    )
}

/** A thread's head in the results: its state dot, title (marked when it matched), Archived, and when. */
@Composable
private fun HitGroupHead(group: HitGroup, agents: AgentsState, onOpen: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val thread = agents.thread(group.threadId)
    val now = rememberNow()
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 44.dp)
            .clip(RoundedCornerShape(8.dp))
            .clickable(role = Role.Button, onClick = onOpen)
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        if (thread != null) ThreadStatusMark(AgentsModel.status(thread)) else SessionStatusMark(needsYou = false, mayNeedYou = false, working = false)
        val titleHit = group.titleHit
        if (titleHit != null) {
            Marked(titleHit.snippet.ifBlank { group.title }, if (titleHit.snippet.isBlank()) emptyList() else AgentSearch.markRanges(titleHit.snippet, titleHit.marks), Modifier.weight(1f), title = true)
        } else {
            Text(group.title, style = MaterialTheme.typography.bodyLarge.copy(fontWeight = FontWeight.Medium), maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        }
        if (group.archived) Text(stringResource(R.string.agents_shelf_archived), style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
        Text(relativeTime(group.whenMs, now), style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant, maxLines = 1)
    }
}

/** A message that matched: its snippet with the matches marked, who said it and when. */
@Composable
private fun HitRow(hit: AgentHit, agentName: String, onOpen: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val now = rememberNow()
    val who = when (hit.kind) {
        AgentItemKind.USER -> stringResource(R.string.agents_search_you)
        else -> agentName
    }
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .clickable(role = Role.Button, onClick = onOpen)
            .padding(start = 34.dp, end = 14.dp, top = 8.dp, bottom = 8.dp),
        verticalArrangement = Arrangement.spacedBy(3.dp),
    ) {
        Marked(hit.snippet, remember(hit.snippet, hit.marks) { AgentSearch.markRanges(hit.snippet, hit.marks) })
        Text(
            listOf(who, relativeTime(hit.whenMs, now)).joinToString(" · "),
            style = MaterialTheme.typography.labelMedium,
            color = scheme.onSurfaceVariant,
            maxLines = 1,
        )
    }
}

/** Text with its matches in the primary weight on a light wash; never markup from the PC. */
@Composable
private fun Marked(text: String, ranges: List<IntRange>, modifier: Modifier = Modifier, title: Boolean = false) {
    val wash = MaterialTheme.colorScheme.primary.copy(alpha = 0.18f)
    val shown = remember(text, ranges, wash) {
        buildAnnotatedString {
            append(text)
            for (range in ranges) addStyle(SpanStyle(fontWeight = FontWeight.SemiBold, background = wash), range.first, range.last + 1)
        }
    }
    Text(
        shown,
        style = if (title) MaterialTheme.typography.bodyLarge.copy(fontWeight = FontWeight.Medium) else MaterialTheme.typography.bodyMedium,
        maxLines = if (title) 1 else 2,
        overflow = TextOverflow.Ellipsis,
        modifier = modifier,
    )
}
