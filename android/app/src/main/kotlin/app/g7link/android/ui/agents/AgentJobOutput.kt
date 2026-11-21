// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.g7link.android.R
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.mono

/**
 * A job's output (contract §SC9.1 "Job output"): plain mono lines on the code
 * ground, no colour, the last twelve in view and the rest above them; "N
 * earlier lines not kept" when the PC or this phone let some go.
 */
@Composable
internal fun JobLines(job: GitJob?, modifier: Modifier = Modifier) {
    val lines = job?.lines.orEmpty()
    val notKept = job?.notKept ?: 0
    if (lines.isEmpty() && notKept == 0) return
    val scheme = MaterialTheme.colorScheme
    val p = agentPalette()
    val style = MaterialTheme.typography.bodySmall.mono(LocalAppFonts.current).copy(fontSize = 12.5.sp, lineHeight = 19.sp)
    val scroll = rememberScrollState()
    // The newest line in view as lines arrive.
    LaunchedEffect(lines.size) { scroll.scrollTo(scroll.maxValue) }
    Column(
        modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(scheme.surfaceContainer)
            .heightIn(max = (19 * AgentJobs.VISIBLE_LINES + 16).dp)
            .verticalScroll(scroll)
            .padding(horizontal = 12.dp, vertical = 8.dp),
    ) {
        if (notKept > 0) {
            Text(pluralStringResource(R.plurals.agents_git_not_kept, notKept, notKept), style = MaterialTheme.typography.labelSmall, color = scheme.onSurfaceVariant)
        }
        for (line in lines) Text(line, style = style, color = p.diffText)
    }
}

/**
 * How a job stands, on one line: a ring and what's running, then its end in
 * words (§SC9.4), a check or an x before it. Stop shows while a commit or a
 * push runs.
 */
@Composable
internal fun JobStatusLine(job: GitJob?, running: String, pcName: String, agentName: String, onStop: (() -> Unit)?) {
    val scheme = MaterialTheme.colorScheme
    val end = job?.let(AgentJobs::end)
    Row(
        Modifier.fillMaxWidth().heightIn(min = 48.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        when {
            end == null -> CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
            end is JobEnd.Failed -> CheckMark(CheckState.FAIL, size = 16.dp)
            else -> CheckMark(CheckState.PASS, size = 16.dp)
        }
        Text(
            if (end == null) running else endText(end, pcName, agentName),
            style = MaterialTheme.typography.bodyMedium.copy(fontWeight = FontWeight.Medium),
            color = scheme.onSurface,
            modifier = Modifier.weight(1f),
        )
        if (end == null && onStop != null && AgentJobs.stoppable(job)) {
            G7RaisedButton(label = stringResource(R.string.agents_stop), onClick = onStop, modifier = Modifier.padding(start = 4.dp))
        }
    }
}

/** A job's end in words. */
@Composable
internal fun endText(end: JobEnd, pcName: String, agentName: String): String = when (end) {
    is JobEnd.Committed -> stringResource(R.string.agents_git_committed, end.oid7, end.subject)
    is JobEnd.Pushed -> stringResource(R.string.agents_git_pushed, end.branch, end.remote)
    JobEnd.UpToDate -> stringResource(R.string.agents_git_up_to_date)
    is JobEnd.Switched -> stringResource(if (end.created) R.string.agents_git_created else R.string.agents_git_switched, end.branch)
    is JobEnd.Opened -> stringResource(R.string.agents_git_opened, end.number)
    JobEnd.Fetched -> stringResource(R.string.agents_git_fetched)
    is JobEnd.Failed -> whyText(end.why, pcName, agentName)
}

/** Why something didn't go through, as §SC9.4 words it. */
@Composable
internal fun whyText(why: JobWhy, pcName: String, agentName: String): String = when (why) {
    JobWhy.COMMIT_HOOK -> stringResource(R.string.agents_git_commit_hook)
    JobWhy.COMMIT_FAILED -> stringResource(R.string.agents_git_commit_failed)
    JobWhy.COMMIT_STOPPED -> stringResource(R.string.agents_git_commit_stopped)
    JobWhy.FILES_CHANGED -> stringResource(R.string.agents_git_changed_looking)
    JobWhy.IDENTITY -> stringResource(R.string.agents_git_identity)
    JobWhy.NOTHING -> stringResource(R.string.agents_git_nothing)
    JobWhy.THREAD_BUSY -> stringResource(R.string.agents_git_wait_finishes, agentName)
    JobWhy.SHARED_BUSY -> stringResource(R.string.agents_git_shared_busy)
    JobWhy.GIT_BUSY -> stringResource(R.string.agents_git_locked)
    JobWhy.DOING -> stringResource(R.string.agents_git_busy)
    JobWhy.DETACHED -> stringResource(R.string.agents_git_detached_line)
    JobWhy.CONFLICTS -> stringResource(R.string.agents_git_conflicts_any)
    JobWhy.PUSH_BEHIND -> stringResource(R.string.agents_git_push_behind, agentName)
    JobWhy.PUSH_REFUSED -> stringResource(R.string.agents_git_push_refused)
    JobWhy.PUSH_HOOK -> stringResource(R.string.agents_git_push_hook)
    JobWhy.SIGN_IN -> stringResource(R.string.agents_git_sign_in)
    JobWhy.NETWORK -> stringResource(R.string.agents_git_network)
    JobWhy.NO_REMOTE -> stringResource(R.string.agents_git_no_remote)
    JobWhy.PICK_REMOTE -> stringResource(R.string.agents_git_choose_remote)
    JobWhy.PUSH_FAILED -> stringResource(R.string.agents_git_push_failed)
    JobWhy.NO_COMMITS -> stringResource(R.string.agents_git_no_commits)
    JobWhy.SWITCH_EXISTS -> stringResource(R.string.agents_git_name_exists)
    JobWhy.SWITCH_BAD_NAME -> stringResource(R.string.agents_git_bad_name)
    JobWhy.IN_WORKTREE -> stringResource(R.string.agents_git_in_worktree)
    JobWhy.DIRTY -> stringResource(R.string.agents_git_dirty, pcName)
    JobWhy.WOULD_OVERWRITE -> stringResource(R.string.agents_git_would_overwrite)
    JobWhy.SWITCH_FAILED -> stringResource(R.string.agents_git_switch_failed)
    JobWhy.PR_EXISTS -> stringResource(R.string.agents_git_pr_exists)
    JobWhy.NOT_PUSHED -> stringResource(R.string.agents_git_not_pushed)
    JobWhy.NOT_GITHUB -> stringResource(R.string.agents_git_not_github)
    JobWhy.NO_GH -> stringResource(R.string.agents_git_gh_missing)
    JobWhy.SIGNED_OUT -> stringResource(R.string.agents_git_gh_signed_out_short)
    JobWhy.OLD_GH -> stringResource(R.string.agents_git_gh_old)
    JobWhy.PR_FAILED -> stringResource(R.string.agents_git_pr_failed)
}

/** The commands git needs for a name and an email, as copyable text. */
internal val IDENTITY_COMMANDS = listOf("git config --global user.name \"…\"", "git config --global user.email \"…\"")

/**
 * A job full-screen (boards 16 and 17 after the confirmation): the title
 * ("Push feat/sync-retry to origin"), the lines as they come, the status
 * line, then Done and, once it went through, the next step ([next]).
 */
@Composable
internal fun AgentJobScreen(
    title: String,
    sub: String?,
    job: GitJob?,
    running: String,
    pcName: String,
    agentName: String,
    onStop: (() -> Unit)?,
    onClose: () -> Unit,
    next: Pair<String, () -> Unit>? = null,
) {
    BackHandler(onBack = onClose)
    val scheme = MaterialTheme.colorScheme
    val end = job?.let(AgentJobs::end)
    Column(Modifier.fillMaxSize().background(scheme.surface)) {
        GitHeader(title = title, sub = sub, onBack = onClose)
        Column(Modifier.weight(1f).fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            JobLines(job)
            JobStatusLine(job, running, pcName, agentName, onStop)
            if (end is JobEnd.Failed && end.why == JobWhy.IDENTITY) IDENTITY_COMMANDS.forEach { CommandLine(it) }
        }
        if (end != null) {
            Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                if (end !is JobEnd.Failed && next != null) {
                    G7RaisedButton(label = stringResource(R.string.agents_settings_done), onClick = onClose, modifier = Modifier.fillMaxWidth())
                    G7PrimaryButton(label = next.first, onClick = next.second)
                } else if (end is JobEnd.Failed) {
                    G7PrimaryButton(label = stringResource(R.string.agents_git_back_to_thread), onClick = onClose)
                } else {
                    G7PrimaryButton(label = stringResource(R.string.agents_settings_done), onClick = onClose)
                }
            }
        }
    }
}

/** The header of the source-control screens: Back, the title, a line under it. */
@Composable
internal fun GitHeader(title: String, sub: String?, onBack: () -> Unit, trailing: @Composable () -> Unit = {}) {
    val scheme = MaterialTheme.colorScheme
    Column {
        Row(
            modifier = Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(start = 4.dp, end = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            G7IconButton(icon = G7Icons.back, contentDescription = stringResource(R.string.action_back), onClick = onBack)
            Column(Modifier.weight(1f).padding(start = 4.dp), verticalArrangement = Arrangement.spacedBy(1.dp)) {
                Text(title, style = MaterialTheme.typography.titleMedium.copy(fontWeight = FontWeight.SemiBold), maxLines = 1)
                if (!sub.isNullOrBlank()) Text(sub, style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant, maxLines = 1)
            }
            trailing()
        }
        Box(Modifier.fillMaxWidth().heightIn(min = 1.dp, max = 1.dp).background(scheme.outlineVariant))
    }
}

/** A line with an icon before it: a notice in a screen ("The files changed. Read them again."). */
@Composable
internal fun GitNotice(text: String, modifier: Modifier = Modifier, warn: Boolean = false) {
    val scheme = MaterialTheme.colorScheme
    Row(
        modifier.fillMaxWidth().padding(vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(if (warn) AgentGlyphs.warning else G7Icons.info, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
        Text(text, style = MaterialTheme.typography.bodyMedium)
    }
}
