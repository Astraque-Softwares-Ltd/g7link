// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.theme

import android.animation.ValueAnimator
import android.app.Activity
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.core.view.WindowCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner

val LocalReducedMotion = compositionLocalOf { false }

/** True when Android's "Remove animations" (animator duration scale 0) is enabled. */
@Composable
fun rememberReducedMotion(): Boolean {
    var reduced by remember { mutableStateOf(!ValueAnimator.areAnimatorsEnabled()) }
    val lifecycleOwner = LocalLifecycleOwner.current
    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) {
                reduced = !ValueAnimator.areAnimatorsEnabled()
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }
    return reduced
}

/** Material color roles of one theme variant. */
@Immutable
data class Palette(
    val primary: Color,
    val onPrimary: Color,
    val primaryContainer: Color,
    val onPrimaryContainer: Color,
    val secondaryContainer: Color,
    val onSecondaryContainer: Color,
    val surface: Color,
    val surfaceContainerLow: Color,
    val surfaceContainer: Color,
    val surfaceContainerHigh: Color,
    val surfaceContainerHighest: Color,
    val onSurface: Color,
    val onSurfaceVariant: Color,
    val outline: Color,
    val outlineVariant: Color,
)

/** The user's look choices (see Preferences). */
@Immutable
data class Appearance(
    /** "quiet" (G7) or the id of one of [Palettes.families]. */
    val theme: String = "quiet",
    val mode: String = "dark",
)

/** One variant of a theme: its Material roles, error color and glyph tints. */
@Immutable
data class ThemeColors(val palette: Palette, val error: Color, val onError: Color?, val tints: G7Tints) {
    companion object {
        val g7Dark = ThemeColors(G7Colors.darkPalette, G7Colors.darkError, null, G7Tints.dark)
        val g7Light = ThemeColors(G7Colors.lightPalette, G7Colors.lightError, null, G7Tints.light)
    }
}

/**
 * What the active theme paints beyond Material's roles: the glyph tints, and the
 * always-dark look of the camera, scanner and desk screens (the theme's dark variant
 * on a deeper ground).
 */
@Immutable
data class G7Look(val tints: G7Tints, val deep: ThemeColors, val deepGround: Color)

val LocalG7Look = staticCompositionLocalOf {
    G7Look(G7Tints.dark, ThemeColors.g7Dark, G7Colors.deepGround)
}

/** Per channel, in sRGB: [a] at 0, [b] at 1. */
private fun mix(a: Color, b: Color, t: Float) = Color(
    red = a.red + (b.red - a.red) * t,
    green = a.green + (b.green - a.green) * t,
    blue = a.blue + (b.blue - a.blue) * t,
)

private fun contrast(a: Color, b: Color): Float {
    val la = a.luminance()
    val lb = b.luminance()
    return (maxOf(la, lb) + 0.05f) / (minOf(la, lb) + 0.05f)
}

/** The ground or the ink, whichever reads better on [color]. */
private fun LauncherColors.inkOn(color: Color) =
    if (contrast(background, color) >= contrast(foreground, color)) background else foreground

/**
 * A G7 Linux theme's colors on the G7 theme's roles: the same layout, its colors.
 * Raised and selected surfaces are the ground with a little of the ink in it.
 */
internal fun LauncherColors.toThemeColors(): ThemeColors {
    val raised = mix(background, foreground, 0.06f)
    val selection = mix(background, foreground, 0.11f)
    val muted = mix(background, foreground, 0.62f)
    return ThemeColors(
        palette = Palette(
            primary = accent,
            onPrimary = inkOn(accent),
            primaryContainer = selection,
            onPrimaryContainer = foreground,
            secondaryContainer = selection,
            onSecondaryContainer = foreground,
            surface = background,
            surfaceContainerLow = background,
            surfaceContainer = raised,
            surfaceContainerHigh = selection,
            surfaceContainerHighest = mix(background, foreground, 0.14f),
            onSurface = foreground,
            onSurfaceVariant = muted,
            outline = mix(background, foreground, 0.40f),
            outlineVariant = border,
        ),
        error = red,
        onError = inkOn(red),
        tints = G7Tints(
            blue = blue,
            teal = cyan,
            coral = orange,
            pink = magenta,
            amber = yellow,
            green = green,
            red = red,
            lavender = purple,
            muted = muted,
        ),
    )
}

/** Whether [appearance] shows dark: the mode's choice, or always for a dark-only palette. */
fun isDark(appearance: Appearance, systemDark: Boolean): Boolean {
    val family = Palettes.families.firstOrNull { it.id == appearance.theme }
    if (family != null && family.light == null) return true
    return when (appearance.mode) {
        "light" -> false
        "dark" -> true
        else -> systemDark
    }
}

/** The colors [appearance] shows in light or dark: G7's own, or a palette's. */
fun themeColors(theme: String, dark: Boolean): ThemeColors {
    val family = Palettes.families.firstOrNull { it.id == theme }
        ?: return if (dark) ThemeColors.g7Dark else ThemeColors.g7Light
    return (if (dark) family.dark else family.light ?: family.dark).toThemeColors()
}

private fun ThemeColors.toScheme(dark: Boolean): ColorScheme {
    val p = palette
    val base = if (dark) darkColorScheme() else lightColorScheme()
    val scheme = base.copy(
        primary = p.primary,
        onPrimary = p.onPrimary,
        primaryContainer = p.primaryContainer,
        onPrimaryContainer = p.onPrimaryContainer,
        secondary = p.primary,
        onSecondary = p.onPrimary,
        secondaryContainer = p.secondaryContainer,
        onSecondaryContainer = p.onSecondaryContainer,
        tertiary = p.primary,
        onTertiary = p.onPrimary,
        tertiaryContainer = p.secondaryContainer,
        onTertiaryContainer = p.onSecondaryContainer,
        background = p.surface,
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
        error = error,
    )
    return if (onError != null) scheme.copy(onError = onError) else scheme
}

/** G7's shapes: small radii, the same for every palette. */
private val g7Shapes = Shapes(
    extraSmall = RoundedCornerShape(5.dp),
    small = RoundedCornerShape(8.dp),
    medium = RoundedCornerShape(10.dp),
    large = RoundedCornerShape(10.dp),
    extraLarge = RoundedCornerShape(14.dp),
)

/** G7: the accent when on, a quiet line when off, no outline. */
@Composable
fun g7SwitchColors(): androidx.compose.material3.SwitchColors {
    val scheme = MaterialTheme.colorScheme
    return androidx.compose.material3.SwitchDefaults.colors(
        checkedThumbColor = scheme.surface,
        checkedTrackColor = scheme.primary,
        checkedBorderColor = scheme.primary,
        uncheckedThumbColor = scheme.onSurfaceVariant,
        uncheckedTrackColor = scheme.outlineVariant,
        uncheckedBorderColor = scheme.outlineVariant,
    )
}

/**
 * The app's theme: G7's layout, type and shapes, in G7's colors or a palette's.
 * Switching palettes only recolors.
 */
@Composable
fun G7LinkTheme(appearance: Appearance = Appearance(), content: @Composable () -> Unit) {
    val dark = isDark(appearance, isSystemInDarkTheme())
    val colors = remember(appearance.theme, dark) { themeColors(appearance.theme, dark) }
    val scheme = remember(colors, dark) { colors.toScheme(dark) }
    val look = remember(appearance.theme, colors) {
        val deep = themeColors(appearance.theme, dark = true)
        // G7's deeper ground is its dark ground at 60%; palettes go the same way.
        val isPalette = Palettes.families.any { it.id == appearance.theme }
        val ground = if (isPalette) mix(deep.palette.surface, Color.Black, 0.4f) else G7Colors.deepGround
        G7Look(tints = colors.tints, deep = deep, deepGround = ground)
    }
    val reducedMotion = rememberReducedMotion()

    // System bar icons follow the app's theme, which can differ from the system's.
    val view = LocalView.current
    if (!view.isInEditMode) {
        androidx.compose.runtime.LaunchedEffect(view, dark) {
            val window = (view.context as? Activity)?.window ?: return@LaunchedEffect
            WindowCompat.getInsetsController(window, view).apply {
                isAppearanceLightStatusBars = !dark
                isAppearanceLightNavigationBars = !dark
            }
        }
    }
    val assets = LocalContext.current.assets
    val fonts = remember(assets) { AppFonts(assets) }
    val type = remember(fonts) { typography(fonts) }
    CompositionLocalProvider(
        LocalAppFonts provides fonts,
        LocalReducedMotion provides reducedMotion,
        LocalG7Look provides look,
    ) {
        MaterialTheme(colorScheme = scheme, shapes = g7Shapes, typography = type, content = content)
    }
}

/**
 * Lightweight zero-RenderNode interior well for bento strips and inner rows.
 */
fun Modifier.bentoWell(
    cornerRadius: Dp = 16.dp,
    fillColor: Color,
    borderColor: Color = Color.Transparent,
): Modifier = this.drawWithCache {
    val r = cornerRadius.toPx()
    val cr = CornerRadius(r, r)
    val strokePx = 1.dp.toPx()
    val halfStroke = strokePx * 0.5f
    val topLeftOffset = Offset(halfStroke, halfStroke)
    val insetSize = Size(
        (size.width - strokePx).coerceAtLeast(0f),
        (size.height - strokePx).coerceAtLeast(0f),
    )
    val strokeStyle = Stroke(width = strokePx)
    onDrawBehind {
        drawRoundRect(color = fillColor, cornerRadius = cr)
        if (borderColor.alpha > 0f) {
            drawRoundRect(
                color = borderColor,
                topLeft = topLeftOffset,
                size = insetSize,
                cornerRadius = cr,
                style = strokeStyle,
            )
        }
    }
}
