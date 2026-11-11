// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.home

import app.g7link.android.ui.theme.G7Slider
import android.graphics.BitmapFactory
import android.net.Uri
import android.text.format.DateUtils
import android.text.format.Formatter
import android.util.LruCache
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.TextAutoSize
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.FilledTonalIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.path
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.isSpecified
import androidx.compose.ui.unit.sp
import app.g7link.android.G7LinkApplication
import app.g7link.android.R
import app.g7link.android.calls.PhoneCalls
import app.g7link.android.clipboard.SendActivity
import app.g7link.android.contacts.PhoneContacts
import app.g7link.android.core.BackgroundAccess
import app.g7link.android.core.CoreState
import app.g7link.android.core.Device
import app.g7link.android.core.PcFolder
import app.g7link.android.core.LocalNetwork
import app.g7link.android.core.WakeState
import app.g7link.android.core.isFinished
import app.g7link.android.files.ReceivedFiles
import app.g7link.android.files.transferTitle
import app.g7link.android.mirror.InputService
import app.g7link.android.mirror.MirrorSession
import app.g7link.android.notifications.NotificationListener
import app.g7link.android.photos.RecentPhotos
import app.g7link.android.recorder.RecorderSession
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.g7link.android.sms.PhoneSms
import app.g7link.android.ui.guide.GuideHighlightTarget
import app.g7link.android.ui.theme.LocalGlassTheme
import app.g7link.android.ui.theme.LocalReducedMotion
import app.g7link.android.ui.theme.bentoWell
import app.g7link.android.ui.theme.glassModalBlur
import app.g7link.android.ui.theme.glassModalContainerColor
import app.g7link.android.ui.theme.glassModalScrimColor
import app.g7link.android.ui.theme.liquidGlassCard
import app.g7link.android.update.AppUpdater
import app.g7link.android.update.UpdateCard
import app.g7link.android.webcam.WebcamSession
import android.app.DownloadManager
import android.content.Intent
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.FilterChip
import androidx.compose.runtime.LaunchedEffect
import androidx.core.content.FileProvider
import app.g7link.android.clipboard.PhoneClipboard
import app.g7link.android.core.QueueItemUi
import app.g7link.android.core.formatQueueItemTitle
import app.g7link.core.AudioOutputDevice
import app.g7link.core.ClipboardHistoryEntry
import app.g7link.core.ClipboardItemKind
import app.g7link.core.DeckState
import app.g7link.core.FfiQueueKind
import app.g7link.core.FfiQueueStatus
import app.g7link.core.Link
import app.g7link.core.LocalSendPeer
import app.g7link.core.RemoteStorageEntry
import app.g7link.core.TimelineEntry
import app.g7link.core.TimelinePage
import app.g7link.core.TimelineKind
import app.g7link.core.Transfer
import app.g7link.core.TransferDirection
import app.g7link.core.TransferStatus
import java.io.File
import java.text.DateFormat
import java.util.Calendar
import java.util.Date
import kotlin.math.roundToInt
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import app.g7link.android.core.PHONE_SPEAKER_OUTPUT
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.FirstLineIcon
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7Mark
import app.g7link.android.ui.theme.G7Row
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.G7SectionLabel
import app.g7link.android.ui.theme.G7SwitchRow
import app.g7link.android.ui.theme.LocalQuietTheme
import app.g7link.android.ui.theme.g7Tints
import app.g7link.android.ui.walkthrough.AccessRows
import app.g7link.android.ui.walkthrough.rememberAccessItems

/**
 * Home: the paired PC's name and connection, then rows to send to it, control
 * it, and use this phone with it. Anything waiting on you comes first.
 */
@Composable
fun HomeScreen(
    state: CoreState,
    onRing: (id: String, on: Boolean) -> Unit,
    onStopRinging: () -> Unit,
    onPairNew: () -> Unit,
    onRefresh: () -> Unit,
    onPower: (pcId: String, sleep: Boolean) -> Unit,
    onWake: (pcId: String) -> Unit,
    onSetReconnect: (pcId: String, on: Boolean) -> Unit,
    onRemote: (pcId: String) -> Unit,
    onDeck: (pcId: String) -> Unit,
    onDesk: (pcId: String) -> Unit = {},
    onRecord: (pcId: String) -> Unit,
    onWebcam: (pcId: String) -> Unit = {},
    updater: AppUpdater,
    onSendFiles: (pcId: String, uris: List<Uri>) -> Unit,
    onSendFolder: (pcId: String, tree: Uri) -> Unit,
    onAccessChanged: () -> Unit,
    onCancelTransfer: (id: String) -> Unit,
    onPauseTransfer: (id: String) -> Unit = {},
    onResumeTransfer: (id: String) -> Unit = {},
    onAcceptTransfer: (id: String) -> Unit = {},
    onCancelQueueItem: (id: String) -> Unit = {},
    onCancelAllQueueItems: () -> Unit = {},
    onRetryQueueItem: (id: String) -> Unit = {},
    onClearFinishedQueueItems: () -> Unit = {},
    onRefreshLocalSend: () -> Unit = {},
    onAllowStorage: (pcId: String) -> Unit = {},
    onDismissStorageRequest: () -> Unit = {},
    onAcceptWebcamRequest: (app.g7link.android.webcam.WebcamRequest) -> Unit = {},
    onDismissWebcamRequest: () -> Unit = {},
    onDeclineMicRequest: (pcId: String) -> Unit = {},
    onStartMic: (pcId: String) -> Unit = {},
    onStopMic: () -> Unit = {},
    onCopyClipboardHistory: (id: String) -> Unit = {},
    onPinClipboardHistory: (id: String, pinned: Boolean) -> Unit = { _, _ -> },
    onDeleteClipboardHistory: (id: String) -> Unit = {},
    onClearClipboardHistory: () -> Unit = {},
    loadClipboardHistoryImage: suspend (id: String) -> ByteArray? = { null },
    queryTimelinePage: suspend (kind: TimelineKind?, deviceId: String?, search: String?, offset: UInt) -> TimelinePage? = { _, _, _, _ -> null },
    onDeleteTimelineEntry: (id: Long) -> Unit = {},
    onClearTimeline: () -> Unit = {},
    onResendTimelineEntry: (entry: TimelineEntry) -> Unit = {},
    onSetPcAudio: (pcId: String, volume: Int?, muted: Boolean?) -> Unit = { _, _, _ -> },
    onStopKeyboardFromPc: (pcId: String) -> Unit = {},
    onListPcFiles: (pcId: String, path: String, onResult: (PcFolder) -> Unit) -> Unit = { _, _, onResult ->
        onResult(PcFolder(emptyList()))
    },
    onDownloadPcFile: (pcId: String, remotePath: String, fileName: String, onSaved: (Uri?) -> Unit) -> Unit = { _, _, _, onSaved ->
        onSaved(null)
    },
    onOpenLink: (pcId: String, url: String) -> Unit = { _, _ -> },
    mirrorSession: MirrorSession = MirrorSession(),
    webcamSession: WebcamSession = WebcamSession(),
    recorderSession: RecorderSession = RecorderSession(),
    /** The PC this phone's microphone streams to, if any. */
    micStreamingTo: String? = null,
    onStopMirror: () -> Unit = {},
    onOpenWebcam: (pcId: String) -> Unit = {},
    onStopWebcam: () -> Unit = {},
    onOpenRecorder: (pcId: String) -> Unit = {},
    onStopRecorder: () -> Unit = {},
    showWhatsNewGuideBanner: Boolean = false,
    onDismissWhatsNewGuide: () -> Unit = {},
    showHiveInvite: Boolean = false,
    onTryHive: () -> Unit = {},
    onDismissHiveInvite: () -> Unit = {},
    backgroundServiceEnabled: Boolean = true,
    onTurnOnBackgroundService: () -> Unit = {},
    onOpenFeaturesGuide: () -> Unit = {},
    onOpenWalkthrough: () -> Unit = {},
    highlightTarget: GuideHighlightTarget? = null,
    onHighlightConsumed: () -> Unit = {},
    modifier: Modifier = Modifier,
) {
    var showClipboardHistory by remember { mutableStateOf(false) }
    var showTimeline by remember { mutableStateOf(false) }
    var browseFilesPcId by remember { mutableStateOf<String?>(null) }
    var linkPcId by remember { mutableStateOf<String?>(null) }
    var pulsingTarget by remember { mutableStateOf<GuideHighlightTarget?>(null) }
    var accessOpen by rememberSaveable { mutableStateOf(false) }
    var selectedPcId by rememberSaveable { mutableStateOf<String?>(null) }
    // Keep a screen of rows composed on each side, so scrolling back does not rebuild them.
    @OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)
    val listState = androidx.compose.foundation.lazy.rememberLazyListState(
        cacheWindow = androidx.compose.foundation.lazy.layout.LazyLayoutCacheWindow(aheadFraction = 1f, behindFraction = 1f),
    )
    val isGlass = LocalGlassTheme.current
    val quiet = LocalQuietTheme.current
    val tints = g7Tints()
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val anyModalOpen = browseFilesPcId != null || showClipboardHistory || showTimeline || linkPcId != null

    val pcs = state.devices.filter { !it.isAndroid }
    val peers = state.devices.filter { it.isAndroid }
    val pc = pcs.firstOrNull { it.id == selectedPcId } ?: pcs.firstOrNull { it.online } ?: pcs.firstOrNull()
    val currentPcId by rememberUpdatedState(pc?.id)
    val pickFiles = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        val id = currentPcId
        if (id != null && uris.isNotEmpty()) onSendFiles(id, uris)
    }
    val pickFolder = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { tree ->
        val id = currentPcId
        if (id != null && tree != null) onSendFolder(id, tree)
    }
    var ringing by remember(pc?.id) { mutableStateOf(false) }

    // Storage is optional and has its own row in Settings, so it doesn't count here.
    val accessItems = rememberAccessItems(state, onAccessChanged).filter { it.id != "storage" }
    val missingAccess = accessItems.filterNot { it.granted }
    val missingCount = missingAccess.size + if (state.elevated) 0 else 1

    LaunchedEffect(highlightTarget) {
        val target = highlightTarget ?: return@LaunchedEffect
        pulsingTarget = target
        onHighlightConsumed()
        when (target) {
            GuideHighlightTarget.HOME_CLIPBOARD_HISTORY ->
                if (state.clipboardHistory.isEmpty() && state.clipboardHistoryEnabled) showClipboardHistory = true
            GuideHighlightTarget.HOME_TIMELINE -> if (state.timeline.isEmpty()) showTimeline = true
            GuideHighlightTarget.HOME_SETUP_PERMISSIONS -> accessOpen = true
            else -> Unit
        }
        delay(60)
        val targetKey = when (target) {
            GuideHighlightTarget.HOME_PC_CARD -> if (pc != null) "pc_send" else "home_header"
            GuideHighlightTarget.HOME_PC_AUDIO -> if (pc != null) "pc_control" else "home_header"
            GuideHighlightTarget.HOME_SEND_QUEUE -> if (state.sendQueue.isNotEmpty()) "send_queue" else "pc_send"
            GuideHighlightTarget.HOME_SETUP_PERMISSIONS -> "permissions_setup_card"
            GuideHighlightTarget.HOME_LOCALSEND -> "localsend_card"
            GuideHighlightTarget.HOME_CLIPBOARD_HISTORY,
            GuideHighlightTarget.HOME_TIMELINE,
            -> "activity"
            else -> "home_header"
        }
        val visibleMatch = listState.layoutInfo.visibleItemsInfo.firstOrNull { it.key == targetKey }
        runCatching { listState.animateScrollToItem(visibleMatch?.index ?: 0) }
        delay(2200)
        if (pulsingTarget == target) pulsingTarget = null
    }

    LazyColumn(
        state = listState,
        modifier = modifier.glassModalBlur(isGlass && anyModalOpen),
        contentPadding = androidx.compose.foundation.layout.PaddingValues(
            start = 10.dp,
            top = 8.dp,
            end = 10.dp,
            bottom = if (quiet) 16.dp else 104.dp,
        ),
    ) {
        item(key = "home_header", contentType = "header") {
            HomeHeader(
                pc = pc,
                pcs = pcs,
                highlighted = pulsingTarget == GuideHighlightTarget.HOME_PAIR_PC,
                onSelectPc = { selectedPcId = it },
                onPairNew = onPairNew,
                onRefresh = onRefresh,
                onSetReconnect = onSetReconnect,
            )
        }
        if (pc != null && !pc.online && pc.wakeState == WakeState.Waking) {
            item(key = "waking", contentType = "notice") {
                if (LocalReducedMotion.current) {
                    LinearProgressIndicator(progress = { 0.5f }, modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp))
                } else {
                    LinearProgressIndicator(modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp))
                }
            }
        }
        if (showWhatsNewGuideBanner) {
            item(key = "whats_new_guide_banner", contentType = "notice") {
                NoticeRow(
                    title = stringResource(R.string.home_whats_new_guide),
                    icon = G7Icons.guide,
                    tint = tints.lavender,
                    actionLabel = stringResource(R.string.clip_action_open),
                    onAction = {
                        onDismissWhatsNewGuide()
                        onOpenFeaturesGuide()
                    },
                    onDismiss = onDismissWhatsNewGuide,
                )
            }
        }
        // After the What's new notice, one at a time.
        if (showHiveInvite && !showWhatsNewGuideBanner) {
            item(key = "hive_invite", contentType = "notice") {
                NoticeRow(
                    title = stringResource(R.string.hive_invite_title),
                    icon = G7Icons.palette,
                    tint = tints.amber,
                    actionLabel = stringResource(R.string.hive_invite_try),
                    onAction = onTryHive,
                    onDismiss = onDismissHiveInvite,
                )
            }
        }
        state.ringingFrom?.let { from ->
            item(key = "ringing_banner", contentType = "notice") {
                NoticeRow(
                    title = if (from.isEmpty()) {
                        stringResource(R.string.ringing_title_unknown)
                    } else {
                        stringResource(R.string.ringing_title, from)
                    },
                    icon = G7Icons.ring,
                    tint = tints.red,
                    actionLabel = stringResource(R.string.action_stop_ringing),
                    onAction = onStopRinging,
                )
            }
        }
        if (mirrorSession.active) {
            item(key = "mirror_active_banner", contentType = "notice") {
                val pcName = mirrorSession.pcId?.let { state.nameOf(it) } ?: stringResource(R.string.your_pc)
                val controlReady = state.inputAccess || state.elevated
                NoticeRow(
                    title = stringResource(
                        if (mirrorSession.soundOnly) R.string.mirror_sharing_sound else R.string.mirror_sharing,
                        pcName,
                    ),
                    text = when {
                        mirrorSession.soundOnly -> stringResource(R.string.mirror_banner_subtitle_sound)
                        controlReady -> stringResource(R.string.mirror_banner_subtitle_control_on)
                        mirrorSession.audio -> stringResource(R.string.mirror_banner_subtitle_audio)
                        else -> stringResource(R.string.mirror_banner_subtitle)
                    },
                    icon = G7Icons.screen,
                    tint = tints.pink,
                    actionLabel = stringResource(R.string.action_stop),
                    onAction = onStopMirror,
                    // Nothing to control without the screen.
                    secondaryLabel = if (!controlReady && !mirrorSession.soundOnly) {
                        stringResource(R.string.mirror_allow_pc_control)
                    } else {
                        null
                    },
                    onSecondary = { runCatching { context.startActivity(InputService.settingsIntent()) } },
                )
            }
        }
        if (webcamSession.active) {
            item(key = "webcam_active_banner", contentType = "notice") {
                val pcName = webcamSession.pcId?.let { state.nameOf(it) } ?: stringResource(R.string.your_pc)
                val cameraLabel = stringResource(
                    if (webcamSession.camera == "front") R.string.webcam_camera_front else R.string.webcam_camera_back,
                )
                NoticeRow(
                    title = stringResource(R.string.webcam_notification_streaming, pcName),
                    text = stringResource(
                        R.string.webcam_notification_detail,
                        webcamSession.width,
                        webcamSession.height,
                        cameraLabel,
                    ),
                    icon = G7Icons.camera,
                    tint = tints.coral,
                    actionLabel = stringResource(R.string.webcam_stop),
                    onAction = onStopWebcam,
                    secondaryLabel = webcamSession.pcId?.let { stringResource(R.string.action_open_controls) },
                    onSecondary = { webcamSession.pcId?.let(onOpenWebcam) },
                )
            }
        }
        if (recorderSession.active) {
            item(key = "recorder_active_banner", contentType = "notice") {
                val pcName = recorderSession.pcId?.let { state.nameOf(it) } ?: stringResource(R.string.your_pc)
                NoticeRow(
                    title = stringResource(
                        if (recorderSession.paused) {
                            R.string.recorder_notification_paused
                        } else {
                            R.string.recorder_notification_recording
                        },
                        pcName,
                    ),
                    text = DateUtils.formatElapsedTime(recorderSession.elapsedMs / 1000L),
                    icon = G7Icons.mic,
                    tint = tints.red,
                    actionLabel = stringResource(R.string.action_stop),
                    onAction = onStopRecorder,
                    secondaryLabel = recorderSession.pcId?.let { stringResource(R.string.timeline_action_open) },
                    onSecondary = { recorderSession.pcId?.let(onOpenRecorder) },
                )
            }
        }
        micStreamingTo?.let { pcId ->
            item(key = "mic_active_banner", contentType = "notice") {
                NoticeRow(
                    title = stringResource(R.string.mic_notification_title, state.nameOf(pcId) ?: stringResource(R.string.your_pc)),
                    icon = G7Icons.mic,
                    tint = tints.pink,
                    actionLabel = stringResource(R.string.action_stop),
                    onAction = onStopMic,
                )
            }
        }
        state.keyboardFromPc.forEach { pcId ->
            item(key = "keyboard_$pcId", contentType = "notice") {
                NoticeRow(
                    title = stringResource(R.string.keyboard_from_pc_banner, state.nameOf(pcId) ?: stringResource(R.string.your_pc)),
                    icon = G7Icons.keyboard,
                    tint = tints.pink,
                    actionLabel = stringResource(R.string.keyboard_stop),
                    onAction = { onStopKeyboardFromPc(pcId) },
                )
            }
        }
        state.webcamRequest?.let { req ->
            item(key = "webcam_request", contentType = "notice") {
                NoticeRow(
                    title = stringResource(R.string.webcam_request_title, state.nameOf(req.pcId) ?: stringResource(R.string.your_pc)),
                    icon = G7Icons.camera,
                    tint = tints.coral,
                    actionLabel = stringResource(R.string.webcam_start),
                    onAction = { onAcceptWebcamRequest(req) },
                    secondaryLabel = stringResource(R.string.action_not_now),
                    onSecondary = onDismissWebcamRequest,
                )
            }
        }
        state.micRequestFrom?.takeIf { it != micStreamingTo }?.let { pcId ->
            item(key = "mic_request", contentType = "notice") {
                NoticeRow(
                    title = stringResource(R.string.mic_request_title, state.nameOf(pcId) ?: stringResource(R.string.your_pc)),
                    icon = G7Icons.mic,
                    tint = tints.pink,
                    actionLabel = stringResource(R.string.mic_request_start),
                    // Asks for the microphone permission if needed, then starts.
                    onAction = { onStartMic(pcId) },
                    secondaryLabel = stringResource(R.string.action_not_now),
                    onSecondary = { onDeclineMicRequest(pcId) },
                )
            }
        }
        state.storageRequestedFrom?.let { pcId ->
            item(key = "storage_request", contentType = "notice") {
                NoticeRow(
                    title = stringResource(R.string.storage_request_title, state.nameOf(pcId) ?: stringResource(R.string.your_pc)),
                    icon = G7Icons.folder,
                    tint = tints.amber,
                    actionLabel = stringResource(R.string.action_allow),
                    onAction = {
                        onAllowStorage(pcId)
                        if (!state.storageAllFilesAccess && state.storageSafFolders.isEmpty()) {
                            runCatching {
                                context.startActivity(app.g7link.android.storage.PhoneStorage.allFilesAccessIntent(context))
                            }
                        }
                    },
                    secondaryLabel = stringResource(R.string.action_not_now),
                    onSecondary = onDismissStorageRequest,
                )
            }
        }
        item(key = "update_card", contentType = "update") { UpdateCard(updater) }
        if (!backgroundServiceEnabled) {
            item(key = "background_connection_off", contentType = "notice") {
                NoticeRow(
                    title = stringResource(R.string.home_background_off_title),
                    icon = G7Icons.bolt,
                    tint = tints.amber,
                    actionLabel = stringResource(R.string.action_turn_on),
                    onAction = onTurnOnBackgroundService,
                )
            }
        }
        if (!state.localNetwork) {
            item(key = "local_network_setup", contentType = "notice") {
                val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
                    onAccessChanged()
                    if (!granted) openAppDetailsSettings(context)
                }
                NoticeRow(
                    title = stringResource(R.string.local_network_title),
                    icon = G7Icons.wifi,
                    tint = tints.blue,
                    actionLabel = stringResource(R.string.action_allow),
                    onAction = { ask.launch(LocalNetwork.PERMISSION) },
                )
            }
        }
        if (state.remoteInputRequested && !state.inputAccess && !state.elevated) {
            item(key = "remote_input_requested", contentType = "notice") {
                NoticeRow(
                    title = stringResource(R.string.remote_input_requested_title),
                    icon = G7Icons.pointer,
                    tint = tints.pink,
                    actionLabel = stringResource(R.string.action_allow),
                    onAction = { runCatching { context.startActivity(InputService.settingsIntent()) } },
                )
            }
        }
        if (pc != null && !pc.online && pc.wakeState == WakeState.TimedOut) {
            item(key = "wake_timeout", contentType = "notice") {
                NoticeRow(
                    title = stringResource(R.string.wake_timeout_title, pc.name),
                    text = stringResource(R.string.wake_timeout_text, pc.name),
                    icon = G7Icons.power,
                    tint = tints.red,
                    actionLabel = stringResource(R.string.action_wake_pc),
                    onAction = { onWake(pc.id) },
                )
            }
        }

        if (pc == null) {
            item(key = "no_paired_pc_card", contentType = "group") {
                Column(Modifier.padding(top = 8.dp)) {
                    G7Row(
                        label = stringResource(R.string.action_pair_with_pc),
                        icon = G7Icons.plus,
                        tint = tints.lavender,
                        highlighted = pulsingTarget == GuideHighlightTarget.HOME_PAIR_PC ||
                            pulsingTarget == GuideHighlightTarget.HOME_PC_CARD,
                        onClick = onPairNew,
                    )
                    G7Row(
                        label = stringResource(R.string.guide_open_walkthrough),
                        icon = G7Icons.info,
                        tint = tints.muted,
                        onClick = onOpenWalkthrough,
                    )
                }
            }
        } else {
            val online = pc.online
            item(key = "pc_send", contentType = "group") {
                val pulsing = pulsingTarget == GuideHighlightTarget.HOME_PC_CARD
                Column {
                    G7SectionLabel(stringResource(R.string.home_send_to, pc.name))
                    G7Row(
                        label = stringResource(R.string.home_files),
                        icon = G7Icons.upload,
                        tint = tints.amber,
                        enabled = !online || pc.has("files.send"),
                        highlighted = pulsing,
                        onClick = { pickFiles.launch(arrayOf("*/*")) },
                    )
                    G7Row(
                        label = stringResource(R.string.home_folder),
                        icon = G7Icons.folder,
                        tint = tints.amber,
                        enabled = !online || pc.has("files.send"),
                        onClick = { pickFolder.launch(null) },
                    )
                    G7Row(
                        label = stringResource(R.string.home_clipboard),
                        icon = G7Icons.clipboard,
                        tint = tints.lavender,
                        // Offline, the clip waits in the send queue.
                        enabled = !online || pc.usable("clipboard.auto_phone_to_pc"),
                        onClick = { context.startActivity(SendActivity.sendClipboardIntent(context, pc.id)) },
                    )
                    G7Row(
                        label = stringResource(R.string.home_link),
                        icon = G7Icons.link,
                        tint = tints.blue,
                        enabled = !online || pc.has("device.links_to_pc"),
                        onClick = { linkPcId = pc.id },
                    )
                }
            }
            item(key = "pc_control", contentType = "group") {
                Column {
                    G7SectionLabel(stringResource(R.string.home_control, pc.name))
                    if (!online && !pc.reconnect) {
                        G7Row(
                            label = stringResource(R.string.action_reconnect),
                            icon = G7Icons.refresh,
                            tint = tints.teal,
                            onClick = { onSetReconnect(pc.id, true) },
                        )
                    } else if (!online && pc.canWake && pc.wakeState != WakeState.Waking) {
                        G7Row(
                            label = stringResource(R.string.action_wake_pc),
                            icon = G7Icons.power,
                            tint = tints.red,
                            onClick = { onWake(pc.id) },
                        )
                    }
                    G7Row(
                        label = stringResource(R.string.home_trackpad),
                        icon = G7Icons.pointer,
                        tint = tints.teal,
                        enabled = online,
                        onClick = { onRemote(pc.id) },
                    )
                    G7Row(
                        label = stringResource(R.string.action_deck_pc),
                        icon = G7Icons.deck,
                        tint = tints.green,
                        enabled = online,
                        onClick = { onDeck(pc.id) },
                    )
                    G7Row(
                        label = stringResource(R.string.action_browse_pc_files),
                        icon = G7Icons.folder,
                        tint = tints.amber,
                        enabled = online,
                        onClick = { browseFilesPcId = pc.id },
                    )
                    G7Row(
                        label = stringResource(if (ringing) R.string.action_stop_ringing else R.string.action_ring_device),
                        icon = G7Icons.bell,
                        tint = tints.blue,
                        enabled = online,
                        onClick = {
                            ringing = !ringing
                            onRing(pc.id, ringing)
                        },
                    )
                    if (pc.has("device.pc_actions")) {
                        G7Row(
                            label = stringResource(R.string.home_lock),
                            icon = G7Icons.lock,
                            tint = tints.teal,
                            enabled = online,
                            onClick = { onPower(pc.id, false) },
                        )
                        G7Row(
                            label = stringResource(R.string.action_sleep_pc),
                            icon = G7Icons.moon,
                            tint = tints.red,
                            enabled = online,
                            onClick = { onPower(pc.id, true) },
                        )
                    }
                    val deckState = pc.deckState
                    if (online && deckState != null) {
                        PcSoundRows(
                            pcId = pc.id,
                            pcName = pc.name,
                            deckState = deckState,
                            highlighted = pulsingTarget == GuideHighlightTarget.HOME_PC_AUDIO,
                            onSetPcAudio = { volume, muted -> onSetPcAudio(pc.id, volume, muted) },
                        )
                    }
                }
            }
            item(key = "pc_phone", contentType = "group") {
                val micOn = micStreamingTo == pc.id
                Column {
                    G7SectionLabel(stringResource(R.string.home_this_phone))
                    G7Row(
                        label = stringResource(R.string.action_webcam_pc),
                        icon = G7Icons.camera,
                        tint = tints.coral,
                        enabled = online,
                        onClick = { onWebcam(pc.id) },
                    )
                    G7Row(
                        label = stringResource(R.string.home_voice_note),
                        icon = G7Icons.mic,
                        tint = tints.red,
                        enabled = online,
                        onClick = { onRecord(pc.id) },
                    )
                    G7Row(
                        label = stringResource(R.string.home_desk_mode),
                        icon = G7Icons.clock,
                        tint = tints.lavender,
                        enabled = online,
                        onClick = { onDesk(pc.id) },
                    )
                    if (online && (micOn || pc.has("media.phone_mic"))) {
                        G7SwitchRow(
                            label = stringResource(R.string.pc_audio_use_phone_mic),
                            checked = micOn,
                            onCheckedChange = { on -> if (on) onStartMic(pc.id) else onStopMic() },
                            icon = G7Icons.mic,
                            tint = tints.pink,
                        )
                    }
                }
            }
        }

        if (state.devices.isNotEmpty() && missingCount > 0) {
            item(key = "permissions_setup_card", contentType = "group") {
                Column {
                    G7Row(
                        label = stringResource(R.string.home_access),
                        icon = G7Icons.shield,
                        tint = tints.amber,
                        note = missingCount.toString(),
                        highlighted = pulsingTarget == GuideHighlightTarget.HOME_SETUP_PERMISSIONS,
                        onClick = { accessOpen = !accessOpen },
                    ) {
                        Icon(
                            imageVector = G7Icons.chevronDown,
                            contentDescription = null,
                            tint = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.size(18.dp).graphicsLayer { rotationZ = if (accessOpen) 180f else 0f },
                        )
                    }
                    if (accessOpen) {
                        AccessRows(missingAccess)
                        if (!state.elevated) {
                            G7Row(
                                label = stringResource(R.string.home_background_clipboard_title),
                                icon = G7Icons.bolt,
                                tint = tints.amber,
                            ) {
                                G7RowAction(
                                    label = stringResource(R.string.elevated_set_up),
                                    onClick = {
                                        scope.launch {
                                            app.g7link.android.elevated.PairingNotification.show(context)
                                            if (!app.g7link.android.elevated.Elevated.start()) {
                                                openDeveloperOptions(context)
                                            }
                                        }
                                    },
                                )
                            }
                        }
                    }
                }
            }
        }

        // Another phone or tablet paired with this one.
        items(peers, key = { it.id }, contentType = { "peer" }) { device ->
            PeerRows(device = device, onRing = onRing, onSendFiles = onSendFiles)
        }

        val showClipRow = state.clipboardHistoryEnabled && state.clipboardHistory.isNotEmpty()
        val showTimelineRow = state.timeline.isNotEmpty()
        if (showClipRow || showTimelineRow || state.sendQueue.isNotEmpty() || state.transfers.isNotEmpty() || state.localsendEnabled) {
            item(key = "activity", contentType = "group") {
                Column {
                    G7SectionLabel(stringResource(R.string.home_activity))
                    if (showClipRow) {
                        G7Row(
                            label = stringResource(R.string.clipboard_history_title),
                            icon = G7Icons.clipboard,
                            tint = tints.lavender,
                            note = state.clipboardHistory.size.toString(),
                            highlighted = pulsingTarget == GuideHighlightTarget.HOME_CLIPBOARD_HISTORY,
                            onClick = { showClipboardHistory = true },
                        )
                    }
                    if (showTimelineRow) {
                        G7Row(
                            label = stringResource(R.string.timeline_title),
                            icon = G7Icons.history,
                            tint = tints.blue,
                            note = state.timelineTotal.takeIf { it > 0u }?.toString(),
                            highlighted = pulsingTarget == GuideHighlightTarget.HOME_TIMELINE,
                            onClick = { showTimeline = true },
                        )
                    }
                }
            }
        }
        if (state.sendQueue.isNotEmpty()) {
            item(key = "send_queue", contentType = "send_queue") {
                SendQueueCard(
                    state = state,
                    onCancel = onCancelQueueItem,
                    onCancelAll = onCancelAllQueueItems,
                    onRetry = onRetryQueueItem,
                    onClearFinished = onClearFinishedQueueItems,
                )
            }
        }
        if (state.transfers.isNotEmpty()) {
            item(key = "transfers_card", contentType = "transfers") {
                TransfersCard(
                    state = state,
                    onAccept = onAcceptTransfer,
                    onCancel = onCancelTransfer,
                    onPause = onPauseTransfer,
                    onResume = onResumeTransfer,
                )
            }
        }
        if (state.localsendEnabled) {
            item(key = "localsend_card", contentType = "localsend") {
                LocalSendCard(
                    peers = state.localsendPeers,
                    onRefresh = onRefreshLocalSend,
                    onSendFiles = onSendFiles,
                )
            }
        }
    }

    val browseDevice = browseFilesPcId?.let { state.device(it) }
    if (browseDevice != null) {
        PcFilesSheet(
            device = browseDevice,
            onListFiles = { path, onResult -> onListPcFiles(browseDevice.id, path, onResult) },
            onDownloadFile = { remotePath, fileName, onSaved ->
                onDownloadPcFile(browseDevice.id, remotePath, fileName, onSaved)
            },
            onDismiss = { browseFilesPcId = null },
        )
    }

    linkPcId?.let { id ->
        LinkDialog(
            pcName = state.nameOf(id) ?: stringResource(R.string.your_pc),
            onOpen = { url ->
                linkPcId = null
                onOpenLink(id, url)
            },
            onDismiss = { linkPcId = null },
        )
    }

    if (showClipboardHistory && state.clipboardHistoryEnabled) {
        ClipboardHistoryDialog(
            entries = state.clipboardHistory,
            onCopy = onCopyClipboardHistory,
            onPin = onPinClipboardHistory,
            onDelete = onDeleteClipboardHistory,
            onClearAll = onClearClipboardHistory,
            loadImage = loadClipboardHistoryImage,
            onDismiss = { showClipboardHistory = false },
        )
    }

    if (showTimeline) {
        TimelineDialog(
            state = state,
            queryPage = queryTimelinePage,
            onCopyClip = onCopyClipboardHistory,
            onResend = onResendTimelineEntry,
            onDelete = onDeleteTimelineEntry,
            onClearAll = onClearTimeline,
            loadClipImage = loadClipboardHistoryImage,
            onDismiss = { showTimeline = false },
        )
    }
}

/**
 * The G7 mark, the PC's name and its connection. The chevron switches PC,
 * pairs another, refreshes, and connects or disconnects.
 */
@Composable
private fun HomeHeader(
    pc: Device?,
    pcs: List<Device>,
    highlighted: Boolean,
    onSelectPc: (String) -> Unit,
    onPairNew: () -> Unit,
    onRefresh: () -> Unit,
    onSetReconnect: (String, Boolean) -> Unit,
) {
    var menuOpen by remember { mutableStateOf(false) }
    val scheme = MaterialTheme.colorScheme
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(androidx.compose.foundation.shape.RoundedCornerShape(10.dp))
            .then(if (highlighted) Modifier.background(scheme.surfaceContainerHigh) else Modifier)
            .padding(start = 10.dp, top = 12.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        G7Mark()
        Column(Modifier.weight(1f)) {
            Text(
                pc?.name ?: stringResource(R.string.app_name),
                style = MaterialTheme.typography.titleSmall,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                if (pc == null) stringResource(R.string.home_no_pc_paired) else pcStatusLine(pc),
                style = MaterialTheme.typography.bodySmall,
                color = scheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Box {
            G7IconButton(
                icon = G7Icons.chevronDown,
                contentDescription = stringResource(R.string.home_switch_pc),
                onClick = { menuOpen = true },
                tint = scheme.onSurfaceVariant,
            )
            DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
                pcs.filter { it.id != pc?.id }.forEach { other ->
                    DropdownMenuItem(
                        text = { Text(other.name) },
                        onClick = {
                            menuOpen = false
                            onSelectPc(other.id)
                        },
                    )
                }
                if (pc != null) {
                    val label = when {
                        !pc.reconnect -> R.string.action_reconnect
                        pc.online -> R.string.action_disconnect
                        else -> R.string.action_stop_reconnecting
                    }
                    DropdownMenuItem(
                        text = { Text(stringResource(label)) },
                        onClick = {
                            menuOpen = false
                            // Stays paired, and stays disconnected until Reconnect (GitHub #48).
                            onSetReconnect(pc.id, !pc.reconnect)
                        },
                    )
                }
                DropdownMenuItem(
                    text = { Text(stringResource(R.string.action_refresh)) },
                    onClick = {
                        menuOpen = false
                        onRefresh()
                    },
                )
                DropdownMenuItem(
                    text = { Text(stringResource(if (pcs.isEmpty()) R.string.action_pair_with_pc else R.string.action_pair_new)) },
                    onClick = {
                        menuOpen = false
                        onPairNew()
                    },
                )
            }
        }
    }
}

/** "USB · 4 ms · Ubuntu 26.04 · 80%": how the PC is connected, what it is, its battery. */
@Composable
private fun pcStatusLine(pc: Device): String {
    val link = when {
        !pc.online && pc.wakeState == WakeState.Waking -> stringResource(R.string.wake_waking, pc.name)
        !pc.online && !pc.reconnect -> stringResource(R.string.link_not_reconnecting)
        else -> linkText(pc.link)
    }
    val battery = pc.battery?.let {
        if (it.charging) {
            stringResource(R.string.battery_charging, it.level.toInt())
        } else {
            stringResource(R.string.battery_level, it.level.toInt())
        }
    }
    return listOfNotNull(link, pc.model?.takeIf { it.isNotBlank() }, battery).joinToString(" · ")
}

/**
 * Something waiting on you: a call to stop, a request from the PC, a stream
 * running. Its actions sit under it, lined up with the text.
 */
@Composable
private fun NoticeRow(
    title: String,
    icon: ImageVector,
    tint: Color,
    text: String? = null,
    actionLabel: String? = null,
    onAction: (() -> Unit)? = null,
    secondaryLabel: String? = null,
    onSecondary: (() -> Unit)? = null,
    onDismiss: (() -> Unit)? = null,
) {
    Column(Modifier.fillMaxWidth()) {
        G7Row(label = title, sublabel = text, icon = icon, tint = tint) {
            if (onDismiss != null) {
                G7IconButton(
                    icon = G7Icons.close,
                    contentDescription = stringResource(R.string.action_dismiss),
                    onClick = onDismiss,
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        if ((actionLabel != null && onAction != null) || (secondaryLabel != null && onSecondary != null)) {
            Row(
                modifier = Modifier.padding(start = 48.dp, bottom = 8.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                if (actionLabel != null && onAction != null) {
                    G7RowAction(label = actionLabel, onClick = onAction, accent = true)
                }
                if (secondaryLabel != null && onSecondary != null) {
                    G7RowAction(label = secondaryLabel, onClick = onSecondary)
                }
            }
        }
    }
}

/** A paired phone or tablet: its name as the label, then what you can do with it. */
@Composable
private fun PeerRows(
    device: Device,
    onRing: (String, Boolean) -> Unit,
    onSendFiles: (String, List<Uri>) -> Unit,
) {
    val context = LocalContext.current
    val tints = g7Tints()
    val pickFiles = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isNotEmpty()) onSendFiles(device.id, uris)
    }
    var ringing by remember(device.id) { mutableStateOf(false) }
    val kind = stringResource(
        if (device.kind == app.g7link.core.DeviceKind.TABLET) R.string.peer_kind_tablet else R.string.peer_kind_phone,
    )
    val battery = device.battery?.let {
        if (it.charging) {
            stringResource(R.string.battery_charging, it.level.toInt())
        } else {
            stringResource(R.string.battery_level, it.level.toInt())
        }
    }
    Column {
        G7SectionLabel(listOfNotNull(device.name, kind, linkText(device.link), battery).joinToString(" · "))
        G7Row(
            label = stringResource(R.string.home_files),
            icon = G7Icons.upload,
            tint = tints.amber,
            enabled = !device.online || device.has("pair.files"),
            onClick = { pickFiles.launch(arrayOf("*/*")) },
        )
        G7Row(
            label = stringResource(R.string.home_clipboard),
            icon = G7Icons.clipboard,
            tint = tints.lavender,
            enabled = !device.online || device.has("pair.clipboard"),
            onClick = { context.startActivity(SendActivity.sendClipboardIntent(context, device.id)) },
        )
        G7Row(
            label = stringResource(if (ringing) R.string.action_stop_ringing else R.string.action_ring_device),
            icon = G7Icons.ring,
            tint = tints.blue,
            enabled = device.online && device.has("pair.ring"),
            onClick = {
                ringing = !ringing
                onRing(device.id, ringing)
            },
        )
    }
}

/** Opens a web link on the PC; starts with the link on the clipboard, if there's one. */
@Composable
private fun LinkDialog(pcName: String, onOpen: (String) -> Unit, onDismiss: () -> Unit) {
    val context = LocalContext.current
    var text by remember {
        mutableStateOf(
            (PhoneClipboard.read(context) as? PhoneClipboard.Read.Text)?.text?.let { SendActivity.handoffUrl(it) }.orEmpty(),
        )
    }
    val url = text.trim()
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = glassModalContainerColor(),
        title = { Text(stringResource(R.string.home_link_title, pcName)) },
        text = {
            androidx.compose.material3.OutlinedTextField(
                value = text,
                onValueChange = { text = it },
                singleLine = true,
                placeholder = { Text(stringResource(R.string.home_link_hint)) },
                keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(
                    keyboardType = androidx.compose.ui.text.input.KeyboardType.Uri,
                    imeAction = androidx.compose.ui.text.input.ImeAction.Go,
                ),
                keyboardActions = androidx.compose.foundation.text.KeyboardActions(
                    onGo = { if (url.isNotEmpty()) onOpen(url) },
                ),
                modifier = Modifier.fillMaxWidth(),
            )
        },
        confirmButton = {
            TextButton(enabled = url.isNotEmpty(), onClick = { onOpen(url) }) {
                Text(stringResource(R.string.clip_action_open))
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_cancel)) }
        },
    )
}

/**
 * The PC's sound under its controls: what plays, the volume, where it plays,
 * and whether it plays on this phone instead.
 */
@Composable
internal fun PcSoundRows(
    pcId: String,
    pcName: String,
    deckState: DeckState,
    onSetPcAudio: (volume: Int?, muted: Boolean?) -> Unit,
    highlighted: Boolean = false,
) {
    val context = LocalContext.current
    val tints = g7Tints()
    val scheme = MaterialTheme.colorScheme
    val pcMedia = remember(context) {
        (context.applicationContext as? G7LinkApplication)?.core?.pcMedia
    }
    val rawSummary = if (pcMedia != null) {
        val summary by pcMedia.summaryFlow.collectAsStateWithLifecycle()
        summary
    } else {
        null
    }
    val mediaSummary = rawSummary?.takeIf { it.pcId == pcId }
    var draggingVolume by remember { mutableStateOf<Float?>(null) }
    var lastSentVolume by remember { mutableStateOf(deckState.volume.toInt()) }
    if (draggingVolume == null) {
        lastSentVolume = deckState.volume.toInt()
    }
    var showOutputs by remember { mutableStateOf(false) }
    val shownVolume = draggingVolume?.roundToInt() ?: deckState.volume.toInt()
    val (streamingToPhone, hardwareOutputs, defaultOutput) = remember(deckState.outputDevices) {
        val streaming = deckState.outputDevices.any { it.id == PHONE_SPEAKER_OUTPUT }
        val hw = deckState.outputDevices.filterNot { it.id == PHONE_SPEAKER_OUTPUT }
        val def = hw.firstOrNull { it.isDefault } ?: hw.firstOrNull()
        Triple(streaming, hw, def)
    }
    val volumeLabel = stringResource(R.string.pc_audio_volume)

    if (mediaSummary != null) {
        G7Row(
            label = mediaSummary.title,
            sublabel = mediaSummary.subtitle.takeIf { it.isNotBlank() },
            icon = G7Icons.music,
            tint = tints.pink,
            highlighted = highlighted,
        ) {
            G7IconButton(
                icon = G7Icons.previous,
                contentDescription = stringResource(R.string.media_previous),
                onClick = { pcMedia?.pressed(app.g7link.core.MediaAction.PREVIOUS) },
            )
            G7IconButton(
                icon = if (mediaSummary.playing) G7Icons.pause else G7Icons.play,
                contentDescription = stringResource(if (mediaSummary.playing) R.string.media_pause else R.string.media_play),
                onClick = {
                    pcMedia?.pressed(
                        if (mediaSummary.playing) app.g7link.core.MediaAction.PAUSE else app.g7link.core.MediaAction.PLAY,
                    )
                },
            )
            G7IconButton(
                icon = G7Icons.next,
                contentDescription = stringResource(R.string.media_next),
                onClick = { pcMedia?.pressed(app.g7link.core.MediaAction.NEXT) },
            )
        }
    }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(androidx.compose.foundation.shape.RoundedCornerShape(10.dp))
            .then(if (highlighted && mediaSummary == null) Modifier.background(scheme.surfaceContainerHigh) else Modifier)
            .padding(start = 12.dp, end = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        val muteLabel = stringResource(if (deckState.muted) R.string.pc_audio_unmute else R.string.pc_audio_mute)
        Icon(
            imageVector = if (deckState.muted) G7Icons.volumeOff else G7Icons.volume,
            contentDescription = muteLabel,
            tint = tints.teal,
            modifier = Modifier
                .size(22.dp)
                .clickable(role = Role.Button, onClickLabel = muteLabel) { onSetPcAudio(null, !deckState.muted) },
        )
        G7Slider(
            value = (draggingVolume ?: deckState.volume.toFloat()).coerceIn(0f, 100f),
            onValueChange = { v ->
                draggingVolume = v
                val rounded = v.roundToInt()
                if (rounded != lastSentVolume) {
                    lastSentVolume = rounded
                    onSetPcAudio(rounded, null)
                }
            },
            onValueChangeFinished = { draggingVolume = null },
            valueRange = 0f..100f,
            tint = tints.teal,
            trackColor = scheme.surfaceContainerHigh,
            modifier = Modifier
                .weight(1f)
                .semantics { contentDescription = volumeLabel },
        )
        Text(
            if (deckState.muted) stringResource(R.string.pc_audio_muted) else shownVolume.toString(),
            style = MaterialTheme.typography.bodyMedium,
            color = scheme.onSurfaceVariant,
        )
    }
    if (defaultOutput != null) {
        G7Row(
            label = stringResource(R.string.pc_audio_output, defaultOutput.name),
            icon = G7Icons.volume,
            tint = tints.muted,
            note = hardwareOutputs.size.takeIf { it > 1 }?.toString(),
            onClick = { showOutputs = true },
        )
    }
    G7SwitchRow(
        label = stringResource(R.string.pc_audio_use_phone_speaker),
        checked = streamingToPhone,
        onCheckedChange = { on -> onSetPcAudio(if (on) 201 else 200, null) },
        icon = G7Icons.speaker,
        tint = tints.teal,
    )

    if (showOutputs && hardwareOutputs.isNotEmpty()) {
        PcAudioOutputsDialog(
            pcName = pcName,
            devices = hardwareOutputs,
            onDismiss = { showOutputs = false },
        )
    }
}

private fun openAppDetailsSettings(context: android.content.Context) {
    runCatching {
        context.startActivity(
            Intent(
                android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                Uri.fromParts("package", context.packageName, null),
            ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
    }
}

private fun openDeveloperOptions(context: android.content.Context) {
    val devIntent = Intent(android.provider.Settings.ACTION_APPLICATION_DEVELOPMENT_SETTINGS)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    if (runCatching { context.startActivity(devIntent) }.isFailure) {
        runCatching {
            context.startActivity(
                Intent(android.provider.Settings.ACTION_SETTINGS)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        }
    }
}

@Composable
internal fun PcAudioOutputsDialog(
    pcName: String,
    devices: List<AudioOutputDevice>,
    onDismiss: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = glassModalContainerColor(),
        title = { Text(stringResource(R.string.pc_audio_outputs_title, pcName)) },
        text = {
            Column(
                verticalArrangement = Arrangement.spacedBy(10.dp),
                modifier = Modifier.verticalScroll(rememberScrollState()),
            ) {
                Text(
                    stringResource(R.string.pc_audio_outputs_hint, pcName),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                devices.forEach { out ->
                    Surface(
                        shape = MaterialTheme.shapes.medium,
                        color = if (out.isDefault) {
                            MaterialTheme.colorScheme.secondaryContainer
                        } else {
                            MaterialTheme.colorScheme.surfaceContainerHigh
                        },
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Row(
                            modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.SpaceBetween,
                        ) {
                            Text(
                                text = out.name,
                                style = MaterialTheme.typography.bodyMedium,
                                color = if (out.isDefault) {
                                    MaterialTheme.colorScheme.onSecondaryContainer
                                } else {
                                    MaterialTheme.colorScheme.onSurface
                                },
                                maxLines = 2,
                                overflow = TextOverflow.Ellipsis,
                                modifier = Modifier.weight(1f),
                            )
                            if (out.isDefault) {
                                Spacer(Modifier.width(8.dp))
                                Text(
                                    text = stringResource(R.string.pc_audio_default_badge),
                                    style = MaterialTheme.typography.labelMedium,
                                    color = MaterialTheme.colorScheme.primary,
                                )
                            }
                        }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) {
                Text(stringResource(R.string.clipboard_history_done))
            }
        },
    )
}

@Composable
private fun ClipboardHistoryDialog(
    entries: List<ClipboardHistoryEntry>,
    onCopy: (String) -> Unit,
    onPin: (String, Boolean) -> Unit,
    onDelete: (String) -> Unit,
    onClearAll: () -> Unit,
    loadImage: suspend (String) -> ByteArray?,
    onDismiss: () -> Unit,
) {
    var query by remember { mutableStateOf("") }
    val trimmed = query.trim().lowercase()
    val filtered = remember(entries, trimmed) {
        if (trimmed.isEmpty()) {
            entries
        } else {
            entries.filter { entry ->
                entry.text.lowercase().contains(trimmed) ||
                    entry.deviceName.lowercase().contains(trimmed) ||
                    (entry.kind == ClipboardItemKind.IMAGE && "image".contains(trimmed))
            }
        }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = glassModalContainerColor(),
        title = {
            Column {
                Text(stringResource(R.string.clipboard_history_title))
                Spacer(Modifier.height(4.dp))
                Text(
                    stringResource(R.string.clipboard_history_subtitle),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                if (entries.isNotEmpty()) {
                    val searchLabel = stringResource(R.string.clipboard_history_search)
                    OutlinedTextField(
                        value = query,
                        onValueChange = { query = it },
                        placeholder = { Text(searchLabel) },
                        singleLine = true,
                        modifier = Modifier
                            .fillMaxWidth()
                            .semantics { contentDescription = searchLabel },
                    )
                }
                when {
                    entries.isEmpty() -> Text(
                        stringResource(R.string.clipboard_history_empty),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    filtered.isEmpty() -> Text(
                        stringResource(R.string.clipboard_history_no_match, query.trim()),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    else -> LazyColumn(
                        modifier = Modifier.fillMaxWidth().heightIn(max = 360.dp),
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        items(filtered, key = { it.id }) { entry ->
                            ClipboardHistoryRow(
                                entry = entry,
                                onCopy = { onCopy(entry.id) },
                                onTogglePin = { onPin(entry.id, !entry.pinned) },
                                onDelete = { onDelete(entry.id) },
                                loadImage = loadImage,
                            )
                        }
                    }
                }
            }
        },
        dismissButton = {
            if (entries.isNotEmpty()) {
                TextButton(onClick = onClearAll) {
                    Text(stringResource(R.string.clipboard_history_clear))
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) {
                Text(stringResource(R.string.clipboard_history_done))
            }
        },
    )
}

@Composable
private fun ClipboardHistoryRow(
    entry: ClipboardHistoryEntry,
    onCopy: () -> Unit,
    onTogglePin: () -> Unit,
    onDelete: () -> Unit,
    loadImage: suspend (String) -> ByteArray?,
) {
    val direction = if (entry.incoming) {
        stringResource(R.string.clipboard_history_from, entry.deviceName)
    } else {
        stringResource(R.string.clipboard_history_sent_to, entry.deviceName)
    }
    val now = System.currentTimeMillis()
    val whenText = if (now - entry.timestamp * 1000L < DateUtils.MINUTE_IN_MILLIS) {
        stringResource(R.string.clipboard_history_just_now)
    } else {
        DateUtils.getRelativeTimeSpanString(entry.timestamp * 1000L, now, DateUtils.MINUTE_IN_MILLIS).toString()
    }
    val header = if (entry.pinned) {
        stringResource(R.string.clipboard_history_meta_pinned, direction, whenText)
    } else {
        stringResource(R.string.clipboard_history_meta, direction, whenText)
    }
    Surface(
        shape = MaterialTheme.shapes.large,
        color = MaterialTheme.colorScheme.surfaceContainerHigh,
    ) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                header,
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            when (entry.kind) {
                ClipboardItemKind.TEXT -> Text(
                    entry.text,
                    style = MaterialTheme.typography.bodyMedium,
                    maxLines = 4,
                    overflow = TextOverflow.Ellipsis,
                )
                ClipboardItemKind.IMAGE -> {
                    val bitmap by produceState<ImageBitmap?>(initialValue = previewBitmapCache.get(entry.id), entry.id) {
                        if (value != null) return@produceState
                        val bytes = loadImage(entry.id) ?: return@produceState
                        val decoded = withContext(Dispatchers.Default) { previewBitmap(bytes)?.asImageBitmap() }
                        if (decoded != null) previewBitmapCache.put(entry.id, decoded)
                        value = decoded
                    }
                    val image = bitmap
                    if (image != null) {
                        Image(
                            bitmap = image,
                            contentDescription = stringResource(R.string.clipboard_history_image),
                            modifier = Modifier.fillMaxWidth().heightIn(max = 120.dp),
                            contentScale = ContentScale.Fit,
                        )
                    } else {
                        Text(
                            stringResource(R.string.clipboard_history_image),
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                FilledTonalButton(onClick = onCopy) {
                    Text(stringResource(R.string.clipboard_history_copy))
                }
                TextButton(onClick = onTogglePin) {
                    Text(
                        stringResource(
                            if (entry.pinned) R.string.clipboard_history_unpin else R.string.clipboard_history_pin,
                        ),
                    )
                }
                Spacer(Modifier.weight(1f))
                TextButton(onClick = onDelete) {
                    Text(stringResource(R.string.clipboard_history_delete))
                }
            }
        }
    }
}

private val previewBitmapCache = LruCache<String, ImageBitmap>(24)

/** A history image decoded small enough for its row (a clip can be a large photo). */
private fun previewBitmap(bytes: ByteArray): android.graphics.Bitmap? {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    var sample = 1
    while (maxOf(bounds.outWidth, bounds.outHeight) / (sample * 2) >= PREVIEW_PX) sample *= 2
    return BitmapFactory.decodeByteArray(bytes, 0, bytes.size, BitmapFactory.Options().apply { inSampleSize = sample })
}

private const val PREVIEW_PX = 720

@Composable
private fun LocalSendCard(
    peers: List<LocalSendPeer>,
    onRefresh: () -> Unit,
    onSendFiles: (pcId: String, uris: List<Uri>) -> Unit,
) {
    val isGlass = LocalGlassTheme.current
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .liquidGlassCard(
                cornerRadius = 28.dp,
                isGlass = isGlass,
                baseColor = MaterialTheme.colorScheme.surfaceContainer,
                tintColor = MaterialTheme.colorScheme.primary,
                outlineColor = if (isGlass) MaterialTheme.colorScheme.outlineVariant else Color.Transparent,
            )
            .padding(if (LocalQuietTheme.current) androidx.compose.foundation.layout.PaddingValues(vertical = 4.dp) else androidx.compose.foundation.layout.PaddingValues(20.dp)),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Row(
            modifier = Modifier.fillMaxWidth().padding(start = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                stringResource(R.string.localsend_card_title),
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.weight(1f),
            )
            IconButton(
                onClick = onRefresh,
                modifier = Modifier.size(32.dp),
            ) {
                Icon(
                    painterResource(R.drawable.ic_refresh),
                    contentDescription = stringResource(R.string.action_refresh),
                    modifier = Modifier.size(20.dp),
                )
            }
        }
        if (peers.isEmpty()) {
            Text(
                stringResource(R.string.localsend_card_empty),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 12.dp),
            )
        } else {
            peers.forEach { peer ->
                LocalSendPeerRow(peer = peer, onSendFiles = onSendFiles)
            }
        }
    }
}

@Composable
private fun LocalSendPeerRow(
    peer: LocalSendPeer,
    onSendFiles: (pcId: String, uris: List<Uri>) -> Unit,
) {
    val pickFiles = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isNotEmpty()) onSendFiles(peer.id, uris)
    }
    G7Row(
        label = peer.alias,
        sublabel = peer.deviceModel?.takeIf { it.isNotBlank() } ?: peer.ip,
        icon = G7Icons.upload,
        tint = g7Tints().amber,
    ) {
        G7RowAction(label = stringResource(R.string.action_send_files), onClick = { pickFiles.launch(arrayOf("*/*")) })
    }
}

@Composable
private fun SendQueueCard(
    state: CoreState,
    onCancel: (String) -> Unit,
    onCancelAll: () -> Unit,
    onRetry: (String) -> Unit,
    onClearFinished: () -> Unit,
) {
    val items = state.sendQueue
    if (items.isEmpty()) return
    val isGlass = LocalGlassTheme.current
    val hasActive = items.any { it.status is FfiQueueStatus.Waiting || it.status is FfiQueueStatus.Sending }
    val hasFinished = items.any { it.status is FfiQueueStatus.Failed || it.status is FfiQueueStatus.Expired }
    val showPeerChip = state.devices.size > 1

    Column(
        modifier = Modifier
            .fillMaxWidth()
            .liquidGlassCard(
                cornerRadius = 28.dp,
                isGlass = isGlass,
                baseColor = MaterialTheme.colorScheme.surfaceContainer,
                tintColor = MaterialTheme.colorScheme.primary,
                outlineColor = if (isGlass) MaterialTheme.colorScheme.outlineVariant else Color.Transparent,
            )
            .padding(if (LocalQuietTheme.current) androidx.compose.foundation.layout.PaddingValues(horizontal = 12.dp, vertical = 4.dp) else androidx.compose.foundation.layout.PaddingValues(20.dp)),
    ) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                text = stringResource(R.string.send_queue_title_count, items.size),
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.weight(1f),
            )
            if (hasActive) {
                TextButton(
                    onClick = onCancelAll,
                    contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 8.dp, vertical = 0.dp),
                    modifier = Modifier.heightIn(min = 28.dp),
                ) {
                    Text(
                        stringResource(R.string.send_queue_cancel_all),
                        style = MaterialTheme.typography.labelMedium,
                    )
                }
            }
            if (hasFinished) {
                TextButton(
                    onClick = onClearFinished,
                    contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 8.dp, vertical = 0.dp),
                    modifier = Modifier.heightIn(min = 28.dp),
                ) {
                    Text(
                        stringResource(R.string.send_queue_clear),
                        style = MaterialTheme.typography.labelMedium,
                    )
                }
            }
        }
        Spacer(Modifier.height(6.dp))
        items.forEachIndexed { index, item ->
            if (index > 0) {
                androidx.compose.material3.HorizontalDivider(
                    color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.35f),
                )
            }
            SendQueueRow(
                item = item,
                transfers = state.transfers,
                showPeerChip = showPeerChip,
                onCancel = { onCancel(item.id) },
                onRetry = { onRetry(item.id) },
            )
        }
    }
}

@Composable
private fun SendQueueRow(
    item: QueueItemUi,
    transfers: List<Transfer>,
    showPeerChip: Boolean = false,
    onCancel: () -> Unit,
    onRetry: () -> Unit,
) {
    val isFileKind = item.kind == FfiQueueKind.FILES ||
        item.kind == FfiQueueKind.FOLDER ||
        item.kind == FfiQueueKind.HANDOFF_DOCUMENT ||
        item.kind == FfiQueueKind.RECORDING
    val kindIcon = when (item.kind) {
        FfiQueueKind.CLIPBOARD, FfiQueueKind.CLIPBOARD_IMAGE -> G7Icons.clipboard
        FfiQueueKind.LINK -> G7Icons.link
        FfiQueueKind.FOLDER -> G7Icons.folder
        FfiQueueKind.RECORDING -> G7Icons.mic
        FfiQueueKind.CAMERA_REQUEST -> G7Icons.camera
        FfiQueueKind.TASK_NOTIFY -> G7Icons.bolt
        FfiQueueKind.FILES,
        FfiQueueKind.HANDOFF_DOCUMENT -> G7Icons.upload
    }
    val status = item.status
    val targetId = item.activeTransferId ?: item.id
    val activeTransfer = if (status is FfiQueueStatus.Sending && isFileKind) {
        transfers.firstOrNull {
            it.id == targetId &&
                it.direction == TransferDirection.OUTGOING &&
                (it.status is TransferStatus.Running || it.status is TransferStatus.Waiting)
        } ?: transfers.firstOrNull {
            it.deviceId == item.peerId &&
                it.direction == TransferDirection.OUTGOING &&
                (it.status is TransferStatus.Running || it.status is TransferStatus.Waiting)
        }
    } else {
        null
    }
    val progressDone = activeTransfer?.done?.toLong() ?: item.bytesDone
    val progressTotal = activeTransfer?.total?.toLong()?.takeIf { it > 0L } ?: item.totalBytes
    val progressPercent = if (status is FfiQueueStatus.Sending && isFileKind && progressTotal > 0L) {
        ((progressDone.toDouble() / progressTotal.toDouble()) * 100.0).roundToInt().coerceIn(0, 100)
    } else {
        null
    }
    val queueFallback = stringResource(queueKindLabel(item.kind))
    val displayTitle = remember(item.title, queueFallback) { formatQueueItemTitle(item.title, queueFallback) }
    val statusText = when (status) {
        is FfiQueueStatus.Waiting -> stringResource(R.string.send_queue_waiting_for, item.peerName)
        is FfiQueueStatus.Sending -> if (progressPercent != null) {
            stringResource(R.string.send_queue_sending_progress, progressPercent)
        } else {
            stringResource(R.string.send_queue_sending)
        }
        is FfiQueueStatus.Failed -> stringResource(R.string.send_queue_failed_reason, status.error)
        is FfiQueueStatus.Expired -> stringResource(R.string.send_queue_expired_reason, status.error)
    }
    val statusColor = when (status) {
        is FfiQueueStatus.Failed -> MaterialTheme.colorScheme.error
        else -> MaterialTheme.colorScheme.onSurfaceVariant
    }

    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Row(Modifier.weight(1f), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                FirstLineIcon(
                    icon = kindIcon,
                    tint = MaterialTheme.colorScheme.primary,
                    style = MaterialTheme.typography.titleSmall,
                    size = 18.dp,
                )
                Column(modifier = Modifier.weight(1f)) {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        Text(
                            text = displayTitle,
                            style = MaterialTheme.typography.titleSmall,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f, fill = false),
                        )
                        if (showPeerChip && item.peerName.isNotBlank()) {
                            Text(
                                text = item.peerName,
                                style = MaterialTheme.typography.labelSmall,
                                color = MaterialTheme.colorScheme.onSecondaryContainer,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                                modifier = Modifier
                                    .bentoWell(
                                        cornerRadius = 8.dp,
                                        fillColor = MaterialTheme.colorScheme.secondaryContainer,
                                    )
                                    .padding(horizontal = 6.dp, vertical = 2.dp),
                            )
                        }
                    }
                    Spacer(Modifier.height(2.dp))
                    Text(
                        text = statusText,
                        style = MaterialTheme.typography.bodySmall,
                        color = statusColor,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
            when (status) {
                is FfiQueueStatus.Waiting, is FfiQueueStatus.Sending -> {
                    IconButton(
                        onClick = onCancel,
                        modifier = Modifier.size(32.dp),
                    ) {
                        Icon(
                            imageVector = G7Icons.close,
                            contentDescription = stringResource(R.string.action_cancel),
                            tint = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.size(18.dp),
                        )
                    }
                }
                is FfiQueueStatus.Failed -> {
                    TextButton(onClick = onRetry) {
                        Text(stringResource(R.string.send_queue_retry))
                    }
                    TextButton(onClick = onCancel) {
                        Text(stringResource(R.string.send_queue_dismiss))
                    }
                }
                is FfiQueueStatus.Expired -> {
                    TextButton(onClick = onCancel) {
                        Text(stringResource(R.string.send_queue_dismiss))
                    }
                }
            }
        }
        if (status is FfiQueueStatus.Sending && isFileKind) {
            val reducedMotion = LocalReducedMotion.current
            if (progressPercent != null) {
                LinearProgressIndicator(
                    progress = { progressPercent / 100f },
                    modifier = Modifier.fillMaxWidth(),
                )
            } else if (reducedMotion) {
                LinearProgressIndicator(
                    progress = { 0.5f },
                    modifier = Modifier.fillMaxWidth(),
                )
            } else {
                LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
            }
        }
    }
}

/** Running transfers and the latest finished ones. */
@Composable
private fun TransfersCard(
    state: CoreState,
    onAccept: (String) -> Unit,
    onCancel: (String) -> Unit,
    onPause: (String) -> Unit = {},
    onResume: (String) -> Unit = {},
) {
    val isGlass = LocalGlassTheme.current
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .liquidGlassCard(
                cornerRadius = 28.dp,
                isGlass = isGlass,
                baseColor = MaterialTheme.colorScheme.surfaceContainer,
                tintColor = MaterialTheme.colorScheme.primary,
                outlineColor = if (isGlass) MaterialTheme.colorScheme.outlineVariant else Color.Transparent,
            )
            .padding(if (LocalQuietTheme.current) androidx.compose.foundation.layout.PaddingValues(horizontal = 12.dp, vertical = 4.dp) else androidx.compose.foundation.layout.PaddingValues(20.dp)),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Text(
            stringResource(R.string.transfers_title),
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        state.transfers.forEach { transfer ->
            TransferRow(
                transfer = transfer,
                pc = state.nameOf(transfer.deviceId).orEmpty(),
                onAccept = onAccept,
                onCancel = onCancel,
                onPause = onPause,
                onResume = onResume,
            )
        }
    }
}

@Composable
private fun TransferRow(
    transfer: Transfer,
    pc: String,
    onAccept: (String) -> Unit,
    onCancel: (String) -> Unit,
    onPause: (String) -> Unit = {},
    onResume: (String) -> Unit = {},
) {
    val context = LocalContext.current
    val incoming = transfer.direction == TransferDirection.INCOMING
    val status = transfer.status
    val title = transferTitle(context.resources, transfer)
    val peerName = pc.ifEmpty { stringResource(R.string.localsend_nearby_device) }
    val detail = when (status) {
        is TransferStatus.Requested ->
            stringResource(
                R.string.transfer_requested_from_size,
                peerName,
                Formatter.formatShortFileSize(context, transfer.total.toLong()),
            )
        is TransferStatus.Waiting -> stringResource(R.string.transfer_waiting, peerName)
        is TransferStatus.Running ->
            stringResource(
                if (incoming) R.string.transfer_receiving_progress else R.string.transfer_sending_progress,
                peerName,
                Formatter.formatShortFileSize(context, transfer.done.toLong()),
                Formatter.formatShortFileSize(context, transfer.total.toLong()),
            )
        is TransferStatus.Paused ->
            stringResource(
                R.string.transfer_paused_progress,
                Formatter.formatShortFileSize(context, transfer.done.toLong()),
                Formatter.formatShortFileSize(context, transfer.total.toLong()),
            )
        is TransferStatus.Done ->
            if (incoming) stringResource(R.string.transfer_received_from, peerName) else stringResource(R.string.transfer_sent_to, peerName)
        is TransferStatus.Cancelled -> stringResource(R.string.transfer_cancelled)
        is TransferStatus.Failed -> when (status.reason) {
            "denied" -> stringResource(R.string.transfer_denied, peerName)
            "unreachable" -> stringResource(R.string.transfer_unreachable, peerName)
            "noSpace" -> stringResource(R.string.transfer_no_space)
            "integrity" -> stringResource(R.string.transfer_failed_integrity)
            else -> stringResource(R.string.transfer_failed)
        }
    }
    Column(modifier = Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(title, style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (status is TransferStatus.Running || status is TransferStatus.Waiting || status is TransferStatus.Paused) {
                    Spacer(Modifier.height(6.dp))
                    val reducedMotion = LocalReducedMotion.current
                    val progress = if (transfer.total == 0uL) {
                        if (status is TransferStatus.Paused) 0f else if (reducedMotion) 0.5f else 1f
                    } else {
                        (transfer.done.toDouble() / transfer.total.toDouble()).toFloat()
                    }
                    val isPaused = status is TransferStatus.Paused
                    val barAlpha = if (isPaused) 0.5f else 1f
                    LinearProgressIndicator(
                        progress = { progress },
                        color = MaterialTheme.colorScheme.primary.copy(alpha = barAlpha),
                        trackColor = MaterialTheme.colorScheme.secondaryContainer.copy(alpha = barAlpha),
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
                Spacer(Modifier.height(4.dp))
                Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            if (status is TransferStatus.Running || status is TransferStatus.Waiting) {
                TextButton(onClick = { onPause(transfer.id) }) { Text(stringResource(R.string.transfer_pause)) }
                TextButton(onClick = { onCancel(transfer.id) }) { Text(stringResource(R.string.action_cancel)) }
            } else if (status is TransferStatus.Paused) {
                TextButton(onClick = { onResume(transfer.id) }) { Text(stringResource(R.string.transfer_resume)) }
                TextButton(onClick = { onCancel(transfer.id) }) { Text(stringResource(R.string.action_cancel)) }
            }
        }
        if (status is TransferStatus.Requested) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = { onAccept(transfer.id) }) {
                    Text(stringResource(R.string.action_accept))
                }
                OutlinedButton(onClick = { onCancel(transfer.id) }) {
                    Text(stringResource(R.string.action_decline))
                }
            }
        }
    }
}

@Composable
internal fun linkText(link: Link): String = when (link) {
    is Link.Online -> when {
        link.relayed -> stringResource(R.string.link_away)
        link.usb -> if (link.rttMs >= 1u) stringResource(R.string.link_usb_rtt, link.rttMs.toInt()) else stringResource(R.string.link_usb)
        link.rttMs >= 1u -> stringResource(R.string.link_online_rtt, link.rttMs.toInt())
        else -> stringResource(R.string.link_online)
    }
    Link.Connecting -> stringResource(R.string.link_connecting)
    is Link.Offline -> when (val seen = link.lastSeen) {
        null -> stringResource(R.string.link_never)
        else -> {
            val now = System.currentTimeMillis()
            if (now - seen * 1000 < DateUtils.MINUTE_IN_MILLIS) {
                stringResource(R.string.link_offline_now)
            } else {
                stringResource(
                    R.string.link_offline_seen,
                    DateUtils.getRelativeTimeSpanString(seen * 1000, now, DateUtils.MINUTE_IN_MILLIS),
                )
            }
        }
    }
}

@Composable
private fun TimelineDialog(
    state: CoreState,
    queryPage: suspend (TimelineKind?, String?, String?, UInt) -> TimelinePage?,
    onCopyClip: (String) -> Unit,
    onResend: (TimelineEntry) -> Unit,
    onDelete: (Long) -> Unit,
    onClearAll: () -> Unit,
    loadClipImage: suspend (String) -> ByteArray?,
    onDismiss: () -> Unit,
) {
    var search by remember { mutableStateOf("") }
    var kindFilter by remember { mutableStateOf<TimelineKind?>(null) }
    var deviceFilter by remember { mutableStateOf<String?>(null) }
    var pagedEntries by remember { mutableStateOf<List<TimelineEntry>>(state.timeline) }
    var hasMore by remember { mutableStateOf(state.timelineHasMore) }
    val scope = rememberCoroutineScope()

    LaunchedEffect(state.timeline, search, kindFilter, deviceFilter) {
        val trimmed = search.trim().takeIf { it.isNotEmpty() }
        if (trimmed == null && kindFilter == null && deviceFilter == null) {
            pagedEntries = state.timeline
            hasMore = state.timelineHasMore
        } else {
            val page = queryPage(kindFilter, deviceFilter, trimmed, 0u)
            pagedEntries = page?.items ?: emptyList()
            hasMore = page?.hasMore ?: false
        }
    }

    val kindChips = listOf<Pair<TimelineKind?, String>>(
        null to stringResource(R.string.timeline_filter_all),
        TimelineKind.FILE to stringResource(R.string.timeline_filter_files),
        TimelineKind.CLIP to stringResource(R.string.timeline_filter_clips),
        TimelineKind.LINK to stringResource(R.string.timeline_filter_links),
        TimelineKind.PHOTO to stringResource(R.string.timeline_filter_photos),
        TimelineKind.RECORDING to stringResource(R.string.timeline_filter_recordings),
        TimelineKind.SESSION to stringResource(R.string.timeline_filter_sessions),
    )

    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = glassModalContainerColor(),
        title = {
            Column {
                Text(stringResource(R.string.timeline_title))
                Spacer(Modifier.height(4.dp))
                Text(
                    stringResource(R.string.timeline_subtitle),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                val searchLabel = stringResource(R.string.timeline_search)
                OutlinedTextField(
                    value = search,
                    onValueChange = { search = it },
                    placeholder = { Text(searchLabel) },
                    singleLine = true,
                    modifier = Modifier
                        .fillMaxWidth()
                        .semantics { contentDescription = searchLabel },
                )
                Row(
                    modifier = Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    kindChips.forEach { (kind, label) ->
                        FilterChip(
                            selected = kindFilter == kind,
                            onClick = { kindFilter = kind },
                            label = { Text(label) },
                        )
                    }
                }
                if (state.devices.size > 1) {
                    Row(
                        modifier = Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        FilterChip(
                            selected = deviceFilter == null,
                            onClick = { deviceFilter = null },
                            label = { Text(stringResource(R.string.timeline_filter_all)) },
                        )
                        state.devices.forEach { dev ->
                            FilterChip(
                                selected = deviceFilter == dev.id,
                                onClick = { deviceFilter = dev.id },
                                label = { Text(dev.name) },
                            )
                        }
                    }
                }
                when {
                    pagedEntries.isEmpty() && search.isBlank() && kindFilter == null && deviceFilter == null -> Text(
                        stringResource(R.string.timeline_empty),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    pagedEntries.isEmpty() -> Text(
                        stringResource(R.string.timeline_no_match, search.trim().ifEmpty { "…" }),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    else -> {
                        val todayLabel = stringResource(R.string.timeline_today)
                        val yesterdayLabel = stringResource(R.string.timeline_yesterday)
                        val dayLabels = remember(pagedEntries, todayLabel, yesterdayLabel) {
                            timelineDayLabels(pagedEntries, todayLabel, yesterdayLabel)
                        }
                        LazyColumn(
                            modifier = Modifier.fillMaxWidth().heightIn(max = 360.dp),
                            verticalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            pagedEntries.forEachIndexed { idx, entry ->
                                val dayLabel = dayLabels[idx]
                                val prevDayLabel = if (idx > 0) dayLabels[idx - 1] else null
                                if (dayLabel != prevDayLabel) {
                                    item(key = "day_${entry.id}", contentType = "day_header") {
                                        Text(
                                            dayLabel,
                                            style = MaterialTheme.typography.labelMedium,
                                            color = MaterialTheme.colorScheme.primary,
                                            modifier = Modifier.padding(top = if (idx > 0) 6.dp else 0.dp),
                                        )
                                    }
                                }
                                item(key = entry.id, contentType = "timeline_row") {
                                    TimelineRow(
                                        entry = entry,
                                        devicePaired = state.device(entry.deviceId) != null,
                                        onCopyClip = onCopyClip,
                                        onResend = { onResend(entry) },
                                        onDelete = { onDelete(entry.id) },
                                        loadClipImage = loadClipImage,
                                    )
                                }
                            }
                            if (hasMore) {
                                item(key = "load_more") {
                                    OutlinedButton(
                                        onClick = {
                                            scope.launch {
                                                val trimmed = search.trim().takeIf { it.isNotEmpty() }
                                                val next = queryPage(kindFilter, deviceFilter, trimmed, pagedEntries.size.toUInt())
                                                if (next != null) {
                                                    pagedEntries = pagedEntries + next.items
                                                    hasMore = next.hasMore
                                                }
                                            }
                                        },
                                        modifier = Modifier.fillMaxWidth(),
                                    ) {
                                        Text(stringResource(R.string.timeline_action_load_more))
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },
        dismissButton = {
            if (state.timeline.isNotEmpty()) {
                TextButton(onClick = onClearAll) {
                    Text(stringResource(R.string.timeline_action_clear_all))
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) {
                Text(stringResource(R.string.clipboard_history_done))
            }
        },
    )
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun TimelineRow(
    entry: TimelineEntry,
    devicePaired: Boolean,
    onCopyClip: (String) -> Unit,
    onResend: () -> Unit,
    onDelete: () -> Unit,
    loadClipImage: suspend (String) -> ByteArray?,
) {
    val context = LocalContext.current
    val direction = if (entry.incoming) {
        stringResource(R.string.clipboard_history_from, entry.deviceName)
    } else {
        stringResource(R.string.clipboard_history_sent_to, entry.deviceName)
    }
    val justNowLabel = stringResource(R.string.clipboard_history_just_now)
    val meta = remember(entry.id, entry.timestamp, entry.sizeBytes, entry.durationSecs, entry.detail, direction, justNowLabel) {
        val now = System.currentTimeMillis()
        val whenText = if (now - entry.timestamp * 1000L < DateUtils.MINUTE_IN_MILLIS) {
            justNowLabel
        } else {
            DateUtils.getRelativeTimeSpanString(entry.timestamp * 1000L, now, DateUtils.MINUTE_IN_MILLIS).toString()
        }
        buildString {
            append(direction)
            append(" · ")
            append(whenText)
            if (entry.sizeBytes > 0uL) {
                append(" · ")
                append(Formatter.formatShortFileSize(context, entry.sizeBytes.toLong()))
            }
            if (entry.durationSecs > 0uL) {
                append(" · ")
                append(DateUtils.formatElapsedTime(entry.durationSecs.toLong()))
            }
            if (entry.detail.isNotEmpty()) {
                append(" · ")
                append(entry.detail)
            }
        }
    }

    val targetExists by produceState(
        initialValue = when (entry.kind) {
            TimelineKind.LINK -> true
            TimelineKind.FILE, TimelineKind.PHOTO, TimelineKind.RECORDING -> entry.target.startsWith("content://")
            else -> false
        },
        entry.target,
        entry.kind,
    ) {
        if (entry.kind == TimelineKind.FILE || entry.kind == TimelineKind.PHOTO || entry.kind == TimelineKind.RECORDING) {
            if (!entry.target.startsWith("content://") && entry.target.isNotEmpty()) {
                value = withContext(Dispatchers.IO) { File(entry.target).exists() }
            }
        }
    }
    val canOpen = when (entry.kind) {
        TimelineKind.LINK -> true
        TimelineKind.FILE, TimelineKind.PHOTO, TimelineKind.RECORDING -> targetExists
        else -> false
    }
    val canShowFolder = when (entry.kind) {
        TimelineKind.FILE, TimelineKind.PHOTO, TimelineKind.RECORDING -> entry.incoming || targetExists
        else -> false
    }
    val canCopy = when (entry.kind) {
        TimelineKind.CLIP -> entry.clipAvailable && entry.refId != null
        TimelineKind.LINK -> true
        else -> false
    }
    val canSendAgain = devicePaired && when (entry.kind) {
        TimelineKind.CLIP -> entry.clipAvailable && entry.refId != null
        TimelineKind.LINK -> true
        TimelineKind.FILE, TimelineKind.PHOTO, TimelineKind.RECORDING -> targetExists
        TimelineKind.SESSION -> false
    }

    Surface(
        shape = MaterialTheme.shapes.large,
        color = MaterialTheme.colorScheme.surfaceContainerHigh,
    ) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(12.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Text(
                meta,
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                entry.title,
                style = MaterialTheme.typography.bodyMedium,
                maxLines = 3,
                overflow = TextOverflow.Ellipsis,
            )
            val clipId = entry.refId
            if (entry.kind == TimelineKind.CLIP && entry.clipAvailable && clipId != null && entry.imageDataUrl != null) {
                val bitmap by produceState<ImageBitmap?>(initialValue = previewBitmapCache.get(clipId), clipId) {
                    if (value != null) return@produceState
                    val bytes = loadClipImage(clipId) ?: return@produceState
                    val decoded = withContext(Dispatchers.Default) { previewBitmap(bytes)?.asImageBitmap() }
                    if (decoded != null) previewBitmapCache.put(clipId, decoded)
                    value = decoded
                }
                bitmap?.let { img ->
                    Image(
                        bitmap = img,
                        contentDescription = stringResource(R.string.clipboard_history_image),
                        modifier = Modifier.fillMaxWidth().heightIn(max = 100.dp),
                        contentScale = ContentScale.Fit,
                    )
                }
            }
            FlowRow(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalArrangement = Arrangement.spacedBy(2.dp),
            ) {
                if (canOpen) {
                    FilledTonalButton(onClick = { openTimelineTarget(context, entry) }) {
                        Text(stringResource(R.string.timeline_action_open))
                    }
                }
                if (canShowFolder) {
                    TextButton(onClick = {
                        runCatching {
                            context.startActivity(
                                Intent(DownloadManager.ACTION_VIEW_DOWNLOADS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                            )
                        }
                    }) {
                        Text(stringResource(R.string.timeline_action_folder))
                    }
                }
                if (canCopy) {
                    FilledTonalButton(onClick = {
                        when (entry.kind) {
                            TimelineKind.CLIP -> entry.refId?.let(onCopyClip)
                            TimelineKind.LINK -> PhoneClipboard.write(context, entry.target.ifEmpty { entry.title })
                            else -> Unit
                        }
                    }) {
                        Text(stringResource(R.string.timeline_action_copy))
                    }
                }
                if (canSendAgain) {
                    TextButton(onClick = onResend) {
                        Text(stringResource(R.string.timeline_action_send_again))
                    }
                }
                TextButton(onClick = onDelete) {
                    Text(stringResource(R.string.timeline_action_remove))
                }
            }
        }
    }
}

private fun openTimelineTarget(context: android.content.Context, entry: TimelineEntry) {
    runCatching {
        when (entry.kind) {
            TimelineKind.LINK -> {
                val url = entry.target.ifEmpty { entry.title }
                context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            }
            TimelineKind.FILE, TimelineKind.PHOTO, TimelineKind.RECORDING -> {
                val target = entry.target
                val uri = when {
                    target.startsWith("content://") -> Uri.parse(target)
                    target.isNotEmpty() && File(target).exists() ->
                        FileProvider.getUriForFile(context, "${context.packageName}.files", File(target))
                    else -> return
                }
                val view = Intent(Intent.ACTION_VIEW)
                    .setData(uri)
                    .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
                context.startActivity(Intent.createChooser(view, null).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            }
            else -> Unit
        }
    }
}

private fun timelineDayLabels(
    entries: List<TimelineEntry>,
    todayLabel: String,
    yesterdayLabel: String,
): List<String> {
    if (entries.isEmpty()) return emptyList()
    val nowCal = Calendar.getInstance()
    val nowYear = nowCal.get(Calendar.YEAR)
    val nowDay = nowCal.get(Calendar.DAY_OF_YEAR)
    val itemCal = Calendar.getInstance()
    val fmt = DateFormat.getDateInstance(DateFormat.MEDIUM)
    val date = Date()
    return entries.map { entry ->
        val ms = entry.timestamp * 1000L
        itemCal.timeInMillis = ms
        val sameYear = nowYear == itemCal.get(Calendar.YEAR)
        val dayDiff = if (sameYear) nowDay - itemCal.get(Calendar.DAY_OF_YEAR) else -1
        when {
            sameYear && dayDiff == 0 -> todayLabel
            sameYear && dayDiff == 1 -> yesterdayLabel
            else -> {
                date.time = ms
                fmt.format(date)
            }
        }
    }
}

internal fun timelineDayLabel(timestampSecs: Long, todayLabel: String, yesterdayLabel: String): String {
    val nowCal = Calendar.getInstance()
    val itemCal = Calendar.getInstance().apply { timeInMillis = timestampSecs * 1000L }
    val sameYear = nowCal.get(Calendar.YEAR) == itemCal.get(Calendar.YEAR)
    val dayDiff = if (sameYear) nowCal.get(Calendar.DAY_OF_YEAR) - itemCal.get(Calendar.DAY_OF_YEAR) else -1
    return when {
        sameYear && dayDiff == 0 -> todayLabel
        sameYear && dayDiff == 1 -> yesterdayLabel
        else -> DateFormat.getDateInstance(DateFormat.MEDIUM).format(Date(timestampSecs * 1000L))
    }
}

private fun RemoteStorageEntry.isDirectoryEntry(): Boolean =
    kind.equals("dir", ignoreCase = true) ||
        kind.equals("directory", ignoreCase = true) ||
        kind.equals("folder", ignoreCase = true)

private fun parentRemotePath(path: String): String {
    val normalized = path.trim('/').replace('\\', '/')
    val slash = normalized.lastIndexOf('/')
    return if (slash <= 0) "" else normalized.substring(0, slash)
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun PcFilesSheet(
    device: Device,
    onListFiles: (path: String, onResult: (PcFolder) -> Unit) -> Unit,
    onDownloadFile: (remotePath: String, fileName: String, onSaved: (Uri?) -> Unit) -> Unit,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val snackbarHostState = remember { SnackbarHostState() }

    var currentPath by remember(device.id) { mutableStateOf("") }
    var entries by remember(device.id) { mutableStateOf<List<RemoteStorageEntry>>(emptyList()) }
    var loading by remember(device.id) { mutableStateOf(false) }
    var downloadingPath by remember(device.id) { mutableStateOf<String?>(null) }
    var refreshTick by remember(device.id) { mutableStateOf(0) }
    var problem by remember(device.id) { mutableStateOf<Int?>(null) }

    LaunchedEffect(device.id, currentPath, refreshTick) {
        loading = true
        onListFiles(currentPath) { folder ->
            problem = folder.problem
            entries = folder.entries.sortedWith(
                compareBy<RemoteStorageEntry> { !it.isDirectoryEntry() }
                    .thenBy(String.CASE_INSENSITIVE_ORDER) { it.name },
            )
            loading = false
        }
    }

    val quickFolders = listOf(
        "" to stringResource(R.string.pc_files_root),
        "Downloads" to stringResource(R.string.pc_files_folder_downloads),
        "Documents" to stringResource(R.string.pc_files_folder_documents),
        "Desktop" to stringResource(R.string.pc_files_folder_desktop),
        "Pictures" to stringResource(R.string.pc_files_folder_pictures),
    )

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        containerColor = glassModalContainerColor(),
        scrimColor = glassModalScrimColor(),
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 20.dp)
                .padding(bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(modifier = Modifier.weight(1f)) {
                    Text(
                        stringResource(R.string.pc_files_title, device.name),
                        style = MaterialTheme.typography.titleLarge,
                    )
                    val breadcrumb = if (currentPath.isEmpty()) {
                        stringResource(R.string.pc_files_root)
                    } else {
                        currentPath.replace('\\', '/')
                    }
                    Text(
                        breadcrumb,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                if (currentPath.isNotEmpty()) {
                    OutlinedButton(onClick = { currentPath = parentRemotePath(currentPath) }) {
                        Text(stringResource(R.string.pc_files_up))
                    }
                    Spacer(Modifier.width(4.dp))
                }
                IconButton(onClick = { refreshTick++ }) {
                    Icon(
                        painterResource(R.drawable.ic_refresh),
                        contentDescription = stringResource(R.string.action_refresh),
                    )
                }
            }

            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                quickFolders.forEach { (folderPath, label) ->
                    FilterChip(
                        selected = currentPath.equals(folderPath, ignoreCase = true),
                        onClick = { currentPath = folderPath },
                        label = { Text(label) },
                    )
                }
            }

            if (loading || downloadingPath != null) {
                if (LocalReducedMotion.current) {
                    LinearProgressIndicator(progress = { 0.5f }, modifier = Modifier.fillMaxWidth())
                } else {
                    LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
                }
            }

            when {
                problem != null && !loading -> {
                    Text(
                        stringResource(problem ?: R.string.pc_files_error, device.name),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(vertical = 16.dp),
                    )
                }
                loading && entries.isEmpty() -> {
                    Text(
                        stringResource(R.string.pc_files_loading),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(vertical = 16.dp),
                    )
                }
                entries.isEmpty() && currentPath.isEmpty() -> {
                    // Quick-access folder list when root listing is empty.
                    LazyColumn(
                        modifier = Modifier
                            .fillMaxWidth()
                            .heightIn(max = 380.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        val rootDefaults = quickFolders.drop(1)
                        items(rootDefaults, key = { it.first }) { (folderPath, label) ->
                            Surface(
                                shape = MaterialTheme.shapes.large,
                                color = MaterialTheme.colorScheme.surfaceContainerHigh,
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .clickable(role = Role.Button) { currentPath = folderPath },
                            ) {
                                Row(
                                    modifier = Modifier
                                        .fillMaxWidth()
                                        .padding(horizontal = 16.dp, vertical = 14.dp),
                                    verticalAlignment = Alignment.CenterVertically,
                                    horizontalArrangement = Arrangement.SpaceBetween,
                                ) {
                                    Text(
                                        label,
                                        style = MaterialTheme.typography.titleSmall,
                                        modifier = Modifier.weight(1f),
                                    )
                                    Text(
                                        "›",
                                        style = MaterialTheme.typography.titleMedium,
                                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                                    )
                                }
                            }
                        }
                    }
                }
                entries.isEmpty() -> {
                    Text(
                        stringResource(R.string.pc_files_empty),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(vertical = 16.dp),
                    )
                }
                else -> {
                    LazyColumn(
                        modifier = Modifier
                            .fillMaxWidth()
                            .heightIn(max = 420.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        items(entries, key = { it.path.ifEmpty { it.name } }) { entry ->
                            val isDir = entry.isDirectoryEntry()
                            val resolvedPath = entry.path.ifEmpty {
                                if (currentPath.isEmpty()) entry.name else "$currentPath/${entry.name}"
                            }
                            val isDownloading = downloadingPath == resolvedPath
                            val savedMessage = stringResource(R.string.pc_files_saved, entry.name)
                            val openActionLabel = stringResource(R.string.timeline_action_open)
                            val failedMessage = stringResource(R.string.pc_files_failed, entry.name)
                            Surface(
                                shape = MaterialTheme.shapes.large,
                                color = MaterialTheme.colorScheme.surfaceContainerHigh,
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .clickable(
                                        enabled = downloadingPath == null,
                                        role = Role.Button,
                                    ) {
                                        if (isDir) {
                                            currentPath = resolvedPath
                                        } else {
                                            downloadingPath = resolvedPath
                                            onDownloadFile(resolvedPath, entry.name) { savedUri ->
                                                downloadingPath = null
                                                scope.launch {
                                                    if (savedUri != null) {
                                                        val res = snackbarHostState.showSnackbar(
                                                            message = savedMessage,
                                                            actionLabel = openActionLabel,
                                                            duration = SnackbarDuration.Short,
                                                        )
                                                        if (res == SnackbarResult.ActionPerformed) {
                                                            openSavedPcFile(context, savedUri, entry.name)
                                                        }
                                                    } else {
                                                        snackbarHostState.showSnackbar(
                                                            message = failedMessage,
                                                            duration = SnackbarDuration.Short,
                                                        )
                                                    }
                                                }
                                            }
                                        }
                                    },
                            ) {
                                Row(
                                    modifier = Modifier
                                        .fillMaxWidth()
                                        .padding(horizontal = 16.dp, vertical = 12.dp),
                                    verticalAlignment = Alignment.CenterVertically,
                                    horizontalArrangement = Arrangement.SpaceBetween,
                                ) {
                                    Column(modifier = Modifier.weight(1f).padding(end = 12.dp)) {
                                        Text(
                                            entry.name,
                                            style = MaterialTheme.typography.titleSmall,
                                            maxLines = 1,
                                            overflow = TextOverflow.Ellipsis,
                                        )
                                        val meta = if (isDownloading) {
                                            stringResource(R.string.pc_files_downloading, entry.name)
                                        } else if (!isDir) {
                                            Formatter.formatShortFileSize(context, entry.size.toLong())
                                        } else {
                                            null
                                        }
                                        if (meta != null) {
                                            Spacer(Modifier.height(2.dp))
                                            Text(
                                                meta,
                                                style = MaterialTheme.typography.bodySmall,
                                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                                maxLines = 1,
                                                overflow = TextOverflow.Ellipsis,
                                            )
                                        }
                                    }
                                    if (isDir) {
                                        Text(
                                            "›",
                                            style = MaterialTheme.typography.titleMedium,
                                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                                        )
                                    }
                                }
                            }
                        }
                    }
                }
            }

            SnackbarHost(hostState = snackbarHostState)
        }
    }
}

private fun openSavedPcFile(context: android.content.Context, uri: Uri, fileName: String) {
    runCatching {
        val mime = ReceivedFiles.mimeOf(fileName)
        val view = Intent(Intent.ACTION_VIEW)
            .setDataAndType(uri, mime)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
        context.startActivity(Intent.createChooser(view, null).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }
}

/** The localized fallback label for a queue item whose title is empty. */
private fun queueKindLabel(kind: FfiQueueKind): Int = when (kind) {
    FfiQueueKind.CLIPBOARD -> R.string.queue_kind_clipboard
    FfiQueueKind.CLIPBOARD_IMAGE -> R.string.queue_kind_clipboard_image
    FfiQueueKind.FILES -> R.string.queue_kind_files
    FfiQueueKind.FOLDER -> R.string.queue_kind_folder
    FfiQueueKind.LINK -> R.string.queue_kind_link
    FfiQueueKind.HANDOFF_DOCUMENT -> R.string.queue_kind_document
    FfiQueueKind.RECORDING -> R.string.queue_kind_recording
    FfiQueueKind.TASK_NOTIFY -> R.string.queue_kind_task_alert
    FfiQueueKind.CAMERA_REQUEST -> R.string.queue_kind_camera_request
}
