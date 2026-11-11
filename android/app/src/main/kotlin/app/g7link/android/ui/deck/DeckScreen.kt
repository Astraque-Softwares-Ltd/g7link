// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.deck

import app.g7link.android.ui.theme.G7Slider
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.SliderDefaults
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.g7link.android.ui.theme.G7Header
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.G7Tabs
import app.g7link.android.ui.theme.G7Tints
import app.g7link.android.ui.theme.LocalQuietTheme
import app.g7link.android.ui.theme.g7Tints
import android.app.Activity
import android.content.Context
import android.content.res.Configuration
import android.view.HapticFeedbackConstants
import android.view.WindowManager
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.snap
import androidx.compose.animation.core.spring
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.foundation.Canvas
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.home.PcAudioOutputsDialog
import app.g7link.android.ui.theme.LocalReducedMotion
import app.g7link.core.DeckLayout
import app.g7link.core.DeckPage
import app.g7link.core.DeckState
import app.g7link.core.DeckTile
import kotlin.math.roundToInt
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

@Composable
fun DeckScreen(
    device: Device,
    core: Core,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val view = LocalView.current
    val context = LocalContext.current
    val configuration = LocalConfiguration.current
    val isLandscape = configuration.orientation == Configuration.ORIENTATION_LANDSCAPE
    val scope = rememberCoroutineScope()

    var fullScreen by rememberSaveable { mutableStateOf(false) }
    var access by remember(device.id) { mutableStateOf(Core.RemoteAccess.Allowed) }
    var commandsDenied by remember(device.id) { mutableStateOf(false) }
    /** Tiles waiting for the PC, or just done, by tile id; the press count tells a later press from an earlier one. */
    val tileProgress = remember(device.id) { mutableStateMapOf<String, DeckTileProgress>() }
    val tilePresses = remember(device.id) { mutableMapOf<String, Int>() }

    // Keep the phone's screen on while Deck is open, and restore system bars on exit.
    DisposableEffect(view, device.id) {
        val window = (context as? Activity)?.window
        val prevKeepOn = view.keepScreenOn
        view.keepScreenOn = true
        window?.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        onDispose {
            view.keepScreenOn = prevKeepOn
            window?.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            if (window != null) {
                WindowCompat.getInsetsController(window, view)
                    .show(WindowInsetsCompat.Type.systemBars())
            }
        }
    }

    LaunchedEffect(fullScreen, view) {
        val window = (context as? Activity)?.window ?: return@LaunchedEffect
        val controller = WindowCompat.getInsetsController(window, view)
        if (fullScreen) {
            controller.systemBarsBehavior =
                WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            controller.hide(WindowInsetsCompat.Type.systemBars())
        } else {
            controller.show(WindowInsetsCompat.Type.systemBars())
        }
    }

    // Check remote_input access on entry and poll while denied so the banner clears
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

    val reducedMotion = LocalReducedMotion.current
    val layout = remember(device.deckLayout, context) {
        device.deckLayout?.let { localizedDefaults(it, context) } ?: fallbackDefaultLayout(context)
    }
    val liveState = device.deckState ?: DeckState(
        playing = false,
        volume = 50u,
        muted = false,
        micMuted = false,
        outputDevices = emptyList(),
    )
    val pages = layout.pages.ifEmpty { fallbackDefaultLayout(context).pages }
    val pagerState = rememberPagerState(pageCount = { pages.size })

    Column(
        modifier = modifier
            .fillMaxSize()
            .padding(
                horizontal = 16.dp,
                vertical = if (fullScreen || isLandscape) 8.dp else 12.dp,
            ),
        verticalArrangement = Arrangement.spacedBy(if (isLandscape) 8.dp else 12.dp),
    ) {
        G7Header(
            title = stringResource(R.string.action_deck_pc),
            onBack = onBack,
            note = device.name,
            noteColor = if (device.online) MaterialTheme.colorScheme.onSurfaceVariant else MaterialTheme.colorScheme.error,
        ) {
            G7RowAction(
                label = stringResource(if (fullScreen) R.string.deck_exit_fullscreen else R.string.deck_fullscreen),
                onClick = { fullScreen = !fullScreen },
            )
        }

        // What plays on the PC, with play and pause
        if (!fullScreen && device.online) {
            DeckNowPlaying(pcId = device.id, core = core)
        }

        // Status / permission banners
        if (!device.online) {
            DeckBanner(
                title = stringResource(R.string.remote_offline_text, device.name),
                text = null,
            )
        } else if (access == Core.RemoteAccess.Denied) {
            DeckBanner(
                title = stringResource(R.string.remote_denied_title, device.name),
                text = stringResource(R.string.remote_denied_text, device.name),
            )
        } else if (commandsDenied) {
            DeckBanner(
                title = stringResource(R.string.deck_commands_denied_title, device.name),
                text = stringResource(R.string.deck_commands_denied_text, device.name),
                onDismiss = { commandsDenied = false },
            )
        }

        // Page tabs when multiple pages exist
        if (pages.size > 1) {
            G7Tabs(
                options = pages.indices.map { it to pages[it].name },
                selected = pagerState.currentPage,
                onSelect = { index ->
                    scope.launch {
                        if (reducedMotion) pagerState.scrollToPage(index) else pagerState.animateScrollToPage(index)
                    }
                },
                modifier = Modifier.fillMaxWidth(),
            )
        }

        // Swipable pages of tactile Deck tiles
        HorizontalPager(
            state = pagerState,
            modifier = Modifier
                .fillMaxWidth()
                .weight(1f),
        ) { pageIndex ->
            val page = pages.getOrNull(pageIndex) ?: pages.first()
            if (page.tiles.isEmpty()) {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    Text(
                        stringResource(R.string.deck_empty_page, device.name),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialResource.onSurfaceVariant(),
                        textAlign = TextAlign.Center,
                        modifier = Modifier.padding(24.dp),
                    )
                }
            } else {
                val largeFont = LocalConfiguration.current.fontScale >= 1.2f
                val columns = when {
                    isLandscape -> if (largeFont) 3 else 4
                    largeFont -> 2
                    else -> 3
                }
                LazyVerticalGrid(
                    columns = GridCells.Fixed(columns),
                    modifier = Modifier.fillMaxSize(),
                    contentPadding = PaddingValues(vertical = 4.dp),
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    items(page.tiles, key = { it.id }) { tile ->
                        DeckTileButton(
                            tile = tile,
                            liveState = liveState,
                            compact = isLandscape,
                            enabled = device.online,
                            progress = tileProgress[tile.id],
                            onPress = {
                                view.performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP)
                                val press = (tilePresses[tile.id] ?: 0) + 1
                                tilePresses[tile.id] = press
                                tileProgress[tile.id] = DeckTileProgress.Working
                                scope.launch {
                                    val result = core.deckPress(device.id, tile.id)
                                    when (result) {
                                        Core.DeckPressResult.Ok -> {
                                            access = Core.RemoteAccess.Allowed
                                            if (tile.kind == "run_command") commandsDenied = false
                                        }
                                        Core.DeckPressResult.Denied -> {
                                            val chk = core.remoteCheck(device.id)
                                            access = chk
                                            if (chk == Core.RemoteAccess.Allowed && tile.kind == "run_command") {
                                                commandsDenied = true
                                            }
                                        }
                                        Core.DeckPressResult.Offline -> {
                                            access = Core.RemoteAccess.Offline
                                        }
                                        else -> {}
                                    }
                                    if (tilePresses[tile.id] != press) return@launch
                                    if (result == Core.DeckPressResult.Ok) {
                                        tileProgress[tile.id] = DeckTileProgress.Done
                                        delay(TILE_DONE_MS)
                                        if (tilePresses[tile.id] == press) tileProgress.remove(tile.id)
                                    } else {
                                        tileProgress.remove(tile.id)
                                    }
                                }
                            },
                        )
                    }
                }
            }
        }

        // The PC's volume, mute and output, under the tiles
        if (!fullScreen && device.online) {
            DeckAudioHeaderBar(
                pcName = device.name,
                liveState = liveState,
                onSetPcAudio = { vol, muted -> core.setPcAudio(device.id, vol, muted) },
            )
        }
    }
}

/** A raised row with what plays on the PC, and play or pause. Nothing while nothing plays. */
@Composable
private fun DeckNowPlaying(pcId: String, core: Core) {
    val summary by core.pcMedia.summaryFlow.collectAsStateWithLifecycle()
    val media = summary?.takeIf { it.pcId == pcId } ?: return
    val tints = g7Tints()
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(MaterialTheme.colorScheme.surfaceContainer, RoundedCornerShape(12.dp))
            .padding(horizontal = 12.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Box(
            modifier = Modifier
                .size(44.dp)
                .background(tints.pink.copy(alpha = 0.16f), RoundedCornerShape(8.dp)),
            contentAlignment = Alignment.Center,
        ) {
            Icon(G7Icons.music, contentDescription = null, tint = tints.pink, modifier = Modifier.size(20.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(media.title, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (media.subtitle.isNotBlank()) {
                Text(
                    media.subtitle,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        G7IconButton(
            icon = if (media.playing) G7Icons.pause else G7Icons.play,
            contentDescription = stringResource(if (media.playing) R.string.media_pause else R.string.media_play),
            onClick = {
                core.pcMedia.pressed(
                    if (media.playing) app.g7link.core.MediaAction.PAUSE else app.g7link.core.MediaAction.PLAY,
                )
            },
            filled = true,
        )
    }
}

private object MaterialResource {
    @Composable
    fun onSurfaceVariant(): Color = MaterialTheme.colorScheme.onSurfaceVariant
}

@Composable
private fun DeckAudioHeaderBar(
    pcName: String,
    liveState: DeckState,
    onSetPcAudio: (volume: Int?, muted: Boolean?) -> Unit,
) {
    var draggingVolume by remember { mutableStateOf<Float?>(null) }
    var lastSentVolume by remember { mutableStateOf(liveState.volume.toInt()) }
    if (draggingVolume == null) {
        lastSentVolume = liveState.volume.toInt()
    }
    var showOutputs by remember { mutableStateOf(false) }
    val shownVolume = draggingVolume?.roundToInt() ?: liveState.volume.toInt()
    val defaultOutput = liveState.outputDevices.firstOrNull { it.isDefault } ?: liveState.outputDevices.firstOrNull()
    val volumeLabel = stringResource(R.string.pc_audio_volume)

    val tints = g7Tints()
    Column(modifier = Modifier.fillMaxWidth()) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(horizontal = 4.dp, vertical = 4.dp),
            verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                G7IconButton(
                    icon = if (liveState.muted) G7Icons.volumeOff else G7Icons.volume,
                    contentDescription = stringResource(if (liveState.muted) R.string.pc_audio_unmute else R.string.pc_audio_mute),
                    onClick = { onSetPcAudio(null, !liveState.muted) },
                    tint = tints.teal,
                )
                G7Slider(
                    tint = tints.teal,
                    trackColor = MaterialTheme.colorScheme.surfaceContainerHigh,
                    value = (draggingVolume ?: liveState.volume.toFloat()).coerceIn(0f, 100f),
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
                    modifier = Modifier
                        .weight(1f)
                        .semantics { contentDescription = volumeLabel },
                )
                Text(
                    text = if (liveState.muted) {
                        stringResource(R.string.pc_audio_muted)
                    } else {
                        stringResource(R.string.deck_volume_percent, shownVolume)
                    },
                    style = MaterialTheme.typography.labelMedium,
                )
            }
            if (defaultOutput != null) {
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable(role = Role.Button) { showOutputs = true }
                        .padding(horizontal = 4.dp, vertical = 2.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.SpaceBetween,
                ) {
                    Text(
                        text = stringResource(R.string.pc_audio_output, defaultOutput.name),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f),
                    )
                    if (liveState.outputDevices.size > 1) {
                        Spacer(Modifier.width(8.dp))
                        Text(
                            text = "${liveState.outputDevices.size}",
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.primary,
                        )
                    }
                }
            }
        }
    }

    if (showOutputs && liveState.outputDevices.isNotEmpty()) {
        PcAudioOutputsDialog(
            pcName = pcName,
            devices = liveState.outputDevices,
            onDismiss = { showOutputs = false },
        )
    }
}

@Composable
private fun DeckBanner(
    title: String,
    text: String?,
    onDismiss: (() -> Unit)? = null,
) {
    Surface(
        shape = MaterialTheme.shapes.large,
        color = MaterialTheme.colorScheme.secondaryContainer,
        contentColor = MaterialTheme.colorScheme.onSecondaryContainer,
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(modifier = Modifier.weight(1f)) {
                Text(title, style = MaterialTheme.typography.titleSmall)
                if (text != null) {
                    Spacer(Modifier.height(2.dp))
                    Text(text, style = MaterialTheme.typography.bodySmall)
                }
            }
            if (onDismiss != null) {
                val dismissLabel = stringResource(R.string.action_dismiss)
                TextButton(
                    onClick = onDismiss,
                    modifier = Modifier.semantics { contentDescription = dismissLabel },
                ) {
                    Text("×", style = MaterialTheme.typography.titleMedium)
                }
            }
        }
    }
}

private data class TilePalette(
    val bg: Color,
    val border: Color,
    val accent: Color,
    val fg: Color,
    val iconWell: Color,
)

@Composable
private fun tilePalette(
    @Suppress("UNUSED_PARAMETER") colorName: String,
    highlighted: Boolean,
): TilePalette {
    val scheme = MaterialTheme.colorScheme
    return if (highlighted) {
        TilePalette(
            bg = scheme.primaryContainer,
            border = scheme.primary,
            accent = scheme.primary,
            fg = scheme.onPrimaryContainer,
            iconWell = scheme.surfaceContainerHigh,
        )
    } else {
        TilePalette(
            bg = scheme.surfaceContainerHigh,
            border = scheme.outlineVariant,
            accent = scheme.primary,
            fg = scheme.onSurface,
            iconWell = scheme.secondaryContainer,
        )
    }
}

@Composable
private fun liveBadgeText(kind: String, state: DeckState): String? = when (kind) {
    "media_play_pause" -> stringResource(if (state.playing) R.string.deck_playing else R.string.deck_paused)
    "volume_mute" ->
        if (state.muted) stringResource(R.string.deck_muted)
        else if (state.volume > 0u) stringResource(R.string.deck_volume_percent, state.volume.toInt())
        else null
    "mic_mute" -> when (state.micMuted) {
        true -> stringResource(R.string.deck_muted)
        false -> stringResource(R.string.deck_live)
        null -> null
    }
    else -> null
}

private fun isLiveHighlighted(kind: String, state: DeckState): Boolean = when (kind) {
    "media_play_pause" -> state.playing
    "volume_mute" -> state.muted
    "mic_mute" -> state.micMuted == true
    else -> false
}

private fun effectiveIcon(tile: DeckTile, state: DeckState): String = when {
    tile.kind == "media_play_pause" && state.playing -> "pause"
    tile.kind == "mic_mute" && state.micMuted == true -> "mic_off"
    tile.kind == "volume_mute" && state.muted -> "volume_off"
    else -> tile.icon
}

@Composable
private fun DeckTileButton(
    tile: DeckTile,
    liveState: DeckState,
    compact: Boolean,
    enabled: Boolean,
    progress: DeckTileProgress?,
    onPress: () -> Unit,
) {
    val reducedMotion = LocalReducedMotion.current
    val badge = liveBadgeText(tile.kind, liveState)
    val highlighted = isLiveHighlighted(tile.kind, liveState)
    val palette = tilePalette(tile.color, highlighted)
    val iconName = effectiveIcon(tile, liveState)

    var pressed by remember { mutableStateOf(false) }
    val scale by animateFloatAsState(
        targetValue = if (pressed && !reducedMotion) 0.96f else 1f,
        animationSpec = if (reducedMotion) snap() else spring(stiffness = Spring.StiffnessHigh),
        label = "tileScale",
    )

    val a11yDescription = if (badge != null) {
        stringResource(R.string.deck_tile_with_badge, tile.label, badge)
    } else {
        tile.label
    }
    val fontScale = LocalConfiguration.current.fontScale.coerceIn(1f, 1.35f)
    val tileHeight = ((if (compact) 98 else 118) * fontScale).dp

    val quiet = LocalQuietTheme.current
    val tileTint = if (quiet) deckTint(iconName, g7Tints()) else palette.fg
    val workingTint = (if (quiet) tileTint else palette.accent).copy(alpha = 0.14f)
    val progressOverlay = Modifier.drawWithContent {
        drawContent()
        if (progress == DeckTileProgress.Working) drawRect(workingTint)
    }
    Surface(
        shape = if (quiet) RoundedCornerShape(12.dp) else MaterialTheme.shapes.large,
        color = if (quiet) {
            (if (highlighted) MaterialTheme.colorScheme.surfaceContainerHigh else MaterialTheme.colorScheme.surfaceContainer)
                .copy(alpha = if (enabled) 1f else 0.55f)
        } else {
            palette.bg.copy(alpha = if (enabled) 1f else 0.55f)
        },
        contentColor = if (quiet) MaterialTheme.colorScheme.onSurface else palette.fg,
        border = if (quiet) {
            null
        } else {
            BorderStroke(
                width = if (highlighted) 2.dp else 1.dp,
                color = if (highlighted) palette.accent else palette.border,
            )
        },
        modifier = Modifier
            .fillMaxWidth()
            .height(tileHeight)
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
            }
            .semantics(mergeDescendants = true) {
                role = Role.Button
                contentDescription = a11yDescription
                if (enabled) {
                    onClick(label = a11yDescription) {
                        onPress()
                        true
                    }
                }
            }
            .pointerInput(enabled, tile.id) {
                if (!enabled) return@pointerInput
                detectTapGestures(
                    onPress = {
                        pressed = true
                        try {
                            if (tryAwaitRelease()) {
                                onPress()
                            }
                        } finally {
                            pressed = false
                        }
                    },
                )
            },
    ) {
        if (quiet) {
            Column(
                modifier = progressOverlay.fillMaxSize().padding(horizontal = 8.dp, vertical = 8.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center,
            ) {
                if (progress == DeckTileProgress.Done) {
                    DeckCheck(tint = tileTint, modifier = Modifier.size(24.dp))
                } else {
                    DeckIcon(name = iconName, tint = tileTint, modifier = Modifier.size(24.dp))
                }
                Spacer(Modifier.height(8.dp))
                Text(
                    text = tile.label,
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurface,
                    textAlign = TextAlign.Center,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                if (badge != null) {
                    Text(
                        text = badge,
                        style = MaterialTheme.typography.labelSmall,
                        color = if (highlighted) tileTint else MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                    )
                }
            }
            return@Surface
        }
        Column(
            modifier = progressOverlay
                .fillMaxSize()
                .padding(horizontal = 11.dp, vertical = 10.dp),
            verticalArrangement = Arrangement.SpaceBetween,
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.Top,
            ) {
                Box(
                    modifier = Modifier
                        .size(if (compact) 34.dp else 36.dp)
                        .background(palette.iconWell, MaterialTheme.shapes.medium),
                    contentAlignment = Alignment.Center,
                ) {
                    if (progress == DeckTileProgress.Done) {
                        DeckCheck(tint = palette.fg, modifier = Modifier.size(if (compact) 18.dp else 20.dp))
                    } else {
                        DeckIcon(
                            name = iconName,
                            tint = palette.fg,
                            modifier = Modifier.size(if (compact) 18.dp else 20.dp),
                        )
                    }
                }

                if (badge != null) {
                    Spacer(Modifier.width(6.dp))
                    Surface(
                        shape = CircleShape,
                        color = if (highlighted) palette.accent else palette.iconWell,
                        contentColor = if (highlighted) MaterialTheme.colorScheme.onPrimary else palette.fg,
                    ) {
                        Text(
                            text = badge,
                            style = MaterialTheme.typography.labelSmall,
                            fontWeight = FontWeight.SemiBold,
                            maxLines = 1,
                            softWrap = false,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.padding(horizontal = 6.dp, vertical = 2.dp),
                        )
                    }
                }
            }

            Text(
                text = tile.label,
                style = if (compact) MaterialTheme.typography.labelLarge else MaterialTheme.typography.titleSmall,
                fontWeight = FontWeight.SemiBold,
                color = palette.fg,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

/** A pressed tile waiting for the PC, or one the PC just ran. */
private enum class DeckTileProgress { Working, Done }

/** How long a tile shows its check once the PC ran it. */
private const val TILE_DONE_MS = 900L

/** A check mark in place of a tile's icon, once the PC ran it. */
@Composable
private fun DeckCheck(tint: Color, modifier: Modifier = Modifier) {
    Canvas(modifier) {
        val path = Path().apply {
            moveTo(size.width * 0.2f, size.height * 0.52f)
            lineTo(size.width * 0.42f, size.height * 0.74f)
            lineTo(size.width * 0.8f, size.height * 0.3f)
        }
        drawPath(
            path,
            color = tint,
            style = Stroke(width = size.minDimension * 0.11f, cap = StrokeCap.Round, join = StrokeJoin.Round),
        )
    }
}

/** A tile's icon: G7 Linux's glyph for the tile's icon name, in the tile's tint. */
@Composable
private fun DeckIcon(
    name: String,
    tint: Color,
    modifier: Modifier = Modifier,
) {
    Icon(deckIcon(name), contentDescription = null, tint = tint, modifier = modifier)
}

private fun deckIcon(name: String): ImageVector = when (name) {
    "play", "play_pause" -> G7Icons.play
    "pause" -> G7Icons.pause
    "skip_next" -> G7Icons.next
    "skip_previous" -> G7Icons.previous
    "volume_up" -> G7Icons.volumeUp
    "volume_down" -> G7Icons.volumeDown
    "volume_off" -> G7Icons.volumeOff
    "mic" -> G7Icons.mic
    "mic_off" -> G7Icons.micOff
    "lock" -> G7Icons.lock
    "desktop" -> G7Icons.desktop
    "switch_window", "windows" -> G7Icons.window
    "screenshot" -> G7Icons.screenshot
    "camera" -> G7Icons.camera
    "keyboard" -> G7Icons.keyboard
    "globe" -> G7Icons.globe
    "text" -> G7Icons.text
    "app", "rocket" -> G7Icons.apps
    "terminal" -> G7Icons.terminal
    "spark" -> G7Icons.sparkle
    "bolt" -> G7Icons.bolt
    "music" -> G7Icons.music
    "video" -> G7Icons.video
    "folder" -> G7Icons.folder
    "star" -> G7Icons.star
    "bell" -> G7Icons.bell
    else -> G7Icons.power
}

private fun fallbackDefaultLayout(context: Context): DeckLayout = DeckLayout(
    pages = listOf(
        DeckPage(
            id = "main",
            name = context.getString(R.string.deck_default_page_main),
            tiles = listOf(
                DeckTile("play_pause", context.getString(R.string.deck_default_play_pause), "play", "amber", "media_play_pause"),
                DeckTile("prev_track", context.getString(R.string.deck_default_previous), "skip_previous", "slate", "media_previous"),
                DeckTile("next_track", context.getString(R.string.deck_default_next), "skip_next", "slate", "media_next"),
                DeckTile("vol_down", context.getString(R.string.deck_default_volume_down), "volume_down", "teal", "volume_down"),
                DeckTile("vol_up", context.getString(R.string.deck_default_volume_up), "volume_up", "teal", "volume_up"),
                DeckTile("vol_mute", context.getString(R.string.deck_default_volume_mute), "volume_off", "teal", "volume_mute"),
                DeckTile("mic_mute", context.getString(R.string.deck_default_mic_mute), "mic", "coral", "mic_mute"),
                DeckTile("show_desktop", context.getString(R.string.deck_default_show_desktop), "desktop", "blue", "show_desktop"),
                DeckTile("switch_window", context.getString(R.string.deck_default_switch_window), "switch_window", "blue", "switch_window"),
                DeckTile("screenshot", context.getString(R.string.deck_default_screenshot), "screenshot", "violet", "screenshot"),
                DeckTile("lock_pc", context.getString(R.string.deck_default_lock_pc), "lock", "red", "lock_pc"),
            ),
        ),
    ),
)

/**
 * The string for a label the PC's default Deck uses, or null for one the user
 * chose. The PC sends these in English, whatever language this phone is in.
 */
internal fun defaultDeckLabel(label: String): Int? = when (label) {
    "Play / pause" -> R.string.deck_default_play_pause
    "Previous" -> R.string.deck_default_previous
    "Next" -> R.string.deck_default_next
    "Volume down" -> R.string.deck_default_volume_down
    "Volume up" -> R.string.deck_default_volume_up
    "Mute audio" -> R.string.deck_default_volume_mute
    "Mic mute" -> R.string.deck_default_mic_mute
    "Show desktop" -> R.string.deck_default_show_desktop
    "Switch window" -> R.string.deck_default_switch_window
    "Screenshot" -> R.string.deck_default_screenshot
    "Lock PC" -> R.string.deck_default_lock_pc
    else -> null
}

private fun localizedDefaults(layout: DeckLayout, context: android.content.Context) = layout.copy(
    pages = layout.pages.map { page ->
        page.copy(
            name = if (page.id == "main" && page.name == "Main") {
                context.getString(R.string.deck_default_page_main)
            } else {
                page.name
            },
            tiles = page.tiles.map { tile ->
                defaultDeckLabel(tile.label)?.let { tile.copy(label = context.getString(it)) } ?: tile
            },
        )
    },
)

/** A tile icon's tint, by what it does. */
private fun deckTint(icon: String, tints: G7Tints): Color = when (icon) {
    "play", "play_pause", "pause", "skip_next", "skip_previous", "music", "video", "keyboard" -> tints.pink
    "volume_up", "volume_down", "volume_off", "lock" -> tints.teal
    "mic", "mic_off" -> tints.red
    "desktop", "switch_window", "windows", "screenshot", "camera", "bell" -> tints.blue
    "globe", "spark", "bolt", "folder", "star" -> tints.amber
    "terminal" -> tints.green
    "text" -> tints.muted
    else -> tints.lavender
}
