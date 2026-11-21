// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import android.app.Activity
import android.content.Context
import android.content.pm.ActivityInfo
import android.content.res.Configuration
import android.database.ContentObserver
import android.graphics.SurfaceTexture
import android.media.MediaFormat
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.Settings
import android.util.Log
import android.view.OrientationEventListener
import android.view.Surface
import android.view.TextureView
import android.view.WindowManager
import androidx.activity.compose.BackHandler
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.fadeIn
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.RoundRect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Outline
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.layout
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.compose.currentStateAsState
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.mirror.StreamPower
import app.g7link.android.ui.theme.G7Keycap
import app.g7link.android.ui.theme.G7Motion
import app.g7link.android.ui.theme.LocalG7Look
import app.g7link.android.ui.theme.LocalReducedMotion
import app.g7link.android.ui.theme.g7Appear
import app.g7link.android.ui.theme.g7Disappear
import app.g7link.core.MirrorInputEvent
import app.g7link.core.PcMirrorNoticeKind
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.math.roundToInt
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/**
 * Fitted video rectangle inside a container of `(containerWidth, containerHeight)`,
 * accounting for letterbox (top/bottom bars) or pillarbox (left/right bars) offsets.
 */
data class VideoViewportRect(
    val left: Float,
    val top: Float,
    val width: Float,
    val height: Float,
) {
    val right: Float get() = left + width
    val bottom: Float get() = top + height

    fun contains(x: Float, y: Float): Boolean =
        x >= left && x <= right && y >= top && y <= bottom
}

/**
 * Pure coordinate and orientation math for the PC screen viewer (`PcMirrorView`),
 * shared between the live composable and unit tests in `RemoteMathTest`.
 */
object PcMirrorMath {
    private const val DEFAULT_ASPECT = 16f / 9f

    /**
     * Computes the exact fitted video rectangle inside a container of `(containerWidth, containerHeight)`
     * while preserving `videoAspect` (`width / height`), letterboxing or pillarboxing as needed.
     */
    fun computeVideoViewport(
        containerWidth: Float,
        containerHeight: Float,
        videoAspect: Float,
    ): VideoViewportRect {
        val cw = containerWidth.coerceAtLeast(1f)
        val ch = containerHeight.coerceAtLeast(1f)
        val aspect = if (videoAspect.isFinite() && videoAspect > 0f) videoAspect else DEFAULT_ASPECT
        val containerAspect = cw / ch

        return if (containerAspect >= aspect) {
            // Container is wider than the video: pillarbox (bars on left and right).
            val fittedHeight = ch
            val fittedWidth = (ch * aspect).coerceAtMost(cw)
            val left = (cw - fittedWidth) * 0.5f
            VideoViewportRect(
                left = left,
                top = 0f,
                width = fittedWidth,
                height = fittedHeight,
            )
        } else {
            // Container is taller than the video: letterbox (bars on top and bottom).
            val fittedWidth = cw
            val fittedHeight = (cw / aspect).coerceAtMost(ch)
            val top = (ch - fittedHeight) * 0.5f
            VideoViewportRect(
                left = 0f,
                top = top,
                width = fittedWidth,
                height = fittedHeight,
            )
        }
    }

    /**
     * Maps a touch point `(touchX, touchY)` in container pixel coordinates into normalized
     * PC desktop coordinates in `[0f, 1f]`, accounting for letterbox or pillarbox bars.
     *
     * When `clampOutside` is `false` and the point falls in the black letterbox/pillarbox bars,
     * returns `null` so an initial tap on a black bar does not click the edge of the PC screen.
     */
    fun mapTouchToNormalized(
        touchX: Float,
        touchY: Float,
        viewport: VideoViewportRect,
        clampOutside: Boolean = true,
    ): Pair<Float, Float>? {
        if (!clampOutside && !viewport.contains(touchX, touchY)) {
            return null
        }
        val vw = viewport.width.coerceAtLeast(1f)
        val vh = viewport.height.coerceAtLeast(1f)
        val nx = ((touchX - viewport.left) / vw).coerceIn(0f, 1f)
        val ny = ((touchY - viewport.top) / vh).coerceIn(0f, 1f)
        return Pair(nx, ny)
    }

    /**
     * Convenience overload that computes the fitted viewport for `(containerWidth, containerHeight)`
     * and maps `(touchX, touchY)` to normalized PC coordinates in `[0f, 1f]`.
     */
    fun mapTouchToNormalized(
        touchX: Float,
        touchY: Float,
        containerWidth: Float,
        containerHeight: Float,
        videoAspect: Float,
        clampOutside: Boolean = true,
    ): Pair<Float, Float>? {
        val viewport = computeVideoViewport(containerWidth, containerHeight, videoAspect)
        return mapTouchToNormalized(touchX, touchY, viewport, clampOutside)
    }

    fun isPhysicalLandscape(degrees: Int): Boolean =
        degrees in 60..120 || degrees in 240..300

    fun isPhysicalPortrait(degrees: Int): Boolean =
        (degrees in 0..30) || (degrees in 330..359) || (degrees in 150..210)

    fun shouldAutoEnterFullScreen(
        sensorDegrees: Int,
        autoRotateEnabled: Boolean,
        isFullScreen: Boolean,
        suppressWhileInLandscape: Boolean,
    ): Boolean =
        autoRotateEnabled &&
            !isFullScreen &&
            !suppressWhileInLandscape &&
            isPhysicalLandscape(sensorDegrees)

    fun shouldAutoExitFullScreen(
        sensorDegrees: Int,
        autoRotateEnabled: Boolean,
        isFullScreen: Boolean,
        enteredViaButton: Boolean,
        sawPhysicalLandscapeSinceEnter: Boolean,
    ): Boolean =
        autoRotateEnabled &&
            isFullScreen &&
            (!enteredViaButton || sawPhysicalLandscapeSinceEnter) &&
            isPhysicalPortrait(sensorDegrees)
}

private fun isSystemAutoRotateOn(context: Context): Boolean =
    runCatching {
        Settings.System.getInt(
            context.contentResolver,
            Settings.System.ACCELEROMETER_ROTATION,
            0,
        ) == 1
    }.getOrDefault(false)

/** Whether Android's Developer options are on: the switch the Screen tab's stats sit behind. */
private fun isDeveloper(context: Context): Boolean =
    runCatching {
        Settings.Global.getInt(context.contentResolver, Settings.Global.DEVELOPMENT_SETTINGS_ENABLED, 0) == 1
    }.getOrDefault(false)

/** How a PC's screen stream stands, apart from its pictures. */
private enum class MirrorStatus {
    /** Asked for, or showing: the picture tells which. */
    Connecting,

    /** The PC doesn't share its screen with this phone (its toggle). */
    Off,

    /** The PC can't capture its screen. */
    CantCapture,

    /** The PC isn't reached; it starts again once it is. */
    Reconnecting,

    /** The PC ended the stream. */
    Stopped,

    /** Play on phone: the player's window is gone. */
    WindowLost,
}

/** The developer stats line's figures (L6). */
private data class StatsLine(
    val width: Int,
    val height: Int,
    val codec: String,
    val hardware: Boolean,
    val fps: Int,
    val megabits: Float,
    val latencyMs: Long?,
)

/** How wide the space beside the picture has to be to hold the controls. */
private val MIN_RAIL_WIDTH = 72.dp

/** A column of full-screen controls in the space beside the picture. */
@Composable
private fun FullScreenRail(width: Dp, modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    Column(
        modifier = modifier
            .fillMaxHeight()
            .width(width)
            // Taps between the buttons stay here instead of reaching the picture.
            .pointerInput(Unit) { detectTapGestures { } }
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 6.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
        content = content,
    )
}

/** A key on a full-screen rail: the remote's keycap, dark over the black beside the picture. */
@Composable
private fun RailButton(
    label: String,
    haptics: (TrackpadHaptic) -> Unit,
    selected: Boolean = false,
    enabled: Boolean = true,
    description: String = label,
    onClick: () -> Unit,
) {
    G7Keycap(
        onClick = {
            haptics(TrackpadHaptic.Key)
            onClick()
        },
        selected = selected,
        enabled = enabled,
        corner = 11.dp,
        edge = 4.dp,
        contentDescription = description,
        contentPadding = PaddingValues(horizontal = 4.dp),
        modifier = Modifier.fillMaxWidth().heightIn(min = RAIL_KEY_HEIGHT),
    ) {
        Text(
            label,
            style = MaterialTheme.typography.labelMedium,
            textAlign = TextAlign.Center,
            maxLines = 2,
        )
    }
}

/**
 * The theme's dark variant, for what sits over the black of full screen (its
 * rails, its bar, its typing input): dark keys and fields however the phone is set.
 */
@Composable
private fun MirrorDarkTheme(content: @Composable () -> Unit) {
    val look = LocalG7Look.current
    val base = MaterialTheme.colorScheme
    val p = look.deep.palette
    val scheme = remember(look, base) {
        base.copy(
            primary = p.primary,
            onPrimary = p.onPrimary,
            primaryContainer = p.primaryContainer,
            onPrimaryContainer = p.onPrimaryContainer,
            secondary = p.primary,
            onSecondary = p.onPrimary,
            secondaryContainer = p.secondaryContainer,
            onSecondaryContainer = p.onSecondaryContainer,
            background = look.deepGround,
            onBackground = p.onSurface,
            surface = p.surface,
            onSurface = p.onSurface,
            surfaceVariant = p.surfaceContainerHigh,
            onSurfaceVariant = p.onSurfaceVariant,
            surfaceContainerLowest = p.surface,
            surfaceContainerLow = p.surfaceContainerLow,
            surfaceContainer = p.surfaceContainer,
            surfaceContainerHigh = p.surfaceContainerHigh,
            surfaceContainerHighest = p.surfaceContainerHighest,
            outline = p.outline,
            outlineVariant = p.outlineVariant,
            error = look.deep.error,
        )
    }
    CompositionLocalProvider(LocalG7Look provides look.copy(tints = look.deep.tints)) {
        MaterialTheme(colorScheme = scheme, shapes = MaterialTheme.shapes, typography = MaterialTheme.typography) {
            CompositionLocalProvider(LocalContentColor provides p.onSurface, content = content)
        }
    }
}

/** The picture's own rectangle in its box, rounded at [corner], so a fitted picture keeps its corners. */
private class PictureShape(private val rect: VideoViewportRect, private val corner: Float) : Shape {
    override fun createOutline(size: Size, layoutDirection: LayoutDirection, density: Density): Outline =
        Outline.Rounded(RoundRect(rect.left, rect.top, rect.right, rect.bottom, CornerRadius(corner)))
}

/**
 * Live PC-to-Phone Reverse Mirroring viewer ("View Your PC").
 * Decodes the PC's low-latency H.264/H.265/AV1 stream onto a single persistent [TextureView]
 * across portrait and immersive landscape full-screen modes, mapping touch, drag,
 * two-finger scroll, right-click, and keyboard input back to the PC ([ScreenGestures]),
 * and zooming the picture with a pinch ([PcMirrorZoom]).
 *
 * With [watch] (Play on phone), it shows what that PC player plays instead, full screen,
 * and sends the PC nothing: a tap calls [onTap].
 *
 * With [touchpadSensitivity], full screen offers a Touchpad switch: the whole
 * screen then works like a laptop's touchpad instead of pointing directly.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun PcMirrorView(
    pcId: String,
    pcName: String,
    core: Core,
    modifier: Modifier = Modifier,
    isFullScreen: Boolean = false,
    onFullScreenChange: (Boolean) -> Unit = {},
    watch: String? = null,
    onTap: () -> Unit = {},
    touchpadSensitivity: Float? = null,
    onStatus: (Core.RemoteAccess) -> Unit = {},
    showKeyboard: Boolean = false,
    onShowKeyboard: (Boolean) -> Unit = {},
    keyboard: (@Composable () -> Unit)? = null,
    onPcClick: () -> Unit = {},
    haptics: (TrackpadHaptic) -> Unit = {},
) {
    val touchpadFlingScope = rememberCoroutineScope()
    val context = LocalContext.current
    val view = LocalView.current
    val configuration = LocalConfiguration.current
    val reducedMotion = LocalReducedMotion.current
    val canType = keyboard != null && watch == null
    val imeVisible = WindowInsets.isImeVisible
    val currentHaptics by rememberUpdatedState(haptics)
    val currentOnPcClick by rememberUpdatedState(onPcClick)

    val mirrorConfig by core.pcMirrorConfig.collectAsStateWithLifecycle()
    val activeConfig = mirrorConfig?.takeIf { it.pcId == pcId }
    val effectiveCodec = activeConfig?.codec ?: "h264"
    val online by remember(core, pcId) {
        core.state.map { it.device(pcId)?.online == true }.distinctUntilChanged()
    }.collectAsStateWithLifecycle(initialValue = true)

    // Sharp by default: the PC's screen at its own size. The data saver asks for
    // what the old 720p setting did. Kept under a name of its own, so a value
    // saved by that setting (the opposite way round) isn't read as this.
    var dataSaver by rememberSaveable(key = "pcMirrorDataSaver") { mutableStateOf(false) }
    // Full screen only: the screen moves the pointer like a touchpad, for precision.
    var touchpadMode by rememberSaveable { mutableStateOf(false) }
    val touchpadActive = isFullScreen && touchpadMode && touchpadSensitivity != null && watch == null
    var surface by remember { mutableStateOf<Surface?>(null) }
    var hasRenderedFrame by remember(pcId) { mutableStateOf(false) }
    // Whether a picture ever showed, so a lost stream dims the last one instead of going black.
    var shownOnce by remember(pcId) { mutableStateOf(false) }
    // The decoder shows that a picture came after each (re)start, not only its first.
    val frameWanted = remember(pcId) { AtomicBoolean(true) }
    var decoderTick by remember { mutableIntStateOf(0) }
    var startTick by remember { mutableIntStateOf(0) }
    var endedTick by remember { mutableIntStateOf(0) }
    var status by remember(pcId) { mutableStateOf(MirrorStatus.Connecting) }
    var unreachable by remember(pcId) { mutableStateOf(false) }
    var zoom by remember { mutableStateOf(PcMirrorZoom()) }
    var minimapShown by remember { mutableStateOf(false) }
    var dragging by remember { mutableStateOf(false) }
    var rightArmed by remember { mutableStateOf(false) }
    var moreOpen by remember { mutableStateOf(false) }
    val prefs = remember(context) { context.getSharedPreferences(PREFS, Context.MODE_PRIVATE) }
    val developer = remember(context) { isDeveloper(context) }
    var showStats by remember { mutableStateOf(prefs.getBoolean(KEY_SHOW_STATS, false)) }
    var hintSeen by remember { mutableStateOf(prefs.getBoolean(KEY_HINT_SEEN, false)) }
    var statsLine by remember { mutableStateOf<StatsLine?>(null) }

    // Full-screen overlay & auto-rotate state
    var overlayVisible by remember { mutableStateOf(true) }
    var overlayRevealTick by remember { mutableIntStateOf(0) }
    var enteredViaButton by rememberSaveable { mutableStateOf(false) }
    var sawPhysicalLandscapeSinceEnter by rememberSaveable { mutableStateOf(false) }
    var suppressAutoEnterInLandscape by rememberSaveable { mutableStateOf(false) }
    var autoRotateEnabled by remember { mutableStateOf(isSystemAutoRotateOn(context)) }

    val currentIsFullScreen by rememberUpdatedState(isFullScreen)
    val currentOnFullScreenChange by rememberUpdatedState(onFullScreenChange)

    val initialRequestedOrientation = remember {
        (context as? Activity)?.requestedOrientation ?: ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
    }

    fun bumpOverlay() {
        overlayVisible = true
        overlayRevealTick++
    }

    fun enterFullScreen(fromButton: Boolean) {
        enteredViaButton = fromButton
        sawPhysicalLandscapeSinceEnter = !fromButton
        overlayVisible = true
        overlayRevealTick++
        currentOnFullScreenChange(true)
    }

    fun exitFullScreen(fromManualCollapse: Boolean) {
        if (fromManualCollapse) {
            suppressAutoEnterInLandscape = true
        }
        enteredViaButton = false
        sawPhysicalLandscapeSinceEnter = false
        currentOnFullScreenChange(false)
    }

    fun seeHint() {
        if (!hintSeen) {
            hintSeen = true
            prefs.edit().putBoolean(KEY_HINT_SEEN, true).apply()
        }
    }

    BackHandler(enabled = isFullScreen) {
        exitFullScreen(fromManualCollapse = true)
    }

    // Observe Settings.System.ACCELEROMETER_ROTATION changes in real time.
    DisposableEffect(context) {
        autoRotateEnabled = isSystemAutoRotateOn(context)
        val uri = Settings.System.getUriFor(Settings.System.ACCELEROMETER_ROTATION)
        val observer = object : ContentObserver(Handler(Looper.getMainLooper())) {
            override fun onChange(selfChange: Boolean) {
                autoRotateEnabled = isSystemAutoRotateOn(context)
            }
        }
        if (uri != null) {
            runCatching {
                context.contentResolver.registerContentObserver(uri, false, observer)
            }
        }
        onDispose {
            runCatching { context.contentResolver.unregisterContentObserver(observer) }
        }
    }

    // Listen to physical orientation via OrientationEventListener so auto-rotate into and out of
    // full-screen works even while requestedOrientation is locked to SENSOR_LANDSCAPE.
    DisposableEffect(context) {
        val listener = object : OrientationEventListener(context) {
            override fun onOrientationChanged(orientation: Int) {
                if (orientation == ORIENTATION_UNKNOWN) return
                val sysAuto = isSystemAutoRotateOn(context)
                autoRotateEnabled = sysAuto
                if (PcMirrorMath.isPhysicalLandscape(orientation)) {
                    if (currentIsFullScreen) {
                        sawPhysicalLandscapeSinceEnter = true
                    } else if (
                        PcMirrorMath.shouldAutoEnterFullScreen(
                            sensorDegrees = orientation,
                            autoRotateEnabled = sysAuto,
                            isFullScreen = currentIsFullScreen,
                            suppressWhileInLandscape = suppressAutoEnterInLandscape,
                        )
                    ) {
                        enterFullScreen(fromButton = false)
                    }
                } else if (PcMirrorMath.isPhysicalPortrait(orientation)) {
                    if (suppressAutoEnterInLandscape) {
                        suppressAutoEnterInLandscape = false
                        val activity = context as? Activity
                        if (!currentIsFullScreen && sysAuto) {
                            activity?.requestedOrientation = initialRequestedOrientation
                        }
                    }
                    if (
                        PcMirrorMath.shouldAutoExitFullScreen(
                            sensorDegrees = orientation,
                            autoRotateEnabled = sysAuto,
                            isFullScreen = currentIsFullScreen,
                            enteredViaButton = enteredViaButton,
                            sawPhysicalLandscapeSinceEnter = sawPhysicalLandscapeSinceEnter,
                        )
                    ) {
                        exitFullScreen(fromManualCollapse = false)
                    }
                }
            }
        }
        // Watching stays full screen, however the phone is held.
        if (watch == null && listener.canDetectOrientation()) {
            listener.enable()
        }
        onDispose {
            listener.disable()
        }
    }

    // Also follow configuration orientation changes when system auto-rotate is enabled
    // (for example, emulator window rotation).
    LaunchedEffect(configuration.orientation, autoRotateEnabled) {
        if (!autoRotateEnabled || watch != null) return@LaunchedEffect
        if (
            configuration.orientation == Configuration.ORIENTATION_LANDSCAPE &&
            !isFullScreen &&
            !suppressAutoEnterInLandscape
        ) {
            enterFullScreen(fromButton = false)
        } else if (
            configuration.orientation == Configuration.ORIENTATION_PORTRAIT &&
            isFullScreen &&
            !enteredViaButton
        ) {
            exitFullScreen(fromManualCollapse = false)
        }
    }

    var appliedFullScreen by remember { mutableStateOf(false) }

    // Apply orientation, keep-screen-on, and immersive system bars for full-screen vs portrait.
    LaunchedEffect(isFullScreen, autoRotateEnabled, suppressAutoEnterInLandscape, view) {
        val activity = context as? Activity ?: return@LaunchedEffect
        val window = activity.window ?: return@LaunchedEffect
        val controller = WindowCompat.getInsetsController(window, view)
        view.keepScreenOn = true
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)

        if (isFullScreen) {
            appliedFullScreen = true
            activity.requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
            controller.systemBarsBehavior =
                WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            controller.hide(WindowInsetsCompat.Type.systemBars())
        } else if (appliedFullScreen) {
            controller.show(WindowInsetsCompat.Type.systemBars())
            activity.requestedOrientation = if (!autoRotateEnabled || suppressAutoEnterInLandscape) {
                ActivityInfo.SCREEN_ORIENTATION_PORTRAIT
            } else {
                initialRequestedOrientation
            }
        }
    }

    // Restore previous requestedOrientation and system bars when leaving PcMirrorView.
    DisposableEffect(view) {
        val activity = context as? Activity
        val window = activity?.window
        onDispose {
            if (window != null) {
                WindowCompat.getInsetsController(window, view)
                    .show(WindowInsetsCompat.Type.systemBars())
            }
            activity?.requestedOrientation = initialRequestedOrientation
        }
    }

    // Auto-fade the full-screen slim overlay 3 seconds after the last tap or action;
    // it stays while typing, as it holds the switch that closes the keys.
    LaunchedEffect(isFullScreen, overlayVisible, overlayRevealTick, showKeyboard) {
        if (isFullScreen && overlayVisible && !showKeyboard) {
            delay(3_000L)
            overlayVisible = false
        }
    }

    // Nobody's watching once the phone is on something else or its screen is off, and video
    // costs the PC its CPU and the phone its data. Play on phone keeps going: its sound does.
    val lifecycleState by LocalLifecycleOwner.current.lifecycle.currentStateAsState()
    val showing = watch != null || lifecycleState.isAtLeast(Lifecycle.State.STARTED)
    var asked by remember(pcId) { mutableStateOf(false) }

    // While it shows, the phone's Wi-Fi stays awake between packets (StreamPower.VIEW).
    DisposableEffect(showing) {
        StreamPower.set(context, StreamPower.VIEW, showing)
        onDispose { StreamPower.set(context, StreamPower.VIEW, false) }
    }

    LaunchedEffect(pcId, dataSaver, showing, startTick) {
        hasRenderedFrame = false
        frameWanted.set(true)
        if (!showing) {
            if (asked) {
                Log.i("PcMirrorView", "stopPcMirror, not on screen")
                core.stopPcMirror(pcId)
                asked = false
            }
            return@LaunchedEffect
        }
        asked = true
        status = MirrorStatus.Connecting
        // Larger than 1920 only where this phone's decoder takes it.
        val decoderMaxEdge = if (dataSaver) {
            PcMirrorQuality.SHARP_MAX_SIZE
        } else {
            withContext(Dispatchers.Default) { PcMirrorDecoders.maxEdge }
        }
        val request = PcMirrorQuality.request(dataSaver, decoderMaxEdge)
        val asking = "maxSize=${request.maxSize} fps=${PcMirrorQuality.FPS} bitrate=${request.bitrate}"
        Log.i("PcMirrorView", "startPcMirror $asking")
        val refreshRate = view.display?.refreshRate
        withContext(Dispatchers.Default) { logStreamStart(context, refreshRate, asking) }
        val answer = core.startPcMirror(
            pcId,
            maxSize = request.maxSize,
            fps = PcMirrorQuality.FPS,
            bitrate = request.bitrate,
            watch = watch,
        )
        Log.i("PcMirrorView", "startPcMirror: $answer")
        status = when (answer) {
            Core.PcMirrorStartResult.Started -> MirrorStatus.Connecting
            Core.PcMirrorStartResult.Denied -> MirrorStatus.Off
            Core.PcMirrorStartResult.Unsupported -> MirrorStatus.CantCapture
            Core.PcMirrorStartResult.Offline -> MirrorStatus.Reconnecting
        }
    }

    DisposableEffect(pcId) {
        onDispose {
            Log.i("PcMirrorView", "stopPcMirror")
            core.stopPcMirror(pcId)
        }
    }

    // What the PC says besides pictures: the stream ended, or the player's window is gone.
    LaunchedEffect(core, pcId) {
        core.pcMirrorSignals.collect { signal ->
            if (signal.pcId != pcId) return@collect
            when (signal) {
                is Core.PcMirrorSignal.Ended -> endedTick++
                // GNOME asking someone at the PC (C2) shows as connecting until R2 draws it.
                is Core.PcMirrorSignal.Notice -> if (signal.notice == PcMirrorNoticeKind.NO_WINDOW) {
                    status = MirrorStatus.WindowLost
                }
            }
        }
    }
    // A stream that ends with another following at once (a quality change) carries on;
    // one that doesn't, the PC stopped, or the link did.
    LaunchedEffect(endedTick) {
        if (endedTick == 0) return@LaunchedEffect
        delay(ENDED_GRACE_MS)
        if (asked && status == MirrorStatus.Connecting && core.pcMirrorConfig.value?.pcId != pcId) {
            status = if (online) MirrorStatus.Stopped else MirrorStatus.Reconnecting
        }
    }
    // The link went: reconnecting. It came back: start again.
    LaunchedEffect(online) {
        if (!online) {
            if (asked && (status == MirrorStatus.Connecting || status == MirrorStatus.Stopped)) {
                status = MirrorStatus.Reconnecting
            }
        } else if (asked && status == MirrorStatus.Reconnecting) {
            startTick++
        }
    }
    // Reconnecting for 10 s: it says it can't reach the PC, with a way to try again.
    LaunchedEffect(status) {
        unreachable = false
        if (status == MirrorStatus.Reconnecting) {
            delay(REACH_MS)
            unreachable = true
        }
    }

    val decoderMutex = remember { Mutex() }
    var useSoftwareDecoder by remember(pcId, effectiveCodec) { mutableStateOf(false) }

    // The decoder (PcMirrorDecoder) bound to the current Surface and codec. Resolution and
    // orientation changes keep the same Surface and decoder.
    LaunchedEffect(
        pcId,
        surface,
        effectiveCodec,
        useSoftwareDecoder,
        decoderTick,
    ) {
        val targetSurface = surface?.takeIf { it.isValid } ?: return@LaunchedEffect
        hasRenderedFrame = false
        frameWanted.set(true)
        val width = (activeConfig?.width ?: 1280).coerceAtLeast(64)
        val height = (activeConfig?.height ?: 720).coerceAtLeast(64)
        val mime = when (effectiveCodec) {
            "h265" -> MediaFormat.MIMETYPE_VIDEO_HEVC
            "av1" -> "video/av01"
            else -> MediaFormat.MIMETYPE_VIDEO_AVC
        }

        withContext(Dispatchers.Default) {
            decoderMutex.withLock {
                var decoder: PcMirrorDecoder? = null
                try {
                    coroutineScope {
                        val failure = CompletableDeferred<Throwable>()
                        val rendered = AtomicBoolean(false)
                        val stats = MirrorStats()
                        val created = PcMirrorDecoder(
                            mime = mime,
                            width = width,
                            height = height,
                            h264 = effectiveCodec == "h264",
                            surface = targetSurface,
                            preferSoftware = useSoftwareDecoder,
                            fps = PcMirrorQuality.FPS,
                            stats = stats,
                            frameWanted = frameWanted,
                            listener = object : PcMirrorDecoder.Listener {
                                override fun onFirstFrame() {
                                    rendered.set(true)
                                    launch(Dispatchers.Main) {
                                        hasRenderedFrame = true
                                        shownOnce = true
                                    }
                                }

                                override fun onKeyframeWanted() = core.requestPcMirrorKeyframe(pcId)

                                override fun onFailed(error: Throwable) {
                                    failure.complete(error)
                                }
                            },
                        )
                        decoder = created
                        Log.i("PcMirrorView", "decoder ${created.name} (hardware=${created.hardware}) for $mime ${width}x$height")

                        val feeding = launch {
                            core.latestPcMirrorKeyframe(pcId)?.let { created.submit(it) }
                            core.pcMirrorPackets.collect { packet ->
                                if (packet.pcId == pcId) created.submit(packet)
                            }
                        }
                        // Keyframes asked for until the first picture shows.
                        val watchdog = launch {
                            delay(1500L)
                            while (isActive && !rendered.get()) {
                                core.requestPcMirrorKeyframe(pcId)
                                delay(1500L)
                            }
                        }
                        // Each second: the PC's clock, the figures in the log and, for
                        // a developer, on the picture (R0).
                        val measuring = launch {
                            stats.window(SystemClock.elapsedRealtime())
                            while (isActive) {
                                delay(1_000L)
                                val clock = core.pcMirrorClock(pcId)
                                created.clockOffsetUs = clock?.offsetUs
                                val window = stats.window(SystemClock.elapsedRealtime()) ?: continue
                                val config = core.pcMirrorConfig.value?.takeIf { it.pcId == pcId }
                                if (window.frames > 0 || window.bytes > 0L || window.asks > 0L) {
                                    Log.i(
                                        "PcMirrorStats",
                                        "${config?.width ?: 0}x${config?.height ?: 0} ${config?.codec ?: "?"} " +
                                            "decoder=${created.name} hw=${created.hardware} " +
                                            "fps=${"%.1f".format(window.fps)} kbit/s=${(window.megabits * 1000f).roundToInt()} " +
                                            "latency p50=${window.latencyP50 ?: "-"} p95=${window.latencyP95 ?: "-"} ms " +
                                            "arrived-to-shown p50=${window.localP50 ?: "-"} p95=${window.localP95 ?: "-"} ms " +
                                            "missed=${window.missed} dropped=${window.dropped} keyframes=${window.asks} " +
                                            "rtt=${clock?.let { it.rttUs.toLong() / 1000 } ?: "-"} ms",
                                    )
                                }
                                val line = StatsLine(
                                    width = config?.width ?: 0,
                                    height = config?.height ?: 0,
                                    codec = config?.codec ?: "h264",
                                    hardware = created.hardware,
                                    fps = window.fps.roundToInt(),
                                    megabits = window.megabits,
                                    latencyMs = window.latencyP50,
                                )
                                withContext(Dispatchers.Main) { statsLine = line }
                            }
                        }
                        try {
                            throw failure.await()
                        } finally {
                            feeding.cancel()
                            watchdog.cancel()
                            measuring.cancel()
                        }
                    }
                } catch (e: Throwable) {
                    if (e is CancellationException) throw e
                    Log.w("PcMirrorView", "decoder failed (software=$useSoftwareDecoder); retrying", e)
                    if (!useSoftwareDecoder) {
                        withContext(Dispatchers.Main) { useSoftwareDecoder = true }
                        core.requestPcMirrorKeyframe(pcId)
                    } else {
                        delay(800L)
                        withContext(Dispatchers.Main) { decoderTick++ }
                    }
                } finally {
                    decoder?.close()
                }
            }
        }
    }

    val aspect = remember(activeConfig?.width, activeConfig?.height) {
        val w = activeConfig?.width ?: 16
        val h = activeConfig?.height ?: 9
        if (w > 0 && h > 0) w.toFloat() / h.toFloat() else 16f / 9f
    }

    // The zoom follows the PC's screen size; touchpad mode and Play on phone show it whole.
    LaunchedEffect(activeConfig?.width, activeConfig?.height) {
        val config = activeConfig ?: return@LaunchedEffect
        zoom = zoom.ofScreen(config.width, config.height)
    }
    LaunchedEffect(touchpadActive, watch) {
        if (touchpadActive || watch != null) zoom = zoom.fitted()
    }
    // The mini-map shows while the picture is zoomed and moving, and fades 1.5 s after.
    LaunchedEffect(zoom) {
        minimapShown = zoom.isZoomed
        if (minimapShown) {
            delay(MINIMAP_MS)
            minimapShown = false
        }
    }
    // The hint shows once, for a while, the first time a picture shows to point at.
    val hintShown = !hintSeen && hasRenderedFrame && watch == null && !touchpadActive &&
        status == MirrorStatus.Connecting
    LaunchedEffect(hintShown) {
        if (hintShown) {
            delay(HINT_MS)
            seeHint()
        }
    }
    LaunchedEffect(status, touchpadActive) {
        if (status != MirrorStatus.Connecting || touchpadActive) {
            rightArmed = false
            dragging = false
        }
    }

    // The black space beside the picture, where full screen puts its controls.
    var viewportSize by remember { mutableStateOf(IntSize.Zero) }
    val railWidth = with(LocalDensity.current) {
        if (viewportSize.width <= 0 || viewportSize.height <= 0) {
            0.dp
        } else {
            PcMirrorMath.computeVideoViewport(
                viewportSize.width.toFloat(),
                viewportSize.height.toFloat(),
                aspect,
            ).left.toDp()
        }
    }
    val useRails = isFullScreen && watch == null && railWidth >= MIN_RAIL_WIDTH
    val live = status == MirrorStatus.Connecting && hasRenderedFrame
    val currentLive by rememberUpdatedState(live)
    val viewportDescription = stringResource(R.string.pc_mirror_viewport_desc, pcName)
    val fullScreenLabel = stringResource(R.string.pc_mirror_fullscreen)
    val exitFullScreenLabel = stringResource(R.string.pc_mirror_exit_fullscreen)
    val rightClickLabel = stringResource(R.string.pc_mirror_right_click)
    val overviewLabel = stringResource(R.string.pc_mirror_overview)
    val touchpadLabel = stringResource(R.string.remote_mode_touchpad)
    val escLabel = stringResource(R.string.remote_key_esc)
    val escDescription = stringResource(R.string.remote_key_esc_desc)
    val copyLabel = stringResource(R.string.remote_key_copy)
    val pasteLabel = stringResource(R.string.remote_key_paste)
    val undoLabel = stringResource(R.string.remote_key_undo)
    val switchWindowLabel = stringResource(R.string.deck_default_switch_window)
    val moreLabel = stringResource(R.string.pc_mirror_more)
    val keyboardLabel = stringResource(
        if (showKeyboard) R.string.pc_mirror_hide_keyboard else R.string.pc_mirror_keyboard,
    )
    val zoomLabel = zoom.percent?.let { stringResource(R.string.pc_mirror_zoom_percent, it) }
        ?: stringResource(R.string.pc_mirror_zoom_fit)
    val zoomDescription = stringResource(R.string.pc_mirror_zoom_desc, zoomLabel)

    val cycleZoom: () -> Unit = {
        zoom = zoom.cycle()
    }
    // In touchpad mode a right-click goes where the pointer is; pointing, it waits for a tap.
    val rightClick: () -> Unit = {
        if (touchpadActive) {
            core.remoteButton(pcId, "right", "click", onStatus)
        } else {
            rightArmed = !rightArmed
        }
    }
    val overview: () -> Unit = { core.sendPcMirrorInput(pcId, MirrorInputEvent.Key("recents")) }
    val setDataSaver: (Boolean) -> Unit = { saver ->
        if (saver != dataSaver) {
            dataSaver = saver
            decoderTick++
        }
    }
    val setShowStats: (Boolean) -> Unit = { on ->
        showStats = on
        prefs.edit().putBoolean(KEY_SHOW_STATS, on).apply()
    }
    val moreMenu: @Composable () -> Unit = {
        MoreMenu(
            expanded = moreOpen,
            onDismiss = { moreOpen = false },
            dataSaver = dataSaver,
            onDataSaver = setDataSaver,
            developer = developer,
            showStats = showStats,
            onShowStats = setShowStats,
        )
    }

    // Unified Column so AndroidView(TextureView) stays in the exact same composition slot
    // across portrait and full-screen landscape transitions.
    Column(
        modifier = modifier
            .fillMaxSize()
            .then(
                if (isFullScreen) {
                    Modifier
                        .background(Color.Black)
                        .imePadding()
                } else {
                    Modifier
                },
            ),
        verticalArrangement = Arrangement.spacedBy(if (isFullScreen) 0.dp else 10.dp),
    ) {
        // Video viewport container: rounded card in portrait, edge-to-edge black canvas in full screen.
        val outerShape = if (isFullScreen) RoundedCornerShape(0.dp) else RoundedCornerShape(18.dp)
        val pictureCorner = with(LocalDensity.current) { (if (isFullScreen) 0.dp else 12.dp).toPx() }

        Box(
            modifier = Modifier
                .fillMaxWidth()
                .weight(1f)
                .clip(outerShape)
                .background(Color.Black)
                .then(if (dragging) Modifier.border(2.dp, MaterialTheme.colorScheme.primary, outerShape) else Modifier)
                .onSizeChanged { viewportSize = it },
        ) {
            // The picture's own box: between the rails in full screen, the whole card otherwise.
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(horizontal = if (useRails) railWidth else 0.dp)
                    .clipToBounds()
                    .onSizeChanged { zoom = zoom.inBox(it.width.toFloat(), it.height.toFloat()) },
            ) {
                // The picture and its touch. The buttons and states drawn over it are
                // siblings, so a touch on them never reaches the PC.
                Box(
                    modifier = Modifier
                        .fillMaxSize()
                        .semantics { contentDescription = viewportDescription }
                        .pointerInput(pcId, watch, touchpadActive, touchpadSensitivity) {
                            if (watch != null) {
                                // Watching: nothing goes to the PC.
                                detectTapGestures(onTap = { onTap() })
                                return@pointerInput
                            }
                            if (touchpadActive && touchpadSensitivity != null) {
                                touchpadGestures(
                                    pcId = pcId,
                                    core = core,
                                    sensitivity = touchpadSensitivity,
                                    flingScope = touchpadFlingScope,
                                    onStatus = onStatus,
                                    onDragging = { dragging = it },
                                    onTap = { bumpOverlay() },
                                    onHaptic = { currentHaptics(it) },
                                )
                                return@pointerInput
                            }
                            // Pointing at the picture (the gesture table, ScreenGestures): the pointer
                            // goes where the finger is, nothing presses until a gesture is known,
                            // and two fingers scroll, move the zoomed picture, or zoom it.
                            screenGestures(
                                pcId = pcId,
                                core = core,
                                pointer = DirectPadPointer(
                                    core = core,
                                    pcId = pcId,
                                    onStatus = onStatus,
                                    toPc = { x, y -> zoom.toPc(x, y) ?: Pair(0.5f, 0.5f) },
                                    onClick = { currentOnPcClick() },
                                ),
                                zoom = object : ScreenZoom {
                                    override fun zoomed(): Boolean = zoom.isZoomed
                                    // Only a live picture takes touches to the PC.
                                    override fun onPicture(x: Float, y: Float): Boolean =
                                        currentLive && zoom.toPc(x, y, clampOutside = false) != null
                                    override fun move(factor: Float, x: Float, y: Float, dx: Float, dy: Float) {
                                        zoom = zoom.zoomBy(factor, x, y, dx, dy)
                                    }
                                    override fun ended(pinched: Boolean) {
                                        if (!pinched) return
                                        val (settled, snap) = zoom.settle()
                                        zoom = settled
                                        if (snap != null) currentHaptics(TrackpadHaptic.Tap)
                                    }
                                },
                                scope = touchpadFlingScope,
                                onDragging = { dragging = it },
                                onTap = {
                                    seeHint()
                                    if (currentIsFullScreen) bumpOverlay()
                                },
                                rightArmed = { rightArmed },
                                onRightClicked = {
                                    rightArmed = false
                                    seeHint()
                                },
                                onHaptic = { currentHaptics(it) },
                            )
                        },
                ) {
                    // Single persistent TextureView over the whole box, the picture placed in it
                    // (fitted or zoomed) through its transform.
                    val currentZoom by rememberUpdatedState(zoom)
                    AndroidView(
                        factory = { ctx ->
                            TextureView(ctx).apply {
                                isOpaque = false
                                addOnLayoutChangeListener { v, _, _, _, _, _, _, _, _ ->
                                    (v as? TextureView)?.showZoomed(currentZoom)
                                }
                                surfaceTextureListener = object : TextureView.SurfaceTextureListener {
                                    private var activeSt: SurfaceTexture? = null

                                    override fun onSurfaceTextureAvailable(
                                        st: SurfaceTexture,
                                        width: Int,
                                        height: Int,
                                    ) {
                                        if (activeSt === st && surface?.isValid == true) {
                                            return
                                        }
                                        activeSt = st
                                        surface?.release()
                                        surface = Surface(st)
                                    }

                                    override fun onSurfaceTextureSizeChanged(
                                        st: SurfaceTexture,
                                        width: Int,
                                        height: Int,
                                    ) = Unit

                                    override fun onSurfaceTextureDestroyed(st: SurfaceTexture): Boolean {
                                        if (activeSt === st) {
                                            activeSt = null
                                        }
                                        surface?.release()
                                        surface = null
                                        return true
                                    }

                                    override fun onSurfaceTextureUpdated(st: SurfaceTexture) = Unit
                                }
                            }
                        },
                        update = { it.showZoomed(zoom) },
                        modifier = Modifier
                            .fillMaxSize()
                            .clip(PictureShape(zoom.rect, pictureCorner)),
                    )
                }

                // The developer's stats line (L6), top-left over the picture.
                val line = statsLine
                if (developer && showStats && line != null && live) {
                    StatsText(line, Modifier.align(Alignment.TopStart).padding(8.dp))
                }

                // Where the zoomed picture is on the PC's screen (P3, L5).
                androidx.compose.animation.AnimatedVisibility(
                    visible = minimapShown && live,
                    enter = g7Appear(),
                    exit = g7Disappear(),
                    modifier = Modifier.align(Alignment.TopEnd).padding(10.dp),
                ) {
                    MiniMap(zoom = zoom, aspect = aspect)
                }

                // One chip at the top: dragging, a right-click waiting for its tap, or the hint.
                val chip = when {
                    !live -> null
                    dragging -> stringResource(R.string.pc_mirror_dragging)
                    rightArmed -> stringResource(R.string.pc_mirror_right_click_armed)
                    hintShown -> stringResource(R.string.pc_mirror_direct_hint)
                    else -> null
                }
                androidx.compose.animation.AnimatedVisibility(
                    visible = chip != null,
                    enter = g7Appear(),
                    exit = g7Disappear(),
                    modifier = Modifier.align(Alignment.TopCenter).padding(top = 10.dp),
                ) {
                    if (chip != null) Chip(chip)
                }

                // Zoom chip and full screen, in the picture's lower corners (P1).
                if (!isFullScreen && watch == null && live) {
                    PictureButton(
                        description = zoomDescription,
                        onClick = {
                            currentHaptics(TrackpadHaptic.Key)
                            cycleZoom()
                        },
                        modifier = Modifier.align(Alignment.BottomStart),
                    ) {
                        Text(zoomLabel, style = MaterialTheme.typography.labelLarge, color = Color.White)
                    }
                }
                if (!isFullScreen && watch == null) {
                    PictureButton(
                        description = fullScreenLabel,
                        onClick = { enterFullScreen(fromButton = true) },
                        modifier = Modifier.align(Alignment.BottomEnd),
                        round = true,
                    ) {
                        ExpandCollapseIcon(isFullScreen = false)
                    }
                }

                MirrorStateOverlay(
                    status = status,
                    live = live,
                    shownOnce = shownOnce,
                    unreachable = unreachable,
                    pcName = pcName,
                    watching = watch != null,
                    watchTitle = watchTitle(core, pcId, watch),
                    onRetry = { startTick++ },
                    onClose = { exitFullScreen(fromManualCollapse = true) },
                    modifier = Modifier.fillMaxSize(),
                )
            }

            // Landscape full screen leaves black space beside the picture: the controls
            // live there, always shown, and never cover the PC's screen.
            if (useRails) {
                MirrorDarkTheme {
                    FullScreenRail(width = railWidth, modifier = Modifier.align(Alignment.CenterStart)) {
                        if (touchpadSensitivity != null) {
                            RailButton(touchpadLabel, currentHaptics, selected = touchpadMode) { touchpadMode = !touchpadMode }
                        }
                        RailButton(
                            rightClickLabel,
                            currentHaptics,
                            selected = rightArmed && !touchpadActive,
                            onClick = rightClick,
                        )
                        if (canType) {
                            RailButton(keyboardLabel, currentHaptics, selected = showKeyboard) { onShowKeyboard(!showKeyboard) }
                        }
                        RailButton(escLabel, currentHaptics, description = escDescription) {
                            core.remoteKey(pcId, "escape", onStatus = onStatus)
                        }
                        RailButton(copyLabel, currentHaptics) { core.remoteKey(pcId, "copy", onStatus = onStatus) }
                        RailButton(pasteLabel, currentHaptics) { core.remoteKey(pcId, "paste", onStatus = onStatus) }
                    }
                    FullScreenRail(width = railWidth, modifier = Modifier.align(Alignment.CenterEnd)) {
                        RailButton(overviewLabel, currentHaptics, onClick = overview)
                        RailButton(switchWindowLabel, currentHaptics) { core.remoteKey(pcId, "tab", listOf("alt"), onStatus) }
                        RailButton(undoLabel, currentHaptics) { core.remoteKey(pcId, "undo", onStatus = onStatus) }
                        RailButton(
                            zoomLabel,
                            currentHaptics,
                            enabled = !touchpadActive,
                            description = zoomDescription,
                            onClick = cycleZoom,
                        )
                        Box {
                            RailButton(moreLabel, currentHaptics, selected = moreOpen) { moreOpen = true }
                            moreMenu()
                        }
                        IconButton(
                            onClick = { exitFullScreen(fromManualCollapse = true) },
                            colors = IconButtonDefaults.iconButtonColors(
                                containerColor = Color.White.copy(alpha = 0.12f),
                                contentColor = Color.White,
                            ),
                            modifier = Modifier
                                .size(44.dp)
                                .semantics { contentDescription = exitFullScreenLabel },
                        ) {
                            ExpandCollapseIcon(isFullScreen = true)
                        }
                    }
                }
            }

            // Slim overlay row in full screen: appears on tap and fades out after 3 s
            if (isFullScreen && watch == null && !useRails) {
                androidx.compose.animation.AnimatedVisibility(
                    visible = overlayVisible,
                    enter = if (reducedMotion) EnterTransition.None else fadeIn(G7Motion.enter(G7Motion.SMALL_IN)),
                    exit = g7Disappear(),
                    modifier = Modifier
                        .align(Alignment.BottomCenter)
                        .padding(horizontal = 16.dp, vertical = 12.dp),
                ) {
                    Surface(
                        shape = RoundedCornerShape(24.dp),
                        color = Color(0xCC14161B),
                        contentColor = Color.White,
                        border = BorderStroke(1.dp, Color.White.copy(alpha = 0.16f)),
                    ) {
                        Row(
                            modifier = Modifier
                                .horizontalScroll(rememberScrollState())
                                .padding(horizontal = 10.dp, vertical = 4.dp),
                            horizontalArrangement = Arrangement.spacedBy(4.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            BarButton(rightClickLabel, selected = rightArmed && !touchpadActive) {
                                bumpOverlay()
                                rightClick()
                            }
                            BarButton(overviewLabel) {
                                bumpOverlay()
                                overview()
                            }
                            if (canType) {
                                BarButton(keyboardLabel, selected = showKeyboard) {
                                    bumpOverlay()
                                    onShowKeyboard(!showKeyboard)
                                }
                            }
                            if (touchpadSensitivity != null) {
                                BarButton(touchpadLabel, selected = touchpadMode) {
                                    bumpOverlay()
                                    touchpadMode = !touchpadMode
                                }
                            }
                            if (!touchpadActive) {
                                BarButton(zoomLabel, description = zoomDescription) {
                                    bumpOverlay()
                                    cycleZoom()
                                }
                            }
                            Box {
                                BarButton(moreLabel, selected = moreOpen) {
                                    bumpOverlay()
                                    moreOpen = true
                                }
                                moreMenu()
                            }
                            IconButton(
                                onClick = { exitFullScreen(fromManualCollapse = true) },
                                colors = IconButtonDefaults.iconButtonColors(
                                    containerColor = Color.White.copy(alpha = 0.12f),
                                    contentColor = Color.White,
                                ),
                                modifier = Modifier
                                    .size(34.dp)
                                    .semantics { contentDescription = exitFullScreenLabel },
                            ) {
                                ExpandCollapseIcon(isFullScreen = true)
                            }
                        }
                    }
                }
            }
        }

        // The Touchpad tab's typing input (TypingInput.kt), under the picture: the column
        // is above the system keyboard, so the picture shrinks to the room left and the
        // keyboard never covers it. Where room is short (landscape), the input takes at
        // most part of it, its keys scrolling and its field kept in sight at the bottom.
        // Over a full-screen picture it's dark, whatever the phone's theme.
        if (showKeyboard && canType && keyboard != null) {
            if (isFullScreen) {
                MirrorDarkTheme {
                    Box(
                        modifier = Modifier
                            .fillMaxWidth()
                            .atMostFractionOfHeight(KEYBOARD_ROOM)
                            .background(LocalG7Look.current.deepGround)
                            .padding(horizontal = 8.dp, vertical = 6.dp)
                            .verticalScroll(rememberScrollState(), reverseScrolling = true),
                    ) {
                        keyboard()
                    }
                }
            } else {
                Box(
                    modifier = Modifier
                        .fillMaxWidth()
                        .atMostFractionOfHeight(KEYBOARD_ROOM)
                        .verticalScroll(rememberScrollState(), reverseScrolling = true),
                ) {
                    keyboard()
                }
            }
        }

        // Portrait action bar. It gives its room to the picture while the system
        // keyboard is up, as the remote's toolbar does.
        if (!isFullScreen && watch == null && !(showKeyboard && imeVisible)) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                // Keys as on every remote tab: raised, with a tap in the hand.
                G7Keycap(
                    label = rightClickLabel,
                    onClick = {
                        haptics(TrackpadHaptic.Key)
                        rightClick()
                    },
                    selected = rightArmed,
                    modifier = Modifier.weight(1.25f).height(ACTION_KEY_HEIGHT),
                )
                G7Keycap(
                    label = overviewLabel,
                    onClick = {
                        haptics(TrackpadHaptic.Key)
                        overview()
                    },
                    modifier = Modifier.weight(1.1f).height(ACTION_KEY_HEIGHT),
                )
                if (canType) {
                    G7Keycap(
                        label = keyboardLabel,
                        onClick = {
                            haptics(TrackpadHaptic.Key)
                            onShowKeyboard(!showKeyboard)
                        },
                        selected = showKeyboard,
                        modifier = Modifier.weight(1f).height(ACTION_KEY_HEIGHT),
                    )
                }
                Box(modifier = Modifier.weight(0.9f)) {
                    G7Keycap(
                        label = moreLabel,
                        onClick = {
                            haptics(TrackpadHaptic.Key)
                            moreOpen = true
                        },
                        selected = moreOpen,
                        modifier = Modifier.fillMaxWidth().height(ACTION_KEY_HEIGHT),
                    )
                    moreMenu()
                }
            }
        }
    }
}

/** What the player on the PC plays, for Play on phone's starting state (C10). */
@Composable
private fun watchTitle(core: Core, pcId: String, watch: String?): String? {
    if (watch == null) return null
    val players by core.pcMedia.everyPlayer.collectAsStateWithLifecycle()
    return players.firstOrNull { it.pc == pcId && it.playerId == watch }?.title?.takeIf { it.isNotBlank() }
}

/**
 * What the picture area says when there's no live picture (§3.3): connecting
 * (C1, C10), sharing off (C3), can't capture (C8), reconnecting and then can't
 * reach (C5), stopped (C7) and the player's window lost (C11).
 */
@Composable
private fun MirrorStateOverlay(
    status: MirrorStatus,
    live: Boolean,
    shownOnce: Boolean,
    unreachable: Boolean,
    pcName: String,
    watching: Boolean,
    watchTitle: String?,
    onRetry: () -> Unit,
    onClose: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val shown = when (status) {
        MirrorStatus.Connecting -> !live
        else -> true
    }
    androidx.compose.animation.AnimatedVisibility(
        visible = shown,
        enter = g7Appear(),
        exit = g7Disappear(),
        modifier = modifier,
    ) {
        // The last picture stays, dimmed, while the PC is away or has stopped; anything
        // else is said on black.
        val dim = when (status) {
            MirrorStatus.Reconnecting -> if (shownOnce) 0.4f else 1f
            MirrorStatus.Stopped -> if (shownOnce) 0.6f else 1f
            else -> 1f
        }
        Box(
            modifier = Modifier
                .fillMaxSize()
                .background(Color.Black.copy(alpha = dim))
                // Touches on a state don't reach the PC.
                .pointerInput(Unit) { detectTapGestures { } },
            contentAlignment = Alignment.Center,
        ) {
            when (status) {
                MirrorStatus.Connecting -> if (watching) {
                    StateMessage(
                        title = stringResource(R.string.pc_mirror_watch_starting),
                        body = watchTitle,
                        spinner = true,
                        action = stringResource(R.string.action_cancel),
                        onAction = onClose,
                    )
                } else {
                    StateMessage(title = stringResource(R.string.pc_mirror_connecting, pcName), spinner = true)
                }
                MirrorStatus.Off -> StateMessage(
                    title = stringResource(R.string.pc_mirror_off_title, pcName),
                    body = stringResource(R.string.pc_mirror_off_text, pcName),
                )
                MirrorStatus.CantCapture -> StateMessage(
                    title = stringResource(R.string.pc_mirror_cant_capture_title, pcName),
                    body = stringResource(R.string.pc_mirror_cant_capture_text, pcName),
                )
                MirrorStatus.WindowLost -> StateMessage(
                    title = stringResource(R.string.pc_mirror_window_lost_title),
                    body = stringResource(R.string.pc_mirror_window_lost_text, pcName),
                    action = stringResource(R.string.watch_close),
                    onAction = onClose,
                )
                MirrorStatus.Stopped -> StateMessage(
                    title = stringResource(R.string.pc_mirror_stopped, pcName),
                    action = stringResource(R.string.pc_mirror_start_again),
                    accent = true,
                    onAction = onRetry,
                )
                MirrorStatus.Reconnecting -> if (unreachable) {
                    Surface(shape = RoundedCornerShape(14.dp), color = Color(0xFF222226), contentColor = Color(0xFFF2F2F3)) {
                        Column(
                            modifier = Modifier
                                .widthIn(min = 220.dp)
                                .padding(horizontal = 20.dp, vertical = 18.dp)
                                .semantics { liveRegion = LiveRegionMode.Polite },
                            horizontalAlignment = Alignment.CenterHorizontally,
                            verticalArrangement = Arrangement.spacedBy(12.dp),
                        ) {
                            Text(
                                stringResource(R.string.pc_mirror_unreachable, pcName),
                                style = MaterialTheme.typography.titleMedium,
                                textAlign = TextAlign.Center,
                            )
                            StateButton(stringResource(R.string.action_try_again), accent = true, onClick = onRetry)
                        }
                    }
                } else {
                    Row(
                        modifier = Modifier
                            .heightIn(min = 34.dp)
                            .background(Color.Black.copy(alpha = 0.72f), RoundedCornerShape(17.dp))
                            .padding(horizontal = 14.dp)
                            .semantics { liveRegion = LiveRegionMode.Polite },
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        CircularProgressIndicator(modifier = Modifier.size(16.dp), strokeWidth = 2.dp, color = Color.White)
                        Text(
                            stringResource(R.string.pc_mirror_reconnecting),
                            style = MaterialTheme.typography.labelLarge,
                            color = Color.White,
                        )
                    }
                }
            }
        }
    }
}

/** A state's message: a title, a line under it and one button, on the picture's black. */
@Composable
private fun StateMessage(
    title: String,
    body: String? = null,
    spinner: Boolean = false,
    action: String? = null,
    accent: Boolean = false,
    onAction: () -> Unit = {},
) {
    Column(
        modifier = Modifier
            .widthIn(max = 290.dp)
            .padding(16.dp)
            .semantics { liveRegion = LiveRegionMode.Polite },
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (spinner) {
            CircularProgressIndicator(
                modifier = Modifier.padding(bottom = 4.dp).size(28.dp),
                strokeWidth = 2.5.dp,
                color = Color.White,
            )
        }
        Text(title, style = MaterialTheme.typography.titleMedium, color = Color.White, textAlign = TextAlign.Center)
        if (body != null) {
            Text(body, style = MaterialTheme.typography.bodyMedium, color = Color(0xFFC4C4CB), textAlign = TextAlign.Center)
        }
        if (action != null) {
            StateButton(action, accent = accent, onClick = onAction, modifier = Modifier.padding(top = 8.dp))
        }
    }
}

@Composable
private fun StateButton(label: String, accent: Boolean, onClick: () -> Unit, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    Button(
        onClick = onClick,
        shape = RoundedCornerShape(10.dp),
        colors = ButtonDefaults.buttonColors(
            containerColor = if (accent) scheme.primary else Color(0xFF2B2B30),
            contentColor = if (accent) scheme.onPrimary else Color(0xFFF2F2F3),
        ),
        modifier = modifier.heightIn(min = 44.dp).widthIn(min = 96.dp),
    ) {
        Text(label, style = MaterialTheme.typography.labelLarge)
    }
}

/** A chip over the top of the picture: dragging, a waiting right-click, the hint. */
@Composable
private fun Chip(text: String) {
    Box(
        modifier = Modifier
            .padding(horizontal = 16.dp)
            .widthIn(max = 320.dp)
            .heightIn(min = 34.dp)
            .background(Color.Black.copy(alpha = 0.72f), RoundedCornerShape(17.dp))
            .padding(horizontal = 14.dp, vertical = 7.dp)
            .semantics { liveRegion = LiveRegionMode.Polite },
        contentAlignment = Alignment.Center,
    ) {
        Text(text, style = MaterialTheme.typography.labelLarge, color = Color.White, textAlign = TextAlign.Center)
    }
}

/** A round button in a corner of the portrait picture: the zoom chip, full screen. */
@Composable
private fun PictureButton(
    description: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    round: Boolean = false,
    content: @Composable () -> Unit,
) {
    Box(
        modifier = modifier
            .padding(6.dp)
            .heightIn(min = 44.dp)
            .widthIn(min = 44.dp)
            .clickable(role = Role.Button, onClick = onClick)
            .semantics { contentDescription = description }
            .padding(4.dp),
        contentAlignment = Alignment.Center,
    ) {
        Box(
            modifier = Modifier
                .height(36.dp)
                .widthIn(min = 36.dp)
                .background(Color.Black.copy(alpha = 0.66f), RoundedCornerShape(18.dp))
                .padding(horizontal = if (round) 0.dp else 13.dp),
            contentAlignment = Alignment.Center,
        ) {
            content()
        }
    }
}

/** A button in the slim full-screen bar. */
@Composable
private fun BarButton(label: String, selected: Boolean = false, description: String = label, onClick: () -> Unit) {
    TextButton(
        onClick = onClick,
        colors = ButtonDefaults.textButtonColors(
            contentColor = if (selected) MaterialTheme.colorScheme.primary else Color.White,
        ),
        contentPadding = PaddingValues(horizontal = 10.dp, vertical = 6.dp),
        modifier = Modifier
            .height(36.dp)
            .semantics {
                contentDescription = description
                this.selected = selected
            },
    ) {
        Text(label, style = MaterialTheme.typography.labelLarge, maxLines = 1)
    }
}

/** The Screen tab's More: the picture's quality and, for a developer, the stats line. */
@Composable
private fun MoreMenu(
    expanded: Boolean,
    onDismiss: () -> Unit,
    dataSaver: Boolean,
    onDataSaver: (Boolean) -> Unit,
    developer: Boolean,
    showStats: Boolean,
    onShowStats: (Boolean) -> Unit,
) {
    DropdownMenu(expanded = expanded, onDismissRequest = onDismiss) {
        DropdownMenuItem(
            text = { Text(stringResource(R.string.pc_mirror_quality_sharp)) },
            onClick = {
                onDataSaver(false)
                onDismiss()
            },
            trailingIcon = { RadioButton(selected = !dataSaver, onClick = null) },
        )
        DropdownMenuItem(
            text = { Text(stringResource(R.string.pc_mirror_quality_saver)) },
            onClick = {
                onDataSaver(true)
                onDismiss()
            },
            trailingIcon = { RadioButton(selected = dataSaver, onClick = null) },
        )
        if (developer) {
            DropdownMenuItem(
                text = { Text(stringResource(R.string.pc_mirror_show_stats)) },
                onClick = { onShowStats(!showStats) },
                trailingIcon = { Switch(checked = showStats, onCheckedChange = null) },
            )
        }
    }
}

/** The developer's stats line (L6): size, codec, decoder, frame rate, data rate and latency. */
@Composable
private fun StatsText(line: StatsLine, modifier: Modifier = Modifier) {
    val codec = when (line.codec) {
        "h265" -> "HEVC"
        "av1" -> "AV1"
        else -> "H.264"
    }
    val decoder = stringResource(if (line.hardware) R.string.pc_mirror_stats_hardware else R.string.pc_mirror_stats_software)
    val text = stringResource(
        R.string.pc_mirror_stats_line,
        line.width,
        line.height,
        codec,
        decoder,
        line.fps,
        "%.1f".format(line.megabits),
        line.latencyMs?.toString() ?: "–",
    )
    Text(
        text,
        style = MaterialTheme.typography.labelSmall.copy(fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace, fontSize = 11.sp),
        color = Color.White,
        maxLines = 1,
        modifier = modifier
            .background(Color.Black.copy(alpha = 0.72f), RoundedCornerShape(6.dp))
            .padding(horizontal = 8.dp, vertical = 6.dp),
    )
}

/** The whole PC screen in outline, the part in sight filled (P3, L5). */
@Composable
private fun MiniMap(zoom: PcMirrorZoom, aspect: Float) {
    val description = stringResource(R.string.pc_mirror_minimap_desc)
    val visible = zoom.visible
    Canvas(
        modifier = Modifier
            .width(MINIMAP_WIDTH)
            .height(MINIMAP_WIDTH / aspect.coerceIn(0.5f, 4f))
            .semantics { contentDescription = description },
    ) {
        val stroke = 1.dp.toPx()
        drawRoundRect(color = Color.Black.copy(alpha = 0.66f), cornerRadius = CornerRadius(3.dp.toPx()))
        drawRoundRect(
            color = Color.White.copy(alpha = 0.85f),
            cornerRadius = CornerRadius(3.dp.toPx()),
            style = Stroke(width = stroke),
        )
        drawRect(
            color = Color.White.copy(alpha = 0.85f),
            topLeft = androidx.compose.ui.geometry.Offset(visible.left * size.width, visible.top * size.height),
            size = Size(visible.width * size.width, visible.height * size.height),
        )
    }
}

/** The portrait keys under the picture, as tall as the remote's mouse buttons. */
private val ACTION_KEY_HEIGHT = 48.dp

/** The keys on the full-screen rails: 44 dp to touch, over their edge. */
private val RAIL_KEY_HEIGHT = 48.dp

private val MINIMAP_WIDTH = 64.dp

/** The most of the room under the picture the typing input takes. */
private const val KEYBOARD_ROOM = 0.55f

/** How long a stream that ended waits for another to follow before it's over. */
private const val ENDED_GRACE_MS = 1_500L

/** How long reconnecting tries before it says it can't reach the PC. */
private const val REACH_MS = 10_000L

/** How long the mini-map stays after the picture last moved. */
private const val MINIMAP_MS = 1_500L

/** How long the pointing hint shows, the once. */
private const val HINT_MS = 8_000L

private const val PREFS = "pc_mirror"
private const val KEY_SHOW_STATS = "show_stats"
private const val KEY_HINT_SEEN = "direct_hint_seen"

/** At most [fraction] of the height on offer, however tall the content. */
private fun Modifier.atMostFractionOfHeight(fraction: Float): Modifier = layout { measurable, constraints ->
    val most = if (constraints.hasBoundedHeight) {
        (constraints.maxHeight * fraction).roundToInt().coerceAtLeast(constraints.minHeight)
    } else {
        constraints.maxHeight
    }
    val placeable = measurable.measure(constraints.copy(maxHeight = most))
    layout(placeable.width, placeable.height) { placeable.place(0, 0) }
}

@Composable
private fun ExpandCollapseIcon(
    isFullScreen: Boolean,
    modifier: Modifier = Modifier,
) {
    val path = remember { Path() }
    Canvas(modifier = modifier.size(18.dp)) {
        val w = size.width
        val h = size.height
        val stroke = Stroke(
            width = 1.9.dp.toPx(),
            cap = StrokeCap.Round,
            join = StrokeJoin.Round,
        )
        fun sx(v: Float) = (v / 24f) * w
        fun sy(v: Float) = (v / 24f) * h

        path.rewind()
        if (!isFullScreen) {
            // Four outward corner brackets (expand to full screen)
            path.moveTo(sx(4f), sy(9f))
            path.lineTo(sx(4f), sy(4f))
            path.lineTo(sx(9f), sy(4f))

            path.moveTo(sx(15f), sy(4f))
            path.lineTo(sx(20f), sy(4f))
            path.lineTo(sx(20f), sy(9f))

            path.moveTo(sx(20f), sy(15f))
            path.lineTo(sx(20f), sy(20f))
            path.lineTo(sx(15f), sy(20f))

            path.moveTo(sx(9f), sy(20f))
            path.lineTo(sx(4f), sy(20f))
            path.lineTo(sx(4f), sy(15f))
        } else {
            // Four inward corner brackets (collapse full screen)
            path.moveTo(sx(4f), sy(9f))
            path.lineTo(sx(9f), sy(9f))
            path.lineTo(sx(9f), sy(4f))

            path.moveTo(sx(15f), sy(4f))
            path.lineTo(sx(15f), sy(9f))
            path.lineTo(sx(20f), sy(9f))

            path.moveTo(sx(20f), sy(15f))
            path.lineTo(sx(15f), sy(15f))
            path.lineTo(sx(15f), sy(20f))

            path.moveTo(sx(9f), sy(20f))
            path.lineTo(sx(9f), sy(15f))
            path.lineTo(sx(4f), sy(15f))
        }
        drawPath(path = path, color = Color.White, style = stroke)
    }
}
