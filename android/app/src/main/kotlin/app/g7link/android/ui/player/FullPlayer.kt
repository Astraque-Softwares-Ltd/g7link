// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.player

import android.graphics.Bitmap
import androidx.compose.animation.EnterExitState
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.animateDp
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.RoundRect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.layout
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.disabled
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.setProgress
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.PHONE_SPEAKER_OUTPUT
import app.g7link.android.media.PcArt
import app.g7link.android.media.PcMedia
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7Motion
import app.g7link.android.ui.theme.G7Row
import app.g7link.android.ui.theme.G7SectionLabel
import app.g7link.android.ui.theme.G7SwitchRow
import app.g7link.android.ui.theme.LocalReducedMotion
import app.g7link.android.ui.theme.g7Appear
import app.g7link.android.ui.theme.g7Disappear
import app.g7link.android.ui.theme.g7Tints
import app.g7link.core.AudioOutputDevice
import app.g7link.core.MediaAction
import app.g7link.core.MediaRepeat
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map

/**
 * The full player: the cover bleeds to the top edge and melts into a ground of the cover's own
 * tonal colours (the extractor's `surface` roles, or the app's for no cover or a black-and-white
 * one); over it the chevron and "Spotify on guchu", with a scrim that keeps them readable
 * ([PlayerColors.headerScrim]). Then the title, artist and album, the seek bar on the PC's real
 * position, previous, play or pause and next (shuffle and repeat only when the player offers
 * them), the PC's volume, and its output, which opens the player and output sheet.
 */
@Composable
internal fun FullPlayer(
    track: PcMedia.Summary,
    timeline: PcMedia.Timeline?,
    players: List<PcMedia.Summary>,
    media: PcMedia,
    core: Core,
    motion: PlayerMotion,
    dragDistance: Float,
    onClose: () -> Unit,
    onDrag: (Float) -> Unit,
    onRelease: () -> Unit,
    onPick: (PcMedia.Summary) -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    val typography = MaterialTheme.typography
    val dark = scheme.background.luminance() < 0.5f
    val target = remember(track.colors, dark, scheme) {
        PlayerModel.roles(track.colors?.scheme, dark)?.let { PlayerPalette.fromRoles(it) } ?: PlayerPalette.fromScheme(scheme)
    }
    val palette = animatedPalette(target)
    val controls = PlayerModel.controls(track, timeline)
    val boundsKey = motion.rememberKey(BOUNDS_KEY)
    val artKey = motion.rememberKey(ART_KEY)
    val titleKey = motion.rememberKey(TITLE_KEY)
    val playKey = motion.rememberKey(PLAY_KEY)
    val appear = g7Appear()
    val disappear = g7Disappear()
    // What only the full player has comes and goes with it.
    val onlyHere = with(motion.visibility) { Modifier.animateEnterExit(enter = appear, exit = disappear) }

    val levels by media.pcLevels.collectAsStateWithLifecycle()
    val level = levels[track.pc]
    val volumeKeys by media.volumeKeysOn.collectAsStateWithLifecycle()
    val deckFlow = remember(core, track.pc) { core.state.map { it.device(track.pc)?.deckState }.distinctUntilChanged() }
    val deck by deckFlow.collectAsStateWithLifecycle(initialValue = null)
    val outputs = deck?.outputDevices.orEmpty()
    val hardware = outputs.filterNot { it.id == PHONE_SPEAKER_OUTPUT }
    val output = hardware.firstOrNull { it.isDefault } ?: hardware.firstOrNull()
    val choices = PlayerModel.hasChoices(players, hardware.size, deck != null)
    var sheet by remember { mutableStateOf(false) }

    val pictures = media.pictures
    val fullKey = track.artFullKey
    var full by remember(track.pc, fullKey) { mutableStateOf(fullKey?.let { pictures.fullNow(track.pc, it) }) }
    val fullFades = remember(track.pc, fullKey) { full == null }
    LaunchedEffect(track.pc, fullKey) {
        if (fullKey != null && full == null) full = pictures.full(track.pc, fullKey)
    }

    val currentDrag by rememberUpdatedState(onDrag)
    val currentRelease by rememberUpdatedState(onRelease)
    val statusTop = WindowInsets.statusBars.asPaddingValues().calculateTopPadding()
    val navBottom = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()
    val closeLabel = stringResource(R.string.player_close)
    val chooseLabel = stringResource(R.string.player_choose)
    val playLabel = stringResource(if (track.playing) R.string.media_pause else R.string.media_play)

    BoxWithConstraints(
        Modifier
            .sharedBoundsOf(motion, boundsKey)
            .fillMaxSize()
            .drawBehind {
                drawRect(
                    Brush.verticalGradient(
                        0f to palette.groundHigh.value,
                        GROUND_HOLD to palette.groundHigh.value,
                        1f to palette.ground.value,
                    ),
                )
            }
            // The player is a surface of its own: touches never reach the screen under it.
            .pointerInput(Unit) { detectTapGestures { } },
    ) {
        val width = maxWidth
        val screenHeight = maxHeight
        val artHeight = artHeightFor(width, screenHeight, navBottom)
        val scrim = remember(track.colors, target, width, artHeight, statusTop) {
            val w = width.value
            val rowTop = statusTop.value + HEADER_TOP.value
            PlayerColors.headerScrim(
                light = track.colors?.light,
                ground = target.ground.toArgb() and PlayerColors.WHITE,
                ink = target.ink.toArgb() and PlayerColors.WHITE,
                boxWidth = w,
                boxHeight = artHeight.value,
                statusBar = statusTop.value,
                // "Spotify on guchu" between the two 48 dp buttons, the chevron in the first.
                label = Area(HEADER_SIDE + 48f, rowTop + 14f, w - HEADER_SIDE - 48f, rowTop + 34f),
                glyph = Area(HEADER_SIDE + 12f, rowTop + 12f, HEADER_SIDE + 36f, rowTop + 36f),
            )
        }
        Column(
            Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .heightIn(min = screenHeight),
        ) {
            Box(Modifier.fillMaxWidth().height(artHeight)) {
                val corner = motion.visibility.transition.animateDp(
                    transitionSpec = { G7Motion.move(G7Motion.LARGE_IN) },
                    label = "artCorner",
                ) { state -> if (state == EnterExitState.Visible) 0.dp else MINI_ART_CORNER }
                Box(
                    Modifier
                        .sharedElementOf(motion, artKey)
                        .fillMaxSize()
                        .clipCorners(corner)
                        .pointerInput(dragDistance) {
                            var dy = 0f
                            detectVerticalDragGestures(
                                onDragStart = { dy = 0f },
                                onDragEnd = { currentRelease() },
                                onDragCancel = { currentRelease() },
                            ) { change, amount ->
                                change.consume()
                                dy += amount
                                currentDrag((dy / dragDistance.coerceAtLeast(1f)).coerceIn(0f, 1f))
                            }
                        },
                ) {
                    Cover(track.artwork, full, fullFades)
                }
                // The cover melts into the ground.
                Spacer(
                    Modifier
                        .align(Alignment.BottomCenter)
                        .fillMaxWidth()
                        .fillMaxHeight(MELT_SHARE)
                        .drawBehind {
                            val high = palette.groundHigh.value
                            drawRect(Brush.verticalGradient(listOf(high.copy(alpha = 0f), high)))
                        },
                )
                Spacer(
                    Modifier
                        .fillMaxWidth()
                        .height(scrim.end.dp)
                        .drawBehind {
                            val ground = palette.ground.value
                            val hold = (scrim.hold / scrim.end).coerceIn(0f, 1f)
                            drawRect(
                                Brush.verticalGradient(
                                    0f to ground.copy(alpha = scrim.alpha),
                                    hold to ground.copy(alpha = scrim.alpha),
                                    1f to ground.copy(alpha = 0f),
                                ),
                            )
                        },
                )
                Row(
                    Modifier
                        .fillMaxWidth()
                        .padding(top = statusTop + HEADER_TOP, start = HEADER_SIDE.dp, end = HEADER_SIDE.dp)
                        .then(onlyHere),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Box(
                        Modifier
                            .size(48.dp)
                            .clip(RoundedCornerShape(8.dp))
                            .clickable(role = Role.Button, onClick = onClose)
                            .semantics { contentDescription = closeLabel },
                        contentAlignment = Alignment.Center,
                    ) {
                        TintedIcon(G7Icons.chevronDown, palette.ink, Modifier.size(24.dp))
                    }
                    Box(Modifier.weight(1f), contentAlignment = Alignment.Center) {
                        Row(
                            Modifier
                                .heightIn(min = 48.dp)
                                .clip(RoundedCornerShape(8.dp))
                                .then(
                                    if (choices) {
                                        Modifier.clickable(onClickLabel = chooseLabel, role = Role.Button) { sheet = true }
                                    } else {
                                        Modifier
                                    },
                                )
                                .padding(horizontal = 6.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(5.dp),
                        ) {
                            BasicText(
                                track.source,
                                style = typography.labelLarge.copy(fontWeight = FontWeight(600)),
                                color = { palette.ink.value },
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                                modifier = Modifier.weight(1f, fill = false),
                            )
                            if (choices) TintedIcon(G7Icons.chevronDown, palette.ink, Modifier.size(12.dp))
                        }
                    }
                    Spacer(Modifier.size(48.dp))
                }
            }

            Column(
                Modifier
                    .pullUp(TITLE_OVERLAP)
                    .fillMaxWidth()
                    .padding(horizontal = 28.dp),
            ) {
                BasicText(
                    track.title,
                    style = typography.headlineMedium.copy(fontWeight = FontWeight(600)),
                    color = { palette.ink.value },
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.sharedBoundsOf(motion, titleKey),
                )
                track.artist?.let { artist ->
                    BasicText(
                        artist,
                        style = typography.bodyLarge.copy(fontSize = 16.sp),
                        color = { palette.inkMuted.value },
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.padding(top = 4.dp).then(onlyHere),
                    )
                }
                track.album?.let { album ->
                    BasicText(
                        album,
                        style = typography.bodyMedium,
                        color = { palette.inkMuted.value },
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.padding(top = 2.dp).then(onlyHere),
                    )
                }
            }

            SeekBlock(track, timeline, media, palette, controls.seek, Modifier.padding(start = 28.dp, end = 28.dp, top = 6.dp).then(onlyHere))

            Row(
                Modifier
                    .fillMaxWidth()
                    .padding(start = 16.dp, end = 16.dp, top = 8.dp),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (controls.shuffle) {
                    val on = track.shuffle == true
                    ToggleButton(
                        icon = PlayerGlyphs.shuffle,
                        on = on,
                        plate = false,
                        label = stringResource(if (on) R.string.player_shuffle_on else R.string.player_shuffle_off),
                        palette = palette,
                        modifier = onlyHere,
                    ) { media.setShuffle(track.pc, track.playerId, !on) }
                } else {
                    Spacer(Modifier.size(48.dp))
                }
                TransportButton(G7Icons.previous, stringResource(R.string.media_previous), controls.previous, palette.ink, onlyHere) {
                    media.pressed(track.pc, track.playerId, MediaAction.PREVIOUS)
                }
                Box(
                    Modifier
                        .sharedBoundsOf(motion, playKey)
                        .size(PLAY_SIZE)
                        .clip(CircleShape)
                        .clickable(enabled = controls.playPause, role = Role.Button) {
                            media.pressed(track.pc, track.playerId, if (track.playing) MediaAction.PAUSE else MediaAction.PLAY)
                        }
                        .semantics { contentDescription = playLabel }
                        .alpha(if (controls.playPause) 1f else DISABLED_ALPHA),
                    contentAlignment = Alignment.Center,
                ) {
                    // The glyph's circle in the accent, its bars (or triangle) cut out to show the disc under it.
                    Spacer(Modifier.size(PLAY_DISC).drawBehind { drawCircle(palette.onAccent.value) })
                    TintedIcon(
                        if (track.playing) PlayerGlyphs.pauseFilled else PlayerGlyphs.playFilled,
                        palette.accent,
                        Modifier.size(PLAY_GLYPH),
                    )
                }
                TransportButton(G7Icons.next, stringResource(R.string.media_next), controls.next, palette.ink, onlyHere) {
                    media.pressed(track.pc, track.playerId, MediaAction.NEXT)
                }
                if (controls.repeat) {
                    val repeat = track.repeat ?: MediaRepeat.NONE
                    ToggleButton(
                        icon = PlayerGlyphs.repeat,
                        on = repeat != MediaRepeat.NONE,
                        plate = repeat == MediaRepeat.TRACK,
                        label = stringResource(
                            when (repeat) {
                                MediaRepeat.NONE -> R.string.player_repeat_off
                                MediaRepeat.PLAYLIST -> R.string.player_repeat_playlist
                                MediaRepeat.TRACK -> R.string.player_repeat_track
                            },
                        ),
                        palette = palette,
                        modifier = onlyHere,
                    ) { media.setRepeat(track.pc, track.playerId, PlayerModel.nextRepeat(track.repeat)) }
                } else {
                    Spacer(Modifier.size(48.dp))
                }
            }

            if (level != null) {
                VolumeRow(track, level, media, palette, Modifier.padding(start = 24.dp, end = 28.dp, top = 10.dp).then(onlyHere))
            }

            Spacer(Modifier.weight(1f).heightIn(min = 16.dp))
            if (output != null) {
                val outputLabel = stringResource(R.string.pc_audio_output, output.name)
                Box(Modifier.fillMaxWidth().then(onlyHere), contentAlignment = Alignment.Center) {
                    Box(
                        Modifier
                            .heightIn(min = 48.dp)
                            .clip(RoundedCornerShape(8.dp))
                            .clickable(onClickLabel = chooseLabel, role = Role.Button) { sheet = true }
                            .semantics { contentDescription = outputLabel },
                        contentAlignment = Alignment.Center,
                    ) {
                        Row(
                            Modifier
                                .height(36.dp)
                                .clip(RoundedCornerShape(8.dp))
                                .drawBehind { drawRect(palette.groundHigh.value) }
                                .padding(horizontal = 12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            TintedIcon(G7Icons.volume, palette.ink, Modifier.size(16.dp))
                            BasicText(
                                output.name,
                                style = typography.labelLarge,
                                color = { palette.ink.value },
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                                modifier = Modifier.widthIn(max = 220.dp),
                            )
                            TintedIcon(G7Icons.chevronDown, palette.inkMuted, Modifier.size(12.dp))
                        }
                    }
                }
            }
            Spacer(Modifier.height(24.dp + navBottom))
        }
    }

    if (sheet) {
        PlaySheet(
            track = track,
            players = players,
            hardware = hardware,
            toPhone = outputs.any { it.id == PHONE_SPEAKER_OUTPUT },
            hasDeck = deck != null,
            volumeNote = volumeKeys && level != null,
            core = core,
            pictures = pictures,
            onPick = { picked ->
                onPick(picked)
                sheet = false
            },
            onDismiss = { sheet = false },
        )
    }
}

/** The cover: the small picture until the large one arrives, which fades in over it; the G7 mark without either. */
@Composable
private fun Cover(small: Bitmap?, full: Bitmap?, fullFades: Boolean) {
    if (small == null && full == null) {
        NoArtwork(markSize = 104.dp)
        return
    }
    if (small != null) {
        val image = remember(small) { small.asImageBitmap() }
        Image(image, contentDescription = null, modifier = Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
    }
    if (full != null) {
        val reducedMotion = LocalReducedMotion.current
        val image = remember(full) { full.asImageBitmap() }
        val shown = remember(full) { Animatable(if (fullFades && small != null && !reducedMotion) 0f else 1f) }
        LaunchedEffect(full) { shown.animateTo(1f, G7Motion.enter(G7Motion.SMALL_IN)) }
        Image(
            image,
            contentDescription = null,
            modifier = Modifier.fillMaxSize().graphicsLayer { alpha = shown.value },
            contentScale = ContentScale.Crop,
        )
    }
}

/** The seek bar on the PC's real position, with the time played and the time left. */
@Composable
private fun SeekBlock(
    track: PcMedia.Summary,
    timeline: PcMedia.Timeline?,
    media: PcMedia,
    palette: PaletteState,
    canSeek: Boolean,
    modifier: Modifier = Modifier,
) {
    val typography = MaterialTheme.typography
    // Every frame while it plays and shows, read only while drawing; the times change once a second.
    val clock = rememberPlayerClock(running = timeline?.playing == true, everyFrame = true)
    var scrub by remember(track.pc, track.playerId) { mutableStateOf<Float?>(null) }
    var released by remember(track.pc, track.playerId) { mutableStateOf(false) }
    // A seek shows its target until the PC's position (held by PcMedia) takes over.
    LaunchedEffect(timeline) {
        if (released) {
            scrub = null
            released = false
        }
    }
    LaunchedEffect(released) {
        if (released) {
            delay(PcMedia.SEEK_HOLD_MS)
            scrub = null
            released = false
        }
    }
    val duration = timeline?.durationMs ?: 0L
    val shownSeconds by remember(timeline) {
        derivedStateOf {
            val at = scrub?.let { (it * duration).toLong() } ?: timeline?.positionAt(clock.longValue) ?: 0L
            at / 1000
        }
    }
    val elapsed = PlayerModel.clock(shownSeconds * 1000)
    val total = PlayerModel.clock(duration)
    Column(modifier.fillMaxWidth()) {
        PlayerBar(
            fraction = { scrub ?: PlayerModel.progress(timeline, clock.longValue) },
            fill = palette.accent,
            track = palette.track,
            thumb = true,
            enabled = canSeek,
            description = stringResource(R.string.player_position),
            stateText = if (duration > 0) stringResource(R.string.player_time_of, elapsed, total) else "",
            progress = if (duration > 0) (shownSeconds * 1000f / duration).coerceIn(0f, 1f) else 0f,
            steps = 0,
            onScrub = { scrub = it },
            onRelease = {
                val at = scrub
                if (at != null) {
                    media.seek(track.pc, track.playerId, (at * duration).toLong())
                    released = true
                }
            },
        )
        if (duration > 0) {
            Row(Modifier.fillMaxWidth().pullUp(8.dp)) {
                BasicText(
                    elapsed,
                    style = typography.bodySmall.copy(fontFeatureSettings = "tnum"),
                    color = { palette.inkMuted.value },
                )
                Spacer(Modifier.weight(1f))
                BasicText(
                    "−" + PlayerModel.clock((duration - shownSeconds * 1000).coerceAtLeast(0)),
                    style = typography.bodySmall.copy(fontFeatureSettings = "tnum"),
                    color = { palette.inkMuted.value },
                )
            }
        }
    }
}

/** The PC's master volume in its 4 % steps, and mute. */
@Composable
private fun VolumeRow(track: PcMedia.Summary, level: PcMedia.PcLevel, media: PcMedia, palette: PaletteState, modifier: Modifier = Modifier) {
    var dragged by remember(track.pc) { mutableStateOf<Int?>(null) }
    val step = dragged ?: PcMedia.toSteps(level.volume)
    val muteLabel = stringResource(if (level.muted) R.string.player_unmute_pc else R.string.player_mute_pc, track.pcName)
    val stateText = if (level.muted) {
        stringResource(R.string.pc_audio_muted)
    } else {
        java.text.NumberFormat.getPercentInstance().format(PlayerModel.volumeOf(step) / 100.0)
    }
    Row(
        modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Box(
            Modifier
                .size(48.dp)
                .clip(RoundedCornerShape(8.dp))
                .clickable(role = Role.Button) { media.setVolume(track.pc, null, !level.muted) }
                .semantics { contentDescription = muteLabel },
            contentAlignment = Alignment.Center,
        ) {
            TintedIcon(if (level.muted) G7Icons.volumeOff else G7Icons.volume, palette.inkMuted, Modifier.size(18.dp))
        }
        PlayerBar(
            fraction = { (dragged ?: PcMedia.toSteps(level.volume)) / PcMedia.VOLUME_STEPS.toFloat() },
            fill = palette.ink,
            track = palette.track,
            thumb = false,
            enabled = true,
            description = stringResource(R.string.player_volume_on, track.pcName),
            stateText = stateText,
            progress = step / PcMedia.VOLUME_STEPS.toFloat(),
            steps = PcMedia.VOLUME_STEPS - 1,
            onScrub = { fraction ->
                val next = PlayerModel.volumeStep(fraction)
                if (next != dragged) {
                    dragged = next
                    media.setVolume(track.pc, PlayerModel.volumeOf(next), null)
                }
            },
            onRelease = { dragged = null },
            modifier = Modifier.weight(1f),
        )
    }
}

/**
 * A flat bar with a 48 dp touch height: a 6 dp track filled in [fill] up to [fraction] (read
 * while drawing), and a round thumb with [thumb]. Dragging or tapping calls [onScrub] with where
 * the finger is; letting go calls [onRelease] once.
 */
@Composable
private fun PlayerBar(
    fraction: () -> Float,
    fill: State<Color>,
    track: State<Color>,
    thumb: Boolean,
    enabled: Boolean,
    description: String,
    stateText: String,
    progress: Float,
    steps: Int,
    onScrub: (Float) -> Unit,
    onRelease: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val currentScrub by rememberUpdatedState(onScrub)
    val currentRelease by rememberUpdatedState(onRelease)
    val input = if (enabled) {
        Modifier
            .pointerInput(Unit) {
                detectHorizontalDragGestures(
                    onDragStart = { offset -> currentScrub(fractionAt(offset.x, size.width)) },
                    onDragEnd = { currentRelease() },
                    onDragCancel = { currentRelease() },
                ) { change, _ ->
                    change.consume()
                    currentScrub(fractionAt(change.position.x, size.width))
                }
            }
            .pointerInput(Unit) {
                detectTapGestures { offset ->
                    currentScrub(fractionAt(offset.x, size.width))
                    currentRelease()
                }
            }
    } else {
        Modifier
    }
    Spacer(
        modifier
            .fillMaxWidth()
            .height(48.dp)
            .then(input)
            .semantics {
                contentDescription = description
                if (stateText.isNotEmpty()) stateDescription = stateText
                progressBarRangeInfo = ProgressBarRangeInfo(progress, 0f..1f, steps)
                if (enabled) {
                    setProgress { value ->
                        currentScrub(value.coerceIn(0f, 1f))
                        currentRelease()
                        true
                    }
                } else {
                    disabled()
                }
            }
            .drawBehind {
                val alpha = if (enabled) 1f else DISABLED_ALPHA
                val height = BAR_HEIGHT.toPx()
                val top = (size.height - height) / 2f
                val radius = CornerRadius(height / 2f, height / 2f)
                val f = fraction().coerceIn(0f, 1f)
                drawRoundRect(track.value, topLeft = Offset(0f, top), size = Size(size.width, height), cornerRadius = radius, alpha = alpha)
                if (f > 0f) {
                    drawRoundRect(fill.value, topLeft = Offset(0f, top), size = Size(size.width * f, height), cornerRadius = radius, alpha = alpha)
                }
                if (thumb) {
                    drawCircle(fill.value, radius = THUMB_RADIUS.toPx(), center = Offset(size.width * f, size.height / 2f), alpha = alpha)
                }
            },
    )
}

private fun fractionAt(x: Float, width: Int): Float = if (width <= 0) 0f else (x / width).coerceIn(0f, 1f)

/** Previous or next: a 30 dp glyph in a 56 dp button, faded when the player doesn't offer it. */
@Composable
private fun TransportButton(
    icon: ImageVector,
    label: String,
    enabled: Boolean,
    tint: State<Color>,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    Box(
        modifier
            .size(56.dp)
            .clip(RoundedCornerShape(8.dp))
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) {
        TintedIcon(icon, tint, Modifier.size(30.dp).alpha(if (enabled) 1f else DISABLED_ALPHA))
    }
}

/**
 * Shuffle or repeat: muted when off, the accent with a dot under it when on; [plate] (repeat
 * this track) adds the accent plate G7's buttons show for a mode that's on.
 */
@Composable
private fun ToggleButton(
    icon: ImageVector,
    on: Boolean,
    plate: Boolean,
    label: String,
    palette: PaletteState,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    Box(
        modifier
            .size(48.dp)
            .clip(RoundedCornerShape(10.dp))
            .drawBehind {
                if (plate) {
                    val accent = palette.accent.value
                    val radius = CornerRadius(10.dp.toPx(), 10.dp.toPx())
                    drawRoundRect(accent.copy(alpha = 0.18f), cornerRadius = radius)
                    drawRoundRect(accent, cornerRadius = radius, style = Stroke(width = 1.dp.toPx() * 2f))
                }
            }
            .clickable(role = Role.Button, onClick = onClick)
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) {
        TintedIcon(icon, if (on) palette.accent else palette.inkMuted, Modifier.size(22.dp))
        if (on) {
            Spacer(
                Modifier
                    .align(Alignment.BottomCenter)
                    .padding(bottom = 6.dp)
                    .size(4.dp)
                    .drawBehind { drawCircle(palette.accent.value) },
            )
        }
    }
}

/**
 * "Play on guchu": where the PC's sound goes (its outputs, or this phone), which player shows,
 * and, while the volume buttons reach the PC, a word about it. In the app's own colours.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun PlaySheet(
    track: PcMedia.Summary,
    players: List<PcMedia.Summary>,
    hardware: List<AudioOutputDevice>,
    toPhone: Boolean,
    hasDeck: Boolean,
    volumeNote: Boolean,
    core: Core,
    pictures: PcArt,
    onPick: (PcMedia.Summary) -> Unit,
    onDismiss: () -> Unit,
) {
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val tints = g7Tints()
    val scheme = MaterialTheme.colorScheme
    val defaultLabel = stringResource(R.string.pc_audio_default_badge)
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        containerColor = scheme.surfaceContainerHigh,
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 8.dp)
                .padding(bottom = 18.dp),
        ) {
            Text(
                stringResource(R.string.player_sheet_title, track.pcName),
                style = MaterialTheme.typography.titleMedium,
                modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp).semantics { heading() },
            )
            if (hardware.isNotEmpty()) {
                G7SectionLabel(stringResource(R.string.player_output))
                for (out in hardware) {
                    val bluetooth = out.id.startsWith("bluez", ignoreCase = true)
                    G7Row(
                        label = out.name,
                        icon = if (bluetooth) G7Icons.bluetooth else G7Icons.volume,
                        tint = if (bluetooth) tints.blue else tints.teal,
                        role = Role.RadioButton,
                        onClick = { if (!out.isDefault) core.setPcAudioOutput(track.pc, out.id) },
                    ) {
                        if (out.isDefault) {
                            Icon(G7Icons.check, contentDescription = defaultLabel, tint = scheme.onSurface, modifier = Modifier.size(18.dp))
                        }
                    }
                }
            }
            if (hasDeck) {
                G7SwitchRow(
                    label = stringResource(R.string.pc_audio_use_phone_speaker),
                    sublabel = stringResource(R.string.player_phone_speaker_note, track.pcName),
                    checked = toPhone,
                    onCheckedChange = { on -> core.setPcAudio(track.pc, if (on) 201 else 200, null) },
                    icon = G7Icons.phone,
                    tint = tints.teal,
                )
            }
            for (list in players.groupBy { it.pc }.values) {
                G7SectionLabel(stringResource(R.string.player_players_on, list.first().pcName))
                for (each in list) {
                    PlayerRow(
                        player = each,
                        current = each.pc == track.pc && each.playerId == track.playerId,
                        pictures = pictures,
                        onClick = { onPick(each) },
                    )
                }
            }
            if (volumeNote) {
                Text(
                    stringResource(R.string.player_volume_keys_note, track.pcName),
                    style = MaterialTheme.typography.bodySmall,
                    color = scheme.onSurfaceVariant,
                    modifier = Modifier.padding(start = 12.dp, end = 12.dp, top = 10.dp),
                )
            }
        }
    }
}

/** A player in the sheet: the app's own icon when the PC has it, its name, what it plays and whether it plays. */
@Composable
private fun PlayerRow(player: PcMedia.Summary, current: Boolean, pictures: PcArt, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val iconKey = player.iconKey
    var icon by remember(player.pc, iconKey) { mutableStateOf(iconKey?.let { pictures.iconNow(player.pc, it) }) }
    LaunchedEffect(player.pc, iconKey) {
        if (iconKey != null && icon == null) icon = pictures.icon(player.pc, iconKey)
    }
    val browser = player.appName in BROWSERS
    val name = player.appName.ifEmpty { player.pcName }
    val state = stringResource(if (player.playing) R.string.player_state_playing else R.string.player_state_paused, player.title)
    val showingLabel = stringResource(R.string.player_showing)
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(RoundedCornerShape(10.dp))
            .clickable(role = Role.RadioButton, onClick = onClick)
            .semantics { selected = current }
            .padding(horizontal = 12.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        val bitmap = icon
        if (bitmap != null) {
            val image = remember(bitmap) { bitmap.asImageBitmap() }
            Image(image, contentDescription = null, modifier = Modifier.size(22.dp).clip(RoundedCornerShape(5.dp)))
        } else {
            Icon(
                if (browser) G7Icons.video else G7Icons.music,
                contentDescription = null,
                tint = if (browser) tints.coral else tints.pink,
                modifier = Modifier.size(22.dp),
            )
        }
        Column(Modifier.weight(1f)) {
            Text(name, style = MaterialTheme.typography.bodyLarge, color = scheme.onSurface, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(state, style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (current) {
            Icon(G7Icons.check, contentDescription = showingLabel, tint = scheme.onSurface, modifier = Modifier.size(18.dp))
        }
    }
}

/** Clips to a rounded rectangle whose corner is read while drawing (the cover's corner during the expand). */
private fun Modifier.clipCorners(radius: State<Dp>): Modifier = drawWithCache {
    val path = Path()
    onDrawWithContent {
        val r = radius.value.toPx()
        if (r <= 0f) {
            drawContent()
        } else {
            path.reset()
            path.addRoundRect(RoundRect(0f, 0f, size.width, size.height, CornerRadius(r, r)))
            clipPath(path) { this@onDrawWithContent.drawContent() }
        }
    }
}

/** Lays this out [by] higher than its place, and takes that much less room: a negative top margin. */
private fun Modifier.pullUp(by: Dp): Modifier = layout { measurable, constraints ->
    val placeable = measurable.measure(constraints)
    val up = by.roundToPx().coerceIn(0, placeable.height)
    layout(placeable.width, placeable.height - up) { placeable.place(0, -up) }
}

/** The cover's height: a square the screen's width, unless the controls under it wouldn't fit (then they scroll). */
private fun artHeightFor(width: Dp, height: Dp, bottomInset: Dp): Dp {
    val room = height - CONTROLS_HEIGHT - bottomInset
    return minOf(width, maxOf(room, width * MIN_ART_SHARE))
}

private val BROWSERS = setOf("Firefox", "Chrome", "Edge", "Brave", "Vivaldi", "Opera")

/** The ground holds its higher tone down to here, then darkens toward the bottom. */
private const val GROUND_HOLD = 0.46f

/** The share of the cover that melts into the ground. */
private const val MELT_SHARE = 0.44f

/** The cover never gets smaller than this share of the width; past that the controls scroll. */
private const val MIN_ART_SHARE = 0.6f

/** About what the controls under the cover take, without the navigation bar. */
private val CONTROLS_HEIGHT = 360.dp

private val HEADER_TOP = 4.dp
private const val HEADER_SIDE = 8f
private val TITLE_OVERLAP = 14.dp
private val PLAY_SIZE = 80.dp
private val PLAY_DISC = 59.dp
private val PLAY_GLYPH = 76.dp
private val BAR_HEIGHT = 6.dp
private val THUMB_RADIUS = 7.dp
private const val DISABLED_ALPHA = 0.45f
