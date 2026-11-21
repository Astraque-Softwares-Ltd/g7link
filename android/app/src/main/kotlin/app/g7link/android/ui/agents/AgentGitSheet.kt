// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import android.text.format.DateFormat
import androidx.compose.foundation.background
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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.mono
import java.util.Date

/**
 * The strip under a thread's status line (contract §SC9.3, board 15): the
 * branch (the worktree's glyph first in a worktree thread), what changed, the
 * commits to push and those on the remote as of the last fetch, and the pull
 * request's number with its checks. Offline it's the PC's last word, "as of
 * 12 min ago". A tap opens the sheet.
 */
@Composable
internal fun GitStripLine(strip: GitStrip, asOfMs: Long?, now: Long, onOpen: () -> Unit, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    val fonts = LocalAppFonts.current
    val description = stringResource(R.string.agents_git_source_control)
    Row(
        modifier = modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp, vertical = 6.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(scheme.surfaceContainer)
            .clickable(role = Role.Button, onClick = onOpen)
            .semantics { contentDescription = description }
            .heightIn(min = 44.dp)
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (strip.worktree != null) {
            Icon(G7Icons.folder, contentDescription = stringResource(R.string.agents_git_worktree), tint = scheme.onSurfaceVariant, modifier = Modifier.size(15.dp))
        }
        Icon(AgentGlyphs.branch, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
        val name = strip.branch ?: strip.detached?.let { stringResource(R.string.agents_git_detached, it) }.orEmpty()
        Text(
            name,
            style = MaterialTheme.typography.labelLarge.mono(fonts),
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f, fill = false),
        )
        if (strip.changed > 0) {
            Text(pluralStringResource(R.plurals.agents_git_changed, strip.changed, strip.changed), style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant, maxLines = 1)
        }
        strip.ahead?.takeIf { it > 0 }?.let { ahead ->
            Count(AgentGlyphs.push, ahead, pluralStringResource(R.plurals.agents_git_to_push, ahead, ahead))
        }
        strip.behind?.takeIf { it > 0 }?.let { behind ->
            Count(AgentGlyphs.behind, behind, pluralStringResource(R.plurals.agents_git_behind, behind, behind))
        }
        strip.pr?.let { pr ->
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                Text("#${pr.number}", style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant, maxLines = 1)
                AgentGit.checksMark(pr.pass, pr.fail, pr.pending)?.let { CheckMark(it, size = 12.dp) }
            }
        }
        if (asOfMs != null) {
            Text(asOf(asOfMs, now), style = MaterialTheme.typography.labelSmall, color = scheme.onSurfaceVariant, maxLines = 1)
        }
        Spacer(Modifier.weight(1f))
        Icon(G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
    }
}

@Composable
private fun Count(icon: ImageVector, count: Int, description: String) {
    val scheme = MaterialTheme.colorScheme
    Row(
        Modifier.semantics(mergeDescendants = true) { contentDescription = description },
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        Icon(icon, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(13.dp))
        Text(count.toString(), style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
    }
}

/** "as of 12 min ago": how old the PC's last word is, while it can't be reached. */
@Composable
internal fun asOf(ms: Long, now: Long): String {
    val (unit, count) = AgentOffline.ago(ms, now)
    return pluralStringResource(
        when (unit) {
            AgentOffline.AgoUnit.MINUTES -> R.plurals.agents_git_as_of_minutes
            AgentOffline.AgoUnit.HOURS -> R.plurals.agents_git_as_of_hours
            AgentOffline.AgoUnit.DAYS -> R.plurals.agents_git_as_of_days
        },
        count,
        count,
    )
}

/** A check's state as a mark of its own shape: passed a check, failed an x, running a ring, skipped or cancelled a hollow ring. */
@Composable
internal fun CheckMark(state: CheckState, size: Dp = 14.dp, modifier: Modifier = Modifier) {
    ThreadStatusMark(checkStatus(state), modifier = modifier, size = size)
}

internal fun checkStatus(state: CheckState): ThreadStatus = when (state) {
    CheckState.PASS -> ThreadStatus.Done
    CheckState.FAIL -> ThreadStatus.Failed(null)
    CheckState.PENDING -> ThreadStatus.Working
    CheckState.SKIPPED, CheckState.CANCELLED -> ThreadStatus.Stopped
}

/** What the sheet needs to know besides the folder: the PC, the thread and this phone's switch. */
internal data class GitSheetContext(
    val pcName: String,
    val agentName: String,
    val offline: Boolean,
    val pushAllowed: Boolean,
    val threadBusy: Boolean,
    /** When the folder was read, while the PC can't be reached. */
    val asOfMs: Long?,
    val now: Long,
)

/**
 * The sheet the strip opens (board 15): the branch line with when it was
 * checked, then Review and commit, Push, Pull request, Branch and Refresh,
 * each off with why when it can't be done. Not a repository: one line.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun AgentGitSheet(
    status: GitStatus?,
    prView: PrView?,
    refreshing: Boolean,
    context: GitSheetContext,
    onReview: () -> Unit,
    onPush: (PushRow.Ready) -> Unit,
    onPr: () -> Unit,
    onBranch: () -> Unit,
    onRefresh: () -> Unit,
    onDismiss: () -> Unit,
) {
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val scheme = MaterialTheme.colorScheme
    val androidContext = LocalContext.current
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = scheme.surfaceContainerHigh) {
        Column(Modifier.fillMaxWidth().padding(bottom = 20.dp)) {
            if (status == null) {
                Box(Modifier.fillMaxWidth().heightIn(min = 120.dp), contentAlignment = Alignment.Center) {
                    if (context.offline) {
                        Text(stringResource(R.string.agents_needs_pc, context.pcName), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
                    } else {
                        CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp)
                    }
                }
                return@Column
            }
            val line = AgentGit.stateLine(status)
            if (status.repo != RepoKind.OK) {
                Row(
                    Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 16.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Icon(G7Icons.info, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(18.dp))
                    Text(line?.let { stateText(it, context.pcName) }.orEmpty(), style = MaterialTheme.typography.bodyLarge.copy(fontWeight = FontWeight.Medium))
                }
                return@Column
            }
            // The branch line.
            Column(Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 10.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (status.worktree != null) Icon(G7Icons.folder, contentDescription = stringResource(R.string.agents_git_worktree), tint = scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
                    Icon(AgentGlyphs.branch, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(17.dp))
                    Text(
                        status.branch ?: status.detached?.let { stringResource(R.string.agents_git_detached, it) }.orEmpty(),
                        style = MaterialTheme.typography.bodyLarge.mono(LocalAppFonts.current),
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f),
                    )
                    if (refreshing) {
                        CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 1.5.dp)
                    } else if (status.checkedMs > 0L) {
                        Text(
                            stringResource(R.string.agents_git_checked, DateFormat.getTimeFormat(androidContext).format(Date(status.checkedMs))),
                            style = MaterialTheme.typography.labelMedium,
                            color = scheme.onSurfaceVariant,
                        )
                    }
                }
                for (sub in summary(status, context)) {
                    Text(sub, style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
                }
                if (line != null && line != GitStateLine.NoCommits) {
                    Text(stateText(line, context.pcName), style = MaterialTheme.typography.bodySmall, color = scheme.onSurface)
                }
            }
            val review = AgentGit.reviewRow(status, context.offline, context.threadBusy)
            SheetRow(
                icon = AgentGlyphs.commit,
                title = stringResource(R.string.agents_git_review),
                sub = when (review) {
                    ReviewRow.NeedsPc -> stringResource(R.string.agents_needs_pc, context.pcName)
                    is ReviewRow.Blocked -> stateText(review.line, context.pcName)
                    ReviewRow.ThreadBusy -> stringResource(R.string.agents_git_wait_finishes, context.agentName)
                    ReviewRow.Nothing -> stringResource(R.string.agents_git_nothing)
                    is ReviewRow.Ready -> pluralStringResource(R.plurals.agents_git_review_sub, review.files, review.files, review.adds, review.dels)
                },
                enabled = review is ReviewRow.Ready,
                onClick = onReview,
            )
            val push = AgentGit.pushRow(status, context.offline, context.pushAllowed)
            SheetRow(
                icon = AgentGlyphs.push,
                title = stringResource(R.string.agents_git_push),
                sub = pushSub(push, context.pcName),
                enabled = push is PushRow.Ready,
                onClick = { (push as? PushRow.Ready)?.let(onPush) },
            )
            val pr = AgentGit.prRow(status, prView, context.offline, context.pushAllowed)
            SheetRow(
                icon = AgentGlyphs.pullRequest,
                title = stringResource(R.string.agents_git_pr),
                sub = prSub(pr, context.pcName),
                enabled = pr is PrRowState.Shown || pr is PrRowState.CanOpen,
                onClick = onPr,
                loading = pr == PrRowState.Loading,
            )
            val branch = AgentGit.branchRow(status, context.offline)
            SheetRow(
                icon = AgentGlyphs.branch,
                title = stringResource(R.string.agents_git_branch),
                sub = when (branch) {
                    BranchRowState.NeedsPc -> stringResource(R.string.agents_needs_pc, context.pcName)
                    BranchRowState.InWorktree -> stringResource(R.string.agents_git_in_worktree)
                    is BranchRowState.Ready -> branch.branch ?: branch.detached?.let { stringResource(R.string.agents_git_detached, it) }.orEmpty()
                },
                enabled = branch is BranchRowState.Ready,
                onClick = onBranch,
            )
            if (!context.offline) {
                SheetRow(icon = G7Icons.refresh, title = stringResource(R.string.action_refresh), sub = null, enabled = !refreshing, onClick = onRefresh, chevron = false)
            }
        }
    }
}

/** The lines under the branch: what changed and what's to push, in step, or how old this is offline. */
@Composable
private fun summary(status: GitStatus, context: GitSheetContext): List<String> {
    if (context.offline && context.asOfMs != null) return listOf(asOf(context.asOfMs, context.now))
    if (AgentGit.inStep(status)) return listOf(stringResource(R.string.agents_git_in_step, status.pushRemote ?: status.upstream.orEmpty()))
    val parts = buildList {
        if (status.changed > 0) add(pluralStringResource(R.plurals.agents_files_changed, status.changed, status.changed))
        status.ahead?.takeIf { it > 0 }?.let { add(pluralStringResource(R.plurals.agents_git_to_push, it, it)) }
    }
    return buildList {
        if (parts.isNotEmpty()) add(parts.joinToString(" · "))
        status.behind?.takeIf { it > 0 }?.let { add(pluralStringResource(R.plurals.agents_git_behind, it, it)) }
    }
}

@Composable
internal fun pushSub(push: PushRow, pcName: String): String = when (push) {
    PushRow.NeedsPc -> stringResource(R.string.agents_needs_pc, pcName)
    PushRow.Detached -> stringResource(R.string.agents_git_detached_line)
    PushRow.NoCommits -> stringResource(R.string.agents_git_no_commits)
    PushRow.NoRemote -> stringResource(R.string.agents_git_no_remote)
    PushRow.UpToDate -> stringResource(R.string.agents_git_up_to_date)
    PushRow.SwitchOff -> stringResource(R.string.agents_git_switch_off, pcName)
    is PushRow.Ready -> {
        val commits = push.commits
        when {
            commits == null -> stringResource(R.string.agents_git_push_new)
            push.remote == null -> pluralStringResource(R.plurals.agents_git_commits, commits, commits)
            else -> pluralStringResource(R.plurals.agents_git_push_sub, commits, commits, push.remote)
        }
    }
}

@Composable
private fun prSub(pr: PrRowState, pcName: String): String = when (pr) {
    PrRowState.NeedsPc -> stringResource(R.string.agents_needs_pc, pcName)
    PrRowState.Loading -> stringResource(R.string.agents_loading)
    is PrRowState.Shown -> listOf(prTitle(pr.pr), checksText(AgentGit.checksLine(pr.pr))).joinToString(" · ")
    is PrRowState.Gh -> ghText(pr.gh)
    PrRowState.NotGithub -> stringResource(R.string.agents_git_not_github)
    PrRowState.NotPushed -> stringResource(R.string.agents_git_not_pushed)
    PrRowState.Detached -> stringResource(R.string.agents_git_detached_line)
    PrRowState.Unreached -> stringResource(R.string.agents_git_unreached_none)
    PrRowState.SwitchOff -> stringResource(R.string.agents_git_switch_off, pcName)
    PrRowState.CanOpen -> stringResource(R.string.agents_git_open_pr)
}

/** "#212 · Open" (or Draft, Merged, Closed). */
@Composable
internal fun prTitle(pr: PrInfo): String = stringResource(R.string.agents_git_pr_number, pr.number, prStateWord(pr.state, pr.draft))

@Composable
internal fun prStateWord(state: PrState, draft: Boolean): String = when {
    state == PrState.MERGED -> stringResource(R.string.agents_git_pr_merged)
    state == PrState.CLOSED -> stringResource(R.string.agents_git_pr_closed)
    draft -> stringResource(R.string.agents_git_pr_draft)
    else -> stringResource(R.string.agents_git_pr_open)
}

@Composable
internal fun checksText(line: ChecksLine): String = when (line) {
    ChecksLine.None -> stringResource(R.string.agents_git_no_checks)
    is ChecksLine.Passed -> pluralStringResource(R.plurals.agents_git_checks_passed, line.count, line.count)
    is ChecksLine.Failed -> stringResource(R.string.agents_git_checks_failed, line.count, line.total)
    is ChecksLine.Running -> stringResource(R.string.agents_git_checks_running, line.count, line.total)
}

/** The GitHub CLI's state on the PC, in words; the command to run is shown under it where there is one. */
@Composable
internal fun ghText(gh: GhState): String = when (gh) {
    GhState.MISSING -> stringResource(R.string.agents_git_gh_missing)
    GhState.SIGNED_OUT -> stringResource(R.string.agents_git_gh_signed_out_short)
    GhState.OLD -> stringResource(R.string.agents_git_gh_old)
    GhState.OK -> ""
}

/** The panel's state line in words. */
@Composable
internal fun stateText(line: GitStateLine, pcName: String): String = when (line) {
    GitStateLine.NotRepo -> stringResource(R.string.agents_git_not_repo)
    GitStateLine.NoGit -> stringResource(R.string.agents_git_no_git, pcName)
    GitStateLine.Refused -> stringResource(R.string.agents_git_refused)
    GitStateLine.Busy -> stringResource(R.string.agents_git_locked)
    is GitStateLine.Doing -> stringResource(
        when (line.doing) {
            GitDoing.MERGE -> R.string.agents_git_doing_merge
            GitDoing.REBASE -> R.string.agents_git_doing_rebase
            GitDoing.CHERRY_PICK -> R.string.agents_git_doing_cherry_pick
            GitDoing.REVERT -> R.string.agents_git_doing_revert
            GitDoing.BISECT -> R.string.agents_git_doing_bisect
        },
    )
    is GitStateLine.Detached -> stringResource(R.string.agents_git_detached_line)
    is GitStateLine.Conflicts -> pluralStringResource(R.plurals.agents_git_conflicts, line.count, line.count)
    GitStateLine.NoCommits -> stringResource(R.string.agents_git_no_commits)
}

@Composable
private fun SheetRow(
    icon: ImageVector,
    title: String,
    sub: String?,
    enabled: Boolean,
    onClick: () -> Unit,
    chevron: Boolean = true,
    loading: Boolean = false,
) {
    val scheme = MaterialTheme.colorScheme
    val ink = if (enabled) scheme.onSurface else scheme.onSurfaceVariant
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 56.dp)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = 20.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Icon(icon, contentDescription = null, tint = ink, modifier = Modifier.size(20.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(title, style = MaterialTheme.typography.bodyLarge.copy(fontWeight = FontWeight.Medium), color = ink)
            if (sub != null) Text(sub, style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
        }
        when {
            loading -> CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 1.5.dp)
            chevron -> Icon(G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp).alpha(if (enabled) 1f else 0.6f))
            else -> Spacer(Modifier.width(12.dp))
        }
    }
}

/** A command to run on the PC, in mono on the code ground ("gh auth login"). */
@Composable
internal fun CommandLine(command: String, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    Text(
        command,
        style = MaterialTheme.typography.bodyMedium.mono(LocalAppFonts.current),
        color = scheme.onSurface,
        modifier = modifier
            .clip(RoundedCornerShape(6.dp))
            .background(scheme.surfaceContainer)
            .padding(horizontal = 10.dp, vertical = 6.dp),
    )
}

/** Muted ink for a row that can't be used, as the boards draw it. */
@Composable
internal fun offInk(enabled: Boolean): Color = if (enabled) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant

/**
 * The push confirmation (board 17), each time: what goes where, the default
 * branch named when it's the one, and the remote to pick when git can't tell.
 */
@Composable
internal fun PushConfirm(push: PushRow.Ready, onPush: (String?) -> Unit, onDismiss: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    var picked by androidx.compose.runtime.saveable.rememberSaveable { androidx.compose.runtime.mutableStateOf(push.remote) }
    val remote = picked
    AgentSheet(onDismiss) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(
                if (remote != null) stringResource(R.string.agents_git_push_confirm, push.branch, remote) else stringResource(R.string.agents_git_choose_remote),
                style = MaterialTheme.typography.titleLarge,
            )
            val commits = push.commits
            Text(
                if (commits != null) pluralStringResource(R.plurals.agents_git_commits, commits, commits) else stringResource(R.string.agents_git_push_new),
                style = MaterialTheme.typography.bodyMedium,
                color = scheme.onSurfaceVariant,
            )
            if (push.remote == null) {
                for (name in push.remotes) {
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .heightIn(min = 44.dp)
                            .clip(RoundedCornerShape(8.dp))
                            .clickable(role = Role.RadioButton) { picked = name }
                            .padding(horizontal = 4.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Box(Modifier.size(18.dp), contentAlignment = Alignment.Center) {
                            if (picked == name) Icon(G7Icons.check, contentDescription = null, tint = scheme.primary, modifier = Modifier.size(18.dp))
                        }
                        Text(name, style = MaterialTheme.typography.bodyLarge.mono(LocalAppFonts.current))
                    }
                }
            }
            if (push.toDefault) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Icon(AgentGlyphs.warning, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
                    Text(stringResource(R.string.agents_git_push_default, push.branch), style = MaterialTheme.typography.bodyMedium)
                }
            }
            Row(Modifier.fillMaxWidth().padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                app.g7link.android.ui.theme.G7RaisedButton(label = stringResource(R.string.action_cancel), onClick = onDismiss, modifier = Modifier.weight(1f))
                app.g7link.android.ui.theme.G7PrimaryButton(
                    label = stringResource(R.string.agents_git_push),
                    onClick = { onPush(remote) },
                    enabled = remote != null,
                    modifier = Modifier.weight(1f),
                )
            }
        }
    }
}
