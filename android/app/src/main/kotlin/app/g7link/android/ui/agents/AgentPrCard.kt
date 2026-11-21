// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import android.text.format.DateFormat
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
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.core.AgentThread
import java.util.Date

/**
 * The branch's pull request on the phone (board 18): the card when there is
 * one, the form to open one when there isn't and this phone may, else why
 * not (no GitHub CLI on the PC, signed out, not on GitHub, not pushed).
 */
@Composable
internal fun AgentPrScreen(
    device: Device,
    core: Core,
    threadId: String,
    initial: GitStatus,
    thread: AgentThread?,
    agentName: String,
    pushAllowed: Boolean,
    onClose: () -> Unit,
) {
    BackHandler(onBack = onClose)
    val scheme = MaterialTheme.colorScheme
    var view by remember(threadId) { mutableStateOf<AgentCall<PrView>?>(null) }
    var ask by remember(threadId) { mutableIntStateOf(0) }
    var refresh by remember(threadId) { mutableStateOf(false) }
    // The folder as it is now (a push may just have happened), then the pull request.
    var fresh by remember(threadId) { mutableStateOf(initial) }
    LaunchedEffect(device.id, threadId, ask, device.online) {
        if (!device.online) return@LaunchedEffect
        (core.agentGitStatus(device.id, threadId) as? AgentCall.Ok)?.let { fresh = it.value }
        view = core.agentPrView(device.id, threadId, refresh)
        refresh = false
    }
    val status = fresh
    val reload: () -> Unit = {
        refresh = true
        ask++
    }
    val repo = (view as? AgentCall.Ok)?.value?.repo ?: status.github
    val sub = listOfNotNull(status.branch, repo?.substringAfterLast('/')).joinToString(" · ")
    val shown = (view as? AgentCall.Ok)?.value
    val canOpen = shown != null && AgentGit.prRow(status, shown, offline = !device.online, pushAllowed = pushAllowed) == PrRowState.CanOpen
    if (canOpen) {
        AgentPrForm(
            device = device,
            core = core,
            threadId = threadId,
            status = status,
            repo = repo,
            thread = thread,
            agentName = agentName,
            onOpened = reload,
            onClose = onClose,
        )
        return
    }
    Column(Modifier.fillMaxSize().background(scheme.surface)) {
        GitHeader(title = stringResource(R.string.agents_git_pr), sub = sub, onBack = onClose)
        Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            when (val result = view) {
                null -> if (!device.online) {
                    GitNotice(stringResource(R.string.agents_needs_pc, device.name))
                } else {
                    Box(Modifier.fillMaxWidth().heightIn(min = 120.dp), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp) }
                }
                is AgentCall.Failed -> GitNotice(stringResource(R.string.agents_failed_generic))
                is AgentCall.Ok -> {
                    val pr = result.value.pr
                    when {
                        pr != null -> PrCard(pr, unreached = result.value.why == NoPr.NETWORK, host = result.value.host, onRefresh = reload)
                        result.value.gh != GhState.OK -> GhReason(result.value.gh, device.name)
                        else -> GitNotice(
                            when (AgentGit.prRow(status, result.value, offline = !device.online, pushAllowed = pushAllowed)) {
                                PrRowState.NotGithub -> stringResource(R.string.agents_git_not_github)
                                PrRowState.Detached -> stringResource(R.string.agents_git_detached_line)
                                PrRowState.Unreached -> stringResource(R.string.agents_git_unreached_none)
                                PrRowState.SwitchOff -> stringResource(R.string.agents_git_switch_off, device.name)
                                else -> stringResource(R.string.agents_git_not_pushed)
                            },
                        )
                    }
                }
            }
        }
    }
}

/** No GitHub CLI on the PC, signed out, or too old: what it says and the command to run there. */
@Composable
internal fun GhReason(gh: GhState, pcName: String) {
    val scheme = MaterialTheme.colorScheme
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        when (gh) {
            GhState.MISSING -> {
                GitNotice(stringResource(R.string.agents_git_gh_missing))
                Text(stringResource(R.string.agents_git_gh_install, pcName), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
                CommandLine(GH_LOGIN)
            }
            GhState.SIGNED_OUT -> {
                GitNotice(stringResource(R.string.agents_git_gh_signed_out, pcName))
                CommandLine(GH_LOGIN)
            }
            GhState.OLD -> GitNotice(stringResource(R.string.agents_git_gh_old))
            GhState.OK -> Unit
        }
    }
}

/** The command that signs the GitHub CLI in (not translated). */
internal const val GH_LOGIN = "gh auth login"

/**
 * The card (§SC9.1 "PR card"): "#212 · Open", when it was read with Refresh,
 * the title, "into main", the checks line (it opens the list, failing first),
 * the review, conflicts, and Open on GitHub. Never a check's log, a diff, a
 * comment box or a merge button.
 */
@Composable
internal fun PrCard(pr: PrInfo, unreached: Boolean, host: String?, onRefresh: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val context = LocalContext.current
    var open by remember(pr.number) { mutableStateOf(pr.fail > 0) }
    val time = if (pr.readMs > 0L) DateFormat.getTimeFormat(context).format(Date(pr.readMs)) else null
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(scheme.surfaceContainer).padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Icon(AgentGlyphs.pullRequest, contentDescription = null, tint = prTint(pr.state, pr.draft), modifier = Modifier.size(17.dp))
            Text(prTitle(pr), style = MaterialTheme.typography.labelLarge, modifier = Modifier.weight(1f))
            if (time != null) Text(stringResource(R.string.agents_git_read_at, time), style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
            G7IconButton(icon = G7Icons.refresh, contentDescription = stringResource(R.string.action_refresh), onClick = onRefresh, tint = scheme.onSurfaceVariant)
        }
        if (unreached && time != null) GitNotice(stringResource(R.string.agents_git_unreached, time))
        Text(pr.title, style = MaterialTheme.typography.bodyLarge.copy(fontWeight = FontWeight.SemiBold))
        Text(stringResource(R.string.agents_git_into, pr.base), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
        val line = AgentGit.checksLine(pr)
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = 44.dp)
                .clip(RoundedCornerShape(8.dp))
                .clickable(enabled = pr.checks.isNotEmpty(), role = Role.Button) { open = !open },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            AgentGit.checksMark(pr.pass, pr.fail, pr.pending)?.let { CheckMark(it, size = 15.dp) }
            Text(checksText(line), style = MaterialTheme.typography.bodyMedium, modifier = Modifier.weight(1f))
            if (pr.checks.isNotEmpty()) {
                Icon(if (open) G7Icons.chevronDown else G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
            }
        }
        if (open) {
            for (check in AgentGit.sortedChecks(pr.checks)) CheckRow(check)
        }
        pr.review?.let { review ->
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Icon(G7Icons.info, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(15.dp))
                Text(
                    stringResource(
                        when (review) {
                            PrReview.APPROVED -> R.string.agents_git_approved
                            PrReview.CHANGES_REQUESTED -> R.string.agents_git_changes_requested
                            PrReview.REVIEW_REQUIRED -> R.string.agents_git_review_required
                        },
                    ),
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
        }
        if (pr.conflicts) GitNotice(stringResource(R.string.agents_git_has_conflicts, pr.base), warn = true)
        if (AgentGit.openable(pr.url, host ?: AgentGit.GITHUB_HOST)) {
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Start) {
                G7RaisedButton(label = stringResource(R.string.agents_git_open_github), onClick = { openLink(context, pr.url) })
            }
        }
    }
}

/** One check: its mark, its name, its state in words. */
@Composable
private fun CheckRow(check: PrCheck) {
    val scheme = MaterialTheme.colorScheme
    Row(Modifier.fillMaxWidth().heightIn(min = 32.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        CheckMark(check.state, size = 13.dp)
        Text(check.name, style = MaterialTheme.typography.bodySmall, modifier = Modifier.weight(1f))
        Text(checkWord(check.state), style = MaterialTheme.typography.labelSmall, color = scheme.onSurfaceVariant)
    }
}

@Composable
internal fun checkWord(state: CheckState): String = stringResource(
    when (state) {
        CheckState.PASS -> R.string.agents_git_check_passed
        CheckState.FAIL -> R.string.agents_git_check_failed
        CheckState.PENDING -> R.string.agents_git_check_running
        CheckState.SKIPPED -> R.string.agents_git_check_skipped
        CheckState.CANCELLED -> R.string.agents_git_check_cancelled
    },
)

/** A pull request's glyph colour: open green, draft and closed muted, merged lavender. */
@Composable
internal fun prTint(state: PrState, draft: Boolean): androidx.compose.ui.graphics.Color {
    val p = agentPalette()
    val scheme = MaterialTheme.colorScheme
    return when {
        state == PrState.MERGED -> scheme.primary
        state == PrState.CLOSED || draft -> scheme.onSurfaceVariant
        else -> p.statusDone
    }
}
