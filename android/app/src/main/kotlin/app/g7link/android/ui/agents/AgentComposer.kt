// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import android.text.format.DateFormat
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
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
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.g7Collapse
import app.g7link.android.ui.theme.g7Expand
import app.g7link.android.ui.theme.g7Tints
import java.util.Date

/** What the reply box's controls row shows of the thread's settings. */
data class ComposerSettings(
    /** The PC offers control: the model control is shown. */
    val shown: Boolean,
    /** The model's name; null for the CLI's default ("Default"). */
    val model: String?,
    /** The effort, as the CLI names it (first letter capitalised); null when none applies. */
    val effort: String?,
    /** "Plan" beside it: the mode isn't the phone's default. */
    val plan: Boolean,
)

/** What the reply box says while it's empty. */
@Composable
internal fun composerHint(hint: ComposerHint, agentName: String, pausedUntil: Long?, pcName: String): String {
    val context = LocalContext.current
    return when (hint) {
        ComposerHint.MESSAGE -> stringResource(R.string.agents_message_hint, agentName)
        ComposerHint.REPLY_WHEN_STOPS -> stringResource(R.string.agents_reply_when_stops, agentName)
        ComposerHint.SAY_WHY -> stringResource(R.string.agents_say_why_hint, agentName)
        ComposerHint.AFTER_TIME -> stringResource(
            R.string.agents_reply_after,
            pausedUntil?.let { DateFormat.getTimeFormat(context).format(Date(it)) }.orEmpty(),
        )
        ComposerHint.AFTER_RESET -> stringResource(R.string.agents_reply_after_reset)
        ComposerHint.OTHER_ANSWER -> stringResource(R.string.agents_question_other_hint)
        ComposerHint.CHOOSE_ABOVE -> stringResource(R.string.agents_question_choose_hint)
        ComposerHint.ANSWER_ON_PC -> stringResource(R.string.agents_question_answer_on, pcName)
        ComposerHint.OFFLINE -> stringResource(R.string.agents_sends_when_back, pcName)
    }
}

/**
 * The reply box (contract §Q6.1, the phone board 7A, 7D): the images going with
 * the message, the text, and a row with Add image, the model and effort, Plan
 * when it's on, Stop whenever a turn runs or waits, the mic when the phone can
 * hear a reply on the device (phase 5 §V8.1, its one slot: [voice]), and the
 * button: send, deny with the words, or while it works steer (long-press:
 * "Queue it instead") or queue. Under it the folder, and what the message will do.
 */
@Composable
internal fun AgentComposer(
    view: ComposerView,
    hint: String,
    text: String,
    onText: (String) -> Unit,
    onFocus: (Boolean) -> Unit,
    focus: FocusRequester,
    sending: Boolean,
    stopping: Boolean,
    onStop: () -> Unit,
    /** The button: null is its own action; a follow-up is the one chosen on long-press. */
    onSend: (FollowUp?) -> Unit,
    joined: Boolean,
    place: String,
    agentName: String,
    /** The thread's agent is Codex: a steer is read at once, not at its next step. */
    codex: Boolean,
    settings: ComposerSettings,
    onSettings: () -> Unit,
    attachShown: Boolean,
    onAttach: () -> Unit,
    images: List<PendingImage>,
    thumbs: ImageThumbs,
    onRemoveImage: (String) -> Unit,
    /** Images are attached but the chosen model takes none. */
    imagesRefused: Boolean,
    /** Words over the limit (as opposed to empty). */
    tooLong: Boolean,
    /** Lines of a diff quoted into the message (§SC9.1 "Comment on a line"). */
    quotes: List<Quote> = emptyList(),
    onRemoveQuote: (Int) -> Unit = {},
    /** The mic, left of the button (contract §V8.1); absent where speech can't stay on the phone. */
    voice: (@Composable () -> Unit)? = null,
    /** Words the mic just added, to select in the box. */
    selection: TextSelection? = null,
) {
    val scheme = MaterialTheme.colorScheme
    val shape = if (joined) RoundedCornerShape(bottomStart = 12.dp, bottomEnd = 12.dp) else RoundedCornerShape(12.dp)
    val divider = scheme.outlineVariant
    Column(Modifier.fillMaxWidth().padding(bottom = 8.dp)) {
        val problem = when {
            tooLong -> stringResource(R.string.agents_too_long)
            imagesRefused -> stringResource(R.string.agents_image_model, agentName)
            else -> null
        }
        if (problem != null) {
            Text(problem, style = MaterialTheme.typography.bodySmall, color = scheme.error, modifier = Modifier.padding(start = 4.dp, bottom = 4.dp))
        }
        Column(
            Modifier
                .fillMaxWidth()
                .clip(shape)
                .background(scheme.surfaceContainer)
                .then(if (joined) Modifier.drawBehind { drawLine(divider, Offset(0f, 0f), Offset(size.width, 0f), 1.dp.toPx()) } else Modifier)
                .padding(end = 8.dp, bottom = 8.dp),
        ) {
            if (images.isNotEmpty()) ImageStrip(images, thumbs, onRemoveImage)
            if (quotes.isNotEmpty()) QuoteChips(quotes, onRemoveQuote)
            AgentTextField(
                value = text,
                onValueChange = onText,
                hint = hint,
                enabled = view.enabled,
                maxLines = 5,
                background = Color.Transparent,
                onFocus = onFocus,
                modifier = Modifier.focusRequester(focus),
                selection = selection,
            )
            Row(
                modifier = Modifier.fillMaxWidth().padding(start = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                if (attachShown) {
                    G7IconButton(
                        icon = G7Icons.photo,
                        contentDescription = stringResource(R.string.agents_add_image),
                        onClick = onAttach,
                        enabled = view.attach && AgentImages.room(images) > 0,
                        tint = scheme.onSurfaceVariant,
                    )
                }
                if (settings.shown) SettingsChip(settings, view.settings, onSettings, Modifier.weight(1f, fill = false))
                Spacer(Modifier.weight(1f))
                if (view.showStop) {
                    ComposerPill(G7Icons.stop, stringResource(R.string.agents_stop), accent = false, enabled = !stopping, onClick = onStop)
                }
                if (voice != null && view.enabled) voice()
                if (sending) {
                    Box(Modifier.size(44.dp), contentAlignment = Alignment.Center) {
                        CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                    }
                } else {
                    when (view.action) {
                        ComposerAction.NONE -> Unit
                        ComposerAction.SEND -> G7IconButton(
                            icon = G7Icons.send,
                            contentDescription = stringResource(R.string.agents_send),
                            onClick = { onSend(null) },
                            enabled = view.actionEnabled,
                            filled = true,
                            tint = if (view.actionEnabled) scheme.primary else scheme.onSurfaceVariant,
                        )
                        ComposerAction.DENY_AND_SEND -> ComposerPill(
                            G7Icons.send,
                            stringResource(R.string.agents_deny_and_send),
                            accent = true,
                            enabled = view.actionEnabled,
                            onClick = { onSend(null) },
                        )
                        ComposerAction.FOLLOW_UP -> FollowUpButton(view, onSend)
                    }
                }
            }
        }
        Row(Modifier.fillMaxWidth().padding(start = 4.dp, end = 4.dp, top = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            FolderLabel(place, Modifier.weight(1f))
            val note = when {
                view.codexNote -> stringResource(R.string.agents_codex_reads_next)
                else -> view.followNote?.let { followNote(it, agentName, codex) }
            }
            if (note != null) {
                Text(
                    note,
                    style = MaterialTheme.typography.labelSmall,
                    color = scheme.onSurfaceVariant,
                    textAlign = TextAlign.End,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.padding(start = 8.dp),
                )
            }
        }
    }
}

/** What the line under the box says a follow-up does (contract §Q8). */
@Composable
private fun followNote(followUp: FollowUp, agentName: String, codex: Boolean): String = when (followUp) {
    FollowUp.STEER -> if (codex) stringResource(R.string.agents_steer_now, agentName) else stringResource(R.string.agents_steer_next_step, agentName)
    FollowUp.QUEUE -> stringResource(R.string.agents_queue_hint, agentName)
}

/** Steer (or queue) while it works; a long-press offers the other ("Queue it instead"). */
@Composable
private fun FollowUpButton(view: ComposerView, onSend: (FollowUp?) -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val primary = view.followUp ?: return
    val other = view.otherFollowUp
    var menu by remember { mutableStateOf(false) }
    val enabled = view.actionEnabled
    val description = stringResource(if (primary == FollowUp.STEER) R.string.agents_send else R.string.agents_queue_it)
    Box {
        Box(
            modifier = Modifier
                .size(44.dp)
                .clip(RoundedCornerShape(10.dp))
                .background(if (enabled) scheme.primary else scheme.surfaceContainerHigh)
                .combinedClickable(
                    enabled = enabled,
                    role = Role.Button,
                    onClick = { onSend(null) },
                    onLongClick = if (other != null) ({ menu = true }) else null,
                )
                .semantics { contentDescription = description },
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                if (primary == FollowUp.STEER) AgentGlyphs.steer else AgentGlyphs.queue,
                contentDescription = null,
                tint = if (enabled) scheme.onPrimary else scheme.onSurfaceVariant,
                modifier = Modifier.size(20.dp),
            )
        }
        if (other != null) {
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                DropdownMenuItem(
                    text = { Text(stringResource(R.string.agents_queue_it)) },
                    leadingIcon = { Icon(AgentGlyphs.queue, contentDescription = null, modifier = Modifier.size(16.dp)) },
                    onClick = {
                        menu = false
                        onSend(other)
                    },
                )
            }
        }
    }
}

/** "Opus · High ˅": opens the sheet; while it works a tap says why it can't, while a question waits nothing happens. */
@Composable
private fun SettingsChip(settings: ComposerSettings, access: SettingsAccess, onClick: () -> Unit, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    val model = settings.model ?: stringResource(R.string.agents_model_default)
    val label = listOfNotNull(model, settings.effort).joinToString(" · ")
    val inert = access == SettingsAccess.INERT
    Row(modifier, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        Row(
            modifier = Modifier
                .heightIn(min = 40.dp)
                .clip(RoundedCornerShape(8.dp))
                .clickable(enabled = !inert, role = Role.Button, onClick = onClick)
                .then(if (inert) Modifier.alpha(0.45f) else Modifier)
                .padding(horizontal = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text(label, style = MaterialTheme.typography.labelLarge, color = scheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Icon(G7Icons.chevronDown, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
        }
        if (settings.plan) {
            Row(
                modifier = Modifier
                    .heightIn(min = 32.dp)
                    .clip(RoundedCornerShape(6.dp))
                    .background(scheme.surfaceContainerHigh)
                    .padding(horizontal = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Icon(G7Icons.clipboardList, contentDescription = null, tint = scheme.onSurface, modifier = Modifier.size(14.dp))
                Text(stringResource(R.string.agents_plan), style = MaterialTheme.typography.labelLarge, maxLines = 1)
            }
        }
    }
}

/** The images going with the message: a thumbnail each once the PC has it, with remove. */
@Composable
internal fun ImageStrip(images: List<PendingImage>, thumbs: ImageThumbs, onRemove: (String) -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Row(
        modifier = Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(start = 12.dp, top = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        for (image in images) {
            Box(Modifier.size(56.dp)) {
                Box(
                    Modifier.size(52.dp).align(Alignment.BottomStart).clip(RoundedCornerShape(8.dp)).background(scheme.surfaceContainerHigh),
                    contentAlignment = Alignment.Center,
                ) {
                    when (image) {
                        is PendingImage.Uploading -> CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                        is PendingImage.Ready -> ThumbImage(thumbs, image.id, Modifier.size(52.dp))
                    }
                }
                Box(
                    Modifier
                        .size(20.dp)
                        .align(Alignment.TopEnd)
                        .clip(CircleShape)
                        .background(scheme.surface)
                        .clickable(role = Role.Button) { onRemove(image.key) },
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(G7Icons.close, contentDescription = stringResource(R.string.agents_remove_image), tint = scheme.onSurface, modifier = Modifier.size(12.dp))
                }
            }
        }
    }
}

/** An image the PC holds, by its thumbnail; the photo glyph until it arrives or when it can't. */
@Composable
internal fun ThumbImage(thumbs: ImageThumbs, id: String, modifier: Modifier = Modifier) {
    val bitmap by produceState<ImageBitmap?>(null, thumbs, id) { value = thumbs.load(id) }
    val shown = bitmap
    if (shown != null) {
        Image(shown, contentDescription = null, contentScale = ContentScale.Crop, modifier = modifier.clip(RoundedCornerShape(8.dp)))
    } else {
        Box(modifier, contentAlignment = Alignment.Center) {
            Icon(G7Icons.photo, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.size(18.dp))
        }
    }
}

/** A button of the reply box with its glyph and word: Stop, or "Deny and send" in the accent. */
@Composable
internal fun ComposerPill(icon: ImageVector, label: String, accent: Boolean, enabled: Boolean, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val on = accent && enabled
    val background = if (on) scheme.primary else scheme.surfaceContainerHigh
    val content = when {
        on -> scheme.onPrimary
        enabled -> scheme.onSurface
        else -> scheme.onSurfaceVariant
    }
    Row(
        modifier = Modifier
            .height(44.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(background)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(start = 12.dp, end = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Icon(icon, contentDescription = null, tint = content, modifier = Modifier.size(20.dp))
        Text(label, style = MaterialTheme.typography.labelLarge.copy(fontWeight = if (accent) FontWeight.SemiBold else FontWeight.Medium), color = content, maxLines = 1)
    }
}

/**
 * The one line pinned directly above the queue, the dock and the reply box
 * while the thread's agent has a limit almost used or reached (reconcile file,
 * "Composer"): never a row of the thread, so it never scrolls or folds.
 */
@Composable
internal fun LimitLine(limit: LimitInfo) {
    val context = LocalContext.current
    val tints = g7Tints()
    val resets = limit.resetsMs?.let { DateFormat.getTimeFormat(context).format(Date(it)) }
    val text = when (limit.state) {
        LimitState.REACHED -> resets?.let { stringResource(R.string.agents_paused_resets, it) } ?: stringResource(R.string.agents_usage_limit)
        LimitState.WARN, LimitState.OK -> {
            val window = windowText(limit.window)
            if (resets != null) stringResource(R.string.agents_limit_almost, window, resets) else stringResource(R.string.agents_limit_almost_no_time, window)
        }
    }
    Row(
        Modifier.fillMaxWidth().heightIn(min = 30.dp).padding(horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(G7Icons.history, contentDescription = null, tint = tints.amber, modifier = Modifier.size(14.dp))
        Text(text, style = MaterialTheme.typography.bodySmall.copy(fontWeight = FontWeight.Medium), color = tints.amber, maxLines = 2)
    }
}

/** A usage window's name as the line says it: "5-hour", "Weekly", or as the CLI gave it. */
@Composable
internal fun windowText(window: String): String = when (AgentUsage.windowName(window)) {
    WindowName.FIVE_HOUR -> stringResource(R.string.agents_window_five_hour)
    WindowName.WEEKLY -> stringResource(R.string.agents_window_weekly)
    WindowName.AS_GIVEN -> window
}

/**
 * The messages waiting their turn, pinned above the dock and the reply box
 * (the phone board, 7D): "2 queued · Sent when Codex finishes.", then each on
 * one line with Send now, edit and remove; held after a turn that didn't finish.
 */
@Composable
internal fun QueueBox(
    control: ThreadControl,
    agentName: String,
    waiting: Boolean,
    busy: Set<String>,
    onSendNow: (String) -> Unit,
    onEdit: (QueuedLine) -> Unit,
    onRemove: (String) -> Unit,
    key: String,
) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    var open by rememberSaveable(key) { mutableStateOf(true) }
    val count = control.queued.size
    Column(
        Modifier
            .fillMaxWidth()
            .padding(bottom = 6.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(scheme.surfaceContainer)
            .padding(horizontal = 8.dp, vertical = 4.dp),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = 40.dp)
                .clip(RoundedCornerShape(6.dp))
                .clickable(role = Role.Button) { open = !open },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Icon(if (open) G7Icons.chevronDown else G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
            Icon(AgentGlyphs.queue, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(15.dp))
            Text(pluralStringResource(R.plurals.agents_queued, count, count), style = MaterialTheme.typography.labelLarge.copy(fontWeight = FontWeight.SemiBold))
            Spacer(Modifier.weight(1f))
            Text(
                if (control.queueHeld) stringResource(R.string.agents_queue_held) else stringResource(R.string.agents_queue_hint, agentName),
                style = MaterialTheme.typography.labelSmall,
                color = if (control.queueHeld) tints.amber else scheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        AnimatedVisibility(visible = open, enter = g7Expand(), exit = g7Collapse()) {
            Column {
                for (line in control.queued) {
                    Row(
                        modifier = Modifier.fillMaxWidth().heightIn(min = 44.dp).padding(start = 20.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(line.text.lineSequence().firstOrNull().orEmpty(), style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                        val lineBusy = line.id in busy
                        val canSendNow = !lineBusy && AgentQueue.canSendNow(waiting)
                        Box(Modifier.then(if (canSendNow) Modifier else Modifier.alpha(0.45f))) {
                            SmallTextButton(
                                label = stringResource(R.string.agents_queue_send_now),
                                onClick = { if (canSendNow) onSendNow(line.id) },
                            )
                        }
                        G7IconButton(
                            icon = G7Icons.edit,
                            contentDescription = stringResource(R.string.agents_queue_edit),
                            onClick = { onEdit(line) },
                            enabled = !lineBusy && AgentQueue.canEdit(line),
                            tint = scheme.onSurfaceVariant,
                        )
                        G7IconButton(
                            icon = G7Icons.trash,
                            contentDescription = stringResource(R.string.agents_queue_remove),
                            onClick = { onRemove(line.id) },
                            enabled = !lineBusy,
                            tint = scheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}

/** Edits a queued message's words; saved to the PC, where it waits. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun QueueEditSheet(line: QueuedLine, onSave: (String) -> Unit, onDismiss: () -> Unit) {
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    var text by rememberSaveable(line.id) { mutableStateOf(line.text) }
    val focus = remember { FocusRequester() }
    LaunchedEffect(focus) { runCatching { focus.requestFocus() } }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = MaterialTheme.colorScheme.surfaceContainerHigh) {
        Column(
            Modifier.fillMaxWidth().padding(horizontal = 20.dp).padding(bottom = 20.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(stringResource(R.string.agents_queue_edit_title), style = MaterialTheme.typography.titleLarge)
            AgentTextField(
                value = text,
                onValueChange = { text = it },
                hint = stringResource(R.string.agents_prompt_hint),
                minLines = 3,
                maxLines = 8,
                background = MaterialTheme.colorScheme.surfaceContainer,
                modifier = Modifier.focusRequester(focus),
            )
            if (AgentsModel.tooLong(text)) {
                Text(stringResource(R.string.agents_too_long), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
            }
            G7PrimaryButton(
                label = stringResource(R.string.agents_queue_save),
                enabled = AgentsModel.fits(text) && text.trim() != line.text.trim(),
                onClick = { onSave(text.trim()) },
            )
        }
    }
}

/**
 * "Add an image" (the phone board, 7C): take a photo or pick from the
 * gallery, and that the PC removes location and camera details first.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ImageSourceSheet(pcName: String, agentName: String, onCamera: () -> Unit, onGallery: () -> Unit, onDismiss: () -> Unit) {
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val scheme = MaterialTheme.colorScheme
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = scheme.surfaceContainerHigh) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp).padding(bottom = 20.dp)) {
            Text(stringResource(R.string.agents_add_an_image), style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(horizontal = 8.dp, vertical = 8.dp))
            SheetAction(G7Icons.camera, stringResource(R.string.agents_take_photo), onCamera)
            SheetAction(G7Icons.photo, stringResource(R.string.agents_add_image), onGallery)
            Text(
                stringResource(R.string.agents_images_note, pcName, agentName),
                style = MaterialTheme.typography.bodySmall,
                color = scheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 8.dp, vertical = 8.dp),
            )
        }
    }
}

@Composable
private fun SheetAction(icon: ImageVector, label: String, onClick: () -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 52.dp)
            .clip(RoundedCornerShape(8.dp))
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Icon(icon, contentDescription = null, tint = MaterialTheme.colorScheme.onSurface, modifier = Modifier.size(20.dp))
        Text(label, style = MaterialTheme.typography.bodyLarge)
    }
}

/** Buttons side by side: an outline and a primary (the plan card's, the question dock's). */
@Composable
internal fun ButtonPair(
    secondary: String,
    onSecondary: () -> Unit,
    secondaryEnabled: Boolean,
    primary: String,
    onPrimary: () -> Unit,
    primaryEnabled: Boolean,
    modifier: Modifier = Modifier,
) {
    Row(modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        G7RaisedButton(label = secondary, onClick = onSecondary, enabled = secondaryEnabled, modifier = Modifier.weight(1f))
        G7PrimaryButton(label = primary, onClick = onPrimary, enabled = primaryEnabled, modifier = Modifier.weight(1f))
    }
}
