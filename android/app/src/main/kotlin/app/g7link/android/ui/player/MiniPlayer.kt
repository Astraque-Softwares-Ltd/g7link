// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.player

import android.graphics.Bitmap
import androidx.compose.animation.core.animate
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LongState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.ColorMatrix
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.g7link.android.R
import app.g7link.android.media.ArtColors
import app.g7link.android.media.PcMedia
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7Motion
import app.g7link.android.ui.theme.LocalG7Look
import app.g7link.android.ui.theme.LocalReducedMotion
import app.g7link.core.MediaAction
import kotlinx.coroutines.launch
import kotlin.math.abs

/**
 * The mini player, direction C ("Backdrop"): the cover, blurred and dimmed, is the card itself,
 * 64 dp tall with a 12 dp corner, 12 dp in from the sides and [bottomPadding] up from the bottom.
 * On it, the cover, the title, "artist · app on PC", play or pause and next; the part already
 * played is a soft lighter fill with a feathered edge, never a line through the text.
 *
 * Tap or drag up to open (the drag follows the finger, [onDrag] with how far of [dragDistance]
 * it has gone); swipe sideways to skip. There's no swipe to hide it.
 */
@Composable
internal fun MiniPlayer(
    track: PcMedia.Summary,
    timeline: PcMedia.Timeline?,
    media: PcMedia,
    motion: PlayerMotion,
    bottomPadding: Dp,
    dragDistance: Float,
    onOpen: () -> Unit,
    onDrag: (Float) -> Unit,
    onRelease: () -> Unit,
) {
    val controls = PlayerModel.controls(track, timeline)
    val reducedMotion = LocalReducedMotion.current
    val boundsKey = motion.rememberKey(BOUNDS_KEY)
    val artKey = motion.rememberKey(ART_KEY)
    val titleKey = motion.rememberKey(TITLE_KEY)
    val playKey = motion.rememberKey(PLAY_KEY)
    val clock = rememberPlayerClock(running = timeline?.playing == true, everyFrame = false)
    val shape = RoundedCornerShape(MINI_CORNER)
    // The card is always dark: the cover dimmed, or the theme's own dark ground without one.
    val fallback = LocalG7Look.current.deep.palette.surfaceContainerHigh
    val openLabel = stringResource(R.string.player_open)
    val nextLabel = stringResource(R.string.media_next)
    val previousLabel = stringResource(R.string.media_previous)
    val playLabel = stringResource(if (track.playing) R.string.media_pause else R.string.media_play)
    val skipThreshold = with(LocalDensity.current) { SKIP_DISTANCE.toPx() }
    val swipe = remember { mutableFloatStateOf(0f) }
    val scope = rememberCoroutineScope()

    val skip: (Int) -> Unit = { direction ->
        when {
            direction > 0 && controls.next -> media.pressed(track.pc, track.playerId, MediaAction.NEXT)
            direction < 0 && controls.previous -> media.pressed(track.pc, track.playerId, MediaAction.PREVIOUS)
            else -> {}
        }
    }
    val currentSkip by rememberUpdatedState(skip)
    val currentDrag by rememberUpdatedState(onDrag)
    val currentRelease by rememberUpdatedState(onRelease)

    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
        Box(
            Modifier
                .padding(start = MINI_SIDE, end = MINI_SIDE, bottom = bottomPadding)
                .sharedBoundsOf(motion, boundsKey)
                .fillMaxWidth()
                .height(MINI_HEIGHT)
                .shadow(MINI_ELEVATION, shape, clip = true)
                .background(fallback)
                .pointerInput(dragDistance, skipThreshold, reducedMotion) {
                    var axis = AXIS_NONE
                    var dx = 0f
                    var dy = 0f
                    fun finish(skips: Boolean) {
                        when (axis) {
                            AXIS_SIDEWAYS -> {
                                if (skips) currentSkip(PlayerModel.skip(dx, skipThreshold))
                                val from = swipe.floatValue
                                if (reducedMotion) {
                                    swipe.floatValue = 0f
                                } else {
                                    scope.launch {
                                        animate(from, 0f, animationSpec = G7Motion.follow<Float>()) { value, _ -> swipe.floatValue = value }
                                    }
                                }
                            }
                            AXIS_UP -> currentRelease()
                        }
                        axis = AXIS_NONE
                    }
                    detectDragGestures(
                        onDragStart = {
                            axis = AXIS_NONE
                            dx = 0f
                            dy = 0f
                        },
                        onDragEnd = { finish(skips = true) },
                        onDragCancel = { finish(skips = false) },
                    ) { change, amount ->
                        change.consume()
                        if (axis == AXIS_NONE) axis = if (abs(amount.x) > abs(amount.y)) AXIS_SIDEWAYS else AXIS_UP
                        if (axis == AXIS_SIDEWAYS) {
                            // The card's content follows the finger, held back a little.
                            dx += amount.x
                            swipe.floatValue = (dx * SWIPE_FOLLOW).coerceIn(-skipThreshold, skipThreshold)
                        } else {
                            dy += amount.y
                            currentDrag((-dy / dragDistance.coerceAtLeast(1f)).coerceIn(0f, 1f))
                        }
                    }
                }
                .clickable(onClickLabel = openLabel, role = Role.Button, onClick = onOpen)
                .semantics {
                    // Swiping isn't there for everyone: skipping is also an action on the card.
                    customActions = buildList {
                        if (controls.next) add(CustomAccessibilityAction(nextLabel) { currentSkip(1); true })
                        if (controls.previous) add(CustomAccessibilityAction(previousLabel) { currentSkip(-1); true })
                    }
                },
        ) {
            MiniBackdrop(track.colors)
            PlayedFill(timeline, clock)
            Row(
                modifier = Modifier
                    .fillMaxSize()
                    .graphicsLayer { translationX = swipe.floatValue }
                    .padding(start = 8.dp, end = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                CoverThumb(track.artwork, Modifier.sharedElementOf(motion, artKey))
                Column(Modifier.weight(1f)) {
                    Text(
                        track.title,
                        style = MaterialTheme.typography.bodyMedium.copy(fontWeight = FontWeight(600)),
                        color = Color.White,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.sharedBoundsOf(motion, titleKey),
                    )
                    if (track.subtitle.isNotBlank()) {
                        Text(
                            track.subtitle,
                            style = MaterialTheme.typography.bodySmall.copy(fontSize = 12.5.sp),
                            color = Color.White.copy(alpha = PlayerColors.MINI_MUTED_ALPHA),
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box(
                        Modifier
                            .sharedBoundsOf(motion, playKey)
                            .size(MINI_BUTTON)
                            .clip(RoundedCornerShape(8.dp))
                            .clickable(enabled = controls.playPause, role = Role.Button) {
                                media.pressed(track.pc, track.playerId, if (track.playing) MediaAction.PAUSE else MediaAction.PLAY)
                            }
                            .semantics { contentDescription = playLabel },
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(
                            imageVector = if (track.playing) G7Icons.pause else G7Icons.play,
                            contentDescription = null,
                            tint = Color.White,
                            modifier = Modifier.size(24.dp).alpha(if (controls.playPause) 1f else DISABLED_ALPHA),
                        )
                    }
                    if (controls.next) {
                        Box(
                            Modifier
                                .size(MINI_BUTTON)
                                .clip(RoundedCornerShape(8.dp))
                                .clickable(role = Role.Button) { currentSkip(1) }
                                .semantics { contentDescription = nextLabel },
                            contentAlignment = Alignment.Center,
                        ) {
                            Icon(G7Icons.next, contentDescription = null, tint = Color.White, modifier = Modifier.size(20.dp))
                        }
                    }
                }
            }
        }
    }
}

/** The cover as the card: blurred, a little more saturated, and dimmed so white text keeps 4.5:1 ([ArtColors.backdropDim]). */
@Composable
private fun BoxScope.MiniBackdrop(colors: ArtColors?) {
    val backdrop = colors?.backdrop ?: return
    val dim = colors?.backdropDim ?: PlayerColors.MINI_DIM
    val image = remember(backdrop) { backdrop.asImageBitmap() }
    val filter = remember(dim) {
        val matrix = ColorMatrix().apply { setToSaturation(PlayerColors.MINI_SATURATION) }
        matrix.timesAssign(ColorMatrix().apply { setToScale(dim, dim, dim, 1f) })
        ColorFilter.colorMatrix(matrix)
    }
    // A few pixels a side, drawn large and smoothed: soft even where the blur isn't (before Android 12).
    Image(
        bitmap = image,
        contentDescription = null,
        modifier = Modifier
            .matchParentSize()
            .graphicsLayer {
                scaleX = BACKDROP_ZOOM
                scaleY = BACKDROP_ZOOM
            }
            .blur(BACKDROP_BLUR),
        contentScale = ContentScale.Crop,
        colorFilter = filter,
        filterQuality = FilterQuality.High,
    )
}

/** The part already played: a lighter fill from the left whose edge fades out, drawn from the clock without composing. */
@Composable
private fun BoxScope.PlayedFill(timeline: PcMedia.Timeline?, clock: LongState) {
    if (timeline == null) return
    Spacer(
        Modifier
            .matchParentSize()
            .drawWithCache {
                val feather = FILL_FEATHER.toPx()
                val fill = Color.White.copy(alpha = PlayerColors.MINI_FILL_ALPHA)
                val edge = Brush.horizontalGradient(listOf(fill, Color.Transparent), startX = 0f, endX = feather)
                onDrawBehind {
                    val end = size.width * PlayerModel.progress(timeline, clock.longValue)
                    if (end > 0f) {
                        val solid = (end - feather).coerceAtLeast(0f)
                        if (solid > 0f) drawRect(fill, size = Size(solid, size.height))
                        translate(left = solid) { drawRect(edge, size = Size(end - solid, size.height)) }
                    }
                }
            },
    )
}

/** The small cover, or the G7 mark on a neutral ground without one. */
@Composable
private fun CoverThumb(artwork: Bitmap?, modifier: Modifier = Modifier) {
    Box(
        modifier.size(MINI_ART).clip(RoundedCornerShape(MINI_ART_CORNER)),
        contentAlignment = Alignment.Center,
    ) {
        if (artwork != null) {
            val image = remember(artwork) { artwork.asImageBitmap() }
            Image(image, contentDescription = null, modifier = Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
        } else {
            NoArtwork(markSize = 26.dp)
        }
    }
}

/** No cover: the G7 mark on the app's own neutral ground. */
@Composable
internal fun NoArtwork(markSize: Dp, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    Box(
        modifier
            .fillMaxSize()
            .background(Brush.verticalGradient(listOf(scheme.surfaceContainerHighest, scheme.surfaceContainerHigh))),
        contentAlignment = Alignment.Center,
    ) {
        Icon(G7Icons.mark, contentDescription = null, tint = scheme.outline, modifier = Modifier.size(markSize))
    }
}

private val MINI_ELEVATION = 12.dp
private val MINI_BUTTON = 48.dp
private val SKIP_DISTANCE = 56.dp
private val FILL_FEATHER = 28.dp
private val BACKDROP_BLUR = 22.dp
private const val BACKDROP_ZOOM = 1.3f
private const val SWIPE_FOLLOW = 0.6f
private const val DISABLED_ALPHA = 0.45f
private const val AXIS_NONE = 0
private const val AXIS_SIDEWAYS = 1
private const val AXIS_UP = 2
