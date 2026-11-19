// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.player

import android.os.SystemClock
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedContentTransitionScope
import androidx.compose.animation.AnimatedVisibilityScope
import androidx.compose.animation.BoundsTransform
import androidx.compose.animation.ContentTransform
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.SharedTransitionLayout
import androidx.compose.animation.SharedTransitionScope
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.AnimationSpec
import androidx.compose.animation.core.SeekableTransitionState
import androidx.compose.animation.core.rememberTransition
import androidx.compose.animation.core.snap
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.animation.using
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.ColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.LongState
import androidx.compose.runtime.ProvidableCompositionLocal
import androidx.compose.runtime.Stable
import androidx.compose.runtime.State
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.withFrameMillis
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.rememberVectorPainter
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.g7link.android.G7LinkApplication
import app.g7link.android.core.Core
import app.g7link.android.ui.theme.G7Motion
import app.g7link.android.ui.theme.LocalReducedMotion
import app.g7link.core.FfiRolePalette
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** How far lists keep their last row above the mini player; MainActivity provides [rememberMiniPlayerInset]. */
val LocalMiniPlayerInset: ProvidableCompositionLocal<Dp> = compositionLocalOf { 0.dp }

/** The bottom inset a list under the mini player needs: [MINI_INSET] while a PC has a player, else none. */
@Composable
fun rememberMiniPlayerInset(): Dp {
    val core = rememberCore() ?: return 0.dp
    val players by core.pcMedia.everyPlayer.collectAsStateWithLifecycle()
    return if (players.isEmpty()) 0.dp else MINI_INSET
}

/**
 * What plays on the PCs, over the Main route: the mini player floating [bottomBarHeight] + 8 dp
 * up, above the tabs, and the full player over everything, tabs included. The mini player's card
 * grows into the full player (shared bounds, the cover as the shared element); a drag up on the
 * card or down on the cover follows the finger and settles on a spring. Back, the chevron or a
 * swipe down closes it. Under reduced motion the two only crossfade.
 *
 * Nothing is composed while no PC has a player.
 */
@Composable
fun PlayerLayer(bottomBarHeight: Dp, modifier: Modifier = Modifier) {
    val core = rememberCore() ?: return
    val media = core.pcMedia
    val players by media.everyPlayer.collectAsStateWithLifecycle()
    val timelines by media.timelines.collectAsStateWithLifecycle()
    val reducedMotion = LocalReducedMotion.current
    var pick by remember { mutableStateOf<PlayerModel.Pick?>(null) }
    val track = PlayerModel.shown(players, pick)

    val state = remember { SeekableTransitionState(false) }
    val transition = rememberTransition(state, label = "player")
    val gesture = remember { ExpandGesture() }
    LaunchedEffect(state, gesture, reducedMotion) { gesture.run(state, reducedMotion) }
    val nothing = track == null
    LaunchedEffect(nothing) { if (nothing) gesture.reset() }
    BackHandler(enabled = !nothing && state.targetState) { gesture.close() }
    if (track == null) return

    val timeline = timelines["${track.pc}/${track.playerId}"]
    var layerHeight by remember { mutableIntStateOf(0) }
    SharedTransitionLayout(modifier.fillMaxSize().onSizeChanged { layerHeight = it.height }) {
        val bounds = remember(transition) {
            BoundsTransform { _, _ ->
                if (transition.targetState) G7Motion.move(G7Motion.LARGE_IN) else G7Motion.move(G7Motion.LARGE_OUT)
            }
        }
        val sharedScope = this
        transition.AnimatedContent(
            modifier = Modifier.fillMaxSize(),
            transitionSpec = { playerSwap(reducedMotion) },
            contentAlignment = Alignment.BottomCenter,
        ) { expanded ->
            val visibility: AnimatedVisibilityScope = this
            val motion = remember(reducedMotion, visibility, bounds) {
                PlayerMotion(if (reducedMotion) null else sharedScope, visibility, bounds)
            }
            if (expanded) {
                FullPlayer(
                    track = track,
                    timeline = timeline,
                    players = players,
                    media = media,
                    core = core,
                    motion = motion,
                    dragDistance = layerHeight * CLOSE_DRAG_SHARE,
                    onClose = gesture::close,
                    onDrag = { fraction -> gesture.drag(toward = false, fraction = fraction) },
                    onRelease = gesture::release,
                    onPick = { picked -> pick = PlayerModel.Pick(picked.pc, picked.playerId, SystemClock.elapsedRealtime()) },
                )
            } else {
                MiniPlayer(
                    track = track,
                    timeline = timeline,
                    media = media,
                    motion = motion,
                    bottomPadding = bottomBarHeight + MINI_ABOVE_BAR,
                    dragDistance = layerHeight * OPEN_DRAG_SHARE,
                    onOpen = gesture::open,
                    onDrag = { fraction -> gesture.drag(toward = true, fraction = fraction) },
                    onRelease = gesture::release,
                )
            }
        }
    }
}

/** The mini and full player's swap: shared elements carry it, so only a fade here; under reduced motion a quick crossfade. */
private fun AnimatedContentTransitionScope<Boolean>.playerSwap(reducedMotion: Boolean): ContentTransform =
    if (reducedMotion) {
        (fadeIn(G7Motion.enter(G7Motion.QUICK_IN)) togetherWith fadeOut(G7Motion.exit(G7Motion.QUICK_OUT))) using null
    } else {
        (fadeIn(G7Motion.enter(G7Motion.LARGE_IN)) togetherWith fadeOut(G7Motion.exit(G7Motion.LARGE_OUT))) using null
    }

/**
 * Opens and closes the full player, one command at a time, so a seek never cancels the settle
 * after it: a drag seeks the transition to how far the finger has gone, its release settles it
 * (open past [PlayerModel.SETTLE_FRACTION], else back), and a tap, the chevron or Back animate it.
 */
private class ExpandGesture {
    private val commands = Channel<Float>(Channel.CONFLATED)

    /** The drag under way: whether it opens, and how far it has taken it (0 to 1). */
    private var toward = true
    private var fraction = 0f

    fun drag(toward: Boolean, fraction: Float) {
        this.toward = toward
        this.fraction = fraction.coerceIn(0f, 1f)
        commands.trySend(this.fraction)
    }

    fun release() {
        commands.trySend(RELEASE)
    }

    fun open() {
        commands.trySend(OPEN)
    }

    fun close() {
        commands.trySend(CLOSE)
    }

    fun reset() {
        commands.trySend(RESET)
    }

    suspend fun run(state: SeekableTransitionState<Boolean>, reducedMotion: Boolean) {
        for (command in commands) {
            when {
                command >= 0f -> if (!reducedMotion) state.seekTo(command, toward)
                command == RELEASE -> {
                    val settles = fraction >= PlayerModel.SETTLE_FRACTION
                    fraction = 0f
                    when {
                        settles -> state.animateTo(toward, if (reducedMotion) null else G7Motion.follow<Float>())
                        state.currentState != state.targetState -> unwind(state)
                    }
                }
                command == OPEN -> settle(state, true)
                command == CLOSE -> settle(state, false)
                command == RESET -> state.snapTo(false)
            }
        }
    }

    private suspend fun settle(state: SeekableTransitionState<Boolean>, to: Boolean) {
        when {
            state.currentState != state.targetState && state.currentState == to -> unwind(state)
            state.currentState != to || state.targetState != to -> state.animateTo(to)
        }
    }

    /** Runs a part-way seek back to where it started, on the gesture spring. */
    private suspend fun unwind(state: SeekableTransitionState<Boolean>) {
        val toward = state.targetState
        val back = Animatable(state.fraction)
        coroutineScope {
            val follower = launch { snapshotFlow { back.value }.collect { state.seekTo(it, toward) } }
            back.animateTo(0f, G7Motion.follow<Float>())
            follower.cancel()
        }
        state.snapTo(state.currentState)
    }

    private companion object {
        const val RELEASE = -1f
        const val OPEN = -2f
        const val CLOSE = -3f
        const val RESET = -4f
    }
}

/** What the two players need to share elements: none under reduced motion. */
@Stable
internal class PlayerMotion(
    val shared: SharedTransitionScope?,
    val visibility: AnimatedVisibilityScope,
    val bounds: BoundsTransform,
) {
    val enter: EnterTransition = fadeIn(G7Motion.enter(G7Motion.LARGE_IN))
    val exit: ExitTransition = fadeOut(G7Motion.exit(G7Motion.LARGE_OUT))
}

/** The shared state for [key], or none under reduced motion. */
@Composable
internal fun PlayerMotion.rememberKey(key: String): SharedTransitionScope.SharedContentState? =
    shared?.rememberSharedContentState(key)

/** This element is the same element in the other player (the cover). Size modifiers go after it. */
internal fun Modifier.sharedElementOf(motion: PlayerMotion, state: SharedTransitionScope.SharedContentState?): Modifier {
    val shared = motion.shared ?: return this
    if (state == null) return this
    return with(shared) {
        this@sharedElementOf.sharedElement(
            sharedContentState = state,
            animatedVisibilityScope = motion.visibility,
            boundsTransform = motion.bounds,
        )
    }
}

/** This element's bounds become the other player's (the card, the title, the play button), crossfading inside. */
internal fun Modifier.sharedBoundsOf(motion: PlayerMotion, state: SharedTransitionScope.SharedContentState?): Modifier {
    val shared = motion.shared ?: return this
    if (state == null) return this
    return with(shared) {
        this@sharedBoundsOf.sharedBounds(
            sharedContentState = state,
            animatedVisibilityScope = motion.visibility,
            enter = motion.enter,
            exit = motion.exit,
            boundsTransform = motion.bounds,
            resizeMode = SharedTransitionScope.ResizeMode.scaleToBounds(),
        )
    }
}

/**
 * The player's colours: the extractor's tonal roles for the cover (see [PlayerModel.roles]), or
 * the app's own. They're used in their pairs: [ink] and [inkMuted] on [ground] and [groundHigh],
 * [onAccent] on [accent].
 */
internal data class PlayerPalette(
    val ground: Color,
    val groundHigh: Color,
    val ink: Color,
    val inkMuted: Color,
    val accent: Color,
    val onAccent: Color,
    val track: Color,
) {
    companion object {
        fun fromRoles(roles: FfiRolePalette) = PlayerPalette(
            ground = rgb(roles.surface),
            groundHigh = rgb(roles.surfaceContainerHigh),
            ink = rgb(roles.onSurface),
            inkMuted = rgb(roles.onSurfaceVariant),
            accent = rgb(roles.primary),
            onAccent = rgb(roles.onPrimary),
            track = rgb(roles.outlineVariant),
        )

        fun fromScheme(scheme: ColorScheme) = PlayerPalette(
            ground = scheme.surface,
            groundHigh = scheme.surfaceContainerHigh,
            ink = scheme.onSurface,
            inkMuted = scheme.onSurfaceVariant,
            accent = scheme.primary,
            onAccent = scheme.onPrimary,
            track = scheme.outlineVariant,
        )

        private fun rgb(value: UInt) = Color(0xFF000000.toInt() or (value.toInt() and 0xFFFFFF))
    }
}

/** [PlayerPalette] moving to a new cover's colours: read the values while drawing. */
@Stable
internal class PaletteState(
    val ground: State<Color>,
    val groundHigh: State<Color>,
    val ink: State<Color>,
    val inkMuted: State<Color>,
    val accent: State<Color>,
    val onAccent: State<Color>,
    val track: State<Color>,
)

/** [target], with colour changes on the move timing (large), instant under reduced motion. */
@Composable
internal fun animatedPalette(target: PlayerPalette): PaletteState {
    val spec: AnimationSpec<Color> = if (LocalReducedMotion.current) snap() else G7Motion.move(G7Motion.LARGE_IN)
    val ground = animateColorAsState(target.ground, spec, label = "playerGround")
    val groundHigh = animateColorAsState(target.groundHigh, spec, label = "playerGroundHigh")
    val ink = animateColorAsState(target.ink, spec, label = "playerInk")
    val inkMuted = animateColorAsState(target.inkMuted, spec, label = "playerInkMuted")
    val accent = animateColorAsState(target.accent, spec, label = "playerAccent")
    val onAccent = animateColorAsState(target.onAccent, spec, label = "playerOnAccent")
    val track = animateColorAsState(target.track, spec, label = "playerTrack")
    return remember(ground, groundHigh, ink, inkMuted, accent, onAccent, track) {
        PaletteState(ground, groundHigh, ink, inkMuted, accent, onAccent, track)
    }
}

/** An icon drawn in [tint], read while drawing so a colour change doesn't compose it again. */
@Composable
internal fun TintedIcon(icon: ImageVector, tint: State<Color>, modifier: Modifier = Modifier) {
    val painter = rememberVectorPainter(icon)
    Spacer(
        modifier.drawBehind {
            with(painter) { draw(size, colorFilter = ColorFilter.tint(tint.value)) }
        },
    )
}

/**
 * Now (`elapsedRealtime`), for drawing where a player is: every frame with [everyFrame], else
 * every [MINI_TICK_MS], only while [running] and only while frames are drawn.
 */
@Composable
internal fun rememberPlayerClock(running: Boolean, everyFrame: Boolean): LongState {
    val clock = remember { mutableLongStateOf(SystemClock.elapsedRealtime()) }
    LaunchedEffect(running, everyFrame) {
        clock.longValue = SystemClock.elapsedRealtime()
        while (running) {
            if (!everyFrame) delay(MINI_TICK_MS)
            withFrameMillis { clock.longValue = SystemClock.elapsedRealtime() }
        }
    }
    return clock
}

@Composable
private fun rememberCore(): Core? {
    val context = LocalContext.current
    return remember(context) { (context.applicationContext as? G7LinkApplication)?.core }
}

/** Lists under the mini player pad their bottom this much, so their last row ends above it. */
internal val MINI_INSET = 72.dp
internal val MINI_HEIGHT = 64.dp
internal val MINI_CORNER = 12.dp
internal val MINI_SIDE = 12.dp
internal val MINI_ABOVE_BAR = 8.dp
internal val MINI_ART = 48.dp
internal val MINI_ART_CORNER = 8.dp

/** How often the mini player's played part moves on. */
internal const val MINI_TICK_MS = 500L

/** How much of the screen's height a drag takes to open or close the player all the way. */
private const val OPEN_DRAG_SHARE = 0.5f
private const val CLOSE_DRAG_SHARE = 0.4f

internal const val BOUNDS_KEY = "player-bounds"
internal const val ART_KEY = "player-art"
internal const val TITLE_KEY = "player-title"
internal const val PLAY_KEY = "player-play"
