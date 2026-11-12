// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.g7link.android.ui.home.PcSoundRows
import app.g7link.android.ui.home.linkText
import app.g7link.android.ui.theme.G7Header
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.G7Row
import app.g7link.android.ui.theme.G7Tabs
import app.g7link.android.ui.theme.g7Tints
import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import android.os.Build
import android.os.Bundle
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import android.view.WindowManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.core.WakeState
import app.g7link.android.ui.theme.LocalReducedMotion
import kotlin.math.abs
import kotlin.math.hypot
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch

enum class RemoteMode { Touchpad, AirMouse, Presentation, Screen, Media }

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun RemoteScreen(
    device: Device,
    core: Core,
    sensitivity: Float,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
    initialMode: RemoteMode = RemoteMode.Touchpad,
) {
    val view = LocalView.current
    val context = LocalContext.current

    // Keep the phone's screen on while controlling the PC, and release any
    // held laser/button when leaving.
    DisposableEffect(view, device.id) {
        val window = (context as? Activity)?.window
        val prevKeepOn = view.keepScreenOn
        view.keepScreenOn = true
        window?.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        onDispose {
            view.keepScreenOn = prevKeepOn
            window?.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            core.remoteLaser(device.id, false)
        }
    }

    var mode by rememberSaveable(initialMode) { mutableStateOf(initialMode) }
    var showKeyboard by rememberSaveable { mutableStateOf(false) }
    var screenFullScreen by rememberSaveable { mutableStateOf(false) }
    var access by remember(device.id) { mutableStateOf(Core.RemoteAccess.Allowed) }
    // In landscape the touchpad takes the screen, with its buttons under it.
    var touchpadOnlyChosen by rememberSaveable { mutableStateOf(true) }
    val landscape = LocalConfiguration.current.orientation == Configuration.ORIENTATION_LANDSCAPE
    val touchpadOnly = landscape && touchpadOnlyChosen && mode == RemoteMode.Touchpad &&
        device.online && access == Core.RemoteAccess.Allowed
    val isFullScreen = (mode == RemoteMode.Screen && screenFullScreen) || touchpadOnly

    // A text box got the focus on the PC: the keyboard opens, as it would for one
    // on the phone, and closes again when the PC's focus leaves text, if it
    // opened by itself. One the user opened or closed stays as they left it.
    val pcTyping by remember(core, device.id) {
        core.state.map { device.id in it.pcTextFocus }.distinctUntilChanged()
    }.collectAsStateWithLifecycle(initialValue = false)
    var keyboardOpenedForPc by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(pcTyping) {
        if (pcTyping) {
            if (!showKeyboard && mode == RemoteMode.Touchpad && !touchpadOnly) {
                showKeyboard = true
                keyboardOpenedForPc = true
            }
        } else if (keyboardOpenedForPc) {
            // Focus moving from one box to the next isn't leaving text.
            delay(1_500)
            showKeyboard = false
            keyboardOpenedForPc = false
        }
    }
    DisposableEffect(view, touchpadOnly) {
        val window = (context as? Activity)?.window
        val bars = window?.let { WindowCompat.getInsetsController(it, view) }
        if (touchpadOnly) {
            bars?.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            bars?.hide(WindowInsetsCompat.Type.systemBars())
        }
        onDispose {
            if (touchpadOnly) bars?.show(WindowInsetsCompat.Type.systemBars())
        }
    }
    val utteranceJoiner = remember(device.id) { UtteranceJoiner() }

    LaunchedEffect(mode) {
        if (mode != RemoteMode.Screen && screenFullScreen) {
            screenFullScreen = false
        }
    }

    // Check access on entry and poll while denied so the prompt banner clears
    // as soon as the user clicks Allow on the PC.
    LaunchedEffect(device.id, device.online) {
        if (!device.online) {
            access = Core.RemoteAccess.Offline
            return@LaunchedEffect
        }
        access = core.remoteCheck(device.id)
        while (access == Core.RemoteAccess.Denied && device.online) {
            delay(1_500)
            access = core.remoteCheck(device.id)
        }
    }

    val onStatus: (Core.RemoteAccess) -> Unit = { next -> access = next }

    Column(
        modifier = modifier
            .fillMaxSize()
            .then(if (isFullScreen) Modifier else Modifier.systemBarsPadding()),
    ) {
        if (!isFullScreen) {
            G7Header(title = device.name, onBack = onBack, note = linkText(device.link))
            G7Tabs(
                options = listOf(
                    RemoteMode.Touchpad to stringResource(R.string.remote_mode_touchpad),
                    RemoteMode.AirMouse to stringResource(R.string.remote_mode_air),
                    RemoteMode.Presentation to stringResource(R.string.remote_mode_presentation),
                    RemoteMode.Screen to stringResource(R.string.remote_mode_screen),
                    RemoteMode.Media to stringResource(R.string.remote_mode_media),
                ),
                selected = mode,
                onSelect = { mode = it },
                modifier = Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, bottom = 10.dp),
            )

            // Status banner when offline, denied, or unsupported
            Column(Modifier.padding(start = 16.dp, end = 16.dp, bottom = 10.dp)) {
                when {
                    !device.online || access == Core.RemoteAccess.Offline -> OfflineWakeBanner(
                        device = device,
                        onWake = { core.wake(device.id) },
                    )
                    access == Core.RemoteAccess.Denied -> StatusBanner(
                        title = stringResource(R.string.remote_denied_title, device.name),
                        text = stringResource(R.string.remote_denied_text, device.name),
                    )
                    access == Core.RemoteAccess.Unsupported -> StatusBanner(
                        title = null,
                        text = stringResource(R.string.remote_unsupported_text, device.name),
                    )
                }
            }
        }

        val reducedMotion = LocalReducedMotion.current
        AnimatedContent(
            targetState = mode,
            transitionSpec = {
                if (reducedMotion) {
                    EnterTransition.None togetherWith ExitTransition.None
                } else {
                    val forward = targetState.ordinal >= initialState.ordinal
                    (slideInHorizontally(tween(240, easing = FastOutSlowInEasing)) { if (forward) it / 8 else -it / 8 } +
                        fadeIn(tween(200))) togetherWith
                        (slideOutHorizontally(tween(180, easing = FastOutSlowInEasing)) { if (forward) -it / 10 else it / 10 } +
                            fadeOut(tween(150)))
                }
            },
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .then(if (isFullScreen) Modifier else Modifier.padding(horizontal = 16.dp)),
            label = "RemoteModeTransition",
        ) { currentMode ->
            when (currentMode) {
                RemoteMode.Touchpad -> TouchpadView(
                    pcId = device.id,
                    pcName = device.name,
                    core = core,
                    sensitivity = sensitivity,
                    utteranceJoiner = utteranceJoiner,
                    onStatus = onStatus,
                    canBeTouchpadOnly = landscape,
                    touchpadOnly = touchpadOnly,
                    onTouchpadOnly = { touchpadOnlyChosen = it },
                    showKeyboard = showKeyboard,
                    onShowKeyboard = {
                        showKeyboard = it
                        keyboardOpenedForPc = false
                    },
                    modifier = Modifier.fillMaxSize(),
                )
                RemoteMode.AirMouse -> AirMouseView(
                    pcId = device.id,
                    pcName = device.name,
                    core = core,
                    sensitivity = sensitivity,
                    utteranceJoiner = utteranceJoiner,
                    onStatus = onStatus,
                    modifier = Modifier.fillMaxSize(),
                )
                RemoteMode.Presentation -> PresentationView(
                    pcId = device.id,
                    pcName = device.name,
                    core = core,
                    onStatus = onStatus,
                    modifier = Modifier.fillMaxSize(),
                )
                RemoteMode.Screen -> Column(
                    // Above the keyboard while it's open: apps are drawn edge to edge
                    // (Android 15 and later), so it would cover the typing field.
                    modifier = Modifier.fillMaxSize().imePadding(),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    PcMirrorView(
                        pcId = device.id,
                        pcName = device.name,
                        core = core,
                        isFullScreen = screenFullScreen,
                        onFullScreenChange = { screenFullScreen = it },
                        touchpadSensitivity = sensitivity,
                        onStatus = onStatus,
                        modifier = Modifier.weight(1f).fillMaxWidth(),
                    )
                    // The touchpad under the screen: watch the PC and move its pointer together.
                    // While typing, the room goes to the screen and the field.
                    if (!screenFullScreen && !WindowInsets.isImeVisible) {
                        TouchpadSurface(
                            pcId = device.id,
                            core = core,
                            sensitivity = sensitivity,
                            onStatus = onStatus,
                            modifier = Modifier.fillMaxWidth().height(180.dp),
                        )
                    }
                }
                RemoteMode.Media -> MediaView(device = device, core = core, modifier = Modifier.fillMaxSize())
            }
        }

        if (!isFullScreen && !WindowInsets.isImeVisible) {
            RemoteToolbar(
                device = device,
                core = core,
                keyboardOn = showKeyboard && mode == RemoteMode.Touchpad,
                onKeyboard = {
                    keyboardOpenedForPc = false
                    if (mode != RemoteMode.Touchpad) {
                        mode = RemoteMode.Touchpad
                        showKeyboard = true
                    } else {
                        showKeyboard = !showKeyboard
                    }
                },
                onVolume = { mode = RemoteMode.Media },
            )
        }
    }
}

/**
 * Under every mode: the keyboard, the PC's sound, play or pause, and Lock and
 * Sleep when the PC takes them.
 */
@Composable
private fun RemoteToolbar(
    device: Device,
    core: Core,
    keyboardOn: Boolean,
    onKeyboard: () -> Unit,
    onVolume: () -> Unit,
) {
    val tints = g7Tints()
    val pcMedia = core.pcMedia
    val summaries by pcMedia.summaries.collectAsStateWithLifecycle()
    val summary = summaries[device.id]
    val playing = summary?.playing ?: (device.deckState?.playing == true)
    val scope = rememberCoroutineScope()
    Row(
        modifier = Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 8.dp, bottom = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        G7IconButton(
            icon = G7Icons.keyboard,
            contentDescription = stringResource(R.string.remote_keyboard),
            onClick = onKeyboard,
            tint = tints.pink,
            selected = keyboardOn,
            enabled = device.online,
            modifier = Modifier.weight(1f),
        )
        G7IconButton(
            icon = G7Icons.volume,
            contentDescription = stringResource(R.string.pc_audio_volume),
            onClick = onVolume,
            tint = tints.teal,
            enabled = device.online,
            modifier = Modifier.weight(1f),
        )
        G7IconButton(
            icon = if (playing) G7Icons.pause else G7Icons.play,
            contentDescription = stringResource(if (playing) R.string.media_pause else R.string.media_play),
            onClick = {
                if (summary != null) {
                    val action = if (playing) app.g7link.core.MediaAction.PAUSE else app.g7link.core.MediaAction.PLAY
                    pcMedia.pressed(device.id, action)
                } else {
                    scope.launch { core.deckPress(device.id, "media_play_pause") }
                }
            },
            tint = tints.pink,
            enabled = device.online,
            modifier = Modifier.weight(1f),
        )
        if (device.has("device.pc_actions")) {
            G7IconButton(
                icon = G7Icons.lock,
                contentDescription = stringResource(R.string.action_lock_pc),
                onClick = { core.pcPower(device.id, false) },
                tint = tints.teal,
                enabled = device.online,
                modifier = Modifier.weight(1f),
            )
            G7IconButton(
                icon = G7Icons.moon,
                contentDescription = stringResource(R.string.action_sleep_pc),
                onClick = { core.pcPower(device.id, true) },
                tint = tints.red,
                enabled = device.online,
                modifier = Modifier.weight(1f),
            )
        }
    }
}

/** The PC's sound: what plays, with its controls, the volume and where it plays. */
@Composable
private fun MediaView(device: Device, core: Core, modifier: Modifier = Modifier) {
    val deckState = device.deckState
    Column(modifier = modifier.verticalScroll(rememberScrollState())) {
        if (deckState != null && device.online) {
            PcSoundRows(
                pcId = device.id,
                pcName = device.name,
                deckState = deckState,
                onSetPcAudio = { volume, muted -> core.setPcAudio(device.id, volume, muted) },
            )
        } else {
            G7Row(
                label = stringResource(R.string.remote_media_nothing),
                icon = G7Icons.music,
                tint = g7Tints().muted,
            )
        }
    }
}

@Composable
private fun OfflineWakeBanner(device: Device, onWake: () -> Unit) {
    Surface(
        shape = MaterialTheme.shapes.large,
        color = MaterialTheme.colorScheme.secondaryContainer,
        contentColor = MaterialTheme.colorScheme.onSecondaryContainer,
    ) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp)) {
            when {
                !device.canWake -> {
                    Text(
                        stringResource(R.string.remote_offline_text, device.name),
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                device.wakeState == WakeState.Waking -> {
                    Text(
                        stringResource(R.string.wake_waking, device.name),
                        style = MaterialTheme.typography.titleSmall,
                    )
                    Spacer(Modifier.height(8.dp))
                    if (LocalReducedMotion.current) {
                        LinearProgressIndicator(progress = { 0.5f }, modifier = Modifier.fillMaxWidth())
                    } else {
                        LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
                    }
                }
                device.wakeState == WakeState.TimedOut -> {
                    Text(
                        stringResource(R.string.wake_timeout_title, device.name),
                        style = MaterialTheme.typography.titleSmall,
                    )
                    Spacer(Modifier.height(4.dp))
                    Text(
                        stringResource(R.string.wake_timeout_text, device.name),
                        style = MaterialTheme.typography.bodySmall,
                    )
                    Spacer(Modifier.height(10.dp))
                    Button(onClick = onWake) {
                        Text(stringResource(R.string.action_wake_pc))
                    }
                }
                else -> {
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(
                            stringResource(R.string.remote_offline_text, device.name),
                            style = MaterialTheme.typography.bodySmall,
                            modifier = Modifier.weight(1f),
                        )
                        Spacer(Modifier.width(12.dp))
                        Button(onClick = onWake) {
                            Text(stringResource(R.string.action_wake_pc))
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun StatusBanner(title: String?, text: String) {
    Surface(
        shape = MaterialTheme.shapes.large,
        color = MaterialTheme.colorScheme.secondaryContainer,
        contentColor = MaterialTheme.colorScheme.onSecondaryContainer,
    ) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp)) {
            if (title != null) {
                Text(title, style = MaterialTheme.typography.titleSmall)
                Spacer(Modifier.height(2.dp))
            }
            Text(text, style = MaterialTheme.typography.bodySmall)
        }
    }
}

/** The touchpad surface: moves, clicks, drags and scrolls the PC pointer. */
@Composable
private fun TouchpadSurface(
    pcId: String,
    core: Core,
    sensitivity: Float,
    onStatus: (Core.RemoteAccess) -> Unit,
    modifier: Modifier = Modifier,
    scrollLocked: Boolean = false,
) {
    var dragging by remember { mutableStateOf(false) }
    var touchPoint by remember { mutableStateOf<Offset?>(null) }
    val flingScope = rememberCoroutineScope()
    val currentScrollLocked by rememberUpdatedState(scrollLocked)
    val primaryColor = MaterialTheme.colorScheme.primary
    val outlineColor = MaterialTheme.colorScheme.outlineVariant
    val touchpadDesc = stringResource(R.string.remote_mode_touchpad)
    Surface(
        shape = MaterialTheme.shapes.extraLarge,
        color = MaterialTheme.colorScheme.surfaceContainer,
        border = if (dragging || scrollLocked) BorderStroke(2.dp, primaryColor) else null,
        modifier = modifier
            .semantics { contentDescription = touchpadDesc }
            .pointerInput(pcId, sensitivity) {
                touchpadGestures(
                    pcId = pcId,
                    core = core,
                    sensitivity = sensitivity,
                    flingScope = flingScope,
                    onStatus = onStatus,
                    onTouch = { touchPoint = it },
                    onDragging = { dragging = it },
                    scrollLocked = { currentScrollLocked },
                )
            },
    ) {
        Box(Modifier.fillMaxSize()) {
            Canvas(Modifier.fillMaxSize()) {
                touchPoint?.let { pt ->
                    drawCircle(
                        color = primaryColor.copy(alpha = 0.22f),
                        radius = 28.dp.toPx(),
                        center = pt,
                    )
                    drawCircle(
                        color = primaryColor.copy(alpha = 0.65f),
                        radius = 6.dp.toPx(),
                        center = pt,
                    )
                }
            }
            Text(
                stringResource(if (scrollLocked) R.string.remote_scroll_mode_hint else R.string.remote_touchpad_hint),
                style = MaterialTheme.typography.bodySmall,
                color = if (scrollLocked) primaryColor else MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
                modifier = Modifier
                    .align(Alignment.Center)
                    .padding(horizontal = 24.dp, vertical = 14.dp),
            )
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun TouchpadView(
    pcId: String,
    pcName: String,
    core: Core,
    sensitivity: Float,
    utteranceJoiner: UtteranceJoiner,
    onStatus: (Core.RemoteAccess) -> Unit,
    canBeTouchpadOnly: Boolean,
    touchpadOnly: Boolean,
    onTouchpadOnly: (Boolean) -> Unit,
    showKeyboard: Boolean,
    onShowKeyboard: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
) {
    // What's been typed since the last Enter: a real field, so the keyboard
    // keeps its page, autocorrects, and has something to delete.
    var typed by remember { mutableStateOf(TextFieldValue("")) }
    val activeMods = remember { mutableStateListOf<String>() }
    // Middle locks the touchpad to scrolling, as a middle click starts autoscroll.
    var scrollLocked by rememberSaveable(pcId) { mutableStateOf(false) }

    val focusRequester = remember { FocusRequester() }
    val keyboardController = LocalSoftwareKeyboardController.current
    val imeVisible = WindowInsets.isImeVisible

    LaunchedEffect(showKeyboard) {
        if (showKeyboard) {
            focusRequester.requestFocus()
            keyboardController?.show()
        } else {
            keyboardController?.hide()
        }
    }

    // Above the keyboard while it's open: apps are drawn edge to edge
    // (Android 15 and later), so the keyboard would cover the typing field.
    BoxWithConstraints(modifier = modifier.fillMaxWidth().imePadding()) {
        // While typing, a strip of touchpad stays when there's room for it and the
        // field, so the pointer can move without closing the keyboard (#52).
        val padWhileTyping = maxHeight >= ROOM_FOR_PAD_WHILE_TYPING
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .then(if (touchpadOnly) Modifier.padding(8.dp) else Modifier),
            verticalArrangement = Arrangement.spacedBy(if (touchpadOnly) 6.dp else 10.dp),
        ) {
            if (!imeVisible) {
                TouchpadSurface(
                    pcId = pcId,
                    core = core,
                    sensitivity = sensitivity,
                    onStatus = onStatus,
                    modifier = Modifier.weight(1f).fillMaxWidth(),
                    scrollLocked = scrollLocked,
                )
            }

            // The mouse buttons, and in landscape the switch to the touchpad alone
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                G7RaisedButton(
                    label = stringResource(R.string.remote_left_click),
                    onClick = { core.remoteButton(pcId, "left", "click", onStatus) },
                    modifier = Modifier.weight(1f),
                )
                G7RaisedButton(
                    label = stringResource(if (scrollLocked) R.string.remote_scroll_mode else R.string.remote_middle_click),
                    onClick = { scrollLocked = !scrollLocked },
                    onLongClick = { core.remoteButton(pcId, "middle", "click", onStatus) },
                    selected = scrollLocked,
                    modifier = Modifier.weight(1f),
                )
                G7RaisedButton(
                    label = stringResource(R.string.remote_right_click),
                    onClick = { core.remoteButton(pcId, "right", "click", onStatus) },
                    modifier = Modifier.weight(1f),
                )
                if (canBeTouchpadOnly) {
                    G7RaisedButton(
                        label = stringResource(if (touchpadOnly) R.string.remote_more_controls else R.string.remote_touchpad_only),
                        onClick = {
                            if (!touchpadOnly) onShowKeyboard(false)
                            onTouchpadOnly(!touchpadOnly)
                        },
                        modifier = Modifier.weight(1.2f),
                    )
                }
            }

            if (touchpadOnly) return@Column

            if (!showKeyboard) return@Column

            // Scroll rather than squeeze the typing field when the keyboard is up.
            Column(
                modifier = if (imeVisible) Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState()) else Modifier,
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                // Voice typing bar (Dictate button: hold to talk, or tap to start/stop)
                VoiceTypingBar(
                    pcId = pcId,
                    pcName = pcName,
                    core = core,
                    utteranceJoiner = utteranceJoiner,
                    onStatus = onStatus,
                )

                // Special keys & modifiers bar (always accessible in Touchpad mode)
                SpecialKeysBar(
                    activeMods = activeMods,
                    showExtraKeys = showKeyboard,
                    onToggleMod = { mod ->
                        if (mod in activeMods) activeMods.remove(mod) else activeMods.add(mod)
                    },
                    onKey = { key, forceMods ->
                        val mods = forceMods ?: activeMods.toList()
                        core.remoteKey(pcId, key, mods, onStatus)
                        if (forceMods == null) activeMods.clear()
                    },
                )
            }

            // Soft keyboard input row when toggled open
            if (showKeyboard) {
                val typePlaceholder = stringResource(R.string.remote_type_placeholder, pcName)
                val pickImage = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
                    if (uri != null) core.pasteImageOnPc(pcId, uri, onStatus)
                }
                OutlinedTextField(
                    value = typed,
                    onValueChange = { next ->
                        val diff = typingDiff(typed.text, next.text)
                        if (activeMods.isNotEmpty() && diff.backspaces == 0 && diff.added.length == 1 &&
                            diff.added[0].isLetterOrDigit()
                        ) {
                            // A shortcut (Ctrl+C...), not text: it stays out of the field.
                            core.remoteKey(pcId, diff.added.lowercase(), activeMods.toList(), onStatus)
                            activeMods.clear()
                            return@OutlinedTextField
                        }
                        repeat(diff.backspaces) { i ->
                            core.remoteKey(pcId, "backspace", if (i == 0) activeMods.toList() else emptyList(), onStatus)
                        }
                        if (diff.added.isNotEmpty()) core.remoteText(pcId, diff.added, onStatus)
                        if (diff.backspaces > 0 || diff.added.isNotEmpty()) activeMods.clear()
                        // The cursor and selection here are free to move: an edit anywhere
                        // goes over as Backspaces from the end, then what follows it again.
                        typed = next
                    },
                    placeholder = { Text(typePlaceholder) },
                    // An image goes where the PC is typing too, as a paste.
                    trailingIcon = {
                        IconButton(
                            onClick = {
                                pickImage.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
                            },
                        ) {
                            Icon(
                                G7Icons.photo,
                                contentDescription = stringResource(R.string.remote_paste_image, pcName),
                                tint = g7Tints().coral,
                            )
                        }
                    },
                    maxLines = 4,
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
                    keyboardActions = KeyboardActions(
                        onSend = {
                            core.remoteKey(pcId, "enter", activeMods.toList(), onStatus)
                            activeMods.clear()
                            typed = TextFieldValue("")
                        },
                    ),
                    modifier = Modifier
                        .fillMaxWidth()
                        .focusRequester(focusRequester)
                        // Backspace with nothing typed here still deletes on the PC.
                        .onPreviewKeyEvent { event ->
                            if (event.type == KeyEventType.KeyDown && event.key == Key.Backspace && typed.text.isEmpty()) {
                                core.remoteKey(pcId, "backspace", activeMods.toList(), onStatus)
                                activeMods.clear()
                                true
                            } else {
                                false
                            }
                        }
                        .semantics { contentDescription = typePlaceholder },
                )
            }

            // While typing, the strip sits right above the keyboard, under the
            // field, where both thumbs reach it (#52).
            if (imeVisible && padWhileTyping) {
                TouchpadSurface(
                    pcId = pcId,
                    core = core,
                    sensitivity = sensitivity,
                    onStatus = onStatus,
                    modifier = Modifier.height(PAD_WHILE_TYPING).fillMaxWidth(),
                    scrollLocked = scrollLocked,
                )
            }
        }
    }
}

/** The touchpad strip while the keyboard is open. */
private val PAD_WHILE_TYPING = 96.dp

/**
 * Height above the keyboard that fits the strip, the buttons, the field and a row
 * of the keys below them, which scroll to make room.
 */
private val ROOM_FOR_PAD_WHILE_TYPING = 280.dp

@Composable
private fun AirMouseView(
    pcId: String,
    pcName: String,
    core: Core,
    sensitivity: Float,
    utteranceJoiner: UtteranceJoiner,
    onStatus: (Core.RemoteAccess) -> Unit,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    val sensorManager = remember(context) {
        context.getSystemService(Context.SENSOR_SERVICE) as? SensorManager
    }
    val gyroSensor = remember(sensorManager) {
        sensorManager?.getDefaultSensor(Sensor.TYPE_GYROSCOPE)
    }
    val hasGyro = gyroSensor != null

    var padHeld by remember { mutableStateOf(false) }
    // While two fingers scroll, the phone's tilt leaves the pointer where the scroll is.
    var twoFingerScrolling by remember { mutableStateOf(false) }
    var aimOffset by remember { mutableStateOf(Offset.Zero) }
    val filter = remember { AirMouseFilter() }
    val flingScope = rememberCoroutineScope()
    val currentSensitivity by rememberUpdatedState(sensitivity)

    DisposableEffect(padHeld, pcId, hasGyro) {
        if (!padHeld || gyroSensor == null || sensorManager == null) {
            filter.reset()
            aimOffset = Offset.Zero
            return@DisposableEffect onDispose {}
        }

        filter.reset()
        val listener = object : SensorEventListener {
            override fun onSensorChanged(event: SensorEvent) {
                if (!padHeld || event.sensor.type != Sensor.TYPE_GYROSCOPE || event.values.size < 3) return
                val (dx, dy) = filter.onSample(
                    wx = event.values[0],
                    wy = event.values[1],
                    wz = event.values[2],
                    timestampNs = event.timestamp,
                    sensitivity = currentSensitivity,
                )
                if (twoFingerScrolling) return
                if (abs(dx) >= 0.05f || abs(dy) >= 0.05f) {
                    core.remoteMove(pcId, dx, dy)
                    val nextX = (aimOffset.x + dx * 0.35f).coerceIn(-120f, 120f)
                    val nextY = (aimOffset.y + dy * 0.35f).coerceIn(-120f, 120f)
                    aimOffset = Offset(nextX, nextY)
                }
            }

            override fun onAccuracyChanged(sensor: Sensor?, accuracy: Int) {}
        }

        sensorManager.registerListener(listener, gyroSensor, SensorManager.SENSOR_DELAY_GAME)
        onDispose {
            sensorManager.unregisterListener(listener)
            filter.reset()
            aimOffset = Offset.Zero
        }
    }

    Column(
        modifier = modifier.fillMaxWidth(),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        if (!hasGyro) {
            StatusBanner(
                title = stringResource(R.string.remote_air_no_gyro_title),
                text = stringResource(R.string.remote_air_no_gyro_text),
            )
        }

        val primaryColor = MaterialTheme.colorScheme.primary
        val outlineColor = MaterialTheme.colorScheme.outlineVariant
        val airDesc = stringResource(R.string.remote_mode_air)
        Surface(
            shape = MaterialTheme.shapes.extraLarge,
            color = if (padHeld) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surfaceContainer,
            contentColor = if (padHeld) MaterialTheme.colorScheme.onPrimaryContainer else MaterialTheme.colorScheme.onSurface,
            border = BorderStroke(
                width = if (padHeld) 2.dp else 1.dp,
                color = if (padHeld) primaryColor else outlineColor,
            ),
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .semantics { contentDescription = airDesc }
                .pointerInput(pcId, hasGyro) {
                    val tapSlop = viewConfiguration.touchSlop
                    val toReference = TOUCHPAD_REFERENCE_DENSITY / density
                    val tapTimeoutMs = 240L
                    var fling: Job? = null

                    awaitEachGesture {
                        val firstDown = awaitFirstDown(requireUnconsumed = false)
                        fling?.cancel()
                        fling = null
                        val downTime = System.currentTimeMillis()
                        var lastEventTime = firstDown.uptimeMillis
                        var maxPointers = 1
                        var totalMoved = 0f
                        val twoFinger = TwoFingerGesture(tapSlop * toReference)

                        filter.reset()
                        aimOffset = Offset.Zero
                        padHeld = hasGyro

                        while (true) {
                            val event = awaitPointerEvent()
                            val changes = event.changes
                            var pressedCount = 0
                            var avgDx = 0f
                            var avgDy = 0f
                            for (i in 0 until changes.size) {
                                val c = changes[i]
                                if (c.pressed) {
                                    pressedCount++
                                    val d = c.positionChange()
                                    totalMoved += hypot(d.x, d.y)
                                    avgDx += d.x
                                    avgDy += d.y
                                }
                            }
                            if (changes.isNotEmpty()) lastEventTime = changes[0].uptimeMillis
                            if (pressedCount == 0) break
                            if (pressedCount > maxPointers) maxPointers = pressedCount
                            if (pressedCount >= 2) {
                                val clicks = twoFinger.onMove(
                                    avgDx / pressedCount * toReference,
                                    avgDy / pressedCount * toReference,
                                    lastEventTime,
                                )
                                twoFingerScrolling = twoFinger.isScroll
                                if (!clicks.isZero) core.remoteScroll(pcId, clicks.x, clicks.y, fast = true)
                                for (i in 0 until changes.size) changes[i].consume()
                            }
                        }

                        padHeld = false
                        twoFingerScrolling = false
                        filter.reset()
                        aimOffset = Offset.Zero

                        val elapsed = System.currentTimeMillis() - downTime
                        if (maxPointers >= 2 && twoFinger.isScroll) {
                            fling = ScrollFling.start(twoFinger.velocity(lastEventTime))
                                ?.let { flingScope.launchScrollFling(core, pcId, it) }
                        } else if (elapsed <= tapTimeoutMs && totalMoved <= tapSlop * 1.5f) {
                            if (maxPointers == 1) {
                                core.remoteButton(pcId, "left", "click", onStatus)
                            } else if (maxPointers == 2 &&
                                twoFinger.onUp(elapsed, tapTimeoutMs) == TwoFingerRelease.RightClick
                            ) {
                                core.remoteButton(pcId, "right", "click", onStatus)
                            }
                        }
                    }
                },
        ) {
            Box(Modifier.fillMaxSize()) {
                Canvas(Modifier.fillMaxSize()) {
                    val center = Offset(size.width / 2f, size.height / 2f)
                    val ringColor = if (padHeld) primaryColor.copy(alpha = 0.35f) else outlineColor.copy(alpha = 0.5f)
                    drawCircle(
                        color = ringColor,
                        radius = 44.dp.toPx(),
                        center = center,
                    )
                    val dotCenter = Offset(
                        x = (center.x + aimOffset.x.dp.toPx() * 0.4f).coerceIn(24.dp.toPx(), size.width - 24.dp.toPx()),
                        y = (center.y + aimOffset.y.dp.toPx() * 0.4f).coerceIn(24.dp.toPx(), size.height - 24.dp.toPx()),
                    )
                    drawCircle(
                        color = if (padHeld) primaryColor else primaryColor.copy(alpha = 0.45f),
                        radius = if (padHeld) 10.dp.toPx() else 6.dp.toPx(),
                        center = dotCenter,
                    )
                }

                Column(
                    horizontalAlignment = Alignment.CenterHorizontally,
                    modifier = Modifier
                        .align(Alignment.TopCenter)
                        .padding(horizontal = 20.dp, vertical = 18.dp),
                ) {
                    Text(
                        text = if (!hasGyro) {
                            stringResource(R.string.remote_air_no_gyro_title)
                        } else if (padHeld) {
                            stringResource(R.string.remote_air_active, pcName)
                        } else {
                            stringResource(R.string.remote_air_hold, pcName)
                        },
                        style = MaterialTheme.typography.titleMedium,
                        textAlign = TextAlign.Center,
                    )
                }

                Text(
                    stringResource(R.string.remote_air_hint),
                    style = MaterialTheme.typography.bodySmall,
                    color = if (padHeld) {
                        MaterialTheme.colorScheme.onPrimaryContainer.copy(alpha = 0.8f)
                    } else {
                        MaterialTheme.colorScheme.onSurfaceVariant
                    },
                    textAlign = TextAlign.Center,
                    modifier = Modifier
                        .align(Alignment.BottomCenter)
                        .padding(horizontal = 20.dp, vertical = 14.dp),
                )
            }
        }

        // Left / Right click buttons under the air mouse pad
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            G7RaisedButton(
                label = stringResource(R.string.remote_left_click),
                onClick = { core.remoteButton(pcId, "left", "click", onStatus) },
                modifier = Modifier.weight(1f),
            )
            G7RaisedButton(
                label = stringResource(R.string.remote_right_click),
                onClick = { core.remoteButton(pcId, "right", "click", onStatus) },
                modifier = Modifier.weight(1f),
            )
        }

        // Voice typing bar (Dictate button: hold to talk, or tap to start/stop)
        VoiceTypingBar(
            pcId = pcId,
            pcName = pcName,
            core = core,
            utteranceJoiner = utteranceJoiner,
            onStatus = onStatus,
        )
    }
}

private enum class DictateError {
    NoRecognizer,
    PermissionNeeded,
    NoSpeech,
    Failed,
}

private sealed interface DictateState {
    data class Idle(val lastTyped: String? = null) : DictateState
    data class Listening(val partial: String = "") : DictateState
    data class Typing(val text: String) : DictateState
    data class Error(val kind: DictateError) : DictateState
}

@Composable
private fun VoiceTypingBar(
    pcId: String,
    pcName: String,
    core: Core,
    utteranceJoiner: UtteranceJoiner,
    onStatus: (Core.RemoteAccess) -> Unit,
) {
    val context = LocalContext.current
    var state by remember(pcId) { mutableStateOf<DictateState>(DictateState.Idle()) }
    var recognizer by remember { mutableStateOf<SpeechRecognizer?>(null) }
    var preferOnDevice by remember { mutableStateOf(true) }

    fun destroyRecognizer() {
        runCatching {
            recognizer?.cancel()
            recognizer?.destroy()
        }
        recognizer = null
    }

    DisposableEffect(pcId) {
        onDispose {
            destroyRecognizer()
        }
    }

    fun startRecognition(allowOnDevice: Boolean = preferOnDevice) {
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            state = DictateState.Error(DictateError.PermissionNeeded)
            return
        }

        destroyRecognizer()

        val useOnDevice = allowOnDevice &&
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            runCatching { SpeechRecognizer.isOnDeviceRecognitionAvailable(context) }.getOrDefault(false)

        val defaultAvailable = runCatching { SpeechRecognizer.isRecognitionAvailable(context) }.getOrDefault(false)
        if (!useOnDevice && !defaultAvailable) {
            state = DictateState.Error(DictateError.NoRecognizer)
            return
        }

        val sr = runCatching {
            if (useOnDevice && Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                SpeechRecognizer.createOnDeviceSpeechRecognizer(context)
            } else {
                SpeechRecognizer.createSpeechRecognizer(context)
            }
        }.getOrNull()

        if (sr == null) {
            state = DictateState.Error(DictateError.NoRecognizer)
            return
        }

        var readyForSpeech = false
        var latestPartial = ""

        sr.setRecognitionListener(object : RecognitionListener {
            override fun onReadyForSpeech(params: Bundle?) {
                readyForSpeech = true
                state = DictateState.Listening(partial = latestPartial)
            }

            override fun onBeginningOfSpeech() {}
            override fun onRmsChanged(rmsdB: Float) {}
            override fun onBufferReceived(buffer: ByteArray?) {}
            override fun onEndOfSpeech() {}

            override fun onPartialResults(partialResults: Bundle?) {
                val partial = partialResults
                    ?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)
                    ?.firstOrNull()
                    ?.let { UtteranceJoiner.sanitizeUtterance(it) }
                    .orEmpty()
                if (partial.isNotEmpty()) {
                    latestPartial = partial
                    state = DictateState.Listening(partial = partial)
                }
            }

            override fun onResults(results: Bundle?) {
                val raw = results
                    ?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)
                    ?.firstOrNull()
                    ?.takeIf { it.isNotBlank() }
                    ?: latestPartial
                val joined = utteranceJoiner.next(raw)
                if (joined.isEmpty()) {
                    state = DictateState.Error(DictateError.NoSpeech)
                    return
                }
                val preview = joined.trimStart()
                state = DictateState.Typing(preview)
                core.remoteText(pcId, joined) { access ->
                    onStatus(access)
                    state = DictateState.Idle(lastTyped = preview)
                }
            }

            override fun onError(error: Int) {
                // If the on-device recognizer fails before speech begins (e.g. its
                // offline model isn't downloaded), fall back to the default service.
                if (useOnDevice && !readyForSpeech) {
                    preferOnDevice = false
                    if (defaultAvailable) {
                        startRecognition(allowOnDevice = false)
                        return
                    }
                }
                if (latestPartial.isNotBlank() &&
                    (error == SpeechRecognizer.ERROR_NO_MATCH || error == SpeechRecognizer.ERROR_SPEECH_TIMEOUT)
                ) {
                    val joined = utteranceJoiner.next(latestPartial)
                    if (joined.isNotEmpty()) {
                        val preview = joined.trimStart()
                        state = DictateState.Typing(preview)
                        core.remoteText(pcId, joined) { access ->
                            onStatus(access)
                            state = DictateState.Idle(lastTyped = preview)
                        }
                        return
                    }
                }
                state = when (error) {
                    SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS ->
                        DictateState.Error(DictateError.PermissionNeeded)
                    SpeechRecognizer.ERROR_NO_MATCH,
                    SpeechRecognizer.ERROR_SPEECH_TIMEOUT ->
                        DictateState.Error(DictateError.NoSpeech)
                    SpeechRecognizer.ERROR_LANGUAGE_NOT_SUPPORTED,
                    SpeechRecognizer.ERROR_LANGUAGE_UNAVAILABLE,
                    SpeechRecognizer.ERROR_CANNOT_CHECK_SUPPORT ->
                        DictateState.Error(DictateError.NoRecognizer)
                    else ->
                        if (!readyForSpeech && error != SpeechRecognizer.ERROR_RECOGNIZER_BUSY) {
                            DictateState.Error(DictateError.NoRecognizer)
                        } else {
                            DictateState.Error(DictateError.Failed)
                        }
                }
            }

            override fun onEvent(eventType: Int, params: Bundle?) {}
        })

        val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
            putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
            putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
        }

        recognizer = sr
        state = DictateState.Listening(partial = "")
        runCatching {
            sr.startListening(intent)
        }.onFailure {
            destroyRecognizer()
            state = DictateState.Error(DictateError.NoRecognizer)
        }
    }

    fun stopRecognition() {
        val sr = recognizer
        if (sr != null) {
            runCatching { sr.stopListening() }
        } else if (state is DictateState.Listening) {
            state = DictateState.Idle()
        }
    }

    val permissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted ->
        if (granted) {
            startRecognition()
        } else {
            state = DictateState.Error(DictateError.PermissionNeeded)
        }
    }

    val isListening = state is DictateState.Listening
    val isError = state is DictateState.Error

    val containerColor = when {
        isListening -> MaterialTheme.colorScheme.primaryContainer
        isError -> MaterialTheme.colorScheme.errorContainer
        else -> MaterialTheme.colorScheme.surfaceContainer
    }
    val contentColor = when {
        isListening -> MaterialTheme.colorScheme.onPrimaryContainer
        isError -> MaterialTheme.colorScheme.onErrorContainer
        else -> MaterialTheme.colorScheme.onSurface
    }

    Surface(
        shape = MaterialTheme.shapes.large,
        color = containerColor,
        contentColor = contentColor,
        modifier = Modifier.fillMaxWidth(),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 12.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Column(modifier = Modifier.weight(1f)) {
                val statusText = when (val s = state) {
                    is DictateState.Idle -> if (s.lastTyped != null) {
                        stringResource(R.string.remote_dictate_typed, pcName, s.lastTyped)
                    } else {
                        stringResource(R.string.remote_dictate_hint, pcName)
                    }
                    is DictateState.Listening -> stringResource(R.string.remote_dictate_listening, pcName)
                    is DictateState.Typing -> stringResource(R.string.remote_dictate_typing, pcName)
                    is DictateState.Error -> when (s.kind) {
                        DictateError.NoRecognizer -> stringResource(R.string.remote_dictate_no_recognizer)
                        DictateError.PermissionNeeded -> stringResource(R.string.remote_dictate_permission_needed, pcName)
                        DictateError.NoSpeech -> stringResource(R.string.remote_dictate_no_speech)
                        DictateError.Failed -> stringResource(R.string.remote_dictate_error)
                    }
                }
                Text(
                    text = statusText,
                    style = MaterialTheme.typography.bodySmall,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )

                val partialOrTyping = when (val s = state) {
                    is DictateState.Listening -> s.partial
                    is DictateState.Typing -> s.text
                    else -> ""
                }
                if (partialOrTyping.isNotEmpty()) {
                    Spacer(Modifier.height(2.dp))
                    Text(
                        text = partialOrTyping,
                        style = MaterialTheme.typography.bodyMedium,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }

            // Dictate button: supports both tap to start/stop and hold to talk.
            val currentIsListening by rememberUpdatedState(isListening)
            val buttonBg = if (isListening) {
                MaterialTheme.colorScheme.primary
            } else {
                MaterialTheme.colorScheme.secondaryContainer
            }
            val buttonFg = if (isListening) {
                MaterialTheme.colorScheme.onPrimary
            } else {
                MaterialTheme.colorScheme.onSecondaryContainer
            }
            val dictateLabel = stringResource(
                if (isListening) R.string.remote_dictate_stop else R.string.remote_dictate,
            )

            Surface(
                shape = MaterialTheme.shapes.small,
                color = buttonBg,
                contentColor = buttonFg,
                modifier = Modifier
                    .height(40.dp)
                    .semantics(mergeDescendants = true) {
                        role = Role.Button
                        contentDescription = dictateLabel
                        onClick(label = dictateLabel) {
                            if (currentIsListening) {
                                stopRecognition()
                            } else if (
                                ContextCompat.checkSelfPermission(
                                    context,
                                    Manifest.permission.RECORD_AUDIO,
                                ) == PackageManager.PERMISSION_GRANTED
                            ) {
                                startRecognition()
                            } else {
                                permissionLauncher.launch(Manifest.permission.RECORD_AUDIO)
                            }
                            true
                        }
                    }
                    .pointerInput(pcId) {
                        val holdThresholdMs = 350L
                        awaitEachGesture {
                            awaitFirstDown(requireUnconsumed = false)
                            val downTime = System.currentTimeMillis()
                            val wasListening = currentIsListening

                            if (wasListening) {
                                while (true) {
                                    val ev = awaitPointerEvent()
                                    if (ev.changes.none { it.pressed }) break
                                }
                                stopRecognition()
                            } else {
                                val hasPerm = ContextCompat.checkSelfPermission(
                                    context,
                                    Manifest.permission.RECORD_AUDIO,
                                ) == PackageManager.PERMISSION_GRANTED

                                if (!hasPerm) {
                                    while (true) {
                                        val ev = awaitPointerEvent()
                                        if (ev.changes.none { it.pressed }) break
                                    }
                                    permissionLauncher.launch(Manifest.permission.RECORD_AUDIO)
                                } else {
                                    startRecognition()
                                    while (true) {
                                        val ev = awaitPointerEvent()
                                        if (ev.changes.none { it.pressed }) break
                                    }
                                    val heldMs = System.currentTimeMillis() - downTime
                                    if (heldMs >= holdThresholdMs) {
                                        stopRecognition()
                                    }
                                }
                            }
                        }
                    },
            ) {
                Box(
                    contentAlignment = Alignment.Center,
                    modifier = Modifier.padding(horizontal = 16.dp),
                ) {
                    Text(
                        text = dictateLabel,
                        style = MaterialTheme.typography.labelLarge,
                        maxLines = 1,
                    )
                }
            }
        }
    }
}

@Composable
private fun SpecialKeysBar(
    activeMods: List<String>,
    showExtraKeys: Boolean,
    onToggleMod: (String) -> Unit,
    onKey: (key: String, forceMods: List<String>?) -> Unit,
) {
    Column(
        modifier = Modifier.fillMaxWidth(),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        // Modifier keys row: 4 equal columns that fit at every font scale without overflowing
        val mods = listOf(
            "ctrl" to stringResource(R.string.remote_key_ctrl),
            "alt" to stringResource(R.string.remote_key_alt),
            "shift" to stringResource(R.string.remote_key_shift),
            "win" to stringResource(R.string.remote_key_win),
        )
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            for ((id, label) in mods) {
                val selected = id in activeMods
                Surface(
                    shape = MaterialTheme.shapes.small,
                    // A held modifier goes with the next key: drawn in the accent so it's never missed.
                    color = if (selected) {
                        MaterialTheme.colorScheme.primary.copy(alpha = 0.18f)
                    } else {
                        MaterialTheme.colorScheme.surfaceContainer
                    },
                    contentColor = if (selected) {
                        MaterialTheme.colorScheme.primary
                    } else {
                        MaterialTheme.colorScheme.onSurface
                    },
                    border = BorderStroke(
                        width = 1.dp,
                        color = if (selected) {
                            MaterialTheme.colorScheme.primary
                        } else {
                            MaterialTheme.colorScheme.outlineVariant
                        },
                    ),
                    modifier = Modifier
                        .weight(1f)
                        .height(36.dp)
                        .semantics(mergeDescendants = true) {
                            role = Role.Switch
                            this.selected = selected
                            contentDescription = label
                        }
                        .clickable { onToggleMod(id) },
                ) {
                    Box(contentAlignment = Alignment.Center, modifier = Modifier.padding(horizontal = 4.dp)) {
                        Text(
                            text = label,
                            style = MaterialTheme.typography.labelMedium,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
            }
        }

        // Navigation & special keys row: 6 equal columns across the full width
        val keys = listOf(
            Triple("escape", stringResource(R.string.remote_key_esc), stringResource(R.string.remote_key_esc_desc)),
            Triple("tab", stringResource(R.string.remote_key_tab), stringResource(R.string.remote_key_tab)),
            Triple("left", "←", stringResource(R.string.remote_key_left_desc)),
            Triple("up", "↑", stringResource(R.string.remote_key_up_desc)),
            Triple("down", "↓", stringResource(R.string.remote_key_down_desc)),
            Triple("right", "→", stringResource(R.string.remote_key_right_desc)),
        )
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            for ((key, label, a11yLabel) in keys) {
                Surface(
                    shape = MaterialTheme.shapes.small,
                    color = MaterialTheme.colorScheme.surface,
                    contentColor = MaterialTheme.colorScheme.onSurface,
                    border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
                    modifier = Modifier
                        .weight(1f)
                        .height(36.dp)
                        .semantics(mergeDescendants = true) {
                            role = Role.Button
                            contentDescription = a11yLabel
                        }
                        .clickable(role = Role.Button) { onKey(key, null) },
                ) {
                    Box(contentAlignment = Alignment.Center, modifier = Modifier.padding(horizontal = 2.dp)) {
                        Text(
                            text = label,
                            style = MaterialTheme.typography.labelMedium,
                            maxLines = 1,
                        )
                    }
                }
            }
        }

        if (showExtraKeys) {
            val extra = listOf(
                Triple("home", stringResource(R.string.remote_key_home), stringResource(R.string.remote_key_home)),
                Triple("end", stringResource(R.string.remote_key_end), stringResource(R.string.remote_key_end)),
                Triple("delete", stringResource(R.string.remote_key_del), stringResource(R.string.remote_key_del_desc)),
                Triple("copy", stringResource(R.string.remote_key_copy), stringResource(R.string.remote_key_copy)),
                Triple("paste", stringResource(R.string.remote_key_paste), stringResource(R.string.remote_key_paste)),
            )
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                for ((key, label, a11yLabel) in extra) {
                    val isShortcut = key == "copy" || key == "paste"
                    Surface(
                        shape = MaterialTheme.shapes.small,
                        color = MaterialTheme.colorScheme.surface,
                        contentColor = MaterialTheme.colorScheme.onSurface,
                        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
                        modifier = Modifier
                            .weight(1f)
                            .height(36.dp)
                            .semantics(mergeDescendants = true) {
                                role = Role.Button
                                contentDescription = a11yLabel
                            }
                            .clickable(role = Role.Button) { onKey(key, if (isShortcut) emptyList() else null) },
                    ) {
                        Box(contentAlignment = Alignment.Center, modifier = Modifier.padding(horizontal = 2.dp)) {
                            Text(
                                text = label,
                                style = MaterialTheme.typography.labelMedium,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun PresentationView(
    pcId: String,
    pcName: String,
    core: Core,
    onStatus: (Core.RemoteAccess) -> Unit,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    var laserHeld by remember { mutableStateOf(false) }
    val laserXState = remember { mutableFloatStateOf(0.5f) }
    val laserYState = remember { mutableFloatStateOf(0.5f) }

    // While held, say it again every second even if it hasn't moved: the PC
    // hides a dot that stops hearing from its phone.
    LaunchedEffect(laserHeld, pcId) {
        while (laserHeld) {
            delay(1000)
            core.remoteLaser(pcId, true, laserXState.floatValue, laserYState.floatValue)
        }
    }

    // Follow the phone's gyroscope / rotation vector while the laser button is held.
    DisposableEffect(laserHeld, pcId) {
        if (!laserHeld) return@DisposableEffect onDispose {}

        val sensorManager = context.getSystemService(Context.SENSOR_SERVICE) as? SensorManager
        val rotationSensor = sensorManager?.getDefaultSensor(Sensor.TYPE_GAME_ROTATION_VECTOR)
            ?: sensorManager?.getDefaultSensor(Sensor.TYPE_ROTATION_VECTOR)

        var baseYaw: Float? = null
        var basePitch: Float? = null
        val rotationMatrix = FloatArray(9)
        val orientation = FloatArray(3)

        val listener = object : SensorEventListener {
            override fun onSensorChanged(event: SensorEvent) {
                SensorManager.getRotationMatrixFromVector(rotationMatrix, event.values)
                SensorManager.getOrientation(rotationMatrix, orientation)
                val yaw = orientation[0]
                val pitch = orientation[1]
                val bYaw = baseYaw ?: yaw.also { baseYaw = it }
                val bPitch = basePitch ?: pitch.also { basePitch = it }

                // ~35 degrees (~0.6 rad) full sweep across the screen
                var dYaw = yaw - bYaw
                if (dYaw > Math.PI.toFloat()) dYaw -= (2.0 * Math.PI).toFloat()
                if (dYaw < -Math.PI.toFloat()) dYaw += (2.0 * Math.PI).toFloat()
                val dPitch = pitch - bPitch

                val nx = (0.5f + dYaw / 0.65f).coerceIn(0f, 1f)
                val ny = (0.5f + dPitch / 0.50f).coerceIn(0f, 1f)
                laserXState.floatValue = nx
                laserYState.floatValue = ny
                core.remoteLaser(pcId, true, nx, ny)
            }

            override fun onAccuracyChanged(sensor: Sensor?, accuracy: Int) {}
        }

        if (rotationSensor != null) {
            sensorManager?.registerListener(listener, rotationSensor, SensorManager.SENSOR_DELAY_GAME)
        }
        onDispose {
            sensorManager?.unregisterListener(listener)
        }
    }

    Column(
        modifier = modifier.fillMaxWidth(),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        // Huge Next Slide button (easy to hit without looking)
        Button(
            onClick = { core.remoteSlide(pcId, "next", onStatus) },
            shape = MaterialTheme.shapes.extraLarge,
            modifier = Modifier
                .fillMaxWidth()
                .height(96.dp),
        ) {
            Text(
                stringResource(R.string.remote_slide_next),
                style = MaterialTheme.typography.headlineSmall,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }

        // Previous Slide button
        FilledTonalButton(
            onClick = { core.remoteSlide(pcId, "previous", onStatus) },
            shape = MaterialTheme.shapes.extraLarge,
            modifier = Modifier
                .fillMaxWidth()
                .height(64.dp),
        ) {
            Text(
                stringResource(R.string.remote_slide_prev),
                style = MaterialTheme.typography.titleLarge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }

        // Start / End / Black screen row
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            G7RaisedButton(
                label = stringResource(R.string.remote_slide_start),
                onClick = { core.remoteSlide(pcId, "start", onStatus) },
                modifier = Modifier.weight(0.98f),
            )
            G7RaisedButton(
                label = stringResource(R.string.remote_slide_stop),
                onClick = { core.remoteSlide(pcId, "stop", onStatus) },
                modifier = Modifier.weight(0.88f),
            )
            G7RaisedButton(
                label = stringResource(R.string.remote_slide_black),
                onClick = { core.remoteSlide(pcId, "black", onStatus) },
                modifier = Modifier.weight(1.22f),
            )
        }

        // Hold for laser pointer pad (supports both gyroscope tilt and finger drag)
        val primaryColor = MaterialTheme.colorScheme.primary
        val laserDesc = stringResource(
            if (laserHeld) R.string.remote_laser_active else R.string.remote_laser_hold,
            pcName,
        )
        Surface(
            shape = MaterialTheme.shapes.extraLarge,
            color = if (laserHeld) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surfaceContainer,
            contentColor = if (laserHeld) MaterialTheme.colorScheme.onPrimaryContainer else MaterialTheme.colorScheme.onSurface,
            border = BorderStroke(
                width = if (laserHeld) 2.dp else 1.dp,
                color = if (laserHeld) primaryColor else MaterialTheme.colorScheme.outlineVariant,
            ),
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .semantics { contentDescription = laserDesc }
                .pointerInput(pcId) {
                    awaitEachGesture {
                        val down = awaitFirstDown(requireUnconsumed = false)
                        val w = size.width.coerceAtLeast(1).toFloat()
                        val h = size.height.coerceAtLeast(1).toFloat()
                        val initX = (down.position.x / w).coerceIn(0f, 1f)
                        val initY = (down.position.y / h).coerceIn(0f, 1f)
                        laserXState.floatValue = initX
                        laserYState.floatValue = initY
                        laserHeld = true
                        core.remoteLaser(pcId, true, initX, initY)

                        while (true) {
                            val event = awaitPointerEvent()
                            val active = event.changes.firstOrNull { it.pressed } ?: break
                            val d = active.positionChange()
                            if (hypot(d.x, d.y) > 0.5f) {
                                val nx = (active.position.x / w).coerceIn(0f, 1f)
                                val ny = (active.position.y / h).coerceIn(0f, 1f)
                                laserXState.floatValue = nx
                                laserYState.floatValue = ny
                                core.remoteLaser(pcId, true, nx, ny)
                                active.consume()
                            }
                        }

                        laserHeld = false
                        core.remoteLaser(pcId, false)
                    }
                },
        ) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                if (laserHeld) {
                    Canvas(Modifier.fillMaxSize()) {
                        val pt = Offset(laserXState.floatValue * size.width, laserYState.floatValue * size.height)
                        drawCircle(
                            color = primaryColor.copy(alpha = 0.28f),
                            radius = 26.dp.toPx(),
                            center = pt,
                        )
                        drawCircle(
                            color = primaryColor,
                            radius = 8.dp.toPx(),
                            center = pt,
                        )
                    }
                }
                Column(
                    horizontalAlignment = Alignment.CenterHorizontally,
                    modifier = Modifier.padding(20.dp),
                ) {
                    Text(
                        stringResource(
                            if (laserHeld) R.string.remote_laser_active else R.string.remote_laser_hold,
                            pcName,
                        ),
                        style = MaterialTheme.typography.titleMedium,
                        textAlign = TextAlign.Center,
                    )
                    Spacer(Modifier.height(6.dp))
                    Text(
                        stringResource(R.string.remote_volume_hint),
                        style = MaterialTheme.typography.bodySmall,
                        color = if (laserHeld) {
                            MaterialTheme.colorScheme.onPrimaryContainer.copy(alpha = 0.8f)
                        } else {
                            MaterialTheme.colorScheme.onSurfaceVariant
                        },
                        textAlign = TextAlign.Center,
                    )
                }
            }
        }
    }
}

/** The screen density (xxhdpi) the touchpad speeds and scroll steps were tuned at. */
