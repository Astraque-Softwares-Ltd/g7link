// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.player

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathFillType
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.unit.dp

/**
 * The player's glyphs that G7Icons doesn't carry: G7 Linux's own 16 px icons
 * (src/server/icons in g7linux: shuffle.svg, repeat.svg, play-filled.svg,
 * pause-filled.svg), filled even-odd as the SVGs are, drawn in a tint.
 */
internal object PlayerGlyphs {
    val shuffle by lazy {
        glyph(
            "shuffle",
            "M11.193 1.248a.75.75 0 0 1 1.059-.055l2.5 2.25a.75.75 0 0 1 0 1.114l-2.5 2.25a.75.75 0 1 1-1.004-1.114l1.048-.943h-1.694a3.25 3.25 0 0 0-3.073 2.19l-.388 1.126.367 1.027a3.25 3.25 0 0 0 3.06 2.157h1.728l-1.048-.943a.75.75 0 1 1 1.004-1.114l2.5 2.25a.75.75 0 0 1 0 1.114l-2.5 2.25a.75.75 0 1 1-1.004-1.114l1.048-.943h-1.727a4.75 4.75 0 0 1-4.304-2.74A4.75 4.75 0 0 1 2.398 12H1.75a.75.75 0 0 1 0-1.5h.648a3.25 3.25 0 0 0 3.073-2.19l.08-.235-.06-.168a3.25 3.25 0 0 0-3.06-2.157H1.75a.75.75 0 0 1 0-1.5h.681A4.75 4.75 0 0 1 6.23 6.146a4.75 4.75 0 0 1 4.373-2.896h1.694l-1.048-.943a.75.75 0 0 1-.055-1.059",
        )
    }
    val repeat by lazy {
        glyph(
            "repeat",
            "M7.193 1.248a.75.75 0 0 1 1.059-.055l2.5 2.25a.75.75 0 0 1 0 1.114l-2.5 2.25a.75.75 0 0 1-1.004-1.114l1.048-.943H5.75A3.25 3.25 0 0 0 2.5 8v1.25a.75.75 0 0 1-1.5 0V8a4.75 4.75 0 0 1 4.75-4.75h2.546l-1.048-.943a.75.75 0 0 1-.055-1.059M14.25 6a.75.75 0 0 1 .75.75V8a4.75 4.75 0 0 1-4.75 4.75H7.704l1.048.943a.75.75 0 0 1-1.004 1.114l-2.5-2.25a.75.75 0 0 1 0-1.114l2.5-2.25a.75.75 0 1 1 1.004 1.114l-1.048.943h2.546A3.25 3.25 0 0 0 13.5 8V6.75a.75.75 0 0 1 .75-.75",
        )
    }
    val playFilled by lazy {
        glyph(
            "playFilled",
            "M8 15A7 7 0 1 0 8 1a7 7 0 0 0 0 14m-.844-9.88A.75.75 0 0 0 6 5.75v4.5a.75.75 0 0 0 1.156.63l3.5-2.25a.75.75 0 0 0 0-1.26z",
        )
    }
    val pauseFilled by lazy {
        glyph(
            "pauseFilled",
            "M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1M6.25 5a.75.75 0 0 1 .75.75v4.5a.75.75 0 0 1-1.5 0v-4.5A.75.75 0 0 1 6.25 5m3.5 0a.75.75 0 0 1 .75.75v4.5a.75.75 0 0 1-1.5 0v-4.5A.75.75 0 0 1 9.75 5",
        )
    }

    private fun glyph(name: String, path: String): ImageVector =
        ImageVector.Builder(
            name = name,
            defaultWidth = 24.dp,
            defaultHeight = 24.dp,
            viewportWidth = 16f,
            viewportHeight = 16f,
        ).addPath(
            pathData = PathParser().parsePathString(path).toNodes(),
            pathFillType = PathFillType.EvenOdd,
            fill = SolidColor(Color.White),
        ).build()
}
