// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathFillType
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.unit.dp

/**
 * The few G7 glyphs the Agents screens draw that the app's own set doesn't
 * have yet, with the paths of desktop/app/qml/G7Glyphs.qml, or of G7 Linux's
 * src/server/icons where that file has none, drawn the way the app draws its glyphs.
 */
internal object AgentGlyphs {
    val copy by lazy {
        glyph(
            "copy",
            "M4 3.75A2.75 2.75 0 0 1 6.75 1h5.5A2.75 2.75 0 0 1 15 3.75v5.5A2.75 2.75 0 0 1 12.25 12H12v.25A2.75 2.75 0 0 1 9.25 15h-5.5A2.75 2.75 0 0 1 1 12.25v-5.5A2.75 2.75 0 0 1 3.75 4H4zm.75 1.75h-1c-.69 0-1.25.56-1.25 1.25v5.5c0 .69.56 1.25 1.25 1.25h5.5c.69 0 1.25-.56 1.25-1.25v-5.5c0-.69-.56-1.25-1.25-1.25zm7.25 5V6.75A2.75 2.75 0 0 0 9.25 4H5.5v-.25c0-.69.56-1.25 1.25-1.25h5.5c.69 0 1.25.56 1.25 1.25v5.5c0 .69-.56 1.25-1.25 1.25zM6.5 7a.75.75 0 0 1 .75.75v1h1a.75.75 0 0 1 0 1.5h-1v1a.75.75 0 0 1-1.5 0v-1h-1a.75.75 0 0 1 0-1.5h1v-1A.75.75 0 0 1 6.5 7",
        )
    }

    val warning by lazy {
        glyph(
            "warning",
            "M10.9 2.793c-1.195-2.39-4.605-2.39-5.8 0l-3.755 7.514C.268 12.463 1.835 15 4.245 15h7.51c2.41 0 3.977-2.537 2.9-4.693zm-4.458.67c.642-1.284 2.474-1.284 3.116 0l3.755 7.514a1.743 1.743 0 0 1-1.558 2.523h-7.51a1.743 1.743 0 0 1-1.558-2.523zM8.75 6a.75.75 0 0 0-1.5 0v2a.75.75 0 1 0 1.5 0zM8 10a1 1 0 1 0 0 2 1 1 0 0 0 0-2",
        )
    }

    val more by lazy {
        glyph(
            "more",
            "M5.007 9.002a1.002 1.002 0 1 0 0-2.004 1.002 1.002 0 0 0 0 2.004M9.002 8a1.002 1.002 0 1 1-2.004 0 1.002 1.002 0 0 1 2.004 0m2.993 0a1.002 1.002 0 1 1-2.004 0 1.002 1.002 0 0 1 2.004 0",
        )
    }

    val chevronUp by lazy {
        glyph(
            "chevronUp",
            "M8 5a.75.75 0 0 1 .545.235l4.25 4.5a.75.75 0 0 1-1.09 1.03L8 6.842l-3.705 3.923a.75.75 0 0 1-1.09-1.03l4.25-4.5A.75.75 0 0 1 8 5",
        )
    }

    /** G7 Linux's `app-window-sidebar-left`: the drawer of threads. */
    val sidebar by lazy {
        glyph(
            "sidebar",
            "M1 3.75A2.75 2.75 0 0 1 3.75 1h8.5A2.75 2.75 0 0 1 15 3.75v8.5A2.75 2.75 0 0 1 12.25 15h-8.5A2.75 2.75 0 0 1 1 12.25zM3.75 2.5c-.69 0-1.25.56-1.25 1.25v8.5c0 .69.56 1.25 1.25 1.25H5.5v-11zM7 2.5v11h5.25c.69 0 1.25-.56 1.25-1.25v-8.5c0-.69-.56-1.25-1.25-1.25z",
        )
    }

    private fun glyph(name: String, vararg paths: String): ImageVector {
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
                pathFillType = PathFillType.EvenOdd,
                fill = SolidColor(Color.White),
            )
        }
        return builder.build()
    }
}
