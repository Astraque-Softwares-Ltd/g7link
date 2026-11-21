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

    /** G7 Linux's `layers`: messages waiting in the queue. */
    val queue by lazy {
        glyph(
            "queue",
            "M7.578 1.13a.75.75 0 0 1 .844 0l6.25 4.25a.75.75 0 0 1 0 1.24L12.642 8l2.03 1.38a.75.75 0 0 1 0 1.24l-6.25 4.25a.75.75 0 0 1-.844 0l-6.25-4.25a.75.75 0 0 1 0-1.24L3.358 8l-2.03-1.38a.75.75 0 0 1 0-1.24zM4.691 8.907 3.084 10 8 13.343 12.916 10 11.31 8.907 8.422 10.87a.75.75 0 0 1-.844 0zM3.084 6 8 9.343 12.916 6 8 2.657z",
        )
    }

    /** G7 Linux's `forward`: a message that steers the agent while it works. */
    val steer by lazy {
        glyph(
            "steer",
            "M1.343 3.12a.75.75 0 0 1 .715-.054L6 4.84V3.75a.75.75 0 0 1 1.12-.653l7.5 4.25a.75.75 0 0 1 0 1.306l-7.5 4.25A.75.75 0 0 1 6 12.25v-1.09l-3.942 1.774A.75.75 0 0 1 1 12.25v-8.5a.75.75 0 0 1 .343-.63M6 9.515v-3.03L2.5 4.91v6.18zm1.5-4.478v5.926L12.729 8z",
        )
    }

    /** G7 Linux's `plug`: the user's MCP servers. */
    val plug by lazy {
        glyph(
            "plug",
            "M11.78 1.22a.75.75 0 0 1 0 1.06L9.803 4.258l1.94 1.94L13.72 4.22a.75.75 0 1 1 1.06 1.06l-1.977 1.978.977.977a.75.75 0 0 1 0 1.061l-1.238 1.239a5.005 5.005 0 0 1-6.507.49L2.28 14.78a.75.75 0 0 1-1.06-1.06l3.755-3.755a5.005 5.005 0 0 1 .49-6.507l1.24-1.238a.75.75 0 0 1 1.06 0l.978.977L10.72 1.22a.75.75 0 0 1 1.06 0M8.212 4.788l-.978-.977-.708.708a3.504 3.504 0 1 0 4.955 4.955l.708-.708-.977-.978z",
        )
    }

    /** G7 Link's `pin`: a pinned thread, and "Pin" on a swipe. */
    val pin by lazy { glyph("pin", "M3.5 7a4.5 4.5 0 0 1 9 0c0 .693-.262 1.45-.718 2.231-.453.776-1.067 1.526-1.7 2.189A20 20 0 0 1 8 13.308q-.151-.12-.343-.277a20 20 0 0 1-1.74-1.611c-.632-.663-1.246-1.413-1.699-2.189C3.763 8.451 3.5 7.693 3.5 7m4.064 7.86L8 14.25za.75.75 0 0 0 .872 0L8 14.25l.436.61.002-.001.004-.003.013-.01.049-.035.176-.132a22 22 0 0 0 2.487-2.223c.68-.713 1.379-1.557 1.91-2.469.53-.906.923-1.93.923-2.987A6 6 0 0 0 2 7c0 1.057.394 2.08.922 2.987.532.912 1.23 1.756 1.91 2.468a22 22 0 0 0 2.664 2.356l.049.035.013.01.004.003zM8 5a2 2 0 1 0 0 4 2 2 0 0 0 0-4") }

    /** G7 Link's `tray`: the Archived shelf, and "Archive" on a swipe. */
    val tray by lazy { glyph("tray", "M2.575 2.847A2.75 2.75 0 0 1 5.172 1h5.656a2.75 2.75 0 0 1 2.598 1.847l.004.011 1.532 4.658A.8.8 0 0 1 15 7.75v4.5A2.75 2.75 0 0 1 12.25 15h-8.5A2.75 2.75 0 0 1 1 12.25v-4.5a.8.8 0 0 1 .038-.234L2.57 2.858zm1.418.487L2.786 7H5a.75.75 0 0 1 .735.599l.002.009.013.053a3 3 0 0 0 .427.909c.332.474.873.93 1.823.93s1.49-.456 1.823-.93a3 3 0 0 0 .44-.962l.002-.009v.003V7.6A.75.75 0 0 1 11 7h2.214l-1.207-3.666a1.25 1.25 0 0 0-1.179-.834H5.172a1.25 1.25 0 0 0-1.179.834M13.5 8.5h-1.951a4.5 4.5 0 0 1-.497.93C10.509 10.206 9.55 11 8 11s-2.51-.794-3.052-1.57a4.5 4.5 0 0 1-.497-.93H2.5v3.75c0 .69.56 1.25 1.25 1.25h8.5c.69 0 1.25-.56 1.25-1.25z") }

    /** G7 Link's `laptop`: "Files aren't changed from the phone. Do that on <PC>." */
    val laptop by lazy { glyph("laptop", "M1 3.75A2.75 2.75 0 0 1 3.75 1h8.5A2.75 2.75 0 0 1 15 3.75v5a2.75 2.75 0 0 1-2.75 2.75h-2v2.039a.75.75 0 0 1-.25 1.457H6a.75.75 0 0 1-.25-1.457V11.5h-2A2.75 2.75 0 0 1 1 8.75zM6.5 10h5.75c.69 0 1.25-.56 1.25-1.25v-5c0-.69-.56-1.25-1.25-1.25h-8.5c-.69 0-1.25.56-1.25 1.25v5c0 .69.56 1.25 1.25 1.25zm.75 1.5v1.996h1.5V11.5z") }

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
