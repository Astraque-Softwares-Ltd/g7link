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
 * The font bundled with the app (`assets/fonts` in the repository, packaged as
 * assets under `fonts/`): G7 sets everything in Geist Mono, as it does on the desktop.
 */
@Immutable
class AppFonts(assets: AssetManager) {
    val mono = FontFamily(
        listOf(400, 500, 600).map { weight ->
            Font(
                "fonts/GeistMono-Variable.ttf",
                assets,
                FontWeight(weight),
                variationSettings = FontVariation.Settings(FontVariation.weight(weight)),
            )
        },
    )
}

val LocalAppFonts = staticCompositionLocalOf<AppFonts> { error("G7LinkTheme provides the fonts") }

/**
 * G7's type scale, smaller than Material's: a mono face reads large, and rows
 * are 15 sp with 12 sp labels.
 */
internal fun typography(fonts: AppFonts): Typography {
    val family = fonts.mono
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
