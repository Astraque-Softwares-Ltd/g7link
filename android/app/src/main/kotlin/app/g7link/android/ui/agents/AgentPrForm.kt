// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
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
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.G7Switch
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.mono
import app.g7link.core.AgentThread
import kotlinx.coroutines.launch

/**
 * Open a pull request from the phone (board 18, behind the PC's switch
 * "Push and open pull requests"): Title (the thread's offered), Description,
 * Into (the repository's default unless another is picked), Draft, "Ask
 * <agent> to write it"; then a confirmation naming where it goes, then the
 * job. The branch is pushed first, on the PC (the Pull request row is off
 * until it is).
 */
@Composable
internal fun AgentPrForm(
    device: Device,
    core: Core,
    threadId: String,
    status: GitStatus,
    repo: String?,
    thread: AgentThread?,
    agentName: String,
    onOpened: () -> Unit,
    onClose: () -> Unit,
) {
    val resources = LocalResources.current
    val scheme = MaterialTheme.colorScheme
    val scope = rememberCoroutineScope()
    val all by core.agents.collectAsStateWithLifecycle()
    val agents = all[device.id] ?: AgentsState()
    val branch = status.branch.orEmpty()
    var title by remember(threadId) { mutableStateOf(thread?.title.orEmpty()) }
    var body by remember(threadId) { mutableStateOf("") }
    var base by remember(threadId) { mutableStateOf<String?>(null) }
    var draft by remember(threadId) { mutableStateOf(false) }
    var write by remember(threadId) { mutableStateOf(WriteField(text = title)) }
    var confirm by remember(threadId) { mutableStateOf(false) }
    var jobId by remember(threadId) { mutableStateOf<String?>(null) }
    var refused by remember(threadId) { mutableStateOf<JobError?>(null) }
    var starting by remember(threadId) { mutableStateOf(false) }
    var bases by remember(threadId) { mutableStateOf(emptyList<String>()) }
    LaunchedEffect(device.id, threadId) {
        // The branches on the push remote, by their name there.
        val remote = AgentGit.pushRemoteOf(status)
        val listed = (core.agentGitBranches(device.id, threadId) as? AgentCall.Ok)?.value.orEmpty()
        bases = AgentPrs.bases(listed, remote, branch)
    }
    val job = jobId?.let { agents.gitJobs[it] }
    val end = job?.let(AgentJobs::end)
    LaunchedEffect(end) { if (end is JobEnd.Opened) onOpened() }
    val wrote = thread?.let(AgentGitFfi::wrote)
    val running = thread?.let(AgentsModel::running) == true
    LaunchedEffect(wrote, running) {
        val before = write
        val next = AgentWrite.thread(write, WritePurpose.PR, wrote, running)
        write = next
        if (before.asking && !next.asking && next.text != before.text && next.offer == null) {
            val (t, b) = AgentGit.prTitleBody(next.text)
            title = t
            body = b
        }
    }
    val into = base ?: status.defaultBranch
    val intoShown = into ?: stringResource(R.string.agents_git_default_branch)
    val open: () -> Unit = {
        confirm = false
        starting = true
        refused = null
        scope.launch {
            when (val started = core.agentPrCreate(device.id, threadId, title.trim(), body, base, draft)) {
                is GitStart.Started -> jobId = started.job.id
                is GitStart.Refused -> {
                    refused = started.error
                    if (started.error == null) core.say(resources.getString(R.string.agents_failed_generic))
                }
            }
            starting = false
        }
    }
    val ask: () -> Unit = {
        write = AgentWrite.asked(write.copy(text = combined(title, body)), wrote)
        scope.launch {
            if (core.agentAsk(device.id, threadId, WritePurpose.PR) is AgentCall.Failed) {
                write = AgentWrite.askFailed(write)
                core.say(resources.getString(R.string.agents_failed_generic))
            }
        }
    }
    BackHandler(onBack = onClose)
    Column(Modifier.fillMaxSize().background(scheme.surface).imePadding()) {
        GitHeader(title = stringResource(R.string.agents_git_open_pr), sub = stringResource(R.string.agents_git_branch_into, branch, intoShown), onBack = onClose)
        if (jobId != null) {
            Column(Modifier.weight(1f).fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                JobLines(job)
                JobStatusLine(job, stringResource(R.string.agents_git_opening), device.name, agentName, onStop = null)
            }
            if (end is JobEnd.Failed) {
                Box(Modifier.fillMaxWidth().padding(16.dp)) { G7PrimaryButton(label = stringResource(R.string.agents_git_back_to_thread), onClick = onClose) }
            }
            return@Column
        }
        Column(
            Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(stringResource(R.string.agents_git_title), style = MaterialTheme.typography.labelLarge, color = scheme.onSurfaceVariant)
            AgentTextField(
                value = title,
                onValueChange = {
                    title = it
                    write = AgentWrite.typed(write, combined(it, body))
                },
                hint = stringResource(R.string.agents_git_title),
                enabled = !write.asking,
                maxLines = 2,
            )
            Text(stringResource(R.string.agents_git_description), style = MaterialTheme.typography.labelLarge, color = scheme.onSurfaceVariant)
            AgentTextField(
                value = body,
                onValueChange = {
                    body = it
                    write = AgentWrite.typed(write, combined(title, it))
                },
                hint = stringResource(R.string.agents_git_description),
                enabled = !write.asking,
                minLines = 5,
                maxLines = 14,
            )
            IntoPicker(into = intoShown, bases = bases, onPick = { base = it })
            Row(Modifier.fillMaxWidth().heightIn(min = 48.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(R.string.agents_git_draft), style = MaterialTheme.typography.bodyLarge, modifier = Modifier.weight(1f))
                G7Switch(checked = draft, onCheckedChange = { draft = it })
            }
            val offer = write.offer
            when {
                write.asking -> G7RaisedButton(label = stringResource(R.string.agents_git_asking, agentName), onClick = {}, enabled = false)
                offer != null -> G7RaisedButton(
                    label = stringResource(R.string.agents_git_use_message, agentName),
                    onClick = {
                        val (t, b) = AgentGit.prTitleBody(offer)
                        title = t
                        body = b
                        write = AgentWrite.use(write, combined(t, b))
                    },
                )
                else -> G7RaisedButton(label = stringResource(R.string.agents_git_ask, agentName), onClick = ask)
            }
            refused?.let { error ->
                Text(whyText(AgentJobs.why(JobKind.PR_CREATE, error.reason), device.name, agentName), style = MaterialTheme.typography.bodyMedium, color = scheme.error)
            }
        }
        Box(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp)) {
            if (starting) {
                Box(Modifier.fillMaxWidth().heightIn(min = 48.dp), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp) }
            } else {
                G7PrimaryButton(label = stringResource(R.string.agents_git_open_pr), onClick = { confirm = true }, enabled = AgentGit.prOk(title, body) && !write.asking)
            }
        }
    }
    if (confirm) {
        AgentSheet({ confirm = false }) {
            Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(stringResource(R.string.agents_git_pr_confirm, intoShown), style = MaterialTheme.typography.titleLarge)
                Text(title.trim(), style = MaterialTheme.typography.bodyLarge.copy(fontWeight = FontWeight.Medium), maxLines = 2, overflow = TextOverflow.Ellipsis)
                Text(
                    if (repo != null) stringResource(R.string.agents_git_pr_from, branch, repo) else branch,
                    style = MaterialTheme.typography.bodyMedium,
                    color = scheme.onSurfaceVariant,
                )
                Row(Modifier.fillMaxWidth().padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    G7RaisedButton(label = stringResource(R.string.action_cancel), onClick = { confirm = false }, modifier = Modifier.weight(1f))
                    G7PrimaryButton(label = stringResource(R.string.agents_link_open), onClick = open, modifier = Modifier.weight(1f))
                }
            }
        }
    }
}

private fun combined(title: String, body: String): String = if (body.isBlank()) title else "$title\n\n$body"

/** "Into main": the base, the repository's default unless another branch on the remote is picked. */
@Composable
private fun IntoPicker(into: String, bases: List<String>, onPick: (String?) -> Unit) {
    val scheme = MaterialTheme.colorScheme
    var open by remember { mutableStateOf(false) }
    Box {
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = 48.dp)
                .clickable(enabled = bases.isNotEmpty(), role = Role.Button) { open = true },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(stringResource(R.string.agents_git_into_label), style = MaterialTheme.typography.bodyLarge, modifier = Modifier.weight(1f))
            Text(into, style = MaterialTheme.typography.bodyMedium.mono(LocalAppFonts.current), color = scheme.onSurface)
            if (bases.isNotEmpty()) Icon(G7Icons.chevronDown, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            DropdownMenuItem(
                text = { Text(stringResource(R.string.agents_git_default_branch)) },
                onClick = {
                    open = false
                    onPick(null)
                },
            )
            for (name in bases) {
                DropdownMenuItem(
                    text = { Text(name, style = MaterialTheme.typography.bodyMedium.mono(LocalAppFonts.current)) },
                    onClick = {
                        open = false
                        onPick(name)
                    },
                )
            }
        }
    }
}

/** The pull-request form's rules. */
object AgentPrs {
    /**
     * The branches a pull request can go into: those on [remote] by their
     * name there ("origin/main" is "main"), never the branch itself.
     */
    fun bases(branches: List<GitBranch>, remote: String?, branch: String): List<String> {
        if (remote == null) return emptyList()
        val prefix = "$remote/"
        return branches.filter { it.remote == remote && it.name.startsWith(prefix) }
            .map { it.name.removePrefix(prefix) }
            .filter { it.isNotEmpty() && it != "HEAD" && it != branch }
            .distinct()
    }
}
