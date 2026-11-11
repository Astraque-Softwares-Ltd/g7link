// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.recorder

import androidx.compose.foundation.clickable
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import app.g7link.android.ui.theme.G7Header
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.G7Row
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.G7SectionLabel
import app.g7link.android.ui.theme.g7Tints
import android.Manifest
import android.media.MediaPlayer
import android.text.format.Formatter
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.snap
import androidx.compose.animation.core.spring
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.CoreState
import app.g7link.android.core.Device
import app.g7link.android.recorder.DeliveryState
import app.g7link.android.recorder.RecorderService
import app.g7link.android.recorder.SavedRecording
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.LocalReducedMotion
import kotlin.math.sin
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun RecorderScreen(
    device: Device,
    state: CoreState,
    core: Core,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    val initialSession = remember { RecorderService.currentSession() }
    val session by remember {
        RecorderService.session.distinctUntilChanged { a, b ->
            a.active == b.active &&
                a.paused == b.paused &&
                a.pcId == b.pcId &&
                a.markers == b.markers &&
                (a.elapsedMs / 1000L) == (b.elapsedMs / 1000L)
        }
    }.collectAsStateWithLifecycle(initialValue = initialSession)
    val recordings by core.recordings.recordings.collectAsStateWithLifecycle()

    var hasMicPermission by remember { mutableStateOf(RecorderService.hasPermission(context)) }
    var startAfterGrant by remember { mutableStateOf(false) }
    val askMic = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        hasMicPermission = granted
        if (granted && startAfterGrant) {
            startAfterGrant = false
            RecorderService.start(context, device.id)
        } else {
            startAfterGrant = false
        }
    }

    var markerText by rememberSaveable { mutableStateOf("") }
    var playingId by remember { mutableStateOf<String?>(null) }
    val playerRef = remember { mutableStateOf<MediaPlayer?>(null) }

    fun stopPlayback() {
        playerRef.value?.let { mp ->
            runCatching { mp.stop() }
            runCatching { mp.release() }
        }
        playerRef.value = null
        playingId = null
    }

    fun togglePlayback(rec: SavedRecording) {
        if (playingId == rec.id) {
            stopPlayback()
            return
        }
        stopPlayback()
        val file = core.recordings.fileFor(rec)
        if (!file.exists()) return
        runCatching {
            val mp = MediaPlayer().apply {
                setDataSource(file.absolutePath)
                setOnCompletionListener {
                    runCatching { release() }
                    if (playerRef.value === this) {
                        playerRef.value = null
                        playingId = null
                    }
                }
                prepare()
                start()
            }
            playerRef.value = mp
            playingId = rec.id
        }
    }

    DisposableEffect(Unit) {
        onDispose { stopPlayback() }
    }

    val tints = g7Tints()
    val scheme = MaterialTheme.colorScheme
    LazyColumn(
        modifier = modifier.fillMaxSize(),
        contentPadding = PaddingValues(start = 10.dp, end = 10.dp, bottom = 16.dp),
    ) {
        item(key = "header", contentType = "header") {
            G7Header(
                title = stringResource(R.string.home_voice_note),
                onBack = onBack,
                note = stringResource(R.string.recorder_saves_to, device.name),
            )
        }

        if (!hasMicPermission) {
            item(key = "mic_permission", contentType = "row") {
                G7Row(label = stringResource(R.string.recorder_permission_title), icon = G7Icons.mic, tint = tints.red) {
                    G7RowAction(
                        label = stringResource(R.string.action_allow),
                        onClick = {
                            startAfterGrant = false
                            askMic.launch(Manifest.permission.RECORD_AUDIO)
                        },
                        accent = true,
                    )
                }
            }
        }

        if (!device.online) {
            item(key = "offline_notice", contentType = "row") {
                G7Row(
                    label = stringResource(R.string.transfer_waiting, device.name),
                    sublabel = stringResource(R.string.recorder_offline_hint, device.name),
                    icon = G7Icons.wifi,
                    tint = tints.muted,
                )
            }
        }

        // The time, the level, markers, and the controls
        item(key = "recorder_card", contentType = "recorder") {
            Column(
                modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 24.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Text(
                    text = when {
                        session.active && session.paused -> stringResource(R.string.recorder_status_paused)
                        session.active -> stringResource(R.string.recorder_status_recording, device.name)
                        else -> stringResource(R.string.recorder_status_ready, device.name)
                    },
                    style = MaterialTheme.typography.labelMedium,
                    color = scheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(16.dp))
                Text(
                    text = RecorderService.formatDuration(session.elapsedMs),
                    style = MaterialTheme.typography.displayMedium.copy(fontFamily = LocalAppFonts.current.mono),
                    maxLines = 1,
                    softWrap = false,
                )
                Spacer(Modifier.height(22.dp))
                LevelMeter(
                    active = session.active && !session.paused,
                    modifier = Modifier.fillMaxWidth().height(70.dp),
                )
                Spacer(Modifier.height(22.dp))

                if (session.active) {
                    val markerHint = stringResource(R.string.recorder_marker_hint)
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        OutlinedTextField(
                            value = markerText,
                            onValueChange = { markerText = it },
                            placeholder = { Text(markerHint) },
                            singleLine = true,
                            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                            keyboardActions = KeyboardActions(
                                onDone = {
                                    RecorderService.addMarker(markerText)
                                    markerText = ""
                                },
                            ),
                            modifier = Modifier
                                .weight(1f)
                                .semantics { contentDescription = markerHint },
                        )
                        G7RaisedButton(
                            label = stringResource(R.string.recorder_add_marker),
                            onClick = {
                                RecorderService.addMarker(markerText)
                                markerText = ""
                            },
                        )
                    }
                    if (session.markers.isNotEmpty()) {
                        Spacer(Modifier.height(12.dp))
                        FlowRow(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                            verticalArrangement = Arrangement.spacedBy(6.dp),
                        ) {
                            session.markers.forEachIndexed { idx, m ->
                                val ts = RecorderService.formatDuration(m.atMs.toLong())
                                val label = m.label ?: stringResource(R.string.recorder_marker_default, idx + 1)
                                // A label, not a button: markers aren't actionable here.
                                Text(
                                    text = stringResource(R.string.recorder_marker_chip, ts, label),
                                    style = MaterialTheme.typography.labelMedium,
                                    color = scheme.onSurfaceVariant,
                                    modifier = Modifier
                                        .background(scheme.surfaceContainer, RoundedCornerShape(8.dp))
                                        .padding(horizontal = 10.dp, vertical = 6.dp),
                                )
                            }
                        }
                    }
                    Spacer(Modifier.height(24.dp))
                }

                // Pause or record in the middle; Save (stop and send) at the right.
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceAround,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Spacer(Modifier.width(72.dp))
                    val bigLabel = stringResource(
                        when {
                            !session.active -> R.string.recorder_start
                            session.paused -> R.string.recorder_resume
                            else -> R.string.recorder_pause
                        },
                    )
                    Box(
                        modifier = Modifier
                            .size(72.dp)
                            .clip(CircleShape)
                            .background(tints.red)
                            .clickable(role = Role.Button, onClickLabel = bigLabel) {
                                when {
                                    !session.active -> {
                                        stopPlayback()
                                        if (RecorderService.hasPermission(context)) {
                                            hasMicPermission = true
                                            RecorderService.start(context, device.id)
                                        } else {
                                            startAfterGrant = true
                                            askMic.launch(Manifest.permission.RECORD_AUDIO)
                                        }
                                    }
                                    session.paused -> RecorderService.resume(context)
                                    else -> RecorderService.pause(context)
                                }
                            }
                            .semantics { contentDescription = bigLabel },
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(
                            imageVector = if (session.active && !session.paused) G7Icons.pause else G7Icons.mic,
                            contentDescription = null,
                            tint = Color(0xFF2A1111),
                            modifier = Modifier.size(26.dp),
                        )
                    }
                    Box(Modifier.width(72.dp), contentAlignment = Alignment.Center) {
                        if (session.active) {
                            Text(
                                stringResource(R.string.recorder_save),
                                style = MaterialTheme.typography.labelLarge,
                                color = scheme.primary,
                                modifier = Modifier
                                    .clip(RoundedCornerShape(8.dp))
                                    .clickable(role = Role.Button) { RecorderService.stop(context) }
                                    .padding(12.dp),
                            )
                        }
                    }
                }
            }
        }

        // Saved recordings: play, send again, delete
        if (recordings.isNotEmpty()) {
            item(key = "saved_header", contentType = "section_header") {
                G7SectionLabel(stringResource(R.string.recorder_saved_title))
            }
            items(recordings, key = { it.id }, contentType = { "recording_row" }) { rec ->
                RecordingRow(
                    recording = rec,
                    pcName = state.nameOf(rec.targetPcId ?: device.id) ?: device.name,
                    playing = playingId == rec.id,
                    onPlayToggle = { togglePlayback(rec) },
                    onSend = { core.sendSavedRecording(device.id, rec.id) },
                    onDelete = {
                        if (playingId == rec.id) stopPlayback()
                        core.recordings.delete(rec.id)
                    },
                )
            }
        }
    }
}

@Composable
private fun LevelMeter(
    active: Boolean,
    modifier: Modifier = Modifier,
) {
    val rawLevel by remember {
        RecorderService.session.map { if (it.active && !it.paused) it.level else 0f }.distinctUntilChanged()
    }.collectAsStateWithLifecycle(initialValue = 0f)
    val reducedMotion = LocalReducedMotion.current
    val animatedLevelState = animateFloatAsState(
        targetValue = if (active) rawLevel else 0f,
        animationSpec = if (reducedMotion) snap() else spring(),
        label = "micLevel",
    )
    val activeColor = g7Tints().red
    val inactiveColor = MaterialTheme.colorScheme.outlineVariant
    Canvas(modifier = modifier) {
        val level = animatedLevelState.value
        val barCount = 24
        val gap = 4.dp.toPx()
        val totalGaps = gap * (barCount - 1)
        val barWidth = ((size.width - totalGaps) / barCount).coerceAtLeast(2f)
        for (i in 0 until barCount) {
            val centerDist = kotlin.math.abs(i - (barCount - 1) / 2f) / (barCount / 2f)
            val envelope = (1f - centerDist * 0.55f)
            val wave = if (active) (0.18f + 0.82f * level * envelope + 0.06f * sin(i * 0.9f)).coerceIn(0.12f, 1f) else 0.12f
            val h = (size.height * wave).coerceAtLeast(4.dp.toPx())
            val x = i * (barWidth + gap)
            val y = (size.height - h) / 2f
            drawRoundRect(
                color = if (active && (i.toFloat() / barCount) <= (level * 1.25f + 0.15f)) activeColor else inactiveColor,
                topLeft = Offset(x, y),
                size = Size(barWidth, h),
                cornerRadius = CornerRadius(barWidth / 2f, barWidth / 2f),
            )
        }
    }
}

@Composable
private fun RecordingRow(
    recording: SavedRecording,
    pcName: String,
    playing: Boolean,
    onPlayToggle: () -> Unit,
    onSend: () -> Unit,
    onDelete: () -> Unit,
) {
    val context = LocalContext.current
    val title = recording.fileName.removeSuffix(".m4a")
    val durationText = RecorderService.formatDuration(recording.durationMs)
    val sizeText = Formatter.formatShortFileSize(context, recording.sizeBytes)
    val markersText = if (recording.markers.isEmpty()) {
        null
    } else {
        pluralStringResource(R.plurals.recorder_markers_count, recording.markers.size, recording.markers.size)
    }
    val statusText = when (recording.delivery) {
        DeliveryState.Waiting -> stringResource(R.string.transfer_waiting, pcName)
        DeliveryState.Sending -> stringResource(R.string.transfer_sending, pcName)
        DeliveryState.Sent -> stringResource(R.string.transfer_sent_to, pcName)
        DeliveryState.Denied -> stringResource(R.string.recorder_denied_text, pcName)
        DeliveryState.Failed -> stringResource(R.string.transfer_failed)
        DeliveryState.Idle -> null
    }

    val meta = listOfNotNull(durationText, sizeText, markersText, statusText).joinToString(" · ")
    G7Row(label = title, sublabel = meta, icon = G7Icons.mic, tint = g7Tints().red) {
        G7IconButton(
            icon = if (playing) G7Icons.stop else G7Icons.play,
            contentDescription = stringResource(if (playing) R.string.action_stop else R.string.media_play),
            onClick = onPlayToggle,
        )
        G7IconButton(
            icon = G7Icons.send,
            contentDescription = stringResource(
                if (recording.delivery == DeliveryState.Sent) R.string.recorder_send_again else R.string.recorder_send_now,
            ),
            onClick = onSend,
        )
        G7IconButton(
            icon = G7Icons.trash,
            contentDescription = stringResource(R.string.recorder_delete),
            onClick = onDelete,
            tint = g7Tints().red,
        )
    }
}
