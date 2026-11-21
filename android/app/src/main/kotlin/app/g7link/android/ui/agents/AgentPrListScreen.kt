// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.theme.G7Icons

/**
 * Pull requests (board 20), from the list's More: Mine, Review requested and
 * one of the PC's agent folders, read through the PC's GitHub CLI. A row
 * opens in the browser; a long-press offers its thread and Copy link. Nothing
 * else is done to a pull request here.
 */
@Composable
internal fun AgentPrListScreen(
    device: Device,
    core: Core,
    agents: AgentsState,
    onOpenThread: (String) -> Unit,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    BackHandler(onBack = onBack)
    val scheme = MaterialTheme.colorScheme
    val resources = LocalResources.current
    val context = LocalContext.current
    val folders = agents.catalog?.folders.orEmpty()
    var show by rememberSaveable(device.id) { mutableStateOf(PrShow.MINE) }
    var folder by rememberSaveable(device.id) { mutableStateOf(folders.firstOrNull()?.id) }
    var list by remember(device.id) { mutableStateOf<AgentCall<PrList>?>(null) }
    LaunchedEffect(device.id, show, folder, device.online) {
        list = null
        if (device.online) list = core.agentPrList(device.id, show, if (show == PrShow.FOLDER) folder else null)
    }
    val copy = rememberCopy { core.say(resources.getString(R.string.agents_copied)) }
    Column(modifier.fillMaxSize().background(scheme.surface)) {
        GitHeader(title = stringResource(R.string.agents_git_prs), sub = device.name, onBack = onBack)
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 10.dp).selectableGroup(),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            for ((value, label) in listOf(PrShow.MINE to R.string.agents_git_mine, PrShow.REVIEW to R.string.agents_git_review_requested, PrShow.FOLDER to R.string.agents_git_this_folder)) {
                val on = value == show
                Box(
                    Modifier
                        .weight(1f)
                        .heightIn(min = 36.dp)
                        .clip(RoundedCornerShape(8.dp))
                        .background(if (on) scheme.surfaceContainerHigh else scheme.surface)
                        .border(1.dp, if (on) scheme.surfaceContainerHigh else scheme.outlineVariant, RoundedCornerShape(8.dp))
                        .selectable(selected = on, role = Role.Tab, onClick = { show = value }),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(stringResource(label), style = MaterialTheme.typography.labelLarge, color = if (on) scheme.onSurface else scheme.onSurfaceVariant, maxLines = 1)
                }
            }
        }
        if (show == PrShow.FOLDER && folders.size > 1) {
            FolderPicker(label = folders.firstOrNull { it.id == folder }?.label.orEmpty(), folders = folders.map { it.id to it.label }, onPick = { folder = it })
        }
        when (val result = list) {
            null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                if (device.online) {
                    CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
                } else {
                    Text(stringResource(R.string.agents_needs_pc, device.name), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
                }
            }
            is AgentCall.Failed -> Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
                Text(stringResource(R.string.agents_failed_generic), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
            }
            is AgentCall.Ok -> when {
                result.value.gh != GhState.OK -> Box(Modifier.fillMaxWidth().padding(16.dp)) { GhReason(result.value.gh, device.name) }
                result.value.rows.isEmpty() -> Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
                    Text(
                        stringResource(if (show == PrShow.REVIEW) R.string.agents_git_no_reviews else R.string.agents_git_no_prs),
                        style = MaterialTheme.typography.bodyLarge,
                        color = scheme.onSurfaceVariant,
                    )
                }
                else -> {
                    val now = rememberNow()
                    LazyColumn(Modifier.fillMaxSize()) {
                        items(result.value.rows, key = { "${it.repo}#${it.number}" }) { row ->
                            PrListRow(
                                row = row,
                                now = now,
                                onOpen = { if (AgentGit.openableOn(row.url, result.value.hosts)) openLink(context, row.url) },
                                onOpenThread = row.threadId?.let { id -> { onOpenThread(id) } },
                                onCopy = { copy(row.url) },
                            )
                        }
                    }
                }
            }
        }
    }
}

/** A row: the glyph (open, draft), the title, the repository, "#212 · Draft · 2 d", checks; the browser glyph. */
@Composable
private fun PrListRow(row: PrRow, now: Long, onOpen: () -> Unit, onOpenThread: (() -> Unit)?, onCopy: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    var menu by remember(row.url) { mutableStateOf(false) }
    Box {
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = 64.dp)
                .combinedClickable(role = Role.Button, onClick = onOpen, onLongClick = { menu = true })
                .padding(horizontal = 16.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Icon(AgentGlyphs.pullRequest, contentDescription = prStateWord(row.state, row.draft), tint = prTint(row.state, row.draft), modifier = Modifier.size(18.dp))
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(row.title, style = MaterialTheme.typography.bodyLarge.copy(fontWeight = FontWeight.Medium), maxLines = 2, overflow = TextOverflow.Ellipsis)
                Text(row.repo, style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    val parts = listOfNotNull(
                        "#${row.number}",
                        if (row.draft) stringResource(R.string.agents_git_pr_draft) else null,
                        row.updatedMs.takeIf { it > 0L }?.let { relativeTime(it, now) },
                    )
                    Text(parts.joinToString(" · "), style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
                    row.checks?.let { checks ->
                        AgentGit.checksMark(checks.pass, checks.fail, checks.pending)?.let { mark ->
                            CheckMark(mark, size = 12.dp)
                            Text(checksText(AgentGit.checksLine(checks.pass, checks.fail, checks.pending)), style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
                        }
                    }
                }
            }
            Icon(AgentGlyphs.external, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
        }
        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
            if (onOpenThread != null) {
                DropdownMenuItem(
                    text = { Text(stringResource(R.string.agents_git_open_thread)) },
                    onClick = {
                        menu = false
                        onOpenThread()
                    },
                )
            }
            DropdownMenuItem(
                text = { Text(stringResource(R.string.agents_git_copy_link)) },
                onClick = {
                    menu = false
                    onCopy()
                },
            )
        }
    }
}

/** The agent folder "This folder" lists. */
@Composable
private fun FolderPicker(label: String, folders: List<Pair<String, String>>, onPick: (String) -> Unit) {
    val scheme = MaterialTheme.colorScheme
    var open by remember { mutableStateOf(false) }
    Box(Modifier.padding(horizontal = 14.dp)) {
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = 44.dp)
                .clip(RoundedCornerShape(8.dp))
                .combinedClickable(role = Role.Button, onClick = { open = true })
                .padding(horizontal = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Icon(G7Icons.folder, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
            Text(label, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
            Icon(G7Icons.chevronDown, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            for ((id, name) in folders) {
                DropdownMenuItem(
                    text = { Text(name) },
                    onClick = {
                        open = false
                        onPick(id)
                    },
                )
            }
        }
    }
}
