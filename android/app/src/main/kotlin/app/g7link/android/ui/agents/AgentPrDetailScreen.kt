// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.core.AgentItem
import app.g7link.core.AgentItemKind
import app.g7link.core.AgentRow
import app.g7link.core.AgentRowKind
import app.g7link.core.AgentThreadState
import app.g7link.core.AgentTimeline
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * A pull request in full (phase 6 §X6.6), from its card in a thread or a row
 * of the Pull requests list: the card, who opened it and its description, the
 * files it changes (each opening its diff), its checks and its comments, read
 * through the PC's GitHub CLI. "Ask <agent> to review it" sends one fixed
 * message in a thread after a confirmation; nothing is posted to GitHub.
 * [threadId]: the thread it was opened from; [repo] and [number]: a row of the
 * list. Lines picked in a file's diff become a quote for the reply box
 * ([onComment]; not offered from the list).
 */
@Composable
internal fun AgentPrDetailScreen(
    device: Device,
    core: Core,
    agents: AgentsState,
    threadId: String?,
    repo: String?,
    number: Int?,
    onComment: ((Quote) -> Unit)?,
    onAsked: (String) -> Unit,
    onClose: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val scheme = MaterialTheme.colorScheme
    val resources = LocalResources.current
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var call by remember(threadId, repo, number) { mutableStateOf<AgentCall<PrDetail>?>(null) }
    var attempt by remember(threadId, repo, number) { mutableIntStateOf(0) }
    var refresh by remember(threadId, repo, number) { mutableStateOf(false) }
    var file by remember(threadId, repo, number) { mutableStateOf<GitFile?>(null) }
    var asking by remember(threadId, repo, number) { mutableStateOf(false) }
    var confirm by remember(threadId, repo, number) { mutableStateOf<PrAsk.Ready?>(null) }
    var link by remember { mutableStateOf<PendingLink?>(null) }
    val copy = rememberCopy { core.say(resources.getString(R.string.agents_copied)) }
    // Text from GitHub: every link shows where it goes before it opens.
    val actions = remember(copy) {
        RowActions(
            toggle = {},
            link = { span, shown -> link = PendingLink(span.href, shown) },
            copy = copy,
            output = {},
            diff = { _, _ -> },
            mark = {},
        )
    }
    LaunchedEffect(device.id, threadId, repo, number, attempt, device.online) {
        if (!device.online) return@LaunchedEffect
        val read = core.agentPrDetail(device.id, threadId, repo, number, refresh)
        refresh = false
        // A failed read again keeps what was shown.
        if (read is AgentCall.Ok || call !is AgentCall.Ok) call = read
    }
    val reload: () -> Unit = {
        refresh = true
        attempt++
    }
    BackHandler {
        if (file != null) file = null else onClose()
    }
    val view = AgentPrDetails.view(call, offline = !device.online)
    val shown = view as? PrDetailView.Shown
    val detail = shown?.detail
    val open = file
    if (detail != null && open != null) {
        PullRequestFileScreen(core, device.id, detail.repo, detail.pr?.number ?: 0, open, onComment = onComment, onBack = { file = null }, modifier = modifier)
        return
    }
    val prNumber = detail?.pr?.number ?: number ?: 0
    Column(modifier.fillMaxSize().background(scheme.surface)) {
        GitHeader(
            title = if (prNumber > 0) stringResource(R.string.agents_pr_detail_title, prNumber) else stringResource(R.string.agents_git_pr),
            sub = (detail?.repo ?: repo)?.takeIf { it.isNotBlank() },
            onBack = onClose,
        )
        when (view) {
            PrDetailView.NeedsPc -> Box(Modifier.fillMaxWidth().padding(16.dp)) { GitNotice(stringResource(R.string.agents_needs_pc, device.name)) }
            PrDetailView.Loading -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp) }
            PrDetailView.Failed -> Box(Modifier.fillMaxWidth().padding(16.dp)) { GitNotice(stringResource(R.string.agents_failed_generic)) }
            is PrDetailView.Gh -> Box(Modifier.fillMaxWidth().padding(16.dp)) { GhReason(view.gh, device.name) }
            is PrDetailView.Missing -> Box(Modifier.fillMaxWidth().padding(16.dp)) { GitNotice(missingText(view.why)) }
            is PrDetailView.Shown -> {
                PrDetailBody(
                    view = view,
                    pcName = device.name,
                    onRefresh = reload,
                    onFile = { picked -> if (AgentPrDetails.opens(view.detail, picked.path)) file = picked },
                    actions = actions,
                    modifier = Modifier.weight(1f),
                )
                val askThread = threadId ?: view.detail.threadId
                val thread = askThread?.let(agents::thread)
                val agentName = thread?.let { agents.agentName(it.agent) }.orEmpty()
                val ask = if (thread == null) {
                    PrAsk.NoThread
                } else {
                    AgentPrDetails.ask(
                        detail = view.detail,
                        threadId = thread.id,
                        readonly = thread.readonly != null,
                        busy = AgentsModel.running(thread) || thread.state == AgentThreadState.WAITING,
                        offline = !device.online,
                        asking = asking,
                    )
                }
                AskBar(ask, agentName = agentName, pcName = device.name, onAsk = { confirm = it })
            }
        }
    }
    confirm?.let { ready ->
        val agentName = agents.thread(ready.threadId)?.let { agents.agentName(it.agent) }.orEmpty()
        AgentSheet(onDismiss = { confirm = null }) {
            Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(stringResource(R.string.agents_pr_ask_review, agentName), style = MaterialTheme.typography.titleLarge)
                Text(stringResource(R.string.agents_pr_ask_review_sub, agentName), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
                Row(Modifier.fillMaxWidth().padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    G7RaisedButton(label = stringResource(R.string.action_cancel), onClick = { confirm = null }, modifier = Modifier.weight(1f))
                    G7PrimaryButton(
                        label = stringResource(R.string.agents_pr_ask_review, agentName),
                        onClick = {
                            confirm = null
                            asking = true
                            scope.launch {
                                val sent = core.agentPrAsk(device.id, ready.threadId, ready.number)
                                asking = false
                                when (sent) {
                                    is AgentCall.Ok -> onAsked(ready.threadId)
                                    is AgentCall.Failed -> core.say(
                                        when (sent.failure) {
                                            AgentFailure.OFFLINE -> resources.getString(R.string.agents_needs_pc, device.name)
                                            // The PC's BUSY: the thread started working meanwhile.
                                            AgentFailure.OTHER -> resources.getString(R.string.agents_git_wait_finishes, agentName)
                                            AgentFailure.DENIED, AgentFailure.UNSUPPORTED, AgentFailure.NOT_FOUND, AgentFailure.TIMEOUT ->
                                                resources.getString(R.string.agents_failed_generic)
                                        },
                                    )
                                }
                            }
                        },
                        modifier = Modifier.weight(1f),
                    )
                }
            }
        }
    }
    link?.let { shownLink ->
        LinkSheet(
            link = shownLink,
            onOpen = {
                link = null
                openLink(context, shownLink.href)
            },
            onDismiss = { link = null },
        )
    }
}

/** Why there's no pull request to show. */
@Composable
private fun missingText(why: PrDetailWhy): String = stringResource(
    when (why) {
        PrDetailWhy.NOT_GITHUB, PrDetailWhy.NOT_REPO -> R.string.agents_git_not_github
        PrDetailWhy.NO_UPSTREAM -> R.string.agents_git_not_pushed
        PrDetailWhy.DETACHED -> R.string.agents_git_detached_line
        PrDetailWhy.NETWORK -> R.string.agents_git_unreached_none
        PrDetailWhy.TOO_LARGE -> R.string.agents_pr_detail_too_large
        PrDetailWhy.NO_PR -> R.string.agents_git_no_prs
    },
)

/** The card, the description, Changes, Checks and Comments, as rows. */
@Composable
private fun PrDetailBody(
    view: PrDetailView.Shown,
    pcName: String,
    onRefresh: () -> Unit,
    onFile: (GitFile) -> Unit,
    actions: RowActions,
    modifier: Modifier = Modifier,
) {
    val scheme = MaterialTheme.colorScheme
    val detail = view.detail
    val pr = detail.pr ?: return
    val now = rememberNow()
    LazyColumn(modifier.fillMaxWidth(), contentPadding = PaddingValues(bottom = 16.dp)) {
        item(key = "card", contentType = "card") {
            Box(Modifier.padding(horizontal = 16.dp, vertical = 12.dp)) {
                PrCard(pr, unreached = view.unreached, host = AgentPrDetails.host(detail.repo), onRefresh = onRefresh)
            }
        }
        item(key = "about", contentType = "text") {
            Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                if (detail.author.isNotBlank()) {
                    Text(stringResource(R.string.agents_pr_detail_by, detail.author), style = MaterialTheme.typography.labelLarge, color = scheme.onSurfaceVariant)
                }
                if (detail.body.isBlank()) {
                    Text(stringResource(R.string.agents_pr_detail_no_body), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
                } else {
                    PrMarkdown(id = "body", text = detail.body, actions = actions)
                }
                if (detail.bodyCut) CutLine()
            }
        }
        item(key = "changes", contentType = "head") {
            SectionHead(stringResource(R.string.agents_pr_detail_changes)) {
                Text(pluralStringResource(R.plurals.agents_pr_detail_files, detail.changed, detail.changed), style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
                Counts(detail.adds.toLong(), detail.dels.toLong())
            }
        }
        if (view.tooLarge) {
            item(key = "too_large", contentType = "label") { GitNoticeRow(stringResource(R.string.agents_pr_detail_too_large)) }
        }
        items(detail.files.size, key = { "file:$it" }, contentType = { "file" }) { index ->
            val file = detail.files[index]
            GitFileRow(file, onOpen = { onFile(file) }, modifier = Modifier.padding(horizontal = 2.dp))
        }
        val more = AgentPrDetails.moreFiles(detail)
        if (more > 0) {
            item(key = "files_more", contentType = "label") {
                Text(
                    stringResource(R.string.agents_pr_detail_files_more, more),
                    style = MaterialTheme.typography.bodySmall,
                    color = scheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                )
            }
        }
        item(key = "checks", contentType = "head") {
            SectionHead(stringResource(R.string.agents_pr_detail_checks)) {
                AgentGit.checksMark(pr.pass, pr.fail, pr.pending)?.let { CheckMark(it, size = 13.dp) }
                Text(checksText(AgentGit.checksLine(pr)), style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
            }
        }
        // Two checks can share a name: keyed by place.
        val checks = AgentGit.sortedChecks(pr.checks)
        items(checks.size, key = { "check:$it" }, contentType = { "check" }) { index ->
            Box(Modifier.padding(horizontal = 16.dp)) { CheckRow(checks[index]) }
        }
        item(key = "comments", contentType = "head") { SectionHead(stringResource(R.string.agents_pr_detail_comments)) {} }
        if (detail.commentsOlder > 0) {
            item(key = "older", contentType = "label") {
                Text(
                    pluralStringResource(R.plurals.agents_pr_detail_older, detail.commentsOlder, detail.commentsOlder),
                    style = MaterialTheme.typography.bodySmall,
                    color = scheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp),
                )
            }
        }
        if (AgentPrDetails.noComments(detail)) {
            item(key = "no_comments", contentType = "label") {
                Text(
                    stringResource(R.string.agents_pr_detail_no_comments),
                    style = MaterialTheme.typography.bodyMedium,
                    color = scheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp),
                )
            }
        }
        items(detail.comments.size, key = { "comment:$it" }, contentType = { "comment" }) { index ->
            CommentView(detail.comments[index], id = "c$index", now = now, actions = actions)
        }
        item(key = "line_comments", contentType = "label") {
            Box(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) { GitNotice(stringResource(R.string.agents_pr_detail_line_comments)) }
        }
    }
}

/** A section's name and what it says on the same line. */
@Composable
private fun SectionHead(title: String, trailing: @Composable () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Column(Modifier.fillMaxWidth().padding(top = 16.dp)) {
        Box(Modifier.fillMaxWidth().height(1.dp).background(scheme.outlineVariant))
        Row(
            Modifier.fillMaxWidth().heightIn(min = 44.dp).padding(horizontal = 16.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(title, style = MaterialTheme.typography.titleSmall.copy(fontWeight = FontWeight.SemiBold), modifier = Modifier.weight(1f))
            trailing()
        }
    }
}

/** "Shortened to fit." */
@Composable
private fun CutLine() {
    Text(stringResource(R.string.agents_pr_detail_cut), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

/** A comment: who, when, the review's word when it's a review, then what it says. */
@Composable
private fun CommentView(comment: PrComment, id: String, now: Long, actions: RowActions) {
    val scheme = MaterialTheme.colorScheme
    Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        val parts = listOfNotNull(
            comment.atMs.takeIf { it > 0L }?.let { relativeTime(it, now) },
            comment.review?.let { reviewText(it) },
        )
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(comment.author, style = MaterialTheme.typography.labelLarge.copy(fontWeight = FontWeight.SemiBold), maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (parts.isNotEmpty()) {
                Text(parts.joinToString(" · ", prefix = "· "), style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        if (comment.body.isNotBlank()) PrMarkdown(id = id, text = comment.body, actions = actions)
        if (comment.cut) CutLine()
    }
}

@Composable
private fun reviewText(word: PrReviewWord): String = stringResource(
    when (word) {
        PrReviewWord.APPROVED -> R.string.agents_pr_detail_approved
        PrReviewWord.CHANGES_REQUESTED -> R.string.agents_pr_detail_changes_requested
        PrReviewWord.COMMENTED -> R.string.agents_pr_detail_reviewed
        PrReviewWord.DISMISSED -> R.string.agents_pr_detail_dismissed
    },
)

/**
 * GitHub text drawn as the render crate reads Markdown, under its bounds: raw
 * HTML stays text, no image is loaded, and every link shows where it goes
 * before it opens.
 */
@Composable
private fun PrMarkdown(id: String, text: String, actions: RowActions) {
    val timeline = remember { AgentTimeline() }
    DisposableEffect(timeline) { onDispose { timeline.close() } }
    val item = remember(id, text) { AgentItem(seq = 1uL, id = id, kind = AgentItemKind.ASSISTANT, whenMs = 0L, text = text, truncated = false, tool = null) }
    val rows by produceState<List<AgentRow>?>(null, timeline, item) {
        value = withContext(Dispatchers.Default) {
            runCatching {
                timeline.rows(listOf(item), emptyList(), AgentThreadState.DONE, 0L, System.currentTimeMillis(), emptyList(), emptyList(), emptyList(), emptyList())
            }.getOrNull()
        }
    }
    val made = rows
    if (made == null) {
        // Until it's made (or if it can't be): the words as they are.
        Text(text, style = MaterialTheme.typography.bodyMedium)
        return
    }
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        for (row in made) {
            val kind = row.kind as? AgentRowKind.Block ?: continue
            BlockRow(row, kind, item, actions)
        }
    }
}

/** "Ask <agent> to review it", or why it isn't offered. */
@Composable
private fun AskBar(ask: PrAsk, agentName: String, pcName: String, onAsk: (PrAsk.Ready) -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Column {
        Box(Modifier.fillMaxWidth().height(1.dp).background(scheme.outlineVariant))
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            when (ask) {
                PrAsk.NoThread -> GitNotice(stringResource(R.string.agents_pr_ask_needs_thread, pcName))
                is PrAsk.Ready -> {
                    G7PrimaryButton(label = stringResource(R.string.agents_pr_ask_review, agentName), onClick = { onAsk(ask) })
                    Text(stringResource(R.string.agents_pr_ask_review_sub, agentName), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
                }
                PrAsk.Asking -> Box(contentAlignment = Alignment.Center) {
                    G7PrimaryButton(label = stringResource(R.string.agents_pr_ask_review, agentName), onClick = {}, enabled = false)
                    CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                }
                PrAsk.Busy -> {
                    G7PrimaryButton(label = stringResource(R.string.agents_pr_ask_review, agentName), onClick = {}, enabled = false)
                    Text(stringResource(R.string.agents_git_wait_finishes, agentName), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
                }
                PrAsk.NeedsPc -> {
                    G7PrimaryButton(label = stringResource(R.string.agents_pr_ask_review, agentName), onClick = {}, enabled = false)
                    Text(stringResource(R.string.agents_needs_pc, pcName), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
                }
            }
        }
    }
}
