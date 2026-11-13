// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.theme

import androidx.compose.animation.AnimatedContentTransitionScope
import androidx.compose.animation.ContentTransform
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.FiniteAnimationSpec
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.VisibilityThreshold
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.snap
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyItemScope
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.State
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorProducer
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.drawOutline
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp

/**
 * G7's motion, shared with the desktop app and the launcher (docs/design/motion.md, and the
 * `motion` tokens in docs/design/tokens.json).
 *
 * The transitions here turn into none under [LocalReducedMotion]; timed ones also follow
 * Android's animator scale through Compose.
 */
object G7Motion {
    const val QUICK_IN = 120
    const val QUICK_OUT = 90
    const val SMALL_IN = 200
    const val SMALL_OUT = 140
    const val LARGE_IN = 260
    const val LARGE_OUT = 180

    /** Something arriving decelerates into place. */
    val Enter = CubicBezierEasing(0.05f, 0.7f, 0.1f, 1f)

    /** Something leaving accelerates away. */
    val Exit = CubicBezierEasing(0.3f, 0f, 0.8f, 0.15f)

    /** A change on screen: color, position, an indicator. */
    val Move = CubicBezierEasing(0.2f, 0f, 0f, 1f)

    /** How far an element rises as it enters. */
    val RISE = 8.dp

    /** Pages slide this share of their width. */
    const val PAGE_SLIDE = 0.1f

    /** Popovers, menus and sheets grow from this scale. */
    const val POP_SCALE = 0.96f

    private const val SPRING_DAMPING = 0.85f

    fun <T> enter(durationMillis: Int): FiniteAnimationSpec<T> = tween(durationMillis, easing = Enter)

    fun <T> exit(durationMillis: Int): FiniteAnimationSpec<T> = tween(durationMillis, easing = Exit)

    fun <T> move(durationMillis: Int): FiniteAnimationSpec<T> = tween(durationMillis, easing = Move)

    /** For motion that carries on from a gesture or can be interrupted: no bounce. */
    fun <T> follow(visibilityThreshold: T? = null): FiniteAnimationSpec<T> =
        spring(SPRING_DAMPING, Spring.StiffnessMediumLow, visibilityThreshold)
}

/** Pages and tab content: they slide a tenth of their width on a spring, and fade. */
fun <S> AnimatedContentTransitionScope<S>.g7Page(forward: Boolean, reducedMotion: Boolean): ContentTransform {
    if (reducedMotion) return EnterTransition.None togetherWith ExitTransition.None
    val slide = G7Motion.follow(IntOffset.VisibilityThreshold)
    val sign = if (forward) 1 else -1
    return (
        slideInHorizontally(slide) { (it * G7Motion.PAGE_SLIDE).toInt() * sign } +
            fadeIn(G7Motion.enter(G7Motion.LARGE_IN))
        ) togetherWith (
        slideOutHorizontally(slide) { -(it * G7Motion.PAGE_SLIDE).toInt() * sign } +
            fadeOut(G7Motion.exit(G7Motion.LARGE_OUT))
        )
}

/** A change of page with no direction to it: a crossfade. */
fun <S> AnimatedContentTransitionScope<S>.g7Crossfade(reducedMotion: Boolean): ContentTransform =
    if (reducedMotion) {
        EnterTransition.None togetherWith ExitTransition.None
    } else {
        fadeIn(G7Motion.enter(G7Motion.LARGE_IN)) togetherWith fadeOut(G7Motion.exit(G7Motion.LARGE_OUT))
    }

/** A small element arriving: it fades in and rises [G7Motion.RISE]. */
@Composable
@ReadOnlyComposable
fun g7Appear(durationMillis: Int = G7Motion.SMALL_IN): EnterTransition {
    if (LocalReducedMotion.current) return EnterTransition.None
    val rise = with(LocalDensity.current) { G7Motion.RISE.roundToPx() }
    return fadeIn(G7Motion.enter(durationMillis)) +
        slideInVertically(G7Motion.enter<IntOffset>(durationMillis)) { rise }
}

/** A small element leaving: it fades out. */
@Composable
@ReadOnlyComposable
fun g7Disappear(durationMillis: Int = G7Motion.SMALL_OUT): ExitTransition =
    if (LocalReducedMotion.current) ExitTransition.None else fadeOut(G7Motion.exit(durationMillis))

/** A popover or menu arriving: it fades in and grows from [G7Motion.POP_SCALE]. */
@Composable
@ReadOnlyComposable
fun g7PopIn(): EnterTransition =
    if (LocalReducedMotion.current) {
        EnterTransition.None
    } else {
        fadeIn(G7Motion.enter(G7Motion.SMALL_IN)) +
            scaleIn(G7Motion.enter(G7Motion.SMALL_IN), initialScale = G7Motion.POP_SCALE)
    }

/** A popover or menu leaving. */
@Composable
@ReadOnlyComposable
fun g7PopOut(): ExitTransition =
    if (LocalReducedMotion.current) {
        ExitTransition.None
    } else {
        fadeOut(G7Motion.exit(G7Motion.SMALL_OUT)) +
            scaleOut(G7Motion.exit(G7Motion.SMALL_OUT), targetScale = G7Motion.POP_SCALE)
    }

/** A section opening: it grows to its height while its content fades in. */
@Composable
@ReadOnlyComposable
fun g7Expand(): EnterTransition =
    if (LocalReducedMotion.current) {
        EnterTransition.None
    } else {
        expandVertically(G7Motion.enter<IntSize>(G7Motion.LARGE_IN)) + fadeIn(G7Motion.enter(G7Motion.LARGE_IN))
    }

/** A section closing. */
@Composable
@ReadOnlyComposable
fun g7Collapse(): ExitTransition =
    if (LocalReducedMotion.current) {
        ExitTransition.None
    } else {
        shrinkVertically(G7Motion.exit<IntSize>(G7Motion.LARGE_OUT)) + fadeOut(G7Motion.exit(G7Motion.LARGE_OUT))
    }

/**
 * A row of a list that changes now and then (Home's notices and cards): it fades in and out, and
 * the rows around it slide over. Placement moves its layer, so neighbours aren't laid out again.
 */
@Composable
fun LazyItemScope.g7ItemMotion(): Modifier =
    if (LocalReducedMotion.current) {
        Modifier
    } else {
        Modifier.animateItem(
            fadeInSpec = G7Motion.enter(G7Motion.SMALL_IN),
            placementSpec = G7Motion.move(G7Motion.SMALL_IN),
            fadeOutSpec = G7Motion.exit(G7Motion.SMALL_OUT),
        )
    }

/** [LazyListScope.item] with [g7ItemMotion]. */
fun LazyListScope.g7Item(key: Any, contentType: Any? = null, content: @Composable LazyItemScope.() -> Unit) {
    item(key = key, contentType = contentType) {
        val itemScope = this
        Box(itemScope.g7ItemMotion()) { itemScope.content() }
    }
}

/** A chevron that turns over as its section opens or closes. */
@Composable
fun G7Chevron(open: Boolean, modifier: Modifier = Modifier) {
    val turn by animateFloatAsState(
        targetValue = if (open) 180f else 0f,
        animationSpec = if (LocalReducedMotion.current) snap() else G7Motion.move(G7Motion.SMALL_IN),
        label = "ChevronTurn",
    )
    Icon(
        imageVector = G7Icons.chevronDown,
        contentDescription = null,
        tint = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = modifier.size(18.dp).graphicsLayer { rotationZ = turn },
    )
}

/**
 * How far a state such as selected or highlighted has come on, from 0 to 1, on the quick timing.
 * Read it while drawing (in a draw lambda, `graphicsLayer` or a [ColorProducer]) so its frames
 * neither lay out nor recompose anything.
 */
@Composable
fun g7StateFraction(on: Boolean, label: String = "G7State"): State<Float> =
    animateFloatAsState(
        targetValue = if (on) 1f else 0f,
        animationSpec = when {
            LocalReducedMotion.current -> snap()
            on -> G7Motion.move(G7Motion.QUICK_IN)
            else -> G7Motion.move(G7Motion.QUICK_OUT)
        },
        label = label,
    )

/**
 * A [fill] and an [edge] in [shape], faded in by [shown] (see [g7StateFraction]) over [under], which
 * fades out. Place it after a clip to the same shape: the edge is stroked on the outline and the
 * clip keeps its inner half.
 */
fun Modifier.g7Plate(
    shown: State<Float>,
    fill: Color,
    edge: Color?,
    shape: Shape,
    under: Color = Color.Transparent,
    edgeWidth: Dp = 1.dp,
): Modifier =
    drawWithCache {
        val outline = shape.createOutline(size, layoutDirection, this)
        val stroke = Stroke(width = edgeWidth.toPx() * 2f)
        onDrawBehind {
            val f = shown.value
            if (f < 1f && under.alpha > 0f) drawOutline(outline, under.copy(alpha = under.alpha * (1f - f)))
            if (f > 0f) {
                drawOutline(outline, fill.copy(alpha = fill.alpha * f))
                if (edge != null) drawOutline(outline, edge.copy(alpha = edge.alpha * f), style = stroke)
            }
        }
    }

/**
 * One small piece of content swapping for another, or for nothing (a status banner): it fades
 * across while the room it takes grows or shrinks to the new size.
 */
fun <S> AnimatedContentTransitionScope<S>.g7Swap(reducedMotion: Boolean): ContentTransform =
    if (reducedMotion) {
        (EnterTransition.None togetherWith ExitTransition.None) using null
    } else {
        (fadeIn(G7Motion.enter(G7Motion.SMALL_IN)) togetherWith fadeOut(G7Motion.exit(G7Motion.SMALL_OUT))) using
            SizeTransform { _, _ -> G7Motion.move(G7Motion.SMALL_IN) }
    }
