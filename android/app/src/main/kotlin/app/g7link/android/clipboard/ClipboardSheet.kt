// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.clipboard

import android.net.Uri
import android.text.format.DateUtils
import android.text.format.Formatter
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.g7link.android.R
import app.g7link.android.core.Device
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.g7Tints
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** What was on the phone's clipboard when the sheet opened. */
internal sealed interface CopiedClip {
    /** [link] when it's a web link, which the sheet labels as one. */
    data class Text(val text: String, val html: String?, val link: Boolean) : CopiedClip

    data class Image(val uri: Uri) : CopiedClip

    /** Marked sensitive by a password manager: sent hidden, never shown. Not a data class, so it never prints. */
    class Hidden(val text: String) : CopiedClip
}

/** What the Send clipboard sheet shows. */
internal sealed interface ClipSheetState {
    /** What was copied, before it's sent; [copiedAt] in wall-clock milliseconds, when Android said. */
    data class Preview(val clip: CopiedClip, val copiedAt: Long?, val sending: Boolean = false) : ClipSheetState

    /** On these PCs' clipboards now; [pasteProblem] says why Paste on the PC didn't work. */
    data class Sent(
        val clip: CopiedClip,
        val pcIds: List<String>,
        val pasting: Boolean = false,
        val pasteProblem: String? = null,
    ) : ClipSheetState

    /** Waiting for an offline PC; [message] says so. */
    data class Queued(val clip: CopiedClip, val message: String) : ClipSheetState

    data object Empty : ClipSheetState

    /** Only a message, and Close: what was copied is private ([isPrivate]), or it didn't go. */
    data class Note(val text: String, val isPrivate: Boolean = false) : ClipSheetState
}

/** Who a clipboard send goes to, as Core's send picks them: the chosen PC, else those online, else all. */
internal fun clipTargets(devices: List<Device>, pcId: String?): List<Device> =
    if (pcId != null) {
        devices.filter { it.id == pcId }
    } else {
        devices.filter { it.online }.ifEmpty { devices }
    }

private const val HIDDEN_DOTS = "••••••••••••"

/** The largest picture of a copied image the sheet decodes, in pixels. */
private const val PICTURE_WIDTH = 1080
private const val PICTURE_HEIGHT = 720

/**
 * The Send clipboard sheet: what was copied and where it goes, then whether it
 * got there. [closing] slides it away and then calls [onClosed], as swiping it
 * away does.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ClipboardSheet(
    state: ClipSheetState,
    devices: List<Device>,
    pcId: String?,
    closing: Boolean,
    onSend: () -> Unit,
    onPaste: () -> Unit,
    onClose: () -> Unit,
    onClosed: () -> Unit,
) {
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    LaunchedEffect(closing) {
        if (closing) {
            sheetState.hide()
            onClosed()
        }
    }
    val context = LocalContext.current
    val clip = when (state) {
        is ClipSheetState.Preview -> state.clip
        is ClipSheetState.Sent -> state.clip
        is ClipSheetState.Queued -> state.clip
        else -> null
    }
    val imageUri = (clip as? CopiedClip.Image)?.uri
    val image by produceState<PhoneClipboard.ImagePreview?>(null, imageUri) {
        val app = context.applicationContext
        value = imageUri?.let { uri ->
            withContext(Dispatchers.IO) { PhoneClipboard.previewImage(app, uri, PICTURE_WIDTH, PICTURE_HEIGHT) }
        }
    }
    val imageDetails = image?.let { preview ->
        ClipPreview.imageDetails(
            format = ClipPreview.imageFormat(preview.mime),
            width = preview.width,
            height = preview.height,
            size = preview.bytes?.let { Formatter.formatShortFileSize(context, it) },
        )
    }?.takeIf { it.isNotEmpty() } ?: stringResource(R.string.clipboard_history_image)

    ModalBottomSheet(
        onDismissRequest = onClosed,
        sheetState = sheetState,
        containerColor = MaterialTheme.colorScheme.surfaceContainerHigh,
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 20.dp)
                .padding(bottom = 20.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            when (state) {
                is ClipSheetState.Preview -> PreviewContent(
                    state = state,
                    targets = clipTargets(devices, pcId),
                    image = image,
                    imageDetails = imageDetails,
                    onSend = onSend,
                    onCancel = onClose,
                )
                is ClipSheetState.Sent -> SentContent(
                    state = state,
                    devices = devices,
                    image = image,
                    imageDetails = imageDetails,
                    onPaste = onPaste,
                    onDone = onClose,
                )
                is ClipSheetState.Queued -> {
                    StatusHeading(icon = G7Icons.clock, tint = g7Tints().amber, title = state.message)
                    OneLine(state.clip, image, imageDetails)
                    G7PrimaryButton(label = stringResource(R.string.action_done), onClick = onClose)
                }
                ClipSheetState.Empty -> {
                    StatusHeading(
                        icon = G7Icons.clipboard,
                        tint = g7Tints().muted,
                        title = stringResource(R.string.clip_sheet_nothing_title),
                        text = stringResource(R.string.clip_sheet_nothing_text),
                    )
                    G7RaisedButton(
                        label = stringResource(R.string.clip_sheet_close),
                        onClick = onClose,
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
                is ClipSheetState.Note -> {
                    StatusHeading(
                        icon = if (state.isPrivate) G7Icons.lock else G7Icons.info,
                        tint = if (state.isPrivate) g7Tints().muted else g7Tints().red,
                        title = null,
                        text = state.text,
                    )
                    G7RaisedButton(
                        label = stringResource(R.string.clip_sheet_close),
                        onClick = onClose,
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            }
        }
    }
}

@Composable
private fun PreviewContent(
    state: ClipSheetState.Preview,
    targets: List<Device>,
    image: PhoneClipboard.ImagePreview?,
    imageDetails: String,
    onSend: () -> Unit,
    onCancel: () -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    val names = targets.joinToString { it.name }
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(stringResource(R.string.action_send_clipboard), style = MaterialTheme.typography.titleLarge)
        Text(
            stringResource(R.string.clip_sheet_subtitle),
            style = MaterialTheme.typography.bodyMedium,
            color = scheme.onSurfaceVariant,
        )
    }

    val copied = copiedLabel(state.copiedAt)
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(14.dp))
            .background(scheme.surfaceContainer)
            .padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        when (val clip = state.clip) {
            is CopiedClip.Text -> {
                if (clip.link) LinkLabel()
                val shown = remember(clip.text) { ClipPreview.lines(clip.text) }
                Text(
                    shown,
                    style = MaterialTheme.typography.bodyMedium,
                    color = scheme.onSurface,
                    maxLines = ClipPreview.LINES,
                    overflow = TextOverflow.Ellipsis,
                )
                val count = remember(clip.text) { ClipPreview.characters(clip.text) }
                CardFooter(pluralStringResource(R.plurals.clip_sheet_text_length, count, count), copied)
            }
            is CopiedClip.Image -> {
                Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                    val picture = image?.picture
                    if (picture != null) {
                        val bitmap = remember(picture) { picture.asImageBitmap() }
                        Image(
                            bitmap = bitmap,
                            contentDescription = stringResource(R.string.clipboard_history_image),
                            contentScale = ContentScale.Fit,
                            modifier = Modifier
                                .heightIn(max = 180.dp)
                                .clip(RoundedCornerShape(10.dp)),
                        )
                    } else {
                        Box(Modifier.fillMaxWidth().height(120.dp), contentAlignment = Alignment.Center) {
                            Icon(
                                G7Icons.photo,
                                contentDescription = null,
                                tint = scheme.onSurfaceVariant,
                                modifier = Modifier.size(32.dp),
                            )
                        }
                    }
                }
                CardFooter(imageDetails, copied)
            }
            is CopiedClip.Hidden -> {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Icon(
                        G7Icons.lock,
                        contentDescription = null,
                        tint = scheme.onSurfaceVariant,
                        modifier = Modifier.size(20.dp),
                    )
                    Text(
                        HIDDEN_DOTS,
                        style = MaterialTheme.typography.titleMedium,
                        letterSpacing = 2.sp,
                        color = scheme.onSurface,
                        maxLines = 1,
                    )
                }
                Text(
                    stringResource(R.string.clip_sheet_hidden_note, names.ifEmpty { stringResource(R.string.your_pc) }),
                    style = MaterialTheme.typography.bodySmall,
                    color = scheme.onSurfaceVariant,
                )
                if (copied != null) CardFooter(null, copied)
            }
        }
    }

    val single = targets.singleOrNull()
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(
            if (targets.isNotEmpty() && targets.all { it.isAndroid }) G7Icons.phone else G7Icons.monitor,
            contentDescription = null,
            tint = scheme.onSurfaceVariant,
            modifier = Modifier.size(22.dp),
        )
        Column(Modifier.weight(1f)) {
            if (targets.isEmpty()) {
                Text(
                    stringResource(R.string.clip_no_pc),
                    style = MaterialTheme.typography.bodyMedium,
                    color = scheme.onSurfaceVariant,
                )
            } else {
                Text(
                    names,
                    style = MaterialTheme.typography.bodyLarge,
                    color = scheme.onSurface,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    when {
                        single == null -> stringResource(R.string.clip_sheet_lands_each)
                        !single.online && state.clip !is CopiedClip.Hidden ->
                            stringResource(R.string.send_queue_offline_toast, single.name)
                        else -> stringResource(R.string.clip_sheet_lands)
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = scheme.onSurfaceVariant,
                )
            }
        }
    }

    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        G7RaisedButton(
            label = stringResource(R.string.action_cancel),
            onClick = onCancel,
            modifier = Modifier.weight(1f),
        )
        G7PrimaryButton(
            label = when {
                state.sending -> stringResource(R.string.sending_clipboard)
                state.clip is CopiedClip.Hidden -> stringResource(R.string.clip_sheet_send_hidden)
                single != null -> stringResource(R.string.home_send_to, single.name)
                else -> stringResource(R.string.clip_sheet_send)
            },
            onClick = onSend,
            modifier = Modifier.weight(2f),
            enabled = !state.sending && targets.isNotEmpty(),
        )
    }
}

@Composable
private fun SentContent(
    state: ClipSheetState.Sent,
    devices: List<Device>,
    image: PhoneClipboard.ImagePreview?,
    imageDetails: String,
    onPaste: () -> Unit,
    onDone: () -> Unit,
) {
    val sentTo = state.pcIds.mapNotNull { id -> devices.firstOrNull { it.id == id } }
    val single = sentTo.singleOrNull()
    val names = sentTo.joinToString { it.name }.ifEmpty { stringResource(R.string.your_pc) }
    StatusHeading(
        icon = G7Icons.check,
        tint = g7Tints().teal,
        title = if (single != null) {
            stringResource(R.string.clip_sheet_on_clipboard, single.name)
        } else {
            stringResource(R.string.clip_sent_to, names)
        },
        text = if (sentTo.none { it.isAndroid }) stringResource(R.string.clip_sheet_paste_hint) else null,
    )
    OneLine(state.clip, image, imageDetails)
    if (state.pasteProblem != null) {
        Text(
            state.pasteProblem,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
            modifier = Modifier.fillMaxWidth(),
        )
    }
    // Only a PC that takes this phone's keys; it says so if control is off there.
    val canPaste = single != null && single.online && !single.isAndroid && single.has("input.remote")
    if (canPaste && single != null) {
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            G7RaisedButton(
                label = stringResource(R.string.action_done),
                onClick = onDone,
                modifier = Modifier.weight(1f),
            )
            G7PrimaryButton(
                label = stringResource(R.string.clip_sheet_paste_on, single.name),
                onClick = onPaste,
                modifier = Modifier.weight(2f),
                enabled = !state.pasting,
            )
        }
    } else {
        G7PrimaryButton(label = stringResource(R.string.action_done), onClick = onDone)
    }
}

/** An outcome's icon in a soft circle of its tint, its title, and a line under it. */
@Composable
private fun StatusHeading(icon: ImageVector, tint: Color, title: String?, text: String? = null) {
    Column(
        modifier = Modifier.fillMaxWidth().padding(top = 4.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Box(
            modifier = Modifier
                .padding(bottom = 6.dp)
                .size(56.dp)
                .clip(CircleShape)
                .background(tint.copy(alpha = 0.16f)),
            contentAlignment = Alignment.Center,
        ) {
            Icon(icon, contentDescription = null, tint = tint, modifier = Modifier.size(28.dp))
        }
        if (title != null) {
            Text(title, style = MaterialTheme.typography.titleLarge, textAlign = TextAlign.Center)
        }
        if (text != null) {
            Text(
                text,
                style = if (title == null) MaterialTheme.typography.bodyLarge else MaterialTheme.typography.bodyMedium,
                color = if (title == null) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
            )
        }
    }
}

/** What was sent, on one line. */
@Composable
private fun OneLine(clip: CopiedClip, image: PhoneClipboard.ImagePreview?, imageDetails: String) {
    val scheme = MaterialTheme.colorScheme
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(scheme.surfaceContainer)
            .padding(horizontal = 12.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        val picture = image?.picture
        if (clip is CopiedClip.Image && picture != null) {
            val bitmap = remember(picture) { picture.asImageBitmap() }
            Image(
                bitmap = bitmap,
                contentDescription = null,
                contentScale = ContentScale.Crop,
                modifier = Modifier.size(36.dp).clip(RoundedCornerShape(6.dp)),
            )
        } else {
            Icon(
                when (clip) {
                    is CopiedClip.Text -> if (clip.link) G7Icons.link else G7Icons.text
                    is CopiedClip.Image -> G7Icons.photo
                    is CopiedClip.Hidden -> G7Icons.lock
                },
                contentDescription = null,
                tint = scheme.onSurfaceVariant,
                modifier = Modifier.size(20.dp),
            )
        }
        Text(
            when (clip) {
                is CopiedClip.Text -> remember(clip.text) { ClipPreview.oneLine(clip.text) }
                is CopiedClip.Image -> imageDetails
                is CopiedClip.Hidden -> HIDDEN_DOTS
            },
            style = MaterialTheme.typography.bodyMedium,
            color = scheme.onSurface,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
    }
}

/** The card's last line: what it is on the left, when it was copied on the right. */
@Composable
private fun CardFooter(what: String?, copied: String?) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text(
            what.orEmpty(),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        if (copied != null) {
            Text(
                copied,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
            )
        }
    }
}

/** The small "Link" label over a copied web link. */
@Composable
private fun LinkLabel() {
    val blue = g7Tints().blue
    Row(
        modifier = Modifier
            .clip(RoundedCornerShape(6.dp))
            .background(blue.copy(alpha = 0.14f))
            .padding(horizontal = 8.dp, vertical = 3.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Icon(G7Icons.link, contentDescription = null, tint = blue, modifier = Modifier.size(14.dp))
        Text(stringResource(R.string.queue_kind_link), style = MaterialTheme.typography.labelMedium, color = blue)
    }
}

/** "Copied 5 minutes ago", or nothing when Android didn't say when. */
@Composable
private fun copiedLabel(copiedAt: Long?): String? {
    val now = remember(copiedAt) { System.currentTimeMillis() }
    return when (val copied = ClipPreview.copied(copiedAt, now)) {
        null -> null
        ClipPreview.Copied.JustNow ->
            stringResource(R.string.clip_sheet_copied, stringResource(R.string.clipboard_history_just_now))
        is ClipPreview.Copied.At -> stringResource(
            R.string.clip_sheet_copied,
            DateUtils.getRelativeTimeSpanString(copied.atMillis, now, DateUtils.MINUTE_IN_MILLIS).toString(),
        )
    }
}
