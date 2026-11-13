// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.theme

import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.snap
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.draw.dropShadow
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathFillType
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.graphics.drawOutline
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.shadow.Shadow
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.DpOffset
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.g7link.android.R

/**
 * G7's "Quiet Tool" colors (docs/design-language.md in G7 Linux): charcoal grounds,
 * one lavender accent, and icons drawn in their own tints. Palettes map theirs onto
 * the same roles (see [toThemeColors]).
 */
object G7Colors {
    val darkPalette = Palette(
        primary = Color(0xFFB292D0),
        onPrimary = Color(0xFF18181B),
        primaryContainer = Color(0xFF2B2B30),
        onPrimaryContainer = Color(0xFFF2F2F3),
        secondaryContainer = Color(0xFF2B2B30),
        onSecondaryContainer = Color(0xFFF2F2F3),
        surface = Color(0xFF18181B),
        surfaceContainerLow = Color(0xFF18181B),
        surfaceContainer = Color(0xFF222226),
        surfaceContainerHigh = Color(0xFF2B2B30),
        surfaceContainerHighest = Color(0xFF303035),
        onSurface = Color(0xFFF2F2F3),
        onSurfaceVariant = Color(0xFFA1A1A8),
        outline = Color(0xFF6F6F78),
        outlineVariant = Color(0xFF303035),
    )

    val lightPalette = Palette(
        primary = Color(0xFF7D5CA9),
        onPrimary = Color(0xFFFAFAFB),
        primaryContainer = Color(0xFFE9E9ED),
        onPrimaryContainer = Color(0xFF1C1C1F),
        secondaryContainer = Color(0xFFE9E9ED),
        onSecondaryContainer = Color(0xFF1C1C1F),
        surface = Color(0xFFFAFAFB),
        surfaceContainerLow = Color(0xFFFAFAFB),
        surfaceContainer = Color(0xFFF1F1F3),
        surfaceContainerHigh = Color(0xFFE9E9ED),
        surfaceContainerHighest = Color(0xFFE4E4E9),
        onSurface = Color(0xFF1C1C1F),
        onSurfaceVariant = Color(0xFF5F5F67),
        outline = Color(0xFF7A7A83),
        outlineVariant = Color(0xFFDCDCE1),
    )

    val darkError = Color(0xFFE5736F)
    val lightError = Color(0xFFA4230F)

    /** The camera and desk screens sit on a deeper ground. */
    val deepGround = Color(0xFF0E0E10)
}

/** The icon tints. Each feature keeps its color wherever it appears. */
@Immutable
data class G7Tints(
    val blue: Color,
    val teal: Color,
    val coral: Color,
    val pink: Color,
    val amber: Color,
    val green: Color,
    val red: Color,
    val lavender: Color,
    val muted: Color,
) {
    companion object {
        val dark = G7Tints(
            blue = Color(0xFF7CB7EE),
            teal = Color(0xFF5DCAA5),
            coral = Color(0xFFF0997B),
            pink = Color(0xFFED93B1),
            amber = Color(0xFFEF9F27),
            green = Color(0xFF97C459),
            red = Color(0xFFE5736F),
            lavender = Color(0xFFB292D0),
            muted = Color(0xFFA1A1A8),
        )
        val light = G7Tints(
            blue = Color(0xFF2F74B5),
            teal = Color(0xFF1E8A68),
            coral = Color(0xFFC5532F),
            pink = Color(0xFFB8436E),
            amber = Color(0xFFA86400),
            green = Color(0xFF4F7F1A),
            red = Color(0xFFB8322E),
            lavender = Color(0xFF7D5CA9),
            muted = Color(0xFF5F5F67),
        )
    }
}

/** The active theme's tints: G7's own, or a palette's. */
@Composable
fun g7Tints(): G7Tints = LocalG7Look.current.tints

private fun strokeIcon(name: String, path: String, width: Float = 1.7f): ImageVector =
    ImageVector.Builder(
        name = name,
        defaultWidth = 24.dp,
        defaultHeight = 24.dp,
        viewportWidth = 24f,
        viewportHeight = 24f,
    ).addPath(
        pathData = PathParser().parsePathString(path).toNodes(),
        fill = null,
        stroke = SolidColor(Color.White),
        strokeLineWidth = width,
        strokeLineCap = StrokeCap.Round,
        strokeLineJoin = StrokeJoin.Round,
    ).build()

/** One of G7 Linux's filled glyphs: its SVG paths on the 16 grid, filled as the SVG fills them. */
private fun glyph(name: String, vararg paths: String, evenOdd: Boolean = true): ImageVector {
    val builder = ImageVector.Builder(
        name = name,
        defaultWidth = 24.dp,
        defaultHeight = 24.dp,
        viewportWidth = 16f,
        viewportHeight = 16f,
    )
    for (path in paths) {
        builder.addPath(
            pathData = PathParser().parsePathString(path).toNodes(),
            pathFillType = if (evenOdd) PathFillType.EvenOdd else PathFillType.NonZero,
            fill = SolidColor(Color.White),
        )
    }
    return builder.build()
}

/**
 * G7 Linux's own filled 16 px glyphs (src/server/icons in g7linux), with the same
 * name-to-glyph choices as the desktop app's G7Glyphs.qml, drawn in a tint with
 * nothing behind them.
 */
object G7Icons {
    /** The G7 alone in one color, as G7 Linux's top bar and tray show it. */
    val mark by lazy {
        strokeIcon(
            "mark",
            "M21.12 4.08L8.64 4.08A6.48 6.48 0 0 0 8.64 17.04L10.56 17.04A2.88 2.88 0 0 0 10.56 11.28L7.92 11.28M21.12 4.08L16.32 21.12",
            width = 2.2f,
        )
    }
    val home by lazy {
        glyph(
            "home",
            "M7.47 1.22a.75.75 0 0 1 1.06 0l5.25 5.25 1 1A.75.75 0 0 1 14 8.707v4.543A1.75 1.75 0 0 1 12.25 15h-8.5A1.75 1.75 0 0 1 2 13.25V8.707a.75.75 0 0 1-.78-1.237l1-1zM3.5 7.31v5.94c0 .138.112.25.25.25h8.5a.25.25 0 0 0 .25-.25V7.31L8 2.81z",
        )
    }
    val guide by lazy {
        glyph(
            "guide",
            "M1 1.75A.75.75 0 0 1 1.75 1c1.768 0 3.438.541 4.641 1.062A16 16 0 0 1 8 2.87l.15-.087c.351-.197.852-.46 1.459-.722C10.813 1.54 12.482 1 14.25 1a.75.75 0 0 1 .75.75v10.5a.75.75 0 0 1-.75.75c-1.482 0-2.937.459-4.046.938a14 14 0 0 0-1.674.864l-.09.056-.02.014-.004.002a.75.75 0 0 1-.832 0l-.004-.002-.02-.014-.09-.056a14 14 0 0 0-1.674-.863C4.687 13.458 3.232 13 1.75 13a.75.75 0 0 1-.75-.75zm7.75 11.214c.253-.128.541-.265.859-.402 1.032-.447 2.406-.908 3.891-1.03V2.536c-1.216.12-2.375.503-3.296.901a14 14 0 0 0-1.454.732zM7.25 4.17l-.138-.08a14 14 0 0 0-1.316-.652c-.92-.398-2.08-.782-3.296-.9v8.993c1.485.123 2.86.584 3.891 1.03q.478.209.859.403z",
        )
    }
    val settings by lazy {
        glyph(
            "settings",
            "M7.205 1a1 1 0 0 0-.98.804L5.972 3.06l-1.07-.64a1 1 0 0 0-1.203.132l-1.076 1.02a1 1 0 0 0-.186 1.212l.657 1.182-.31.062-.981.196A1 1 0 0 0 1 7.205v1.569a1 1 0 0 0 .9.995l1.192.119-.682 1.365a1 1 0 0 0 .18 1.148l.958.993.01.01c.193.193.43.266.616.284a1.05 1.05 0 0 0 .605-.124l.89-.495.282-.156.232 1.267a1 1 0 0 0 .984.82h1.666a1 1 0 0 0 .984-.82l.232-1.267 1.172.651a1 1 0 0 0 1.193-.167l.989-.99a1 1 0 0 0 .187-1.154l-.67.336.67-.336-.682-1.365 1.192-.12a1 1 0 0 0 .9-.994V7.205a1 1 0 0 0-.804-.98l-1.291-.259.659-1.187a1 1 0 0 0-.167-1.193l-.492-.491-.491-.492a1 1 0 0 0-1.193-.167l-1.187.66-.062-.31-.196-.982A1 1 0 0 0 8.795 1zm.1 3.05.31-1.55h.77l.116.58.196.981a1 1 0 0 0 1.467.678l1.456-.809.225.225.225.225-.81 1.456a1 1 0 0 0 .679 1.467l1.561.312v.706l-1.464.147a1 1 0 0 0-.795 1.442l.846 1.692-.467.468-1.451-.806a1 1 0 0 0-1.47.693L8.417 13.5h-.834l-.282-1.543a1 1 0 0 0-1.47-.694l-.89.495-.577.321-.454-.471.849-1.698a1 1 0 0 0-.795-1.442L2.5 8.32v-.706l.58-.116.981-.196-.147-.736.147.736a1 1 0 0 0 .678-1.467l-.801-1.442.52-.493 1.353.81a1 1 0 0 0 1.494-.661M7.5 8a.5.5 0 1 1 1 0 .5.5 0 0 1-1 0M8 6a2 2 0 1 0 0 4 2 2 0 0 0 0-4",
        )
    }
    val upload by lazy {
        glyph(
            "upload",
            "M8 1a.75.75 0 0 1 .55.24l3.25 3.5a.75.75 0 0 1-1.1 1.02l-1.95-2.1v6.59a.75.75 0 0 1-1.5 0V3.66L5.3 5.76a.75.75 0 1 1-1.1-1.02l3.25-3.5A.75.75 0 0 1 8 1M1.75 11a.75.75 0 0 1 .75.75v.5c0 .69.56 1.25 1.25 1.25h8.5c.69 0 1.25-.56 1.25-1.25v-.5a.75.75 0 0 1 1.5 0v.5A2.75 2.75 0 0 1 12.25 15h-8.5A2.75 2.75 0 0 1 1 12.25v-.5a.75.75 0 0 1 .75-.75",
        )
    }
    val download by lazy {
        glyph(
            "download",
            "M8 1a.75.75 0 0 1 .75.75v6.59l1.95-2.1a.75.75 0 1 1 1.1 1.02l-3.25 3.5a.75.75 0 0 1-1.1 0L4.2 7.26a.75.75 0 0 1 1.1-1.02l1.95 2.1V1.75A.75.75 0 0 1 8 1M1.75 11a.75.75 0 0 1 .75.75v.5c0 .69.56 1.25 1.25 1.25h8.5c.69 0 1.25-.56 1.25-1.25v-.5a.75.75 0 0 1 1.5 0v.5A2.75 2.75 0 0 1 12.25 15h-8.5A2.75 2.75 0 0 1 1 12.25v-.5a.75.75 0 0 1 .75-.75",
        )
    }
    val clipboard by lazy {
        glyph(
            "clipboard",
            "M7.076 2.5a.25.25 0 0 0-.24.319l.292 1.023a.907.907 0 0 0 1.744 0l.293-1.023a.25.25 0 0 0-.24-.319zM5.494 2c.277-.582.869-1 1.582-1h1.848c.713 0 1.305.418 1.582 1h.744A2.75 2.75 0 0 1 14 4.75v7.5A2.75 2.75 0 0 1 11.25 15h-6.5A2.75 2.75 0 0 1 2 12.25v-7.5A2.75 2.75 0 0 1 4.75 2zm5.036 1.5-.215.754a2.407 2.407 0 0 1-4.63 0L5.47 3.5h-.72c-.69 0-1.25.56-1.25 1.25v7.5c0 .69.56 1.25 1.25 1.25h6.5c.69 0 1.25-.56 1.25-1.25v-7.5c0-.69-.56-1.25-1.25-1.25z",
        )
    }
    val link by lazy {
        glyph(
            "link",
            "M9.138 1.993a3.442 3.442 0 0 1 4.869 4.869l-1.7 1.89a.75.75 0 0 1-1.115-1.003l1.712-1.904.027-.03a1.942 1.942 0 1 0-2.776-2.719L8.251 4.808A.75.75 0 1 1 7.25 3.692zM9.78 6.22a.75.75 0 0 1 0 1.06l-2.5 2.5a.75.75 0 0 1-1.06-1.06l2.5-2.5a.75.75 0 0 1 1.06 0m-5.029.972a.75.75 0 0 1 .057 1.06l-1.712 1.903-.027.03a1.942 1.942 0 1 0 2.776 2.719l1.904-1.712a.75.75 0 0 1 1.002 1.116l-1.89 1.699a3.442 3.442 0 0 1-4.868-4.869l1.7-1.89a.75.75 0 0 1 1.058-.056",
        )
    }
    val pointer by lazy {
        glyph(
            "pointer",
            "M3 3.75A2.75 2.75 0 0 1 5.75 1h4.5A2.75 2.75 0 0 1 13 3.75v6.5A4.75 4.75 0 0 1 8.25 15h-.5A4.75 4.75 0 0 1 3 10.25zM5.75 2.5c-.69 0-1.25.56-1.25 1.25v6.5a3.25 3.25 0 0 0 3.25 3.25h.5a3.25 3.25 0 0 0 3.25-3.25v-6.5c0-.69-.56-1.25-1.25-1.25h-1.5v1.75a.75.75 0 0 1-1.5 0V2.5z",
        )
    }
    val deck by lazy {
        glyph(
            "deck",
            "M3.75 1A2.75 2.75 0 0 0 1 3.75v8.5A2.75 2.75 0 0 0 3.75 15h8.5A2.75 2.75 0 0 0 15 12.25v-8.5A2.75 2.75 0 0 0 12.25 1zM2.5 3.75c0-.69.56-1.25 1.25-1.25h8.5c.69 0 1.25.56 1.25 1.25v8.5c0 .69-.56 1.25-1.25 1.25h-8.5c-.69 0-1.25-.56-1.25-1.25zm2 .25a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5h1a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5zm0 3a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5h1a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5zm0 3a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5h1a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5zm3-6a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5h1a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5zm0 3a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5h1a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5zm0 3a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5h1a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5zm3-6a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5h1a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5zm0 3a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5h1a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5zm0 3a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5h1a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5z",
        )
    }
    val lock by lazy {
        glyph(
            "lock",
            "M8 2.5A2.5 2.5 0 0 0 5.5 5h5A2.5 2.5 0 0 0 8 2.5m4 2.604V5a4 4 0 0 0-8 0v.104A2.75 2.75 0 0 0 2 7.75v4.5A2.75 2.75 0 0 0 4.75 15h6.5A2.75 2.75 0 0 0 14 12.25v-4.5a2.75 2.75 0 0 0-2-2.646M4.75 6.5c-.69 0-1.25.56-1.25 1.25v4.5c0 .69.56 1.25 1.25 1.25h6.5c.69 0 1.25-.56 1.25-1.25v-4.5c0-.69-.56-1.25-1.25-1.25z",
        )
    }
    val power by lazy {
        glyph(
            "power",
            "M8 .95a.75.75 0 0 1 .75.75v5.55a.75.75 0 0 1-1.5 0V1.7A.75.75 0 0 1 8 .95m-3.89 2.1a.75.75 0 0 1 0 1.06 5.5 5.5 0 1 0 7.78 0 .75.75 0 1 1 1.06-1.06 7 7 0 1 1-9.9 0 .75.75 0 0 1 1.06 0",
        )
    }
    val camera by lazy {
        glyph(
            "camera",
            "M6.368 3.5a.25.25 0 0 0-.224.138l-.447.895a1.75 1.75 0 0 1-1.565.967H3.75c-.69 0-1.25.56-1.25 1.25v4.5c0 .69.56 1.25 1.25 1.25h8.5c.69 0 1.25-.56 1.25-1.25v-4.5c0-.69-.56-1.25-1.25-1.25h-.382a1.75 1.75 0 0 1-1.565-.967l-.447-.895a.25.25 0 0 0-.224-.138zm-1.565-.533A1.75 1.75 0 0 1 6.368 2h3.264a1.75 1.75 0 0 1 1.565.967l.447.895-.67.335.67-.335a.25.25 0 0 0 .224.138h.382A2.75 2.75 0 0 1 15 6.75v4.5A2.75 2.75 0 0 1 12.25 14h-8.5A2.75 2.75 0 0 1 1 11.25v-4.5A2.75 2.75 0 0 1 3.75 4h.382a.25.25 0 0 0 .224-.138zl.67.336zM8 7a1.5 1.5 0 1 0 0 3 1.5 1.5 0 0 0 0-3M5 8.5a3 3 0 1 1 6 0 3 3 0 0 1-6 0",
        )
    }
    val mic by lazy {
        glyph(
            "mic",
            "M8 1a4 4 0 0 0-4 4v2a4 4 0 1 0 8 0V5a4 4 0 0 0-4-4M5.5 5a2.5 2.5 0 0 1 5 0v2a2.5 2.5 0 0 1-5 0zm-1.18 5.55a.75.75 0 0 0-1.14.974l.565-.482-.565.482.001.002.003.003.006.007.021.024q.025.029.071.077a7.3 7.3 0 0 0 1.208.997c.675.443 1.615.9 2.76 1.045v.571a.75.75 0 1 0 1.5 0v-.571c1.146-.146 2.085-.602 2.76-1.046.41-.27.731-.539.952-.743a6 6 0 0 0 .327-.33l.02-.024.008-.007.002-.003v-.001h.001l-.57-.488.57.487a.75.75 0 0 0-1.139-.975h-.001l-.007.009-.043.046q-.06.066-.19.187a6 6 0 0 1-.753.589c-.659.432-1.58.846-2.687.846-1.108 0-2.028-.414-2.687-.846a5.8 5.8 0 0 1-.943-.776l-.043-.046zm0 0-.57.487z",
        )
    }
    val micOff by lazy {
        glyph(
            "micOff",
            "M3.324 1.268a.75.75 0 0 0-1.149.964l2.09 2.487 4.728 5.63 3.682 4.383a.75.75 0 1 0 1.15-.964l-3.172-3.775A4 4 0 0 0 12 7V5a4 4 0 0 0-7.362-2.168zM10.5 7c0 .73-.312 1.386-.812 1.844L5.678 4.07A2.501 2.501 0 0 1 10.5 5zm-6.18 3.55a.75.75 0 0 0-1.14.974l.57-.487-.57.487v.002l.003.003.007.007.02.024.072.077a7.3 7.3 0 0 0 1.208.997c.675.443 1.614.9 2.76 1.045v.571a.75.75 0 0 0 1.5 0v-1.274a.75.75 0 0 0-.75-.75c-1.108 0-2.029-.414-2.687-.846a5.8 5.8 0 0 1-.943-.776l-.043-.046-.008-.009zm0 0-.57.487z",
        )
    }
    val clock by lazy {
        glyph(
            "clock",
            "M8 2.5a5.5 5.5 0 1 0 0 11 5.5 5.5 0 0 0 0-11M1 8a7 7 0 1 1 14 0A7 7 0 0 1 1 8m7-4a.75.75 0 0 1 .75.75v2.836c0 .066.026.13.073.177L10.78 9.72a.75.75 0 1 1-1.06 1.06L7.763 8.823a1.75 1.75 0 0 1-.513-1.237V4.75A.75.75 0 0 1 8 4",
        )
    }
    val bell by lazy {
        glyph(
            "bell",
            "M2 7a6 6 0 1 1 12 0v.887l.967 3.142a.75.75 0 0 1-.717.971H11q-.001.343-.074.666A3 3 0 0 1 5 12H1.75a.75.75 0 0 1-.717-.97L2 7.886zm4.5 5q0 .174.037.334A1.5 1.5 0 0 0 9.5 12zM8 2.5A4.5 4.5 0 0 0 3.5 7v1a.8.8 0 0 1-.033.22l-.702 2.28h10.47l-.702-2.28A.8.8 0 0 1 12.5 8V7A4.5 4.5 0 0 0 8 2.5",
        )
    }
    val message by lazy {
        glyph(
            "message",
            "M5.75 2.5A3.25 3.25 0 0 0 2.5 5.75v2.5a3.25 3.25 0 0 0 2.437 3.148.75.75 0 0 1 .563.726v.709l1.77-1.203a.75.75 0 0 1 .421-.13h2.559a3.25 3.25 0 0 0 3.25-3.25v-2.5a3.25 3.25 0 0 0-3.25-3.25zM1 5.75A4.75 4.75 0 0 1 5.75 1h4.5A4.75 4.75 0 0 1 15 5.75v2.5A4.75 4.75 0 0 1 10.25 13H7.922l-2.75 1.87A.75.75 0 0 1 4 14.25v-1.583A4.75 4.75 0 0 1 1 8.25z",
        )
    }
    val phone by lazy {
        glyph(
            "phone",
            "M2.5 3.75A2.75 2.75 0 0 1 5.25 1h5.5a2.75 2.75 0 0 1 2.75 2.75v8.5A2.75 2.75 0 0 1 10.75 15h-5.5a2.75 2.75 0 0 1-2.75-2.75zM5.25 2.5C4.56 2.5 4 3.06 4 3.75v8.5c0 .69.56 1.25 1.25 1.25h5.5c.69 0 1.25-.56 1.25-1.25v-8.5c0-.69-.56-1.25-1.25-1.25zM7 4.25a.75.75 0 0 1 .75-.75h.5a.75.75 0 0 1 0 1.5h-.5A.75.75 0 0 1 7 4.25",
        )
    }
    val call by lazy {
        glyph(
            "call",
            "M2.632 2.562a.13.13 0 0 0-.033.111c.199 1.392.767 3.983 2.342 6.278 1.553 2.262 4.097 4.26 8.353 4.52.091.006.206-.072.206-.221v-2.163l-1.619-1.44-1.256.63A2.75 2.75 0 0 1 7.45 9.76L6.24 8.55a2.75 2.75 0 0 1-.515-3.175l.628-1.256L4.913 2.5H2.791c-.076 0-.13.03-.159.062m-1.518.323C.964 1.825 1.814 1 2.791 1H5.25a.75.75 0 0 1 .56.252l2 2.25a.75.75 0 0 1 .11.833l-.855 1.711A1.25 1.25 0 0 0 7.3 7.49L8.51 8.7c.381.38.963.475 1.444.235l1.71-.856a.75.75 0 0 1 .834.11l2.25 2a.75.75 0 0 1 .252.561v2.5c0 .956-.787 1.78-1.798 1.719-4.769-.292-7.714-2.571-9.497-5.17-1.762-2.566-2.377-5.418-2.59-6.914",
        )
    }
    val photo by lazy {
        glyph(
            "photo",
            "M3.75 2.5c-.69 0-1.25.56-1.25 1.25v6.433l.981-1.174a1.775 1.775 0 0 1 2.771.048.985.985 0 0 0 1.521.039l2.148-2.479a1.773 1.773 0 0 1 2.428-.248l1.151.886V3.75c0-.69-.56-1.25-1.25-1.25zm9.75 6.647-2.065-1.589a.273.273 0 0 0-.38.042l-2.148 2.478a2.485 2.485 0 0 1-3.838-.099.275.275 0 0 0-.437-.008l-2.108 2.523A1.25 1.25 0 0 0 3.75 13.5h8.5c.69 0 1.25-.56 1.25-1.25zM1 3.75A2.75 2.75 0 0 1 3.75 1h8.5A2.75 2.75 0 0 1 15 3.75v8.5A2.75 2.75 0 0 1 12.25 15h-8.5A2.75 2.75 0 0 1 1 12.25z",
            "M3.75 4.75a1 1 0 1 1 2 0 1 1 0 0 1-2 0",
        )
    }
    val folder by lazy {
        glyph(
            "folder",
            "M3.75 2.5c-.69 0-1.25.56-1.25 1.25V4h5.472l-.939-1.073a1.25 1.25 0 0 0-.94-.427zM10.079 4h3.171c.966 0 1.75.784 1.75 1.75v6.5A2.75 2.75 0 0 1 12.25 15h-8.5A2.75 2.75 0 0 1 1 12.25v-8.5A2.75 2.75 0 0 1 3.75 1h2.342a2.75 2.75 0 0 1 2.07.94l-.564.493.564-.494 1.729 1.976a.25.25 0 0 0 .188.085M2.5 5.5v6.75c0 .69.56 1.25 1.25 1.25h8.5c.69 0 1.25-.56 1.25-1.25v-6.5a.25.25 0 0 0-.25-.25z",
        )
    }
    val monitor by lazy {
        glyph(
            "monitor",
            "M1 3.75A2.75 2.75 0 0 1 3.75 1h8.5A2.75 2.75 0 0 1 15 3.75v5a2.75 2.75 0 0 1-2.75 2.75h-2v2.039a.75.75 0 0 1-.25 1.457H6a.75.75 0 0 1-.25-1.457V11.5h-2A2.75 2.75 0 0 1 1 8.75zM6.5 10h5.75c.69 0 1.25-.56 1.25-1.25v-5c0-.69-.56-1.25-1.25-1.25h-8.5c-.69 0-1.25.56-1.25 1.25v5c0 .69.56 1.25 1.25 1.25zm.75 1.5v1.996h1.5V11.5z",
        )
    }
    val plus by lazy {
        glyph(
            "plus",
            "M8 3a.75.75 0 0 1 .75.75v3.5h3.5a.75.75 0 0 1 0 1.5h-3.5v3.5a.75.75 0 0 1-1.5 0v-3.5h-3.5a.75.75 0 0 1 0-1.5h3.5v-3.5A.75.75 0 0 1 8 3",
        )
    }
    val bolt by lazy {
        glyph(
            "bolt",
            "M9.537 1.057A.75.75 0 0 1 10 1.75V7h3.25a.75.75 0 0 1 .53 1.28l-6.5 6.5A.75.75 0 0 1 6 14.25V9H2.75a.75.75 0 0 1-.53-1.28l6.5-6.5a.75.75 0 0 1 .817-.163M4.561 7.5H6.75a.75.75 0 0 1 .75.75v4.19l3.94-3.94H9.25a.75.75 0 0 1-.75-.75V3.56z",
        )
    }
    val wifi by lazy {
        glyph(
            "wifi",
            "m2.306 6.753.013-.014.062-.062c.058-.057.149-.143.271-.248a7.7 7.7 0 0 1 1.103-.783A8.3 8.3 0 0 1 8 4.5a8.33 8.33 0 0 1 4.245 1.146 7.7 7.7 0 0 1 1.103.783 5 5 0 0 1 .333.31l.012.013.001.002a.75.75 0 0 0 1.113-1.006l-.557.502.557-.502-.002-.002-.003-.003-.008-.01a3 3 0 0 0-.122-.124 7 7 0 0 0-.348-.32 9 9 0 0 0-1.319-.935A9.8 9.8 0 0 0 8 3a9.8 9.8 0 0 0-5.005 1.354 9 9 0 0 0-1.319.936 7 7 0 0 0-.47.443l-.008.01-.003.003-.001.001h-.001l.557.503-.557-.502a.75.75 0 0 0 1.113 1.006m3.018 2.98a.75.75 0 0 1-1.15-.963l.576.48-.576-.48V8.77l.002-.001.002-.003.006-.007.017-.02.057-.063a5.7 5.7 0 0 1 .951-.8c.643-.428 1.6-.875 2.791-.875 1.192 0 2.148.447 2.791.876a5.7 5.7 0 0 1 .951.8l.057.062.018.02.005.007.003.003v.001l-.575.481.576-.48a.75.75 0 0 1-1.15.963l-.004-.005-.03-.032a4 4 0 0 0-.684-.572c-.48-.321-1.15-.624-1.958-.624s-1.477.303-1.959.624a4 4 0 0 0-.684.572l-.029.032zM7 12a1 1 0 1 1 2 0 1 1 0 0 1-2 0",
        )
    }
    val signal by lazy {
        glyph(
            "signal",
            "M13 2.75a.75.75 0 0 1 .75-.75h.5a.75.75 0 0 1 .75.75v10.5a.75.75 0 0 1-.75.75h-.5a.75.75 0 0 1-.75-.75zm-4 2A.75.75 0 0 1 9.75 4h.5a.75.75 0 0 1 .75.75v8.5a.75.75 0 0 1-.75.75h-.5a.75.75 0 0 1-.75-.75zm-4 3A.75.75 0 0 1 5.75 7h.5a.75.75 0 0 1 .75.75v5.5a.75.75 0 0 1-.75.75h-.5a.75.75 0 0 1-.75-.75zm-4 2A.75.75 0 0 1 1.75 9h.5a.75.75 0 0 1 .75.75v3.5a.75.75 0 0 1-.75.75h-.5a.75.75 0 0 1-.75-.75z",
        )
    }
    val moon by lazy {
        glyph(
            "moon",
            "M8.517 1.406a.75.75 0 0 1-.062.791A3.86 3.86 0 0 0 7.693 4.5c0 2.183 1.846 4 4.183 4a4.3 4.3 0 0 0 2.023-.498.75.75 0 0 1 1.097.742A7.01 7.01 0 0 1 8.018 15C4.144 15 1 11.868 1 8c0-3.806 3.044-6.9 6.832-6.998a.75.75 0 0 1 .685.404M6.5 2.71c-2.31.657-4 2.778-4 5.29 0 3.036 2.468 5.5 5.518 5.5a5.52 5.52 0 0 0 5.199-3.654c-.43.1-.88.154-1.341.154-3.112 0-5.683-2.436-5.683-5.5 0-.627.108-1.229.308-1.79",
        )
    }
    val info by lazy {
        glyph(
            "info",
            "M8 2.5a5.5 5.5 0 1 0 0 11 5.5 5.5 0 0 0 0-11M1 8a7 7 0 1 1 14 0A7 7 0 0 1 1 8m7 4a1 1 0 0 0 1-1V8a1 1 0 0 0-2 0v3a1 1 0 0 0 1 1m0-6a1 1 0 1 1 0-2 1 1 0 0 1 0 2",
        )
    }
    /** G7 Linux's question-mark-circle. */
    val help by lazy {
        glyph(
            "help",
            "M2.5 8a5.5 5.5 0 1 1 11 0 5.5 5.5 0 0 1-11 0M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1m-.75 5.25a1 1 0 0 1 2 0v.125c0 .621-.504 1.125-1.125 1.125H8a.75.75 0 0 0-.75.75V9a.75.75 0 0 0 1.5 0v-.075c1.148-.28 2-1.316 2-2.55V6.25a2.5 2.5 0 0 0-5 0 .75.75 0 0 0 1.5 0m.75 4a1 1 0 1 0 0 2 1 1 0 0 0 0-2",
        )
    }
    val keyboard by lazy {
        glyph(
            "keyboard",
            "M8 1a.75.75 0 0 1 .75.75V4h3.5A2.75 2.75 0 0 1 15 6.75v5.5A2.75 2.75 0 0 1 12.25 15h-8.5A2.75 2.75 0 0 1 1 12.25v-5.5A2.75 2.75 0 0 1 3.75 4h3.5V1.75A.75.75 0 0 1 8 1M3.75 5.5c-.69 0-1.25.56-1.25 1.25v5.5c0 .69.56 1.25 1.25 1.25h8.5c.69 0 1.25-.56 1.25-1.25v-5.5c0-.69-.56-1.25-1.25-1.25zM4 8.25a.75.75 0 0 1 .75-.75h.5a.75.75 0 0 1 0 1.5h-.5A.75.75 0 0 1 4 8.25m3 0a.75.75 0 0 1 .75-.75h.5a.75.75 0 0 1 0 1.5h-.5A.75.75 0 0 1 7 8.25m3 0a.75.75 0 0 1 .75-.75h.5a.75.75 0 0 1 0 1.5h-.5a.75.75 0 0 1-.75-.75m-5 3a.75.75 0 0 1 .75-.75h4.5a.75.75 0 0 1 0 1.5h-4.5a.75.75 0 0 1-.75-.75",
        )
    }
    val volume by lazy {
        glyph(
            "volume",
            "M9.557 1.066A.75.75 0 0 1 10 1.75v12.5a.75.75 0 0 1-1.248.56L4.82 11.317A1.25 1.25 0 0 0 3.99 11H2.75A1.75 1.75 0 0 1 1 9.25v-2.5C1 5.784 1.784 5 2.75 5h1.24c.306 0 .601-.112.83-.316L8.752 1.19a.75.75 0 0 1 .805-.123M8.5 3.42 5.817 5.805A2.75 2.75 0 0 1 3.99 6.5H2.75a.25.25 0 0 0-.25.25v2.5c0 .138.112.25.25.25h1.24a2.75 2.75 0 0 1 1.827.695L8.5 12.58zm3.705-.185a.75.75 0 0 1 1.06-.03l-.515.545.515-.545h.001l.001.002.003.003.008.007.021.021.066.069q.081.086.207.242c.165.209.38.513.59.913C14.588 5.264 15 6.446 15 8s-.412 2.736-.837 3.538a6 6 0 0 1-.591.913 4 4 0 0 1-.273.31l-.021.022-.008.007-.003.003v.001h-.002l-.515-.544.515.545a.75.75 0 0 1-1.034-1.087l.004-.003.03-.031q.043-.046.132-.156c.116-.146.277-.373.44-.681.325-.615.663-1.558.663-2.837s-.338-2.222-.663-2.837a4.4 4.4 0 0 0-.44-.681 3 3 0 0 0-.166-.19.75.75 0 0 1-.026-1.057",
        )
    }
    val volumeOff by lazy {
        glyph(
            "volumeOff",
            "M2.268 1.176a.75.75 0 0 1 1.056.092L6.46 5h1.53c.306 0 .601-.112.83-.316l3.932-3.495A.75.75 0 0 1 14 1.75v12.5a.75.75 0 0 1-1.324.482l-7.14-8.5-3.36-4a.75.75 0 0 1 .092-1.056M7.72 6.5l4.78 5.691v-8.77L9.817 5.804A2.75 2.75 0 0 1 7.99 6.5zM2.75 5a.75.75 0 0 1 .75.75v3.5c0 .138.112.25.25.25h2.5a.75.75 0 1 1 0 1.5h-2.5A1.75 1.75 0 0 1 2 9.25v-3.5A.75.75 0 0 1 2.75 5",
        )
    }
    val play by lazy {
        glyph(
            "play",
            "M2.5 8a5.5 5.5 0 1 1 11 0 5.5 5.5 0 0 1-11 0M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1m-.844 4.12A.75.75 0 0 0 6 5.75v4.5a.75.75 0 0 0 1.156.63l3.5-2.25a.75.75 0 0 0 0-1.26z",
        )
    }
    val pause by lazy {
        glyph(
            "pause",
            "M8 2.5a5.5 5.5 0 1 0 0 11 5.5 5.5 0 0 0 0-11M1 8a7 7 0 1 1 14 0A7 7 0 0 1 1 8m5.25-3a.75.75 0 0 1 .75.75v4.5a.75.75 0 0 1-1.5 0v-4.5A.75.75 0 0 1 6.25 5m3.5 0a.75.75 0 0 1 .75.75v4.5a.75.75 0 0 1-1.5 0v-4.5A.75.75 0 0 1 9.75 5",
        )
    }
    val next by lazy {
        glyph(
            "next",
            "M1.343 3.12a.75.75 0 0 1 .715-.054L6 4.84V3.75a.75.75 0 0 1 1.12-.653l7.5 4.25a.75.75 0 0 1 0 1.306l-7.5 4.25A.75.75 0 0 1 6 12.25v-1.09l-3.942 1.774A.75.75 0 0 1 1 12.25v-8.5a.75.75 0 0 1 .343-.63M6 9.515v-3.03L2.5 4.91v6.18zm1.5-4.478v5.926L12.729 8z",
        )
    }
    val previous by lazy {
        glyph(
            "previous",
            "M14.657 3.12a.75.75 0 0 1 .343.63v8.5a.75.75 0 0 1-1.058.684L10 11.16v1.09a.75.75 0 0 1-1.12.652l-7.5-4.25a.75.75 0 0 1 0-1.305l7.5-4.25A.75.75 0 0 1 10 3.75v1.09l3.942-1.774a.75.75 0 0 1 .715.054M10 6.485v3.03l3.5 1.575V4.91zM3.271 8 8.5 10.963V5.037z",
        )
    }
    val music by lazy {
        glyph(
            "music",
            "M12.826 1.333A1.75 1.75 0 0 1 15 3.031V10.5a2.5 2.5 0 1 1-1.5-2.292V3.031a.25.25 0 0 0-.31-.243l-7 1.75a.25.25 0 0 0-.19.243V12.5a2.5 2.5 0 1 1-1.5-2.292V4.781a1.75 1.75 0 0 1 1.326-1.698zM4.5 12.5a1 1 0 1 0-2 0 1 1 0 0 0 2 0m9-2a1 1 0 1 0-2 0 1 1 0 0 0 2 0",
        )
    }
    val close by lazy {
        glyph(
            "close",
            "M3.084 11.888a.734.734 0 0 0 .006 1.022.74.74 0 0 0 1.022.006l3.885-3.89 3.891 3.89a.734.734 0 0 0 1.022-.006c.273-.286.28-.75 0-1.022L9.025 7.997l3.885-3.885a.73.73 0 0 0 0-1.022c-.285-.273-.749-.28-1.022-.006l-3.89 3.89-3.886-3.89c-.266-.267-.749-.28-1.022.006a.74.74 0 0 0-.006 1.022l3.891 3.885z",
            evenOdd = false,
        )
    }
    val back by lazy {
        glyph(
            "back",
            "M6.8 4.24a.75.75 0 0 1-.04 1.06l-2.1 1.95h8.59a.75.75 0 0 1 0 1.5H4.66l2.1 1.95a.75.75 0 1 1-1.02 1.1l-3.5-3.25a.75.75 0 0 1 0-1.1l3.5-3.25a.75.75 0 0 1 1.06.04",
        )
    }
    /** G7 Linux's chevron-up-down: the touchpad scrolling instead of pointing. */
    val scroll by lazy {
        glyph(
            "scroll",
            "M7.47 2.47a.75.75 0 0 1 1.06 0l3 3a.75.75 0 0 1-1.06 1.06L8 4.06 5.53 6.53a.75.75 0 0 1-1.06-1.06zm-3 7a.75.75 0 0 1 1.06 0L8 11.94l2.47-2.47a.75.75 0 1 1 1.06 1.06l-3 3a.75.75 0 0 1-1.06 0l-3-3a.75.75 0 0 1 0-1.06",
        )
    }
    val chevronDown by lazy {
        glyph(
            "chevronDown",
            "M3.235 5.205a.75.75 0 0 1 1.06.03L8 9.158l3.705-3.923a.75.75 0 0 1 1.09 1.03l-4.25 4.5a.75.75 0 0 1-1.09 0l-4.25-4.5a.75.75 0 0 1 .03-1.06",
        )
    }
    val chevronRight by lazy {
        glyph(
            "chevronRight",
            "M5.205 3.235a.75.75 0 0 1 1.06-.03l4.5 4.25a.75.75 0 0 1 0 1.09l-4.5 4.25a.75.75 0 0 1-1.03-1.09L9.158 8 5.235 4.295a.75.75 0 0 1-.03-1.06",
        )
    }
    val search by lazy {
        glyph(
            "search",
            "M6.5 2.5a4 4 0 1 0 0 8 4 4 0 0 0 0-8M1 6.5a5.5 5.5 0 1 1 9.883 3.323l3.897 3.897a.75.75 0 1 1-1.06 1.06l-3.897-3.897A5.5 5.5 0 0 1 1 6.5",
        )
    }
    val refresh by lazy {
        glyph(
            "refresh",
            "M1 8.875a5.875 5.875 0 0 1 11.75 0v1.42l.943-1.047a.75.75 0 0 1 1.114 1.004l-2.25 2.5a.75.75 0 0 1-1.114 0l-2.25-2.5a.75.75 0 1 1 1.114-1.004l.943 1.048V8.875a4.375 4.375 0 1 0-8.75 0v3.375a.75.75 0 0 1-1.5 0z",
        )
    }
    val stop by lazy {
        glyph(
            "stop",
            "M2.5 8a5.5 5.5 0 1 1 11 0 5.5 5.5 0 0 1-11 0M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1M6.75 6a.75.75 0 0 0-.75.75v2.5c0 .414.336.75.75.75h2.5a.75.75 0 0 0 .75-.75v-2.5A.75.75 0 0 0 9.25 6z",
        )
    }
    val screen by lazy {
        glyph(
            "screen",
            "M4 2a2 2 0 0 0-2 2v6.5h-.25a.75.75 0 0 0 0 1.5h4.5a.75.75 0 0 0 0-1.5H3.5V4a.5.5 0 0 1 .5-.5h8c.163 0 .308.077.4.2a.75.75 0 1 0 1.2-.9A2 2 0 0 0 12 2zm6 3a2 2 0 0 0-2 2v5a2 2 0 0 0 2 2h3a2 2 0 0 0 2-2V7a2 2 0 0 0-2-2zm-.5 2a.5.5 0 0 1 .5-.5h3a.5.5 0 0 1 .5.5v5a.5.5 0 0 1-.5.5h-3a.5.5 0 0 1-.5-.5zm1.75 0a.75.75 0 0 0 0 1.5h.5a.75.75 0 0 0 0-1.5z",
        )
    }
    val speaker by lazy {
        glyph(
            "speaker",
            "M4.75 2.5c-.69 0-1.25.56-1.25 1.25v8.5c0 .69.56 1.25 1.25 1.25h6.5c.69 0 1.25-.56 1.25-1.25v-8.5c0-.69-.56-1.25-1.25-1.25zM2 3.75A2.75 2.75 0 0 1 4.75 1h6.5A2.75 2.75 0 0 1 14 3.75v8.5A2.75 2.75 0 0 1 11.25 15h-6.5A2.75 2.75 0 0 1 2 12.25zM8 8.5a1 1 0 1 0 0 2 1 1 0 0 0 0-2m-2.5 1a2.5 2.5 0 1 1 5 0 2.5 2.5 0 0 1-5 0m2.5-4A.75.75 0 1 0 8 4a.75.75 0 0 0 0 1.5",
        )
    }
    val bluetooth by lazy {
        glyph(
            "bluetooth",
            "M7.402 1.08a.75.75 0 0 1 .785.07l3.918 2.925a.75.75 0 0 1 .042 1.168L8.96 8l3.186 2.757a.75.75 0 0 1-.042 1.168l-3.918 2.926a.75.75 0 0 1-1.199-.601V9.708l-2.153 1.864a.75.75 0 1 1-.982-1.135L6.67 8 3.853 5.563a.75.75 0 0 1 .982-1.134l2.153 1.863V1.75a.75.75 0 0 1 .414-.67m1.086 8.494 1.972 1.707-1.972 1.473zm0-3.148v-3.18L10.46 4.72z",
        )
    }
    val shield by lazy {
        glyph(
            "shield",
            "M1.75 3.75 1 3.804v.004l.002.012.003.04.013.148a27 27 0 0 0 .348 2.384c.306 1.582.86 3.678 1.852 5.397C4.209 13.506 5.734 15 7.999 15c2.267 0 3.792-1.494 4.782-3.21.991-1.72 1.545-3.816 1.85-5.398a27 27 0 0 0 .35-2.384l.012-.148.003-.04.001-.012v-.004l-.748-.054.749.053a.75.75 0 0 0-.355-.692l-.394.639.393-.639h-.001l-.003-.002-.009-.006-.032-.02-.118-.068q-.153-.09-.431-.24a19 19 0 0 0-1.507-.716C11.318 1.544 9.647 1 8 1s-3.318.544-4.541 1.059a19 19 0 0 0-1.939.955l-.118.07-.032.019-.009.005-.003.002-.001.001zm.793.411.024.208c.036.3.089.687.163 1.131h4.52V2.543c-1.125.125-2.268.502-3.21.898a17 17 0 0 0-1.497.72M8.75 2.543V5.5h4.519a25 25 0 0 0 .188-1.339l-.115-.062a17 17 0 0 0-1.383-.658c-.942-.396-2.085-.773-3.21-.898M12.97 7H8.75v6.414c1.125-.265 2.015-1.131 2.731-2.374.705-1.222 1.179-2.71 1.489-4.04m-5.72 6.414V7H3.03c.31 1.331.783 2.817 1.488 4.04.717 1.243 1.607 2.109 2.732 2.374M1.75 3.75l-.393-.639A.75.75 0 0 0 1 3.803z",
        )
    }
    val battery by lazy {
        glyph(
            "battery",
            "M1 5.75A2.75 2.75 0 0 1 3.75 3h6.5A2.75 2.75 0 0 1 13 5.75V6a2 2 0 1 1 0 4v.25A2.75 2.75 0 0 1 10.25 13h-6.5A2.75 2.75 0 0 1 1 10.25zM13 8.5a.5.5 0 0 0 0-1zm-9.25-4c-.69 0-1.25.56-1.25 1.25v4.5c0 .69.56 1.25 1.25 1.25h6.5c.69 0 1.25-.56 1.25-1.25v-4.5c0-.69-.56-1.25-1.25-1.25z",
        )
    }
    val history by lazy {
        glyph(
            "history",
            "M8 2.5a5.5 5.5 0 1 0 0 11 5.5 5.5 0 0 0 0-11M1 8a7 7 0 1 1 14 0A7 7 0 0 1 1 8m7-4a.75.75 0 0 1 .75.75v2.836c0 .066.026.13.073.177L10.78 9.72a.75.75 0 1 1-1.06 1.06L7.763 8.823a1.75 1.75 0 0 1-.513-1.237V4.75A.75.75 0 0 1 8 4",
        )
    }
    val globe by lazy {
        glyph(
            "globe",
            "M2.55 7.25h1.73a10.05 10.05 0 0 1 1.27-4.175A5.5 5.5 0 0 0 2.551 7.25M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1m0 1.507c-.038.017-.163.075-.37.274-.262.253-.563.646-.849 1.162a8.55 8.55 0 0 0-.997 3.307h4.432a8.55 8.55 0 0 0-.997-3.307c-.286-.516-.587-.91-.849-1.162-.207-.2-.332-.257-.37-.274m3.721 4.743a10.05 10.05 0 0 0-1.27-4.175 5.5 5.5 0 0 1 2.998 4.175zm-1.505 1.5H5.784a8.55 8.55 0 0 0 .997 3.307c.286.516.587.91.849 1.162.207.2.332.257.37.274.038-.017.163-.075.37-.274.262-.253.563-.646.849-1.162a8.6 8.6 0 0 0 .997-3.307m-4.667 4.175-.08-.143a10.05 10.05 0 0 1-1.19-4.032H2.55a5.5 5.5 0 0 0 2.998 4.175M11.72 8.75h1.728a5.5 5.5 0 0 1-2.998 4.175l.08-.143a10.05 10.05 0 0 0 1.19-4.032",
        )
    }
    val palette by lazy {
        glyph(
            "palette",
            "M1 1.75A.75.75 0 0 1 1.75 1h4.5a.75.75 0 0 1 .75.75v10.5A2.75 2.75 0 0 1 4.25 15h-.5A2.75 2.75 0 0 1 1 12.25zm1.5.75V5h3V2.5zm3 4h-3V9h3zm0 4h-3v1.75c0 .69.56 1.25 1.25 1.25h.5c.69 0 1.25-.56 1.25-1.25zm5.22-9.28a.75.75 0 0 1 1.06 0l3 3a.75.75 0 0 1 0 1.06l-5.5 5.5a.75.75 0 1 1-1.06-1.06l4.97-4.97-1.94-1.94-1.97 1.97a.75.75 0 0 1-1.06-1.06z",
        )
    }
    val check by lazy {
        glyph(
            "check",
            "M12.707 3.293a1 1 0 0 1 0 1.414c-2.512 2.512-4.325 4.347-5.245 7.568a1 1 0 0 1-1.81.255l-2.5-4a1 1 0 0 1 1.696-1.06l1.418 2.268c1.18-2.598 2.975-4.393 4.969-6.387l.058-.058a1 1 0 0 1 1.414 0",
        )
    }
    val trash by lazy {
        glyph(
            "trash",
            "M7.362 3.005h1.302a1 1 0 0 0-.116-.266c-.076-.114-.198-.226-.548-.226-.26 0-.406.094-.513.245a1 1 0 0 0-.125.247m2.834 0a2.7 2.7 0 0 0-.398-1.095c-.392-.592-1.02-.897-1.798-.897-.74 0-1.344.323-1.737.878-.23.326-.373.709-.448 1.114H2.838a1 1 0 0 0-.175 0H1.75a.75.75 0 0 0 0 1.5h.335l.96 8.07A2.75 2.75 0 0 0 5.776 15h4.448a2.75 2.75 0 0 0 2.73-2.425l.961-8.07h.335a.75.75 0 0 0 0-1.5h-.913a1 1 0 0 0-.175 0zm2.209 1.5h-8.81l.94 7.893A1.25 1.25 0 0 0 5.776 13.5h4.448a1.25 1.25 0 0 0 1.241-1.102zM6.5 6.25a.75.75 0 0 1 .75.75v4a.75.75 0 0 1-1.5 0V7a.75.75 0 0 1 .75-.75m3 0a.75.75 0 0 1 .75.75v4a.75.75 0 0 1-1.5 0V7a.75.75 0 0 1 .75-.75",
        )
    }
    val send by lazy {
        glyph(
            "send",
            "M8 1a.75.75 0 0 1 .55.24l3.25 3.5a.75.75 0 0 1-1.1 1.02l-1.95-2.1v6.59a.75.75 0 0 1-1.5 0V3.66L5.3 5.76a.75.75 0 1 1-1.1-1.02l3.25-3.5A.75.75 0 0 1 8 1M1.75 11a.75.75 0 0 1 .75.75v.5c0 .69.56 1.25 1.25 1.25h8.5c.69 0 1.25-.56 1.25-1.25v-.5a.75.75 0 0 1 1.5 0v.5A2.75 2.75 0 0 1 12.25 15h-8.5A2.75 2.75 0 0 1 1 12.25v-.5a.75.75 0 0 1 .75-.75",
        )
    }
    /** G7 Linux's arrow-up (src/server/icons): sends what's typed. */
    val arrowUp by lazy {
        glyph(
            "arrowUp",
            "M8 2a.75.75 0 0 1 .55.24l3.25 3.5a.75.75 0 1 1-1.1 1.02l-1.95-2.1v8.59a.75.75 0 0 1-1.5 0V4.66L5.3 6.76a.75.75 0 1 1-1.1-1.02l3.25-3.5A.75.75 0 0 1 8 2",
        )
    }
    /** G7 Linux's paperclip (src/server/icons): attaches a file. */
    val paperclip by lazy {
        glyph(
            "paperclip",
            "M12.713 3.25c-1.084-1-2.863-1-3.947 0L3.801 7.839c-1.735 1.603-1.735 4.176 0 5.78l-.509.55.51-.55.088.081a.75.75 0 0 1-1.018 1.102l-.089-.082c-2.377-2.197-2.377-5.786 0-7.983L7.748 2.15c1.658-1.532 4.324-1.532 5.983 0l-.498.539.498-.539c1.682 1.554 1.73 4.077.03 5.65l-4.748 4.475-.006.005c-.973.898-2.527.898-3.5 0l.509-.551-.51.55a2.286 2.286 0 0 1 0-3.395h.001l3.984-3.68a.75.75 0 1 1 1.018 1.102l-3.984 3.68a.787.787 0 0 0 0 1.192 1.11 1.11 0 0 0 1.462.002l4.749-4.475.005-.005c1.04-.961 1.03-2.47-.028-3.448",
        )
    }
    val person by lazy {
        glyph(
            "person",
            "M8 2.5A1.75 1.75 0 1 0 8 6a1.75 1.75 0 0 0 0-3.5M4.75 4.25a3.25 3.25 0 1 1 6.5 0 3.25 3.25 0 0 1-6.5 0M8 10c-2.034 0-3.771.948-4.44 2.58-.087.213-.046.402.11.576.173.194.479.344.83.344h7c.351 0 .657-.15.83-.344.156-.174.197-.363.11-.576C11.772 10.948 10.034 10 8 10m-5.828 2.012C3.135 9.662 5.544 8.5 8 8.5s4.865 1.161 5.828 3.512c.332.81.109 1.598-.38 2.144-.473.528-1.193.844-1.947.844H4.499c-.754 0-1.474-.316-1.947-.844-.489-.546-.712-1.334-.38-2.144",
        )
    }
    val sun by lazy {
        glyph(
            "sun",
            "M8 1a.75.75 0 0 1 .75.75v2.32a4 4 0 0 1 1.497.62l1.64-1.64a.75.75 0 1 1 1.061 1.06l-1.64 1.641c.303.444.518.952.622 1.499h2.32a.75.75 0 0 1 0 1.5h-2.32a4 4 0 0 1-.622 1.499l1.64 1.64a.75.75 0 0 1-1.06 1.06l-1.64-1.64a4 4 0 0 1-1.499.62v2.321a.75.75 0 0 1-1.5 0v-2.32a4 4 0 0 1-1.498-.622L4.11 12.95a.75.75 0 0 1-1.061-1.06l1.642-1.642a4 4 0 0 1-.62-1.498H1.75a.75.75 0 0 1 0-1.5h2.32a4 4 0 0 1 .62-1.498L3.05 4.111a.75.75 0 0 1 1.06-1.06l1.641 1.64a4 4 0 0 1 1.498-.62V1.75A.75.75 0 0 1 8 1m0 4.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5",
        )
    }
    val torch by lazy {
        glyph(
            "torch",
            "M13.5 4.56a4.1 4.1 0 0 0-2.017.941c-.579.497-1.389.999-2.341.999H2.75a.25.25 0 0 0-.25.25v2.5c0 .138.112.25.25.25h6.392c.952 0 1.762.502 2.34.999a4.1 4.1 0 0 0 2.018.94zm-2.995-.196C11.248 3.724 12.485 3 14.25 3a.75.75 0 0 1 .75.75v8.5a.75.75 0 0 1-.75.75c-1.764 0-3.002-.724-3.745-1.364-.468-.402-.937-.636-1.364-.636H2.75A1.75 1.75 0 0 1 1 9.25v-2.5C1 5.784 1.784 5 2.75 5h6.392c.426 0 .895-.234 1.363-.636M8 8a.75.75 0 0 1 .75-.75h.5a.75.75 0 0 1 0 1.5h-.5A.75.75 0 0 1 8 8",
        )
    }
    val ring by lazy {
        glyph(
            "ring",
            "M2.632 2.562a.13.13 0 0 0-.033.111c.199 1.392.767 3.983 2.342 6.278 1.553 2.262 4.097 4.26 8.353 4.52.091.006.206-.072.206-.221v-2.163l-1.619-1.44-1.256.63A2.75 2.75 0 0 1 7.45 9.76L6.24 8.55a2.75 2.75 0 0 1-.515-3.175l.628-1.256L4.913 2.5H2.791c-.076 0-.13.03-.159.062m-1.518.323C.964 1.825 1.814 1 2.791 1H5.25a.75.75 0 0 1 .56.252l2 2.25a.75.75 0 0 1 .11.833l-.855 1.711A1.25 1.25 0 0 0 7.3 7.49L8.51 8.7c.381.38.963.475 1.444.235l1.71-.856a.75.75 0 0 1 .834.11l2.25 2a.75.75 0 0 1 .252.561v2.5c0 .956-.787 1.78-1.798 1.719-4.769-.292-7.714-2.571-9.497-5.17-1.762-2.566-2.377-5.418-2.59-6.914M10 1.75a.75.75 0 0 1 .75-.75h.134A4.115 4.115 0 0 1 15 5.115v.135a.75.75 0 0 1-1.5 0v-.135A2.615 2.615 0 0 0 10.884 2.5h-.134a.75.75 0 0 1-.75-.75m-1 3A.75.75 0 0 1 9.75 4h.058C11.018 4 12 4.982 12 6.192v.058a.75.75 0 0 1-1.5 0v-.058a.69.69 0 0 0-.692-.692H9.75A.75.75 0 0 1 9 4.75",
        )
    }
    val volumeUp by lazy {
        glyph(
            "volumeUp",
            "M9.81 1.252a.75.75 0 0 1-.062 1.059L5.817 5.805A2.75 2.75 0 0 1 3.99 6.5H2.75a.25.25 0 0 0-.25.25v2.5c0 .138.112.25.25.25h1.24a2.75 2.75 0 0 1 1.827.695l3.931 3.495a.75.75 0 1 1-.996 1.12L4.82 11.317A1.25 1.25 0 0 0 3.99 11H2.75A1.75 1.75 0 0 1 1 9.25v-2.5C1 5.784 1.784 5 2.75 5h1.24c.306 0 .601-.112.83-.316L8.752 1.19a.75.75 0 0 1 1.059.063M11.977 4a.75.75 0 0 1 .558.248l2.25 2.5a.75.75 0 0 1-1.115 1.004l-.943-1.048v4.546a.75.75 0 0 1-1.5 0V6.704l-.942 1.048A.75.75 0 0 1 9.17 6.748l2.25-2.5A.75.75 0 0 1 11.976 4",
        )
    }
    val volumeDown by lazy {
        glyph(
            "volumeDown",
            "M9.81 1.252a.75.75 0 0 1-.062 1.059L5.817 5.805A2.75 2.75 0 0 1 3.99 6.5H2.75a.25.25 0 0 0-.25.25v2.5c0 .138.112.25.25.25h1.24a2.75 2.75 0 0 1 1.827.695l3.931 3.495a.75.75 0 1 1-.996 1.12L4.82 11.317A1.25 1.25 0 0 0 3.99 11H2.75A1.75 1.75 0 0 1 1 9.25v-2.5C1 5.784 1.784 5 2.75 5h1.24c.306 0 .601-.112.83-.316L8.752 1.19a.75.75 0 0 1 1.059.063M11.977 4a.75.75 0 0 1 .75.75v4.546l.943-1.048a.75.75 0 0 1 1.115 1.004l-2.25 2.5a.75.75 0 0 1-1.115 0l-2.25-2.5a.75.75 0 1 1 1.115-1.004l.942 1.048V4.75a.75.75 0 0 1 .75-.75",
        )
    }
    val desktop by lazy {
        glyph(
            "desktop",
            "M1 3.75A2.75 2.75 0 0 1 3.75 1h8.5A2.75 2.75 0 0 1 15 3.75v8.5A2.75 2.75 0 0 1 12.25 15h-8.5A2.75 2.75 0 0 1 1 12.25zM3.75 2.5c-.69 0-1.25.56-1.25 1.25v8.5c0 .69.56 1.25 1.25 1.25h8.5c.69 0 1.25-.56 1.25-1.25v-8.5c0-.69-.56-1.25-1.25-1.25zM4 10.75a.75.75 0 0 1 .75-.75h6.5a.75.75 0 0 1 0 1.5h-6.5a.75.75 0 0 1-.75-.75",
        )
    }
    val window by lazy {
        glyph(
            "window",
            "M1 3.75A2.75 2.75 0 0 1 3.75 1h8.5A2.75 2.75 0 0 1 15 3.75v8.5A2.75 2.75 0 0 1 12.25 15h-8.5A2.75 2.75 0 0 1 1 12.25zM3.75 2.5c-.69 0-1.25.56-1.25 1.25V4h11v-.25c0-.69-.56-1.25-1.25-1.25zm9.75 3h-11v6.75c0 .69.56 1.25 1.25 1.25h8.5c.69 0 1.25-.56 1.25-1.25z",
        )
    }
    val text by lazy {
        glyph(
            "text",
            "M2 2.75A.75.75 0 0 1 2.75 2h10.5a.75.75 0 0 1 .75.75v1.5a.75.75 0 0 1-1.5 0V3.5H8.75v9h.5a.75.75 0 0 1 0 1.5h-2.5a.75.75 0 0 1 0-1.5h.5v-9H3.5v.75a.75.75 0 0 1-1.5 0z",
        )
    }
    val apps by lazy {
        glyph(
            "apps",
            "M3.75 1A2.75 2.75 0 0 0 1 3.75v8.5A2.75 2.75 0 0 0 3.75 15h8.5A2.75 2.75 0 0 0 15 12.25v-8.5A2.75 2.75 0 0 0 12.25 1zM2.5 3.75c0-.69.56-1.25 1.25-1.25h8.5c.69 0 1.25.56 1.25 1.25v8.5c0 .69-.56 1.25-1.25 1.25h-8.5c-.69 0-1.25-.56-1.25-1.25zM4 5a1 1 0 0 1 1-1h1.5a1 1 0 0 1 1 1v1.5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1zm5.5-1a1 1 0 0 0-1 1v1.5a1 1 0 0 0 1 1H11a1 1 0 0 0 1-1V5a1 1 0 0 0-1-1zm0 4.5a1 1 0 0 0-1 1V11a1 1 0 0 0 1 1H11a1 1 0 0 0 1-1V9.5a1 1 0 0 0-1-1zM5 8.5a1 1 0 0 0-1 1V11a1 1 0 0 0 1 1h1.5a1 1 0 0 0 1-1V9.5a1 1 0 0 0-1-1z",
        )
    }
    val terminal by lazy {
        glyph(
            "terminal",
            "M1 3.75A2.75 2.75 0 0 1 3.75 1h8.5A2.75 2.75 0 0 1 15 3.75v8.5A2.75 2.75 0 0 1 12.25 15h-8.5A2.75 2.75 0 0 1 1 12.25zM3.75 2.5c-.69 0-1.25.56-1.25 1.25v8.5c0 .69.56 1.25 1.25 1.25h8.5c.69 0 1.25-.56 1.25-1.25v-8.5c0-.69-.56-1.25-1.25-1.25zm.443 3.748a.75.75 0 0 1 1.059-.055l2.5 2.25a.75.75 0 0 1 0 1.114l-2.5 2.25a.75.75 0 1 1-1.004-1.114L6.128 9l-1.88-1.693a.75.75 0 0 1-.055-1.059",
        )
    }
    val sparkle by lazy {
        glyph(
            "sparkle",
            "M4.704 1.745a.75.75 0 0 0-1.5 0v.005l-.003.043q-.004.065-.022.185c-.027.162-.08.366-.177.562a1.2 1.2 0 0 1-.413.484c-.18.12-.454.226-.885.226a.75.75 0 1 0 0 1.5c.431 0 .705.106.885.226.184.123.317.292.413.484a2 2 0 0 1 .202.79v.005a.75.75 0 0 0 1.5 0V6.25l.002-.043q.004-.065.023-.185c.027-.162.08-.366.177-.562.096-.192.229-.361.413-.484.18-.12.454-.226.885-.226a.75.75 0 0 0 0-1.5c-.431 0-.705-.106-.885-.226a1.2 1.2 0 0 1-.413-.484 2 2 0 0 1-.202-.79zm6 4.002v.026l.006.106c.007.096.021.239.05.412.058.35.173.803.396 1.249.221.442.541.861 1.007 1.172.461.307 1.11.538 2.04.538a.75.75 0 0 1 0 1.5c-.93 0-1.579.23-2.04.538-.466.31-.786.73-1.007 1.172-.223.446-.338.9-.396 1.25a4 4 0 0 0-.056.517v.023a.75.75 0 0 1-1.5.003v-.026l-.007-.106a5 5 0 0 0-.05-.412 4.5 4.5 0 0 0-.395-1.249 3 3 0 0 0-1.007-1.172c-.461-.307-1.11-.538-2.041-.538a.75.75 0 0 1 0-1.5c.931 0 1.58-.23 2.04-.538A2.97 2.97 0 0 0 8.752 7.54c.222-.446.337-.9.396-1.25a5 5 0 0 0 .055-.517V5.75m0 0v-.002a.75.75 0 0 1 1.5 0",
        )
    }
    val video by lazy {
        glyph(
            "video",
            "M3.75 4.5c-.69 0-1.25.56-1.25 1.25v4.5c0 .69.56 1.25 1.25 1.25h4.5c.69 0 1.25-.56 1.25-1.25v-4.5c0-.69-.56-1.25-1.25-1.25zm7.086.311A2.75 2.75 0 0 0 8.25 3h-4.5A2.75 2.75 0 0 0 1 5.75v4.5A2.75 2.75 0 0 0 3.75 13h4.5c1.19 0 2.202-.755 2.586-1.811q.031.02.066.038l2.98 1.677A.75.75 0 0 0 15 12.25v-8.5a.75.75 0 0 0-1.118-.654l-2.98 1.677zM11 7.17v1.66c0 .452.244.868.637 1.09l1.863 1.048V5.032L11.637 6.08A1.25 1.25 0 0 0 11 7.17",
        )
    }
    val star by lazy {
        glyph(
            "star",
            "M6.717 1.951c.389-1.268 2.178-1.268 2.566 0l1.02 3.328h3.353c1.297 0 1.825 1.64.818 2.416l-2.775 2.137 1.05 3.422c.377 1.233-1.046 2.278-2.101 1.465L8 12.68l-2.647 2.04c-1.056.813-2.48-.232-2.101-1.465L4.3 9.832 1.527 7.695c-1.008-.775-.48-2.416.817-2.416h3.353zM8 2.883l-.903 2.945c-.172.562-.69.95-1.283.95H2.795l2.504 1.93c.447.343.63.928.466 1.464l-.937 3.057 2.355-1.813a1.34 1.34 0 0 1 1.634 0l2.355 1.813-.937-3.057a1.35 1.35 0 0 1 .466-1.465l2.504-1.928h-3.019a1.34 1.34 0 0 1-1.283-.951z",
        )
    }
    val doctor by lazy {
        glyph(
            "doctor",
            "M6.08 1.004a.75.75 0 0 1 .66.628l1.535 9.587L10.293 5.5a.75.75 0 0 1 1.355-.128L13.18 8h1.069a.75.75 0 0 1 0 1.5h-1.5a.75.75 0 0 1-.648-.372l-.934-1.601-2.46 6.973a.75.75 0 0 1-1.449-.132L5.693 4.581 3.948 9.024a.75.75 0 0 1-.698.476h-1.5a.75.75 0 0 1 0-1.5h.989l2.563-6.524a.75.75 0 0 1 .779-.472",
        )
    }
    val lightbulb by lazy {
        glyph(
            "lightbulb",
            "M8 2.5a3.5 3.5 0 0 0-3.09 5.147c.178.33.584.97.889 1.44.272.42.43.908.449 1.413H7.25V6.75a.75.75 0 0 1 1.5 0v3.75h1.002a2.8 2.8 0 0 1 .45-1.413c.304-.47.71-1.11.887-1.44A3.5 3.5 0 0 0 8 2.5M9.75 12h-3.5v.75c0 .215.073.406.186.533.1.113.268.217.564.217h2c.296 0 .464-.104.564-.217a.8.8 0 0 0 .186-.533zM3 6a5 5 0 1 1 9.412 2.353c-.213.399-.657 1.095-.952 1.55-.138.212-.21.455-.21.7v2.147c0 .535-.177 1.094-.564 1.53-.4.449-.982.72-1.686.72H7c-.704 0-1.286-.271-1.686-.72a2.3 2.3 0 0 1-.564-1.53v-2.146c0-.246-.073-.489-.21-.7-.296-.456-.74-1.152-.952-1.55A5 5 0 0 1 3 6",
        )
    }
    /** Tabler Icons' toggle-right (MIT, LICENSES/MIT-Tabler-Icons.txt): G7 Linux has no switch. */
    val toggles by lazy {
        strokeIcon(
            "toggles",
            "M16 12m-2 0a2 2 0 1 0 4 0a2 2 0 1 0 -4 0 M2 6m0 6a6 6 0 0 1 6 -6h8a6 6 0 0 1 6 6v0a6 6 0 0 1 -6 6h-8a6 6 0 0 1 -6 -6z",
            width = 2f,
        )
    }
    /** Tabler Icons' camera-rotate (MIT, LICENSES/MIT-Tabler-Icons.txt): G7 Linux has no camera flip. */
    val flipCamera by lazy {
        strokeIcon(
            "flipCamera",
            "M5 7h1a2 2 0 0 0 2 -2a1 1 0 0 1 1 -1h6a1 1 0 0 1 1 1a2 2 0 0 0 2 2h1a2 2 0 0 1 2 2v9a2 2 0 0 1 -2 2h-14a2 2 0 0 1 -2 -2v-9a2 2 0 0 1 2 -2 M11.245 15.904a3 3 0 0 0 3.755 -2.904m-2.25 -2.905a3 3 0 0 0 -3.75 2.905 M14 13h2v2 M10 13h-2v-2",
            width = 2f,
        )
    }
    /** Tabler Icons' infinity (MIT, LICENSES/MIT-Tabler-Icons.txt): G7 Linux has no infinity. */
    val always by lazy {
        strokeIcon(
            "always",
            "M9.828 9.172a4 4 0 1 0 0 5.656a10 10 0 0 0 2.172 -2.828a10 10 0 0 1 2.172 -2.828a4 4 0 1 1 0 5.656a10 10 0 0 1 -2.172 -2.828a10 10 0 0 0 -2.172 -2.828",
            width = 2f,
        )
    }
    /** Tabler Icons' screenshot (MIT, LICENSES/MIT-Tabler-Icons.txt): G7 Linux's is off-grid strokes. */
    val screenshot by lazy {
        strokeIcon(
            "screenshot",
            "M7 19a2 2 0 0 1 -2 -2 M5 13v-2 M5 7a2 2 0 0 1 2 -2 M11 5h2 M17 5a2 2 0 0 1 2 2 M19 11v2 M19 17v4 M21 19h-4 M13 19h-2",
            width = 2f,
        )
    }
    /** Tabler Icons' hand-click (MIT, LICENSES/MIT-Tabler-Icons.txt): G7 Linux has no hand. */
    val gesture by lazy {
        strokeIcon(
            "gesture",
            "M8 13v-8.5a1.5 1.5 0 0 1 3 0v7.5 M11 11.5v-2a1.5 1.5 0 0 1 3 0v2.5 M14 10.5a1.5 1.5 0 0 1 3 0v1.5 M17 11.5a1.5 1.5 0 0 1 3 0v4.5a6 6 0 0 1 -6 6h-2h.208a6 6 0 0 1 -5.012 -2.7l-.196 -.3c-.312 -.479 -1.407 -2.388 -3.286 -5.728a1.5 1.5 0 0 1 .536 -2.022a1.867 1.867 0 0 1 2.28 .28l1.47 1.47 M5 3l-1 -1 M4 7h-1 M14 3l1 -1 M15 6h1",
            width = 2f,
        )
    }
}

/** The G7 mark in the accent color, with nothing behind it. */
@Composable
fun G7Mark(modifier: Modifier = Modifier, large: Boolean = false) {
    Icon(
        G7Icons.mark,
        contentDescription = stringResource(R.string.app_name),
        tint = MaterialTheme.colorScheme.primary,
        modifier = modifier.size(if (large) 48.dp else 24.dp),
    )
}

/** A group's label: small, muted, sentence case. */
@Composable
fun G7SectionLabel(text: String, modifier: Modifier = Modifier) {
    Text(
        text,
        style = MaterialTheme.typography.labelMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
        modifier = modifier
            .padding(start = 12.dp, end = 12.dp, top = 14.dp, bottom = 4.dp)
            .semantics { heading() },
    )
}

/**
 * A row: a tinted icon, a label (and maybe a line under it), a muted note on the
 * right, and whatever [trailing] adds. At least 48 dp tall, with a 10 dp corner
 * when it highlights. [active] is a mode or session that's on (drawn in the
 * accent); [busy] is an action under way (a spinner, and no second tap).
 */
@Composable
fun G7Row(
    label: String,
    modifier: Modifier = Modifier,
    icon: ImageVector? = null,
    tint: Color = Color.Unspecified,
    sublabel: String? = null,
    note: String? = null,
    enabled: Boolean = true,
    highlighted: Boolean = false,
    role: Role = Role.Button,
    active: Boolean = false,
    busy: Boolean = false,
    onClick: (() -> Unit)? = null,
    trailing: (@Composable RowScope.() -> Unit)? = null,
) {
    val scheme = MaterialTheme.colorScheme
    val shape = RoundedCornerShape(10.dp)
    Row(
        modifier = modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(shape)
            .then(
                when {
                    active -> Modifier.background(scheme.primary.copy(alpha = 0.14f)).border(1.dp, scheme.primary, shape)
                    highlighted -> Modifier.background(scheme.surfaceContainerHigh)
                    else -> Modifier
                },
            )
            .then(
                if (onClick != null) {
                    Modifier.clickable(enabled = enabled && !busy, role = role, onClick = onClick)
                } else {
                    Modifier
                },
            )
            .semantics { if (active) selected = true }
            .padding(horizontal = 12.dp, vertical = 6.dp)
            .then(if (enabled) Modifier else Modifier.alpha(0.45f)),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        // The icon sits on the label's first line, however many lines the text takes.
        Row(Modifier.weight(1f), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            if (icon != null) {
                FirstLineIcon(
                    icon = icon,
                    tint = if (tint == Color.Unspecified) scheme.onSurfaceVariant else tint,
                    style = MaterialTheme.typography.bodyLarge,
                )
            }
            Column(Modifier.weight(1f)) {
                Text(
                    label,
                    style = MaterialTheme.typography.bodyLarge,
                    color = if (active) scheme.primary else scheme.onSurface,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                if (!sublabel.isNullOrEmpty()) {
                    Text(
                        sublabel,
                        style = MaterialTheme.typography.bodySmall,
                        color = scheme.onSurfaceVariant,
                        maxLines = 3,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
        }
        if (!note.isNullOrEmpty()) {
            Text(
                note,
                style = MaterialTheme.typography.bodyMedium,
                color = scheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        if (busy) {
            androidx.compose.material3.CircularProgressIndicator(
                modifier = Modifier.size(18.dp),
                strokeWidth = 2.dp,
                color = scheme.primary,
            )
        }
        trailing?.invoke(this)
    }
}

/**
 * A leading icon centred on the first line of text set in [style], for a row
 * whose text can wrap: the row lays it out at the top, and the text beside it
 * starts at the same top.
 */
@Composable
fun FirstLineIcon(icon: ImageVector, tint: Color, style: TextStyle, modifier: Modifier = Modifier, size: Dp = 22.dp) {
    val lineHeight = with(LocalDensity.current) {
        if (style.lineHeight.isSp) style.lineHeight.toDp() else size
    }
    Box(modifier.height(lineHeight), contentAlignment = Alignment.Center) {
        Icon(
            imageVector = icon,
            contentDescription = null,
            tint = tint,
            modifier = Modifier.requiredSize(size),
        )
    }
}

/** A row with a switch; the whole row toggles it. */
@Composable
fun G7SwitchRow(
    label: String,
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
    icon: ImageVector? = null,
    tint: Color = Color.Unspecified,
    sublabel: String? = null,
    enabled: Boolean = true,
    highlighted: Boolean = false,
) {
    G7Row(
        label = label,
        modifier = modifier,
        icon = icon,
        tint = tint,
        sublabel = sublabel,
        enabled = enabled,
        highlighted = highlighted,
        role = Role.Switch,
        onClick = { onCheckedChange(!checked) },
    ) {
        G7Switch(checked = checked, onCheckedChange = null, enabled = enabled)
    }
}

@Composable
fun G7Switch(checked: Boolean, onCheckedChange: ((Boolean) -> Unit)?, enabled: Boolean = true) {
    Switch(checked = checked, onCheckedChange = onCheckedChange, enabled = enabled, colors = g7SwitchColors())
}

/** A small action at the end of a row: Allow, Open, Stop. */
@Composable
fun G7RowAction(
    label: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    accent: Boolean = false,
    enabled: Boolean = true,
) {
    val scheme = MaterialTheme.colorScheme
    Text(
        label,
        style = MaterialTheme.typography.labelLarge,
        color = if (accent) scheme.onPrimary else scheme.onSurface,
        maxLines = 1,
        modifier = modifier
            .clip(RoundedCornerShape(8.dp))
            .background(if (accent) scheme.primary else scheme.surfaceContainer)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 7.dp)
            .then(if (enabled) Modifier else Modifier.alpha(0.45f)),
    )
}

/** The screen's one primary action: full width, lavender. */
@Composable
fun G7PrimaryButton(label: String, onClick: () -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true) {
    val scheme = MaterialTheme.colorScheme
    Box(
        modifier = modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(RoundedCornerShape(10.dp))
            .background(scheme.primary)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(14.dp)
            .then(if (enabled) Modifier else Modifier.alpha(0.45f)),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, style = MaterialTheme.typography.titleSmall, color = scheme.onPrimary, maxLines = 1)
    }
}

/**
 * A raised, quiet button: clicks, keys, secondary actions. [selected] is a
 * mode that's on, drawn in the accent so it reads at a glance;
 * [onLongClick] is a second action on a hold.
 */
@Composable
fun G7RaisedButton(
    label: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    selected: Boolean = false,
    onLongClick: (() -> Unit)? = null,
) {
    val scheme = MaterialTheme.colorScheme
    val shape = RoundedCornerShape(10.dp)
    Box(
        modifier = modifier
            .heightIn(min = 48.dp)
            .clip(shape)
            .background(if (selected) scheme.primary.copy(alpha = 0.18f) else scheme.surfaceContainer)
            .then(if (selected) Modifier.border(1.dp, scheme.primary, shape) else Modifier)
            .combinedClickable(enabled = enabled, role = Role.Button, onLongClick = onLongClick, onClick = onClick)
            .semantics { this.selected = selected }
            .padding(horizontal = 8.dp, vertical = 12.dp)
            .then(if (enabled) Modifier else Modifier.alpha(0.45f)),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            label,
            style = MaterialTheme.typography.labelLarge,
            color = if (selected) scheme.primary else scheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** A bare icon button in a tint, for toolbars. */
@Composable
fun G7IconButton(
    icon: ImageVector,
    contentDescription: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    tint: Color = Color.Unspecified,
    enabled: Boolean = true,
    filled: Boolean = false,
    selected: Boolean = false,
) {
    val scheme = MaterialTheme.colorScheme
    val description = contentDescription
    val shape = RoundedCornerShape(10.dp)
    Box(
        modifier = modifier
            .heightIn(min = 48.dp)
            .clip(shape)
            .then(
                when {
                    // A mode that's on: the accent, so it reads at a glance.
                    selected -> Modifier.background(scheme.primary.copy(alpha = 0.18f)).border(1.dp, scheme.primary, shape)
                    filled -> Modifier.background(scheme.surfaceContainer)
                    else -> Modifier
                },
            )
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .semantics {
                this.contentDescription = description
                this.selected = selected
            }
            .padding(12.dp)
            .then(if (enabled) Modifier else Modifier.alpha(0.45f)),
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            imageVector = icon,
            contentDescription = null,
            tint = if (selected) scheme.primary else if (tint == Color.Unspecified) scheme.onSurface else tint,
            modifier = Modifier.size(22.dp),
        )
    }
}

/** Whether the theme shows dark: keycaps and wells shade differently on a light ground. */
@Composable
@ReadOnlyComposable
private fun g7ShowsDark(): Boolean = MaterialTheme.colorScheme.background.luminance() < 0.5f

/**
 * A keycap's body: a raised face on a darker edge, with a soft shadow under
 * it. [pressed] sinks the face onto its edge; [selected] (a key latched on)
 * tints it in the accent and holds it part way down; [ready] (something waits
 * for this key) rings it in the accent with a faint glow. [face] replaces the
 * face's color, and its edge is darkened from it.
 */
@Composable
fun G7KeycapSurface(
    pressed: Boolean,
    modifier: Modifier = Modifier,
    selected: Boolean = false,
    ready: Boolean = false,
    shape: Shape = RoundedCornerShape(14.dp),
    edge: Dp = 5.dp,
    face: Color = Color.Unspecified,
    contentColor: Color = Color.Unspecified,
    contentPadding: PaddingValues = PaddingValues(0.dp),
    content: @Composable BoxScope.() -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    val dark = g7ShowsDark()
    val ownFace = face != Color.Unspecified
    val restFace = if (ownFace) face else scheme.surfaceContainerHigh
    val faceColor = if (selected && !ownFace) lerp(restFace, scheme.primary, 0.16f) else restFace
    val edgeColor = lerp(
        faceColor,
        Color.Black,
        when {
            !dark -> 0.24f
            selected || ownFace -> 0.5f
            else -> 0.7f
        },
    )
    // The face's top catches the light: a 1 dp line along its upper edge.
    val litFace = Color.White.copy(alpha = if (dark) 0.09f else 0.6f).compositeOver(faceColor)
    val accentRing = selected || ready
    val fg = when {
        contentColor != Color.Unspecified -> contentColor
        accentRing -> scheme.primary
        else -> scheme.onSurface
    }
    val shadow = Shadow(
        radius = 6.dp,
        color = Color.Black,
        offset = DpOffset(0.dp, 3.dp),
        alpha = if (dark) 0.45f else 0.14f,
    )
    val glow = if (ready) Shadow(radius = 10.dp, color = scheme.primary, alpha = 0.35f) else null
    val travel = when {
        pressed -> edge - 1.dp
        selected -> minOf(2.dp, edge - 1.dp)
        else -> 0.dp
    }
    val sink = animateDpAsState(
        targetValue = travel,
        animationSpec = if (LocalReducedMotion.current) snap<Dp>() else tween<Dp>(durationMillis = 60),
        label = "KeycapPress",
    )
    Box(modifier = modifier, propagateMinConstraints = true) {
        Box(
            Modifier
                .matchParentSize()
                .padding(top = edge)
                .then(if (glow != null) Modifier.dropShadow(shape, glow) else Modifier)
                .dropShadow(shape, shadow)
                .background(edgeColor, shape),
        )
        Box(
            modifier = Modifier
                .padding(bottom = edge)
                .offset { IntOffset(0, sink.value.roundToPx()) }
                .clip(shape)
                .background(litFace)
                .drawWithCache {
                    val outline = shape.createOutline(size, layoutDirection, this)
                    val lift = 1.dp.toPx()
                    onDrawBehind { translate(top = lift) { drawOutline(outline, faceColor) } }
                }
                .then(if (accentRing) Modifier.border(1.dp, scheme.primary, shape) else Modifier)
                .padding(contentPadding),
            contentAlignment = Alignment.Center,
        ) {
            CompositionLocalProvider(LocalContentColor provides fg) { content() }
        }
    }
}

/**
 * A raised key that goes down under the finger: mouse buttons, special keys,
 * quick actions. [selected] is a key latched on (a held modifier, a mode);
 * [ready] is a key with something waiting for it; [onLongClick] is a second
 * action on a hold. The caller plays any haptics.
 */
@Composable
fun G7Keycap(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    onLongClick: (() -> Unit)? = null,
    enabled: Boolean = true,
    selected: Boolean = false,
    ready: Boolean = false,
    corner: Dp = 14.dp,
    edge: Dp = 5.dp,
    role: Role = Role.Button,
    contentDescription: String? = null,
    contentPadding: PaddingValues = PaddingValues(horizontal = 8.dp),
    content: @Composable BoxScope.() -> Unit,
) {
    val interactions = remember { MutableInteractionSource() }
    val pressed by interactions.collectIsPressedAsState()
    val description = contentDescription
    G7KeycapSurface(
        pressed = pressed && enabled,
        selected = selected,
        ready = ready,
        shape = RoundedCornerShape(corner),
        edge = edge,
        contentPadding = contentPadding,
        modifier = modifier
            .combinedClickable(
                interactionSource = interactions,
                indication = null,
                enabled = enabled,
                role = role,
                onLongClick = onLongClick,
                hapticFeedbackEnabled = false,
                onClick = onClick,
            )
            .semantics {
                this.selected = selected || ready
                if (description != null) this.contentDescription = description
            }
            .then(if (enabled) Modifier else Modifier.alpha(0.45f)),
        content = content,
    )
}

/** A [G7Keycap] with a one-line label in the app's mono face. */
@Composable
fun G7Keycap(
    label: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    onLongClick: (() -> Unit)? = null,
    enabled: Boolean = true,
    selected: Boolean = false,
    ready: Boolean = false,
    corner: Dp = 14.dp,
    edge: Dp = 5.dp,
    role: Role = Role.Button,
    contentDescription: String? = null,
    style: TextStyle = MaterialTheme.typography.labelLarge,
    contentPadding: PaddingValues = PaddingValues(horizontal = 8.dp),
) {
    G7Keycap(
        onClick = onClick,
        modifier = modifier,
        onLongClick = onLongClick,
        enabled = enabled,
        selected = selected,
        ready = ready,
        corner = corner,
        edge = edge,
        role = role,
        contentDescription = contentDescription,
        contentPadding = contentPadding,
    ) {
        Text(label, style = style, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** A rounded-square [G7Keycap] with an icon in its tint, for a row of quick actions. */
@Composable
fun G7IconKeycap(
    icon: ImageVector,
    contentDescription: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    tint: Color = Color.Unspecified,
    enabled: Boolean = true,
    selected: Boolean = false,
) {
    G7Keycap(
        onClick = onClick,
        modifier = modifier,
        enabled = enabled,
        selected = selected,
        corner = 18.dp,
        contentDescription = contentDescription,
        contentPadding = PaddingValues(0.dp),
    ) {
        Icon(
            imageVector = icon,
            contentDescription = null,
            tint = if (selected || tint == Color.Unspecified) LocalContentColor.current else tint,
            modifier = Modifier.size(22.dp),
        )
    }
}

/**
 * A sunken well, as the touchpad is: a ground a shade under the screen's with
 * its top edge in shadow. [dots] lays a faint grid over it, and [outline] rings
 * its inside in that color.
 */
@Composable
fun G7Well(
    modifier: Modifier = Modifier,
    shape: Shape = RoundedCornerShape(22.dp),
    dots: Boolean = false,
    outline: Color? = null,
    content: @Composable BoxScope.() -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    val dark = g7ShowsDark()
    val ground = lerp(scheme.surface, scheme.surfaceContainer, if (dark) 0.35f else 0.8f)
    val shade = Color.Black.copy(alpha = if (dark) 0.42f else 0.09f)
    val rim = if (dark) Color.White.copy(alpha = 0.035f) else Color.Black.copy(alpha = 0.05f)
    val dot = if (dark) Color.White.copy(alpha = 0.055f) else Color.Black.copy(alpha = 0.06f)
    val density = LocalDensity.current
    // Built once: the touchpad's well changes size on every frame while the keyboard moves.
    val look = remember(density, shade, dot) { WellLook(density, shade, dot) }
    Box(
        modifier = modifier
            .clip(shape)
            .background(ground)
            .drawWithCache {
                val wellOutline = shape.createOutline(size, layoutDirection, this)
                val dotCount = if (dots) look.layDots(size) else 0
                onDrawBehind {
                    drawRect(look.shadeBrush, size = Size(size.width, look.shadeHeight))
                    if (dotCount > 0) {
                        drawContext.canvas.nativeCanvas.drawPoints(look.dots, 0, dotCount * 2, look.dotPaint)
                    }
                    drawOutline(wellOutline, rim, style = look.rimStroke)
                    if (outline != null) drawOutline(wellOutline, outline, style = look.outlineStroke)
                }
            },
        content = content,
    )
}

/** What [G7Well] draws with, so a well that resizes every frame allocates nothing for it. */
private class WellLook(density: Density, shade: Color, dot: Color) {
    val shadeHeight = with(density) { 16.dp.toPx() }
    val shadeBrush = Brush.verticalGradient(listOf(shade, Color.Transparent), startY = 0f, endY = shadeHeight)

    // Strokes are centred on the edge and the clip keeps their inner half.
    val rimStroke = Stroke(width = with(density) { 2.dp.toPx() })
    val outlineStroke = Stroke(width = with(density) { 3.dp.toPx() })
    val dotPaint = android.graphics.Paint().apply {
        isAntiAlias = true
        color = dot.toArgb()
        strokeWidth = with(density) { 1.dp.toPx() }
        strokeCap = android.graphics.Paint.Cap.ROUND
    }
    private val step = with(density) { 18.dp.toPx() }

    /** Dot centres as x, y pairs; it only grows, so shrinking and growing back is free. */
    var dots = FloatArray(0)
        private set

    /** Lays a grid centred in [size] into [dots] and returns how many dots it holds. */
    fun layDots(size: Size): Int {
        if (step <= 0f) return 0
        val columns = (size.width / step).toInt().coerceAtLeast(0)
        val rows = (size.height / step).toInt().coerceAtLeast(0)
        val count = columns * rows
        if (count == 0) return 0
        if (dots.size < count * 2) dots = FloatArray(count * 2)
        val left = (size.width - (columns - 1) * step) / 2f
        val top = (size.height - (rows - 1) * step) / 2f
        var i = 0
        for (row in 0 until rows) {
            for (column in 0 until columns) {
                dots[i++] = left + column * step
                dots[i++] = top + row * step
            }
        }
        return count
    }
}

/** A sub-screen's header: Back, the title, and a muted note after it. */
@Composable
fun G7Header(
    title: String,
    onBack: (() -> Unit)?,
    modifier: Modifier = Modifier,
    note: String? = null,
    noteColor: Color = Color.Unspecified,
    trailing: (@Composable RowScope.() -> Unit)? = null,
) {
    Row(
        modifier = modifier.fillMaxWidth().padding(start = 4.dp, end = 8.dp, top = 4.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (onBack != null) {
            G7IconButton(
                icon = G7Icons.back,
                contentDescription = stringResource(R.string.action_back),
                onClick = onBack,
            )
        } else {
            Box(Modifier.size(8.dp))
        }
        Row(
            modifier = Modifier.weight(1f),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                title,
                style = MaterialTheme.typography.titleMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f, fill = false),
            )
            if (!note.isNullOrEmpty()) {
                Text(
                    note,
                    style = MaterialTheme.typography.labelMedium,
                    color = if (noteColor == Color.Unspecified) MaterialTheme.colorScheme.onSurfaceVariant else noteColor,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        trailing?.invoke(this)
    }
}

/** Text tabs: the picked one sits on the selection fill. */
@Composable
fun <T> G7Tabs(
    options: List<Pair<T, String>>,
    selected: T,
    onSelect: (T) -> Unit,
    modifier: Modifier = Modifier,
) {
    val scheme = MaterialTheme.colorScheme
    Row(
        modifier = modifier.horizontalScroll(rememberScrollState()),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        options.forEach { (value, label) ->
            val isSelected = value == selected
            val shape = RoundedCornerShape(8.dp)
            Text(
                label,
                style = MaterialTheme.typography.labelLarge,
                color = if (isSelected) scheme.primary else scheme.onSurfaceVariant,
                maxLines = 1,
                modifier = Modifier
                    .clip(shape)
                    .background(if (isSelected) scheme.primary.copy(alpha = 0.18f) else Color.Transparent)
                    .then(if (isSelected) Modifier.border(1.dp, scheme.primary, shape) else Modifier)
                    .clickable(role = Role.Tab) { onSelect(value) }
                    .semantics { this.selected = isSelected; role = Role.Tab }
                    .padding(horizontal = 12.dp, vertical = 8.dp),
            )
        }
    }
}

/**
 * A flat slider: a thin track filled in [tint] up to a small round thumb, like
 * G7's own bars, in place of Material's thick two-part track and bar thumb.
 */
@OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
@Composable
fun G7Slider(
    value: Float,
    onValueChange: (Float) -> Unit,
    modifier: Modifier = Modifier,
    valueRange: ClosedFloatingPointRange<Float> = 0f..1f,
    onValueChangeFinished: (() -> Unit)? = null,
    tint: Color = MaterialTheme.colorScheme.primary,
    trackColor: Color = MaterialTheme.colorScheme.surfaceContainerHigh,
    enabled: Boolean = true,
) {
    androidx.compose.material3.Slider(
        value = value,
        onValueChange = onValueChange,
        modifier = modifier,
        enabled = enabled,
        valueRange = valueRange,
        onValueChangeFinished = onValueChangeFinished,
        thumb = {
            Box(Modifier.size(14.dp).clip(RoundedCornerShape(7.dp)).background(tint))
        },
        track = { state ->
            val span = (state.valueRange.endInclusive - state.valueRange.start).takeIf { it > 0f } ?: 1f
            val fraction = ((state.value - state.valueRange.start) / span).coerceIn(0f, 1f)
            androidx.compose.foundation.Canvas(Modifier.fillMaxWidth().heightIn(min = 6.dp)) {
                val height = 6.dp.toPx()
                val top = (size.height - height) / 2f
                val radius = androidx.compose.ui.geometry.CornerRadius(height / 2f, height / 2f)
                drawRoundRect(
                    color = trackColor,
                    topLeft = androidx.compose.ui.geometry.Offset(0f, top),
                    size = androidx.compose.ui.geometry.Size(size.width, height),
                    cornerRadius = radius,
                )
                drawRoundRect(
                    color = tint,
                    topLeft = androidx.compose.ui.geometry.Offset(0f, top),
                    size = androidx.compose.ui.geometry.Size(size.width * fraction, height),
                    cornerRadius = radius,
                )
            }
        },
    )
}

/** A dot in a status color, before a line of status text. */
@Composable
fun G7StatusDot(color: Color, modifier: Modifier = Modifier) {
    Box(modifier.size(6.dp).clip(RoundedCornerShape(3.dp)).background(color))
}
