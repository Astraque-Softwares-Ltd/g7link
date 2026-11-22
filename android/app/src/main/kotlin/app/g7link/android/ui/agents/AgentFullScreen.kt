// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import androidx.compose.foundation.ScrollState
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.net.toUri
import app.g7link.android.R
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.G7StatusDot
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.g7Tints
import app.g7link.android.ui.theme.mono
import app.g7link.core.AgentDiffDoc
import app.g7link.core.AgentItem
import app.g7link.core.AgentToolStatus
import app.g7link.core.agentDiff
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withContext

/** What fills the screen over a thread: an edit request, a step's output, or an edit. */
@Immutable
sealed interface FullView {
    /** The request with this token, to review whole before Allow. */
    data class Edit(val token: String) : FullView

    /** The output of the tool item with this id. */
    data class Output(val itemId: String) : FullView

    /** An edit read from a step or a changed file, with its heading. */
    data class Diff(val title: String, val doc: AgentDiffDoc) : FullView

    /** A turn's recorded changes (contract §U9.1 "Changes"), at [path] when a file was tapped. */
    data class Changes(val turn: UInt, val path: String?) : FullView

    /** Review and commit (phase 4 §SC9.3), from the folder as it was read. */
    data class Review(val status: GitStatus) : FullView

    /** The branch's pull request, or the form to open one. */
    data class Pr(val status: GitStatus) : FullView

    /** A push on the PC, as it runs and how it ended. */
    data class Push(val job: String, val branch: String, val remote: String) : FullView

    /** A run of one of the folder's scripts, its output as it comes (phase 6 §X5.2). */
    data class RunOutput(val run: String) : FullView
}

/** A link waiting for Open or Cancel: the target as the render crate let it through, and the text that showed it. */
@Immutable
data class PendingLink(val href: String, val text: String)

/** A full-screen view's header: Back or Close, a title, a line under it, and something at its end. */
@Composable
private fun FullHeader(
    title: String,
    onClose: () -> Unit,
    closeIcon: ImageVector,
    closeLabel: String,
    subtitle: @Composable () -> Unit,
    trailing: @Composable () -> Unit = {},
) {
    Column {
        Row(
            modifier = Modifier.fillMaxWidth().padding(start = 4.dp, end = 4.dp, top = 4.dp, bottom = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            G7IconButton(icon = closeIcon, contentDescription = closeLabel, onClick = onClose)
            Column(Modifier.weight(1f).padding(start = 4.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(title, style = MaterialTheme.typography.titleMedium.copy(fontWeight = FontWeight.SemiBold), maxLines = 2, overflow = TextOverflow.Ellipsis)
                subtitle()
            }
            trailing()
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(MaterialTheme.colorScheme.outlineVariant))
    }
}

/**
 * The agent's own words about a request, set apart from ours: "Claude says:" in
 * the text colour, the reason muted, a bar before it.
 */
@Composable
internal fun AgentSays(agentName: String, reason: String, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    val resources = LocalResources.current
    val text = remember(agentName, reason, resources, scheme.onSurface) {
        val full = resources.getString(R.string.agents_ask_says, agentName, reason)
        val at = full.lastIndexOf(reason).takeIf { it > 0 } ?: 0
        buildAnnotatedString {
            withStyle(SpanStyle(color = scheme.onSurface, fontWeight = FontWeight.Medium)) { append(full.substring(0, at)) }
            append(full.substring(at))
        }
    }
    Row(modifier.fillMaxWidth().height(IntrinsicSize.Min)) {
        Box(Modifier.width(2.dp).fillMaxHeight().background(scheme.outline))
        Text(text, style = MaterialTheme.typography.bodySmall.copy(lineHeight = 18.sp), color = scheme.onSurfaceVariant, modifier = Modifier.padding(start = 10.dp))
    }
}

/** An edit's text read as a diff off the main thread; null until read. */
@Composable
internal fun rememberDiff(text: String?): AgentDiffDoc? {
    val doc by produceState<AgentDiffDoc?>(null, text) {
        value = text?.takeIf { it.isNotBlank() }?.let { withContext(Dispatchers.Default) { runCatching { agentDiff(it) }.getOrNull() } }
    }
    return doc
}

/** True once [scroll] has been laid out and is at its end: everything has been on screen. */
@Composable
internal fun rememberSeenToEnd(key: Any, scroll: ScrollState, ready: Boolean): Boolean {
    var seen by remember(key) { mutableStateOf(false) }
    LaunchedEffect(key, ready) {
        if (!ready || seen) return@LaunchedEffect
        snapshotFlow { scroll.maxValue != Int.MAX_VALUE && scroll.value >= scroll.maxValue }.first { it }
        seen = true
    }
    return seen
}

/**
 * An edit request full-screen: the file's name, "Claude wants to edit a file · 2
 * of 2" and its counts; the folder and the file's path in it; the agent's reason;
 * the whole change, long lines wrapped; Deny, and Allow once the end of the
 * change has been on screen.
 */
@Composable
internal fun EditFullScreen(
    queue: AskQueue,
    agentName: String,
    folder: String,
    answering: Boolean,
    onClose: () -> Unit,
    onAnswer: (Boolean) -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    val ask = queue.shown
    val doc = rememberDiff(ask.diff)
    val scroll = rememberScrollState()
    val seen = rememberSeenToEnd(ask.token, scroll, ready = ask.diff.isNullOrBlank() || doc != null)
    val name = ask.path?.takeIf { it.isNotBlank() }?.let { AgentText.splitPath(it).second } ?: ask.title
    Column(Modifier.fillMaxSize().background(scheme.surface)) {
        FullHeader(
            title = name,
            onClose = onClose,
            closeIcon = G7Icons.close,
            closeLabel = stringResource(R.string.agents_close),
            subtitle = {
                val count = if (queue.total > 1) stringResource(R.string.agents_dock_count, queue.position, queue.total) else null
                Text(
                    listOfNotNull(stringResource(R.string.agents_dock_wants_edit, agentName), count).joinToString(" · "),
                    style = MaterialTheme.typography.labelMedium,
                    color = scheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            },
            trailing = { doc?.let { Box(Modifier.padding(end = 12.dp)) { Counts(it.adds.toLong(), it.dels.toLong()) } } },
        )
        Column(
            modifier = Modifier.weight(1f).fillMaxWidth().verticalScroll(scroll).padding(horizontal = 10.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Row(Modifier.padding(horizontal = 6.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Icon(G7Icons.folder, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.padding(top = 2.dp).size(12.dp))
                Text(
                    listOfNotNull(folder.ifBlank { null }, ask.path?.takeIf { it.isNotBlank() }).joinToString(" · "),
                    style = MaterialTheme.typography.labelSmall,
                    color = scheme.onSurfaceVariant,
                )
            }
            ask.reason?.takeIf { it.isNotBlank() }?.let { AgentSays(agentName, it, Modifier.padding(horizontal = 6.dp)) }
            when {
                doc != null -> Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(6.dp)).background(agentPalette().groundCode).padding(vertical = 4.dp)) {
                    DiffView(doc, wrap = true)
                    Row(
                        modifier = Modifier.padding(horizontal = 12.dp, vertical = 10.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Icon(G7Icons.check, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(14.dp))
                        Text(stringResource(R.string.agents_diff_end), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
                    }
                }
                !ask.diff.isNullOrBlank() -> Box(Modifier.fillMaxWidth().padding(24.dp), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
                }
            }
            if (ask.truncated) {
                Text(stringResource(R.string.agents_ask_truncated), style = MaterialTheme.typography.bodySmall, color = g7Tints().amber)
            }
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(scheme.outlineVariant))
        AnswerButtons(
            queue = queue,
            answering = answering,
            seenAll = seen,
            allowPrimary = true,
            onAnswer = onAnswer,
            modifier = Modifier.padding(start = 12.dp, end = 12.dp, top = 10.dp, bottom = 16.dp),
        )
    }
}

/**
 * Deny then Allow, and under them why Allow waits: the request came while you
 * were typing, or the end of it hasn't been on screen yet.
 */
@Composable
internal fun AnswerButtons(
    queue: AskQueue,
    answering: Boolean,
    seenAll: Boolean,
    allowPrimary: Boolean,
    onAnswer: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
) {
    val scheme = MaterialTheme.colorScheme
    Column(modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            G7RaisedButton(
                label = stringResource(R.string.agents_deny),
                onClick = { onAnswer(false) },
                enabled = AgentDock.canDeny(queue, answering),
                modifier = Modifier.weight(1f),
            )
            val canAllow = AgentDock.canAllow(queue, answering, seenAll)
            // With words in the reply box "Deny and send" is the primary, and Allow an outline.
            if (allowPrimary) {
                G7PrimaryButton(label = stringResource(R.string.agents_allow), onClick = { onAnswer(true) }, enabled = canAllow, modifier = Modifier.weight(1f))
            } else {
                G7RaisedButton(label = stringResource(R.string.agents_allow), onClick = { onAnswer(true) }, enabled = canAllow, modifier = Modifier.weight(1f))
            }
        }
        val note = when {
            queue.held -> stringResource(R.string.agents_held)
            !seenAll && !queue.shown.truncated -> stringResource(R.string.agents_scroll_to_allow)
            else -> null
        }
        if (note != null) {
            Text(
                note,
                style = MaterialTheme.typography.bodySmall,
                color = if (queue.held) g7Tints().amber else scheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth(),
            )
        }
        if (answering) {
            Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
            }
        }
    }
}

/** A step's output full-screen: how it ended and how long it ran, the command with its folder, then the output. */
@Composable
internal fun OutputFullScreen(item: AgentItem?, folder: String, onClose: () -> Unit, onCopy: (String) -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val palette = agentPalette()
    val fonts = LocalAppFonts.current
    val resources = LocalResources.current
    val tool = item?.tool
    val output = tool?.output.orEmpty()
    Column(Modifier.fillMaxSize().background(scheme.surface)) {
        FullHeader(
            title = stringResource(R.string.agents_output_title),
            onClose = onClose,
            closeIcon = G7Icons.back,
            closeLabel = stringResource(R.string.action_back),
            subtitle = {
                if (item != null && tool != null) {
                    val (dot, word, color) = when (tool.status) {
                        AgentToolStatus.RUNNING -> Triple(palette.toolStatusRunning, stringResource(R.string.agents_step_running), scheme.onSurfaceVariant)
                        AgentToolStatus.OK -> Triple(palette.toolStatusOk, stringResource(R.string.agents_state_done), scheme.onSurfaceVariant)
                        AgentToolStatus.ERROR -> Triple(palette.toolStatusFailed, stringResource(R.string.agents_state_failed), palette.statusFailedText)
                        AgentToolStatus.DENIED -> Triple(palette.toolStatusDenied, stringResource(R.string.agents_step_denied), scheme.onSurfaceVariant)
                    }
                    val end = item.endMs
                    val took = if (end != null && end > item.whenMs) durationText(resources, end - item.whenMs) else null
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        G7StatusDot(dot)
                        Text(word, style = MaterialTheme.typography.labelMedium.copy(fontWeight = FontWeight.SemiBold), color = color, maxLines = 1)
                        if (took != null) Text("· $took", style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant, maxLines = 1)
                    }
                }
            },
            trailing = {
                if (output.isNotEmpty()) {
                    G7IconButton(icon = AgentGlyphs.copy, contentDescription = stringResource(R.string.agents_copy), onClick = { onCopy(output) })
                }
            },
        )
        Column(
            modifier = Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 14.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            val command = tool?.detail?.takeIf { it.isNotBlank() }
            if (command != null) {
                Column(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(agentPalette().groundCode).padding(horizontal = 12.dp, vertical = 10.dp),
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    FolderLabel(folder)
                    Text(command, style = MaterialTheme.typography.bodySmall.mono(fonts).copy(fontSize = 13.sp, lineHeight = 19.sp))
                }
            }
            if (item?.truncated == true) {
                Text(stringResource(R.string.agents_output_last_lines), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
            }
            SelectionContainer {
                Text(output, style = MaterialTheme.typography.bodySmall.mono(fonts).copy(fontSize = 12.5.sp, lineHeight = 19.sp))
            }
        }
    }
}

/** An edit full-screen: a step's, or one of the turn's edits to a changed file. */
@Composable
internal fun DiffFullScreen(title: String, doc: AgentDiffDoc, onClose: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val name = doc.files.singleOrNull()?.path?.takeIf { it.isNotBlank() }?.let { AgentText.splitPath(it).second }
    Column(Modifier.fillMaxSize().background(scheme.surface)) {
        FullHeader(
            title = name ?: title.ifBlank { stringResource(R.string.agents_output_title) },
            onClose = onClose,
            closeIcon = G7Icons.back,
            closeLabel = stringResource(R.string.action_back),
            subtitle = {
                if (title.isNotBlank() && name != null) {
                    Text(title, style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            },
            trailing = { Box(Modifier.padding(end = 12.dp)) { Counts(doc.adds.toLong(), doc.dels.toLong()) } },
        )
        Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 10.dp, vertical = 12.dp)) {
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(6.dp)).background(agentPalette().groundCode).padding(vertical = 4.dp)) {
                DiffView(doc, wrap = true)
            }
        }
    }
}

/**
 * Where a link goes, before it opens (contract §P6.2): "Open this link?", the
 * target in full in the code face (an internationalised host in its ASCII
 * form), the text that showed it, Cancel and Open.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun LinkSheet(link: PendingLink, onOpen: () -> Unit, onDismiss: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val fonts = LocalAppFonts.current
    val state = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = state, containerColor = scheme.surfaceContainerHigh) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(horizontal = 20.dp).padding(bottom = 20.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(stringResource(R.string.agents_link_title), style = MaterialTheme.typography.titleMedium)
            Text(
                remember(link.href) { AgentText.shownTarget(link.href) },
                style = MaterialTheme.typography.bodySmall.mono(fonts).copy(fontSize = 12.5.sp, lineHeight = 19.sp),
                modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(6.dp)).background(scheme.surface).padding(horizontal = 10.dp, vertical = 8.dp),
            )
            if (link.text.isNotBlank() && link.text.trim() != link.href) {
                Text(stringResource(R.string.agents_link_shown_as, link.text.trim()), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
            }
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                G7RaisedButton(label = stringResource(R.string.action_cancel), onClick = onDismiss, modifier = Modifier.weight(1f))
                G7PrimaryButton(label = stringResource(R.string.agents_link_open), onClick = onOpen, modifier = Modifier.weight(1f))
            }
        }
    }
}

/** Opens a link the render crate let through (`http`, `https`, `mailto`); anything else is refused here too. */
internal fun openLink(context: Context, href: String): Boolean {
    if (!AgentText.openable(href)) return false
    val intent = Intent(Intent.ACTION_VIEW, href.toUri())
        .addCategory(Intent.CATEGORY_BROWSABLE)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    return try {
        context.startActivity(intent)
        true
    } catch (_: ActivityNotFoundException) {
        false
    }
}
