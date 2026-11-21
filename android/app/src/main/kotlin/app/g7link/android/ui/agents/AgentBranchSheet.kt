// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
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
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.mono
import kotlinx.coroutines.launch

/** Branches listed before the search field shows. */
private const val SEARCH_FROM = 8

/**
 * Switch branch on the phone (board 19): New branch… and the branches, the
 * current one checked, then those on a remote ("On origin"). The phone
 * switches or makes a branch only while nothing is changed; with changes,
 * every row is off and says to switch on the PC. A switch runs the user's
 * `post-checkout` hook on the PC: its job's end is said here.
 */
@Composable
internal fun AgentBranchSheet(
    device: Device,
    core: Core,
    threadId: String,
    status: GitStatus,
    threadBusy: Boolean,
    agentName: String,
    onSwitched: (JobEnd) -> Unit,
    onDismiss: () -> Unit,
) {
    val resources = LocalResources.current
    val scheme = MaterialTheme.colorScheme
    val scope = rememberCoroutineScope()
    val all by core.agents.collectAsStateWithLifecycle()
    val agents = all[device.id] ?: AgentsState()
    val offline = !device.online
    val dirty = status.changed > 0
    var branches by remember(threadId) { mutableStateOf<AgentCall<List<GitBranch>>?>(null) }
    var query by remember(threadId) { mutableStateOf("") }
    var naming by remember(threadId) { mutableStateOf(false) }
    var name by remember(threadId) { mutableStateOf("") }
    var jobId by remember(threadId) { mutableStateOf<String?>(null) }
    var refused by remember(threadId) { mutableStateOf<JobError?>(null) }
    LaunchedEffect(device.id, threadId, device.online) {
        if (!offline) branches = core.agentGitBranches(device.id, threadId)
    }
    val job = jobId?.let { agents.gitJobs[it] }
    val end = job?.let(AgentJobs::end)
    LaunchedEffect(end) {
        if (end != null && end !is JobEnd.Failed) onSwitched(end)
    }
    val listed = (branches as? AgentCall.Ok)?.value.orEmpty()
    val switch: (String, Boolean, Boolean) -> Unit = { branch, create, track ->
        refused = null
        scope.launch {
            when (val started = core.agentGitSwitch(device.id, threadId, branch, create, track)) {
                is GitStart.Started -> jobId = started.job.id
                is GitStart.Refused -> {
                    refused = started.error
                    if (started.error == null) core.say(resources.getString(R.string.agents_failed_generic))
                }
            }
        }
    }
    AgentSheet(onDismiss) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(stringResource(R.string.agents_git_switch_branch), style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp))
            if (dirty && !offline) Box(Modifier.padding(horizontal = 12.dp)) { GitNotice(stringResource(R.string.agents_git_dirty, device.name)) }
            val running = job != null && end == null
            if (running) {
                Row(Modifier.padding(horizontal = 12.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
                    Text(stringResource(R.string.agents_loading), style = MaterialTheme.typography.bodyMedium)
                }
            }
            val failed = (end as? JobEnd.Failed)?.why ?: refused?.let { AgentJobs.why(JobKind.SWITCH, it.reason) }
            if (failed != null) {
                Column(Modifier.padding(horizontal = 12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(whyText(failed, device.name, agentName), style = MaterialTheme.typography.bodyMedium, color = scheme.error)
                    JobLines(job)
                }
            }
            if (naming) {
                val problem = if (name.isEmpty()) null else AgentGit.nameProblem(name, listed)
                Column(Modifier.padding(horizontal = 12.dp, vertical = 6.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(stringResource(R.string.agents_git_branch_name), style = MaterialTheme.typography.labelLarge, color = scheme.onSurfaceVariant)
                    AgentTextField(value = name, onValueChange = { name = it.trim() }, hint = stringResource(R.string.agents_git_branch_name), maxLines = 1)
                    if (problem != null) {
                        Text(
                            stringResource(if (problem == NameProblem.EXISTS) R.string.agents_git_name_exists else R.string.agents_git_bad_name),
                            style = MaterialTheme.typography.bodySmall,
                            color = scheme.error,
                        )
                    }
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        G7RaisedButton(label = stringResource(R.string.action_cancel), onClick = { naming = false }, modifier = Modifier.weight(1f))
                        G7RaisedButton(
                            label = stringResource(R.string.agents_git_create),
                            onClick = { switch(name, true, false) },
                            enabled = name.isNotEmpty() && problem == null && !running && AgentGit.canCreate(dirty, threadBusy, offline),
                            modifier = Modifier.weight(1f),
                        )
                    }
                }
                return@Column
            }
            BranchRow(
                icon = G7Icons.plus,
                name = stringResource(R.string.agents_git_new_branch),
                mono = false,
                note = null,
                enabled = AgentGit.canCreate(dirty, threadBusy, offline) && !running,
                onClick = {
                    name = ""
                    naming = true
                },
            )
            when (val result = branches) {
                null -> if (offline) {
                    Text(stringResource(R.string.agents_needs_pc, device.name), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant, modifier = Modifier.padding(12.dp))
                } else {
                    Box(Modifier.fillMaxWidth().padding(16.dp), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp) }
                }
                is AgentCall.Failed -> Text(stringResource(R.string.agents_failed_generic), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant, modifier = Modifier.padding(12.dp))
                is AgentCall.Ok -> {
                    if (listed.size > SEARCH_FROM) {
                        Box(Modifier.padding(horizontal = 12.dp, vertical = 4.dp)) {
                            AgentTextField(value = query, onValueChange = { query = it }, hint = stringResource(R.string.agents_git_search_branches), maxLines = 1)
                        }
                    }
                    val choices = AgentGit.choices(AgentGit.search(listed, query), dirty, threadBusy, offline)
                    val now = rememberNow()
                    var remoteShown: String? = null
                    for (choice in choices) {
                        val remote = choice.branch.remote
                        if (remote != null && remote != remoteShown) {
                            remoteShown = remote
                            Text(
                                stringResource(R.string.agents_git_on_remote, remote),
                                style = MaterialTheme.typography.labelLarge,
                                color = scheme.onSurfaceVariant,
                                modifier = Modifier.padding(start = 12.dp, end = 12.dp, top = 12.dp, bottom = 4.dp),
                            )
                        }
                        BranchRow(
                            icon = if (choice.branch.current) G7Icons.check else null,
                            iconLabel = if (choice.branch.current) stringResource(R.string.agents_git_current) else null,
                            name = choice.branch.name,
                            mono = true,
                            note = when (choice.off) {
                                BranchOff.OTHER_WORKTREE -> stringResource(R.string.agents_git_other_worktree, choice.branch.worktree.orEmpty())
                                BranchOff.THREAD_BUSY -> stringResource(R.string.agents_git_wait_finishes, agentName)
                                BranchOff.DIRTY, BranchOff.NEEDS_PC, null -> null
                            },
                            trailing = choice.branch.lastMs.takeIf { it > 0L }?.let { relativeTime(it, now) },
                            enabled = choice.off == null && !choice.branch.current && !running,
                            onClick = { switch(choice.branch.name, false, choice.branch.remote != null) },
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun BranchRow(
    icon: androidx.compose.ui.graphics.vector.ImageVector?,
    name: String,
    mono: Boolean,
    note: String?,
    enabled: Boolean,
    onClick: () -> Unit,
    iconLabel: String? = null,
    trailing: String? = null,
) {
    val scheme = MaterialTheme.colorScheme
    val fonts = LocalAppFonts.current
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Box(Modifier.size(18.dp), contentAlignment = Alignment.Center) {
            if (icon != null) Icon(icon, contentDescription = iconLabel, tint = if (enabled || iconLabel != null) scheme.onSurface else scheme.onSurfaceVariant, modifier = Modifier.size(18.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(
                name,
                style = if (mono) MaterialTheme.typography.bodyMedium.mono(fonts) else MaterialTheme.typography.bodyLarge,
                color = offInk(enabled || iconLabel != null),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (note != null) Text(note, style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
        }
        if (trailing != null) {
            Text(trailing, style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
        } else {
            Spacer(Modifier.width(4.dp))
        }
    }
}
