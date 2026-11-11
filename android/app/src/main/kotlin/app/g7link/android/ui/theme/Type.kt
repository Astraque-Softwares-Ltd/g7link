// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.theme

import android.content.res.AssetManager
import androidx.compose.material3.Typography
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

/**
 * The fonts bundled with the app (`assets/fonts` in the repository, packaged
 * as assets under `fonts/`): Figtree for text, Instrument Serif for
 * Graphite's headings, Space Mono for codes. The G7 theme sets everything in
 * Geist Mono, as G7 does on the desktop.
 */
@Immutable
class AppFonts(assets: AssetManager, quiet: Boolean = false) {
    private val geist = FontFamily(
        listOf(400, 500, 600).map { weight ->
            Font(
                "fonts/GeistMono-Variable.ttf",
                assets,
                FontWeight(weight),
                variationSettings = FontVariation.Settings(FontVariation.weight(weight)),
            )
        },
    )

    /** Figtree is a variable font: one file, every weight the app uses. */
    val sans = if (quiet) geist else FontFamily(
        listOf(400, 500, 600, 650, 700).map { weight ->
            Font(
                "fonts/Figtree.ttf",
                assets,
                FontWeight(weight),
                variationSettings = FontVariation.Settings(FontVariation.weight(weight)),
            )
        },
    )
    val serif = FontFamily(Font("fonts/InstrumentSerif-Regular.ttf", assets))
    val mono = if (quiet) geist else FontFamily(
        Font("fonts/SpaceMono-Regular.ttf", assets),
        Font("fonts/SpaceMono-Bold.ttf", assets, FontWeight.Bold),
    )
}

val LocalAppFonts = staticCompositionLocalOf<AppFonts> { error("G7LinkTheme provides the fonts") }

/**
 * Material's type scale (sizes and line heights suit a phone) in the bundled
 * fonts, with the weights from docs/design/tokens.json: bold headings in
 * Bloom, serif headings in Graphite. G7 has its own, smaller scale: a mono
 * face reads large, and rows are 15 sp with 12 sp labels.
 */
internal fun typography(fonts: AppFonts, graphite: Boolean, quiet: Boolean = false): Typography {
    val base = Typography()
    if (quiet) return quietTypography(fonts.sans)
    fun TextStyle.text(weight: Int) = copy(fontFamily = fonts.sans, fontWeight = FontWeight(weight))
    fun TextStyle.heading() =
        if (graphite) copy(fontFamily = fonts.serif, fontWeight = FontWeight.Normal) else text(650)
    return base.copy(
        displayLarge = base.displayLarge.heading(),
        displayMedium = base.displayMedium.heading(),
        displaySmall = base.displaySmall.heading(),
        headlineLarge = base.headlineLarge.heading(),
        headlineMedium = base.headlineMedium.heading(),
        headlineSmall = base.headlineSmall.heading(),
        titleLarge = base.titleLarge.text(600),
        titleMedium = base.titleMedium.text(600),
        titleSmall = base.titleSmall.text(600),
        bodyLarge = base.bodyLarge.text(400),
        bodyMedium = base.bodyMedium.text(400),
        bodySmall = base.bodySmall.text(400),
        labelLarge = base.labelLarge.text(600),
        labelMedium = base.labelMedium.text(600),
        labelSmall = base.labelSmall.text(500),
    )
}

private fun quietTypography(family: FontFamily): Typography {
    fun style(size: Int, weight: Int, lineHeight: Float = 1.35f, tracking: Float = 0f) = TextStyle(
        fontFamily = family,
        fontWeight = FontWeight(weight),
        fontSize = size.sp,
        lineHeight = (size * lineHeight).sp,
        letterSpacing = tracking.sp,
    )
    return Typography(
        displayLarge = style(56, 400, 1.05f, -1.5f),
        displayMedium = style(44, 400, 1.1f, -1f),
        displaySmall = style(34, 400, 1.15f, -0.5f),
        headlineLarge = style(28, 500, 1.2f),
        headlineMedium = style(24, 500, 1.25f),
        headlineSmall = style(20, 500, 1.3f),
        titleLarge = style(20, 500, 1.3f),
        titleMedium = style(17, 500),
        titleSmall = style(15, 500),
        bodyLarge = style(15, 400),
        bodyMedium = style(14, 400),
        bodySmall = style(12, 400),
        labelLarge = style(13, 500),
        labelMedium = style(12, 500),
        labelSmall = style(11, 500),
    )
}
