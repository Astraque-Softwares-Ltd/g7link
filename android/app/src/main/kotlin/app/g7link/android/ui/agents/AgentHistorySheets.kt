// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import android.text.format.DateFormat
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLocale
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.G7Switch
import app.g7link.android.ui.theme.g7Tints
import app.g7link.core.AgentHeld
import app.g7link.core.AgentHeldWhy
import app.g7link.core.AgentThread
import java.time.ZoneId
import java.util.Date
import kotlinx.coroutines.launch

/** A sheet's row: an icon, the label, a reason under it when it's off, and something at its end. */
@Composable
internal fun SheetRow(
    label: String,
    onClick: () -> Unit,
    enabled: Boolean = true,
    reason: String? = null,
    icon: ImageVector? = null,
    trailing: (@Composable () -> Unit)? = null,
) {
    val scheme = MaterialTheme.colorScheme
    val color = if (enabled) scheme.onSurface else scheme.onSurfaceVariant
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 52.dp)
            .clip(RoundedCornerShape(8.dp))
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        if (icon != null) Icon(icon, contentDescription = null, tint = color, modifier = Modifier.size(20.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(label, style = MaterialTheme.typography.bodyLarge, color = color)
            if (reason != null) Text(reason, style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
        }
        trailing?.invoke()
    }
}

/** A sheet's heading: a title and a line under it. */
@Composable
private fun SheetHead(title: String, sub: String? = null) {
    Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp).padding(bottom = 8.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(title, style = MaterialTheme.typography.titleMedium, maxLines = 2, overflow = TextOverflow.Ellipsis)
        if (!sub.isNullOrBlank()) {
            Text(sub, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AgentSheet(onDismiss: () -> Unit, content: @Composable () -> Unit) {
    val state = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = state, containerColor = MaterialTheme.colorScheme.surfaceContainerHigh) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 8.dp).padding(bottom = 20.dp)) { content() }
    }
}

/**
 * A thread's long-press sheet (board 16): Pin, Mark read or unread, Snooze,
 * Archive, Rename. Off with "Needs <PC>" while the PC can't be reached.
 */
@Composable
internal fun ThreadActionsSheet(
    thread: AgentThread,
    place: String,
    pcName: String,
    offline: Boolean,
    onAction: (ThreadAction) -> Unit,
    onDismiss: () -> Unit,
) {
    AgentSheet(onDismiss) {
        SheetHead(thread.title, place)
        for (action in AgentMarks.actions(thread)) {
            val enabled = !offline && AgentMarks.enabled(thread, action)
            val reason = when {
                offline -> stringResource(R.string.agents_needs_pc, pcName)
                !AgentMarks.enabled(thread, action) -> stringResource(R.string.agents_wait_until_finishes)
                else -> null
            }
            SheetRow(
                label = threadActionLabel(action),
                icon = when (action) {
                    ThreadAction.PIN, ThreadAction.UNPIN -> AgentGlyphs.pin
                    ThreadAction.MARK_READ, ThreadAction.MARK_UNREAD -> G7Icons.check
                    ThreadAction.SNOOZE, ThreadAction.UNSNOOZE -> G7Icons.clock
                    ThreadAction.ARCHIVE, ThreadAction.UNARCHIVE -> AgentGlyphs.tray
                    ThreadAction.RENAME -> G7Icons.edit
                },
                enabled = enabled,
                reason = reason,
                trailing = if (action == ThreadAction.SNOOZE) {
                    { Icon(G7Icons.chevronRight, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.size(14.dp)) }
                } else {
                    null
                },
                onClick = { onAction(action) },
            )
        }
    }
}

@Composable
internal fun threadActionLabel(action: ThreadAction): String = stringResource(
    when (action) {
        ThreadAction.PIN -> R.string.agents_mark_pin
        ThreadAction.UNPIN -> R.string.agents_mark_unpin
        ThreadAction.MARK_UNREAD -> R.string.agents_mark_unread
        ThreadAction.MARK_READ -> R.string.agents_mark_read
        ThreadAction.SNOOZE -> R.string.agents_mark_snooze
        ThreadAction.UNSNOOZE -> R.string.agents_unsnooze
        ThreadAction.ARCHIVE -> R.string.agents_mark_archive
        ThreadAction.UNARCHIVE -> R.string.agents_mark_unarchive
        ThreadAction.RENAME -> R.string.agents_mark_rename
    },
)

/** The snooze times (board 16): each with when it comes back. */
@Composable
internal fun SnoozeSheet(
    thread: AgentThread,
    agentName: String,
    limitResetsMs: Long?,
    nowMs: Long,
    onPick: (Long) -> Unit,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    val choices = remember(nowMs, limitResetsMs) { AgentMarks.snoozeChoices(nowMs, ZoneId.systemDefault(), limitResetsMs) }
    val time = DateFormat.getTimeFormat(context)
    AgentSheet(onDismiss) {
        SheetHead(stringResource(R.string.agents_mark_snooze), thread.title)
        for (choice in choices) {
            val at = time.format(Date(choice.atMs))
            val label = when (choice.kind) {
                SnoozeKind.HOUR -> stringResource(R.string.agents_snooze_hour)
                SnoozeKind.EVENING -> stringResource(R.string.agents_snooze_evening)
                SnoozeKind.TOMORROW -> stringResource(R.string.agents_snooze_tomorrow)
                SnoozeKind.NEXT_WEEK -> stringResource(R.string.agents_snooze_next_week)
                SnoozeKind.LIMIT -> stringResource(R.string.agents_snooze_limit, agentName, at)
            }
            val shown = when (choice.kind) {
                SnoozeKind.NEXT_WEEK -> DateFormat.format(DateFormat.getBestDateTimePattern(LocalLocale.current.platformLocale, "EEE jm"), choice.atMs).toString()
                SnoozeKind.LIMIT -> null
                SnoozeKind.HOUR, SnoozeKind.EVENING, SnoozeKind.TOMORROW -> at
            }
            SheetRow(
                label = label,
                onClick = { onPick(choice.atMs) },
                trailing = if (shown != null) {
                    { Text(shown, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                } else {
                    null
                },
            )
        }
    }
}

/** Renames a thread: one line, as the PC takes it. */
@Composable
internal fun RenameSheet(thread: AgentThread, onRename: (String) -> Unit, onDismiss: () -> Unit) {
    var text by rememberSaveable(thread.id) { mutableStateOf(thread.title) }
    val focus = remember { FocusRequester() }
    LaunchedEffect(focus) { runCatching { focus.requestFocus() } }
    val title = AgentMarks.title(text)
    AgentSheet(onDismiss) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(stringResource(R.string.agents_mark_rename), style = MaterialTheme.typography.titleLarge)
            AgentTextField(
                value = text,
                onValueChange = { text = it.replace('\n', ' ') },
                hint = thread.title,
                maxLines = 2,
                background = MaterialTheme.colorScheme.surfaceContainer,
                modifier = Modifier.focusRequester(focus),
            )
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                G7RaisedButton(label = stringResource(R.string.action_cancel), onClick = onDismiss, modifier = Modifier.weight(1f))
                G7PrimaryButton(
                    label = stringResource(R.string.agents_mark_rename),
                    enabled = title != null && title != thread.title,
                    onClick = { title?.let(onRename) },
                    modifier = Modifier.weight(1f),
                )
            }
        }
    }
}

/**
 * A message's long-press sheet (board 20): Copy, then Edit from here, Rewind
 * to here and Fork from here as the row allows; one that can't be done shows
 * off with why.
 */
@Composable
internal fun MessageActionsSheet(
    rows: List<MessageActionRow>,
    agentName: String,
    pcName: String,
    onAction: (MessageAction) -> Unit,
    onDismiss: () -> Unit,
) {
    AgentSheet(onDismiss) {
        for (row in rows) {
            SheetRow(
                label = stringResource(
                    when (row.action) {
                        MessageAction.COPY -> R.string.agents_copy
                        MessageAction.EDIT -> R.string.agents_edit_from_here
                        MessageAction.REWIND -> R.string.agents_rewind_to_here
                        MessageAction.FORK -> R.string.agents_fork_from_here
                    },
                ),
                icon = when (row.action) {
                    MessageAction.COPY -> AgentGlyphs.copy
                    MessageAction.EDIT -> G7Icons.edit
                    MessageAction.REWIND -> G7Icons.history
                    MessageAction.FORK -> G7Icons.plus
                },
                enabled = row.block == null,
                reason = row.block?.let { actBlockText(it, agentName, pcName) },
                onClick = { onAction(row.action) },
            )
        }
    }
}

@Composable
private fun actBlockText(block: ActBlock, agentName: String, pcName: String): String = when (block) {
    ActBlock.LatestOnly -> stringResource(R.string.agents_latest_only, agentName)
    ActBlock.NoRestorePoint -> stringResource(R.string.agents_no_restore_point)
    ActBlock.NotGit -> stringResource(R.string.agents_not_git)
    ActBlock.NeedsPc -> stringResource(R.string.agents_needs_pc, pcName)
}

/** Why a rewind can't be done, as the copy words it. */
@Composable
internal fun rewindReason(reason: AgentRewind.Reason, agentName: String): String = when (reason) {
    AgentRewind.Reason.SHARED -> stringResource(R.string.agents_shared_folder)
    AgentRewind.Reason.BUSY -> stringResource(R.string.agents_git_busy)
    AgentRewind.Reason.NO_RESTORE_POINT -> stringResource(R.string.agents_no_restore_point)
    AgentRewind.Reason.NOT_GIT -> stringResource(R.string.agents_not_git)
    AgentRewind.Reason.EXPIRED -> stringResource(R.string.agents_changes_expired)
    AgentRewind.Reason.OFF -> stringResource(R.string.agents_why_off)
    AgentRewind.Reason.LATEST_ONLY -> stringResource(R.string.agents_latest_only, agentName)
}

/**
 * Rewind to before a message, conversation only (board 21): the message, how
 * many later messages leave, and that files are changed on the PC, not here.
 * Cancel and Rewind; Rewind is off until the plan is in and nothing blocks it.
 */
@Composable
internal fun RewindSheetView(
    sheet: RewindSheet,
    message: String,
    agentName: String,
    pcName: String,
    nowMs: Long,
    onRewind: () -> Unit,
    onRetry: () -> Unit,
    onDismiss: () -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    AgentSheet(onDismiss) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(stringResource(R.string.agents_rewind_title), style = MaterialTheme.typography.titleLarge)
            if (message.isNotBlank()) {
                Text(
                    message,
                    style = MaterialTheme.typography.bodyMedium,
                    color = scheme.onSurfaceVariant,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(scheme.surface).padding(horizontal = 12.dp, vertical = 9.dp),
                )
            }
            when (sheet) {
                is RewindSheet.Loading -> Box(Modifier.fillMaxWidth().padding(8.dp), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp)
                }
                is RewindSheet.Ready, is RewindSheet.Applying -> {
                    val plan = if (sheet is RewindSheet.Ready) sheet.plan else (sheet as RewindSheet.Applying).plan
                    val messages = plan.messages.toInt()
                    Text(pluralStringResource(R.plurals.agents_rewind_leave, messages, messages), style = MaterialTheme.typography.bodyLarge)
                    AgentRewind.reason(plan.blocked)?.let { reason ->
                        Text(rewindReason(reason, agentName), style = MaterialTheme.typography.bodyMedium, color = g7Tints().amber)
                    }
                }
                is RewindSheet.Failed -> Text(
                    stringResource(if (sheet.failure == AgentFailure.OTHER) R.string.agents_wait_until_finishes else R.string.agents_failed_generic),
                    style = MaterialTheme.typography.bodyMedium,
                    color = scheme.error,
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Icon(AgentGlyphs.laptop, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.padding(top = 2.dp).size(17.dp))
                Text(stringResource(R.string.agents_rewind_phone, pcName), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
            }
            Row(Modifier.fillMaxWidth().padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                // Cancel first and plain; Rewind is the second, never the primary look (contract §U9.2).
                G7RaisedButton(label = stringResource(R.string.action_cancel), onClick = onDismiss, modifier = Modifier.weight(1f))
                if (sheet is RewindSheet.Failed) {
                    G7RaisedButton(label = stringResource(R.string.agents_try_again), onClick = onRetry, modifier = Modifier.weight(1f))
                } else {
                    G7RaisedButton(
                        label = stringResource(R.string.agents_rewind),
                        onClick = onRewind,
                        enabled = AgentRewind.canApply(sheet, nowMs),
                        modifier = Modifier.weight(1f),
                    )
                }
            }
        }
    }
}

/**
 * Agent settings on this phone (board 23): keep threads here for reading
 * while the PC is away, and clear what's kept (its size).
 */
@Composable
internal fun AgentCacheSheet(core: Core, pcName: String, onDismiss: () -> Unit) {
    val scope = rememberCoroutineScope()
    var keeps by remember { mutableStateOf<Boolean?>(null) }
    var bytes by remember { mutableStateOf<Long?>(null) }
    var busy by remember { mutableStateOf(false) }
    LaunchedEffect(core) {
        keeps = (core.agentCacheKeeps() as? AgentCall.Ok)?.value
        bytes = (core.agentCacheBytes() as? AgentCall.Ok)?.value
    }
    val context = LocalContext.current
    AgentSheet(onDismiss) {
        SheetHead(stringResource(R.string.agents_phone_settings), pcName)
        val keep = keeps
        SheetRow(
            label = stringResource(R.string.agents_cache_keep),
            enabled = keep != null && !busy,
            onClick = {
                val next = keep != true
                busy = true
                scope.launch {
                    if (core.agentCacheKeep(next) is AgentCall.Ok) keeps = next
                    bytes = (core.agentCacheBytes() as? AgentCall.Ok)?.value
                    busy = false
                }
            },
            trailing = { G7Switch(checked = keep == true, onCheckedChange = null, enabled = keep != null && !busy) },
        )
        val size = bytes?.let { android.text.format.Formatter.formatShortFileSize(context, it) }
        SheetRow(
            label = stringResource(R.string.agents_cache_clear, size.orEmpty()),
            enabled = !busy && (bytes ?: 0L) > 0L,
            onClick = {
                busy = true
                scope.launch {
                    core.agentCacheClear()
                    bytes = (core.agentCacheBytes() as? AgentCall.Ok)?.value
                    busy = false
                }
            },
        )
    }
}

/** "<PC> isn't reachable. Saved 12 min ago." over a list or a thread from what this phone saved. */
@Composable
internal fun OfflineBar(pcName: String, savedMs: Long?, nowMs: Long, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    val text = if (savedMs == null) {
        stringResource(R.string.agents_offline_unsaved, pcName)
    } else {
        val (unit, count) = AgentOffline.ago(savedMs, nowMs)
        pluralStringResource(
            when (unit) {
                AgentOffline.AgoUnit.MINUTES -> R.plurals.agents_offline_minutes
                AgentOffline.AgoUnit.HOURS -> R.plurals.agents_offline_hours
                AgentOffline.AgoUnit.DAYS -> R.plurals.agents_offline_days
            },
            count,
            pcName,
            count,
        )
    }
    Row(
        modifier = modifier.fillMaxWidth().background(scheme.surfaceContainer).heightIn(min = 36.dp).padding(horizontal = 16.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(G7Icons.wifi, contentDescription = null, tint = g7Tints().amber, modifier = Modifier.size(15.dp))
        Text(text, style = MaterialTheme.typography.bodySmall)
    }
}

/**
 * A message held on this phone, at the end of its thread (board 22): the
 * text, dashed, and how it stands. One that wasn't sent offers Send and
 * Discard.
 */
@Composable
internal fun HeldMessage(
    held: AgentHeld,
    pcName: String,
    folder: String,
    busy: Boolean,
    onSend: () -> Unit,
    onDiscard: () -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    val p = agentPalette()
    val line = AgentOutbox.line(held)
    Column(Modifier.fillMaxWidth().padding(start = 48.dp, top = 10.dp), horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text(
            held.text,
            style = MaterialTheme.typography.bodyLarge,
            modifier = Modifier
                .clip(RoundedCornerShape(12.dp))
                .border(1.dp, scheme.outline, RoundedCornerShape(12.dp))
                .background(scheme.surfaceContainer)
                .padding(horizontal = 14.dp, vertical = 10.dp),
        )
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            when (line) {
                HeldLine.Waiting -> {
                    Icon(G7Icons.history, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(13.dp))
                    Text(stringResource(R.string.agents_held_waiting), style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
                }
                HeldLine.Sending -> {
                    CircularProgressIndicator(Modifier.size(12.dp), strokeWidth = 1.5.dp)
                    Text(stringResource(R.string.agents_held_sending), style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
                }
                HeldLine.AnswerFirst -> {
                    Icon(G7Icons.history, contentDescription = null, tint = p.statusNeedsYou, modifier = Modifier.size(13.dp))
                    Text(stringResource(R.string.agents_held_answer_first), style = MaterialTheme.typography.labelMedium)
                }
                HeldLine.QueueFull -> {
                    Icon(G7Icons.history, contentDescription = null, tint = p.statusNeedsYou, modifier = Modifier.size(13.dp))
                    Text(stringResource(R.string.agents_held_queue_full, pcName), style = MaterialTheme.typography.labelMedium)
                }
                is HeldLine.NotSent -> {
                    Icon(AgentGlyphs.warning, contentDescription = null, tint = p.statusFailed, modifier = Modifier.size(13.dp))
                    Text(stringResource(R.string.agents_held_not_sent), style = MaterialTheme.typography.labelMedium, color = scheme.error)
                    if (busy) {
                        CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
                    } else {
                        G7RowAction(label = stringResource(R.string.agents_held_send), onClick = onSend)
                        G7RowAction(label = stringResource(R.string.agents_held_discard), onClick = onDiscard)
                    }
                }
            }
        }
        val why = (line as? HeldLine.NotSent)?.why?.let { heldWhy(it, pcName, folder) }
        if (why != null) Text(why, style = MaterialTheme.typography.labelSmall, color = scheme.onSurfaceVariant)
    }
}

@Composable
private fun heldWhy(why: AgentHeldWhy, pcName: String, folder: String): String? = when (why) {
    AgentHeldWhy.GONE -> stringResource(R.string.agents_thread_gone)
    AgentHeldWhy.READ_ONLY -> stringResource(R.string.agents_readonly_add, folder)
    AgentHeldWhy.TOO_OLD -> stringResource(R.string.agents_held_too_old)
    AgentHeldWhy.REFUSED -> stringResource(R.string.agents_held_refused, pcName)
    AgentHeldWhy.ANSWER_FIRST, AgentHeldWhy.QUEUE_FULL -> null
}

/** "A newer draft from %1 · Use it" over the composer (board 23). */
@Composable
internal fun DraftOfferLine(byName: String, onUse: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Row(
        modifier = Modifier.fillMaxWidth().heightIn(min = 36.dp).padding(start = 8.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(G7Icons.edit, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(15.dp))
        Text(stringResource(R.string.agents_draft_newer, byName), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant, modifier = Modifier.weight(1f))
        G7RowAction(label = stringResource(R.string.agents_draft_use), onClick = onUse)
    }
}

/** "Editing your message · Cancel" over the composer after Edit from here; Cancel undoes the rewind. */
@Composable
internal fun EditingStrip(busy: Boolean, onCancel: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Row(
        modifier = Modifier.fillMaxWidth().heightIn(min = 36.dp).padding(start = 8.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(G7Icons.edit, contentDescription = null, tint = scheme.primary, modifier = Modifier.size(15.dp))
        Text(stringResource(R.string.agents_editing), style = MaterialTheme.typography.bodySmall, modifier = Modifier.weight(1f))
        if (busy) {
            CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
        } else {
            G7RowAction(label = stringResource(R.string.action_cancel), onClick = onCancel)
        }
    }
}

/** One muted line over the composer: why something can't be done now. */
@Composable
internal fun ComposerNote(text: String, icon: ImageVector = G7Icons.info, warn: Boolean = false) {
    val scheme = MaterialTheme.colorScheme
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 6.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(scheme.surfaceContainer)
            .padding(horizontal = 12.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(icon, contentDescription = null, tint = if (warn) g7Tints().amber else scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
        Text(text, style = MaterialTheme.typography.bodySmall)
    }
}

/** Shown off, a little faded. */
internal fun Modifier.dimmed(off: Boolean): Modifier = if (off) alpha(0.45f) else this
