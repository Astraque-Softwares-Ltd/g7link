// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.foundation.background
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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.mono
import app.g7link.core.AgentDiffDoc
import app.g7link.core.AgentDiffKind
import app.g7link.core.AgentDiffNote
import app.g7link.core.AgentDiffRow
import app.g7link.core.agentDiff
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** One row of a diff drawn in a lazy list: a file's name, a hunk's head, or a line. */
@Immutable
sealed interface DiffItem {
    data class File(val path: String) : DiffItem

    data class Hunk(val text: String) : DiffItem

    data class Line(val row: AgentDiffRow, val numbers: Boolean) : DiffItem

    data object Binary : DiffItem
}

/** Lines picked for a quote: from where the long-press began to where the last tap was. */
@Immutable
data class LineSelection(val anchor: Int, val end: Int) {
    val range: IntRange get() = minOf(anchor, end)..maxOf(anchor, end)
}

/** A diff as a list to draw, and the lines picked in it for a quote (contract §SC9.1 "Comment on a line"). */
object AgentDiffLines {
    fun items(doc: AgentDiffDoc): List<DiffItem> = buildList {
        for (file in doc.files) {
            if (doc.files.size > 1 && file.path.isNotEmpty()) add(DiffItem.File(file.path))
            if (file.note == AgentDiffNote.BINARY && file.rows.isEmpty()) {
                add(DiffItem.Binary)
                continue
            }
            val numbers = file.rows.any { it.oldNo > 0u || it.newNo > 0u }
            for (row in file.rows) add(if (row.kind == AgentDiffKind.HUNK) DiffItem.Hunk(row.text) else DiffItem.Line(row, numbers))
        }
    }

    /** A long-press starts a selection at a line; null on anything else. */
    fun start(items: List<DiffItem>, index: Int): LineSelection? = if (items.getOrNull(index) is DiffItem.Line) LineSelection(index, index) else null

    /** A tap with a selection made: the range runs from where it began to this line; a tap on the one line picked clears it. */
    fun tap(items: List<DiffItem>, selection: LineSelection?, index: Int): LineSelection? {
        if (selection == null || items.getOrNull(index) !is DiffItem.Line) return selection
        if (selection.anchor == selection.end && index == selection.anchor) return null
        return selection.copy(end = index)
    }

    /** The lines picked, with their numbers in the file: the new side's, else the old side's. */
    fun quoted(items: List<DiffItem>, selection: LineSelection?): List<QuotedLine> {
        if (selection == null) return emptyList()
        return selection.range.mapNotNull { i ->
            val row = (items.getOrNull(i) as? DiffItem.Line)?.row ?: return@mapNotNull null
            val number = if (row.newNo > 0u) row.newNo else row.oldNo
            QuotedLine(number.toLong().coerceAtMost(Int.MAX_VALUE.toLong()).toInt(), row.text)
        }
    }

    /** "%n lines" picked. */
    fun count(items: List<DiffItem>, selection: LineSelection?): Int = quoted(items, selection).size
}

/**
 * A diff in a lazy list, wrapped, with the look of the timeline's diffs:
 * long-press a line to pick it and tap another to pick the lines between
 * ([onSelect]; off when null). [header] and [footer] go above and below.
 */
@Composable
internal fun DiffLinesList(
    items: List<DiffItem>,
    state: LazyListState,
    selection: LineSelection?,
    onSelect: ((LineSelection?) -> Unit)?,
    modifier: Modifier = Modifier,
    header: (LazyListScope.() -> Unit)? = null,
    footer: (LazyListScope.() -> Unit)? = null,
) {
    val scheme = MaterialTheme.colorScheme
    val p = agentPalette()
    val fonts = LocalAppFonts.current
    val style = MaterialTheme.typography.bodySmall.mono(fonts).copy(fontSize = 12.5.sp, lineHeight = 20.sp)
    val addBg = p.diffAddRow.color.copy(alpha = p.diffAddRow.alpha)
    val delBg = p.diffDelRow.color.copy(alpha = p.diffDelRow.alpha)
    val addMark = p.diffAddWord.color.copy(alpha = p.diffAddWord.alpha)
    val delMark = p.diffDelWord.color.copy(alpha = p.diffDelWord.alpha)
    val picked = selection?.range
    val bar = scheme.primary
    LazyColumn(modifier.fillMaxWidth(), state = state) {
        header?.invoke(this)
        items(items.size, contentType = { items[it]::class.simpleName }) { index ->
            when (val item = items[index]) {
                is DiffItem.File -> Text(
                    item.path,
                    style = style,
                    color = scheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.StartEllipsis,
                    modifier = Modifier.padding(horizontal = 10.dp, vertical = 4.dp),
                )
                is DiffItem.Hunk -> Text(
                    item.text,
                    style = style.copy(fontSize = 11.5.sp),
                    color = p.diffHunk,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.fillMaxWidth().background(p.diffHunkGround).padding(horizontal = 10.dp, vertical = 4.dp),
                )
                DiffItem.Binary -> Text(
                    stringResource(R.string.agents_diff_binary),
                    style = MaterialTheme.typography.bodySmall,
                    color = scheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 10.dp, vertical = 6.dp),
                )
                is DiffItem.Line -> {
                    val row = item.row
                    val (bg, mark, sign, signColor) = when (row.kind) {
                        AgentDiffKind.ADD -> DiffRowLook(addBg, addMark, "+", p.diffAddSign)
                        AgentDiffKind.DEL -> DiffRowLook(delBg, delMark, "−", p.diffDelSign)
                        AgentDiffKind.CONTEXT, AgentDiffKind.HUNK -> DiffRowLook(Color.Transparent, Color.Transparent, "", p.diffLineNo)
                    }
                    val struck = row.kind == AgentDiffKind.DEL
                    val text = remember(row, mark, struck) {
                        buildAnnotatedString {
                            append(row.text)
                            for (m in row.marks) {
                                val start = m.start.toInt().coerceIn(0, row.text.length)
                                val end = (m.start.toLong() + m.len.toLong()).toInt().coerceIn(start, row.text.length)
                                if (end > start) addStyle(SpanStyle(background = mark, textDecoration = if (struck) TextDecoration.LineThrough else null), start, end)
                            }
                        }
                    }
                    val on = picked != null && index in picked
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .background(bg)
                            .then(if (on) Modifier.drawBehind { drawRect(bar, topLeft = Offset.Zero, size = size.copy(width = 4.dp.toPx())) } else Modifier)
                            .then(
                                if (onSelect == null) {
                                    Modifier
                                } else {
                                    Modifier.combinedClickable(
                                        role = Role.Button,
                                        onLongClick = { onSelect(AgentDiffLines.start(items, index)) },
                                        onClick = { if (selection != null) onSelect(AgentDiffLines.tap(items, selection, index)) },
                                    )
                                },
                            ),
                    ) {
                        if (item.numbers) {
                            NumberCell(row.oldNo.toInt(), style, p.diffLineNo)
                            NumberCell(row.newNo.toInt(), style, p.diffLineNo)
                        }
                        Text(sign, style = style, color = signColor, textAlign = TextAlign.Center, modifier = Modifier.width(18.dp))
                        Text(text, style = style, color = p.diffText, modifier = Modifier.padding(end = 10.dp))
                    }
                }
            }
        }
        footer?.invoke(this)
    }
}

private data class DiffRowLook(val background: Color, val mark: Color, val sign: String, val signColor: Color)

@Composable
private fun NumberCell(number: Int, style: androidx.compose.ui.text.TextStyle, color: Color) {
    Text(if (number > 0) number.toString() else "", style = style, color = color, textAlign = TextAlign.End, modifier = Modifier.width(36.dp).padding(end = 6.dp))
}

/** "2 lines · Comment": the bar under a diff while lines are picked. */
@Composable
internal fun CommentBar(count: Int, onComment: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Column {
        Box(Modifier.fillMaxWidth().heightIn(min = 1.dp, max = 1.dp).background(scheme.outlineVariant))
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(pluralStringResource(R.plurals.agents_git_lines, count, count), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant, modifier = Modifier.weight(1f))
            Row(verticalAlignment = Alignment.CenterVertically) {
                G7PrimaryButton(label = stringResource(R.string.agents_git_comment), onClick = onComment, modifier = Modifier.width(140.dp))
            }
        }
    }
}

/** A file's uncommitted change as the screen shows it once read. */
@Immutable
sealed interface GitFileDiff {
    data object Loading : GitFileDiff

    data class Shown(val items: List<DiffItem>, val cut: Boolean) : GitFileDiff

    /** An untracked folder: the first names inside, and how many more. */
    data class Folder(val names: List<String>, val more: Int) : GitFileDiff

    data object Binary : GitFileDiff

    data class Failed(val failure: AgentFailure) : GitFileDiff
}

/** A patch as text, parsed off the main thread, as [GitFileDiff]. */
internal suspend fun gitFileDiff(patch: GitPatch): GitFileDiff {
    if (patch.binary) return GitFileDiff.Binary
    if (patch.names.isNotEmpty() || patch.moreNames > 0) return GitFileDiff.Folder(patch.names, patch.moreNames)
    val doc = withContext(Dispatchers.Default) { runCatching { agentDiff(patch.patch) }.getOrNull() } ?: return GitFileDiff.Failed(AgentFailure.OTHER)
    if (doc.files.any { it.note == AgentDiffNote.BINARY && it.rows.isEmpty() }) return GitFileDiff.Binary
    val cut = patch.truncated || doc.more || doc.files.any { it.note == AgentDiffNote.TRUNCATED }
    return GitFileDiff.Shown(AgentDiffLines.items(doc), cut)
}

/** The uncommitted changes' files (the diff view's third scope): the status list, each opening its diff. */
@Composable
internal fun UncommittedFiles(status: AgentCall<GitStatus>?, pcName: String, onOpen: (GitFile) -> Unit, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    when (status) {
        null -> Box(modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp) }
        is AgentCall.Failed -> Box(modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
            Text(stringResource(R.string.agents_failed_generic), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
        }
        is AgentCall.Ok -> {
            val git = status.value
            val line = AgentGit.stateLine(git)
            LazyColumn(modifier.fillMaxWidth()) {
                if (line != null) {
                    item(key = "state", contentType = "label") { Box(Modifier.padding(horizontal = 16.dp)) { GitNotice(stateText(line, pcName)) } }
                }
                if (git.repo == RepoKind.OK && git.changed == 0) {
                    item(key = "nothing", contentType = "label") { Box(Modifier.padding(horizontal = 16.dp)) { GitNotice(stringResource(R.string.agents_git_nothing)) } }
                }
                items(git.files, key = { "git:${it.path}" }, contentType = { "file" }) { file -> GitFileRow(file, onOpen = { onOpen(file) }) }
                if (git.more) {
                    item(key = "more", contentType = "label") {
                        val more = (git.total - git.files.size).coerceAtLeast(0)
                        Text(
                            pluralStringResource(R.plurals.agents_git_more_files, more, more),
                            style = MaterialTheme.typography.bodySmall,
                            color = scheme.onSurfaceVariant,
                            modifier = Modifier.padding(horizontal = 14.dp, vertical = 8.dp),
                        )
                    }
                }
            }
        }
    }
}

/** A changed file: its letter in its colour (the label is the word), the path cut from the start, its counts. */
@Composable
internal fun GitFileRow(file: GitFile, onOpen: (() -> Unit)?, modifier: Modifier = Modifier, leading: @Composable () -> Unit = {}, trailing: @Composable () -> Unit = {}) {
    val scheme = MaterialTheme.colorScheme
    val fonts = LocalAppFonts.current
    val word = changeWord(file)
    Row(
        modifier = modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .then(if (onOpen != null) Modifier.combinedClickable(role = Role.Button, onClick = onOpen) else Modifier)
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        leading()
        Text(
            changeLetter(file.change),
            style = MaterialTheme.typography.labelLarge.mono(fonts),
            color = changeColor(file.change),
            modifier = Modifier.semanticsLabel(word),
        )
        Text(
            file.path,
            style = MaterialTheme.typography.bodySmall.mono(fonts),
            color = scheme.onSurface,
            maxLines = 1,
            overflow = TextOverflow.StartEllipsis,
            modifier = Modifier.weight(1f),
        )
        val adds = file.adds
        val dels = file.dels
        if (!file.binary && adds != null && dels != null) Counts(adds.toLong(), dels.toLong())
        trailing()
        if (onOpen != null) Icon(G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
    }
}

/** The status letter the diff view already uses (not translated); a conflict is C. */
internal fun changeLetter(change: FileChange): String = when (change) {
    FileChange.ADDED, FileChange.UNTRACKED -> "A"
    FileChange.MODIFIED -> "M"
    FileChange.DELETED -> "D"
    FileChange.RENAMED -> "R"
    FileChange.TYPE -> "T"
    FileChange.CONFLICT -> "C"
}

@Composable
internal fun changeColor(change: FileChange): Color {
    val p = agentPalette()
    return when (change) {
        FileChange.ADDED, FileChange.UNTRACKED -> p.syntaxInserted
        FileChange.DELETED, FileChange.CONFLICT -> p.syntaxDeleted
        FileChange.RENAMED -> p.syntaxFunction
        FileChange.MODIFIED, FileChange.TYPE -> p.syntaxType
    }
}

/** The letter's word, as its accessible label (§SC9.4 "File states"). */
@Composable
internal fun changeWord(file: GitFile): String = stringResource(
    when (file.change) {
        FileChange.ADDED -> R.string.agents_git_file_added
        FileChange.MODIFIED -> R.string.agents_git_file_changed
        FileChange.DELETED -> R.string.agents_git_file_deleted
        FileChange.RENAMED -> R.string.agents_git_file_renamed
        FileChange.TYPE -> R.string.agents_git_file_type
        FileChange.CONFLICT -> R.string.agents_git_file_conflict
        FileChange.UNTRACKED -> if (file.folder) R.string.agents_git_file_new_folder else R.string.agents_git_file_new
    },
)

private fun Modifier.semanticsLabel(label: String): Modifier = semantics { contentDescription = label }

/**
 * One uncommitted file full-screen, read when opened: its diff, wrapped,
 * where a long-press picks lines and Comment quotes them into the reply box.
 */
@Composable
internal fun UncommittedFile(
    core: Core,
    pcId: String,
    threadId: String,
    file: GitFile,
    onComment: ((Quote) -> Unit)?,
    onBack: () -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    var shown by remember(file.path) { mutableStateOf<GitFileDiff>(GitFileDiff.Loading) }
    LaunchedEffect(pcId, threadId, file.path) {
        shown = GitFileDiff.Loading
        shown = when (val patch = core.agentGitPatch(pcId, threadId, file.path, review = null)) {
            is AgentCall.Ok -> gitFileDiff(patch.value)
            is AgentCall.Failed -> GitFileDiff.Failed(patch.failure)
        }
    }
    var selection by remember(file.path) { mutableStateOf<LineSelection?>(null) }
    val state = rememberLazyListState()
    Column(Modifier.fillMaxSize().background(scheme.surface)) {
        GitHeader(
            title = stringResource(R.string.agents_diff_title),
            sub = listOf(stringResource(R.string.agents_git_uncommitted), file.path).joinToString(" · "),
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
                        { item(key = "cut", contentType = "label") { GitNoticeRow(stringResource(R.string.agents_diff_cut)) } }
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

/** An untracked folder's first names, and "and %n more". */
@Composable
internal fun FolderNames(folder: GitFileDiff.Folder, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    val fonts = LocalAppFonts.current
    LazyColumn(modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp)) {
        items(folder.names.size) { i ->
            Text(folder.names[i], style = MaterialTheme.typography.bodySmall.mono(fonts), modifier = Modifier.padding(vertical = 3.dp))
        }
        if (folder.more > 0) {
            item {
                Text(
                    pluralStringResource(R.plurals.agents_diff_more, folder.more, folder.more),
                    style = MaterialTheme.typography.bodySmall,
                    color = scheme.onSurfaceVariant,
                    modifier = Modifier.padding(vertical = 6.dp),
                )
            }
        }
    }
}

@Composable
internal fun GitNoticeRow(text: String) {
    Box(Modifier.fillMaxWidth().padding(horizontal = 16.dp)) { GitNotice(text) }
}
