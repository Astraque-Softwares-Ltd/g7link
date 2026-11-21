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
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Checkbox
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
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
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
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.mono
import app.g7link.core.AgentThread
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch

/** How long a file that fits on the screen stays in view before it counts as read. */
private const val FITS_READ_MS = 1_000L

/** Reviews asked for again, at most, when the PC finds files it can't send whole. */
private const val REVIEW_RETRIES = 3

/**
 * Review and commit on the phone (contract §SC9.3, board 16): the files, all
 * ticked; each read to its last line ("Read 2 of 4"); then the message and
 * Commit, confirmed in a sheet before anything is sent; then the job's
 * output and how it ended. A change in the folder starts it over.
 */
@Composable
internal fun AgentReviewScreen(
    device: Device,
    core: Core,
    threadId: String,
    initial: GitStatus,
    thread: AgentThread?,
    agentName: String,
    pushAllowed: Boolean,
    onPush: (PushRow.Ready) -> Unit,
    onClose: () -> Unit,
) {
    val resources = LocalResources.current
    val scope = rememberCoroutineScope()
    val all by core.agents.collectAsStateWithLifecycle()
    val agents = all[device.id] ?: AgentsState()
    val offline = !device.online
    var review by remember(threadId) { mutableStateOf(AgentReview.open(initial)) }
    var reviewAsk by remember(threadId) { mutableIntStateOf(1) }
    var reviewTries by remember(threadId) { mutableIntStateOf(0) }
    var reviewing by remember(threadId) { mutableStateOf(false) }
    var startAfter by remember(threadId) { mutableStateOf(false) }
    var docs by remember(threadId) { mutableStateOf(emptyMap<String, GitFileDiff>()) }
    var write by remember(threadId) { mutableStateOf(WriteField()) }
    var confirm by remember(threadId) { mutableStateOf(false) }
    var committing by remember(threadId) { mutableStateOf(false) }
    var jobId by remember(threadId) { mutableStateOf<String?>(null) }
    var refused by remember(threadId) { mutableStateOf<JobError?>(null) }
    var after by remember(threadId) { mutableStateOf<GitStatus?>(null) }
    val job = jobId?.let { agents.gitJobs[it] }
    val strip = agents.gitStrips[threadId]

    // ---- The PC's review of the ticked files: asked on opening, and again when the ticks changed ----
    LaunchedEffect(reviewAsk, device.online) {
        if (offline || jobId != null || !AgentReview.needsReview(review)) return@LaunchedEffect
        reviewing = true
        val paths = review.tickedPaths
        when (val answer = core.agentGitReview(device.id, threadId, paths)) {
            is AgentCall.Ok -> {
                review = AgentReview.reviewed(review, paths, answer.value.review, answer.value.unreadable)
                if (AgentReview.needsReview(review) && reviewTries < REVIEW_RETRIES) {
                    // It covered a file it can't send whole: asked again without it.
                    reviewTries++
                    reviewAsk++
                } else if (startAfter && !AgentReview.needsReview(review)) {
                    startAfter = false
                    review = AgentReview.startReading(review)
                }
            }
            is AgentCall.Failed -> {
                startAfter = false
                core.say(resources.getString(R.string.agents_failed_generic))
            }
        }
        reviewing = false
    }

    // ---- The file on screen, read for the review in use; the PC counts it sent once all of it went ----
    val current = review.current
    LaunchedEffect(review.review, current?.path, device.online) {
        val path = current?.path ?: return@LaunchedEffect
        val id = review.review ?: return@LaunchedEffect
        if (offline) return@LaunchedEffect
        if (review.files.firstOrNull { it.path == path }?.served == true && docs[path] is GitFileDiff.Shown) return@LaunchedEffect
        docs = docs + (path to GitFileDiff.Loading)
        val shown = when (val patch = core.agentGitPatch(device.id, threadId, path, id)) {
            is AgentCall.Ok -> {
                review = AgentReview.served(review, id, path, AgentReview.hash(patch.value.content), whole = !patch.value.truncated)
                gitFileDiff(patch.value)
            }
            is AgentCall.Failed -> GitFileDiff.Failed(patch.failure)
        }
        docs = docs + (path to shown)
    }
    // Files read before a new review: sent again under it, quietly, and checked against what was read.
    val toServe = AgentReview.toServe(review)
    LaunchedEffect(review.review, toServe, device.online) {
        val id = review.review ?: return@LaunchedEffect
        if (offline) return@LaunchedEffect
        for (path in toServe) {
            val patch = core.agentGitPatch(device.id, threadId, path, id) as? AgentCall.Ok ?: continue
            review = AgentReview.served(review, id, path, AgentReview.hash(patch.value.content), whole = !patch.value.truncated)
        }
    }
    // The PC says the folder changed: read it again, and start over if the files differ.
    LaunchedEffect(strip, device.online) {
        if (offline || jobId != null || committing) return@LaunchedEffect
        val status = core.agentGitStatus(device.id, threadId) as? AgentCall.Ok ?: return@LaunchedEffect
        val next = AgentReview.folderChanged(review, status.value)
        if (next !== review) {
            review = next
            docs = emptyMap()
            reviewTries = 0
            reviewAsk++
        }
    }
    // The agent's answer when asked to write the message.
    val wrote = thread?.let(AgentGitFfi::wrote)
    val running = thread?.let(AgentsModel::running) == true
    LaunchedEffect(wrote, running) { write = AgentWrite.thread(write, WritePurpose.COMMIT, wrote, running) }
    // Once committed, what a push would send.
    LaunchedEffect(job?.phase) {
        if (job?.phase != JobPhase.DONE) return@LaunchedEffect
        after = (core.agentGitStatus(device.id, threadId) as? AgentCall.Ok)?.value
    }

    val reload: () -> Unit = {
        scope.launch {
            val status = core.agentGitStatus(device.id, threadId) as? AgentCall.Ok
            if (status != null) {
                review = AgentReview.open(status.value, changed = true)
                docs = emptyMap()
                jobId = null
                refused = null
                reviewTries = 0
                reviewAsk++
            }
        }
    }
    val commit: () -> Unit = commit@{
        val id = AgentReview.commitReview(review, write.text) ?: return@commit
        confirm = false
        committing = true
        refused = null
        scope.launch {
            when (val started = core.agentGitCommit(device.id, threadId, id, write.text.trim())) {
                is GitStart.Started -> jobId = started.job.id
                is GitStart.Refused -> {
                    refused = started.error
                    if (started.error == null) core.say(resources.getString(R.string.agents_failed_generic))
                }
            }
            committing = false
        }
    }
    val ask: () -> Unit = {
        write = AgentWrite.asked(write, wrote)
        scope.launch {
            if (core.agentAsk(device.id, threadId, WritePurpose.COMMIT) is AgentCall.Failed) {
                write = AgentWrite.askFailed(write)
                core.say(resources.getString(R.string.agents_failed_generic))
            }
        }
    }

    BackHandler {
        when {
            jobId != null -> onClose()
            review.step == ReviewStep.READ -> review = AgentReview.files(review)
            review.step == ReviewStep.MESSAGE -> review = AgentReview.files(review)
            else -> onClose()
        }
    }

    val scheme = MaterialTheme.colorScheme
    val branch = review.branch ?: initial.branch.orEmpty()
    val sub = listOf(stringResource(R.string.agents_git_read_of, review.readCount, review.ticked.size), branch).filter { it.isNotBlank() }.joinToString(" · ")
    Column(Modifier.fillMaxSize().background(scheme.surface).imePadding()) {
        GitHeader(
            title = stringResource(R.string.agents_git_review),
            sub = sub,
            onBack = {
                if (jobId == null && review.step != ReviewStep.FILES) review = AgentReview.files(review) else onClose()
            },
        )
        if (jobId != null) {
            CommitJob(
                job = job,
                pcName = device.name,
                agentName = agentName,
                push = after?.let { AgentGit.pushRow(it, offline, pushAllowed) } as? PushRow.Ready,
                onStop = { jobId?.let { id -> scope.launch { core.agentGitStop(device.id, id) } } },
                onReload = reload,
                onPush = onPush,
                onClose = onClose,
            )
            return@Column
        }
        when (review.step) {
            ReviewStep.FILES -> FilesStep(
                review = review,
                pcName = device.name,
                reviewing = reviewing,
                onToggle = { path -> review = AgentReview.toggle(review, path) },
                onOpen = { path ->
                    if (AgentReview.needsReview(review)) {
                        startAfter = true
                        reviewTries = 0
                        reviewAsk++
                    } else {
                        review = AgentReview.read(review, path)
                    }
                },
                onStart = {
                    when {
                        AgentReview.allRead(review) -> review = AgentReview.toMessage(review)
                        AgentReview.needsReview(review) -> {
                            startAfter = true
                            reviewTries = 0
                            reviewAsk++
                        }
                        else -> review = AgentReview.startReading(review)
                    }
                },
            )
            ReviewStep.READ -> ReadStep(
                review = review,
                doc = current?.let { docs[it.path] },
                onEnd = { path -> review = AgentReview.reachedEnd(review, path) },
                onNext = { review = AgentReview.next(review) },
            )
            ReviewStep.MESSAGE -> MessageStep(
                review = review,
                write = write,
                title = thread?.title.orEmpty(),
                agentName = agentName,
                committing = committing,
                refused = refused,
                pcName = device.name,
                onWrite = { text -> write = AgentWrite.typed(write, text) },
                onUse = { text -> write = AgentWrite.use(write, text) },
                onAsk = ask,
                onCommit = { if (AgentReview.canCommit(review, write.text)) confirm = true },
                onReload = reload,
            )
        }
    }
    if (confirm) {
        CommitConfirm(
            files = review.ticked.size,
            branch = branch,
            pcName = device.name,
            subject = AgentGit.subject(write.text),
            onCommit = commit,
            onDismiss = { confirm = false },
        )
    }
}

/** Step 1: the files, ticked; each with how far it's been read, or why it can't be read here. */
@Composable
private fun FilesStep(
    review: ReviewState,
    pcName: String,
    reviewing: Boolean,
    onToggle: (String) -> Unit,
    onOpen: (String) -> Unit,
    onStart: () -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    Column(Modifier.fillMaxSize()) {
        LazyColumn(Modifier.weight(1f).fillMaxWidth()) {
            if (review.changed) item(key = "changed", contentType = "label") { GitNoticeRow(stringResource(R.string.agents_git_files_again)) }
            if (review.capped) item(key = "capped", contentType = "label") { GitNoticeRow(stringResource(R.string.agents_git_max_files)) }
            items(review.files, key = { "f:${it.path}" }, contentType = { "file" }) { file ->
                val include = stringResource(R.string.agents_git_include, file.path)
                Column {
                    GitFileRow(
                        file = file.file,
                        onOpen = if (file.ticked) ({ onOpen(file.path) }) else null,
                        leading = {
                            Checkbox(
                                checked = file.ticked,
                                onCheckedChange = { onToggle(file.path) },
                                enabled = file.tickable,
                                modifier = Modifier.semantics { contentDescription = include },
                            )
                        },
                        trailing = { if (file.tickable) ReadMarkLabel(file.mark) },
                    )
                    if (file.unreadable != null) {
                        Text(
                            stringResource(R.string.agents_git_too_large, pcName),
                            style = MaterialTheme.typography.bodySmall,
                            color = scheme.onSurfaceVariant,
                            modifier = Modifier.padding(start = 62.dp, end = 16.dp, bottom = 8.dp),
                        )
                    }
                }
            }
        }
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            if (review.ticked.isEmpty()) {
                Text(stringResource(R.string.agents_git_tick_one), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
            }
            val label = when {
                AgentReview.allRead(review) -> stringResource(R.string.agents_question_next)
                review.readCount > 0 -> stringResource(R.string.agents_git_keep_reading)
                else -> stringResource(R.string.agents_git_start_reading)
            }
            G7PrimaryButton(label = label, onClick = onStart, enabled = review.ticked.isNotEmpty() && !reviewing)
        }
    }
}

/** "Not read" (a hollow ring), "Reading" (a ring), "Read" (a check). */
@Composable
private fun ReadMarkLabel(mark: ReadMark) {
    val scheme = MaterialTheme.colorScheme
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp)) {
        when (mark) {
            ReadMark.NOT_READ -> CheckMark(CheckState.SKIPPED, size = 12.dp)
            ReadMark.READING -> CheckMark(CheckState.PENDING, size = 12.dp)
            ReadMark.READ -> CheckMark(CheckState.PASS, size = 12.dp)
        }
        Text(
            stringResource(
                when (mark) {
                    ReadMark.NOT_READ -> R.string.agents_git_not_read
                    ReadMark.READING -> R.string.agents_git_reading
                    ReadMark.READ -> R.string.agents_git_read
                },
            ),
            style = MaterialTheme.typography.labelSmall,
            color = scheme.onSurfaceVariant,
        )
    }
}

/**
 * Step 2: one file at a time. "Next file" stays off until the last line has
 * been in view (a file that fits counts after a second on screen); a bar
 * says "Scroll to the end to continue".
 */
@Composable
private fun ReadStep(review: ReviewState, doc: GitFileDiff?, onEnd: (String) -> Unit, onNext: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val fonts = LocalAppFonts.current
    val file = review.current ?: return
    val ticked = review.ticked
    val position = ticked.indexOfFirst { it.path == file.path } + 1
    val list = rememberLazyListState()
    val served = file.served
    // Read once the end has been in view: at once after scrolling there, after a second when it all fits.
    LaunchedEffect(file.path, served, doc) {
        if (!served || doc == null || doc is GitFileDiff.Loading || doc is GitFileDiff.Failed || file.mark == ReadMark.READ) return@LaunchedEffect
        while (true) {
            snapshotFlow { list.layoutInfo.totalItemsCount > 0 && !list.canScrollForward }.first { it }
            val fits = list.firstVisibleItemIndex == 0 && list.firstVisibleItemScrollOffset == 0
            if (!fits) break
            delay(FITS_READ_MS)
            if (!list.canScrollForward) break
        }
        onEnd(file.path)
    }
    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 44.dp).padding(horizontal = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(changeLetter(file.file.change), style = MaterialTheme.typography.labelLarge.mono(fonts), color = changeColor(file.file.change))
            Text(file.path, style = MaterialTheme.typography.bodySmall.mono(fonts), maxLines = 1, overflow = TextOverflow.StartEllipsis, modifier = Modifier.weight(1f))
            val adds = file.file.adds
            val dels = file.file.dels
            if (!file.file.binary && adds != null && dels != null) Counts(adds.toLong(), dels.toLong())
            Text(stringResource(R.string.agents_dock_count, position, ticked.size), style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
        }
        Box(Modifier.weight(1f).fillMaxWidth()) {
            when (doc) {
                null, GitFileDiff.Loading -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp) }
                is GitFileDiff.Failed -> Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
                    Text(stringResource(R.string.agents_failed_generic), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
                }
                GitFileDiff.Binary -> LazyColumn(Modifier.fillMaxSize(), state = list) {
                    item { GitNoticeRow(stringResource(R.string.agents_diff_binary)) }
                    item { EndOf(file.path) }
                }
                is GitFileDiff.Folder -> LazyColumn(Modifier.fillMaxSize().padding(horizontal = 16.dp), state = list) {
                    items(doc.names.size) { i -> Text(doc.names[i], style = MaterialTheme.typography.bodySmall.mono(fonts), modifier = Modifier.padding(vertical = 3.dp)) }
                    if (doc.more > 0) {
                        item {
                            Text(pluralStringResource(R.plurals.agents_diff_more, doc.more, doc.more), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
                        }
                    }
                    item { EndOf(file.path) }
                }
                is GitFileDiff.Shown -> DiffLinesList(
                    items = doc.items,
                    state = list,
                    selection = null,
                    onSelect = null,
                    footer = { item(key = "end", contentType = "end") { EndOf(file.path) } },
                )
            }
        }
        val read = file.mark == ReadMark.READ && served
        Column {
            Box(Modifier.fillMaxWidth().heightIn(min = 1.dp, max = 1.dp).background(scheme.outlineVariant))
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                if (read) {
                    CheckMark(CheckState.PASS, size = 15.dp)
                    Text(stringResource(R.string.agents_git_read), style = MaterialTheme.typography.bodyMedium, modifier = Modifier.weight(1f))
                } else {
                    Icon(AgentGlyphs.behind, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(15.dp))
                    Text(stringResource(R.string.agents_git_scroll_to_end), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant, modifier = Modifier.weight(1f))
                }
                val last = ticked.count { it.mark != ReadMark.READ } == 0 ||
                    ticked.none { it.path != file.path && it.mark != ReadMark.READ }
                G7RaisedButton(
                    label = stringResource(if (last) R.string.agents_question_next else R.string.agents_git_next_file),
                    onClick = onNext,
                    enabled = AgentReview.canNext(review),
                )
            }
        }
    }
}

@Composable
private fun EndOf(path: String) {
    val scheme = MaterialTheme.colorScheme
    Text(
        stringResource(R.string.agents_git_end_of, path),
        style = MaterialTheme.typography.labelMedium,
        color = scheme.onSurfaceVariant,
        maxLines = 1,
        overflow = TextOverflow.StartEllipsis,
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 14.dp),
    )
}

/**
 * Step 3: the message (its first line the subject, "Subject is long" past
 * 72), "Use the thread's title", "Ask <agent> to write it", what is committed
 * where, and Commit.
 */
@Composable
private fun MessageStep(
    review: ReviewState,
    write: WriteField,
    title: String,
    agentName: String,
    committing: Boolean,
    refused: JobError?,
    pcName: String,
    onWrite: (String) -> Unit,
    onUse: (String) -> Unit,
    onAsk: () -> Unit,
    onCommit: () -> Unit,
    onReload: () -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    val files = review.ticked.size
    Column(Modifier.fillMaxSize()) {
        Column(
            Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(stringResource(R.string.agents_git_message), style = MaterialTheme.typography.labelLarge, color = scheme.onSurfaceVariant)
            AgentTextField(
                value = write.text,
                onValueChange = onWrite,
                hint = stringResource(R.string.agents_git_message),
                enabled = !write.asking && !committing,
                minLines = 4,
                maxLines = 12,
            )
            if (AgentGit.subjectLong(write.text)) {
                Text(stringResource(R.string.agents_git_subject_long), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (write.text.isBlank() && title.isNotBlank() && !write.asking) {
                    G7RaisedButton(label = stringResource(R.string.agents_git_use_title), onClick = { onUse(title) })
                }
                val offer = write.offer
                when {
                    write.asking -> G7RaisedButton(label = stringResource(R.string.agents_git_asking, agentName), onClick = {}, enabled = false)
                    offer != null -> G7RaisedButton(label = stringResource(R.string.agents_git_use_message, agentName), onClick = { onUse(offer) })
                    else -> G7RaisedButton(label = stringResource(R.string.agents_git_ask, agentName), onClick = onAsk, enabled = !committing)
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Icon(AgentGlyphs.commit, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
                Text(pluralStringResource(R.plurals.agents_git_files_to, files, files, review.branch.orEmpty()), style = MaterialTheme.typography.bodyMedium)
            }
            if (refused != null) {
                val why = AgentJobs.why(JobKind.COMMIT, refused.reason)
                Text(whyText(why, pcName, agentName), style = MaterialTheme.typography.bodyMedium, color = scheme.error)
                if (why == JobWhy.IDENTITY) IDENTITY_COMMANDS.forEach { CommandLine(it) }
                if (why == JobWhy.FILES_CHANGED) G7RaisedButton(label = stringResource(R.string.agents_git_reload), onClick = onReload)
            }
        }
        Box(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp)) {
            if (committing) {
                Box(Modifier.fillMaxWidth().heightIn(min = 48.dp), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp) }
            } else {
                G7PrimaryButton(label = stringResource(R.string.agents_git_commit), onClick = onCommit, enabled = AgentReview.canCommit(review, write.text) && !write.asking)
            }
        }
    }
}

/** "Commit 4 files on Kilele?" with the message's first line and where it goes; Cancel first. */
@Composable
private fun CommitConfirm(files: Int, branch: String, pcName: String, subject: String, onCommit: () -> Unit, onDismiss: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    AgentSheet(onDismiss) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(pluralStringResource(R.plurals.agents_git_commit_on, files, files, pcName), style = MaterialTheme.typography.titleLarge)
            Text(subject, style = MaterialTheme.typography.bodyLarge.copy(fontWeight = FontWeight.Medium), maxLines = 2, overflow = TextOverflow.Ellipsis)
            Text(pluralStringResource(R.plurals.agents_git_files_to, files, files, branch), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
            Row(Modifier.fillMaxWidth().padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                G7RaisedButton(label = stringResource(R.string.action_cancel), onClick = onDismiss, modifier = Modifier.weight(1f))
                G7PrimaryButton(label = stringResource(R.string.agents_git_commit), onClick = onCommit, modifier = Modifier.weight(1f))
            }
        }
    }
}

/** The commit running: its hooks' lines, Stop, then how it ended and what's next (Done, Push). */
@Composable
private fun CommitJob(
    job: GitJob?,
    pcName: String,
    agentName: String,
    push: PushRow.Ready?,
    onStop: () -> Unit,
    onReload: () -> Unit,
    onPush: (PushRow.Ready) -> Unit,
    onClose: () -> Unit,
) {
    val end = job?.let(AgentJobs::end)
    Column(Modifier.fillMaxSize()) {
        Column(Modifier.weight(1f).fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            if (end == null && job?.lines.isNullOrEmpty()) {
                Text(stringResource(R.string.agents_git_hooks_running), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            JobLines(job)
            JobStatusLine(job, stringResource(R.string.agents_git_hooks_running), pcName, agentName, onStop)
            if (end is JobEnd.Failed && end.why == JobWhy.IDENTITY) IDENTITY_COMMANDS.forEach { CommandLine(it) }
        }
        if (end != null) {
            Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                when {
                    end is JobEnd.Failed && end.why == JobWhy.FILES_CHANGED -> {
                        G7RaisedButton(label = stringResource(R.string.agents_git_back_to_thread), onClick = onClose, modifier = Modifier.fillMaxWidth())
                        G7PrimaryButton(label = stringResource(R.string.agents_git_reload), onClick = onReload)
                    }
                    end is JobEnd.Failed -> G7PrimaryButton(label = stringResource(R.string.agents_git_back_to_thread), onClick = onClose)
                    push != null -> {
                        G7RaisedButton(label = stringResource(R.string.agents_settings_done), onClick = onClose, modifier = Modifier.fillMaxWidth())
                        G7PrimaryButton(label = stringResource(R.string.agents_git_push), onClick = { onPush(push) })
                    }
                    else -> G7PrimaryButton(label = stringResource(R.string.agents_settings_done), onClick = onClose)
                }
            }
        }
    }
}
