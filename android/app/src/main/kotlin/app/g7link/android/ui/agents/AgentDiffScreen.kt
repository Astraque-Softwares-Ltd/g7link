// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.activity.compose.BackHandler
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
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.mono
import app.g7link.core.AgentDiffDoc
import app.g7link.core.AgentDiffNote
import app.g7link.core.AgentFileStatus
import app.g7link.core.AgentPatch
import app.g7link.core.AgentTurnFile
import app.g7link.core.AgentTurnFiles
import app.g7link.core.AgentTurnRange
import app.g7link.core.agentDiff
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * Which turns the diff covers (contract §U9.1 "Changes"), what isn't committed
 * yet (phase 4 §SC9.1), or a pull request's files (phase 6 §X6.6, opened from its view).
 */
enum class DiffScope { THIS_TURN, WHOLE_THREAD, UNCOMMITTED, PULL_REQUEST }

/** What a file of the diff shows once read. */
@Immutable
sealed interface FileDiff {
    data object Loading : FileDiff

    data class Shown(val doc: AgentDiffDoc, val cut: Boolean) : FileDiff

    data object Binary : FileDiff

    data class Failed(val failure: AgentFailure) : FileDiff
}

object AgentDiffModel {
    /** The turns [scope] covers, from a thread's newest turn [newest]. */
    fun range(scope: DiffScope, turn: UInt, newest: UInt): AgentTurnRange = when (scope) {
        DiffScope.THIS_TURN, DiffScope.UNCOMMITTED, DiffScope.PULL_REQUEST -> AgentTurnRange.Turn(turn)
        DiffScope.WHOLE_THREAD -> AgentTurnRange.Span(1u, maxOf(newest, turn, 1u))
    }

    /** The files with a diff, then those too large to record (no diff). */
    fun split(files: List<AgentTurnFile>): Pair<List<AgentTurnFile>, List<AgentTurnFile>> =
        files.partition { it.status != AgentFileStatus.UNRECORDED }

    /** "and %n more": files past the 500 listed, known for one turn only (the whole thread's may repeat a file). */
    fun more(scope: DiffScope, files: AgentTurnFiles, turnFiles: Int?): Int =
        if (!files.more || scope != DiffScope.THIS_TURN || turnFiles == null) 0 else (turnFiles - files.files.size).coerceAtLeast(0)

    /** A file's letter, as git writes it (not translated). */
    fun letter(status: AgentFileStatus?): String = when (status) {
        AgentFileStatus.ADDED -> "A"
        AgentFileStatus.DELETED -> "D"
        AgentFileStatus.RENAMED -> "R"
        AgentFileStatus.TYPE_CHANGED -> "T"
        AgentFileStatus.MODIFIED, AgentFileStatus.UNRECORDED, null -> "M"
    }

    /** A patch as the screen shows it: binary, or rows; cut short when either the PC or the parser says so. */
    fun shown(file: AgentTurnFile?, patch: AgentPatch, doc: AgentDiffDoc?): FileDiff {
        if (patch.binary || file?.binary == true) return FileDiff.Binary
        val parsed = doc ?: return FileDiff.Failed(AgentFailure.OTHER)
        if (parsed.files.any { it.note == AgentDiffNote.BINARY }) return FileDiff.Binary
        val cut = patch.truncated || parsed.more || parsed.files.any { it.note == AgentDiffNote.TRUNCATED }
        return FileDiff.Shown(parsed, cut)
    }
}

/**
 * A turn's changes full-screen (board 19): "This turn" or "Whole thread",
 * the file list with each file's status and counts, then one file at a time,
 * wrapped by default with a switch to scroll sideways. Binary, cut short at
 * 1 MB, more than 500 files, and not recorded each say so.
 */
@Composable
internal fun ChangesFullScreen(
    core: Core,
    pcId: String,
    threadId: String,
    title: String,
    turn: UInt,
    newestTurn: UInt,
    /** How many files the turn changed in all, when known ("and %n more" past the 500 listed). */
    turnFiles: Int?,
    startPath: String?,
    onClose: () -> Unit,
    pcName: String = "",
    /** The thread's folder is a repository: "Uncommitted" is offered too. */
    uncommitted: Boolean = false,
    /** Lines picked in an uncommitted diff become a quote chip in the reply box; null: not offered. */
    onComment: ((Quote) -> Unit)? = null,
) {
    var scope by rememberSaveable(threadId, turn) { mutableStateOf(DiffScope.THIS_TURN) }
    var path by rememberSaveable(threadId, turn) { mutableStateOf(startPath) }
    var attempt by remember { mutableIntStateOf(0) }
    val range = AgentDiffModel.range(scope, turn, newestTurn)
    var files by remember(threadId, turn) { mutableStateOf<AgentCall<AgentTurnFiles>?>(null) }
    LaunchedEffect(pcId, threadId, range, attempt, scope) {
        files = null
        if (scope != DiffScope.UNCOMMITTED) files = core.agentDiffFiles(pcId, threadId, range)
    }
    // What isn't committed: the folder's status, its files each opening their diff.
    var git by remember(threadId) { mutableStateOf<AgentCall<GitStatus>?>(null) }
    var gitFile by remember(threadId) { mutableStateOf<GitFile?>(null) }
    LaunchedEffect(pcId, threadId, scope) {
        if (scope == DiffScope.UNCOMMITTED) git = core.agentGitStatus(pcId, threadId)
    }
    BackHandler {
        when {
            gitFile != null -> gitFile = null
            path != null -> path = null
            else -> onClose()
        }
    }
    val listed = (files as? AgentCall.Ok<AgentTurnFiles>)?.value
    val open = path
    val shownFile = gitFile
    if (scope == DiffScope.UNCOMMITTED && shownFile != null) {
        UncommittedFile(core, pcId, threadId, shownFile, onComment = onComment, onBack = { gitFile = null })
        return
    }
    if (open != null && scope != DiffScope.UNCOMMITTED) {
        FileView(core, pcId, threadId, range, listed?.files?.firstOrNull { it.path == open }, open, onBack = { path = null })
        return
    }
    val scheme = MaterialTheme.colorScheme
    Column(Modifier.fillMaxSize().background(scheme.surface)) {
        DiffHeader(title = stringResource(R.string.agents_diff_title), sub = title, onBack = onClose)
        ScopeSwitch(scope, uncommitted = uncommitted, onScope = { scope = it })
        if (scope == DiffScope.UNCOMMITTED) {
            UncommittedFiles(git, pcName, onOpen = { gitFile = it }, modifier = Modifier.weight(1f))
            return@Column
        }
        when (val result = files) {
            null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp) }
            is AgentCall.Failed -> Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
                // Unsupported: a turn in range has no restore point ready.
                Text(
                    stringResource(if (result.failure == AgentFailure.UNSUPPORTED) R.string.agents_changes_missing else R.string.agents_failed_generic),
                    style = MaterialTheme.typography.bodyMedium,
                    color = scheme.onSurfaceVariant,
                )
            }
            is AgentCall.Ok -> {
                val (withDiff, unrecorded) = remember(result.value) { AgentDiffModel.split(result.value.files) }
                val total = withDiff.size
                LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = PaddingValues(start = 12.dp, end = 12.dp, bottom = 16.dp)) {
                    item(key = "count", contentType = "label") {
                        Row(Modifier.padding(horizontal = 14.dp, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
                            Text(pluralStringResource(R.plurals.agents_files_changed, total, total), style = MaterialTheme.typography.labelLarge, color = scheme.onSurfaceVariant)
                            Counts(result.value.adds.toLong(), result.value.dels.toLong())
                        }
                    }
                    items(withDiff, key = { "file:${it.path}" }, contentType = { "file" }) { file -> FileRow(file, onOpen = { path = file.path }) }
                    val more = AgentDiffModel.more(scope, result.value, turnFiles)
                    if (more > 0) {
                        item(key = "more", contentType = "label") {
                            Text(
                                pluralStringResource(R.plurals.agents_diff_more, more, more),
                                style = MaterialTheme.typography.bodySmall,
                                color = scheme.onSurfaceVariant,
                                modifier = Modifier.padding(horizontal = 14.dp, vertical = 8.dp),
                            )
                        }
                    }
                    if (unrecorded.isNotEmpty()) {
                        item(key = "unrecorded", contentType = "label") {
                            Text(
                                pluralStringResource(R.plurals.agents_changes_unrecorded, unrecorded.size, unrecorded.size),
                                style = MaterialTheme.typography.bodySmall,
                                color = scheme.onSurfaceVariant,
                                modifier = Modifier.padding(horizontal = 14.dp, vertical = 8.dp),
                            )
                        }
                        items(unrecorded, key = { "unrecorded:${it.path}" }, contentType = { "file" }) { file -> FileRow(file, onOpen = null) }
                    }
                }
            }
        }
    }
}

@Composable
private fun DiffHeader(title: String, sub: String?, onBack: () -> Unit, mono: Boolean = false, trailing: @Composable () -> Unit = {}) {
    val scheme = MaterialTheme.colorScheme
    Column {
        Row(
            modifier = Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(start = 4.dp, end = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            G7IconButton(icon = G7Icons.back, contentDescription = stringResource(R.string.action_back), onClick = onBack)
            Column(Modifier.weight(1f).padding(start = 4.dp), verticalArrangement = Arrangement.spacedBy(1.dp)) {
                Text(
                    title,
                    style = if (mono) MaterialTheme.typography.titleSmall.mono(LocalAppFonts.current) else MaterialTheme.typography.titleMedium.copy(fontWeight = FontWeight.SemiBold),
                    maxLines = 1,
                    overflow = if (mono) TextOverflow.StartEllipsis else TextOverflow.Ellipsis,
                )
                if (!sub.isNullOrBlank()) Text(sub, style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            trailing()
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(scheme.outlineVariant))
    }
}

/** A scope's name, as its switch and the file's header say it. */
@Composable
internal fun diffScopeName(scope: DiffScope): String = stringResource(
    when (scope) {
        DiffScope.THIS_TURN -> R.string.agents_diff_this_turn
        DiffScope.WHOLE_THREAD -> R.string.agents_diff_whole
        DiffScope.UNCOMMITTED -> R.string.agents_git_uncommitted
        DiffScope.PULL_REQUEST -> R.string.agents_pr_detail_scope
    },
)

/** "This turn" · "Whole thread" · "Uncommitted" (when the folder is a repository). */
@Composable
private fun ScopeSwitch(scope: DiffScope, uncommitted: Boolean, onScope: (DiffScope) -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val scopes = buildList {
        add(DiffScope.THIS_TURN to R.string.agents_diff_this_turn)
        add(DiffScope.WHOLE_THREAD to R.string.agents_diff_whole)
        if (uncommitted) add(DiffScope.UNCOMMITTED to R.string.agents_git_uncommitted)
    }
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 10.dp).selectableGroup(),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        for ((value, label) in scopes) {
            val on = value == scope
            Box(
                modifier = Modifier
                    .weight(1f)
                    .heightIn(min = 36.dp)
                    .clip(RoundedCornerShape(8.dp))
                    .background(if (on) scheme.surfaceContainerHigh else scheme.surface)
                    .border(1.dp, if (on) scheme.surfaceContainerHigh else scheme.outlineVariant, RoundedCornerShape(8.dp))
                    .selectable(selected = on, role = Role.RadioButton, onClick = { onScope(value) }),
                contentAlignment = Alignment.Center,
            ) {
                Text(stringResource(label), style = MaterialTheme.typography.labelLarge, color = if (on) scheme.onSurface else scheme.onSurfaceVariant)
            }
        }
    }
}

/** A changed file: its status letter in its colour, the path (cut from the start), counts; a tap opens it. */
@Composable
private fun FileRow(file: AgentTurnFile, onOpen: (() -> Unit)?) {
    val scheme = MaterialTheme.colorScheme
    val p = agentPalette()
    val fonts = LocalAppFonts.current
    val color = when (file.status) {
        AgentFileStatus.ADDED -> p.syntaxInserted
        AgentFileStatus.DELETED -> p.syntaxDeleted
        AgentFileStatus.RENAMED -> p.syntaxFunction
        AgentFileStatus.UNRECORDED -> scheme.onSurfaceVariant
        AgentFileStatus.MODIFIED, AgentFileStatus.TYPE_CHANGED, null -> p.syntaxType
    }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(RoundedCornerShape(8.dp))
            .then(if (onOpen != null) Modifier.clickable(role = Role.Button, onClick = onOpen) else Modifier)
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text(AgentDiffModel.letter(file.status), style = MaterialTheme.typography.labelLarge.mono(fonts), color = color)
        Text(
            file.path,
            style = MaterialTheme.typography.bodySmall.mono(fonts),
            color = if (onOpen != null) scheme.onSurface else scheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.StartEllipsis,
            modifier = Modifier.weight(1f),
        )
        if (file.status != AgentFileStatus.UNRECORDED && !file.binary) Counts(file.adds.toLong(), file.dels.toLong())
        if (onOpen != null) Icon(G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
    }
}

/** One file of the diff, read when opened and parsed off the main thread. */
@Composable
private fun FileView(
    core: Core,
    pcId: String,
    threadId: String,
    range: AgentTurnRange,
    file: AgentTurnFile?,
    path: String,
    onBack: () -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    var wrap by rememberSaveable(path) { mutableStateOf(true) }
    var shown by remember(path, range) { mutableStateOf<FileDiff>(FileDiff.Loading) }
    LaunchedEffect(pcId, threadId, range, path) {
        shown = FileDiff.Loading
        shown = when (val patch = core.agentDiffPatch(pcId, threadId, range, path)) {
            is AgentCall.Ok -> {
                val doc = if (patch.value.binary) null else withContext(Dispatchers.Default) { runCatching { agentDiff(patch.value.patch) }.getOrNull() }
                AgentDiffModel.shown(file, patch.value, doc)
            }
            is AgentCall.Failed -> FileDiff.Failed(patch.failure)
        }
    }
    Column(Modifier.fillMaxSize().background(scheme.surface)) {
        DiffHeader(title = path, sub = null, onBack = onBack, mono = true) {
            if (file != null && !file.binary) Box(Modifier.padding(end = 8.dp)) { Counts(file.adds.toLong(), file.dels.toLong()) }
            val label = stringResource(R.string.agents_diff_wrap)
            Text(
                label,
                style = MaterialTheme.typography.labelLarge,
                maxLines = 1,
                modifier = Modifier
                    .clip(RoundedCornerShape(8.dp))
                    .background(if (wrap) scheme.surfaceContainerHigh else Color.Transparent)
                    .border(1.dp, if (wrap) scheme.primary else scheme.outlineVariant, RoundedCornerShape(8.dp))
                    .toggleable(value = wrap, role = Role.Switch, onValueChange = { wrap = it })
                    .heightIn(min = 32.dp)
                    .padding(horizontal = 12.dp, vertical = 7.dp),
            )
        }
        when (val view = shown) {
            FileDiff.Loading -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp) }
            FileDiff.Binary -> Column(Modifier.fillMaxSize(), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
                Icon(G7Icons.photo, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(22.dp))
                Text(stringResource(R.string.agents_diff_binary), style = MaterialTheme.typography.bodyLarge, color = scheme.onSurfaceVariant, modifier = Modifier.padding(top = 8.dp))
            }
            is FileDiff.Failed -> Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
                Text(
                    stringResource(if (view.failure == AgentFailure.UNSUPPORTED) R.string.agents_changes_missing else R.string.agents_failed_generic),
                    style = MaterialTheme.typography.bodyMedium,
                    color = scheme.onSurfaceVariant,
                )
            }
            is FileDiff.Shown -> {
                Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState())) {
                    DiffView(view.doc, wrap = wrap, modifier = Modifier.padding(vertical = 4.dp))
                }
                if (view.cut) {
                    Column {
                        Box(Modifier.fillMaxWidth().height(1.dp).background(scheme.outlineVariant))
                        Row(
                            modifier = Modifier.fillMaxWidth().heightIn(min = 40.dp).padding(horizontal = 16.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            Icon(G7Icons.info, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(15.dp))
                            Text(stringResource(R.string.agents_diff_cut), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
                        }
                    }
                }
            }
        }
    }
}

/**
 * One file of a pull request (the diff view's "Pull request" scope): its diff
 * from the PC, wrapped, where a long-press picks lines and Comment quotes them
 * into the reply box. Nothing goes to GitHub.
 */
@Composable
internal fun PullRequestFileScreen(
    core: Core,
    pcId: String,
    repo: String,
    number: Int,
    file: GitFile,
    onComment: ((Quote) -> Unit)?,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val scheme = MaterialTheme.colorScheme
    var shown by remember(file.path) { mutableStateOf<GitFileDiff>(GitFileDiff.Loading) }
    LaunchedEffect(pcId, repo, number, file.path) {
        shown = GitFileDiff.Loading
        shown = when (val patch = core.agentPrDiff(pcId, repo, number, file.path)) {
            is AgentCall.Ok -> gitFileDiff(patch.value)
            is AgentCall.Failed -> GitFileDiff.Failed(patch.failure)
        }
    }
    var selection by remember(file.path) { mutableStateOf<LineSelection?>(null) }
    val state = rememberLazyListState()
    Column(modifier.fillMaxSize().background(scheme.surface)) {
        GitHeader(
            title = stringResource(R.string.agents_diff_title),
            sub = listOf(diffScopeName(DiffScope.PULL_REQUEST), file.path).joinToString(" · "),
            onBack = onBack,
        )
        when (val view = shown) {
            GitFileDiff.Loading -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp) }
            GitFileDiff.Binary -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text(stringResource(R.string.agents_diff_binary), style = MaterialTheme.typography.bodyLarge, color = scheme.onSurfaceVariant)
            }
            is GitFileDiff.Failed -> Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
                Text(stringResource(R.string.agents_failed_generic), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
            }
            is GitFileDiff.Folder -> FolderNames(view, Modifier.weight(1f))
            is GitFileDiff.Shown -> {
                DiffLinesList(
                    items = view.items,
                    state = state,
                    selection = selection,
                    onSelect = if (onComment != null) { sel -> selection = sel } else null,
                    modifier = Modifier.weight(1f),
                    footer = if (view.cut) {
                        { item(key = "cut", contentType = "label") { GitNoticeRow(stringResource(R.string.agents_pr_detail_diff_cut)) } }
                    } else {
                        null
                    },
                )
                val picked = AgentDiffLines.count(view.items, selection)
                if (onComment != null && picked > 0) {
                    CommentBar(picked) {
                        val quote = AgentQuotes.of(file.path, AgentDiffLines.quoted(view.items, selection))
                        selection = null
                        if (quote != null) onComment(quote)
                    }
                }
            }
        }
    }
}
